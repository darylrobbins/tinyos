//! Lightweight file editor.
//!
//! A thin `App` over the shared [`TextView`]: loads a file into an editable
//! buffer, edits multi-line text with arrows/backspace/enter, and saves back
//! to tinyfs on `Ctrl+S`. Launched from the Terminal via `edit <file>`.

use alloc::format;
use alloc::string::{String, ToString};

use tinyfs::FsError;

use crate::drivers::input::keys;
use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::ui::shell::app::{App, Rect};
use crate::ui::shell::tokens::{ACCENT, TEXT, TEXT_DIM};
use crate::ui::textview::{TextView, CELL_H};

/// evdev keycode for the `S` key (Ctrl+S = save).
const KEY_S: u16 = 31;

pub struct EditorApp {
    view: TextView,
    cwd: String,
    path: String,
    title: String,
    dirty: bool,
    status: String,
}

impl EditorApp {
    /// Open `path` (relative to `cwd`) for editing. A missing file starts an
    /// empty "new file" buffer; a non-UTF-8 or directory target reports an
    /// error in the status line and opens empty.
    pub fn open(cwd: String, path: String) -> Self {
        let mut view = TextView::editor(TEXT, ACCENT);
        let status = match crate::fs::read(&cwd, &path) {
            Ok(bytes) => {
                view.set_text(&String::from_utf8_lossy(&bytes));
                String::new()
            }
            Err(FsError::NotFound) => "new file".to_string(),
            Err(e) => format!("{e}"),
        };
        let mut ed = Self {
            view,
            cwd,
            path,
            title: String::new(),
            dirty: false,
            status,
        };
        ed.refresh_title();
        ed
    }

    fn refresh_title(&mut self) {
        self.title = if self.dirty {
            format!("{} *", self.path)
        } else {
            self.path.clone()
        };
    }

    fn mark_dirty(&mut self) {
        if !self.dirty {
            self.dirty = true;
            self.refresh_title();
        }
        self.status.clear();
    }

    fn save(&mut self) {
        let body = self.view.to_string();
        match crate::fs::write(&self.cwd, &self.path, body.as_bytes(), false) {
            Ok(()) => {
                self.dirty = false;
                self.refresh_title();
                self.status = "saved".to_string();
            }
            Err(e) => self.status = format!("save: {e}"),
        }
    }
}

impl App for EditorApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn glyph(&self) -> &str {
        "E"
    }

    fn preferred_size(&self, _sw: i32, _sh: i32) -> (i32, i32) {
        (640, 460)
    }

    fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, body: Rect, focused: bool, now: u64) {
        // Reserve the bottom row for the status bar.
        let rows = (body.h / CELL_H).max(1) as usize;
        let text_rows = rows.saturating_sub(1).max(1);
        self.view
            .draw(s, fonts, body.x, body.y, text_rows, now, focused);

        let (ln, col) = self.view.cursor();
        let dirty = if self.dirty { " *" } else { "" };
        let status = if self.status.is_empty() {
            format!("{}{dirty}   Ln {ln}, Col {col}   ^S save", self.path)
        } else {
            format!(
                "{}{dirty}   Ln {ln}, Col {col}   {}   ^S save",
                self.path, self.status
            )
        };
        let sy = body.y + text_rows as i32 * CELL_H;
        fonts.mono.draw(s, &status, 13.0, body.x, sy, TEXT_DIM);
    }

    fn on_char(&mut self, c: char) {
        match c {
            '\n' => {
                self.view.split_line();
                self.mark_dirty();
            }
            '\t' => {}
            _ => {
                self.view.insert_char(c);
                self.mark_dirty();
            }
        }
    }

    fn on_key(&mut self, code: u16) {
        match code {
            keys::BACKSPACE => {
                self.view.backspace();
                self.mark_dirty();
            }
            keys::LEFT => self.view.left(),
            keys::RIGHT => self.view.right(),
            keys::UP => self.view.up(),
            keys::DOWN => self.view.down(),
            _ => {}
        }
    }

    fn on_ctrl_key(&mut self, code: u16) {
        if code == KEY_S {
            self.save();
        }
    }
}
