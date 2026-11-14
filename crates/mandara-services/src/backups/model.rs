//! Backup preferences and the small snapshots retained by an open panel.

use std::path::PathBuf;
use anyhow::{Result, ensure};
use mandara_i18n::t;
use serde::{Deserialize, Serialize};

/// Bound command arguments independently of user configuration size.
pub const MAX_SOURCES: usize = 32;
pub const MAX_PATH_BYTES: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub repository: PathBuf,
    pub password_file: PathBuf,
    pub sources: Vec<PathBuf>,
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!([&self.repository, &self.password_file].into_iter().all(|path| path.is_absolute() && path.as_os_str().len() <= MAX_PATH_BYTES), "{}", t("Choose an absolute repository path and password-file path in Studio."));
        ensure!(self.sources.len() <= MAX_SOURCES && self.sources.iter().all(|path| path.is_absolute() && path.as_os_str().len() <= MAX_PATH_BYTES), "{}", t("Choose at most 32 source folders, using absolute paths."));
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub id: String,
    pub time: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub summary: Option<SnapshotSummary>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SnapshotSummary {
    #[serde(default)]
    pub total_files_processed: Option<u64>,
    #[serde(default)]
    pub total_bytes_processed: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Initialize,
    Backup,
    Check,
    /// The parent is user-selected; a fresh child directory prevents overwrites.
    Restore { snapshot: String, parent: PathBuf },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Progress {
    #[serde(default)]
    pub percent_done: f64,
    #[serde(default)]
    pub total_files: u64,
    #[serde(default, alias = "files_restored")]
    pub files_done: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default, alias = "bytes_restored")]
    pub bytes_done: u64,
    #[serde(default)]
    pub error_count: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub snapshot_id: Option<String>,
    pub restored_to: Option<PathBuf>,
    pub files: u64,
    pub bytes: u64,
}
