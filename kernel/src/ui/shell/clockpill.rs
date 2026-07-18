//! Bottom-right glass pill: clock over date. Click toggles quick settings.

use alloc::format;

use crate::arch::timer;
use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;

use super::tokens::*;

const PILL_H: i32 = 64;

pub fn rect(screen: (i32, i32)) -> (i32, i32, i32, i32) {
    let w = 148;
    (screen.0 - w - 24, screen.1 - PILL_H - 18, w, PILL_H)
}

pub fn clock_strings() -> (alloc::string::String, alloc::string::String) {
    let mins = 9 * 60 + 41 + timer::uptime_ms() / 60_000;
    (
        format!("{:02}:{:02}", mins / 60 % 24, mins % 60),
        alloc::string::String::from("Fri, Jul 17"),
    )
}

pub fn draw(s: &mut Surface, fonts: &mut Fonts, backdrop: &Surface, screen: (i32, i32)) {
    let (px, py, pw, ph) = rect(screen);
    s.frosted_panel(backdrop, px, py, pw, ph, RADIUS_PILL, GLASS_TINT);

    let (clock, date) = clock_strings();
    let (cw, _) = fonts.mono.measure(&clock, 16.0);
    fonts.mono.draw(s, &clock, 16.0, px + pw - cw - 18, py + 12, TX);
    let (dw, _) = fonts.ui.measure(&date, 11.0);
    fonts.ui.draw(s, &date, 11.0, px + pw - dw - 18, py + 36, TX2);
}

pub fn hit_test(pxy: (i32, i32), screen: (i32, i32)) -> bool {
    let (px, py, pw, ph) = rect(screen);
    pxy.0 >= px && pxy.0 < px + pw && pxy.1 >= py && pxy.1 < py + ph
}
