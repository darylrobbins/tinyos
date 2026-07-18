pub mod exceptions;
pub mod io;
pub mod timer;

use core::arch::asm;
use core::fmt;

pub const NAME: &str = "x86_64";
pub const MACHINE: &str = "QEMU q35, UEFI boot";

/// COM1. QEMU's default 16550 state needs no init for polled TX.
pub struct Serial;

impl Serial {
    const THR: u16 = 0x3F8;
    const LSR: u16 = 0x3FD;

    pub const fn new() -> Self {
        Self
    }

    fn write_byte(&mut self, b: u8) {
        while io::inb(Self::LSR) & 0x20 == 0 {}
        io::outb(Self::THR, b);
    }
}

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
    "ring 0"
}

pub fn park() -> ! {
    loop {
        unsafe { asm!("hlt") };
    }
}
