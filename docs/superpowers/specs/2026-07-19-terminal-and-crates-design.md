# tinyOS Crate Organization & Terminal Stack — Design Spec

Date: 2026-07-19
Status: approved direction; milestones M1–M4 below
Companion spec: `2026-07-19-roadmap.md` (overall phases; "Phase N"
references below point there)

## Goal

Two interlocking designs:

1. **Package organization** for kernel and user-level code as the system
   grows: where crates live, when something earns crate-hood, and how the
   kernel and the app SDK stop hand-duplicating ABI constants.
2. **A terminal stack** that turns today's in-kernel terminal-widget-plus-
   shell into a terminal *emulator* hosting terminal *apps* — from
   `println!`-grade tools to vim-class full-screen editors and Claude
   Code-class scrollback-hybrid apps — without importing Linux TTY baggage.

Both follow the existing design pillars (see
`2026-07-18-app-api-design.md`): capabilities over ambient authority, a
small kernel, richness as protocols over channels.

## Part 1 — Package organization

### Principles

- **Build target = workspace, purpose = directory.** There are three build
  worlds that cannot comfortably share a cargo workspace: the kernel
  (`aarch64-unknown-uefi`), apps (`aarch64-unknown-none`, fixed-base linker
  script, own `.cargo/config.toml`), and host code (tools, tests). These
  remain the workspace boundaries. Organization happens *within* them.
- **Crate-hood is earned, not default.** A library becomes a crate only
  when it has **≥ 2 consumers** or needs **host-testability** the kernel
  binary can't provide (the tinyfs precedent). Single-consumer subsystems
  (`sched/`, `gfx/`, `obj/`) stay kernel modules — no Cargo ceremony for
  code with one caller.
- **All new kernel APIs are handle/channel-shaped.** No service — files,
  process control, terminals — ever ships as bespoke syscalls. Protocols
  over channels, even when the first server lives in-kernel. This is what
  makes "where does this code run" a deployment decision, not an
  architecture decision.

### Layout

```
crates/            target-independent no_std libraries, host-testable
  abi/             the single source of ABI truth (below)
  tinyfs/          moves here from top level (cosmetic; same commit as
                   the tier's creation)
  textui/          cell model + TUI toolkit (Part 2, Layer 2)
kernel/            one crate; subsystems are modules
apps/              separate workspace: sdk/ + apps (unchanged)
tools/             host tools (mkfs-tinyfs, future installers)
docs/  build/  assets/
```

The top level stays at these entries. New libraries go under `crates/`
behind the two-consumers-or-host-tests bar.

### `crates/abi`

A zero-dependency `no_std` crate, path-depended by the kernel workspace,
the apps workspace, and host tools. Contents:

- Syscall numbers, status codes, handle rights, signal bits, ABI version.
- The bootstrap record layout and grant tags.
- Every channel protocol, one module per protocol: `window.rs` (v0, as
  shipped), `console.rs` (v1, Part 2), future `fs.rs`, `proc.rs`.
  Constants plus plain-struct encode/decode helpers. No I/O, no alloc
  requirement beyond `core`.
- Meridian design tokens (colors, metrics) shared by kernel compositor and
  SDK `gfx`.

This kills the known drift hazard: opcodes and tokens are currently
hand-copied between `kernel/src/ui/shell/extern_app.rs` /
`kernel/src/ui/shell/tokens.rs` and `apps/sdk/src/window.rs` /
`apps/sdk/src/gfx.rs` with "userspace cannot see kernel code" comments.
After M1 those definitions exist exactly once. `crates/abi` diffs are the
ABI review surface.

## Part 2 — The terminal stack

### Framing

**The terminal protocol is a sibling of the window protocol, not a
descendant of the TTY.** The window protocol's shape — shared-memory
surface + damage messages out, input events in, over a granted channel —
is already right; only the unit changes: *cells* instead of pixels.

TTY baggage explicitly refused, and its replacement:

| Refused | Replaced by |
|---|---|
| In-band control (escape sequences) | Typed channel messages |
| Kernel line discipline | Line editing in the *emulator* |
| termios/ioctl mode soup | One `SET_INPUT_MODE` message |
| terminfo capability database | `HELLO` feature negotiation |
| Resize via signal racing the stream | First-class `RESIZE` message |
| App-managed scrollback tricks | Scrollback owned by the emulator |

### The layers

