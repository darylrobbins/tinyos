use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, Surface};

use super::tokens::{ACCENT, SURFACE_HI, TEXT, TILE_RADIUS};

pub const APPS: [(&str, &str); 4] = [
    ("terminal", ">_"),
    ("notes", "N"),
    ("monitor", "~"),
    ("clock", "()"),
];

const TILE: i32 = 46;
const GAP: i32 = 8;
const PAD: i32 = 10;

fn pill_rect(screen: (i32, i32)) -> (i32, i32, i32, i32) {
    let n = APPS.len() as i32;
    let w = n * TILE + (n - 1) * GAP + PAD * 2;
    let h = TILE + PAD * 2;
    ((screen.0 - w) / 2, screen.1 - h - 10, w, h)
}

pub fn draw(
    s: &mut Surface,
    fonts: &mut Fonts,
    backdrop: &Surface,
    screen: (i32, i32),
    running: &[(&str, bool)],
) {
    let (px, py, pw, ph) = pill_rect(screen);
    s.frosted_panel(backdrop, px, py, pw, ph, ph / 2, argb(150, 18, 20, 34));

    for (i, (name, glyph)) in APPS.iter().enumerate() {
        let tx = px + PAD + i as i32 * (TILE + GAP);
        let ty = py + PAD;
        s.fill_rounded_rect(tx, ty, TILE, TILE, TILE_RADIUS, SURFACE_HI);
        let (gw, _) = fonts.mono.measure(glyph, 16.0);
        fonts
            .mono
            .draw(s, glyph, 16.0, tx + (TILE - gw) / 2, ty + 13, TEXT);
        if running.iter().any(|&(n, r)| n == *name && r) {
            s.fill_rounded_rect(tx + TILE / 2 - 2, py + ph - 7, 4, 4, 2, ACCENT);
        }
    }
}

pub fn hit_test(pxy: (i32, i32), screen: (i32, i32)) -> Option<&'static str> {
    let (px, py, _, ph) = pill_rect(screen);
    for (i, (name, _)) in APPS.iter().enumerate() {
        let tx = px + PAD + i as i32 * (TILE + GAP);
        let ty = py + PAD;
        if pxy.0 >= tx && pxy.0 < tx + TILE && pxy.1 >= ty && pxy.1 < ty + TILE {
            return Some(name);
        }
    }
    let _ = ph;
    None
}
