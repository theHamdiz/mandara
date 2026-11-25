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
    visits: usize,
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
        if super::is_generated(&real) {
            return Ok(None);
        }
        self.read_real(file, real).map(Some)
    }

    pub(super) fn read_all(&mut self, file: &Path) -> Result<String> {
        self.read_real(file, std::fs::canonicalize(file)?)
    }

    pub(super) fn snapshot(&mut self, file: &Path, text: &str) -> Result<()> {
        self.visit(std::fs::canonicalize(file)?)?;
        self.retain(text.len())
    }

    fn visit(&mut self, real: PathBuf) -> Result<()> {
        self.seen.insert(real);
        self.visits += 1;
        ensure!(
            self.visits <= crate::keybinds::MAX_BINDINGS,
            mandara_i18n::t("The shortcut configuration includes too many files.")
        );
        ensure!(
            self.seen.len() <= crate::keybinds::MAX_FILES,
            mandara_i18n::t("The shortcut configuration includes too many files.")
        );
        Ok(())
    }

    fn read_real(&mut self, file: &Path, real: PathBuf) -> Result<String> {
        self.visit(real)?;
        let input = crate::regular::open(file)?;
        let mut text = String::new();
        input
            .take(crate::keybinds::MAX_FILE_BYTES as u64 + 1)
            .read_to_string(&mut text)?;
        ensure!(
            text.len() <= crate::keybinds::MAX_FILE_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        self.retain(text.len())?;
        Ok(text)
    }

    fn retain(&mut self, bytes: usize) -> Result<()> {
        self.bytes += bytes;
        ensure!(
            self.bytes <= crate::keybinds::MAX_TOTAL_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        Ok(())
    }
}
