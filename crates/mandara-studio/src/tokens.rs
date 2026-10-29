//! Mandara Studio design tokens: every spacing step, radius, type size,
//! line height, dimension, icon size and opacity the Studio pages use,
//! in one place. Pages pick steps from these ladders (`px(SPACE_12_PX)`),
//! so restyling Studio means editing this file, not hunting literals.
//!
//! Names carry the value and the unit (`_PX` = logical pixels); the
//! ladders are the design decision, the names keep call sites readable.

// ── Spacing ───────────────────────────────────────────────────────────
/// Gaps, padding and margins: 3 logical px.
pub const SPACE_3_PX: f32 = 3.0;
/// Gaps, padding and margins: 4 logical px.
pub const SPACE_4_PX: f32 = 4.0;
/// Gaps, padding and margins: 5 logical px.
pub const SPACE_5_PX: f32 = 5.0;
/// Gaps, padding and margins: 6 logical px.
pub const SPACE_6_PX: f32 = 6.0;
/// Gaps, padding and margins: 7 logical px.
pub const SPACE_7_PX: f32 = 7.0;
/// Gaps, padding and margins: 8 logical px.
pub const SPACE_8_PX: f32 = 8.0;
/// Gaps, padding and margins: 9 logical px.
pub const SPACE_9_PX: f32 = 9.0;
/// Gaps, padding and margins: 10 logical px.
pub const SPACE_10_PX: f32 = 10.0;
/// Gaps, padding and margins: 11 logical px.
pub const SPACE_11_PX: f32 = 11.0;
/// Gaps, padding and margins: 12 logical px.
pub const SPACE_12_PX: f32 = 12.0;
/// Gaps, padding and margins: 14 logical px.
pub const SPACE_14_PX: f32 = 14.0;
/// Gaps, padding and margins: 16 logical px.
pub const SPACE_16_PX: f32 = 16.0;
/// Gaps, padding and margins: 18 logical px.
pub const SPACE_18_PX: f32 = 18.0;
/// Gaps, padding and margins: 20 logical px.
pub const SPACE_20_PX: f32 = 20.0;
/// Gaps, padding and margins: 22 logical px.
pub const SPACE_22_PX: f32 = 22.0;
/// Gaps, padding and margins: 24 logical px.
pub const SPACE_24_PX: f32 = 24.0;
/// Gaps, padding and margins: 26 logical px.
pub const SPACE_26_PX: f32 = 26.0;
/// Gaps, padding and margins: 28 logical px.
pub const SPACE_28_PX: f32 = 28.0;
/// Gaps, padding and margins: 30 logical px.
pub const SPACE_30_PX: f32 = 30.0;

// ── Corner radii ──────────────────────────────────────────────────────
/// Rounded corners (smooth, generous radii; no hard outlines): 1.5 logical px.
pub const RADIUS_1_5_PX: f32 = 1.5;
/// Rounded corners (smooth, generous radii; no hard outlines): 3 logical px.
pub const RADIUS_3_PX: f32 = 3.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 4 logical px.
pub const RADIUS_4_PX: f32 = 4.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 5 logical px.
pub const RADIUS_5_PX: f32 = 5.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 6 logical px.
pub const RADIUS_6_PX: f32 = 6.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 8 logical px.
pub const RADIUS_8_PX: f32 = 8.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 9 logical px.
pub const RADIUS_9_PX: f32 = 9.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 10 logical px.
pub const RADIUS_10_PX: f32 = 10.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 11 logical px.
pub const RADIUS_11_PX: f32 = 11.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 12 logical px.
pub const RADIUS_12_PX: f32 = 12.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 14 logical px.
pub const RADIUS_14_PX: f32 = 14.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 15 logical px.
pub const RADIUS_15_PX: f32 = 15.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 16 logical px.
pub const RADIUS_16_PX: f32 = 16.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 18 logical px.
pub const RADIUS_18_PX: f32 = 18.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 20 logical px.
pub const RADIUS_20_PX: f32 = 20.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 22 logical px.
pub const RADIUS_22_PX: f32 = 22.0;
/// Rounded corners (smooth, generous radii; no hard outlines): 26 logical px.
pub const RADIUS_26_PX: f32 = 26.0;

