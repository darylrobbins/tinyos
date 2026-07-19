# vi-compatible editor — design

Date: 2026-07-19
Status: approved (scope confirmed via brainstorming)

## Context

tinyos already ships a lightweight editor: `kernel/src/apps/editor.rs` (`EditorApp`),
a thin `App` over the shared `TextView`, launched from the Terminal via `edit <file>`.
It handles arrows / backspace / enter / `Ctrl+S` and nothing else — no modes, no
motions, no undo.

We want a **proper, maximal vi-compatible modal editor** added *alongside* the
existing simple editor (invoked as `vi <file>`), including visual mode, registers
(yank/put), undo/redo, `:s` substitute, marks, and `.` repeat.

## Goals / scope

Confirmed via brainstorming:
- **Maximal vi/vim-ish** feature set (see "Command set" below).
- Added as a **new `vi` command**; `edit`/`EditorApp` stays unchanged.
- **Undo/redo** (`u` / `Ctrl-R`) included.

Non-goals (YAGNI for v1): full regex in search/substitute (use plain substring +
`\c` optionality later), `:g/global`, ex-mode scripting, macros (`q`), `:set`
options, split windows, syntax highlighting.

## Architecture

Two units with a clean boundary:

### 1. `vicore` — new workspace crate (`#![no_std]` + `alloc`, `std` feature for tests)

Mirrors the existing `tinyfs` crate pattern so the logic is **host-unit-testable**
(the kernel itself only builds for `aarch64-unknown-uefi`, so pure logic must live
in a host-buildable crate to get automated tests). Contains the entire editor
*model and behavior*, decoupled from rendering and input hardware:

- `Buffer` — `Vec<String>` of lines (char-indexed columns, UTF-8 safe), plus helpers
  for insert/delete/split/join and range extraction.
- `Editor` — owns a `Buffer`, cursor `(line, col)`, desired column (for vertical
  motion), `Mode`, pending count/operator/register state, unnamed + named registers,
  undo/redo stacks, last-search, last-change (for `.`), and marks.
- `Mode` — `Normal`, `Insert`, `Visual`, `VisualLine`, `Command` (ex `:`),
  `Search` (`/`,`?`), `OperatorPending`, `ReplaceChar` (`r`).
- Input is fed as **semantic events**, not scancodes: `Editor::on_char(char)`,
  `Editor::on_special(Special)` where `Special ∈ {Esc, Enter, Backspace, Tab,
  Left, Right, Up, Down}`, and `Editor::on_ctrl(char)` (e.g. `Ctrl-R`, `Ctrl-F`).
  This keeps the kernel adapter a pure keycode→event translation with no vi logic.
- Side effects the engine cannot perform itself (file write, quit) are returned as
  **effects**: `Editor::take_effects() -> Vec<Effect>` where
  `Effect ∈ {Save(Option<String>), Quit, ForceQuit, Status(String)}`. The kernel
  adapter drains these and performs fs I/O / window close.
- Rendering data is exposed read-only: `lines()`, `cursor()`, `mode()`,
  `visual_span()` (selection rect for highlight), `status_left()`, `command_line()`
  (the `:`/`/` line being typed), `top` viewport hint via `scroll_to(rows)`.

### 2. `kernel/src/apps/vi.rs` — `ViApp` (thin `App` adapter)

Models on `editor.rs`. Owns a `vicore::Editor`, `cwd`, `path`, `title`. Responsibilities:
- `open(cwd, path)` — `crate::fs::read`, seed the buffer (reuse the `NotFound` →
  "new file" behavior from `EditorApp::open`).
