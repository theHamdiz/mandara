//! The snipping tool: a full-screen overlay showing a frozen frame of the
//! output (captured natively via wlr-screencopy) where you drag a region,
//! pick a window, or take the whole screen, then copy, save or copy the
//! text in it (OCR). Each action shows its progress and a ✓ on its button.
//!
//! Esc (or the pause button) leaves snipping without closing the tool: the
//! frozen frame goes away and a compact toolbar stays, so a panel can be
//! opened first and captured with "Capture now" or "Capture in N s"
//! (`session` runs the stages, `flow` holds their rules, `dock` draws the
//! compact surfaces).

mod dock;
mod flow;
mod session;

pub(crate) use session::{Job, begin};

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, ClickEvent, Context, FocusHandle, FontWeight, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, ParentElement, Pixels, Render, RenderImage, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_services::screencopy::Capture;
use mandara_config::workflow::{ScreenshotSettings, ToolbarPosition};
use mandara_services::screenshot::{Annotation, AnnotationKind, annotate, arrow_barbs, deliver_with, describe};

use mandara_i18n::{t, tf};

use self::flow::Input;
use crate::dismiss::KeepInside as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{metrics, text, tokens};

/// The selection overlay fades out before recording starts, so it is not captured.
const RECORD_START_DELAY: std::time::Duration = std::time::Duration::from_millis(400);
/// The ✓ stays this long before the overlay fades out (part of the confirmation animation).
const DONE_HOLD: std::time::Duration = std::time::Duration::from_millis(650);
/// Repaint period of the download percentage.
const PROGRESS_TICK: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SnipMode {
    Region,
    Window,
    Screen,
}

/// What a drag inside the selection does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Tool {
    Select,
    Draw(AnnotationKind),
}

/// Annotation colors: red, amber, accent-free blue, white.
const INKS: [[u8; 4]; 4] = [[255, 69, 58, 255], [255, 204, 0, 255], [10, 132, 255, 255], [255, 255, 255, 255]];
/// Stroke width in logical pixels.
const INK_WIDTH: f32 = 3.5;
/// Highest scrim over the frozen screen.
const DIM_MAX: f32 = 0.8;
/// Selection outline width bounds, logical px.
const BORDER_RANGE: (f32, f32) = (1.0, 6.0);
/// Corner radius of the selection outline (before the radius scale).
const SELECTION_RADIUS_PX: f32 = 6.0;
/// Region-mode corner handle diameter.
const HANDLE_PX: f32 = 10.0;
/// Size badge: height, side padding, gap to the selection, and how much
/// room above the selection it needs before it moves below.
const BADGE_H_PX: f32 = 24.0;
const BADGE_PAD_X_PX: f32 = tokens::SM + metrics::NUDGE;
const BADGE_GAP_PX: f32 = tokens::SM;
const BADGE_ROOM_PX: f32 = BADGE_H_PX + tokens::MD;

/// Capture toolbar geometry at `toolbar_scale` 1 (Studio → Screenshot →
/// Toolbar size scales all of it; corners also follow the radius scale).
mod bar {
    use crate::kit::{metrics, tokens};
    /// Toolbar height, side padding, corner radius and gap between groups.
    pub const H: f32 = 50.0;
    pub const PAD: f32 = tokens::SM;
    pub const RADIUS: f32 = 18.0;
    pub const GAP: f32 = tokens::XS;
    /// Distance from the screen edge.
    pub const MARGIN: f32 = tokens::XXL;
    /// Buttons: height (and icon-button size), corner radius, icon size.
    pub const BUTTON: f32 = 34.0;
    pub const BUTTON_RADIUS: f32 = tokens::MD;
    pub const ICON: f32 = tokens::LG;
    /// Side padding of mode, action and record buttons; icon–label gaps.
    pub const MODE_PAD: f32 = tokens::MD;
    pub const ACTION_PAD: f32 = tokens::MD + metrics::NUDGE;
    pub const GO_PAD: f32 = tokens::LG;
    pub const MODE_GAP: f32 = 6.0;
    pub const ACTION_GAP: f32 = 7.0;
    pub const GO_GAP: f32 = tokens::SM;
    /// Group separators: hairline length and side margin.
    pub const SEPARATOR_H: f32 = 22.0;
    pub const SEPARATOR_MX: f32 = 6.0;
    /// Gap inside the annotation-tools and action groups.
    pub const TOOLS_GAP: f32 = metrics::NUDGE;
    pub const GROUP_GAP: f32 = 6.0;
    /// Ink swatch diameter and the swatch row's side padding.
    pub const INK: f32 = tokens::XL;
    pub const INK_PAD: f32 = tokens::SM;
    /// Spacer before the record / close buttons.
    pub const TAIL: f32 = 6.0;
    /// Recording dot diameter.
    pub const REC_DOT: f32 = 9.0;
    /// Drop shadow offset / blur.
    pub const SHADOW_Y: f32 = tokens::XS;
    pub const SHADOW_BLUR: f32 = 14.0;
    /// Hint strip: height, side padding, gap to the toolbar, widest.
    pub const HINT_H: f32 = 26.0;
    pub const HINT_PAD: f32 = tokens::MD;
    pub const HINT_GAP: f32 = 8.0;
    pub const HINT_W: f32 = 720.0;
}

