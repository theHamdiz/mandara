//! The installer must start before its GUI runtime packages exist.

#[test]
fn the_binary_does_not_require_keyboard_libraries_at_process_start() {
    let report = std::process::Command::new("readelf")
        .args(["--dynamic", env!("CARGO_BIN_EXE_mandara")]).env("LC_ALL", "C").output().unwrap();
    assert!(report.status.success(), "{}", String::from_utf8_lossy(&report.stderr));
    let report = String::from_utf8(report.stdout).unwrap();
    let needed: Vec<_> = report.lines().filter(|line| line.contains("(NEEDED)")).collect();
    assert!(!needed.is_empty(), "expected the Linux C runtime dependencies");
    assert!(!needed.iter().any(|line| line.contains("libxkbcommon")), "{report}");
}
