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
        Ok(tid) => Ok(Child { thread_id: tid as u32, proc_h: out[0], main_h: out[1] }),
        Err(st) => Err(st),
    }
}

impl Child {
    /// Block until the child exits, then release its handles.
    pub fn wait(self) {
        let mut it = [WaitItem { handle: self.proc_h, want: SIG_EXITED, observed: 0 }];
        let _ = wait_many(&mut it, u64::MAX);
        syscall1(SYS_HANDLE_CLOSE, self.proc_h as u64);
        syscall1(SYS_HANDLE_CLOSE, self.main_h as u64);
    }
}
