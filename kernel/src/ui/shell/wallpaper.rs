use crate::gfx::surface::{lerp, Surface};

use super::tokens::{BG, WALL1, WALL2};

/// Meridian field: near-black with two soft radial washes —
/// teal upper-right (76%, 12%), violet lower-left (12%, 88%).
pub fn render(surface: &mut Surface) {
    let w = surface.width as i32;
    let h = surface.height as i32;

    let blobs = [
        (w * 76 / 100, h * 12 / 100, w * 55 / 100, WALL1, 31u32), // .12
        (w * 12 / 100, h * 88 / 100, w * 52 / 100, WALL2, 26u32), // .10
    ];

    for y in 0..h {
        for x in 0..w {
            let mut c = BG;
            for &(bx, by, radius, color, strength) in &blobs {
                let dx = x - bx;
                let dy = y - by;
                let dist2 = (dx * dx + dy * dy) as u32;
                let rad2 = (radius * radius) as u32;
                if dist2 < rad2 {
                    let t = 255 - dist2 * 255 / rad2;
                    let t = t * t / 255;
                    c = lerp(c, color, t * strength / 255);
                }
            }
            surface.pixels[(y * w + x) as usize] = c;
        }
    }
}
