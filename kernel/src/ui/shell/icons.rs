//! Meridian icon set: thin-stroke vector icons drawn at runtime with the
//! anti-aliased primitives on `Surface`. Each icon is authored in a normalized
//! `[0,1]` box and mapped onto a `size`x`size` square centered at `(cx, cy)`,
//! so a single glyph scales crisply from a 16px control to a 46px launcher
//! tile. Shapes trace the mockup's inline SVGs (`docs/reference/meridian-os.html`).

use crate::gfx::surface::Surface;

#[derive(Clone, Copy, PartialEq, Eq)]
// The full Meridian app roster is kept as a documented icon set; a few members
// (Files/Compass/Chat/Music/Settings) map to mockup apps tinyOS doesn't ship yet.
#[allow(dead_code)]
pub enum Icon {
    /// Terminal: chevron prompt + underscore (`❯_`).
    Terminal,
    /// Notes: a document page with text lines.
    Notes,
    /// Monitor: a heartbeat / waveform pulse.
    Monitor,
    /// Clock: a ringed face with two hands.
    Clock,
    /// Launcher orb: a bold chevron (`❯`) over the gradient tile.
    Orb,
    /// Search: a magnifier (launcher header).
    Search,
    /// Settings: three sliders with knobs.
    Settings,
    /// Files: a folder.
    Files,
    /// Compass / browser: a globe.
    Compass,
    /// Relay / chat: a speech bubble.
    Chat,
    /// Tempo / music: two note heads on a beam.
    Music,
    /// A padlock (quick settings lock tile, launcher lock).
    Lock,
    /// An equals sign (calculator suggestion).
    Equals,
    /// Generic application fallback.
    App,
    /// Window control: minimize (`–`).
    Minimize,
    /// Window control: maximize (`□`).
    Maximize,
    /// Window control: close (`✕`).
    Close,
}

/// Map a normalized `[0,1]` box coordinate to an absolute pixel coordinate.
#[inline]
fn m(cx: f32, cy: f32, size: f32, nx: f32, ny: f32) -> (f32, f32) {
    (cx + (nx - 0.5) * size, cy + (ny - 0.5) * size)
}

