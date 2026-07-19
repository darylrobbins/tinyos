//! Kernel object model: handles, channels, memory objects, processes — the
//! capability layer the app ABI is built on. `syscall` is the EL0 dispatch.

pub mod syscall;
pub mod usertest;
