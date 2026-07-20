# Userspace Terminal — Line World (SP1a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A launchable (non-default) userspace terminal app that opens a window, spawns `/apps/sh` into a console it serves, and renders the line world (scrollback + editable prompt) with a generated monospace atlas.

**Architecture:** Pure line-world logic (scrollback, line editor, console-protocol server state machine) lives in a new host-tested crate `crates/termcore` — mirroring `vicore`/`textui`. The `apps/terminal` binary is the I/O wrapper: it opens a window (SDK `window`), creates a console channel, spawns `sh` (SDK `process` + `broker`), drives `termcore`, and renders its state with a new `Canvas::draw_mono_text` + generated `monofont`. The kernel gains one launch path (`Shell::launch_uterm`) and a `uterm` desktop entry; the in-kernel terminal stays the boot default.

**Tech Stack:** Rust `no_std` (kernel: `aarch64-unknown-uefi` + `x86_64-unknown-uefi`; apps: `aarch64-unknown-none`; `termcore`: host-testable `no_std` lib). `tinyos-abi`, SP0 FS/PROC brokers, `make` + QEMU.

## Global Constraints

- `no_std` everywhere. `crates/termcore` is `#![cfg_attr(not(test), no_std)]` like `crates/textui`, depends only on `tinyos-abi` (+ `alloc`), NO kernel/SDK deps — so it host-tests.
- Both kernel targets compile: `cargo check -p kernel --target aarch64-unknown-uefi` AND `--target x86_64-unknown-uefi`. The kernel launch path is aarch64-only spawn logic — guard with `#[cfg(target_arch = "aarch64")]` exactly as `kernel/src/term/mod.rs::spawn_app` does.
- The new app is aarch64-only (`apps/` workspace is `aarch64-unknown-none`).
- Line-world boundary: the terminal grants `sh` NO window channel (`TAG_SHELL`), so `sh`'s windowed/surface children don't open — this is intended for SP1a. Do NOT forward `TAG_SHELL` to `sh`.
- Terminal-grade grants come from `launch_uterm` via `loader::spawn_with_grants` (privileged, manifest NOT intersected): `TAG_SHELL`(window) + `TAG_FS`=`svc::mint_fs()` + `TAG_PROC`=`svc::mint_proc()` + `TAG_FS_BROKER`=`svc::fs_broker_handle()` + `TAG_PROC_BROKER`=`svc::proc_broker_handle()`. No `TAG_CONSOLE`. All handles carry `RIGHTS_ALL`.
- Cell metrics match the in-kernel terminal: `CELL_W=9`, `CELL_H=19` (mono atlas advance 9, line height 19).
- Meridian palette colors (match `apps/shell`): FG=`abi::tokens::TX`, ACCENT=`abi::tokens::ACC`, DIM=`abi::tokens::TX3`.
- Commit trailer on its own line after a blank line: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Spec: `docs/superpowers/specs/2026-07-19-userspace-terminal-sp1a-design.md`.

---

## File structure

- `apps/sdk/src/monofont.rs` (create, generated) — GeistMono atlas, `UiGlyph`-shaped, `GLYPHS: [&UiGlyph;95]`, `ASCENT`/`LINE_H`/`ADVANCE`.
- `apps/sdk/src/gfx.rs` (modify) — `Canvas::draw_mono_text`, `measure_mono_text`.
- `apps/sdk/src/window.rs` (modify) — `Window::handle()` accessor for dual-wake.
- `apps/sdk/src/lib.rs` (modify) — `pub mod monofont;`.
- `crates/termcore/{Cargo.toml,src/lib.rs}` (create) — pure line-world model + tests.
- `crates/Cargo.toml` or workspace root (modify) — register `termcore` in the workspace.
- `apps/terminal/{Cargo.toml,src/main.rs}` (create) — the terminal binary.
- `apps/Cargo.toml` (modify) — add `"terminal"` to members.
- `Makefile` (modify) — add `terminal` to `APP_BINS`.
- `kernel/src/ui/shell/mod.rs` (modify) — `launch_uterm()`, `"uterm"` arm in `open_named`.
- `kernel/src/ui/shell/palette.rs` (modify) — `uterm` palette entry.
- `tools/smoke/smoke.py` (modify) — launch-path smoke step.

