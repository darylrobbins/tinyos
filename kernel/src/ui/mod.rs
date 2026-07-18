pub mod cursor;
pub mod splash;

/// Ease-in-out cubic, t in 0.0..=1.0.
pub fn ease(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u * u / 2.0
    }
}

pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}
