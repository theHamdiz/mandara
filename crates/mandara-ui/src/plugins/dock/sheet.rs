//! What the dock's popovers share (stack fans and grids, the Applications
//! grid, icon menus, the app picker): their lifecycle, the glass they sit
//! on, an in-surface item menu and lazily decoded thumbnails.
//!
//! Every popover is a full-output overlay layer surface (so a click
//! anywhere outside its card dismisses it). [`Sheet`] owns its open /
//! close spring and, the moment it starts closing, gives the pointer back
//! (empty input region), releases the dock it pinned out
//! ([`super::popover_closed`]) and makes sure the surface goes away even
//! if no further frame arrives — a closed popover must never linger as an
//! invisible full-screen surface that swallows the dock's reveal strip.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{AnyElement, App, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, Window, WindowOptions, div, prelude::FluentBuilder as _, px};
use mandara_core::{FrameClock, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_theme::{Color, MandaraPalette};

use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{elevation, readable, rim};

/// Below this openness a closing popover is gone.
const CLOSED_EPSILON: f32 = 0.02;
/// A closing popover is removed after this long even if its outro never
/// finishes (no frame callbacks while covered, a stalled clock).
const CLOSE_DEADLINE: Duration = Duration::from_millis(900);
/// Glass: least opacity of a popover panel (a fully clear dock glass
/// still gets a readable panel).
const GLASS_MIN_ALPHA: f32 = 0.86;
/// Item menu geometry.
const MENU_W: f32 = 236.0;
const MENU_ROW_H: f32 = 30.0;
const MENU_PAD: f32 = 6.0;
const MENU_SEP_H: f32 = 7.0;
const MENU_RADIUS: f32 = 12.0;
const MENU_ROW_RADIUS: f32 = 8.0;
const MENU_ROW_PAD: f32 = 9.0;
const MENU_GAP: f32 = 9.0;
const MENU_GLYPH: f32 = 14.0;
const MENU_EDGE: f32 = 8.0;
const DIVIDER_ALPHA: f32 = 0.08;
const HOVER_ALPHA: f32 = 0.08;
/// An item menu sits a step lighter than the panel it opens over.
const MENU_LIFT: f32 = 0.7;
/// Previews: images bigger than this (file size or pixels) show their
/// type icon (decoding them costs more memory than a glance is worth).
pub const PREVIEW_MAX_BYTES: u64 = 12 * 1024 * 1024;
const PREVIEW_MAX_PIXELS: u64 = 24_000_000;

/// A popover's lifecycle: its open / close spring and its release.
pub struct Sheet {
    open: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    released: bool,
    /// Animation time since the first frame / since closing started, in
    /// seconds of [`FrameClock`] steps: vsync-even, and a slow frame (the
    /// surface's first, icon uploads) slows the motion by one capped step
    /// instead of skipping ahead. Wall-clock time made the fan jump.
    open_t: f32,
    close_t: f32,
    gone: bool,
}

impl Sheet {
    pub fn new(spring: SpringConfig) -> Self {
        Self { open: SpringValue::new(0.0, spring), clock: FrameClock::new(), closing: false, released: false, open_t: 0.0, close_t: 0.0, gone: false }
    }

    pub fn closing(&self) -> bool {
        self.closing
    }

    /// The window was removed this frame (ask for no more frames).
    pub fn gone(&self) -> bool {
        self.gone
    }

    /// Starts the outro (the caller notifies).
    pub fn close(&mut self) {
        if !self.closing {
            self.closing = true;
            self.close_t = 0.0;
        }
    }

    /// Animation seconds since the popover's first frame.
    pub fn age(&self) -> f32 {
        self.open_t
    }

    /// Animation seconds since it started closing (0 while open).
    pub fn closing_for(&self) -> f32 {
        if self.closing { self.close_t } else { 0.0 }
    }

    /// Advances the spring and returns the openness (0–1, may overshoot
    /// slightly). `hold` keeps a closing popover alive while its own
    /// choreography (the fan) still runs. Removes the window once closed.
    pub fn step(&mut self, hold: bool, window: &mut Window, cx: &mut App) -> f32 {
        let reduce = mandara_core::spring::motion_profile().reduce;
        self.open.set_instant(reduce);
        self.open.set_target(if self.closing { 0.0 } else { 1.0 });
        let dt = self.clock.tick_vsync(Instant::now(), window.refresh_interval(cx));
        self.open_t += dt;
        if self.closing {
            self.close_t += dt;
        }
        let moving = self.open.step(dt);
        if self.closing && !self.released {
            self.released = true;
            // Clicks fall through at once; the dock may hide again.
            window.set_input_region(Some(&[]));
            let handle = window.window_handle();
            cx.defer(move |cx| super::popover_closed(handle, cx));
            cx.spawn(async move |cx| {
                cx.background_executor().timer(CLOSE_DEADLINE).await;
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            })
            .detach();
        }
        if self.closing && self.open.value() <= CLOSED_EPSILON && !hold {
            self.gone = true;
            window.remove_window();
            return 0.0;
        }
        if moving || hold {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        self.open.value().max(0.0)
    }
}

/// Window options of a popover over `monitor`'s whole output.
pub fn popover_options(monitor: &str, keyboard: KeyboardInteractivity, cx: &App) -> WindowOptions {
    let (ow, oh) = super::output_size(monitor, cx);
    let display = super::display_for(monitor, cx);
    super::layer_options(SurfaceRole::Dock, Layer::Overlay, Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT, (ow, oh), None, keyboard, display)
}

/// The panel colour of a popover: the dock's glass (its tint, its
/// opacity, never too clear to read).
pub fn glass(palette: &MandaraPalette, cfg: &mandara_config::workflow::DockConfig) -> Color {
    crate::shell::frame_color(palette).mix(palette.primary, cfg.tint).with_alpha(cfg.opacity.max(GLASS_MIN_ALPHA))
}

/// The shadow of a popover panel at openness `p`: elevation and the
/// 1 px specular rim.
pub fn panel_shadow(palette: &MandaraPalette, p: f32) -> Vec<gpui::BoxShadow> {
    let mut s = elevation(palette, p.clamp(0.0, 1.0));
    s.extend(rim(palette));
    s
}

/// The rectangle of a card of `size` growing out of `from` (both output
/// coordinates) at progress `p` (0 = the icon, 1 = full size).
pub fn grow_from(from: (f32, f32, f32, f32), to: (f32, f32, f32, f32), p: f32) -> (f32, f32, f32, f32) {
    let l = |a: f32, b: f32| a + (b - a) * p;
    (l(from.0, to.0), l(from.1, to.1), l(from.2, to.2).max(0.0), l(from.3, to.3).max(0.0))
}

/// Where a card of `w × h` sits next to `anchor` (the dock icon, output
/// coordinates) for a dock on `position`, inside `screen` with `gap`.
pub fn place_card(position: mandara_config::workflow::DockPosition, anchor: (f32, f32, f32, f32), (w, h): (f32, f32), screen: (f32, f32), gap: f32) -> (f32, f32) {
    use mandara_config::workflow::DockPosition;
    let (ax, ay, aw, ah) = anchor;
    let (x, y) = match position {
        DockPosition::Bottom => (ax + aw / 2.0 - w / 2.0, ay - gap - h),
        DockPosition::Left => (ax + aw + gap, ay + ah / 2.0 - h / 2.0),
        DockPosition::Right => (ax - gap - w, ay + ah / 2.0 - h / 2.0),
    };
    (x.clamp(gap, (screen.0 - w - gap).max(gap)), y.clamp(gap, (screen.1 - h - gap).max(gap)))
}

// ── Item menu ───────────────────────────────────────────────────────────

/// One row of an item menu.
#[derive(Clone, Debug)]
pub struct MenuRow {
    pub label: SharedString,
    pub glyph: &'static str,
    /// `Some` for a toggle (✓ when on).
    pub check: Option<bool>,
    pub danger: bool,
    /// A hairline above the row.
    pub sep: bool,
}

impl MenuRow {
    pub fn new(label: impl Into<SharedString>, glyph: &'static str) -> Self {
        Self { label: label.into(), glyph, check: None, danger: false, sep: false }
    }
    pub fn check(mut self, on: bool) -> Self {
        self.check = Some(on);
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn sep(mut self) -> Self {
        self.sep = true;
        self
    }
}

/// What a click on an item-menu row runs (view, row index).
pub type MenuHandler<V> = Rc<dyn Fn(&mut V, usize, &mut Window, &mut Context<V>)>;

/// Height of a menu of `rows`.
pub fn menu_height(rows: &[MenuRow]) -> f32 {
    MENU_PAD * 2.0 + rows.len() as f32 * MENU_ROW_H + rows.iter().filter(|r| r.sep).count() as f32 * MENU_SEP_H
}

/// An item menu at `at` (surface coordinates, kept on `screen`); a click
/// on row `i` calls `on(i)`.
pub fn item_menu<V: 'static>(rows: Vec<MenuRow>, at: (f32, f32), screen: (f32, f32), palette: &MandaraPalette, fill: Color, on: MenuHandler<V>, cx: &mut Context<V>) -> AnyElement {
    let h = menu_height(&rows);
    let x = at.0.clamp(MENU_EDGE, (screen.0 - MENU_W - MENU_EDGE).max(MENU_EDGE));
    let y = if at.1 + h + MENU_EDGE > screen.1 { (at.1 - h).max(MENU_EDGE) } else { at.1 };
    let accent = readable(palette.primary, palette);
    let mut list = div().flex().flex_col();
    for (i, r) in rows.into_iter().enumerate() {
        if r.sep {
            list = list.child(div().h(px(MENU_SEP_H)).flex().items_center().child(div().w_full().h(px(1.0)).bg(palette.on_surface.with_alpha(DIVIDER_ALPHA).hsla())));
        }
        let tint = if r.danger { palette.urgent_coral } else { palette.on_surface_variant };
        let on = on.clone();
        list = list.child(
            div()
                .id(("dock-item-menu", i))
                .h(px(MENU_ROW_H))
                .px(px(MENU_ROW_PAD))
                .rounded(px(crate::style::radius(MENU_ROW_RADIUS)))
                .flex()
                .items_center()
                .gap(px(MENU_GAP))
                .cursor_pointer()
                .hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla()))
                .child(icon(r.glyph, MENU_GLYPH, tint))
                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).when(r.danger, |el| el.text_color(palette.urgent_coral.hsla())).child(r.label.clone()))
                .when_some(r.check, |el, on| el.child(div().size(px(MENU_GLYPH)).when(on, |el| el.child(icon("check", MENU_GLYPH, accent)))))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| on(this, i, window, cx))),
        );
    }
    div()
        .id("dock-item-menu")
        .occlude()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(MENU_W))
        .h(px(h))
        .p(px(MENU_PAD))
        .rounded(px(crate::style::radius(MENU_RADIUS)))
        .bg(fill.mix(palette.surface_container_high, MENU_LIFT).hsla())
        .shadow(panel_shadow(palette, 1.0))
        .font_weight(FontWeight::NORMAL)
        .child(list)
        .into_any_element()
}

