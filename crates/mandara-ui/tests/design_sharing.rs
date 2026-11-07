//! Full reading-card PNG delivery in a marked private desktop with real content.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use image::RgbaImage;
use serde_json::Value;

#[path = "support/private_desktop.rs"]
mod private_desktop;
use private_desktop::PrivateDesktop;

/// A stalled clipboard owner or tool must not leave this fixture blocked.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
/// The production PNG payload bound, plus one byte to detect overflow.
const PNG_LIMIT: u64 = (32 << 20) + 1;
/// Let live configuration reach the existing plugin models before the next export.
const SETTLE: Duration = Duration::from_millis(350);
/// Poll only the short-lived owned child while its output is being drained.
const CHILD_WAIT: Duration = Duration::from_millis(10);
/// The panel's scrolling body; a long complete export must extend beyond it.
const PANEL_VIEWPORT: u32 = 640;
/// Keep a private seat keyboard alive while clipboard ownership changes.
const KEYBOARD_LIFETIME: Duration = Duration::from_secs(180);

struct Process(Child);
impl Process {
    fn finish(&mut self) {
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "clipboard tool failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "clipboard tool timed out");
            std::thread::sleep(CHILD_WAIT);
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn clipboard(fixture: &PrivateDesktop, arguments: &[&str], limit: u64) -> Vec<u8> {
    fixture.check().unwrap();
    let mut process = Process(
        fixture
            .tool("wl-paste")
            .unwrap()
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let output = process.0.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output.take(limit).read_to_end(&mut bytes).map(|_| bytes);
        let _ = send.send(result);
    });
    let bytes = receive
        .recv_timeout(TRANSFER_TIMEOUT)
        .expect("private PNG transfer timed out")
        .unwrap();
    process.finish();
    reader.join().unwrap();
    assert!(
        (bytes.len() as u64) < limit,
        "clipboard data exceeds its bound"
    );
    bytes
}

struct Restore<'a> {
    fixture: &'a PrivateDesktop,
    fields: Vec<(&'static str, Value)>,
}
impl Drop for Restore<'_> {
    fn drop(&mut self) {
        for (path, value) in &self.fields {
            if let Err(error) = self
                .fixture
                .request(&["config", "set", path, &value.to_string()])
            {
                let _ = writeln!(
                    std::io::stderr(),
                    "private design-share restoration stopped: {error}"
                );
                break;
            }
        }
    }
}

fn footer_luminance(image: &RgbaImage) -> u32 {
    let pixel = image.get_pixel(image.width() / 2, image.height() - 8);
    u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])
}

/// The runner preloads real verified Quran text/English meaning and Nawawi
/// Arabic/English into this throwaway HOME; no network or audio is used here.
#[test]
#[ignore = "requires the marked offline reading-content desktop and private Wayland clipboard"]
fn complete_designs_reach_the_clipboard_in_both_languages_and_modes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = PrivateDesktop::from_env(
        "MANDARA_DESIGN_SHARE_TEST",
        &root,
        ".mandara-design-share-test",
    )
    .expect("verified private desktop");
    let _keyboard = Process(
        fixture
            .tool("wtype")
            .unwrap()
            .args(["-s", &KEYBOARD_LIFETIME.as_millis().to_string()])
            .spawn()
            .unwrap(),
    );
    assert_eq!(
        fixture
            .request(&["config", "get", "/general/offline"])
            .unwrap(),
        true
    );
    let _restore = Restore {
        fixture: &fixture,
        fields: ["/general/locale", "/theme/mode", "/plugins"]
            .into_iter()
            .map(|path| (path, fixture.request(&["config", "get", path]).unwrap()))
            .collect(),
    };
    for plugin in ["quran", "hadith", "jannah"] {
        fixture.request(&[plugin, "language", "follow"]).unwrap();
    }
    let catalog = mandara_services::jannah::catalog::catalog();
    let longest = catalog
        .hadith_ids()
        .filter_map(|id| catalog.get(id))
        .max_by_key(|item| item.full_arabic().len())
        .unwrap();
    let mut dark = Vec::new();
    for language in ["english", "arabic"] {
        fixture
            .request(&[
                "config",
                "set",
                "/general/locale",
                &format!("\"{language}\""),
            ])
            .unwrap();
        // Only the runner’s verified local text/meaning is loaded; offline is asserted above.
        fixture.request(&["quran", "download"]).unwrap();
        for mode in ["dark", "light"] {
            fixture
                .request(&["config", "set", "/theme/mode", &format!("\"{mode}\"")])
                .unwrap();
            std::thread::sleep(SETTLE);
            for (plugin, reference) in [
                ("quran", "2:282"),
                ("hadith", "nawawi 24"),
                ("jannah", longest.key.as_str()),
            ] {
                let reply = fixture.request(&[plugin, "copy-image", reference]).unwrap();
                assert_eq!(reply["copied"], true);
                assert_eq!(reply["mime"], "image/png");
                let types = clipboard(&fixture, &["--list-types"], 4096);
                assert!(
                    String::from_utf8(types)
                        .unwrap()
                        .lines()
                        .any(|mime| mime == "image/png")
                );
                let png = clipboard(
                    &fixture,
                    &["--no-newline", "--type", "image/png"],
                    PNG_LIMIT,
                );
                let image = image::load_from_memory(&png).unwrap().into_rgba8();
                assert!(
                    image.width() > 0 && image.width() < 1024,
                    "a card export must not be a desktop capture"
                );
                assert!(
                    image.height() > PANEL_VIEWPORT,
                    "{plugin}: the complete long entry must exceed the panel viewport"
                );
                assert_eq!(
                    image.get_pixel(0, 0)[3],
                    0,
                    "rounded corners keep transparency"
                );
                let name = format!("design-share-{plugin}-{language}-{mode}.png");
                std::fs::write(fixture.artifact(&name).unwrap(), &png).unwrap();
                if mode == "dark" {
                    dark.push((language, plugin, footer_luminance(&image)));
                } else {
                    assert!(
                        footer_luminance(&image)
                            > dark
                                .iter()
                                .find(|(lang, name, _)| *lang == language && *name == plugin)
                                .unwrap()
                                .2,
                        "the exported ground follows light mode"
                    );
                }
                fixture.check().unwrap();
            }
        }
    }
}

