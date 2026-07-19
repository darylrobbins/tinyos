//! Raw syscall stubs (ABI v0). `svc #0`; x8 = number, args x0-x5, returns
//! x0 = status, x1 = value.

use core::arch::asm;

pub const ABI_VERSION: u32 = 0;

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

/// A syscall result: status code plus the value register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ret {
    pub status: u32,
    pub value: u64,
}

impl Ret {
    pub fn ok(self) -> Result<u64, u32> {
        if self.status == 0 { Ok(self.value) } else { Err(self.status) }
    }
}

// Status codes.
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

// Signals.
pub const SIG_READABLE: u32 = 1;
pub const SIG_WRITABLE: u32 = 2;
pub const SIG_PEER_CLOSED: u32 = 4;
pub const SIG_EXITED: u32 = 8;

// Handle rights.
pub const RIGHT_READ: u32 = 1;
pub const RIGHT_WRITE: u32 = 2;
pub const RIGHT_DUP: u32 = 4;
pub const RIGHT_TRANSFER: u32 = 8;
pub const RIGHT_MAP: u32 = 16;
pub const RIGHT_WAIT: u32 = 32;
pub const RIGHTS_ALL: u32 = 0x3F;

#[inline]
pub fn syscall6(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> Ret {
    let status: u64;
    let value: u64;
    unsafe {
        asm!(
            "svc #0",
            in("x8") n,
            inout("x0") a0 => status,
            inout("x1") a1 => value,
            in("x2") a2,
            in("x3") a3,
            in("x4") a4,
            in("x5") a5,
            options(nostack),
        );
    }
    Ret { status: status as u32, value }
}

#[inline]
pub fn syscall0(n: u64) -> Ret {
    syscall6(n, 0, 0, 0, 0, 0, 0)
}
#[inline]
pub fn syscall1(n: u64, a0: u64) -> Ret {
    syscall6(n, a0, 0, 0, 0, 0, 0)
}
#[inline]
pub fn syscall2(n: u64, a0: u64, a1: u64) -> Ret {
    syscall6(n, a0, a1, 0, 0, 0, 0)
}
#[inline]
pub fn syscall3(n: u64, a0: u64, a1: u64, a2: u64) -> Ret {
    syscall6(n, a0, a1, a2, 0, 0, 0)
}
