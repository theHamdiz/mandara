//! A bounded preview records exact file identities and leaves links untouched.

use super::*;
use super::paths::{open, child, parent};

pub fn preview(candidate: &Candidate, age_days: u64, cancel: &AtomicBool) -> Result<Preview> {
    ensure!(cfg!(target_os = "linux"), "{}", t("Reviewed file cleanup is available on Linux."));
    let root = open(&candidate.root)?;
    let stamp = Stamp::of(&root)?;
    ensure!(candidate.root_stamp.identity(stamp), "{}", t("The cleanup location changed. Refresh its locations before previewing files."));
    ensure!(root.metadata()?.is_dir() && root.metadata()?.uid() == rustix::process::getuid().as_raw(), "cleanup root is not your directory");
    let mut preview = Preview { candidate: candidate.clone(), logical_bytes: 0, left_untouched: 0, root: stamp, files: Vec::new(), directories: Vec::new(), info: None };
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs(age_days.clamp(AGE_DAYS_RANGE.0, AGE_DAYS_RANGE.1) * SECONDS_PER_DAY)).context("invalid cleanup age")?;
    Scanner { root: &root, device: stamp.device, mount: stamp.mount, kind: candidate.kind, cutoff, cancel, preview: &mut preview, path_bytes: 0, visits: 0 }.walk(&candidate.relative)?;
    if let Some(path) = &candidate.info {
        use std::io::Read as _;
        let directory = open(path.parent().context("Trash information has no parent")?)?;
        let parent_stamp = Stamp::of(&directory)?;
        ensure!(candidate.info_directory.is_some_and(|stamp| stamp.identity(parent_stamp)), "Trash information directory changed");
        let info = child(&directory, path.file_name().context("Trash information has no name")?)?;
        ensure!(info.metadata()?.is_file(), "Trash information is not a regular file");
        let mut document = String::new();
        info.try_clone()?.take(MAX_INFO_BYTES + 1).read_to_string(&mut document)?;
        ensure!(document.len() as u64 <= MAX_INFO_BYTES && document.lines().next() == Some("[Trash Info]")
            && document.lines().any(|line| line.starts_with("Path=")) && document.lines().any(|line| line.starts_with("DeletionDate=")), "Trash information is missing or invalid; inspect the item before cleaning it");
        preview.info = Some((path.clone(), Stamp::of(&info)?, parent_stamp));
    }
    Ok(preview)
}

struct Scanner<'a> {
    root: &'a File,
    device: u64,
    mount: u64,
    kind: Kind,
    cutoff: SystemTime,
    cancel: &'a AtomicBool,
    preview: &'a mut Preview,
    path_bytes: usize,
    visits: usize,
}
impl Scanner<'_> {
fn walk(&mut self, relative: &Path) -> Result<()> {
    ensure!(!self.cancel.load(Ordering::Acquire), "{}", t("Cancelled"));
    self.visits += 1;
    ensure!(self.visits <= MAX_FILES, "cleanup selection has too many entries");
    ensure!(relative.components().count() <= MAX_DEPTH, "cleanup directory is too deep");
    ensure!(self.preview.files.len() + self.preview.directories.len() < MAX_FILES, "cleanup selection has too many files");
    self.path_bytes = self.path_bytes.checked_add(relative.as_os_str().len()).context("cleanup paths overflow")?;
    ensure!(self.path_bytes <= MAX_PATH_BYTES, "cleanup selection is too large");
    let (directory, name) = parent(self.root, relative)?;
    let file = match child(&directory, &name) {
        Ok(file) => file,
        Err(error) if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::LOOP) => { self.preview.left_untouched += 1; return Ok(()); }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    let stamp = Stamp::from_metadata(&metadata, paths::mount_id(&file)?);
    if stamp.device != self.device || stamp.mount != self.mount || metadata.uid() != rustix::process::getuid().as_raw() { self.preview.left_untouched += 1; return Ok(()); }
    if metadata.is_dir() {
        for entry in Dir::read_from(&file)? {
            let entry = entry?;
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if matches!(name.to_str(), Some("." | "..")) { continue; }
            self.walk(&relative.join(name))?;
        }
        self.preview.directories.push(Entry { relative: relative.into(), stamp });
    // Shared inodes can have aliases outside the reviewed tree. Leave them intact.
    } else if metadata.is_file() && metadata.nlink() == 1 && (self.kind == Kind::Trash || metadata.modified()? < self.cutoff) {
        self.preview.logical_bytes = self.preview.logical_bytes.checked_add(metadata.len()).context("cleanup size overflow")?;
        self.preview.files.push(Entry { relative: relative.into(), stamp });
    } else { self.preview.left_untouched += 1; }
    Ok(())
}

}
