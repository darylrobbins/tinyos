pub mod exceptions;
pub mod timer;

use core::arch::asm;
use core::fmt;

pub const NAME: &str = "aarch64";
pub const MACHINE: &str = "QEMU virt, UEFI boot";

/// PL011 UART on the QEMU virt board. Works both before and after
/// exit_boot_services since we poke the MMIO registers directly.
pub struct Serial {
    base: usize,
}

impl Serial {
    const DR: usize = 0x00;
    const FR: usize = 0x18;
    const FR_TXFF: u32 = 1 << 5;

    pub const fn new() -> Self {
        Self { base: 0x0900_0000 }
    }

    fn write_byte(&mut self, b: u8) {
        unsafe {
            let fr = (self.base + Self::FR) as *const u32;
            while core::ptr::read_volatile(fr) & Self::FR_TXFF != 0 {}
            core::ptr::write_volatile((self.base + Self::DR) as *mut u32, b as u32);
        }
    }
}

unsafe impl Send for Serial {}

impl fmt::Write for Serial {
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

pub fn boot_privilege() -> &'static str {
    let el: u64;
    unsafe { asm!("mrs {0}, CurrentEL", out(reg) el) };
    match el >> 2 {
        1 => "EL1",
        2 => "EL2",
        _ => "EL?",
    }
}

pub fn park() -> ! {
    loop {
        unsafe { asm!("wfe") };
    }
}
