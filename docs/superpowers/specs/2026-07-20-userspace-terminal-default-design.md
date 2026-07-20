# SP2 — Promote the userspace terminal to the boot default

Date: 2026-07-20
Status: design, approved direction — pending written-spec review
Builds on: SP1a–c (userspace terminal + surfaces + kernel-attested exec), the
least-privilege manifest default (#14).
Architecture: `docs/architecture/display-and-windowing.md` (the compositor is a
window server hosting userspace clients; the shell + terminal already moved to
userspace, the compositor follows in SP3).

## Context & motivation

The userspace terminal (`/apps/terminal`, "uterm") reached parity with the
in-kernel terminal across SP1a–c: it opens a compositor window, spawns
`/apps/sh` into a console it serves, renders the line world + cell surfaces, and
hosts windowed children via kernel-attested exec. But it is still reachable only
from the command palette — **the kernel terminal is what boots.** SP2 makes the
userspace terminal the face of the OS: the default window on capable systems.

The original roadmap framed SP2 as *deleting* the in-kernel terminal, `textview`
widget, `App` trait, and `monitor`. Reconnaissance surfaced a hard constraint
that blocks the deletion:

- **x86_64 has zero userspace.** `AddrSpace::new()` returns `None`
  (`kernel/src/arch/x86_64/paging.rs`), `enter_user` is `unreachable!`
  (`kernel/src/arch/x86_64/user.rs`). It cannot exec `/apps` at all — the kernel
  terminal's builtin interpreter is x86_64's only shell.
- **Diskless / no-`sync-apps` aarch64** has no `/apps/terminal`, so uterm cannot
  launch there either.

So the kernel terminal is not merely "window 0" — it is the only
**arch-independent, disk-independent** shell. Deleting it now would leave
x86_64 and diskless boots with no terminal and no way to get one.

**Decision (approved):** *flip the default, keep the fallback.* SP2 promotes
uterm to the boot default where it can run, and leaves the kernel terminal fully
intact as the fallback. The deletion is deferred behind an x86_64 userspace port
(see Future work) — that is the tracked endgame, not an abandoned goal.

## Scope

In scope:
1. Boot default = uterm, with a clean fallback to the kernel terminal.
2. Compositor respawn of the default terminal (resilience).
3. Migrate the smoke serial mirror to a debug-write syscall so the harness sees
   the userspace terminal's output.

Explicitly **not** in scope (deferred — see Future work): deleting the kernel
terminal, `textview`, the `App` trait, `TerminalApp`, or `monitor`. `monitor`
stays: the `App` trait stays anyway, and monitor is the only source of the
compositor FPS / input-rate telemetry (no userspace equivalent — `apps/top`
covers threads/heap but not compositor metrics).

## Design

### 1. Boot default = uterm, with fallback

`Shell::new` (`kernel/src/ui/shell/mod.rs`) currently opens the kernel
`TerminalApp` as window 0 — synchronous, needs no disk or `/apps`:

```rust
shell.open(Box::new(crate::apps::terminal::TerminalApp::new()), true);
```

Replace with a capability-gated choice:

- **On aarch64 with `/apps/terminal` readable** → launch uterm as the default
  window (the existing `launch_uterm` recipe: read the ELF, mint
  window+FS+PROC+brokers grants, `spawn_with_grants`, `extern_app::register(..,
  focus=true)`).
- **Otherwise** (x86_64, or `fs::read("/","/apps/terminal")` fails) → open the
  kernel `TerminalApp` exactly as today.

The gate is the same pair of conditions `launch_uterm` already encodes (the
aarch64 `cfg` + the `/apps/terminal` read), so the fallback is "whatever
`launch_uterm` would have refused to do."

**Boot-time behavior note.** `launch_uterm` registers the app on the
`SPAWN_QUEUE`; the window does not exist until `pump_externals` drains it and the
app sends `OP_OPEN` — a few frames after the splash. This is a brief post-splash
gap with no window, versus today's synchronous window 0. Acceptable (single-digit
frames); the wallpaper/backdrop is already drawn, so it reads as "desktop, then
terminal appears," not a blank screen.

### 2. Compositor respawn of the default terminal

Today `pump_externals` reaps an exited hosted window by removing it silently. For
an ordinary app that is correct. For the *default terminal* it would leave the
desktop with no shell.

The shell gains a record of the **default terminal's launch recipe** (enough to
re-invoke `launch_uterm`) and a small respawn policy:

- When the default-terminal window is reaped (its process exited or crashed),
  re-launch it.
- **Rate-limit** to avoid a crash-loop: track the time of the last respawn; if
  the terminal dies again within a short window (e.g. it never stayed up long
  enough to be usable), stop respawning uterm and **fall back to opening the
  kernel `TerminalApp`**. The user always ends with a working shell — never a
  spin, never a bare desktop.
