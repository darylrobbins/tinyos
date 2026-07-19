//! Shared framebuffer text-buffer widget.
//!
//! A colored, line-oriented text buffer with a viewport, a cursor, and an
//! editable region. Two consumers today:
//!   * the Terminal — an immutable scrollback of frozen lines plus one editable
//!     input line, with a multi-color prompt rendered as a `prefix`;
//!   * the editor App — a fully editable multi-line buffer.
//!
//! Both share the monospace cell metrics, the char-index -> byte-index cursor
//! math, word wrap, viewport scrolling, and caret drawing that used to be
//! duplicated in `term` and `apps::notes`.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;

/// Monospace cell metrics shared by every text-grid consumer.
pub const CELL_W: i32 = 9;
pub const CELL_H: i32 = 19;
const FONT_PX: f32 = 15.0;

/// A single stored display line: text plus a uniform color.
struct Line {
    text: String,
    color: u32,
}

/// Caret shape drawn at the cursor.
#[derive(Clone, Copy)]
pub enum Caret {
    /// Thin vertical bar (insert-style, for the editor).
    Bar,
    /// Full-cell block (overwrite-style, for the terminal prompt).
    Block,
}

pub struct TextView {
    lines: Vec<Line>,
    /// Cursor line index; always `>= edit_floor`.
    cur_line: usize,
    /// Cursor column as a char index within `lines[cur_line].text`.
    cur_col: usize,
    /// Lines in `0..edit_floor` are immutable (terminal scrollback).
    edit_floor: usize,
    /// Non-editable spans rendered before the active (`cur_line`) line only.
    prefix: Vec<(String, u32)>,
    /// Char width of `prefix` (caret x-offset on the active line).
    prefix_cols: usize,
    /// First visible line (viewport scroll offset).
    top: usize,
    /// Wrap width in cells; the host updates this from its rect each frame.
    pub cols: usize,
    /// Retained-line cap; 0 = unbounded (editor). Front lines evict first.
    cap: usize,
    caret: Caret,
    caret_color: u32,
    /// Color applied to freshly created/typed editable lines.
    default_color: u32,
}

impl TextView {
    /// Console: one editable line at the bottom, scrollback frozen above it.
    pub fn console(cap: usize, default_color: u32, caret_color: u32) -> Self {
        Self {
            lines: vec![Line {
                text: String::new(),
                color: default_color,
            }],
            cur_line: 0,
            cur_col: 0,
            edit_floor: 0,
            prefix: Vec::new(),
            prefix_cols: 0,
            top: 0,
            cols: 80,
            cap,
            caret: Caret::Block,
            caret_color,
            default_color,
        }
    }

    /// Editor: the whole buffer is editable, no scrollback, no prompt.
    pub fn editor(default_color: u32, caret_color: u32) -> Self {
        Self {
            lines: vec![Line {
                text: String::new(),
                color: default_color,
            }],
            cur_line: 0,
            cur_col: 0,
            edit_floor: 0,
            prefix: Vec::new(),
            prefix_cols: 0,
            top: 0,
            cols: 80,
            cap: 0,
            caret: Caret::Bar,
            caret_color,
            default_color,
        }
    }

    // --- content -----------------------------------------------------------

    /// Replace the whole buffer with `text` split on '\n' (editor load).
    pub fn set_text(&mut self, text: &str) {
        self.lines = text
            .split('\n')
            .map(|l| Line {
                text: String::from(l),
                color: self.default_color,
            })
            .collect();
        if self.lines.is_empty() {
            self.lines.push(Line {
                text: String::new(),
                color: self.default_color,
            });
        }
        self.edit_floor = 0;
        self.cur_line = 0;
        self.cur_col = 0;
        self.top = 0;
    }

