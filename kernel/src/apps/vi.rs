//! A vi-compatible modal editor.
//!
//! A thin `App` adapter over the [`vicore`] engine: it translates keycodes into
//! semantic engine events, renders the buffer on the shared monospace grid with
//! a mode-dependent caret and visual-selection highlight, and performs the file
//! I/O and window-close [`Effect`]s the engine requests. Launched from the
//! Terminal via `vi <file>`.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use tinyfs::FsError;
use vicore::editor::{Effect, Mode};
use vicore::Editor;

use crate::drivers::input::{keycode_to_char, keys};
use crate::gfx::font::Fonts;
use crate::gfx::surface::{with_alpha, Surface};
use crate::ui::shell::app::{App, Rect};
use crate::ui::shell::tokens::{ACCENT, TEXT, TEXT_DIM};
use crate::ui::textview::{CELL_H, CELL_W};

/// Font size for the text grid (matches `TextView`'s `FONT_PX`).
const FONT_PX: f32 = 15.0;
/// Display width of a tab stop.
const TAB: usize = 4;

pub struct ViApp {
    ed: Editor,
    cwd: String,
    path: String,
    title: String,
    /// Set when the engine asks to quit; drained by the shell to close us.
    want_close: bool,
}

impl ViApp {
    /// Open `path` (relative to `cwd`). A missing file starts an empty buffer;
    /// other read errors open empty and surface the error in the status line.
    pub fn open(cwd: String, path: String) -> Self {
        let (text, load_err) = match crate::fs::read(&cwd, &path) {
            Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), None),
            Err(FsError::NotFound) => (String::new(), Some("new file".to_string())),
            Err(e) => (String::new(), Some(format!("{e}"))),
        };
        let ed = Editor::new(&text);
        let mut app = Self {
            ed,
            cwd,
            path,
            title: String::new(),
            want_close: false,
        };
        if let Some(msg) = load_err {
            app.ed.set_status(msg);
        }
        app.refresh_title();
        app
    }

    fn refresh_title(&mut self) {
        let star = if self.ed.is_dirty() { " *" } else { "" };
        self.title = format!("{}{star}", self.path);
    }

    /// The shell polls this to close the window on `:q` / `:wq`.
    pub fn wants_close(&self) -> bool {
        self.want_close
    }

    /// Run any effects the engine queued after an input event.
    fn drain_effects(&mut self) {
        for eff in self.ed.take_effects() {
            match eff {
                Effect::Save(path) => self.save(path),
                Effect::Quit | Effect::ForceQuit => self.want_close = true,
                Effect::SaveQuit(path) => {
                    self.save(path);
                    self.want_close = true;
                }
            }
        }
        self.refresh_title();
    }

    fn save(&mut self, path: Option<String>) {
        let target = path.unwrap_or_else(|| self.path.clone());
        let body = self.ed.text();
        match crate::fs::write(&self.cwd, &target, body.as_bytes(), false) {
            Ok(()) => {
                self.ed.mark_saved();
                let n = self.ed.line_count();
                self.ed.set_status(format!("\"{target}\" {n}L written"));
            }
            Err(e) => self.ed.set_status(format!("E: {e}")),
        }
    }
}

