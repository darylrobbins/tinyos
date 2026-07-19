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

/// Per-CPU reschedule hints, set by the IPI handler.
pub static RESCHED: [AtomicBool; N] = [const { AtomicBool::new(false) }; N];

/// One idle wait: sleep until `deadline_us`, an input IRQ, or an IPI —
/// then return so the scheduler can re-examine the ready queue.
///
/// Race-freedom: flags are checked with IF clear; `sti` takes effect after
/// the following instruction, so an IRQ arriving between the check and the
/// `hlt` is delivered during the hlt and wakes it.
pub fn idle_once(deadline_us: u64) {
    let cpu = super::cpu_id();
    let now = super::timer::uptime_us();
    if now >= deadline_us
        || WAKE_INPUT.load(Ordering::Acquire)
        || RESCHED[cpu].swap(false, Ordering::Acquire)
    {
        note_busy(cpu);
        return;
    }
    apic::set_timer_us(deadline_us);
    unsafe {
        asm!("sti", "hlt", "cli");
    }
    let woke = super::timer::uptime_us();
    SLEPT_US[cpu].fetch_add(woke - now, Ordering::Relaxed);
    WAKES[cpu].fetch_add(1, Ordering::Relaxed);
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

/// Poke every other CPU out of hlt so it re-runs its scheduler pass.
pub fn kick_others(from: usize) {
    for cpu in 0..super::MAX_CPUS {
        if cpu != from {
            super::apic::send_ipi(cpu, super::apic::VEC_IPI);
        }
    }
}

/// Called from the IPI-vector gate.
pub fn on_ipi() {
    RESCHED[super::cpu_id()].store(true, Ordering::Release);
    apic::eoi();
}

/// (wakes per second, idle percent) for one CPU over its last ~1s window.
/// A window that hasn't rolled in >2s means the CPU is in a long sleep
/// (busy CPUs roll every second via note_busy): report it fully idle.
pub fn wake_stats(cpu: usize) -> (u32, u32) {
    let start = WINDOW_START_US[cpu].load(Ordering::Relaxed);
    if super::timer::uptime_us().saturating_sub(start) > 2_000_000 {
        return (0, 100);
    }
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
