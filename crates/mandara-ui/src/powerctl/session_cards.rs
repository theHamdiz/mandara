//! The session menu's cards style (`session_menu.style = "cards"`): every
//! entry is its own floating card, centred on the screen in a row that
//! wraps onto more rows (or a column that wraps into more columns,
//! `session_menu.cards_layout`), with a hint line under them.
//!
//! This module is the pure part: where each card goes on a screen of any
//! size, in both reading directions, and where the arrow keys move the
//! selection. It is plain arithmetic over the entry count (no allocation
//! per frame) so the view can call it every frame and the tests can check
//! every size. The view draws the cards ([`super::session_menu`]).

use mandara_config::power::SessionCardsLayout;

use crate::launcher_place::Rect;

/// A card's size and the gap between cards (logical px, before fitting).
pub const CARD_W_PX: f32 = 136.0;
pub const CARD_H_PX: f32 = 140.0;
pub const CARD_GAP_PX: f32 = 18.0;
/// Free room kept around the cards at the screen's edges, at most this
/// share of the screen on small outputs.
const SCREEN_MARGIN_PX: f32 = 48.0;
const SCREEN_MARGIN_SHARE: f32 = 0.08;
/// The hint line under the cards and its distance from them.
pub const HINT_H_PX: f32 = 32.0;
pub const HINT_GAP_PX: f32 = 22.0;
/// The smallest a card is drawn when many entries must share a small
/// screen (a share of its full size); below it they simply overflow less
/// gracefully rather than become unreadable.
const MIN_FIT: f32 = 0.35;

/// A move of the selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
    /// Tab: the next entry in order (wrapping).
    Next,
    /// Shift+Tab: the previous one.
    Prev,
    First,
    Last,
}

impl Nav {
    /// The move for a key name (arrows, the vim keys, Tab, Home and End).
    pub fn from_key(key: &str, shift: bool) -> Option<Self> {
        Some(match key {
            "left" | "h" => Nav::Left,
            "right" | "l" => Nav::Right,
            "up" | "k" => Nav::Up,
            "down" | "j" => Nav::Down,
            "tab" if shift => Nav::Prev,
            "tab" => Nav::Next,
            "home" => Nav::First,
            "end" => Nav::Last,
            _ => return None,
        })
    }
}

/// Where the cards go: `lines` rows (or columns) of up to `per_line`
/// cards, each line centred, the whole block centred on the screen with
/// the hint under it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CardsGrid {
    layout: SessionCardsLayout,
    n: usize,
    lines: usize,
    per_line: usize,
    /// How much the cards were shrunk to fit (1: full size).
    pub fit: f32,
    /// One card's size and the gap between cards, after fitting.
    pub card: (f32, f32),
    pub gap: f32,
    /// The cards' bounding box.
    pub block: Rect,
    /// The hint line under them (as wide as the screen's free room).
    pub hint: Rect,
}

impl CardsGrid {
    /// The cards of `n` entries (at least one) laid out as `layout` on a
    /// `screen`-sized output (logical px), centred on all of it.
    pub fn new(layout: SessionCardsLayout, n: usize, screen: (f32, f32)) -> Self {
        Self::within(layout, n, Rect::new(0.0, 0.0, screen.0, screen.1))
    }

