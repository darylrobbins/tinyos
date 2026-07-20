# Userspace terminal — line world (SP1a)

Date: 2026-07-19
Status: design, approved direction — pending written-spec review
Builds on: SP0 service brokers (merged, main 93f8bbe)
Precedes: SP1b (window broker + full-screen surfaces + live regions), SP2 (flip default, delete in-kernel terminal/App trait)

## Context

The terminal emulator is in the kernel (`kernel/src/term/mod.rs`, ~1200 lines) — it is both the text renderer (an in-kernel `App`) and the console-protocol server that hosts userspace apps. SP1 moves it to userspace. Per the agreed decomposition, SP1 is split:

- **SP1a (this spec):** a userspace terminal that opens a window, spawns `/apps/sh`, and renders the **line world** — scrollback of styled output plus an editable prompt line — with a generated monospace atlas. Launchable from the desktop, NOT the boot default. Delivers a real shell-in-a-window for all line commands.
- **SP1b (later):** full-screen cell surfaces (host `vi`/`top`), live regions, and a **window broker** so `sh`'s windowed children (`edit`, `pixels`) can each get their own compositor window.
- **SP2 (later):** flip the boot default to the userspace terminal, delete the in-kernel terminal + `App` trait + `textview` + `monitor`, migrate the smoke serial mirror.

SP0 already landed the FS/PROC brokers, so a userspace terminal can mint isolated FS/PROC connections for `sh` exactly as `sh` mints for its children.

## Goals

- A userspace app `apps/terminal` that: opens a compositor window; creates a console channel and spawns `/apps/sh` into it; runs the **line-world** console-protocol server; renders scrollback + an editable prompt with a monospace atlas; routes window keystrokes into line editing / child input; kills a hung foreground child with Ctrl+C.
- Launchable via a new desktop entry `uterm` (palette + dock). The in-kernel terminal stays the boot default and the `terminal` entry.
- Always-bootable and additive: nothing that boots today changes; the new terminal is a separate window.

## Non-goals (explicitly SP1b / SP2)

- Full-screen cell surfaces (`OP_SURFACE_*`) — hosting `vi`, `top`. SP1b.
- Live regions (`OP_LIVE_*`). SP1b.
- `sh`'s windowed children (`edit`, `pixels`) opening from the userspace terminal — needs a window broker (window channels are 1:1). SP1b. **In SP1a, `sh` is granted no window channel; `run edit`/`run pixels`/`run vi`/`run top` fail gracefully.** Plain line commands (`ls`, `ps`, `echo`, `cat`, `write`, `append`, `mkdir`, `rm`, `mv`, `cp`, `cd`, `pwd`, `help`, `fsinfo`, `sysinfo`, `ps`, `kill`, `spin`, `jobs`, bg jobs, `shutdown`/`reboot`) all work.
- Becoming the boot default; deleting the in-kernel terminal. SP2.
- Automated smoke coverage of the userspace terminal — it renders to its window, not serial, and adding a serial mirror needs the deferred debug-write syscall. SP1a is validated by host tests + manual QEMU; smoke still gates the (unchanged) in-kernel terminal.

## Detailed design

### Capabilities — how the launcher grants terminal-grade authority

Desktop apps spawned by the launcher (`SvcJob::spawn`, `kernel/src/ui/shell/svc.rs`) get jailed FS (`/data/<name>`), `can_kill=false` PROC, and no brokers — insufficient for a terminal. SP1a adds a dedicated launch path that grants the terminal what the in-kernel terminal has, minus console:

- `TAG_SHELL` — a window channel (app end of a fresh pair; kernel end registered with the compositor via `extern_app::register`), so the terminal can `window::open` its own window.
- `TAG_FS` — a fresh whole-root FS connection from `svc::mint_fs()` (to read `/apps/sh`).
- `TAG_PROC` — a fresh `can_kill` PROC connection from `svc::mint_proc()` (to kill a hung foreground child).
- `TAG_FS_BROKER` / `TAG_PROC_BROKER` — `svc::fs_broker_handle()` / `svc::proc_broker_handle()`, so the terminal mints `sh`'s connections.
- No `TAG_CONSOLE`: the terminal is a console *provider*, not consumer.

