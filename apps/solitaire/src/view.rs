//! Solitaire board layout, hit-testing and rendering (Meridian-styled).

use alloc::format;
use alloc::vec::Vec;
use solitaire::{Card, Game, Loc, Suit};
use tinyos_app::gfx::{self, argb, rgb, with_alpha, Canvas, Rect};

use crate::glyphs::{self, Glyph};

pub const WIN_W: i32 = 920;
pub const WIN_H: i32 = 640;

pub const CARD_W: i32 = 110;
pub const CARD_H: i32 = 150;
const MARGIN: i32 = 15;
const PITCH: i32 = 125;
const TOP_Y: i32 = 48;
const TABLEAU_Y: i32 = 215;
pub const FAN_UP: i32 = 30;
pub const FAN_DOWN: i32 = 12;

const CARD_FACE: u32 = gfx::TX; // #e8ecf2
const INK_BLACK: u32 = rgb(0x1a, 0x1e, 0x26);
const INK_RED: u32 = rgb(0xb8, 0x4a, 0x4a); // HUE_RED deepened for light cards
const CARD_BACK: u32 = rgb(0x0e, 0x11, 0x17);

fn suit_small(s: Suit) -> &'static Glyph {
    match s {
        Suit::Hearts => &glyphs::HEART_SM,
        Suit::Diamonds => &glyphs::DIAMOND_SM,
        Suit::Spades => &glyphs::SPADE_SM,
        Suit::Clubs => &glyphs::CLUB_SM,
    }
}

fn suit_big(s: Suit) -> &'static Glyph {
    match s {
        Suit::Hearts => &glyphs::HEART_BIG,
        Suit::Diamonds => &glyphs::DIAMOND_BIG,
        Suit::Spades => &glyphs::SPADE_BIG,
        Suit::Clubs => &glyphs::CLUB_BIG,
    }
}

fn ink(s: Suit) -> u32 {
    if s.is_red() {
        INK_RED
    } else {
        INK_BLACK
    }
}

fn draw_glyph(c: &mut Canvas, x: i32, y: i32, g: &Glyph, color: u32) {
    c.draw_alpha_mask(x, y, g.data, g.w, g.h, color);
}

fn col_x(col: i32) -> i32 {
    MARGIN + col * PITCH
}

pub fn stock_rect() -> Rect {
    Rect::new(col_x(0), TOP_Y, CARD_W, CARD_H)
}

pub fn waste_rect() -> Rect {
    Rect::new(col_x(1), TOP_Y, CARD_W, CARD_H)
}

pub fn foundation_rect(i: usize) -> Rect {
    Rect::new(col_x(3 + i as i32), TOP_Y, CARD_W, CARD_H)
}

/// Fan offset of card `idx` from the top of tableau pile `pile`.
fn fan_offset(g: &Game, pile: usize, idx: usize) -> i32 {
    g.tableau[pile][..idx]
        .iter()
        .map(|c| if c.face_up { FAN_UP } else { FAN_DOWN })
        .sum()
}

pub fn tableau_card_rect(g: &Game, pile: usize, idx: usize) -> Rect {
    Rect::new(
        col_x(pile as i32),
        TABLEAU_Y + fan_offset(g, pile, idx),
        CARD_W,
        CARD_H,
    )
}

/// Where the next card dropped on tableau `pile` would land.
pub fn tableau_drop_rect(g: &Game, pile: usize) -> Rect {
    let len = g.tableau[pile].len();
    if len == 0 {
        Rect::new(col_x(pile as i32), TABLEAU_Y, CARD_W, CARD_H)
    } else {
        let mut r = tableau_card_rect(g, pile, len - 1);
        r.y += FAN_UP;
        r
    }
}

/// What the pointer is over, topmost first.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Stock,
    Waste,
    Foundation(usize),
    /// (pile, card index)
    Tableau(usize, usize),
}

pub fn hit_test(g: &Game, x: i32, y: i32) -> Option<Hit> {
    if stock_rect().contains(x, y) {
        return Some(Hit::Stock);
    }
    if waste_rect().contains(x, y) && !g.waste.is_empty() {
        return Some(Hit::Waste);
    }
    for i in 0..4 {
        if foundation_rect(i).contains(x, y) && !g.foundations[i].is_empty() {
            return Some(Hit::Foundation(i));
        }
    }
    for pile in 0..7 {
        for idx in (0..g.tableau[pile].len()).rev() {
            if tableau_card_rect(g, pile, idx).contains(x, y) {
                return Some(Hit::Tableau(pile, idx));
            }
        }
    }
    None
}