    /// [`Self::new`] centred in `area`: the screen less the bar's band and
    /// a bottom dock, so the cards never sit under either.
    pub fn within(layout: SessionCardsLayout, n: usize, area: Rect) -> Self {
        let n = n.max(1);
        let (sw, sh) = (area.w.max(1.0), area.h.max(1.0));
        let (mx, my) = (SCREEN_MARGIN_PX.min(sw * SCREEN_MARGIN_SHARE), SCREEN_MARGIN_PX.min(sh * SCREEN_MARGIN_SHARE));
        let avail_w = (sw - 2.0 * mx).max(1.0);
        let avail_h = (sh - 2.0 * my - HINT_H_PX - HINT_GAP_PX).max(1.0);
        let (cw, ch, gap) = (CARD_W_PX, CARD_H_PX, CARD_GAP_PX);
        // How many fit along a line at full size, then as few lines as
        // that allows, balanced (nine in rows of seven are five and four).
        let along = |avail: f32, size: f32| (((avail + gap) / (size + gap)).floor() as usize).max(1);
        let max_line = match layout {
            SessionCardsLayout::Row => along(avail_w, cw),
            SessionCardsLayout::Column => along(avail_h, ch),
        };
        let lines = n.div_ceil(max_line);
        let per_line = n.div_ceil(lines);
        let span = |count: usize, size: f32| count as f32 * size + count.saturating_sub(1) as f32 * gap;
        let (w, h) = match layout {
            SessionCardsLayout::Row => (span(per_line, cw), span(lines, ch)),
            SessionCardsLayout::Column => (span(lines, cw), span(per_line, ch)),
        };
        // Still too big (many entries on a small screen): shrink them all.
        let fit = (avail_w / w).min(avail_h / h).clamp(MIN_FIT, 1.0);
        let (w, h) = (w * fit, h * fit);
        let total_h = h + HINT_GAP_PX + HINT_H_PX;
        let block = Rect::new(area.x + (sw - w) / 2.0, area.y + (sh - total_h) / 2.0, w, h);
        let hint = Rect::new(area.x + mx, block.bottom() + HINT_GAP_PX, avail_w, HINT_H_PX);
        Self { layout, n, lines, per_line, fit, card: (cw * fit, ch * fit), gap: gap * fit, block, hint }
    }

    /// The number of cards.
    pub fn len(&self) -> usize {
        self.n
    }

    /// Never empty (one card at least).
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Rows (row layout) or columns (column layout).
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// `(line, place in the line)` of card `i`.
    fn line_of(&self, i: usize) -> (usize, usize) {
        (i / self.per_line, i % self.per_line)
    }

    /// Cards in `line` (the last may be short).
    fn line_len(&self, line: usize) -> usize {
        self.per_line.min(self.n.saturating_sub(line * self.per_line))
    }

    /// Where card `i` sits; right to left, rows run from the right and
    /// columns from the right edge.
    pub fn rect(&self, i: usize, rtl: bool) -> Rect {
        let i = i.min(self.n - 1);
        let (line, k) = self.line_of(i);
        let len = self.line_len(line) as f32;
        let (cw, ch, gap, b) = (self.card.0, self.card.1, self.gap, self.block);
        let (x, y) = match self.layout {
            SessionCardsLayout::Row => {
                let row_w = len * cw + (len - 1.0) * gap;
                (b.x + (b.w - row_w) / 2.0 + k as f32 * (cw + gap), b.y + line as f32 * (ch + gap))
            }
            SessionCardsLayout::Column => {
                let col_h = len * ch + (len - 1.0) * gap;
                (b.x + line as f32 * (cw + gap), b.y + (b.h - col_h) / 2.0 + k as f32 * (ch + gap))
            }
        };
        let x = if rtl { b.x + b.w - (x - b.x) - cw } else { x };
        Rect::new(x, y, cw, ch)
    }

    /// The card under `(x, y)`, if any.
    pub fn hit(&self, x: f32, y: f32, rtl: bool) -> Option<usize> {
        (0..self.n).find(|&i| {
            let r = self.rect(i, rtl);
            x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
        })
    }

    /// The card the selection moves to from `from` on `nav`: along a line
    /// the arrows step in the direction they point and wrap round; across
    /// lines they go to the nearest card in the next row (or column), and
    /// stay put at the edge.
    pub fn step(&self, from: usize, nav: Nav, rtl: bool) -> usize {
        let n = self.n;
        let from = from.min(n - 1);
        let next = (from + 1) % n;
        let prev = (from + n - 1) % n;
        // Right moves forward in reading order left to right, back right to left.
        let (rightward, leftward) = if rtl { (prev, next) } else { (next, prev) };
        let (line, _) = self.line_of(from);
        let across = |forward: bool| -> usize {
            let target = if forward { line + 1 } else { line.wrapping_sub(1) };
            if target >= self.lines {
                return from;
            }
            self.nearest_in(target, from, rtl)
        };
        match (self.layout, nav) {
            (_, Nav::Next) => next,
            (_, Nav::Prev) => prev,
            (_, Nav::First) => 0,
            (_, Nav::Last) => n - 1,
            (SessionCardsLayout::Row, Nav::Right) => rightward,
            (SessionCardsLayout::Row, Nav::Left) => leftward,
            (SessionCardsLayout::Row, Nav::Down) => across(true),
            (SessionCardsLayout::Row, Nav::Up) => across(false),
            (SessionCardsLayout::Column, Nav::Down) => next,
            (SessionCardsLayout::Column, Nav::Up) => prev,
            // Columns run from the reading side: right to left, the next
            // column is to the left.
            (SessionCardsLayout::Column, Nav::Right) => across(!rtl),
            (SessionCardsLayout::Column, Nav::Left) => across(rtl),
        }
    }

