# Kernel Idle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Interrupt-driven tickless idle per `docs/superpowers/specs/2026-07-18-kernel-idle-design.md` — GICv3 + virtual timer + virtio INTx on aarch64; LAPIC + IOAPIC on x86_64; adaptive deadlines + dirty rendering in the shell.

**Architecture:** New `arch::irq` facade (`init`, `sleep_until`, `wake_stats`) implemented per-arch; handlers only ack + set atomics; the cooperative loop keeps doing all real work after waking. Shell computes deadlines and skips clean frames.

**Tech Stack:** existing tinyOS kernel; all MMIO through `drivers/mmio.rs` accessors (HVF ISV constraint — see memory).

## Global Constraints

- No 8259/PIT interrupt sources on x86 — LAPIC timer + IOAPIC only (PIT remains solely as the existing one-time TSC calibration).
- GICv3 only; Makefile pins `-machine virt,gic-version=3`.
- Interrupt handlers: ack hardware + set static atomics, nothing else.
- All device MMIO via `drivers::mmio` asm accessors on aarch64.
- Test cycle per task: silent `make build`, then the repo's QMP harness (see prior plans) on branch `kernel-idle`.
- Commit after each task.

---

### Task K1: aarch64 — GICv3, virtual-timer IRQ, wfi sleep, adaptive loop

**Files:**
- Create: `kernel/src/arch/aarch64/gic.rs`, `kernel/src/arch/aarch64/irq.rs`
- Modify: `kernel/src/arch/aarch64/mod.rs` (pub mod gic/irq), `kernel/src/arch/aarch64/exceptions.rs` (IRQ vector entry + context save asm), `kernel/src/arch/aarch64/timer.rs` (`pub fn ticks_hz()`, `pub fn set_timer_us(deadline_us)` via CNTV), `Makefile` (`-machine virt,gic-version=3`), `kernel/src/main.rs` (loop), `kernel/src/ui/shell/mod.rs` (`next_deadline`, dirty flag, interactive tracking, caret parking)
- `kernel/src/arch/x86_64/mod.rs`: stub `pub mod irq` with `init()` no-op + `sleep_until` = existing busy wait + `wake_stats` zeros, so x86 still builds until K3.

**Interfaces:**
- `gic.rs`: `pub fn init()` (GICD ctrl enable ARE|G1NS; GICR wake (clear ProcessorSleep, wait ChildrenAsleep); SGI/PPI enable for INTID 27 via GICR_ISENABLER0; sysreg ICC_SRE=1, PMR=0xFF, IGRPEN1=1); `pub fn enable_spi(intid: u32)` (GICD_ISENABLER + GICD_ICFGR level); `pub fn ack() -> u32` (ICC_IAR1); `pub fn eoi(intid: u32)`.
- `irq.rs`:
  ```rust
  pub static WAKE_INPUT: AtomicBool; static WAKES: AtomicU32; static SLEPT_US: AtomicU64;
  pub fn init() { gic::init(); }
  pub fn sleep_until(deadline_us: u64) { // returns on deadline or input flag
      while now < deadline && !WAKE_INPUT { timer::set_timer_us(deadline); wfi(); }
      // stats accumulate here
  }
  pub fn wake_stats() -> (u32 /*wakes/s*/, u32 /*idle pct*/) // decayed each call window
  #[no_mangle] extern "C" fn irq_entry() { let id = gic::ack(); match id { 27 => timer off, 35..=38 => input flag (isr reads happen in K2), _ => {} } gic::eoi(id); }
  ```
- exceptions.rs: `.balign 0x80` entry at vector slot 5 (curr-EL SPx IRQ): full caller-saved save (x0–x18, x29, x30, ELR_EL1, SPSR_EL1 — 24 slots, keep sp 16-aligned), `bl irq_entry`, restore, `eret`. Other slots unchanged.
- main loop:
  ```rust
  loop {
    events.clear(); input.poll(&mut events);
    let now = timer::uptime_us();
    let dirty = shell.handle_frame(&events, now); // handle + stats_tick + dirty calc
    if dirty { shell.compose(...); surface.present(&fb); }
    arch::irq::sleep_until(shell.next_deadline(now));
  }
  ```
