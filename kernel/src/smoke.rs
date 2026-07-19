//! Smoke-test console mirror.
//!
//! Normally the shell and userspace apps render only to the framebuffer, so a
//! headless harness can see nothing but the kernel's own `kprintln!` boot log
//! on the serial port. When QEMU is launched with `-fw_cfg
//! name=opt/tinyos/smoke,string=1`, this mirror echoes every scrollback line
//! (see `term::Terminal::out`) to serial prefixed with `[out]`, letting
//! `tools/smoke/smoke.py` assert on actual command output — not just liveness.
//!
//! Default off: with the flag absent `enabled()` is a single relaxed load and
//! nothing about a normal boot changes.

use core::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Read the fw_cfg flag once at boot. Requires the heap (read_str allocates).
pub fn init() {
    let on = crate::drivers::fwcfg::read_str("opt/tinyos/smoke")
        .map(|s| s.trim().starts_with('1'))
        .unwrap_or(false);
    ENABLED.store(on, Ordering::Relaxed);
    if on {
        kprintln!("tinyos: smoke-test console mirror on");
    }
}

#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Mirror one rendered scrollback line to serial. Cheap no-op when disabled.
#[inline]
pub fn mirror(line: &str) {
    if enabled() {
        kprintln!("[out] {line}");
    }
}
