//! MangoWM's native config location and format-preserving TOML include.

use anyhow::{Context as _, Result, ensure};
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

/// MangoWM reads HOME directly; XDG_CONFIG_HOME belongs to Mandara's own files.
pub(super) fn user_file(home: &Path) -> PathBuf {
    let legacy = home.join(".config/mango/config.conf");
    let toml = legacy.with_extension("toml");
    if !legacy.exists() && toml.exists() {
        toml
    } else {
        legacy
    }
}

/// Extensions take precedence; MangoWM sniffs unknown extensions for a TOML table.
pub(crate) fn is_toml(file: &Path, source: &str) -> bool {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("toml") => true,
        Some(extension) if extension.eq_ignore_ascii_case("conf") => false,
        _ => source
            .lines()
            .map(str::trim_start)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .is_some_and(|line| line.starts_with('[')),
    }
}

pub(super) fn include_hint(file: &Path, generated: &Path) -> String {
    if file
        .extension()
        .is_some_and(|extension| extension == "toml")
    {
        format!(
            "[global]\nsource_optional = [{}]",
            super::toml_str(&generated.display().to_string())
        )
    } else {
        format!("source_optional={}", generated.display())
    }
}

pub(super) fn add_include(text: &str, path: &str) -> Result<Option<String>> {
    let mut document: DocumentMut = text.parse().context(mandara_i18n::t(
        "MangoWM's configuration is not valid TOML.",
    ))?;
    let listed = |item: Option<&Item>| {
        item.is_some_and(|item| {
            item.as_str() == Some(path)
                || item
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(path)))
        })
    };
    if let Some(global) = document.get("global").and_then(Item::as_table_like)
        && (listed(global.get("source")) || listed(global.get("source_optional")))
    {
        return Ok(None);
    }
    let global = document
        .entry("global")
        .or_insert_with(|| Item::Table(Table::new()));
    let global = global.as_table_like_mut().context(mandara_i18n::t(
        "MangoWM's global settings must be a table.",
    ))?;
    let sources = global
        .entry("source_optional")
        .or_insert_with(|| Item::Value(Value::Array(Array::new())));
    if let Some(previous) = sources.as_str() {
        let mut values = Array::new();
        values.push(previous);
        values.push(path);
        let decor = sources.as_value().unwrap().decor().clone();
        let mut value = Value::Array(values);
        *value.decor_mut() = decor;
        *sources = Item::Value(value);
    } else {
        sources
            .as_array_mut()
            .context(mandara_i18n::t(
                "MangoWM's optional sources must be a path or a list of paths.",
            ))?
            .push(path);
    }
    Ok(Some(document.to_string()))
}

pub(super) fn install_include(path: &Path) -> Result<bool> {
    use super::Target;
    use crate::doctor::{self, CheckEnv, CheckerStatus, fix};
    if super::include_installed_at(Target::Mango, path) {
        return Ok(false);
    }
    let env = CheckEnv::default();
    let before = doctor::check_with(Target::Mango, path, &env);
    ensure!(
        before.status == CheckerStatus::Ran,
        "{}\n{}",
        mandara_i18n::t("MangoWM could not validate the configuration. Nothing was changed."),
        before.raw
    );
    fix::backup_files(Target::Mango, &[path.to_path_buf()], &fix::backups_root())?;
    let added = std::cell::Cell::new(false);
    let outcome = fix::guarded_write_in(
        &before,
        &env,
        &|| {
            added.set(super::install_include_at(Target::Mango, path)?);
            Ok(())
        },
        path,
        &fix::Store::default(),
    )?;
    ensure!(
        !outcome.rolled_back,
        "{}",
        mandara_i18n::t("MangoWM rejected the configuration change. Previous settings were kept.")
    );
    Ok(added.get())
}
