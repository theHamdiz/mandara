//! Full-screen frozen-frame pickers built on the screenshot module's native
//! screencopy capture: an eyedropper with a magnifier loupe (pixel grid,
//! crosshair, live HEX under the cursor; click to pick, arrows nudge one
//! pixel, scroll zooms, Esc cancels) and a region selector (used to scan
//! QR codes). A `demo` eyedropper previews the overlay without taking the
//! keyboard or the pointer and closes itself after a few seconds.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, Bounds, Context, Corners, FocusHandle, FontWeight, Hsla, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Render, RenderImage, ScrollEvent, Styled, StyledImage as _, Window, canvas, div,
    fill, img, point, px, size,
};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::t;
use mandara_services::color_picker as cp;
use mandara_services::screencopy::Capture;

use crate::fonts::{fonts, tabular_nums};
use crate::kit::text;

/// Loupe grid side (odd, so one cell is the centre).
const LOUPE_CELLS: usize = 15;
/// Cell sizes the scroll wheel steps through (logical px).
const ZOOM_STEPS: [f32; 4] = [7.0, 9.0, 11.0, 14.0];
const DEFAULT_ZOOM: usize = 1;
/// Gap between the cursor and the loupe.
const LOUPE_OFFSET: f32 = 26.0;
const LOUPE_PAD: f32 = 8.0;
const LOUPE_RADIUS: f32 = 20.0;
const LOUPE_FOOTER_H: f32 = 34.0;
/// Smallest region accepted by the region selector.
const MIN_REGION: f32 = 8.0;
/// How long the demo eyedropper stays up.
pub const DEMO_FOR: Duration = Duration::from_millis(2800);
/// Demo pointer path: radius and speed of its orbit around the centre,
/// and the vertical squash of the orbit.
const DEMO_ORBIT: f32 = 140.0;
const DEMO_SPEED: f32 = 1.6;
const DEMO_ORBIT_SQUASH: f32 = 0.6;
/// Fade spring of the overlay.
const FADE_SPRING: SpringConfig = SpringConfig::new(300.0, 30.0, 1.0);
/// Arrow-key nudge with Shift held, in physical pixels.
const SHIFT_NUDGE: i64 = 10;
/// Pixels per scroll line when converting wheel deltas.
const SCROLL_LINE_PX: f32 = 20.0;
/// Bottom grid corners are rounded less (the footer sits under them).
const LOUPE_BOTTOM_RADIUS_FRAC: f32 = 0.4;
/// Crosshair line alpha.
const HAIR_ALPHA: f32 = 0.28;
/// Picked-cell ring: outset, radius and halo (alpha, blur, spread).
const PICK_RING_OUTSET_PX: f32 = 2.0;
const PICK_RING_RADIUS_PX: f32 = 3.0;
const PICK_HALO: (f32, f32, f32) = (0.6, 2.0, 1.0);
/// Loupe card colour (a fixed near-black: the overlay is theme-neutral so
/// any picked colour reads against it) and its shadow (alpha, dy, blur).
const LOUPE_BG: u32 = 0x0011_1318;
const LOUPE_SHADOW: (f32, f32, f32) = (0.45, 6.0, 18.0);
const FOOTER_PAD_X_PX: f32 = 4.0;
const FOOTER_GAP_PX: f32 = 8.0;
const SWATCH_PX: f32 = 16.0;
const SWATCH_RADIUS_PX: f32 = 5.0;
const SWATCH_EDGE_ALPHA: f32 = 0.25;
const RGB_TEXT_ALPHA: f32 = 0.6;
/// Region mode: dim alpha outside the selection, selection radius.
const DIM_ALPHA: f32 = 0.45;
const SELECTION_RADIUS_PX: f32 = 4.0;
/// Hint pill: top offset, opacity, padding, height, fill alpha, text size.
const HINT_TOP_PX: f32 = 32.0;
const HINT_OPACITY: f32 = 0.92;
const HINT_PAD_X_PX: f32 = 14.0;
const HINT_H_PX: f32 = 28.0;
const HINT_FILL_ALPHA: f32 = 0.55;
const HINT_TEXT_PX: f32 = text::LABEL + 0.5;
/// Bytes per BGRA / RGBA pixel.
const BYTES_PER_PIXEL: usize = 4;
/// Bit offsets of red and green in a packed `0xRRGGBB`.
const RED_SHIFT: u32 = 16;
const GREEN_SHIFT: u32 = 8;

