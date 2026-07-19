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
    spawn_mode(if spin { Mode::Spin } else { Mode::Hello })
}

#[derive(PartialEq, Clone, Copy)]
pub enum Mode {
    Hello,
    Spin,
    /// clock_uptime → wait_many(count=0, now+200ms) → expect TIMED_OUT →
    /// exit(42); any deviation exits 1.
    Sys,
}

pub fn spawn_mode(mode: Mode) -> Result<u32, &'static str> {
    let Some(mut asp) = AddrSpace::new() else {
        return Err("userspace unsupported on this arch");
    };
    let (Some(code_pa), Some(stack_pa)) = (alloc_frames(1), alloc_frames(4)) else {
        return Err("out of memory");
    };

    let msg = b"hello from EL0";
    let insns: &[u32] = match mode {
        Mode::Spin => &[0x1400_0000], // b . : never yields, only preemption reaps it
        Mode::Sys => &[
            0xD280_0168, // movz x8, #11          (SYS_CLOCK_UPTIME)
            0xD400_0001, // svc #0                -> x1 = uptime µs
            0xD280_1A82, // movz x2, #0x0D40      (200_000 = 0x30D40)
            0xF2A0_0062, // movk x2, #0x3, lsl 16
            0x8B01_0042, // add x2, x2, x1        (deadline = now + 200ms)
            0xAA1F_03E0, // mov x0, xzr           (items = null)
            0xAA1F_03E1, // mov x1, xzr           (count = 0: pure sleep)
            0xD280_00C8, // movz x8, #6           (SYS_WAIT_MANY)
            0xD400_0001, // svc #0                -> x0 = status
            0xF100_1C1F, // subs xzr, x0, #7      (ST_TIMED_OUT?)
            0x5400_0061, // b.ne +12
            0xD280_0540, // movz x0, #42
            0x1400_0002, // b +8
            0xD280_0020, // movz x0, #1
            0xD280_0148, // movz x8, #10          (SYS_PROCESS_EXIT)
            0xD400_0001, // svc #0
            0x1400_0000, // b .
        ],
        Mode::Hello => &[
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
        alloc::format!(
            "user{}",
            match mode {
                Mode::Spin => "spin",
                Mode::Sys => "sys",
                Mode::Hello => "test",
            }
        ),
        Class::Normal,
        if sched::online_cpus() > 1 { 0b1110 } else { 0b0001 },
        Arc::new(spin::Mutex::new(asp)),
        APP_IMAGE_BASE,
        stack_va + stack_len as u64,
        0,
        None,
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
        Some("sys") => match spawn_mode(Mode::Sys) {
            Ok(id) => kprintln!("tinyos: boottest spawned EL0 sys-test thread {id}"),
            Err(e) => kprintln!("tinyos: boottest FAILED: {e}"),
        },
        Some("run") => {
            sched::spawn(
                alloc::string::String::from("boottest"),
                Class::Normal,
                0b0001,
                run_hello_watchdog,
            );
        }
        Some("spawnonly") => {
            // Spawn hello directly (no watchdog), let it exit; UI heartbeat
            // tells us if the system survives teardown.
            let argv = [alloc::string::String::from("a"), alloc::string::String::from("b")];
            match crate::fs::read("/", "/apps/hello") {
                Ok(elf) => match super::loader::spawn(alloc::string::String::from("hello"), &elf, &argv, &super::loader::GrantSet::all()) {
                    Ok(app) => kprintln!("tinyos: spawnonly thread {}", app.thread_id),
                    Err(e) => kprintln!("tinyos: spawnonly FAILED {}", e.msg()),
                },
                Err(e) => kprintln!("tinyos: spawnonly no hello ({e})"),
            }
        }
        Some("fs") => {
            for path in ["/", "/apps"] {
                match crate::fs::list("/", path) {
                    Ok(entries) => {
                        for e in entries {
                            let slash = if matches!(e.kind, tinyfs::InodeKind::Dir) { "/" } else { "" };
                            kprintln!("tinyos: fstest {path} -> {}{slash} {}", e.name, e.size);
                        }
                    }
                    Err(err) => kprintln!("tinyos: fstest {path} FAILED ({err})"),
                }
            }
        }
        Some("obj") => {
            for line in super::objtest::run() {
                kprintln!("tinyos: objtest {line}");
            }
        }
        _ => {}
    }
}

/// Headless end-to-end: load /apps/hello with argv, pump its console to
/// serial, and report the exit code. Mirrors what the terminal `run` does.
/// Runs twice to prove repeatability + continued liveness after teardown.
fn run_hello_watchdog() {
    run_hello_once();
    kprintln!("tinyos: runtest done, system alive");
}

fn run_hello_once() {
    let argv = [
        alloc::string::String::from("alpha"),
        alloc::string::String::from("beta"),
        alloc::string::String::from("gamma"),
    ];
    let elf = match crate::fs::read("/", "/apps/hello") {
        Ok(e) => e,
        Err(err) => {
            kprintln!("tinyos: runtest FAILED: /apps/hello: {err}");
            return;
        }
    };
    let app = match super::loader::spawn(alloc::string::String::from("hello"), &elf, &argv, &super::loader::GrantSet::all()) {
        Ok(a) => a,
        Err(e) => {
            kprintln!("tinyos: runtest FAILED: {}", e.msg());
            return;
        }
    };
    const OP_WRITE: u32 = 1;
    let deadline = crate::arch::timer::uptime_us() + 3_000_000;
    let mut drain = |console: &alloc::sync::Arc<super::channel::ChannelEnd>| {
        while let Ok(msg) = console.recv() {
            if msg.bytes.len() >= 4
                && u32::from_le_bytes(msg.bytes[0..4].try_into().unwrap()) == OP_WRITE
            {
                if let Ok(s) = core::str::from_utf8(&msg.bytes[4..]) {
                    for line in s.lines() {
                        kprintln!("tinyos: runtest| {line}");
                    }
                }
            }
        }
    };
    // Poll cooperatively. This vCPU must not peg a host core under HVF (that
    // starves the emulated APs running the app); a periodic serial write per
    // iteration paces it host-friendly. The real terminal drains from a
    // frame-driven pump that blocks between frames, so it never busy-loops.
    let mut it = 0u64;
    loop {
        it += 1;
        if it <= 8 || it % 100_000 == 0 {
            kprintln!("tinyos: runtest draining (iter {it})");
        }
        drain(&app.console);
        if let Some(code) = app.process.exited() {
            drain(&app.console); // catch anything queued before exit
            kprintln!(
                "tinyos: runtest exit code {code} (expect 3) {}",
                if code == 3 { "PASS" } else { "FAIL" }
            );
            return;
        }
        if crate::arch::timer::uptime_us() > deadline {
            kprintln!("tinyos: runtest FAILED: timeout");
            return;
        }
        sched::yield_now();
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
