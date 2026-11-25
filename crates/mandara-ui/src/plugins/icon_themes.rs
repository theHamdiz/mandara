//! Icon themes: a gallery of fifty open-source icon themes to install,
//! preview and apply live, plus your own from an archive, a folder or a
//! link (Studio → Themes → Icons and the `icons list|install|apply|remove|
//! import` commands, registered with `system_admin`).
//!
//! Opt-in: switched off, Mandara leaves icon configuration alone entirely
//! (no downloads, no GTK / Qt / KDE settings, no `gsettings`). Applying a
//! theme sets Mandara's own icons (`appearance.icon_theme`: the launcher,
//! dock and tray) and the desktop's (`mandara_services::icons::apply`);
//! `none` goes back to the desktop's own setting and puts back what
//! Mandara changed.

use gpui::{App, AsyncApp};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::icons::{self, apply, catalog, install};
use serde_json::{Value, json};

use crate::state::{config, update_config};

pub const ID: &str = mandara_config::upgrade::ICONS_ID;

/// Theme names `icons apply` takes for "follow the desktop".
const UNSET: [&str; 4] = ["none", "default", "desktop", "follow"];
/// The flag a pasted link needs (it has no checksum to verify).
const UNVERIFIED_FLAG: &str = "--unverified";

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Icon themes"), tk("Install, preview and apply icon themes, live, from a curated gallery or your own files"), None).opt_in(), |_, _, _| PluginInstance::empty());
}

/// Runs `f` on a worker thread; the command resolves to its JSON. With
/// `after`, the result is first handed to the UI thread (config updates).
fn job(cx: &mut App, f: impl FnOnce() -> Result<Value, String> + Send + 'static, after: Option<Box<dyn FnOnce(&Value, &mut App)>>) -> Output {
    let (tx, rx) = async_channel::bounded::<Result<Value, String>>(1);
    let _ = std::thread::Builder::new().name("mandara-icons".into()).spawn(move || {
        let _ = tx.send_blocking(f());
    });
    let (out_tx, out) = async_channel::bounded(1);
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        if let (Ok(value), Some(after)) = (&result, after) {
            cx.update(|cx| after(value, cx));
        }
        let _ = out_tx.send(result.map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())).await;
    })
    .detach();
    Output::Pending(out)
}

/// The installed folder name a spec means: a catalog `id[/variant]` or a
/// theme name.
fn theme_name(spec: &str) -> String {
    catalog::resolve(spec).map(|(_, v)| v.theme.to_string()).unwrap_or_else(|| spec.to_string())
}

/// Makes `theme` the icon theme of Mandara and the desktop (`None`: back to
/// the desktop's own). The desktop part runs on a worker thread.
pub fn apply_theme(cx: &mut App, theme: Option<String>) -> Output {
    let next = theme.clone();
    update_config(cx, move |c| c.appearance.icon_theme = next);
    job(
        cx,
        move || {
            let env = apply::ApplyEnv::current();
            let report = match &theme {
                Some(name) => apply::apply(name, &env, &apply::GsettingsCommand)?,
                None => apply::restore(&env, &apply::GsettingsCommand)?,
            };
            Ok(json!({ "theme": theme, "changed": report.changed, "kept": report.kept }))
        },
        None,
    )
}

fn list(cx: &mut App) -> Output {
    let active = config(cx).read(cx).get().appearance.icon_theme.clone();
    job(
        cx,
        move || {
            let roots = mandara_config::input::icon_dirs();
            let found = icons::installed_themes(&roots);
            let ours = install::installed(&install::Env::current());
            let applied = apply::applied(&apply::ApplyEnv::current());
            let is_installed = |name: &str| found.iter().any(|f| f.name == name);
            let catalog: Vec<Value> = catalog::CATALOG
                .iter()
                .map(|t| {
                    let variants: Vec<Value> = t
                        .variants
                        .iter()
                        .map(|v| json!({ "id": format!("{}/{}", t.id, v.id), "theme": v.theme, "dark": v.dark, "installed": is_installed(v.theme), "by_mandara": ours.iter().any(|i| i.theme == v.theme), "active": active.as_deref() == Some(v.theme), "folders": v.dirs.iter().map(|d| d.name).collect::<Vec<_>>() }))
                        .collect();
                    json!({ "id": t.id, "name": t.name, "author": t.author, "license": t.license, "homepage": t.homepage, "stars": t.stars, "style": t.style.slug(), "tone": t.tone.slug(), "download_bytes": t.archive.size, "url": t.archive.url, "sha256": t.archive.sha256, "variants": variants })
                })
                .collect();
            let imported: Vec<Value> = ours.iter().filter(|i| !i.verified).map(|i| json!({ "theme": i.theme, "title": i.title, "source": i.source, "verified": false })).collect();
            let others: Vec<Value> = found.iter().filter(|f| catalog::by_theme_name(&f.name).is_none() && !ours.iter().any(|i| i.theme == f.name)).map(|f| json!({ "theme": f.name, "title": f.title, "dir": f.dir })).collect();
            Ok(json!({ "active": active, "desktop_applied": applied, "catalog": catalog, "imported": imported, "other_installed": others }))
        },
        None,
    )
}

