//! Checked, independent fragments for app colors and saved display preferences.

mod snapshot;

use anyhow::{Context as _,Result, ensure};
use snapshot::Snapshot;
use mandara_config::{
    compositor::{self, Target},
    compositor_theme::{self, Kind},
    doctor::{self, CheckEnv, CheckerStatus, fix},
};
use std::path::{Path, PathBuf};

fn read(path: &Path) -> Result<String> {
    crate::app_themes::apply::read_text(path)
}

/// Validate the exact fragment before attaching it to the managed include.
/// The supplied writer records any file ownership needed by its caller.
pub fn set(
    target: Target,
    kind: Kind,
    body: &str,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<Vec<PathBuf>> {
    ensure!(
        compositor_theme::supported(target),
        "this compositor keeps these settings in its own configuration"
    );
    let _lock = compositor_theme::lock()?;
    crate::outputs::reload_preserving(&[], true, crate::outputs::current_revision(), || {
        set_unlocked(target, kind, body, write).map_err(|error| format!("{error:#}"))
    })
    .map_err(anyhow::Error::msg)
}

fn set_unlocked(
    target: Target,
    kind: Kind,
    body: &str,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<Vec<PathBuf>> {
    let path = compositor_theme::file(target, kind);
    let generated = target.generated_file();
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("fragment has no directory"))?;
    std::fs::create_dir_all(directory)?;
    let scratch = tempfile::Builder::new()
        .prefix(".check-")
        .suffix(&format!(
            ".{}",
            path.extension().unwrap_or_default().to_string_lossy()
        ))
        .tempfile_in(directory)?;
    std::fs::write(scratch.path(), body)?;
    let env = CheckEnv::default();
    let fragment = doctor::check_with(target, scratch.path(), &env);
    ensure!(
        fragment.status == CheckerStatus::Ran && fragment.errors() == 0,
        "the compositor's checker must accept the fragment first: {}",
        fragment.raw
    );
    let before = doctor::check(target);
    ensure!(
        before.status == CheckerStatus::Ran,
        "the compositor's configuration must be readable before enabling the fragment"
    );
    let previous = Snapshot::capture(&path)?;
    let previous_generated = Snapshot::capture(&generated)?;
    let mut primary_backup = None;
    let result = (|| {
        write(&path)?;
        let old = if generated.exists() {
            read(&generated)?
        } else {
            String::new()
        };
        let outcome = fix::guarded_write(&before, &env, &|| {
            let body = compositor_theme::compose(target, kind, &old, &path, true)?;
            mandara_config::store::write_atomic(&generated, body.as_bytes())
        })?;
        ensure!(
            !outcome.rolled_back,
            "the compositor rejected the fragment include; previous settings were restored"
        );
        if !compositor::include_installed(target) {
            let before = doctor::check(target);
            let user = target.user_config();
            primary_backup = Some(fix::backup_files(target, std::slice::from_ref(&user), &fix::backups_root())?);
            let included = fix::guarded_write_in(
                &before,
                &env,
                &|| compositor::install_include(target).map(|_| ()),
                &user,
                &fix::Store::default(),
            )?;
            ensure!(
                !included.rolled_back,
                "the compositor rejected the include; previous settings were restored"
            );
        }
        reload(target)?;
        Ok(vec![path.clone(), generated.clone()])
    })();
    if let Err(error) = result {
        if let Some(backup) = primary_backup { fix::restore(&backup).context("restoring the primary compositor include")?; }
        previous_generated.restore(&generated).context("restoring the managed compositor include")?;
        previous.restore(&path).context("restoring the previous compositor fragment")?;
        reload(target).context("reloading the restored compositor configuration")?;
        return Err(error);
    }
    result
}

pub fn detach(target: Target, kind: Kind) -> Result<()> {
    let _lock = compositor_theme::lock()?;
    crate::outputs::reload_preserving(&[], true, crate::outputs::current_revision(), || {
        let result = (|| {
            let generated = target.generated_file();
            if !generated.exists() {
                return Ok(());
            }
            let before = doctor::check(target);
            let body = read(&generated)?;
            let updated = compositor_theme::compose(
                target,
                kind,
                &body,
                &compositor_theme::file(target, kind),
                false,
            )?;
            if updated == body {
                return Ok(());
            }
            let result = fix::guarded_write(&before, &CheckEnv::default(), &|| {
                mandara_config::store::write_atomic(&generated, updated.as_bytes())
            })?;
            ensure!(
                !result.rolled_back,
                "the compositor rejected removing its fragment include"
            );
            reload(target)
        })();
        result.map_err(|error: anyhow::Error| format!("{error:#}"))
    })
    .map_err(anyhow::Error::msg)
}

fn reload(target: Target) -> Result<()> {
    if let Some(backend) =
        mandara_compositor::connect().filter(|backend| backend.target() == target)
    {
        backend.reload_config()?;
    }
    Ok(())
}

/// Saving a display arrangement does not turn on management of binds, input
/// or layout. Its fragment contains only the preferences the user kept.
pub fn persist_outputs(
    target: Target,
    outputs: &[mandara_config::outputs::OutputConfig],
) -> Result<()> {
    let kind = Kind::Displays;
    let path = compositor_theme::file(target, kind);
    if outputs.is_empty() {
        if path.exists() {
            detach(target, kind)?;
            std::fs::remove_file(path)?;
        }
        return Ok(());
    }
    let body = mandara_config::outputs::emit_outputs(
        target,
        outputs,
        target == Target::Hyprland && Target::hyprland_lua(),
    );
    if read(&path).is_ok_and(|previous| previous == body) && compositor::include_installed(target) {
        return Ok(());
    }
    set(target, kind, &body, |path| {
        mandara_config::store::write_atomic(path, body.as_bytes())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_fragments_contain_no_unrelated_compositor_preferences() {
        let output = mandara_config::outputs::OutputConfig {
            name: "DP-1".into(),
            scale: Some(1.5),
            x: Some(0),
            y: Some(0),
            ..Default::default()
        };
        for target in [
            Target::Mango,
            Target::Niri,
            Target::Hyprland,
            Target::Umbriel,
            Target::Sway,
        ] {
            let body =
                mandara_config::outputs::emit_outputs(target, std::slice::from_ref(&output), false);
            assert!(
                body.contains("DP-1") && body.contains("1.5"),
                "{target:?}: {body}"
            );
            for unrelated in ["exec ", "bindsym", "borderpx", "input {", "keybind"] {
                assert!(!body.contains(unrelated), "{target:?}: {body}");
            }
            let included = compositor_theme::compose(
                target,
                Kind::Displays,
                &body,
                Path::new("/tmp/outputs.conf"),
                true,
            ).unwrap();
            let both = compositor_theme::compose(
                target,
                Kind::AppColors,
                &included,
                Path::new("/tmp/colors.conf"),
                true,
            ).unwrap();
            let without_colors = compositor_theme::compose(
                target,
                Kind::AppColors,
                &both,
                Path::new("/tmp/colors.conf"),
                false,
            ).unwrap();
            assert_eq!(without_colors, included);
        }
    }
}
