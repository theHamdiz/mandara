use super::*;

fn candidate(root: &Path, kind: Kind) -> Candidate {
    let mut entries = Vec::new();
    locations::collect(root, kind, &mut entries).unwrap();
    entries.remove(0)
}

fn old_file(path: &Path, value: &str) {
    std::fs::write(path, value).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH)).unwrap();
}

#[test]
fn cache_preview_removes_only_old_selected_files_and_keeps_new_files_and_links() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("thumbnails");
    std::fs::create_dir(&directory).unwrap();
    old_file(&directory.join("old.png"), "old cache");
    std::fs::write(directory.join("recent.png"), "recent cache").unwrap();
    let outside = tempfile::tempdir().unwrap();
    old_file(&outside.path().join("personal.txt"), "keep outside");
    std::os::unix::fs::symlink(outside.path(), directory.join("link")).unwrap();
    let preview = preview(&candidate(root.path(), Kind::Cache), DEFAULT_AGE_DAYS, &AtomicBool::new(false)).unwrap();
    assert_eq!(preview.file_count(), 1);
    let (relative, _) = preview.files().next().unwrap();
    assert_eq!(preview.base_directory().join(relative), directory.join("old.png"));
    assert_eq!(preview.logical_bytes, "old cache".len() as u64);
    std::fs::write(directory.join("created-after-preview"), "new").unwrap();
    let report = remove(&preview, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.files_removed, 1);
    assert!(!directory.join("old.png").exists());
    assert!(directory.join("recent.png").exists());
    assert!(directory.join("created-after-preview").exists());
    assert!(directory.join("link").is_symlink());
    assert_eq!(std::fs::read_to_string(outside.path().join("personal.txt")).unwrap(), "keep outside");
}

#[test]
fn changed_files_and_cancelled_previews_remove_nothing() {
    let root = tempfile::tempdir().unwrap();
    old_file(&root.path().join("cached"), "old");
    let candidate = candidate(root.path(), Kind::Cache);
    let selected = preview(&candidate, DEFAULT_AGE_DAYS, &AtomicBool::new(false)).unwrap();
    assert!(remove(&selected, &AtomicBool::new(true)).is_err());
    assert_eq!(std::fs::read_to_string(root.path().join("cached")).unwrap(), "old");
    std::fs::write(root.path().join("cached"), "personal replacement").unwrap();
    assert!(remove(&selected, &AtomicBool::new(false)).is_err());
    assert_eq!(std::fs::read_to_string(root.path().join("cached")).unwrap(), "personal replacement");
    assert!(preview(&candidate, DEFAULT_AGE_DAYS, &AtomicBool::new(true)).is_err());
    assert!(root.path().join("cached").exists());
}

#[test]
fn trash_information_is_removed_only_when_its_selected_payload_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    let info = root.path().join("info");
    std::fs::create_dir(&files).unwrap(); std::fs::create_dir(&info).unwrap();
    std::fs::create_dir(files.join("item")).unwrap();
    std::fs::write(files.join("item/document"), "trashed content").unwrap();
    let metadata = info.join("item.trashinfo");
    std::fs::write(&metadata, "[Trash Info]\nPath=/example/document\nDeletionDate=2026-10-01T12:00:00\n").unwrap();
    let selected = preview(&candidate(&files, Kind::Trash), DEFAULT_AGE_DAYS, &AtomicBool::new(false)).unwrap();
    std::fs::write(files.join("item/new-document"), "added later").unwrap();
    remove(&selected, &AtomicBool::new(false)).unwrap();
    assert!(metadata.exists(), "new content keeps its recovery information");
    let selected = preview(&candidate(&files, Kind::Trash), DEFAULT_AGE_DAYS, &AtomicBool::new(false)).unwrap();
    remove(&selected, &AtomicBool::new(false)).unwrap();
    assert!(!metadata.exists()); assert!(!files.join("item").exists());
    assert!(files.is_dir() && info.is_dir(), "Trash roots remain intact");
}

#[test]
fn critical_locations_and_symlink_swaps_are_refused() {
    let critical = [PathBuf::from("/home/test"), "/home/test/.local/share".into(), "/home/test/.config".into(), "/home/test/.local/state".into()];
    for path in ["/", "/home", "/home/test", "/home/test/.config", "/home/test/.config/data"] {
        assert!(locations::protected_cache(Path::new(path), &critical), "{path}");
    }
    assert!(!locations::protected_cache(Path::new("/home/test/.cache"), &critical));
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("cache"); std::fs::create_dir(&folder).unwrap();
    old_file(&folder.join("selected"), "old");
    let selected = preview(&candidate(root.path(), Kind::Cache), DEFAULT_AGE_DAYS, &AtomicBool::new(false)).unwrap();
    std::fs::rename(&folder, root.path().join("saved")).unwrap();
    std::os::unix::fs::symlink(root.path().join("saved"), &folder).unwrap();
    assert!(remove(&selected, &AtomicBool::new(false)).is_err());
    assert!(root.path().join("saved/selected").is_file());
}

#[test]
fn a_location_cannot_change_between_listing_and_preview_through_an_ancestor_alias() {
    let base = tempfile::tempdir().unwrap();
    let parent = base.path().join("parent");
    let cache = parent.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    old_file(&cache.join("selected"), "cache");
    let candidate = candidate(&cache, Kind::Cache);
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(outside.path().join("cache")).unwrap();
    old_file(&outside.path().join("cache/selected"), "personal content");
    std::fs::rename(&parent, base.path().join("saved")).unwrap();
    std::os::unix::fs::symlink(outside.path(), &parent).unwrap();
    assert!(preview(&candidate, DEFAULT_AGE_DAYS, &AtomicBool::new(false)).is_err());
    assert_eq!(std::fs::read_to_string(outside.path().join("cache/selected")).unwrap(), "personal content");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the disposable storage VM marker and synthetic Btrfs/ext4 disks"]
fn native_storage_vm_cleanup_never_crosses_even_a_same_device_bind_mount() {
    assert_eq!(std::fs::read_to_string("/mandara-storage-fixture").unwrap(), "mandara-disposable-storage-264\n");
    assert!(rustix::process::geteuid().is_root());
    let base = Path::new("/fixture/ext4/cleanup");
    std::fs::create_dir(base).unwrap();
    let cache = base.join("cache");
    let outside = base.join("outside");
    std::fs::create_dir(&cache).unwrap(); std::fs::create_dir(&outside).unwrap();
    old_file(&cache.join("old"), "old cache");
    old_file(&outside.join("personal"), "keep");
    std::fs::create_dir(cache.join("bound")).unwrap();
    let status = std::process::Command::new("mount").arg("--bind").arg(&outside).arg(cache.join("bound")).status().unwrap();
    assert!(status.success());
    let selected = preview(&candidate(base, Kind::Cache), DEFAULT_AGE_DAYS, &AtomicBool::new(false)).unwrap();
    assert_eq!(selected.file_count(), 1);
    assert!(selected.left_untouched > 0);
    remove(&selected, &AtomicBool::new(false)).unwrap();
    assert!(!cache.join("old").exists());
    assert_eq!(std::fs::read_to_string(outside.join("personal")).unwrap(), "keep");
    assert!(std::process::Command::new("umount").arg(cache.join("bound")).status().unwrap().success());
}
