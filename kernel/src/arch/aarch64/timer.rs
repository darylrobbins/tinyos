use core::arch::asm;

fn counter() -> u64 {
    let v: u64;
    unsafe { asm!("isb", "mrs {0}, CNTVCT_EL0", out(reg) v) };
    v
}

fn frequency() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {0}, CNTFRQ_EL0", out(reg) v) };
    v
}

pub fn uptime_us() -> u64 {
    counter() * 1_000_000 / frequency()
}

pub fn uptime_ms() -> u64 {
    uptime_us() / 1000
}

/// Busy-wait frame pacing; no interrupts needed.
pub fn wait_until_us(deadline: u64) {
    while uptime_us() < deadline {
        core::hint::spin_loop();
    }
}
