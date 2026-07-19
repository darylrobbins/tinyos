//! Klondike solitaire rules engine. Pure logic, no I/O — host-testable.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Suit {
    Spades,
    Hearts,
    Diamonds,
    Clubs,
}

impl Suit {
    pub fn is_red(self) -> bool {
        matches!(self, Suit::Hearts | Suit::Diamonds)
    }
}

pub const SUITS: [Suit; 4] = [Suit::Spades, Suit::Hearts, Suit::Diamonds, Suit::Clubs];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Card {
    /// 1 = Ace .. 13 = King.
    pub rank: u8,
    pub suit: Suit,
    pub face_up: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loc {
    Stock,
    Waste,
    Foundation(usize),
    Tableau(usize),
}

pub struct Game {
    pub stock: Vec<Card>,
    pub waste: Vec<Card>,
    pub foundations: [Vec<Card>; 4],
    pub tableau: [Vec<Card>; 7],
    pub draw_count: usize,
}

fn xorshift64star(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

impl Game {
    pub fn new(seed: u64, draw_count: usize) -> Game {
        let mut deck: Vec<Card> = Vec::with_capacity(52);
        for suit in SUITS {
            for rank in 1..=13 {
                deck.push(Card { rank, suit, face_up: false });
            }
        }
        let mut rng = seed | 1; // xorshift must not start at 0
        for i in (1..deck.len()).rev() {
            let j = (xorshift64star(&mut rng) % (i as u64 + 1)) as usize;
            deck.swap(i, j);
        }
        let mut tableau: [Vec<Card>; 7] = Default::default();
        for (i, pile) in tableau.iter_mut().enumerate() {
            for j in 0..=i {
                let mut c = deck.pop().unwrap();
                c.face_up = j == i;
                pile.push(c);
            }
        }
        Game {
            stock: deck,
            waste: Vec::new(),
            foundations: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            tableau,
            draw_count,
        }
    }

    /// Stock -> waste (`draw_count` cards); recycles waste when stock is empty.
    pub fn draw(&mut self) {
        if self.stock.is_empty() {
            while let Some(mut c) = self.waste.pop() {
                c.face_up = false;
                self.stock.push(c);
            }
            return;
        }
        for _ in 0..self.draw_count {
            match self.stock.pop() {
                Some(mut c) => {
                    c.face_up = true;
                    self.waste.push(c);
                }
                None => break,
            }
        }
    }

    /// Is the suffix of tableau pile `pile` starting at `idx` a movable run?
    pub fn movable_run(&self, pile: usize, idx: usize) -> bool {
        let p = &self.tableau[pile];
        if idx >= p.len() {
            return false;
        }
        p[idx..].windows(2).all(|w| {
            w[0].face_up && w[0].rank == w[1].rank + 1 && w[0].suit.is_red() != w[1].suit.is_red()
        }) && p[idx].face_up
    }

    fn fits_tableau(&self, pile: usize, c: Card) -> bool {
        match self.tableau[pile].last() {
            None => c.rank == 13,
            Some(t) => t.face_up && t.rank == c.rank + 1 && t.suit.is_red() != c.suit.is_red(),
        }
    }

    fn fits_foundation(&self, f: usize, c: Card) -> bool {
        match self.foundations[f].last() {
            None => c.rank == 1,
            Some(t) => t.suit == c.suit && c.rank == t.rank + 1,
        }
    }

    /// Validate and apply a move of `count` cards; flips newly exposed cards.
    pub fn try_move(&mut self, from: Loc, count: usize, to: Loc) -> bool {
        if count == 0 || from == to {
            return false;
        }
        // Everything except a tableau run moves a single top card.
        let moving: Vec<Card> = match from {
            Loc::Stock => return false,
            Loc::Waste => match (count, self.waste.last()) {
                (1, Some(&c)) => alloc::vec![c],
                _ => return false,
            },
            Loc::Foundation(f) => match (count, self.foundations[f].last()) {
                (1, Some(&c)) => alloc::vec![c],
                _ => return false,
            },
            Loc::Tableau(p) => {
                let len = self.tableau[p].len();
                if count > len || !self.movable_run(p, len - count) {
                    return false;
                }
                self.tableau[p][len - count..].to_vec()
            }
        };
        let ok = match to {
            Loc::Stock | Loc::Waste => false,
            Loc::Foundation(f) => moving.len() == 1 && self.fits_foundation(f, moving[0]),
            Loc::Tableau(p) => self.fits_tableau(p, moving[0]),
        };
        if !ok {
            return false;
        }
        match from {
            Loc::Waste => {
                self.waste.pop();
            }
            Loc::Foundation(f) => {
                self.foundations[f].pop();
            }
            Loc::Tableau(p) => {
                let len = self.tableau[p].len();
                self.tableau[p].truncate(len - count);
                if let Some(top) = self.tableau[p].last_mut() {
                    top.face_up = true;
                }
            }
            Loc::Stock => unreachable!(),
        }
        match to {
            Loc::Foundation(f) => self.foundations[f].extend(moving),
            Loc::Tableau(p) => self.tableau[p].extend(moving),
            _ => unreachable!(),
        }
        true
    }

    /// Move the top card at `from` to a fitting foundation, if any.
    pub fn auto_to_foundation(&mut self, from: Loc) -> bool {
        for f in 0..4 {
            if self.try_move(from, 1, Loc::Foundation(f)) {
                return true;
            }
        }
        false
    }

    pub fn is_won(&self) -> bool {
        self.foundations.iter().all(|f| f.len() == 13)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(rank: u8, suit: Suit) -> Card {
        Card { rank, suit, face_up: true }
    }

    fn empty_game() -> Game {
        Game {
            stock: Vec::new(),
            waste: Vec::new(),
            foundations: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            tableau: Default::default(),
            draw_count: 1,
        }
    }

    #[test]
    fn deal_has_correct_shape() {
        let g = Game::new(42, 1);
        for (i, pile) in g.tableau.iter().enumerate() {
            assert_eq!(pile.len(), i + 1, "tableau pile {i}");
            for (j, c) in pile.iter().enumerate() {
                assert_eq!(c.face_up, j == i, "pile {i} card {j} face_up");
            }
        }
        assert_eq!(g.stock.len(), 24);
        assert!(g.stock.iter().all(|c| !c.face_up));
        assert!(g.waste.is_empty());
        assert!(g.foundations.iter().all(|f| f.is_empty()));
    }

    #[test]
    fn deal_uses_all_52_unique_cards() {
        let g = Game::new(7, 1);
        let mut seen = std::collections::HashSet::new();
        let all = g
            .stock
            .iter()
            .chain(g.tableau.iter().flatten())
            .collect::<Vec<_>>();
        assert_eq!(all.len(), 52);
        for c in all {
            assert!((1..=13).contains(&c.rank));
            assert!(seen.insert((c.rank, c.suit as u8)), "duplicate {c:?}");
        }
    }

    #[test]
    fn same_seed_same_deal_different_seed_differs() {
        let a = Game::new(1, 1);
        let b = Game::new(1, 1);
        let c = Game::new(2, 1);
        assert_eq!(a.stock, b.stock);
        assert!(a.stock != c.stock || a.tableau != c.tableau);
    }

    #[test]
    fn draw_one_moves_top_of_stock_to_waste_face_up() {
        let mut g = Game::new(3, 1);
        let top = *g.stock.last().unwrap();
        g.draw();
        assert_eq!(g.stock.len(), 23);
        assert_eq!(g.waste.len(), 1);
        let w = *g.waste.last().unwrap();
        assert!(w.face_up);
        assert_eq!((w.rank, w.suit), (top.rank, top.suit));
    }

    #[test]
    fn draw_three_moves_up_to_three() {
        let mut g = Game::new(3, 3);
        g.draw();
        assert_eq!(g.waste.len(), 3);
        assert_eq!(g.stock.len(), 21);
        assert!(g.waste.iter().all(|c| c.face_up));
    }

    #[test]
    fn draw_recycles_waste_when_stock_empty() {
        let mut g = Game::new(3, 1);
        for _ in 0..24 {
            g.draw();
        }
        assert!(g.stock.is_empty());
        assert_eq!(g.waste.len(), 24);
        let first_waste = g.waste[0];
        g.draw(); // recycle, then nothing drawn this call
        assert_eq!(g.stock.len(), 24);
        assert!(g.waste.is_empty());
        assert!(g.stock.iter().all(|c| !c.face_up));
        // Waste is reversed back into the stock: first-drawn card returns to the top.
        let recycled_top = *g.stock.last().unwrap();
        assert_eq!((recycled_top.rank, recycled_top.suit), (first_waste.rank, first_waste.suit));
    }

    #[test]
    fn draw_on_both_empty_is_noop() {
        let mut g = empty_game();
        g.draw();
        assert!(g.stock.is_empty() && g.waste.is_empty());
    }

    #[test]
    fn movable_run_requires_face_up_descending_alternating() {
        let mut g = empty_game();
        g.tableau[0] = vec![
            Card { rank: 9, suit: Suit::Clubs, face_up: false },
            card(5, Suit::Spades),
            card(4, Suit::Hearts),
            card(3, Suit::Clubs),
        ];
        assert!(g.movable_run(0, 1)); // 5S 4H 3C
        assert!(g.movable_run(0, 2)); // 4H 3C
        assert!(g.movable_run(0, 3)); // single card
        assert!(!g.movable_run(0, 0)); // face-down
        assert!(!g.movable_run(0, 4)); // out of range
        // Break alternation: 5S 4C is same color.
        g.tableau[1] = vec![card(5, Suit::Spades), card(4, Suit::Clubs)];
        assert!(!g.movable_run(1, 0));
        assert!(g.movable_run(1, 1));
    }

    #[test]
    fn tableau_accepts_descending_alternating_color() {
        let mut g = empty_game();
        g.tableau[0] = vec![card(7, Suit::Hearts)];
        g.tableau[1] = vec![card(6, Suit::Spades)];
        assert!(g.try_move(Loc::Tableau(1), 1, Loc::Tableau(0)));
        assert_eq!(g.tableau[0].len(), 2);
        assert!(g.tableau[1].is_empty());
    }

    #[test]
    fn tableau_rejects_same_color_or_wrong_rank() {
        let mut g = empty_game();
        g.tableau[0] = vec![card(7, Suit::Hearts)];
        g.tableau[1] = vec![card(6, Suit::Diamonds)]; // same color
        g.tableau[2] = vec![card(5, Suit::Spades)]; // wrong rank
        assert!(!g.try_move(Loc::Tableau(1), 1, Loc::Tableau(0)));
        assert!(!g.try_move(Loc::Tableau(2), 1, Loc::Tableau(0)));
    }

    #[test]
    fn only_king_moves_to_empty_tableau() {
        let mut g = empty_game();
        g.tableau[0] = vec![card(13, Suit::Spades)];
        g.tableau[1] = vec![card(12, Suit::Hearts)];
        assert!(!g.try_move(Loc::Tableau(1), 1, Loc::Tableau(2)));
        assert!(g.try_move(Loc::Tableau(0), 1, Loc::Tableau(2)));
    }

    #[test]
    fn multi_card_run_moves_together() {
        let mut g = empty_game();
        g.tableau[0] = vec![card(9, Suit::Hearts), card(8, Suit::Spades), card(7, Suit::Hearts)];
        g.tableau[1] = vec![card(10, Suit::Clubs)];
        assert!(g.try_move(Loc::Tableau(0), 3, Loc::Tableau(1)));
        assert_eq!(g.tableau[1].len(), 4);
        assert!(g.tableau[0].is_empty());
        assert_eq!(g.tableau[1][1].rank, 9);
        assert_eq!(g.tableau[1][3].rank, 7);
    }

    #[test]
    fn moving_run_that_breaks_rules_is_rejected() {
        let mut g = empty_game();
        // 8S over face-down card; try to move both.
        g.tableau[0] = vec![Card { rank: 3, suit: Suit::Clubs, face_up: false }, card(8, Suit::Spades)];
        g.tableau[1] = vec![card(9, Suit::Hearts)];
        assert!(!g.try_move(Loc::Tableau(0), 2, Loc::Tableau(1)));
        assert!(g.try_move(Loc::Tableau(0), 1, Loc::Tableau(1)));
    }

    #[test]
    fn exposed_card_flips_after_move() {
        let mut g = empty_game();
        g.tableau[0] = vec![Card { rank: 9, suit: Suit::Clubs, face_up: false }, card(5, Suit::Spades)];
        g.tableau[1] = vec![card(6, Suit::Hearts)];
        assert!(g.try_move(Loc::Tableau(0), 1, Loc::Tableau(1)));
        assert!(g.tableau[0][0].face_up, "newly exposed card must flip");
    }

    #[test]
    fn foundation_accepts_ace_then_same_suit_ascending() {
        let mut g = empty_game();
        g.waste = vec![card(1, Suit::Hearts)];
        assert!(g.try_move(Loc::Waste, 1, Loc::Foundation(0)));
        g.waste = vec![card(2, Suit::Hearts)];
        assert!(g.try_move(Loc::Waste, 1, Loc::Foundation(0)));
        assert_eq!(g.foundations[0].len(), 2);
        // Wrong suit and wrong rank rejected.
        g.waste = vec![card(3, Suit::Spades)];
        assert!(!g.try_move(Loc::Waste, 1, Loc::Foundation(0)));
        g.waste = vec![card(4, Suit::Hearts)];
        assert!(!g.try_move(Loc::Waste, 1, Loc::Foundation(0)));
        // Non-ace on empty foundation rejected.
        assert!(!g.try_move(Loc::Waste, 1, Loc::Foundation(1)));
    }

    #[test]
    fn foundation_only_accepts_single_cards() {
        let mut g = empty_game();
        g.tableau[0] = vec![card(2, Suit::Hearts), card(1, Suit::Spades)];
        assert!(!g.try_move(Loc::Tableau(0), 2, Loc::Foundation(0)));
    }

    #[test]
    fn waste_to_tableau_moves_top_card() {
        let mut g = empty_game();
        g.waste = vec![card(9, Suit::Clubs), card(6, Suit::Spades)];
        g.tableau[0] = vec![card(7, Suit::Hearts)];
        assert!(g.try_move(Loc::Waste, 1, Loc::Tableau(0)));
        assert_eq!(g.waste.len(), 1);
        assert_eq!(g.tableau[0].last().unwrap().rank, 6);
    }

    #[test]
    fn foundation_card_can_return_to_tableau() {
        let mut g = empty_game();
        g.foundations[0] = vec![card(1, Suit::Hearts), card(2, Suit::Hearts)];
        g.tableau[0] = vec![card(3, Suit::Spades)];
        assert!(g.try_move(Loc::Foundation(0), 1, Loc::Tableau(0)));
        assert_eq!(g.foundations[0].len(), 1);
        assert_eq!(g.tableau[0].len(), 2);
    }

    #[test]
    fn illegal_sources_rejected() {
        let mut g = empty_game();
        assert!(!g.try_move(Loc::Stock, 1, Loc::Tableau(0)));
        assert!(!g.try_move(Loc::Waste, 1, Loc::Tableau(0))); // empty waste
        assert!(!g.try_move(Loc::Tableau(0), 1, Loc::Tableau(1))); // empty pile
        assert!(!g.try_move(Loc::Tableau(0), 1, Loc::Tableau(0))); // self-move
    }

    #[test]
    fn auto_to_foundation_finds_slot() {
        let mut g = empty_game();
        g.foundations[2] = vec![card(1, Suit::Diamonds)];
        g.waste = vec![card(2, Suit::Diamonds)];
        assert!(g.auto_to_foundation(Loc::Waste));
        assert_eq!(g.foundations[2].len(), 2);
        // Ace goes to the first empty foundation.
        g.tableau[0] = vec![card(1, Suit::Clubs)];
        assert!(g.auto_to_foundation(Loc::Tableau(0)));
        assert!(g.foundations.iter().any(|f| f.len() == 1 && f[0].suit == Suit::Clubs));
        // No fit -> false.
        g.waste = vec![card(9, Suit::Spades)];
        assert!(!g.auto_to_foundation(Loc::Waste));
    }

    #[test]
    fn win_detection() {
        let mut g = empty_game();
        assert!(!g.is_won());
        for (i, suit) in SUITS.iter().enumerate() {
            g.foundations[i] = (1..=13).map(|r| card(r, *suit)).collect();
        }
        assert!(g.is_won());
        g.foundations[3].pop();
        assert!(!g.is_won());
    }
}
