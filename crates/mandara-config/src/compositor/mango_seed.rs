//! Start a missing user config from MangoWM's system defaults, without replacing one.

use super::Target;
use crate::doctor::{self, CheckEnv, CheckerStatus, fix};
use anyhow::{Context as _, Result, ensure};
use mandara_theme::templates::{FileStamp, WriteLock};
use std::{cell::Cell, io::Read as _, path::Path};

/// Match the Config doctor's input bound before copying a system configuration.
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

pub fn seed_user_config() -> Result<bool> {
    seed_in(
        &crate::paths::home(),
        Path::new("/etc/mango"),
        &CheckEnv::default(),
        &fix::Store::default(),
    )
}

pub(super) fn seed_in(
    home: &Path,
    system: &Path,
    env: &CheckEnv,
    store: &fix::Store,
) -> Result<bool> {
    let legacy = home.join(".config/mango/config.conf");
    let toml = legacy.with_extension("toml");
    // A broken symlink is still the user's configuration, not an invitation to replace it.
    if [&legacy, &toml]
        .iter()
        .any(|path| std::fs::symlink_metadata(path).is_ok())
    {
        return Ok(false);
    }
    let Some(source) = [system.join("config.conf"), system.join("config.toml")]
        .into_iter()
        .find(|path| path.is_file())
    else {
        return Ok(false);
    };
    let destination = legacy.with_extension(
        source
            .extension()
            .context("system configuration has no extension")?,
    );
    let lock = WriteLock::new(&destination)?;
    // Another setup process may have finished while this one waited for the directory.
    if [&legacy, &toml]
        .iter()
        .any(|path| std::fs::symlink_metadata(path).is_ok())
    {
        return Ok(false);
    }
    lock.check_stamp(None)?;
    let mut text = String::new();
    crate::regular::open(&source)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= MAX_CONFIG_BYTES,
        "{}",
        mandara_i18n::t("MangoWM's system configuration is too large to copy safely.")
    );
    let mut before = doctor::check_with(Target::Mango, &source, env);
    ensure!(
        before.status == CheckerStatus::Ran && before.problems() == 0,
        "{}\n{}",
        mandara_i18n::t("MangoWM could not validate the configuration. Nothing was changed."),
        before.raw
    );
    before.root = destination.clone();
    fix::backup_files(
        Target::Mango,
        std::slice::from_ref(&destination),
        &store.backups,
    )?;
    let written = Cell::new(None);
    let result = fix::guarded_write_in(
        &before,
        env,
        &|| match crate::store::write_atomic_checked(&lock, None, text.as_bytes()) {
            Ok(stamp) => {
                written.set(Some(stamp));
                Ok(())
            }
            Err(error) => {
                written.set(crate::store::published_stamp(&error));
                Err(error)
            }
        },
        &destination,
        store,
    );
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            if let Some(stamp) = written.get() {
                remove_written(&lock, stamp).context(format!("{error:#}"))?;
            }
            return Err(error);
        }
    };
    ensure!(
        !outcome.rolled_back,
        "{}",
        mandara_i18n::t("MangoWM rejected the configuration change. Previous settings were kept.")
    );
    Ok(true)
}

fn remove_written(lock: &WriteLock, stamp: FileStamp) -> Result<()> {
    lock.check_stamp(Some(stamp))?;
    std::fs::remove_file(lock.target())?;
    std::fs::File::open(
        lock.target()
            .parent()
            .context("configuration has no parent")?,
    )?
    .sync_all()?;
    Ok(())
}
