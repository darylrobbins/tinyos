# TinyOS — Lightweight Rust OS (arm64-first, Apple-inspired GUI)

## Context

Greenfield hobby OS in `/Users/daryl/code/tinyos` (empty dir, not yet a git repo). Goals, as decided with the user:

- **Arch order:** arm64 first (fast HVF-accelerated QEMU on the Apple Silicon Mac), x86_64 as a later milestone. Code structured with a HAL split from the start so the port is tractable.
- **GUI scope:** "Desktop-lite" — animated boot splash → desktop with wallpaper, Apple-style menu bar and dock → terminal in a movable window. Compositor with basic window management (move/focus/close); terminal is the only app initially.
- **Kernel depth:** terminal runs **kernel built-in commands** (no userspace/privilege separation yet). Single-threaded cooperative event loop.
- **Boot:** kernel is itself a **UEFI application** (unikernel-style, no separate bootloader handoff): grab GOP framebuffer + memory map, exit boot services, keep running. Same strategy will work for x86_64 later and in principle on real hardware.
- Inspiration/reference: https://os.phil-opp.com (concepts adapted; that series is x86_64-centric).

## Toolchain & dev loop

- `brew install qemu` (not currently installed) — ships edk2 firmware at `/opt/homebrew/share/qemu/edk2-aarch64-code.fd`.
- `rustup target add aarch64-unknown-uefi` (tier-2 target, builds a PE binary; no custom target JSON or `-Zbuild-std` needed on stable-ish nightly — use **nightly** anyway for `alloc_error_handler`/asm ergonomics via `rust-toolchain.toml`).
- **Run script** (`justfile` or `Makefile`, pick `Makefile` for zero extra deps): builds, stages `esp/EFI/BOOT/BOOTAA64.EFI`, launches:
  ```
  qemu-system-aarch64 -machine virt -accel hvf -cpu host -m 512M \
    -drive if=pflash,format=raw,readonly=on,file=<edk2-aarch64-code.fd copy> \
    -drive if=pflash,format=raw,file=<edk2 vars scratch copy> \
    -device virtio-gpu-pci -device virtio-keyboard-pci -device virtio-tablet-pci \
    -drive format=raw,file=fat:rw:esp \
    -serial stdio
  ```
  Note: GOP over `virtio-gpu-pci` gives a linear framebuffer via edk2; QEMU window shows the display, serial goes to the terminal for logs. If virtio-gpu GOP proves awkward, fall back to `-device ramfb` (edk2 also exposes GOP over ramfb).
- `make run` = full loop. Also `make gdb` target later (`-s -S`).

## Repo layout (cargo workspace)

```
tinyos/
  Makefile
  rust-toolchain.toml            # nightly, targets: aarch64-unknown-uefi
  kernel/                        # the UEFI app binary crate
    src/main.rs                  # UEFI entry, boot phase, then kmain event loop
    src/arch/aarch64/            # HAL: exception vectors, timer, cache/MMU glue, interrupts (GIC) later
    src/arch/mod.rs              # arch trait/facade — x86_64 lands here later
    src/mem/                     # bump→linked-list heap over UEFI memory map
    src/drivers/                 # pci scan, virtio (queue core), virtio-input
    src/gfx/                     # framebuffer, double buffer, drawing prims, font, alpha blending
    src/ui/                      # compositor, window, menubar, dock, cursor, splash
    src/term/                    # terminal emulator widget + built-in command shell
  assets/                        # font (Inter or similar OFL-licensed TTF), wallpaper gen or embedded
  docs/superpowers/specs/2026-07-17-tinyos-design.md   # this design, committed
```

