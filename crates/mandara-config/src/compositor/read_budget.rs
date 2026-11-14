//! Finite source reads for native shortcut conflict checks.

use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    io::Read as _,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(super) struct ReadBudget {
    seen: BTreeSet<PathBuf>,
    bytes: usize,
}

impl ReadBudget {
    pub(super) fn remaining_files(&self) -> usize {
        crate::keybinds::MAX_FILES.saturating_sub(self.seen.len())
    }

    pub(super) fn read(&mut self, file: &Path) -> Result<Option<String>> {
        if super::is_generated(file) {
            return Ok(None);
        }
        let real = std::fs::canonicalize(file)?;
        if super::is_generated(&real) || !self.seen.insert(real) {
            return Ok(None);
        }
        ensure!(
            self.seen.len() <= crate::keybinds::MAX_FILES,
            mandara_i18n::t("The shortcut configuration includes too many files.")
        );
        let input = crate::regular::open(file)?;
        let mut text = String::new();
        input
            .take(crate::keybinds::MAX_FILE_BYTES as u64 + 1)
            .read_to_string(&mut text)?;
        ensure!(
            text.len() <= crate::keybinds::MAX_FILE_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        self.bytes += text.len();
        ensure!(
            self.bytes <= crate::keybinds::MAX_TOTAL_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        Ok(Some(text))
    }
}
