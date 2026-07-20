pub mod apic;
pub mod context;
pub mod exceptions;
pub mod io;
pub mod irq;
pub mod paging;
pub mod smp;
pub mod timer;
pub mod uaccess;
pub mod user;

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

pub const MAX_CPUS: usize = 4;

/// 0-based CPU index. On QEMU q35, LAPIC IDs are 0..N-1.
pub fn cpu_id() -> usize {
    let id = unsafe { (0xFEE0_0020usize as *const u32).read_volatile() } >> 24;
    (id as usize).min(MAX_CPUS - 1)
}

pub fn boot_privilege() -> &'static str {
    "ring 0"
}

pub fn park() -> ! {
    loop {
        unsafe { asm!("hlt") };
    }
}

/// ACPI S5 on QEMU q35: SLP_EN with SLP_TYP 0 to the ICH9 PM1a control port.
pub fn poweroff() -> ! {
    io::outw(0x604, 0x2000);
    park() // unreachable on QEMU
}

/// Full reset via the ICH reset-control register.
pub fn reboot() -> ! {
    io::outb(0xCF9, 0x06);
    park()
}
