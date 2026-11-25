use super::*;
use std::fs;

const OWNER: &str = "user/example";

#[test]
fn an_unreadable_target_keeps_ownership_until_undo_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    fs::write(&path, "personal").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger.put_file(OWNER, &path, "palette").unwrap();
    let second = root.path().join("z-theme");
    fs::write(&second, "second personal file").unwrap();
    ledger.put_file(OWNER, &second, "second palette").unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();

    let report = ledger.take_back_checked(OWNER);
    assert_eq!(report.errors.len(), 1);
    assert!(path.is_dir());
    assert!(ledger.has(OWNER));
    assert!(ledger.pending.is_some());
    assert_eq!(fs::read_to_string(&second).unwrap(), "second palette");
    drop(ledger);

    let mut recovered = Ledger::read(&record).unwrap();
    assert!(recovered.has(OWNER));
    fs::remove_dir(&path).unwrap();
    fs::write(&path, "palette").unwrap();
    assert!(recovered.take_back_checked(OWNER).errors.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "personal");
    assert_eq!(fs::read_to_string(&second).unwrap(), "second personal file");
    assert!(recovered.is_empty());
}

#[test]
fn failed_restore_sync_recovers_files_links_and_created_directories() {
    for original in ["file", "link", "absent"] {
        for missing_target in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let record = root.path().join("ledger.toml");
            let directory = root.path().join("app");
            let path = directory.join("theme");
            if original != "absent" {
                fs::create_dir(&directory).unwrap();
                if original == "link" {
                    fs::write(directory.join("personal"), "personal target").unwrap();
                    std::os::unix::fs::symlink("personal", &path).unwrap();
                } else {
                    fs::write(&path, "personal").unwrap();
                }
            }
            let mut ledger = Ledger::read(&record).unwrap();
            ledger.put_file(OWNER, &path, "palette").unwrap();
            if missing_target {
                fs::remove_file(&path).unwrap();
            }
            let mut report = SyncReport::default();
            let mut sync_attempted = false;
            let result = ledger.restore_file_with(path.to_str().unwrap(), &mut report, |_| {
                sync_attempted = true;
                Err(io::Error::other("injected directory sync failure"))
            });
            // An absent, already-removed created file has no mutation to sync.
            if original == "absent" && missing_target {
                assert!(result.is_ok());
                assert!(!sync_attempted);
            } else {
                assert!(result.is_err());
                assert!(sync_attempted);
                assert!(ledger.pending.is_some());
                assert!(ledger.has(OWNER));
            }
            drop(ledger);

            let recovered = Ledger::read(&record).unwrap();
            assert!(recovered.is_empty(), "{original}, missing={missing_target}");
            match original {
                "file" => assert_eq!(fs::read_to_string(&path).unwrap(), "personal"),
                "link" => {
                    assert_eq!(fs::read_link(&path).unwrap(), Path::new("personal"));
                    assert_eq!(
                        fs::read_to_string(directory.join("personal")).unwrap(),
                        "personal target"
                    );
                }
                _ => assert!(!directory.exists()),
            }
            assert!(!record.exists());
        }
    }
}
