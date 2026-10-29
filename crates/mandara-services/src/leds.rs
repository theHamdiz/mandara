//! Keyboard LEDs for on-screen displays: Caps / Num / Scroll Lock and the
//! keyboard backlight level (`/sys/class/leds/*::kbd_backlight`).
//!
//! LED attributes don't notify, so the (tiny) sysfs files are sampled a
//! few times a second through descriptors opened once — and only while
//! `enabled` is set; otherwise the task idles.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

const LEDS_ROOT: &str = "/sys/class/leds";
/// Sampling period while enabled.
const SAMPLE: Duration = Duration::from_millis(200);
/// Re-check of the enable flag while idle.
const IDLE_CHECK: Duration = Duration::from_secs(1);
/// Re-scan for keyboards plugged in later.
const RESCAN: Duration = Duration::from_secs(10);
const UPDATE_QUEUE: usize = 4;
const SYSFS_NUMBER_BYTES: usize = 16;
const CAPS: &str = "::capslock";
const NUM: &str = "::numlock";
const SCROLL: &str = "::scrolllock";
const KBD_BACKLIGHT: &str = "::kbd_backlight";

/// One sample of every keyboard indicator.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize)]
pub struct LedState {
    pub caps: bool,
    pub num: bool,
    pub scroll: bool,
    /// Keyboard backlight `0..=1`, when the machine has one.
    pub kbd_backlight: Option<f32>,
}

/// What changed between two samples.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LedChange {
    CapsLock(bool),
    NumLock(bool),
    ScrollLock(bool),
    KeyboardBacklight(f32),
}

struct Files {
    caps: Vec<std::fs::File>,
    num: Vec<std::fs::File>,
    scroll: Vec<std::fs::File>,
    /// `(brightness file, max)`.
    kbd: Option<(std::fs::File, u32)>,
}

fn read_u32_file(file: &std::fs::File) -> Option<u32> {
    use std::os::unix::fs::FileExt;
    let mut buf = [0u8; SYSFS_NUMBER_BYTES];
    let n = file.read_at(&mut buf, 0).ok()?;
    std::str::from_utf8(&buf[..n]).ok()?.trim().parse().ok()
}

fn led_dirs(suffix: &str) -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(LEDS_ROOT) else { return Vec::new() };
    dir.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(suffix)).map(|e| e.path()).collect()
}

fn open_all(suffix: &str) -> Vec<std::fs::File> {
    led_dirs(suffix).iter().filter_map(|d| std::fs::File::open(d.join("brightness")).ok()).collect()
}

impl Files {
    fn open() -> Self {
        let kbd = led_dirs(KBD_BACKLIGHT).into_iter().find_map(|d| {
            let max = std::fs::read_to_string(d.join("max_brightness")).ok()?.trim().parse::<u32>().ok().filter(|m| *m > 0)?;
            Some((std::fs::File::open(d.join("brightness")).ok()?, max))
        });
        Self { caps: open_all(CAPS), num: open_all(NUM), scroll: open_all(SCROLL), kbd }
    }

    fn is_empty(&self) -> bool {
        self.caps.is_empty() && self.num.is_empty() && self.scroll.is_empty() && self.kbd.is_none()
    }

    fn sample(&self) -> LedState {
        let lit = |files: &[std::fs::File]| files.iter().any(|f| read_u32_file(f).is_some_and(|v| v > 0));
        LedState {
            caps: lit(&self.caps),
            num: lit(&self.num),
            scroll: lit(&self.scroll),
            kbd_backlight: self.kbd.as_ref().and_then(|(f, max)| read_u32_file(f).map(|v| v.min(*max) as f32 / *max as f32)),
        }
    }
}

/// The current indicator state (for `mandara msg osd …`).
pub fn current() -> LedState {
    Files::open().sample()
}

/// Every change between `a` and `b`.
pub fn diff(a: &LedState, b: &LedState) -> Vec<LedChange> {
    let mut out = Vec::new();
    if a.caps != b.caps {
        out.push(LedChange::CapsLock(b.caps));
    }
    if a.num != b.num {
        out.push(LedChange::NumLock(b.num));
    }
    if a.scroll != b.scroll {
        out.push(LedChange::ScrollLock(b.scroll));
    }
    if let (Some(x), Some(y)) = (a.kbd_backlight, b.kbd_backlight)
        && (x - y).abs() > f32::EPSILON
    {
        out.push(LedChange::KeyboardBacklight(y));
    }
    out
}

/// Reports indicator changes while `enabled`. `None` without any keyboard
/// LED at all.
pub fn watch(enabled: Arc<AtomicBool>) -> Option<async_channel::Receiver<LedChange>> {
    if Files::open().is_empty() {
        return None;
    }
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    crate::runtime::spawn("keyboard-leds", async move {
        let mut files = Files::open();
        let mut last = files.sample();
        let mut scanned = Instant::now();
        loop {
            if tx.is_closed() {
                return;
            }
            if !enabled.load(Ordering::Relaxed) {
                crate::runtime::sleep(IDLE_CHECK).await;
                // Don't report what changed while off.
                last = files.sample();
                continue;
            }
            crate::runtime::sleep(SAMPLE).await;
            if scanned.elapsed() >= RESCAN {
                scanned = Instant::now();
                files = Files::open();
            }
            let now = files.sample();
            for change in diff(&last, &now) {
                if tx.send(change).await.is_err() {
                    return;
                }
            }
            last = now;
        }
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_report_each_change() {
        let a = LedState { kbd_backlight: Some(0.5), ..Default::default() };
        let b = LedState { caps: true, scroll: true, kbd_backlight: Some(1.0), ..Default::default() };
        assert_eq!(diff(&a, &b), vec![LedChange::CapsLock(true), LedChange::ScrollLock(true), LedChange::KeyboardBacklight(1.0)]);
        assert!(diff(&b, &b).is_empty());
        // A backlight appearing isn't a change of level.
        let c = LedState { kbd_backlight: None, ..b };
        assert!(diff(&c, &b).is_empty());
    }

    #[test]
    fn sampling_works_on_this_machine() {
        let _ = current();
    }
}