/// Small pill anchored at its bottom-right corner showing `n` in crisp digits.
fn draw_count_badge(c: &mut Canvas, right: i32, bottom: i32, n: usize) {
    let mut digits: Vec<&Glyph> = Vec::new();
    let mut v = n.max(0);
    loop {
        digits.push(glyphs::DIGITS[v % 10]);
        v /= 10;
        if v == 0 {
            break;
        }
    }
    digits.reverse();
    let tw: i32 = digits.iter().map(|g| g.w).sum();
    let th = digits.iter().map(|g| g.h).max().unwrap_or(0);
    let (pw, ph) = (tw + 14, th + 6);
    let pill = Rect::new(right - pw, bottom - ph, pw, ph);
    c.fill_rounded_rect(pill, ph / 2, argb(225, 0x0e, 0x11, 0x17));
    c.stroke_rounded_rect(pill, ph / 2, 1, gfx::STROKE2);
    let mut tx = pill.x + 7;
    for g in digits {
        draw_glyph(c, tx, pill.y + (ph - g.h) / 2, g, gfx::TX);
        tx += g.w;
    }
}

fn draw_empty_slot(c: &mut Canvas, r: Rect) {
    c.fill_rounded_rect(r, 8, gfx::CARD);
    c.stroke_rounded_rect(r, 8, 1, gfx::STROKE);
}

pub fn draw_card(c: &mut Canvas, x: i32, y: i32, card: Card) {
    let r = Rect::new(x, y, CARD_W, CARD_H);
    if !card.face_up {
        // Back: dark panel, teal border, diagonal lattice.
        c.fill_rounded_rect(r, 8, CARD_BACK);
        c.stroke_rounded_rect(r, 8, 1, with_alpha(gfx::ACC, 140));
        let weave = with_alpha(gfx::ACC, 36);
        for py in y + 8..y + CARD_H - 8 {
            for px in x + 8..x + CARD_W - 8 {
                let d = px - x + py - y;
                let a = px - x - (py - y) + CARD_H;
                if d % 14 < 2 || a % 14 < 2 {
                    c.put(px, py, weave);
                }
            }
        }
        return;
    }
    // Face: light card with a soft edge, rank + suit in the corner, big pip.
    c.fill_rounded_rect(r, 8, CARD_FACE);
    c.stroke_rounded_rect(r, 8, 1, argb(70, 0x0a, 0x0c, 0x10));
    let color = ink(card.suit);
    let rank = &glyphs::RANKS[card.rank as usize - 1];
    draw_glyph(c, x + 9, y + 8, rank, color);
    let sm = suit_small(card.suit);
    draw_glyph(c, x + CARD_W - sm.w - 9, y + 8, sm, color);
    let big = suit_big(card.suit);
    draw_glyph(
        c,
        x + (CARD_W - big.w) / 2,
        y + (CARD_H - big.h) / 2 + 10,
        big,
        color,
    );
}

/// A card being dragged, drawn last with a soft shadow.
fn draw_ghost(c: &mut Canvas, x: i32, y: i32, cards: &[Card]) {
    let total_h = CARD_H + (cards.len() as i32 - 1) * FAN_UP;
    c.fill_rounded_rect(Rect::new(x + 4, y + 6, CARD_W, total_h), 8, argb(90, 0, 0, 0));
    for (i, card) in cards.iter().enumerate() {
        draw_card(c, x, y + i as i32 * FAN_UP, *card);
    }
}

/// Cards currently lifted out of the model by an in-progress drag.
pub struct DragView<'a> {
    pub from: Loc,
    pub cards: &'a [Card],
    /// Ghost top-left.
    pub x: i32,
    pub y: i32,
}

