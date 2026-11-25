//! Sound themes: a gallery of open sound themes to install, preview and
//! apply (Studio → Themes → Sounds and the `sound
//! list|install|apply|remove|import|play` commands, registered with
//! `system_admin`). Mandara's own event sounds follow the applied theme
//! (`[sounds] theme`); with "Apply to apps" (`[sounds] desktop_sync`) so do
//! GTK, KDE and libcanberra apps (`mandara_services::sound_themes::desktop`).
//!
//! Opt-in: switched off, Mandara installs nothing and leaves the desktop's
//! sound settings alone; switching it off puts back what it changed there.

use std::sync::Mutex;

use gpui::App;
use mandara_config::ShellConfig;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::sound_themes::{self, catalog, install};
use mandara_services::sounds::{self, SoundEvent};
use serde_json::{Value, json};

use crate::state::{config, update_config};

pub const ID: &str = mandara_config::upgrade::SOUND_THEMES_ID;

/// Names `sound apply` takes for "back to your desktop's own".
const UNSET: [&str; 4] = ["none", "default", "system", "your-desktop"];
/// The flag a pasted link needs (it isn't pinned to a checksum).
const UNVERIFIED_FLAG: &str = "--unverified";

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Sound themes"), tk("Install, preview and apply desktop sound themes for Mandara and your apps"), None).opt_in(), |_, _, _| PluginInstance::empty());
}

/// The theme apps should have under `cfg` (`None`: their own).
fn desktop_wish(cfg: &ShellConfig) -> Option<String> {
    let theme = cfg.sounds.theme.trim();
    (cfg.sound_themes_enabled() && cfg.sounds.desktop_sync && !theme.is_empty()).then(|| theme.to_string())
}

/// The newest desktop change not yet made, and whether a thread is making
/// them (changes in a row are folded into the last one).
static PENDING: Mutex<(Option<Option<String>>, bool)> = Mutex::new((None, false));

/// Sets the theme for apps off the UI thread, one change at a time.
fn sync_desktop(wish: Option<String>) {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    p.0 = Some(wish);
    if p.1 {
        return;
    }
    p.1 = true;
    drop(p);
    let started = std::thread::Builder::new().name("mandara-sound-desktop".into()).spawn(|| loop {
        let next = {
            let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            match p.0.take() {
                Some(w) => w,
                None => {
                    p.1 = false;
                    return;
                }
            }
        };
        if let Err(err) = sound_themes::apply_desktop(next.as_deref(), &install::Env::current()) {
            tracing::warn!("sound themes: setting the theme for apps: {err}");
        }
    });
    if started.is_err() {
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).1 = false;
    }
}

/// Follows the config: the sounds kept ready go when the theme changes, and
/// apps get the theme when it (or the plugin, or "Apply to apps") changes.
/// Nothing runs at startup or while nothing changes.
pub fn init(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let mut theme = cfg.sounds.theme.clone();
    let mut wish = desktop_wish(&cfg);
    cx.observe(&config(cx), move |c, cx| {
        let cfg = c.read(cx).snapshot();
        if cfg.sounds.theme != theme {
            theme = cfg.sounds.theme.clone();
            sounds::release();
        }
        let next = desktop_wish(&cfg);
        if next != wish {
            wish = next.clone();
            sync_desktop(next);
        }
    })
    .detach();
}

/// Runs `f` on a thread; the command resolves to its JSON. `after` runs on
/// the UI thread with the result first (sounds, config).
fn job(cx: &mut App, f: impl FnOnce() -> Result<Value, String> + Send + 'static, after: impl FnOnce(&Result<Value, String>, &mut App) + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-sound-theme".into()).spawn(move || {
        let _ = tx.send_blocking(f());
    });
    let (out_tx, out) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").to_string()));
        cx.update(|cx| after(&result, cx));
        let _ = out_tx.send(result.map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())).await;
    })
    .detach();
    Output::Pending(out)
}

/// A finished install: the "Finished" sound.
fn installed(result: &Result<Value, String>, cx: &mut App) {
    if result.is_ok() {
        crate::alerts::play_sound(SoundEvent::Complete, cx);
    }
}