- A clean exit (user typed `exit`/closed it deliberately) also respawns: the
  default terminal is the desktop's shell, like the kernel terminal already
  re-launches `sh` internally (`Terminal::pump`). (If a deliberate "close the
  desktop shell" gesture is ever wanted, that's future UX, not SP2.)

Only the **default** terminal respawns. Other windowed apps (and additional
uterm instances launched from the palette) reap silently as they do now — the
shell distinguishes the one window it launched as the boot default.

### 3. Migrate the smoke serial mirror to a debug-write syscall

The smoke harness (`tools/smoke/smoke.py`) asserts on `[out] …` lines. Today
those come from the kernel terminal: `Terminal::out` calls
`crate::smoke::mirror(&line)` (`kernel/src/term/mod.rs`), which — when the fw_cfg
flag `opt/tinyos/smoke` is set — echoes each scrollback line to serial as
`[out] {line}`. With uterm as the boot default, the shell's output flows through
the **userspace** terminal, which never touches `term::out`, so the harness would
go blind.

Add a minimal **debug-mirror syscall** and have the userspace terminal call it:

- **Kernel:** a new syscall (working name `SYS_DEBUG_MIRROR`) that takes a
  `(ptr, len)` string and, **only when smoke mode is active**, emits it to serial
  as `[out] {s}` via the same path `smoke::mirror` uses. When smoke mode is off it
  is a cheap no-op (one branch). Smoke mode is the existing fw_cfg flag the kernel
  already reads at boot (`smoke::init`).
- **Userspace terminal:** query smoke-active once at startup (a syscall/flag), and
  if active, call the mirror syscall for each console line it commits to
  scrollback — the userspace analogue of `Terminal::out`'s mirror call. Gating on
  a startup-cached flag keeps the per-line syscall out of normal (non-smoke) runs.
- **smoke.py:** the `[out] …` contract is preserved, so most assertions are
  unchanged. Any markers that were kernel-terminal-interpreter specific (e.g. the
  builtin `help`/`ps` text vs. sh's) are already sh-sourced in the current smoke
  (it hosts sh), so the shift should be minimal; reconcile any that differ.

This makes the smoke suite drive the **real** default — uterm hosting sh — end to
end, which is a stronger test than today's kernel-terminal-hosting-sh path.

Keep `smoke::mirror` and `Terminal::out`'s call to it intact (the kernel terminal
still exists and still mirrors on the fallback path); SP2 *adds* a second mirror
source, it does not move the existing one.

## Testing

- **`make smoke`**: boots into uterm (aarch64 + `/apps` present), drives sh over
  the console, asserts the `[out] …` round-trip via the new mirror. All existing
  steps (echo, help, ls, ps, run hello, fs write/cat, bg jobs, run top, run
  pixels detach, reboot durability, clean shutdown) must pass against uterm.
- **Respawn**: a smoke step that makes the default terminal exit and asserts a new
  one comes back (e.g. output round-trips again after the respawn), plus asserts
  no crash-loop.
- **Both arches**: `cargo check` aarch64 + x86_64. x86_64 must still boot to the
  kernel terminal (the fallback branch) — the flip must be a true no-op there.
- **Manual QEMU** (boot-critical + visual): `make run` on aarch64 comes up in the
  userspace terminal; kill it and confirm it respawns; confirm the palette,
  dock, monitor, and windowed apps still work.

## Known gaps / risks

- **Fallback path is smoke-untested.** With `/apps` present, smoke boots uterm, so
  the kernel-terminal fallback (x86_64 / diskless) is not exercised by the
  harness. A diskless / no-`sync-apps` smoke variant is future work; for now the
  fallback is covered by the fact that its code is unchanged.
- **Boot gap.** The few-frames no-terminal window after splash (§1). Cosmetic.
- **Duplicate terminal code remains.** Keeping the fallback means the kernel
  terminal + `textview` + builtin interpreter stay alongside the userspace
  terminal until the x86_64 port lands (Future work). Accepted cost of the
  "keep fallback" decision.

## Future work — the deletion endgame

The deletion goals from the original SP2 are **not abandoned**; they are gated on
one prerequisite:

- **Port userspace (EL0/ring-3) to x86_64** — implement `AddrSpace::new`, page
  table management, `enter_user`, and the syscall/trap entry for x86_64
  (`kernel/src/arch/x86_64/{paging,user}.rs`), mirroring the aarch64 path. Once
  x86_64 can exec `/apps`, uterm runs on every target and the disk-present case is
  the only remaining fallback concern.
- **Then, and only then, eliminate the kernel terminal**: delete
  `kernel/src/apps/terminal.rs`, `kernel/src/term/`, `kernel/src/ui/textview.rs`,
  collapse the `App` trait to `ExternApp`-only (or remove it), and move the smoke
  mirror wholly to the userspace path. Diskless boot would then either ship a
  minimal `/apps` in the image or accept a "no shell without disk" state.
- **`monitor`** can be reconsidered at that point: either keep it as the sole
  in-kernel `App` for compositor telemetry, or expose FPS/input-rate via a syscall
  so a userspace `top`/monitor can show it and the in-kernel `App` disappears too.

SP2 deliberately stops short of all of this: it delivers the user-facing win (the
userspace terminal is the OS) and the resilience/testing to stand on it, while
leaving a working fallback on every target until the x86_64 port removes the need
for one.

## Out of scope → later sub-projects

- SP3: move the compositor itself to userspace (framebuffer MemObj + input-event
  channel), per the architecture note.
- Live regions (`OP_LIVE_*`) — the deferred SP1d.
- Per-request broker reply channel before scoped FS (SP0 review debt).
