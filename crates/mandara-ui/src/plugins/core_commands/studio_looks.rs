//! Studio appearance commands update only the independent snapshot.

use mandara_config::studio::StudioLook;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use serde_json::json;

use crate::state::{config, update_config};

pub(super) fn register(registry: &mut PluginRegistry) {
    registry.command("core", CommandSpec::new("studio look list", tk("List Studio's independent window designs"), |_, cx| {
        let config = config(cx).read(cx).snapshot();
        let looks: Vec<_> = StudioLook::ALL.into_iter().map(|look| {
            let definition = look.definition();
            json!({ "id": look.slug(), "name": tr(definition.name), "description": tr(definition.description), "dark": definition.dark })
        }).collect();
        Ok(json!({ "looks": looks, "active": config.studio.as_deref().map(|studio| studio.look.slug()) }))
    }).example("studio look list"));

    let choices = StudioLook::ALL.map(StudioLook::slug);
    registry.command(
        "core",
        CommandSpec::new(
            "studio look apply",
            tk("Apply a Studio window design without changing the desktop Look"),
            |arguments, cx| {
                let name = arguments.str(0)?;
                let look = StudioLook::parse(name)
                    .ok_or_else(|| tf("No Studio look named {name}.", &[("name", &name)]))?;
                update_config(cx, |config| {
                    config.studio = Some(Box::new(look.appearance()))
                });
                Ok(json!({ "applied": look.slug() }))
            },
        )
        .arg(
            ArgSpec::required(
                "look",
                ArgKind::Enum,
                tk("Studio look id (see studio look list)"),
            )
            .choices(&choices),
        )
        .example("studio look apply cupertino-glass"),
    );

    registry.command(
        "core",
        CommandSpec::new(
            "studio look follow-shell",
            tk("Let Studio follow the shell's appearance again"),
            |_, cx| {
                update_config(cx, |config| config.studio = None);
                Ok(json!({ "appearance": t("Follow the shell") }))
            },
        )
        .example("studio look follow-shell"),
    );
}
