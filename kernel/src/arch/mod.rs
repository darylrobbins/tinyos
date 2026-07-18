// Architecture facade: everything the rest of the kernel needs from the CPU
// goes through here so an x86_64 port only has to fill in a sibling module.

#[cfg(target_arch = "aarch64")]
mod aarch64;

#[cfg(target_arch = "aarch64")]
pub use aarch64::*;
