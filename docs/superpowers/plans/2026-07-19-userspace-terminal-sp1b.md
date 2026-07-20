# Userspace Terminal — Full-Screen Surfaces (SP1b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make full-screen TUI apps (`vi`, `top`) render inside the userspace terminal (`uterm`) by hosting the console `OP_SURFACE_*` protocol: map the child's cell MemObj and render it with the mono atlas honoring `ATTR_*` + cursor.

**Architecture:** Entirely userspace (no kernel change — `SYS_MEMOBJ_SIZE` already exists). `crates/termcore` gains pure surface *meta* + the `is_raw() |= surface` clause + a host-tested `resolve_cell` attribute resolver. `apps/terminal` maps the child's cell MemObj (received as a moved handle over the console channel), and forks its render between line-world (SP1a) and a cell grid. A thin SDK `memobj` module wraps the map/size/unmap syscalls.

**Tech Stack:** Rust `no_std` (apps: `aarch64-unknown-none`; `termcore`: host-testable). `tinyos-abi`, `make` + QEMU.

## Global Constraints

- SP1b touches **no kernel code**. `SYS_MEMOBJ_SIZE` (abi value 9), `SYS_MEMOBJ_MAP` (8), `SYS_MEMOBJ_UNMAP` (15) already exist with kernel handlers. Run `cargo check -p kernel` on both arches once as a guard (should be a no-op).
- `crates/termcore` stays pure (`#![cfg_attr(not(test), no_std)]`, dep only `tinyos-abi`) and host-tested via `cargo test -p termcore`.
- Render **parity with the kernel `draw_cells`** (`kernel/src/term/mod.rs:1112-1171`): handle INVERSE/DIM/UNDERLINE/WIDE/WIDE_CONT + cursor shapes/blink ONLY. Do NOT implement BOLD/ITALIC/UNDERCURL/STRIKE (the kernel doesn't either).
- `abi::console::Cell` is `#[repr(C)]` 16 bytes: `glyph:u32, fg:u32, bg:u32, attrs:u16, _pad:u16`. `COLOR_DEFAULT = 0` means alpha-0 ⇒ theme default. The child writes `Cell`s into the MemObj, so the terminal reads them by a direct `*const Cell` cast (layout matches).
- Line-world boundary (SP1a) unchanged: `sh` still gets no window channel; `edit`/`pixels` still deferred to SP1c. This plan only adds *surface* hosting.
- Cell metrics come from `monofont`: `CELL_W = ADVANCE = 9`, `CELL_H = LINE_H = 19` (the font fix merged; do not hardcode).
- Commit trailer on its own line after a blank line: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Spec: `docs/superpowers/specs/2026-07-19-userspace-terminal-sp1b-design.md`.

---

## File structure

- `apps/sdk/src/memobj.rs` (create) — `size`/`map`/`unmap` syscall wrappers.
- `apps/sdk/src/lib.rs` (modify) — `pub mod memobj;`.
- `crates/termcore/src/lib.rs` (modify) — surface meta, surface ops, `is_raw` clause, resize re-arm, `resolve_cell`, accessors, + tests.
- `apps/terminal/src/main.rs` (modify) — surface map lifecycle + cell-grid render.
- `tools/smoke/smoke.py` (modify) — drive a surface app in `uterm`.

---

### Task 1: SDK — `memobj` syscall wrappers

**Files:**
- Create: `apps/sdk/src/memobj.rs`
- Modify: `apps/sdk/src/lib.rs` (add `pub mod memobj;` after `pub mod gfx;`)

**Interfaces:**
- Consumes: `crate::syscall::{syscall1, syscall3, SYS_MEMOBJ_SIZE, SYS_MEMOBJ_MAP, SYS_MEMOBJ_UNMAP}` (SYS_MEMOBJ_SIZE=9, MAP=8, UNMAP=15 already in abi).
- Produces: `tinyos_app::memobj::{size(handle: u32) -> Result<u64, u32>, map(handle: u32, offset: u64, len: u64) -> Result<u64, u32>, unmap(va: u64)}`.

- [ ] **Step 1: Create the module**

`apps/sdk/src/memobj.rs`:
```rust
//! Thin wrappers over the memory-object syscalls, for mapping a MemObj a
//! process received over a channel (e.g. a hosted app's cell surface) — not
//! just self-created ones. size/map/unmap already exist in the kernel.

use crate::syscall::{syscall1, syscall3, SYS_MEMOBJ_MAP, SYS_MEMOBJ_SIZE, SYS_MEMOBJ_UNMAP};

/// Byte size of the MemObj referenced by `handle`.
pub fn size(handle: u32) -> Result<u64, u32> {
    syscall1(SYS_MEMOBJ_SIZE, handle as u64).ok()
}

/// Map `len` bytes of the MemObj at `offset` into this process; returns the VA.
pub fn map(handle: u32, offset: u64, len: u64) -> Result<u64, u32> {
    syscall3(SYS_MEMOBJ_MAP, handle as u64, offset, len).ok()
}

/// Unmap a mapping previously returned by `map`.
pub fn unmap(va: u64) {
    let _ = syscall1(SYS_MEMOBJ_UNMAP, va);
}
```
(Confirm `SyscallRet::ok()` returns `Result<u64,u32>` — it is used this way in `apps/sdk/src/window.rs`. If `syscall1`/`syscall3` live behind `crate::syscall::*`, match the existing import style in `window.rs`.)

- [ ] **Step 2: Register the module**

`apps/sdk/src/lib.rs` — add after `pub mod gfx;`:
```rust
pub mod memobj;
```

- [ ] **Step 3: Build**

Run: `cd apps && cargo build --release`
Expected: `Finished` (dead-code warnings on unused wrappers until Task 3 are fine).

- [ ] **Step 4: Commit**
```bash
git add apps/sdk/src/memobj.rs apps/sdk/src/lib.rs
git commit -m "sdk: memobj size/map/unmap wrappers (for mapping received MemObjs)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `termcore` — surface meta + raw clause + cell resolver

**Files:**
- Modify: `crates/termcore/src/lib.rs` (add fields, surface ops, `is_raw` clause, resize re-arm, `resolve_cell`, accessors, tests)

**Interfaces:**
- Consumes: `abi::console::{OP_SURFACE_OPEN=4, OP_SURFACE_PRESENT=5, OP_SURFACE_CURSOR=6, OP_SURFACE_CLOSE=7, Cell, ATTR_INVERSE=64, ATTR_DIM=32, ATTR_UNDERLINE=4, ATTR_WIDE=128, ATTR_WIDE_CONT=256, OP_RESIZE}`.
- Produces: `termcore::{SurfaceMeta { cols: usize, rows: usize, cursor: (usize, usize, u32, bool) }, Resolved { glyph: Option<char>, fg: u32, bg: Option<u32>, wide: bool, underline: bool }, resolve_cell(cell: &Cell, theme_fg: u32, theme_bg: u32) -> Option<Resolved>}`; `Term::surface(&self) -> Option<&SurfaceMeta>`.

- [ ] **Step 1: Add surface state + accessor**

Add to `struct Term` (after `dirty: bool,`): `surface: Option<SurfaceMeta>,` and init `surface: None` in `new()`. Define:
```rust
/// Full-screen cell surface a hosted app (vi/top) opened. Pure meta — the
/// cell bytes live in the app's MemObj, mapped and read by `apps/terminal`.
pub struct SurfaceMeta {
    pub cols: usize,
    pub rows: usize,
    /// (row, col, shape, visible) from OP_SURFACE_CURSOR.
    pub cursor: (usize, usize, u32, bool),
}
```
Accessor on `Term`: `pub fn surface(&self) -> Option<&SurfaceMeta> { self.surface.as_ref() }`.

- [ ] **Step 2: Handle the surface ops in `on_console_msg`**

Replace the `_ => {}` catch-all's neighbors by adding these arms before it:
```rust
OP_SURFACE_OPEN if bytes.len() >= 12 => {
    let cols = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let rows = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    self.surface = Some(SurfaceMeta { cols, rows, cursor: (0, 0, 0, false) });
    // The kernel forces a fresh RESIZE on surface open so the child learns
    // the dims; mirror it by re-queuing from our stored window cell dims.
    self.queue_resize();
    self.dirty = true;
}
OP_SURFACE_PRESENT => {
    // The pixel re-read is apps/terminal's job; just mark dirty.
    self.dirty = true;
}
OP_SURFACE_CURSOR if bytes.len() >= 20 => {
    if let Some(s) = self.surface.as_mut() {
        let f = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        s.cursor = (f(4) as usize, f(8) as usize, f(12), f(16) != 0);
    }
    self.dirty = true;
}
OP_SURFACE_CLOSE => {
    self.surface = None;
    self.mode = INPUT_MODE_LINES; // self-heal to line world
    self.dirty = true;
}
```
Add the imports (`OP_SURFACE_OPEN`, etc.) to the `use abi::console::{...}` block.

- [ ] **Step 3: Fix `is_raw` + add `queue_resize`**

```rust
fn is_raw(&self) -> bool {
    self.mode == INPUT_MODE_KEYS || self.surface.is_some()
}

/// Queue an OP_RESIZE with the current window cell dims (stored by set_size).
fn queue_resize(&mut self) {
    let mut b = OP_RESIZE.to_le_bytes().to_vec();
    b.extend_from_slice(&(self.cols as u32).to_le_bytes());
    b.extend_from_slice(&(self.rows as u32).to_le_bytes());
    self.out.push(b);
}
```
`set_size` currently builds the OP_RESIZE frame inline (after its early-return-if-unchanged). Refactor it to call `queue_resize` (DRY): keep the `if (cols,rows)==(self.cols,self.rows) { return; }` guard, set `self.cols/self.rows`, then `self.queue_resize();`.

- [ ] **Step 4: Add `resolve_cell` (the tested attribute logic)**

```rust
/// Draw parameters for one cell. `None` = skip (a WIDE_CONT continuation).
pub struct Resolved {
    pub glyph: Option<char>,
    pub fg: u32,
    pub bg: Option<u32>,
    pub wide: bool,
    pub underline: bool,
}

/// Resolve a Cell to draw params, matching kernel draw_cells: alpha-0 colors
/// fall back to theme; INVERSE swaps fg/bg (bg default = theme_bg); DIM halves
/// fg; WIDE = 2 cells; WIDE_CONT is skipped; whitespace/NUL glyphs draw nothing.
pub fn resolve_cell(cell: &abi::console::Cell, theme_fg: u32, theme_bg: u32) -> Option<Resolved> {
    use abi::console::{ATTR_DIM, ATTR_INVERSE, ATTR_UNDERLINE, ATTR_WIDE, ATTR_WIDE_CONT};
    if cell.attrs & ATTR_WIDE_CONT != 0 {
        return None;
    }
    let mut fg = if cell.fg >> 24 == 0 { theme_fg } else { cell.fg };
    let mut bg = if cell.bg >> 24 == 0 { None } else { Some(cell.bg) };
    if cell.attrs & ATTR_INVERSE != 0 {
        let old_fg = fg;
        fg = bg.unwrap_or(theme_bg);
        bg = Some(old_fg);
    }
    if cell.attrs & ATTR_DIM != 0 {
        fg = (fg >> 1) & 0x007F_7F7F | 0xFF00_0000;
    }
    let glyph = char::from_u32(cell.glyph).filter(|c| !c.is_whitespace() && *c != '\0');
    Some(Resolved {
        glyph,
        fg,
        bg,
        wide: cell.attrs & ATTR_WIDE != 0,
        underline: cell.attrs & ATTR_UNDERLINE != 0,
    })
}
```

- [ ] **Step 5: Write the tests**

Add to the `#[cfg(test)] mod tests`:
```rust
use abi::console::{Cell, ATTR_DIM, ATTR_INVERSE, ATTR_UNDERLINE, ATTR_WIDE, ATTR_WIDE_CONT,
    OP_SURFACE_OPEN, OP_SURFACE_CLOSE, OP_RESIZE};

fn cell(glyph: char, fg: u32, bg: u32, attrs: u16) -> Cell {
    Cell { glyph: glyph as u32, fg, bg, attrs, _pad: 0 }
}

#[test]
fn resolve_default_colors() {
    let r = resolve_cell(&cell('a', 0, 0, 0), 0xFF111111, 0xFF222222).unwrap();
    assert_eq!(r.fg, 0xFF111111);     // alpha-0 fg -> theme_fg
    assert_eq!(r.bg, None);           // alpha-0 bg -> None
    assert_eq!(r.glyph, Some('a'));
}
#[test]
fn resolve_explicit_and_inverse() {
    let r = resolve_cell(&cell('x', 0xFFAAAAAA, 0xFFBBBBBB, ATTR_INVERSE), 0xFFFFFFFF, 0xFF000000).unwrap();
    assert_eq!(r.fg, 0xFFBBBBBB);     // fg <- old bg
    assert_eq!(r.bg, Some(0xFFAAAAAA)); // bg <- old fg
}
#[test]
fn resolve_dim_and_wide_and_underline() {
    let r = resolve_cell(&cell('m', 0xFFFFFFFF, 0, ATTR_DIM | ATTR_WIDE | ATTR_UNDERLINE), 0xFF888888, 0xFF000000).unwrap();
    assert_eq!(r.fg, (0xFFFFFFFF >> 1) & 0x007F_7F7F | 0xFF00_0000);
    assert!(r.wide && r.underline);
}
#[test]
fn resolve_wide_cont_skipped() {
    assert!(resolve_cell(&cell('\0', 0, 0, ATTR_WIDE_CONT), 0xFF111111, 0xFF222222).is_none());
}
#[test]
fn resolve_whitespace_glyph_none() {
    assert_eq!(resolve_cell(&cell(' ', 0, 0, 0), 0xFF111111, 0xFF222222).unwrap().glyph, None);
}
#[test]
fn surface_open_sets_meta_forces_raw_and_requeues_resize() {
    let mut t = Term::new();
    t.set_size(80, 24);
    let _ = t.take_outbound(); // drain the set_size RESIZE
    let mut m = OP_SURFACE_OPEN.to_le_bytes().to_vec();
    m.extend_from_slice(&40u32.to_le_bytes());
    m.extend_from_slice(&12u32.to_le_bytes());
    t.on_console_msg(&m);
    assert!(t.surface().is_some());
    let s = t.surface().unwrap();
    assert_eq!((s.cols, s.rows), (40, 12));
    // a RESIZE was re-queued with the window dims (80x24)
    let out = t.take_outbound();
    assert_eq!(u32::from_le_bytes(out[0][0..4].try_into().unwrap()), OP_RESIZE);
    // raw input: a char now forwards as OP_CHAR (not local edit)
    t.on_char('q');
    let out = t.take_outbound();
    assert_eq!(u32::from_le_bytes(out[0][0..4].try_into().unwrap()), abi::console::OP_CHAR);
}
#[test]
fn surface_close_restores_line_world() {
    let mut t = Term::new();
    t.set_size(80, 24);
    let mut m = OP_SURFACE_OPEN.to_le_bytes().to_vec();
    m.extend_from_slice(&40u32.to_le_bytes()); m.extend_from_slice(&12u32.to_le_bytes());
    t.on_console_msg(&m);
    t.on_console_msg(&OP_SURFACE_CLOSE.to_le_bytes());
    assert!(t.surface().is_none());
    // back to LINES: a char edits locally (no outbound OP_CHAR)
    let _ = t.take_outbound();
    t.on_char('a');
    assert_eq!(t.input(), "a");
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p termcore`
Expected: PASS (existing 12 + the new ones).

- [ ] **Step 7: Commit**
```bash
git add crates/termcore/src/lib.rs
git commit -m "termcore: surface meta + is_raw surface clause + cell resolver

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `apps/terminal` — map the surface + render the cell grid

**Files:**
- Modify: `apps/terminal/src/main.rs`

**Interfaces:**
- Consumes: `termcore::{Term::surface, SurfaceMeta, resolve_cell, Resolved}` (Task 2); `tinyos_app::memobj::{size, map, unmap}` (Task 1); `abi::console::{OP_SURFACE_OPEN, OP_SURFACE_CLOSE, Cell, CURSOR_BLOCK=0, CURSOR_BAR=1, CURSOR_UNDERLINE=2}`.

- [ ] **Step 1: Track the mapped surface**

Add a local in `main` alongside `term`: `let mut surf_va: Option<u64> = None;` (the VA of the currently-mapped cell MemObj; `None` = none).

- [ ] **Step 2: Intercept surface open/close in the console drain**

Replace the drain loop (currently `while let Ok(msg) = con_kern.try_recv() { term.on_console_msg(&msg.bytes); }`) with:
```rust
while let Ok(msg) = con_kern.try_recv() {
    let op = msg.bytes.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if op == Some(abi::console::OP_SURFACE_OPEN) && msg.bytes.len() >= 12 {
        let cols = u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap()) as u64;
        let rows = u32::from_le_bytes(msg.bytes[8..12].try_into().unwrap()) as u64;
        let need = cols * rows * 16;
        if let (Some(&h), true) = (msg.handles.first(), need > 0) {
            if let Ok(sz) = memobj::size(h) {
                if need <= sz {
                    if let Some(va) = surf_va.take() { memobj::unmap(va); }
                    let len = (need + 0xFFF) & !0xFFF;
                    if let Ok(va) = memobj::map(h, 0, len) { surf_va = Some(va); }
                }
            }
        }
    } else if op == Some(abi::console::OP_SURFACE_CLOSE) {
        if let Some(va) = surf_va.take() { memobj::unmap(va); }
    }
    term.on_console_msg(&msg.bytes);
}
```
(Import `memobj` from `tinyos_app`.)

- [ ] **Step 3: Fork the render on an active surface**

Change the render call site: `if term.dirty() { render(&mut win, &mut term, surf_va); term.clear_dirty(); }`, and update `render`'s signature to take `surf_va: Option<u64>`. In `render`, after `cv.clear(BG)`:
```rust
if let (Some(s), Some(va)) = (term.surface(), surf_va) {
    render_surface(&mut cv, s, va);
    win.present_from(&back);
    return;
}
// ... existing line-world rendering ...
```

- [ ] **Step 4: Implement `render_surface`**

```rust
fn render_surface(cv: &mut Canvas, s: &termcore::SurfaceMeta, va: u64) {
    let cells = unsafe {
        core::slice::from_raw_parts(va as *const abi::console::Cell, s.cols * s.rows)
    };
    let mut buf = [0u8; 4];
    for row in 0..s.rows {
        for col in 0..s.cols {
            let Some(r) = termcore::resolve_cell(&cells[row * s.cols + col], TX, BG) else {
                continue; // WIDE_CONT
            };
            let x = (col * CELL_W as usize) as i32;
            let y = (row * CELL_H as usize) as i32;
            let w = CELL_W * if r.wide { 2 } else { 1 };
            if let Some(bg) = r.bg {
                cv.fill_rect(Rect::new(x, y, w, CELL_H), bg);
            }
            if let Some(g) = r.glyph {
                cv.draw_mono_text(x, y, g.encode_utf8(&mut buf), r.fg);
            }
            if r.underline {
                cv.fill_rect(Rect::new(x, y + CELL_H - 3, w, 1), r.fg);
            }
        }
    }
    // Cursor (matches kernel draw_cells shapes; no blink for simplicity — the
    // kernel blinks via caret_on, optional here).
    let (crow, ccol, shape, visible) = s.cursor;
    if visible && crow < s.rows && ccol < s.cols {
        let x = (ccol * CELL_W as usize) as i32;
        let y = (crow * CELL_H as usize) as i32;
        let acc = (ACC & 0x00FF_FFFF) | 0x8000_0000;
        match shape {
            abi::console::CURSOR_BAR => cv.fill_rect(Rect::new(x, y, 2, CELL_H), acc),
            abi::console::CURSOR_UNDERLINE => cv.fill_rect(Rect::new(x, y + CELL_H - 2, CELL_W, 2), acc),
            _ => cv.fill_rect(Rect::new(x, y, CELL_W, CELL_H), acc), // block
        }
    }
}
```
(Blink is optional in SP1b; a steady cursor is acceptable and matches `top`/`vi` usage. If added, gate on `uptime_us()` like SP1a's line-world cursor.)

- [ ] **Step 5: Build**

Run: `cd apps && cargo build --release`
Expected: `Finished`, `apps/target/aarch64-unknown-none/release/terminal` present.

- [ ] **Step 6: Commit**
```bash
git add apps/terminal/src/main.rs
git commit -m "terminal: host full-screen cell surfaces (vi/top) with the mono atlas

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Integration — drive a surface app in uterm

