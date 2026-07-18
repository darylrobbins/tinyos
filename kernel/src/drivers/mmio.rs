//! MMIO accessors as inline asm with base-register + zero-immediate
//! addressing. QEMU's HVF backend can only emulate MMIO traps whose
//! ESR has ISV=1 — plain single-register loads/stores with immediate
//! offset. Letting the compiler pick addressing modes (e.g. register
//! offset) makes HVF abort with an `isv` assertion.

#[cfg(target_arch = "x86_64")]
mod imp {
    // No trapping hypervisor quirks on x86: plain volatile access is fine.
    pub fn r8(a: usize) -> u8 {
        unsafe { (a as *const u8).read_volatile() }
    }
    pub fn w8(a: usize, v: u8) {
        unsafe { (a as *mut u8).write_volatile(v) }
    }
    pub fn r16(a: usize) -> u16 {
        unsafe { (a as *const u16).read_volatile() }
    }
    pub fn w16(a: usize, v: u16) {
        unsafe { (a as *mut u16).write_volatile(v) }
    }
    pub fn r32(a: usize) -> u32 {
        unsafe { (a as *const u32).read_volatile() }
    }
    pub fn w32(a: usize, v: u32) {
        unsafe { (a as *mut u32).write_volatile(v) }
    }
    pub fn w64(a: usize, v: u64) {
        unsafe { (a as *mut u64).write_volatile(v) }
    }
}

#[cfg(target_arch = "x86_64")]
pub use imp::*;

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

#[cfg(target_arch = "aarch64")]
pub fn r8(addr: usize) -> u8 {
    let v: u32;
    unsafe { asm!("ldrb {v:w}, [{a}]", a = in(reg) addr, v = out(reg) v) };
    v as u8
}

#[cfg(target_arch = "aarch64")]
pub fn w8(addr: usize, val: u8) {
    unsafe { asm!("strb {v:w}, [{a}]", a = in(reg) addr, v = in(reg) val as u32) };
}

#[cfg(target_arch = "aarch64")]
pub fn r16(addr: usize) -> u16 {
    let v: u32;
    unsafe { asm!("ldrh {v:w}, [{a}]", a = in(reg) addr, v = out(reg) v) };
    v as u16
}

#[cfg(target_arch = "aarch64")]
pub fn w16(addr: usize, val: u16) {
    unsafe { asm!("strh {v:w}, [{a}]", a = in(reg) addr, v = in(reg) val as u32) };
}

#[cfg(target_arch = "aarch64")]
pub fn r32(addr: usize) -> u32 {
    let v: u32;
    unsafe { asm!("ldr {v:w}, [{a}]", a = in(reg) addr, v = out(reg) v) };
    v
}

#[cfg(target_arch = "aarch64")]
pub fn w32(addr: usize, val: u32) {
    unsafe { asm!("str {v:w}, [{a}]", a = in(reg) addr, v = in(reg) val) };
}

#[cfg(target_arch = "aarch64")]
pub fn w64(addr: usize, val: u64) {
    unsafe { asm!("str {v}, [{a}]", a = in(reg) addr, v = in(reg) val) };
}
