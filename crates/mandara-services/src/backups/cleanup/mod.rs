//! Reviewed cache and Trash removal. A preview owns exact file identities, never a recursive delete.

use std::ffi::{OsStr, OsString};
mod paths;
mod locations;
mod scan;
mod remove;
#[cfg(all(test, target_os = "linux"))]
mod tests;

pub use locations::candidates;
pub use scan::preview;
pub use remove::remove;


use std::fs::File;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;
use rustix::fs::{AtFlags, Dir, Mode, OFlags, mkdirat, openat, unlinkat};

/// Large caches require a smaller selection rather than an unbounded retained manifest.
const MAX_FILES: usize = 50_000;
const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_CANDIDATES: usize = 512;
const MAX_INFO_BYTES: u64 = 64 * 1024;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
pub const DEFAULT_AGE_DAYS: u64 = 30;
pub const AGE_DAYS_RANGE: (u64, u64) = (1, 3650);
const OPEN: OFlags = OFlags::RDONLY.union(OFlags::CLOEXEC).union(OFlags::NOFOLLOW).union(OFlags::NONBLOCK);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Cache, Trash }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub kind: Kind,
    pub name: String,
    pub path: PathBuf,
    root: PathBuf,
    root_stamp: Stamp,
    relative: PathBuf,
    info: Option<PathBuf>,
    info_directory: Option<Stamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp { device: u64, mount: u64, inode: u64, bytes: u64, modified: (i64, i64), changed: (i64, i64) }
impl Stamp {
    fn of(file: &File) -> Result<Self> {
        let meta = file.metadata()?;
        Ok(Self { device: meta.dev(), mount: paths::mount_id(file)?, inode: meta.ino(), bytes: meta.len(), modified: (meta.mtime(), meta.mtime_nsec()), changed: (meta.ctime(), meta.ctime_nsec()) })
    }
    fn identity(self, other: Self) -> bool { self.device == other.device && self.mount == other.mount && self.inode == other.inode }
    fn after_rename(self, other: Self) -> bool { self.identity(other) && self.bytes == other.bytes && self.modified == other.modified }
}

struct Entry { relative: PathBuf, stamp: Stamp }
pub struct Preview {
    pub candidate: Candidate,
    pub logical_bytes: u64,
    pub left_untouched: usize,
    root: Stamp,
    files: Vec<Entry>,
    directories: Vec<Entry>,
    info: Option<(PathBuf, Stamp, Stamp)>,
}
impl Preview {
    pub fn file_count(&self) -> usize { self.files.len() }
    /// The retained file paths are relative to this checked cache or Trash root.
    pub fn base_directory(&self) -> &Path { &self.candidate.root }
    pub fn files(&self) -> impl Iterator<Item = (&Path, u64)> { self.files.iter().map(|entry| (entry.relative.as_path(), entry.stamp.bytes)) }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Report { pub files_removed: usize, pub logical_bytes: u64, pub items_left: usize }
