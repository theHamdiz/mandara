//! Lossless, field-by-field editing of native compositor shortcuts.

pub mod cosmic;
#[cfg(test)]
mod continuation_tests;
mod edit;
mod encoding;
mod formats;
mod identity;
mod kwin_settings;
pub(crate) mod logical_lines;
mod qt_keys;
mod qt_sequence;
mod qt_xkb;
pub mod river;
mod scan;
mod text_undo;

use std::{ops::Range, path::PathBuf};

use anyhow::{Result, ensure};
use mandara_i18n::tk;

pub use edit::{PreparedEdit, UndoEdit, prepare};
pub use encoding::Encoding;
pub use formats::aqueous::normalized as aqueous_chord;
pub(crate) use formats::hyprland_lua;
pub use formats::labwc_keyboard;
pub use kwin_settings::same_settings as kwin_same_settings;
pub use kwin_settings::same_shortcut_settings as kwin_same_shortcut_settings;
pub use kwin_settings::validate_settings as kwin_validate_settings;
pub use qt_sequence::keybind as qt_keybind;
pub use qt_sequence::sequence as qt_sequence;
pub use scan::{
    Scan, Source, default_root, default_root_in, read_source, resolve_niri_mod, scan, scan_from,
    scan_roots, scan_roots_in,
};
pub use text_undo::TextUndo;

/// Bound one config file before parsing it or retaining its field values.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
/// A scan is finite even with recursive includes or generated binding lists.
pub const MAX_FILES: usize = 64;
pub const MAX_BINDINGS: usize = 4096;
pub const MAX_TOTAL_BYTES: usize = 8 * MAX_FILE_BYTES;
/// Prevent a single edit from retaining an unbounded field or argument list.
pub const MAX_FIELD_BYTES: usize = 64 * 1024;
pub const MAX_FIELDS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Field {
    pub label: &'static str,
    pub name: Option<String>,
    pub value: String,
    pub range: Range<usize>,
    pub encoding: Encoding,
    pub read_only: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Binding {
    pub line: usize,
    pub scope: String,
    /// Normalized only when the compositor's key expression is statically known.
    pub chord: Option<String>,
    pub fields: Vec<Field>,
    pub read_only: Option<&'static str>,
}

/// A source span is meaningful only in the exact scanned version of its file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BindingId {
    pub file: PathBuf,
    pub target: PathBuf,
    #[serde(with = "identity")]
    pub fingerprint: u64,
    pub index: usize,
}

pub trait ShortcutFormat: Sync {
    fn parse(&self, source: &str) -> Result<Vec<Binding>>;
    fn validate_edit(&self, _before: &Binding, _after: &Binding) -> Result<()> {
        Ok(())
    }
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
    let bindings = formats::format(target, file, source).parse(source)?;
    validate_bindings(&bindings)?;
    Ok(bindings)
}

fn validate_bindings(bindings: &[Binding]) -> Result<()> {
    ensure!(
        bindings.len() <= MAX_BINDINGS,
        "{}",
        mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
    );
    ensure!(
        bindings.iter().all(|row| row.fields.len() <= MAX_FIELDS
            && row
                .fields
                .iter()
                .all(|field| field.value.len() <= MAX_FIELD_BYTES)),
        "{}",
        mandara_i18n::t("A shortcut has too many arguments or an oversized value.")
    );
    Ok(())
}

/// Patch only the selected values. Surrounding comments and formatting stay exact.
pub fn patch<'a>(
    source: &'a str,
    binding: &Binding,
    values: &[String],
) -> Result<std::borrow::Cow<'a, str>> {
    ensure!(
        binding.read_only.is_none(),
        "{}",
        mandara_i18n::t(binding.read_only.unwrap_or_default())
    );
    ensure!(
        binding.fields.len() == values.len(),
        "shortcut fields changed"
    );
    ensure!(
        values.len() <= MAX_FIELDS && values.iter().all(|value| value.len() <= MAX_FIELD_BYTES),
        "{}",
        mandara_i18n::t("A shortcut has too many arguments or an oversized value.")
    );
    let mut changes = Vec::new();
    for (field, value) in binding.fields.iter().zip(values) {
        ensure!(
            source.get(field.range.clone()).is_some(),
            "shortcut span is stale"
        );
        if *value != field.value {
            ensure!(
                field.read_only.is_none(),
                "{}",
                mandara_i18n::t(field.read_only.unwrap_or_default())
            );
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
    if changes.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(source));
    }
    let mut next = source.to_owned();
    for (range, value) in changes.into_iter().rev() {
        next.replace_range(range, &value);
    }
    ensure!(
        next.len() <= MAX_FILE_BYTES,
        "{}",
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    Ok(std::borrow::Cow::Owned(next))
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
        read_only: None,
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
pub fn normalize(combo: &str) -> Option<String> {
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
pub(crate) const SOURCE_ONLY: &str =
    tk("Edit this value in its source configuration to preserve its formatting.");

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