/// The last captured region (output, rect), for "remember region".
static LAST_REGION: std::sync::Mutex<Option<(String, Rect)>> = std::sync::Mutex::new(None);

/// What a snip opens with selected.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Preselect {
    None,
    /// The focused window (editing after a window capture).
    FocusedWindow,
    /// The whole output (editing after a screen capture).
    Full,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn from_points(a: (f32, f32), b: (f32, f32)) -> Self {
        Rect { x: a.0.min(b.0), y: a.1.min(b.1), w: (a.0 - b.0).abs(), h: (a.1 - b.1).abs() }
    }

    fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 <= self.x + self.w && p.1 >= self.y && p.1 <= self.y + self.h
    }
}

/// What to do with the chosen area.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Action {
    /// Enter / click: copy and/or save per the plugin settings.
    Capture,
    Copy,
    Save,
    /// Recognize the text in the area (OCR) and copy it.
    Text,
}

impl Action {
    fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Action::Capture => (t("Capture"), t("Capturing…"), t("Done")),
            Action::Copy => (t("Copy"), t("Copying…"), t("Copied")),
            Action::Save => (t("Save"), t("Saving…"), t("Saved")),
            Action::Text => (t("Copy text"), t("Reading…"), t("Text copied")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Working,
    Done,
    Failed(String),
}

pub struct SnipView {
    capture: Arc<Capture>,
    frozen: Arc<RenderImage>,
    /// Window rects in this output's logical coordinates.
    windows: Vec<(String, Rect)>,
    mode: SnipMode,
    drag_start: Option<(f32, f32)>,
    selection: Option<Rect>,
    pointer: (f32, f32),
    tool: Tool,
    ink: [u8; 4],
    /// Annotations in overlay (logical) coordinates.
    shapes: Vec<Annotation>,
    draft: Option<Annotation>,
    /// Record a video of the chosen area instead of taking a screenshot.
    record: bool,
    save: bool,
    copy: bool,
    /// The screenshot plugin's settings at opening.
    shot: ScreenshotSettings,
    /// The running / finished action, for button feedback.
    busy: Option<(Action, Phase)>,
    /// Model download progress (per mille) while OCR models are fetched.
    fetching: Arc<std::sync::atomic::AtomicU32>,
    focus: FocusHandle,
    fade: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
}

/// Converts RGBA to the BGRA layout GPUI's renderer expects.
fn frozen_image(capture: &Capture) -> Option<Arc<RenderImage>> {
    let mut bgra = capture.rgba.clone();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(capture.width, capture.height, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

/// Tells the user how a capture went (saved path / copied, or the error).
pub fn report(result: Result<String, String>, cx: &mut App) {
    let (summary, body) = match &result {
        Ok(text) => (t("Screenshot taken").to_string(), text.clone()),
        Err(err) => (t("Screenshot failed").to_string(), err.clone()),
    };
    match &result {
        Ok(t) => tracing::info!("{t}"),
        Err(e) => tracing::error!("screenshot: {e}"),
    }
    let shot = ScreenshotSettings::from_plugins(&crate::state::config(cx).read(cx).get().plugins);
    if result.is_ok() && shot.sound {
        crate::alerts::play_sound(mandara_services::sounds::SoundEvent::Screenshot, cx);
    }
    // Successes notify only when asked to; failures always do.
    if result.is_ok() && !shot.notify {
        return;
    }
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Screenshot").into(), icon: "camera".into(), summary, body });
}

impl SnipView {
    fn new(capture: Capture, frozen: Arc<RenderImage>, mode: SnipMode, record: bool, preselect: Preselect, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (ox, oy) = (capture.logical.0 as f32, capture.logical.1 as f32);
        let hub = crate::hub::hub(cx).read(cx);
        let mut windows: Vec<(String, Rect)> = hub
            .clients
            .iter()
            .filter_map(|c| {
                let (x, y, w, h) = c.geometry?;
                Some((c.app_id.clone(), Rect { x: x as f32 - ox, y: y as f32 - oy, w: w as f32, h: h as f32 }))
            })
            .collect();
        // Focused window first so overlapping picks favour it.
        if let Some(f) = hub.focused.as_ref().and_then(|f| f.geometry) {
            let r = Rect { x: f.0 as f32 - ox, y: f.1 as f32 - oy, w: f.2 as f32, h: f.3 as f32 };
            windows.retain(|(_, w)| *w != r);
            windows.insert(0, (hub.focused.as_ref().map(|c| c.app_id.clone()).unwrap_or_default(), r));
        }
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let shot = ScreenshotSettings::from_plugins(&cfg.plugins);
        let full = Rect { x: 0.0, y: 0.0, w: capture.logical.2 as f32, h: capture.logical.3 as f32 };
        let selection = match preselect {
            Preselect::FocusedWindow => windows.first().map(|(_, r)| *r).filter(|_| hub.focused.is_some()),
            Preselect::Full => Some(full),
            Preselect::None if mode == SnipMode::Region && shot.remember_region && !record => {
                LAST_REGION.lock().ok().and_then(|l| l.clone()).filter(|(o, _)| *o == capture.output).map(|(_, r)| r)
            }
            Preselect::None => None,
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            frozen,
            capture: Arc::new(capture),
            windows,
            mode,
            drag_start: None,
            selection,
            pointer: (0.0, 0.0),
            tool: Tool::Select,
            ink: INKS[0],
            shapes: Vec::new(),
            draft: None,
            record,
            save: shot.save,
            copy: shot.copy,
            shot,
            busy: None,
            fetching: Arc::new(std::sync::atomic::AtomicU32::new(u32::MAX)),
            focus,
            fade: SpringValue::new(0.0, SpringConfig::new(300.0, 30.0, 1.0)),
            clock: FrameClock::new(),
            closing: false,
        }
    }

    fn window_at(&self, p: (f32, f32)) -> Option<Rect> {
        self.windows.iter().find(|(_, r)| r.contains(p)).map(|(_, r)| *r)
    }

    fn full(&self) -> Rect {
        Rect { x: 0.0, y: 0.0, w: self.capture.logical.2 as f32, h: self.capture.logical.3 as f32 }
    }

    /// The rect a click / Enter would capture right now.
    fn candidate(&self) -> Option<Rect> {
        match self.mode {
            SnipMode::Region => self.selection.filter(|r| r.w >= 2.0 && r.h >= 2.0),
            SnipMode::Window => self.selection.or_else(|| self.window_at(self.pointer)),
            SnipMode::Screen => Some(self.full()),
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        self.act(Action::Capture, cx);
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        tracing::debug!(target: "mandara::snip", mode = ?self.mode, ?action, candidate = ?self.candidate(), closing = self.closing, "act");
        if matches!(self.busy, Some((_, Phase::Working | Phase::Done))) {
            return;
        }
        let Some(r) = self.candidate() else { return };
        if self.record {
            if self.closing {
                return;
            }
            let region = (self.mode != SnipMode::Screen).then(|| (r.x.round() as i32, r.y.round() as i32, r.w.round() as i32, r.h.round() as i32));
            // Start once the overlay has faded so it isn't in the video.
            if let Err(err) = crate::recorder::start(self.capture.output.clone(), region, RECORD_START_DELAY, cx) {
                report(Err(err), cx);
            }
            self.close(cx);
            return;
        }
        if action == Action::Text {
            if self.closing {
                return;
            }
            self.read_text(r, cx);
            return;
        }
        let (save, copy) = match action {
            Action::Capture => (self.save, self.copy),
            Action::Copy => (false, true),
            Action::Save => (true, false),
            Action::Text => unreachable!(),
        };
        if self.closing || !(save || copy) {
            return;
        }
        if self.mode == SnipMode::Region
            && let Ok(mut last) = LAST_REGION.lock()
        {
            *last = Some((self.capture.output.clone(), r));
        }
        let capture = self.capture.clone();
        let shot = self.shot.clone();
        // Annotations move into the cropped image's pixel space.
        let scale = capture.scale();
        let shapes: Vec<Annotation> = if self.mode == SnipMode::Region {
            self.shapes
                .iter()
                .map(|a| Annotation {
                    from: ((a.from.0 - r.x) * scale, (a.from.1 - r.y) * scale),
                    to: ((a.to.0 - r.x) * scale, (a.to.1 - r.y) * scale),
                    width: a.width * scale,
                    ..*a
                })
                .collect()
        } else {
            Vec::new()
        };
        // Encode / save / copy off the UI thread; close the overlay at once.
        let shutter = crate::alerts::deferred_sound(mandara_services::sounds::SoundEvent::Screenshot, cx);
        let task = cx.background_executor().spawn(async move {
            mandara_plugins::guard("screenshot encode", || {
                let mut image = capture.crop_logical(r.x, r.y, r.w, r.h);
                annotate(&mut image, &shapes);
                let delivered = deliver_with(&image, &shot, save, copy).map(|res| describe(&res));
                if delivered.is_ok() {
                    shutter();
                }
                delivered
            })
            .unwrap_or_else(|| Err(t("internal error while encoding (logged)").into()))
        });
        self.busy = Some((action, Phase::Working));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let ok = result.is_ok();
            let _ = this.update(cx, |this, cx| {
                this.busy = Some((action, if ok { Phase::Done } else { Phase::Failed(result.clone().err().unwrap_or_default()) }));
                cx.notify();
            });
            // Copy shows its ✓ on the button; saves also get a notification
            // with the path. Failures always notify.
            if !ok || save {
                cx.update(|cx| report(result, cx));
            } else {
                cx.update(|cx| {
                    if ScreenshotSettings::from_plugins(&crate::state::config(cx).read(cx).get().plugins).sound {
                        crate::alerts::play_sound(mandara_services::sounds::SoundEvent::Screenshot, cx);
                    }
                });
            }
            if ok {
                // Let the ✓ register before fading out.
                cx.background_executor().timer(DONE_HOLD).await;
                let _ = this.update(cx, |this, cx| this.close(cx));
            }
        })
        .detach();
    }

    /// OCR of the selection (without annotations) into the clipboard.
    /// Missing models are downloaded first, with progress on the button.
    fn read_text(&mut self, r: Rect, cx: &mut Context<Self>) {
        let capture = self.capture.clone();
        let script = mandara_services::ocr::Script::parse(&crate::state::config(cx).read(cx).snapshot().plugins.setting(crate::plugins::screenshot::ID, "ocr_language", String::from("auto")));
        let fetching = self.fetching.clone();
        let task = cx.background_executor().spawn(async move {
            mandara_plugins::guard("screenshot text", || -> Result<String, String> {
                use std::sync::atomic::Ordering;
                for model in mandara_services::ocr::missing_models(script) {
                    let f = fetching.clone();
                    model.download(&move |done, total| f.store((done * 1000 / total.max(1)) as u32, Ordering::Relaxed))?;
                }
                fetching.store(u32::MAX, Ordering::Relaxed);
                let image = capture.crop_logical(r.x, r.y, r.w, r.h);
                let text = mandara_services::ocr::recognize(&image, script)?;
                mandara_services::clipboard::serve(text.clone().into_bytes(), None)?;
                Ok(text)
            })
            .unwrap_or_else(|| Err(t("internal error while reading text (logged)").into()))
        });
        self.busy = Some((Action::Text, Phase::Working));
        cx.notify();
        // Repaint while downloading so the percentage moves.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_TICK).await;
                let working = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.busy, Some((Action::Text, Phase::Working)))
                });
                if !matches!(working, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let ok = result.is_ok();
            if let Ok(text) = &result {
                tracing::info!("copied {} characters of recognized text", text.chars().count());
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = Some((Action::Text, if ok { Phase::Done } else { Phase::Failed(result.clone().err().unwrap_or_default()) }));
                cx.notify();
            });
            if ok {
                cx.background_executor().timer(DONE_HOLD).await;
                let _ = this.update(cx, |this, cx| this.close(cx));
            }
        })
        .detach();
    }

    /// Esc / right-click / the pause button: leave snipping, keep the
    /// tool as the idle toolbar (the session fades this overlay out).
    fn pause(&mut self, input: Input, cx: &mut Context<Self>) {
        tracing::debug!(target: "mandara::snip", ?input, "pause");
        if !self.closing {
            session::pause(self.mode, input, cx);
        }
    }

    /// "Capture in N s": the overlay goes, the countdown runs, and a fresh
    /// frame comes back in the same mode.
    fn delay(&mut self, cx: &mut Context<Self>) {
        if !self.closing {
            session::set_mode(self.mode, cx);
            session::capture_in(cx);
        }
    }

    /// Moves the toolbar to the other screen edge (saved to the settings).
    fn flip_toolbar(&mut self, cx: &mut Context<Self>) {
        self.shot.toolbar_position = session::flip_position(cx);
        cx.notify();
    }

    /// Closes the tool (close button, a finished capture).
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        cx.notify();
        session::send(Input::Close, cx);
    }
}

