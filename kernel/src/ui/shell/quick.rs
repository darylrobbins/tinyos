//! Quick settings: glass panel above the clock pill.

use alloc::format;

use crate::arch::timer;
use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::mem;

use super::clockpill;
use super::icons::{self, Icon};
use super::tokens::*;

const W: i32 = 400;
const H: i32 = 300;

pub enum QuickHit {
    Lock,
    Timer,
    About,
}

fn rect(screen: (i32, i32)) -> (i32, i32, i32, i32) {
    let (_, cy, cw, _) = clockpill::rect(screen);
    let _ = cw;
    (screen.0 - W - 24, cy - H - 14, W, H)
}

fn tile_rect(screen: (i32, i32), i: i32) -> (i32, i32, i32, i32) {
    let (px, py, pw, _) = rect(screen);
    let tw = (pw - 16 * 2 - 10 * 2) / 3;
    (px + 16 + i * (tw + 10), py + 16, tw, 64)
}

pub fn draw(s: &mut Surface, fonts: &mut Fonts, backdrop: &Surface, screen: (i32, i32)) {
    let (px, py, pw, ph) = rect(screen);
    s.frosted_panel(backdrop, px, py, pw, ph, 16, GLASS_TINT);

    let tiles = [
        ("Lock", "Ctrl+L", Icon::Lock, ACC),
        ("Timer", "5 min", Icon::Clock, HUE_VIOLET),
        ("About", "tinyOS", Icon::App, TX2),
    ];
    for (i, (label, sub, icon, hue)) in tiles.iter().enumerate() {
        let (tx, ty, tw, th) = tile_rect(screen, i as i32);
        s.fill_rounded_rect(tx, ty, tw, th, 12, CARD);
        icons::draw(s, *icon, tx + tw - 24, ty + 22, 18.0, *hue);
        fonts.ui_semibold.draw(s, label, 13.0, tx + 14, ty + 12, TX);
        fonts.ui.draw(s, sub, 11.0, tx + 14, ty + 34, TX2);
    }

    // SYSTEM section.
    let sy = py + 100;
    fonts.mono.draw(s, "SYSTEM", 11.0, px + 18, sy, TX3);
    let (used, free) = mem::stats();
    let frac = (used as u64 * (pw - 36) as u64 / (used + free).max(1) as u64) as i32;
    s.fill_rect(px + 18, sy + 24, pw - 36, 4, CARD2);
    s.fill_rect(px + 18, sy + 24, frac.max(2), 4, ACC);
    let secs = timer::uptime_ms() / 1000;
    let rows = [
        format!("heap      {} / {} MiB", used >> 20, (used + free) >> 20),
        format!("uptime    {}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60),
        format!("display   {}x{}", crate::fb_size().0, crate::fb_size().1),
    ];
    for (i, row) in rows.iter().enumerate() {
        fonts
            .mono
            .draw(s, row, 12.0, px + 18, sy + 42 + i as i32 * 24, TX2);
    }

    let footer = "tinyOS 0.1 \u{201c}meridian\u{201d}";
    fonts.ui.draw(s, footer, 11.0, px + 18, py + ph - 28, TX3);
}

pub fn hit_test(pxy: (i32, i32), screen: (i32, i32)) -> Option<Option<QuickHit>> {
    let (px, py, pw, ph) = rect(screen);
    if !(pxy.0 >= px && pxy.0 < px + pw && pxy.1 >= py && pxy.1 < py + ph) {
        return None; // outside the panel entirely
    }
    for (i, hit) in [QuickHit::Lock, QuickHit::Timer, QuickHit::About]
        .into_iter()
        .enumerate()
    {
        let (tx, ty, tw, th) = tile_rect(screen, i as i32);
        if pxy.0 >= tx && pxy.0 < tx + tw && pxy.1 >= ty && pxy.1 < ty + th {
            return Some(Some(hit));
        }
    }
    Some(None) // inside panel, no tile
}
