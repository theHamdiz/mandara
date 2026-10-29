//! Responsive card grids, shared by every Studio page.
//!
//! The page column's width is known analytically from the window width
//! ([`PageMetrics`]), so every grid gets its column count in the same frame
//! the window resizes (no measure-then-relayout flicker). [`fit`] turns an
//! available width and a [`CardGrid`] (min / max card width + gap) into a
//! [`GridFit`]: as many columns as fit at the minimum width, cards widened
//! to fill the row exactly and never past the maximum. [`GridFit::render`]
//! lays the cards out on a CSS grid of equal `1fr` columns, so rows always
//! end flush with the column (no ragged trailing gap, no fractional-scale
//! wrap), the last row keeps the same column widths and starts at the
//! leading edge, and right-to-left languages mirror the order.

use mandara_ui::bidi::Bidi as _;
use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder as _, px};

use crate::tokens::*;

/// Page geometry for a window width: the same numbers Studio's frame uses
/// to place the sidebar, gutters and page column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageMetrics {
    /// The sidebar is collapsed to its icon rail.
    pub rail: bool,
    /// Controls stack under their labels; two-column sections stack.
    pub stack_rows: bool,
    /// Horizontal page padding on each side.
    pub gutter: f32,
    /// Width of the page column (the body every page draws into).
    pub column_w: f32,
}

impl PageMetrics {
    pub fn for_width(window_w: f32) -> Self {
        let rail = window_w < RAIL_BELOW_PX;
        let content_w = (window_w - if rail { RAIL_W_PX } else { NAV_W_PX }).max(0.0);
        let gutter = if content_w < GUTTER_SMALL_BELOW_PX {
            GUTTER_SMALL_PX
        } else if content_w < GUTTER_MEDIUM_BELOW_PX {
            GUTTER_MEDIUM_PX
        } else {
            GUTTER_LARGE_PX
        };
        Self { rail, stack_rows: content_w < STACK_ROWS_BELOW_PX, gutter, column_w: (content_w - 2.0 * gutter).clamp(0.0, PAGE_MAX_W_PX) }
    }

    pub fn of(window: &Window) -> Self {
        Self::for_width(f32::from(window.viewport_size().width))
    }
}

/// Columns and card width for one grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFit {
    pub cols: usize,
    /// Each card's width (every column is this wide).
    pub card_w: f32,
    pub gap: f32,
    /// The whole grid's width: the available width, unless the cards hit
    /// their maximum (then the grid is narrower and sits at the leading edge).
    pub width: f32,
    max_w: f32,
    avail: f32,
}

/// Fits cards of `spec` into `avail` logical px.
pub fn fit(avail: f32, spec: CardGrid) -> GridFit {
    let avail = avail.max(0.0);
    // As many columns as fit at the minimum width; the cards then widen to
    // fill the row, up to their maximum (past it the grid stops growing and
    // stays at the leading edge — pick `max_w ≥ 1.5 × min_w + gap` so that
    // only happens to a single column).
    let cols = ((avail + spec.gap) / (spec.min_w + spec.gap)).floor().max(1.0) as usize;
    GridFit { cols: 0, card_w: 0.0, gap: spec.gap, width: 0.0, max_w: spec.max_w, avail }.with_cols(cols)
}

impl GridFit {
    fn with_cols(mut self, cols: usize) -> Self {
        let cols = cols.max(1);
        let gaps = self.gap * (cols - 1) as f32;
        self.cols = cols;
        self.card_w = ((self.avail - gaps) / cols as f32).clamp(0.0, self.max_w);
        self.width = (self.card_w * cols as f32 + gaps).min(self.avail.max(self.card_w));
        self
    }

    /// At most `max` columns (a fixed-shape board, e.g. the 3 × 3 tag grid).
    pub fn at_most(self, max: usize) -> Self {
        if self.cols > max { self.with_cols(max) } else { self }
    }

    /// Spreads `n` cards evenly over the fewest rows the current column
    /// count needs: 4 tiles on 3 columns become 2 × 2, 11 chips on 7
    /// columns become 6 + 5, so no row ends with a lone orphan.
    pub fn balanced(self, n: usize) -> Self {
        if n == 0 || self.cols <= 1 {
            return self;
        }
        let rows = n.div_ceil(self.cols);
        self.with_cols(n.div_ceil(rows))
    }

    /// Height for a box `ratio` tall per unit of the card's width, less
    /// `inset` (padding / border) on each side horizontally.
    pub fn aspect_h(&self, ratio: f32, inset: f32) -> f32 {
        ((self.card_w - 2.0 * inset).max(0.0) * ratio).floor()
    }

    /// An empty grid container (for callers that place cells themselves).
    pub fn container(&self) -> Div {
        let rtl = mandara_i18n::language().is_rtl();
        div()
            .grid()
            .grid_cols(self.cols as u16)
            .gap(px(self.gap))
            .w_full()
            .max_w(px(self.width.max(1.0)))
            // A grid narrower than the column sits at the leading edge.
            .when(rtl, |el| el.ml_auto())
    }

