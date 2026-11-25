//! The game plugins' shared parts: Sudoku and 2048.
//!
//! Each game has one model (one game in progress, its history and its
//! records) shared by every surface that shows it: the bar's drop-down
//! panel on any display, the sidebar card and the desktop widget, so a game
//! started in the panel continues on the desktop. The model exists only
//! while its plugin is on and a view of it exists: it is read from the
//! state directory off the UI thread, saved coalesced while you play, and
//! saved and let go when its last view goes (the sidebar closed with only
//! its card showing the game), the plugin is switched off or the shell
//! restarts. `mandara msg` commands keep it for [`COMMAND_KEEP`] after
//! their last use when no view shows it.
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
use std::time::Duration;

use gpui::{App, Context, Entity, Global, IntoElement, Styled as _, Subscription, Task, canvas};
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

/// A model with no view is kept this long after a `mandara msg` command
/// used it, so a script's next command finds it loaded.
pub const COMMAND_KEEP: Duration = Duration::from_secs(60);

/// One game's live model and how many views show it.
pub struct Slot<T> {
    model: Option<Entity<T>>,
    /// Views (panel, sidebar card, desktop widget) holding the model.
    users: Users,
    /// Lets a command's model go once nothing used it for [`COMMAND_KEEP`].
    idle: Option<Task<()>>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self { model: None, users: Users::default(), idle: None }
    }
}

/// Counts a model's views; the last one leaving lets the model go.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Users(usize);

impl Users {
    pub fn join(&mut self) {
        self.0 += 1;
    }

    /// One view left; `true` when it was the last.
    pub fn leave(&mut self) -> bool {
        self.0 = self.0.saturating_sub(1);
        self.0 == 0
    }

    pub fn none(&self) -> bool {
        self.0 == 0
    }
}

/// The live models (`None` until something shows the game).
#[derive(Default)]
struct Models {
    sudoku: Slot<sudoku::model::Sudoku>,
    tiles: Slot<tiles::model::Tiles>,
}

impl Global for Models {}

/// The two games, for the helpers below.
trait Game: Sized + 'static {
    fn slot(models: &mut Models) -> &mut Slot<Self>;
    fn peek(models: &Models) -> &Slot<Self>;
    fn create(cx: &mut App) -> Entity<Self>;
    /// Saves and stops the clock (the model is about to go).
    fn stop(&mut self, cx: &mut Context<Self>);
}

impl Game for sudoku::model::Sudoku {
    fn slot(models: &mut Models) -> &mut Slot<Self> { &mut models.sudoku }
    fn peek(models: &Models) -> &Slot<Self> { &models.sudoku }
    fn create(cx: &mut App) -> Entity<Self> { Self::create(cx) }
    fn stop(&mut self, cx: &mut Context<Self>) { Self::stop(self, cx) }
}

impl Game for tiles::model::Tiles {
    fn slot(models: &mut Models) -> &mut Slot<Self> { &mut models.tiles }
    fn peek(models: &Models) -> &Slot<Self> { &models.tiles }
    fn create(cx: &mut App) -> Entity<Self> { Self::create(cx) }
    fn stop(&mut self, cx: &mut Context<Self>) { Self::stop(self, cx) }
}

fn model<T: Game>(cx: &mut App) -> Entity<T> {
    if let Some(m) = cx.try_global::<Models>().and_then(|m| T::peek(m).model.clone()) {
        return m;
    }
    let m = T::create(cx);
    T::slot(cx.default_global::<Models>()).model = Some(m.clone());
    m
}

fn slot_mut<T: Game>(cx: &mut App) -> Option<&mut Slot<T>> {
    cx.has_global::<Models>().then(|| T::slot(cx.global_mut::<Models>()))
}

fn if_live<T: Game>(cx: &App) -> Option<Entity<T>> {
    cx.try_global::<Models>().and_then(|m| T::peek(m).model.clone())
}

