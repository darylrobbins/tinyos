//! textui: cell-grid drawing for tinyOS terminal apps (terminal spec,
//! Layer 2). A `CellBuffer` you draw into, plus diffing that turns
//! "redraw everything" app code into a minimal damage rect.
//!
//! Pure data-in/data-out over the abi `Cell` model — no I/O — so it is
//! host-testable (`cargo test -p textui`) and usable from both apps and,
//! if ever needed, the kernel.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

pub use abi::console::{
    Cell, ATTR_BOLD, ATTR_DIM, ATTR_INVERSE, ATTR_ITALIC, ATTR_STRIKE, ATTR_UNDERCURL,
    ATTR_UNDERLINE, ATTR_WIDE, ATTR_WIDE_CONT, COLOR_DEFAULT,
};

/// Fg/bg/attrs applied by drawing calls. `Style::default()` = theme colors.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: u32,
    pub bg: u32,
    pub attrs: u16,
}

impl Style {
    pub const fn fg(fg: u32) -> Self {
        Style { fg, bg: COLOR_DEFAULT, attrs: 0 }
    }

    pub const fn new(fg: u32, bg: u32, attrs: u16) -> Self {
        Style { fg, bg, attrs }
    }

    const fn cell(&self, glyph: char, extra_attrs: u16) -> Cell {
        Cell {
            glyph: glyph as u32,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs | extra_attrs,
            _pad: 0,
        }
    }
}

/// True for characters that occupy two cells (pragmatic ranges: CJK,
/// Hangul, kana, fullwidth forms, common emoji). Grapheme clusters beyond
/// one scalar are out of scope for v1.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    match u {
        0x1100..=0x115F // Hangul jamo
        | 0x2E80..=0x303E // CJK radicals, punctuation
        | 0x3041..=0x33FF // kana, CJK symbols
        | 0x3400..=0x4DBF // CJK ext A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xA000..=0xA4CF // Yi
        | 0xAC00..=0xD7A3 // Hangul syllables
        | 0xF900..=0xFAFF // CJK compat
        | 0xFE30..=0xFE4F // CJK compat forms
        | 0xFF00..=0xFF60 // fullwidth forms
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF // emoji blocks
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// A rectangle of cells, row-major, stride = cols.
#[derive(Clone)]
pub struct CellBuffer {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
}

/// Damage: (x, y, w, h) in cells.
pub type Damage = (usize, usize, usize, usize);