fn apply(cx: &mut App, theme: Option<String>) -> Value {
    let next = theme.clone().unwrap_or_default();
    update_config(cx, move |c| c.sounds.theme = next);
    let cfg = config(cx).read(cx).get().sounds.clone();
    json!({ "theme": theme, "sounds_on": cfg.enabled, "apps_follow": cfg.desktop_sync })
}

/// A catalog id / folder name to the installed theme name it means.
fn theme_name(spec: &str) -> String {
    catalog::resolve(spec).map(|t| t.theme.to_string()).unwrap_or_else(|| spec.to_string())
}

/// `sound list|install|apply|remove|import|play`: the Studio → Themes →
/// Sounds gallery.
pub fn register_commands(r: &mut PluginRegistry) {
    r.command(
        ID,
        CommandSpec::new("sound list", tk("The sound theme catalog (licence, installed, active), other installed themes, and the active theme's sound for each event"), |_, cx| {
            let installed = sound_themes::installed();
            let ours: Vec<String> = install::installed(&install::Env::current()).into_iter().map(|i| i.theme).collect();
            let cfg = config(cx).read(cx).get().sounds.clone();
            let active = if cfg.theme.trim().is_empty() { None } else { Some(cfg.theme.trim().to_string()) };
            let catalog: Vec<Value> = catalog::CATALOG
                .iter()
                .map(|t| {
                    let have = installed.iter().find(|i| i.name == t.theme);
                    json!({ "id": t.id, "name": t.name, "theme": t.theme, "author": t.author, "license": t.license, "homepage": t.homepage, "origin": mandara_i18n::tr(t.origin), "installed": have.is_some(), "system": have.is_some_and(|i| !i.user), "by_mandara": ours.iter().any(|o| o == t.theme), "active": active.as_deref() == Some(t.theme), "download_bytes": t.asset.size, "url": t.asset.url, "sha256": t.asset.sha256 })
                })
                .collect();
            let others: Vec<&sound_themes::ThemeInfo> = installed.iter().filter(|i| catalog::by_theme_name(&i.name).is_none()).collect();
            let shown = active.clone().unwrap_or_else(sounds::system_theme);
            let events: serde_json::Map<String, Value> = SoundEvent::CONFIGURABLE.iter().map(|e| (e.key().to_string(), json!({ "on": e.enabled_in(&cfg), "file": sounds::resolve(e, &cfg) }))).collect();
            Ok(json!({ "active": active, "playing_from": shown, "catalog": catalog, "other_installed": others, "events": events }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("sound install", tk("Download a catalog sound theme from its upstream release, verify its SHA-256 and install it to ~/.local/share/sounds"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme id, e.g. ocean (see `sound list`)")?.to_string();
            let theme = catalog::resolve(&spec).ok_or_else(|| format!("unknown sound theme {spec:?} (see `sound list`)"))?;
            Ok(job(
                cx,
                move || {
                    let i = install::install(theme, &install::Env::current(), &|_| {}, &std::sync::atomic::AtomicBool::new(false))?;
                    Ok(json!({ "installed": i.theme, "dir": i.dir, "sha256": i.sha256 }))
                },
                installed,
            ))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("catalog id or theme name")))
        .example("sound install ocean"),
    );
    r.command(
        ID,
        CommandSpec::new("sound apply", tk("Make an installed sound theme Mandara's (and, with Apply to apps, your apps'); `none` returns to your desktop's own"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme (see `sound list`)")?;
            if UNSET.contains(&spec.to_lowercase().as_str()) {
                return Ok(apply(cx, None));
            }
            let name = theme_name(spec);
            if !sound_themes::installed().iter().any(|i| i.name == name) {
                return Err(format!("{name} isn't installed (run `mandara msg sound install {spec}`)"));
            }
            Ok(apply(cx, Some(name)))
        })
        .arg(ArgSpec::required("theme", ArgKind::String, tk("catalog id, theme name, or none")))
        .example("sound apply ocean"),
    );
    r.command(
        ID,
        CommandSpec::new("sound remove", tk("Remove a sound theme Mandara installed (never one from a package or your own folders)"), |a, cx| {
            let name = theme_name(a.get(0).ok_or("expected a theme (see `sound list`)")?);
            install::remove(&name, &install::Env::current())?;
            // A removed theme can't stay the one playing.
            if config(cx).read(cx).get().sounds.theme.trim() == name {
                apply(cx, None);
            }
            Ok(json!({ "removed": name }))
        })
        .arg(ArgSpec::required("theme", ArgKind::String, tk("catalog id or theme name")))
        .example("sound remove ocean"),
    );
    r.command(
        ID,
        CommandSpec::new_output("sound import", tk("Install a sound theme from a folder, an archive, or an https:// link (links aren't verified: add --unverified)"), |a, cx| {
            let source = a.get(0).ok_or("expected a folder, an archive or an https:// link")?.to_string();
            let unverified = a.0.iter().skip(1).any(|f| f == UNVERIFIED_FLAG);
            if let Some(bad) = a.0.iter().skip(1).find(|f| f.as_str() != UNVERIFIED_FLAG) {
                return Err(tf("Unknown option {option}; expected {options}", &[("option", bad), ("options", &UNVERIFIED_FLAG)]));
            }
            let is_link = source.contains("://");
            if is_link && !unverified {
                return Err(tf("{url} isn't pinned to a checksum, so Mandara can't vouch for it. Run the command again with {flag} to install it anyway.", &[("url", &source), ("flag", &UNVERIFIED_FLAG)]));
            }
            Ok(job(
                cx,
                move || {
                    let env = install::Env::current();
                    let no = std::sync::atomic::AtomicBool::new(false);
                    let i = if is_link { install::import_url(&source, &env, &|_| {}, &no)? } else { install::import(&mandara_config::paths::expand_tilde(&source), &env, &|_| {}, &no)? };
                    Ok(json!({ "installed": i.theme, "dir": i.dir, "sha256": i.sha256, "source": i.source }))
                },
                installed,
            ))
        })
        .arg(ArgSpec::required("source", ArgKind::String, tk("a folder, a .tar.* / .zip archive, or an https:// link")))
        .arg(ArgSpec::optional("options", ArgKind::String, tk("--unverified (needed for links)")))
        .example("sound import ~/Downloads/my-sounds.tar.gz"),
    );
    r.command(
        ID,
        CommandSpec::new_output("sound play", tk("Play one event's sound from a theme (the active one by default), even while sounds are off; answers with its length and level outline"), |a, cx| {
            let event = SoundEvent::parse(a.str(0)?);
            let cfg = config(cx).read(cx).get().sounds.clone();
            let theme = match a.get(1) {
                Some(spec) => theme_name(spec),
                None if cfg.theme.trim().is_empty() => sounds::system_theme(),
                None => cfg.theme.trim().to_string(),
            };
            let (tx, rx) = async_channel::bounded(1);
            let reply_tx = tx.clone();
            let file = sounds::preview_theme(&event, &theme, cfg.volume.max(PREVIEW_MIN_VOLUME), Some(Box::new(move |played| {
                let _ = reply_tx.send_blocking(played.map(|p| serde_json::to_string(&p).unwrap_or_default()));
            })));
            if file.is_none() {
                return Err(tf("No sound for \"{event}\" in {theme}", &[("event", &event.key()), ("theme", &theme)]));
            }
            drop(tx);
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("event", ArgKind::String, tk("notification, notification-critical, volume-change, device-added, login, lock… or a sound name")))
        .arg(ArgSpec::optional("theme", ArgKind::String, tk("catalog id or theme name")))
        .example("sound play notification ocean"),
    );
}

/// Previews stay audible with the volume slider low.
const PREVIEW_MIN_VOLUME: f32 = 0.3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_follows_only_when_asked() {
        let mut cfg = ShellConfig::default();
        cfg.sounds.theme = "ocean".into();
        assert_eq!(desktop_wish(&cfg), None, "plugin off: apps are left alone");
        cfg.plugins.enabled.push(ID.into());
        assert_eq!(desktop_wish(&cfg).as_deref(), Some("ocean"));
        cfg.sounds.desktop_sync = false;
        assert_eq!(desktop_wish(&cfg), None);
        cfg.sounds.desktop_sync = true;
        cfg.sounds.theme = " ".into();
        assert_eq!(desktop_wish(&cfg), None, "no theme: apps get their own back");
    }

    #[test]
    fn specs_name_installed_folders() {
        assert_eq!(theme_name("cosmic"), "COSMIC");
        assert_eq!(theme_name("ocean"), "ocean");
        assert_eq!(theme_name("My Own"), "My Own");
        assert!(UNSET.contains(&"none"));
    }
}
