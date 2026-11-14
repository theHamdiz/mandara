//! Ordered Mango shortcut conflicts, with native mode filtering in both source dialects.

use super::*;

pub(super) fn chords_from(
    root: &Path,
    keys: &mango_keys::MangoKeys,
    limit: usize,
    read: &mut dyn FnMut(&Path) -> Result<Option<String>>,
    environment: &IncludeEnvironment,
) -> Result<BTreeSet<String>> {
    struct Walk<'k, 'r, 'e> {
        keys: &'k mango_keys::MangoKeys,
        read: &'r mut dyn FnMut(&Path) -> Result<Option<String>>,
        limit: usize,
        visited: BTreeSet<PathBuf>,
        binds: Vec<(String, String)>,
        retained: usize,
        ours: Option<String>,
        environment: &'e IncludeEnvironment,
    }
    fn walk(w: &mut Walk, file: &Path, mode: String) -> Result<String> {
        let key = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        anyhow::ensure!(
            w.visited.insert(key.clone()),
            mandara_i18n::t("The shortcut configuration contains an include cycle.")
        );
        let result = (|| {
            let Some(text) = (w.read)(file)? else {
                return Ok(mode);
            };
            if file
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                return toml_file(w, file, &text, mode);
            }
            let mut mode = mode;
            // Split the file at its `source=` lines so each part starts in the
            // mode the previous part (or sourced file) left.
            let mut chunk = String::new();
            for line in text.lines() {
                let sourced = (!line.trim_start().starts_with('#'))
                    .then(|| include_path(Target::Mango, line))
                    .flatten();
                match sourced {
                    Some(path) => {
                        mode = flush(w, &mut chunk, mode)?;
                        let path = w
                            .environment
                            .resolve(file, &w.environment.expand(path.trim().trim_matches('"'))?)?;
                        if path == Target::Mango.generated_file()
                            || is_generated(&path)
                            || std::fs::canonicalize(&path).is_ok_and(|path| is_generated(&path))
                        {
                            w.ours.get_or_insert_with(|| mode.clone());
                        } else if path.try_exists()?
                            || !line.trim_start().starts_with("source_optional")
                        {
                            mode = walk(w, &path, mode)?;
                        }
                    }
                    None => {
                        chunk.push_str(line);
                        chunk.push('\n');
                    }
                }
            }
            flush(w, &mut chunk, mode)
        })();
        w.visited.remove(&key);
        result
    }
    fn flush(w: &mut Walk, chunk: &mut String, mode: String) -> Result<String> {
        let (binds, mode) = mango_binds_checked(
            chunk,
            &mode,
            w.keys,
            &mut w.retained,
            w.limit.saturating_sub(w.binds.len()),
        )?;
        w.binds.extend(binds.into_iter().map(|b| (b.chord, b.mode)));
        anyhow::ensure!(
            w.binds.len() <= w.limit,
            mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
        );
        chunk.clear();
        Ok(mode)
    }
    fn toml_file(w: &mut Walk, file: &Path, text: &str, inherited: String) -> Result<String> {
        let document: toml::Value = toml::from_str(text)?;
        let mode = document
            .get("global")
            .and_then(|global| global.get("key_mode"))
            .and_then(toml::Value::as_str)
            .unwrap_or(&inherited);
        anyhow::ensure!(
            mode.len() <= crate::keybinds::MAX_FIELD_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        let mut mode = mode.to_owned();
        if let Some(table) = document.get("bind").and_then(toml::Value::as_table) {
            toml_table(w, table, &mode, false)?;
        }
        for path in crate::keybinds::mango_includes(file, text, w.environment)? {
            if is_generated(&path)
                || std::fs::canonicalize(&path).is_ok_and(|path| is_generated(&path))
            {
                w.ours.get_or_insert_with(|| mode.clone());
            } else {
                mode = walk(w, &path, mode)?;
            }
        }
        Ok(mode)
    }
    fn toml_table(w: &mut Walk, table: &toml::Table, mode: &str, nested: bool) -> Result<()> {
        for (key, value) in table {
            if let Some(child) = value.as_table() {
                anyhow::ensure!(
                    key.len() <= crate::keybinds::MAX_FIELD_BYTES,
                    mandara_i18n::t("The shortcut configuration is too large to edit.")
                );
                toml_table(w, child, if nested { mode } else { key }, true)?;
            } else if value.is_str()
                || value
                    .as_array()
                    .is_some_and(|values| values.iter().any(toml::Value::is_str))
            {
                anyhow::ensure!(
                    key.len() <= crate::keybinds::MAX_FIELD_BYTES,
                    mandara_i18n::t("The shortcut configuration is too large to edit.")
                );
                anyhow::ensure!(
                    w.binds.len() < w.limit,
                    mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
                );
                let chord = crate::keybinds::normalize(key).ok_or_else(|| {
                    anyhow::anyhow!(mandara_i18n::t(
                        "Computed shortcuts must be edited in their source configuration."
                    ))
                })?;
                let (mods, key) = chord.rsplit_once('+').unwrap_or(("", &chord));
                let chord = mango_chord(
                    mods.split('+').filter(|value| !value.is_empty()),
                    w.keys.canonical(key, false),
                );
                retain_mango_binding(&mut w.retained, &chord, mode)?;
                w.binds.push((chord, mode.to_owned()));
                anyhow::ensure!(
                    w.binds.len() <= w.limit,
                    mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
                );
            }
        }
        Ok(())
    }
    let mut w = Walk {
        keys,
        read,
        limit,
        visited: BTreeSet::new(),
        binds: Vec::new(),
        retained: 0,
        ours: None,
        environment,
    };
    walk(&mut w, root, mango_keys::DEFAULT_MODE.to_string())?;
    let ours = w
        .ours
        .unwrap_or_else(|| mango_keys::DEFAULT_MODE.to_string());
    Ok(w.binds
        .into_iter()
        .filter(|(_, mode)| mango_keys::modes_meet(mode, &ours))
        .map(|(chord, _)| chord)
        .collect())
}
