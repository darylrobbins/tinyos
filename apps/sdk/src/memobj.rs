//! Thin wrappers over the memory-object syscalls, for mapping a MemObj a
//! process received over a channel (e.g. a hosted app's cell surface) — not
//! just self-created ones. size/map/unmap already exist in the kernel.

use crate::syscall::{syscall1, syscall3, SYS_MEMOBJ_MAP, SYS_MEMOBJ_SIZE, SYS_MEMOBJ_UNMAP};

/// Byte size of the MemObj referenced by `handle`.
pub fn size(handle: u32) -> Result<u64, u32> {
    syscall1(SYS_MEMOBJ_SIZE, handle as u64).ok()
}

/// Map `len` bytes of the MemObj at `offset` into this process; returns the VA.
pub fn map(handle: u32, offset: u64, len: u64) -> Result<u64, u32> {
    syscall3(SYS_MEMOBJ_MAP, handle as u64, offset, len).ok()
}

/// Unmap a mapping previously returned by `map`.
pub fn unmap(va: u64) {
    let _ = syscall1(SYS_MEMOBJ_UNMAP, va);
}
