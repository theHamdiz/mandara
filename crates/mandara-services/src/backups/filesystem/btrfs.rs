//! Stable btrfs-progs commands; destructive operations use persistent subvolume identities.

use std::path::{Component, Path, PathBuf};
use std::fs::File;
use std::os::fd::AsRawFd as _;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;
use super::{Backend, Maintenance, Mount, Reply, Request, Snapshot, mounted};

/// A complete snapshot listing is bounded independently of command diagnostics.
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 4096;
const READ_SECONDS: u64 = 60;
const READ_TIMEOUT: Duration = Duration::from_secs(READ_SECONDS);
/// Scrub may run over a large filesystem, but no process is detached or orphaned.
const MAINTENANCE_SECONDS: u64 = 24 * 60 * 60;
const MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(MAINTENANCE_SECONDS);

pub(super) trait Runner {
    fn run(&self, program: &str, arguments: &[std::ffi::OsString], cancel: &AtomicBool, timeout: Duration) -> Result<String>;
}
pub(super) struct SystemRunner;
impl Runner for SystemRunner {
    fn run(&self, program: &str, arguments: &[std::ffi::OsString], cancel: &AtomicBool, timeout: Duration) -> Result<String> {
        let program = crate::sysupdate::helper::trusted_program(program)?;
        let mut command = Command::new(program);
        command.args(arguments).env_clear().env("PATH", crate::sysupdate::SAFE_PATH).env("LC_ALL", "C").env("HOME", "/root");
        let mut output = Vec::new();
        let diagnostics = crate::app_themes::process::stream_native(command, cancel, timeout, &mut |bytes| {
            ensure!(bytes.len() <= MAX_SNAPSHOT_BYTES.saturating_sub(output.len()), "filesystem output is too large");
            output.extend_from_slice(bytes);
            Ok(())
        })?;
        ensure!(diagnostics.trim().is_empty(), "filesystem tool reported: {}", diagnostics.trim());
        String::from_utf8(output).context("filesystem output is not UTF-8")
    }
}

pub(super) fn run(runner: &impl Runner, program: &str, args: &[&str], path: &Path, cancel: &AtomicBool, timeout: Duration) -> Result<String> {
    let args: Vec<_> = args.iter().map(std::ffi::OsString::from).chain(std::iter::once(path.as_os_str().into())).collect();
    runner.run(program, &args, cancel, timeout)
}

fn uuid(value: &str) -> bool {
    value.len() == 36 && value.bytes().enumerate().all(|(index, byte)| if matches!(index, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() })
}

pub(super) fn snapshots(text: &str, mount: &Mount, mounts: &[Mount], default: u64) -> Result<Vec<Snapshot>> {
    let mut rows = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        ensure!(rows.len() < MAX_SNAPSHOTS, "too many local snapshots");
        let (fields, path) = line.split_once(" path ").context("unrecognized Btrfs snapshot listing")?;
        let fields: Vec<_> = fields.split_whitespace().collect();
        let value = |name: &str| fields.windows(2).find(|pair| pair[0] == name).map(|pair| pair[1]);
        let id = value("ID").context("missing Btrfs snapshot id")?.parse::<u64>()?;
        let identity = value("uuid").context("missing Btrfs snapshot UUID")?;
        let parent = value("parent_uuid").context("missing Btrfs snapshot parent UUID")?;
        ensure!(uuid(identity) && uuid(parent), "unrecognized Btrfs snapshot identity");
        let path = PathBuf::from(path);
        ensure!(!path.as_os_str().is_empty() && path.components().all(|part| matches!(part, Component::Normal(_))) && !path.as_os_str().as_encoded_bytes().iter().any(|byte| byte.is_ascii_control()), "invalid Btrfs snapshot path");
        let protected = id <= 5 || id == default || mounts.iter().any(|other| other.device == mount.device && (other.subvolume_id == Some(id) || other.root == Path::new("/").join(&path)));
        rows.push(Snapshot { id, uuid: identity.into(), parent_uuid: parent.into(), path, protected });
    }
    Ok(rows)
}

fn default_id(text: &str) -> Result<u64> {
    let mut fields = text.split_whitespace();
    ensure!(fields.next() == Some("ID"), "unrecognized Btrfs default subvolume");
    fields.next().context("missing Btrfs default subvolume id")?.parse().map_err(Into::into)
}

pub(super) fn inspect(runner: &impl Runner, mount: &Mount, mounts: &[Mount], cancel: &AtomicBool) -> Result<Reply> {
    ensure!(mount.supports_snapshots(), "{}", t("This filesystem does not support native Btrfs snapshots."));
    let default = run(runner, "btrfs", &["subvolume", "get-default"], &mount.target, cancel, READ_TIMEOUT)?;
    let text = run(runner, "btrfs", &["subvolume", "list", "-s", "-u", "-q"], &mount.target, cancel, READ_TIMEOUT)?;
    Ok(Reply { snapshots: snapshots(&text, mount, mounts, default_id(&default)?)?, output: String::new() })
}

