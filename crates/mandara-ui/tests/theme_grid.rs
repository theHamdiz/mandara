//! Seven-row theme navigation in a marked, private 4K/1.5 resource desktop.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use image::RgbaImage;
use serde_json::{Value, json};

/// Resource-check fixes this logical viewport at scale 1.5.
const SCREEN: (u32, u32) = (3840, 2160);
const SCALE: f32 = 1.5;
/// Only the Themes list is compared; the clock and action tiles are excluded.
const REGION: &str = "1106,218 528x334";
const CROP: (u32, u32) = (792, 501);
/// The last card's expected centre and a clear strip beyond its short label.
const LAST_Y: u32 = 525;
const LAST_X: [u32; 2] = [1260, 1470];
/// A 48-point row has 72 device pixels; allow its border and antialiasing.
const MIN_INTERIOR: u32 = 68;
/// Allow catalog loading and native input to finish before reading pixels.
const SETTLE: Duration = Duration::from_millis(800);
/// Every child and socket operation is bounded, including failed capture tools.
const TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);
const REPLY_LIMIT: u64 = 128 * 1024;
/// Keep the pointer present through capture and final-card activation.
const POINTER_HOLD_MS: &str = "4000";

struct Process(Child);

impl Process {
    /// Waits for this recorded tool only; Drop reaps it on failure or timeout.
    fn finish(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "native tool failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "native tool timed out");
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Process {
    /// Stops and reaps only the child this test spawned.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Rejects foreign markers, symlink escapes and non-private runtime permissions.
fn marked_runtime(runtime: &Path, root: &Path) -> bool {
    let (Ok(runtime), Ok(temporary), Ok(root)) = (
        runtime.canonicalize(),
        std::env::temp_dir().canonicalize(),
        root.canonicalize(),
    ) else {
        return false;
    };
    runtime.parent() == Some(temporary.as_path())
        && runtime
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("mrc."))
        && runtime.metadata().is_ok_and(|m| {
            m.uid() == rustix::process::getuid().as_raw() && m.mode() & 0o777 == 0o700
        })
        && std::fs::read_to_string(runtime.join(".mandara-theme-grid-test"))
            .is_ok_and(|marker| marker == root.to_string_lossy())
}

/// Reads PID start identity without relying on process names or command patterns.
fn start_ticks(pid: u32) -> std::io::Result<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("missing process start identity"))
}

struct Fixture {
    runtime: PathBuf,
    display: String,
    pointer: PathBuf,
    pid: u32,
    start: String,
    inode: u64,
}

impl Fixture {
    /// Checks the checkout marker, executable and private HOME/buses before input.
    fn from_env() -> Self {
        let runtime = PathBuf::from(
            std::env::var_os("MANDARA_THEME_GRID_TEST_RUNTIME").expect("private runtime"),
        );
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(
            marked_runtime(&runtime, &root),
            "refusing an unmarked desktop"
        );
        let runtime = runtime.canonicalize().unwrap();
        let display =
            std::env::var("MANDARA_THEME_GRID_TEST_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        assert!(
            !Path::new(&display).is_absolute() && Path::new(&display).components().count() == 1
        );
        assert!(runtime.join(&display).exists());
        let pid: u32 = std::env::var("MANDARA_THEME_GRID_TEST_PID")
            .unwrap()
            .parse()
            .unwrap();
        let process = PathBuf::from(format!("/proc/{pid}"));
        assert_eq!(
            process.metadata().unwrap().uid(),
            rustix::process::getuid().as_raw()
        );
        let binary = PathBuf::from(
            std::env::var_os("MANDARA_THEME_GRID_TEST_BINARY").expect("known candidate"),
        );
        let actual = process.join("exe").metadata().unwrap();
        let expected = binary.metadata().unwrap();
        assert_eq!(
            (actual.dev(), actual.ino()),
            (expected.dev(), expected.ino())
        );
        let bytes = std::fs::read(process.join("environ")).unwrap();
        let env: BTreeMap<_, _> = bytes
            .split(|b| *b == 0)
            .filter_map(|entry| std::str::from_utf8(entry).ok()?.split_once('='))
            .collect();
        assert_eq!(env.get("XDG_RUNTIME_DIR"), Some(&runtime.to_str().unwrap()));
        assert_eq!(
            env.get("DBUS_SYSTEM_BUS_ADDRESS"),
            Some(&"unix:path=/nonexistent")
        );
        let bus = format!("unix:path={}/bus", runtime.display());
        assert_eq!(env.get("DBUS_SESSION_BUS_ADDRESS"), Some(&bus.as_str()));
        let home = Path::new(env["HOME"]).canonicalize().unwrap();
        assert_eq!(home.file_name().unwrap(), "home");
        let work = home.parent().unwrap();
        assert_eq!(work.parent(), runtime.parent());
        assert_eq!(
            work.file_name().unwrap().to_string_lossy(),
            format!(
                "mandara-rc.{}",
                runtime
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .trim_start_matches("mrc.")
            )
        );
        assert_eq!(
            env.get("XDG_CONFIG_HOME"),
            Some(&home.join("config").to_str().unwrap())
        );
        let pointer = PathBuf::from(
            std::env::var_os("MANDARA_THEME_GRID_TEST_POINTER").expect("prebuilt vpointer example"),
        )
        .canonicalize()
        .unwrap();
        Self {
            runtime,
            display,
            pointer,
            pid,
            start: start_ticks(pid).unwrap(),
            inode: actual.ino(),
        }
    }

