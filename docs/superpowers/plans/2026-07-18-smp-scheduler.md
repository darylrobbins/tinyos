# SMP Cooperative Scheduler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring up 4 CPUs on both arches and add cooperative kernel threads (global ready queue + affinity), keeping the desktop responsive while `spin` threads saturate cores 1–3.

**Architecture:** New arch-neutral `kernel/src/sched/` (thread table, ready queue, wait queues, per-CPU idle threads) plus per-arch context switch, AP bring-up (PSCI on aarch64, UEFI MP Services park on x86_64), and IPIs. The existing main loop becomes the UI thread pinned to CPU 0. IRQ handlers stay ack-and-flag; IRQs remain masked except inside the per-CPU idle wait, so no locks are ever taken in IRQ context.

**Tech Stack:** Rust nightly, `aarch64-unknown-uefi` + `x86_64-unknown-uefi`, uefi 0.35 (`proto::pi::mp::MpServices`), spin 0.9, QEMU virt (HVF, GICv3) / q35 (TCG, OVMF).

**Spec:** `docs/superpowers/specs/2026-07-18-smp-scheduler-design.md`

## Global Constraints

- All aarch64 device MMIO goes through `kernel/src/drivers/mmio.rs` accessors (HVF asserts `isv` on register-offset MMIO). Plain volatile is fine on x86_64.
- Cooperative only: threads switch at `yield_now` / block / exit. No timer preemption in this milestone.
- Plain spinlocks only. IRQ handlers never take locks — they ack hardware and set atomics. Never hold a sched lock across `switch_to`.
- 4 CPUs (`-smp 4`); every feature must keep working if AP bring-up fails (single-core fallback with serial warning).
- x86_64 stays on the win64 ABI (`extern "C"` on this target = MS ABI: args in rcx/rdx, shadow space required). All hand-written x86 asm in this plan is written for that ABI.
- Verify each task with `make build` AND `make build ARCH=x86_64` before commit; boot-smoke on aarch64 (HVF) where the task changes runtime behavior.
- Work on branch `smp-scheduler` (create from `main` at start; user consented to branch workflow in prior milestones).

**Boot-smoke recipe** (used by several tasks; run from repo root):

```bash
make build && mkdir -p esp/EFI/BOOT && \
qemu-system-aarch64 -machine virt,gic-version=3 -accel hvf -cpu host -smp 4 -m 512M \
  -drive if=pflash,format=raw,readonly=on,file=fw/edk2-aarch64-code.fd \
  -drive if=pflash,format=raw,file=fw/vars.fd \
  -device ramfb -device qemu-xhci -device virtio-keyboard-pci -device virtio-tablet-pci \
  -drive format=raw,file=fat:rw:esp -display none -serial stdio \
  -qmp unix:/tmp/tinyos-qmp.sock,server,nowait &
sleep 20; kill %1
```