    /// The card of `line` closest to card `from` along the line's axis.
    fn nearest_in(&self, line: usize, from: usize, rtl: bool) -> usize {
        let centre = |i: usize| {
            let r = self.rect(i, rtl);
            match self.layout {
                SessionCardsLayout::Row => r.x + r.w / 2.0,
                SessionCardsLayout::Column => r.y + r.h / 2.0,
            }
        };
        let at = centre(from);
        let first = line * self.per_line;
        (first..first + self.line_len(line)).min_by(|&a, &b| (centre(a) - at).abs().total_cmp(&(centre(b) - at).abs())).unwrap_or(from)
    }
}

/// When card `i` starts its entrance, seconds after the menu first draws:
/// one after another, `stagger_s` apart (none with reduced motion), at
/// most `max_s` for the last of many.
pub fn entrance_delay(i: usize, stagger_s: f32, max_s: f32, reduce: bool) -> f32 {
    if reduce { 0.0 } else { (i as f32 * stagger_s).min(max_s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: [(f32, f32); 6] = [(2560.0, 1440.0), (1920.0, 1080.0), (1536.0, 864.0), (1280.0, 720.0), (1080.0, 1920.0), (800.0, 600.0)];
    const LAYOUTS: [SessionCardsLayout; 2] = [SessionCardsLayout::Row, SessionCardsLayout::Column];

    fn inside(r: Rect, screen: (f32, f32)) -> bool {
        r.x >= -0.01 && r.y >= -0.01 && r.right() <= screen.0 + 0.01 && r.bottom() <= screen.1 + 0.01
    }

    #[test]
    fn five_cards_sit_centred_in_one_row() {
        let screen = (2560.0, 1440.0);
        let g = CardsGrid::new(SessionCardsLayout::Row, 5, screen);
        assert_eq!((g.lines(), g.fit), (1, 1.0));
        assert!((g.block.x + g.block.w / 2.0 - screen.0 / 2.0).abs() < 0.01, "centred across: {:?}", g.block);
        let middle = (g.block.y + g.hint.bottom()) / 2.0;
        assert!((middle - screen.1 / 2.0).abs() < 0.01, "cards and hint centred down: {middle}");
        for i in 0..4 {
            let (a, b) = (g.rect(i, false), g.rect(i + 1, false));
            assert_eq!(a.y, b.y, "one row");
            assert!((b.x - a.right() - CARD_GAP_PX).abs() < 0.01, "separate cards with a gap");
        }
        assert!(g.hint.y >= g.block.bottom() + HINT_GAP_PX - 0.01, "the hint under the cards");
    }

    #[test]
    fn many_cards_wrap_into_balanced_centred_lines() {
        // 9 on a 1280-wide output: 7 fit in a row, so two rows of 5 and 4.
        let screen = (1280.0, 720.0);
        let g = CardsGrid::new(SessionCardsLayout::Row, 9, screen);
        assert_eq!(g.lines(), 2);
        let (first, last) = (g.rect(0, false), g.rect(8, false));
        assert!(last.y > first.y);
        assert_eq!(g.rect(4, false).y, first.y, "five in the first row");
        assert_eq!(g.rect(5, false).y, last.y, "four in the second");
        // The short row is centred under the full one.
        let row_mid = (g.rect(5, false).x + g.rect(8, false).right()) / 2.0;
        assert!((row_mid - screen.0 / 2.0).abs() < 0.01, "{row_mid}");
        // A column of 9 is too tall for 720 (three cards fit): three
        // columns of three, side by side.
        let c = CardsGrid::new(SessionCardsLayout::Column, 9, screen);
        assert_eq!(c.lines(), 3);
        assert_eq!(c.rect(0, false).x, c.rect(1, false).x, "down first");
        assert!(c.rect(8, false).x > c.rect(0, false).x);
    }

    #[test]
    fn every_card_stays_on_screen_at_every_size_and_count() {
        for screen in SIZES {
            for layout in LAYOUTS {
                for n in 1..=24 {
                    let g = CardsGrid::new(layout, n, screen);
                    assert!(inside(g.block, screen) && inside(g.hint, screen), "{layout:?} {n} on {screen:?}: {:?} {:?}", g.block, g.hint);
                    for rtl in [false, true] {
                        for i in 0..n {
                            let r = g.rect(i, rtl);
                            assert!(inside(r, screen), "{layout:?} {n} on {screen:?}, card {i}: {r:?}");
                            for j in 0..i {
                                let o = g.rect(j, rtl);
                                let apart = r.x >= o.right() - 0.01 || o.x >= r.right() - 0.01 || r.y >= o.bottom() - 0.01 || o.y >= r.bottom() - 0.01;
                                assert!(apart, "{layout:?} {n} on {screen:?}: cards {j} and {i} overlap");
                            }
                            assert_eq!(g.hit(r.x + r.w / 2.0, r.y + r.h / 2.0, rtl), Some(i));
                        }
                    }
                }
            }
        }
        // Between the cards is nothing (a click there closes the menu).
        let g = CardsGrid::new(SessionCardsLayout::Row, 5, (1920.0, 1080.0));
        let (a, b) = (g.rect(0, false), g.rect(1, false));
        assert_eq!(g.hit((a.right() + b.x) / 2.0, a.y + a.h / 2.0, false), None);
    }

    #[test]
    fn cards_keep_clear_of_the_bar_and_the_dock() {
        // 1536x864 with a 34 px bar on top and a 96 px dock band below.
        let (screen, top, bottom) = ((1536.0, 864.0), 34.0, 96.0);
        let area = Rect::new(0.0, top, screen.0, screen.1 - top - bottom);
        for layout in LAYOUTS {
            for n in [5, 9, 14] {
                let g = CardsGrid::within(layout, n, area);
                assert!(g.block.y >= top && g.hint.bottom() <= screen.1 - bottom, "{layout:?} {n}: {:?} {:?}", g.block, g.hint);
                for i in 0..n {
                    let r = g.rect(i, false);
                    assert!(r.y >= top && r.bottom() <= screen.1 - bottom, "{layout:?} {n}, card {i}: {r:?}");
                }
                let middle = (g.block.y + g.hint.bottom()) / 2.0;
                assert!((middle - (area.y + area.h / 2.0)).abs() < 0.01, "centred in the free room");
                assert!((g.block.x + g.block.w / 2.0 - screen.0 / 2.0).abs() < 0.01, "still centred across");
            }
        }
    }

    #[test]
    fn the_layout_is_the_same_in_physical_pixels_at_every_scale() {
        // The surface is sized in logical px; a scaled output only changes
        // how many of those there are. At 1.5 and 2 the same five cards fit
        // whole, centred, and land on the same logical spots.
        for (physical, scale) in [((3840.0, 2160.0), 1.5), ((3840.0, 2160.0), 2.0), ((2880.0, 1620.0), 1.5), ((1920.0, 1080.0), 1.25)] {
            let screen = (physical.0 / scale, physical.1 / scale);
            let g = CardsGrid::new(SessionCardsLayout::Row, 5, screen);
            assert_eq!(g.fit, 1.0, "{screen:?}");
            assert!((g.block.x + g.block.w / 2.0 - screen.0 / 2.0).abs() < 0.01);
        }
    }

    #[test]
    fn right_to_left_mirrors_the_cards() {
        let screen = (1920.0, 1080.0);
        for layout in LAYOUTS {
            let g = CardsGrid::new(layout, 9, screen);
            for i in 0..9 {
                let (l, r) = (g.rect(i, false), g.rect(i, true));
                assert!((l.x + l.w / 2.0 - (screen.0 - (r.x + r.w / 2.0))).abs() < 0.01, "{layout:?} {i}: mirrored");
                assert_eq!(l.y, r.y);
            }
        }
        // The first card is on the reading side.
        let g = CardsGrid::new(SessionCardsLayout::Row, 5, screen);
        assert!(g.rect(0, true).x > g.rect(4, true).x);
    }

    #[test]
    fn the_arrow_keys_move_the_way_they_point() {
        let screen = (1280.0, 720.0);
        let g = CardsGrid::new(SessionCardsLayout::Row, 9, screen);
        // Along a row, wrapping round.
        assert_eq!(g.step(0, Nav::Right, false), 1);
        assert_eq!(g.step(0, Nav::Left, false), 8);
        assert_eq!(g.step(8, Nav::Right, false), 0);
        // Right to left the arrows still point where they move.
        assert_eq!(g.step(0, Nav::Left, true), 1);
        assert_eq!(g.step(1, Nav::Right, true), 0);
        for rtl in [false, true] {
            for i in 0..9 {
                let (r, l) = (g.step(i, Nav::Right, rtl), g.step(i, Nav::Left, rtl));
                // Within a row, right is the card to the right.
                if g.rect(r, rtl).y == g.rect(i, rtl).y && r != 0 && i != 0 {
                    assert!(g.rect(r, rtl).x > g.rect(i, rtl).x, "{i} right, rtl {rtl}");
                }
                assert_eq!(g.step(r, Nav::Left, rtl), i, "{i}: left undoes right");
                assert_eq!(g.step(l, Nav::Right, rtl), i);
            }
        }
        // Down from the first row lands on the nearest card below; the
        // short row is centred, so card 0 (far left) goes to 5, the middle
        // card 2 to 6 or 7, and the bottom row stays put going down.
        assert_eq!(g.step(0, Nav::Down, false), 5);
        let below = g.step(2, Nav::Down, false);
        assert!(below == 6 || below == 7, "{below}");
        assert_eq!(g.step(6, Nav::Down, false), 6);
        let above = g.step(6, Nav::Up, false);
        assert!(above == 1 || above == 2, "6 sits between 1 and 2: {above}");
        assert_eq!(g.step(0, Nav::Up, false), 0);
        // Tab, Shift+Tab, Home and End follow the order.
        assert_eq!((g.step(8, Nav::Next, false), g.step(0, Nav::Prev, false), g.step(4, Nav::First, true), g.step(4, Nav::Last, true)), (0, 8, 0, 8));
        // A column: up and down step, left and right change column.
        let c = CardsGrid::new(SessionCardsLayout::Column, 9, screen);
        assert_eq!((c.step(0, Nav::Down, false), c.step(0, Nav::Up, false)), (1, 8));
        assert_eq!(c.step(0, Nav::Right, false), 3, "the top of the next column");
        assert_eq!(c.step(4, Nav::Right, false), 7, "the same height in the next column");
        assert_eq!(c.step(0, Nav::Left, false), 0, "no column left of the first");
        assert_eq!(c.step(0, Nav::Left, true), 3, "right to left, the second column is on the left");
        assert_eq!(c.step(7, Nav::Left, false), 4);
        // One row of five: up and down have nowhere to go.
        let one = CardsGrid::new(SessionCardsLayout::Row, 5, screen);
        assert_eq!((one.step(3, Nav::Up, false), one.step(3, Nav::Down, false)), (3, 3));
        // Out-of-range input (an entry removed while open) stays in range.
        assert!(one.step(40, Nav::Right, false) < 5);
        assert_eq!(Nav::from_key("tab", true), Some(Nav::Prev));
        assert_eq!(Nav::from_key("j", false), Some(Nav::Down));
        assert_eq!(Nav::from_key("enter", false), None);
    }

    #[test]
    fn cards_enter_one_after_another_unless_motion_is_reduced() {
        assert_eq!(entrance_delay(0, 0.04, 0.24, false), 0.0);
        assert!(entrance_delay(3, 0.04, 0.24, false) > entrance_delay(2, 0.04, 0.24, false));
        assert_eq!(entrance_delay(20, 0.04, 0.24, false), 0.24, "many entries don't keep the last waiting");
        assert_eq!(entrance_delay(5, 0.04, 0.24, true), 0.0);
    }
}
