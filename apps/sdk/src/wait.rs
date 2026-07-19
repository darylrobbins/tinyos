//! Waiting: the one blocking primitive, plus a sleep helper.

use crate::syscall::*;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WaitItem {
    pub handle: u32,
    pub want: u32,
    pub observed: u32,
}

/// Block until one of `items` has a wanted signal, `deadline_us` (absolute
/// uptime µs) passes, or the process is killed. Observed signals are filled
/// in. Returns OK / TIMED_OUT / KILLED.
pub fn wait_many(items: &mut [WaitItem], deadline_us: u64) -> u32 {
    syscall3(
        SYS_WAIT_MANY,
        items.as_mut_ptr() as u64,
        items.len() as u64,
        deadline_us,
    )
    .status
}

/// Wait on a single handle; returns the observed signals or an error status.
pub fn wait_one(handle: u32, want: u32, deadline_us: u64) -> Result<u32, u32> {
    let mut it = [WaitItem { handle, want, observed: 0 }];
    match wait_many(&mut it, deadline_us) {
        ST_OK => Ok(it[0].observed),
        st => Err(st),
    }
}

pub fn uptime_us() -> u64 {
    syscall0(SYS_CLOCK_UPTIME).value
}

/// Sleep for `us` microseconds (wait with no objects).
pub fn sleep_us(us: u64) {
    let _ = syscall3(SYS_WAIT_MANY, 0, 0, uptime_us() + us);
}
