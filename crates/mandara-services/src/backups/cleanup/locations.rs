//! Only cache and Trash directories owned by the current user are offered.

use super::*;
use super::paths::open;

pub(super) fn protected_cache(cache: &Path, critical: &[PathBuf]) -> bool {
    cache == Path::new("/") || critical.iter().enumerate().any(|(index, path)| path.starts_with(cache) || cache == path || index != 0 && cache.starts_with(path))
}

pub fn candidates() -> Result<Vec<Candidate>> {
    ensure!(cfg!(target_os = "linux"), "{}", t("Reviewed file cleanup is available on Linux."));
    let home = dirs::home_dir().context(t("Your home folder is unavailable."))?.canonicalize()?;
    let data = mandara_config::paths::data_dir().with_file_name("");
    let cache = mandara_config::paths::cache_dir().with_file_name("");
    let config = mandara_config::paths::config_dir().with_file_name("");
    let state = mandara_config::paths::state_dir().with_file_name("");
    let mut result = Vec::new();
    if cache.exists() {
        let cache = cache.canonicalize()?;
        let identity = mandara_theme::templates::destination::identity;
        let critical = [home, identity(&data)?, identity(&config)?, identity(&state)?];
        ensure!(!protected_cache(&cache, &critical), "{}", t("The cache location overlaps personal data. Cleanup is unavailable for this location."));
        collect(&cache, Kind::Cache, &mut result)?;
    }
    let trash = data.join("Trash/files");
    if trash.exists() {
        for directory in [data.join("Trash"), trash.clone(), data.join("Trash/info")] {
            let metadata = std::fs::symlink_metadata(&directory)?;
            ensure!(metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.uid() == rustix::process::getuid().as_raw(), "Trash folders must be real directories owned by you");
        }
        collect(&trash.canonicalize()?, Kind::Trash, &mut result)?;
    }
    Ok(result)
}

pub(super) fn collect(root: &Path, kind: Kind, result: &mut Vec<Candidate>) -> Result<()> {
    let directory = open(root)?;
    ensure!(directory.metadata()?.is_dir() && directory.metadata()?.uid() == rustix::process::getuid().as_raw(), "cleanup root is not your directory");
    let root_stamp = Stamp::of(&directory)?;
    let info_directory = (kind == Kind::Trash).then(|| open(&root.parent().context("Trash has no parent")?.join("info")).and_then(|file| Stamp::of(&file))).transpose()?;
    for entry in Dir::read_from(&directory)? {
        let entry = entry?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes());
        if matches!(name.to_str(), Some("." | ".." | "mandara")) || name.as_bytes().starts_with(b".mandara-cleanup-") { continue; }
        ensure!(result.len() < MAX_CANDIDATES, "too many cleanup folders; choose a smaller cache location");
        let relative = PathBuf::from(name);
        let path = root.join(name);
        let info = (kind == Kind::Trash).then(|| {
            let mut filename = name.to_os_string(); filename.push(".trashinfo");
            root.parent().unwrap_or(root).join("info").join(filename)
        });
        result.push(Candidate { kind, name: name.to_string_lossy().into_owned(), path, root: root.into(), root_stamp, relative, info, info_directory });
    }
    result.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(())
}
