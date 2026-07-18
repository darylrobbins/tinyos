use crate::arch::timer;
use crate::gfx::font::Fonts;
use crate::gfx::surface::{rgb, with_alpha, Surface};
use crate::gfx::FbInfo;

use super::{clamp01, ease};

const BG: u32 = rgb(8, 8, 12);
const FADE_IN_MS: f32 = 600.0;
const PROGRESS_START_MS: f32 = 500.0;
const PROGRESS_MS: f32 = 2200.0;
const FADE_OUT_MS: f32 = 450.0;
const TOTAL_MS: f32 = PROGRESS_START_MS + PROGRESS_MS + FADE_OUT_MS + 250.0;

const FRAME_US: u64 = 16_667;

/// Apple-style boot splash: dark screen, wordmark, thin progress bar.
pub fn run(fb: &FbInfo, surface: &mut Surface, fonts: &mut Fonts) {
    let cx = surface.width as i32 / 2;
    let cy = surface.height as i32 / 2;

    let bar_w = 240;
    let bar_h = 6;
    let bar_x = cx - bar_w / 2;
    let bar_y = cy + 70;

    let start = timer::uptime_us();
    loop {
        let elapsed_ms = (timer::uptime_us() - start) as f32 / 1000.0;
        if elapsed_ms >= TOTAL_MS {
            break;
        }

        surface.clear(BG);

        // Wordmark fades in.
        let fade_in = ease(clamp01(elapsed_ms / FADE_IN_MS));
        let alpha = (fade_in * 255.0) as u8;
        fonts.ui_semibold.draw_centered(
            surface,
            "tinyOS",
            64.0,
            cx,
            cy - 80,
            with_alpha(rgb(240, 240, 245), alpha),
        );

        // Progress bar: track plus eased fill.
        let progress = ease(clamp01((elapsed_ms - PROGRESS_START_MS) / PROGRESS_MS));
        surface.fill_rounded_rect(
            bar_x,
            bar_y,
            bar_w,
            bar_h,
            bar_h / 2,
            with_alpha(rgb(255, 255, 255), (0.16 * alpha as f32) as u8),
        );
        let fill_w = (progress * bar_w as f32) as i32;
        if fill_w > bar_h {
            surface.fill_rounded_rect(
                bar_x,
                bar_y,
                fill_w,
                bar_h,
                bar_h / 2,
                with_alpha(rgb(255, 255, 255), (0.9 * alpha as f32) as u8),
            );
        }

        // Global fade to black at the end.
        let fade_out = clamp01((elapsed_ms - (TOTAL_MS - FADE_OUT_MS)) / FADE_OUT_MS);
        if fade_out > 0.0 {
            surface.fill_rect(
                0,
                0,
                surface.width as i32,
                surface.height as i32,
                with_alpha(rgb(0, 0, 0), (ease(fade_out) * 255.0) as u8),
            );
        }

        surface.present(fb);

        let next = timer::uptime_us() / FRAME_US * FRAME_US + FRAME_US;
        timer::wait_until_us(next);
    }
}