// ── Type sizes ────────────────────────────────────────────────────────
/// Font sizes: 9.5 logical px.
pub const TEXT_9_5_PX: f32 = 9.5;
/// Font sizes: 10 logical px.
pub const TEXT_10_PX: f32 = 10.0;
/// Font sizes: 10.5 logical px.
pub const TEXT_10_5_PX: f32 = 10.5;
/// Font sizes: 11 logical px.
pub const TEXT_11_PX: f32 = 11.0;
/// Font sizes: 11.5 logical px.
pub const TEXT_11_5_PX: f32 = 11.5;
/// Font sizes: 12 logical px.
pub const TEXT_12_PX: f32 = 12.0;
/// Font sizes: 12.5 logical px.
pub const TEXT_12_5_PX: f32 = 12.5;
/// Font sizes: 13 logical px.
pub const TEXT_13_PX: f32 = 13.0;
/// Font sizes: 13.5 logical px.
pub const TEXT_13_5_PX: f32 = 13.5;
/// Font sizes: 14 logical px.
pub const TEXT_14_PX: f32 = 14.0;
/// Font sizes: 14.5 logical px.
pub const TEXT_14_5_PX: f32 = 14.5;
/// Font sizes: 15 logical px.
pub const TEXT_15_PX: f32 = 15.0;
/// Font sizes: 17 logical px.
pub const TEXT_17_PX: f32 = 17.0;
/// Font sizes: 18 logical px.
pub const TEXT_18_PX: f32 = 18.0;
/// Font sizes: 20 logical px.
pub const TEXT_20_PX: f32 = 20.0;
/// Font sizes: 24 logical px.
pub const TEXT_24_PX: f32 = 24.0;
/// Font sizes: 26 logical px.
pub const TEXT_26_PX: f32 = 26.0;

// ── Line heights ──────────────────────────────────────────────────────
/// Line heights for multi-line text: 16 logical px.
pub const LEADING_16_PX: f32 = 16.0;
/// Line heights for multi-line text: 17 logical px.
pub const LEADING_17_PX: f32 = 17.0;
/// Line heights for multi-line text: 19 logical px.
pub const LEADING_19_PX: f32 = 19.0;
/// Line heights for multi-line text: 21 logical px.
pub const LEADING_21_PX: f32 = 21.0;

// ── Dimensions ────────────────────────────────────────────────────────
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 5 logical px.
pub const DIM_5_PX: f32 = 5.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 6 logical px.
pub const DIM_6_PX: f32 = 6.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 8 logical px.
pub const DIM_8_PX: f32 = 8.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 8.5 logical px.
pub const DIM_8_5_PX: f32 = 8.5;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 9 logical px.
pub const DIM_9_PX: f32 = 9.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 10 logical px.
pub const DIM_10_PX: f32 = 10.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 12 logical px.
pub const DIM_12_PX: f32 = 12.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 14 logical px.
pub const DIM_14_PX: f32 = 14.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 15 logical px.
pub const DIM_15_PX: f32 = 15.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 16 logical px.
pub const DIM_16_PX: f32 = 16.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 18 logical px.
pub const DIM_18_PX: f32 = 18.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 20 logical px.
pub const DIM_20_PX: f32 = 20.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 22 logical px.
pub const DIM_22_PX: f32 = 22.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 24 logical px.
pub const DIM_24_PX: f32 = 24.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 26 logical px.
pub const DIM_26_PX: f32 = 26.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 28 logical px.
pub const DIM_28_PX: f32 = 28.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 30 logical px.
pub const DIM_30_PX: f32 = 30.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 32 logical px.
pub const DIM_32_PX: f32 = 32.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 34 logical px.
pub const DIM_34_PX: f32 = 34.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 36 logical px.
pub const DIM_36_PX: f32 = 36.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 38 logical px.
pub const DIM_38_PX: f32 = 38.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 40 logical px.
pub const DIM_40_PX: f32 = 40.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 44 logical px.
pub const DIM_44_PX: f32 = 44.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 46 logical px.
pub const DIM_46_PX: f32 = 46.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 48 logical px.
pub const DIM_48_PX: f32 = 48.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 50 logical px.
pub const DIM_50_PX: f32 = 50.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 52 logical px.
pub const DIM_52_PX: f32 = 52.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 56 logical px.
pub const DIM_56_PX: f32 = 56.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 58 logical px.
pub const DIM_58_PX: f32 = 58.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 60 logical px.
pub const DIM_60_PX: f32 = 60.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 64 logical px.
pub const DIM_64_PX: f32 = 64.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 68 logical px.
pub const DIM_68_PX: f32 = 68.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 70 logical px.
pub const DIM_70_PX: f32 = 70.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 76 logical px.
pub const DIM_76_PX: f32 = 76.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 88 logical px.
pub const DIM_88_PX: f32 = 88.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 90 logical px.
pub const DIM_90_PX: f32 = 90.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 96 logical px.
pub const DIM_96_PX: f32 = 96.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 108 logical px.
pub const DIM_108_PX: f32 = 108.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 110 logical px.
pub const DIM_110_PX: f32 = 110.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 112 logical px.
pub const DIM_112_PX: f32 = 112.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 118 logical px.
pub const DIM_118_PX: f32 = 118.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 120 logical px.
pub const DIM_120_PX: f32 = 120.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 128 logical px.
pub const DIM_128_PX: f32 = 128.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 138 logical px.
pub const DIM_138_PX: f32 = 138.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 140 logical px.
pub const DIM_140_PX: f32 = 140.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 150 logical px.
pub const DIM_150_PX: f32 = 150.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 170 logical px.
pub const DIM_170_PX: f32 = 170.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 180 logical px.
pub const DIM_180_PX: f32 = 180.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 200 logical px.
pub const DIM_200_PX: f32 = 200.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 220 logical px.
pub const DIM_220_PX: f32 = 220.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 240 logical px.
pub const DIM_240_PX: f32 = 240.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 260 logical px.
pub const DIM_260_PX: f32 = 260.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 280 logical px.
pub const DIM_280_PX: f32 = 280.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 320 logical px.
pub const DIM_320_PX: f32 = 320.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 360 logical px.
pub const DIM_360_PX: f32 = 360.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 380 logical px.
pub const DIM_380_PX: f32 = 380.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 420 logical px.
pub const DIM_420_PX: f32 = 420.0;
/// Fixed widths, heights, square sizes and absolute offsets of controls, tiles and sketches: 1000 logical px.
pub const DIM_1000_PX: f32 = 1000.0;

