//! Retain one small inverse edit, never a duplicate of an entire shortcut map.

use super::scan::fingerprint;
use anyhow::{Result, ensure};
use std::{ops::Range, path::PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TextUndo {
    pub file: PathBuf,
    pub target: PathBuf,
    #[serde(with = "super::identity")]
    fingerprint: u64,
    range: Range<usize>,
    previous: String,
}

impl TextUndo {
    pub fn between(file: PathBuf, target: PathBuf, before: &str, after: &str) -> Self {
        let start = before
            .chars()
            .zip(after.chars())
            .take_while(|(a, b)| a == b)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        let tail = before[start..]
            .chars()
            .rev()
            .zip(after[start..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        Self {
            file,
            target,
            fingerprint: fingerprint(after),
            range: start..after.len() - tail,
            previous: before[start..before.len() - tail].into(),
        }
    }

    pub fn apply(&self, current: &str) -> Result<String> {
        ensure!(
            fingerprint(current) == self.fingerprint && current.get(self.range.clone()).is_some(),
            "{}",
            mandara_i18n::t(
                "The compositor shortcuts changed while they were being checked. Refresh the list and try again."
            )
        );
        let mut result =
            String::with_capacity(current.len() - self.range.len() + self.previous.len());
        result.push_str(&current[..self.range.start]);
        result.push_str(&self.previous);
        result.push_str(&current[self.range.end..]);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inverse_edits_keep_only_changed_utf8_bytes_and_reject_foreign_changes() {
        let before = format!("{}}} // نهاية", " ".repeat(128 * 1024));
        let after = before.replacen('}', "(modifiers:[Super],key:\"q\"): Close,\n}", 1);
        let undo = TextUndo::between("custom".into(), "custom".into(), &before, &after);
        assert!(undo.previous.len() < 4);
        assert_eq!(undo.apply(&after).unwrap(), before);
        assert!(undo.apply(&(after.clone() + " ")).is_err());
        let redo = TextUndo::between("custom".into(), "custom".into(), &after, &before);
        assert_eq!(redo.apply(&before).unwrap(), after);
    }
}
