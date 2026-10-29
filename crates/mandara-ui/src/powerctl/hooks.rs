//! Event hooks in the shell: keeps [`mandara_services::hooks`] configured,
//! turns shell changes (wallpaper, palette, appearance, Wi-Fi, Bluetooth,
//! startup, app theme files) into hook events, and offers the `hooks …`
//! commands. Lock / power / idle events are emitted where they happen
//! (`lock`, [`super::battery`], [`super::idle`]).
//!
//! Also mirrors `general.offline` into [`mandara_services::offline`].

use std::path::PathBuf;
use std::time::Duration;

use gpui::App;
use mandara_config::power::HookEvent;
use mandara_config::ShellConfig;
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::hooks;
use serde_json::json;

use crate::state::config;

/// Owner id of the hook commands.
const OWNER: &str = "core";

/// What the last observed config said, to detect edges.
#[derive(Clone, PartialEq)]
struct Seen {
    hooks: mandara_config::power::HooksConfig,
    skills: Vec<mandara_config::Skill>,
    offline: bool,
    wallpaper: Option<PathBuf>,
    mode: mandara_config::ThemeMode,
}

impl Seen {
    fn of(cfg: &ShellConfig) -> Self {
        Self { hooks: cfg.hooks.clone(), skills: cfg.launcher.skills.clone(), offline: cfg.general.offline, wallpaper: cfg.wallpaper.path.clone(), mode: cfg.theme.mode }
    }
}

fn path_text(p: &Option<PathBuf>) -> String {
    p.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Installs the hooks and starts the change watchers.
pub fn init(cx: &mut App) {
    let store = config(cx);
    let first = Seen::of(store.read(cx).get());
    hooks::configure(&first.hooks, &first.skills);
    mandara_services::offline::set(first.offline);
    hooks::emit(HookEvent::Started, &[("version", env!("CARGO_PKG_VERSION").to_string())]);

    let mut seen = first;
    cx.observe(&store, move |store, cx| {
        let next = Seen::of(store.read(cx).get());
        if next == seen {
            return;
        }
        if next.hooks != seen.hooks || next.skills != seen.skills {
            hooks::configure(&next.hooks, &next.skills);
        }
        if next.offline != seen.offline {
            mandara_services::offline::set(next.offline);
        }
        if next.wallpaper != seen.wallpaper && next.wallpaper.is_some() {
            hooks::emit(HookEvent::WallpaperChanged, &[("wallpaper", path_text(&next.wallpaper)), ("previous_wallpaper", path_text(&seen.wallpaper))]);
        }
        if next.mode != seen.mode {
            let mode = if next.mode.is_dark() { "dark" } else { "light" };
            hooks::emit(HookEvent::ThemeModeChanged, &[("theme_mode", mode.to_string())]);
        }
        seen = next;
    })
    .detach();

    // The palette (any source: preset, wallpaper colors, adjustments).
    let theme = mandara_core::theme_entity(cx);
    let key = |cx: &App| {
        let p = mandara_core::theme_entity(cx).read(cx).target().clone();
        (p.name.clone(), p.primary.to_hex(), p.surface_base.to_hex(), p.is_dark)
    };
    let mut last = key(cx);
    cx.observe(&theme, move |_, cx| {
        let next = key(cx);
        if next != last {
            let (name, primary, surface, dark) = next.clone();
            hooks::emit(
                HookEvent::ColorsChanged,
                &[("theme", name.to_string()), ("primary", primary), ("surface", surface), ("theme_mode", if dark { "dark" } else { "light" }.to_string())],
            );
            last = next;
        }
    })
    .detach();

    // Wi-Fi and Bluetooth switches (edges only, never the first reading).
    let hub = crate::hub::hub(cx);
    let mut radios: Option<(bool, bool, bool, bool)> = None;
    cx.observe(&hub, move |hub, cx| {
        let h = hub.read(cx);
        let next = (h.network.available, h.network.wifi_enabled, h.bluetooth.available, h.bluetooth.powered);
        if let Some((net_ok, wifi, bt_ok, bt)) = radios {
            if net_ok && next.0 && wifi != next.1 {
                hooks::emit(if next.1 { HookEvent::WifiOn } else { HookEvent::WifiOff }, &[]);
            }
            if bt_ok && next.2 && bt != next.3 {
                hooks::emit(if next.3 { HookEvent::BluetoothOn } else { HookEvent::BluetoothOff }, &[]);
            }
        }
        radios = Some(next);
    })
    .detach();
}

/// Template keys `[hooks.template_post]` may use, matched against the
/// written file paths.
pub const TEMPLATE_KEYS: &[&str] = &["ghostty", "gtk", "qt", "kvantum", "zed", "fuzzel", "btop", "kitty", "foot", "alacritty", "wezterm", "helix", "nvim", "firefox", "vesktop", "kde"];

/// Template keys among `files` (by path).
fn rendered_keys(files: &[PathBuf]) -> Vec<&'static str> {
    TEMPLATE_KEYS.iter().copied().filter(|key| files.iter().any(|f| f.to_string_lossy().to_lowercase().contains(key))).collect()
}

