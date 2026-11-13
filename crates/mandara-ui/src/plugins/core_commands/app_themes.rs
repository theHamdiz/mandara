//! App styling commands share Studio's catalog and template worker.

use gpui::App;
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::app_themes::{jobs, paths::Locations};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

use crate::state::{config, update_config};

fn pending(task: gpui::Task<Result<Value, String>>, cx: &mut App) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |_| {
        let _ = tx.send(task.await.map(|value| value.to_string())).await;
    })
    .detach();
    Output::Pending(rx)
}

fn id(words: &[String]) -> Result<(&str, &str), String> {
    let (source, app) = words
        .first()
        .and_then(|id| id.split_once(':'))
        .ok_or("use a source:app id from `theme apps catalog`")?;
    if !mandara_services::app_themes::manifest::valid_id(source)
        || !mandara_services::app_themes::manifest::valid_id(app)
    {
        return Err("invalid app template id".into());
    }
    Ok((source, app))
}

pub(super) fn register(registry: &mut PluginRegistry) {
    registry.command(
        "core",
        CommandSpec::new_output(
            "theme apps catalog",
            tk("Browse available app templates; pass refresh to update the catalog"),
            |arguments, cx| {
                let sources = config(cx)
                    .read(cx)
                    .get()
                    .integrations
                    .app_themes
                    .sources
                    .clone();
                let root = Locations::current()
                    .map_err(|error| error.to_string())?
                    .catalogs();
                let refresh = arguments
                    .0
                    .first()
                    .is_some_and(|argument| argument == "refresh");
                let task = cx.spawn(async move |_| {
                    let result =
                        jobs::discover(sources, root, refresh, Arc::new(AtomicBool::new(false)))
                            .await;
                    Ok(json!({"apps":result.entries,"errors":result.errors}))
                });
                Ok(pending(task, cx))
            },
        )
        .arg(
            ArgSpec::optional(
                "refresh",
                ArgKind::Enum,
                tk("refresh installed catalog metadata"),
            )
            .choices(&["refresh"]),
        )
        .example("theme apps catalog"),
    );
    registry.command(
        "core",
        CommandSpec::new_output("theme apps enable", tk("Review an app template; pass its approval token to enable it"), |arguments, cx| {
            let (source, app) = id(&arguments.0)?;
            if source == "user" {
                if !config(cx).read(cx).get().integrations.app_themes.user.contains_key(app) {
                    return Err(format!("no user template {app}"));
                }
                update_config(cx, |config| {
                    if let Some(entry) = config.integrations.app_themes.user.get_mut(app) {
                        entry.enabled = true;
                    }
                });
                let qualified = arguments.0[0].clone();
                let task = cx.spawn(async move |cx| {
                    let report = cx
                        .update(|cx| crate::state::export_app_themes_report(mandara_core::theme_entity(cx).read(cx).target().clone(), cx))
                        .await;
                    Ok(json!({"id":qualified,"enabled":true,"result":report}))
                });
                return Ok(pending(task, cx));
            }
            let source = config(cx).read(cx).get().integrations.app_themes.sources.iter()
                .find(|entry| entry.id == source).cloned().ok_or("unknown app template source")?;
            let qualified = arguments.0[0].clone();
            if let Some(approval) = arguments.0.get(1) {
                if !mandara_config::app_themes::valid_approval(approval) {
                    return Err("invalid app-template approval token".into());
                }
                // Record the user's intent now. The export worker checks the
                // token before writing; no later completion enables the app.
                let mut next = config(cx).read(cx).get().integrations.app_themes.clone();
                next.approve(&qualified, approval).map_err(|error| error.to_string())?;
                update_config(cx, |config| config.integrations.app_themes = next);
                let task = cx.spawn(async move |cx| {
                    let report = cx.update(|cx| crate::state::export_app_themes_report(mandara_core::theme_entity(cx).read(cx).target().clone(), cx)).await;
                    Ok(json!({"id":qualified,"result":report}))
                });
                return Ok(pending(task, cx));
            }
            let app = app.to_string();
            let locations = Locations::current().map_err(|error| error.to_string())?;
            let root = locations.catalogs();
            let task = cx.spawn(async move |_| {
                let cancel = Arc::new(AtomicBool::new(false));
                let listing = jobs::discover(vec![source.clone()], root.clone(), false, cancel.clone()).await;
                let entry = listing.entries.into_iter().find(|entry| entry.id == app)
                    .ok_or_else(|| format!("app template not found: {app}; {}", listing.errors.join("; ")))?;
                let review = jobs::install_review(source, entry, locations, cancel).await.map_err(|error| format!("{error:#}"))?;
                Ok(json!({"review_required":true,"enable_args":[review.id,review.fingerprint],"review":review}))
            });
            Ok(pending(task, cx))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("source:app template id")))
        .arg(ArgSpec::optional("approval", ArgKind::String, tk("approval token returned by the review")))
        .example("theme apps enable community:bat"),
    );
    registry.command(
        "core",
        CommandSpec::new_output(
            "theme apps disable",
            tk("Disable an app template and undo its managed changes"),
            |arguments, cx| {
                let (source, app) = id(&arguments.0)?;
                update_config(cx, |config| {
                    if source == "user" {
                        if let Some(entry) = config.integrations.app_themes.user.get_mut(app) {
                            entry.enabled = false;
                        }
                    } else {
                        config.integrations.app_themes.disable_catalog(&arguments.0[0]);
                    }
                });
                let qualified = arguments.0[0].clone();
                let task = cx.spawn(async move |cx| {
                    let report = cx
                        .update(|cx| {
                            crate::state::export_app_themes_report(
                                mandara_core::theme_entity(cx).read(cx).target().clone(),
                                cx,
                            )
                        })
                        .await;
                    Ok(json!({"id":qualified,"enabled":false,"result":report}))
                });
                Ok(pending(task, cx))
            },
        )
        .arg(ArgSpec::required(
            "id",
            ArgKind::String,
            tk("source:app template id"),
        ))
        .example("theme apps disable community:bat"),
    );
    registry.command(
        "core",
        CommandSpec::new_output(
            "theme apps import",
            tk("Import a local app-template document without enabling new entries"),
            |arguments, cx| {
                let path = arguments
                    .0
                    .first()
                    .ok_or("a template document path is required")?
                    .clone();
                let task = cx.spawn(async move |cx| {
                    let imported = mandara_services::runtime::unblock(move || {
                        let locations = Locations::current()?;
                        let path =
                            locations.resolve(&locations.home, std::path::Path::new(&path))?;
                        mandara_services::app_themes::import::document(&path)
                    })
                    .await
                    .map_err(|error| format!("{error:#}"))?;
                    let ids: Vec<_> = imported.templates.keys().cloned().collect();
                    cx.update(|cx| {
                        let mut next = config(cx).read(cx).get().integrations.app_themes.clone();
                        mandara_services::app_themes::import::merge(&mut next, imported)
                            .map_err(|error| format!("{error:#}"))?;
                        update_config(cx, |config| config.integrations.app_themes = next);
                        Ok::<_, String>(())
                    })?;
                    Ok(json!({"imported":ids}))
                });
                Ok(pending(task, cx))
            },
        )
        .arg(ArgSpec::required(
            "path",
            ArgKind::String,
            tk("local template document"),
        ))
        .example("theme apps import ~/themes/apps.toml"),
    );
    registry.command(
        "core",
        CommandSpec::new("theme apps status", tk("Show the last app-theme export and any setup errors"), |_, cx| {
            Ok(json!({"enabled":config(cx).read(cx).get().integrations.app_themes.enabled,"result":crate::state::app_theme_status(cx)}))
        })
        .example("theme apps status"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_ids_are_qualified_and_cannot_escape_the_catalog() {
        assert_eq!(
            id(&["community:editor".into()]).unwrap(),
            ("community", "editor")
        );
        for value in ["editor", "../source:app", "source:../../app", "source:"] {
            assert!(id(&[value.into()]).is_err());
        }
    }
    #[test]
    fn the_documented_approval_command_parses_in_a_shell() {
        for docs in [include_str!("../../../../../docs/commands.md"), include_str!("../../../../../docs/app-themes.md")] {
            let example = docs.lines().find(|line| line.starts_with("mandara msg theme apps enable") && line.contains("APPROVAL_TOKEN")).unwrap();
            assert!(std::process::Command::new("sh").args(["-n", "-c", example]).status().unwrap().success());
        }
    }
}
