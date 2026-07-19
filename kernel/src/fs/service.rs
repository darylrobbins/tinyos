//! File-protocol service (abi::fs v0): serves one app's FS channel, pumped
//! each frame by the terminal that spawned it. In-kernel today; clients only
//! see the channel protocol, so this can re-host to a userspace fsd later.
//!
//! Offset I/O is implemented over tinyfs's whole-file read/write (correct,
//! O(file size) per op — a cache slots in behind this protocol later).

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::fs::*;

use crate::obj::channel::{self, ChannelEnd, Message};
use crate::obj::handle::{Handle, RIGHTS_ALL};
use crate::obj::{Object, SIG_PEER_CLOSED};

use tinyfs::FsError;

struct OpenFile {
    path: String,
    writable: bool,
}

pub struct FsService {
    ch: Arc<ChannelEnd>,
    /// Jail root (canonical absolute dir, no trailing '/'; "/" = unconfined).
    /// The app's whole namespace is re-rooted here: its "/" is this dir, and
    /// `..` clamps at it. The jail is the security boundary; `base` is not.
    jail: String,
    /// Cwd *within the jail* for relative paths (spawner's cwd at spawn time).
    base: String,
    fds: Vec<Option<OpenFile>>,
    /// Subtree services handed out via OP_OPEN_DIR, pumped with the parent
    /// and dropped once their peer closes. Dropping this service drops them
    /// all — hierarchical revocation.
    children: Vec<FsService>,
}

fn status(e: FsError) -> u32 {
    match e {
        FsError::NotFound => FS_NOT_FOUND,
        FsError::IsADir => FS_IS_DIR,
        FsError::NotADir => FS_NOT_DIR,
        FsError::Exists => FS_EXISTS,
        FsError::NoSpace | FsError::NoInodes | FsError::FileTooBig => FS_NO_SPACE,
        FsError::InvalidPath | FsError::NameTooLong => FS_INVALID,
        _ => FS_IO,
    }
}

fn le_u32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

fn le_u64(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|c| u64::from_le_bytes(c.try_into().unwrap()))
}

fn utf8(b: &[u8]) -> Option<&str> {
    core::str::from_utf8(b).ok()
}

/// Map an app-visible path into a real filesystem path: canonicalize against
/// the in-jail cwd (absolute paths and `..` clamp at the app's "/"), then
/// re-root at the jail. Every FsService op funnels through this.
pub fn jailed_path(jail: &str, base: &str, path: &str) -> Result<String, u32> {
    let inner = tinyfs::path::canonical(base, path).map_err(status)?;
    Ok(if jail == "/" {
        inner
    } else if inner == "/" {
        jail.to_string()
    } else {
        let mut s = String::from(jail);
        s.push_str(&inner);
        s
    })
}

impl FsService {
    pub fn new(ch: Arc<ChannelEnd>, jail: String, base: String) -> Self {
        Self { ch, jail, base, fds: Vec::new(), children: Vec::new() }
    }

    fn resolve(&self, path: &str) -> Result<String, u32> {
        jailed_path(&self.jail, &self.base, path)
    }

    /// Drain and answer every queued request, then pump subtree children
    /// (reaping any whose peer closed).
    pub fn pump(&mut self) {
        while let Ok(msg) = self.ch.recv() {
            let reply = self.handle(&msg.bytes);
            let _ = self.ch.send(reply);
        }
        self.children.retain_mut(|c| {
            c.pump();
            c.ch.signals() & SIG_PEER_CLOSED == 0
        });
    }

    fn handle(&mut self, b: &[u8]) -> Message {
        // OP_OPEN_DIR is the one reply that carries a handle.
        if le_u32(b, 0) == Some(OP_OPEN_DIR) {
            return self.op_open_dir(b);
        }
        Message { bytes: self.handle_bytes(b), handles: Vec::new() }
    }

    /// Mint a subtree capability: a fresh FS channel whose service is jailed
    /// to the (validated) directory. The child can only ever narrow — its
    /// jail is a subdir of ours, and its own OP_OPEN_DIR narrows again.
    fn op_open_dir(&mut self, b: &[u8]) -> Message {
        let plain = |st: u32| Message { bytes: reply1(R_OPEN_DIR, st), handles: Vec::new() };
        let Some(path) = b.get(4..).and_then(utf8) else {
            return plain(FS_INVALID);
        };
        let jail = match self.resolve(path) {
            Ok(p) => p,
            Err(st) => return plain(st),
        };
        if self.children.len() >= MAX_SUBDIRS {
            return plain(FS_LIMIT);
        }
        if let Err(e) = crate::fs::resolve_dir("/", &jail) {
            return plain(status(e));
        }
        let (app_end, kern_end) = channel::create();
        self.children
            .push(FsService::new(kern_end, jail, String::from("/")));
        Message {
            bytes: reply1(R_OPEN_DIR, FS_OK),
            handles: alloc::vec![Handle::new(Object::Channel(app_end), RIGHTS_ALL)],
        }
    }