impl CellBuffer {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows, cells: vec![Cell::default(); cols * rows] }
    }

    /// Clear to empty glyphs with the given style, e.g. for a themed bg.
    pub fn clear(&mut self, style: Style) {
        self.cells.fill(style.cell('\0', 0));
    }

    /// Discard contents and take on a new size (cleared to default cells).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells.resize(cols * rows, Cell::default());
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn get(&self, x: usize, y: usize) -> Cell {
        self.cells[y * self.cols + x]
    }

    /// Place one character; a double-width glyph also claims the next cell
    /// as its continuation. Out-of-bounds (including a wide glyph in the
    /// last column) is quietly clipped. Returns the width consumed.
    pub fn put(&mut self, x: usize, y: usize, c: char, style: Style) -> usize {
        let w = char_width(c);
        if y >= self.rows || x + w > self.cols {
            return w;
        }
        let i = y * self.cols + x;
        if w == 2 {
            self.cells[i] = style.cell(c, ATTR_WIDE);
            self.cells[i + 1] = style.cell('\0', ATTR_WIDE_CONT);
        } else {
            self.cells[i] = style.cell(c, 0);
        }
        w
    }

    /// Draw a string starting at (x, y); no wrapping. Returns the x after
    /// the last cell drawn.
    pub fn put_str(&mut self, x: usize, y: usize, s: &str, style: Style) -> usize {
        let mut cx = x;
        for c in s.chars() {
            cx += self.put(cx, y, c, style);
            if cx >= self.cols {
                break;
            }
        }
        cx
    }

    /// Fill a rect with one character (clipped).
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: char, style: Style) {
        for row in y..(y + h).min(self.rows) {
            for col in x..(x + w).min(self.cols) {
                self.cells[row * self.cols + col] = style.cell(c, 0);
            }
        }
    }

    /// Box-drawing border around the given rect (clipped; needs w, h >= 2).
    pub fn draw_box(&mut self, x: usize, y: usize, w: usize, h: usize, style: Style) {
        if w < 2 || h < 2 {
            return;
        }
        let (x1, y1) = (x + w - 1, y + h - 1);
        self.put(x, y, '┌', style);
        self.put(x1, y, '┐', style);
        self.put(x, y1, '└', style);
        self.put(x1, y1, '┘', style);
        for col in x + 1..x1 {
            self.put(col, y, '─', style);
            self.put(col, y1, '─', style);
        }
        for row in y + 1..y1 {
            self.put(x, row, '│', style);
            self.put(x1, row, '│', style);
        }
    }

    /// Bounding rect of cells that differ from `prev`, or None if identical
    /// (sizes must match; a size change is full damage).
    pub fn diff(&self, prev: &CellBuffer) -> Option<Damage> {
        if self.cols != prev.cols || self.rows != prev.rows {
            return Some((0, 0, self.cols, self.rows));
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for y in 0..self.rows {
            for x in 0..self.cols {
                if self.cells[y * self.cols + x] != prev.cells[y * self.cols + x] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 != usize::MAX).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_str_and_get() {
        let mut b = CellBuffer::new(10, 2);
        let end = b.put_str(1, 0, "hi", Style::fg(0xFF00FF00));
        assert_eq!(end, 3);
        assert_eq!(b.get(1, 0).glyph, 'h' as u32);
        assert_eq!(b.get(2, 0).glyph, 'i' as u32);
        assert_eq!(b.get(2, 0).fg, 0xFF00FF00);
        assert_eq!(b.get(0, 0).glyph, 0);
    }

    #[test]
    fn wide_glyph_claims_continuation() {
        let mut b = CellBuffer::new(10, 1);
        assert_eq!(char_width('漢'), 2);
        let end = b.put_str(0, 0, "漢a", Style::default());
        assert_eq!(end, 3);
        assert_eq!(b.get(0, 0).attrs & ATTR_WIDE, ATTR_WIDE);
        assert_eq!(b.get(1, 0).attrs & ATTR_WIDE_CONT, ATTR_WIDE_CONT);
        assert_eq!(b.get(1, 0).glyph, 0);
        assert_eq!(b.get(2, 0).glyph, 'a' as u32);
    }

    #[test]
    fn wide_glyph_clips_at_last_column() {
        let mut b = CellBuffer::new(2, 1);
        b.put(1, 0, '漢', Style::default()); // would need cols 1..=2
        assert_eq!(b.get(1, 0).glyph, 0); // clipped, not half-drawn
    }

    #[test]
    fn diff_bounding_box() {
        let mut a = CellBuffer::new(8, 4);
        a.clear(Style::default());
        let prev = a.clone();
        assert_eq!(a.diff(&prev), None);
        a.put(2, 1, 'x', Style::default());
        a.put(5, 3, 'y', Style::default());
        assert_eq!(a.diff(&prev), Some((2, 1, 4, 3)));
    }

    #[test]
    fn box_corners() {
        let mut b = CellBuffer::new(6, 3);
        b.draw_box(0, 0, 6, 3, Style::default());
        assert_eq!(b.get(0, 0).glyph, '┌' as u32);
        assert_eq!(b.get(5, 2).glyph, '┘' as u32);
        assert_eq!(b.get(3, 0).glyph, '─' as u32);
        assert_eq!(b.get(0, 1).glyph, '│' as u32);
    }
}
