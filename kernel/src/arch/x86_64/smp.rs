//! AP bring-up via UEFI MP Services: APs are launched into a park loop
//! before exit_boot_services (the firmware does INIT-SIPI for us) and
//! released into the scheduler once the kernel owns the machine.
//!
//! Risk note: EDK2 may reclaim APs at ExitBootServices depending on its AP
//! loop mode. start_secondary_cpus() therefore treats "no AP checked in
//! within 500 ms of release" as bring-up failure and continues single-core.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use uefi::boot::{self, AllocateType, EventType, MemoryType, Tpl};
use uefi::proto::pi::mp::MpServices;

use crate::arch::MAX_CPUS;

const AP_STACK_SIZE: usize = 64 * 1024;

static RELEASED: AtomicBool = AtomicBool::new(false);
static PARKED: AtomicU32 = AtomicU32::new(0);
static AP_ONLINE: AtomicU32 = AtomicU32::new(0);
/// Stack tops for cpus 1..MAX_CPUS, written by park_aps() before any AP
/// reads them (the park procedure only reads after RELEASED, which the BSP
/// sets long after these are filled).
static mut AP_STACKS: [u64; MAX_CPUS] = [0; MAX_CPUS];

extern "efiapi" fn ap_park(_arg: *mut c_void) {
    let cpu = super::cpu_id();
    PARKED.fetch_add(1, Ordering::Release);
    while !RELEASED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    // The firmware-provided stack is dead to us after exit_boot_services:
    // switch to our own, then never return.
    unsafe {
        let stack = AP_STACKS[cpu];
        core::arch::asm!(
            "mov rsp, {0}",
            "sub rsp, 40",  // MS ABI shadow space + alignment
            "call ap_main",
            in(reg) stack,
            in("rcx") cpu as u64,
            options(noreturn),
        );
    }
}

#[unsafe(no_mangle)]
extern "C" fn ap_main(cpu: u64) -> ! {
    let cpu = cpu as usize;
    super::exceptions::load();
    super::apic::init_ap();
    AP_ONLINE.fetch_add(1, Ordering::Release);
    kprintln!("tinyos: cpu{cpu} online");
    crate::sched::ap_enter(cpu)
}

/// Call while boot services are live: launch all APs into the park loop.
pub fn park_aps() {
    let Ok(handle) = boot::get_handle_for_protocol::<MpServices>() else {
        kprintln!("tinyos: no MP services, staying single-core");
        return;
    };
    let Ok(mp) = boot::open_protocol_exclusive::<MpServices>(handle) else {
        kprintln!("tinyos: MP services busy, staying single-core");
        return;
    };
    let count = match mp.get_number_of_processors() {
        Ok(c) => c.enabled.min(MAX_CPUS),
        Err(_) => 1,
    };
    kprintln!("tinyos: firmware reports {count} cpus");
    if count <= 1 {
        return;
    }

    for cpu in 1..count {
        // LOADER_DATA survives exit_boot_services (we keep that type).
        let pages = AP_STACK_SIZE / 4096;
        match boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages) {
            Ok(mem) => unsafe {
                AP_STACKS[cpu] = (mem.as_ptr() as u64 + AP_STACK_SIZE as u64) & !0xF;
            },
            Err(_) => {
                kprintln!("tinyos: ap stack alloc failed, staying single-core");
                return;
            }
        }
    }

    // A wait event makes startup_all_aps non-blocking (our procedure never
    // returns). We never check the event; leaking it is fine.
    let Ok(event) = (unsafe { boot::create_event(EventType::empty(), Tpl::NOTIFY, None, None) })
    else {
        kprintln!("tinyos: create_event failed, staying single-core");
        return;
    };
    match mp.startup_all_aps(false, ap_park, core::ptr::null_mut(), Some(event), None) {
        Ok(()) => {
            let t0 = super::timer::uptime_us();
            while (PARKED.load(Ordering::Acquire) as usize) < count - 1
                && super::timer::uptime_us() - t0 < 500_000
            {
                core::hint::spin_loop();
            }
            kprintln!("tinyos: {} aps parked", PARKED.load(Ordering::Acquire));
        }
        Err(e) => kprintln!("tinyos: startup_all_aps failed ({e:?}), single-core"),
    }
}

/// Post-exit, post-sched: open the pen.
pub fn start_secondary_cpus() {
    let parked = PARKED.load(Ordering::Acquire);
    if parked == 0 {
        return;
    }
    RELEASED.store(true, Ordering::Release);
    let t0 = super::timer::uptime_us();
    while AP_ONLINE.load(Ordering::Acquire) < parked && super::timer::uptime_us() - t0 < 500_000 {
        core::hint::spin_loop();
    }
    let online = AP_ONLINE.load(Ordering::Acquire);
    if online < parked {
        kprintln!("tinyos: only {online} of {parked} parked aps woke (firmware reclaimed them?)");
    }
    kprintln!("tinyos: {} of {} cpus online", 1 + online, 1 + parked);
}
