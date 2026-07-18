use alloc::format;

use crate::arch::timer;
use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::ui::shell::app::{App, Rect};
use crate::ui::shell::tokens::{ACCENT, SURFACE_HI, TEXT, TEXT_DIM};

pub struct ClockApp {
    /// (end_ms, total_ms) when counting down.
    timer: Option<(u64, u64)>,
}

impl ClockApp {
    pub fn new() -> Self {
        Self { timer: None }
    }

    pub fn start_timer(&mut self, secs: u64) {
        let now = timer::uptime_ms();
        self.timer = Some((now + secs * 1000, secs * 1000));
    }
}

impl App for ClockApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn title(&self) -> &str {
        if self.timer.is_some() {
            "Timer"
        } else {
            "Clock"
        }
    }

    fn glyph(&self) -> &str {
        "()"
    }

    fn preferred_size(&self, _sw: i32, _sh: i32) -> (i32, i32) {
        (300, 200)
    }

    fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, body: Rect, _focused: bool, now: u64) {
        match self.timer {
            None => {
                let mins = 9 * 60 + 41 + now / 60_000;
                let text = format!("{}:{:02}", mins / 60 % 24, mins % 60);
                fonts
                    .ui_semibold
                    .draw(s, &text, 46.0, body.x + 2, body.y + 6, TEXT);
                fonts.ui_medium.draw(s, "Fri Jul 17 2026", 13.0, body.x + 4, body.y + 62, TEXT_DIM);
            }
            Some((end, total)) => {
                let remaining = end.saturating_sub(now);
                if remaining == 0 {
                    if now / 750 % 2 == 0 {
                        fonts
                            .ui_semibold
                            .draw(s, "DONE", 46.0, body.x + 2, body.y + 6, ACCENT);
                    }
                    fonts.ui_medium.draw(s, "Timer elapsed", 13.0, body.x + 4, body.y + 62, TEXT_DIM);
                    return;
                }
                let secs = remaining.div_ceil(1000);
                let text = format!("{}:{:02}", secs / 60, secs % 60);
                fonts
                    .ui_semibold
                    .draw(s, &text, 46.0, body.x + 2, body.y + 6, ACCENT);
                // Draining progress rule.
                let w = body.w - 8;
                let left = (w as u64 * remaining / total.max(1)) as i32;
                s.fill_rect(body.x + 2, body.y + 68, w, 2, SURFACE_HI);
                s.fill_rect(body.x + 2, body.y + 68, left, 2, ACCENT);
            }
        }
    }
}
