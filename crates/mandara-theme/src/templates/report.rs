use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Completed writes and failures from a theme update or restoration.
#[derive(Default)]
pub struct SyncReport {
    pub files: Vec<PathBuf>,
    pub errors: BTreeMap<String, String>,
}

impl SyncReport {
    pub(super) fn failed(&mut self, path: &Path, error: impl std::fmt::Display) {
        self.errors.insert(path.display().to_string(), error.to_string());
    }

    pub fn append(&mut self, other: Self) {
        self.files.extend(other.files);
        self.errors.extend(other.errors);
    }

    pub(super) fn into_files(self) -> Vec<PathBuf> {
        for (path, error) in self.errors {
            tracing::warn!("theme sync {path}: {error}");
        }
        self.files
    }
}
