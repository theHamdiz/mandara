//! The software wallpaper's GPU transitions: a temporary layer surface
//! (`mandara-wallpaper-transition`, background layer, above the wallpaper,
//! no input, no exclusive zone) that exists only while a transition plays.
//!
//! The renderer thread decides a change should play on the GPU, bakes the
//! outgoing and incoming desktops (wallpaper, clock layer, subject,
//! widgets) from pixels it decoded anyway and posts them
//! ([`Request`]); its own planes keep showing the old desktop, untouched.
//! Here the surface opens on that output and plays the transition from
//! exactly the old desktop to exactly the new one: the images are uploaded
//! on the first frame and their CPU copies dropped. At the end the
//! renderer is told ([`super::Msg::OverlayDone`]); it puts the new desktop
//! on its planes in one commit and answers ([`super::Event::OverlaySwapped`]),
//! and the surface closes, its textures and swapchain with it.
//!
//! When the surface can't open (no display for the output, no GPU), the
//! renderer is told at once and plays a fade instead
//! ([`super::Msg::OverlayFailed`]); after [`FAILURES_BEFORE_OFF`] failures
//! in a row it stops asking. `MANDARA_WALLPAPER_OVERLAY=0` turns it off.

use std::collections::HashMap;
use std::time::Duration;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{AnyWindowHandle, App, AppContext as _, Context, Global, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px, size};
use mandara_core::{LayerSurfaceSpec, SurfaceRole};

use crate::wallpaper_shader::{Frames, Look, Player, Shared, State, TransitionElement};

/// `MANDARA_WALLPAPER_OVERLAY=0` keeps transitions on the software planes.
pub const OVERLAY_ENV: &str = "MANDARA_WALLPAPER_OVERLAY";
/// Failed overlays in a row before the renderer stops asking for them.
pub const FAILURES_BEFORE_OFF: u32 = 3;
/// After the renderer swapped, the surface stays this long (a frame or
/// two) so the compositor shows the new planes before it goes.
const CLOSE_DELAY: Duration = Duration::from_millis(32);
/// A surface still open this long after its last frame closes anyway.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Whether the environment allows the overlay.
pub fn allowed_by_env() -> bool {
    std::env::var(OVERLAY_ENV).map(|v| !matches!(v.trim(), "0" | "false" | "off" | "no")).unwrap_or(true)
}

/// A transition to play on one output.
pub struct Request {
    /// The renderer's output id and the change's serial.
    pub out: u32,
    pub serial: u64,
    /// Connector name, and logical size as the renderer measured it.
    pub output: String,
    pub logical: (u32, u32),
    pub frames: Frames,
    pub look: Look,
    pub duration: Duration,
}

/// What the renderer should do about a decision of the overlay manager.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The last frame is on screen: put the new desktop on the planes.
    Done,
    /// No surface: play the change without it.
    Failed,
}

/// Why a surface could not open, if it can't: decided before any window
/// work, so the renderer can fall back at once.
pub fn refusal(logical: (u32, u32), display: bool) -> Option<&'static str> {
    if logical.0 == 0 || logical.1 == 0 {
        // A zero dimension trips GPUI's viewport (never request one).
        Some("output size unknown")
    } else if !display {
        Some("no display for the output")
    } else {
        None
    }
}

struct Shown {
    serial: u64,
    window: AnyWindowHandle,
}

#[derive(Default)]
struct Overlays(HashMap<u32, Shown>);
impl Global for Overlays {}