pub type OnColor = Box<dyn FnOnce([u8; 3], &mut App)>;
pub type OnRegion = Box<dyn FnOnce(image::RgbaImage, &mut App)>;

enum Purpose {
    Color(Option<OnColor>),
    Region(Option<OnRegion>),
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
}

pub struct PickView {
    /// Output geometry. Its pixels were moved into `frozen` (BGRA) — the
    /// overlay keeps one copy of the frame, not two.
    capture: Arc<Capture>,
    frozen: Arc<RenderImage>,
    purpose: Purpose,
    pointer: Option<(f32, f32)>,
    /// Keyboard nudge in physical pixels.
    nudge: (i64, i64),
    zoom: usize,
    drag_start: Option<(f32, f32)>,
    selection: Option<Rect>,
    focus: FocusHandle,
    fade: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    /// Demo mode: opened at, no input.
    demo: Option<Instant>,
}

/// Moves the capture's RGBA pixels into the BGRA layout GPUI's renderer
/// expects, in place (no copy of a multi-megabyte frame). Runs off the UI
/// thread; `capture.rgba` is left empty.
fn frozen_image(capture: &mut Capture) -> Option<Arc<RenderImage>> {
    let mut bgra = std::mem::take(&mut capture.rgba);
    for px in bgra.chunks_exact_mut(BYTES_PER_PIXEL) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(capture.width, capture.height, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

/// A BGR sample (from the frozen BGRA frame) as RGB.
fn rgb_of([b, g, r]: [u8; 3]) -> [u8; 3] {
    [r, g, b]
}

/// Crops a logical-space rect out of the frozen BGRA frame as RGBA,
/// copying only the cropped pixels.
fn crop_logical(capture: &Capture, bgra: &[u8], r: Rect) -> image::RgbaImage {
    let s = capture.scale();
    let to_px = |v: f32, max: u32| ((v * s).round().max(0.0) as u32).min(max);
    let (x0, y0) = (to_px(r.x, capture.width), to_px(r.y, capture.height));
    let x1 = to_px(r.x + r.w, capture.width).max(x0 + 1).min(capture.width);
    let y1 = to_px(r.y + r.h, capture.height).max(y0 + 1).min(capture.height);
    let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    image::RgbaImage::from_fn(w, h, |x, y| {
        let i = ((y0 + y) as usize * capture.width as usize + (x0 + x) as usize) * BYTES_PER_PIXEL;
        match bgra.get(i..i + BYTES_PER_PIXEL) {
            Some(p) => image::Rgba([p[2], p[1], p[0], p[3]]),
            None => image::Rgba([0, 0, 0, u8::MAX]),
        }
    })
}

fn rgb_hsla([r, g, b]: [u8; 3]) -> Hsla {
    gpui::rgb((u32::from(r) << RED_SHIFT) | (u32::from(g) << GREEN_SHIFT) | u32::from(b)).into()
}

/// Physical pixel under a logical point (plus nudge).
fn physical(capture: &Capture, p: (f32, f32), nudge: (i64, i64)) -> (i64, i64) {
    let s = capture.scale();
    ((p.0 * s).floor() as i64 + nudge.0, (p.1 * s).floor() as i64 + nudge.1)
}

/// Loupe origin beside the cursor, flipped to stay on screen.
fn loupe_origin(pointer: (f32, f32), loupe: (f32, f32), screen: (f32, f32)) -> (f32, f32) {
    let x = if pointer.0 + LOUPE_OFFSET + loupe.0 > screen.0 { pointer.0 - LOUPE_OFFSET - loupe.0 } else { pointer.0 + LOUPE_OFFSET };
    let y = if pointer.1 + LOUPE_OFFSET + loupe.1 > screen.1 { pointer.1 - LOUPE_OFFSET - loupe.1 } else { pointer.1 + LOUPE_OFFSET };
    (x.max(0.0), y.max(0.0))
}

impl PickView {
    fn new(capture: Capture, frozen: Arc<RenderImage>, purpose: Purpose, demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        if !demo {
            focus.focus(window, cx);
        }
        Self {
            capture: Arc::new(capture),
            frozen,
            purpose,
            pointer: None,
            nudge: (0, 0),
            zoom: DEFAULT_ZOOM,
            drag_start: None,
            selection: None,
            focus,
            fade: SpringValue::new(0.0, FADE_SPRING),
            clock: FrameClock::new(),
            closing: false,
            demo: demo.then(Instant::now),
        }
    }

    /// The frozen frame's BGRA bytes.
    fn pixels(&self) -> &[u8] {
        self.frozen.as_bytes(0).unwrap_or_default()
    }

    fn sample_at(&self, p: (f32, f32)) -> Option<[u8; 3]> {
        let (x, y) = physical(&self.capture, p, self.nudge);
        cp::sample(self.capture.width, self.capture.height, self.pixels(), x, y).map(rgb_of)
    }

    fn pick(&mut self, cx: &mut Context<Self>) {
        if self.closing || self.demo.is_some() {
            return;
        }
        let Some(p) = self.pointer else { return };
        let Some(rgb) = self.sample_at(p) else { return };
        if let Purpose::Color(slot) = &mut self.purpose
            && let Some(done) = slot.take()
        {
            self.closing = true;
            cx.notify();
            cx.defer(move |cx| done(rgb, cx));
        }
    }

    fn finish_region(&mut self, r: Rect, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if let Purpose::Region(slot) = &mut self.purpose
            && let Some(done) = slot.take()
        {
            let image = crop_logical(&self.capture, self.pixels(), r);
            self.closing = true;
            cx.notify();
            cx.defer(move |cx| done(image, cx));
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        cx.notify();
    }

    fn full(&self) -> Rect {
        Rect { x: 0.0, y: 0.0, w: self.capture.logical.2 as f32, h: self.capture.logical.3 as f32 }
    }

    fn loupe(&self, pointer: (f32, f32), screen: (f32, f32), mono: gpui::SharedString, fade: f32) -> gpui::Div {
        let cell = ZOOM_STEPS[self.zoom];
        let grid_side = cell * LOUPE_CELLS as f32;
        let (w, h) = (grid_side + LOUPE_PAD * 2.0, grid_side + LOUPE_PAD * 2.0 + LOUPE_FOOTER_H);
        let (ox, oy) = loupe_origin(pointer, (w, h), screen);
        let (cx_, cy_) = physical(&self.capture, pointer, self.nudge);
        let cells: Vec<Hsla> = cp::loupe(self.capture.width, self.capture.height, self.pixels(), cx_, cy_, LOUPE_CELLS)
            .into_iter()
            .map(|c| c.map(|bgr| rgb_hsla(rgb_of(bgr))).unwrap_or_else(gpui::black))
            .collect();
        let center = cp::sample(self.capture.width, self.capture.height, self.pixels(), cx_, cy_).map(rgb_of);
        let last = LOUPE_CELLS - 1;
        let inner_r = px(LOUPE_RADIUS - LOUPE_PAD);
        let bottom_r = inner_r * LOUPE_BOTTOM_RADIUS_FRAC;
        // One canvas paints every magnified pixel as an SDF quad (instead of
        // a div per cell); the outer corner cells follow the card's radius.
        let pixels = canvas(
            |_, _, _| {},
            move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                for (i, color) in cells.iter().enumerate() {
                    let (row, col) = (i / LOUPE_CELLS, i % LOUPE_CELLS);
                    let origin = point(bounds.origin.x + px(col as f32 * cell), bounds.origin.y + px(row as f32 * cell));
                    let corners = Corners {
                        top_left: if row == 0 && col == 0 { inner_r } else { px(0.0) },
                        top_right: if row == 0 && col == last { inner_r } else { px(0.0) },
                        bottom_left: if row == last && col == 0 { bottom_r } else { px(0.0) },
                        bottom_right: if row == last && col == last { bottom_r } else { px(0.0) },
                    };
                    window.paint_quad(fill(Bounds::new(origin, size(px(cell), px(cell))), *color).corner_radii(corners));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mid = (LOUPE_CELLS / 2) as f32 * cell;
        let hair = gpui::white().opacity(HAIR_ALPHA);
        let grid = div()
            .relative()
            .size(px(grid_side))
            .child(pixels)
            // Crosshair through the centre row and column.
            .child(div().absolute().left(px(mid)).top_0().w(px(cell)).h_full().border_l_1().border_r_1().border_color(hair))
            .child(div().absolute().top(px(mid)).left_0().h(px(cell)).w_full().border_t_1().border_b_1().border_color(hair))
            // The picked cell: white ring with a dark halo (reads on any colour).
            .child(
                div()
                    .absolute()
                    .left(px(mid - PICK_RING_OUTSET_PX))
                    .top(px(mid - PICK_RING_OUTSET_PX))
                    .size(px(cell + PICK_RING_OUTSET_PX * 2.0))
                    .rounded(px(crate::style::radius(PICK_RING_RADIUS_PX)))
                    .border_2()
                    .border_color(gpui::white())
                    .shadow(vec![gpui::BoxShadow {
                        color: gpui::black().opacity(PICK_HALO.0),
                        offset: gpui::point(px(0.0), px(0.0)),
                        blur_radius: px(PICK_HALO.1),
                        spread_radius: px(PICK_HALO.2),
                        inset: false,
                    }]),
            );
        let (hex, rgb_text, swatch) = match center {
            Some(rgb) => (cp::to_hex(rgb).to_uppercase(), format!("{} {} {}", rgb[0], rgb[1], rgb[2]), rgb_hsla(rgb)),
            None => ("—".into(), String::new(), gpui::black()),
        };
        div()
            .absolute()
            .left(px(ox))
            .top(px(oy))
            .w(px(w))
            .h(px(h))
            .p(px(LOUPE_PAD))
            .rounded(px(crate::style::radius(LOUPE_RADIUS)))
            .bg(gpui::rgb(LOUPE_BG))
            .opacity(fade)
            .shadow(vec![gpui::BoxShadow {
                color: gpui::black().opacity(LOUPE_SHADOW.0),
                offset: gpui::point(px(0.0), px(LOUPE_SHADOW.1)),
                blur_radius: px(LOUPE_SHADOW.2),
                spread_radius: px(0.0),
                inset: false,
            }])
            .flex()
            .flex_col()
            .child(grid)
            .child(
                div()
                    .h(px(LOUPE_FOOTER_H))
                    .px(px(FOOTER_PAD_X_PX))
                    .flex()
                    .items_center()
                    .gap(px(FOOTER_GAP_PX))
                    .child(div().size(px(SWATCH_PX)).rounded(px(crate::style::radius(SWATCH_RADIUS_PX))).bg(swatch).border_1().border_color(gpui::white().opacity(SWATCH_EDGE_ALPHA)))
                    .child(div().font_family(mono).font_features(tabular_nums()).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(gpui::white()).child(hex))
                    .child(div().flex_1())
                    .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(gpui::white().opacity(RGB_TEXT_ALPHA)).child(rgb_text)),
            )
    }
}

impl Render for PickView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        if let Some(opened) = self.demo {
            // Demo: no input at all; orbit the centre, then close.
            window.set_input_region(Some(&[]));
            let t = opened.elapsed().as_secs_f32();
            self.pointer = Some((sw / 2.0 + DEMO_ORBIT * (t * DEMO_SPEED).cos(), sh / 2.0 + DEMO_ORBIT * DEMO_ORBIT_SQUASH * (t * DEMO_SPEED).sin()));
            if opened.elapsed() >= DEMO_FOR {
                self.closing = true;
            }
            window.request_animation_frame();
        }
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
        let region_mode = matches!(self.purpose, Purpose::Region(_));
        let mut root = div()
            .id("screen-pick")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .cursor(gpui::CursorStyle::Crosshair)
            .font_family(f.ui.clone())
            .text_size(px(text::BODY))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let step = if e.keystroke.modifiers.shift { SHIFT_NUDGE } else { 1 };
                match e.keystroke.key.as_str() {
                    "escape" => this.cancel(cx),
                    "enter" | "space" => {
                        if matches!(this.purpose, Purpose::Region(_)) {
                            let r = this.selection.filter(|r| r.w >= MIN_REGION && r.h >= MIN_REGION).unwrap_or_else(|| this.full());
                            this.finish_region(r, cx);
                        } else {
                            this.pick(cx);
                        }
                    }
                    "left" => this.nudge.0 -= step,
                    "right" => this.nudge.0 += step,
                    "up" => this.nudge.1 -= step,
                    "down" => this.nudge.1 += step,
                    _ => {}
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.demo.is_some() {
                    return;
                }
                let p = (f32::from(e.position.x), f32::from(e.position.y));
                this.pointer = Some(p);
                this.nudge = (0, 0);
                if let Some(start) = this.drag_start {
                    this.selection = Some(Rect::from_points(start, p));
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let p = (f32::from(e.position.x), f32::from(e.position.y));
                this.pointer = Some(p);
                if matches!(this.purpose, Purpose::Region(_)) {
                    this.drag_start = Some(p);
                    this.selection = None;
                } else {
                    this.pick(cx);
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, cx| {
                if let Some(start) = this.drag_start.take() {
                    let r = Rect::from_points(start, (f32::from(e.position.x), f32::from(e.position.y)));
                    if r.w >= MIN_REGION && r.h >= MIN_REGION {
                        this.finish_region(r, cx);
                    }
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.cancel(cx)))
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                let dy = f32::from(e.delta.pixel_delta(px(SCROLL_LINE_PX)).y);
                if dy > 0.0 && this.zoom + 1 < ZOOM_STEPS.len() {
                    this.zoom += 1;
                } else if dy < 0.0 && this.zoom > 0 {
                    this.zoom -= 1;
                }
                cx.notify();
            }))
            .child(img(ImageSource::Render(self.frozen.clone())).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Fill));

        if region_mode {
            let dim = gpui::black().opacity(DIM_ALPHA * fade);
            let bands = match self.selection {
                Some(r) => vec![(0.0, 0.0, sw, r.y), (0.0, r.y + r.h, sw, sh - r.y - r.h), (0.0, r.y, r.x, r.h), (r.x + r.w, r.y, sw - r.x - r.w, r.h)],
                None => vec![(0.0, 0.0, sw, sh)],
            };
            for (x, y, w, h) in bands {
                root = root.child(div().absolute().left(px(x)).top(px(y)).w(px(w.max(0.0))).h(px(h.max(0.0))).bg(dim));
            }
            if let Some(r) = self.selection {
                root = root.child(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).rounded(px(crate::style::radius(SELECTION_RADIUS_PX))).border_2().border_color(palette.primary.hsla()));
            }
        } else if let Some(p) = self.pointer {
            root = root.child(self.loupe(p, (sw, sh), f.mono.clone(), fade));
        }

        let hint = if self.demo.is_some() {
            t("Color picker preview")
        } else if region_mode {
            t("Drag around the QR code · Enter scans the whole screen · Esc cancels")
        } else {
            t("Click to pick · arrows nudge a pixel · scroll zooms · Esc cancels")
        };
        root.child(
            div().absolute().top(px(HINT_TOP_PX)).left_0().w(px(sw)).flex().justify_center().opacity(fade * HINT_OPACITY).child(
                div()
                    .px(px(HINT_PAD_X_PX))
                    .h(px(HINT_H_PX))
                    .rounded_full()
                    .bg(gpui::black().opacity(HINT_FILL_ALPHA))
                    .text_color(gpui::white())
                    .text_size(px(HINT_TEXT_PX))
                    .flex()
                    .items_center()
                    .child(hint),
            ),
        )
    }
}