/// A view of the game was made: it holds the model until it is dropped.
fn join<V: 'static, T: Game>(cx: &mut Context<V>) -> Subscription {
    let slot = T::slot(cx.default_global::<Models>());
    slot.users.join();
    slot.idle = None;
    cx.on_release(|_, cx| {
        let last = slot_mut::<T>(cx).is_some_and(|slot| slot.users.leave());
        // Deferred: a view replaced in the same update (a rebuilt panel)
        // joins again before the model would go.
        if last {
            cx.defer(|cx| {
                if cx.try_global::<Models>().is_some_and(|m| T::peek(m).users.none()) {
                    stop::<T>(cx);
                }
            });
        }
    })
}

/// Saves the game and lets its model go (no view shows it, or the plugin
/// was switched off).
fn stop<T: Game>(cx: &mut App) {
    let Some(slot) = slot_mut::<T>(cx) else { return };
    slot.idle = None;
    if let Some(m) = slot.model.take() {
        m.update(cx, |m, cx| m.stop(cx));
    }
}

/// A command used the model: with no view of it, let it go after
/// [`COMMAND_KEEP`] without another command.
fn keep_for_command<T: Game>(cx: &mut App) {
    if !cx.try_global::<Models>().is_some_and(|m| T::peek(m).users.none() && T::peek(m).model.is_some()) {
        return;
    }
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(COMMAND_KEEP).await;
        cx.update(|cx| {
            if cx.try_global::<Models>().is_some_and(|m| T::peek(m).users.none()) {
                stop::<T>(cx);
            }
        });
    });
    T::slot(cx.default_global::<Models>()).idle = Some(task);
}

/// The Sudoku model, created (and loading) on first use.
pub fn sudoku(cx: &mut App) -> Entity<sudoku::model::Sudoku> {
    model(cx)
}

/// The 2048 model, created (and loading) on first use.
pub fn tiles(cx: &mut App) -> Entity<tiles::model::Tiles> {
    model(cx)
}

/// The Sudoku model if it exists (never creates it).
pub fn sudoku_if_live(cx: &App) -> Option<Entity<sudoku::model::Sudoku>> {
    if_live(cx)
}

pub fn tiles_if_live(cx: &App) -> Option<Entity<tiles::model::Tiles>> {
    if_live(cx)
}

/// A Sudoku view holds the model while it lives (keep the subscription).
pub fn join_sudoku<V: 'static>(cx: &mut Context<V>) -> Subscription {
    join::<V, sudoku::model::Sudoku>(cx)
}

/// A 2048 view holds the model while it lives (keep the subscription).
pub fn join_tiles<V: 'static>(cx: &mut Context<V>) -> Subscription {
    join::<V, tiles::model::Tiles>(cx)
}

/// `mandara msg sudoku` used the model.
pub fn sudoku_command_done(cx: &mut App) {
    keep_for_command::<sudoku::model::Sudoku>(cx);
}

/// `mandara msg 2048` used the model.
pub fn tiles_command_done(cx: &mut App) {
    keep_for_command::<tiles::model::Tiles>(cx);
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
    stop::<sudoku::model::Sudoku>(cx);
}

pub fn stop_tiles(cx: &mut App) {
    stop::<tiles::model::Tiles>(cx);
}

/// Whether a game's model is alive (tests and `diagnostics`).
pub fn live_models(cx: &App) -> (bool, bool) {
    cx.try_global::<Models>().map_or((false, false), |m| (m.sudoku.model.is_some(), m.tiles.model.is_some()))
}

#[cfg(test)]
mod tests {
    use super::Users;

    #[test]
    fn the_last_view_leaving_lets_the_model_go() {
        let mut users = Users::default();
        assert!(users.none());
        users.join();
        users.join();
        assert!(!users.leave(), "the panel still shows the game");
        assert!(users.leave(), "the sidebar card was the last view");
        assert!(users.none());
        // A late release never underflows into "in use".
        assert!(users.leave());
        assert!(users.none());
    }
}