- Shell: `last_input_us`, `interactive() = now - last_input_us < 3s`; `next_deadline`; caret draws blink only when interactive (terminal/notes/palette take an `interactive: bool` via draw's `now_ms`… simplest: shell publishes `pub static INTERACTIVE: AtomicBool` read by caret code); every 5s of idle print `tinyos: wakes/s=N idle=P%` on serial.

**Steps:**
- [ ] Implement; `make build` silent (x86 stub included: `make build ARCH=x86_64` silent).
- [ ] QMP: boot, wait 6s idle, grep serial for `wakes/s` ≤ 4 and `idle=9x%`; screendump; send keys `sysinfo\n` → screendump shows output (input still responsive at slow tick via 16ms interactive burst after first event; first key may land on next tick — full instant-wake arrives in K2).
- [ ] `git add -A && git commit -m "idle: GICv3 + one-shot virtual timer + wfi tickless loop (aarch64)"`

### Task K2: virtio INTx input wake + Monitor idle gauge

**Files:**
- Modify: `kernel/src/drivers/virtio.rs` (map ISR cap type 3 → `isr_addr`; `pub fn isr_read(&self) -> u8` via mmio::r8), `kernel/src/drivers/input.rs` (`pub fn isr_read_all(&self)`, `pub fn init` returns devices with INTx info), `kernel/src/drivers/pci.rs` (`pub fn interrupt_line(&self) -> u8` = cfg 0x3C), `kernel/src/arch/aarch64/irq.rs` (SPI 35–38 enable + handler reads ISR of registered devices via a registered callback: `pub fn register_input_isr(f: fn())` storing a fn pointer that calls into a static list), `kernel/src/apps/monitor.rs` (Idle bar-meter row from `arch::irq::wake_stats`), `kernel/src/ui/shell/mod.rs` / `main.rs` wiring.
- Input ISR access from IRQ context: store raw `isr_addr` usizes in a static `[AtomicUsize; 8]` filled during `Input::init` — handler iterates and `mmio::r8`s each nonzero entry. No locks.

**Steps:**
- [ ] Implement; builds silent both arches.
- [ ] QMP: idle 6s (wakes/s low), then send a key WITHOUT waiting for a tick boundary; serial logs wake cause `input-irq` (add temp log, keep behind a counter); typed char visible on immediate screendump. Open monitor → Idle gauge ≥90% at rest; wiggle pointer → Idle drops, FPS 60.
- [ ] Commit `"idle: virtio INTx instant wake + Monitor idle gauge"`.

### Task K3: x86_64 — LAPIC one-shot timer + IOAPIC INTx + parity

**Files:**
- Create: `kernel/src/arch/x86_64/apic.rs` (LAPIC: enable via SVR 0x1FF vector 0xFF; timer LVT vector 48 one-shot, divide 16 (0x3); `set_timer_us` from a boot-time calibration of LAPIC ticks vs TSC over 10ms; EOI reg 0xB0. IOAPIC: `redirect(gsi, vector, level_low: bool)` via IOREGSEL/IOWIN at 0xFEC00000), replace stub `kernel/src/arch/x86_64/irq.rs` (same interface as aarch64: init masks both 8259s (0xFF to 0x21/0xA1), LAPIC+IOAPIC init, virtio GSIs from `pci.interrupt_line()` of input devices → vectors 49+, ISR-read statics shared via the same `drivers` mechanism; `sleep_until` = `sti; hlt` loop with `cli` re-check)
- Modify: `kernel/src/arch/x86_64/exceptions.rs` (IDT grows to 64 entries; vectors 48/49+ gates call handlers that ack: LAPIC EOI always; input vectors also read virtio ISRs + set flag; spurious 0xFF gate no-EOI), `kernel/src/arch/x86_64/timer.rs` (expose TSC helpers for calibration), `kernel/src/main.rs` (drop arch cfg differences — same loop both arches), `README.md` (idle feature blurb)
- Interrupt-flag discipline: normal execution runs with IF=0 (as today); `sleep_until` does `sti; hlt; cli` per iteration so handlers run only inside the sleep window (identical semantics to wfi with masked DAIF + GIC signaling on arm — document in irq.rs).

**Steps:**
- [ ] Implement; builds silent both arches.
- [ ] QMP x86 (TCG, 40s boot): idle serial wakes/s ≤ 4; `Ctrl+K` `monitor\n` → Idle gauge present; keypress instant-wakes (serial counter).
- [ ] aarch64 regression: quick boot + wakes/s check.
- [ ] Host CPU check: `ps -o %cpu= -p <qemu pid>` during 10s idle desktop (aarch64/HVF) — record in commit message (expect single digits vs ~100 before).
- [ ] README + commit `"idle: LAPIC/IOAPIC tickless parity on x86_64; docs"`.

## Self-Review Notes

- Spec coverage: GICv3/timer/vectors → K1; INTx + ISR + monitor gauge → K2; LAPIC/IOAPIC/no-PIC + loop parity + host-CPU proof → K3. Adaptive deadlines + caret parking + dirty skip → K1 (shell).
- Interfaces consistent: `arch::irq::{init, sleep_until, wake_stats}` on both arches; ISR statics shared in `drivers`.
- No placeholders.
