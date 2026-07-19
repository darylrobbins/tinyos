use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// A program hosted in a shell window.
pub trait App: core::any::Any {
    /// Enables shell-level downcasts (monitor stats feed, timer retarget).
    fn as_any(&mut self) -> &mut dyn core::any::Any;
    fn title(&self) -> &str;
    /// Short glyph for the dock tile.
    fn glyph(&self) -> &str;
    fn preferred_size(&self, screen_w: i32, screen_h: i32) -> (i32, i32);
    fn min_size(&self) -> (i32, i32) {
        (260, 160)
    }
    fn draw(
        &mut self,
        s: &mut Surface,
        fonts: &mut Fonts,
        body: Rect,
        focused: bool,
        now_ms: u64,
    );
    fn on_char(&mut self, _c: char) {}
    fn on_key(&mut self, _code: u16) {}
    /// A Ctrl+key chord the shell did not reserve (e.g. Ctrl+S). Lets apps
    /// implement their own shortcuts; the shell keeps Ctrl+K/L/arrows.
    fn on_ctrl_key(&mut self, _code: u16) {}
    /// Apps that animate continuously (hosted userspace surfaces) return true
    /// so the shell keeps composing frames while they are visible.
    fn wants_frames(&self) -> bool {
        false
    }
    /// The shell is closing this window: a host can ask its process to exit
    /// gracefully instead of being torn down immediately.
    fn on_close_request(&mut self) {}
}