// ── Icon sizes ────────────────────────────────────────────────────────
/// Glyph sizes for `icon()` / `any_icon()`: 10 logical px.
pub const ICON_10_PX: f32 = 10.0;
/// Glyph sizes for `icon()` / `any_icon()`: 11 logical px.
pub const ICON_11_PX: f32 = 11.0;
/// Glyph sizes for `icon()` / `any_icon()`: 12 logical px.
pub const ICON_12_PX: f32 = 12.0;
/// Glyph sizes for `icon()` / `any_icon()`: 13 logical px.
pub const ICON_13_PX: f32 = 13.0;
/// Glyph sizes for `icon()` / `any_icon()`: 14 logical px.
pub const ICON_14_PX: f32 = 14.0;
/// Glyph sizes for `icon()` / `any_icon()`: 15 logical px.
pub const ICON_15_PX: f32 = 15.0;
/// Glyph sizes for `icon()` / `any_icon()`: 16 logical px.
pub const ICON_16_PX: f32 = 16.0;
/// Glyph sizes for `icon()` / `any_icon()`: 17 logical px.
pub const ICON_17_PX: f32 = 17.0;
/// Glyph sizes for `icon()` / `any_icon()`: 22 logical px.
pub const ICON_22_PX: f32 = 22.0;
/// Glyph sizes for `icon()` / `any_icon()`: 24 logical px.
pub const ICON_24_PX: f32 = 24.0;
/// Glyph sizes for `icon()` / `any_icon()`: 26 logical px.
pub const ICON_26_PX: f32 = 26.0;

// ── Opacities ─────────────────────────────────────────────────────────
/// Alpha of tints, state layers and dimmed content (fraction of 1): 3 %.
pub const ALPHA_3: f32 = 0.03;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 3.5 %.
pub const ALPHA_3_5: f32 = 0.035;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 5 %.
pub const ALPHA_5: f32 = 0.05;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 6 %.
pub const ALPHA_6: f32 = 0.06;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 8 %.
pub const ALPHA_8: f32 = 0.08;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 10 %.
pub const ALPHA_10: f32 = 0.1;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 12 %.
pub const ALPHA_12: f32 = 0.12;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 13 %.
pub const ALPHA_13: f32 = 0.13;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 14 %.
pub const ALPHA_14: f32 = 0.14;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 15 %.
pub const ALPHA_15: f32 = 0.15;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 16 %.
pub const ALPHA_16: f32 = 0.16;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 18 %.
pub const ALPHA_18: f32 = 0.18;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 20 %.
pub const ALPHA_20: f32 = 0.2;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 22 %.
pub const ALPHA_22: f32 = 0.22;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 25 %.
pub const ALPHA_25: f32 = 0.25;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 28 %.
pub const ALPHA_28: f32 = 0.28;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 30 %.
pub const ALPHA_30: f32 = 0.3;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 35 %.
pub const ALPHA_35: f32 = 0.35;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 40 %.
pub const ALPHA_40: f32 = 0.4;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 45 %.
pub const ALPHA_45: f32 = 0.45;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 50 %.
pub const ALPHA_50: f32 = 0.5;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 55 %.
pub const ALPHA_55: f32 = 0.55;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 60 %.
pub const ALPHA_60: f32 = 0.6;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 70 %.
pub const ALPHA_70: f32 = 0.7;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 72 %.
pub const ALPHA_72: f32 = 0.72;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 75 %.
pub const ALPHA_75: f32 = 0.75;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 78 %.
pub const ALPHA_78: f32 = 0.78;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 80 %.
pub const ALPHA_80: f32 = 0.8;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 85 %.
pub const ALPHA_85: f32 = 0.85;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 86 %.
pub const ALPHA_86: f32 = 0.86;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 88 %.
pub const ALPHA_88: f32 = 0.88;
/// Alpha of tints, state layers and dimmed content (fraction of 1): 90 %.
pub const ALPHA_90: f32 = 0.9;

