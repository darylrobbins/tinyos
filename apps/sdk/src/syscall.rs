//! Raw syscall stubs (ABI v0). `svc #0`; x8 = number, args x0-x5, returns
//! x0 = status, x1 = value.

use core::arch::asm;

// All ABI constants come from the shared `abi` crate (crates/abi) — the same
// definitions the kernel compiles against, so they cannot drift.
pub use abi::syscall::*;

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