Key crates (all `no_std`+`alloc` compatible): `uefi` (boot phase), `linked_list_allocator`, `fontdue` (TTF rasterization), `spin`, `log`. Avoid heavyweight GUI crates — the compositor/widgets are hand-rolled (that's the fun part).

## Milestones

### M1 — Boot & print (foundation)
- Workspace scaffold, `git init`, Makefile, toolchain file.
- UEFI entry with `uefi` crate: log to serial (UEFI stdout + raw PL011 after exit).
- Query GOP → framebuffer ptr/stride/format; get memory map; `exit_boot_services`.
- Post-exit: keep UEFI's identity-mapped MMU tables as-is; install exception vectors (report panics/faults over serial); heap from largest usable memory region.
- Verify: `make run` shows text on serial and fills the screen with a color.

### M2 — Graphics core + boot splash
- `gfx`: back buffer in RAM, blit-to-GOP, rects, rounded rects w/ AA edges, alpha blending, gradients, `fontdue` glyph cache for embedded Inter font.
- Timer: aarch64 generic timer (CNTPCT/CNTFRQ) for millisecond time; simple frame pacing (~60fps busy loop is fine, no interrupts needed yet).
- Boot splash: dark background, TinyOS logo/wordmark, Apple-style indeterminate→determinate progress bar, fade-out transition to desktop.
- Verify: `make run` shows animated splash in the QEMU window.

### M3 — Input (virtio)
- Minimal PCI ECAM scan on the virt board (ECAM base from known virt layout; parse from DTB later if needed).
- Virtio modern driver core (single virtqueue impl, no interrupts — poll used-ring each frame from the event loop).
- `virtio-keyboard` (evdev-style events → key events w/ US layout map) and `virtio-tablet` (absolute pointer → cursor position, buttons).
- Verify: debug overlay echoing keys and drawing the cursor at pointer position.

### M4 — Desktop shell (the payoff)
- Compositor: layered scene — wallpaper (procedural gradient), menu bar, windows, dock, cursor; dirty-rect or full recomposite per frame (full is fine at 1024×768).
- Menu bar: translucent bar,  logo-ish glyph, app name, clock (from timer, fake epoch is fine).
- Dock: centered, rounded translucent panel, terminal icon, magnify-on-hover optional.
- Window chrome: rounded corners, drop shadow (precomputed blur sprite), traffic-light close button, title bar dragging (move), click-to-focus.
- Verify: boots to desktop, window can be dragged and closed, dock relaunches terminal.

### M5 — Terminal + built-in shell
- Terminal widget: monospace grid renderer (embed a mono font, e.g. JetBrains Mono), cursor, scrollback, line editing (backspace, arrows, history).
- Built-ins: `help`, `echo`, `clear`, `sysinfo` (arch, memory, uptime, fb res), `memstat`, `uptime`, `date`, `about`, easter egg.
- Verify: end-to-end demo — power on → splash → desktop → open terminal → run commands.

### M6 (later, out of first scope) — x86_64 port
- Add `x86_64-unknown-uefi` target; implement `arch/x86_64` (exception vectors/IDT, TSC or HPET timer); same UEFI/GOP/virtio path under `qemu-system-x86_64` (emulated on the Mac). The HAL boundary in `arch/` is designed for this from M1.

## Design notes / risks

- **HVF + `-cpu host`**: if edk2 or the kernel misbehaves under HVF, fall back to `-accel tcg -cpu cortex-a72` (still fast enough); keep both as Makefile variants.
- **No interrupts initially**: everything (input polling, animation, terminal) runs in one cooperative main loop paced by the generic timer. GIC + timer interrupts are a stretch goal, not needed for the milestones.
- **Translucency**: real gaussian blur is expensive in software; use precomputed shadow sprites and simple alpha-over-wallpaper "frosted" approximation (dim+tint), which reads as Apple-like at low cost.
- **Fonts**: Inter (UI) + JetBrains Mono (terminal), both OFL — embed subsets via `include_bytes!`.
- **Resolution**: request 1024×768 or 1280×800 via GOP mode selection; retina scaling out of scope.

## Verification (end state)

`make run` on the Mac: QEMU window opens → animated splash → desktop with menu bar/dock/wallpaper → click dock icon → terminal window opens → type `sysinfo`, `help`, `echo hi` → drag window around → close it. Serial log on stdout shows boot stages. Each milestone has its own verify step above; commit per milestone.
