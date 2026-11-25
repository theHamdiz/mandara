//! Terminate a separate process after publishing, before ownership commits.

use super::super::Ledger;
use std::path::PathBuf;

/// Distinguish the deliberate interruption from a failed assertion or panic.
const INTERRUPTED: i32 = 99;
const FIXTURE_DIRECTORY: &str = "MANDARA_LEDGER_CRASH_FIXTURE_DIRECTORY";
const MARKER: &str = "isolated ownership recovery fixture\n";

#[test]
fn a_terminated_writer_can_be_undone_by_a_fresh_process() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".mandara-fixture"), MARKER).unwrap();
    std::fs::write(root.path().join("theme"), "personal settings").unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "templates::ledger::journal::crash_tests::stop_after_publishing",
            "--ignored",
        ])
        .env(FIXTURE_DIRECTORY, root.path())
        .current_dir(root.path())
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(INTERRUPTED),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("theme")).unwrap(),
        "published palette"
    );
    let mut recovered = Ledger::read(&root.path().join("ledger.toml")).unwrap();
    assert!(
        recovered
            .take_back_checked("user/example")
            .errors
            .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("theme")).unwrap(),
        "personal settings"
    );
}

#[test]
#[ignore = "the parent regression supplies the isolated fixture directory"]
fn stop_after_publishing() {
    let Some(directory) = std::env::var_os(FIXTURE_DIRECTORY) else {
        return;
    };
    let root = PathBuf::from(directory).canonicalize().unwrap();
    assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert_eq!(
        std::fs::read_to_string(root.join(".mandara-fixture")).unwrap(),
        MARKER
    );
    let mut ledger = Ledger::read(&root.join("ledger.toml")).unwrap();
    ledger
        .put_file_with(
            "user/example",
            &root.join("theme"),
            "published palette",
            |path, body| {
                crate::templates::file_io::write(path, body)?;
                std::process::exit(INTERRUPTED);
            },
        )
        .unwrap();
}
