//! 2048 (opt-in): a bar item with a drop-down panel to play in, a desktop
//! widget you play right on the desktop, and `mandara msg 2048`. One game
//! is shared by all of them and kept across restarts
//! ([`crate::games::tiles`]); settings in `[plugins.settings.2048]`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AppContext as _, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_config::desktop_cards as dc;
use mandara_config::desktop_widgets::Anchor;
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, DesktopWidgetType, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};

use super::fn_view_observing;
use crate::fonts::tabular_nums;
use crate::games::sudoku::view::Surface;
use crate::games::tiles::board::{Dir, MAX_SIDE, MIN_SIDE};
use crate::games::tiles::view::{TilesView, size as view_size};
use crate::icons::icon;

pub use crate::games::tiles::model::ID;

const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 5.0;
const ACTIONS: [&str; 5] = ["status", "new", "move", "undo", "continue"];

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, "2048", tk("Slide and merge the tiles to reach 2048, in the bar and on the desktop"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let model = crate::games::tiles(cx);
        if panel_cx.is_card() {
            // A sidebar card: only the card (it holds the model while it lives).
            let sidebar = cx.new(|cx| TilesView::new(model, Surface::Sidebar, window, cx));
            return PluginInstance { sidebar: Some(sidebar.into()), ..PluginInstance::empty() };
        }
        let bar = fn_view_observing(
            None,
            {
                let model = model.clone();
                move |cx| Some(cx.observe(&model, |_, _, cx| cx.notify()))
            },
            |palette, _, cx| {
                let score = crate::games::tiles_if_live(cx).and_then(|m| {
                    let m = m.read(cx);
                    m.game.as_ref().filter(|g| m.settings.bar_score && g.moves > 0).map(|g| g.score)
                });
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon("game-2048", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface))
                    .when_some(score, |el, s| el.child(div().font_features(tabular_nums()).child(s.to_string())))
                    .into_any_element()
            },
            window,
            cx,
        );
        let (w, h) = view_size(1.0);
        let panel = cx.new(|cx| TilesView::new(model.clone(), Surface::Panel(panel_cx), window, cx));
        // The sidebar card plays the same game.
        let sidebar = cx.new(|cx| TilesView::new(model, Surface::Sidebar, window, cx));
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel.into(), size: Rc::new(Cell::new(size(px(w), px(h)))) }),
            sidebar: Some(sidebar.into()),
            on_click: None,
            on_secondary_click: None,
        }
    });
    // The game's model goes (saved) with its last view: `games::join_tiles`.
    r.sidebar_card(ID, mandara_plugins::CardRelease::Views);
    r.desktop_widget(
        DesktopWidgetType::new(ID, "card", "2048", tk("Play 2048 right on the desktop: click it, then use the arrows or drag"), "game-2048", || vec![dc::new_card(dc::TILES, Anchor::Center)]).in_menu(),
    );
    r.command(
        ID,
        CommandSpec::new("2048", tk("Play 2048: a new game, moves, undo and the board"), |a, cx| {
            let action = a.get(0).unwrap_or("status");
            let value = a.get(1);
            let model = crate::games::tiles(cx);
            crate::games::tiles_command_done(cx);
            let changed = model.update(cx, |m, cx| -> Result<bool, String> {
                if !m.loaded && action != "status" {
                    return Err(mandara_i18n::t("Loading…").to_string());
                }
                Ok(match action {
                    "status" => false,
                    "new" => {
                        let side = value.map(|v| v.trim().parse::<u8>().ok().filter(|s| (MIN_SIDE..=MAX_SIDE).contains(s)).ok_or_else(|| format!("a size {MIN_SIDE}–{MAX_SIDE}"))).transpose()?;
                        m.new_game(side, cx);
                        true
                    }
                    "move" => {
                        let dir = value.and_then(Dir::parse).ok_or_else(|| "move up|down|left|right".to_string())?;
                        m.slide(dir, cx)
                    }
                    "undo" => m.undo(cx),
                    "continue" => m.keep_going(cx),
                    other => return Err(format!("unknown action {other:?}")),
                })
            })?;
            let mut status = model.read(cx).status();
            status["changed"] = serde_json::Value::Bool(changed);
            Ok(status)
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, tk("what to do (default: status)")).choices(&ACTIONS))
        .arg(ArgSpec::optional("value", ArgKind::String, tk("a board size 3–6 for new, a direction for move")))
        .example("2048 move left")
        .example("2048 new 5")
        .example("2048 status"),
    );
}