fn open(purpose: Purpose, demo: bool, cx: &mut App) {
    let monitor = crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone()).unwrap_or_default();
    // Capture, pick the output and convert the frame to BGRA off the UI
    // thread (a 4K frame is ~33 MB: copying it on the foreground stutters).
    let task = cx.background_executor().spawn(async move {
        let captures = mandara_plugins::guard("screen capture", || mandara_services::screencopy::capture_all().map_err(|e| format!("{e:#}")))
            .unwrap_or_else(|| Err(t("internal error while capturing (logged)").into()))?;
        let index = captures.iter().position(|c| c.output == monitor).unwrap_or(0);
        let mut capture = captures.into_iter().nth(index).ok_or_else(|| "no outputs captured".to_string())?;
        let frozen = frozen_image(&mut capture).ok_or_else(|| "capture buffer size mismatch".to_string())?;
        Ok::<_, String>((capture, frozen))
    });
    cx.spawn(async move |cx| {
        let (capture, frozen) = match task.await {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("screen pick: {err}");
                return;
            }
        };
        let (w, h) = (capture.logical.2.max(1) as f32, capture.logical.3.max(1) as f32);
        cx.update(|cx| {
            let keyboard = if demo { KeyboardInteractivity::None } else { KeyboardInteractivity::Exclusive };
            let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, size(px(w), px(h)), None).with_keyboard(keyboard);
            spec.layer = Layer::Overlay;
            spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
            let _ = cx.open_window(spec.window_options(), move |window, cx| cx.new(|cx| PickView::new(capture, frozen, purpose, demo, window, cx))).map_err(|e| tracing::error!("screen pick overlay: {e:#}"));
        });
    })
    .detach();
}

