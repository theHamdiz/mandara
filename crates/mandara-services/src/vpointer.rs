//! Dev tool: drives the pointer through `zwlr_virtual_pointer_v1` so shell
//! interactions (clicks inside panels, sidebar, toasts) can be reproduced
//! and tested end to end. Used by `examples/vpointer.rs` (the session the
//! environment names) and by `cargo xtask sandbox pointer` (the headless
//! sandbox's socket only), which compiles this file directly so it stays
//! free of the rest of the crate.
//!
//! Gestures (`Gesture::parse`):
//!
//! ```text
//! move   X Y
//! click  X Y [left|right|middle]
//! drag   X1 Y1 X2 Y2
//! flyby  X Y MS      # touch X Y for MS ms, then leave
//! scroll X Y STEPS   # wheel notches (negative = up)
//! ```
//!
//! Coordinates are logical pixels in the output layout (`Options::layout`;
//! `MANDARA_LAYOUT=WxH` in `Options::from_env`, default 2560x1440).
//! `VPOINTER_HOLD_MS=N` keeps the virtual pointer device alive N ms after the
//! action: MangoWM sends the surface under the cursor a pointer leave when
//! the device goes away, which a real mouse never does.
//! `VPOINTER_DRAG_HOLD_MS=N` keeps the button down at the end of a drag (to
//! screenshot a drag in progress).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1 as mgr, zwlr_virtual_pointer_v1 as vp};

/// Keeps the device alive this many ms after the gesture.
pub const HOLD_ENV: &str = "VPOINTER_HOLD_MS";
/// Milliseconds a `drag` holds the button at its end before releasing.
pub const DRAG_HOLD_ENV: &str = "VPOINTER_DRAG_HOLD_MS";
/// `WxH` logical size of the output layout.
pub const LAYOUT_ENV: &str = "MANDARA_LAYOUT";
/// Layout size when `MANDARA_LAYOUT` is unset.
pub const DEFAULT_LAYOUT: (u32, u32) = (2560, 1440);
/// Command-line usage of the gestures.
pub const USAGE: &str = "move X Y | click X Y [left|right|middle] | drag X1 Y1 X2 Y2 | flyby X Y MS | scroll X Y STEPS (negative: up)";

/// Scroll distance of one wheel notch (wl_pointer axis units).
const SCROLL_NOTCH: f64 = 15.0;
/// Linux input event codes of the mouse buttons.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// Offset of the first approach step, so the target sees enter + motion.
const APPROACH_OFFSET: i32 = 6;
/// Pause between approach steps.
const APPROACH_PAUSE: Duration = Duration::from_millis(60);
/// How long a click holds the button.
const CLICK_HOLD: Duration = Duration::from_millis(70);
/// A beat between pressing and moving at the start of a drag.
const DRAG_START_PAUSE: Duration = Duration::from_millis(150);
/// Motion events of a drag, one per frame.
const DRAG_STEPS: u32 = 20;
/// One frame at 60 Hz (drag motion and wheel notches).
const FRAME: Duration = Duration::from_millis(16);
/// wl_seat versions this tool can bind.
const SEAT_VERSIONS: std::ops::RangeInclusive<u32> = 1..=8;
/// zwlr_virtual_pointer_manager_v1 versions this tool can bind.
const MANAGER_VERSIONS: std::ops::RangeInclusive<u32> = 1..=2;

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Self::Left => BTN_LEFT,
            Self::Right => BTN_RIGHT,
            Self::Middle => BTN_MIDDLE,
        }
    }
}

/// What to do once the pointer is at the gesture's point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Move,
    Click(Button),
    /// Press, move to `to`, release.
    Drag { to: (u32, u32) },
    /// Rest at the point for `rest`, then leave for the layout's centre.
    Flyby { rest: Duration },
    /// Wheel notches (negative: up).
    Scroll { steps: i32 },
}

/// A pointer gesture at a logical point of the output layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gesture {
    pub at: (u32, u32),
    pub action: Action,
}

impl Gesture {
    /// Parses `VERB X Y [ARGS]` (see [`USAGE`]).
    pub fn parse(args: &[String]) -> Result<Self> {
        let usage = || anyhow!("usage: {USAGE}");
        let num = |i: usize| -> Result<u32> { args.get(i).ok_or_else(usage)?.parse().with_context(usage) };
        let verb = args.first().ok_or_else(usage)?.as_str();
        let at = (num(1)?, num(2)?);
        let action = match verb {
            "move" => Action::Move,
            "click" => Action::Click(match args.get(3).map(String::as_str) {
                None | Some("left") => Button::Left,
                Some("right") => Button::Right,
                Some("middle") => Button::Middle,
                Some(_) => return Err(usage()),
            }),
            "drag" => Action::Drag { to: (num(3)?, num(4)?) },
            "flyby" => Action::Flyby { rest: Duration::from_millis(num(3)?.into()) },
            "scroll" => Action::Scroll { steps: args.get(3).ok_or_else(usage)?.parse().with_context(usage)? },
            _ => return Err(usage()),
        };
        Ok(Self { at, action })
    }
}

/// How the gesture is performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Logical size of the output layout (absolute motion is relative to it).
    pub layout: (u32, u32),
    /// Keep the device alive this long after the gesture.
    pub hold: Option<Duration>,
    /// Keep the button down this long at the end of a drag.
    pub drag_hold: Option<Duration>,
}

impl Options {
    /// `MANDARA_LAYOUT`, `VPOINTER_HOLD_MS` and `VPOINTER_DRAG_HOLD_MS`.
    pub fn from_env() -> Self {
        let ms = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok()).map(Duration::from_millis);
        let layout = std::env::var(LAYOUT_ENV).ok().and_then(|s| parse_size(&s)).unwrap_or(DEFAULT_LAYOUT);
        Self { layout, hold: ms(HOLD_ENV), drag_hold: ms(DRAG_HOLD_ENV) }
    }
}

