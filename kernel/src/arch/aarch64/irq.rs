//! IRQ facade: tickless sleep on the virtual timer + input-wake flags.
//! Handlers only ack hardware and set atomics; all real work happens in
//! the cooperative main loop after waking.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::{gic, timer};

pub static WAKE_INPUT: AtomicBool = AtomicBool::new(false);
static WAKES: AtomicU32 = AtomicU32::new(0);
static SLEPT_US: AtomicU64 = AtomicU64::new(0);
static WINDOW_START_US: AtomicU64 = AtomicU64::new(0);
static LAST_RATE: AtomicU32 = AtomicU32::new(0);
static LAST_IDLE_PCT: AtomicU32 = AtomicU32::new(0);

/// Physical MMIO addresses of virtio ISR registers, registered by the
/// input driver; the IRQ handler reads each to deassert INTx.
pub static INPUT_ISR_ADDRS: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

/// PCIe INTA..INTD on the virt board.
const SPI_PCIE_BASE: u32 = 32 + 3;

pub fn init() {
    gic::init();
    for i in 0..4 {
        gic::enable_spi(SPI_PCIE_BASE + i);
    }
    WINDOW_START_US.store(timer::uptime_us(), Ordering::Relaxed);
}

/// Sleep until `deadline_us` or an input interrupt, whichever is first.
/// IRQs are taken only while waiting inside `wfi`.
pub fn sleep_until(deadline_us: u64) {
    loop {
        let now = timer::uptime_us();
        if now >= deadline_us || WAKE_INPUT.swap(false, Ordering::Acquire) {
            break;
        }
        timer::set_timer_us(deadline_us);
        unsafe {
            // Unmask IRQs just for the wait; handler runs, then we re-mask.
            asm!("msr daifclr, #2");
            asm!("wfi");
            asm!("msr daifset, #2");
        }
        let woke = timer::uptime_us();
        SLEPT_US.fetch_add(woke - now, Ordering::Relaxed);
        WAKES.fetch_add(1, Ordering::Relaxed);
    }
    timer::clear_timer();
    update_stats();
}

fn update_stats() {
    let now = timer::uptime_us();
    let start = WINDOW_START_US.load(Ordering::Relaxed);
    let span = now - start;
    if span >= 1_000_000 {
        let wakes = WAKES.swap(0, Ordering::Relaxed);
        let slept = SLEPT_US.swap(0, Ordering::Relaxed);
        LAST_RATE.store((wakes as u64 * 1_000_000 / span) as u32, Ordering::Relaxed);
        LAST_IDLE_PCT.store((slept * 100 / span).min(100) as u32, Ordering::Relaxed);
        WINDOW_START_US.store(now, Ordering::Relaxed);
    }
}

/// (wakes per second, idle percent) over the last ~1s window.
pub fn wake_stats() -> (u32, u32) {
    (
        LAST_RATE.load(Ordering::Relaxed),
        LAST_IDLE_PCT.load(Ordering::Relaxed),
    )
}

#[unsafe(no_mangle)]
extern "C" fn irq_entry() {
    loop {
        let id = gic::ack();
        if id >= 1020 {
            break; // spurious / no more pending
        }
        match id {
            27 => timer::clear_timer(),
            id if (SPI_PCIE_BASE..SPI_PCIE_BASE + 4).contains(&id) => {
                for slot in &INPUT_ISR_ADDRS {
                    let addr = slot.load(Ordering::Relaxed);
                    if addr != 0 {
                        let _ = crate::drivers::mmio::r8(addr);
                    }
                }
                WAKE_INPUT.store(true, Ordering::Release);
            }
            _ => {}
        }
        gic::eoi(id);
    }
}