pub fn draw_scene(c: &mut Canvas, g: &Game, drag: Option<&DragView>, status: &str) {
    // Meridian backdrop: near-black with a faint teal wash falling from the top.
    c.clear(gfx::BG);
    c.fill_gradient_v(
        Rect::new(0, 0, WIN_W, WIN_H / 2),
        with_alpha(gfx::ACC, 18),
        with_alpha(gfx::ACC, 0),
    );

    let hidden = |loc: Loc, n: usize| -> usize {
        match drag {
            Some(d) if d.from == loc => n,
            _ => 0,
        }
    };

    // Stock: stacked-edge depth cue + remaining-count badge; once empty, a
    // teal recycle arrow shows another pass through the waste is available.
    let sr = stock_rect();
    if g.stock.is_empty() {
        draw_empty_slot(c, sr);
        if !g.waste.is_empty() {
            let rg = &glyphs::RECYCLE;
            draw_glyph(
                c,
                sr.x + (CARD_W - rg.w) / 2,
                sr.y + (CARD_H - rg.h) / 2,
                rg,
                gfx::ACC,
            );
        }
    } else {
        // Edge slivers behind the top card hint at the pile's depth.
        let depth = ((g.stock.len() + 7) / 8).min(3) as i32;
        for d in (1..=depth).rev() {
            c.fill_rounded_rect(Rect::new(sr.x + 2 * d, sr.y + 2 * d, CARD_W, CARD_H), 8, CARD_BACK);
            c.stroke_rounded_rect(
                Rect::new(sr.x + 2 * d, sr.y + 2 * d, CARD_W, CARD_H),
                8,
                1,
                with_alpha(gfx::ACC, 70),
            );
        }
        draw_card(c, sr.x, sr.y, Card { rank: 1, suit: Suit::Spades, face_up: false });
        draw_count_badge(c, sr.x + CARD_W - 8, sr.y + CARD_H - 8, g.stock.len());
    }

    // Waste: top card (minus any dragged one).
    let wr = waste_rect();
    let wn = g.waste.len() - hidden(Loc::Waste, 1).min(g.waste.len());
    if wn == 0 {
        draw_empty_slot(c, wr);
    } else {
        draw_card(c, wr.x, wr.y, g.waste[wn - 1]);
    }

    // Foundations.
    for i in 0..4 {
        let fr = foundation_rect(i);
        let n = g.foundations[i].len() - hidden(Loc::Foundation(i), 1).min(g.foundations[i].len());
        if n == 0 {
            draw_empty_slot(c, fr);
            let a = &glyphs::RANKS[0];
            draw_glyph(
                c,
                fr.x + (CARD_W - a.w) / 2,
                fr.y + (CARD_H - a.h) / 2,
                a,
                with_alpha(gfx::TX3, 150),
            );
        } else {
            draw_card(c, fr.x, fr.y, g.foundations[i][n - 1]);
        }
    }

    // Tableau.
    for pile in 0..7 {
        let n = g.tableau[pile].len() - hidden(Loc::Tableau(pile), drag.map_or(0, |d| d.cards.len()));
        if n == 0 {
            draw_empty_slot(c, Rect::new(col_x(pile as i32), TABLEAU_Y, CARD_W, CARD_H));
        }
        for idx in 0..n {
            let r = tableau_card_rect(g, pile, idx);
            draw_card(c, r.x, r.y, g.tableau[pile][idx]);
        }
    }

    // Status line, bottom-left.
    c.draw_ui_text(MARGIN, WIN_H - 26, status, gfx::TX3);

    if let Some(d) = drag {
        draw_ghost(c, d.x, d.y, d.cards);
    }
}

/// Best drop target for a ghost whose top card covers `ghost`: candidate
/// piles ordered by overlap area (largest first).
pub fn drop_candidates(g: &Game, ghost: Rect, count: usize) -> Vec<Loc> {
    let mut cands: Vec<(i32, Loc)> = Vec::new();
    if count == 1 {
        for i in 0..4 {
            let ov = ghost.overlap(&foundation_rect(i));
            if ov > 0 {
                cands.push((ov, Loc::Foundation(i)));
            }
        }
    }
    for pile in 0..7 {
        let ov = ghost.overlap(&tableau_drop_rect(g, pile));
        if ov > 0 {
            cands.push((ov, Loc::Tableau(pile)));
        }
    }
    cands.sort_by_key(|(ov, _)| -ov);
    cands.into_iter().map(|(_, l)| l).collect()
}

pub fn status_text(moves: u32, g: &Game) -> alloc::string::String {
    format!("moves {}   stock {}   waste {}", moves, g.stock.len(), g.waste.len())
}