/// Opens the eyedropper; `done` receives the picked colour.
pub fn pick_color(cx: &mut App, done: impl FnOnce([u8; 3], &mut App) + 'static) {
    open(Purpose::Color(Some(Box::new(done))), false, cx);
}

/// Opens a non-interactive eyedropper preview that closes itself after
/// [`DEMO_FOR`] (for screenshots and tests; takes no input).
pub fn demo_color(cx: &mut App) {
    open(Purpose::Color(None), true, cx);
}

/// Opens the region selector; `done` receives the cropped area.
pub fn pick_region(cx: &mut App, done: impl FnOnce(image::RgbaImage, &mut App) + 'static) {
    open(Purpose::Region(Some(Box::new(done))), false, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loupe_flips_at_screen_edges() {
        let loupe = (150.0, 190.0);
        assert_eq!(loupe_origin((100.0, 100.0), loupe, (2560.0, 1440.0)), (100.0 + LOUPE_OFFSET, 100.0 + LOUPE_OFFSET));
        let (x, y) = loupe_origin((2500.0, 1400.0), loupe, (2560.0, 1440.0));
        assert!(x + loupe.0 <= 2500.0 && y + loupe.1 <= 1400.0);
    }

    #[test]
    fn physical_pixels_follow_scale_and_nudge() {
        let capture = Capture { output: "x".into(), logical: (0, 0, 100, 50), width: 150, height: 75, rgba: vec![0; 150 * 75 * 4] };
        assert_eq!(physical(&capture, (10.0, 10.0), (0, 0)), (15, 15));
        // BGRA crops come back as RGBA, sized in physical pixels.
        let bgra: Vec<u8> = (0..150 * 75).flat_map(|_| [1, 2, 3, 255]).collect();
        let crop = crop_logical(&capture, &bgra, Rect { x: 10.0, y: 10.0, w: 20.0, h: 10.0 });
        assert_eq!(crop.dimensions(), (30, 15));
        assert_eq!(crop.get_pixel(0, 0).0, [3, 2, 1, 255]);
        assert_eq!(physical(&capture, (10.0, 10.0), (-1, 2)), (14, 17));
        assert_eq!(Rect::from_points((5.0, 9.0), (1.0, 2.0)), Rect { x: 1.0, y: 2.0, w: 4.0, h: 7.0 });
    }
}