// ── Responsive layout ─────────────────────────────────────────────────
// Window → page geometry (see `app::grid::PageMetrics`) and the card
// grids every page lays out with `app::grid::fit`. Card widths are ranges:
// a grid picks as many columns as fit at the minimum, then widens the
// cards to fill the row exactly, never past the maximum.

/// Below this window width the sidebar collapses to an icon rail…
pub const RAIL_BELOW_PX: f32 = 900.0;
/// …this wide (the full sidebar is [`NAV_W_PX`]).
pub const RAIL_W_PX: f32 = 72.0;
/// The full navigation sidebar.
pub const NAV_W_PX: f32 = 240.0;
/// Sidebar density (compact, so every group fits expanded at 1440 px):
/// a section row, the gap between rows, a group header and the space
/// above it.
pub const NAV_ITEM_H_PX: f32 = 30.0;
pub const NAV_ITEM_GAP_PX: f32 = 1.0;
pub const NAV_HEADER_H_PX: f32 = 24.0;
pub const NAV_GROUP_GAP_PX: f32 = 10.0;
/// Section icons in the sidebar, and the group header chevron.
pub const NAV_ICON_PX: f32 = 15.0;
pub const NAV_CHEVRON_PX: f32 = 10.0;
/// Sections without search matches fade to this opacity while searching.
pub const NAV_NO_MATCH_ALPHA: f32 = 0.42;
/// Home → a group's icon chip.
pub const HOME_CHIP_PX: f32 = 40.0;
/// Home → a section link row inside a group card.
pub const HOME_LINK_H_PX: f32 = 30.0;
/// Home / Themes → the space between a page's sections.
pub const PAGE_SECTION_GAP_PX: f32 = 34.0;
/// Home → the gap between bento widget cards.
pub const BENTO_GAP_PX: f32 = SPACE_14_PX;
/// Content narrower than this stacks each control under its label, and
/// two-column sections become one column.
pub const STACK_ROWS_BELOW_PX: f32 = 640.0;
/// Page gutters by content width.
pub const GUTTER_SMALL_BELOW_PX: f32 = 560.0;
pub const GUTTER_MEDIUM_BELOW_PX: f32 = 760.0;
pub const GUTTER_SMALL_PX: f32 = 16.0;
pub const GUTTER_MEDIUM_PX: f32 = 24.0;
pub const GUTTER_LARGE_PX: f32 = 40.0;
/// The page column never grows wider than this (ultra-wide windows centre it).
pub const PAGE_MAX_W_PX: f32 = 1000.0;
/// A grouped-settings card's inner padding (`rows()`, compositor `card()`),
/// subtracted from the column when a grid sits inside such a card.
pub const SECTION_CARD_PAD_PX: f32 = 18.0;

/// A card grid: card width range and the gap between cards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardGrid {
    pub min_w: f32,
    pub max_w: f32,
    pub gap: f32,
}

