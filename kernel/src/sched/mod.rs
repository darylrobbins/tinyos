//! Cooperative SMP scheduler: global ready queue + per-thread affinity.
//! Locks are never held across switch_to; IRQ handlers never touch this
//! module (they only set atomics that drain_wakes() consumes).

pub mod thread;
pub mod waitq;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use spin::Mutex;

use crate::arch::{self, context, cpu_id, MAX_CPUS};
use thread::{Class, State, Thread, ThreadInfo};

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static ONLINE: AtomicUsize = AtomicUsize::new(1);
static UI_ID: AtomicU32 = AtomicU32::new(0);
static THREADS: Mutex<Vec<Arc<Thread>>> = Mutex::new(Vec::new());
pub(crate) static READY: Mutex<VecDeque<Arc<Thread>>> = Mutex::new(VecDeque::new());

/// What the resumed context must do with the thread that ran before it.
pub(crate) enum Handoff {
    None,
    Requeue(Arc<Thread>),
    Drop(Arc<Thread>),
    Wait(&'static waitq::WaitQueue, Arc<Thread>),
}

struct CpuSlot {
    current: Option<Arc<Thread>>,
    idle: Option<Arc<Thread>>,
    handoff: Handoff,
}

const EMPTY_SLOT: Mutex<CpuSlot> = Mutex::new(CpuSlot {
    current: None,
    idle: None,
    handoff: Handoff::None,
});
static CPUS: [Mutex<CpuSlot>; MAX_CPUS] = [EMPTY_SLOT; MAX_CPUS];

pub fn online_cpus() -> usize {
    ONLINE.load(Ordering::Relaxed)
}

/// Id of the UI thread (protected from `kill`).
pub fn ui_thread_id() -> u32 {
    UI_ID.load(Ordering::Relaxed)
}

pub fn spawn(name: String, class: Class, affinity: u8, entry: fn()) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let t = Arc::new(Thread::new(id, name, class, affinity, entry));
    THREADS.lock().push(t.clone());
    READY.lock().push_back(t);
    arch::irq::kick_others(cpu_id());
    id
}

pub fn current() -> Arc<Thread> {
    CPUS[cpu_id()].lock().current.clone().expect("sched not started")
}

pub fn current_id() -> u32 {
    current().id
}

pub fn yield_now() {
    let cpu = cpu_id();
    arch::irq::note_busy(cpu);
    arch::irq::service_irqs();
    let me = current();
    if me.kill_pending.load(Ordering::Acquire) && me.class != Class::Idle {
        exit();
    }
    // Idle threads are per-CPU fallbacks and must NEVER enter READY.
    let handoff = if me.class == Class::Idle {
        Handoff::None
    } else {
        Handoff::Requeue(me)
    };
    schedule(handoff);
}

pub fn exit() -> ! {
    let me = current();
    me.set_state(State::Exited);
    THREADS.lock().retain(|t| t.id != me.id);
    schedule(Handoff::Drop(me));
    unreachable!("resumed an exited thread");
}

