//! Sudoku (opt-in): a bar item with a drop-down panel to play in, a
//! desktop widget you play right on the desktop, and `mandara msg sudoku`.
//! One game is shared by all of them and kept across restarts
//! ([`crate::games::sudoku`]); settings in `[plugins.settings.sudoku]`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_config::desktop_cards as dc;
use mandara_config::desktop_widgets::Anchor;
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, DesktopWidgetType, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};

use super::fn_view_observing;
use crate::fonts::tabular_nums;
use crate::games::clock::format_ms;
use crate::games::sudoku::grid::Difficulty;
use crate::games::sudoku::view::{Surface, SudokuView, size as view_size};
use crate::icons::icon;

pub use crate::games::sudoku::model::ID;

const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 5.0;
/// The bar's clock redraws once a second while it runs.
const BAR_TICK: Duration = Duration::from_secs(1);
const ACTIONS: [&str; 10] = ["status", "new", "select", "digit", "note", "erase", "hint", "undo", "redo", "notes"];

/// `"r,c"` (1-based) or a cell index 0–80.
fn parse_cell(s: &str) -> Option<usize> {
    if let Some((r, c)) = s.split_once(',') {
        let (r, c) = (r.trim().parse::<usize>().ok()?, c.trim().parse::<usize>().ok()?);
        return ((1..=9).contains(&r) && (1..=9).contains(&c)).then(|| (r - 1) * 9 + c - 1);
    }
    s.trim().parse::<usize>().ok().filter(|&i| i < 81)
}

fn digit(s: Option<&str>) -> Result<u8, String> {
    s.and_then(|v| v.trim().parse::<u8>().ok()).filter(|d| (1..=9).contains(d)).ok_or_else(|| "a digit 1–9".to_string())
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Sudoku"), tk("The number puzzle with notes, hints and undo, in the bar and on the desktop"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let model = crate::games::sudoku(cx);
        let bar = fn_view_observing(
            None,
            {
                let model = model.clone();
                move |cx| cx.observe(&model, |_, _, cx| cx.notify()).detach()
            },
            |palette, window, cx| {
                let shown = crate::games::sudoku_if_live(cx).and_then(|m| {
                    let m = m.read(cx);
                    m.game.as_ref().filter(|g| m.settings.bar_time && !g.won && g.left() > 0)?;
                    Some((m.clock.elapsed_ms(Instant::now()), m.clock.running()))
                });
                if shown.is_some_and(|(_, running)| running) {
                    crate::plugins::panel_fx::redraw_in(BAR_TICK, window, cx);
                }
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon("sudoku", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface))
                    .when_some(shown, |el, (ms, _)| el.child(div().font_features(tabular_nums()).child(format_ms(ms))))
                    .into_any_element()
            },
            window,
            cx,
        );
        let (w, h) = view_size(1.0);
        let panel = cx.new(|cx| SudokuView::new(model.clone(), Surface::Panel(panel_cx), window, cx));
        // The sidebar card plays the same game.
        let sidebar = cx.new(|cx| SudokuView::new(model, Surface::Sidebar, window, cx));
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel.into(), size: Rc::new(Cell::new(size(px(w), px(h)))) }),
            sidebar: Some(sidebar.into()),
            on_click: None,
            on_secondary_click: None,
        }
    });
    r.desktop_widget(
        DesktopWidgetType::new(ID, "card", tk("Sudoku"), tk("Play Sudoku right on the desktop: click it, then type or tap"), "sudoku", || vec![dc::new_card(dc::SUDOKU, Anchor::Center)]).in_menu(),
    );
    r.command(
        ID,
        CommandSpec::new("sudoku", tk("Play Sudoku: a new game, the board, digits, notes, hints and undo"), |a, cx| {
            let action = a.get(0).unwrap_or("status");
            let value = a.get(1);
            let model = crate::games::sudoku(cx);
            let changed = model.update(cx, |m, cx| -> Result<bool, String> {
                if !m.loaded && action != "status" {
                    return Err(mandara_i18n::t("Loading…").to_string());
                }
                Ok(match action {
                    "status" => false,
                    "new" => {
                        let d = match value {
                            Some(v) => Difficulty::parse(v).ok_or_else(|| format!("unknown level {v:?} (easy, medium, hard, expert)"))?,
                            None => m.settings.difficulty,
                        };
                        let seed = a.get(2).map(|s| s.parse::<u64>().map_err(|e| format!("seed: {e}"))).transpose()?;
                        m.new_game(d, seed, cx);
                        true
                    }
                    "select" => {
                        let cell = value.and_then(parse_cell).ok_or_else(|| "select <row>,<column> (1–9) or a cell 0–80".to_string())?;
                        m.select(cell, cx);
                        true
                    }
                    "digit" => m.digit(digit(value)?, false, cx),
                    "note" => m.digit(digit(value)?, true, cx),
                    "erase" => m.erase(cx),
                    "hint" => m.hint(cx),
                    "undo" => m.undo(cx),
                    "redo" => m.redo(cx),
                    "notes" => {
                        m.toggle_notes_mode(cx);
                        true
                    }
                    other => return Err(format!("unknown action {other:?}")),
                })
            })?;
            let mut status = model.read(cx).status();
            status["changed"] = serde_json::Value::Bool(changed);
            Ok(status)
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, tk("what to do (default: status)")).choices(&ACTIONS))
        .arg(ArgSpec::optional("value", ArgKind::String, tk("a level for new, a cell (row,column) for select, a digit for digit and note")))
        .arg(ArgSpec::optional("seed", ArgKind::Integer, tk("deal this puzzle again (new only)")))
        .example("sudoku new hard")
        .example("sudoku select 5,3")
        .example("sudoku digit 7")
        .example("sudoku status"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_parse_as_row_column_or_index() {
        assert_eq!(parse_cell("1,1"), Some(0));
        assert_eq!(parse_cell("9, 9"), Some(80));
        assert_eq!(parse_cell("5,3"), Some(38));
        assert_eq!(parse_cell("40"), Some(40));
        assert_eq!(parse_cell("0,1"), None);
        assert_eq!(parse_cell("81"), None);
        assert_eq!(digit(Some("7")), Ok(7));
        assert!(digit(Some("0")).is_err());
        assert!(digit(None).is_err());
    }

    #[test]
    fn both_games_offer_a_sidebar_card() {
        for id in [ID, crate::plugins::tiles::ID] {
            assert!(crate::sidebar_tabs::SIDEBAR_WIDGETS.contains(&id), "{id}");
        }
    }

    #[test]
    fn the_panel_is_the_views_size() {
        let (w, h) = view_size(1.0);
        assert!(w >= 300.0 && h > w);
    }
}