**Files:**
- Modify: `tools/smoke/smoke.py`

**Interfaces:** Consumes the whole system (Tasks 1-3).

Note on scope: the rendered cells are framebuffer-only (not serial-visible), so the automated gate proves that launching a surface app inside `uterm` doesn't panic/wedge; visual correctness is a manual checklist.

- [ ] **Step 1: Build the disk + confirm existing smoke passes**

Run: `make sync-apps && make smoke`
Expected: `smoke: PASS` (the existing script, incl. the uterm launch step, unchanged).

- [ ] **Step 2: Add a surface-in-uterm step**

`tools/smoke/smoke.py` — after the existing `uterm launched cleanly` step, drive `sh`-in-`uterm` to run a surface app and quit it, asserting no panic. After launch, `uterm` is the focused window, so `qmp.type_line` reaches `sh` inside it:
```python
        # Run a full-screen surface app (top) inside the userspace terminal and
        # quit it. Renders to uterm's window (not serial), so we only assert the
        # surface host path doesn't panic/wedge.
        print("smoke: > (in uterm) run top")
        qmp.type_line("run top")
        time.sleep(1.5)                 # let top open its surface + render frames
        qmp.key(["q"])                  # top quits on 'q' (apps/top/src/main.rs:110)
        time.sleep(0.6)
        if serial.panic:
            raise AssertionError("panic hosting a surface app in uterm")
        print("smoke: surface app hosted in uterm cleanly")
```
`top` runs in KEYS mode, so `qmp.key(["q"])` reaches it as a forwarded `OP_CHAR('q')` and it breaks its loop (confirmed at `apps/top/src/main.rs:110`, `ConsoleEvent::Char('q') => break`).