(Adjust flash paths to match the Makefile's actual `FLASH` variable — reuse the Makefile's values verbatim; regenerate `vars.fd` the way `make run` does. Omit `-smp 4` until Task 5 lands. For x86 smoke swap in the Makefile's x86 values and `-machine q35 -vga none`.)

---

### Task 1: `cpu_id()` + per-CPU idle/wake stats

Multi-CPU groundwork: every CPU needs an identity, and the idle/wake statistics in both `irq.rs` files must become per-CPU arrays (the Monitor's per-core gauges read them later).

**Files:**
- Modify: `kernel/src/arch/aarch64/mod.rs` (add `cpu_id`)
- Modify: `kernel/src/arch/x86_64/mod.rs` (add `cpu_id`)
- Modify: `kernel/src/arch/aarch64/irq.rs` (stats arrays)
- Modify: `kernel/src/arch/x86_64/irq.rs` (stats arrays)
- Modify: `kernel/src/apps/monitor.rs:115` (call site)
- Modify: `kernel/src/main.rs:155` (call site)

**Interfaces:**
- Produces: `arch::cpu_id() -> usize` (0-based, < `MAX_CPUS`); `arch::irq::wake_stats(cpu: usize) -> (u32, u32)`; `pub const MAX_CPUS: usize = 4` in each arch `mod.rs`, re-exported through the `arch` facade.
- Consumes: existing `WAKES`/`SLEPT_US`/`WINDOW_START_US`/`LAST_RATE`/`LAST_IDLE_PCT` statics.

- [ ] **Step 1: Add `MAX_CPUS` and `cpu_id()` to both arch mods**

In `kernel/src/arch/aarch64/mod.rs` add:

```rust
pub const MAX_CPUS: usize = 4;

/// 0-based CPU index. On QEMU virt, MPIDR Aff0 is 0..N-1.
pub fn cpu_id() -> usize {
    let mpidr: u64;
    unsafe { asm!("mrs {0}, MPIDR_EL1", out(reg) mpidr) };
    ((mpidr & 0xFF) as usize).min(MAX_CPUS - 1)
}
```

In `kernel/src/arch/x86_64/mod.rs` add:

```rust
pub const MAX_CPUS: usize = 4;

/// 0-based CPU index. On QEMU q35, LAPIC IDs are 0..N-1.
pub fn cpu_id() -> usize {
    let id = unsafe { (0xFEE0_0020usize as *const u32).read_volatile() } >> 24;
    (id as usize).min(MAX_CPUS - 1)
}
```

- [ ] **Step 2: Per-CPU stats in `kernel/src/arch/aarch64/irq.rs`**

Replace the five stats statics and their uses:

```rust
const N: usize = super::MAX_CPUS;
static WAKES: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static SLEPT_US: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static WINDOW_START_US: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static LAST_RATE: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static LAST_IDLE_PCT: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
```

Index every access with `let cpu = super::cpu_id();` inside `sleep_until`, `update_stats` (take `cpu: usize` param), and `init` (initialize `WINDOW_START_US[0]`). Change the getter:

```rust
/// (wakes per second, idle percent) for one CPU over its last ~1s window.
pub fn wake_stats(cpu: usize) -> (u32, u32) {
    (
        LAST_RATE[cpu].load(Ordering::Relaxed),
        LAST_IDLE_PCT[cpu].load(Ordering::Relaxed),
    )
}
```

- [ ] **Step 3: Same change in `kernel/src/arch/x86_64/irq.rs`** (identical shape; `const N: usize = super::MAX_CPUS;`).

- [ ] **Step 4: Fix call sites**

`kernel/src/main.rs:155`: `let (wakes, idle) = arch::irq::wake_stats(0);`
`kernel/src/apps/monitor.rs:115`: `let (_wakes, idle_pct) = crate::arch::irq::wake_stats(0);`

- [ ] **Step 5: Build both arches**

Run: `make build && make build ARCH=x86_64` — Expected: both succeed.

- [ ] **Step 6: Boot smoke (aarch64, no `-smp` yet)** — serial still shows `wakes/s=… idle=…` heartbeats.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m 'smp: cpu_id() and per-CPU idle/wake stats'
```

---

### Task 2: Arch context switch (`Context`, `switch_to`, thread trampoline)

**Files:**
- Create: `kernel/src/arch/aarch64/context.rs`
- Create: `kernel/src/arch/x86_64/context.rs`
- Modify: `kernel/src/arch/aarch64/mod.rs` (add `pub mod context;`)
- Modify: `kernel/src/arch/x86_64/mod.rs` (add `pub mod context;`)

**Interfaces:**
- Produces: `arch::context::Context` (`#[repr(C)]`, `Context::new(stack_top: u64, entry: fn()) -> Context`, `Context::empty() -> Context`); `arch::context::switch_to(old: *mut Context, new: *const Context)` (unsafe); both arches call `sched::rust_thread_start(entry: fn()) -> !` on first entry.
- Consumes: nothing. (`rust_thread_start` is defined in Task 3; this task only builds once Task 3's stub exists — implement Tasks 2 and 3 on the same branch and build after both, or add a temporary `#[unsafe(no_mangle)] extern "C" fn rust_thread_start(_: fn()) -> ! { loop {} }` in the context file and delete it in Task 3.)

- [ ] **Step 1: aarch64 context**

Create `kernel/src/arch/aarch64/context.rs`:

```rust
//! Callee-saved register context + cooperative switch. IRQs are masked
//! everywhere outside the idle wait, so switches are never interrupted.

use core::arch::global_asm;

/// AAPCS64 callee-saved state. Field order is baked into the asm below.
#[repr(C)]
pub struct Context {
    pub sp: u64,        // 0x00
    pub x19: u64,       // 0x08  (holds the entry fn for new threads)
    pub x20: u64,       // 0x10
    pub x21: u64,       // 0x18
    pub x22: u64,       // 0x20
    pub x23: u64,       // 0x28
    pub x24: u64,       // 0x30
    pub x25: u64,       // 0x38
    pub x26: u64,       // 0x40
    pub x27: u64,       // 0x48
    pub x28: u64,       // 0x50
    pub x29: u64,       // 0x58
    pub x30: u64,       // 0x60  (resume address; `ret` target)
}

impl Context {
    pub fn empty() -> Self {
        unsafe { core::mem::zeroed() }
    }

    /// A context that, when switched to, calls `sched::rust_thread_start(entry)`
    /// on the given stack.
    pub fn new(stack_top: u64, entry: fn()) -> Self {
        let mut c = Self::empty();
        c.sp = stack_top & !0xF; // AAPCS: 16-byte aligned
        c.x19 = entry as usize as u64;
        c.x30 = thread_trampoline as usize as u64;
        c
    }
}

unsafe extern "C" {
    fn thread_trampoline();
    /// switch_to(old: *mut Context, new: *const Context)
    pub fn switch_to(old: *mut Context, new: *const Context);
}

global_asm!(
    r#"
.global switch_to
switch_to:
    mov x9, sp
    str x9,  [x0, #0x00]
    stp x19, x20, [x0, #0x08]
    stp x21, x22, [x0, #0x18]
    stp x23, x24, [x0, #0x28]
    stp x25, x26, [x0, #0x38]
    stp x27, x28, [x0, #0x48]
    stp x29, x30, [x0, #0x58]
    ldr x9,  [x1, #0x00]
    mov sp, x9
    ldp x19, x20, [x1, #0x08]
    ldp x21, x22, [x1, #0x18]
    ldp x23, x24, [x1, #0x28]
    ldp x25, x26, [x1, #0x38]
    ldp x27, x28, [x1, #0x48]
    ldp x29, x30, [x1, #0x58]
    ret

.global thread_trampoline
thread_trampoline:
    mov x0, x19
    bl rust_thread_start
"#
);
```

- [ ] **Step 2: x86_64 context**

Create `kernel/src/arch/x86_64/context.rs`:

```rust
//! Callee-saved context + cooperative switch, written for the uefi target's
//! MS x64 ABI (extern "C" args in rcx/rdx; calls need 32 bytes shadow space).

use core::arch::global_asm;

#[repr(C)]
pub struct Context {
    pub rsp: u64,       // 0x00
    pub rbx: u64,       // 0x08
    pub rbp: u64,       // 0x10
    pub r12: u64,       // 0x18  (holds the entry fn for new threads)
    pub r13: u64,       // 0x20
    pub r14: u64,       // 0x28
    pub r15: u64,       // 0x30
    pub rdi: u64,       // 0x38  (callee-saved in MS ABI)
    pub rsi: u64,       // 0x40  (callee-saved in MS ABI)
}

impl Context {
    pub fn empty() -> Self {
        unsafe { core::mem::zeroed() }
    }

    pub fn new(stack_top: u64, entry: fn()) -> Self {
        let mut c = Self::empty();
        // Push the trampoline as the `ret` target of the first switch_to.
        // Stack top is 16-aligned; after the ret pops 8, a `call` inside the
        // trampoline realigns per the ABI (call pushes 8 more).
        let sp = (stack_top & !0xF) - 8;
        unsafe { (sp as *mut u64).write(thread_trampoline as usize as u64) };
        c.rsp = sp;
        c.r12 = entry as usize as u64;
        c
    }
}

unsafe extern "C" {
    fn thread_trampoline();
    /// switch_to(old: *mut Context, new: *const Context) — MS ABI: rcx, rdx.
    pub fn switch_to(old: *mut Context, new: *const Context);
}

global_asm!(
    r#"
.global switch_to
switch_to:
    mov [rcx + 0x00], rsp
    mov [rcx + 0x08], rbx
    mov [rcx + 0x10], rbp
    mov [rcx + 0x18], r12
    mov [rcx + 0x20], r13
    mov [rcx + 0x28], r14
    mov [rcx + 0x30], r15
    mov [rcx + 0x38], rdi
    mov [rcx + 0x40], rsi
    mov rsp, [rdx + 0x00]
    mov rbx, [rdx + 0x08]
    mov rbp, [rdx + 0x10]
    mov r12, [rdx + 0x18]
    mov r13, [rdx + 0x20]
    mov r14, [rdx + 0x28]
    mov r15, [rdx + 0x30]
    mov rdi, [rdx + 0x38]
    mov rsi, [rdx + 0x40]
    ret

.global thread_trampoline
thread_trampoline:
    mov rcx, r12
    sub rsp, 40
    call rust_thread_start
"#
);
```

- [ ] **Step 3: Register the modules** — add `pub mod context;` to both arch `mod.rs` files.

- [ ] **Step 4: Build gate** — deferred to Task 3 Step 6 (needs `rust_thread_start`). If committing separately, add the temporary stub noted in Interfaces, build both arches, then commit:

```bash
git add -A && git commit -m 'smp: per-arch callee-saved context switch'
```

---

### Task 3: Scheduler core — threads, ready queue, idle threads, UI thread

The big one: `sched/` lands and `main.rs`'s loop moves into a UI thread. Still single-CPU after this task (APs come in Tasks 5–6), but multiple kernel threads already run cooperatively on CPU 0.

**Files:**
- Create: `kernel/src/sched/mod.rs`
- Create: `kernel/src/sched/thread.rs`
- Modify: `kernel/src/main.rs` (loop → UI thread; add `mod sched;`)
- Modify: `kernel/src/arch/*/context.rs` (delete temp stub if added)

**Interfaces:**
- Produces:
  - `sched::spawn(name: String, class: Class, affinity: u8, entry: fn()) -> u32`
  - `sched::yield_now()`, `sched::exit() -> !`, `sched::current_id() -> u32`
  - `sched::start(ui_main: fn()) -> !` (BSP; never returns), `sched::ap_enter(cpu: usize) -> !` (Tasks 5–6)
  - `sched::kill(id: u32) -> bool`, `sched::snapshot() -> Vec<ThreadInfo>`, `sched::online_cpus() -> usize`
  - `#[unsafe(no_mangle)] extern "C" fn rust_thread_start(entry: fn()) -> !`
  - `sched::thread::{Class, ThreadInfo}` with `Class::{Realtime, Interactive, Normal, Idle}`
- Consumes: `arch::context::{Context, switch_to}`, `arch::{cpu_id, MAX_CPUS}`, `arch::irq::sleep_until` (idle loop keeps today's behavior until Task 4 refines it).

- [ ] **Step 1: `kernel/src/sched/thread.rs`**

```rust
//! Thread control block. Shared-mutable fields are atomics; the Context is
//! only ever touched by the CPU that owns the thread at that moment.

use alloc::boxed::Box;
use alloc::string::String;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use crate::arch::context::Context;

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
    _stack: Option<Box<[u8]>>, // None for the boot/idle-0 bootstrap thread
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
        }
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
```

- [ ] **Step 2: `kernel/src/sched/mod.rs`**

```rust
//! Cooperative SMP scheduler: global ready queue + per-thread affinity.
//! Locks are never held across switch_to; IRQ handlers never touch this
//! module (they only set atomics that drain_wakes() consumes).

pub mod thread;
pub mod waitq; // created in Task 4; omit this line until then

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
static THREADS: Mutex<Vec<Arc<Thread>>> = Mutex::new(Vec::new());
static READY: Mutex<VecDeque<Arc<Thread>>> = Mutex::new(VecDeque::new());

/// What the resumed context must do with the thread that ran before it.
enum Handoff {
    None,
    Requeue(Arc<Thread>),
    Drop(Arc<Thread>),
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

pub fn cpu_online(_cpu: usize) {
    ONLINE.fetch_add(1, Ordering::Relaxed);
}

pub fn spawn(name: String, class: Class, affinity: u8, entry: fn()) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let t = Arc::new(Thread::new(id, name, class, affinity, entry));
    THREADS.lock().push(t.clone());
    READY.lock().push_back(t);
    arch::irq::kick_others(cpu_id()); // no-op until Tasks 5-6 add IPIs
    id
}

pub fn current() -> Arc<Thread> {
    CPUS[cpu_id()].lock().current.clone().expect("sched not started")
}

pub fn current_id() -> u32 {
    current().id
}

pub fn yield_now() {
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
/// Until Task 4 adds wait queues this is a no-op.
fn drain_wakes() {
    // Task 4 fills this in (input wait queue + timer deadlines).
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
fn schedule(handoff: Handoff) {
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
        // Nothing better to do; if we were requeueing ourselves, just keep going.
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
    }
}

#[unsafe(no_mangle)]
extern "C" fn rust_thread_start(entry: fn()) -> ! {
    finish_switch();
    entry();
    exit()
}

/// Deadline the idle loop may sleep to. Task 4 folds in blocked-thread
/// deadlines; until then, a coarse 500 ms tick keeps the CPU responsive to
/// newly spawned threads.
fn idle_deadline() -> u64 {
    arch::timer::uptime_us() + 500_000
}

fn idle_loop(cpu: usize) -> ! {
    loop {
        arch::irq::sleep_until(idle_deadline());
        let _ = cpu;
        yield_now();
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
    spawn(String::from("ui"), Class::Interactive, 1 << 0, ui_main);
    idle_loop(cpu)
}

/// AP entry (Tasks 5-6): adopt this AP's boot stack as its idle thread.
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
    cpu_online(cpu);
    idle_loop(cpu)
}
```

Also add to both `arch/*/irq.rs` a stub (real IPIs land in Tasks 5–6):

```rust
/// Wake other CPUs so they notice new ready threads. IPI lands in the SMP task.
pub fn kick_others(_from: usize) {}
```

Note `idle` threads never enter `READY`; `pick_next` returning `None` falls back to the slot's idle thread. `Thread::adopt_current` ids: 0 for CPU0, 100+cpu for APs — cosmetic only, uniqueness is what matters.

- [ ] **Step 3: Move the main loop into the UI thread (`kernel/src/main.rs`)**

Add `mod sched;` to the module list. Replace everything in `kmain` from `let mut input = drivers::input::Input::init();` to the end of the `loop` with:

```rust
    let mut input = drivers::input::Input::init();
    arch::irq::init();
    kprintln!("tinyos: starting scheduler on cpu{}", arch::cpu_id());

    // Hand the UI's owned state to the thread through statics: the UI thread
    // is the sole consumer, spin::Once is just the safe transport.
    UI_STATE.call_once(|| spin::Mutex::new(Some((fb, surface, fonts, input))));
    sched::start(ui_thread_main)
```

Add above `kmain`:

```rust
type UiState = (
    FbInfo,
    gfx::surface::Surface,
    gfx::font::Fonts,
    drivers::input::Input,
);
static UI_STATE: spin::Once<spin::Mutex<Option<UiState>>> = spin::Once::new();

fn ui_thread_main() {
    let (fb, mut surface, mut fonts, mut input) =
        UI_STATE.get().unwrap().lock().take().expect("ui state");
    let mut shell = ui::shell::Shell::new(fb.width, fb.height);
    kprintln!("tinyos: shell up");

    let mut events = alloc::vec::Vec::new();
    let mut deadline = 0u64;
    let mut last_log_us = 0u64;
    loop {
        events.clear();
        input.poll(&mut events);
        let now = arch::timer::uptime_us();
        let frame_due = now >= deadline;
        shell.handle(&events);
        shell.stats_tick(events.len() as u32);

        if !events.is_empty() || frame_due {
            shell.compose(&mut surface, &mut fonts);
            surface.present(&fb);
        }

        if now.saturating_sub(last_log_us) >= 5_000_000 {
            let (wakes, idle) = arch::irq::wake_stats(0);
            kprintln!("tinyos: wakes/s={wakes} idle={idle}%");
            last_log_us = now;
        }

        deadline = shell.next_deadline(now);
        // Task 4 replaces this with a blocking wait on the input wait queue.
        arch::irq::sleep_until(deadline);
        sched::yield_now();
    }
}
```

(`kmain`'s `-> !` still holds: `sched::start` never returns. The interim `sleep_until` in the UI thread means CPU 0's idle thread rarely runs until Task 4 — that's fine; correctness first.)

Delete the temporary `rust_thread_start` stub from Task 2 if it was added. Comment out `pub mod waitq;` in `sched/mod.rs` until Task 4.

- [ ] **Step 4: Build both arches** — `make build && make build ARCH=x86_64`.

- [ ] **Step 5: Boot smoke with a witness thread**

Temporarily add to `ui_thread_main` before the loop:

```rust
sched::spawn(alloc::string::String::from("hello"), sched::thread::Class::Normal, 1 << 0, || {
    kprintln!("tinyos: hello from thread {}", sched::current_id());
});
```

Boot (aarch64 recipe, still no `-smp`). Expected serial: `hello from thread 2`, shell heartbeat continues, desktop renders. Then delete the witness spawn.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m 'sched: cooperative threads, global ready queue, UI thread'
```

---

### Task 4: Wait queues + truly-idle UI blocking

Replace the UI thread's `sleep_until` with blocking on an input wait queue, and teach the idle loop to sleep until the earliest blocked-thread deadline. Restores the deep-idle behavior (wakes/s≈0) under the scheduler.

**Files:**
- Create: `kernel/src/sched/waitq.rs`
- Modify: `kernel/src/sched/mod.rs` (`drain_wakes`, `idle_deadline`, uncomment `pub mod waitq;`)
- Modify: `kernel/src/main.rs` (UI thread blocks on `waitq::INPUT`)

**Interfaces:**
- Produces: `sched::waitq::WaitQueue` with `block_current(&self, deadline_us: u64)` and `wake_all(&self)`; `pub static INPUT: WaitQueue`.
- Consumes: `arch::irq::WAKE_INPUT` (existing atomic, still set by IRQ handlers), `sched::{schedule, Handoff}` internals.

- [ ] **Step 1: `kernel/src/sched/waitq.rs`**

```rust
//! Wait queues: threads block here until woken (IRQ-driven flag) or their
//! deadline passes. IRQ handlers never touch these structures — they set
//! `arch::irq::WAKE_INPUT`, and drain_wakes() (thread context) does the rest.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use spin::Mutex;

use super::thread::{State, Thread};

pub struct WaitQueue {
    waiters: Mutex<Vec<Arc<Thread>>>,
}

pub static INPUT: WaitQueue = WaitQueue::new();
/// Generic timed sleeps (`sched::sleep_us`).
pub static TIMER: WaitQueue = WaitQueue::new();

impl WaitQueue {
    pub const fn new() -> Self {
        Self { waiters: Mutex::new(Vec::new()) }
    }

    /// Block the calling thread until wake_all() or `deadline_us` (absolute,
    /// u64::MAX = no deadline). Returns after this thread is next scheduled.
    pub fn block_current(&self, deadline_us: u64) {
        let me = super::current();
        me.wake_deadline.store(deadline_us, Ordering::Release);
        me.set_state(State::Blocked);
        self.waiters.lock().push(me);
        super::schedule(super::Handoff::None);
    }

    pub fn wake_all(&self) {
        let mut ready = super::READY.lock();
        for t in self.waiters.lock().drain(..) {
            t.wake_deadline.store(u64::MAX, Ordering::Release);
            t.set_state(State::Ready);
            ready.push_back(t);
        }
    }

    /// Wake only waiters whose deadline has passed; used by drain_wakes().
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
}

pub(super) fn earliest_deadline() -> u64 {
    INPUT.earliest_deadline().min(TIMER.earliest_deadline())
}
```

- [ ] **Step 2: Wire into `sched/mod.rs`**

Uncomment `pub mod waitq;`. Make `schedule` and `Handoff` `pub(crate)` (waitq calls them). Make `READY` `pub(super)`-visible to waitq (same module tree: change to `pub(crate) static READY`). Replace the stubs:

```rust
fn drain_wakes() {
    waitq::drain(arch::timer::uptime_us());
}

fn idle_deadline() -> u64 {
    let dl = waitq::earliest_deadline();
    if dl != u64::MAX {
        dl
    } else {
        arch::timer::uptime_us() + 60_000_000 // deep idle: minute-scale tick
    }
}
```

Also add the convenience used by `spin` threads later:

```rust
pub fn sleep_us(us: u64) {
    waitq::TIMER.block_current(arch::timer::uptime_us() + us);
}
```

- [ ] **Step 3: UI thread blocks instead of sleeping**

In `ui_thread_main`, replace

```rust
        arch::irq::sleep_until(deadline);
        sched::yield_now();
```

with

```rust
        sched::waitq::INPUT.block_current(deadline);
```

- [ ] **Step 4: Build both arches**, boot smoke (aarch64): shell responsive to typing, heartbeat shows deep idle again (`wakes/s=0 idle=9x%` once the desktop settles; the clock-minute deadline logic in `shell.next_deadline` still governs the UI's own wakeups).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m 'sched: wait queues; UI thread blocks on input, idle restored'
```

---

### Task 5: aarch64 SMP — PSCI CPU_ON, per-CPU GIC, SGI IPIs

**Files:**
- Create: `kernel/src/arch/aarch64/smp.rs`
- Modify: `kernel/src/arch/aarch64/gic.rs` (split per-CPU init; SGI enable + send)
- Modify: `kernel/src/arch/aarch64/irq.rs` (`kick_others`, `init_ap`)
- Modify: `kernel/src/arch/aarch64/mod.rs` (`pub mod smp;`)
- Modify: `kernel/src/main.rs` (call `arch::smp::start_secondary_cpus()` after `sched` is ready to accept APs — see step 5)
- Modify: `Makefile` (add `-smp 4` to the aarch64 `MACHINE` line)

**Interfaces:**
- Produces: `arch::smp::start_secondary_cpus()` (bring up CPUs 1..MAX_CPUS; logs per-CPU success/failure); `gic::init_cpu(cpu)`; `gic::send_sgi(cpu, sgi)`; real `irq::kick_others(from)`.
- Consumes: `sched::ap_enter(cpu)`, `arch::context` unused here, `drivers::mmio` accessors, existing `gic::{init, enable_spi}` refactored.

- [ ] **Step 1: Refactor `gic.rs` for per-CPU redistributors**

Replace `init()` with:

```rust
const GICR_STRIDE: usize = 0x2_0000;

fn gicr_base(cpu: usize) -> usize {
    GICR + cpu * GICR_STRIDE
}

pub fn init() {
    // Distributor: enable group-1 non-secure + affinity routing.
    mmio::w32(GICD + GICD_CTLR, 0b10 | (1 << 4)); // EnableGrp1NS | ARE_NS
    init_cpu(0);
}

/// Per-CPU redistributor + CPU-interface init. Called on the CPU itself
/// (the ICC_* system registers are banked per CPU).
pub fn init_cpu(cpu: usize) {
    let r = gicr_base(cpu);
    let waker = mmio::r32(r + GICR_WAKER);
    mmio::w32(r + GICR_WAKER, waker & !(1 << 1)); // clear ProcessorSleep
    while mmio::r32(r + GICR_WAKER) & (1 << 2) != 0 {} // ChildrenAsleep

    // Virtual-timer PPI (27) + SGI 0 (our IPI).
    mmio::w32(r + GICR_ISENABLER0, (1 << 27) | (1 << 0));

    unsafe {
        asm!("msr ICC_SRE_EL1, {0:x}", in(reg) 1u64);
        asm!("isb");
        asm!("msr ICC_PMR_EL1, {0:x}", in(reg) 0xFFu64);
        asm!("msr ICC_IGRPEN1_EL1, {0:x}", in(reg) 1u64);
        asm!("isb");
    }
}

/// Send SGI `sgi` to a single CPU (Aff0 = cpu, Aff1..3 = 0 on QEMU virt).
pub fn send_sgi(cpu: usize, sgi: u32) {
    let val: u64 = ((sgi as u64) << 24) | (1u64 << (cpu & 0xF));
    unsafe {
        asm!("msr ICC_SGI1R_EL1, {0}", in(reg) val);
        asm!("isb");
    }
}
```

- [ ] **Step 2: `kernel/src/arch/aarch64/smp.rs`**

```rust
//! Secondary-CPU bring-up via PSCI. QEMU virt exposes PSCI with an HVC
//! conduit when running under HVF/KVM (guest at EL1). Under TCG the conduit
//! is SMC and the HVC below would take an undefined-instruction exception —
//! acceptable: arm runs HVF-accelerated per the Makefile.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::MAX_CPUS;

const PSCI_VERSION: u32 = 0x8400_0000;
const PSCI_CPU_ON64: u32 = 0xC400_0003;

/// Everything an AP needs before it can run Rust. Written by the BSP,
/// cache-cleaned to PoC, read by the AP with the MMU still off.
#[repr(C, align(64))]
struct ApBoot {
    stack_top: u64, // 0x00
    ttbr0: u64,     // 0x08
    mair: u64,      // 0x10
    tcr: u64,       // 0x18
    sctlr: u64,     // 0x20
    cpu: u64,       // 0x28
}

static AP_ONLINE: AtomicU32 = AtomicU32::new(0);

fn psci_call(func: u32, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!(
            "hvc #0",
            inout("x0") func as u64 => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            clobber_abi("C"),
        );
    }
    ret
}

global_asm!(
    r#"
// x0 = &ApBoot. MMU/caches off; turn them on with the BSP's exact config
// (identity-mapped UEFI tables), then jump to Rust on the new stack.
.global ap_entry
ap_entry:
    ldr x1, [x0, #0x08]
    msr ttbr0_el1, x1
    ldr x1, [x0, #0x10]
    msr mair_el1, x1
    ldr x1, [x0, #0x18]
    msr tcr_el1, x1
    isb
    ldr x1, [x0, #0x20]
    msr sctlr_el1, x1
    isb
    ldr x1, [x0, #0x00]
    mov sp, x1
    bl ap_main
"#
);

unsafe extern "C" {
    fn ap_entry();
}

#[unsafe(no_mangle)]
extern "C" fn ap_main(boot: &'static ApBoot) -> ! {
    let cpu = boot.cpu as usize;
    super::exceptions::install();
    super::gic::init_cpu(cpu);
    AP_ONLINE.fetch_add(1, Ordering::Release);
    kprintln!("tinyos: cpu{cpu} online");
    crate::sched::ap_enter(cpu)
}

/// Clean a range to PoC so MMU-off APs see it.
fn clean_dcache(addr: usize, len: usize) {
    let mut a = addr & !63;
    while a < addr + len {
        unsafe { asm!("dc cvac, {0}", in(reg) a) };
        a += 64;
    }
    unsafe { asm!("dsb sy") };
}

pub fn start_secondary_cpus() {
    let ver = psci_call(PSCI_VERSION, 0, 0, 0);
    if ver <= 0 {
        kprintln!("tinyos: psci unavailable ({ver}), staying single-core");
        return;
    }
    kprintln!("tinyos: psci v{}.{}", (ver >> 16) & 0xFFFF, ver & 0xFFFF);

    for cpu in 1..MAX_CPUS {
        let stack = alloc::vec![0u8; crate::sched::thread::STACK_SIZE].into_boxed_slice();
        let boot = alloc::boxed::Box::leak(alloc::boxed::Box::new(ApBoot {
            stack_top: (stack.as_ptr() as u64 + stack.len() as u64) & !0xF,
            ttbr0: read_sysreg!("ttbr0_el1"),
            mair: read_sysreg!("mair_el1"),
            tcr: read_sysreg!("tcr_el1"),
            sctlr: read_sysreg!("sctlr_el1"),
            cpu: cpu as u64,
        }));
        core::mem::forget(stack); // AP owns it forever (idle stack)
        clean_dcache(boot as *const ApBoot as usize, core::mem::size_of::<ApBoot>());
        clean_dcache(ap_entry as usize, 128);

        let ret = psci_call(
            PSCI_CPU_ON64,
            cpu as u64, // target MPIDR: Aff0 = cpu on virt
            ap_entry as usize as u64,
            boot as *const ApBoot as u64,
        );
        if ret != 0 {
            kprintln!("tinyos: cpu{cpu} CPU_ON failed ({ret})");
        }
    }

    // Give stragglers a moment, then report.
    let t0 = super::timer::uptime_us();
    while (AP_ONLINE.load(Ordering::Acquire) as usize) < MAX_CPUS - 1
        && super::timer::uptime_us() - t0 < 500_000
    {
        core::hint::spin_loop();
    }
    kprintln!(
        "tinyos: {} of {} cpus online",
        1 + AP_ONLINE.load(Ordering::Acquire),
        MAX_CPUS
    );
}
```

Add a tiny macro at the top of the file (or as inline fns):

```rust
macro_rules! read_sysreg {
    ($name:literal) => {{
        let v: u64;
        unsafe { core::arch::asm!(concat!("mrs {0}, ", $name), out(reg) v) };
        v
    }};
}
```

Add `pub mod smp;` to `arch/aarch64/mod.rs`.

- [ ] **Step 3: Real IPIs in `arch/aarch64/irq.rs`**

Replace the `kick_others` stub:

```rust
/// Poke every other online CPU out of wfi so it re-runs its scheduler pass.
pub fn kick_others(from: usize) {
    for cpu in 0..super::MAX_CPUS {
        if cpu != from {
            super::gic::send_sgi(cpu, 0);
        }
    }
}
```

In `irq_entry`'s match, document the SGI arm (behavior is already correct — ack+eoi is the wake):

```rust
            0 => {} // SGI 0: IPI, wake-only
```

- [ ] **Step 4: Makefile** — change line 12 to `MACHINE     := -machine virt,gic-version=3 -smp 4`.

- [ ] **Step 5: Call it from `kmain`**

APs call `sched::ap_enter` immediately, so the scheduler must be able to accept them the moment CPU_ON fires. Restructure the end of `kmain`: move AP start-up *into the UI thread's first moments* — top of `ui_thread_main`, before the loop:

```rust
    #[cfg(target_arch = "aarch64")]
    crate::arch::smp::start_secondary_cpus();
```

(The UI thread runs after `sched::start` has initialized CPU 0's slot; `ap_enter` only touches its own slot + shared queues, so this ordering is safe on both arches.)

- [ ] **Step 6: Build both arches; boot smoke with `-smp 4` (aarch64)**

Expected serial: `psci v1.x`, `cpu1 online` … `cpu3 online`, `4 of 4 cpus online`, shell up, deep idle heartbeat still ~0 wakes/s (all four CPUs sleeping in wfi). Type into the terminal to confirm input still works.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m 'smp: aarch64 PSCI bring-up, per-CPU GIC, SGI IPIs'
```

---

### Task 6: x86_64 SMP — MP Services park, per-AP LAPIC, IPI vector

**Known risk (verify explicitly):** EDK2's MpInitLib registers an ExitBootServices callback that relocates APs into its own wait loop — depending on OVMF's AP loop mode it may INIT-SIPI the APs, stealing our parked cores. Step 6 verifies empirically. If APs die at exit: log, fall back to single-core (spec-sanctioned), and surface the finding to the user — do NOT silently ship a hang.

**Files:**
- Create: `kernel/src/arch/x86_64/smp.rs`
- Modify: `kernel/src/arch/x86_64/apic.rs` (`init_ap`, `send_ipi`, `VEC_IPI`)
- Modify: `kernel/src/arch/x86_64/exceptions.rs` (IPI gate; `load()` for APs)
- Modify: `kernel/src/arch/x86_64/irq.rs` (`kick_others`)
- Modify: `kernel/src/arch/x86_64/mod.rs` (`pub mod smp;`)
- Modify: `kernel/src/main.rs` (pre-exit park + post-sched release)
- Modify: `Makefile` (add `-smp 4` to the x86 `MACHINE` line)

**Interfaces:**
- Produces: `arch::smp::park_aps()` (call while boot services are live; x86 only), `arch::smp::start_secondary_cpus()` (releases the pen; logs outcome), `apic::{VEC_IPI, init_ap, send_ipi}`, `exceptions::load()`.
- Consumes: `sched::ap_enter(cpu)`, `uefi::proto::pi::mp::MpServices`, `boot::{allocate_pages, create_event, get_handle_for_protocol, open_protocol_exclusive}`.

- [ ] **Step 1: APIC additions (`apic.rs`)**

```rust
pub const VEC_IPI: u8 = 50;
const ICR_LO: usize = 0x300;
const ICR_HI: usize = 0x310;

/// Per-AP LAPIC setup: software-enable + timer divider. Calibration is
/// shared — all LAPIC timers run off the same crystal.
pub fn init_ap() {
    lapic_w(SVR, 0x100 | VEC_SPURIOUS as u32);
    lapic_w(TIMER_DIV, 0x3);
    lapic_w(LVT_TIMER, 1 << 16);
}

/// Fixed-delivery IPI to one CPU (LAPIC ID == cpu index on QEMU).
pub fn send_ipi(cpu: usize, vector: u8) {
    lapic_w(ICR_HI, (cpu as u32) << 24);
    lapic_w(ICR_LO, vector as u32);
    while lapic_r(ICR_LO) & (1 << 12) != 0 {} // wait for delivery
}
```

- [ ] **Step 2: IDT — IPI gate + AP loader (`exceptions.rs`)**

In `install()` next to the other gates:

```rust
    extern "x86-interrupt" fn ipi_gate(_f: StackFrame) {
        super::apic::eoi(); // wake-only
    }
```

and in the unsafe block: `idt[super::apic::VEC_IPI as usize].set(ipi_gate as u64, cs);`

Add an AP-side loader that reuses the BSP-filled table:

```rust
/// Load the already-populated IDT on an AP.
pub fn load() {
    unsafe {
        let idt = &raw const IDT;
        let idtr = Idtr {
            limit: (size_of::<[Entry; 256]>() - 1) as u16,
            base: idt as u64,
        };
        asm!("lidt [{0}]", in(reg) &idtr);
    }
}
```

- [ ] **Step 3: `kernel/src/arch/x86_64/smp.rs`**

```rust
//! AP bring-up via UEFI MP Services: APs are launched into a park loop
//! before exit_boot_services (firmware does INIT-SIPI for us) and released
//! into the scheduler after the kernel owns the machine.
//!
//! Risk note: EDK2 may reclaim APs at ExitBootServices depending on its AP
//! loop mode. start_secondary_cpus() therefore treats "no AP checked in
//! within 500 ms" as bring-up failure and continues single-core.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use uefi::boot::{self, AllocateType, MemoryType};
use uefi::proto::pi::mp::MpServices;

use crate::arch::MAX_CPUS;

const AP_STACK_SIZE: usize = 64 * 1024;

static RELEASED: AtomicBool = AtomicBool::new(false);
static PARKED: AtomicU32 = AtomicU32::new(0);
static AP_ONLINE: AtomicU32 = AtomicU32::new(0);
/// Stack tops for cpus 1..MAX_CPUS, filled by park_aps().
static mut AP_STACKS: [u64; MAX_CPUS] = [0; MAX_CPUS];

extern "efiapi" fn ap_park(_arg: *mut c_void) {
    let cpu = super::cpu_id();
    PARKED.fetch_add(1, Ordering::Release);
    while !RELEASED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    // Firmware stack is dead to us after exit_boot_services: switch to ours.
    unsafe {
        let stack = AP_STACKS[cpu];
        core::arch::asm!(
            "mov rsp, {0}",
            "mov rcx, {1}",
            "sub rsp, 40",
            "call ap_main",
            in(reg) stack,
            in(reg) cpu as u64,
            options(noreturn),
        );
    }
}

#[unsafe(no_mangle)]
extern "C" fn ap_main(cpu: u64) -> ! {
    let cpu = cpu as usize;
    super::exceptions::load();
    super::apic::init_ap();
    AP_ONLINE.fetch_add(1, Ordering::Release);
    kprintln!("tinyos: cpu{cpu} online");
    crate::sched::ap_enter(cpu)
}

/// Call while boot services are live: launch all APs into the park loop.
pub fn park_aps() {
    let Ok(handle) = boot::get_handle_for_protocol::<MpServices>() else {
        kprintln!("tinyos: no MP services, staying single-core");
        return;
    };
    let Ok(mp) = boot::open_protocol_exclusive::<MpServices>(handle) else {
        kprintln!("tinyos: MP services busy, staying single-core");
        return;
    };
    let count = match mp.get_number_of_processors() {
        Ok(c) => c.enabled.min(MAX_CPUS),
        Err(_) => 1,
    };
    kprintln!("tinyos: {count} cpus reported by firmware");
    if count <= 1 {
        return;
    }

    for cpu in 1..count {
        // LOADER_DATA survives exit_boot_services (we keep that type).
        let pages = AP_STACK_SIZE / 4096;
        if let Ok(mem) = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
        {
            unsafe {
                AP_STACKS[cpu] = (mem.as_ptr() as u64 + AP_STACK_SIZE as u64) & !0xF;
            }
        }
    }

    // A wait event makes startup_all_aps non-blocking (the procedure never
    // returns). We never check the event; leaking it is fine.
    let event = unsafe {
        boot::create_event(boot::EventType::empty(), boot::Tpl::NOTIFY, None, None)
    };
    let Ok(event) = event else {
        kprintln!("tinyos: create_event failed, staying single-core");
        return;
    };
    match mp.startup_all_aps(false, ap_park, core::ptr::null_mut(), Some(event), None) {
        Ok(()) => {
            let t0 = super::timer::uptime_us();
            while (PARKED.load(Ordering::Acquire) as usize) < count - 1
                && super::timer::uptime_us() - t0 < 500_000
            {
                core::hint::spin_loop();
            }
            kprintln!("tinyos: {} aps parked", PARKED.load(Ordering::Acquire));
        }
        Err(e) => kprintln!("tinyos: startup_all_aps failed ({e:?}), single-core"),
    }
}

/// Post-exit, post-sched: open the pen.
pub fn start_secondary_cpus() {
    let parked = PARKED.load(Ordering::Acquire);
    if parked == 0 {
        return;
    }
    RELEASED.store(true, Ordering::Release);
    let t0 = super::timer::uptime_us();
    while AP_ONLINE.load(Ordering::Acquire) < parked
        && super::timer::uptime_us() - t0 < 500_000
    {
        core::hint::spin_loop();
    }
    kprintln!(
        "tinyos: {} of {} cpus online",
        1 + AP_ONLINE.load(Ordering::Acquire),
        1 + parked
    );
}
```

Add `pub mod smp;` to `arch/x86_64/mod.rs`. Check `boot::create_event`'s exact signature against the uefi 0.35 docs when implementing (`EventType`/`Tpl` paths are `uefi::boot::EventType`? they live in `uefi::table::boot` re-exports — fix imports to whatever compiles; the call shape is `create_event(EventType::empty(), Tpl::NOTIFY, None, None)`).

- [ ] **Step 4: `kick_others` for x86 (`irq.rs`)** — replace the stub:

```rust
pub fn kick_others(from: usize) {
    for cpu in 0..super::MAX_CPUS {
        if cpu != from {
            super::apic::send_ipi(cpu, super::apic::VEC_IPI);
        }
    }
}
```

Guard: `send_ipi` to a CPU that never started is harmless on QEMU (no LAPIC at that ID swallows it), but keep the loop bounded by MAX_CPUS as shown.

- [ ] **Step 5: main.rs hooks + Makefile**

In `main()` (pre-exit), right before `let memory_map = …exit_boot_services…`:

```rust
    #[cfg(target_arch = "x86_64")]
    arch::smp::park_aps();
```

In `ui_thread_main`, next to the aarch64 call from Task 5:

```rust
    #[cfg(target_arch = "x86_64")]
    crate::arch::smp::start_secondary_cpus();
```

Makefile line 24: `MACHINE     := -machine q35 -smp 4`.

- [ ] **Step 6: Build both; x86 boot smoke — THE HIJACK CHECK**

Boot x86_64 headless (Makefile's x86 flags + `-display none -serial stdio`), watch serial for: `N cpus reported`, `3 aps parked`, then after shell up: `cpu1 online` … `4 of 4 cpus online`. If instead the parked count is right but no `cpuN online` ever appears (EDK2 reclaimed the APs at exit) or the machine wedges at exit_boot_services: keep the code paths, make `start_secondary_cpus` time out cleanly (it already does), confirm the desktop still boots single-core, and REPORT THIS to the user as a known limitation with the INIT-SIPI trampoline as the documented fix — do not attempt the trampoline inside this task.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m 'smp: x86_64 MP-services AP park/release, LAPIC IPIs'
```

---

### Task 7: Terminal built-ins — `spin`, `ps`, `kill`

**Files:**
- Modify: `kernel/src/term/mod.rs` (three new commands + help entries)
- Modify: `kernel/src/sched/mod.rs` (only if a helper is missing — interfaces below already exist)

**Interfaces:**
- Consumes: `sched::spawn(String, Class, u8, fn()) -> u32`, `sched::snapshot() -> Vec<ThreadInfo>`, `sched::kill(u32) -> bool`, `sched::current()` / kill_pending, `sched::online_cpus()`, `arch::timer::uptime_us`.

- [ ] **Step 1: The spin-thread body and commands (`term/mod.rs`)**

Add near the top:

```rust
use crate::sched;
use crate::sched::thread::Class;
```

Add a free function at the bottom of the file:

```rust
/// Busy work in ~10 ms slices with a yield between slices, so cooperative
/// scheduling (and kill) always gets a look-in.
fn spin_worker() {
    loop {
        let t0 = crate::arch::timer::uptime_us();
        while crate::arch::timer::uptime_us() - t0 < 10_000 {
            core::hint::spin_loop();
        }
        sched::yield_now(); // exits here when kill_pending is set
    }
}
```

New match arms in `execute()` before the `"sudo"` arm:

```rust
            "spin" => {
                let n: usize = rest.trim().parse().unwrap_or(1).clamp(1, 16);
                // Cores 1..N get the load; core 0 keeps the desktop smooth.
                let others = if sched::online_cpus() > 1 { 0b1110 } else { 0b0001 };
                for _ in 0..n {
                    let id = sched::spawn(
                        format!("spin"),
                        Class::Normal,
                        others,
                        spin_worker,
                    );
                    self.out(format!("spawned spin thread {id}"), FG);
                }
            }
            "ps" => {
                self.out(format!("{:>4}  {:<10} {:<8} {:>3}  {}", "ID", "NAME", "STATE", "CPU", "CLASS"), DIM);
                for t in sched::snapshot() {
                    self.out(
                        format!(
                            "{:>4}  {:<10} {:<8} {:>3}  {:?}",
                            t.id, t.name, format!("{:?}", t.state), t.cpu, t.class
                        ),
                        FG,
                    );
                }
            }
            "kill" => match rest.trim().parse::<u32>() {
                Ok(id) if id == sched::current_id() || id == 2 => {
                    // id 2 is the ui thread (first spawn); killing it or
                    // ourselves takes the desktop down.
                    self.out("kill: refusing to kill the ui thread".to_string(), ERR)
                }
                Ok(id) if sched::kill(id) => self.out(format!("kill: signalled {id}"), FG),
                Ok(id) => self.out(format!("kill: no such thread {id}"), ERR),
                Err(_) => self.out("usage: kill <id>".to_string(), ERR),
            },
```

(Nuance: the UI thread's id is whatever `spawn` returned first in `sched::start` — verify with `ps` during smoke and hard-code the guard accordingly, or better: add `pub fn ui_thread_id() -> u32` as a `static UI_ID: AtomicU32` set in `sched::start`, and guard with that. Prefer the static — implement it.)

Help table additions (in the existing `help` list):

```rust
                    ("spin [n]", "spawn n busy threads on cores 1-3"),
                    ("ps", "list threads"),
                    ("kill <id>", "stop a thread"),
```

- [ ] **Step 2: Build both; boot smoke (aarch64, -smp 4)**

In the terminal: `spin 6` → six `spawned spin thread N` lines; desktop stays smooth while dragging a window; `ps` shows spin threads with cpu 1–3; `kill <id>` for each; `ps` shows them gone.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m 'term: spin/ps/kill built-ins'
```

---

### Task 8: Monitor — per-core gauges + thread table

**Files:**
- Modify: `kernel/src/apps/monitor.rs`

**Interfaces:**
- Consumes: `arch::irq::wake_stats(cpu) -> (u32, u32)`, `sched::{snapshot, online_cpus}`.

- [ ] **Step 1: Replace the single Idle meter with per-CPU bars and append a thread table**

In `MonitorApp::draw`, replace the "IDLE bar-meter" block with:

```rust
        // Per-CPU load bars: busy % = 100 - idle % over the last second.
        let ix = body.x + col_w / 2 + 10;
        let iw = col_w / 2 - 10;
        fonts.ui_medium.draw(s, "CPU", 13.0, ix, body.y, TEXT_DIM);
        let online = crate::sched::online_cpus();
        for cpu in 0..online {
            let (_wakes, idle) = crate::arch::irq::wake_stats(cpu);
            let busy = 100u32.saturating_sub(idle);
            let y = body.y + 22 + cpu as i32 * 14;
            s.fill_rect(ix + 28, y, iw - 28, 6, SURFACE_HI);
            s.fill_rect(ix + 28, y, ((iw - 28) * busy as i32 / 100).max(2), 6, ACCENT);
            let label = format!("{cpu}");
            fonts.mono.draw(s, &label, 12.0, ix + 8, y - 4, TEXT_DIM);
        }
```

And after the "INPUT events/sec" block, append:

```rust
        // Thread table.
        let ty = body.y + 232;
        fonts.ui_medium.draw(s, "Threads", 13.0, body.x, ty, TEXT_DIM);
        let mut row = 0;
        for t in crate::sched::snapshot().into_iter().take(7) {
            let line = format!(
                "{:>3} {:<8} {:<7} cpu{} {:?}",
                t.id,
                &t.name[..t.name.len().min(8)],
                format!("{:?}", t.state),
                t.cpu,
                t.class
            );
            fonts
                .mono
                .draw(s, &line, 13.0, body.x, ty + 20 + row * 17, TEXT);
            row += 1;
        }
```

Bump the preferred size to fit: `fn preferred_size(...) -> (i32, i32) { (420, 420) }`.

Caveat: `busy%` on an idle-stat basis reads 100 for a core running a spin thread only when that core's `sleep_until` window has closed — cores running spin threads rarely enter `sleep_until`, so their `LAST_IDLE_PCT` goes stale. Fix inside `idle_loop`/stats: a core that hasn't slept for >1 s should read as 0 % idle. Implement by also updating stats from `yield_now` — cheapest correct version: in `arch::irq`, add

```rust
/// Roll the stats window for a CPU that is busy (not sleeping).
pub fn note_busy(cpu: usize) {
    let now = super::timer::uptime_us();
    let start = WINDOW_START_US[cpu].load(Ordering::Relaxed);
    if now.saturating_sub(start) >= 1_000_000 {
        let slept = SLEPT_US[cpu].swap(0, Ordering::Relaxed);
        let wakes = WAKES[cpu].swap(0, Ordering::Relaxed);
        let span = now - start;
        LAST_RATE[cpu].store((wakes as u64 * 1_000_000 / span) as u32, Ordering::Relaxed);
        LAST_IDLE_PCT[cpu].store((slept * 100 / span).min(100) as u32, Ordering::Relaxed);
        WINDOW_START_US[cpu].store(now, Ordering::Relaxed);
    }
}
```

(both arches; refactor `update_stats` to call it) and call `crate::arch::irq::note_busy(cpu_id())` at the top of `sched::yield_now`.

- [ ] **Step 2: Build both; boot smoke (aarch64)** — open Monitor: 4 CPU bars near 0 busy; run `spin 6`: bars 1–3 fill, bar 0 stays low, thread table lists spin threads. Kill them; bars fall.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m 'monitor: per-core load bars and thread table'
```

---

### Task 9: Full verification (both arches) + docs

**Files:**
- Modify: `README.md` or `docs/` architecture notes if they describe the event loop (update to mention the scheduler)

- [ ] **Step 1: aarch64 full QMP pass**

Boot headless with the smoke recipe (`-smp 4`, QMP socket). Via QMP:
1. `screendump` → confirm desktop; open Monitor via palette or dock (inject Ctrl+K, type "monitor", Enter — or click the dock orb via `input-send-event` abs+btn). Screendump: 4 CPU bars.
2. Inject keystrokes for `spin 6` + Enter in the terminal. Wait 3 s. Screendump: bars 1–3 loaded, UI intact.
3. Inject a window drag (tablet abs move + btn down/move/up). Screendump: window moved.
4. Inject `ps` + Enter, screendump (threads listed, cpus 1–3); inject `kill <id>` for each spin id, then `ps` again.
5. Serial log check: `4 of 4 cpus online`; after kills + 10 s settle, heartbeat `wakes/s=0` (or ≤2) — the deep-idle regression gate.

- [ ] **Step 2: x86_64 full QMP pass** — same sequence under TCG. If Task 6's hijack check ended single-core, run the sequence anyway (spin threads share core 0 cooperatively; UI must still be usable because spin_worker yields every 10 ms) and note the limitation.

- [ ] **Step 3: Update docs** — wherever the docs say "single cooperative event loop / no processes", amend to describe threads + 4 CPUs. Also update the `about` command's "no processes, no problems." line in `kernel/src/term/mod.rs` if desired (suggested: `"4 cores, cooperative threads, no problems."`).

- [ ] **Step 4: Final commit**

```bash
git add -A && git commit -m 'smp: verification pass + docs'
```

Then use superpowers:finishing-a-development-branch (tests = the two builds + both QMP passes) and present the merge options.

---

## Self-review notes (already applied)

- Spec coverage: threads/classes/affinity (T3), waitq (T4), PSCI + GICR (T5), MP park (T6), IPIs (T5/T6), spin/ps/kill (T7), Monitor (T8), single-core fallback (T5/T6 timeouts), idle regression (T4/T9). Locking inventory: heap + serial already spin-locked; virtio queues remain UI-thread-owned (documented ownership instead of a lock — deviation from spec's "new spinlock per device" line, justified: no second accessor exists; revisit when one does).
- Known risks called out inline: EDK2 AP hijack (T6 step 6), TCG-vs-HVC PSCI conduit (T5 header), stale busy% for non-sleeping cores (T8 caveat + fix), MS x64 ABI for all x86 asm (global constraints).
- The `Handoff` enqueue-after-switch pattern (T3 `finish_switch`) is the load-bearing correctness piece: a thread may not appear in READY until its context save completed. Do not "simplify" it to enqueue-before-switch.
