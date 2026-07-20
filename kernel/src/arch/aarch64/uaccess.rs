//! Unprivileged user-memory accessors.
//!
//! With PAN enabled (see `paging::init_cpu`), a plain EL1 load/store to an
//! EL0-accessible page faults; `ldtr`/`sttr` perform the access with EL0
//! privileges instead, so these are the only legitimate way for kernel code
//! to touch user memory. On CPUs without FEAT_PAN the instructions behave
//! the same way, so callers never need to care which mode the CPU is in.
//!
//! Callers must validate the range against the address space first
//! (`AddrSpace::user_buf_ok`) and hold the aspace lock across validate+copy;
//! an unvalidated access to an unmapped VA is a kernel data abort.

use core::arch::asm;

/// Copy `len` bytes from user VA `src` into kernel memory at `dst`.
///
/// # Safety
/// `[src, src+len)` must be validated as mapped readable in the current
/// TTBR1 space, and `dst` must be a valid kernel buffer of `len` bytes.
pub unsafe fn copy_from_user(mut dst: *mut u8, mut src: u64, mut len: usize) {
    unsafe {
        while len >= 8 {
            let v: u64;
            asm!("ldtr {v}, [{a}]", v = out(reg) v, a = in(reg) src, options(nostack));
            (dst as *mut u64).write_unaligned(v);
            src += 8;
            dst = dst.add(8);
            len -= 8;
        }
        while len > 0 {
            let v: u64;
            asm!("ldtrb {v:w}, [{a}]", v = out(reg) v, a = in(reg) src, options(nostack));
            dst.write(v as u8);
            src += 1;
            dst = dst.add(1);
            len -= 1;
        }
    }
}

/// Copy `len` bytes from kernel memory at `src` to user VA `dst`.
///
/// # Safety
/// `[dst, dst+len)` must be validated as mapped writable in the current
/// TTBR1 space, and `src` must be a valid kernel buffer of `len` bytes.
pub unsafe fn copy_to_user(mut dst: u64, mut src: *const u8, mut len: usize) {
    unsafe {
        while len >= 8 {
            let v = (src as *const u64).read_unaligned();
            asm!("sttr {v}, [{a}]", v = in(reg) v, a = in(reg) dst, options(nostack));
            dst += 8;
            src = src.add(8);
            len -= 8;
        }
        while len > 0 {
            let v = src.read();
            asm!("sttrb {v:w}, [{a}]", v = in(reg) v, a = in(reg) dst, options(nostack));
            dst += 1;
            src = src.add(1);
            len -= 1;
        }
    }
}
