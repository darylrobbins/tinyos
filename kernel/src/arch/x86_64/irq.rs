//! IRQ facade for x86_64: LAPIC one-shot timer + IOAPIC-routed virtio
//! INTx, tickless `hlt` sleeping. The legacy 8259 PICs are fully masked.
//! Handlers only ack + set atomics; the main loop does all real work.
//!
//! Interrupt-flag discipline: IF stays clear during normal execution;
//! `sleep_until` runs `sti; hlt; cli` per wait, so handlers run only
//! inside the sleep window (mirroring wfi+DAIF masking on aarch64).

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::apic;
use super::io::outb;

const N: usize = super::MAX_CPUS;

pub static WAKE_INPUT: AtomicBool = AtomicBool::new(false);
static WAKES: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static SLEPT_US: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static WINDOW_START_US: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static LAST_RATE: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static LAST_IDLE_PCT: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];

pub static INPUT_ISR_ADDRS: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

/// GSIs of the virtio input devices, registered before init() by the
/// input driver via `register_input_gsi`.
static INPUT_GSIS: [AtomicU32; 8] = [const { AtomicU32::new(u32::MAX) }; 8];

pub fn register_input_gsi(slot: usize, gsi: u32) {
    if slot < INPUT_GSIS.len() {
        INPUT_GSIS[slot].store(gsi, Ordering::Relaxed);
    }
}

pub fn init() {
    // Mask both 8259 PICs entirely — the IOAPIC owns everything.
    outb(0x21, 0xFF);
    outb(0xA1, 0xFF);

    apic::init();

    for (i, gsi) in INPUT_GSIS.iter().enumerate() {
        let gsi = gsi.load(Ordering::Relaxed);
        if gsi != u32::MAX {
            apic::ioapic_redirect(gsi, apic::VEC_INPUT_BASE + i as u8);
        }
    }

    WINDOW_START_US[0].store(super::timer::uptime_us(), Ordering::Relaxed);
}

pub fn sleep_until(deadline_us: u64) {
    let cpu = super::cpu_id();
    loop {
        let now = super::timer::uptime_us();
        if now >= deadline_us || WAKE_INPUT.swap(false, Ordering::Acquire) {
            break;
        }
        apic::set_timer_us(deadline_us);
        unsafe {
            // sti takes effect after the next instruction, so no wake can
            // be lost between sti and hlt.
            asm!("sti", "hlt", "cli");
        }
        let woke = super::timer::uptime_us();
        SLEPT_US[cpu].fetch_add(woke - now, Ordering::Relaxed);
        WAKES[cpu].fetch_add(1, Ordering::Relaxed);
    }
    apic::clear_timer();
    note_busy(cpu);
}

/// Roll the 1s stats window for a CPU; safe to call whether it slept or not.
pub fn note_busy(cpu: usize) {
    let now = super::timer::uptime_us();
    let start = WINDOW_START_US[cpu].load(Ordering::Relaxed);
    let span = now.saturating_sub(start);
    if span >= 1_000_000 {
        let wakes = WAKES[cpu].swap(0, Ordering::Relaxed);
        let slept = SLEPT_US[cpu].swap(0, Ordering::Relaxed);
        LAST_RATE[cpu].store((wakes as u64 * 1_000_000 / span) as u32, Ordering::Relaxed);
        LAST_IDLE_PCT[cpu].store((slept * 100 / span).min(100) as u32, Ordering::Relaxed);
        WINDOW_START_US[cpu].store(now, Ordering::Relaxed);
    }
}

/// Wake other CPUs so they notice new ready threads. Real IPI lands with SMP.
pub fn kick_others(_from: usize) {}

/// (wakes per second, idle percent) for one CPU over its last ~1s window.
pub fn wake_stats(cpu: usize) -> (u32, u32) {
    (
        LAST_RATE[cpu].load(Ordering::Relaxed),
        LAST_IDLE_PCT[cpu].load(Ordering::Relaxed),
    )
}

/// Called from the timer-vector gate.
pub fn on_timer_irq() {
    apic::clear_timer();
    apic::eoi();
}

/// Called from any input-vector gate: deassert INTx, flag the wake.
pub fn on_input_irq() {
    for slot in &INPUT_ISR_ADDRS {
        let addr = slot.load(Ordering::Relaxed);
        if addr != 0 {
            let _ = crate::drivers::mmio::r8(addr);
        }
    }
    WAKE_INPUT.store(true, Ordering::Release);
    apic::eoi();
}
