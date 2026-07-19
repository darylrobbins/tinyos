//! Console protocol v1 (app <-> terminal emulator over the CONSOLE channel).
//!
//! Message = u32 LE opcode + payload. Three tiers: line world (WRITE /
//! INPUT_LINE), the live region (bottom-pinned cell surface while lines keep
//! scrolling above), and the full-screen text surface (alt-screen semantics).
//! At most one of {surface, live region} is open per connection.
//!
//! v1 is defined here ahead of implementation (terminal spec M2-M4); only
//! OP_WRITE is live today. Design: 2026-07-19-terminal-and-crates-design.md.

// app -> terminal
pub const OP_WRITE: u32 = 1; // utf8: append to scrollback
pub const OP_HELLO: u32 = 2; // {ver:u32}; requests HELLO_ACK
pub const OP_SET_INPUT_MODE: u32 = 3; // {mode:u32}
pub const OP_SURFACE_OPEN: u32 = 4; // {cols,rows:u32} + cell MemObj
pub const OP_SURFACE_PRESENT: u32 = 5; // {x,y,w,h:u32} damage, in cells
pub const OP_SURFACE_CURSOR: u32 = 6; // {row,col,shape,visible:u32}
pub const OP_SURFACE_CLOSE: u32 = 7;
pub const OP_LIVE_OPEN: u32 = 8; // {rows:u32} + cell MemObj
pub const OP_LIVE_RESIZE: u32 = 9; // {rows:u32} + new cell MemObj
pub const OP_LIVE_CLOSE: u32 = 10;
pub const OP_WRITE_STYLED: u32 = 11; // {fg:u32, utf8} scrollback line(s), colored
pub const OP_CLEAR: u32 = 12; // clear scrollback
pub const OP_SET_PROMPT: u32 = 13; // {count:u32, per span: fg:u32, len:u32, utf8}
                              // the LINES-mode editable-line prefix (colored)

// terminal -> app
pub const OP_INPUT_LINE: u32 = 16; // utf8, no trailing newline
pub const OP_KEY: u32 = 17; // {code:u16, down:u8, mods:u8}
pub const OP_CHAR: u32 = 18; // {c:u32}
pub const OP_RESIZE: u32 = 19; // {cols,rows:u32}
pub const OP_PASTE: u32 = 20; // utf8, atomic
pub const OP_FOCUS: u32 = 21; // {gained:u32}
pub const OP_MOUSE: u32 = 22; // {row,col:u32, buttons:u32, kind:u32}
pub const OP_HELLO_ACK: u32 = 23; // {ver:u32, features:u32}
pub const OP_CLOSE_REQ: u32 = 24;

// SET_INPUT_MODE modes.
pub const INPUT_MODE_LINES: u32 = 0; // emulator edits/echoes; INPUT_LINE
pub const INPUT_MODE_KEYS: u32 = 1; // raw KEY/CHAR events, no echo

// SURFACE_CURSOR shapes.
pub const CURSOR_BLOCK: u32 = 0;
pub const CURSOR_BAR: u32 = 1;
pub const CURSOR_UNDERLINE: u32 = 2;

// MOUSE kinds.
pub const MOUSE_MOVE: u32 = 0;
pub const MOUSE_DOWN: u32 = 1;
pub const MOUSE_UP: u32 = 2;
pub const MOUSE_SCROLL: u32 = 3; // buttons = signed lines as i32

/// One character cell. Surfaces are row-major arrays of these, stride = cols.
///
/// Colors are 0xAA_RR_GG_BB; alpha 0x00 means "theme default fg/bg" so themes
/// work without a palette layer. A double-width glyph occupies its cell with
/// `ATTR_WIDE` and the next with `ATTR_WIDE_CONT` (glyph 0).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cell {
    /// Unicode scalar; 0 = empty.
    pub glyph: u32,
    pub fg: u32,
    pub bg: u32,
    pub attrs: u16,
    pub _pad: u16,
}

// Cell attribute bits. Remaining bits reserved (anticipated: image /
// hyperlink side-table reference).
pub const ATTR_BOLD: u16 = 1;
pub const ATTR_ITALIC: u16 = 2;
pub const ATTR_UNDERLINE: u16 = 4;
pub const ATTR_UNDERCURL: u16 = 8;
pub const ATTR_STRIKE: u16 = 16;
pub const ATTR_DIM: u16 = 32;
pub const ATTR_INVERSE: u16 = 64;
pub const ATTR_WIDE: u16 = 128;
pub const ATTR_WIDE_CONT: u16 = 256;

/// Color alpha byte meaning "use the theme default for this plane".
pub const COLOR_DEFAULT: u32 = 0;