/// `WxH` → `(W, H)`.
pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

struct State;
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<mgr::ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(_: &mut Self, _: &mgr::ZwlrVirtualPointerManagerV1, _: mgr::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<vp::ZwlrVirtualPointerV1, ()> for State {
    fn event(_: &mut Self, _: &vp::ZwlrVirtualPointerV1, _: vp::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

fn now_ms() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u32).unwrap_or(0)
}

/// Performs `gesture` with a fresh virtual pointer on `conn`'s compositor.
pub fn perform(conn: &Connection, gesture: &Gesture, opts: &Options) -> Result<()> {
    let (globals, mut queue) = registry_queue_init::<State>(conn)?;
    let qh = queue.handle();
    let seat: wl_seat::WlSeat = globals.bind(&qh, SEAT_VERSIONS, ())?;
    let manager: mgr::ZwlrVirtualPointerManagerV1 = globals.bind(&qh, MANAGER_VERSIONS, ()).context("compositor lacks zwlr_virtual_pointer_manager_v1")?;
    let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
    let mut state = State;
    let (w, h) = opts.layout;
    let (x, y) = gesture.at;
    let mut flush = |pointer: &vp::ZwlrVirtualPointerV1| -> Result<()> {
        pointer.frame();
        queue.roundtrip(&mut state)?;
        Ok(())
    };

    // Approach in two steps so the target sees enter + motion like a hand
    // would (a fly-by arrives in one step: its time there is all it gets).
    let flyby = matches!(gesture.action, Action::Flyby { .. });
    let approach: &[(i32, i32)] = if flyby { &[(0, 0)] } else { &[(APPROACH_OFFSET, APPROACH_OFFSET), (0, 0)] };
    for &(dx, dy) in approach {
        pointer.motion_absolute(now_ms(), (x as i32 + dx).max(0) as u32, (y as i32 + dy).max(0) as u32, w, h);
        flush(&pointer)?;
        if !flyby {
            std::thread::sleep(APPROACH_PAUSE);
        }
    }
    match gesture.action {
        Action::Move => {}
        Action::Flyby { rest } => {
            // A pointer passing through: rests, then moves on.
            std::thread::sleep(rest);
            pointer.motion_absolute(now_ms(), w / 2, h / 2, w, h);
            flush(&pointer)?;
        }
        Action::Drag { to: (x2, y2) } => {
            pointer.button(now_ms(), BTN_LEFT, wl_pointer::ButtonState::Pressed);
            flush(&pointer)?;
            std::thread::sleep(DRAG_START_PAUSE);
            for step in 1..=DRAG_STEPS {
                let t = step as f32 / DRAG_STEPS as f32;
                let (cx, cy) = (x as f32 + (x2 as f32 - x as f32) * t, y as f32 + (y2 as f32 - y as f32) * t);
                pointer.motion_absolute(now_ms(), cx as u32, cy as u32, w, h);
                flush(&pointer)?;
                std::thread::sleep(FRAME);
            }
            if let Some(hold) = opts.drag_hold {
                std::thread::sleep(hold);
            }
            pointer.button(now_ms(), BTN_LEFT, wl_pointer::ButtonState::Released);
            flush(&pointer)?;
        }
        Action::Click(button) => {
            pointer.button(now_ms(), button.code(), wl_pointer::ButtonState::Pressed);
            flush(&pointer)?;
            std::thread::sleep(CLICK_HOLD);
            pointer.button(now_ms(), button.code(), wl_pointer::ButtonState::Released);
            flush(&pointer)?;
        }
        Action::Scroll { steps } => {
            // A wheel turned steadily: one notch per frame. Wheel motion must
            // carry its notch count (`axis_discrete`, which the compositor
            // forwards as `axis_value120`): clients such as GPUI ignore plain
            // `axis` values from a wheel source, as real mice always send both.
            let (notch, discrete) = if steps < 0 { (-SCROLL_NOTCH, -1) } else { (SCROLL_NOTCH, 1) };
            for _ in 0..steps.unsigned_abs() {
                pointer.axis_source(wl_pointer::AxisSource::Wheel);
                pointer.axis_discrete(now_ms(), wl_pointer::Axis::VerticalScroll, notch, discrete);
                flush(&pointer)?;
                std::thread::sleep(FRAME);
            }
        }
    }
    if let Some(hold) = opts.hold {
        std::thread::sleep(hold);
    }
    pointer.destroy();
    queue.roundtrip(&mut state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_gestures() {
        assert_eq!(Gesture::parse(&args("click 10 20")).unwrap(), Gesture { at: (10, 20), action: Action::Click(Button::Left) });
        assert_eq!(Gesture::parse(&args("click 1 2 right")).unwrap().action, Action::Click(Button::Right));
        assert_eq!(Gesture::parse(&args("drag 1 2 3 4")).unwrap().action, Action::Drag { to: (3, 4) });
        assert_eq!(Gesture::parse(&args("scroll 1 2 -3")).unwrap().action, Action::Scroll { steps: -3 });
        assert_eq!(Gesture::parse(&args("flyby 1 2 250")).unwrap().action, Action::Flyby { rest: Duration::from_millis(250) });
        assert!(Gesture::parse(&args("click 1")).is_err());
        assert!(Gesture::parse(&args("wiggle 1 2")).is_err());
        assert!(Gesture::parse(&args("click 1 2 thumb")).is_err());
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("1707x960"), Some((1707, 960)));
        assert_eq!(parse_size("1707"), None);
    }
}
