//! Stub until K3 brings LAPIC/IOAPIC parity: no interrupts, busy wait.

use core::sync::atomic::AtomicUsize;

pub static INPUT_ISR_ADDRS: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

pub fn init() {}

pub fn sleep_until(deadline_us: u64) {
    super::timer::wait_until_us(deadline_us);
}

pub fn wake_stats() -> (u32, u32) {
    (0, 0)
}
