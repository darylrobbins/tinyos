# tinyOS

A tiny operating system written in Rust for arm64 and x86_64: UEFI boot, a
software-composited Apple-inspired GUI, and a terminal with built-in
commands. No interrupts, no processes, no problems.

![desktop](docs/screenshots/desktop.png)

## What it does

- Boots as a UEFI application on QEMU — arm64 `virt` (HVF-accelerated on
  Apple Silicon, the fast dev loop) or x86_64 `q35` (TCG-emulated) — grabs
  the GOP framebuffer, exits boot services, and runs freestanding.
- Animated boot splash → desktop with procedural "aurora" wallpaper, frosted
  menu bar and dock (real box-blur backdrop), and a draggable terminal window
  with macOS-style chrome.
- virtio-input drivers (keyboard + tablet) polled from a cooperative 60 fps
  event loop; fontdue-rasterized Inter and JetBrains Mono.
- Shell built-ins: `help`, `echo`, `clear`, `sysinfo`, `memstat`, `uptime`,
  `date`, `about` (and one you'll find on your own).

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
  main.rs        UEFI entry, boot handoff, event loop
  arch/aarch64/  exception vectors, generic timer, PL011 serial
  arch/x86_64/   IDT, TSC timer (PIT-calibrated), COM1 serial, port I/O
  mem/           heap over the UEFI memory map
  drivers/       PCI ECAM, virtio-pci transport, virtio-input
  gfx/           software surface, blending, blur, fontdue glyph cache
  ui/            splash, wallpaper, desktop shell, cursor
  term/          terminal widget + built-in shell
```

Design doc: `docs/superpowers/specs/2026-07-17-tinyos-design.md`.

Fonts: [Inter](https://rsms.me/inter/) and
[JetBrains Mono](https://www.jetbrains.com/lp/mono/), both OFL (licenses in
`assets/`). Inspired by [Philipp Oppermann's blog_os](https://os.phil-opp.com).
