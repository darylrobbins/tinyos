use alloc::vec;
use alloc::vec::Vec;

use super::{FbFormat, FbInfo};

/// Colors are 0xAARRGGBB. In memory (little-endian u32) that is B,G,R,A —
/// which matches a BGRX framebuffer byte-for-byte, so presenting to ramfb
/// is a straight copy.
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

pub struct Surface {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl Surface {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0xFF00_0000; width * height],
        }
    }

    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color | 0xFF00_0000);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = y as usize * self.width + x as usize;
            self.pixels[i] = over(self.pixels[i], color);
        }
    }

    fn clip(&self, x: i32, y: i32, w: i32, h: i32) -> Option<(usize, usize, usize, usize)> {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w).min(self.width as i32)).max(0) as usize;
        let y1 = ((y + h).min(self.height as i32)).max(0) as usize;
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else {
            return;
        };
        let opaque = color >> 24 == 255;
        for row in y0..y1 {
            let line = &mut self.pixels[row * self.width + x0..row * self.width + x1];
            if opaque {
                line.fill(color);
            } else {
                for px in line {
                    *px = over(*px, color);
                }
            }
        }
    }

    /// Vertical gradient over the given rect.
    #[allow(dead_code)] // drawing helper kept for future surface work
    pub fn fill_gradient_v(&mut self, x: i32, y: i32, w: i32, h: i32, top: u32, bottom: u32) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else {
            return;
        };
        for row in y0..y1 {
            let t = ((row as i32 - y) * 255 / h.max(1)) as u32;
            let c = lerp(top, bottom, t.min(255));
            for px in &mut self.pixels[row * self.width + x0..row * self.width + x1] {
                *px = over(*px, c);
            }
        }
    }

    /// Rounded rectangle with anti-aliased corners.
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: u32) {
        let r = radius.min(w / 2).min(h / 2).max(0);
        // Center body plus edge strips.
        self.fill_rect(x + r, y, w - 2 * r, h, color);
        self.fill_rect(x, y + r, r, h - 2 * r, color);
        self.fill_rect(x + w - r, y + r, r, h - 2 * r, color);
        // Corners: per-pixel coverage from distance to the corner circle.
        let centers = [
            (x + r, y + r),             // top-left
            (x + w - r - 1, y + r),     // top-right
            (x + r, y + h - r - 1),     // bottom-left
            (x + w - r - 1, y + h - r - 1), // bottom-right
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

    /// Whole-surface copy (dimensions must match).
    pub fn copy_from(&mut self, src: &Surface) {
        self.pixels.copy_from_slice(&src.pixels);
    }

    /// Two-pass box blur; returns a new surface. Run once on static
    /// content (the wallpaper) to back frosted-glass panels.
    pub fn blurred(&self, radius: usize) -> Surface {
        let (w, h) = (self.width, self.height);
        let r = radius as i32;
        let win = 2 * radius as u32 + 1;
        let clamp = |v: i32, hi: i32| v.clamp(0, hi - 1) as usize;
        let mut tmp = Surface::new(w, h);
        let mut out = Surface::new(w, h);

        let pass = |src: &[u32], dst: &mut [u32], len: i32, stride: usize, line_base: usize| {
            let at = |i: i32| src[line_base + clamp(i, len) * stride];
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for i in -r..=r {
                let c = at(i);
                sr += c >> 16 & 0xFF;
                sg += c >> 8 & 0xFF;
                sb += c & 0xFF;
            }
            for i in 0..len {
                dst[line_base + i as usize * stride] =
                    0xFF00_0000 | (sr / win) << 16 | (sg / win) << 8 | sb / win;
                let add = at(i + r + 1);
                let sub = at(i - r);
                sr += (add >> 16 & 0xFF).wrapping_sub(sub >> 16 & 0xFF);
                sg += (add >> 8 & 0xFF).wrapping_sub(sub >> 8 & 0xFF);
                sb += (add & 0xFF).wrapping_sub(sub & 0xFF);
            }
        };

        for y in 0..h {
            pass(&self.pixels, &mut tmp.pixels, w as i32, 1, y * w);
        }
        let tmp_pixels = tmp.pixels.clone();
        for x in 0..w {
            pass(&tmp_pixels, &mut out.pixels, h as i32, w, x);
        }
        out
    }

    /// Rounded-corner pixel coverage for a rect at (x,y,w,h) radius r.
    fn corner_coverage(x: i32, y: i32, w: i32, h: i32, r: i32, px: i32, py: i32) -> f32 {
        let cx = if px < x + r {
            x + r
        } else if px >= x + w - r {
            x + w - r - 1
        } else {
            return 1.0;
        };
        let cy = if py < y + r {
            y + r
        } else if py >= y + h - r {
            y + h - r - 1
        } else {
            return 1.0;
        };
        let dx = (px - cx) as f32;
        let dy = (py - cy) as f32;
        (r as f32 + 0.5 - libm::sqrtf(dx * dx + dy * dy)).clamp(0.0, 1.0)
    }

    /// Frosted-glass panel: rounded rect whose pixels sample a pre-blurred
    /// backdrop, overlaid with `tint`, edged with a hairline highlight.
    pub fn frosted_panel(
        &mut self,
        backdrop: &Surface,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
        tint: u32,
    ) {
        for py in y.max(0)..(y + h).min(self.height as i32) {
            for px in x.max(0)..(x + w).min(self.width as i32) {
                let cov = Self::corner_coverage(x, y, w, h, r, px, py);
                if cov <= 0.0 {
                    continue;
                }
                let idx = py as usize * self.width + px as usize;
                let base = backdrop.pixels[py as usize * backdrop.width + px as usize];
                let mut c = over(base, tint);
                // Hairline light edge on the top row of the panel.
                if py - y <= 1 {
                    c = over(c, 0x2EFF_FFFF);
                }
                let a = (cov * 255.0) as u32;
                self.pixels[idx] = over(self.pixels[idx], (c & 0x00FF_FFFF) | a << 24);
            }
        }
    }

    /// Copy this surface to the hardware framebuffer.
    pub fn present(&self, fb: &FbInfo) {
        let w = self.width.min(fb.width);
        let h = self.height.min(fb.height);
        match fb.format {
            FbFormat::Bgrx => {
                for y in 0..h {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.pixels.as_ptr().add(y * self.width),
                            (fb.base as *mut u32).add(y * fb.stride),
                            w,
                        );
                    }
                }
            }
            FbFormat::Rgbx => {
                for y in 0..h {
                    for x in 0..w {
                        let c = self.pixels[y * self.width + x];
                        let swizzled = (c & 0xFF00_FF00) | (c & 0xFF) << 16 | (c >> 16) & 0xFF;
                        unsafe {
                            (fb.base as *mut u32)
                                .add(y * fb.stride + x)
                                .write_volatile(swizzled)
                        };
                    }
                }
            }
        }
    }
}
