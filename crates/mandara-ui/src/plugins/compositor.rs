//! Compositor integration commands and live sync of the generated drop-in
//! (layer rules, window/tag rules, keybinds, palette border colors).

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, AsyncApp};
use mandara_config::compositor::{BorderColors, Target, include_installed, install_include};
use mandara_config::doctor::{self, CheckEnv, Report, fix as dfix, state as dstate};
use mandara_i18n::{t, tf};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
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

type SyncInputs = (std::sync::Arc<dyn mandara_compositor::CompositorBackend>, std::sync::Arc<mandara_config::ShellConfig>, Option<BorderColors>);

fn sync_inputs(cx: &App) -> Result<SyncInputs, String> {
    let backend = hub(cx).read(cx).compositor.clone().ok_or("no supported compositor is running")?;
    let cfg = config(cx).read(cx).snapshot();
    let borders = cfg.integrations.compositor_borders.then(|| border_colors(cx));
    Ok((backend, cfg, borders))
}

/// Writes the generated file for the running compositor (checked by the
/// Config doctor, rolled back if the compositor rejects it) and reloads it
/// when the user's config includes it. Blocking: always run off the UI
/// thread.
fn sync_with(backend: &dyn mandara_compositor::CompositorBackend, cfg: &mandara_config::ShellConfig, borders: Option<BorderColors>,
    preserve_outputs: bool,
    output_revision: mandara_services::outputs::Revision,
) -> Result<Value, String> {
    let _file_lock = mandara_config::compositor_theme::lock().map_err(|error| error.to_string())?;
    if include_installed(backend.target()) {
        mandara_services::outputs::reload_preserving(
            &cfg.compositor.outputs,
            preserve_outputs,
            output_revision,
            || sync_with_inner(backend, cfg, borders),
        )
    } else {
        sync_with_inner(backend, cfg, borders)
    }
}