An app's console channel (grant tag `CONSOLE`, handle delivered in the
bootstrap record — unchanged) speaks one protocol with three tiers of
sophistication. At most **one** of {text surface, live region} is open per
connection at a time.

**Layer 0 — line world.** App sends `WRITE`; emulator owns echo, line
editing, and scrollback; complete lines arrive as `INPUT_LINE`. Default
mode. `hello`-grade apps use `println!` / `read_line()` and never know
more exists.

**Layer 0.5 — live region.** For Ink/Claude Code-style hybrids that
stream lines into scrollback *while* continuously redrawing a bottom
status/input area. `LIVE_OPEN {rows}` + a cell MemObj pins a small
surface to the bottom of the terminal; `WRITE` lines keep appending
above it. On close or app exit the emulator flattens the final frame
into scrollback. This is Ink's static/dynamic split as a protocol
object instead of cursor-up-and-clear escape gymnastics — and it can't
tear or interleave, because the emulator composites.

**Layer 1 — text surface (full screen).** vim-class. `SURFACE_OPEN
{cols, rows}` + a cell MemObj claims the grid; scrollback freezes
underneath and is restored on close (alt-screen semantics, implied, not
opted into). Damage-tracked `PRESENT`, cursor control, key/mouse/paste
events. VT scroll regions are not needed: the app `memmove`s cell rows in
shared memory and marks damage — the optimization VT scroll regions
existed to provide, without the state machine.

**Layer 2 — `crates/textui` toolkit.** Pure `no_std` library over the
cell model: `CellBuffer` drawing (text runs, fills, box borders), layout
splits, a small immediate-mode widget set (matching the SDK's pixel
`ui.rs` idiom), and internal diffing that turns "redraw everything" app
code into minimal damage rects. Data-in/data-out → host-testable: TUI
rendering gets `cargo test` on the host, the tinyfs trick applied to UIs.

### The cell model

```rust
#[repr(C)]                 // 16 bytes; row-major, stride = cols
pub struct Cell {
    pub glyph: u32,        // Unicode scalar; 0 = empty
    pub fg: u32,           // 0xFF_RR_GG_BB; alpha 0x00 = theme default
    pub bg: u32,           // same encoding
    pub attrs: u16,        // bitflags below
    pub _pad: u16,
}
```

Attr bits: `BOLD=1, ITALIC=2, UNDERLINE=4, UNDERCURL=8, STRIKE=16,
DIM=32, INVERSE=64, WIDE=128, WIDE_CONT=256`; the rest reserved (an
image/hyperlink side-table reference is the anticipated future use).

- Colors are RGB from day one (Meridian tokens); alpha byte `0x00` means
  "theme default fg/bg" so themes work without a palette layer. No
  16/256-color archaeology.
- **Wide glyphs are v1, not a retrofit:** a double-width glyph occupies
  its cell with `WIDE` and the next with `WIDE_CONT` (glyph 0). Grapheme
  clusters beyond one scalar + width flag are explicitly v2 (side table).
- Underline/undercurl color = `fg` in v1.

The MemObj holds raw cells only; dimensions travel in messages.

### Console protocol v1

Message = `u32 LE` opcode + payload, like window v0. `WRITE=1` keeps its
v0 number and meaning; v0's reserved `READ=2` is retired (input is pushed,
never pulled).

**app → terminal**

| op | name | payload |
|---|---|---|
| 1 | `WRITE` | utf8 — append to scrollback (line world) |
| 2 | `HELLO` | `ver: u32` — optional; requests `HELLO_ACK` |
| 3 | `SET_INPUT_MODE` | `mode: u32` — 0 `LINES` (default), 1 `KEYS` |
| 4 | `SURFACE_OPEN` | `cols: u32, rows: u32` + 1 cell-MemObj handle |
| 5 | `SURFACE_PRESENT` | damage `x,y,w,h: u32` (cells) — applies to whichever of surface/live is open |
| 6 | `SURFACE_CURSOR` | `row,col: u32, shape: u32 (0 block,1 bar,2 underline), visible: u32` |
| 7 | `SURFACE_CLOSE` | — (restore scrollback) |
| 8 | `LIVE_OPEN` | `rows: u32` + 1 cell-MemObj handle (width = terminal cols) |
| 9 | `LIVE_RESIZE` | `rows: u32` + 1 new cell-MemObj handle |
| 10 | `LIVE_CLOSE` | — (flatten final frame into scrollback) |

**terminal → app**

