use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use fontdue::{Font as FontdueFont, FontSettings, Metrics};

use super::surface::{over, Surface};

pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/inter-regular.ttf");
pub static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/inter-medium.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/inter-semibold.ttf");
pub static JB_MONO: &[u8] = include_bytes!("../../../assets/jbmono.ttf");

/// A parsed font plus a rasterized-glyph cache keyed by (glyph index, px).
pub struct Font {
    font: FontdueFont,
    cache: BTreeMap<(u16, u32), (Metrics, Vec<u8>)>,
}

impl Font {
    pub fn load(data: &[u8]) -> Self {
        let font = FontdueFont::from_bytes(data, FontSettings::default())
            .expect("font parse failed");
        Self {
            font,
            cache: BTreeMap::new(),
        }
    }

    fn glyph(&mut self, index: u16, px: f32) -> &(Metrics, Vec<u8>) {
        self.cache
            .entry((index, px as u32))
            .or_insert_with(|| self.font.rasterize_indexed(index, px))
    }

    pub fn measure(&self, text: &str, px: f32) -> (i32, i32) {
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.append(
            core::slice::from_ref(&self.font),
            &TextStyle::new(text, px, 0),
        );
        let mut w = 0i32;
        for g in layout.glyphs() {
            w = w.max((g.x + g.width as f32) as i32);
        }
        (w, layout.height() as i32)
    }

    /// Draw text with its top-left at (x, y).
    pub fn draw(&mut self, surface: &mut Surface, text: &str, px: f32, x: i32, y: i32, color: u32) {
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.reset(&LayoutSettings {
            x: 0.0,
            y: 0.0,
            ..LayoutSettings::default()
        });
        layout.append(
            core::slice::from_ref(&self.font),
            &TextStyle::new(text, px, 0),
        );
        let glyphs: Vec<_> = layout.glyphs().clone();
        for g in glyphs {
            if g.width == 0 {
                continue;
            }
            let (metrics, coverage) = self.glyph(g.key.glyph_index, g.key.px);
            let gw = metrics.width;
            for (i, &cov) in coverage.iter().enumerate() {
                if cov == 0 {
                    continue;
                }
                let gx = x + g.x as i32 + (i % gw) as i32;
                let gy = y + g.y as i32 + (i / gw) as i32;
                let a = (color >> 24) * cov as u32 / 255;
                let px_color = (color & 0x00FF_FFFF) | a << 24;
                if gx >= 0
                    && gy >= 0
                    && (gx as usize) < surface.width
                    && (gy as usize) < surface.height
                {
                    let idx = gy as usize * surface.width + gx as usize;
                    surface.pixels[idx] = over(surface.pixels[idx], px_color);
                }
            }
        }
    }

    /// Draw text horizontally centered on `cx`.
    pub fn draw_centered(
        &mut self,
        surface: &mut Surface,
        text: &str,
        px: f32,
        cx: i32,
        y: i32,
        color: u32,
    ) {
        let (w, _) = self.measure(text, px);
        self.draw(surface, text, px, cx - w / 2, y, color);
    }
}

pub struct Fonts {
    pub ui: Font,
    pub ui_medium: Font,
    pub ui_semibold: Font,
    pub mono: Font,
}

impl Fonts {
    pub fn load() -> Self {
        Self {
            ui: Font::load(INTER_REGULAR),
            ui_medium: Font::load(INTER_MEDIUM),
            ui_semibold: Font::load(INTER_SEMIBOLD),
            mono: Font::load(JB_MONO),
        }
    }
}
