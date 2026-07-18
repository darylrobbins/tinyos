//! MMIO accessors as inline asm with base-register + zero-immediate
//! addressing. QEMU's HVF backend can only emulate MMIO traps whose
//! ESR has ISV=1 — plain single-register loads/stores with immediate
//! offset. Letting the compiler pick addressing modes (e.g. register
//! offset) makes HVF abort with an `isv` assertion.

use core::arch::asm;

pub fn r8(addr: usize) -> u8 {
    let v: u32;
    unsafe { asm!("ldrb {v:w}, [{a}]", a = in(reg) addr, v = out(reg) v) };
    v as u8
}

pub fn w8(addr: usize, val: u8) {
    unsafe { asm!("strb {v:w}, [{a}]", a = in(reg) addr, v = in(reg) val as u32) };
}

pub fn r16(addr: usize) -> u16 {
    let v: u32;
    unsafe { asm!("ldrh {v:w}, [{a}]", a = in(reg) addr, v = out(reg) v) };
    v as u16
}

pub fn w16(addr: usize, val: u16) {
    unsafe { asm!("strh {v:w}, [{a}]", a = in(reg) addr, v = in(reg) val as u32) };
}

pub fn r32(addr: usize) -> u32 {
    let v: u32;
    unsafe { asm!("ldr {v:w}, [{a}]", a = in(reg) addr, v = out(reg) v) };
    v
}

pub fn w32(addr: usize, val: u32) {
    unsafe { asm!("str {v:w}, [{a}]", a = in(reg) addr, v = in(reg) val) };
}

pub fn w64(addr: usize, val: u64) {
    unsafe { asm!("str {v}, [{a}]", a = in(reg) addr, v = in(reg) val) };
}
