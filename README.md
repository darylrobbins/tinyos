# tinyOS

A tiny operating system written in Rust for arm64 and x86_64: UEFI boot, a
software-composited GUI (Meridian design), a terminal with built-in
commands, and a cooperative multi-core scheduler on 4 CPUs. Tickless,
interrupt-driven, no processes (yet), no problems.

![desktop](docs/screenshots/desktop.png)

## What it does

- Boots as a UEFI application on QEMU — arm64 `virt` (HVF-accelerated on
  Apple Silicon, the fast dev loop) or x86_64 `q35` (TCG-emulated) — grabs
  the GOP framebuffer, exits boot services, and runs freestanding.
- Animated boot splash → desktop with procedural "aurora" wallpaper, frosted
  menu bar and dock (real box-blur backdrop), and a draggable terminal window
  with macOS-style chrome.
- Interrupt-driven and tickless: GICv3 (arm64) / LAPIC+IOAPIC (x86_64),
  virtio INTx wakes, one-shot timers. Idle CPUs sit in wfi/hlt at ~0 host
  CPU; the UI thread blocks on an input wait queue between frames.
- Cooperative SMP scheduler: 4 cores (PSCI on arm64, UEFI MP Services on
  x86_64), kernel threads with priority classes and CPU affinity, a global
  ready queue, SGI/IPI cross-core wakes. `spin 6` pegs cores 1–3 while the
  desktop stays smooth on core 0.
- virtio-input drivers (keyboard + tablet); fontdue-rasterized Geist and
  Geist Mono.
- **tinyfs**, a native copy-on-write filesystem on virtio-blk: shadow-paging
  checkpoints (crash-consistent by construction — no journal, no fsck, no
  GC), files persist across reboots in `disk.img`, same image mounts on both
  arches. Comes with a host-side `mkfs-tinyfs` tool and `cargo test -p tinyfs`
  unit + crash-consistency tests.
- Shell built-ins: `help`, `echo`, `clear`, `sysinfo`, `memstat`, `uptime`,
  `date`, `spin`, `ps`, `kill`, `about` (and one you'll find on your own),
  plus files: `ls`, `cat`, `write`, `append`, `mkdir`, `rm`, `mv`, `cd`,
  `pwd`, `fsinfo` — and `shutdown` / `reboot` (sync the disk, then PSCI
  SYSTEM_OFF/RESET on arm64, ACPI S5 / reset port on x86_64).

| | |
|---|---|
| ![splash](docs/screenshots/splash.png) | ![terminal](docs/screenshots/terminal.png) |

## Running it (macOS, Apple Silicon)

```sh
brew install qemu     # once
make run              # arm64, near-native under HVF
make run ARCH=x86_64  # x86_64, emulated (slower boot, same OS)
```

`make run` builds the kernel for `aarch64-unknown-uefi`, stages it as
`esp/EFI/BOOT/BOOTAA64.EFI`, and boots QEMU with edk2 firmware under
Hypervisor.framework. Serial output lands on stdout. If HVF gives you
trouble: `make run ACCEL="-accel tcg -cpu cortex-a72"`.

The desktop runs at 1440×900 by default (the kernel re-points QEMU's ramfb
at its own framebuffer via fw_cfg, past edk2's 1024×768 GOP ceiling). Pick
any size with `make run RES=1920x1200`, and use the window's View → Zoom to
Fit (on by default) to scale.

## Layout

```
kernel/src/
  main.rs        UEFI entry, boot handoff, UI thread
  sched/         cooperative SMP scheduler: threads, ready queue, wait queues
  arch/aarch64/  vectors, GICv3, generic timer, PSCI SMP, context switch, PL011
  arch/x86_64/   IDT, LAPIC/IOAPIC, TSC timer, MP-services SMP, context switch
  mem/           heap over the UEFI memory map
  drivers/       PCI ECAM, virtio-pci transport, virtio-input, virtio-blk
  fs/            mounted-filesystem singleton + shell-facing wrappers
  gfx/           software surface, blending, blur, fontdue glyph cache
  ui/            splash, wallpaper, desktop shell, cursor
  term/          terminal widget + built-in shell
crates/abi/      shared ABI: syscall numbers, protocols, design tokens
crates/tinyfs/   the filesystem itself: no_std core, host-testable
crates/vicore/   vi editor core: no_std, host-testable
tools/mkfs-tinyfs/  host tool: create/populate/inspect/check disk images
```

Design doc: `docs/superpowers/specs/2026-07-17-tinyos-design.md`.

Fonts: [Geist and Geist Mono](https://vercel.com/font), OFL (license in
`assets/`). Inspired by [Philipp Oppermann's blog_os](https://os.phil-opp.com).
