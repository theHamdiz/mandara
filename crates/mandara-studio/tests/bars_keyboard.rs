//! Keyboard tab selection through Studio's production view in a private desktop.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

struct Keyboard(Child);
impl Drop for Keyboard {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn private_runtime(runtime: &Path, sandbox: &Path) -> bool {
    private_runtime_under(runtime, sandbox, &std::env::temp_dir())
}

fn private_runtime_under(runtime: &Path, sandbox: &Path, temporary: &Path) -> bool {
    let Ok(temporary) = temporary.canonicalize() else { return false; };
    runtime.canonicalize().is_ok_and(|path| path.starts_with(&temporary)
        && sandbox.canonicalize().is_ok_and(|root| std::fs::read_to_string(path.join(".mandara-sandbox")).is_ok_and(|marker| marker == root.to_string_lossy())))
}

#[test]
fn keyboard_fixture_rejects_an_unmarked_desktop() {
    let runtime = tempfile::tempdir().unwrap();
    let sandbox = tempfile::tempdir().unwrap();
    assert!(!private_runtime(runtime.path(), sandbox.path()));
    std::fs::write(runtime.path().join(".mandara-sandbox"), "another desktop").unwrap();
    assert!(!private_runtime(runtime.path(), sandbox.path()));
}

#[test]
fn keyboard_fixture_accepts_a_symlinked_temporary_root_and_rejects_escapes() {
    let fixture = tempfile::tempdir().unwrap();
    let temporary = fixture.path().join("temporary");
    let outside = fixture.path().join("outside");
    std::fs::create_dir(&temporary).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let alias = fixture.path().join("temporary-link");
    std::os::unix::fs::symlink(&temporary, &alias).unwrap();
    let runtime = tempfile::tempdir_in(&alias).unwrap();
    std::fs::write(runtime.path().join(".mandara-sandbox"), fixture.path().canonicalize().unwrap().to_string_lossy().as_bytes()).unwrap();
    assert!(private_runtime_under(runtime.path(), fixture.path(), &alias));
    assert!(!private_runtime_under(runtime.path(), &outside, &alias));
    assert!(!private_runtime_under(runtime.path(), fixture.path(), &alias.join("missing")));
    std::fs::write(outside.join(".mandara-sandbox"), fixture.path().canonicalize().unwrap().to_string_lossy().as_bytes()).unwrap();
    let escape = temporary.join("escape");
    std::os::unix::fs::symlink(&outside, &escape).unwrap();
    assert!(!private_runtime_under(&escape, fixture.path(), &alias));
}

#[test]
#[ignore = "requires a marked two-output desktop, English dark Studio at 1100x680 on HEADLESS-2, grim and wtype"]
fn tab_arrow_and_enter_reach_the_sidebar_editor() {
    let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox");
    let runtime = PathBuf::from(std::env::var_os("MANDARA_STUDIO_TEST_RUNTIME").expect("private runtime"));
    assert!(private_runtime(&runtime, &sandbox), "refusing synthetic input outside the marked fixture");
    let xtask = PathBuf::from(std::env::var_os("MANDARA_STUDIO_TEST_XTASK").expect("private controller"));
    let control = |words: &[&str]| {
        let output = Command::new(&xtask).env("MANDARA_SANDBOX_RUNTIME", &runtime).arg("sandbox").args(words).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    };
    let files = tempfile::tempdir().unwrap();
    let capture = |name: &str| {
        let path = files.path().join(name);
        let status = Command::new("grim").env_clear().env("XDG_RUNTIME_DIR", &runtime).env("WAYLAND_DISPLAY", "wayland-0").args(["-o", "HEADLESS-2"]).arg(&path).status().unwrap();
        assert!(status.success());
        image::open(path).unwrap().crop_imm(450, 260, 780, 490).to_rgba8()
    };
    control(&["msg", "studio", "bars/sidebar"]);
    std::thread::sleep(Duration::from_secs(1));
    let expected = capture("sidebar.png");
    control(&["msg", "studio", "bars"]);
    let _keyboard = Keyboard(Command::new("wtype").env_clear().env("XDG_RUNTIME_DIR", &runtime).env("WAYLAND_DISPLAY", "wayland-0")
        .args(["-s", "1000", "-k", "Tab", "-s", "150", "-k", "Right", "-s", "150", "-k", "Tab", "-s", "150", "-k", "Return", "-s", "10000"]).spawn().unwrap());
    // The controller normalizes against the first 1365x769 output; the
    // desktop is 2805x900. This point lands in Studio's search field.
    control(&["pointer", "click", "801", "162"]);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(capture("keyboard.png"), expected, "Tab must enter the tab strip, Right select Side bars, and Tab/Enter open Sidebar");
}
