//! Syscall dispatch (ABI v0). x8 = number, x0-x5 args; returns
//! (status, value). Numbers are stable once shipped — append, never renumber.

#![allow(dead_code)]

pub const ABI_VERSION: u64 = 0;

// Status codes (kept in sync with the design spec and the SDK).
pub const ST_OK: u32 = 0;
pub const ST_BAD_HANDLE: u32 = 1;
pub const ST_WRONG_TYPE: u32 = 2;
pub const ST_ACCESS_DENIED: u32 = 3;
pub const ST_INVALID_ARGS: u32 = 4;
pub const ST_PEER_CLOSED: u32 = 5;
pub const ST_SHOULD_WAIT: u32 = 6;
pub const ST_TIMED_OUT: u32 = 7;
pub const ST_NO_MEMORY: u32 = 8;
pub const ST_BUFFER_TOO_SMALL: u32 = 9;
pub const ST_LIMIT_EXCEEDED: u32 = 10;
pub const ST_NOT_SUPPORTED: u32 = 11;
pub const ST_KILLED: u32 = 12;

pub const SYS_LOG: u64 = 0;
pub const SYS_HANDLE_CLOSE: u64 = 1;
pub const SYS_HANDLE_DUP: u64 = 2;
pub const SYS_CHANNEL_CREATE: u64 = 3;
pub const SYS_CHANNEL_SEND: u64 = 4;
pub const SYS_CHANNEL_RECV: u64 = 5;
pub const SYS_WAIT_MANY: u64 = 6;
pub const SYS_MEMOBJ_CREATE: u64 = 7;
pub const SYS_MEMOBJ_MAP: u64 = 8;
pub const SYS_MEMOBJ_SIZE: u64 = 9;
pub const SYS_PROCESS_EXIT: u64 = 10;
pub const SYS_CLOCK_UPTIME: u64 = 11;
pub const SYS_ABI_VERSION: u64 = 12;

const LOG_MAX: u64 = 4096;

pub fn dispatch(sysno: u64, args: [u64; 6]) -> (u32, u64) {
    match sysno {
        SYS_LOG => sys_log(args[0], args[1]),
        SYS_PROCESS_EXIT => sys_process_exit(args[0]),
        SYS_CLOCK_UPTIME => (ST_OK, crate::arch::timer::uptime_us()),
        SYS_ABI_VERSION => (ST_OK, ABI_VERSION),
        _ => (ST_NOT_SUPPORTED, 0),
    }
}

/// Check a user buffer against the calling thread's address space.
fn user_buf_ok(va: u64, len: u64, write: bool) -> bool {
    let me = crate::sched::current();
    match &me.aspace {
        Some(a) => a.lock().user_buf_ok(va, len, write),
        None => false,
    }
}

fn sys_log(buf: u64, len: u64) -> (u32, u64) {
    if len > LOG_MAX || !user_buf_ok(buf, len, false) {
        return (ST_INVALID_ARGS, 0);
    }
    // The caller's TTBR1 is active and PAN is clear: read directly.
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
    match core::str::from_utf8(bytes) {
        Ok(s) => {
            let id = crate::sched::current_id();
            kprintln!("app[{id}]: {}", s.trim_end_matches('\n'));
            (ST_OK, 0)
        }
        Err(_) => (ST_INVALID_ARGS, 0),
    }
}

fn sys_process_exit(code: u64) -> (u32, u64) {
    let id = crate::sched::current_id();
    kprintln!("app[{id}]: exit({})", code as u32 as i32);
    crate::sched::exit()
}
