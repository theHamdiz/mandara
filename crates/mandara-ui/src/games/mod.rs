//! The game plugins' shared parts: Sudoku and 2048.
//!
//! Each game has one model (one game in progress, its history and its
//! records) shared by every surface that shows it: the bar's drop-down
//! panel on any display and the desktop widget, so a game started in the
//! panel continues on the desktop. The model exists only while its plugin
//! is on and something shows it; it is read once from the state directory
//! off the UI thread, saved coalesced while you play and flushed when the
//! plugin is switched off or the shell restarts.
//!
//! Resources: boards are fixed arrays of `u8` cells with bitmask notes;
//! nothing runs while no view is visible (the Sudoku clock runs only while
//! its panel is open or its desktop widget has the keyboard, 2048's tiles
//! request frames only while they move).

pub mod clock;
pub mod rng;
pub mod save_io;
pub mod sudoku;
pub mod swipe;
pub mod tiles;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Entity, Global, IntoElement, Styled as _, canvas};
use mandara_theme::{Color, MandaraPalette};

/// Fallback width of a sidebar card before its first layout (logical px).
pub const SIDEBAR_WIDTH_PX: f32 = 320.0;

/// Records the width a sidebar card was laid out at; a new width redraws
/// the window once so the board follows the sidebar's width.
pub fn measure_width(cell: Rc<Cell<f32>>) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let w = f32::from(bounds.size.width);
            if (cell.get() - w).abs() > 0.5 {
                cell.set(w);
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Which theme colour a game leans on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Primary,
    Secondary,
    Tertiary,
}

impl Accent {
    pub fn parse(s: &str) -> Self {
        match s {
            "secondary" => Self::Secondary,
            "tertiary" => Self::Tertiary,
            _ => Self::Primary,
        }
    }

    pub fn color(self, p: &MandaraPalette) -> Color {
        match self {
            Self::Primary => p.primary,
            Self::Secondary => p.secondary,
            Self::Tertiary => p.tertiary,
        }
    }
}

/// The live models (`None` until something shows the game).
#[derive(Default)]
struct Models {
    sudoku: Option<Entity<sudoku::model::Sudoku>>,
    tiles: Option<Entity<tiles::model::Tiles>>,
}

impl Global for Models {}

/// The Sudoku model, created (and loading) on first use.
pub fn sudoku(cx: &mut App) -> Entity<sudoku::model::Sudoku> {
    if let Some(m) = cx.try_global::<Models>().and_then(|m| m.sudoku.clone()) {
        return m;
    }
    let m = sudoku::model::Sudoku::create(cx);
    cx.default_global::<Models>().sudoku = Some(m.clone());
    m
}

/// The 2048 model, created (and loading) on first use.
pub fn tiles(cx: &mut App) -> Entity<tiles::model::Tiles> {
    if let Some(m) = cx.try_global::<Models>().and_then(|m| m.tiles.clone()) {
        return m;
    }
    let m = tiles::model::Tiles::create(cx);
    cx.default_global::<Models>().tiles = Some(m.clone());
    m
}

/// The Sudoku model if it exists (never creates it).
pub fn sudoku_if_live(cx: &App) -> Option<Entity<sudoku::model::Sudoku>> {
    cx.try_global::<Models>().and_then(|m| m.sudoku.clone())
}

pub fn tiles_if_live(cx: &App) -> Option<Entity<tiles::model::Tiles>> {
    cx.try_global::<Models>().and_then(|m| m.tiles.clone())
}

/// Writes any unsaved game now (the shell is restarting).
pub fn flush_all(cx: &mut App) {
    if let Some(m) = sudoku_if_live(cx) {
        m.update(cx, |m, _| save_io::flush(m));
    }
    if let Some(m) = tiles_if_live(cx) {
        m.update(cx, |m, _| save_io::flush(m));
    }
}

/// The plugin was switched off: save its game and let the model go (the
/// views that held it are gone with the plugin's instances).
pub fn stop_sudoku(cx: &mut App) {
    if let Some(m) = cx.has_global::<Models>().then(|| cx.global_mut::<Models>().sudoku.take()).flatten() {
        m.update(cx, |m, cx| m.stop(cx));
    }
}

pub fn stop_tiles(cx: &mut App) {
    if let Some(m) = cx.has_global::<Models>().then(|| cx.global_mut::<Models>().tiles.take()).flatten() {
        m.update(cx, |m, cx| m.stop(cx));
    }
}
