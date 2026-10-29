//! Compositor integration commands and live sync of the generated drop-in
//! (layer rules, window/tag rules, keybinds, palette border colors).

use std::cell::RefCell;
use std::rc::Rc;

use gpui::App;
use mandara_config::compositor::{BorderColors, Target, include_installed, install_include};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use serde_json::{Value, json};

use super::json;
use crate::hub::hub;
use crate::state::config;

pub const ID: &str = "compositor";

/// Border colors derived from the active palette's target.
pub fn border_colors(cx: &App) -> BorderColors {
    let p = mandara_core::theme_entity(cx).read(cx).target().clone();
    BorderColors {
        active: p.primary.to_hex(),
        active_end: p.accent_gradient_end.to_hex(),
        inactive: p.surface_container_high.to_hex(),
        urgent: p.urgent_coral.to_hex(),
    }
}

/// Writes the generated file for the running compositor and reloads it when
/// the user's config includes it.
pub fn sync(cx: &App) -> Result<Value, String> {
    let (backend, cfg, borders) = sync_inputs(cx)?;
    sync_with(backend.as_ref(), &cfg, borders)
}

type SyncInputs = (std::sync::Arc<dyn mandara_compositor::CompositorBackend>, std::sync::Arc<mandara_config::ShellConfig>, Option<BorderColors>);

fn sync_inputs(cx: &App) -> Result<SyncInputs, String> {
    let backend = hub(cx).read(cx).compositor.clone().ok_or("no supported compositor is running")?;
    let cfg = config(cx).read(cx).snapshot();
    let borders = cfg.integrations.compositor_borders.then(|| border_colors(cx));
    Ok((backend, cfg, borders))
}

/// [`sync`] without the app: file writes and the compositor reload are
/// blocking, so live re-syncs run this off the UI thread.
fn sync_with(backend: &dyn mandara_compositor::CompositorBackend, cfg: &mandara_config::ShellConfig, borders: Option<BorderColors>) -> Result<Value, String> {
    let (path, emitted) = backend.sync_managed_config(&cfg.effective_compositor(), borders).map_err(|e| format!("{e:#}"))?;
    let target = backend.target();
    let included = include_installed(target);
    // X11 and older toolkit apps read `~/.icons/default` (only a file
    // Mandara wrote is updated, and it goes when the theme is unset).
    if let Err(err) = mandara_config::input::sync_default_cursor(cfg.compositor.input.cursor.theme.as_deref()) {
        tracing::warn!("default cursor: {err}");
    }
    if included {
        backend.reload_config().map_err(|e| format!("reload: {e:#}"))?;
        // Hyprland's `env = XCURSOR_*` lines only reach apps started later;
        // the cursor on screen changes through `setcursor`.
        let cursor = &cfg.compositor.input.cursor;
        if let Err(err) = backend.apply_cursor(cursor.theme.as_deref(), cursor.size) {
            tracing::warn!("live cursor: {err:#}");
        }
    }
    Ok(json!({
        "compositor": backend.kind(),
        "file": path,
        "included": included,
        "hint": (!included).then(|| format!("run `mandara msg compositor install-include` to add `{}` to {}", target.include_line(), target.user_config().display())),
        "skipped_binds": emitted.skipped,
    }))
}

/// Live re-syncs started / the newest one applied (ordering guard).
static SYNC_STARTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SYNC_APPLIED: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

/// Initial sync plus re-sync whenever the palette target or compositor
/// config changes.
pub fn init(cx: &mut App) {
    // The watcher below is installed even when management is off, so
    // turning "Manage the compositor" on later syncs without a restart.
    if config(cx).read(cx).get().compositor.manage {
        initial_sync(cx);
    }
    watch(cx);
}

fn initial_sync(cx: &App) {
    match sync(cx) {
        Ok(report) => {
            if let Some(hint) = report.get("hint").and_then(Value::as_str) {
                tracing::warn!("compositor drop-in written but not included yet: {hint}");
            }
            for skipped in report.get("skipped_binds").and_then(Value::as_array).into_iter().flatten() {
                tracing::info!("keybind {skipped}");
            }
        }
        Err(err) => tracing::warn!("compositor sync: {err}"),
    }
}

/// What the drop-in is made of, compared to skip needless re-syncs.
/// Keyboard-layout labels are bar-only, so editing one never rewrites the
/// drop-in or reloads the compositor.
fn sync_key(cfg: &mandara_config::ShellConfig) -> String {
    let mut compositor = cfg.effective_compositor().into_owned();
    compositor.input.keyboard.labels.clear();
    serde_json::to_string(&compositor).unwrap_or_default()
}

