pub mod font;
pub mod surface;

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
    #[allow(dead_code)] // framebuffer-format-aware pixel packing, used by future gfx paths
    pub fn pack(&self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            FbFormat::Rgbx => (r as u32) | (g as u32) << 8 | (b as u32) << 16,
            FbFormat::Bgrx => (b as u32) | (g as u32) << 8 | (r as u32) << 16,
        }
    }
}