    /// A peer credential check prevents a stale socket or reused PID receiving writes.
    fn request(&self, args: &[&str]) -> std::io::Result<Value> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !marked_runtime(&self.runtime, &root)
            || start_ticks(self.pid)? != self.start
            || std::fs::metadata(format!("/proc/{}/exe", self.pid))?.ino() != self.inode
        {
            return Err(std::io::Error::other("private fixture identity changed"));
        }
        let mut socket = UnixStream::connect(self.runtime.join("mandara.sock"))?;
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
        // SAFETY: both output pointers reference initialized, correctly sized locals.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        assert_eq!(length as usize, std::mem::size_of_val(&credentials));
        assert_eq!(u32::try_from(credentials.pid).unwrap(), self.pid);
        assert_eq!(credentials.uid, rustix::process::getuid().as_raw());
        socket.set_read_timeout(Some(TIMEOUT))?;
        socket.set_write_timeout(Some(TIMEOUT))?;
        writeln!(socket, "{}", json!({"args":args}))?;
        let mut line = String::new();
        BufReader::new(socket)
            .take(REPLY_LIMIT)
            .read_line(&mut line)?;
        assert!(line.ends_with('\n'), "truncated shell reply");
        let reply: Value = serde_json::from_str(&line)?;
        assert_eq!(reply["ok"], true, "{reply}");
        Ok(reply["data"].clone())
    }

    /// Native tools inherit only the verified Wayland endpoint and fixed layout.
    fn tool(&self, name: impl AsRef<std::ffi::OsStr>) -> Command {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(marked_runtime(&self.runtime, &root));
        assert_eq!(start_ticks(self.pid).unwrap(), self.start);
        assert_eq!(
            std::fs::metadata(format!("/proc/{}/exe", self.pid))
                .unwrap()
                .ino(),
            self.inode
        );
        let mut cmd = Command::new(name);
        cmd.env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("WAYLAND_DISPLAY", &self.display)
            .env("MANDARA_LAYOUT", "2560x1440")
            .env("VPOINTER_HOLD_MS", POINTER_HOLD_MS)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        cmd
    }

    /// Keeps the device alive until its captured wheel position has been checked.
    fn pointer(&self, args: &[&str]) -> Process {
        Process(self.tool(&self.pointer).args(args).spawn().unwrap())
    }

    /// Captures only the list viewport, excluding unrelated changing UI content.
    fn capture(&self, name: &str) -> RgbaImage {
        let path = self.runtime.join(format!("theme-grid-{name}.png"));
        Process(
            self.tool("grim")
                .args(["-g", REGION])
                .arg(&path)
                .spawn()
                .unwrap(),
        )
        .finish();
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), CROP);
        image
    }
}

struct Restore<'a> {
    fixture: &'a Fixture,
    values: Vec<(&'static str, String)>,
}

impl Drop for Restore<'_> {
    /// Restores only the private fixture fields touched by this test, even on panic.
    fn drop(&mut self) {
        let _ = self.fixture.request(&["panel", "close"]);
        for (path, value) in &self.values {
            let _ = self.fixture.request(&["config", "set", path, value]);
        }
    }
}

/// A full final row has an enclosed 48-point interior, rather than a clipped tail.
fn last_card_is_whole(image: &RgbaImage, rtl: bool) -> bool {
    let x = ((LAST_X[usize::from(rtl)] - 1106) as f32 * SCALE).round() as u32;
    let y = ((LAST_Y - 218) as f32 * SCALE).round() as u32;
    let colour = *image.get_pixel(x, y);
    let mut top = y;
    let mut bottom = y + 1;
    while top > 0 && *image.get_pixel(x, top - 1) == colour {
        top -= 1;
    }
    while bottom < image.height() && *image.get_pixel(x, bottom) == colour {
        bottom += 1;
    }
    top > 0 && bottom < image.height() && bottom - top >= MIN_INTERIOR
}

