//! Shell-level commands: panels, sidebar, themes, layout, config, plugins.

use mandara_config::{BarLayout, ThemeMode};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_theme::{SchemeVariant, ThemeSource, catalog};
use serde_json::json;

use crate::state::{config, reload_config_from_disk, shell, update_config};

/// `mandara msg studio <section>`: every slug of the launcher's Studio
/// entries (`crate::overlays::STUDIO_SECTIONS`, checked by a test).
pub const STUDIO_SECTION_ARG: &str = mandara_i18n::tk("a section (home, themes, wallpaper, desktop-widgets, typography, motion, bar, sidebar, dock, launcher, window-switcher, notifications, on-screen-displays, displays, audio, network, power, input, keyboard-shortcuts, date, applications, users, login-screen, lock-appearance, privacy, plugins, compositor, general, profiles, performance, maintenance, about), power/<part>, plugins/<id>[/settings] or plugins/<installed|marketplace|local>");

/// Lets the `restart` reply reach the client before the process exits.
const RESTART_FLUSH: std::time::Duration = std::time::Duration::from_millis(150);
/// Exit code asking the supervisor to restart the shell.
const SUPERVISOR_RESTART_CODE: i32 = 75;
/// Wallpaper seed colors extracted for `theme seeds` / `theme wallpaper`.
const SEED_COUNT: usize = 4;
/// Seed proportions are reported to three decimals.
const PROPORTION_SCALE: f32 = 1000.0;

fn open_close_toggle() -> ArgSpec {
    ArgSpec::required("action", ArgKind::Enum, mandara_i18n::tk("what to do")).choices(&["open", "close", "toggle"])
}

/// Writes `new` at JSON pointer `ptr`. Missing keys are created only inside
/// free-form tables (`/plugins/settings/...`); elsewhere the path must exist.
fn set_pointer(root: &mut serde_json::Value, ptr: &str, new: serde_json::Value) -> Result<(), String> {
    if let Some(slot) = root.pointer_mut(ptr) {
        *slot = new;
        return Ok(());
    }
    if !ptr.starts_with("/plugins/settings/") {
        return Err(format!("no setting at {ptr}"));
    }
    let mut node = root;
    let parts: Vec<String> = ptr.trim_start_matches('/').split('/').map(|p| p.replace("~1", "/").replace("~0", "~")).collect();
    for (i, part) in parts.iter().enumerate() {
        let map = node.as_object_mut().ok_or_else(|| format!("{ptr}: not a table"))?;
        if i + 1 == parts.len() {
            map.insert(part.clone(), new);
            return Ok(());
        }
        node = map.entry(part.clone()).or_insert_with(|| serde_json::Value::Object(Default::default()));
    }
    Err(format!("no setting at {ptr}"))
}

