/// How the GOP framebuffer lays out a pixel in memory (byte order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FbFormat {
    /// R, G, B, reserved
    Rgbx,
    /// B, G, R, reserved
    Bgrx,
}

pub struct FbInfo {
    pub base: *mut u8,
    pub width: usize,
    pub height: usize,
    /// In pixels, not bytes.
    pub stride: usize,
    pub format: FbFormat,
}

unsafe impl Send for FbInfo {}

impl FbInfo {
    pub fn pack(&self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            FbFormat::Rgbx => (r as u32) | (g as u32) << 8 | (b as u32) << 16,
            FbFormat::Bgrx => (b as u32) | (g as u32) << 8 | (r as u32) << 16,
        }
    }
}

/// M1 proof of life: vertical gradient, midnight blue to teal.
pub fn test_pattern(fb: &FbInfo) {
    let ptr = fb.base as *mut u32;
    for y in 0..fb.height {
        let t = (y * 255 / fb.height) as u32;
        let r = (10 + t * 20 / 255) as u8;
        let g = (15 + t * 120 / 255) as u8;
        let b = (40 + t * 110 / 255) as u8;
        let px = fb.pack(r, g, b);
        for x in 0..fb.width {
            unsafe { ptr.add(y * fb.stride + x).write_volatile(px) };
        }
    }
}
