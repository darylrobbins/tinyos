//! Lock screen: full-field wash, giant mono clock, gradient avatar.

use crate::gfx::font::Fonts;
use crate::gfx::surface::{lerp, Surface};

use super::clockpill;
use super::tokens::*;

pub fn draw(s: &mut Surface, fonts: &mut Fonts, wallpaper: &Surface, now: u64) {
    s.copy_from(wallpaper);
    let cx = s.width as i32 / 2;
    let h = s.height as i32;

    let (clock, _) = clockpill::clock_strings();
    // Giant mono clock (canvas-scaled ~110px).
    let (cw, _) = fonts.mono.measure(&clock, 110.0);
    fonts
        .mono
        .draw(s, &clock, 110.0, cx - cw / 2, h / 5, TX);
    let date = "Friday, July 17";
    let (dw, _) = fonts.ui.measure(date, 17.0);
    fonts.ui.draw(s, date, 17.0, cx - dw / 2, h / 5 + 132, TX2);

    // Gradient avatar circle with the user initial.
    let ay = h * 62 / 100;
    let r = 37;
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            if d2 <= r * r {
                let t = (((dx + dy) + 2 * r) * 255 / (4 * r)) as u32;
                let c = lerp(ACC, HUE_VIOLET, t.min(255));
                s.pixels[((ay + dy) as usize) * s.width + (cx + dx) as usize] = c;
            }
        }
    }
    let (iw, _) = fonts.ui_semibold.measure("D", 26.0);
    fonts
        .ui_semibold
        .draw(s, "D", 26.0, cx - iw / 2, ay - 17, ORB_TX);

    let (nw, _) = fonts.ui_semibold.measure("daryl", 15.0);
    fonts
        .ui_semibold
        .draw(s, "daryl", 15.0, cx - nw / 2, ay + 54, TX);

    // Unlock hint with a blinking chevron.
    let hint = "press Enter to unlock";
    let (hw, _) = fonts.ui.measure(hint, 12.0);
    let alpha_on = now / 900 % 2 == 0;
    fonts.ui.draw(
        s,
        hint,
        12.0,
        cx - hw / 2,
        ay + 84,
        if alpha_on { TX2 } else { TX3 },
    );

    let footer = "tinyOS 0.1 \u{201c}meridian\u{201d}";
    let (fw, _) = fonts.mono.measure(footer, 11.0);
    fonts
        .mono
        .draw(s, footer, 11.0, cx - fw / 2, h - 40, TX3);
}