/// Expand tabs in `line` for display, returning the expanded string and a map
/// from char index → visual column (with one extra entry for the end position).
fn expand(line: &str) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut map = Vec::with_capacity(line.chars().count() + 1);
    let mut col = 0;
    for ch in line.chars() {
        map.push(col);
        if ch == '\t' {
            let n = TAB - (col % TAB);
            for _ in 0..n {
                out.push(' ');
            }
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    map.push(col);
    (out, map)
}

/// Visual column of char `col` on `line`.
fn vis_col(line: &str, col: usize) -> usize {
    let (_, map) = expand(line);
    *map.get(col).unwrap_or_else(|| map.last().unwrap_or(&0))
}

impl App for ViApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn glyph(&self) -> &str {
        "V"
    }

    fn preferred_size(&self, _sw: i32, _sh: i32) -> (i32, i32) {
        (660, 480)
    }

    fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, body: Rect, focused: bool, now: u64) {
        let rows = (body.h / CELL_H).max(1) as usize;
        let text_rows = rows.saturating_sub(1).max(1);
        self.ed.set_view_rows(text_rows);
        let top = self.ed.scroll_into_view();

        let (cl, cc) = self.ed.cursor();
        let sel = self.ed.visual_range();
        let hi = with_alpha(ACCENT, 56);

        let lines = self.ed.lines();
        for i in 0..text_rows {
            let idx = top + i;
            if idx >= lines.len() {
                break;
            }
            let line = &lines[idx];
            let y = body.y + i as i32 * CELL_H;

            // Visual selection highlight for this row.
            if let Some((a, b, linewise)) = sel {
                if idx >= a.line && idx <= b.line {
                    let (sx, ex) = if linewise {
                        (0, body.w / CELL_W)
                    } else {
                        let start = if idx == a.line { vis_col(line, a.col) } else { 0 };
                        let end = if idx == b.line {
                            vis_col(line, b.col + 1)
                        } else {
                            (body.w / CELL_W) as usize
                        };
                        (start as i32, end as i32)
                    };
                    let x0 = body.x + sx * CELL_W;
                    s.fill_rect(x0, y, (ex - sx).max(0) * CELL_W, CELL_H, hi);
                }
            }

            let (expanded, _) = expand(line);
            fonts.mono.draw(s, &expanded, FONT_PX, body.x, y, TEXT);
        }

        // Cursor: block in normal/visual, bar in insert.
        let in_view = cl >= top && cl < top + text_rows;
        if focused && in_view && crate::ui::shell::caret_on(now) {
            let vcol = vis_col(self.ed.lines().get(cl).map(String::as_str).unwrap_or(""), cc);
            let cx = body.x + vcol as i32 * CELL_W;
            let cy = body.y + (cl - top) as i32 * CELL_H;
            match self.ed.mode() {
                Mode::Insert => s.fill_rect(cx, cy + 1, 2, CELL_H - 4, ACCENT),
                _ => {
                    s.fill_rect(cx, cy + 1, CELL_W, CELL_H - 2, ACCENT);
                    // Redraw the covered glyph in the background ink for contrast.
                    if let Some(ch) = self
                        .ed
                        .lines()
                        .get(cl)
                        .and_then(|l| l.chars().nth(cc))
                        .filter(|c| *c != '\t')
                    {
                        let mut buf = [0u8; 4];
                        fonts.mono.draw(
                            s,
                            ch.encode_utf8(&mut buf),
                            FONT_PX,
                            cx,
                            cy,
                            crate::ui::shell::tokens::BG,
                        );
                    }
                }
            }
        }

        // Status / command line along the bottom row.
        let sy = body.y + text_rows as i32 * CELL_H;
        let (text, color) = if let Some(cmd) = self.ed.command_line() {
            (cmd, TEXT)
        } else if !self.ed.status().is_empty() {
            (self.ed.status().to_string(), TEXT_DIM)
        } else {
            let label = self.ed.mode_label();
            let star = if self.ed.is_dirty() { "[+]" } else { "" };
            (
                format!("{label:<16}{} {star}  {}:{}", self.path, cl + 1, cc + 1),
                TEXT_DIM,
            )
        };
        fonts.mono.draw(s, &text, 13.0, body.x, sy, color);
    }

    fn on_char(&mut self, c: char) {
        match c {
            '\n' => self.ed.on_special(vicore::editor::Special::Enter),
            '\t' => self.ed.on_special(vicore::editor::Special::Tab),
            _ => self.ed.on_char(c),
        }
        self.drain_effects();
    }

    fn on_key(&mut self, code: u16) {
        use vicore::editor::Special;
        let sp = match code {
            keys::ESC => Special::Esc,
            keys::BACKSPACE => Special::Backspace,
            keys::ENTER => Special::Enter,
            keys::LEFT => Special::Left,
            keys::RIGHT => Special::Right,
            keys::UP => Special::Up,
            keys::DOWN => Special::Down,
            _ => return,
        };
        self.ed.on_special(sp);
        self.drain_effects();
    }

    fn on_ctrl_key(&mut self, code: u16) {
        if let Some(ch) = keycode_to_char(code, false) {
            self.ed.on_ctrl(ch);
            self.drain_effects();
        }
    }
}
