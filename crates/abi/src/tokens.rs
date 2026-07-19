//! Meridian design tokens, shared by the kernel compositor and the app SDK.
//! Source of truth: docs/reference/meridian-os.html. Colors are 0xAARRGGBB.

pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    argb(255, r, g, b)
}

pub const BG: u32 = rgb(0x07, 0x09, 0x0d);
/// Radial wash hues (blended into the wallpaper at low strength).
pub const WALL1: u32 = rgb(0x5f, 0xd4, 0xc4); // teal, 12%
pub const WALL2: u32 = rgb(0x7a, 0x6e, 0xe4); // violet, 10%

/// Window glass tint: rgba(17,20,26,.82) over the blurred backdrop.
pub const WIN_TINT: u32 = argb(209, 0x11, 0x14, 0x1a);
/// Pill/launcher glass tint: rgba(14,17,23,.72).
pub const GLASS_TINT: u32 = argb(184, 0x0e, 0x11, 0x17);

pub const CARD: u32 = argb(11, 255, 255, 255); // white .045
pub const CARD2: u32 = argb(20, 255, 255, 255); // white .08
pub const STROKE: u32 = argb(23, 255, 255, 255); // white .09
pub const STROKE2: u32 = argb(41, 255, 255, 255); // white .16

pub const TX: u32 = rgb(0xe8, 0xec, 0xf2);
pub const TX2: u32 = rgb(0x9a, 0xa4, 0xb5);
pub const TX3: u32 = rgb(0x5f, 0x68, 0x79);

pub const ACC: u32 = rgb(0x5f, 0xd4, 0xc4);
pub const ACC_TX: u32 = rgb(0x05, 0x2a, 0x24);

// Secondary hues: app icons and syntax only.
pub const HUE_AMBER: u32 = rgb(0xe2, 0xb8, 0x6b);
pub const HUE_BLUE: u32 = rgb(0x7f, 0xb2, 0xff);
pub const HUE_VIOLET: u32 = rgb(0xb7, 0x9b, 0xff);
pub const HUE_RED: u32 = rgb(0xff, 0x9e, 0x9e);
/// Dark ink on the orb gradient.
pub const ORB_TX: u32 = rgb(0x08, 0x11, 0x0f);

pub const RADIUS_WIN: i32 = 14;
pub const RADIUS_PILL: i32 = 18;
pub const RADIUS_TILE: i32 = 13;
pub const TITLE_H: i32 = 44;
