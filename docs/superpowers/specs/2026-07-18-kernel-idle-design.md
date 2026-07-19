# Kernel idle — interrupt-driven tickless event loop

## Goal

Replace the busy-spinning 60fps loop with interrupt-driven idle: the CPU
sleeps in `wfi`/`hlt` and wakes on a one-shot timer deadline or input
interrupt. Rendering happens only when something changed. This is also the
interrupt plumbing a future multi-core scheduler will build on, so the
modern controller paths are used on both arches (GICv3, LAPIC/IOAPIC —
no 8259 PIC, no PIT timer source).

## Architecture

### aarch64 (QEMU virt, `gic-version=3` pinned in the Makefile)
- GICv3: distributor `0x0800_0000`, redistributor frames `0x080A_0000`
  (per-CPU stride 0x20000; only CPU0 initialized now, structured so a
  future CPU brings up its own redistributor). CPU interface via sysregs:
  `ICC_SRE_EL1`, `ICC_PMR_EL1`, `ICC_IGRPEN1_EL1`, `ICC_IAR1_EL1`,
  `ICC_EOIR1_EL1`.
- Timer: virtual timer `CNTV_TVAL_EL0`/`CNTV_CTL_EL0`, PPI 27, one-shot:
  armed with ticks-to-deadline before sleeping, disabled in the handler.
- Vector table gains a real IRQ path (curr-EL SPx IRQ entry, offset
  0x280): save x0–x18, x29, x30, ELR_EL1, SPSR_EL1; call
  `irq_entry()`; restore; `eret`. Sync exceptions keep report-and-park.
- Virtio INTx: PCIe INTA–D on virt are SPIs 35–38 — enable all four,
  level-triggered. Handler reads each virtio device's ISR register
  (deasserts the line), sets the input-wake flag.

### x86_64 (QEMU q35)
- Local APIC (xAPIC MMIO `0xFEE0_0000`): enabled via spurious-interrupt
  register (vector 0xFF), timer in one-shot mode, divider 16, calibrated
  once against the TSC (already PIT-calibrated at boot). Timer vector 48.
  EOI via LAPIC `0xB0`.
- IOAPIC (`0xFEC0_0000`): redirection entries for virtio INTx lines.
  GSI = the firmware-programmed PCI Interrupt Line register (OVMF routes
  to 10/11; identity-mapped on the IOAPIC), level-triggered, active-low,
  vectors 49+. Both 8259 PICs are masked entirely (no legacy path).
- IDT extended with `extern "x86-interrupt"` gates for vectors 48+.
- Idle: `sti; hlt` (interrupts enabled only while halted; handlers run,
  set flags, return; the loop re-checks with IRQs off via `cli`).

### Shared (arch facade additions)
- `arch::irq::init()` — controller + timer + INTx bring-up.
- `arch::irq::sleep_until(deadline_us)` — arm one-shot timer, sleep-loop
  until deadline or `WAKE_INPUT` flag; returns the wake cause. Handlers
  only ack hardware and set `AtomicBool`/counter statics — no other work
  in interrupt context (the loop polls virtqueues after waking).
- `arch::irq::wake_stats()` — wakes/sec + sleep-time ratio for Monitor.
- Virtio driver: map the ISR capability (cfg type 3) at init; expose
  `pub fn isr_read(&self)`; PCI: keep INTx enabled (never set DisINTx),
  read Interrupt Line for x86 GSI selection.

### Event loop (main.rs + shell)
- `Shell::next_deadline(now) -> u64`: `now+16_667us` while interactive
  (any input in the last 3s) or a timer-countdown window exists;
  `now+500_000us` while a Monitor window is open; else the next minute
  boundary (clock pill refresh).
- Dirty tracking: recompose+present only when (a) input events arrived,
  (b) the deadline elapsed (animation frame), or (c) first frame. The
  caret (terminal, notes, launcher) blinks only while interactive and
  parks visible when idle, so it never forces frames on its own.
- Loop: poll input → handle → if dirty: compose+present → sleep_until
  (next deadline).

### Monitor app
- New "Idle" gauge: percent of the last second spent asleep (from
  `wake_stats`), drawn as a bar-meter like Heap. Sparkline buckets keep
  the existing 500ms cadence.

## Non-goals (this milestone)

MSI-X, SMP bring-up itself, scheduler/threads, interrupt priorities/
nesting, GIC ITS. The controller init is structured per-CPU-aware but
only CPU0 is brought up.

## Verification

- Serial: wake-counter line every ~5s of idle (`tinyos: wakes/s=...`).
  Idle desktop must be ≤ ~4 wakes/s with no Monitor open.
- QMP: idle 5s → screendump unchanged; keypress wakes instantly (typed
  char appears on next dump); Monitor shows Idle ≥ 90% at rest.
- Host-side: `ps -o %cpu` of the QEMU process before/after the change
  while the guest sits idle (expect ~100% → single digits).
- Both arches; `make run` in person for acceptance. Branch `kernel-idle`.