fn pos(e: gpui::Point<Pixels>) -> (f32, f32) {
    (f32::from(e.x), f32::from(e.y))
}

impl Render for SnipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        self.fade.set_target(if self.closing { 0.0 } else { 1.0 });
        let dt = self.clock.tick(Instant::now());
        if self.fade.step(dt) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
            if self.closing {
                window.remove_window();
            }
        }
        let fade = self.fade.value().clamp(0.0, 1.0);
        let dim = gpui::black().opacity(self.shot.dim.clamp(0.0, DIM_MAX) * fade);
        let border = self.shot.border.clamp(BORDER_RANGE.0, BORDER_RANGE.1);
        let candidate = self.candidate();

        // Dim everything outside the candidate rect with four bands.
        let bands = match candidate {
            Some(r) => vec![
                (0.0, 0.0, sw, r.y),
                (0.0, r.y + r.h, sw, sh - r.y - r.h),
                (0.0, r.y, r.x, r.h),
                (r.x + r.w, r.y, sw - r.x - r.w, r.h),
            ],
            None => vec![(0.0, 0.0, sw, sh)],
        };
        let mut root = div()
            .id("snip")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .cursor(gpui::CursorStyle::Crosshair)
            .font_family(f.ui.clone())
            .text_size(px(text::BODY))
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                // Fading out (paused, counting down, done): keys are no longer ours.
                _ if this.closing => {}
                "escape" => this.pause(Input::Escape, cx),
                "enter" | "space" => this.confirm(cx),
                "c" if e.keystroke.modifiers.control => this.act(Action::Copy, cx),
                "s" if e.keystroke.modifiers.control => this.act(Action::Save, cx),
                "t" if e.keystroke.modifiers.control => this.act(Action::Text, cx),
                "t" => this.delay(cx),
                "m" => this.flip_toolbar(cx),
                "z" if e.keystroke.modifiers.control => {
                    this.shapes.pop();
                    cx.notify();
                }
                "r" => this.mode = SnipMode::Region,
                "w" => this.mode = SnipMode::Window,
                "s" => this.mode = SnipMode::Screen,
                "v" => this.tool = Tool::Select,
                "a" => this.tool = Tool::Draw(AnnotationKind::Arrow),
                "b" => this.tool = Tool::Draw(AnnotationKind::Rect),
                "c" | "o" => this.tool = Tool::Draw(AnnotationKind::Ellipse),
                _ => {}
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                if this.closing {
                    return;
                }
                let p = pos(e.position);
                match this.mode {
                    // Drawing tool + a selection: start a shape inside it.
                    SnipMode::Region if matches!(this.tool, Tool::Draw(_)) && this.selection.is_some_and(|r| r.w >= 2.0 && r.h >= 2.0) => {
                        let Tool::Draw(kind) = this.tool else { return };
                        let r = this.selection.unwrap_or(Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
                        let p = (p.0.clamp(r.x, r.x + r.w), p.1.clamp(r.y, r.y + r.h));
                        this.draft = Some(Annotation { kind, from: p, to: p, color: this.ink, width: INK_WIDTH });
                    }
                    SnipMode::Region => {
                        this.drag_start = Some(p);
                        this.selection = Some(Rect { x: p.0, y: p.1, w: 0.0, h: 0.0 });
                        // A new region starts a clean canvas.
                        this.shapes.clear();
                    }
                    // First click picks a window (then choose an action);
                    // clicking the picked window again captures it.
                    SnipMode::Window => {
                        if let Some(r) = this.window_at(p) {
                            if this.selection == Some(r) {
                                this.confirm(cx);
                            } else {
                                this.selection = Some(r);
                            }
                        }
                    }
                    SnipMode::Screen => this.confirm(cx),
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                this.pointer = pos(e.position);
                if let (Some(draft), Some(r)) = (this.draft.as_mut(), this.selection) {
                    draft.to = (this.pointer.0.clamp(r.x, r.x + r.w), this.pointer.1.clamp(r.y, r.y + r.h));
                } else if let (SnipMode::Region, Some(start), true) = (this.mode, this.drag_start, e.pressed_button == Some(MouseButton::Left)) {
                    this.selection = Some(Rect::from_points(start, this.pointer));
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                let dragged = this.drag_start.take().is_some();
                if let Some(d) = this.draft.take() {
                    // Ignore accidental clicks.
                    if (d.to.0 - d.from.0).abs() + (d.to.1 - d.from.1).abs() > 6.0 {
                        this.shapes.push(d);
                    }
                } else if dragged && this.mode == SnipMode::Region && this.shot.capture_on_release && this.candidate().is_some() {
                    this.confirm(cx);
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.pause(Input::Escape, cx)))
            .child(img(ImageSource::Render(self.frozen.clone())).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Fill));
        for (x, y, w, h) in bands {
            if w > 0.0 && h > 0.0 {
                root = root.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(dim));
            }
        }
        if let Some(r) = candidate {
            let accent = palette.primary;
            // Selection outline (outside the rect so it never eats pixels).
            root = root.child(
                div()
                    .absolute()
                    .left(px(r.x - border))
                    .top(px(r.y - border))
                    .w(px(r.w + border * 2.0))
                    .h(px(r.h + border * 2.0))
                    .rounded(px(crate::style::radius(SELECTION_RADIUS_PX)))
                    .border(px(border))
                    .border_color(accent.with_alpha(0.95 * fade).hsla()),
            );
            // Corner handles for region mode.
            if self.mode == SnipMode::Region {
                for (hx, hy) in [(r.x, r.y), (r.x + r.w, r.y), (r.x, r.y + r.h), (r.x + r.w, r.y + r.h)] {
                    root = root.child(
                        div().absolute().left(px(hx - HANDLE_PX / 2.0)).top(px(hy - HANDLE_PX / 2.0)).size(px(HANDLE_PX)).rounded_full().bg(accent.hsla()).border_2().border_color(gpui::white()),
                    );
                }
            }
            // Size label.
            let show_size = self.shot.show_size;
            let scale = self.capture.scale();
            let label = format!("{} × {}", (r.w * scale).round() as u32, (r.h * scale).round() as u32);
            let ly = if r.y > BADGE_ROOM_PX + BADGE_GAP_PX / 2.0 { r.y - BADGE_H_PX - BADGE_GAP_PX } else { r.y + r.h + BADGE_GAP_PX };
            root = root.when(show_size, |root| root.child(
                div()
                    .absolute()
                    .left(px(r.x))
                    .top(px(ly))
                    .px(px(BADGE_PAD_X_PX))
                    .h(px(BADGE_H_PX))
                    .rounded_full()
                    .bg(palette.surface_container_high.with_alpha(0.92).hsla())
                    .flex()
                    .items_center()
                    .font_features(tabular_nums())
                    .text_size(px(text::BODY_S))
                    .child(label),
            ));
        }

        // Annotations (and the one being drawn), painted live.
        let mut inked: Vec<Annotation> = self.shapes.clone();
        inked.extend(self.draft);
        if !inked.is_empty() {
            root = root.child(
                gpui::canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        for a in &inked {
                            let [r, g, b, al] = a.color;
                            let color = gpui::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: al as f32 / 255.0 };
                            let mut pb = gpui::PathBuilder::stroke(px(a.width));
                            let pt = |p: (f32, f32)| point(px(p.0), px(p.1));
                            let (x0, y0, x1, y1) = (a.from.0.min(a.to.0), a.from.1.min(a.to.1), a.from.0.max(a.to.0), a.from.1.max(a.to.1));
                            match a.kind {
                                AnnotationKind::Arrow => {
                                    pb.move_to(pt(a.from));
                                    pb.line_to(pt(a.to));
                                    let [l, rr] = arrow_barbs(a.from, a.to, a.width);
                                    pb.move_to(pt(l));
                                    pb.line_to(pt(a.to));
                                    pb.line_to(pt(rr));
                                }
                                AnnotationKind::Rect => {
                                    pb.move_to(pt((x0, y0)));
                                    pb.line_to(pt((x1, y0)));
                                    pb.line_to(pt((x1, y1)));
                                    pb.line_to(pt((x0, y1)));
                                    pb.close();
                                }
                                AnnotationKind::Ellipse => {
                                    let (cx_, cy_, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
                                    for i in 0..=64 {
                                        let t = i as f32 / 64.0 * std::f32::consts::TAU;
                                        let q = pt((cx_ + rx * t.cos(), cy_ + ry * t.sin()));
                                        if i == 0 { pb.move_to(q) } else { pb.line_to(q) }
                                    }
                                }
                            }
                            if let Ok(path) = pb.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );
        }

        // Toolbar: modes, outputs, actions.
        let ts = self.shot.toolbar_scale();
        let z = move |v: f32| px(v * ts);
        let corner = move |v: f32| px(crate::style::radius(v) * ts);
        let mode_btn = |id: &'static str, m: SnipMode, cx: &mut Context<Self>| {
            dock::mode_chip(id, m, self.mode == m, &palette, ts).keep_inside().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.mode = m;
                this.selection = None;
                cx.notify();
            }))
        };
        let can_confirm = candidate.is_some() && (self.record || self.copy || self.save);
        // Action buttons: icon + label, with live progress / ✓ feedback.
        let busy = self.busy.clone();
        let fetching = self.fetching.clone();
        let action_btn = |id: &'static str, ic: &'static str, action: Action, primary: bool, cx: &mut Context<Self>| {
            let (idle, working, done) = action.labels();
            let phase = busy.as_ref().filter(|(a, _)| *a == action).map(|(_, p)| p.clone());
            let enabled = candidate.is_some() && !matches!(busy, Some((_, Phase::Working | Phase::Done)));
            let (bg, fg) = match (&phase, primary) {
                (Some(Phase::Done), _) => (palette.success_emerald, palette.on_primary),
                (Some(Phase::Failed(_)), _) => (palette.urgent_coral, palette.on_primary),
                (_, true) if candidate.is_some() => (palette.primary, palette.on_primary),
                _ => (palette.on_surface.with_alpha(0.07), palette.on_surface),
            };
            let fetch = fetching.load(std::sync::atomic::Ordering::Relaxed);
            let fetch_label: gpui::SharedString = if action == Action::Text && fetch != u32::MAX { tf("Downloading models {percent}%", &[("percent", &(fetch / 10))]).into() } else { working.into() };
            let (glyph, label): (&'static str, gpui::SharedString) = match &phase {
                Some(Phase::Working) => (ic, fetch_label),
                Some(Phase::Done) => ("check", done.into()),
                Some(Phase::Failed(_)) => ("close", t("Failed").into()),
                None => (ic, idle.into()),
            };
            div()
                .id(id)
                .h(z(bar::BUTTON))
                .px(z(bar::ACTION_PAD))
                .rounded(corner(bar::BUTTON_RADIUS))
                .flex()
                .items_center()
                .gap(z(bar::ACTION_GAP))
                .font_weight(FontWeight::MEDIUM)
                .bg(bg.hsla())
                .text_color(fg.hsla())
                .when(!enabled, |el| el.opacity(if phase.is_some() { 1.0 } else { 0.45 }))
                .when(matches!(phase, Some(Phase::Working)), |el| el.opacity(0.75))
                .when(enabled, |el| el.cursor_pointer().hover(|s| s.opacity(0.88)).active(|s| s.opacity(0.7)))
                .child(icon(glyph, bar::ICON * ts, fg))
                .child(label)
                .keep_inside()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.act(action, cx)))
        };
        let error = match &self.busy {
            Some((_, Phase::Failed(e))) => Some(e.clone()),
            _ => None,
        };
        // Annotation tools (region mode): select / arrow / box / circle,
        // ink colors, undo.
        let tool_btn = |id: &'static str, ic: &'static str, t: Tool, cx: &mut Context<Self>| {
            let active = self.tool == t;
            div()
                .id(id)
                .size(z(bar::BUTTON))
                .rounded(corner(bar::BUTTON_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(active, |el| el.bg(palette.primary.with_alpha(0.18).hsla()))
                .when(!active, |el| el.hover(|s| s.bg(palette.on_surface.with_alpha(0.08).hsla())))
                .child(icon(ic, bar::ICON * ts, if active { palette.primary } else { palette.on_surface }))
                .keep_inside()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.tool = t;
                    cx.notify();
                }))
        };
        let annotate_tools = (self.mode == SnipMode::Region && !self.record).then(|| {
            let inks = INKS.iter().enumerate().map(|(i, ink)| {
                let ink = *ink;
                let active = self.ink == ink;
                div()
                    .id(("ink", i))
                    .size(z(bar::INK))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(gpui::Rgba { r: ink[0] as f32 / 255.0, g: ink[1] as f32 / 255.0, b: ink[2] as f32 / 255.0, a: 1.0 })
                    .border_2()
                    .border_color(if active { palette.on_surface.hsla() } else { palette.outline.with_alpha(0.35).hsla() })
                    .keep_inside()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.ink = ink;
                        cx.notify();
                    }))
            }).collect::<Vec<_>>();
            div()
                .flex()
                .items_center()
                .gap(z(bar::TOOLS_GAP))
                .child(div().w(px(1.0)).h(z(bar::SEPARATOR_H)).mx(z(bar::SEPARATOR_MX)).bg(palette.outline.with_alpha(0.4).hsla()))
                .child(tool_btn("tool-select", "pointer", Tool::Select, cx))
                .child(tool_btn("tool-arrow", "arrow", Tool::Draw(AnnotationKind::Arrow), cx))
                .child(tool_btn("tool-rect", "maximize", Tool::Draw(AnnotationKind::Rect), cx))
                .child(tool_btn("tool-ellipse", "circle", Tool::Draw(AnnotationKind::Ellipse), cx))
                .child(div().flex().items_center().gap(z(bar::GROUP_GAP)).px(z(bar::INK_PAD)).children(inks))
                .child(
                    div()
                        .id("tool-undo")
                        .size(z(bar::BUTTON))
                        .rounded(corner(bar::BUTTON_RADIUS))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(palette.on_surface.with_alpha(0.08).hsla()))
                        .child(icon("undo", bar::ICON * ts, if self.shapes.is_empty() { palette.on_surface_variant.with_alpha(0.5) } else { palette.on_surface }))
                        .keep_inside()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.shapes.pop();
                            cx.notify();
                        })),
                )
        });
        let toolbar = dock::frame(&palette, ts, fade)
            .child(mode_btn("snip-region", SnipMode::Region, cx))
            .child(mode_btn("snip-window", SnipMode::Window, cx))
            .child(mode_btn("snip-screen", SnipMode::Screen, cx))
            .children(annotate_tools)
            .when(!self.record, |el| {
                el.child(div().w(px(1.0)).h(z(bar::SEPARATOR_H)).mx(z(bar::SEPARATOR_MX)).bg(palette.outline.with_alpha(0.4).hsla()))
                    .child(div().flex().items_center().gap(z(bar::GROUP_GAP)).child(action_btn("snip-copy", "clipboard", Action::Copy, true, cx)).child(action_btn("snip-save", "disk", Action::Save, false, cx)).child(action_btn("snip-text", "text-scan", Action::Text, false, cx)))
            })
            .child(div().w(z(bar::TAIL)))
            .when(self.record, |el| el.child(
                div()
                    .id("snip-go")
                    .h(z(bar::BUTTON))
                    .px(z(bar::GO_PAD))
                    .rounded(corner(bar::BUTTON_RADIUS))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .gap(z(bar::GO_GAP))
                    .bg(if !can_confirm { palette.surface_container_high.hsla() } else if self.record { palette.urgent_coral.hsla() } else { palette.primary.hsla() })
                    .text_color(if can_confirm { palette.on_primary.hsla() } else { palette.on_surface_variant.hsla() })
                    .when(self.record, |el| el.child(div().size(z(bar::REC_DOT)).rounded_full().bg(gpui::white())))
                    .child(if self.record { t("Record") } else { t("Capture") })
                    .keep_inside()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.confirm(cx))),
            ))
            // Leave snipping (timer, pause), move the toolbar, close.
            .child(dock::separator(&palette, ts))
            .child(dock::icon_button("snip-delay", "timer", dock::delay_hint(self.shot.countdown_secs()), &palette, ts).keep_inside().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.delay(cx))))
            .child(
                dock::icon_button("snip-pause", "pause", t("Leave snipping and keep the toolbar · Esc"), &palette, ts)
                    .keep_inside()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pause(Input::Pause, cx))),
            )
            .child(
                dock::icon_button("snip-move", dock::move_glyph(self.shot.toolbar_position), dock::move_hint(self.shot.toolbar_position), &palette, ts)
                    .keep_inside()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.flip_toolbar(cx))),
            )
            .child(
                dock::icon_button("snip-close", "close", t("Close the screenshot tool"), &palette, ts)
                    .keep_inside()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close(cx))),
            );
        // Enter does what the plugin settings say (copy only when saving
        // to disk is off).
        let enter = match (self.copy, self.save) {
            (true, true) => t("Enter copy + save"),
            (true, false) => t("Enter copy"),
            (false, _) => t("Enter save"),
        };
        let hint: gpui::SharedString = match (&error, self.mode) {
            (Some(e), _) => format!("{e}").into(),
            (_, SnipMode::Region) if self.record => t("Drag to choose the area to record · Enter to start · Esc to cancel").into(),
            (_, SnipMode::Region) if self.tool != Tool::Select && candidate.is_some() => tf("Drag inside the selection to annotate · Ctrl+Z undo · {enter}", &[("enter", &enter)]).into(),
            (_, SnipMode::Region) => tf("Drag to select · A arrow · B box · C circle · Ctrl+C copy · Ctrl+S save · Ctrl+T text · {enter}", &[("enter", &enter)]).into(),
            (_, SnipMode::Window) => tf("Click a window to pick it, again to capture · {enter}", &[("enter", &enter)]).into(),
            (_, SnipMode::Screen) => tf("Click anywhere to capture the whole screen · Ctrl+C copy · {enter}", &[("enter", &enter)]).into(),
        };
        let show_hint = self.shot.show_hints || error.is_some();
        let bottom = self.shot.toolbar_position == ToolbarPosition::Bottom;
        // The hint strip sits on the screen side of the toolbar.
        let hint_inset = z(bar::MARGIN + bar::H + bar::HINT_GAP);
        let hint_w = bar::HINT_W * ts;
        let toolbar_row = div().absolute().left_0().right_0().flex().justify_center().child(toolbar);
        let toolbar_row = if bottom { toolbar_row.bottom(z(bar::MARGIN)) } else { toolbar_row.top(z(bar::MARGIN)) };
        root.child(toolbar_row).when(show_hint, |root| root.child(
            div()
                .absolute()
                .when(bottom, |el| el.bottom(hint_inset))
                .when(!bottom, |el| el.top(hint_inset))
                .left(px(sw / 2.0 - hint_w / 2.0))
                .w(px(hint_w))
                .flex()
                .justify_center()
                .opacity(fade * 0.9)
                .child(
                    div()
                        .px(z(bar::HINT_PAD))
                        .h(z(bar::HINT_H))
                        .rounded_full()
                        .bg(if error.is_some() { palette.urgent_coral.with_alpha(0.9).hsla() } else { gpui::black().opacity(0.45) })
                        .text_color(gpui::white())
                        .text_size(z(text::LABEL))
                        .flex()
                        .items_center()
                        .child(hint),
                ),
        ))
    }
}

