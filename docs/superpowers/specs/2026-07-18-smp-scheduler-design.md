# SMP Cooperative Scheduler — Design Spec

Date: 2026-07-18
Status: approved for planning

## Goal

Bring up all 4 CPUs on both architectures and introduce kernel threads with a
cooperative, preemption-ready scheduler. The desktop stays responsive while
compute threads saturate the other cores. This is the plumbing userspace
processes will later stand on.

## Decisions (with the user)

- **Cooperative now, preemptive later.** Threads switch only when they yield,
  block, or exit. The timer tick and context-switch mechanism are built so
  timer preemption is a small follow-up diff, not surgery. Soft real-time is
  explicitly deferred to the userspace milestone (when untrusted code exists).
- **4 cores** (`-smp 4`) on both arches. Boot must degrade gracefully to fewer
  cores (including 1) if AP bring-up fails.
- **Global ready queue + per-thread affinity masks.** One spin-locked ready
  list shared by all cores. UI thread pinned to core 0; stress threads masked
  to cores 1–3. `pick_next()` is the policy seam — per-CPU queues or a
  deadline class can replace it later without touching callers.
- **x86 APs via UEFI MP Services** (parked pre-`exit_boot_services`), NOT an
  INIT-SIPI real-mode trampoline. aarch64 APs via PSCI `CPU_ON` (HVC conduit).
- **Plain spinlocks only.** IRQ handlers remain ack-and-flag (they never take
  locks), so no irqsave variant is needed. Critical sections stay short; never
  block while holding a lock.
- **Payoff demo:** per-core Monitor gauges + thread table, terminal `spin`,
  `ps`, `kill` built-ins.

## Architecture

New arch-neutral module `kernel/src/sched/` plus three small per-arch pieces
(AP bring-up, context switch, IPI). After driver init, `main.rs` calls
`sched::start(ui_thread_main)`; the existing shell loop moves essentially
unchanged into the UI thread (pinned core 0, class Interactive). Cores with
nothing to run idle exactly like today — tickless `wfi`/`hlt` via the existing
`arch::irq` machinery, now per-CPU.

### Thread (`sched/thread.rs`)

```rust
pub struct Thread {
    pub id: u32,
    pub name: alloc::string::String, // heap-allocated: spin threads get numbered names
    pub state: State,              // Ready | Running | Blocked | Exited
    pub class: Class,              // Realtime | Interactive | Normal | Idle (advisory until preemption)
    pub affinity: u8,              // bitmask of allowed CPUs
    stack: KernelStack,            // 64 KiB heap allocation, freed on reap
    ctx: arch::Context,            // callee-saved registers + sp
    // Userspace seam: an Option<AddressSpace> field lands here later;
    // the scheduler itself never changes.
}
```

Threads live in a global table as `Arc<SpinMutex<Thread>>` (alloc crate). A
thread whose entry fn returns (or panics) transitions to Exited and is reaped
by the next scheduler pass **on another stack** — a CPU never frees the stack
it is standing on.

### Scheduler core (`sched/mod.rs`)

- Global thread table + global ready queue (`VecDeque` per class) behind
  spinlocks.
- API: `spawn(name, class, affinity, entry_fn) -> ThreadId`, `yield_now()`,
  `exit() -> !`, `block_current(waitq, deadline)`, `wake(thread)`,
  `current() -> ThreadId`, `start(ui_main) -> !`.
- Per-CPU state: `current` thread pointer + idle/busy accounting, stored in a
  fixed array indexed by `cpu_id()` (MPIDR Aff0 / LAPIC ID mapped to 0..4).
- `pick_next(cpu)` — the policy function: scan classes high→low, round-robin
  within a class, skip threads whose affinity excludes this CPU. Nothing
  runnable → idle loop.
- Kill: `kill(id)` sets a pending-exit flag; the thread exits at its next
  yield/block point (cooperative kernel threads cannot be destroyed mid-run
  safely). `yield_now()` checks the flag.

### Wait queues (`sched/waitq.rs`)

