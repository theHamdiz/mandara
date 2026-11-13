//! Apply a color scheme one key at a time, preserving other desktop preferences.

use super::{Action, Context};
use crate::app_themes::preferences;
use anyhow::Result;

pub(super) struct Kde;

impl Action for Kde {
    fn apply(&self, context: &Context<'_>) -> Result<()> {
        apply_with(context,||crate::runtime::block_on(notify()).map_err(Into::into))
    }
}

fn apply_with(context: &Context<'_>,notify: impl FnOnce()->Result<()>) -> Result<()> {
        let body = crate::app_themes::apply::read_text(context.output)?;
        preferences::set(
            context.owner,
            &context.locations.config.join("kdeglobals"),
            &preferences::ini_keys(&body)?,
            context.locations,
        )?;
        notify()
}

async fn notify() -> zbus::Result<()> {
    crate::runtime::session()
        .await?
        .emit_signal(
            None::<&str>,
            "/KGlobalSettings",
            "org.kde.KGlobalSettings",
            "notifyChange",
            &(0_i32, 0_i32),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_groups_are_merged_without_changing_existing_non_color_keys() {
        let root = tempfile::tempdir().unwrap();
        let locations = crate::app_themes::paths::Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        std::fs::create_dir_all(&locations.config).unwrap();
        let output = root.path().join("scheme.colors");
        std::fs::write(
            &output,
            "[General]\nColorScheme=Test\n[Colors:Window]\nForegroundNormal=1,2,3\n",
        )
        .unwrap();
        let target = locations.config.join("kdeglobals");
        std::fs::write(&target, "[Icons]\nTheme=Keep\n").unwrap();
        let context = Context { owner:"theme",output:&output,locations:&locations };
        apply_with(&context,||Ok(())).unwrap();
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("Theme=Keep")
        );
        let error = apply_with(&context,||anyhow::bail!("session bus is unavailable")).unwrap_err();
        assert!(error.to_string().contains("session bus is unavailable"));
    }
}
