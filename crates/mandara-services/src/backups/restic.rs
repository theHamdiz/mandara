//! Native restic JSON with bounded retained output and cancellable child ownership.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;

use super::{Backend, Operation, Progress, Settings, Snapshot, Summary};

/// A bounded recent history keeps very old repositories cheap to open.
const SNAPSHOTS: usize = 64;
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
/// Each streamed progress record is discarded after updating the readout.
const MAX_EVENT_BYTES: usize = 256 * 1024;
/// Large user-started backups may run overnight; cancellation remains immediate.
const OPERATION_SECONDS: u64 = 24 * 60 * 60;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(OPERATION_SECONDS);
const LIST_SECONDS: u64 = 60;
const LIST_TIMEOUT: Duration = Duration::from_secs(LIST_SECONDS);

#[derive(Default)]
pub struct Restic;

pub(super) fn command(settings: &Settings) -> Result<Command> {
    settings.validate()?;
    let metadata = std::fs::metadata(&settings.password_file).context(t("Could not open the backup password file."))?;
    ensure!(metadata.is_file() && metadata.uid() == rustix::process::getuid().as_raw() && metadata.permissions().mode() & 0o777 == 0o600, "{}", t("The backup password file must belong to you and have permissions 0600."));
    let mut command = Command::new("restic");
    command.args(["--json", "--repo"]).arg(&settings.repository)
        .arg("--password-file").arg(&settings.password_file)
        .env_remove("RESTIC_PASSWORD").env_remove("RESTIC_PASSWORD_COMMAND")
        .env_remove("RESTIC_PASSWORD_FILE").env_remove("RESTIC_REPOSITORY_FILE")
        .env("RESTIC_PROGRESS_FPS", "2");
    Ok(command)
}

pub(super) fn snapshots(text: &[u8]) -> Result<Vec<Snapshot>> {
    ensure!(text.len() <= MAX_JSON_BYTES, "backup history is too large");
    let mut rows: Vec<Snapshot> = serde_json::from_slice(text)?;
    ensure!(rows.len() <= SNAPSHOTS, "backup history has too many entries");
    ensure!(rows.iter().all(|row| valid_id(&row.id)), "invalid backup snapshot identity");
    rows.sort_unstable_by(|a, b| b.time.cmp(&a.time));
    Ok(rows)
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn restore_directory(parent: &Path, snapshot: &str) -> Result<tempfile::TempDir> {
    ensure!(parent.is_absolute() && valid_id(snapshot), "{}", t("Choose a restore folder and a valid snapshot."));
    let parent = parent.canonicalize()?;
    ensure!(parent.is_dir(), "{}", t("The restore destination must be a folder."));
    let directory = tempfile::Builder::new().prefix(&format!("Restored-{}-", &snapshot[..8])).tempdir_in(parent)?;
    Ok(directory)
}

/// JSON lines may arrive split across any pipe boundary.
pub(super) struct Events {
    pending: Vec<u8>,
    pub summary: Summary,
}
impl Events {
    pub fn new() -> Self { Self { pending: Vec::new(), summary: Summary::default() } }

    pub fn push(&mut self, mut bytes: &[u8], progress: &mut dyn FnMut(Progress)) -> Result<()> {
        while !bytes.is_empty() {
            let length = bytes.iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |index| index + 1);
            ensure!(length <= MAX_EVENT_BYTES.saturating_sub(self.pending.len()), "backup progress record is too large");
            self.pending.extend_from_slice(&bytes[..length]);
            bytes = &bytes[length..];
            if self.pending.last() == Some(&b'\n') { self.record(progress)?; }
        }
        Ok(())
    }

    pub fn finish(&mut self, progress: &mut dyn FnMut(Progress)) -> Result<()> {
        if !self.pending.is_empty() { self.record(progress)?; }
        Ok(())
    }

    fn record(&mut self, progress: &mut dyn FnMut(Progress)) -> Result<()> {
        let value: serde_json::Value = serde_json::from_slice(&self.pending)?;
        match value.get("message_type").and_then(|value| value.as_str()) {
            Some("status") => {
                let mut state: Progress = serde_json::from_value(value)?;
                ensure!(state.percent_done.is_finite(), "invalid backup progress");
                state.percent_done = state.percent_done.clamp(0.0, 1.0);
                progress(state);
            }
            Some("summary") => {
                self.summary.snapshot_id = value.get("snapshot_id").and_then(|id| id.as_str()).map(str::to_owned);
                self.summary.files = value.get("total_files_processed").or_else(|| value.get("files_restored")).and_then(|n| n.as_u64()).unwrap_or(0);
                self.summary.bytes = value.get("total_bytes_processed").or_else(|| value.get("bytes_restored")).and_then(|n| n.as_u64()).unwrap_or(0);
            }
            _ => {}
        }
        self.pending.clear();
        Ok(())
    }
}

impl Backend for Restic {
    fn snapshots(&self, settings: &Settings, cancel: &AtomicBool) -> Result<Vec<Snapshot>> {
        let mut command = command(settings)?;
        command.args(["snapshots", "--latest", &SNAPSHOTS.to_string(), "--group-by", ""]);
        let mut bytes = Vec::new();
        crate::app_themes::process::stream_native(command, cancel, LIST_TIMEOUT, &mut |chunk| {
            ensure!(chunk.len() <= MAX_JSON_BYTES.saturating_sub(bytes.len()), "backup history is too large");
            bytes.extend_from_slice(chunk);
            Ok(())
        })?;
        snapshots(&bytes)
    }

    fn execute(&self, settings: &Settings, operation: Operation, cancel: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<Summary> {
        let mut command = command(settings)?;
        let mut destination = None;
        match operation {
            Operation::Initialize => { command.arg("init"); }
            Operation::Check => { command.arg("check"); }
            Operation::Backup => {
                ensure!(!settings.sources.is_empty(), "{}", t("Choose source folders in Studio before starting a backup."));
                let repository = settings.repository.canonicalize()?;
                command.args(["backup", "--exclude"]).arg(&repository).arg("--");
                for source in &settings.sources {
                    let source = source.canonicalize()?;
                    ensure!(source.is_dir() && !source.starts_with(&repository), "{}", t("A backup source must be a folder outside the backup repository."));
                    command.arg(source);
                }
            }
            Operation::Restore { snapshot, parent } => {
                let directory = restore_directory(&parent, &snapshot)?;
                command.args(["restore", &snapshot, "--target"]).arg(directory.path()).args(["--overwrite", "never"]);
                destination = Some(directory);
            }
        }
        let mut events = Events::new();
        // Keep partial restores too: cancellation must never erase already restored copies.
        let restored_to = destination.map(tempfile::TempDir::keep);
        let result = crate::app_themes::process::stream_native(command, cancel, OPERATION_TIMEOUT, &mut |chunk| events.push(chunk, progress));
        result.with_context(|| restored_to.as_ref().map_or_else(|| t("The backup operation did not finish.").to_string(), |path| format!("{} {}", t("The restore did not finish. Partial copies remain in:"), path.display())))?;
        events.finish(progress)?;
        events.summary.restored_to = restored_to;
        Ok(events.summary)
    }
}
