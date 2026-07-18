// Architecture facade: everything the rest of the kernel needs from the CPU
// goes through here. Each arch module exports the same surface:
// Serial, NAME, MACHINE, boot_privilege(), park(), exceptions, timer.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;