// ── Thumbnails ──────────────────────────────────────────────────────────

/// Image previews decoded off the UI thread, downscaled to the size they
/// are shown at, held only by the popover showing them (gone with it).
#[derive(Default)]
pub struct Thumbs {
    images: HashMap<PathBuf, Option<Arc<RenderImage>>>,
}

impl Thumbs {
    /// The preview of `path` at `side` device px: `Some` once decoded;
    /// starts the decode on first ask.
    pub fn get<V: 'static>(&mut self, path: &Path, side: u32, slot: fn(&mut V) -> &mut Thumbs, cx: &mut Context<V>) -> Option<Arc<RenderImage>> {
        if let Some(hit) = self.images.get(path) {
            return hit.clone();
        }
        self.images.insert(path.to_path_buf(), None);
        let p = path.to_path_buf();
        let task = cx.background_executor().spawn(async move { decode_thumb(&p, side) });
        let key = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let img = task.await;
            let _ = this.update(cx, |v, cx| {
                if let Some(img) = img {
                    slot(v).images.insert(key, Some(img));
                    cx.notify();
                }
            });
        })
        .detach();
        None
    }

    /// Frees every preview (their atlas textures too).
    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        for img in self.images.drain().filter_map(|(_, i)| i) {
            cx.drop_image(img, Some(window));
        }
    }
}

