//! TSC-based time, calibrated once against the PIT (channel 2 one-shot).

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use super::io::{inb, outb};

static TSC_PER_US: AtomicU64 = AtomicU64::new(0);

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { asm!("lfence", "rdtsc", out("eax") lo, out("edx") hi) };
    (hi as u64) << 32 | lo as u64
}

/// Run the PIT down for 50 ms and count TSC ticks.
fn calibrate() -> u64 {
    const PIT_HZ: u64 = 1_193_182;
    const TICKS: u64 = PIT_HZ / 20; // 50 ms

    // Gate channel 2 on, speaker off.
    outb(0x61, (inb(0x61) & !0x02) | 0x01);
    // Channel 2, lobyte/hibyte, mode 0 (interrupt on terminal count).
    outb(0x43, 0xB0);
    outb(0x42, TICKS as u8);
    outb(0x42, (TICKS >> 8) as u8);

    let start = rdtsc();
    // OUT2 (port 0x61 bit 5) goes high at terminal count.
    while inb(0x61) & 0x20 == 0 {
        core::hint::spin_loop();
    }
    let ticks_50ms = rdtsc() - start;

    (ticks_50ms / 50_000).max(1)
}

fn tsc_per_us() -> u64 {
    let v = TSC_PER_US.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    let v = calibrate();
    TSC_PER_US.store(v, Ordering::Relaxed);
    v
}

pub fn uptime_us() -> u64 {
    rdtsc() / tsc_per_us()
}

pub fn uptime_ms() -> u64 {
    uptime_us() / 1000
}

pub fn wait_until_us(deadline: u64) {
    while uptime_us() < deadline {
        core::hint::spin_loop();
    }
}
