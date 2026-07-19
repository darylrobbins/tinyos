//! Live system gauges: the instrument-panel showpiece.

use alloc::format;

use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::mem;
use crate::ui::shell::app::{App, Rect};
use crate::ui::shell::tokens::{ACCENT, SURFACE_HI, TEXT, TEXT_DIM};

const SAMPLES: usize = 120;

struct RingBuf {
    data: [u32; SAMPLES],
    at: usize,
}

impl RingBuf {
    const fn new() -> Self {
        Self {
            data: [0; SAMPLES],
            at: 0,
        }
    }

    fn push(&mut self, v: u32) {
        self.data[self.at] = v;
        self.at = (self.at + 1) % SAMPLES;
    }

    fn iter_ordered(&self) -> impl Iterator<Item = u32> + '_ {
        (0..SAMPLES).map(move |i| self.data[(self.at + i) % SAMPLES])
    }

    fn last(&self) -> u32 {
        self.data[(self.at + SAMPLES - 1) % SAMPLES]
    }
}

pub struct MonitorApp {
    fps: RingBuf,
    evs: RingBuf,
    // Accumulators for one-second event buckets.
    ev_acc: u32,
    frame_acc: u32,
    window_start_ms: u64,
}

impl MonitorApp {
    pub fn new() -> Self {
        Self {
            fps: RingBuf::new(),
            evs: RingBuf::new(),
            ev_acc: 0,
            frame_acc: 0,
            window_start_ms: 0,
        }
    }

    /// Fed once per frame from the event loop.
    pub fn tick(&mut self, now_ms: u64, events: u32) {
        self.ev_acc += events;
        self.frame_acc += 1;
        if self.window_start_ms == 0 {
            self.window_start_ms = now_ms;
        }
        // Half-second buckets keep the sparkline lively.
        if now_ms - self.window_start_ms >= 500 {
            self.fps.push(self.frame_acc * 2);
            self.evs.push(self.ev_acc * 2);
            self.frame_acc = 0;
            self.ev_acc = 0;
            self.window_start_ms = now_ms;
        }
    }

}

fn sparkline(s: &mut Surface, ring: &RingBuf, x: i32, y: i32, w: i32, h: i32) {
    let max = ring.iter_ordered().max().unwrap_or(1).max(1);
    let n = w.min(SAMPLES as i32);
    for (i, v) in ring.iter_ordered().skip(SAMPLES - n as usize).enumerate() {
        let bar = (v * h as u32 / max) as i32;
        if bar > 0 {
            s.fill_rect(x + i as i32 * (w / n).max(1), y + h - bar, (w / n).max(1), bar, TEXT_DIM);
        }
    }
    // Needle tick on the latest sample.
    let last = (ring.last() * h as u32 / max) as i32;
    s.fill_rect(x + w - 2, y + h - last.max(2), 2, last.max(2), ACCENT);
}

impl App for MonitorApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn title(&self) -> &str {
        "Monitor"
    }

    fn glyph(&self) -> &str {
        "~"
    }

    fn preferred_size(&self, _sw: i32, _sh: i32) -> (i32, i32) {
        (420, 430)
    }

    fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, body: Rect, _focused: bool, _now: u64) {
        let (used, free) = mem::stats();
        let col_w = body.w;

        // Per-CPU load bars: busy % = 100 - idle % over the last second.
        let ix = body.x + col_w / 2 + 10;
        let iw = col_w / 2 - 10;
        fonts.ui_medium.draw(s, "CPU", 13.0, ix, body.y, TEXT_DIM);
        for cpu in 0..crate::sched::online_cpus() {
            let (_wakes, idle) = crate::arch::irq::wake_stats(cpu);
            let busy = 100u32.saturating_sub(idle);
            let y = body.y + 22 + cpu as i32 * 14;
            let label = format!("{cpu}");
            fonts.mono.draw(s, &label, 12.0, ix + 4, y - 4, TEXT_DIM);
            s.fill_rect(ix + 20, y, iw - 20, 6, SURFACE_HI);
            s.fill_rect(ix + 20, y, ((iw - 20) * busy as i32 / 100).max(2), 6, ACCENT);
        }

        // HEAP bar-meter.
        fonts.ui_medium.draw(s, "Heap", 13.0, body.x, body.y, TEXT_DIM);
        let meter_y = body.y + 22;
        let hw = col_w / 2 - 10;
        let frac = (used as u64 * hw as u64 / (used + free).max(1) as u64) as i32;
        s.fill_rect(body.x, meter_y, hw, 6, SURFACE_HI);
        s.fill_rect(body.x, meter_y, frac.max(2), 6, TEXT);
        let heap_txt = format!("{} / {} MiB", used >> 20, (used + free) >> 20);
        fonts
            .mono
            .draw(s, &heap_txt, 14.0, body.x, meter_y + 14, TEXT_DIM);

        // FPS sparkline.
        fonts.ui_medium.draw(s, "FPS", 13.0, body.x, body.y + 72, TEXT_DIM);
        sparkline(s, &self.fps, body.x, body.y + 92, col_w - 80, 44);
        let fps_txt = format!("{}", self.fps.last());
        fonts
            .mono
            .draw(s, &fps_txt, 26.0, body.x + col_w - 62, body.y + 98, TEXT);

        // INPUT events/sec sparkline.
        fonts.ui_medium.draw(s, "Input / s", 13.0, body.x, body.y + 152, TEXT_DIM);
        sparkline(s, &self.evs, body.x, body.y + 172, col_w - 80, 44);
        let ev_txt = format!("{}", self.evs.last());
        fonts
            .mono
            .draw(s, &ev_txt, 26.0, body.x + col_w - 62, body.y + 178, TEXT);

        // Thread table.
        let ty = body.y + 232;
        fonts.ui_medium.draw(s, "Threads", 13.0, body.x, ty, TEXT_DIM);
        for (row, t) in crate::sched::snapshot().into_iter().take(8).enumerate() {
            let line = format!(
                "{:>3} {:<8} {:<8} cpu{} {:?}",
                t.id,
                &t.name[..t.name.len().min(8)],
                format!("{:?}", t.state),
                t.cpu,
                t.class
            );
            fonts
                .mono
                .draw(s, &line, 13.0, body.x, ty + 20 + row as i32 * 17, TEXT);
        }
    }
}
