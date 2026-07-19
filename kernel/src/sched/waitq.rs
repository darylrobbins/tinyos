//! Wait queues: threads block here until woken (IRQ-driven flag) or their
//! deadline passes. IRQ handlers never touch these structures — they set
//! `arch::irq::WAKE_INPUT`, and drain() (thread context) does the rest.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

use super::thread::{State, Thread};

pub struct WaitQueue {
    waiters: Mutex<Vec<Arc<Thread>>>,
}

pub static INPUT: WaitQueue = WaitQueue::new();
/// Generic timed sleeps (`sched::sleep_us`).
pub static TIMER: WaitQueue = WaitQueue::new();
/// Kernel-object state changes (channel send/close, process exit). One
/// global queue: every waiter re-evaluates its signal set on wake —
/// thundering-herd-but-correct; per-object queues are a later optimization
/// invisible to the ABI.
pub static OBJECTS: WaitQueue = WaitQueue::new();

/// Object-wake sequence number: wake_objects() bumps it BEFORE waking, and
/// an object waiter records it before re-checking signals. If the recorded
/// value is stale by the time the (post-context-save) enqueue happens, a
/// wake ran in the window — the waiter is made ready instead of enqueued.
/// This closes the lost-wakeup race that previously needed a 100 ms
/// poll cap in wait_many.
pub static OBJ_SEQ: AtomicU64 = AtomicU64::new(0);

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: Mutex::new(Vec::new()),
        }
    }

    /// Block the calling thread until wake_all() or `deadline_us` (absolute,
    /// u64::MAX = no deadline). Returns after this thread is next scheduled.
    ///
    /// The thread enters `waiters` only AFTER its context save completes
    /// (via the scheduler handoff) — otherwise another CPU could wake it and
    /// switch into a half-saved context.
    pub fn block_current(&'static self, deadline_us: u64) {
        let me = super::current();
        me.wake_deadline.store(deadline_us, Ordering::Release);
        me.set_state(State::Blocked);
        super::schedule(super::Handoff::Wait(self, me));
    }

    pub(super) fn enqueue_waiter(&self, t: Arc<Thread>) {
        let seq = t.wait_seq.swap(u64::MAX, Ordering::AcqRel);
        if seq != u64::MAX && OBJ_SEQ.load(Ordering::SeqCst) != seq {
            // A wake happened between the signal check and this enqueue:
            // don't sleep on it, run again immediately. Kick other CPUs in
            // case this thread lands on one idling with no near deadline.
            t.wake_deadline.store(u64::MAX, Ordering::Release);
            t.set_state(State::Ready);
            super::READY.lock().push_back(t);
            super::kick_others();
            return;
        }
        self.waiters.lock().push(t);
    }

    pub fn wake_all(&self) {
        let mut ready = super::READY.lock();
        for t in self.waiters.lock().drain(..) {
            t.wake_deadline.store(u64::MAX, Ordering::Release);
            t.set_state(State::Ready);
            ready.push_back(t);
        }
    }

    /// Wake only waiters whose deadline passed (or that are being killed).
    fn wake_expired(&self, now_us: u64) {
        let mut ready = super::READY.lock();
        self.waiters.lock().retain(|t| {
            if t.wake_deadline.load(Ordering::Acquire) <= now_us
                || t.kill_pending.load(Ordering::Acquire)
            {
                t.wake_deadline.store(u64::MAX, Ordering::Release);
                t.set_state(State::Ready);
                ready.push_back(t.clone());
                false
            } else {
                true
            }
        });
    }

    fn earliest_deadline(&self) -> u64 {
        self.waiters
            .lock()
            .iter()
            .map(|t| t.wake_deadline.load(Ordering::Acquire))
            .min()
            .unwrap_or(u64::MAX)
    }
}

pub(super) fn drain(now_us: u64) {
    if crate::arch::irq::WAKE_INPUT.swap(false, Ordering::Acquire) {
        INPUT.wake_all();
    }
    INPUT.wake_expired(now_us);
    TIMER.wake_expired(now_us);
    OBJECTS.wake_expired(now_us);
}

pub(super) fn earliest_deadline() -> u64 {
    INPUT
        .earliest_deadline()
        .min(TIMER.earliest_deadline())
        .min(OBJECTS.earliest_deadline())
}