/// Captures the screen natively, then opens the snipping overlay on the
/// output under the focused window (or the first output).
pub fn open(mode: SnipMode, cx: &mut App) {
    begin(mode, false, Preselect::None, 0, None, None, cx);
}

/// Opens the editor on a fresh capture with the focused window or the
/// whole screen already selected ("edit after capture").
pub fn open_editor(preselect: Preselect, cx: &mut App) {
    begin(SnipMode::Region, false, preselect, 0, None, None, cx);
}

/// The snipping tool in record mode: pick a region / window / screen to
/// record as video.
pub fn open_for_recording(cx: &mut App) {
    begin(SnipMode::Region, true, Preselect::None, 0, None, None, cx);
}

/// Non-interactive capture for commands: the focused window or the screen.
pub fn capture_now(window_only: bool, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let hub = crate::hub::hub(cx).read(cx);
    let focused = hub.focused.clone();
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let shot = ScreenshotSettings::from_plugins(&cfg.plugins);
    let (save, copy) = (shot.save, shot.copy);
    let shutter = crate::alerts::deferred_sound(mandara_services::sounds::SoundEvent::Screenshot, cx);
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = mandara_plugins::guard("screenshot", || -> Result<String, String> {
            let captures = mandara_services::screencopy::capture_all_with(shot.cursor).map_err(|e| format!("{e:#}"))?;
            let monitor = focused.as_ref().map(|c| c.monitor.clone()).unwrap_or_default();
            let cap = captures.iter().find(|c| c.output == monitor).or(captures.first()).ok_or(t("no outputs"))?;
            let image = if window_only {
                let (x, y, w, h) = focused.as_ref().and_then(|c| c.geometry).ok_or(t("the compositor did not report a focused window geometry"))?;
                cap.crop_logical((x - cap.logical.0) as f32, (y - cap.logical.1) as f32, w as f32, h as f32)
            } else {
                cap.to_image()
            };
            let delivered = deliver_with(&image, &shot, save, copy).map(|r| describe(&r));
            if delivered.is_ok() {
                shutter();
            }
            delivered
        })
        .unwrap_or_else(|| Err(t("internal error while capturing (logged)").into()));
        let _ = tx.send_blocking(result);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_any_drag_direction() {
        let r = Rect::from_points((50.0, 40.0), (10.0, 100.0));
        assert_eq!(r, Rect { x: 10.0, y: 40.0, w: 40.0, h: 60.0 });
        assert!(r.contains((20.0, 50.0)));
        assert!(!r.contains((5.0, 50.0)));
    }
}
