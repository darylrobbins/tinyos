//! Secondary-CPU bring-up via PSCI. QEMU virt exposes PSCI with an HVC
//! conduit when running under HVF/KVM (guest at EL1). Under TCG the conduit
//! is SMC and the HVC below would take an undefined-instruction exception —
//! acceptable: arm runs HVF-accelerated per the Makefile.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::MAX_CPUS;

const PSCI_VERSION: u32 = 0x8400_0000;
const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;
const PSCI_SYSTEM_RESET: u32 = 0x8400_0009;
const PSCI_CPU_ON64: u32 = 0xC400_0003;

macro_rules! read_sysreg {
    ($name:literal) => {{
        let v: u64;
        unsafe { core::arch::asm!(concat!("mrs {0}, ", $name), out(reg) v) };
        v
    }};
}

/// Everything an AP needs before it can run Rust. Written by the BSP,
/// cache-cleaned to PoC, read by the AP with the MMU still off.
#[repr(C, align(64))]
struct ApBoot {
    stack_top: u64, // 0x00
    ttbr0: u64,     // 0x08
    mair: u64,      // 0x10
    tcr: u64,       // 0x18
    sctlr: u64,     // 0x20
    cpu: u64,       // 0x28
}

static AP_ONLINE: AtomicU32 = AtomicU32::new(0);

/// PSCI SYSTEM_OFF: powers off the whole machine, all cores.
pub fn system_off() -> ! {
    psci_call(PSCI_SYSTEM_OFF, 0, 0, 0);
    super::park() // unreachable unless firmware refuses
}

/// PSCI SYSTEM_RESET: cold reboot.
pub fn system_reset() -> ! {
    psci_call(PSCI_SYSTEM_RESET, 0, 0, 0);
    super::park()
}

fn psci_call(func: u32, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "hvc #0",
            inout("x0") func as u64 => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            clobber_abi("C"),
        );
    }
    ret
}

global_asm!(
    r#"
// x0 = &ApBoot. MMU/caches off; turn them on with the BSP's exact config
// (identity-mapped UEFI tables), then jump to Rust on the new stack.
.global ap_entry
ap_entry:
    // APs reset with FP/SIMD trapped (CPACR_EL1.FPEN=0); Rust code uses
    // SIMD registers freely, so enable before any Rust runs.
    mov x1, #(3 << 20)
    msr cpacr_el1, x1
    ldr x1, [x0, #0x08]
    msr ttbr0_el1, x1
    ldr x1, [x0, #0x10]
    msr mair_el1, x1
    ldr x1, [x0, #0x18]
    msr tcr_el1, x1
    isb
    ldr x1, [x0, #0x20]
    msr sctlr_el1, x1
    isb
    ldr x1, [x0, #0x00]
    mov sp, x1
    bl ap_main
"#
);

unsafe extern "C" {
    fn ap_entry();
}

#[unsafe(no_mangle)]
extern "C" fn ap_main(boot: &'static ApBoot) -> ! {
    let cpu = boot.cpu as usize;
    super::exceptions::install();
    super::gic::init_cpu(cpu);
    AP_ONLINE.fetch_add(1, Ordering::Release);
    kprintln!("tinyos: cpu{cpu} online");
    crate::sched::ap_enter(cpu)
}

/// Clean a range to PoC so MMU-off APs see it.
fn clean_dcache(addr: usize, len: usize) {
    let mut a = addr & !63;
    while a < addr + len {
        unsafe { asm!("dc cvac, {0}", in(reg) a) };
        a += 64;
    }
    unsafe { asm!("dsb sy") };
}

pub fn start_secondary_cpus() {
    let ver = psci_call(PSCI_VERSION, 0, 0, 0);
    if ver <= 0 {
        kprintln!("tinyos: psci unavailable ({ver}), staying single-core");
        return;
    }
    kprintln!("tinyos: psci v{}.{}", (ver >> 16) & 0xFFFF, ver & 0xFFFF);

    for cpu in 1..MAX_CPUS {
        let stack = alloc::vec![0u8; crate::sched::thread::STACK_SIZE].into_boxed_slice();
        let stack_top = (stack.as_ptr() as u64 + stack.len() as u64) & !0xF;
        core::mem::forget(stack); // the AP owns it forever (its idle stack)
        let boot = alloc::boxed::Box::leak(alloc::boxed::Box::new(ApBoot {
            stack_top,
            ttbr0: read_sysreg!("ttbr0_el1"),
            mair: read_sysreg!("mair_el1"),
            tcr: read_sysreg!("tcr_el1"),
            sctlr: read_sysreg!("sctlr_el1"),
            cpu: cpu as u64,
        }));
        clean_dcache(boot as *const ApBoot as usize, core::mem::size_of::<ApBoot>());
        clean_dcache(ap_entry as usize, 128);

        let ret = psci_call(
            PSCI_CPU_ON64,
            cpu as u64, // target MPIDR: Aff0 = cpu on virt
            ap_entry as usize as u64,
            boot as *const ApBoot as u64,
        );
        if ret != 0 {
            kprintln!("tinyos: cpu{cpu} CPU_ON failed ({ret})");
        }
    }

    // Give stragglers a moment, then report.
    let t0 = super::timer::uptime_us();
    while (AP_ONLINE.load(Ordering::Acquire) as usize) < MAX_CPUS - 1
        && super::timer::uptime_us() - t0 < 500_000
    {
        core::hint::spin_loop();
    }
    kprintln!(
        "tinyos: {} of {} cpus online",
        1 + AP_ONLINE.load(Ordering::Acquire),
        MAX_CPUS
    );
}
