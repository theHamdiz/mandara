//! Native filesystem snapshots and maintenance, separate from off-disk backups.

mod mounts;
mod btrfs;
pub mod admin;
#[cfg(test)]
mod tests;
#[cfg(all(test, target_os = "linux"))]
mod native_tests;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use mounts::{Mount, mounted};
pub use btrfs::Native;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: u64,
    pub uuid: String,
    pub parent_uuid: String,
    /// Filesystem-root relative path, not necessarily accessible through this mount.
    pub path: PathBuf,
    pub protected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Maintenance { Scrub, ReclaimEmptyChunks, Trim }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Inspect { mount: Mount },
    Create { mount: Mount, source: PathBuf, directory: PathBuf, name: String },
    Delete { mount: Mount, snapshot: Snapshot },
    Maintain { mount: Mount, operation: Maintenance },
}
impl Request {
    pub fn mount(&self) -> &Mount {
        match self { Self::Inspect { mount } | Self::Create { mount, .. } | Self::Delete { mount, .. } | Self::Maintain { mount, .. } => mount }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub snapshots: Vec<Snapshot>,
    pub output: String,
}

pub trait Backend {
    fn inspect(&self, mount: &Mount, cancel: &AtomicBool) -> Result<Reply>;
    fn apply(&self, request: &Request, cancel: &AtomicBool) -> Result<Reply>;
}