/// Themes → designed theme cards (preview + name + blurb).
pub const THEME_GRID: CardGrid = CardGrid { min_w: 260.0, max_w: 440.0, gap: SPACE_18_PX };
/// Height of a theme card's shell preview per unit of its width.
pub const THEME_PREVIEW_ASPECT: f32 = 9.0 / 16.0;
/// Plugins → Installed tiles and Marketplace results.
pub const PLUGIN_GRID: CardGrid = CardGrid { min_w: 300.0, max_w: 480.0, gap: SPACE_12_PX };
/// Plugins → Discover / Community: store cards (cover art, name, pitch,
/// Get / On button).
pub const STORE_CARD_GRID: CardGrid = CardGrid { min_w: 220.0, max_w: 360.0, gap: SPACE_14_PX };
/// Height of a store card's cover per unit of its width (16:9 art).
pub const STORE_COVER_ASPECT: f32 = 9.0 / 16.0;
/// Plugins → Installed → "Removed Mandara plugins" rows.
pub const REMOVED_GRID: CardGrid = CardGrid { min_w: 260.0, max_w: 420.0, gap: SPACE_10_PX };
/// Artwork tile on a removed-plugin row.
pub const REMOVED_TILE_PX: f32 = 32.0;
/// Users → account cards.
pub const USER_GRID: CardGrid = CardGrid { min_w: 176.0, max_w: 280.0, gap: SPACE_14_PX };
/// Users → profile photo presets in the avatar dialog.
pub const AVATAR_PRESET_GRID: CardGrid = CardGrid { min_w: 64.0, max_w: 104.0, gap: SPACE_8_PX };
/// General → language cards.
pub const LANGUAGE_GRID: CardGrid = CardGrid { min_w: 160.0, max_w: 260.0, gap: SPACE_10_PX };
/// Bar → layout preset cards.
pub const BAR_PRESET_GRID: CardGrid = CardGrid { min_w: 180.0, max_w: 320.0, gap: SPACE_10_PX };
/// Login screen → theme swatches.
pub const GREETER_THEME_GRID: CardGrid = CardGrid { min_w: 112.0, max_w: 180.0, gap: SPACE_8_PX };
/// Compositor → MangoWM per-tag cards (always a 3 × 3 board).
pub const TAG_GRID: CardGrid = CardGrid { min_w: 96.0, max_w: 168.0, gap: SPACE_10_PX };
/// Compositor → layout picker chips.
pub const LAYOUT_CHIP_GRID: CardGrid = CardGrid { min_w: 80.0, max_w: 130.0, gap: SPACE_6_PX };
/// Performance → the live usage tiles (4 → 2 × 2 → 1 column).
pub const STAT_TILE_GRID: CardGrid = CardGrid { min_w: 220.0, max_w: 1000.0, gap: SPACE_12_PX };
/// Plugins → Local → reference cards (view kinds, hot reload / settings).
pub const LOCAL_CARD_GRID: CardGrid = CardGrid { min_w: 280.0, max_w: 1000.0, gap: SPACE_8_PX };
/// Home → one card per sidebar group.
pub const HOME_GROUP_GRID: CardGrid = CardGrid { min_w: 250.0, max_w: 1000.0, gap: SPACE_14_PX };
/// Maintenance / Audio → side-by-side cards.
pub const PANEL_GRID: CardGrid = CardGrid { min_w: 340.0, max_w: 1000.0, gap: SPACE_14_PX };
/// Tag board shape.
pub const TAG_GRID_COLS: usize = 3;
/// The tag editor beside the board needs at least this much; below it
/// the editor moves under the board.
pub const TAG_EDITOR_MIN_W_PX: f32 = 300.0;
/// Gap between the tag board and its editor.
pub const TAG_PANE_GAP_PX: f32 = SPACE_20_PX;
/// A tag card's layout sketch: height per unit of width.
pub const TAG_GLYPH_ASPECT: f32 = 0.5;
/// Width of GPUI's `border_2()` (selection rings on cards).
pub const BORDER_2_PX: f32 = 2.0;
/// Launcher icon picker tiles.
pub const ICON_TILE_GRID: CardGrid = CardGrid { min_w: 64.0, max_w: 104.0, gap: SPACE_6_PX };
/// Themes → the current wallpaper's thumbnail: beside the controls…
pub const WALLPAPER_THUMB_W_PX: f32 = 220.0;
/// …height per unit of width (a 16:10 crop), also when stacked full-width.
pub const WALLPAPER_THUMB_ASPECT: f32 = 0.625;
/// Width of GPUI's `border_1()`.
pub const BORDER_1_PX: f32 = 1.0;
/// Login screen → a theme swatch's gradient: height per unit of width.
pub const GREETER_SWATCH_ASPECT: f32 = 44.0 / 102.0;
/// Audio → a mixer row's app name column: this share of the row, at most…
pub const MIXER_NAME_SHARE: f32 = 0.3;
/// …this wide.
pub const MIXER_NAME_MAX_W_PX: f32 = 200.0;
