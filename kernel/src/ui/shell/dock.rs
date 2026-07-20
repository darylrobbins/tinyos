//! Meridian dock: floating glass pill — orb launcher, separator, app
//! tiles with per-app colored glyphs and teal running dots.

use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, lerp, Surface};

use super::icons::{self, Icon};
use super::tokens::*;

pub const APPS: [(&str, Icon, u32); 4] = [
    ("terminal", Icon::Terminal, ACC),
    ("notes", Icon::Notes, HUE_AMBER),
    ("monitor", Icon::Monitor, HUE_BLUE),
    ("clock", Icon::Clock, HUE_VIOLET),
];

const TILE: i32 = 44;
const GAP: i32 = 8;
const PAD: i32 = 12;
const PILL_H: i32 = 64;
const SEP_W: i32 = 9; // 1px line + margins

pub enum DockHit {
    Orb,
    App(&'static str),
}

fn pill_rect(screen: (i32, i32)) -> (i32, i32, i32, i32) {
    let n = APPS.len() as i32;
    let w = PAD * 2 + TILE + SEP_W + n * TILE + (n - 1) * GAP + (n) * 0 + 8;
    ((screen.0 - w) / 2, screen.1 - PILL_H - 18, w, PILL_H)
}

fn tile_x(pill_x: i32, i: i32) -> i32 {
    // orb, separator, then app tiles
    pill_x + PAD + TILE + SEP_W + 8 + i * (TILE + GAP)
}

pub fn draw(
    s: &mut Surface,
    _fonts: &mut Fonts,
    backdrop: &Surface,
    screen: (i32, i32),
    running: &[(&str, bool)],
) {
    let (px, py, pw, ph) = pill_rect(screen);
    s.frosted_panel(backdrop, px, py, pw, ph, RADIUS_PILL, GLASS_TINT);

    // Orb: teal-to-violet gradient tile with a soft teal glow and a bold
    // chevron prompt glyph.
    let ox = px + PAD;
    let oy = py + (ph - TILE) / 2;
    s.fill_rounded_rect(ox - 3, oy + 4, TILE + 6, TILE + 4, RADIUS_TILE + 3, argb(28, 0x5f, 0xd4, 0xc4));
    for row in 0..TILE {
        let c = lerp(ACC, HUE_VIOLET, (row * 255 / TILE) as u32);
        let inset = corner_inset(row, TILE, RADIUS_TILE);
        s.fill_rect(ox + inset, oy + row, TILE - 2 * inset, 1, c);
    }
    icons::draw(s, Icon::Orb, ox + TILE / 2, oy + TILE / 2, 22.0, ORB_TX);

    // Separator.
    s.fill_rect(px + PAD + TILE + 4, py + (ph - 34) / 2, 1, 34, STROKE);

    for (i, (name, icon, hue)) in APPS.iter().enumerate() {
        let tx = tile_x(px, i as i32);
        let ty = py + (ph - TILE) / 2 - 2;
        s.fill_rounded_rect(tx, ty, TILE, TILE, RADIUS_TILE, CARD2);
        icons::draw(s, *icon, tx + TILE / 2, ty + TILE / 2, 22.0, *hue);
        if running.iter().any(|&(n, r)| n == *name && r) {
            s.fill_rounded_rect(tx + TILE / 2 - 2, py + ph - 9, 4, 4, 2, ACC);
        }
    }
}

/// Rough rounded-corner inset for a scanline-gradient tile.
fn corner_inset(row: i32, h: i32, r: i32) -> i32 {
    let d = if row < r {
        r - row
    } else if row >= h - r {
        row - (h - r - 1)
    } else {
        0
    };
    if d <= 0 {
        0
    } else {
        // circle approximation
        r - int_sqrt((r * r - d * d).max(0))
    }
}

fn int_sqrt(v: i32) -> i32 {
    let mut x = 0;
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

pub fn hit_test(pxy: (i32, i32), screen: (i32, i32)) -> Option<DockHit> {
    let (px, py, _, ph) = pill_rect(screen);
    let oy = py + (ph - TILE) / 2;
    if pxy.0 >= px + PAD && pxy.0 < px + PAD + TILE && pxy.1 >= oy && pxy.1 < oy + TILE {
        return Some(DockHit::Orb);
    }
    for (i, (name, _, _)) in APPS.iter().enumerate() {
        let tx = tile_x(px, i as i32);
        let ty = py + (ph - TILE) / 2 - 2;
        if pxy.0 >= tx && pxy.0 < tx + TILE && pxy.1 >= ty && pxy.1 < ty + TILE {
            return Some(DockHit::App(name));
        }
    }
    None
}
