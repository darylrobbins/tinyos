//! Shell v2 design tokens — soft-dark neutral.

use crate::gfx::surface::{argb, rgb};

pub const FIELD: u32 = rgb(0x0F, 0x12, 0x22);
pub const BLOB_A: u32 = rgb(0x7C, 0x5C, 0xFF); // violet
pub const BLOB_B: u32 = rgb(0x2B, 0xB8, 0xD9); // cyan
pub const SURFACE: u32 = rgb(0x1C, 0x1E, 0x26);
pub const SURFACE_HI: u32 = rgb(0x25, 0x28, 0x34);
pub const BORDER: u32 = argb(20, 255, 255, 255);
pub const ACCENT: u32 = rgb(0x7C, 0x5C, 0xFF);
pub const TEXT: u32 = rgb(0xE6, 0xE8, 0xF0);
pub const TEXT_DIM: u32 = rgb(0x8A, 0x8F, 0xA3);

pub const RADIUS: i32 = 14;
pub const TILE_RADIUS: i32 = 10;
pub const STATUS_H: i32 = 30;
pub const TITLE_H: i32 = 34;