pub(super) fn checked_mount(requested: &Mount, mounts: &[Mount]) -> Result<()> {
    ensure!(mounts.iter().any(|mount| mount == requested), "{}", t("The filesystem mount changed. Refresh before continuing."));
    ensure!(requested.target.canonicalize()? == requested.target, "{}", t("The filesystem mount moved. Refresh before continuing."));
    Ok(())
}

fn selected_directory(path: &Path, mount: &Mount, mounts: &[Mount]) -> Result<File> {
    let deepest = mounts.iter().filter(|other| path.starts_with(&other.target))
        .max_by_key(|other| (other.target.components().count(), other.id));
    ensure!(deepest == Some(mount), "{}", t("The source and snapshot folder must belong to the selected Btrfs mount."));
    let file = File::from(rustix::fs::open(path, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW, rustix::fs::Mode::empty())?);
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{statx, AtFlags, StatxFlags};
        let current = statx(&file, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)?;
        ensure!(current.stx_mask & StatxFlags::MNT_ID.bits() != 0 && current.stx_mnt_id == mount.id, "{}", t("The filesystem mount changed. Refresh before continuing."));
    }
    Ok(file)
}

fn pinned_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()))
}

pub(super) fn apply(runner: &impl Runner, request: &Request, mounts: &[Mount], cancel: &AtomicBool) -> Result<Reply> {
    let mount = request.mount();
    checked_mount(mount, mounts)?;
    if !matches!(request, Request::Inspect { .. }) { ensure!(!mount.read_only, "{}", t("This filesystem is mounted read-only.")); }
    match request {
        Request::Inspect { .. } => inspect(runner, mount, mounts, cancel),
        Request::Delete { snapshot, .. } => {
            let current = inspect(runner, mount, mounts, cancel)?;
            ensure!(!snapshot.protected && current.snapshots.iter().any(|row| row == snapshot && !row.protected), "{}", t("This snapshot is active, protected or changed. Refresh before removing it."));
            let output = run(runner, "btrfs", &["subvolume", "delete", "--commit-after", "--subvolid", &snapshot.id.to_string()], &mount.target, cancel, READ_TIMEOUT)?;
            Ok(Reply { output, ..Default::default() })
        }
        Request::Create { source, directory, name, .. } => {
            ensure!(mount.supports_snapshots(), "{}", t("This filesystem does not support native Btrfs snapshots."));
            ensure!(!name.is_empty() && name.len() <= 128 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')), "invalid local snapshot name");
            let source = source.canonicalize()?;
            let directory = directory.canonicalize()?;
            let source = selected_directory(&source, mount, mounts)?;
            let directory = selected_directory(&directory, mount, mounts)?;
            let destination = pinned_path(&directory).join(name);
            ensure!(std::fs::symlink_metadata(&destination).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound), "{}", t("The snapshot destination already exists."));
            let args = ["subvolume".into(), "snapshot".into(), "-r".into(), pinned_path(&source).into_os_string(), destination.into_os_string()];
            let output = runner.run("btrfs", &args, cancel, READ_TIMEOUT)?;
            Ok(Reply { output, ..Default::default() })
        }
        Request::Maintain { operation, .. } => {
            let (program, args): (&str, &[&str]) = match operation {
                Maintenance::Scrub => {
                    ensure!(mount.supports_snapshots(), "Btrfs scrub requires Btrfs");
                    ("btrfs", &["scrub", "start", "-B"])
                }
                Maintenance::ReclaimEmptyChunks => {
                    ensure!(mount.supports_snapshots(), "Btrfs space reclamation requires Btrfs");
                    ("btrfs", &["balance", "start", "-dusage=0", "-musage=0"])
                }
                Maintenance::Trim => {
                    ensure!(mount.supports_trim(), "{}", t("Trim is unavailable for this filesystem or mount."));
                    ("fstrim", &["--verbose", "--"])
                }
            };
            let output = run(runner, program, args, &mount.target, cancel, MAINTENANCE_TIMEOUT)?;
            Ok(Reply { output, ..Default::default() })
        }
    }
}

#[derive(Default)]
pub struct Native;
impl Backend for Native {
    fn inspect(&self, mount: &Mount, cancel: &AtomicBool) -> Result<Reply> {
        let mounts = mounted()?;
        checked_mount(mount, &mounts)?;
        inspect(&SystemRunner, mount, &mounts, cancel)
    }
    fn apply(&self, request: &Request, cancel: &AtomicBool) -> Result<Reply> { super::admin::run(request, cancel) }
}
