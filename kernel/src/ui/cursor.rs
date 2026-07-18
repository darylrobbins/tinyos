use crate::gfx::surface::{rgb, Surface};

/// macOS-style arrow: black fill, white outline. '#' fill, '.' outline.
const ARROW: [&str; 18] = [
    ".           ",
    "..          ",
    ".#.         ",
    ".##.        ",
    ".###.       ",
    ".####.      ",
    ".#####.     ",
    ".######.    ",
    ".#######.   ",
    ".########.  ",
    ".#####....  ",
    ".##.##.     ",
    ".#. .##.    ",
    "..  .##.    ",
    ".    .##.   ",
    "     .##.   ",
    "      ..    ",
    "            ",
];

pub fn draw(surface: &mut Surface, x: i32, y: i32) {
    for (dy, row) in ARROW.iter().enumerate() {
        for (dx, c) in row.bytes().enumerate() {
            let color = match c {
                b'#' => rgb(20, 20, 24),
                b'.' => rgb(245, 245, 248),
                _ => continue,
            };
            surface.put(x + dx as i32, y + dy as i32, color);
        }
    }
}
