//! Lossless, field-by-field editing of native compositor shortcuts.

mod encoding;
mod formats;
mod scan;

use std::{ops::Range, path::PathBuf};

use anyhow::{Result, ensure};
use mandara_i18n::tk;

pub use encoding::Encoding;
pub use scan::{Scan, Source, scan, scan_from};

/// Bound one config file before parsing it or retaining its field values.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
/// A scan is finite even with recursive includes or generated binding lists.
pub const MAX_FILES: usize = 64;
pub const MAX_BINDINGS: usize = 4096;
pub const MAX_TOTAL_BYTES: usize = 8 * MAX_FILE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub name: Option<String>,
    pub value: String,
    pub range: Range<usize>,
    pub encoding: Encoding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub line: usize,
    pub scope: String,
    /// Normalized only when the compositor's key expression is statically known.
    pub chord: Option<String>,
    pub fields: Vec<Field>,
    pub read_only: Option<&'static str>,
}

/// A source span is meaningful only in the exact scanned version of its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingId {
    pub file: PathBuf,
    pub fingerprint: u64,
    pub index: usize,
}

pub trait ShortcutFormat: Sync {
    fn parse(&self, source: &str) -> Result<Vec<Binding>>;
}

pub fn parse(
    target: crate::compositor::Target,
    file: &std::path::Path,
    source: &str,
) -> Result<Vec<Binding>> {
    ensure!(
        source.len() <= MAX_FILE_BYTES,
        "{}",
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    let bindings = formats::format(target, file).parse(source)?;
    ensure!(
        bindings.len() <= MAX_BINDINGS,
        "{}",
        mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
    );
    Ok(bindings)
}

/// Patch only the selected values. Surrounding comments and formatting stay exact.
pub fn patch(source: &str, binding: &Binding, values: &[String]) -> Result<String> {
    ensure!(
        binding.read_only.is_none(),
        "{}",
        mandara_i18n::t(binding.read_only.unwrap_or_default())
    );
    ensure!(
        binding.fields.len() == values.len(),
        "shortcut fields changed"
    );
    let mut changes = Vec::new();
    for (field, value) in binding.fields.iter().zip(values) {
        ensure!(
            source.get(field.range.clone()).is_some(),
            "shortcut span is stale"
        );
        if *value != field.value {
            changes.push((field.range.clone(), field.encoding.encode(value)?));
        }
    }
    changes.sort_unstable_by_key(|(range, _)| range.start);
    ensure!(
        changes
            .windows(2)
            .all(|pair| pair[0].0.end <= pair[1].0.start),
        "shortcut spans overlap"
    );
    let mut next = source.to_owned();
    for (range, value) in changes.into_iter().rev() {
        next.replace_range(range, &value);
    }
    ensure!(
        next.len() <= MAX_FILE_BYTES,
        "{}",
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    Ok(next)
}

pub(crate) fn field(
    label: &'static str,
    value: impl Into<String>,
    range: Range<usize>,
    encoding: Encoding,
) -> Field {
    Field {
        label,
        name: None,
        value: value.into(),
        range,
        encoding,
    }
}

impl Field {
    pub(crate) fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

pub(crate) fn line(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset.min(source.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Do not guess variables, keycodes or compositor-specific `Mod` aliases.
pub(crate) fn normalize(combo: &str) -> Option<String> {
    if combo.contains(['$', ':'])
        || combo
            .split('+')
            .any(|part| part.trim().eq_ignore_ascii_case("mod"))
    {
        return None;
    }
    let mut parts = combo.rsplitn(2, '+');
    let key = parts.next()?.trim();
    if key.is_empty() {
        return None;
    }
    let modifiers = parts.next().unwrap_or_default();
    if modifiers.split('+').any(|part| {
        !part.trim().is_empty()
            && !part.trim().eq_ignore_ascii_case("none")
            && crate::compositor::normalize_mod(part).is_none()
    }) {
        return None;
    }
    Some(crate::compositor::chord(
        modifiers.split('+').map(str::trim),
        &crate::compositor::keysym_for(key),
    ))
}

pub(crate) const DYNAMIC: &str = tk(
    "This shortcut is computed by the compositor configuration. Edit its source expression in your editor.",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_changes_only_selected_bytes_and_rejects_overlapping_or_missing_fields() {
        let source = "prefix 'old' // keep me\n";
        let mut binding = Binding {
            line: 1,
            scope: String::new(),
            chord: None,
            fields: vec![field(tk("Action"), "old", 7..12, Encoding::TomlString)],
            read_only: None,
        };
        assert_eq!(
            patch(source, &binding, &["new value".into()]).unwrap(),
            "prefix \"new value\" // keep me\n"
        );
        assert!(patch(source, &binding, &[]).is_err());
        binding.fields.push(binding.fields[0].clone());
        assert!(patch(source, &binding, &["a".into(), "b".into()]).is_err());
    }

    #[test]
    fn symbolic_chords_remain_unresolved() {
        assert!(normalize("$mod+Q").is_none());
        assert!(normalize("Mod+Q").is_none());
        assert!(normalize("Super+code:24").is_none());
        assert_eq!(normalize("Control+Alt+q"), Some("alt+ctrl+q".into()));
    }
}