pub fn kill(id: u32) -> bool {
    let threads = THREADS.lock();
    if let Some(t) = threads.iter().find(|t| t.id == id) {
        t.kill_pending.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

pub fn snapshot() -> Vec<ThreadInfo> {
    THREADS
        .lock()
        .iter()
        .map(|t| ThreadInfo {
            id: t.id,
            name: t.name.clone(),
            state: t.state(),
            cpu: t.last_cpu.load(Ordering::Relaxed),
            class: t.class,
        })
        .collect()
}

/// Move IRQ-flagged and deadline-expired sleepers to the ready queue.
fn drain_wakes() {
    waitq::drain(arch::timer::uptime_us());
}

/// Block the calling thread for `us` microseconds.
pub fn sleep_us(us: u64) {
    waitq::TIMER.block_current(arch::timer::uptime_us() + us);
}

fn pick_next(cpu: usize) -> Option<Arc<Thread>> {
    let mut q = READY.lock();
    let mask = 1u8 << cpu;
    // Highest class first; FIFO within a class.
    let mut best: Option<(usize, Class)> = None;
    for (i, t) in q.iter().enumerate() {
        if t.affinity & mask == 0 {
            continue;
        }
        if best.map_or(true, |(_, c)| t.class > c) {
            best = Some((i, t.class));
        }
    }
    best.and_then(|(i, _)| q.remove(i))
}

/// Switch this CPU to the best ready thread (or its idle thread).
pub(crate) fn schedule(handoff: Handoff) {
    let cpu = cpu_id();
    drain_wakes();

    let next = {
        let slot = CPUS[cpu].lock();
        let idle = slot.idle.clone().expect("cpu not entered");
        drop(slot);
        match pick_next(cpu) {
            Some(t) => t,
            // Nothing else runnable: a yielding thread just keeps running
            // (switching to idle here would strand it — idle sleeps, and the
            // requeue happens only after the switch). Blockers/exiters must
            // fall through to idle.
            None => match &handoff {
                Handoff::Requeue(_) => return,
                _ => idle,
            },
        }
    };

    let me = current();
    if Arc::ptr_eq(&next, &me) {
        return;
    }

    next.set_state(State::Running);
    next.last_cpu.store(cpu as u8, Ordering::Relaxed);
    let old_ctx = me.ctx_ptr();
    let new_ctx = next.ctx_ptr();
    {
        let mut slot = CPUS[cpu].lock();
        slot.current = Some(next);
        slot.handoff = handoff;
    } // lock dropped before the switch
    unsafe { context::switch_to(old_ctx, new_ctx) };
    // We are back on this thread's stack (possibly on another CPU).
    finish_switch();
}

/// Runs in the newly-switched-to context: retire the previous thread.
/// Only NOW may the previous thread be picked up by another CPU — its
/// context save is complete.
fn finish_switch() {
    let cpu = cpu_id();
    let handoff = core::mem::replace(&mut CPUS[cpu].lock().handoff, Handoff::None);
    match handoff {
        Handoff::None => {}
        Handoff::Requeue(t) => {
            t.set_state(State::Ready);
            READY.lock().push_back(t);
        }
        Handoff::Drop(t) => drop(t), // last Arc frees TCB + stack (not ours)
        Handoff::Wait(q, t) => q.enqueue_waiter(t),
    }
}

#[unsafe(no_mangle)]
extern "C" fn rust_thread_start(entry: fn()) -> ! {
    finish_switch();
    entry();
    exit()
}

/// Deadline the idle loop may sleep to: the earliest blocked-thread wake,
/// else a minute-scale deep-idle tick.
fn idle_deadline() -> u64 {
    let dl = waitq::earliest_deadline();
    if dl != u64::MAX {
        dl
    } else {
        arch::timer::uptime_us() + 60_000_000
    }
}

fn idle_loop(cpu: usize) -> ! {
    let _ = cpu;
    loop {
        // Run anything runnable first; sleep only when the queue is empty.
        yield_now();
        arch::irq::idle_once(idle_deadline());
    }
}

/// BSP entry: adopt the boot stack as CPU 0's idle thread, spawn the UI
/// thread, and start scheduling. Never returns.
pub fn start(ui_main: fn()) -> ! {
    let cpu = cpu_id();
    let idle = Arc::new(Thread::adopt_current(0, alloc::format!("idle{cpu}"), 1 << cpu));
    THREADS.lock().push(idle.clone());
    {
        let mut slot = CPUS[cpu].lock();
        slot.idle = Some(idle.clone());
        slot.current = Some(idle);
    }
    let ui = spawn(String::from("ui"), Class::Interactive, 1 << 0, ui_main);
    UI_ID.store(ui, Ordering::Relaxed);
    idle_loop(cpu)
}

/// AP entry: adopt this AP's boot stack as its idle thread.
pub fn ap_enter(cpu: usize) -> ! {
    let idle = Arc::new(Thread::adopt_current(
        (100 + cpu) as u32,
        alloc::format!("idle{cpu}"),
        1 << cpu,
    ));
    THREADS.lock().push(idle.clone());
    {
        let mut slot = CPUS[cpu].lock();
        slot.idle = Some(idle.clone());
        slot.current = Some(idle);
    }
    ONLINE.fetch_add(1, Ordering::Relaxed);
    idle_loop(cpu)
}
