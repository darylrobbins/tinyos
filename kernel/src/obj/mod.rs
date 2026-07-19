//! Kernel object model: handles, channels, memory objects, processes — the
//! capability layer the app ABI is built on. `syscall` is the EL0 dispatch.

pub mod channel;
pub mod handle;
pub mod loader;
pub mod memobj;
pub mod objtest;
pub mod process;
pub mod procsrv;
pub mod syscall;
pub mod usertest;

use alloc::sync::Arc;

// Signal bits (ABI; defined in the shared abi crate).
pub use abi::syscall::{SIG_EXITED, SIG_PEER_CLOSED, SIG_READABLE, SIG_WRITABLE};

/// Every kernel object a handle can reference.
#[derive(Clone)]
pub enum Object {
    Channel(Arc<channel::ChannelEnd>),
    MemObj(Arc<memobj::MemObj>),
    Process(Arc<process::Process>),
}

impl Object {
    pub fn signals(&self) -> u32 {
        match self {
            Object::Channel(c) => c.signals(),
            Object::MemObj(_) => 0,
            Object::Process(p) => p.signals(),
        }
    }
}

/// Wake every object-waiter to re-evaluate its signal set. Thread context
/// only (all object state changes happen in syscalls or kernel threads).
pub fn wake_objects() {
    use core::sync::atomic::Ordering;
    crate::sched::waitq::OBJ_SEQ.fetch_add(1, Ordering::SeqCst);
    crate::sched::waitq::OBJECTS.wake_all();
    // Readied threads may belong to idle CPUs sitting in wfi/hlt with no
    // near deadline (object waits block indefinitely now that the 100 ms
    // poll cap is gone) — IPI them to re-check the ready queue, as spawn does.
    crate::sched::kick_others();
}

/// Block until any of `sets` has a wanted signal, the (absolute µs)
/// deadline passes, or the caller is killed. Returns OK/TIMED_OUT/KILLED;
/// observed signals are written into each entry's `.1`.
pub fn wait_many(sets: &mut [(Object, u32, u32)], deadline_us: u64) -> u32 {
    use syscall::{ST_KILLED, ST_OK, ST_TIMED_OUT};
    loop {
        // Record the wake sequence BEFORE checking signals: if a wake fires
        // after this point, enqueue_waiter sees the stale value and refuses
        // to sleep — no poll cap needed.
        let seq = crate::sched::waitq::OBJ_SEQ.load(core::sync::atomic::Ordering::SeqCst);
        let mut hit = false;
        for (obj, want, observed) in sets.iter_mut() {
            *observed = obj.signals();
            if *observed & *want != 0 {
                hit = true;
            }
        }
        if hit {
            return ST_OK;
        }
        if crate::arch::timer::uptime_us() >= deadline_us {
            return ST_TIMED_OUT;
        }
        let me = crate::sched::current();
        if me.kill_pending.load(core::sync::atomic::Ordering::Acquire) {
            return ST_KILLED;
        }
        me.wait_seq
            .store(seq, core::sync::atomic::Ordering::Release);
        crate::sched::waitq::OBJECTS.block_current(deadline_us);
    }
}
