//! File client (abi::fs v0): blocking request/response over the FS channel
//! (bootstrap grant TAG_FS). One outstanding request at a time; every call
//! blocks until the service replies.

use alloc::string::String;
use alloc::vec::Vec;

use abi::fs::*;

use crate::channel::{Channel, Msg};

pub struct FsClient {
    ch: Channel,
}

static mut FS: Option<FsClient> = None;

pub(crate) fn set_client(ch: Channel) {
    unsafe { FS = Some(FsClient { ch }) };
}

fn client() -> Option<&'static mut FsClient> {
    // Single-threaded app; set before main runs.
    unsafe { (&mut *core::ptr::addr_of_mut!(FS)).as_mut() }
}

fn le_u32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

impl FsClient {
    /// Send one request and block for its reply, which must carry `expect`.
    fn rpc(&self, req: &[u8], expect: u32) -> Result<Msg, u32> {
        rpc(self.ch, req, expect)
    }

    fn status_of(m: &Msg) -> u32 {
        le_u32(&m.bytes, 4).unwrap_or(FS_IO)
    }

    fn simple(&self, req: &[u8]) -> Result<(), u32> {
        simple(self.ch, req)
    }
}

fn rpc(ch: Channel, req: &[u8], expect: u32) -> Result<Msg, u32> {
    ch.send(req, &[]).map_err(|_| FS_IO)?;
    let m = ch.recv().map_err(|_| FS_IO)?;
    match le_u32(&m.bytes, 0) {
        Some(op) if op == expect => Ok(m),
        _ => Err(FS_IO),
    }
}

fn simple(ch: Channel, req: &[u8]) -> Result<(), u32> {
    let m = rpc(ch, req, R_STATUS)?;
    match FsClient::status_of(&m) {
        FS_OK => Ok(()),
        st => Err(st),
    }
}

/// A capability to a directory subtree: its own FS channel, confined by the
/// service to the opened directory. Obtained via `open_dir` (from the app's
/// root grant) or `Dir::open_dir` (narrowing further). The raw channel
/// handle (`Dir::handle`) can ride a `process_spawn` grant to delegate the
/// subtree to a child process. Dropping the Dir closes the channel, which
/// revokes nothing for the parent but ends this capability.
pub struct Dir {
    ch: Channel,
}

impl Dir {
    /// The channel handle (e.g. to pass as a TAG_FS spawn grant).
    pub fn handle(&self) -> u32 {
        self.ch.0
    }

    /// Narrow further: a Dir confined to `path` within this Dir.
    pub fn open_dir(&self, path: &str) -> Result<Dir, u32> {
        open_dir_on(self.ch, path)
    }

    /// Read a whole file under this Dir.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, u32> {
        read_on(self.ch, path)
    }

    /// Create/replace a whole file under this Dir.
    pub fn write(&self, path: &str, data: &[u8]) -> Result<(), u32> {
        write_on(self.ch, path, data)
    }

    /// Directory entries as (name, kind, size).
    pub fn list(&self, path: &str) -> Result<Vec<(String, u32, u64)>, u32> {
        list_on(self.ch, path)
    }

    pub fn mkdir(&self, path: &str) -> Result<(), u32> {
        let mut r = req(OP_MKDIR);
        r.extend_from_slice(path.as_bytes());
        simple(self.ch, &r)
    }

    pub fn remove(&self, path: &str, recursive: bool) -> Result<(), u32> {
        let mut r = req(OP_REMOVE);
        r.extend_from_slice(&(recursive as u32).to_le_bytes());
        r.extend_from_slice(path.as_bytes());
        simple(self.ch, &r)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        crate::syscall::syscall1(crate::syscall::SYS_HANDLE_CLOSE, self.ch.0 as u64);
    }
}

/// Open a subtree capability rooted at `path` (on the app's root FS grant).
pub fn open_dir(path: &str) -> Result<Dir, u32> {
    let c = client().ok_or(FS_IO)?;
    open_dir_on(c.ch, path)
}

fn open_dir_on(ch: Channel, path: &str) -> Result<Dir, u32> {
    let mut r = req(OP_OPEN_DIR);
    r.extend_from_slice(path.as_bytes());
    let m = rpc(ch, &r, R_OPEN_DIR)?;
    match FsClient::status_of(&m) {
        FS_OK => {
            let h = *m.handles.first().ok_or(FS_IO)?;
            Ok(Dir { ch: Channel(h) })
        }
        st => Err(st),
    }
}