fn sync_with_inner(
    backend: &dyn mandara_compositor::CompositorBackend,
    cfg: &mandara_config::ShellConfig,
    borders: Option<BorderColors>,
) -> Result<Value, String> {
    let target = backend.target();
    let compositor = cfg.effective_compositor();
    // The config as the compositor reads it now (the stored report unless
    // a file or the compositor changed), so the write below can be judged.
    let (before, _) = dstate::check_cached(target);
    let written = RefCell::new(None);
    let write = || -> anyhow::Result<()> {
        *written.borrow_mut() = Some(backend.sync_managed_config(&compositor, borders.clone())?);
        Ok(())
    };
    let env = CheckEnv { version: doctor::installed_version(target), ..CheckEnv::default() };
    let guarded = dfix::guarded_write(&before, &env, &write).map_err(|e| format!("{e:#}"))?;
    for option in &guarded.left_out {
        tracing::info!("config doctor: {} {} rejects `{option}`; left out of the drop-in", target.name(), guarded.after.version.as_deref().unwrap_or("?"));
    }
    if guarded.rolled_back {
        tracing::warn!("config doctor: {} rejected the new drop-in, so the previous one was kept", target.name());
    }
    dstate::record(guarded.after);
    let (path, emitted) = written.into_inner().ok_or("the drop-in was not written")?;
    let included = include_installed(target);
    // The cursor is the opt-in cursor themes plugin's: switched off,
    // Mandara touches no cursor setting at all.
    let cursors = cfg.cursors_enabled();
    // X11 and older toolkit apps read `~/.icons/default` (only a file
    // Mandara wrote is updated, and it goes when the theme is unset).
    if cursors && let Err(err) = mandara_config::input::sync_default_cursor(cfg.compositor.input.cursor.theme.as_deref()) {
        tracing::warn!("default cursor: {err}");
    }
    if included {
        backend.reload_config().map_err(|e| format!("reload: {e:#}"))?;
        // Hyprland's `env = XCURSOR_*` lines only reach apps started later;
        // the cursor on screen changes through `setcursor`.
        let cursor = &cfg.compositor.input.cursor;
        if cursors && let Err(err) = backend.apply_cursor(cursor.theme.as_deref(), cursor.size) {
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
    let manage = config(cx).read(cx).get().compositor.manage;
    let inputs = sync_inputs(cx);
    let output_revision = mandara_services::outputs::current_revision();
    let generation = SYNC_STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    // Off the UI thread: the sync checks the config with the compositor's
    // own checker, then the Config doctor announces new problems once.
    cx.spawn(async move |cx| {
        let task = mandara_services::runtime::unblock(move || {
            let (backend, cfg, borders) = inputs.ok()?;
            let mut applied = SYNC_APPLIED.lock().unwrap_or_else(|e| e.into_inner());
            if generation < *applied {
                return None;
            }
            *applied = generation;
            if sync_enabled(manage, backend.kind()) {
                initial_sync(backend.as_ref(), &cfg, borders, output_revision);
            }
            Some(dstate::check_cached(backend.target()).0)
        });
        if let Some(report) = task.await {
            announce(report, cx).await;
        }
    })
    .detach();
    watch(cx);
}

fn initial_sync(backend: &dyn mandara_compositor::CompositorBackend, cfg: &mandara_config::ShellConfig, borders: Option<BorderColors>,
    output_revision: mandara_services::outputs::Revision,
) {
    match sync_with(backend, cfg, borders, false, output_revision) {
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

/// One gentle notification about problems not announced before (and not
/// snoozed): the first problem in plain words, and a button that opens the
/// Config doctor in Studio.
async fn announce(report: Report, cx: &mut AsyncApp) {
    if !cx.update(|cx| config(cx).read(cx).get().compositor.doctor_notify) {
        return;
    }
    let fresh = cx.background_executor().spawn(async move {
        let fresh = dstate::take_unannounced(&report);
        (report, fresh)
    });
    let (report, fresh) = fresh.await;
    let Some(first) = fresh.first() else { return };
    let compositor = report.target.name();
    let summary = if fresh.len() == 1 { tf("{compositor} reports a problem in your config", &[("compositor", &compositor)]) } else { tf("{compositor} reports {count} problems in your config", &[("compositor", &compositor), ("count", &fresh.len())]) };
    let place = first.place().map(|p| format!("{p}: ")).unwrap_or_default();
    let body = format!("{place}{}", doctor::explain(first, report.target, report.version.as_deref()));
    let clicked = mandara_services::desktop_notify::post_with_action(t("Mandara Shell"), "monitor", &summary, &body, t("Open Config doctor"));
    if clicked.recv().await == Ok(true) {
        cx.update(|cx| crate::state::open_studio_at(Some("compositor".into()), cx));
    }
}

/// What a command's `compositor` argument takes: every compositor's id.
const COMPOSITOR_NAMES: [&str; Target::ALL.len()] = {
    let mut names = [""; Target::ALL.len()];
    let mut i = 0;
    while i < names.len() {
        names[i] = Target::ALL[i].id();
        i += 1;
    }
    names
};

/// The compositor a command names (`mango`, `niri`…), else the running one.
fn target_arg(name: Option<&str>, cx: &App) -> Result<Target, String> {
    match name {
        Some(name) => Target::parse(name).ok_or_else(|| {
            format!("unknown compositor {name:?} ({})", COMPOSITOR_NAMES.join(", "))
        }),
        None => Ok(backend(cx)?.target()),
    }
}

/// Runs `work` on a thread of its own and answers with its JSON.
fn pending(work: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work().map(|v| v.to_string()));
    });
    Output::Pending(rx)
}

/// What the drop-in is made of, compared to skip needless re-syncs.
/// Keyboard-layout labels are bar-only, so editing one never rewrites the
/// drop-in or reloads the compositor.
fn sync_key(cfg: &mandara_config::ShellConfig) -> String {
    let mut compositor = cfg.effective_compositor().into_owned();
    compositor.input.keyboard.labels.clear();
    // The Config doctor's notification switch isn't part of the drop-in.
    compositor.doctor_notify = true;
    serde_json::to_string(&compositor).unwrap_or_default()
}

/// A compositor edit is written to the drop-in once edits have rested
/// this long.
const SYNC_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// Writes the drop-in for the current config in the background.
fn start_sync(cx: &mut App, preserve_outputs: bool) {
    let output_revision = mandara_services::outputs::current_revision();
    match sync_inputs(cx) {
        Ok((backend, cfg, borders)) => {
            let generation = SYNC_STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            cx.spawn(async move |_| {
                mandara_services::runtime::unblock(move || {
                    // Background syncs may finish out of order: an older one
                    // never overwrites a newer drop-in.
                    let mut applied = SYNC_APPLIED.lock().unwrap_or_else(|e| e.into_inner());
                    if generation < *applied {
                        return;
                    }
                    *applied = generation;
                    if let Err(err) = sync_with(backend.as_ref(), &cfg, borders,
                        preserve_outputs,
                        output_revision,
                    ) {
                        tracing::warn!("compositor sync: {err}");
                    }
                })
                .await;
            })
                .detach();
        }
        Err(err) => tracing::warn!("compositor sync: {err}"),
    }
}

fn watch(cx: &mut App) {
    let pending: Rc<RefCell<Option<gpui::Task<()>>>> = Rc::default();
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
        let generation = config(cx).read(cx).snapshot();
        let p = mandara_core::theme_entity(cx).read(cx).target();
        (generation, [p.primary, p.accent_gradient_end, p.surface_container_high, p.urgent_coral])
    };
    let initial = key(cx);
    let last_key = RefCell::new((std::sync::Arc::downgrade(&initial.0), initial.1));
    let previous_outputs = RefCell::new(config(cx).read(cx).get().compositor.outputs.clone());
    let outputs_changed = Rc::new(std::cell::Cell::new(false));
    let on_change = move |cx: &mut App| {
        let now_key = key(cx);
        if last_key.borrow()
            .0
            .upgrade()
            .is_some_and(|previous| std::sync::Arc::ptr_eq(&previous, &now_key.0))
            && last_key.borrow().1 == now_key.1
        {
            return;
        }
        *last_key.borrow_mut() = (std::sync::Arc::downgrade(&now_key.0), now_key.1);
        if !backend(cx).is_ok_and(|b| sync_enabled(config(cx).read(cx).get().compositor.manage, b.kind())) {
            return;
        }
        let now = snapshot(cx);
        if *last.borrow() != now {
            *last.borrow_mut() = now;
            let outputs = config(cx).read(cx).get().compositor.outputs.clone();
            outputs_changed.set(outputs_changed.get() || *previous_outputs.borrow() != outputs);
            *previous_outputs.borrow_mut() = outputs;
            // Committed once the edits rest: a dragged Studio slider (gaps,
            // borders) rewrites and reloads the drop-in once, not per step.
            // A newer edit drops the waiting one.
            let outputs_changed = outputs_changed.clone();
            *pending.borrow_mut() = Some(cx.spawn(async move |cx| {
                cx.background_executor().timer(SYNC_SETTLE).await;
                cx.update(|cx| start_sync(cx, !outputs_changed.replace(false)));
            }));
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

fn sync_enabled(manage: bool, kind: mandara_compositor::CompositorKind) -> bool {
    // KWin writes only Mandara's own startup entry, never compositor
    // settings; Mandara writes nothing at all for Aqueous, labwc or COSMIC.
    match kind {
        mandara_compositor::CompositorKind::KWin => true,
        mandara_compositor::CompositorKind::Aqueous | mandara_compositor::CompositorKind::Labwc | mandara_compositor::CompositorKind::Cosmic => false,
        _ => manage,
    }
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
    r.command(ID, CommandSpec::new_output("compositor sync", mandara_i18n::tk("Regenerate the drop-in (rules, binds, border colors) and reload"), |_, cx| {
        let (backend, cfg, borders) = sync_inputs(cx)?;
                let output_revision = mandara_services::outputs::current_revision();
                Ok(pending(move || {
                    sync_with(backend.as_ref(), &cfg, borders, true, output_revision)
                }))
    }));
    r.command(
        ID,
        CommandSpec::new_output("compositor check", mandara_i18n::tk("Config doctor: check the compositor's config with its own checker; problems with file, line, explanation and fixes"), |a, cx| {
            let target = target_arg(a.get(0), cx)?;
            Ok(pending(move || Ok(dstate::record(doctor::check(target)).to_json())))
        })
        .arg(ArgSpec::optional("compositor", ArgKind::Enum, mandara_i18n::tk("defaults to the running one")).choices(&COMPOSITOR_NAMES))
        .example("compositor check"),
    );
    r.command(
        ID,
        CommandSpec::new_output("compositor fix", mandara_i18n::tk("Config doctor: fix a problem from the last check (backed up, re-checked, rolled back if worse)"), |a, cx| {
            let id = a.str(0)?.to_string();
            let fix = dfix::Fix::parse(a.str(1)?).ok_or("the fix is comment, remove or regenerate")?;
            let (backend, cfg, borders) = sync_inputs(cx)?;
            let target = target_arg(a.get(2), cx)?;
            Ok(pending(move || {
                let report = dstate::last_report(target).filter(|r| r.find(&id).is_some()).ok_or_else(|| format!("no problem {id} in the last check; run `mandara msg compositor check` first"))?;
                let regenerate = || backend.sync_managed_config(&cfg.effective_compositor(), borders.clone()).map(|_| ());
                let env = CheckEnv { version: report.version.clone(), ..CheckEnv::default() };
                let outcome = dfix::apply(&report, &id, fix, &env, &regenerate).map_err(|e| format!("{e:#}"))?;
                let after = dstate::record(outcome.after.clone());
                if fix == dfix::Fix::Regenerate && include_installed(target) {
                    backend.reload_config().map_err(|e| format!("reload: {e:#}"))?;
                }
                Ok(json!({ "fix": fix, "id": id, "resolved": outcome.resolved, "rolled_back": outcome.rolled_back, "backup": outcome.backup, "report": after.to_json() }))
            }))
        })
        .arg(ArgSpec::required("id", ArgKind::String, mandara_i18n::tk("the problem's id from `compositor check`")))
        .arg(ArgSpec::required("fix", ArgKind::Enum, mandara_i18n::tk("comment or remove (your files), regenerate (Mandara's drop-in)")).choices(&["comment", "remove", "regenerate"]))
        .arg(ArgSpec::optional("compositor", ArgKind::Enum, mandara_i18n::tk("defaults to the running one")).choices(&COMPOSITOR_NAMES))
        .example("compositor fix 1a2b3c4d comment"),
    );
    r.command(
        ID,
        CommandSpec::new("compositor snooze", mandara_i18n::tk("Config doctor: no reminders about the current problems until the config or the compositor changes"), |a, cx| {
            let target = target_arg(a.get(0), cx)?;
            let report = dstate::last_report(target).ok_or("nothing checked yet; run `mandara msg compositor check` first")?;
            dstate::snooze(&report);
            Ok(json!({ "snoozed": report.diagnostics.len() }))
        })
        .arg(ArgSpec::optional("compositor", ArgKind::Enum, mandara_i18n::tk("defaults to the running one")).choices(&COMPOSITOR_NAMES)),
    );
    r.command(
        ID,
        CommandSpec::new_output("compositor install-include", mandara_i18n::tk("Append the single include line to your compositor config (backup kept)"), |a, cx| {
            let target = target_arg(a.get(0), cx)?;
            let inputs = sync_inputs(cx);
            let output_revision = mandara_services::outputs::current_revision();
            Ok(pending(move || {
                let added = install_include(target).map_err(|e| format!("{e:#}"))?;
                let report = match (added, inputs) {
                    (true, Ok((backend, cfg, borders))) => sync_with(backend.as_ref(), &cfg, borders, false, output_revision).unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                Ok(json!({ "file": target.user_config(), "added": added, "line": target.include_line(), "sync": report }))
            }))
        })
        .arg(ArgSpec::optional("compositor", ArgKind::Enum, mandara_i18n::tk("defaults to the running one")).choices(&COMPOSITOR_NAMES))
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
        CommandSpec::new("compositor dispatch", mandara_i18n::tk("Raw compositor dispatch (MangoWM function, Hyprland dispatcher, Niri or Umbriel action, Sway or River command)"), |a, cx| {
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
    use super::{sync_enabled, sync_key};
    use mandara_config::ShellConfig;

    #[test]
    fn native_startup_sync_is_independent_of_drop_in_management() {
        use mandara_compositor::CompositorKind;
        assert!(sync_enabled(false, CompositorKind::KWin));
        for kind in [CompositorKind::Aqueous, CompositorKind::Labwc, CompositorKind::Cosmic] {
            assert!(!sync_enabled(true, kind), "no drop-in for {kind:?}, whatever the switch says");
        }
        for kind in [CompositorKind::MangoWM, CompositorKind::Niri, CompositorKind::Hyprland, CompositorKind::Umbriel, CompositorKind::Sway, CompositorKind::River] {
            assert!(!sync_enabled(false, kind));
            assert!(sync_enabled(true, kind));
        }
    }

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