| op | name | payload |
|---|---|---|
| 16 | `INPUT_LINE` | utf8, no trailing newline (LINES mode) |
| 17 | `KEY` | `code: u16, down: u8, mods: u8` (KEYS mode; same codes as window protocol) |
| 18 | `CHAR` | `c: u32` (KEYS mode) |
| 19 | `RESIZE` | `cols,rows: u32` — always sent on change and once after any open |
| 20 | `PASTE` | utf8 — atomic paste, distinct from typed input (bracketed paste without the escaping hazard) |
| 21 | `FOCUS` | `gained: u32` |
| 22 | `MOUSE` | `row,col: u32, buttons: u32, kind: u32 (0 move,1 down,2 up,3 scroll±)` — surface/live modes only |
| 23 | `HELLO_ACK` | `ver: u32, features: u32` bitmask |
| 24 | `CLOSE_REQ` | — (user closed the tab/window; app should exit) |

Resize semantics: on terminal resize the emulator sends `RESIZE`; an app
with an open surface/live region allocates a new MemObj at the new
dimensions and re-sends `SURFACE_OPEN`/`LIVE_RESIZE` (open-replaces-open).
The emulator keeps compositing the old frame until the replacement
arrives — no torn intermediate states.

Trust: the emulator composites **only** from the kernel-side MemObj
object the handle refers to, never from any address the app claims, and
bounds-checks damage rects against the surface dimensions it recorded.

### The emulator and the shell

`kernel/src/term/mod.rs` splits along the line this protocol draws:

- **Terminal emulator** (the `App`-trait window, in-kernel today, an SDK
  app after the Phase-4 eviction, same protocol either way): scrollback
  + rendering, line editing/echo for LINES-mode children, protocol
  server for surface/live/cursor/input messages, and **spawning children
  with a console channel pair it keeps one end of**.
- **The shell** (today's built-ins: `ls`, `cat`, `run`, …) becomes the
  emulator's first *child* — a program speaking console v1 like anything
  else. In-kernel initially; its direct calls into `fs`/`sched`/`mem`
  migrate to the file and process-control protocols as those land
  (roadmap Phases 3–4), at which point it can be rebuilt as an SDK app
  unchanged in behavior.

Because "can host a terminal app" reduces to "holds a console channel
pair", nesting and multiplexing (a future tmux, a shell running `run`
which runs another TUI) fall out of the design with no pty analogue.

### ANSI/VT compatibility (future, out of core)

Ported software (real vim) is served by a userspace **shim crate**: a VT
escape-sequence interpreter that consumes a byte stream and drives a
Layer-1 cell surface. The cell surface is a strict superset of VT
semantics, and the shim answers query sequences (cursor position, device
attributes) locally and synchronously since it owns the grid state.
Compatibility is one optional crate at the edge — the same move as
"POSIX as a userspace layer" — and no VT parser ever enters the emulator
or the kernel. (Running actual Claude Code is blocked on a POSIX/Node
userland, not on this terminal design.)

## Milestones

- **M1 — `crates/abi` (no behavior change).** Create the `crates/` tier,
  move `tinyfs/` into it, extract syscall/status/rights/signal constants,
  window-protocol opcodes, bootstrap layout, and Meridian tokens from
  kernel + SDK into `crates/abi`; both sides consume it. Add console-v1
  and cell-model definitions to `abi` (constants only, unimplemented).
- **M2 — console v1 line world.** Emulator/shell split inside
  `term/mod.rs`; `INPUT_LINE`, `SET_INPUT_MODE`, `RESIZE`, `PASTE`,
  `HELLO`; SDK gains `read_line()` and raw-key mode. Apps get stdin.
- **M3 — text surface.** `SURFACE_*` + cursor in the emulator;
  `crates/textui` v1 (CellBuffer, wide-glyph handling, diff→damage,
  basic widgets, host tests). Proof app: port the file editor (or a
  pager) as an SDK terminal app.
- **M4 — live region.** `LIVE_*` in the emulator; textui helper for
  static-above/dynamic-below apps. Proof app: a progress/spinner demo
  that streams lines while animating a bottom panel.

Future (unscheduled): ANSI shim crate; emulator as an SDK app; mouse
reporting polish; grapheme-cluster side table; image/hyperlink cells via
the reserved attr bits.

## Out of scope

Escape sequences anywhere in emulator or kernel; app access to
scrollback contents; terminfo or any capability database beyond the
`HELLO` bitmask; pty devices; job control; multiple simultaneous
surfaces per connection; scrollback persistence.