/// `path` decoded and shrunk to fit `side` px, as GPUI's BGRA.
pub fn decode_thumb(path: &Path, side: u32) -> Option<Arc<RenderImage>> {
    let (w, h) = image::image_dimensions(path).ok()?;
    if u64::from(w) * u64::from(h) > PREVIEW_MAX_PIXELS {
        return None;
    }
    let img = image::open(path).ok()?.thumbnail(side, side);
    let mut rgba = img.to_rgba8();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

/// Mouse-down catcher behind a card: dismisses on any button.
pub fn outside<V: 'static>(id: &'static str, cx: &mut Context<V>, f: impl Fn(&mut V, &mut Context<V>) + 'static) -> gpui::Stateful<gpui::Div> {
    div().id(id).absolute().inset_0().on_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, _, cx| f(this, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::workflow::DockPosition;

    #[test]
    fn cards_sit_beside_their_icon_and_stay_on_screen() {
        let screen = (2560.0, 1440.0);
        let icon = (1400.0, 1370.0, 48.0, 48.0);
        let (x, y) = place_card(DockPosition::Bottom, icon, (400.0, 300.0), screen, 12.0);
        assert_eq!((x, y), (1224.0, 1058.0), "centred above the icon");
        let (x, _) = place_card(DockPosition::Bottom, (2540.0, 1370.0, 48.0, 48.0), (400.0, 300.0), screen, 12.0);
        assert_eq!(x, 2560.0 - 400.0 - 12.0, "kept on screen");
        let (x, y) = place_card(DockPosition::Left, (8.0, 700.0, 48.0, 48.0), (400.0, 300.0), screen, 12.0);
        assert_eq!((x, y), (68.0, 574.0));
        let (x, _) = place_card(DockPosition::Right, (2504.0, 700.0, 48.0, 48.0), (400.0, 300.0), screen, 12.0);
        assert_eq!(x, 2092.0);
        assert_eq!(grow_from((0.0, 0.0, 10.0, 10.0), (100.0, 50.0, 110.0, 60.0), 0.5), (50.0, 25.0, 60.0, 35.0));
    }

    #[test]
    fn menus_are_as_tall_as_their_rows() {
        let rows = vec![MenuRow::new("Open", "play"), MenuRow::new("Trash", "trash").sep().danger()];
        assert_eq!(menu_height(&rows), MENU_PAD * 2.0 + MENU_ROW_H * 2.0 + MENU_SEP_H);
    }

    #[test]
    fn thumbnails_shrink_to_their_display_size() {
        let dir = std::env::temp_dir().join(format!("mandara-dock-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.png");
        image::RgbaImage::from_pixel(640, 320, image::Rgba([10, 20, 30, 255])).save(&p).unwrap();
        let t = decode_thumb(&p, 64).expect("decoded");
        let s = t.size(0);
        assert_eq!((s.width.0, s.height.0), (64, 32), "fits the side, keeps the aspect");
        assert!(decode_thumb(&dir.join("missing.png"), 64).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