/// Missing/foreign markers and broad permissions never authorize native input.
#[test]
fn theme_grid_guard_requires_its_private_checkout_marker() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let runtime = tempfile::Builder::new().prefix("mrc.").tempdir().unwrap();
    std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!marked_runtime(runtime.path(), root.path()));
    let marker = runtime.path().join(".mandara-theme-grid-test");
    std::fs::write(&marker, "another checkout").unwrap();
    assert!(!marked_runtime(runtime.path(), root.path()));
    std::fs::write(
        &marker,
        root.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    assert!(marked_runtime(runtime.path(), root.path()));
    std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!marked_runtime(runtime.path(), root.path()));
}

/// Pixel bounds reject a row cut by the viewport even when its centre is visible.
#[test]
fn final_row_probe_distinguishes_full_and_clipped_cards() {
    let mut image = RgbaImage::new(CROP.0, CROP.1);
    for y in 400..470 {
        for x in 0..CROP.0 {
            image.put_pixel(x, y, image::Rgba([40, 80, 120, 255]));
        }
    }
    assert!(last_card_is_whole(&image, false));
    assert!(last_card_is_whole(&image, true));
    for y in 470..CROP.1 {
        for x in 0..CROP.0 {
            image.put_pixel(x, y, image::Rgba([40, 80, 120, 255]));
        }
    }
    assert!(!last_card_is_whole(&image, false));
    assert!(!last_card_is_whole(&image, true));
}

/// Real wheel input must move seven rows inside the fixed menu and reveal the
/// complete final card; clicking it must select its actual custom theme.
#[test]
#[ignore = "requires a marked 4K/1.5 resource desktop, 19 themes ending in qa-extra-4, grim, wtype and prebuilt vpointer"]
fn seven_theme_rows_scroll_and_reveal_the_complete_final_card() {
    let fixture = Fixture::from_env();
    let config = fixture.request(&["config", "get"]).unwrap();
    assert_eq!(config["general"]["offline"], true);
    let _restore = Restore {
        fixture: &fixture,
        values: [
            "/general/locale",
            "/theme/mode",
            "/theme/source",
            "/motion/reduce_motion",
            "/panels/menu_rail",
        ]
        .into_iter()
        .map(|path| (path, config.pointer(path).unwrap().to_string()))
        .collect(),
    };
    assert_eq!(
        fixture
            .request(&["theme", "list"])
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        19
    );
    fixture
        .request(&["config", "set", "/motion/reduce_motion", "true"])
        .unwrap();
    fixture
        .request(&["config", "set", "/panels/menu_rail", "full"])
        .unwrap();
    let _keyboard = Process(fixture.tool("wtype").args(["-s", "60000"]).spawn().unwrap());
    let directory = tempfile::tempdir().unwrap();
    let full = directory.path().join("geometry.png");
    Process(fixture.tool("grim").arg(&full).spawn().unwrap()).finish();
    assert_eq!(image::image_dimensions(full).unwrap(), SCREEN);
    fixture.request(&["panel", "open", "main-menu"]).unwrap();
    std::thread::sleep(SETTLE);
    let mut click = fixture.pointer(&["click", "950", "173"]);
    std::thread::sleep(SETTLE);
    for (locale, rtl) in [("english", false), ("arabic", true)] {
        for mode in ["light", "dark"] {
            fixture
                .request(&[
                    "config",
                    "set",
                    "/theme/source",
                    r#"{"kind":"preset","preset":"mandara-corniche"}"#,
                ])
                .unwrap();
            fixture
                .request(&["config", "set", "/general/locale", locale])
                .unwrap();
            fixture
                .request(&["config", "set", "/theme/mode", mode])
                .unwrap();
            fixture.request(&["panel", "open", "main-menu"]).unwrap();
            std::thread::sleep(SETTLE);
            let _up = fixture.pointer(&["scroll", "1400", "450", "-12"]);
            std::thread::sleep(SETTLE);
            let before = fixture.capture(&format!("{locale}-{mode}-top"));
            let _down = fixture.pointer(&["scroll", "1400", "450", "12"]);
            std::thread::sleep(SETTLE);
            let after = fixture.capture(&format!("{locale}-{mode}-bottom"));
            assert_ne!(
                before, after,
                "wheel must move the actual theme rows ({locale}/{mode})"
            );
            assert!(
                last_card_is_whole(&after, rtl),
                "the complete final card must remain inside the list viewport ({locale}/{mode})"
            );
            let x = if rtl { "1540" } else { "1180" };
            let _select = fixture.pointer(&["click", x, "525"]);
            std::thread::sleep(SETTLE);
            assert_eq!(
                fixture
                    .request(&["config", "get", "/theme/source"])
                    .unwrap(),
                json!({"kind":"preset","preset":"qa-extra-4"}),
                "the visible final card must activate ({locale}/{mode})"
            );
            fixture.capture(&format!("{locale}-{mode}-selected"));
        }
    }
    click.finish();
}
