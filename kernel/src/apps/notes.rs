use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::input::keys;
use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::ui::shell::app::{App, Rect};
use crate::ui::shell::tokens::{ACCENT, TEXT};

const LINE_H: i32 = 22;

pub struct NotesApp {
    lines: Vec<String>,
    line: usize,
    col: usize,
}

impl NotesApp {
    pub fn new() -> Self {
        Self {
            lines: alloc::vec![String::new()],
            line: 0,
            col: 0,
        }
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
}

impl App for NotesApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn title(&self) -> &str {
        "Notes"
    }

    fn glyph(&self) -> &str {
        "N"
    }

    fn preferred_size(&self, _sw: i32, _sh: i32) -> (i32, i32) {
        (420, 320)
    }

    fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, body: Rect, focused: bool, now: u64) {
        let visible = (body.h / LINE_H).max(1) as usize;
        let start = (self.line + 1).saturating_sub(visible);
        for (i, line) in self.lines.iter().skip(start).take(visible).enumerate() {
            fonts
                .mono
                .draw(s, line, 15.0, body.x, body.y + i as i32 * LINE_H, TEXT);
        }
        if focused && crate::ui::shell::caret_on(now) {
            let row = (self.line - start) as i32;
            let cx = body.x + self.col as i32 * 9;
            s.fill_rect(cx, body.y + row * LINE_H, 2, LINE_H - 4, ACCENT);
        }
    }

    fn on_char(&mut self, c: char) {
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

    fn on_key(&mut self, code: u16) {
        match code {
            keys::BACKSPACE => {
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
            keys::RIGHT => self.col = (self.col + 1).min(self.lines[self.line].chars().count()),
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
}
