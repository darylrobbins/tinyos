//! EL0 smoke test: hand-assembled user code in a fresh address space.
//! Invoked by the terminal `usertest` command, or at boot when QEMU passes
//! `-fw_cfg name=opt/tinyos/boottest,string=user` (headless verification).

use alloc::sync::Arc;

use crate::arch::paging::{APP_IMAGE_BASE, AddrSpace, MapFlags, sync_icache};
use crate::mem::frames::{FRAME_SIZE, alloc_frames};
use crate::sched::{self, thread::Class};

/// Spawn the test thread. `spin` = infinite EL0 loop (proves preemption and
/// `kill`); otherwise: syscall-log a string, then exit(42).
pub fn spawn(spin: bool) -> Result<u32, &'static str> {
    let Some(mut asp) = AddrSpace::new() else {
        return Err("userspace unsupported on this arch");
    };
    let (Some(code_pa), Some(stack_pa)) = (alloc_frames(1), alloc_frames(4)) else {
        return Err("out of memory");
    };

    let msg = b"hello from EL0";
    let insns: &[u32] = if spin {
        &[0x1400_0000] // b . : never yields, only preemption reaps it
    } else {
        &[
            0xD280_2000,                           // movz x0, #0x100        (msg VA, low)
            0xF2A0_0800,                           // movk x0, #0x40, lsl 16
            0xF2DF_FF80,                           // movk x0, #0xFFFC, lsl 32
            0xF2FF_FFE0,                           // movk x0, #0xFFFF, lsl 48
            0xD280_0001 | (msg.len() as u32) << 5, // movz x1, #len
            0xD280_0008,                           // movz x8, #0            (SYS_LOG)
            0xD400_0001,                           // svc #0
            0xD280_0540,                           // movz x0, #42           (exit code)
            0xD280_0148,                           // movz x8, #10           (SYS_PROCESS_EXIT)
            0xD400_0001,                           // svc #0
            0x1400_0000,                           // b .   (not reached)
        ]
    };
    unsafe {
        core::ptr::copy_nonoverlapping(insns.as_ptr(), code_pa as *mut u32, insns.len());
        core::ptr::copy_nonoverlapping(msg.as_ptr(), (code_pa + 0x100) as *mut u8, msg.len());
    }
    sync_icache(code_pa, FRAME_SIZE);

    let stack_len = 4 * FRAME_SIZE;
    let mut ok = asp
        .map(APP_IMAGE_BASE, code_pa, FRAME_SIZE, MapFlags { write: false, exec: true }, true)
        .is_some();
    let stack_va = asp.alloc_va(stack_len);
    ok = ok
        && asp
            .map(stack_va, stack_pa, stack_len, MapFlags { write: true, exec: false }, true)
            .is_some();
    if !ok {
        return Err("mapping failed");
    }

    Ok(sched::spawn_user(
        alloc::format!("user{}", if spin { "spin" } else { "test" }),
        Class::Normal,
        if sched::online_cpus() > 1 { 0b1110 } else { 0b0001 },
        Arc::new(spin::Mutex::new(asp)),
        APP_IMAGE_BASE,
        stack_va + stack_len as u64,
        0,
    ))
}

/// Boot-time hook: run a smoke test when fw_cfg asks for it. "user" = the
/// syscall round-trip; "spin" = preemption/kill proof (an EL0 thread that
/// never syscalls can still be reaped).
pub fn boot_hook() {
    match crate::drivers::fwcfg::read_str("opt/tinyos/boottest").as_deref() {
        Some("user") => match spawn(false) {
            Ok(id) => kprintln!("tinyos: boottest spawned EL0 thread {id}"),
            Err(e) => kprintln!("tinyos: boottest FAILED: {e}"),
        },
        Some("spin") => match spawn(true) {
            Ok(id) => {
                kprintln!("tinyos: boottest spawned EL0 spinner {id}");
                SPIN_ID.store(id, core::sync::atomic::Ordering::Relaxed);
                sched::spawn(
                    alloc::string::String::from("boottest"),
                    Class::Normal,
                    0b0001,
                    spin_watchdog,
                );
            }
            Err(e) => kprintln!("tinyos: boottest FAILED: {e}"),
        },
        Some("obj") => {
            for line in super::objtest::run() {
                kprintln!("tinyos: objtest {line}");
            }
        }
        _ => {}
    }
}

static SPIN_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Let the EL0 spinner burn CPU for a second (only timer preemption can get
/// it off the core), then kill it and confirm it vanished.
fn spin_watchdog() {
    let id = SPIN_ID.load(core::sync::atomic::Ordering::Relaxed);
    sched::sleep_us(1_000_000);
    let alive = sched::snapshot().iter().any(|t| t.id == id);
    kprintln!("tinyos: boottest spinner alive under preemption: {alive}");
    sched::kill(id);
    sched::sleep_us(500_000);
    let gone = !sched::snapshot().iter().any(|t| t.id == id);
    kprintln!(
        "tinyos: boottest preempt/kill {}",
        if alive && gone { "PASS" } else { "FAIL" }
    );
}
