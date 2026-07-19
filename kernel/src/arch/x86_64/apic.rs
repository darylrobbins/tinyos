//! Local APIC (xAPIC MMIO) + IOAPIC — the modern x86 interrupt path.
//! No 8259 PIC, no PIT interrupt source; per-CPU by construction, ready
//! for SMP bring-up later.

use core::sync::atomic::{AtomicU64, Ordering};

const LAPIC: usize = 0xFEE0_0000;
const IOAPIC: usize = 0xFEC0_0000;

// LAPIC registers.
const SVR: usize = 0xF0;
const EOI: usize = 0xB0;
const LVT_TIMER: usize = 0x320;
const TIMER_INIT: usize = 0x380;
const TIMER_CURR: usize = 0x390;
const TIMER_DIV: usize = 0x3E0;

pub const VEC_TIMER: u8 = 48;
pub const VEC_INPUT_BASE: u8 = 49;
const VEC_SPURIOUS: u8 = 0xFF;

/// LAPIC timer ticks per microsecond (calibrated once at init).
static TICKS_PER_US: AtomicU64 = AtomicU64::new(0);

fn lapic_r(reg: usize) -> u32 {
    unsafe { ((LAPIC + reg) as *const u32).read_volatile() }
}

fn lapic_w(reg: usize, v: u32) {
    unsafe { ((LAPIC + reg) as *mut u32).write_volatile(v) }
}

pub fn eoi() {
    lapic_w(EOI, 0);
}

pub fn init() {
    // Software-enable the LAPIC with the spurious vector.
    lapic_w(SVR, 0x100 | VEC_SPURIOUS as u32);
    // Divide-by-16, timer masked while we calibrate.
    lapic_w(TIMER_DIV, 0x3);
    lapic_w(LVT_TIMER, 1 << 16);

    // Calibrate LAPIC ticks against the TSC-derived clock over 10ms.
    lapic_w(TIMER_INIT, u32::MAX);
    let t0 = super::timer::uptime_us();
    while super::timer::uptime_us() - t0 < 10_000 {
        core::hint::spin_loop();
    }
    let elapsed_ticks = u32::MAX - lapic_r(TIMER_CURR);
    lapic_w(TIMER_INIT, 0);
    TICKS_PER_US.store((elapsed_ticks as u64 / 10_000).max(1), Ordering::Relaxed);
}

/// Arm the LAPIC timer to fire once at `deadline_us`.
pub fn set_timer_us(deadline_us: u64) {
    let now = super::timer::uptime_us();
    let delta = deadline_us.saturating_sub(now).max(1);
    let ticks = delta
        .saturating_mul(TICKS_PER_US.load(Ordering::Relaxed))
        .min(u32::MAX as u64) as u32;
    lapic_w(LVT_TIMER, VEC_TIMER as u32); // one-shot, unmasked
    lapic_w(TIMER_INIT, ticks.max(1));
}

pub fn clear_timer() {
    lapic_w(TIMER_INIT, 0);
    lapic_w(LVT_TIMER, 1 << 16);
}

/// Route a GSI through the IOAPIC: level-triggered, active-low (PCI INTx).
pub fn ioapic_redirect(gsi: u32, vector: u8) {
    let sel = IOAPIC as *mut u32;
    let win = (IOAPIC + 0x10) as *mut u32;
    let idx = 0x10 + gsi * 2;
    unsafe {
        // Low dword: vector, fixed delivery, level-triggered, active-low.
        sel.write_volatile(idx);
        win.write_volatile(vector as u32 | (1 << 13) | (1 << 15));
        // High dword: destination APIC 0.
        sel.write_volatile(idx + 1);
        win.write_volatile(0);
    }
}