    /// Push an immutable, word-wrapped line just above the editable region
    /// (terminal output). Applies the scrollback cap from the front.
    pub fn append_frozen(&mut self, text: String, color: u32) {
        let width = self.cols.max(1);
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            self.insert_frozen(String::new(), color);
        } else {
            for chunk in chars.chunks(width) {
                self.insert_frozen(chunk.iter().collect(), color);
            }
        }
        self.enforce_cap();
    }

    fn insert_frozen(&mut self, text: String, color: u32) {
        self.lines.insert(self.edit_floor, Line { text, color });
        self.edit_floor += 1;
        self.cur_line += 1;
    }

    fn enforce_cap(&mut self) {
        if self.cap == 0 {
            return;
        }
        while self.lines.len() > self.cap && self.edit_floor > 0 {
            self.lines.remove(0);
            self.edit_floor -= 1;
            self.cur_line = self.cur_line.saturating_sub(1);
            self.top = self.top.saturating_sub(1);
        }
    }

    /// Terminal Enter: freeze the current input line as `echo` (immutable),
    /// then open a fresh empty editable line below it.
    pub fn freeze_active_as(&mut self, echo: String, color: u32) {
        self.lines[self.cur_line] = Line { text: echo, color };
        self.lines.push(Line {
            text: String::new(),
            color: self.default_color,
        });
        self.edit_floor = self.lines.len() - 1;
        self.cur_line = self.edit_floor;
        self.cur_col = 0;
        self.enforce_cap();
    }

    /// Replace the active line's text (history recall / fresh prompt).
    pub fn set_active(&mut self, text: String) {
        self.cur_col = text.chars().count();
        self.lines[self.cur_line].text = text;
    }

    /// Clear to a single empty editable line (terminal `clear`).
    pub fn clear(&mut self) {
        self.lines = vec![Line {
            text: String::new(),
            color: self.default_color,
        }];
        self.edit_floor = 0;
        self.cur_line = 0;
        self.cur_col = 0;
        self.top = 0;
    }

    /// The active editable line's text (terminal reads the command).
    pub fn active_text(&self) -> String {
        self.lines[self.cur_line].text.clone()
    }

    /// The full editable buffer joined with '\n' (editor save).
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate().skip(self.edit_floor) {
            if i > self.edit_floor {
                out.push('\n');
            }
            out.push_str(&line.text);
        }
        out
    }

    /// Cursor position as 1-based (line, col) within the editable region.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cur_line - self.edit_floor + 1, self.cur_col + 1)
    }

    // --- editing -----------------------------------------------------------

    fn byte_index(&self, line: usize, col: usize) -> usize {
        let s = &self.lines[line].text;
        s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
    }

    fn clamp_col(&mut self) {
        self.cur_col = self.cur_col.min(self.lines[self.cur_line].text.chars().count());
    }

    pub fn insert_char(&mut self, c: char) {
        let bc = self.byte_index(self.cur_line, self.cur_col);
        self.lines[self.cur_line].text.insert(bc, c);
        self.cur_col += 1;
    }

    /// Split the active line at the cursor into two editable lines (editor).
    pub fn split_line(&mut self) {
        let bc = self.byte_index(self.cur_line, self.cur_col);
        let rest = self.lines[self.cur_line].text.split_off(bc);
        self.lines.insert(
            self.cur_line + 1,
            Line {
                text: rest,
                color: self.default_color,
            },
        );
        self.cur_line += 1;
        self.cur_col = 0;
    }

    pub fn backspace(&mut self) {
        if self.cur_col > 0 {
            self.cur_col -= 1;
            let bc = self.byte_index(self.cur_line, self.cur_col);
            self.lines[self.cur_line].text.remove(bc);
        } else if self.cur_line > self.edit_floor {
            let cur = self.lines.remove(self.cur_line);
            self.cur_line -= 1;
            self.cur_col = self.lines[self.cur_line].text.chars().count();
            self.lines[self.cur_line].text.push_str(&cur.text);
        }
    }

    pub fn left(&mut self) {
        self.cur_col = self.cur_col.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cur_col = (self.cur_col + 1).min(self.lines[self.cur_line].text.chars().count());
    }

    pub fn up(&mut self) {
        if self.cur_line > self.edit_floor {
            self.cur_line -= 1;
            self.clamp_col();
        }
    }

    pub fn down(&mut self) {
        if self.cur_line + 1 < self.lines.len() {
            self.cur_line += 1;
            self.clamp_col();
        }
    }

    // --- prompt / rendering ------------------------------------------------

    /// Set the non-editable prompt spans drawn before the active line.
    pub fn set_prompt(&mut self, spans: Vec<(String, u32)>) {
        self.prefix_cols = spans.iter().map(|(s, _)| s.chars().count()).sum();
        self.prefix = spans;
    }

    fn ensure_visible(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.cur_line < self.top {
            self.top = self.cur_line;
        } else if self.cur_line >= self.top + rows {
            self.top = self.cur_line + 1 - rows;
        }
    }

    pub fn draw(
        &mut self,
        s: &mut Surface,
        fonts: &mut Fonts,
        ox: i32,
        oy: i32,
        rows: usize,
        now_ms: u64,
        focused: bool,
    ) {
        self.ensure_visible(rows);
        for i in 0..rows {
            let idx = self.top + i;
            if idx >= self.lines.len() {
                break;
            }
            let y = oy + i as i32 * CELL_H;
            let mut x = ox;
            if idx == self.cur_line && !self.prefix.is_empty() {
                for (text, color) in &self.prefix {
                    fonts.mono.draw(s, text, FONT_PX, x, y, *color);
                    x += text.chars().count() as i32 * CELL_W;
                }
            }
            let line = &self.lines[idx];
            fonts.mono.draw(s, &line.text, FONT_PX, x, y, line.color);
        }

        if focused && crate::ui::shell::caret_on(now_ms) && self.cur_line >= self.top {
            let row = (self.cur_line - self.top) as i32;
            let cx = ox + (self.prefix_cols + self.cur_col) as i32 * CELL_W;
            let y = oy + row * CELL_H;
            match self.caret {
                Caret::Bar => s.fill_rect(cx, y + 1, 2, CELL_H - 4, self.caret_color),
                Caret::Block => s.fill_rect(cx, y + 1, CELL_W, CELL_H - 2, self.caret_color),
            }
        }
    }
}
