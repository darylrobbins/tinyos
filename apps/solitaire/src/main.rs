//! Klondike solitaire for the Meridian desktop.

#![no_std]
#![no_main]

extern crate alloc;

mod glyphs;
mod view;

use alloc::vec::Vec;
use solitaire::{Card, Game, Loc};
use tinyos_app::app;
use tinyos_app::entry::Env;
use tinyos_app::gfx::{self, Canvas, Rect};
use tinyos_app::ui::{self, UiInput};
use tinyos_app::wait::uptime_us;
use tinyos_app::window::{Event, Window};
use view::{DragView, Hit};

const DOUBLE_CLICK_US: u64 = 400_000;

struct Drag {
    from: Loc,
    cards: Vec<Card>,
    /// Pointer offset from the ghost's top-left at grab time.
    grab: (i32, i32),
    pos: (i32, i32),
}

/// The classic post-win card cascade, drawn over a persisting frame.
struct WinAnim {
    stacks: [Vec<Card>; 4],
    fly: Option<(f32, f32, f32, f32, Card)>, // x, y, vx, vy
    next_f: usize,
    rng: u64,
    done: bool,
}

impl WinAnim {
    fn new(g: &Game, seed: u64) -> WinAnim {
        WinAnim {
            stacks: g.foundations.clone(),
            fly: None,
            next_f: 0,
            rng: seed | 1,
            done: false,
        }
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// One physics step; draws the flying card onto `c` (no clearing, so the
    /// card leaves the classic trail). Returns false once every card flew.
    fn step(&mut self, c: &mut Canvas) {
        if self.fly.is_none() {
            for _ in 0..4 {
                let f = self.next_f;
                self.next_f = (self.next_f + 1) % 4;
                if let Some(card) = self.stacks[f].pop() {
                    let r = view::foundation_rect(f);
                    let vx = 1.5 + (self.rand() % 300) as f32 / 100.0;
                    let dir = if self.rand() & 1 == 0 { 1.0 } else { -1.0 };
                    let vy = -1.0 - (self.rand() % 400) as f32 / 100.0;
                    self.fly = Some((r.x as f32, r.y as f32, vx * dir, vy, card));
                    break;
                }
            }
            if self.fly.is_none() {
                self.done = true;
                return;
            }
        }
        if let Some((mut x, mut y, vx, mut vy, card)) = self.fly.take() {
            for _ in 0..2 {
                x += vx;
                vy += 0.35;
                y += vy;
                let floor = (view::WIN_H - view::CARD_H) as f32;
                if y > floor {
                    y = floor;
                    vy = -vy * 0.75;
                }
            }
            view::draw_card(c, x as i32, y as i32, card);
            if x < -(view::CARD_W as f32) || x > view::WIN_W as f32 {
                // Off-screen: next card launches on the next step.
            } else {
                self.fly = Some((x, y, vx, vy, card));
            }
        }
    }
}

fn main(env: Env) -> i32 {
    let mut win = match Window::open(env.shell, view::WIN_W as u32, view::WIN_H as u32, "solitaire")
    {
        Ok(w) => w,
        Err(_) => return 1,
    };
    let (w, h) = (win.width as i32, win.height as i32);
    // Render into a back buffer and present complete frames only; drawing
    // straight into the shared surface flickers (the shell may blit mid-draw).
    let mut back = alloc::vec![0u32; (w * h) as usize];

    let mut game = Game::new(uptime_us(), 1);
    let mut moves: u32 = 0;
    let mut drag: Option<Drag> = None;
    let mut anim: Option<WinAnim> = None;
    // Endgame auto-play: one card per frame flies to the foundations.
    let mut finishing = false;
    let mut ui_in = UiInput::default();
    let mut last_press: Option<(Hit, u64)> = None;
    let mut events = Vec::new();

    loop {
        ui_in.begin_frame();
        events.clear();
        win.poll_events(&mut events);
        let mut new_game = false;
        let mut toggle_draw = false;
        for ev in &events {
            ui_in.feed(ev);
            match *ev {
                Event::CloseRequested => return 0,
                Event::Char('n') | Event::Char('N') => new_game = true,
                Event::Char('f') | Event::Char('F') if game.can_autofinish() => finishing = true,
                Event::Button { down: true, x, y } if anim.is_none() && !finishing => {
                    on_press(&mut game, &mut drag, &mut moves, &mut last_press, x, y);
                }
                Event::Button { down: false, x, y } if anim.is_none() && !finishing => {
                    on_release(&mut game, &mut drag, &mut moves, x, y);
                }
                Event::PointerMoved { x, y } => {
                    if let Some(d) = drag.as_mut() {
                        d.pos = (x, y);
                    }
                }
                _ => {}
            }
        }

        if finishing {
            if game.autofinish_step() {
                moves += 1;
            } else {
                finishing = false;
            }
        }

        if anim.is_none() && game.is_won() {
            anim = Some(WinAnim::new(&game, uptime_us()));
            finishing = false;
        }

        // Draw. During the win cascade the frame persists (card trails); in
        // play the scene redraws fully every frame.
        let mut c = Canvas::new(&mut back, w, h);
        match anim.as_mut() {
            Some(a) => {
                a.step(&mut c);
                let banner = &glyphs::BANNER_WON;
                c.draw_alpha_mask(
                    (w - banner.w) / 2,
                    h / 2 - 60 - banner.h / 2,
                    banner.data,
                    banner.w,
                    banner.h,
                    gfx::ACC,
                );
                if a.done {
                    let hint = "Press N or New Game to deal again";
                    let (hw, _) = gfx::measure_ui_text(hint);
                    c.draw_ui_text((w - hw) / 2, h / 2 + 10, hint, gfx::TX2);
                }
            }
            None => {
                let dv = drag.as_ref().map(|d| DragView {
                    from: d.from,
                    cards: &d.cards,
                    x: d.pos.0 - d.grab.0,
                    y: d.pos.1 - d.grab.1,
                });
                let status = view::status_text(moves, &game);
                view::draw_scene(&mut c, &game, dv.as_ref(), &status);
            }
        }
        // Toolbar rides on top in both modes.
        if ui::button(&mut c, &ui_in, Rect::new(15, 8, 96, 26), "New Game") {
            new_game = true;
        }
        let draw_label = if game.draw_count == 1 { "Draw 1" } else { "Draw 3" };
        if ui::button(&mut c, &ui_in, Rect::new(123, 8, 80, 26), draw_label) {
            toggle_draw = true;
        }
        // Endgame shortcut: only offered once nothing is hidden anymore.
        if !finishing && anim.is_none() && game.can_autofinish() {
            if ui::button(&mut c, &ui_in, Rect::new(215, 8, 76, 26), "Finish") {
                finishing = true;
            }
        }
        win.present_from(&back);

        if new_game || toggle_draw {
            let dc = match (toggle_draw, game.draw_count) {
                (true, 1) => 3,
                (true, _) => 1,
                (false, dc) => dc,
            };
            game = Game::new(uptime_us(), dc);
            moves = 0;
            drag = None;
            anim = None;
            finishing = false;
            last_press = None;
            // Immediate-mode buttons fire after this frame already drew the
            // old state; redraw right away instead of sleeping on it.
            continue;
        }

        // ~30fps while something is in motion; otherwise sleep until input.
        let busy = drag.is_some() || finishing || anim.as_ref().is_some_and(|a| !a.done);
        let dt = if busy { 33_000 } else { 1_000_000 };
        win.wait(uptime_us() + dt);
    }
}

fn on_press(
    game: &mut Game,
    drag: &mut Option<Drag>,
    moves: &mut u32,
    last_press: &mut Option<(Hit, u64)>,
    x: i32,
    y: i32,
) {
    let Some(hit) = view::hit_test(game, x, y) else {
        *last_press = None;
        return;
    };
    let now = uptime_us();
    let double = matches!(*last_press, Some((h, t)) if h == hit && now - t < DOUBLE_CLICK_US);
    *last_press = Some((hit, now));

    match hit {
        Hit::Stock => {
            game.draw();
            *moves += 1;
        }
        Hit::Waste => {
            if double && game.auto_to_foundation(Loc::Waste) {
                *moves += 1;
                return;
            }
            let card = *game.waste.last().unwrap();
            let r = view::waste_rect();
            *drag = Some(Drag {
                from: Loc::Waste,
                cards: alloc::vec![card],
                grab: (x - r.x, y - r.y),
                pos: (x, y),
            });
        }
        Hit::Foundation(f) => {
            let card = *game.foundations[f].last().unwrap();
            let r = view::foundation_rect(f);
            *drag = Some(Drag {
                from: Loc::Foundation(f),
                cards: alloc::vec![card],
                grab: (x - r.x, y - r.y),
                pos: (x, y),
            });
        }
        Hit::Tableau(pile, idx) => {
            let len = game.tableau[pile].len();
            if double && idx == len - 1 && game.auto_to_foundation(Loc::Tableau(pile)) {
                *moves += 1;
                return;
            }
            if !game.movable_run(pile, idx) {
                return;
            }
            let r = view::tableau_card_rect(game, pile, idx);
            *drag = Some(Drag {
                from: Loc::Tableau(pile),
                cards: game.tableau[pile][idx..].to_vec(),
                grab: (x - r.x, y - r.y),
                pos: (x, y),
            });
        }
    }
}

fn on_release(game: &mut Game, drag: &mut Option<Drag>, moves: &mut u32, x: i32, y: i32) {
    let Some(d) = drag.take() else {
        return;
    };
    let ghost = Rect::new(x - d.grab.0, y - d.grab.1, view::CARD_W, view::CARD_H);
    for to in view::drop_candidates(game, ghost, d.cards.len()) {
        if game.try_move(d.from, d.cards.len(), to) {
            *moves += 1;
            return;
        }
    }
    // No legal target: the model was never mutated, so the cards snap back.
}

app!(main);