    fn handle_bytes(&mut self, b: &[u8]) -> Vec<u8> {
        let Some(op) = le_u32(b, 0) else {
            return r_status(FS_INVALID);
        };
        match op {
            OP_OPEN => self.op_open(b),
            OP_CLOSE => self.op_close(b),
            OP_READ => self.op_read(b),
            OP_WRITE => self.op_write(b),
            OP_STAT => self.op_stat(b),
            OP_READDIR => self.op_readdir(b),
            OP_MKDIR => self.path_op(b, 4, |p| {
                crate::fs::mkdir("/", p).map_err(status)
            }),
            OP_REMOVE => {
                let Some(recursive) = le_u32(b, 4) else {
                    return r_status(FS_INVALID);
                };
                self.path_op(b, 8, |p| {
                    crate::fs::remove("/", p, recursive != 0).map_err(status)
                })
            }
            OP_RENAME => {
                let Some(flen) = le_u32(b, 4).map(|v| v as usize) else {
                    return r_status(FS_INVALID);
                };
                let (Some(from), Some(to)) = (
                    b.get(8..8 + flen).and_then(utf8),
                    b.get(8 + flen..).and_then(utf8),
                ) else {
                    return r_status(FS_INVALID);
                };
                let (from, to) = match (self.resolve(from), self.resolve(to)) {
                    (Ok(f), Ok(t)) => (f, t),
                    (Err(st), _) | (_, Err(st)) => return r_status(st),
                };
                match crate::fs::rename("/", &from, &to) {
                    Ok(()) => r_status(FS_OK),
                    Err(e) => r_status(status(e)),
                }
            }
            _ => r_status(FS_INVALID),
        }
    }

    fn path_op(
        &mut self,
        b: &[u8],
        off: usize,
        f: impl Fn(&str) -> Result<(), u32>,
    ) -> Vec<u8> {
        let Some(path) = b.get(off..).and_then(utf8) else {
            return r_status(FS_INVALID);
        };
        match self.resolve(path).and_then(|p| f(&p)) {
            Ok(()) => r_status(FS_OK),
            Err(st) => r_status(st),
        }
    }

    fn op_open(&mut self, b: &[u8]) -> Vec<u8> {
        let (Some(flags), Some(path)) = (le_u32(b, 4), b.get(8..).and_then(utf8)) else {
            return r_status(FS_INVALID);
        };
        // OpenFile stores the jailed-absolute path: fd ops never re-resolve.
        let path = match self.resolve(path) {
            Ok(p) => p,
            Err(st) => return reply2(R_OPEN, st, 0),
        };
        let exists = match crate::fs::read("/", &path) {
            Ok(_) => true,
            Err(FsError::IsADir) => return reply2(R_OPEN, FS_IS_DIR, 0),
            Err(FsError::NotFound) => false,
            Err(e) => return reply2(R_OPEN, status(e), 0),
        };
        if !exists && flags & O_CREATE == 0 {
            return reply2(R_OPEN, FS_NOT_FOUND, 0);
        }
        if (!exists || flags & O_TRUNC != 0) && flags & (O_WRITE | O_CREATE) != 0 {
            if let Err(e) = crate::fs::write("/", &path, &[], false) {
                return reply2(R_OPEN, status(e), 0);
            }
        }
        if self.fds.iter().filter(|f| f.is_some()).count() >= MAX_FDS {
            return reply2(R_OPEN, FS_LIMIT, 0);
        }
        let file = OpenFile { path, writable: flags & O_WRITE != 0 };
        let fd = match self.fds.iter().position(Option::is_none) {
            Some(i) => {
                self.fds[i] = Some(file);
                i
            }
            None => {
                self.fds.push(Some(file));
                self.fds.len() - 1
            }
        };
        reply2(R_OPEN, FS_OK, fd as u32)
    }

    fn op_close(&mut self, b: &[u8]) -> Vec<u8> {
        match le_u32(b, 4).map(|fd| fd as usize) {
            Some(fd) if fd < self.fds.len() && self.fds[fd].is_some() => {
                self.fds[fd] = None;
                r_status(FS_OK)
            }
            _ => r_status(FS_BAD_FD),
        }
    }

    fn file(&self, b: &[u8]) -> Result<&OpenFile, u32> {
        let fd = le_u32(b, 4).ok_or(FS_INVALID)? as usize;
        self.fds
            .get(fd)
            .and_then(Option::as_ref)
            .ok_or(FS_BAD_FD)
    }

