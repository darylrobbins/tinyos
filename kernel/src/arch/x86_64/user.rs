//! Ring-3 stub: user threads cannot exist on x86_64 yet (AddrSpace::new()
//! returns None), so these are never reached at runtime.

pub fn activate(_ttbr1: u64) {}

/// # Safety
/// Never called on this arch.
pub unsafe fn enter_user(_pc: u64, _sp: u64, _arg: u64) -> ! {
    unreachable!("userspace not supported on x86_64 yet")
}