/// Theme files for other apps were written: `templates-rendered` hooks and
/// each written template's `[hooks.template_post]` command.
pub fn templates_rendered(files: &[PathBuf], cx: &App) {
    if files.is_empty() {
        return;
    }
    let cfg = config(cx).read(cx).snapshot();
    let list = files.iter().map(|f| f.to_string_lossy()).collect::<Vec<_>>().join(":");
    let keys = rendered_keys(files);
    hooks::emit(HookEvent::TemplatesRendered, &[("files", list), ("templates", keys.join(","))]);
    if !cfg.hooks.enabled {
        return;
    }
    let timeout = Duration::from_secs(u64::from(cfg.hooks.timeout_s.max(1)));
    for (key, command) in &cfg.hooks.template_post {
        let matched = keys.iter().any(|k| key.to_lowercase().starts_with(k) || k.starts_with(key.to_lowercase().as_str()));
        if matched && !command.trim().is_empty() {
            let dir = mandara_config::paths::themes_cache_dir().to_string_lossy().into_owned();
            hooks::run_command("template-post", command, &[("template", key.clone()), ("theme_dir", dir)], timeout);
        }
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new("hooks list", tk("Event hooks: every event with its rules (JSON)"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let events: Vec<_> = HookEvent::ALL
                .iter()
                .map(|e| {
                    let rules: Vec<_> = cfg.hooks.rules.iter().filter(|r| r.event == *e).map(|r| json!({ "command": r.command, "enabled": r.enabled, "timeout_s": cfg.hooks.timeout_of(r) })).collect();
                    json!({ "event": e.id(), "waits": e.blocks(), "rules": rules })
                })
                .collect();
            Ok(json!({ "enabled": cfg.hooks.enabled, "timeout_s": cfg.hooks.timeout_s, "events": events, "template_post": cfg.hooks.template_post, "log": hooks::log_path() }))
        })
        .example("hooks list"),
    );
    r.command(
        OWNER,
        CommandSpec::new("hooks status", tk("Recent hook runs with exit codes and durations (JSON)"), |_, _| Ok(json!(hooks::history()))).example("hooks status"),
    );
    let ids: &'static [&'static str] = Box::leak(HookEvent::ALL.iter().map(|e| e.id()).collect::<Vec<_>>().into_boxed_slice());
    r.command(
        OWNER,
        CommandSpec::new_output("hooks test", tk("Run an event's hooks now with MANDARA_TEST=1 and report the results"), |a, _| {
            let event = HookEvent::from_id(a.str(0)?).ok_or_else(|| mandara_i18n::t("Unknown event").to_string())?;
            let rule = a.get(1).map(|_| a.int(1)).transpose()?;
            Ok(super::pending(move || {
                let vars = [("test", "1".to_string())];
                let runs = match rule {
                    Some(i) => vec![hooks::test_rule(event, usize::try_from(i).map_err(|_| mandara_i18n::t("No such hook").to_string())?, &vars)?],
                    None => hooks::emit_and_wait(event, &vars),
                };
                Ok(json!({ "event": event.id(), "runs": runs }))
            }))
        })
        .arg(ArgSpec::required("event", ArgKind::Enum, tk("event name, e.g. locked or wallpaper-changed")).choices(ids))
        .arg(ArgSpec::optional("rule", ArgKind::Integer, tk("only this rule (its position in [[hooks.rules]], from 0), even when off")))
        .example("hooks test wallpaper-changed")
        .example("hooks test locked 2"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_keys_match_written_paths() {
        let files = [PathBuf::from("/home/u/.cache/mandara/themes/ghostty/mandara"), PathBuf::from("/home/u/.config/gtk-4.0/gtk.css")];
        assert_eq!(rendered_keys(&files), ["ghostty", "gtk"]);
        assert!(rendered_keys(&[]).is_empty());
    }
}