fn open_on(ch: Channel, path: &str, flags: u32) -> Result<u32, u32> {
    let mut r = req(OP_OPEN);
    r.extend_from_slice(&flags.to_le_bytes());
    r.extend_from_slice(path.as_bytes());
    let m = rpc(ch, &r, R_OPEN)?;
    match FsClient::status_of(&m) {
        FS_OK => le_u32(&m.bytes, 8).ok_or(FS_IO),
        st => Err(st),
    }
}

fn close_on(ch: Channel, fd: u32) {
    let mut r = req(OP_CLOSE);
    r.extend_from_slice(&fd.to_le_bytes());
    let _ = simple(ch, &r);
}

fn read_on(ch: Channel, path: &str) -> Result<Vec<u8>, u32> {
    let fd = open_on(ch, path, O_READ)?;
    let mut out = Vec::new();
    loop {
        let mut r = req(OP_READ);
        r.extend_from_slice(&fd.to_le_bytes());
        r.extend_from_slice(&(out.len() as u64).to_le_bytes());
        r.extend_from_slice(&MAX_IO.to_le_bytes());
        let m = match rpc(ch, &r, R_READ) {
            Ok(m) => m,
            Err(st) => {
                close_on(ch, fd);
                return Err(st);
            }
        };
        match FsClient::status_of(&m) {
            FS_OK if m.bytes.len() > 8 => out.extend_from_slice(&m.bytes[8..]),
            FS_OK => {
                close_on(ch, fd);
                return Ok(out);
            }
            st => {
                close_on(ch, fd);
                return Err(st);
            }
        }
    }
}

fn write_on(ch: Channel, path: &str, data: &[u8]) -> Result<(), u32> {
    let fd = open_on(ch, path, O_WRITE | O_CREATE | O_TRUNC)?;
    let mut off = 0usize;
    loop {
        let end = (off + MAX_IO as usize).min(data.len());
        if off >= data.len() {
            break;
        }
        let mut r = req(OP_WRITE);
        r.extend_from_slice(&fd.to_le_bytes());
        r.extend_from_slice(&(off as u64).to_le_bytes());
        r.extend_from_slice(&data[off..end]);
        if let Err(st) = simple(ch, &r) {
            close_on(ch, fd);
            return Err(st);
        }
        off = end;
    }
    close_on(ch, fd);
    Ok(())
}

fn list_on(ch: Channel, path: &str) -> Result<Vec<(String, u32, u64)>, u32> {
    let mut r = req(OP_READDIR);
    r.extend_from_slice(path.as_bytes());
    let m = rpc(ch, &r, R_DIR)?;
    if FsClient::status_of(&m) != FS_OK {
        return Err(FsClient::status_of(&m));
    }
    parse_dir(&m.bytes)
}

fn parse_dir(b: &[u8]) -> Result<Vec<(String, u32, u64)>, u32> {
    let count = le_u32(b, 8).ok_or(FS_IO)? as usize;
    let mut out = Vec::with_capacity(count);
    let mut o = 12usize;
    for _ in 0..count {
        let kind = le_u32(b, o).ok_or(FS_IO)?;
        let size = u64::from_le_bytes(b.get(o + 4..o + 12).ok_or(FS_IO)?.try_into().unwrap());
        let nlen = le_u32(b, o + 12).ok_or(FS_IO)? as usize;
        let name = b.get(o + 16..o + 16 + nlen).ok_or(FS_IO)?;
        out.push((String::from_utf8_lossy(name).into_owned(), kind, size));
        o += 16 + nlen;
    }
    Ok(out)
}

fn req(op: u32) -> Vec<u8> {
    op.to_le_bytes().to_vec()
}

/// An open file (protocol-level fd). Closed on drop.
pub struct File {
    fd: u32,
}

impl File {
    pub fn open(path: &str, flags: u32) -> Result<File, u32> {
        let c = client().ok_or(FS_IO)?;
        let mut r = req(OP_OPEN);
        r.extend_from_slice(&flags.to_le_bytes());
        r.extend_from_slice(path.as_bytes());
        let m = c.rpc(&r, R_OPEN)?;
        match FsClient::status_of(&m) {
            FS_OK => Ok(File { fd: le_u32(&m.bytes, 8).ok_or(FS_IO)? }),
            st => Err(st),
        }
    }