`WaitQueue`: block the current thread on it with an optional deadline
(`block_current` re-queues the thread when woken or when the deadline
passes). IRQ-side wakeups stay lock-free: handlers only set an atomic pending
flag (exactly today's `WAKE_INPUT` pattern); the drain — moving blocked
threads to the ready queue — happens in thread context inside the scheduler.
The existing input wake flag becomes a `WaitQueue` wake. This is the primitive
future syscalls (`read()` etc.) will block on.

### Per-arch additions

**aarch64** (`arch/aarch64/`):
- `smp.rs`: PSCI over HVC (QEMU sets conduit=hvc under HVF). Probe
  `PSCI_VERSION` (0x8400_0000) first; sane version → proceed, else stay
  single-core with a serial warning. `CPU_ON` (0xC400_0003) per core with
  entry point + context arg. AP entry asm: set SP from the passed per-CPU
  block, then Rust: init own GICR (base + 0x20000 × cpu), ICC sysregs, vector
  table, enter scheduler idle loop.
- `context.rs`: `Context` = x19–x28, x29, x30(lr), sp. `switch_to(&mut old,
  &new)` in asm.
- IPI: GIC SGI 0 via `ICC_SGI1R_EL1`, enabled per-CPU at each GICR. IRQ
  handler treats SGI 0 as wake-only (ack + eoi).

**x86_64** (`arch/x86_64/`):
- `smp.rs`: locate `MpServices` protocol before `exit_boot_services`;
  `startup_all_aps` (non-blocking) sends each AP into a park loop in our code
  with its own stack from our heap. BSP exits boot services, then releases the
  pen via an atomic. Each AP then: load our GDT/IDT, enable its LAPIC (SVR;
  timer calibration value shared from BSP — same crystal), enter scheduler
  idle loop. MP Services absent → single-core with a serial warning.
- `context.rs`: `Context` = rbx, rbp, r12–r15, rsp. `switch_to` in asm.
- IPI: LAPIC ICR fixed-delivery vector 50 (`VEC_IPI`), wake-only handler.
- IOAPIC input routing still targets CPU 0 only (UI core owns input).

**Both**: `arch::irq::sleep_until` generalizes to the per-CPU idle primitive
— sleep until deadline, device IRQ, or IPI. An IPI is sent to a target CPU
when a thread it may run is enqueued and that CPU might be idle.

### Locking inventory

Newly multi-core-visible state that gains (or already has) spinlocks: heap
allocator (already `spin`-locked), serial `kprintln` (new spinlock), virtio
queues (new spinlock per device), thread table / ready queues / wait queues
(new). The framebuffer and all shell/UI state remain owned by the UI thread —
no locks.

## Payoff features

- **Monitor app**: per-core load bars (busy % from per-CPU idle accounting,
  1s window) and a thread table: id, name, state, CPU, class.
- **Terminal built-ins**:
  - `spin [n]` — spawn n Normal-class busy threads (default 1), affinity
    cores 1–3, each yielding every ~10 ms of work; they run until killed.
  - `ps` — list threads (id, name, state, cpu, class).
  - `kill <id>` — request cooperative exit; refuses id of UI thread.

## Failure handling

- Panic handler prints CPU id + current thread name over serial, then parks
  that CPU (`wfi`/`hlt` loop); other cores keep running.
- AP bring-up failure (PSCI error / MP Services absent or failing): log to
  serial, continue with the cores that made it — all functionality must work
  single-core.
- `kill` of a blocked thread: pending-exit flag + wake, so it exits on the
  next pass.

## Verification (QMP harness, both arches)

1. Boot `-smp 4`; serial shows `cpu0..cpu3 online`.
2. Screendump: Monitor shows 4 per-core gauges, thread table lists ui thread.
3. Inject `spin 6` + Enter: cores 1–3 near 100 %, core 0 mostly idle, UI
   intact; drag a window mid-stress via QMP tablet events — screendump shows
   it moved.
4. `ps` shows the spin threads with CPUs 1–3; `kill` each; gauges fall back
   to idle.
5. Idle regression: with nothing running, wakes/s ≈ 0 on all cores and host
   CPU usage ~0 % (existing tickless behavior preserved).

## Out of scope

Timer-slice preemption, deadline/EDF scheduling, userspace/address spaces,
per-CPU run queues, thread-local storage, priorities beyond advisory classes.
