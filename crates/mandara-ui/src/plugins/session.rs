//! Session plugin: power button in the bar opening the session dock; the
//! launcher button; and `session …` / `launcher …` commands (including
//! `launcher looks` / `launcher look <id>`, the ready-made launcher looks).

use gpui::{IntoElement, ParentElement, Styled, div};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_config::workflow::{LAUNCHER_LOOKS, LauncherLookPreset, LauncherPosition};
use mandara_services::SessionAction;
use serde_json::json;

use super::fn_view;
use crate::icons::icon;
use crate::state::shell;

pub const ID: &str = "session";
pub const LAUNCHER_ID: &str = "launcher";

/// Bar glyph sizes of the power and launcher buttons.
const POWER_ICON_PX: f32 = 15.0;
const LAUNCHER_ICON_PX: f32 = 16.0;

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Session"), mandara_i18n::tk("Power and session menu"),Some(Zone::Right)), |window, cx, _| {
        let bar = fn_view(None, |palette, _, _| div().flex().items_center().child(icon("power", crate::style::bar_icon(POWER_ICON_PX), palette.urgent_coral)).into_any_element(), window, cx);
        PluginInstance {
            bar: Some(bar),
            on_click: Some(std::rc::Rc::new(|_, cx| {
                let shell = shell(cx);
                let open = !shell.read(cx).session_open;
                shell.update(cx, |s, cx| s.set_session(open, cx));
            })),
            on_secondary_click: Some(std::rc::Rc::new(|_, _| {
                let _ = SessionAction::Lock.run();
            })),
            ..PluginInstance::empty()
        }
    });
    r.register(PluginManifest::builtin(LAUNCHER_ID, mandara_i18n::tk("Launcher"), mandara_i18n::tk("Application launcher (Super+Space)"),Some(Zone::Left)), |window, cx, _| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let spec = &crate::state::config(cx).read(cx).get().appearance.launcher_icon;
                div().flex().items_center().child(crate::icons::any_icon(spec, crate::style::bar_icon(LAUNCHER_ICON_PX), palette.primary)).into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            on_click: Some(std::rc::Rc::new(|_, cx| {
                let shell = shell(cx);
                let open = !shell.read(cx).launcher_open;
                shell.update(cx, |s, cx| s.set_launcher(open, cx));
            })),
            on_secondary_click: Some(std::rc::Rc::new(|_, cx| {
                let shell = shell(cx);
                let open = !shell.read(cx).sidebar_visible();
                shell.update(cx, |s, cx| s.set_sidebar(open, cx));
            })),
            ..PluginInstance::empty()
        }
    });

    for (path, summary, which) in [
        ("session menu", mandara_i18n::tk("Open, close or toggle the session dock"), 0u8),
        ("launcher", mandara_i18n::tk("Open, close or toggle the launcher"), 1),
    ] {
        r.command(
            if which == 0 { ID } else { LAUNCHER_ID },
            CommandSpec::new(path, summary, move |a, cx| {
                let shell = shell(cx);
                let current = if which == 0 { shell.read(cx).session_open } else { shell.read(cx).launcher_open };
                let open = match a.str(0)? {
                    "open" => true,
                    "close" => false,
                    _ => !current,
                };
                shell.update(cx, |s, cx| if which == 0 { s.set_session(open, cx) } else { s.set_launcher(open, cx) });
                Ok(json!({ "open": open }))
            })
            .arg(ArgSpec::required("action", ArgKind::Enum, mandara_i18n::tk("what to do")).choices(&["open", "close", "toggle"]))
            .example(if which == 0 { "session menu toggle" } else { "launcher toggle" }),
        );
    }
    r.command(
        ID,
        CommandSpec::new("session run", mandara_i18n::tk("Lock, log out, sleep, hibernate, restart or shut down"), |a, _| {
            let id = a.str(0)?;
            let action = SessionAction::from_id(id).ok_or_else(|| format!("unknown action {id:?}"))?;
            if action.is_destructive() && a.get(1) != Some("confirm") {
                return Err(format!("`{id}` ends the session; repeat as `session run {id} confirm`"));
            }
            action.run()?;
            Ok(json!({ "action": action.id(), "via": action.describe() }))
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, mandara_i18n::tk("session action")).choices(&["lock", "logout", "suspend", "hibernate", "reboot", "shutdown"]))
        .arg(ArgSpec::optional("confirm", ArgKind::Enum, mandara_i18n::tk("required for logout / reboot / shutdown")).choices(&["confirm"]))
        .example("session run lock")
        .example("session run shutdown confirm"),
    );
    r.command(
        ID,
        CommandSpec::new("launcher search", mandara_i18n::tk("What the launcher shows for a query (apps, settings, commands, slash commands, skills)"), |a, cx| {
            Ok(crate::overlays::search_json(&a.rest(0), cx))
        })
        .arg(ArgSpec::required("query", ArgKind::String, mandara_i18n::tk("search text, e.g. wallpaper or /g rust")))
        .example("launcher search background"),
    );
    r.command(
        LAUNCHER_ID,
        CommandSpec::new("launcher run", mandara_i18n::tk("Activate a launcher result exactly as Enter would (--index N picks the Nth, --dry-run only reports what would run)"), |a, cx| {
            crate::overlays::run_json(&a.0, cx)
        })
        .arg(ArgSpec::required("query", ArgKind::String, mandara_i18n::tk("search text, optionally with --index N and --dry-run")))
        .example("launcher run theme mode dark --dry-run")
        .example("launcher run =2^10"),
    );
    r.command(
        LAUNCHER_ID,
        CommandSpec::new("launcher placement", mandara_i18n::tk("Where the launcher opens: in the middle of the screen or attached to the top / bottom edge at the left, middle or right (no argument: the current one)"), |a, cx| {
            let chosen = match a.get(0) {
                Some(id) => Some(LauncherPosition::from_id(id).ok_or_else(|| format!("unknown placement {id:?}"))?),
                None => None,
            };
            if let Some(position) = chosen {
                crate::state::update_config(cx, move |c| c.launcher.look.position = position);
            }
            let cfg = crate::state::config(cx).read(cx).snapshot();
            let look = &cfg.launcher.look;
            let (width, height) = look.size();
            let at_bar = !cfg.bar.position.is_vertical() && look.position.edge().is_some_and(|e| (e == mandara_config::workflow::LauncherEdge::Top) == (cfg.bar.position == mandara_config::BarPosition::Top));
            Ok(json!({
                "placement": look.position.id(),
                "attached_to_bar": at_bar,
                "align_to_button": look.align_to_button,
                "width": width,
                "max_height": height,
                "choices": LauncherPosition::IDS,
            }))
        })
        .arg(
            ArgSpec::optional("placement", ArgKind::Enum, mandara_i18n::tk("center, top-left, top-center, top-right, bottom-left, bottom-center or bottom-right"))
                .choices(&LauncherPosition::IDS),
        )
        .example("launcher placement top-left")
        .example("launcher placement center"),
    );
    r.command(
        LAUNCHER_ID,
        CommandSpec::new("launcher looks", mandara_i18n::tk("The ready-made launcher looks (Classic, Grid, Beacon) and which one is applied"), |_, cx| {
            let look = crate::state::config(cx).read(cx).get().launcher.look.clone();
            Ok(looks_json(&look))
        })
        .example("launcher looks"),
    );
    r.command(
        LAUNCHER_ID,
        CommandSpec::new("launcher look", mandara_i18n::tk("Apply a ready-made launcher look: its layout and style settings change together (no argument: the current one)"), |a, cx| {
            if let Some(id) = a.get(0) {
                let preset = LauncherLookPreset::find(id).ok_or_else(|| format!("unknown launcher look {id:?} (see launcher looks)"))?;
                crate::state::update_config(cx, move |c| preset.apply(&mut c.launcher.look));
            }
            let look = crate::state::config(cx).read(cx).get().launcher.look.clone();
            let active = LauncherLookPreset::active(&look);
            Ok(json!({
                "look": active.map_or("custom", |l| l.id),
                "style": look.style.id(),
                "layout": if look.layout == mandara_config::workflow::LauncherLayout::Grid { "grid" } else { "list" },
                "placement": look.position.id(),
                "group_results": look.group_results,
                "hints": look.hints,
                "grid_icon_size": look.grid_icon_size,
                "grid_categories": look.grid_categories,
                "beacon_width": look.beacon_width,
                "beacon_top": look.beacon_top,
            }))
        })
        .arg(ArgSpec::optional("look", ArgKind::Enum, mandara_i18n::tk("classic, grid or beacon")).choices(&LOOK_IDS))
        .example("launcher look grid")
        .example("launcher look classic"),
    );
}