    /// Read up to MAX_IO bytes at `offset`; short (possibly empty) at EOF.
    pub fn read_at(&self, offset: u64, len: u32) -> Result<Vec<u8>, u32> {
        let c = client().ok_or(FS_IO)?;
        let mut r = req(OP_READ);
        r.extend_from_slice(&self.fd.to_le_bytes());
        r.extend_from_slice(&offset.to_le_bytes());
        r.extend_from_slice(&len.min(MAX_IO).to_le_bytes());
        let m = c.rpc(&r, R_READ)?;
        match FsClient::status_of(&m) {
            FS_OK => Ok(m.bytes[8..].to_vec()),
            st => Err(st),
        }
    }

    /// Write at `offset` (OFFSET_APPEND = append). Max MAX_IO bytes.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), u32> {
        let c = client().ok_or(FS_IO)?;
        let mut r = req(OP_WRITE);
        r.extend_from_slice(&self.fd.to_le_bytes());
        r.extend_from_slice(&offset.to_le_bytes());
        r.extend_from_slice(data);
        c.simple(&r)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if let Some(c) = client() {
            let mut r = req(OP_CLOSE);
            r.extend_from_slice(&self.fd.to_le_bytes());
            let _ = c.simple(&r);
        }
    }
}

/// Read a whole file.
pub fn read(path: &str) -> Result<Vec<u8>, u32> {
    let f = File::open(path, O_READ)?;
    let mut out = Vec::new();
    loop {
        let chunk = f.read_at(out.len() as u64, MAX_IO)?;
        if chunk.is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(&chunk);
    }
}

/// Create/replace a whole file.
pub fn write(path: &str, data: &[u8]) -> Result<(), u32> {
    let f = File::open(path, O_WRITE | O_CREATE | O_TRUNC)?;
    let mut off = 0usize;
    while off < data.len() {
        let end = (off + MAX_IO as usize).min(data.len());
        f.write_at(off as u64, &data[off..end])?;
        off = end;
    }
    Ok(())
}

/// (kind, size) for a path; kind is KIND_FILE or KIND_DIR.
pub fn stat(path: &str) -> Result<(u32, u64), u32> {
    let c = client().ok_or(FS_IO)?;
    let mut r = req(OP_STAT);
    r.extend_from_slice(path.as_bytes());
    let m = c.rpc(&r, R_STAT)?;
    match FsClient::status_of(&m) {
        FS_OK => Ok((
            le_u32(&m.bytes, 8).ok_or(FS_IO)?,
            m.bytes
                .get(12..20)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                .ok_or(FS_IO)?,
        )),
        st => Err(st),
    }
}

/// Directory entries as (name, kind, size).
pub fn list(path: &str) -> Result<Vec<(String, u32, u64)>, u32> {
    let c = client().ok_or(FS_IO)?;
    let mut r = req(OP_READDIR);
    r.extend_from_slice(path.as_bytes());
    let m = c.rpc(&r, R_DIR)?;
    if FsClient::status_of(&m) != FS_OK {
        return Err(FsClient::status_of(&m));
    }
    let b = &m.bytes;
    let count = le_u32(b, 8).ok_or(FS_IO)? as usize;
    let mut out = Vec::with_capacity(count);
    let mut o = 12usize;
    for _ in 0..count {
        let kind = le_u32(b, o).ok_or(FS_IO)?;
        let size = u64::from_le_bytes(b.get(o + 4..o + 12).ok_or(FS_IO)?.try_into().unwrap());
        let nlen = le_u32(b, o + 12).ok_or(FS_IO)? as usize;
        let name = b.get(o + 16..o + 16 + nlen).ok_or(FS_IO)?;
        out.push((String::from_utf8_lossy(name).into_owned(), kind, size));
        o += 16 + nlen;
    }
    Ok(out)
}

pub fn mkdir(path: &str) -> Result<(), u32> {
    let c = client().ok_or(FS_IO)?;
    let mut r = req(OP_MKDIR);
    r.extend_from_slice(path.as_bytes());
    c.simple(&r)
}

pub fn remove(path: &str, recursive: bool) -> Result<(), u32> {
    let c = client().ok_or(FS_IO)?;
    let mut r = req(OP_REMOVE);
    r.extend_from_slice(&(recursive as u32).to_le_bytes());
    r.extend_from_slice(path.as_bytes());
    c.simple(&r)
}

pub fn rename(from: &str, to: &str) -> Result<(), u32> {
    let c = client().ok_or(FS_IO)?;
    let mut r = req(OP_RENAME);
    r.extend_from_slice(&(from.len() as u32).to_le_bytes());
    r.extend_from_slice(from.as_bytes());
    r.extend_from_slice(to.as_bytes());
    c.simple(&r)
}
