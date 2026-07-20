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
use thread::{Class, State, Thread, ThreadInfo, UserInit};

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

/// Spawn a thread that enters EL0 at `pc` with `sp`/`arg` once scheduled.
pub fn spawn_user(
    name: String,
    class: Class,
    affinity: u8,
    aspace: Arc<spin::Mutex<crate::arch::paging::AddrSpace>>,
    pc: u64,
    sp: u64,
    arg: u64,
    proc: Option<Arc<crate::obj::process::Process>>,
) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let t = Arc::new(Thread::new_user(
        id,
        name,
        class,
        affinity,
        user_trampoline,
        aspace,
        UserInit { pc, sp, arg },
        proc,
    ));
    THREADS.lock().push(t.clone());
    READY.lock().push_back(t);
    arch::irq::kick_others(cpu_id());
    id
}

/// Kernel-side entry of every user thread: drop to EL0. The scheduler has
/// already activated this thread's TTBR1.
fn user_trampoline() {
    let me = current();
    let u = me.user.as_ref().expect("user thread without UserInit");
    unsafe { arch::user::enter_user(u.pc, u.sp, u.arg) }
}

/// Requeue the current (user) thread from the EL0 preemption path. Unlike
/// `yield_now` this never opens an IRQ service window — we're already in
/// one — and leaves kill handling to the trap tail.
#[allow(dead_code)] // driven by the aarch64 EL0 timer trap; unused on x86
pub fn preempt_from_user() {
    let me = current();
    arch::irq::note_busy(cpu_id());
    schedule(Handoff::Requeue(me));
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

/// IPI the other CPUs to re-check the ready queue. Called after readying a
/// thread that may be destined for a CPU idling in wfi/hlt.
pub fn kick_others() {
    arch::irq::kick_others(cpu_id());
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
    // Make the incoming thread's user address space current before its
    // kernel context resumes: it may be mid-syscall, about to touch user
    // buffers (possibly on a different CPU than it blocked on).
    arch::user::activate(next.user_ttbr1);
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
    // Catch silent kernel-stack overflow at the earliest safe moment: we're
    // on the incoming thread's stack, so a corrupted canary means its lowest
    // page was already written. Panic loudly rather than corrupt the heap
    // further.
    {
        let cur = current();
        if !cur.stack_ok() {
            panic!("kernel stack overflow: thread {} ({})", cur.id, cur.name);
        }
    }
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

// `fn()` isn't a C type, but this is only ever called from our own asm
// trampoline with a Rust fn pointer — the C ABI is what the trampoline speaks,
// not a real FFI boundary.
#[allow(improper_ctypes_definitions)]
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
/// True once the scheduler is running on the boot CPU (threads may block).
pub fn started() -> bool {
    STARTED.load(core::sync::atomic::Ordering::Acquire)
}

static STARTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn start(ui_main: fn()) -> ! {
    STARTED.store(true, core::sync::atomic::Ordering::Release);
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