---

### Task 1: SDK — monospace atlas + rendering primitives + Window accessor

**Files:**
- Create: `apps/sdk/src/monofont.rs` (generated)
- Modify: `apps/sdk/src/lib.rs` (module list, after `pub mod gfx;`), `apps/sdk/src/gfx.rs` (add methods), `apps/sdk/src/window.rs` (add accessor)

**Interfaces:**
- Produces: `tinyos_app::monofont::{GLYPHS: [&UiGlyph;95], ASCENT: i32, LINE_H: i32, ADVANCE: i32}` (reuse `uifont::UiGlyph` type); `Canvas::draw_mono_text(&mut self, x: i32, y: i32, s: &str, color: u32)`; `gfx::measure_mono_text(s: &str) -> (i32, i32)`; `Window::handle(&self) -> u32`.

- [ ] **Step 1: Generate the mono atlas**

`monofont.rs` is generated from `assets/geistmono.ttf` using the existing `apps/solitaire/genglyphs.swift` toolchain (which emits the `UiGlyph`-shaped alpha atlas that `uifont.rs` uses). Run it targeting a 15px render (GeistMono at 15px gives a ~9px advance):

```bash
swift apps/solitaire/genglyphs.swift assets/geistmono.ttf /dev/null apps/sdk/src/monofont.rs
```

