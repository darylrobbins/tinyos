//! Debug/serial helpers. `mirror` echoes a line to the serial port as
//! `[out] …` when the kernel's smoke-test mirror is active (a no-op
//! otherwise), so the userspace terminal can feed the headless harness the
//! console output it renders.

use crate::syscall::{syscall2, SYS_DEBUG_MIRROR};

/// Mirror one console line to serial (`[out] …`). Cheap no-op unless the
/// kernel booted with the smoke fw_cfg flag set.
pub fn mirror(s: &str) {
    let _ = syscall2(SYS_DEBUG_MIRROR, s.as_ptr() as u64, s.len() as u64);
}