/// Opens the surface for `req`, replacing one still open on that output.
/// `reply` reaches the renderer.
pub fn open(req: Request, reply: impl Fn(u32, u64, Reply, &mut App) + 'static, cx: &mut App) {
    let (out, serial) = (req.out, req.serial);
    close(out, None, cx);
    let display = crate::outputs::display_for(&req.output, cx);
    if let Some(why) = refusal(req.logical, display.is_some()) {
        tracing::warn!("wallpaper transition on {}: {why}; fading instead", req.output);
        reply(out, serial, Reply::Failed, cx);
        return;
    }
    let mut spec = LayerSurfaceSpec::background(size(px(req.logical.0 as f32), px(req.logical.1 as f32)), display);
    spec.role = SurfaceRole::WallpaperTransition;
    spec.layer = Layer::Background;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    spec.keyboard = KeyboardInteractivity::None;
    let player = std::rc::Rc::new(std::cell::RefCell::new(Player::new(req.look, req.frames, req.duration)));
    let reply = std::rc::Rc::new(reply);
    let opened = cx.open_window(spec.window_options(), {
        let reply = reply.clone();
        move |_, cx| cx.new(|_| OverlayView { player, out, serial, input_cleared: false, reported: false, reply: Box::new(move |r, cx| reply(out, serial, r, cx)) })
    });
    match opened {
        Ok(handle) => {
            cx.default_global::<Overlays>().0.insert(out, Shown { serial, window: handle.into() });
        }
        Err(err) => {
            tracing::warn!("wallpaper transition surface on {}: {err:#}; fading instead", req.output);
            reply(out, serial, Reply::Failed, cx);
        }
    }
}

/// The renderer put the new desktop on its planes: the surface goes a
/// moment later.
pub fn swapped(out: u32, serial: u64, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(CLOSE_DELAY).await;
        cx.update(|cx| close(out, Some(serial), cx));
    })
    .detach();
}

/// Closes the surface of output `out` (only that change's, with `serial`),
/// freeing its textures.
pub fn close(out: u32, serial: Option<u64>, cx: &mut App) {
    let Some(overlays) = cx.try_global::<Overlays>() else { return };
    if !overlays.0.get(&out).is_some_and(|s| serial.is_none_or(|n| n == s.serial)) {
        return;
    }
    let Some(shown) = cx.global_mut::<Overlays>().0.remove(&out) else { return };
    let _ = shown.window.update(cx, |view, window, cx| {
        if let Ok(view) = view.downcast::<OverlayView>() {
            view.update(cx, |v, _| v.player.borrow_mut().release(window));
        }
        window.remove_window();
    });
    if cx.global::<Overlays>().0.is_empty() {
        cx.remove_global::<Overlays>();
    }
}

/// Outputs with an open transition surface (for tests and diagnostics).
pub fn open_count(cx: &App) -> usize {
    cx.try_global::<Overlays>().map_or(0, |o| o.0.len())
}

struct OverlayView {
    player: Shared,
    out: u32,
    serial: u64,
    input_cleared: bool,
    reported: bool,
    reply: Box<dyn Fn(Reply, &mut App)>,
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _frame = crate::plugins::diagnostics::frame(cx.entity_id().as_u64());
        if !self.input_cleared {
            // Pointer input goes to the desktop underneath.
            window.set_input_region(Some(&[]));
            self.input_cleared = true;
        }
        match self.player.borrow().state() {
            State::Waiting | State::Playing => window.request_animation_frame(),
            State::Done | State::Lost if !self.reported => {
                self.reported = true;
                (self.reply)(Reply::Done, cx);
                let (out, serial) = (self.out, self.serial);
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(CLOSE_TIMEOUT).await;
                    cx.update(|cx| close(out, Some(serial), cx));
                })
                .detach();
            }
            _ => {}
        }
        div().size_full().child(TransitionElement(self.player.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_open_only_where_they_can() {
        assert_eq!(refusal((2560, 1440), true), None);
        assert!(refusal((0, 1440), true).is_some(), "never a zero width");
        assert!(refusal((2560, 0), true).is_some());
        assert!(refusal((2560, 1440), false).is_some(), "an output GPUI doesn't know");
    }

    #[test]
    fn the_environment_can_turn_it_off() {
        // Read-only check of the parser: unset means on.
        if std::env::var(OVERLAY_ENV).is_err() {
            assert!(allowed_by_env());
        }
    }
}