    fn op_read(&mut self, b: &[u8]) -> Vec<u8> {
        let (path, offset, len) = match (self.file(b), le_u64(b, 8), le_u32(b, 16)) {
            (Ok(f), Some(o), Some(l)) => (f.path.clone(), o, l.min(MAX_IO)),
            (Err(st), ..) => return reply1(R_READ, st),
            _ => return reply1(R_READ, FS_INVALID),
        };
        match crate::fs::read("/", &path) {
            Ok(data) => {
                let start = (offset as usize).min(data.len());
                let end = (start + len as usize).min(data.len());
                let mut r = reply1(R_READ, FS_OK);
                r.extend_from_slice(&data[start..end]);
                r
            }
            Err(e) => reply1(R_READ, status(e)),
        }
    }

    fn op_write(&mut self, b: &[u8]) -> Vec<u8> {
        let (path, writable, offset) = match (self.file(b), le_u64(b, 8)) {
            (Ok(f), Some(o)) => (f.path.clone(), f.writable, o),
            (Err(st), _) => return r_status(st),
            _ => return r_status(FS_INVALID),
        };
        if !writable {
            return r_status(FS_DENIED);
        }
        let Some(data) = b.get(16..) else {
            return r_status(FS_INVALID);
        };
        if data.len() > MAX_IO as usize {
            return r_status(FS_LIMIT);
        }
        let res = if offset == OFFSET_APPEND {
            crate::fs::write("/", &path, data, true)
        } else {
            // Read-modify-write: correct, O(file); a cache changes this
            // behind the same protocol later.
            let mut cur = match crate::fs::read("/", &path) {
                Ok(c) => c,
                Err(e) => return r_status(status(e)),
            };
            let end = offset as usize + data.len();
            if cur.len() < end {
                cur.resize(end, 0);
            }
            cur[offset as usize..end].copy_from_slice(data);
            crate::fs::write("/", &path, &cur, false)
        };
        match res {
            Ok(()) => r_status(FS_OK),
            Err(e) => r_status(status(e)),
        }
    }

    fn op_stat(&mut self, b: &[u8]) -> Vec<u8> {
        let Some(path) = b.get(4..).and_then(utf8) else {
            return r_stat(FS_INVALID, 0, 0);
        };
        let path = match self.resolve(path) {
            Ok(p) => p,
            Err(st) => return r_stat(st, 0, 0),
        };
        // A dir resolves via resolve_dir; a file reads (whole-file — fine
        // at hobby scale, revisit with a real stat when tinyfs grows one).
        if crate::fs::resolve_dir("/", &path).is_ok() {
            return r_stat(FS_OK, KIND_DIR, 0);
        }
        match crate::fs::read("/", &path) {
            Ok(data) => r_stat(FS_OK, KIND_FILE, data.len() as u64),
            Err(e) => r_stat(status(e), 0, 0),
        }
    }

    fn op_readdir(&mut self, b: &[u8]) -> Vec<u8> {
        let Some(path) = b.get(4..).and_then(utf8) else {
            return r_status(FS_INVALID);
        };
        let path = match self.resolve(path) {
            Ok(p) => p,
            Err(st) => return r_status(st),
        };
        match crate::fs::list("/", &path) {
            Ok(entries) => {
                let mut r = reply1(R_DIR, FS_OK);
                r.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                for e in entries {
                    let kind = match e.kind {
                        tinyfs::InodeKind::Dir => KIND_DIR,
                        _ => KIND_FILE,
                    };
                    r.extend_from_slice(&kind.to_le_bytes());
                    r.extend_from_slice(&e.size.to_le_bytes());
                    r.extend_from_slice(&(e.name.len() as u32).to_le_bytes());
                    r.extend_from_slice(e.name.as_bytes());
                }
                r
            }
            Err(e) => {
                let mut r = reply1(R_DIR, status(e));
                r.extend_from_slice(&0u32.to_le_bytes());
                r
            }
        }
    }
}

fn r_status(st: u32) -> Vec<u8> {
    reply1(R_STATUS, st)
}

fn reply1(op: u32, st: u32) -> Vec<u8> {
    let mut v = op.to_le_bytes().to_vec();
    v.extend_from_slice(&st.to_le_bytes());
    v
}

fn reply2(op: u32, st: u32, val: u32) -> Vec<u8> {
    let mut v = reply1(op, st);
    v.extend_from_slice(&val.to_le_bytes());
    v
}

fn r_stat(st: u32, kind: u32, size: u64) -> Vec<u8> {
    let mut v = reply1(R_STAT, st);
    v.extend_from_slice(&kind.to_le_bytes());
    v.extend_from_slice(&size.to_le_bytes());
    v
}
