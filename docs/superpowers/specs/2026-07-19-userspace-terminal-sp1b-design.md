# Userspace terminal — full-screen surfaces (SP1b)

Date: 2026-07-19
Status: design, approved direction — pending written-spec review
Builds on: SP1a userspace terminal (merged, main a45dc4d), SP0 brokers
Precedes: SP1c (window broker → edit/pixels from uterm; live regions), SP2 (flip default, delete in-kernel terminal)

## Context

SP1a shipped a userspace terminal (`apps/terminal`) that hosts `sh`'s **line world** — scrollback + editable prompt — with `crates/termcore` as the pure model. It explicitly ignores `OP_SURFACE_*`/`OP_LIVE_*`, so full-screen TUI apps (`vi`, `top`) launched inside `uterm` don't render.

SP1b makes `vi`/`top` run inside the userspace terminal by porting the kernel terminal's cell-surface hosting to userspace. The crux — confirmed feasible — is that a userspace app can map a MemObj it *received* over a channel (`Msg.handles[0]` is installed in the terminal's handle table; `SYS_MEMOBJ_MAP` works on any held handle). The kernel terminal reads the child's cells by physical address (a kernel-only zero-copy trick); the userspace terminal maps the MemObj and reads the same 16-byte-stride cell layout from a VA.

SP1b is full-screen surfaces only. Live regions (`OP_LIVE_*`) and the window broker (for `edit`/`pixels`) are SP1c. The in-kernel terminal stays the boot default, unchanged.

## Goals

- `vi <file>` and `top` (and any console app that opens a full-screen `OP_SURFACE_*`) render correctly inside the userspace `uterm` terminal: the terminal maps the child's cell MemObj, renders cells with the mono atlas honoring `ATTR_*` (INVERSE/DIM/UNDERLINE/WIDE/WIDE_CONT) + cursor shapes + blink, and forwards raw keys while a surface is open; on surface close it restores the line world.
- A child can't fault the terminal with bogus surface dimensions: the terminal validates `cols*rows*16 <= memobj_size` before mapping (new `SYS_MEMOBJ_SIZE`).

## Non-goals (SP1c / SP2)

- Live regions (`OP_LIVE_*`, the Ink-style bottom panel used by `progress`). Deferred.
- Window broker so `sh`'s windowed children (`edit`, `pixels`) open their own compositor windows. Deferred (SP1c). `run edit`/`run pixels` from `uterm` still fail gracefully.
- Flipping the boot default / deleting the in-kernel terminal (SP2).
- Extra attributes the kernel doesn't render either (`ATTR_BOLD/ITALIC/UNDERCURL/STRIKE`) — parity with `draw_cells`, which handles only INVERSE/DIM/UNDERLINE/WIDE.
- Automated framebuffer assertions — `vi`-in-`uterm` rendering is verified manually (framebuffer is not serial-visible); the automated smoke gate proves launch + drive + no-panic.

## Detailed design

### Kernel — `SYS_MEMOBJ_SIZE`

A new syscall returning the byte size of a MemObj handle the caller holds (any rights). Mirrors the kernel terminal's `mem.size()` validation (`term/mod.rs:907`) so the userspace terminal can reject a surface whose declared `cols*rows*16` exceeds the actual MemObj. Add the ABI constant (`crates/abi/src/syscall.rs`), the kernel handler (`kernel/src/obj/syscall.rs`), and an SDK wrapper (`apps/sdk/src/...` — e.g. `memobj::size(handle) -> Result<u64,u32>` or a `syscall` helper). Trivial, arch-neutral.

### `termcore` — surface meta + raw-input clause + cell resolver (pure, host-tested)

`termcore` stays pure (no I/O). It gains:

