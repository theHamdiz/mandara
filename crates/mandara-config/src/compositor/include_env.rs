//! Resolve native includes without borrowing another process's environment.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};

/// Reject expansion before allocating beyond a Linux pathname's byte limit.
pub(super) const MAX_PATH_BYTES: usize = 4096;

/// None keeps legacy callers on their own environment; captured maps are
/// authoritative, including when a variable is absent. Values are never logged.
#[derive(Default)]
pub struct IncludeEnvironment(Option<BTreeMap<OsString, OsString>>);

impl std::fmt::Debug for IncludeEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IncludeEnvironment")
            .field("captured", &self.0.is_some())
            .field("variables", &self.0.as_ref().map(BTreeMap::len))
            .finish()
    }
}

impl IncludeEnvironment {
    pub fn captured(values: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        Self(Some(values.into_iter().collect()))
    }

    pub(super) fn value(&self, key: &str) -> Option<Cow<'_, OsStr>> {
        match &self.0 {
            Some(values) => values
                .get(OsStr::new(key))
                .map(|value| Cow::Borrowed(value.as_os_str())),
            None => std::env::var_os(key).map(Cow::Owned),
        }
    }

    pub(crate) fn expand(&self, raw: &str) -> Result<OsString> {
        ensure!(
            raw.len() <= MAX_PATH_BYTES,
            mandara_i18n::t("An include path is too long.")
        );
        let mut result = OsString::new();
        let mut rest = raw;
        while let Some(at) = rest.find('$') {
            append(&mut result, OsStr::new(&rest[..at]))?;
            let tail = &rest[at + 1..];
            let (name, next) = if let Some(braced) = tail.strip_prefix('{') {
                braced
                    .find('}')
                    .map_or(("", tail), |end| (&braced[..end], &braced[end + 1..]))
            } else {
                let end = tail
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .unwrap_or(tail.len());
                (&tail[..end], &tail[end..])
            };
            if name.is_empty() {
                append(&mut result, OsStr::new("$"))?;
            } else if let Some(value) = self.value(name) {
                append(&mut result, &value)?;
            }
            rest = next;
        }
        append(&mut result, OsStr::new(rest))?;
        Ok(result)
    }

    /// Niri expands only a leading home component; variable expansion is a
    /// separate step for the dialects that support it.
    pub(crate) fn resolve(&self, including: &Path, raw: &OsStr) -> Result<PathBuf> {
        ensure!(
            raw.len() <= MAX_PATH_BYTES,
            mandara_i18n::t("An include path is too long.")
        );
        let path = Path::new(raw);
        let path = if let Ok(rest) = path.strip_prefix("~") {
            let home = self.value("HOME").context(mandara_i18n::t(
                "The compositor's configuration path is missing.",
            ))?;
            ensure!(
                home.len()
                    .checked_add(rest.as_os_str().len())
                    .is_some_and(|bytes| bytes < MAX_PATH_BYTES),
                mandara_i18n::t("An include path is too long.")
            );
            PathBuf::from(&*home).join(rest)
        } else {
            path.to_path_buf()
        };
        let path = if path.is_absolute() {
            path
        } else {
            including.parent().unwrap_or(Path::new(".")).join(path)
        };
        ensure!(
            path.as_os_str().len() <= MAX_PATH_BYTES,
            mandara_i18n::t("An include path is too long.")
        );
        Ok(path)
    }
}

fn append(out: &mut OsString, value: &OsStr) -> Result<()> {
    ensure!(
        out.len()
            .checked_add(value.len())
            .is_some_and(|bytes| bytes <= MAX_PATH_BYTES),
        mandara_i18n::t("An include path is too long.")
    );
    out.push(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_paths_are_authoritative_redacted_and_bounded_before_expansion() {
        let environment = IncludeEnvironment::captured([
            ("HOME".into(), "/server/home".into()),
            ("XDG_CONFIG_HOME".into(), "/server/config".into()),
            ("TOKEN".into(), "private-value".into()),
        ]);
        let file = Path::new("/root/config");
        assert_eq!(
            environment.resolve(file, OsStr::new("~/keys")).unwrap(),
            Path::new("/server/home/keys")
        );
        assert_eq!(
            environment
                .resolve(
                    file,
                    &environment.expand("${XDG_CONFIG_HOME}/keys").unwrap()
                )
                .unwrap(),
            Path::new("/server/config/keys")
        );
        assert_eq!(
            environment.expand("$UNSET/keys").unwrap(),
            OsStr::new("/keys")
        );
        assert!(!format!("{environment:?}").contains("private-value"));
        let oversized =
            IncludeEnvironment::captured([("BIG".into(), "x".repeat(MAX_PATH_BYTES + 1).into())]);
        assert!(oversized.expand("$BIG").is_err());
        assert!(
            IncludeEnvironment::captured([])
                .resolve(file, OsStr::new("~/keys"))
                .is_err()
        );
    }
}
