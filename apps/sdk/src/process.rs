//! Spawn processes from userspace (SYS_PROCESS_SPAWN): stage an ELF into a
//! MemObj, build the argv record and grant list, and hand the kernel the
//! capabilities the child should be born with.

use alloc::vec::Vec;

use crate::syscall::*;
use crate::wait::{wait_many, WaitItem};

pub struct Child {
    pub thread_id: u32,
    /// Process handle: wait on SIG_EXITED; close when done.
    pub proc_h: u32,
    /// Parent end of the child's main channel; close when done.
    pub main_h: u32,
    /// This child is a pure-window app (declares `window`, not `console`) that
    /// the kernel gave its own window (exec only). A shell auto-backgrounds
    /// these — they don't touch the launching console, so blocking the prompt on
    /// them would strand a GUI window. Console-surface apps (top, vi) are NOT
    /// detached and stay foreground.
    pub detached: bool,
}

/// Spawn `elf` with `args` and explicit `grants` (tag, handle) — granted
/// handles MOVE to the child. Note: the kernel names the process after
/// args[0] (an ABI-v0 wart; a name field is a future record extension).
pub fn spawn(elf: &[u8], args: &[&str], grants: &[(u32, u32)]) -> Result<Child, u32> {
    // Stage the image in a memobj the kernel snapshots at spawn.
    let size = (elf.len() as u64 + 0xFFF) & !0xFFF;
    let mem = syscall1(SYS_MEMOBJ_CREATE, size).ok()?;
    let va = syscall3(SYS_MEMOBJ_MAP, mem, 0, size).ok()?;
    unsafe {
        core::ptr::copy_nonoverlapping(elf.as_ptr(), va as *mut u8, elf.len());
    }

    // argv record: u32 argc, then u32 len + utf8 per arg.
    let mut rec = (args.len() as u32).to_le_bytes().to_vec();
    for a in args {
        rec.extend_from_slice(&(a.len() as u32).to_le_bytes());
        rec.extend_from_slice(a.as_bytes());
    }
    // Grant array: (tag u32, handle u32) pairs.
    let mut gr: Vec<u8> = Vec::with_capacity(grants.len() * 8);
    for (tag, h) in grants {
        gr.extend_from_slice(&tag.to_le_bytes());
        gr.extend_from_slice(&h.to_le_bytes());
    }
    let mut out = [0u32; 2];
    let r = syscall6(
        SYS_PROCESS_SPAWN,
        mem,
        rec.as_ptr() as u64,
        rec.len() as u64,
        gr.as_ptr() as u64,
        grants.len() as u64,
        out.as_mut_ptr() as u64,
    );
    // The kernel snapshotted the image; drop our staging mapping either way.
    syscall1(SYS_MEMOBJ_UNMAP, va);
    syscall1(SYS_HANDLE_CLOSE, mem);
    match r.ok() {
        // spawn never mints a window (raw ELF, unattested identity).
        Ok(tid) => Ok(Child { thread_id: tid as u32, proc_h: out[0], main_h: out[1], detached: false }),
        Err(st) => Err(st),
    }
}

/// Exec a `/system/apps/…`-style path. The KERNEL loads it (attesting identity from
/// the path) and delegates `grants` unchanged. `want_window` asks the kernel to
/// mint a window under the attested identity (honored iff the app declares
/// `window`). The child sees `args` as its argv (the path is kernel-only).
pub fn exec(path: &str, args: &[&str], grants: &[(u32, u32)], want_window: bool) -> Result<Child, u32> {
    // argv record: argv[0] = path (kernel reads it), then the child's args.
    let argc = 1 + args.len();
    let mut rec = (argc as u32).to_le_bytes().to_vec();
    rec.extend_from_slice(&(path.len() as u32).to_le_bytes());
    rec.extend_from_slice(path.as_bytes());
    for a in args {
        rec.extend_from_slice(&(a.len() as u32).to_le_bytes());
        rec.extend_from_slice(a.as_bytes());
    }
    let mut gr: Vec<u8> = Vec::with_capacity(grants.len() * 8);
    for (tag, h) in grants {
        gr.extend_from_slice(&tag.to_le_bytes());
        gr.extend_from_slice(&h.to_le_bytes());
    }
    let flags = if want_window { EXEC_REQUEST_WINDOW } else { 0 };
    // out = [proc_h, main_h, windowed]; the kernel writes all three.
    let mut out = [0u32; 3];
    let r = syscall6(
        SYS_PROCESS_EXEC,
        rec.as_ptr() as u64,
        rec.len() as u64,
        gr.as_ptr() as u64,
        grants.len() as u64,
        out.as_mut_ptr() as u64,
        flags,
    );
    match r.ok() {
        Ok(tid) => Ok(Child {
            thread_id: tid as u32,
            proc_h: out[0],
            main_h: out[1],
            detached: out[2] != 0,
        }),
        Err(st) => Err(st),
    }
}

impl Child {
    /// Block until the child exits, then release its handles.
    pub fn wait(self) {
        let mut it = [WaitItem { handle: self.proc_h, want: SIG_EXITED, observed: 0 }];
        let _ = wait_many(&mut it, u64::MAX);
        self.release();
    }

    /// Non-blocking: true if the child has exited (for background jobs).
    pub fn exited(&self) -> bool {
        let mut it = [WaitItem { handle: self.proc_h, want: SIG_EXITED, observed: 0 }];
        // Deadline 0 = poll: wait_many checks signals then times out.
        let _ = wait_many(&mut it, 0);
        it[0].observed & SIG_EXITED != 0
    }

    /// Release the process and main-channel handles.
    pub fn release(&self) {
        syscall1(SYS_HANDLE_CLOSE, self.proc_h as u64);
        syscall1(SYS_HANDLE_CLOSE, self.main_h as u64);
    }
}
