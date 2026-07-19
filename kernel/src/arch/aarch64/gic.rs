//! GICv3 on the QEMU virt board: MMIO distributor/redistributor init,
//! system-register CPU interface. Only CPU0's redistributor is brought up
//! today; the layout is per-CPU so SMP bring-up can init its own later.

use core::arch::asm;

use crate::drivers::mmio;

const GICD: usize = 0x0800_0000;
const GICR: usize = 0x080A_0000; // CPU0 frame (stride 0x20000 per CPU)

// Distributor registers.
const GICD_CTLR: usize = 0x0000;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICFGR: usize = 0x0C00;

// Redistributor (RD_base + SGI_base frames).
const GICR_WAKER: usize = 0x0014;
const GICR_SGI: usize = 0x1_0000;
const GICR_ISENABLER0: usize = GICR_SGI + 0x0100;

const GICR_STRIDE: usize = 0x2_0000;

fn gicr_base(cpu: usize) -> usize {
    GICR + cpu * GICR_STRIDE
}

pub fn init() {
    // Distributor: enable group-1 non-secure + affinity routing.
    mmio::w32(GICD + GICD_CTLR, 0b10 | (1 << 4)); // EnableGrp1NS | ARE_NS
    init_cpu(0);
}

/// Per-CPU redistributor + CPU-interface init. Must run on the CPU itself
/// (the ICC_* system registers are banked per CPU).
pub fn init_cpu(cpu: usize) {
    let r = gicr_base(cpu);
    let waker = mmio::r32(r + GICR_WAKER);
    mmio::w32(r + GICR_WAKER, waker & !(1 << 1)); // clear ProcessorSleep
    while mmio::r32(r + GICR_WAKER) & (1 << 2) != 0 {} // ChildrenAsleep

    // Virtual-timer PPI (27) + SGI 0 (our IPI).
    mmio::w32(r + GICR_ISENABLER0, (1 << 27) | (1 << 0));

    unsafe {
        // Enable the sysreg CPU interface, open the priority mask, group 1 on.
        asm!("msr ICC_SRE_EL1, {0:x}", in(reg) 1u64);
        asm!("isb");
        asm!("msr ICC_PMR_EL1, {0:x}", in(reg) 0xFFu64);
        asm!("msr ICC_IGRPEN1_EL1, {0:x}", in(reg) 1u64);
        asm!("isb");
    }
}

/// Send SGI `sgi` to a single CPU (Aff0 = cpu, Aff1..3 = 0 on QEMU virt).
pub fn send_sgi(cpu: usize, sgi: u32) {
    let val: u64 = ((sgi as u64) << 24) | (1u64 << (cpu & 0xF));
    unsafe {
        asm!("msr ICC_SGI1R_EL1, {0}", in(reg) val);
        asm!("isb");
    }
}

/// Enable a shared peripheral interrupt as level-triggered.
pub fn enable_spi(intid: u32) {
    let reg = (intid / 32) as usize * 4;
    // Level-triggered: clear the 2-bit config field (bit1 = edge).
    let cfg_reg = GICD + GICD_ICFGR + (intid / 16) as usize * 4;
    let shift = (intid % 16) * 2;
    let cfg = mmio::r32(cfg_reg);
    mmio::w32(cfg_reg, cfg & !(0b10 << shift));
    mmio::w32(GICD + GICD_ISENABLER + reg, 1 << (intid % 32));
}

/// Acknowledge: returns the interrupt ID (1023 = spurious).
pub fn ack() -> u32 {
    let id: u64;
    unsafe { asm!("mrs {0}, ICC_IAR1_EL1", out(reg) id) };
    id as u32
}

pub fn eoi(intid: u32) {
    unsafe {
        asm!("msr ICC_EOIR1_EL1, {0:x}", in(reg) intid as u64);
        asm!("isb");
    }
}
