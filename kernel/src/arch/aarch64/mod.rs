pub mod exceptions;

use core::arch::asm;
use core::fmt;

/// PL011 UART on the QEMU virt board. Works both before and after
/// exit_boot_services since we poke the MMIO registers directly.
pub struct Pl011 {
    base: usize,
}

impl Pl011 {
    const DR: usize = 0x00;
    const FR: usize = 0x18;
    const FR_TXFF: u32 = 1 << 5;

    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    fn write_byte(&mut self, b: u8) {
        unsafe {
            let fr = (self.base + Self::FR) as *const u32;
            while core::ptr::read_volatile(fr) & Self::FR_TXFF != 0 {}
            core::ptr::write_volatile((self.base + Self::DR) as *mut u32, b as u32);
        }
    }
}

unsafe impl Send for Pl011 {}

impl fmt::Write for Pl011 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
        Ok(())
    }
}

pub fn current_el() -> u64 {
    let el: u64;
    unsafe { asm!("mrs {0}, CurrentEL", out(reg) el) };
    el >> 2
}

pub fn park() -> ! {
    loop {
        unsafe { asm!("wfe") };
    }
}