/// The Image action runs outside GPUI's render phase, including its redraws.
#[test]
#[ignore = "requires the marked private desktop and its recorded pointer tool"]
fn image_button_publishes_the_current_entry() {
    use std::os::unix::fs::MetadataExt as _;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = PrivateDesktop::from_env(
        "MANDARA_DESIGN_SHARE_TEST",
        &root,
        ".mandara-design-share-test",
    )
    .expect("verified private desktop");
    let pointer = std::env::var("MANDARA_DESIGN_SHARE_POINTER")
        .expect("the runner must supply its private pointer tool");
    let pointer = Path::new(&pointer).canonicalize().unwrap();
    let metadata = pointer.metadata().unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.uid(), rustix::process::getuid().as_raw());
    assert_ne!(metadata.mode() & 0o111, 0);
    let _keyboard = Process(
        fixture
            .tool("wtype")
            .unwrap()
            .args(["-s", &KEYBOARD_LIFETIME.as_millis().to_string()])
            .spawn()
            .unwrap(),
    );
    let _restore = Restore {
        fixture: &fixture,
        fields: ["/general/locale", "/theme/mode", "/plugins"]
            .into_iter()
            .map(|path| (path, fixture.request(&["config", "get", path]).unwrap()))
            .collect(),
    };
    fixture
        .request(&["config", "set", "/general/locale", "\"english\""])
        .unwrap();
    fixture
        .request(&["config", "set", "/theme/mode", "\"light\""])
        .unwrap();
    fixture.request(&["jannah", "language", "follow"]).unwrap();
    fixture
        .request(&["jannah", "show", "bukhari-5982"])
        .unwrap();
    std::thread::sleep(SETTLE);
    let mut seed = Process(
        fixture
            .tool("wl-copy")
            .unwrap()
            .args(["--type", "text/plain"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    seed.0
        .stdin
        .take()
        .unwrap()
        .write_all(b"private image-button control")
        .unwrap();
    seed.finish();
    // This short bundled card's painted Image pill in resource-check's 4K/1.5 layout.
    let status = fixture
        .tool(pointer.to_str().unwrap())
        .unwrap()
        .env("MANDARA_LAYOUT", "2560x1440")
        .args(["click", "1369", "568"])
        .status()
        .unwrap();
    assert!(status.success());
    fixture
        .check()
        .expect("the actual Image click must not panic");
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    let png = loop {
        let bytes = clipboard(&fixture, &["--no-newline"], PNG_LIMIT);
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            break bytes;
        }
        assert!(
            Instant::now() < deadline,
            "Image click never published its PNG"
        );
        assert_eq!(bytes, b"private image-button control");
        std::thread::sleep(CHILD_WAIT);
    };
    let image = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(image.width(), 618);
    assert!(image.height() > 500);
    assert_eq!(image.get_pixel(0, 0)[3], 0);
    std::fs::write(
        fixture.artifact("design-share-actual-button.png").unwrap(),
        png,
    )
    .unwrap();
    fixture.request(&["panel", "close"]).unwrap();
    assert!(fixture.request(&["shell", "state"]).unwrap()["panel"].is_null());
    fixture.check().unwrap();
}
