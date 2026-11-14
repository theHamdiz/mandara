//! Restore the disposable storage VM after a native test, including failed assertions.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

pub(super) struct Fixture {
    directory: Option<tempfile::TempDir>,
    mounts: Vec<(PathBuf, Vec<u64>)>,
    subvolumes: Vec<PathBuf>,
    default: Option<(PathBuf, u64)>,
    _guard: MutexGuard<'static, ()>,
}

impl Fixture {
    pub fn new(parent: &Path) -> Self {
        let guard = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            std::fs::read_to_string("/mandara-storage-fixture").unwrap(),
            "mandara-disposable-storage-264\n"
        );
        assert!(rustix::process::geteuid().is_root());
        assert!(matches!(
            parent.to_str(),
            Some("/fixture/btrfs" | "/fixture/ext4")
        ));
        Self {
            directory: Some(
                tempfile::Builder::new()
                    .prefix(".mandara-storage-test-")
                    .tempdir_in(parent)
                    .unwrap(),
            ),
            mounts: Vec::new(),
            subvolumes: Vec::new(),
            default: None,
            _guard: guard,
        }
    }

    pub fn path(&self) -> &Path {
        self.directory.as_ref().unwrap().path()
    }
    pub fn mount(&mut self, path: &Path) {
        let original = super::filesystem::mounted()
            .unwrap()
            .into_iter()
            .filter(|mount| mount.target == path)
            .map(|mount| mount.id)
            .collect();
        self.mounts.push((path.into(), original));
    }
    pub fn subvolume(&mut self, path: &Path) {
        self.subvolumes.push(path.into());
    }

    pub fn default_subvolume(&mut self, mount: &Path) {
        assert!(self.default.is_none());
        let output = Command::new("btrfs")
            .args(["subvolume", "get-default"])
            .arg(mount)
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let id = text
            .strip_prefix("ID ")
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        self.default = Some((mount.into(), id));
    }

    fn cleanup(&self) -> anyhow::Result<()> {
        for (path, original) in self.mounts.iter().rev() {
            while super::filesystem::mounted()?
                .iter()
                .any(|mount| mount.target == *path && !original.contains(&mount.id))
            {
                run(Command::new("umount").arg(path))?;
            }
        }
        if let Some((mount, id)) = &self.default {
            run(Command::new("btrfs")
                .args(["subvolume", "set-default", &id.to_string()])
                .arg(mount))?;
        }
        for path in self.subvolumes.iter().rev() {
            if path.exists() {
                anyhow::ensure!(
                    path.starts_with(self.path()) && !path.is_symlink(),
                    "native fixture subvolume escaped its directory"
                );
                run(Command::new("btrfs")
                    .args(["subvolume", "delete", "--commit-after"])
                    .arg(path))?;
            }
        }
        Ok(())
    }
}

fn run(command: &mut Command) -> anyhow::Result<()> {
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    anyhow::ensure!(
        status.success(),
        "fixture cleanup failed: {command:?}: {status}"
    );
    Ok(())
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            // Do not recursively remove a tree whose mount or default could not be restored.
            let retained = self.directory.take().unwrap().keep();
            if std::thread::panicking() {
                eprintln!(
                    "Storage fixture retained at {}: {error:#}",
                    retained.display()
                );
            } else {
                panic!(
                    "Storage fixture retained at {}: {error:#}",
                    retained.display()
                );
            }
        }
    }
}
