//! User-memory accessor stubs. No ring-3 exists on x86_64 yet
//! (`AddrSpace::new()` returns None and `user_buf_ok` is always false), so
//! these can never be reached with a real user VA; plain copies keep the
//! arch-neutral syscall code compiling.

pub unsafe fn copy_from_user(dst: *mut u8, src: u64, len: usize) {
    unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst, len) }
}

pub unsafe fn copy_to_user(dst: u64, src: *const u8, len: usize) {
    unsafe { core::ptr::copy_nonoverlapping(src, dst as *mut u8, len) }
}
