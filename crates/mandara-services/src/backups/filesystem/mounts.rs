//! Mount identity is refreshed before every storage action.

use std::io::Read as _;
use std::path::PathBuf;
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use mandara_i18n::t;

/// Bound the mount table without interpreting a truncated prefix as complete.
const MAX_MOUNT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MOUNTS: usize = 32768;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub id: u64,
    pub device: String,
    pub root: PathBuf,
    pub target: PathBuf,
    pub filesystem: String,
    pub read_only: bool,
    pub subvolume_id: Option<u64>,
}
impl Mount {
    pub fn supports_snapshots(&self) -> bool { cfg!(target_os = "linux") && self.filesystem == "btrfs" }
    pub fn supports_trim(&self) -> bool {
        cfg!(target_os = "linux") && !self.read_only && matches!(self.filesystem.as_str(), "btrfs" | "ext4" | "xfs" | "f2fs" | "vfat" | "exfat" | "ntfs3")
    }
}

pub(super) fn parse(text: &str) -> Result<Vec<Mount>> {
    let mut mounts = Vec::new();
    for line in text.lines() {
        ensure!(mounts.len() < MAX_MOUNTS, t("The filesystem mount table is invalid or too large."));
        let (left, right) = line.split_once(" - ").context(t("The filesystem mount table is invalid or too large."))?;
        let mut fields = left.split(' ').filter(|field| !field.is_empty());
        let id = fields.next().context(t("The filesystem mount table is invalid or too large."))?.parse()?;
        let _parent = fields.next().context(t("The filesystem mount table is invalid or too large."))?;
        let device = fields.next().context(t("The filesystem mount table is invalid or too large."))?.to_owned();
        let root = PathBuf::from(crate::boot::detect::unescape_mount(fields.next().context(t("The filesystem mount table is invalid or too large."))?));
        let target = PathBuf::from(crate::boot::detect::unescape_mount(fields.next().context(t("The filesystem mount table is invalid or too large."))?));
        let options = fields.next().context(t("The filesystem mount table is invalid or too large."))?;
        let mut trailing = right.split(' ').filter(|field| !field.is_empty());
        let filesystem = trailing.next().context(t("The filesystem mount table is invalid or too large."))?.to_owned();
        let _source = trailing.next().context(t("The filesystem mount table is invalid or too large."))?;
        let super_options = trailing.next().context(t("The filesystem mount table is invalid or too large."))?;
        let subvolume_id = super_options.split(',').find_map(|option| option.strip_prefix("subvolid=")).map(str::parse).transpose()?;
        // nsfs names its anonymous root `net:[inode]`; retain the mount as an overlap guard.
        ensure!((root.is_absolute() || filesystem == "nsfs") && target.is_absolute(), t("The filesystem mount table is invalid or too large."));
        mounts.push(Mount { id, device, root, target, filesystem, read_only: options.split(',').any(|option| option == "ro"), subvolume_id });
    }
    Ok(mounts)
}

pub fn mounted() -> Result<Vec<Mount>> {
    ensure!(cfg!(target_os = "linux"), t("Native filesystem maintenance is available on Linux."));
    let mut text = String::new();
    std::fs::File::open("/proc/self/mountinfo")?.take(MAX_MOUNT_BYTES + 1).read_to_string(&mut text)?;
    ensure!(text.len() as u64 <= MAX_MOUNT_BYTES, t("The filesystem mount table is invalid or too large."));
    parse(&text)
}
