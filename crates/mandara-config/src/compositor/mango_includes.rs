//! MangoWM include connections and relocation checks share native path semantics.

use super::{IncludeEnvironment, mango_toml, read_budget::ReadBudget};
use anyhow::{Result, ensure};
use std::{
    borrow::Cow,
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
};

/// MangoWM's variable expander uses a 128-byte name buffer, including its terminator.
const VARIABLE_NAME_BYTES: usize = 128;
/// MangoWM 0.18.5's `resolve_config_path` writes into this buffer, including NUL.
pub(super) const PATH_BYTES: usize = 1024;
/// Its TOML variable reader has a separate decoded-value buffer, including NUL.
const TOML_VARIABLE_BYTES: usize = 2048;

pub(super) fn validate_generated_path(path: &Path) -> Result<()> {
    ensure!(
        path.as_os_str().len() < PATH_BYTES,
        mandara_i18n::t("An include path is too long.")
    );
    Ok(())
}

pub(super) fn connected(root: &Path, generated: &Path) -> Result<bool> {
    let mut walk = Walk::new(root, Some(generated), None)?;
    walk.file(root)?;
    Ok(walk.found)
}

pub(super) fn preserve_targets(root: &Path, destination: &Path, text: &str) -> Result<()> {
    let mut walk = Walk::new(root, None, Some(destination))?;
    walk.budget.snapshot(root, text)?;
    walk.active.insert(std::fs::canonicalize(root)?);
    walk.source(root, text)
}

struct Walk<'a> {
    root: &'a Path,
    generated: Option<&'a Path>,
    relocated: Option<&'a Path>,
    cwd: PathBuf,
    environment: IncludeEnvironment,
    variables: IncludeEnvironment,
    variable_bytes: usize,
    budget: ReadBudget,
    active: BTreeSet<PathBuf>,
    targets: BTreeSet<PathBuf>,
    found: bool,
}

impl<'a> Walk<'a> {
    fn new(
        root: &'a Path,
        generated: Option<&'a Path>,
        relocated: Option<&'a Path>,
    ) -> Result<Self> {
        Ok(Self {
            root,
            generated,
            relocated,
            cwd: std::env::current_dir()?.join("config"),
            environment: IncludeEnvironment::default(),
            variables: IncludeEnvironment::captured([]),
            variable_bytes: 0,
            budget: ReadBudget::default(),
            active: BTreeSet::new(),
            targets: BTreeSet::new(),
            found: false,
        })
    }

    fn file(&mut self, file: &Path) -> Result<()> {
        let real = std::fs::canonicalize(file)?;
        ensure!(
            self.active.insert(real.clone()),
            mandara_i18n::t("The shortcut configuration contains an include cycle.")
        );
        let result = (|| {
            let text = self.budget.read_all(file)?;
            self.source(file, &text)
        })();
        self.active.remove(&real);
        result
    }