This is `term::spawn_app`'s grant list minus console. It uses `loader::spawn_with_grants` (the `GrantSet`/`SvcJob` path can't express brokers). Like `term::spawn_app`, this is a **privileged launch that grants explicitly** — the app's `declare_caps!` manifest is NOT intersected on this path, so the terminal receives exactly the grants above regardless of manifest. The terminal should still `declare_caps!(b"window\nproc\nfs:self")`-style for hygiene/documentation, but grants come from `launch_uterm`. Because the terminal's FS/PROC are served by the standing kernel servers (`svc::pump`), the launcher does NOT pump per-app services for it — it only spawns and registers the window channel.

### Launch entry

`Shell::open_named` (`kernel/src/ui/shell/mod.rs`) gains a `"uterm"` arm calling a new `Shell::launch_uterm()` that performs the spawn+grant+register above. The command palette (`palette.rs`) and dock gain a `uterm` entry mapping to `Action::Open("uterm")`. The existing `"terminal"` arm (in-kernel `TerminalApp`) is unchanged.

### The monospace atlas (SDK)

`apps/sdk/src/monofont.rs` (generated, do-not-edit) — a fixed-advance atlas baked from `assets/geistmono.ttf`, printable ASCII, 8bpp alpha coverage, same `UiGlyph { w,h,ox,oy,adv,data }` shape as `uifont.rs`, with `ASCENT`/`LINE_H`/`ADVANCE` constants (target ~9px advance, 19px line — matching the in-kernel terminal's `CELL_W=9,CELL_H=19`). Generated via the existing `apps/solitaire/genglyphs.swift` toolchain (a mono variant that emits a constant advance). `gfx::Canvas` gains `draw_mono_text(x, y, s, color)` that lays glyphs at fixed `ADVANCE` using the existing `draw_alpha_mask` primitive (the same one `draw_ui_text` uses for `uifont`). Cell metrics `CELL_W`/`CELL_H` derive from the atlas.

### Scrollback + line editing (SDK or app module)

A `scrollback` model (host-testable, pure): a ring of frozen styled lines (`(String, color)`), an editable current input line with a cursor, and the app's prompt spans (`Vec<(String, color)>` from `OP_SET_PROMPT`). Operations: `push_line`, `insert_char`, `backspace`, `left`/`right`, `set_prompt`, `clear`, `take_input` (returns the typed line on Enter and clears it), and a `render(canvas, width, height, mono)` that lays out the visible tail bottom-anchored, soft-wrapping to window width, drawing the prompt + input + a cursor block. Long history is capped (e.g. 400 lines) like the kernel `TextView`.

### The line-world console server (app module)

The terminal drains `sh`'s console channel each iteration and dispatches the **line-world subset** of `abi::console` (the same opcodes the kernel `Terminal::pump` handles, ported to userspace, minus SURFACE_*/LIVE_*):

- `OP_HELLO` → reply `OP_HELLO_ACK` (protocol v1, no features).
- `OP_WRITE` / `OP_WRITE_STYLED` → accumulate into a partial line; on `\n` freeze into scrollback (color = FG or the styled color).
- `OP_SET_PROMPT` → parse the colored spans into the scrollback's prompt.
- `OP_CLEAR` → clear scrollback.
- `OP_SET_INPUT_MODE` → track LINES vs KEYS (SP1a is LINES-centric; KEYS forwarding is trivial and included).
- `OP_SET_FOREGROUND` → record the foreground child tid (for Ctrl+C).
- `OP_SURFACE_*` / `OP_LIVE_*` → **acknowledged/ignored** in SP1a (no surface). A child that opens a surface (vi/top) simply won't render; since `sh` can't launch those without a window channel anyway, this path is unreached in SP1a.
- Size: send `OP_RESIZE(cols, rows)` once after spawn and on window resize, computed from window pixels / cell metrics.

Input: window `Event::Char` → line edit (LINES) or `OP_CHAR` (KEYS); `Event::Key` → cursor/history keys or `OP_KEY`; on Enter (LINES) send `OP_INPUT_LINE` with the typed text; `Event::Ctrl(c)` where c is 'C' → `proc::kill(foreground_tid)` via the terminal's PROC connection (mirrors the in-kernel `on_ctrl_key`).

### The terminal app main loop

```
open window (TAG_SHELL)
create console channel pair (SDK channel::create)
spawn /apps/sh granting: TAG_CONSOLE=client end, TAG_FS=broker::connect(fs_broker),
  TAG_PROC=broker::connect(proc_broker), TAG_FS_BROKER=dup, TAG_PROC_BROKER=dup
  (NO TAG_SHELL — line-world boundary)
loop:
  poll window events -> line edit / OP_CHAR/OP_KEY / OP_INPUT_LINE / Ctrl+C kill
  pump console server (drain sh writes -> scrollback; answer HELLO/RESIZE)
  if dirty: render scrollback into window surface (Canvas + mono atlas), present
  wait on window channel readable OR console channel readable (bounded)
```

Note the terminal must wake on **either** its window channel (keystrokes) or `sh`'s console channel (output). It waits on both via the SDK wait primitive (`wait_many` / the SDK `wait` helper over both handles).

## Data flow

Desktop `uterm` → `Shell::launch_uterm` spawns `/apps/terminal` (ExternApp window) with window+FS+PROC+brokers → terminal opens window, spawns `sh` into a console it serves → keystroke: compositor→terminal window (`OP_CHAR`) → line edit → Enter → `OP_INPUT_LINE`→sh → `sh` runs `ps` → `OP_WRITE_STYLED`→terminal scrollback → render into the window's shared framebuffer → compositor blits (zero-copy, like any ExternApp).

## Testing

- **Host tests (pure):** the `scrollback` model (insert/backspace/wrap/history/take_input/clear) and mono metrics — like `vicore`/`textui`, run under `make test`.
- **Manual QEMU (`make run`):** launch `uterm` from the palette; confirm a window opens with the Meridian prompt, type `help`/`ls`/`ps`/`echo`, confirm `sh` responds and output renders in the mono font; confirm Ctrl+C returns to a prompt; confirm `run edit` fails gracefully (no window, an error line).
- **Both arches** compile (`cargo check` aarch64 + x86_64). The new app is aarch64-only (userspace is aarch64-first); the kernel launch path must compile on both (guard the spawn like `term::spawn_app` is `#[cfg(target_arch="aarch64")]`).
- Automated smoke of the userspace terminal is deferred to SP2 (needs the debug-write serial mirror when it becomes default). The existing `make smoke` continues to gate the unchanged in-kernel terminal.

## Registration

- `apps/Cargo.toml` members += `"terminal"`; `Makefile` `APP_BINS` += `terminal`; baked into the disk by `make sync-apps`.
- New app crate `apps/terminal` (bin name `terminal`).

## Risks

- **Mono atlas generation is macOS/Swift** (the `genglyphs.swift` toolchain). Generated output is committed like `uifont.rs`; regeneration is a documented one-off. Confirm the mono variant emits a constant advance and covers printable ASCII.
- **Keyboard focus:** the window protocol delivers keys only to the focused window. On launch the terminal becomes focused (top window). If focus is elsewhere, no keys — expected windowing behavior, not a bug.
- **Dual-wake:** the loop must wake on window OR console readability; a naive wait on only one starves the other (a known class of bug here — cf. the SP0/smoke runtime bugs). Wait on both handles.
- **Console server parity:** the line-world opcode handling is ported from the well-exercised kernel `Terminal::pump`; keep the semantics identical (partial-line-as-prompt fallback, styled colors, prompt spans) so behavior matches the in-kernel terminal.
- **Performance:** re-render the whole window on change (one BGRA blit per present, like every ExternApp). Fine at this scale.

## Out of scope → future

- SP1b: window broker (per-child window channels), full-screen surface hosting (`vi`/`top`), live regions, `run edit`/`pixels`/`vi`/`top` from the userspace terminal.
- SP2: flip boot default to the userspace terminal, delete in-kernel `term`/`textview`/`App` trait/`monitor`, migrate the smoke serial mirror (debug-write syscall) so `make smoke` drives the userspace terminal.
- Also pending (from the SP0 review): per-request broker reply channel, needed before any scoped-FS / read-only-PROC policy.