- [ ] **Step 3: Run smoke with the new step**

Run: `make smoke`
Expected: `smoke: PASS`, including `smoke: surface app hosted in uterm cleanly`, no panic.

- [ ] **Step 4: Manual verification checklist (for the human reviewer)**

`make run` → Ctrl+K → `uterm` → Enter. In the terminal: `vi /x` — confirm the full-screen editor renders in the mono font with a cursor, typing inserts, `:q` (or the editor's quit) returns to the shell prompt. Then `top` — confirm the process table renders and refreshes (~2 Hz), colors/inverse on the header look right. Confirm the line-world prompt returns cleanly after each.

- [ ] **Step 5: Both-arch guard + commit**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished` (SP1b changed no kernel code).
```bash
git add tools/smoke/smoke.py
git commit -m "smoke: host a full-screen surface app in uterm (no-panic gate)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** memobj wrappers (Task 1), termcore surface meta + is_raw clause + resolver (Task 2), the map lifecycle + cell render (Task 3), the smoke gate + manual checklist (Task 4). No kernel change (SYS_MEMOBJ_SIZE exists). Live regions + window broker remain SP1c.
- **The crux is exercised:** Task 3 maps `msg.handles[0]` (a received MemObj) and reads it as `&[Cell]` — the thing that makes userspace surface hosting work.
- **Parity boundary:** `resolve_cell` implements exactly the kernel's INVERSE/DIM/UNDERLINE/WIDE/WIDE_CONT logic, no more.
- **Validation:** `need <= memobj_size(h)` before mapping — a child can't fault the terminal with oversized dims.
- **Type consistency:** `resolve_cell`/`SurfaceMeta`/`Resolved`/`surface()` signatures (Task 2) match their uses in Task 3; `memobj::{size,map,unmap}` (Task 1) match Task 3's calls; `Cell` is read by direct `*const Cell` cast (layout is `#[repr(C)]` 16 bytes).
- **Self-heal:** `OP_SURFACE_CLOSE` restores LINES mode + unmaps; a child that exits without CLOSE is covered because the terminal already kills/reaps on window close and the console peer-close ends the loop (SP1a behavior).