- **Surface meta state:** `surface: Option<SurfaceMeta { cols: usize, rows: usize, cursor: (usize, usize, u32, bool) }>`. Set on `OP_SURFACE_OPEN` (cols/rows from the message bytes; the MemObj handle is consumed by `apps/terminal`, not `termcore`). Updated on `OP_SURFACE_CURSOR`. Cleared on `OP_SURFACE_CLOSE`, which also resets `mode = INPUT_MODE_LINES` (matching the kernel's self-heal). `OP_SURFACE_PRESENT` is a no-op in `termcore` (the pixel re-read is `apps/terminal`'s job) but marks dirty.
- **The missing raw clause:** `is_raw()` becomes `mode == INPUT_MODE_KEYS || surface.is_some()` — an open surface forces raw key forwarding regardless of stored mode (the kernel does this at `term/mod.rs:204-208`; `termcore` currently keys only off `mode`). This is the one behavioral bug to fix so `vi`'s raw keys forward across mode races.
- **A pure cell attribute resolver** — the error-prone `draw_cells` logic, extracted and tested:
  ```rust
  pub struct Resolved { pub glyph: Option<char>, pub fg: u32, pub bg: Option<u32>, pub wide: bool, pub underline: bool }
  /// Resolve a Cell to draw parameters. `theme_fg`/`theme_bg` supply the
  /// alpha-0-means-default colors. Returns None for a WIDE_CONT cell (skip).
  pub fn resolve_cell(cell: &abi::console::Cell, theme_fg: u32, theme_bg: u32) -> Option<Resolved>;
  ```
  implementing: `WIDE_CONT` → None (skip); `fg = if cell.fg>>24==0 { theme_fg } else { cell.fg }`; `bg = if cell.bg>>24==0 { None } else { Some(cell.bg) }`; INVERSE swaps fg/bg (bg default becomes `theme_bg`); DIM applies `(c>>1)&0x007F7F7F | 0xFF000000`; WIDE → `wide=true` (2 cells); glyph = `char::from_u32(cell.glyph).filter(|c| !c.is_whitespace() && *c!='\0')`; UNDERLINE → `underline=true`. Host tests: default colors, explicit colors, inverse swap, dim mask, wide, wide-cont skip, whitespace glyph → None.
- Accessors for `apps/terminal`: `surface() -> Option<&SurfaceMeta>` (drives the render-mode switch + cursor), and the existing dirty flag.

`apps/terminal` still calls `term.on_console_msg(&bytes)` for the surface ops (so meta stays in sync), but ALSO inspects the raw message for the moved handle on `OP_SURFACE_OPEN` (see below) — `termcore` never sees handles.

### `apps/terminal` — surface lifecycle + cell rendering (I/O + pixels)