/// Ids of the launcher looks, in order.
const LOOK_IDS: [&str; LAUNCHER_LOOKS.len()] = {
    let mut ids = [""; LAUNCHER_LOOKS.len()];
    let mut i = 0;
    while i < ids.len() {
        ids[i] = LAUNCHER_LOOKS[i].id;
        i += 1;
    }
    ids
};

/// `launcher looks`: every look with what it sets, the applied one marked
/// (`active` is `custom` when settings were changed after applying).
fn looks_json(look: &mandara_config::workflow::LauncherLook) -> serde_json::Value {
    let active = LauncherLookPreset::active(look).map_or("custom", |l| l.id);
    let looks: Vec<serde_json::Value> = LAUNCHER_LOOKS
        .iter()
        .map(|l| {
            json!({
                "id": l.id,
                "name": mandara_i18n::tr(l.name),
                "description": mandara_i18n::tr(l.blurb),
                "active": l.id == active,
                "style": l.style.id(),
                "layout": if l.layout == mandara_config::workflow::LauncherLayout::Grid { "grid" } else { "list" },
                "placement": l.position.id(),
                "group_results": l.group_results,
                "hints": l.hints,
                "grid_icon_size": l.grid_icon_size,
            })
        })
        .collect();
    json!({ "active": active, "looks": looks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_looks_list_marks_the_applied_one() {
        let mut look = mandara_config::workflow::LauncherLook::default();
        let v = looks_json(&look);
        assert_eq!(v["active"], "classic");
        assert_eq!(v["looks"].as_array().unwrap().len(), LAUNCHER_LOOKS.len());
        assert_eq!(LOOK_IDS, ["classic", "grid", "beacon"]);
        LAUNCHER_LOOKS[2].apply(&mut look);
        let v = looks_json(&look);
        assert_eq!(v["active"], "beacon");
        assert_eq!(v["looks"][2]["active"], true);
        assert_eq!(v["looks"][2]["style"], "beacon");
        look.hints = false;
        assert_eq!(looks_json(&look)["active"], "custom");
    }
}
