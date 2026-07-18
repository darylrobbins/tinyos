use alloc::format;

use crate::arch::timer;
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, Surface};
use crate::mem;

use super::tokens::{STATUS_H, TEXT, TEXT_DIM};

pub fn draw(s: &mut Surface, fonts: &mut Fonts, backdrop: &Surface, width: i32) {
    s.frosted_panel(backdrop, 0, -14, width, STATUS_H + 14, 14, argb(140, 15, 17, 30));

    fonts.ui_semibold.draw(s, "tinyOS", 15.0, 16, 7, TEXT);

    let secs = timer::uptime_ms() / 1000;
    let mins = 9 * 60 + 41 + timer::uptime_ms() / 60_000;
    let (used, _) = mem::stats();
    let right = format!(
        "up {}:{:02}:{:02}  ·  {} MiB  ·  {}:{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        used >> 20,
        mins / 60 % 24,
        mins % 60
    );
    let (w, _) = fonts.ui_medium.measure(&right, 13.0);
    fonts.ui_medium.draw(s, &right, 13.0, width - w - 16, 8, TEXT_DIM);
}