The console-drain loop changes: for each received `Msg`, if the op is `OP_SURFACE_OPEN`, the terminal takes `msg.handles[0]` (the child's cell MemObj), validates `cols*rows*16 <= memobj_size(handle)`, maps it (`SYS_MEMOBJ_MAP`), and stores `{ va, size, handle }`. It still feeds `msg.bytes` to `termcore` for meta. On `OP_SURFACE_CLOSE`, unmap (`SYS_MEMOBJ_UNMAP`) + close the handle.

Rendering forks on `term.surface()`:
- **Line world (no surface):** the SP1a path (scrollback + prompt).
- **Surface active:** iterate `rows × cols`; for each cell read 16 bytes at `va + (row*cols+col)*16`, decode into `abi::console::Cell` (glyph/fg/bg/attrs LE), `termcore::resolve_cell(...)` → draw: `Canvas::fill_rect` the bg (if Some) across `CELL_W * (wide?2:1) × CELL_H`, `Canvas::draw_mono_glyph`/`draw_mono_text` the glyph in fg, `fill_rect` a 1px underline if set; skip `wide_cont`. Then draw the cursor from `surface().cursor` honoring `CURSOR_BLOCK/BAR/UNDERLINE` + a caret blink (time-based via `uptime_us`), in ACCENT — matching `draw_cells` 1160-1170.

A new SDK primitive may help: a `Canvas` method to draw a single glyph by char at a cell (the current `draw_mono_text` takes a `&str`; a single-cell draw is cleaner for the grid). Add `Canvas::draw_mono_glyph(x, y, ch, color)` or reuse `draw_mono_text` with a 1-char slice — implementer's call.

Keys: unchanged in `apps/terminal` — window `Char`/`Key` events go to `term.on_char`/`on_key`, which now forward raw `OP_CHAR`/`OP_KEY` because `is_raw()` is true while a surface is open. The outbound wire format already matches `vi`'s `next_event` decode, so no change there.

Resize: the kernel forces a fresh `OP_RESIZE` when a surface opens (`sent_size=(0,0)`) so the child learns the dims. `termcore` mirrors this: on `OP_SURFACE_OPEN` it re-queues `OP_RESIZE(cols, rows)` from its own stored window cell dims (set earlier by `set_size`) into the outbound queue — no involvement from `apps/terminal`, which just flushes `take_outbound()` as usual.

## Data flow

`uterm` running `sh` → user types `vi /x` → `sh` spawns `vi` (dup console/fs/proc from `sh`) → `vi` sets KEYS mode, gets `OP_RESIZE`, `open_surface(cols,rows)` → the child's cell MemObj rides as a moved handle over the shared console channel to the **terminal** → terminal validates + maps it → `vi` draws its `CellBuffer`, `present`s → terminal re-reads cells and blits them into its window with the mono atlas → keystrokes forward raw to `vi` → `:q` → `vi` sends `OP_SURFACE_CLOSE` + exits → terminal unmaps, restores line world, `sh` prompt returns.

## Testing

- **`termcore` host tests:** `resolve_cell` (all attribute branches), the `is_raw()` surface clause, surface meta set/update/clear + mode reset on close, `OP_RESIZE` re-arm on surface open. Under `make test`.
- **Manual QEMU (`make run`):** launch `uterm`, run `vi /etc/x` (or any file) — confirm the full-screen editor renders in the mono font with a cursor, typing edits, `:q` returns to the shell prompt; run `top` — confirm the live table renders and refreshes; confirm colors/inverse (e.g. `top` header) look right.
- **Automated smoke:** extend the launch-path step — after launching `uterm`, drive `sh`-in-`uterm` to `run top` (or `vi`), wait, then quit, asserting no kernel panic and the machine stays live (heartbeat). This proves the surface host path doesn't wedge/fault, even though the rendered cells aren't serial-visible.
- **Both arches:** `cargo check` aarch64 + x86_64 (the `SYS_MEMOBJ_SIZE` handler must compile on both).

## Risks

- **Untrusted cell contents:** `glyph` is `char::from_u32`-filtered; `fg`/`bg` are arbitrary u32 colors (fine); dims are validated against the real MemObj size (via `SYS_MEMOBJ_SIZE`) so reads stay in bounds. Re-validate on every `OP_SURFACE_PRESENT` damage rect (clamp to cols/rows).
- **Map/unmap lifecycle:** a surface app that exits without `OP_SURFACE_CLOSE` (crash) must not leak the mapping — the terminal detects child exit (console peer close / `OP_SURFACE_CLOSE`) and unmaps + restores line world (the kernel self-heals via `OP_SET_INPUT_MODE`→LINES; mirror it).
- **Dual-wake still required:** surface presents arrive on the console channel; the SP1a dual-wake (window + console) already covers this. Re-render on console readability when a surface is active.
- **Performance:** re-blitting the whole cell grid per present (up to cols×rows glyph draws) each frame; acceptable at 80×24 and `top`'s ~2 Hz, same as the kernel.

## Out of scope → future

- SP1c: window broker (per-child window channels → `edit`/`pixels` from `uterm`); live regions (`OP_LIVE_*`).
- SP2: flip boot default to `uterm`, delete in-kernel `term`/`textview`/`App` trait/`monitor`, migrate the smoke serial mirror.
- Also pending (SP0 review): per-request broker reply channel before scoped FS / read-only PROC.