    fn source(&mut self, file: &Path, text: &str) -> Result<()> {
        if mango_toml(file, text) {
            let document: toml_edit::DocumentMut = text.parse()?;
            for (section, value) in document.iter() {
                let Some(table) = value.as_table_like() else {
                    continue;
                };
                for (key, value) in table.iter() {
                    if section == "var" {
                        if let Some(value) = value.as_value() {
                            self.define_toml(key, value)?;
                        }
                    } else if section == "global" {
                        if let Some(value) = value.as_str() {
                            self.option(key, value)?;
                        } else if let Some(values) = value.as_array() {
                            for value in values.iter().filter_map(toml_edit::Value::as_str) {
                                self.option(key, value)?;
                                if self.found {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    if self.found {
                        return Ok(());
                    }
                }
            }
        } else {
            for line in text
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with('#'))
            {
                if let Some((key, value)) = line.split_once('=') {
                    self.option(key.trim(), value.trim())?;
                    if self.found {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn option(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "var" => {
                if let Some((name, value)) = value.split_once(',') {
                    self.define(name.trim(), value.trim())?;
                }
            }
            "source" | "source_optional" => self.include(value, key == "source_optional")?,
            _ => {}
        }
        Ok(())
    }

    fn define(&mut self, name: &str, value: &str) -> Result<()> {
        let value = self.variables.expand_config(value)?;
        self.store_variable(name, value)
    }

    fn define_toml(&mut self, name: &str, value: &toml_edit::Value) -> Result<()> {
        let raw = if let Some(value) = value.as_str() {
            Cow::Borrowed(value)
        } else {
            let mut value = value.clone();
            value.decor_mut().clear();
            Cow::Owned(value.to_string())
        };
        let end = raw.len().min(TOML_VARIABLE_BYTES - 1);
        let mut boundary = end;
        while !raw.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let mut expanded = self.variables.expand_config(&raw[..boundary])?;
        expanded.push(OsStr::from_bytes(&raw.as_bytes()[boundary..end]));
        self.store_variable(name, expanded)
    }

    fn store_variable(&mut self, name: &str, value: OsString) -> Result<()> {
        let expanded_name = self.variables.expand_config(name)?;
        let name = expanded_name
            .to_str()
            .filter(|name| name.is_ascii())
            .ok_or_else(|| {
                anyhow::anyhow!(mandara_i18n::t(
                    "The shortcut configuration is too large to edit."
                ))
            })?
            .trim();
        ensure!(
            !name.is_empty()
                && name.len() < VARIABLE_NAME_BYTES
                && name.bytes().enumerate().all(|(i, byte)| byte == b'_'
                    || byte.is_ascii_alphabetic()
                    || i > 0 && byte.is_ascii_digit()),
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        let previous = self.variables.value(name).map_or(0, |value| value.len());
        self.variable_bytes = self.variable_bytes - previous + value.len();
        ensure!(
            self.variable_bytes <= crate::keybinds::MAX_TOTAL_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        self.variables.define(name, value)
    }

    fn include(&mut self, raw: &str, optional: bool) -> Result<()> {
        let expanded = self.variables.expand_config(raw)?;
        let path = self.resolve(self.root, &expanded)?;
        if let Some(destination) = self.relocated {
            let relocated = self.resolve(destination, &expanded)?;
            ensure!(
                same_path(&path, &relocated),
                mandara_i18n::t(
                    "Copying MangoWM's system configuration would change its include paths. Create your user configuration manually before running setup."
                )
            );
        }
        self.targets.insert(path.clone());
        ensure!(
            self.targets.len() <= crate::keybinds::MAX_FILES,
            mandara_i18n::t("The shortcut configuration includes too many files.")
        );
        if self
            .generated
            .is_some_and(|generated| same_path(generated, &path))
        {
            self.found = true;
        } else if !optional || path.try_exists()? {
            self.file(&path)?;
        }
        Ok(())
    }

    fn resolve(&self, root: &Path, raw: &OsStr) -> Result<PathBuf> {
        // Only ./ is relative to the primary config; bare paths use MangoWM's cwd.
        let native = if let Some(relative) = raw.as_bytes().strip_prefix(b"./") {
            root.parent()
                .unwrap_or(Path::new("."))
                .join(OsStr::from_bytes(relative))
        } else if Path::new(raw).starts_with("~") {
            self.environment.resolve(&self.cwd, raw)?
        } else {
            PathBuf::from(raw)
        };
        let mut bytes = native.into_os_string().into_vec();
        bytes.truncate(PATH_BYTES - 1);
        let native = PathBuf::from(OsString::from_vec(bytes));
        Ok(if native.is_absolute() {
            native
        } else {
            self.cwd.parent().unwrap_or(Path::new(".")).join(native)
        })
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || std::fs::canonicalize(a).is_ok_and(|a| std::fs::canonicalize(b).is_ok_and(|b| a == b))
}

#[cfg(test)]
#[path = "mango_include_tests.rs"]
mod tests;