/// Draw `icon` centered at `(cx, cy)` filling a `size`x`size` box, stroked or
/// filled in `color` (single hue, matching the mockup's `currentColor` icons).
pub fn draw(s: &mut Surface, icon: Icon, cx: i32, cy: i32, size: f32, color: u32) {
    let cx = cx as f32;
    let cy = cy as f32;
    // Default stroke width tracks the box size (~1.6px at 20px, like the SVGs).
    let sw = (size / 11.0).max(1.4);
    let p = |nx: f32, ny: f32| m(cx, cy, size, nx, ny);
    let line = |s: &mut Surface, a: (f32, f32), b: (f32, f32), w: f32| {
        s.stroke_line(a.0, a.1, b.0, b.1, w, color);
    };

    match icon {
        Icon::Terminal => {
            // `>` chevron on the left, underscore on the lower right.
            s.stroke_polyline(&[p(0.16, 0.26), p(0.5, 0.5), p(0.16, 0.74)], sw, color);
            line(s, p(0.6, 0.76), p(0.9, 0.76), sw);
        }
        Icon::Orb => {
            // Bold chevron, centered.
            s.stroke_polyline(
                &[p(0.34, 0.24), p(0.66, 0.5), p(0.34, 0.76)],
                (size / 7.0).max(2.0),
                color,
            );
        }
        Icon::Notes => {
            // Page outline with a folded top-right corner + three text lines.
            let x = p(0.26, 0.12);
            let sz = size;
            s.stroke_round_rect(x.0, x.1, 0.48 * sz, 0.76 * sz, 0.06 * sz, sw, color);
            for ny in [0.34, 0.5, 0.66] {
                line(s, p(0.37, ny), p(0.63, ny), sw * 0.9);
            }
        }
        Icon::Monitor => {
            // EKG-style pulse across the middle.
            s.stroke_polyline(
                &[
                    p(0.08, 0.5),
                    p(0.3, 0.5),
                    p(0.4, 0.28),
                    p(0.52, 0.74),
                    p(0.62, 0.44),
                    p(0.7, 0.5),
                    p(0.92, 0.5),
                ],
                sw,
                color,
            );
        }
        Icon::Clock => {
            let c = p(0.5, 0.5);
            s.stroke_circle(c.0, c.1, 0.36 * size, sw, color);
            line(s, c, p(0.5, 0.26), sw);
            line(s, c, p(0.66, 0.56), sw);
        }
        Icon::Search => {
            let c = p(0.42, 0.42);
            s.stroke_circle(c.0, c.1, 0.26 * size, sw, color);
            line(s, p(0.62, 0.62), p(0.84, 0.84), sw);
        }
        Icon::Settings => {
            // Three sliders, each a rail with a knob at a different position.
            let knobs = [(0.28, 0.68), (0.5, 0.34), (0.72, 0.72)];
            for (ny, kx) in knobs {
                line(s, p(0.14, ny), p(0.86, ny), sw * 0.85);
                let k = p(kx, ny);
                s.fill_circle(k.0, k.1, 0.09 * size, color);
            }
        }
        Icon::Files => {
            // Folder: back tab + front body, filled.
            let tab = p(0.12, 0.26);
            s.fill_rounded_rect(
                tab.0 as i32,
                tab.1 as i32,
                (0.4 * size) as i32,
                (0.2 * size) as i32,
                (0.05 * size) as i32,
                color,
            );
            let body = p(0.12, 0.34);
            s.fill_rounded_rect(
                body.0 as i32,
                body.1 as i32,
                (0.76 * size) as i32,
                (0.48 * size) as i32,
                (0.07 * size) as i32,
                color,
            );
        }
        Icon::Compass => {
            let c = p(0.5, 0.5);
            let r = 0.38 * size;
            s.stroke_circle(c.0, c.1, r, sw, color);
            // Equator.
            line(s, p(0.12, 0.5), p(0.88, 0.5), sw * 0.85);
            // Vertical meridian: sampled ellipse (rx small, ry = r).
            let mut pts = alloc::vec::Vec::with_capacity(25);
            for i in 0..=24 {
                let t = i as f32 / 24.0 * core::f32::consts::TAU;
                pts.push((
                    c.0 + libm::sinf(t) * 0.15 * size,
                    c.1 + libm::cosf(t) * r,
                ));
            }
            s.stroke_polyline(&pts, sw * 0.85, color);
        }
        Icon::Chat => {
            let x = p(0.12, 0.16);
            s.stroke_round_rect(x.0, x.1, 0.76 * size, 0.5 * size, 0.14 * size, sw, color);
            // Tail off the lower-left.
            s.stroke_polyline(&[p(0.32, 0.64), p(0.24, 0.86), p(0.48, 0.64)], sw, color);
        }
        Icon::Music => {
            // Two note heads with stems joined by a beam.
            let h1 = p(0.3, 0.76);
            let h2 = p(0.72, 0.66);
            s.fill_circle(h1.0, h1.1, 0.1 * size, color);
            s.fill_circle(h2.0, h2.1, 0.1 * size, color);
            line(s, p(0.39, 0.76), p(0.39, 0.24), sw);
            line(s, p(0.81, 0.66), p(0.81, 0.16), sw);
            line(s, p(0.39, 0.24), p(0.81, 0.16), sw);
        }
        Icon::Lock => {
            // Body + shackle arc.
            let b = p(0.28, 0.46);
            s.stroke_round_rect(b.0, b.1, 0.44 * size, 0.38 * size, 0.06 * size, sw, color);
            let mut pts = alloc::vec::Vec::with_capacity(13);
            let c = p(0.5, 0.46);
            let r = 0.16 * size;
            for i in 0..=12 {
                let t = core::f32::consts::PI * (i as f32 / 12.0); // 0..PI (top half)
                pts.push((c.0 - libm::cosf(t) * r, c.1 - libm::sinf(t) * r));
            }
            s.stroke_polyline(&pts, sw, color);
        }
        Icon::Equals => {
            line(s, p(0.28, 0.42), p(0.72, 0.42), sw);
            line(s, p(0.28, 0.58), p(0.72, 0.58), sw);
        }
        Icon::App => {
            let x = p(0.22, 0.22);
            s.stroke_round_rect(x.0, x.1, 0.56 * size, 0.56 * size, 0.16 * size, sw, color);
            let c = p(0.5, 0.5);
            s.fill_circle(c.0, c.1, 0.09 * size, color);
        }
        Icon::Minimize => {
            line(s, p(0.26, 0.52), p(0.74, 0.52), sw);
        }
        Icon::Maximize => {
            let x = p(0.28, 0.28);
            s.stroke_round_rect(x.0, x.1, 0.44 * size, 0.44 * size, 0.1 * size, sw, color);
        }
        Icon::Close => {
            line(s, p(0.3, 0.3), p(0.7, 0.7), sw);
            line(s, p(0.7, 0.3), p(0.3, 0.7), sw);
        }
    }
}
