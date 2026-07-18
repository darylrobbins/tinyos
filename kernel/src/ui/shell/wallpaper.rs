use crate::gfx::surface::{lerp, Surface};

use super::tokens::{BLOB_A, BLOB_B, FIELD};

/// Mesh-gradient wallpaper: deep navy field with two big soft radial
/// blobs (violet low-left, cyan upper-right). Rendered once at startup.
pub fn render(surface: &mut Surface) {
    let w = surface.width as i32;
    let h = surface.height as i32;

    let blobs = [
        (w / 4, h * 3 / 4, w / 2, BLOB_A, 90u32),  // violet, 35% max
        (w * 5 / 6, h / 5, w / 2, BLOB_B, 76u32),  // cyan, 30% max
    ];

    for y in 0..h {
        for x in 0..w {
            let mut c = FIELD;
            for &(bx, by, radius, color, strength) in &blobs {
                let dx = x - bx;
                let dy = y - by;
                let dist2 = (dx * dx + dy * dy) as u32;
                let rad2 = (radius * radius) as u32;
                if dist2 < rad2 {
                    // Quadratic falloff reads softer than linear.
                    let t = 255 - dist2 * 255 / rad2;
                    let t = t * t / 255;
                    c = lerp(c, color, t * strength / 255);
                }
            }
            surface.pixels[(y * w + x) as usize] = c;
        }
    }
}
