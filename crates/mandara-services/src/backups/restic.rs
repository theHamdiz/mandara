//! Native restic JSON with bounded retained output and cancellable child ownership.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
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
    ensure!(text.len() <= MAX_JSON_BYTES, "{}", t("The backup history is too large to display."));
    let mut rows: Vec<DatedSnapshot> = serde_json::from_slice(text)?;
    ensure!(rows.iter().all(|row| valid_id(&row.snapshot.id)), "{}", t("The backup history contains an invalid snapshot."));
    rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.time));
    rows.truncate(SNAPSHOTS);
    let mut rows: Vec<_> = rows.into_iter().map(|row| row.snapshot).collect();
    rows.shrink_to(SNAPSHOTS);
    Ok(rows)
}

#[derive(serde::Deserialize)]
#[serde(try_from = "Snapshot")]
struct DatedSnapshot { snapshot: Snapshot, time: chrono::DateTime<chrono::FixedOffset> }
impl TryFrom<Snapshot> for DatedSnapshot {
    type Error = chrono::ParseError;
    fn try_from(snapshot: Snapshot) -> std::result::Result<Self, Self::Error> {
        let time = chrono::DateTime::parse_from_rfc3339(&snapshot.time)?;
        Ok(Self { snapshot, time })
    }
}

/// Restic treats exclusions as Go filepath patterns, even when the repository is a literal path.
pub(super) fn repository_exclusion(path: &Path) -> OsString {
    let mut pattern = Vec::with_capacity(path.as_os_str().len());
    for &byte in path.as_os_str().as_bytes() {
        if matches!(byte, b'\\' | b'*' | b'?' | b'[' | b']') { pattern.push(b'\\'); }
        pattern.push(byte);
    }
    OsString::from_vec(pattern)
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn restore_directory(parent: &Path, snapshot: &str) -> Result<tempfile::TempDir> {
    ensure!(parent.is_absolute() && valid_id(snapshot), "{}", t("Choose a restore folder and a valid snapshot."));
    let parent = parent.canonicalize()?;
    ensure!(parent.is_dir(), "{}", t("The restore destination must be a folder."));
    let directory = tempfile::Builder::new().prefix(&format!("Restored-{}-", &snapshot[..8])).permissions(std::fs::Permissions::from_mode(0o700)).tempdir_in(parent)?;
    Ok(directory)
}

/// JSON lines may arrive split across any pipe boundary.
pub(super) struct Events {
    pending: Vec<u8>,
    pub summary: Summary,
    json: bool,
}
impl Events {
    pub fn new(operation: &Operation) -> Self {
        // Older Restic checks print plain text even with --json; their exit status is authoritative.
        Self { pending: Vec::new(), summary: Summary::default(), json: !matches!(operation, Operation::Check) }
    }

    pub fn push(&mut self, mut bytes: &[u8], progress: &mut dyn FnMut(Progress)) -> Result<()> {
        if !self.json { return Ok(()); }
        while !bytes.is_empty() {
            let length = bytes.iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |index| index + 1);
            ensure!(length <= MAX_EVENT_BYTES.saturating_sub(self.pending.len()), "{}", t("The backup progress record is too large."));
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
                ensure!(state.percent_done.is_finite(), "{}", t("Restic returned invalid backup progress."));
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
            ensure!(chunk.len() <= MAX_JSON_BYTES.saturating_sub(bytes.len()), "{}", t("The backup history is too large to display."));
            bytes.extend_from_slice(chunk);
            Ok(())
        })?;
        snapshots(&bytes)
    }

    fn execute(&self, settings: &Settings, operation: Operation, cancel: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<Summary> {
        let mut command = command(settings)?;
        let mut events = Events::new(&operation);
        let mut destination = None;
        match operation {
            Operation::Initialize => { command.arg("init"); }
            Operation::Check => { command.arg("check"); }
            Operation::Backup => {
                ensure!(!settings.sources.is_empty(), "{}", t("Choose source folders in Studio before starting a backup."));
                let repository = settings.repository.canonicalize()?;
                command.args(["backup", "--exclude"]).arg(repository_exclusion(&repository)).arg("--");
                for source in &settings.sources {
                    let source = source.canonicalize()?;
                    ensure!(source.is_dir() && !source.starts_with(&repository), "{}", t("A backup source must be a folder outside the backup repository."));
                    command.arg(source);
                }
            }
            Operation::Restore { snapshot, parent } => {
                let directory = restore_directory(&parent, &snapshot)?;
                // The fresh, private destination also keeps older Restic releases safe.
                command.args(["restore", &snapshot, "--target"]).arg(directory.path());
                destination = Some(directory);
            }
        }
        // Keep partial restores too: cancellation must never erase already restored copies.
        let restored_to = destination.map(tempfile::TempDir::keep);
        let result = crate::app_themes::process::stream_native(command, cancel, OPERATION_TIMEOUT, &mut |chunk| events.push(chunk, progress));
        result.with_context(|| restored_to.as_ref().map_or_else(|| t("The backup operation did not finish.").to_string(), |path| format!("{} {}", t("The restore did not finish. Partial copies remain in:"), path.display())))?;
        events.finish(progress)?;
        events.summary.restored_to = restored_to;
        Ok(events.summary)
    }
}