fn watch(cx: &mut App) {
    let last: Rc<RefCell<(Option<BorderColors>, String)>> = Rc::default();
    let snapshot = |cx: &App| {
        let cfg = config(cx).read(cx).snapshot();
        (cfg.integrations.compositor_borders.then(|| border_colors(cx)), sync_key(&cfg))
    };
    *last.borrow_mut() = snapshot(cx);
    // Cheap change key — the config generation (its snapshot's address)
    // and the palette target's border roles — checked before the costly
    // serialize-and-compare: the theme entity notifies on every frame of a
    // palette tween.
    let key = |cx: &App| {
        let generation = std::sync::Arc::as_ptr(&config(cx).read(cx).snapshot()) as usize;
        let p = mandara_core::theme_entity(cx).read(cx).target();
        (generation, [p.primary, p.accent_gradient_end, p.surface_container_high, p.urgent_coral])
    };
    let last_key = RefCell::new(key(cx));
    let on_change = move |cx: &mut App| {
        let now_key = key(cx);
        if *last_key.borrow() == now_key {
            return;
        }
        *last_key.borrow_mut() = now_key;
        if !config(cx).read(cx).get().compositor.manage {
            return;
        }
        let now = snapshot(cx);
        if *last.borrow() != now {
            *last.borrow_mut() = now;
            match sync_inputs(cx) {
                Ok((backend, cfg, borders)) => {
                    let generation = SYNC_STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    cx.background_executor()
                        .spawn(async move {
                            // Background syncs may finish out of order: an
                            // older one never overwrites a newer drop-in.
                            let mut applied = SYNC_APPLIED.lock().unwrap_or_else(|e| e.into_inner());
                            if generation < *applied {
                                return;
                            }
                            *applied = generation;
                            if let Err(err) = sync_with(backend.as_ref(), &cfg, borders) {
                                tracing::warn!("compositor sync: {err}");
                            }
                        })
                        .detach();
                }
                Err(err) => tracing::warn!("compositor sync: {err}"),
            }
        }
    };
    let on_change = Rc::new(on_change);
    let (a, b) = (on_change.clone(), on_change);
    cx.observe(&mandara_core::theme_entity(cx), move |_, cx| a(cx)).detach();
    cx.observe(&config(cx), move |_, cx| b(cx)).detach();
}

fn backend(cx: &App) -> Result<std::sync::Arc<dyn mandara_compositor::CompositorBackend>, String> {
    hub(cx).read(cx).compositor.clone().ok_or_else(|| "no supported compositor is running".to_string())
}

pub fn register(r: &mut PluginRegistry) {
    r.command(ID, CommandSpec::new("compositor status", mandara_i18n::tk("Detected compositor and drop-in state"), |_, cx| {
        let b = backend(cx)?;
        let target = b.target();
        Ok(json!({
            "compositor": b.kind(),
            "user_config": target.user_config(),
            "generated": target.generated_file(),
            "generated_exists": target.generated_file().exists(),
            "included": include_installed(target),
            "include_line": target.include_line(),
        }))
    }));
    r.command(ID, CommandSpec::new("compositor sync", mandara_i18n::tk("Regenerate the drop-in (rules, binds, border colors) and reload"), |_, cx| sync(cx)));
    r.command(
        ID,
        CommandSpec::new("compositor install-include", mandara_i18n::tk("Append the single include line to your compositor config (backup kept)"), |a, cx| {
            let target = match a.get(0) {
                Some("mango") => Target::Mango,
                Some("niri") => Target::Niri,
                Some("hyprland") => Target::Hyprland,
                _ => backend(cx)?.target(),
            };
            let added = install_include(target).map_err(|e| format!("{e:#}"))?;
            let report = if added { sync(cx).unwrap_or(Value::Null) } else { Value::Null };
            Ok(json!({ "file": target.user_config(), "added": added, "line": target.include_line(), "sync": report }))
        })
        .arg(ArgSpec::optional("compositor", ArgKind::Enum, mandara_i18n::tk("defaults to the running one")).choices(&["mango", "niri", "hyprland"]))
        .example("compositor install-include"),
    );
    r.command(ID, CommandSpec::new("compositor reload", mandara_i18n::tk("Ask the compositor to reload its config"), |_, cx| {
        backend(cx)?.reload_config().map_err(|e| format!("{e:#}"))?;
        Ok(json!("reloaded"))
    }));
    r.command(ID, CommandSpec::new("compositor clients", mandara_i18n::tk("All windows"), |_, cx| {
        Ok(json(backend(cx)?.get_all_clients().map_err(|e| format!("{e:#}"))?))
    }));
    r.command(ID, CommandSpec::new("compositor monitors", mandara_i18n::tk("Outputs as the compositor sees them"), |_, cx| {
        Ok(json(backend(cx)?.get_monitors().map_err(|e| format!("{e:#}"))?))
    }));
    r.command(
        ID,
        CommandSpec::new("compositor dispatch", mandara_i18n::tk("Raw compositor dispatch (MangoWM function, Hyprland dispatcher, Niri action)"), |a, cx| {
            let cmd = a.str(0)?.to_string();
            let rest: Vec<String> = a.0.iter().skip(1).cloned().collect();
            let args: Vec<&str> = rest.iter().map(String::as_str).collect();
            backend(cx)?.dispatch_raw(&cmd, &args).map_err(|e| format!("{e:#}"))?;
            Ok(json!({ "dispatched": cmd, "args": rest }))
        })
        .arg(ArgSpec::required("command", ArgKind::String, mandara_i18n::tk("function / dispatcher / action name, then its arguments")))
        .example("compositor dispatch togglefloating"),
    );
}

#[cfg(test)]
mod tests {
    use super::sync_key;
    use mandara_config::ShellConfig;

    #[test]
    fn bar_labels_never_trigger_a_resync_but_input_does() {
        let base = ShellConfig::default();
        let mut labelled = base.clone();
        labelled.compositor.input.keyboard.labels.insert("ara".into(), "ع".into());
        assert_eq!(sync_key(&base), sync_key(&labelled), "a bar label is not compositor output");
        let mut repeat = base.clone();
        repeat.compositor.input.keyboard.repeat_rate = Some(40);
        assert_ne!(sync_key(&base), sync_key(&repeat));
        let mut managed = base.clone();
        managed.compositor.manage = !base.compositor.manage;
        assert_ne!(sync_key(&base), sync_key(&managed), "turning management on re-syncs");
    }
}
