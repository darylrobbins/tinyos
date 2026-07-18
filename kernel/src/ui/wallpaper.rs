use crate::gfx::surface::{lerp, rgb, Surface};

/// "Aurora" wallpaper: diagonal indigo-to-teal sweep with a violet glow
/// low in the frame. Purely procedural; rendered once at startup.
pub fn render(surface: &mut Surface) {
    let w = surface.width as i32;
    let h = surface.height as i32;

    const TOP: u32 = rgb(24, 21, 54); // deep indigo
    const MID: u32 = rgb(64, 42, 110); // violet
    const BOT: u32 = rgb(16, 84, 106); // teal

    for y in 0..h {
        for x in 0..w {
            // Diagonal position 0..255 across the frame.
            let d = ((x + y * 2) * 255 / (w + h * 2)) as u32;
            let base = if d < 140 {
                lerp(TOP, MID, d * 255 / 140)
            } else {
                lerp(MID, BOT, (d - 140) * 255 / 115)
            };

            // Soft violet glow centered low-left.
            let gx = x - w / 4;
            let gy = y - h * 3 / 4;
            let dist2 = (gx * gx + gy * gy) as u32;
            let rad2 = (w * w / 9) as u32;
            let glow = if dist2 < rad2 {
                (255 - dist2 * 255 / rad2) * 90 / 255
            } else {
                0
            };

            surface.pixels[(y * w + x) as usize] = lerp(base, rgb(150, 92, 200), glow);
        }
    }
}
