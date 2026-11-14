//! Descriptor-relative traversal never follows a selected symlink.

use super::*;

pub(super) fn mount_id(file: &File) -> Result<u64> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{statx, StatxFlags};
        let stamp = statx(file, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)?;
        ensure!(stamp.stx_mask & StatxFlags::MNT_ID.bits() != 0, "filesystem mount identity is unavailable");
        Ok(stamp.stx_mnt_id)
    }
    #[cfg(not(target_os = "linux"))]
    { let _ = file; anyhow::bail!("{}", t("Reviewed file cleanup is available on Linux.")) }
}

pub(super) fn claim(from: &File, name: &OsStr, to: &File, destination: &OsStr) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        rustix::fs::renameat_with(from, name, to, destination, rustix::fs::RenameFlags::NOREPLACE)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (from, name, to, destination);
        anyhow::bail!("{}", t("Reviewed file cleanup is available on Linux."))
    }
}

pub(super) fn open(path: &Path) -> Result<File> { Ok(File::from(rustix::fs::open(path, OPEN, Mode::empty())?)) }
pub(super) fn child(parent: &File, name: &OsStr) -> Result<File> { Ok(File::from(openat(parent, name, OPEN, Mode::empty())?)) }

pub(super) fn parent(root: &File, relative: &Path) -> Result<(File, OsString)> {
    let name = relative.file_name().context("cleanup selection has no filename")?.to_os_string();
    let mut directory = root.try_clone()?;
    for component in relative.parent().unwrap_or_else(|| Path::new("")).components() {
        let Component::Normal(name) = component else { anyhow::bail!("cleanup path escapes its root") };
        directory = File::from(openat(&directory, name, OPEN | OFlags::DIRECTORY, Mode::empty())?);
    }
    Ok((directory, name))
}
