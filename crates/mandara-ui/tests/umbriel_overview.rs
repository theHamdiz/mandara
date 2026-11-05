//! Checks real workspace previews only in an explicitly marked private desktop.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mandara_services::screencopy::{Capture, capture_all};

const MARKER: &str = "mandara-umbriel-110-test\n";
/// Allow the private compositor's overview animation to finish before capture.
const SETTLE: Duration = Duration::from_secs(1);

fn private_runtime(path: &Path) -> bool {
    path.canonicalize().is_ok_and(|p| p.starts_with(std::env::temp_dir()) && std::fs::read_to_string(p.join(".mandara-test")).is_ok_and(|s| s == MARKER))
}

fn overview(binary: &Path, action: &str) {
    let output = Command::new(binary).args(["msg", action]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    std::thread::sleep(SETTLE);
}

struct CloseOverview(PathBuf);
impl Drop for CloseOverview {
    fn drop(&mut self) { let _ = Command::new(&self.0).args(["msg", "overview-close"]).output(); }
}

/// Sample inside the workspace tile, away from its rounded edge and the bar.
fn center_energy(frame: &Capture) -> u64 {
    let (x, y) = (frame.width / 2, frame.height / 2);
    let mut energy = 0;
    for dy in y.saturating_sub(8)..(y + 8).min(frame.height) {
        for dx in x.saturating_sub(8)..(x + 8).min(frame.width) {
            let at = ((dy * frame.width + dx) * 4) as usize;
            energy += frame.rgba[at..at + 3].iter().map(|v| u64::from(*v)).sum::<u64>();
        }
    }
    energy
}

#[test]
fn overview_fixture_rejects_unmarked_runtime() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!private_runtime(dir.path()));
    std::fs::write(dir.path().join(".mandara-test"), "another fixture\n").unwrap();
    assert!(!private_runtime(dir.path()));
    std::fs::write(dir.path().join(".mandara-test"), MARKER).unwrap();
    assert!(private_runtime(dir.path()));
}

#[test]
#[ignore = "requires a marked private Umbriel desktop with two outputs, a coloured wallpaper and no application windows"]
fn overview_does_not_mirror_an_opaque_black_wallpaper_root() {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("private XDG_RUNTIME_DIR"));
    assert!(private_runtime(&runtime), "refusing to control an unmarked desktop");
    let runtime = runtime.canonicalize().unwrap();
    let socket = PathBuf::from(std::env::var_os("UMBRIEL_SOCKET").expect("private UMBRIEL_SOCKET"));
    let display = runtime.join(std::env::var_os("WAYLAND_DISPLAY").expect("private WAYLAND_DISPLAY"));
    assert!(socket.canonicalize().unwrap().starts_with(&runtime));
    assert!(display.canonicalize().unwrap().starts_with(&runtime));
    let binary = PathBuf::from(std::env::var_os("MANDARA_UMBRIEL_TEST_BIN").expect("private compositor binary"));
    let _close = CloseOverview(binary.clone());
    overview(&binary, "overview-close");
    let desktop = capture_all().unwrap();
    assert!(desktop.len() >= 2, "the regression covers every output");
    assert!(desktop.iter().all(|frame| center_energy(frame) > 10_000), "fixture needs coloured wallpaper on every output");
    overview(&binary, "overview-open");
    let previews = capture_all().unwrap();
    assert_eq!(previews.len(), desktop.len());
    for frame in previews {
        assert!(center_energy(&frame) > 1_000, "{} has an opaque black overview tile", frame.output);
    }
}
