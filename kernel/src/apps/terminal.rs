use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::term::{Terminal, CELL_H, CELL_W};
use crate::ui::shell::app::{App, Rect};

pub struct TerminalApp {
    term: Terminal,
}

impl TerminalApp {
    pub fn new() -> Self {
        Self {
            term: Terminal::new(),
        }
    }

    /// A pending `edit <file>` request, drained by the shell each frame.
    pub fn take_pending_edit(&mut self) -> Option<(alloc::string::String, alloc::string::String)> {
        self.term.take_pending_edit()
    }
}

impl App for TerminalApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn title(&self) -> &str {
        "Terminal"
    }

    fn glyph(&self) -> &str {
        ">_"
    }

    fn preferred_size(&self, _sw: i32, _sh: i32) -> (i32, i32) {
        (760, 500)
    }

    fn min_size(&self) -> (i32, i32) {
        (340, 220)
    }

    fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, body: Rect, _focused: bool, now: u64) {
        self.term.pump();
        self.term.set_cols((body.w / CELL_W).max(10) as usize);
        let rows = (body.h / CELL_H).max(2) as usize;
        self.term.draw(s, fonts, body.x, body.y, rows, now);
    }

    fn on_char(&mut self, c: char) {
        self.term.on_char(c);
    }

    fn on_key(&mut self, code: u16) {
        self.term.on_key(code);
    }
}