pub fn register(r: &mut PluginRegistry) {
    let core = "core";
    // Debug builds only: proves a panicking plugin command is isolated.
    #[cfg(debug_assertions)]
    r.command(core, CommandSpec::new("debug panic", mandara_i18n::tk("Panic inside a command (debug builds; tests fault isolation)"), |_, _| -> Result<serde_json::Value, String> {
        panic!("deliberate test panic")
    }));
    r.command(
        core,
        CommandSpec::new("restart", mandara_i18n::tk("Restart the shell process (config and plugins reload from scratch)"), |_, cx| {
            // Let the reply flush, then hand over: under the supervisor exit
            // with its restart code; standalone, re-exec in place.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(RESTART_FLUSH).await;
                if std::env::var_os("MANDARA_SUPERVISED").is_some() {
                    std::process::exit(SUPERVISOR_RESTART_CODE);
                }
                use std::os::unix::process::CommandExt as _;
                let err = std::process::Command::new(mandara_config::paths::self_exe()).arg("run").exec();
                tracing::error!("restart failed: {err}");
            })
            .detach();
            Ok(json!({ "restarting": true }))
        })
        .example("restart"),
    );
    r.command(core, CommandSpec::new("ping", mandara_i18n::tk("Check that the shell is running"), |_, _| Ok(json!("pong"))).example("ping"));
    r.command(
        core,
        CommandSpec::new("shell state", mandara_i18n::tk("What is open: panel, sidebar, launcher, power menu, wallpaper picker; whether the launcher opens on hover"), |_, cx| {
            let s = shell(cx).read(cx);
            let cfg = config(cx).read(cx).snapshot();
            Ok(json!({
                "panel": s.panel,
                "sidebar": s.sidebar_visible(),
                "sidebar_pinned": cfg.sidebar.pinned(),
                "sidebar_mode": cfg.sidebar.mode,
                "launcher": s.launcher_open,
                "session_menu": s.session_open,
                "wallpaper_picker": s.wallpaper_open,
                "dock": cfg.dock_enabled(),
                "launcher_on_hover": cfg.launcher_hover_enabled(),
            }))
        })
        .example("shell state"),
    );

    r.command(
        core,
        CommandSpec::new("panel", mandara_i18n::tk("Open, close or toggle a plugin's panel"), |a, cx| {
            let action = a.str(0)?.to_string();
            let id = a.get(1).map(str::to_string);
            let shell = shell(cx);
            match (action.as_str(), id) {
                ("close", _) => shell.update(cx, |s, cx| s.set_panel(None, cx)),
                ("open", Some(id)) => shell.update(cx, |s, cx| s.set_panel(Some(id), cx)),
                ("toggle", Some(id)) => shell.update(cx, |s, cx| s.toggle_panel(&id, cx)),
                _ => return Err("usage: panel <open|toggle> <plugin-id> | panel close".into()),
            }
            Ok(json!({ "panel": shell.read(cx).panel }))
        })
        .arg(open_close_toggle())
        .arg(ArgSpec::optional("plugin", ArgKind::String, mandara_i18n::tk("plugin id, e.g. clock, media, network")))
        .example("panel toggle media")
        .example("panel close"),
    );

    r.command(
        core,
        CommandSpec::new("sidebar", mandara_i18n::tk("Open, close or toggle the left sidebar"), |a, cx| {
            let shell = shell(cx);
            let open = match a.str(0)? {
                "open" => true,
                "close" => false,
                _ => !shell.read(cx).sidebar_visible(),
            };
            shell.update(cx, |s, cx| s.set_sidebar(open, cx));
            Ok(json!({ "sidebar": open }))
        })
        .arg(open_close_toggle())
        .example("sidebar toggle"),
    );

    r.command(
        core,
        CommandSpec::new("sidebar pin", mandara_i18n::tk("Pin, unpin or toggle-pin the sidebar (pinned pushes windows aside)"), |a, cx| {
            let pinned = config(cx).read(cx).get().sidebar.pinned();
            let next = match a.str(0)? {
                "on" => true,
                "off" => false,
                _ => !pinned,
            };
            update_config(cx, |c| c.sidebar.set_pinned(next));
            Ok(json!({ "pinned": next }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, mandara_i18n::tk("pin state")).choices(&["on", "off", "toggle"]))
        .example("sidebar pin toggle"),
    );
    r.command(
        core,
        CommandSpec::new("theme set", mandara_i18n::tk("Switch to a theme preset (OKLCH crossfade)"), |a, cx| {
            let id = a.rest(0);
            let theme = catalog::find(&id).ok_or_else(|| format!("unknown theme {id:?}; see `theme list`"))?;
            let preset = theme.id.clone();
            // An explicit theme stops following the wallpaper's colors
            // (else the wallpaper sync switches straight back).
            update_config(cx, |c| {
                c.theme.source = ThemeSource::Preset { preset };
                c.wallpaper.match_colors = false;
            });
            Ok(json!({ "theme": theme.id, "name": theme.name }))
        })
        .arg(ArgSpec::required("theme", ArgKind::String, mandara_i18n::tk("theme id or name (see `theme list`)")))
        .example("theme set montaza-twilight"),
    );
    r.command(
        core,
        CommandSpec::new("theme next", mandara_i18n::tk("Cycle to the next preset"), |_, cx| {
            let next = match &config(cx).read(cx).get().theme.source {
                ThemeSource::Preset { preset } => catalog::next_id(preset),
                _ => catalog::DEFAULT_THEME.to_string(),
            };
            update_config(cx, |c| {
                c.theme.source = ThemeSource::Preset { preset: next.clone() };
                c.wallpaper.match_colors = false;
            });
            Ok(json!({ "theme": next }))
        }),
    );
    r.command(
        core,
        CommandSpec::new("theme list", mandara_i18n::tk("List themes (built-in and ~/.config/mandara/themes)"), |_, cx| {
            let current = match &config(cx).read(cx).get().theme.source {
                ThemeSource::Preset { preset } => Some(preset.clone()),
                _ => None,
            };
            Ok(json!(catalog::all()
                .iter()
                .map(|t| {
                    let report = catalog::ContrastReport::of(&t.palette(true));
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "description": t.description,
                        "signature": t.signature,
                        "origin": t.origin,
                        "light_variant": if t.has_mode(false) { "authored" } else { "derived" },
                        "contrast_ok": report.passes(),
                        "active": current.as_deref() == Some(t.id.as_str()),
                    })
                })
                .collect::<Vec<_>>()))
        }),
    );
    r.command(
        core,
        CommandSpec::new_output("theme seeds", mandara_i18n::tk("Material You seed colors extracted from an image (default: the wallpaper)"), |a, cx| {
            let path = match a.get(0) {
                Some(p) => mandara_config::paths::expand_tilde(&a.rest(0).replace(p, p)),
                None => config(cx).read(cx).get().wallpaper.path.clone().ok_or("no wallpaper set; pass an image path")?,
            };
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let result = mandara_theme::material::extract_seeds(&path, SEED_COUNT)
                    .map(|seeds| {
                        serde_json::to_string(&seeds.iter().map(|s| json!({
                            "color": s.color.to_hex(),
                            "proportion": (s.proportion * PROPORTION_SCALE).round() / PROPORTION_SCALE,
                            "hue": s.hue.round(),
                            "chroma": s.chroma.round(),
                        })).collect::<Vec<_>>()).unwrap_or_default()
                    })
                    .map_err(|e| e.to_string());
                let _ = tx.send_blocking(result);
            });
            Ok(mandara_plugins::Output::Pending(rx))
        })
        .arg(ArgSpec::optional("path", ArgKind::String, mandara_i18n::tk("image file")))
        .example("theme seeds"),
    );
    r.command(
        core,
        CommandSpec::new("theme wallpaper", mandara_i18n::tk("Use Material You colors from the wallpaper (optionally choosing style and seed)"), |a, cx| {
            let variant: SchemeVariant = match a.get(0) {
                Some(v) => serde_json::from_value(json!(v)).map_err(|_| format!("unknown style {v:?}"))?,
                None => SchemeVariant::default(),
            };
            let seed = a.get(1).map(|s| s.parse::<usize>().map_err(|_| "seed must be 0-3")).transpose()?.unwrap_or(0).min(SEED_COUNT - 1);
            let path = config(cx).read(cx).get().wallpaper.path.clone().ok_or("no wallpaper set")?;
            update_config(cx, |c| {
                if let ThemeSource::Preset { preset } = &c.theme.source {
                    c.theme.last_preset = Some(preset.clone());
                }
                c.wallpaper.match_colors = true;
                c.theme.source = ThemeSource::Wallpaper { path, variant, seed };
            });
            Ok(json!({ "variant": variant, "seed": seed }))
        })
        .arg(ArgSpec::optional("style", ArgKind::Enum, mandara_i18n::tk("scheme style")).choices(&["tonal-spot", "vibrant", "expressive", "fidelity", "fruit-salad", "monochrome"]))
        .arg(ArgSpec::optional("seed", ArgKind::Integer, mandara_i18n::tk("which extracted color leads, 0-3 (see `theme seeds`)")))
        .example("theme wallpaper vibrant 1"),
    );
    r.command(
        core,
        CommandSpec::new_output("theme export", mandara_i18n::tk("Write the current palette to Ghostty, GTK, Qt, Zed, Fuzzel and btop theme files"), |_, cx| {
            let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
            let task = crate::state::export_app_themes(palette, cx);
            let (tx, rx) = async_channel::bounded(1);
            cx.spawn(async move |_| {
                let files = task.await;
                let _ = tx.send(Ok(files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join("\n"))).await;
            })
            .detach();
            Ok(mandara_plugins::Output::Pending(rx))
        })
        .example("theme export"),
    );
    r.command(
        core,
        CommandSpec::new("theme current", mandara_i18n::tk("Describe the active theme"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let theme = mandara_core::theme_entity(cx);
            let palette = theme.read(cx).target().clone();
            Ok(json!({
                "name": palette.name,
                "mode": cfg.theme.mode,
                "source": cfg.theme.source,
                "primary": palette.primary.to_hex(),
                "surface": palette.surface_base.to_hex(),
            }))
        }),
    );
    r.command(
        core,
        CommandSpec::new("theme mode", mandara_i18n::tk("Switch between light and dark appearance"), |a, cx| {
            let current = config(cx).read(cx).get().theme.mode;
            let mode = match a.str(0)? {
                "light" => ThemeMode::Light,
                "dark" => ThemeMode::Dark,
                _ => current.toggled(),
            };
            update_config(cx, |c| c.theme.mode = mode);
            Ok(json!({ "mode": mode }))
        })
        .arg(ArgSpec::required("mode", ArgKind::Enum, mandara_i18n::tk("appearance")).choices(&["light", "dark", "toggle"]))
        .example("theme mode toggle"),
    );

    r.command(
        core,
        CommandSpec::new("bar layout", mandara_i18n::tk("Set the bar layout"), |a, cx| {
            let current = config(cx).read(cx).get().bar.layout;
            let layout = match a.str(0)? {
                "edge-frame" => BarLayout::MinimalEdge,
                "glass-pills" => BarLayout::FloatingPills,
                "island" => BarLayout::ContiguousIsland,
                "hairline" => BarLayout::Hairline,
                "micro" => BarLayout::Micro,
                _ => current.next(),
            };
            update_config(cx, |c| c.bar.layout = layout);
            Ok(json!({ "layout": layout, "name": layout.display_name() }))
        })
        .arg(ArgSpec::required("layout", ArgKind::Enum, mandara_i18n::tk("layout")).choices(&["edge-frame", "glass-pills", "island", "hairline", "micro", "next"]))
        .example("bar layout hairline"),
    );

    r.command(
        core,
        CommandSpec::new("bar position", mandara_i18n::tk("Put the bar on a screen edge"), |a, cx| {
            let position = match a.str(0)? {
                "top" => mandara_config::BarPosition::Top,
                "bottom" => mandara_config::BarPosition::Bottom,
                "left" => mandara_config::BarPosition::Left,
                "right" => mandara_config::BarPosition::Right,
                other => return Err(format!("unknown edge {other:?}")),
            };
            update_config(cx, |c| c.bar.position = position);
            Ok(json!({ "position": position }))
        })
        .arg(ArgSpec::required("edge", ArgKind::Enum, mandara_i18n::tk("screen edge")).choices(&["top", "bottom", "left", "right"]))
        .example("bar position left"),
    );

    r.command(
        core,
        CommandSpec::new("look list", mandara_i18n::tk("List the one-click looks"), |_, cx| {
            let active = config(cx).read(cx).get().looks.active.clone();
            let looks: Vec<_> = mandara_config::looks::LOOKS.iter().map(|l| json!({ "id": l.id, "name": l.name, "active": l.id == active })).collect();
            Ok(json!({ "looks": looks, "active": active }))
        }),
    );
    r.command(
        core,
        CommandSpec::new("look apply", mandara_i18n::tk("Apply a look (your setup is saved first so it can be restored)"), |a, cx| {
            let id = a.str(0)?.to_string();
            let look = mandara_config::looks::find(&id).ok_or_else(|| format!("no look {id:?}"))?;
            let wallpaper_colors = a.get(1) == Some("wallpaper-colors");
            let cfg = config(cx).read(cx).snapshot();
            if let Some(before) = mandara_config::looks::before_snapshot(&cfg, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()) {
                mandara_config::profiles::save(&before).map_err(|e| format!("saving your setup: {e:#}"))?;
            }
            let next = look.apply(&cfg, mandara_config::looks::LookOptions { wallpaper_colors });
            update_config(cx, move |c| *c = next);
            Ok(json!({ "applied": look.id, "restore_profile": mandara_config::looks::BEFORE_PROFILE }))
        })
        .arg(ArgSpec::required("look", ArgKind::String, mandara_i18n::tk("look id (see look list)")))
        .arg(ArgSpec::optional("colors", ArgKind::Enum, mandara_i18n::tk("palette source")).choices(&["own", "wallpaper-colors"]))
        .example("look apply night-frame"),
    );
    r.command(
        core,
        CommandSpec::new("look restore", mandara_i18n::tk("Return to your setup from before the first look"), |_, cx| {
            let before = mandara_config::profiles::find(mandara_config::looks::BEFORE_PROFILE).ok_or("no saved setup to restore")?;
            let cfg = config(cx).read(cx).snapshot();
            let next = mandara_config::looks::restore(&cfg, &before).map_err(|e| format!("{e:#}"))?;
            update_config(cx, move |c| *c = next);
            Ok(json!({ "restored": true }))
        }),
    );

    r.command(
        core,
        CommandSpec::new("config reload", mandara_i18n::tk("Re-read config.toml from disk"), |_, cx| {
            let changed = reload_config_from_disk(cx)?;
            Ok(json!({ "changed": changed }))
        }),
    );
    r.command(
        core,
        CommandSpec::new("config show", mandara_i18n::tk("Print the live configuration"), |_, cx| {
            Ok(serde_json::to_value(&*config(cx).read(cx).snapshot()).unwrap_or_default())
        }),
    );

    r.command(
        core,
        CommandSpec::new("config get", mandara_i18n::tk("Read a setting by JSON pointer (whole config when omitted)"), |a, cx| {
            let value = serde_json::to_value(&*config(cx).read(cx).snapshot()).map_err(|e| e.to_string())?;
            match a.get(0) {
                None | Some("") | Some("/") => Ok(value),
                Some(ptr) => value.pointer(ptr).cloned().ok_or_else(|| format!("no setting at {ptr}")),
            }
        })
        .arg(ArgSpec::optional("pointer", ArgKind::String, mandara_i18n::tk("JSON pointer, e.g. /bar/height")))
        .example("config get /theme/mode"),
    );
    r.command(
        core,
        CommandSpec::new("config set", mandara_i18n::tk("Change one setting live (value is JSON; bare words are strings)"), |a, cx| {
            let ptr = a.str(0)?.to_string();
            let raw = a.rest(1);
            let new: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
            let mut value = serde_json::to_value(&*config(cx).read(cx).snapshot()).map_err(|e| e.to_string())?;
            set_pointer(&mut value, &ptr, new.clone())?;
            let next: mandara_config::ShellConfig = serde_json::from_value(value).map_err(|e| format!("invalid value for {ptr}: {e}"))?;
            update_config(cx, |c| *c = next);
            Ok(json!({ "path": ptr, "value": new }))
        })
        .arg(ArgSpec::required("pointer", ArgKind::String, mandara_i18n::tk("JSON pointer, e.g. /bar/height")))
        .arg(ArgSpec::required("value", ArgKind::String, mandara_i18n::tk("JSON value")))
        .example("config set /bar/height 30")
        .example("config set /theme/mode light"),
    );
    r.command(
        core,
        CommandSpec::new("config apply", mandara_i18n::tk("Replace the live configuration with a full JSON document (used by Studio)"), |a, cx| {
            let next: mandara_config::ShellConfig = serde_json::from_str(&a.rest(0)).map_err(|e| format!("invalid config: {e}"))?;
            update_config(cx, |c| *c = next);
            Ok(json!({ "applied": true }))
        })
        .arg(ArgSpec::required("json", ArgKind::String, mandara_i18n::tk("the whole config as JSON"))),
    );
    r.command(core, CommandSpec::new("models list", mandara_i18n::tk("Downloadable models (OCR) and whether they're installed"), |_, _| Ok(json!(mandara_services::models::status()))));
    r.command(
        core,
        CommandSpec::new_output("models download", mandara_i18n::tk("Download a model by id, or all of them"), |a, _| {
            let which = a.get(0).unwrap_or("all").to_string();
            let models: Vec<&'static mandara_services::models::Model> = match which.as_str() {
                "all" => mandara_services::models::MODELS.iter().collect(),
                id => vec![mandara_services::models::find(id).ok_or_else(|| format!("unknown model {id:?}"))?],
            };
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let result = models
                    .iter()
                    .try_for_each(|m| m.download(&|_, _| {}))
                    .map(|()| format!("installed: {}", models.iter().map(|m| m.id).collect::<Vec<_>>().join(", ")));
                let _ = tx.send_blocking(result);
            });
            Ok(mandara_plugins::Output::Pending(rx))
        })
        .arg(ArgSpec::optional("id", ArgKind::String, mandara_i18n::tk("model id from `models list` (default: all)")))
        .example("models download ocr-arabic"),
    );
    r.command(
        core,
        CommandSpec::new("models remove", mandara_i18n::tk("Delete a downloaded model"), |a, _| {
            let id = a.get(0).ok_or("expected a model id")?;
            let m = mandara_services::models::find(id).ok_or_else(|| format!("unknown model {id:?}"))?;
            m.remove()?;
            Ok(json!({ "removed": id }))
        })
        .arg(ArgSpec::required("id", ArgKind::String, mandara_i18n::tk("model id"))),
    );
    r.command(
        core,
        CommandSpec::new("info", mandara_i18n::tk("Shell process facts: pid, version, compositor"), |_, cx| {
            let compositor = crate::hub::hub(cx).read(cx).compositor.as_ref().map(|c| format!("{:?}", c.kind()));
            let build = mandara_core::build_info(cx);
            Ok(json!({ "pid": std::process::id(), "version": build.version, "build": build.long, "components": build.components.into_iter().collect::<std::collections::BTreeMap<_, _>>(), "compositor": compositor }))
        })
        .example("info"),
    );

    r.command(
        core,
        CommandSpec::new("plugins reload", mandara_i18n::tk("Hot-reload plugins from the plugin folders (also happens automatically)"), |_, cx| {
            Ok(json!(crate::plugins::external::sync(cx)))
        }),
    );
    r.command(
        core,
        CommandSpec::new("plugins list", mandara_i18n::tk("List plugins with origin and enablement"), |_, cx| {
            let registry = cx.global::<PluginRegistry>();
            let cfg = config(cx).read(cx).snapshot();
            let mut list: Vec<_> = registry
                .manifests()
                .map(|m| {
                    let mut entry = json!({
                        "id": m.id,
                        "name": m.name,
                        "description": m.description,
                        "origin": m.origin,
                        "enabled": registry.is_enabled(&m.id, &cfg.plugins),
                        "removed": cfg.plugins.is_removed(&m.id),
                        "essential": m.origin == mandara_plugins::PluginOrigin::Builtin && mandara_plugins::lifecycle::essential_reason(&m.id).is_some(),
                    });
                    // Third-party runtime state: running / failed / disabled
                    // (with the error), or "not-loaded" until first shown.
                    if matches!(m.origin, mandara_plugins::PluginOrigin::ThirdParty { .. }) {
                        entry["runtime"] = crate::plugins::external::load_state(&m.id).map(|s| json!(s)).unwrap_or_else(|| json!({ "state": "not-loaded" }));
                    }
                    entry
                })
                .collect();
            let third_party = mandara_plugins::discover_third_party(&mandara_config::paths::config_dir().join("plugins"));
            list.extend(third_party.into_iter().map(|m| json!({
                "id": m.id,
                "name": m.name,
                "description": m.description,
                "origin": m.origin,
                "enabled": cfg.plugins.enabled.contains(&m.id),
                "note": "third-party runtime (WASM/WIT) loads these from the plugin directory",
            })));
            Ok(json!(list))
        }),
    );
    r.command(
        core,
        CommandSpec::new_output("plugins vet", mandara_i18n::tk("Run the marketplace's compliance check on a plugin folder: ok, or broken / malicious / placeholder with the reason"), |a, cx| {
            let dir = std::path::PathBuf::from(a.rest(0));
            let reserved = mandara_plugins::vet::builtin_ids(cx.global::<PluginRegistry>());
            // Off the UI thread: it compiles and runs the component.
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let verdict = mandara_plugins::vet::vet(&dir, &reserved.iter().map(String::as_str).collect::<Vec<_>>(), None);
                let _ = tx.send_blocking(Ok(mandara_plugins::vet::verdict_json(&verdict).to_string()));
            });
            Ok(rx.into())
        })
        .arg(ArgSpec::required("dir", ArgKind::String, mandara_i18n::tk("the plugin folder (plugin.toml and plugin.wasm)")))
        .example("plugins vet ~/.local/share/mandara/plugins-dev/my-plugin"),
    );
    use crate::plugins::lifecycle::Action;
    for (path, summary, action) in [
        ("plugins enable", mandara_i18n::tk("Turn a plugin on (a removed Mandara plugin is restored)"), Action::Enable),
        ("plugins disable", mandara_i18n::tk("Turn a plugin off; it stops everything it runs"), Action::Disable),
        ("plugins remove", mandara_i18n::tk("Remove a Mandara plugin: off, out of the bar and sidebar, listed under Removed"), Action::Remove),
        ("plugins restore", mandara_i18n::tk("Bring back a removed Mandara plugin where it was"), Action::Restore),
    ] {
        r.command(
            core,
            CommandSpec::new(path, summary, move |a, cx| {
                let id = a.str(0)?.to_string();
                crate::plugins::lifecycle::apply(&id, action, cx)?;
                let plugins = &config(cx).read(cx).get().plugins;
                let enabled = cx.global::<PluginRegistry>().is_enabled(&id, plugins) || (cx.global::<PluginRegistry>().get(&id).is_none() && plugins.enabled.contains(&id));
                Ok(json!({ "plugin": id, "enabled": enabled, "removed": plugins.is_removed(&id) }))
            })
            .arg(ArgSpec::required("plugin", ArgKind::String, mandara_i18n::tk("plugin id"))),
        );
    }

    r.command(
        core,
        CommandSpec::new("studio", mandara_i18n::tk("Open Mandara Studio, optionally on a section"), |a, cx| {
            // Words after the section (`--demo`) go to `mandara studio` too.
            let words = a.rest(0);
            let section = a.get(0).map(str::to_string);
            crate::state::open_studio_at((!words.trim().is_empty()).then_some(words), cx);
            Ok(json!({ "opened": section.unwrap_or_else(|| "home".into()) }))
        })
        .arg(ArgSpec::optional("section", ArgKind::String, STUDIO_SECTION_ARG))
        .example("studio themes"),
    );

    r.command(core, CommandSpec::new("quit", mandara_i18n::tk("Quit Mandara Shell"), |_, cx| {
        cx.quit();
        Ok(json!("bye"))
    }));
}
