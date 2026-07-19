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

/// Busy-wait frame pacing (pre-IRQ boot phases only).
pub fn wait_until_us(deadline: u64) {
    while uptime_us() < deadline {
        core::hint::spin_loop();
    }
}

/// Arm the virtual timer to fire at `deadline_us` (one-shot).
pub fn set_timer_us(deadline_us: u64) {
    let now = uptime_us();
    let delta_us = deadline_us.saturating_sub(now).max(1);
    let ticks = (delta_us as u128 * frequency() as u128 / 1_000_000) as u64;
    unsafe {
        asm!("msr CNTV_TVAL_EL0, {0}", in(reg) ticks.max(1));
        asm!("msr CNTV_CTL_EL0, {0}", in(reg) 1u64); // enable, unmasked
        asm!("isb");
    }
}

pub fn clear_timer() {
    unsafe {
        asm!("msr CNTV_CTL_EL0, {0}", in(reg) 0u64);
        asm!("isb");
    }
}