- `draw` — render the buffer on the `CELL_W`/`CELL_H` mono grid directly (not via
  `TextView`, because vi needs mode-dependent caret shape and visual-selection
  highlighting that `TextView` doesn't expose). Reuse `fonts.mono.draw`,
  `Surface::fill_rect`, and `crate::ui::shell::caret_on` for blink. Bottom row is a
  status/command line (mode indicator, `Ln,Col`, filename+`[+]`, or the `:`/`/` input).
- Input decode: `on_char(c)` → `editor.on_char`; `on_key(code)` maps
  `ESC/ENTER/BACKSPACE/arrows` → `editor.on_special`; `on_ctrl_key(code)` maps the
  Ctrl chord's letter → `editor.on_ctrl`. After each event, drain `take_effects()`:
  perform `Save` via `crate::fs::write`, set `pending_close` on `Quit`.

### Wiring

- `kernel/src/apps/mod.rs`: add `pub mod vi;`.
- `kernel/src/term/mod.rs`: add a `"vi"` command mirroring `"edit"` — set
  `pending_vi: Option<(String, String)>`; expose `take_pending_vi()`. Add `vi` to
  the `help` command table.
- `kernel/src/ui/shell/mod.rs` `pump_app_requests()`: drain `take_pending_vi()` and
  `self.open(Box::new(ViApp::open(cwd, path)))`, mirroring the `edit` path. Also add
  programmatic close: poll a `ViApp::wants_close()` flag and `windows.remove(i)` (the
  shell already removes windows at lines 360/417/515/582 — reuse that mechanism) so
  `:q` / `:wq` can close the window.
- Optionally register `vi` in `open_named`/palette (stretch; the Terminal command is
  the primary entry point).

## Command set (v1 "maximal")

**Motions** (count-aware): `h j k l` + arrows, `0 ^ $`, `w W b B e E`, `gg`, `G`,
`{N}G`, `f/F/t/T{char}` + `; ,`, `H M L`, `Ctrl-F/B/D/U` (paging).
Stretch: `% ` (bracket match), `{ }` (paragraph).

**Operators** (count-aware, take a motion or double for linewise): `d c y`,
`dd cc yy`, `D C Y`, `x X`, `s S`, `r{char}`, `~`, `J` (join).

**Enter insert**: `i I a A o O`; from visual: `c s`.

**Visual**: `v` (charwise), `V` (linewise); operators `d c y x` apply to selection;
`o` swaps ends; `Esc` exits.

**Registers**: unnamed register with linewise/charwise flag filled by yank/delete;
`p` / `P` paste (after/before, line-aware); named `"a`–`"z` (stretch — unnamed first).

**Undo**: `u` undo, `Ctrl-R` redo. Snapshot-based (buffer+cursor pushed before each
mutating command; a whole insert-mode session coalesces into one undo step).

**Repeat**: `.` repeats the last buffer-changing command.

**Search**: `/pattern` `?pattern` (plain substring), `n` `N` repeat.
Stretch: `* #`.

**Marks**: `m{a-z}`, `` `{a-z} ``, `'{a-z}` (stretch).

**Ex commands**: `:w` `:w <file>` `:w!`, `:q` `:q!`, `:wq` `:x`, `:{N}` (goto line),
`:s/old/new/[g]` (current line), `:%s/old/new/[g]`. Unknown command → status error.

## Error handling

- File read errors: reuse `EditorApp::open` behavior (`NotFound` → new file; other
  errors → status line, open empty).
- Save errors: shown in status line (`:w` reports `"E: <err>"`), buffer stays dirty.
- `:q` with unsaved changes → error `E37: No write since last change` (use `:q!`).
- Invalid ex command / bad `:s` pattern → status-line error, no mutation.
- All engine operations are total (clamp cursor, ignore no-op motions) — never panic.

## Testing

`vicore` gets host unit tests (`cargo test -p vicore`), TDD:
- Buffer primitives: insert/delete/split/join, char-index/byte-index round trips,
  range extraction, UTF-8 boundaries.
- Motions: each motion from representative cursor positions, with counts, at
  buffer edges (empty line, last line, past EOL clamp).
- Operators: `dw`, `dd`, `3dd`, `d$`, `cw`, `yy`+`p`, `x` at EOL, `J`.
- Visual: select + `d`/`y`, linewise vs charwise.
- Registers/paste: charwise vs linewise `p`/`P` placement.
- Undo/redo: single change, coalesced insert session, redo after undo, undo past
  start is a no-op.
- Search: forward/backward, wrap-around, `n`/`N`.
- Ex: `:s`, `:%s/g`, `:{N}`, `:w` emits `Save` effect, `:q!` emits `ForceQuit`.
- `.` repeat: `x`, `dd`, `dw`, an insert-session.

Kernel adapter (`vi.rs`) is verified by building for the UEFI target
(`make` / `cargo build -p kernel --target aarch64-unknown-uefi`) and a manual
`make run` smoke test (open `vi`, edit, `:wq`, re-open to confirm persisted).

## Build sequence

1. Scaffold `vicore` crate + workspace member; `Buffer` + tests.
2. `Editor` normal-mode motions + tests.
3. Insert mode + `i a o` family + undo coalescing + tests.
4. Operators (`d c y` + doubles + `x`/`D`/etc.) + registers + `p`/`P` + tests.
5. Visual mode + tests.
6. Search + ex commands (`:w :q :wq :s :%s :{N}`) + effects + tests.
7. `.` repeat + marks (stretch) + tests.
8. Kernel `ViApp` adapter + rendering.
9. Terminal `vi` command + shell `pump_app_requests` + programmatic close + `help`.
10. Build UEFI target; `make run` smoke test; commit; draft PR.
