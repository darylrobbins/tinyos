//! Userspace drawing over a window's BGRA pixel buffer: a clipped canvas,
//! alpha blending, rounded rects and a built-in 8x8 bitmap font with integer
//! scaling. Blend math mirrors the kernel rasterizer (`kernel/src/gfx/
//! surface.rs`); colors are 0xAARRGGBB.

use crate::font8x8::{FONT8X8, GLYPH_H, GLYPH_W};
use crate::uifont;

// Meridian design tokens the SDK re-exports for app use. Values mirror
// `kernel/src/ui/shell/tokens.rs` (source of truth:
// docs/reference/meridian-os.html); userspace cannot see kernel code.
pub const BG: u32 = rgb(0x07, 0x09, 0x0d);
pub const TX: u32 = rgb(0xe8, 0xec, 0xf2);
pub const TX2: u32 = rgb(0x9a, 0xa4, 0xb5);
pub const TX3: u32 = rgb(0x5f, 0x68, 0x79);
pub const ACC: u32 = rgb(0x5f, 0xd4, 0xc4);
pub const HUE_RED: u32 = rgb(0xff, 0x9e, 0x9e);
pub const CARD: u32 = argb(11, 0xff, 0xff, 0xff); // white @ .045
pub const CARD2: u32 = argb(20, 0xff, 0xff, 0xff); // white @ .08
pub const STROKE: u32 = argb(23, 0xff, 0xff, 0xff); // white @ .09
pub const STROKE2: u32 = argb(41, 0xff, 0xff, 0xff); // white @ .16

pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    argb(255, r, g, b)
}

pub fn with_alpha(color: u32, a: u8) -> u32 {
    let scaled = ((color >> 24) * a as u32 / 255) << 24;
    (color & 0x00FF_FFFF) | scaled
}

/// Standard source-over blend of `src` onto opaque-ish `dst`.
pub fn over(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    match sa {
        0 => dst,
        255 => src | 0xFF00_0000,
        _ => {
            let na = 255 - sa;
            let rb = ((src & 0x00FF_00FF) * sa + (dst & 0x00FF_00FF) * na) >> 8;
            let g = ((src & 0x0000_FF00) * sa + (dst & 0x0000_FF00) * na) >> 8;
            0xFF00_0000 | (rb & 0x00FF_00FF) | (g & 0x0000_FF00)
        }
    }
}

/// Linear interpolation between two colors, t in 0..=255.
pub fn lerp(c0: u32, c1: u32, t: u32) -> u32 {
    let nt = 255 - t;
    let rb = (((c0 & 0x00FF_00FF) * nt + (c1 & 0x00FF_00FF) * t) >> 8) & 0x00FF_00FF;
    let g = (((c0 & 0x0000_FF00) * nt + (c1 & 0x0000_FF00) * t) >> 8) & 0x0000_FF00;
    let a = ((c0 >> 24) * nt + (c1 >> 24) * t) >> 8;
    a << 24 | rb | g
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Area of overlap with `other`, in pixels.
    pub fn overlap(&self, other: &Rect) -> i32 {
        let ox = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let oy = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        if ox > 0 && oy > 0 {
            ox * oy
        } else {
            0
        }
    }
}

/// A drawing target over a window's pixel buffer (row-major, stride = w).
pub struct Canvas<'a> {
    px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
}

impl<'a> Canvas<'a> {
    pub fn new(px: &'a mut [u32], w: i32, h: i32) -> Canvas<'a> {
        Canvas { px, w, h }
    }

    pub fn clear(&mut self, color: u32) {
        self.px.fill(color | 0xFF00_0000);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = (y * self.w + x) as usize;
            self.px[i] = over(self.px[i], color);
        }
    }

    fn clip(&self, r: Rect) -> Option<(i32, i32, i32, i32)> {
        let x0 = r.x.max(0);
        let y0 = r.y.max(0);
        let x1 = (r.x + r.w).min(self.w);
        let y1 = (r.y + r.h).min(self.h);
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }

    pub fn fill_rect(&mut self, r: Rect, color: u32) {
        let Some((x0, y0, x1, y1)) = self.clip(r) else {
            return;
        };
        let opaque = color >> 24 == 255;
        for row in y0..y1 {
            let line = &mut self.px[(row * self.w + x0) as usize..(row * self.w + x1) as usize];
            if opaque {
                line.fill(color);
            } else {
                for px in line {
                    *px = over(*px, color);
                }
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, w, 1), color);
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, 1, h), color);
    }

    /// Vertical gradient over the given rect.
    pub fn fill_gradient_v(&mut self, r: Rect, top: u32, bottom: u32) {
        let Some((x0, y0, x1, y1)) = self.clip(r) else {
            return;
        };
        for row in y0..y1 {
            let t = ((row - r.y) * 255 / r.h.max(1)) as u32;
            let c = lerp(top, bottom, t.min(255));
            for px in &mut self.px[(row * self.w + x0) as usize..(row * self.w + x1) as usize] {
                *px = over(*px, c);
            }
        }
    }

    /// Rounded rectangle with anti-aliased corners.
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: i32, color: u32) {
        let Rect { x, y, w, h } = rect;
        let r = radius.min(w / 2).min(h / 2).max(0);
        // Center body plus edge strips.
        self.fill_rect(Rect::new(x + r, y, w - 2 * r, h), color);
        self.fill_rect(Rect::new(x, y + r, r, h - 2 * r), color);
        self.fill_rect(Rect::new(x + w - r, y + r, r, h - 2 * r), color);
        // Corners: per-pixel coverage from distance to the corner circle.
        let centers = [
            (x + r, y + r),
            (x + w - r - 1, y + r),
            (x + r, y + h - r - 1),
            (x + w - r - 1, y + h - r - 1),
        ];
        for (ci, &(cx, cy)) in centers.iter().enumerate() {
            let (sx, sy) = match ci {
                0 => (x, y),
                1 => (x + w - r, y),
                2 => (x, y + h - r),
                _ => (x + w - r, y + h - r),
            };
            for py in sy..sy + r {
                for px in sx..sx + r {
                    let dx = (px - cx) as f32;
                    let dy = (py - cy) as f32;
                    let dist = libm::sqrtf(dx * dx + dy * dy);
                    let cov = (r as f32 + 0.5 - dist).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        let a = ((color >> 24) as f32 * cov) as u32;
                        self.put(px, py, (color & 0x00FF_FFFF) | a << 24);
                    }
                }
            }
        }
    }

    /// Rounded-rect outline: filled ring of `thickness` pixels.
    pub fn stroke_rounded_rect(&mut self, rect: Rect, radius: i32, thickness: i32, color: u32) {
        let Rect { x, y, w, h } = rect;
        if w <= 0 || h <= 0 {
            return;
        }
        // Cap at (dim-1)/2 so the clamp bounds below stay ordered.
        let r = radius.min((w - 1) / 2).min((h - 1) / 2).max(0);
        let t = thickness.max(1) as f32;
        for py in y..y + h {
            for px in x..x + w {
                // Distance from the rect's rounded boundary, negative inside.
                let cx = px.clamp(x + r, x + w - r - 1);
                let cy = py.clamp(y + r, y + h - r - 1);
                let (dx, dy) = ((px - cx) as f32, (py - cy) as f32);
                let corner = dx != 0.0 || dy != 0.0;
                let dist = if corner {
                    libm::sqrtf(dx * dx + dy * dy) - r as f32
                } else {
                    // Straight edges: distance to the nearest side.
                    let to_edge = (px - x)
                        .min(x + w - 1 - px)
                        .min(py - y)
                        .min(y + h - 1 - py);
                    -(to_edge as f32)
                };
                // Coverage of a ring [-t, 0] around the boundary.
                let cov = (0.5 - dist).clamp(0.0, 1.0) * (dist + t + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let a = ((color >> 24) as f32 * cov) as u32;
                    self.put(px, py, (color & 0x00FF_FFFF) | a << 24);
                }
            }
        }
    }

    /// Draw text with the built-in 8x8 font at an integer `scale`.
    pub fn draw_text(&mut self, x: i32, y: i32, s: &str, scale: i32, color: u32) {
        let mut cx = x;
        for ch in s.chars() {
            let idx = (ch as usize).wrapping_sub(32);
            if let Some(glyph) = FONT8X8.get(idx) {
                for (gy, row) in glyph.iter().enumerate() {
                    for gx in 0..GLYPH_W {
                        if row >> gx & 1 != 0 {
                            self.fill_rect(
                                Rect::new(cx + gx * scale, y + gy as i32 * scale, scale, scale),
                                color,
                            );
                        }
                    }
                }
            }
            cx += GLYPH_W * scale;
        }
    }

    /// Draw a 1bpp mask (row-major, bit 0 = leftmost) at an integer scale.
    /// Rows are `w` bits wide, packed into u16s.
    pub fn draw_mask(&mut self, x: i32, y: i32, mask: &[u16], w: i32, h: i32, scale: i32, color: u32) {
        for my in 0..h.min(mask.len() as i32) {
            let row = mask[my as usize];
            for mx in 0..w.min(16) {
                if row >> mx & 1 != 0 {
                    self.fill_rect(Rect::new(x + mx * scale, y + my * scale, scale, scale), color);
                }
            }
        }
    }

    /// Draw an 8bpp alpha mask (row-major coverage, e.g. a pre-rasterized
    /// anti-aliased glyph) tinted with `color`.
    pub fn draw_alpha_mask(&mut self, x: i32, y: i32, mask: &[u8], w: i32, h: i32, color: u32) {
        let ca = color >> 24;
        for my in 0..h {
            for mx in 0..w {
                let a = mask[(my * w + mx) as usize] as u32;
                if a > 0 {
                    let sa = (a * ca / 255) as u32;
                    self.put(x + mx, y + my, (color & 0x00FF_FFFF) | sa << 24);
                }
            }
        }
    }
}

/// Pixel size of `s` drawn with `draw_text` at `scale`.
pub fn measure_text(s: &str, scale: i32) -> (i32, i32) {
    (s.chars().count() as i32 * GLYPH_W * scale, GLYPH_H * scale)
}

impl<'a> Canvas<'a> {
    /// Draw `s` with the anti-aliased proportional UI font (Geist SemiBold
    /// 15px); `y` is the top of the line box. Non-ASCII chars are skipped.
    pub fn draw_ui_text(&mut self, x: i32, y: i32, s: &str, color: u32) {
        let baseline = y + uifont::ASCENT;
        let mut pen = x;
        for ch in s.chars() {
            let Some(g) = uifont::GLYPHS.get((ch as usize).wrapping_sub(32)) else {
                continue;
            };
            if g.w > 0 {
                self.draw_alpha_mask(pen + g.ox, baseline - g.oy, g.data, g.w, g.h, color);
            }
            pen += g.adv;
        }
    }
}

/// Pixel size of `s` drawn with `draw_ui_text`.
pub fn measure_ui_text(s: &str) -> (i32, i32) {
    let w = s
        .chars()
        .filter_map(|ch| uifont::GLYPHS.get((ch as usize).wrapping_sub(32)))
        .map(|g| g.adv)
        .sum();
    (w, uifont::LINE_H)
}