Then hand-edit the generated `monofont.rs` header + add the fixed-advance constant (GeistMono is monospace, so every glyph's `adv` is equal; expose it as `ADVANCE`):
```rust
//! Generated monospace font — do not edit the glyph data by hand.
//! GeistMono 15px, printable ASCII, 8bpp alpha, baseline metrics.
//! Regenerate: swift apps/solitaire/genglyphs.swift assets/geistmono.ttf \
//!     /dev/null apps/sdk/src/monofont.rs   (then re-add ADVANCE)
pub use crate::uifont::UiGlyph;
pub const ASCENT: i32 = 15;
pub const LINE_H: i32 = 19;
pub const ADVANCE: i32 = 9;   // fixed monospace cell width
// ... generated static G32..G126 and `pub static GLYPHS: [&UiGlyph; 95] = [...]`
```

(`swift` is confirmed available on the build host — Apple Swift 6.3.) If a re-run ever finds it missing, this is the one manual asset step — flag BLOCKED and request the generated file rather than hand-authoring glyph bitmaps.

- [ ] **Step 2: Register the module**

`apps/sdk/src/lib.rs` — add after `pub mod gfx;` (line 25):
```rust
pub mod monofont;
```

- [ ] **Step 3: Add `draw_mono_text` + `measure_mono_text`**

`apps/sdk/src/gfx.rs` — add a new `impl<'a> Canvas<'a>` block near `draw_ui_text` (mirrors it, but a FIXED advance so columns align):
```rust
impl<'a> Canvas<'a> {
    /// Draw `s` with the monospace atlas at a fixed cell advance; `y` is the
    /// top of the line box. Non-ASCII chars occupy a cell but draw nothing.
    pub fn draw_mono_text(&mut self, x: i32, y: i32, s: &str, color: u32) {
        let baseline = y + crate::monofont::ASCENT;
        let mut pen = x;
        for ch in s.chars() {
            if let Some(g) = crate::monofont::GLYPHS.get((ch as usize).wrapping_sub(32)) {
                if g.w > 0 {
                    self.draw_alpha_mask(pen + g.ox, baseline - g.oy, g.data, g.w, g.h, color);
                }
            }
            pen += crate::monofont::ADVANCE;
        }
    }
}

/// Pixel size of `s` drawn with `draw_mono_text`.
pub fn measure_mono_text(s: &str) -> (i32, i32) {
    (s.chars().count() as i32 * crate::monofont::ADVANCE, crate::monofont::LINE_H)
}
```

- [ ] **Step 4: Add the `Window::handle` accessor**

`apps/sdk/src/window.rs` — inside `impl Window`, after `wait`:
```rust
    /// The raw channel handle, for building a combined wait_many across the
    /// window channel and (e.g.) a hosted app's console channel.
    pub fn handle(&self) -> u32 {
        self.ch.0
    }
```

- [ ] **Step 5: Build the apps workspace**

Run: `cd apps && cargo build --release`
Expected: `Finished`. (The new symbols are unused until Task 3 — dead-code warnings are fine.)

- [ ] **Step 6: Commit**
```bash
git add apps/sdk/src/monofont.rs apps/sdk/src/lib.rs apps/sdk/src/gfx.rs apps/sdk/src/window.rs
git commit -m "sdk: monospace atlas + draw_mono_text + Window::handle

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `crates/termcore` — pure line-world model (host-tested)

**Files:**
- Create: `crates/termcore/Cargo.toml`, `crates/termcore/src/lib.rs`
- Modify: root `Cargo.toml` (`[workspace] members` — add `"crates/termcore"`), `Makefile` (`test` target — add `cargo test -p termcore` after the `cargo test -p vicore` line)

**Interfaces:**
- Consumes: `abi::console::*` opcodes (OP_WRITE=1, OP_HELLO=2, OP_SET_INPUT_MODE=3, OP_WRITE_STYLED=11, OP_CLEAR=12, OP_SET_PROMPT=13, OP_SET_FOREGROUND=14, OP_INPUT_LINE=16, OP_KEY=17, OP_CHAR=18, OP_RESIZE=19, OP_HELLO_ACK=23; INPUT_MODE_LINES=0, INPUT_MODE_KEYS=1).
- Produces: `termcore::Term` with the API below. `apps/terminal` (Task 3) drives it.

**Design.** `Term` is the pure state of the line world — it never does I/O. The app feeds it (a) inbound console-message bytes from `sh` and (b) keystrokes, and reads back (c) the scrollback+prompt+input for rendering and (d) outbound console-message bytes to send to `sh`. This is the line-world subset of `kernel/src/term/mod.rs::pump` (lines ~797-1040) and `on_char`/`on_key`/`app_char` (lines ~176-262), ported to a pure struct. SURFACE_*/LIVE_* are intentionally dropped (SP1b).

```rust
pub struct Line { pub text: String, pub color: u32 }       // one frozen scrollback line
pub struct Term {
    scrollback: alloc::collections::VecDeque<Line>,        // capped at SCROLLBACK=400
    prompt: Vec<(String, u32)>,                            // colored prompt spans
    input: String,                                         // current edit line
    cursor: usize,                                         // byte index into input (ASCII)
    partial: String,                                       // unterminated OP_WRITE bytes
    partial_color: u32,
    mode: u32,                                             // INPUT_MODE_LINES/KEYS
    foreground_tid: u32,                                   // for the app's Ctrl+C
    cols: usize, rows: usize,
    out: Vec<Vec<u8>>,                                     // queued outbound messages
    dirty: bool,
}
impl Term {
    pub fn new() -> Self;
    /// Feed one inbound console message from the child (bytes only; SP1a
    /// ignores any moved handles). Updates scrollback/prompt/mode/foreground,
    /// may queue a HELLO_ACK. Sets dirty on any visible change.
    pub fn on_console_msg(&mut self, bytes: &[u8]);
    /// A typed character from the window. LINES: local edit; KEYS: queue OP_CHAR.
    pub fn on_char(&mut self, c: char);
    /// A non-char key (backspace/left/right/enter handled; others -> OP_KEY in KEYS).
    pub fn on_key(&mut self, code: u16);
    /// Set terminal size in cells; queues OP_RESIZE if changed.
    pub fn set_size(&mut self, cols: usize, rows: usize);
    /// The foreground child tid the app should target for Ctrl+C (0 = none).
    pub fn foreground_tid(&self) -> u32;
    /// Drain queued outbound messages (each a full console-protocol frame).
    pub fn take_outbound(&mut self) -> Vec<Vec<u8>>;
    /// True since the last render; cleared by `clear_dirty`.
    pub fn dirty(&self) -> bool;
    pub fn clear_dirty(&mut self);
    /// Read models for rendering (Task 3 lays these out with the mono atlas).
    pub fn scrollback(&self) -> impl Iterator<Item = &Line>;
    pub fn prompt(&self) -> &[(String, u32)];
    pub fn input(&self) -> &str;
    pub fn cursor(&self) -> usize;
}
```

Behavior to port (match the kernel terminal exactly):
- `on_console_msg`: read `op = u32 le at [0..4]`. `OP_WRITE`: color=FG, push chars to `partial`, on `\n` freeze `(partial, FG)` into scrollback. `OP_WRITE_STYLED`: fg = le[4..8], utf8 from [8..], same freeze with fg. `OP_CLEAR`: clear scrollback. `OP_SET_PROMPT`: parse `count` then `count` spans of `{fg:u32,len:u32,utf8}` into `prompt`. `OP_HELLO`: queue `OP_HELLO_ACK ++ 1u32 ++ 0u32`. `OP_SET_INPUT_MODE`: `mode = le[4..8]`. `OP_SET_FOREGROUND`: `foreground_tid = le[4..8]`. Any SURFACE_*/LIVE_* op: ignore. Set `dirty` on visible changes.
- `on_char` LINES: `'\n'` → freeze the echoed prompt+input line into scrollback (DIM), queue `OP_INPUT_LINE ++ input.as_bytes()`, clear input/cursor. printable → insert at cursor. `on_char` KEYS: queue `OP_CHAR ++ (c as u32)`.
- `on_key` LINES: backspace/left/right edit `input`/`cursor`; enter same as `'\n'`. KEYS: queue `OP_KEY ++ code:u16 ++ 1u8 ++ 0u8`.
- Use `abi::keys` codes for backspace/left/right (the SDK exposes them; import from `abi`).

- [ ] **Step 1: Create the crate + register in the workspace**

`crates/termcore/Cargo.toml`:
```toml
[package]
name = "termcore"
version = "0.1.0"
edition = "2021"

[dependencies]
tinyos-abi = { path = "../abi" }
```
Root `Cargo.toml` — add `"crates/termcore"` to `[workspace] members` (currently `["kernel", "crates/abi", "crates/textui", "crates/tinyfs", "crates/vicore", "tools/mkfs-tinyfs"]`).
`Makefile` — in the `test` target, add `cargo test -p termcore` right after the `cargo test -p vicore` line (~line 98), so `make test` runs the model's host tests.

- [ ] **Step 2: Write a failing test for OP_WRITE_STYLED scrollback**

`crates/termcore/src/lib.rs` (test module):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use abi::console::{OP_WRITE_STYLED, OP_INPUT_LINE};

    fn styled(fg: u32, s: &str) -> Vec<u8> {
        let mut b = OP_WRITE_STYLED.to_le_bytes().to_vec();
        b.extend_from_slice(&fg.to_le_bytes());
        b.extend_from_slice(s.as_bytes());
        b
    }

    #[test]
    fn write_styled_freezes_line_on_newline() {
        let mut t = Term::new();
        t.on_console_msg(&styled(0xAABBCC, "hello\n"));
        let lines: Vec<_> = t.scrollback().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[0].color, 0xAABBCC);
    }

    #[test]
    fn enter_queues_input_line_and_echoes() {
        let mut t = Term::new();
        for c in "ls /apps".chars() { t.on_char(c); }
        t.on_char('\n');
        let out = t.take_outbound();
        assert_eq!(out.len(), 1);
        let op = u32::from_le_bytes(out[0][0..4].try_into().unwrap());
        assert_eq!(op, OP_INPUT_LINE);
        assert_eq!(&out[0][4..], b"ls /apps");
        assert_eq!(t.input(), "");
    }
}
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p termcore`
Expected: FAIL (Term / methods not defined).

- [ ] **Step 4: Implement `Term`**

Write `crates/termcore/src/lib.rs` (`#![cfg_attr(not(test), no_std)]`, `extern crate alloc;`) implementing the API and behavior above. Port the opcode handling from `kernel/src/term/mod.rs` (read lines 797-1040 and 176-262 for exact semantics; drop SURFACE_*/LIVE_*).

- [ ] **Step 5: Run the tests to green (add edge-case tests)**

Add tests for: `OP_SET_PROMPT` parsing, backspace/left/right editing, KEYS-mode `on_char` queuing `OP_CHAR`, `set_size` queuing `OP_RESIZE` only on change, `OP_SET_FOREGROUND` updating `foreground_tid`, and scrollback cap at 400.
Run: `cargo test -p termcore`
Expected: PASS (all).

- [ ] **Step 6: Commit**
```bash
git add crates/termcore Cargo.toml
git commit -m "termcore: pure line-world model (scrollback, line edit, console server)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `apps/terminal` — the terminal binary

**Files:**
- Create: `apps/terminal/Cargo.toml`, `apps/terminal/src/main.rs`
- Modify: `apps/Cargo.toml` (members += `"terminal"`), `Makefile` (`APP_BINS` += `terminal`)

**Interfaces:**
- Consumes: `termcore::Term` (Task 2); `tinyos_app::{window::{Window, Event}, gfx::Canvas, monofont, channel::Channel, process, broker, entry::Env, wait::{wait_many, WaitItem}}`; the mono metrics (`CELL_W = monofont::ADVANCE`, `CELL_H = monofont::LINE_H`).
- Produces: the `/apps/terminal` binary.

**Design.** `main(env)`:
1. Open a window: `Window::open(env.shell, W, H, "terminal")` (e.g. 720×456 → 80×24 cells).
2. Create the console channel: `let (con_app, con_kern) = Channel::create()?;` — `con_app` (client) goes to `sh`, `con_kern` (server) stays here.
3. Spawn `sh`: read `/apps/sh` via `tinyos_app::fs::read`, then `process::spawn(&elf, &[], &grants)` where grants = `[(TAG_CONSOLE, con_app.0), (TAG_FS, broker::connect(env.fs_broker)?.0), (TAG_PROC, broker::connect(env.proc_broker)?.0), (TAG_FS_BROKER, dup(env.fs_broker)), (TAG_PROC_BROKER, dup(env.proc_broker))]`. NO `TAG_SHELL`.
4. `let mut term = Term::new(); term.set_size(cols, rows);` push the initial `OP_RESIZE` to `sh` via `con_kern.send`.
5. Loop:
   - `window.poll_events(&mut evs)`; for each: `Char(c)`→`term.on_char(c)`; `Key{code,down:true}`→`term.on_key(code)`; `Ctrl('C' as u16 or KEY_C)`→`tinyos_app::proc::kill(term.foreground_tid())`; `CloseRequested`→break.
   - drain `con_kern.try_recv()` → `term.on_console_msg(&msg.bytes)`.
   - `for m in term.take_outbound() { con_kern.send(&m, &[]); }`.
   - if `term.dirty()`: render into a back buffer with `Canvas` + `draw_mono_text` (bottom-anchored scrollback tail, then prompt spans + input + a cursor block), `window.present_from(&back)`, `term.clear_dirty()`.
   - dual-wake: `wait_many(&mut [WaitItem{handle: window.handle(), want: SIG_READABLE, observed:0}, WaitItem{handle: con_kern.0, want: SIG_READABLE, observed:0}], deadline)` — wake on window keys OR `sh` output.
6. `declare_caps!(b"window\nproc\nfs:self")` (hygiene; grants come from `launch_uterm`).

- [ ] **Step 1: Create the crate**

`apps/terminal/Cargo.toml` (copy the shape of `apps/pixels/Cargo.toml`; bin name `terminal`; deps `tinyos-app`, `tinyos-abi`, `termcore`).

- [ ] **Step 2: Write `main.rs`**

Implement the loop above. Rendering: clear back buffer to the Meridian bg; lay out the last `rows-1` scrollback lines with `draw_mono_text` at `CELL_H` spacing; draw the prompt spans then `input` on the bottom line; draw a filled cursor cell at the cursor column. Use FG/ACCENT/DIM from `abi::tokens`.

- [ ] **Step 3: Register the app**

`apps/Cargo.toml` — add `"terminal"` to `members`.
`Makefile` — add `terminal` to `APP_BINS` (line 72).

- [ ] **Step 4: Build the apps workspace**

Run: `cd apps && cargo build --release`
Expected: `Finished`, and `apps/target/aarch64-unknown-none/release/terminal` exists.

- [ ] **Step 5: Commit**
```bash
git add apps/terminal apps/Cargo.toml Makefile
git commit -m "apps: userspace terminal (line world) hosting sh in a window

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Kernel — `launch_uterm` + desktop entry

**Files:**
- Modify: `kernel/src/ui/shell/mod.rs` (add `launch_uterm`; `"uterm"` arm in `open_named` at line ~507), `kernel/src/ui/shell/palette.rs` (add `uterm` to the recognized commands ~line 133-140)

**Interfaces:**
- Consumes: `crate::svc::{mint_fs, mint_proc, fs_broker_handle, proc_broker_handle}`; `crate::obj::loader::spawn_with_grants`; `crate::obj::channel::create`; `crate::obj::handle::{Handle, RIGHTS_ALL}`; `crate::obj::Object::Channel`; `crate::ui::shell::extern_app::register`; `abi::bootstrap::{TAG_SHELL, TAG_FS, TAG_PROC, TAG_FS_BROKER, TAG_PROC_BROKER}`.

- [ ] **Step 1: Add `launch_uterm`**

`kernel/src/ui/shell/mod.rs` — add a method on `Shell` (near `launch_app`):
```rust
    /// Launch the userspace terminal (/apps/terminal) as a top-level window
    /// with terminal-grade grants: window + a broker-minted whole-root FS and
    /// can_kill PROC + the FS/PROC brokers (to mint sh's connections). No
    /// console — it creates its own to serve sh. aarch64 only.
    #[cfg(target_arch = "aarch64")]
    pub fn launch_uterm(&mut self) {
        use crate::obj::channel::create;
        use crate::obj::handle::{Handle, RIGHTS_ALL};
        use crate::obj::Object;
        use abi::bootstrap::{TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL};
        let elf = match crate::fs::read("/", "/apps/terminal") {
            Ok(e) => e,
            Err(e) => { kprintln!("uterm: /apps/terminal: {e}"); return; }
        };
        let (shell_app, shell_kern) = create();
        let grants = alloc::vec![
            (TAG_SHELL, Handle::new(Object::Channel(shell_app), RIGHTS_ALL)),
            (TAG_FS, crate::svc::mint_fs()),
            (TAG_PROC, crate::svc::mint_proc()),
            (TAG_FS_BROKER, crate::svc::fs_broker_handle()),
            (TAG_PROC_BROKER, crate::svc::proc_broker_handle()),
        ];
        match crate::obj::loader::spawn_with_grants("terminal".into(), &elf, &[], grants) {
            Ok((_p, tid, _main)) => {
                crate::ui::shell::extern_app::register(shell_kern, "terminal".into());
                kprintln!("tinyos: uterm launched (thread {tid})");
            }
            Err(e) => kprintln!("uterm: spawn failed: {}", e.msg()),
        }
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub fn launch_uterm(&mut self) {
        kprintln!("uterm: userspace unsupported on this arch");
    }
```

- [ ] **Step 2: Route the `uterm` name**

`kernel/src/ui/shell/mod.rs` — in `open_named`, add to the `match name` (after the `"clock" | "solitaire" | "pixels"` arm, line ~507):
```rust
            "uterm" => self.launch_uterm(),
```

- [ ] **Step 3: Recognize `uterm` in the palette**

`kernel/src/ui/shell/palette.rs` — in `submit` (line ~133), add `"uterm"` to the recognized set and map it:
```rust
            "terminal" | "uterm" | "notes" | "monitor" | "clock" | "solitaire" | "pixels" => {
                Action::Open(match cmd.as_str() {
                    "terminal" => "terminal",
                    "uterm" => "uterm",
                    "notes" => "notes",
                    "monitor" => "monitor",
                    "solitaire" => "solitaire",
                    "pixels" => "pixels",
                    _ => "clock",
                })
            }
```

- [ ] **Step 4: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.

- [ ] **Step 5: Commit**
```bash
git add kernel/src/ui/shell/mod.rs kernel/src/ui/shell/palette.rs
git commit -m "shell: launch_uterm — userspace terminal via palette 'uterm'

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Integration — build the disk, verify the launch path boots

**Files:**
- Modify: `tools/smoke/smoke.py` (add a launch-path step)

**Interfaces:** Consumes the whole system (Tasks 1-4).

Note on scope: the userspace terminal renders to its window (framebuffer), which the serial-based smoke harness cannot read. So the automated gate here proves the **launch path** end-to-end (palette → `launch_uterm` → spawn `/apps/terminal` → it spawns `sh`) without a kernel panic or wedge. Visual/interaction correctness (the mono prompt renders, typing `ls` shows output) is verified manually in `make run` and is called out for the human reviewer.

- [ ] **Step 1: Build the disk**

Run: `make sync-apps`
Expected: builds kernel + apps (incl. `terminal`), bakes them into `disk.img`, no errors.

- [ ] **Step 2: Confirm the existing smoke still passes (in-kernel terminal unchanged)**

Run: `make smoke`
Expected: `smoke: PASS` — SP1a changed nothing in the boot-default path.

- [ ] **Step 3: Add a launch-path smoke step**

`tools/smoke/smoke.py` — after the shell is up and before `shutdown`, drive the command palette to launch `uterm` and assert the kernel logged the launch with no panic. The palette opens on Ctrl+K; type `uterm`; Enter. Insert (using the harness's `qmp` + `serial` objects; Ctrl+K is `["ctrl","k"]`):
```python
        # Launch the userspace terminal from the palette and confirm the
        # launch path spawned it (renders to its window; serial only sees the
        # kernel-side launch marker + that nothing panicked).
        print("smoke: > (Ctrl+K) uterm")
        qmp.key(["ctrl", "k"])
        time.sleep(0.4)
        qmp.type_line("uterm")
        cur = serial.wait_for("uterm launched", args.step_timeout, cur)
        time.sleep(0.6)   # let /apps/terminal spawn sh
        if serial.panic:
            raise AssertionError("panic after launching uterm")
        print("smoke: uterm launched cleanly")
```
(If `type_line`'s per-char keys land in the palette text field correctly, `uterm` + Enter submits. The palette `submit` maps `uterm` → `Action::Open("uterm")` → `launch_uterm`, which logs `tinyos: uterm launched`.)

- [ ] **Step 4: Run smoke with the new step**

Run: `make smoke`
Expected: `smoke: PASS`, including `smoke: uterm launched cleanly` and the serial line `tinyos: uterm launched (thread N)`, no panic.

- [ ] **Step 5: Manual verification checklist (for the human reviewer — document in the commit)**

`make run`, then: Ctrl+K → type `uterm` → Enter. Confirm: a window titled "terminal" opens; the Meridian prompt renders in the mono font; typing `help`/`ls`/`ps`/`echo hi` shows `sh`'s output in the window; Ctrl+C returns to a prompt; `run edit` prints an error (no window — expected in SP1a); the in-kernel `terminal` still works independently.

- [ ] **Step 6: Final both-arch check + commit**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.
```bash
git add tools/smoke/smoke.py
git commit -m "smoke: launch-path check for the userspace terminal (uterm)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** mono atlas + primitives (Task 1), pure line-world model (Task 2), the terminal binary (Task 3), launcher wiring + `uterm` entry (Task 4), integration/launch-path gate + manual checklist (Task 5). Non-goals (surfaces, live regions, windowed children, boot default, serial mirror) are untouched by design.
- **Line-world boundary honored:** Task 3 grants `sh` no `TAG_SHELL`; the plan says so explicitly in the constraints and Task 3 grant list.
- **Same-thread mint safety:** `launch_uterm` uses `svc::mint_fs/mint_proc` DIRECTLY (in-kernel, on the ui_thread) — never a broker channel round-trip — consistent with SP0.
- **Testing honesty:** `termcore` is genuinely host-tested (Task 2); the integrated terminal's rendering is manually verified (Task 5 step 5) because it draws to the framebuffer, not serial — the automated step proves only the launch path.
- **Types consistent:** `Term` methods (Task 2) match their call sites in Task 3; `monofont::{GLYPHS,ASCENT,LINE_H,ADVANCE}` (Task 1) match `draw_mono_text`'s use; grant tuples are `(u32, Handle)` for `spawn_with_grants` (kernel, Task 4) and `(u32, u32)` for `process::spawn` (userspace, Task 3).
- **Risk — mono atlas generation:** Task 1 Step 1 depends on `swift`; flagged as the one manual-asset BLOCKED path if unavailable.
