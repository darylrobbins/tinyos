//! Thread control block. Shared-mutable fields are atomics; the Context is
//! only ever touched by the CPU that owns the thread at that moment.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use spin::Mutex;

use crate::arch::context::Context;
use crate::arch::paging::AddrSpace;

pub const STACK_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Class {
    Idle = 0,
    Normal = 1,
    Interactive = 2,
    Realtime = 3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum State {
    Ready = 0,
    Running = 1,
    Blocked = 2,
    Exited = 3,
}

pub struct Thread {
    pub id: u32,
    pub name: String,
    pub class: Class,
    pub affinity: u8,
    state: AtomicU8,
    pub kill_pending: AtomicBool,
    pub last_cpu: AtomicU8,
    /// For blocked threads: absolute wake deadline in µs (u64::MAX = none).
    pub wake_deadline: AtomicU64,
    ctx: UnsafeCell<Context>,
    _stack: Option<Box<[u8]>>, // None for adopted boot/AP stacks
    /// The userspace seam: a user thread carries its address space (shared
    /// with the owning process later) and its EL0 entry state. The 64 KiB
    /// `_stack` doubles as the kernel stack traps land on.
    pub aspace: Option<Arc<Mutex<AddrSpace>>>,
    pub user: Option<UserInit>,
    /// Owning process (handle table, exit state) for app threads.
    pub proc: Option<Arc<crate::obj::process::Process>>,
    /// Cached TTBR1 value (root | ASID) for lock-free scheduler activation;
    /// 0 = kernel-only thread.
    pub user_ttbr1: u64,
}

/// EL0 entry state consumed by the user trampoline.
pub struct UserInit {
    pub pc: u64,
    pub sp: u64,
    pub arg: u64,
}

// Safety: `ctx` is only accessed by the CPU switching this thread in or out,
// which the ready-queue/current-slot handoff serializes; the rest is atomic.
unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    pub fn new(id: u32, name: String, class: Class, affinity: u8, entry: fn()) -> Self {
        let stack = alloc::vec![0u8; STACK_SIZE].into_boxed_slice();
        let top = stack.as_ptr() as u64 + STACK_SIZE as u64;
        Self {
            id,
            name,
            class,
            affinity,
            state: AtomicU8::new(State::Ready as u8),
            kill_pending: AtomicBool::new(false),
            last_cpu: AtomicU8::new(0),
            wake_deadline: AtomicU64::new(u64::MAX),
            ctx: UnsafeCell::new(Context::new(top, entry)),
            _stack: Some(stack),
            aspace: None,
            user: None,
            proc: None,
            user_ttbr1: 0,
        }
    }

    /// A thread that enters EL0: same as `new`, but carrying an address
    /// space and entry state for the user trampoline.
    pub fn new_user(
        id: u32,
        name: String,
        class: Class,
        affinity: u8,
        entry: fn(),
        aspace: Arc<Mutex<AddrSpace>>,
        user: UserInit,
        proc: Option<Arc<crate::obj::process::Process>>,
    ) -> Self {
        let user_ttbr1 = aspace.lock().ttbr1();
        let mut t = Self::new(id, name, class, affinity, entry);
        t.aspace = Some(aspace);
        t.user = Some(user);
        t.proc = proc;
        t.user_ttbr1 = user_ttbr1;
        t
    }

    /// TCB for a context that already exists (the boot stack becomes CPU 0's
    /// idle thread; AP boot stacks become theirs).
    pub fn adopt_current(id: u32, name: String, affinity: u8) -> Self {
        Self {
            id,
            name,
            class: Class::Idle,
            affinity,
            state: AtomicU8::new(State::Running as u8),
            kill_pending: AtomicBool::new(false),
            last_cpu: AtomicU8::new(0),
            wake_deadline: AtomicU64::new(u64::MAX),
            ctx: UnsafeCell::new(Context::empty()),
            _stack: None,
            aspace: None,
            user: None,
            proc: None,
            user_ttbr1: 0,
        }
    }

    pub fn state(&self) -> State {
        match self.state.load(Ordering::Acquire) {
            0 => State::Ready,
            1 => State::Running,
            2 => State::Blocked,
            _ => State::Exited,
        }
    }

    pub fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Release);
    }

    pub fn ctx_ptr(&self) -> *mut Context {
        self.ctx.get()
    }
}

/// Read-only view for `ps` and the Monitor.
pub struct ThreadInfo {
    pub id: u32,
    pub name: String,
    pub state: State,
    pub cpu: u8,
    pub class: Class,
}
