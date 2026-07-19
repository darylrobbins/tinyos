//! Minimal immediate-mode widgets over `gfx::Canvas`: feed window events into
//! a per-frame `UiInput`, then call widget functions during drawing. Styled to
//! the Meridian design language.

use crate::gfx::{self, measure_text, Canvas, Rect};
use crate::window::Event;

/// Per-frame pointer snapshot for immediate-mode hit testing.
#[derive(Default)]
pub struct UiInput {
    pub pointer: (i32, i32),
    pub down: bool,
    /// Position where the button went down this frame, if it did.
    pub pressed: Option<(i32, i32)>,
    /// Position where the button went up this frame, if it did.
    pub released: Option<(i32, i32)>,
}

impl UiInput {
    /// Reset the edge-triggered state; call once at the top of each frame,
    /// before feeding that frame's events.
    pub fn begin_frame(&mut self) {
        self.pressed = None;
        self.released = None;
    }

    pub fn feed(&mut self, ev: &Event) {
        match *ev {
            Event::PointerMoved { x, y } => self.pointer = (x, y),
            Event::Button { down, x, y } => {
                self.pointer = (x, y);
                self.down = down;
                if down {
                    self.pressed = Some((x, y));
                } else {
                    self.released = Some((x, y));
                }
            }
            _ => {}
        }
    }
}

/// A Meridian-style pill button. Draws with hover/pressed states and returns
/// true when clicked (button released inside the rect this frame).
pub fn button(c: &mut Canvas, ui: &UiInput, r: Rect, label: &str) -> bool {
    let hover = r.contains(ui.pointer.0, ui.pointer.1);
    let held = hover && ui.down;
    let fill = if held {
        gfx::with_alpha(gfx::ACC, 60)
    } else if hover {
        gfx::argb(31, 0xff, 0xff, 0xff)
    } else {
        gfx::CARD2
    };
    let radius = r.h / 2;
    c.fill_rounded_rect(r, radius, fill);
    c.stroke_rounded_rect(r, radius, 1, if hover { gfx::STROKE2 } else { gfx::STROKE });
    let (tw, th) = measure_text(label, 1);
    c.draw_text(
        r.x + (r.w - tw) / 2,
        r.y + (r.h - th) / 2 + 1,
        label,
        1,
        gfx::ACC,
    );
    hover && ui.released.is_some()
}

/// A plain text label.
pub fn label(c: &mut Canvas, x: i32, y: i32, text: &str, color: u32) {
    c.draw_text(x, y, text, 1, color);
}
