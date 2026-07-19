//! A small editable multi-line text widget over a pixel Canvas (8x8 font at
//! 2x). Ported from the kernel Notes editing logic during the Phase 4
//! eviction; used by the windowed editor app.

use alloc::string::String;
use alloc::vec::Vec;

use abi::keys;

use crate::gfx::{self, Canvas, Rect};

const SCALE: i32 = 2;
pub const CELL_W: i32 = 8 * SCALE;
pub const LINE_H: i32 = 8 * SCALE + 4;

pub struct TextPad {
    lines: Vec<String>,
    line: usize,
    col: usize,
    pub dirty: bool,
}

impl TextPad {
    pub fn new(text: &str) -> Self {
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines, line: 0, col: 0, dirty: false }
    }

    pub fn text(&self) -> String {
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }

    fn byte_col(&self) -> usize {
        self.lines[self.line]
            .char_indices()
            .nth(self.col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[self.line].len())
    }

    fn clamp_col(&mut self) {
        self.col = self.col.min(self.lines[self.line].chars().count());
    }

    pub fn on_char(&mut self, c: char) {
        self.dirty = true;
        if c == '\n' {
            let bc = self.byte_col();
            let rest = self.lines[self.line].split_off(bc);
            self.lines.insert(self.line + 1, rest);
            self.line += 1;
            self.col = 0;
        } else if c != '\t' {
            let bc = self.byte_col();
            self.lines[self.line].insert(bc, c);
            self.col += 1;
        }
    }

    pub fn on_key(&mut self, code: u16) {
        match code {
            keys::BACKSPACE => {
                self.dirty = true;
                if self.col > 0 {
                    self.col -= 1;
                    let bc = self.byte_col();
                    self.lines[self.line].remove(bc);
                } else if self.line > 0 {
                    let cur = self.lines.remove(self.line);
                    self.line -= 1;
                    self.col = self.lines[self.line].chars().count();
                    self.lines[self.line].push_str(&cur);
                }
            }
            keys::LEFT => self.col = self.col.saturating_sub(1),
            keys::RIGHT => {
                self.col = (self.col + 1).min(self.lines[self.line].chars().count())
            }
            keys::UP => {
                self.line = self.line.saturating_sub(1);
                self.clamp_col();
            }
            keys::DOWN => {
                self.line = (self.line + 1).min(self.lines.len() - 1);
                self.clamp_col();
            }
            _ => {}
        }
    }

    /// Draw into `area`; the caret is shown when `blink_on`.
    pub fn render(&self, c: &mut Canvas, area: Rect, blink_on: bool) {
        let visible = (area.h / LINE_H).max(1) as usize;
        let start = (self.line + 1).saturating_sub(visible);
        for (i, line) in self.lines.iter().skip(start).take(visible).enumerate() {
            c.draw_text(area.x, area.y + i as i32 * LINE_H, line, SCALE, gfx::TX);
        }
        if blink_on {
            let row = (self.line - start) as i32;
            let cx = area.x + self.col as i32 * CELL_W;
            c.fill_rect(Rect::new(cx, area.y + row * LINE_H, 2, LINE_H - 3), gfx::ACC);
        }
    }
}