    /// Lays `items` out row by row. Each card is wrapped in a cell that
    /// stretches to the row's height; give a card `.flex_1()` (or `.h_full()`)
    /// to make every card in a row the same height.
    pub fn render<E: IntoElement>(&self, items: impl IntoIterator<Item = E>) -> AnyElement {
        let rtl = mandara_i18n::language().is_rtl();
        let cols = self.cols;
        self.container()
            .children(items.into_iter().enumerate().map(|(i, item)| {
                div()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    // Right-to-left: the first card takes the rightmost column.
                    .when(rtl, |el| el.col_start((cols - i % cols) as i16).row_start((i / cols + 1) as i16))
                    .child(item)
            }))
            .into_any_element()
    }
}

/// Two side-by-side sections that stack into one column below `min_each`
/// per section (or when the page stacks its rows).
pub fn pair(avail: f32, min_each: f32, gap: f32, first: impl IntoElement, second: impl IntoElement) -> AnyElement {
    fit(avail, CardGrid { min_w: min_each, max_w: f32::MAX, gap }).at_most(2).render([first.into_any_element(), second.into_any_element()])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: CardGrid = CardGrid { min_w: 200.0, max_w: 300.0, gap: 10.0 };

    fn row_w(f: &GridFit) -> f32 {
        f.card_w * f.cols as f32 + f.gap * (f.cols - 1) as f32
    }

    #[test]
    fn fills_the_row_exactly() {
        for avail in [200.0, 333.3, 419.0, 640.0, 853.33, 1000.0] {
            let f = fit(avail, SPEC);
            assert!(f.card_w >= SPEC.min_w - 0.01 || f.cols == 1, "{avail}: {f:?}");
            assert!(f.card_w <= SPEC.max_w + 0.01, "{avail}: {f:?}");
            if f.card_w < SPEC.max_w {
                assert!((row_w(&f) - avail).abs() < 0.01, "{avail}: {f:?}");
            }
            assert!(row_w(&f) <= avail + 0.01, "{avail}: overflows {f:?}");
        }
    }

    #[test]
    fn never_stretches_past_the_maximum() {
        // One column of a 200..300 card on a 390 px column: capped at 300.
        let f = fit(390.0, SPEC);
        assert_eq!(f.cols, 1);
        assert_eq!(f.card_w, 300.0);
        assert_eq!(f.width, 300.0);
        // Ultra-wide: more columns, never wider cards.
        assert!(fit(5000.0, SPEC).card_w <= SPEC.max_w);
    }

    #[test]
    fn narrower_than_one_card_shrinks_it() {
        let f = fit(120.0, SPEC);
        assert_eq!((f.cols, f.card_w), (1, 120.0));
    }

    #[test]
    fn balanced_rows() {
        let tiles = CardGrid { min_w: 220.0, max_w: 1000.0, gap: 12.0 };
        assert_eq!(fit(1000.0, tiles).balanced(4).cols, 4);
        assert_eq!(fit(720.0, tiles).balanced(4).cols, 2, "3 + 1 becomes 2 × 2");
        assert_eq!(fit(400.0, tiles).balanced(4).cols, 1);
        let chips = CardGrid { min_w: 80.0, max_w: 120.0, gap: 6.0 };
        assert_eq!(fit(600.0, chips).balanced(11).cols, 6, "6 + 5, not 7 + 4");
        assert_eq!(fit(600.0, TAG_GRID).at_most(TAG_GRID_COLS).balanced(9).cols, 3);
    }

    /// Every Studio grid widens its cards enough that a row of two or more
    /// never hits the maximum (only a lone column can be capped).
    #[test]
    fn token_grids_fill_their_rows() {
        for (name, g) in [
            ("THEME", THEME_GRID),
            ("PLUGIN", PLUGIN_GRID),
            ("STORE_CARD", STORE_CARD_GRID),
            ("REMOVED", REMOVED_GRID),
            ("USER", USER_GRID),
            ("AVATAR_PRESET", AVATAR_PRESET_GRID),
            ("LANGUAGE", LANGUAGE_GRID),
            ("BAR_PRESET", BAR_PRESET_GRID),
            ("GREETER_THEME", GREETER_THEME_GRID),
            ("TAG", TAG_GRID),
            ("LAYOUT_CHIP", LAYOUT_CHIP_GRID),
            ("STAT_TILE", STAT_TILE_GRID),
            ("LOCAL_CARD", LOCAL_CARD_GRID),
            ("PANEL", PANEL_GRID),
            ("ICON_TILE", ICON_TILE_GRID),
        ] {
            assert!(g.max_w >= 1.5 * g.min_w + g.gap, "{name}_GRID: max_w must be ≥ 1.5 × min_w + gap");
            let mut w = g.min_w;
            while w < 3000.0 {
                let f = fit(w, g);
                if f.cols > 1 {
                    assert!((row_w(&f) - w).abs() < 0.01, "{name}_GRID at {w}: ragged {f:?}");
                }
                w += 7.3;
            }
        }
    }

    #[test]
    fn page_metrics_match_the_frame() {
        let narrow = PageMetrics::for_width(760.0);
        assert!(narrow.rail && !narrow.stack_rows);
        assert!(PageMetrics::for_width(600.0).stack_rows);
        assert_eq!(narrow.column_w, 760.0 - RAIL_W_PX - 2.0 * GUTTER_MEDIUM_PX);
        let wide = PageMetrics::for_width(2560.0);
        assert!(!wide.rail && !wide.stack_rows);
        assert_eq!(wide.column_w, PAGE_MAX_W_PX);
    }
}