/// `icons list|install|apply|remove|import`: the Studio → Themes → Icons
/// gallery.
pub fn register_commands(r: &mut PluginRegistry) {
    r.command(ID, CommandSpec::new_output("icons list", tk("The icon theme catalog (variants, licence, installed, active), your imports and other installed icon themes"), |_, cx| Ok(list(cx))));
    r.command(
        ID,
        CommandSpec::new_output("icons install", tk("Download a catalog icon theme from its project's GitHub release, verify its SHA-256 and install it to ~/.local/share/icons"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme id, e.g. papirus/dark (see `icons list`)")?.to_string();
            let (theme, variant) = catalog::resolve(&spec).ok_or_else(|| format!("unknown icon theme {spec:?} (see `icons list`)"))?;
            Ok(job(
                cx,
                move || {
                    let i = install::install(theme, variant, &install::Env::current(), &|_| {}, &std::sync::atomic::AtomicBool::new(false))?;
                    Ok(json!({ "installed": i.theme, "folders": i.dirs, "sha256": i.sha256, "verified": i.verified }))
                },
                None,
            ))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("catalog id[/variant] or icon theme name")))
        .example("icons install papirus/dark"),
    );
    r.command(
        ID,
        CommandSpec::new_output("icons apply", tk("Make an installed icon theme Mandara's and the desktop's (GTK, Qt / KDE), live; `none` goes back to the desktop's own"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme (see `icons list`)")?;
            if UNSET.contains(&spec.to_lowercase().as_str()) {
                return Ok(apply_theme(cx, None));
            }
            let name = theme_name(spec);
            if icons::theme_dir(&name, &mandara_config::input::icon_dirs()).is_none() {
                return Err(format!("{name} isn't installed (run `mandara msg icons install {spec}`)"));
            }
            Ok(apply_theme(cx, Some(name)))
        })
        .arg(ArgSpec::required("theme", ArgKind::String, tk("catalog id[/variant], icon theme name, or none")))
        .example("icons apply papirus/dark"),
    );
    r.command(
        ID,
        CommandSpec::new_output("icons remove", tk("Remove an icon theme Mandara installed (never one from a package or your own folders)"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme id (see `icons list`)")?;
            let name = theme_name(spec);
            let active = config(cx).read(cx).get().appearance.icon_theme.clone();
            let gone = name.clone();
            Ok(job(
                cx,
                move || {
                    let removed = install::remove(&name, &install::Env::current())?;
                    Ok(json!({ "removed": name, "folders": removed }))
                },
                Some(Box::new(move |_, cx| {
                    // A removed theme can't stay the icon theme.
                    if active.as_deref() == Some(gone.as_str()) {
                        let _ = apply_theme(cx, None);
                    }
                })),
            ))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("catalog id[/variant] or icon theme name")))
        .example("icons remove papirus/dark"),
    );
    r.command(
        ID,
        CommandSpec::new_output("icons import", tk("Install icon themes from an archive or folder, or from an https:// link (no checksum exists for those: pass --unverified)"), |a, cx| {
            let target = a.get(0).ok_or("expected a path or an https:// link")?.to_string();
            let source = if target.contains("://") {
                if a.get(1) != Some(UNVERIFIED_FLAG) {
                    return Err(format!("a link can't be checked against a known checksum: add {UNVERIFIED_FLAG} to install it anyway"));
                }
                install::ImportSource::Url(target)
            } else {
                install::ImportSource::Path(mandara_config::paths::expand_tilde(&target))
            };
            Ok(job(
                cx,
                move || {
                    let report = install::import(&source, &install::Env::current(), &|_| {}, &std::sync::atomic::AtomicBool::new(false))?;
                    Ok(json!({ "installed": report.installed.iter().map(|i| &i.theme).collect::<Vec<_>>(), "skipped": report.skipped, "verified": false }))
                },
                None,
            ))
        })
        .arg(ArgSpec::required("source", ArgKind::String, tk("an archive or folder path, or an https:// link")))
        .arg(ArgSpec::optional("flag", ArgKind::String, tk("--unverified, required for a link")).choices(&[UNVERIFIED_FLAG]))
        .example("icons import ~/Downloads/Nordic-icons.tar.xz"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_name_installed_folders() {
        assert_eq!(theme_name("papirus/dark"), "Papirus-Dark");
        assert_eq!(theme_name("papirus"), "Papirus");
        assert_eq!(theme_name("My-Theme"), "My-Theme");
        assert!(UNSET.contains(&"none"));
    }

    #[test]
    fn commands_are_registered_under_the_plugin() {
        let mut r = PluginRegistry::default();
        register(&mut r);
        register_commands(&mut r);
        assert!(r.manifest(ID).is_some());
        assert!(!r.is_enabled(ID, &mandara_config::PluginsConfig::default()), "opt-in");
        for c in ["icons list", "icons install", "icons apply", "icons remove", "icons import"] {
            assert!(r.commands.all().iter().any(|s| s.path == c && s.owner == ID), "{c}");
        }
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(r.commands.resolve(&words(&["icons", "install", "papirus/dark"])).is_ok());
        assert!(r.commands.resolve(&words(&["icons", "import", "https://example.org/x.tar.gz", "--unverified"])).is_ok());
        assert!(r.commands.resolve(&words(&["icons", "import", "https://example.org/x.tar.gz", "--yes"])).is_err(), "only the one flag");
    }
}
