//! A shelf's window: a chromatic-glass card on its own layer surface
//! (`mandara-stash`, Overlay, anchored top-left and placed by its margins),
//! with room around the card for its shadow outside the input region.
//!
//! The header is the handle (drag to move, anywhere on any output; it
//! settles gently against an edge it lands near), the count badge drags
//! everything out, the corner grip resizes. Items show as a grid of tiles,
//! a list or a fanned stack; they fly in on a spring when they land and
//! leave in a poof when the shelf is emptied. Tiles drag out into any app
//! (several at once when selected), open on a double click, and have a
//! menu (open, reveal, copy, quick look, fetch, remove). Space shows a big
//! preview; holding the pointer still on a tile does too.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    AnyElement, App, AppContext as _, BoxShadow, Bounds, ClickEvent, Context, DragContent, DragData, DragOffer, ExternalData, ExternalDragPayload, ExternalPaths, FileDropEvent, FocusHandle, Focusable, FontWeight,
    ImageSource, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, Render, RenderImage, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, Subscription, Task, UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas, div, img,
    point, prelude::FluentBuilder as _, px, size, uniform_list,
};
use mandara_config::stash::{self as cfg_stash, ShelfView};
use mandara_core::{SurfaceRole, ToGpui as _};
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::model::{Item, ItemKind};
use super::thumbs::{Key, Thumb};
use super::{EDGE_INSET, Origin, entity, entity_ref, store};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};

/// Transparent room around the card for its shadow, logical px.
pub const PAD: f32 = 26.0;
const HEADER_H: f32 = 40.0;
const BODY_PAD: f32 = 10.0;
/// Grid tiles.
const TILE_W: f32 = 84.0;
const TILE_H: f32 = 104.0;
const THUMB: f32 = 68.0;
const TILE_GAP: f32 = 6.0;
const LIST_ROW_H: f32 = 44.0;
const LIST_ICON: f32 = 30.0;
const CARD_RADIUS: f32 = 20.0;
const THUMB_RADIUS: f32 = 12.0;
const GRIP: f32 = 18.0;
/// Icon fractions of a tile's picture box: file-type icons and SVG pictures.
const ICON_FRAC: f32 = 0.62;
const VECTOR_FRAC: f32 = 0.76;
/// Sizes file-type icons and SVG pictures are rasterized at (grid, list).
pub const ICON_SIZES: [f32; 4] = [THUMB * ICON_FRAC, THUMB * VECTOR_FRAC, LIST_ICON * ICON_FRAC, LIST_ICON * VECTOR_FRAC];
const SNAP_PX: f32 = 24.0;
/// The press must move this far before the header moves the shelf.
const MOVE_SLOP_PX: f32 = 3.0;
/// Hovering still on a tile this long shows its big preview.
const HOLD_FOR_PREVIEW: Duration = Duration::from_millis(1100);
/// Landing spring (items fly into place) and its length.
const LAND_SPRING: mandara_core::SpringConfig = mandara_core::SpringConfig::new(260.0, 19.0, 1.0);
const LAND_FOR: Duration = Duration::from_millis(900);
/// Farthest an item flies in from, px.
const LAND_REACH: f32 = 220.0;
const POOF_FOR: Duration = Duration::from_millis(480);
const POOF_SPARKS: usize = 8;
const OPEN_SPRING: mandara_core::SpringConfig = mandara_core::SpringConfig::new(320.0, 22.0, 1.0);
const OPEN_FOR: Duration = Duration::from_millis(650);
const SNAP_FOR: Duration = Duration::from_millis(260);
/// An edge-born shelf whose drag left without dropping retracts after this.
const RETRACT_AFTER: Duration = Duration::from_millis(700);

/// Layer-shell exclusive zone `-1`: placed against the whole output.
pub(super) const IGNORE_ZONES: f32 = -1.0;
/// The drag ghost: height and its title's widest.
const GHOST_H: f32 = 30.0;
const GHOST_TEXT_W: f32 = 200.0;
/// Soft drop shadow of the card, the ghost and the menus.
const SHADOW_Y: f32 = 8.0;
const SHADOW_BLUR: f32 = 22.0;
const SHADOW_SPREAD: f32 = -4.0;
/// Count badge.
const BADGE_H: f32 = 18.0;
const BADGE_PAD_X: f32 = 5.0;
/// The header's grip line.
const HANDLE_W: f32 = 22.0;
const HANDLE_H: f32 = 4.0;
/// The stack's badge above the bottom edge.
const STACK_BADGE_INSET: f32 = 8.0;
/// A link tile: gap, favicon corners, host label size and its inset.
const LINK_GAP: f32 = 4.0;
const FAVICON_RADIUS: f32 = 6.0;
const LINK_HOST_TEXT: f32 = 9.0;
const LINK_HOST_INSET: f32 = 8.0;
/// A grid tile's padding and the gap between its picture and name.
const TILE_PAD: f32 = 4.0;
const TILE_INNER_GAP: f32 = 4.0;
/// The lift under a tile's picture.
const THUMB_SHADOW_Y: f32 = 3.0;
const THUMB_SHADOW_BLUR: f32 = 8.0;
const THUMB_SHADOW_SPREAD: f32 = -2.0;
/// A list row's icon corners.
const LIST_ICON_RADIUS: f32 = 8.0;
/// The empty shelf's round badge.
const EMPTY_BADGE: f32 = 52.0;
/// Notes and the cache-limit banner: inset from the card's sides and corners.
const BANNER_INSET: f32 = 10.0;
const BANNER_RADIUS: f32 = 10.0;
/// The resize grip's dots: size, and the offset from each dot's centre.
const GRIP_DOT: f32 = 3.0;
const GRIP_DOT_HALF: f32 = 2.0;
/// Menus: width, padding, corners, row height and corners, edge inset.
const MENU_W: f32 = 196.0;
const MENU_PAD: f32 = 4.0;
const MENU_RADIUS: f32 = 12.0;
const MENU_ROW_H: f32 = 30.0;
const MENU_ROW_RADIUS: f32 = 8.0;
const MENU_EDGE: f32 = 8.0;
/// The close dialog: button height and the panel's corners.
const DIALOG_BUTTON_H: f32 = 32.0;
const DIALOG_RADIUS: f32 = 16.0;
/// Zero, for the layout's origins.
const ZERO: f32 = 0.0;

/// The surface rect for a card at `(x, y)` sized `(w, h)` on an output
/// `(ow, oh)`: the card plus its shadow room, kept on the output.
pub fn surface_rect((x, y): (f32, f32), (w, h): (f32, f32), (ow, oh): (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let (sw, sh) = ((w + 2.0 * PAD).min(ow.max(w)), (h + 2.0 * PAD).min(oh.max(h)));
    let ox = (x - PAD).clamp(0.0, (ow - sw).max(0.0)).round();
    let oy = (y - PAD).clamp(0.0, (oh - sh).max(0.0)).round();
    ((ox, oy), (sw, sh))
}

fn options(origin: (f32, f32), surface: (f32, f32), display: Option<gpui::DisplayId>) -> WindowOptions {
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(surface.0), px(surface.1)) })),
        focus: false,
        show: true,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: SurfaceRole::Stash.namespace().to_string(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::LEFT,
            // Placed against the whole output.
            exclusive_zone: Some(px(IGNORE_ZONES)),
            exclusive_edge: None,
            margin: Some((px(origin.1), px(0.0), px(0.0), px(origin.0))),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
        }),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(mandara_core::SHELL_APP_ID.to_string()),
        inactive_frame_interval: None,
        ..Default::default()
    }
}

/// Opens shelf `id`'s window.
pub fn open(id: u64, cx: &mut App) -> Option<WindowHandle<ShelfWindow>> {
    let e = entity_ref(cx)?;
    let (output, xy, wh) = {
        let s = e.read(cx).state.shelf(id)?;
        (s.output.clone(), (s.x, s.y), (s.w, s.h))
    };
    let all = super::outputs(cx);
    let output = output.filter(|o| all.iter().any(|g| g.name == *o)).or_else(|| crate::outputs::focused_monitor(cx)).or_else(|| all.first().map(|g| g.name.clone()));
    let out = output.as_deref().map_or_else(|| crate::overlays::primary_output_size(cx), |o| super::output_size(o, cx));
    // A shelf saved for a bigger output comes back on screen.
    let (w, h) = (wh.0.min(out.0 - 2.0 * EDGE_INSET).max(cfg_stash::SHELF_MIN_W), wh.1.min(out.1 - 2.0 * EDGE_INSET).max(cfg_stash::SHELF_MIN_H));
    let (x, y) = cfg_stash::snap_shelf(xy.0, xy.1, w, h, out.0, out.1, 0.0, 0.0);
    let (origin, surface) = surface_rect((x, y), (w, h), out);
    let display = output.as_deref().and_then(|o| crate::outputs::display_for(o, cx));
    let name = output.clone();
    match cx.open_window(options(origin, surface, display), move |window, cx| {
        window.set_accepts_data_drops(true);
        cx.new(|cx| ShelfWindow::new(id, name, origin, out, cx))
    }) {
        Ok(h) => {
            tracing::info!("stash: shelf {id} shown on {}", output.as_deref().unwrap_or("?"));
            Some(h)
        }
        Err(err) => {
            tracing::error!("stash: shelf surface: {err:#}");
            None
        }
    }
}

/// A press on the header (moving) or the grip (resizing).
#[derive(Copy, Clone, Debug)]
struct Press {
    /// Window-local pointer at the press.
    at: (f32, f32),
    /// Where in the card it was grabbed.
    grab: (f32, f32),
    /// The card's place and size then (output-local).
    card: (f32, f32, f32, f32),
    started: bool,
}

/// What a tile shows.
enum Look {
    Picture(Option<Arc<RenderImage>>),
    /// Drawn by GPUI's own rasterizer (SVG).
    Vector(PathBuf),
    Icon(Option<PathBuf>, &'static str),
    Text(SharedString),
    Link { host: SharedString, icon: Option<Arc<RenderImage>> },
}

struct TileData {
    id: u64,
    index: usize,
    title: SharedString,
    subtitle: SharedString,
    look: Look,
    selected: bool,
    /// Landing: displacement (1 → 0) and where from.
    landing: Option<(f32, (f32, f32))>,
    missing: bool,
}

/// The items being dragged out.
#[derive(Clone)]
pub struct DragItems {
    shelf: u64,
    ids: Rc<[u64]>,
    title: SharedString,
}

/// What follows the pointer while items are dragged inside the shelf.
pub struct Ghost {
    count: usize,
    title: SharedString,
}

impl Render for Ghost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        div()
            .row()
            .items_center()
            .gap(px(tokens::SM))
            .px(px(tokens::MD))
            .h(px(GHOST_H))
            .rounded_full()
            .bg(p.surface_container_high.hsla())
            .shadow(soft_shadow(&p, 0.3))
            .text_size(px(text::BODY_S))
            .text_color(p.on_surface.hsla())
            .child(icon("stash", 14.0, p.primary))
            .child(div().max_w(px(GHOST_TEXT_W)).truncate().child(self.title.clone()))
            .when(self.count > 1, |el| el.child(badge(self.count, &p)))
    }
}

pub struct ShelfWindow {
    id: u64,
    output: Option<String>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The surface's top-left on its output (as last requested).
    origin: (f32, f32),
    out: (f32, f32),
    moving: Option<Press>,
    resizing: Option<Press>,
    /// The card while moved / resized / settling (else the stored one).
    live: Option<(f32, f32, f32, f32)>,
    settle: Option<(Instant, (f32, f32), (f32, f32))>,
    input: Option<Bounds<Pixels>>,
    hold: Option<(u64, Task<()>)>,
    held: Option<u64>,
    _observe: Subscription,
}

impl Focusable for ShelfWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ShelfWindow {
    fn new(id: u64, output: Option<String>, origin: (f32, f32), out: (f32, f32), cx: &mut Context<Self>) -> Self {
        let e = entity(cx);
        let observe = cx.observe(&e, |_, _, cx| cx.notify());
        Self { id, output, focus: cx.focus_handle(), scroll: UniformListScrollHandle::new(), origin, out, moving: None, resizing: None, live: None, settle: None, input: None, hold: None, held: None, _observe: observe }
    }

    /// The card's place and size (output-local).
    fn card(&self, cx: &App) -> Option<(f32, f32, f32, f32)> {
        self.live.or_else(|| entity_ref(cx)?.read(cx).state.shelf(self.id).map(|s| (s.x, s.y, s.w, s.h)))
    }

    /// Moves the surface so the card sits at `(x, y)`.
    fn place(&mut self, (x, y, w, h): (f32, f32, f32, f32), window: &mut Window) {
        let ((ox, oy), (sw, sh)) = surface_rect((x, y), (w, h), self.out);
        if (ox, oy) != self.origin {
            self.origin = (ox, oy);
            window.set_layer_margin((px(oy), px(0.0), px(0.0), px(ox)));
        }
        let size = window.viewport_size();
        if (f32::from(size.width) - sw).abs() > 0.5 || (f32::from(size.height) - sh).abs() > 0.5 {
            window.resize(gpui::size(px(sw), px(sh)));
        }
        self.live = Some((x, y, w, h));
    }

    fn on_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (px_, py_) = (f32::from(e.position.x), f32::from(e.position.y));
        if let Some(mut press) = self.moving {
            if !press.started && (px_ - press.at.0).hypot(py_ - press.at.1) < MOVE_SLOP_PX {
                return;
            }
            press.started = true;
            self.moving = Some(press);
            // The pointer on the output: the surface's requested origin plus
            // its window-local position. The card stays on this output; a
            // release over another one moves it there.
            let (gx, gy) = (self.origin.0 + px_, self.origin.1 + py_);
            let (w, h) = (press.card.2, press.card.3);
            let x = (gx - press.grab.0).clamp(0.0, (self.out.0 - w).max(0.0));
            let y = (gy - press.grab.1).clamp(0.0, (self.out.1 - h).max(0.0));
            self.place((x, y, w, h), window);
            cx.notify();
        } else if let Some(mut press) = self.resizing {
            press.started = true;
            let (x, y, w0, h0) = press.card;
            let w = (w0 + px_ - press.at.0).clamp(cfg_stash::SHELF_MIN_W, (self.out.0 - x).min(cfg_stash::SHELF_MAX_W));
            let h = (h0 + py_ - press.at.1).clamp(cfg_stash::SHELF_MIN_H, (self.out.1 - y).min(cfg_stash::SHELF_MAX_H));
            self.resizing = Some(press);
            self.place((x, y, w, h), window);
            cx.notify();
        }
    }

    fn on_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let moved = self.moving.take().filter(|p| p.started).is_some();
        let resized = self.resizing.take().filter(|p| p.started).is_some();
        let Some((x, y, w, h)) = self.live else { return };
        if resized {
            super::set_geometry(self.id, None, (x, y), (w, h), cx);
            self.live = None;
            return;
        }
        if !moved {
            self.live = None;
            return;
        }
        // Dropped off this output (the pointer crossed to another one):
        // the shelf moves there.
        let pointer = window.mouse_position();
        let (gx, gy) = (self.origin.0 + f32::from(pointer.x), self.origin.1 + f32::from(pointer.y));
        if let Some((name, nx, ny)) = self.other_output_at(gx, gy, (x, y), cx) {
            super::set_geometry(self.id, Some(name), (nx, ny), (w, h), cx);
            self.live = None;
            return;
        }
        let to = cfg_stash::snap_shelf(x, y, w, h, self.out.0, self.out.1, SNAP_PX, EDGE_INSET);
        if super::reduce_motion() || to == (x, y) {
            self.place((to.0, to.1, w, h), window);
            super::set_geometry(self.id, None, to, (w, h), cx);
            self.live = None;
        } else {
            self.settle = Some((Instant::now(), (x, y), to));
            window.request_animation_frame();
        }
    }

    /// The output under a point beyond this one (layout coordinates), and
    /// where the card lands there.
    fn other_output_at(&self, gx: f32, gy: f32, card: (f32, f32), cx: &App) -> Option<(String, f32, f32)> {
        if gx >= 0.0 && gy >= 0.0 && gx < self.out.0 && gy < self.out.1 {
            return None;
        }
        let all = super::outputs(cx);
        let here = all.iter().find(|o| Some(&o.name) == self.output.as_ref())?;
        let (lx, ly) = (here.x as f32 + gx, here.y as f32 + gy);
        let there = all.iter().find(|o| o.has_size() && lx >= o.x as f32 && ly >= o.y as f32 && lx < (o.x + o.width) as f32 && ly < (o.y + o.height) as f32)?;
        let (cx_, cy_) = (here.x as f32 + card.0 - there.x as f32, here.y as f32 + card.1 - there.y as f32);
        Some((there.name.clone(), cx_.max(0.0), cy_.max(0.0)))
    }

    fn step_settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((at, from, to)) = self.settle else { return };
        let Some((_, _, w, h)) = self.live else {
            self.settle = None;
            return;
        };
        let t = at.elapsed().as_secs_f32() / SNAP_FOR.as_secs_f32();
        if t >= 1.0 {
            self.settle = None;
            self.place((to.0, to.1, w, h), window);
            super::set_geometry(self.id, None, to, (w, h), cx);
            self.live = None;
            return;
        }
        let k = 1.0 - (1.0 - t).powi(3);
        self.place((from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k, w, h), window);
        window.request_animation_frame();
    }

    fn key(&mut self, e: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let id = self.id;
        let ks = &e.keystroke;
        let ctrl = ks.modifiers.control || ks.modifiers.platform;
        let selected = entity(cx).read(cx).ui.get(&id).map(|u| u.selected.clone()).unwrap_or_default();
        match ks.key.as_str() {
            "space" => {
                if super::quicklook::is_open(cx) {
                    super::quicklook::close(cx);
                } else if let Some(item) = selected.last() {
                    super::quicklook::open(id, *item, cx);
                }
            }
            "escape" => {
                if super::quicklook::is_open(cx) {
                    super::quicklook::close(cx);
                } else {
                    ui(id, cx, |u| {
                        u.selected.clear();
                        u.menu = None;
                        u.shelf_menu = false;
                        u.confirm_close = false;
                    });
                }
            }
            "delete" | "backspace" if !selected.is_empty() => super::remove_items(id, &selected, cx),
            "enter" => {
                for item in items_of(id, &selected, cx) {
                    super::open_item(&item, id, cx);
                }
            }
            "a" if ctrl => {
                let all: Vec<u64> = entity(cx).read(cx).state.shelf(id).map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_default();
                ui(id, cx, |u| u.selected = all);
            }
            "c" if ctrl && !selected.is_empty() => super::copy(id, &selected, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Hovering still on a tile shows its preview; leaving it hides one
    /// opened that way.
    fn hover_tile(&mut self, item: u64, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hold.as_ref().is_some_and(|(i, _)| *i == item) {
                return;
            }
            let shelf = self.id;
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOLD_FOR_PREVIEW).await;
                let _ = this.update(cx, |this, cx| {
                    this.held = Some(item);
                    super::quicklook::open(shelf, item, cx);
                });
            });
            self.hold = Some((item, task));
        } else if self.hold.as_ref().is_some_and(|(i, _)| *i == item) {
            self.hold = None;
            if self.held.take() == Some(item) {
                super::quicklook::close(cx);
            }
        }
    }
}

/// Runs `f` on a shelf's view state.
fn ui(shelf: u64, cx: &mut App, f: impl FnOnce(&mut super::ShelfUi)) {
    entity(cx).update(cx, |s, cx| {
        f(s.ui.entry(shelf).or_default());
        cx.notify();
    });
}

fn items_of(shelf: u64, ids: &[u64], cx: &App) -> Vec<Item> {
    entity_ref(cx).and_then(|e| e.read(cx).state.shelf(shelf).map(|s| s.items.iter().filter(|i| ids.contains(&i.id)).cloned().collect())).unwrap_or_default()
}

/// The payload of a drag out: every MIME type the items offer.
fn payload(d: &DragItems, cx: &mut App) -> Option<ExternalDragPayload> {
    let e = entity_ref(cx)?;
    let offers = {
        let s = e.read(cx);
        let sh = s.state.shelf(d.shelf)?;
        let items: Vec<&Item> = sh.items.iter().filter(|i| d.ids.contains(&i.id)).collect();
        if items.is_empty() {
            return None;
        }
        super::model::drag_offers(&items, |b| store::blob_path(d.shelf, b))
    };
    let data = DragData::new(offers.into_iter().map(|(mime, content)| DragOffer {
        mime_type: mime.into(),
        content: match content {
            super::model::OfferContent::Text(t) => DragContent::Bytes(Arc::from(t.into_bytes())),
            super::model::OfferContent::File(p) => DragContent::File(p),
        },
    }));
    let ids = d.ids.to_vec();
    e.update(cx, |s, _| s.drag_out = Some((d.shelf, ids)));
    tracing::info!("stash: dragging {} item(s) out of shelf {}: {:?}", d.ids.len(), d.shelf, data.offers().iter().map(|o| o.mime_type.as_ref()).collect::<Vec<_>>());
    Some(ExternalDragPayload::Data(data))
}

fn soft_shadow(_p: &MandaraPalette, alpha: f32) -> Vec<BoxShadow> {
    vec![BoxShadow { color: gpui::black().opacity(alpha), offset: point(px(ZERO), px(SHADOW_Y)), blur_radius: px(SHADOW_BLUR), spread_radius: px(SHADOW_SPREAD), inset: false }]
}

fn badge(n: usize, p: &MandaraPalette) -> impl IntoElement {
    div()
        .h(px(BADGE_H))
        .min_w(px(BADGE_H))
        .px(px(BADGE_PAD_X))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(p.primary.hsla())
        .text_color(p.on_primary.hsla())
        .text_size(px(text::CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(crate::fonts::tabular_nums())
        .child(n.to_string())
}

fn small_button(id: (&'static str, u64), glyph: &'static str, p: &MandaraPalette, on_click: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    // Its press stays here: the card's press-outside closes menus, and the
    // menu button toggles its own.
    div()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(crate::kit::icon_button(gpui::ElementId::from((id.0, id.1 as usize)), glyph, 26.0, p.on_surface_variant, None, on_click))
}

/// The subtitle under a tile's name.
fn subtitle(item: &Item) -> SharedString {
    match &item.kind {
        ItemKind::File { dir: true, .. } => t("Folder").into(),
        ItemKind::File { mime, .. } => short_type(mime).into(),
        ItemKind::Image { mime, .. } => short_type(mime).into(),
        ItemKind::Text { chars, .. } => tf("{count} characters", &[("count", chars)]).into(),
        ItemKind::Url { url, .. } => super::model::host_of(url).unwrap_or(url).to_string().into(),
        ItemKind::Data { mime, bytes, .. } => format!("{} · {}", short_type(mime), human_bytes(*bytes)).into(),
    }
}

fn short_type(mime: &str) -> String {
    let sub = mime.split('/').nth(1).unwrap_or(mime);
    let sub = sub.split([';', '+']).next().unwrap_or(sub);
    let sub = sub.strip_prefix("x-").unwrap_or(sub);
    let sub = sub.strip_prefix("vnd.").unwrap_or(sub);
    sub.rsplit('.').next().unwrap_or(sub).to_uppercase()
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

impl Render for ShelfWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        self.step_settle(window, cx);
        let id = self.id;
        let Some((x, y, w, h)) = self.card(cx) else { return div().into_any_element() };
        let e = entity(cx);
        let now = Instant::now();
        let reduce = super::reduce_motion();
        let scale = window.scale_factor();
        // Card position inside the surface.
        let (left, top) = (x - self.origin.0, y - self.origin.1);
        let card = Bounds::new(point(px(left), px(top)), size(px(w), px(h)));
        if self.input != Some(card) {
            self.input = Some(card);
            window.set_input_region(Some(&[card]));
        }
        let cfg = crate::state::config(cx).read(cx).get();
        let (fill, _) = crate::overlays::panel_fill(cfg, &p, false);
        let (view, count, move_out) = e.read(cx).state.shelf(id).map(|s| (s.view, s.items.len(), s.move_out)).unwrap_or((ShelfView::Grid, 0, false));
        // View state, with finished animations dropped.
        let (opened, poof, busy, note, confirm, shelf_menu, menu, drag_over, selected_n, origin, over) = e.update(cx, |s, _| {
            s.thumbs.frame();
            let over = s.over_limit;
            let ui = s.ui.entry(id).or_default();
            ui.scale = scale;
            ui.landing.retain(|_, (at, _)| now.duration_since(*at) < LAND_FOR);
            if ui.poof.as_ref().is_some_and(|(at, _)| now.duration_since(*at) >= POOF_FOR) {
                ui.poof = None;
            }
            if ui.opened.is_some_and(|at| now.duration_since(at) >= OPEN_FOR) {
                ui.opened = None;
            }
            (ui.opened, ui.poof.clone(), ui.busy, ui.note.clone(), ui.confirm_close, ui.shelf_menu, ui.menu, ui.drag_over, ui.selected.len(), ui.origin, over)
        });
        let animating = !reduce && (opened.is_some() || poof.is_some() || e.read(cx).ui.get(&id).is_some_and(|u| !u.landing.is_empty()));
        if animating || self.settle.is_some() {
            window.request_animation_frame();
        }
        // Entrance: a spring from below (or from the edge it sprang from).
        let (enter_dx, enter_dy, enter_alpha) = match opened.filter(|_| !reduce) {
            Some(at) => {
                let (d, _) = OPEN_SPRING.solve(1.0, 0.0, now.duration_since(at).as_secs_f32());
                let (ux, uy) = match origin {
                    Some(Origin::Edge(cfg_stash::ScreenEdge::Left)) => (-1.0, 0.0),
                    Some(Origin::Edge(cfg_stash::ScreenEdge::Right)) => (1.0, 0.0),
                    Some(Origin::Edge(cfg_stash::ScreenEdge::Top)) => (0.0, -1.0),
                    _ => (0.0, 1.0),
                };
                (ux * 40.0 * d, uy * 40.0 * d, (1.0 - d).clamp(0.0, 1.0))
            }
            None => (0.0, 0.0, 1.0),
        };
        let rim = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        let mut shadows = soft_shadow(&p, 0.32);
        shadows.extend(rim);
        let glow = p.primary;
        let body_h = h - HEADER_H;
        let body = match (count, view) {
            (0, _) => empty_state(&p, busy > 0).into_any_element(),
            (_, ShelfView::Grid) => self.grid(w, body_h, scale, &p, cx),
            (_, ShelfView::List) => self.list(w, body_h, scale, &p, cx),
            (_, ShelfView::Stack) => self.stack(count, w, body_h, scale, &p, cx),
        };
        let header = self.header(count, view, busy, selected_n, move_out, &p, cx);
        let this = cx.entity().downgrade();
        let this_up = this.clone();
        let this_drop = this.clone();
        let dragging = self.moving.is_some() || self.resizing.is_some();
        let listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                // Moves and resizes follow the pointer anywhere (an implicit
                // grab keeps it on this surface while the button is held).
                if dragging {
                    let this = this.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase == gpui::DispatchPhase::Capture {
                            let _ = this.update(cx, |v, cx| v.on_move(e, window, cx));
                        }
                    });
                    let this = this_up.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                        if phase == gpui::DispatchPhase::Capture {
                            let _ = this.update(cx, |v, cx| v.on_up(window, cx));
                        }
                    });
                }
                // The end of a drag out (moving items out removes them) and
                // a drag leaving without a drop (an edge-born shelf retracts).
                let this = this_drop.clone();
                window.on_mouse_event(move |e: &FileDropEvent, phase, _, cx| {
                    if phase != gpui::DispatchPhase::Bubble {
                        return;
                    }
                    let _ = this.update(cx, |v, cx| v.on_file_drop(e, cx));
                });
            },
        )
        .absolute()
        .size_full();
        let card_el = div()
            .id("stash-card")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .absolute()
            .left(px(left + enter_dx))
            .top(px(top + enter_dy))
            .w(px(w))
            .h(px(h))
            .opacity(enter_alpha)
            .rounded(px(crate::style::radius(CARD_RADIUS)))
            .bg(fill)
            .shadow(shadows)
            .when(drag_over, |el| el.border_2().border_color(glow.with_alpha(0.9).hsla()))
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                // A press anywhere else closes an open menu (menus and the
                // header's buttons keep their own presses).
                let id = this.id;
                let open = entity(cx).read(cx).ui.get(&id).is_some_and(|u| u.menu.is_some() || u.shelf_menu);
                if open {
                    ui(id, cx, |u| {
                        u.menu = None;
                        u.shelf_menu = false;
                    });
                }
            }))
            .child(mandara_core::grain_overlay())
            .child(header)
            .child(div().flex_1().min_h(px(0.0)).relative().child(body))
            .when_some(note, |el, n| el.child(note_bar(id, n, &p)))
            .when_some(over, |el, bytes| el.child(over_bar(bytes, &p)))
            .child(grip(&p, cx))
            .when(shelf_menu, |el| el.child(shelf_menu_el(id, move_out, count, &p)))
            .when_some(menu, |el, (item, at)| el.when_some(items_of(id, &[item], cx).into_iter().next(), |el, it| el.child(item_menu(id, &it, point(at.x - px(left), at.y - px(top)), w, h, &p))))
            .when(confirm, |el| el.child(confirm_close(id, count, &p)))
            .when_some(poof.filter(|_| !reduce), |el, (at, centres)| el.child(poof_el(now.duration_since(at), &centres, (w, h), &p)))
            // Drops land here (files by reference, other data read off the
            // UI thread), and flip the glow while a drag is over the card.
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_2().border_color(glow.hsla()))
            .drag_over::<ExternalData>(move |s, _, _, _| s.border_2().border_color(glow.hsla()))
            .on_drag_move::<ExternalPaths>(cx.listener(|this, ev: &gpui::DragMoveEvent<ExternalPaths>, _, cx| this.drag_moved(ev.bounds.contains(&ev.event.position), cx)))
            .on_drag_move::<ExternalData>(cx.listener(|this, ev: &gpui::DragMoveEvent<ExternalData>, _, cx| this.drag_moved(ev.bounds.contains(&ev.event.position), cx)))
            .on_drop(move |paths: &ExternalPaths, window, cx| {
                ui(id, cx, |u| u.drag_over = false);
                if entity(cx).read(cx).drag_out.as_ref().is_some_and(|(s, _)| *s == id) {
                    return;
                }
                super::drop_paths(id, paths, Some(window.mouse_position()), cx);
            })
            .on_drop(move |data: &ExternalData, window, cx| {
                ui(id, cx, |u| u.drag_over = false);
                if entity(cx).read(cx).drag_out.as_ref().is_some_and(|(s, _)| *s == id) {
                    return;
                }
                super::drop_data(id, data, Some(window.mouse_position()), cx);
            });
        let shake = self.output.as_deref().and_then(|o| super::shake_hook(o, self.origin));
        div().size_full().relative().child(card_el).child(listeners).children(shake).into_any_element()
    }
}

impl ShelfWindow {
    /// A drag moved over the card (or off it).
    fn drag_moved(&mut self, inside: bool, cx: &mut Context<Self>) {
        let id = self.id;
        let e = entity(cx);
        let changed = e.update(cx, |s, _| {
            let ui = s.ui.entry(id).or_default();
            let was = ui.drag_over;
            ui.drag_over = inside;
            if inside {
                ui.retract = None;
            }
            was != inside
        });
        if changed {
            e.update(cx, |_, cx| cx.notify());
        }
    }

    fn on_file_drop(&mut self, ev: &FileDropEvent, cx: &mut Context<Self>) {
        let id = self.id;
        let e = entity(cx);
        match ev {
            FileDropEvent::Delivered => {
                let out = e.read(cx).drag_out.clone();
                let move_out = e.read(cx).state.shelf(id).is_some_and(|s| s.move_out);
                if let Some((shelf, ids)) = out.filter(|(s, _)| *s == id)
                    && move_out
                {
                    tracing::info!("stash: moved {} item(s) out of shelf {shelf}", ids.len());
                    super::remove_items(shelf, &ids, cx);
                }
            }
            FileDropEvent::Ended => e.update(cx, |s, _| s.drag_out = None),
            FileDropEvent::Exited => {
                let edge_born = e.update(cx, |s, cx| {
                    let empty = s.state.shelf(id).is_some_and(|sh| sh.items.is_empty());
                    let ui = s.ui.entry(id).or_default();
                    ui.drag_over = false;
                    cx.notify();
                    empty && matches!(ui.origin, Some(Origin::Edge(_))) && ui.busy == 0
                });
                if edge_born {
                    let task = cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(RETRACT_AFTER).await;
                        cx.update(|cx| {
                            let still = entity_ref(cx).is_some_and(|e| {
                                let s = e.read(cx);
                                s.state.shelf(id).is_some_and(|sh| sh.items.is_empty()) && s.ui.get(&id).is_some_and(|u| u.retract.is_some() && u.busy == 0)
                            });
                            if still {
                                tracing::info!("stash: edge shelf {id} retracts (the drag went elsewhere)");
                                super::close(id, true, cx);
                            }
                        });
                    });
                    e.update(cx, |s, _| s.ui.entry(id).or_default().retract = Some(task));
                }
            }
            _ => {}
        }
    }

    fn header(&self, count: usize, view: ShelfView, busy: u32, selected: usize, move_out: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = self.id;
        let next_view = match view {
            ShelfView::Grid => ShelfView::List,
            ShelfView::List => ShelfView::Stack,
            ShelfView::Stack => ShelfView::Grid,
        };
        let view_glyph = match view {
            ShelfView::Grid => "grid",
            ShelfView::List => "list",
            ShelfView::Stack => "layers",
        };
        let all: Rc<[u64]> = entity(cx).read(cx).state.shelf(id).map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_else(|| Rc::from(Vec::new()));
        let title: SharedString = if busy > 0 {
            t("Adding…").into()
        } else if selected > 0 {
            tf("{count} selected", &[("count", &selected)]).into()
        } else {
            t("Shelf").into()
        };
        let all_title: SharedString = tf("{count} items", &[("count", &count)]).into();
        let handle = div()
            .id("stash-handle")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .row()
            .items_center()
            .gap(px(tokens::SM))
            .cursor(gpui::CursorStyle::OpenHand)
            .child(div().w(px(HANDLE_W)).h(px(HANDLE_H)).rounded_full().bg(p.on_surface_variant.with_alpha(0.35).hsla()))
            .child(div().truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(title))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let Some(card) = this.card(cx) else { return };
                    let at = (f32::from(e.position.x), f32::from(e.position.y));
                    // Where in the card it was grabbed, kept while it moves.
                    let grab = (at.0 - (card.0 - this.origin.0), at.1 - (card.1 - this.origin.1));
                    this.moving = Some(Press { at, grab, card, started: false });
                    this.settle = None;
                    cx.notify();
                }),
            );
        let count_badge = div()
            .id("stash-count")
            .when(count > 0, |el| {
                el.cursor(gpui::CursorStyle::OpenHand)
                    .on_drag(DragItems { shelf: id, ids: all.clone(), title: all_title.clone() }, |d, _, _, cx| cx.new(|_| Ghost { count: d.ids.len(), title: d.title.clone() }))
                    .external_drag_payload(|d: &DragItems, _, cx| payload(d, cx))
            })
            .when(count > 0, |el| el.child(badge(count, p)));
        div()
            .h(px(HEADER_H))
            .flex_none()
            .px(px(tokens::MD))
            .row()
            .items_center()
            .gap(px(tokens::XS))
            .child(handle)
            .child(count_badge)
            .when(move_out, |el| el.child(icon("arrow-up", 12.0, p.tertiary)))
            .child(small_button(("stash-view", id), view_glyph, p, move |_, cx| super::set_view(id, next_view, cx)))
            .child(small_button(("stash-more", id), "chevron-down", p, move |_, cx| ui(id, cx, |u| {
                u.shelf_menu = !u.shelf_menu;
                u.menu = None;
            })))
            .child(small_button(("stash-min", id), "minus", p, move |_, cx| super::minimize(id, cx)))
            .child(small_button(("stash-close", id), "close", p, move |_, cx| {
                let empty = entity(cx).read(cx).state.shelf(id).is_none_or(|s| s.items.is_empty());
                if empty {
                    super::close(id, true, cx);
                } else {
                    ui(id, cx, |u| u.confirm_close = true);
                }
            }))
            .into_any_element()
    }

    /// Tile data for items `range` of the shelf, with their pictures.
    fn tiles(id: u64, range: std::ops::Range<usize>, edge: u32, cx: &mut App) -> Vec<TileData> {
        let now = Instant::now();
        let e = entity(cx);
        let tiles = e.update(cx, |s, _| {
            let Some(sh) = s.state.shelf(id) else { return Vec::new() };
            let ui = s.ui.get(&id);
            let mut out = Vec::with_capacity(range.len());
            for index in range.clone() {
                let Some(item) = sh.items.get(index) else { break };
                let key = Key { item: item.id, w: edge, h: edge, cover: true };
                let picture = |path: PathBuf, thumbs: &mut super::thumbs::Thumbs| match thumbs.get(id, key, &path) {
                    Some(Thumb::Ready(i)) => Some(i.clone()),
                    _ => None,
                };
                let (look, missing) = match &item.kind {
                    ItemKind::Image { blob, mime, .. } if mime == "image/svg+xml" => (Look::Vector(store::blob_path(id, blob)), false),
                    ItemKind::Image { blob, .. } => (Look::Picture(picture(store::blob_path(id, blob), &mut s.thumbs)), false),
                    ItemKind::File { path, mime, .. } if mime.starts_with("image/") && mime != "image/svg+xml" => (Look::Picture(picture(path.clone(), &mut s.thumbs)), false),
                    ItemKind::File { path, mime, .. } if mime == "image/svg+xml" => (Look::Vector(path.clone()), false),
                    ItemKind::File { dir, .. } => (Look::Icon(s.icons.get(&item.id).cloned().flatten(), if *dir { "folder" } else { "note" }), false),
                    ItemKind::Text { preview, .. } => (Look::Text(super::model::preview_of(preview).into()), false),
                    ItemKind::Url { url, icon, .. } => (
                        Look::Link { host: super::model::host_of(url).unwrap_or(url).trim_start_matches("www.").to_string().into(), icon: icon.as_ref().and_then(|b| picture(store::blob_path(id, b), &mut s.thumbs)) },
                        false,
                    ),
                    ItemKind::Data { .. } => (Look::Icon(s.icons.get(&item.id).cloned().flatten(), "box"), false),
                };
                let landing = ui.and_then(|u| u.landing.get(&item.id)).map(|(at, from)| {
                    let (d, _) = LAND_SPRING.solve(1.0, 0.0, now.duration_since(*at).as_secs_f32());
                    (d, from.map_or((0.0, 40.0), |p| (f32::from(p.x), f32::from(p.y))))
                });
                out.push(TileData {
                    id: item.id,
                    index,
                    title: item.title().into(),
                    subtitle: subtitle(item),
                    look,
                    selected: ui.is_some_and(|u| u.selected.contains(&item.id)),
                    landing,
                    missing,
                });
            }
            out
        });
        super::kick_thumbs(cx);
        tiles
    }

    fn grid(&self, w: f32, body_h: f32, scale: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = self.id;
        let cols = (((w - 2.0 * BODY_PAD + TILE_GAP) / (TILE_W + TILE_GAP)).floor() as usize).max(1);
        let count = entity(cx).read(cx).state.shelf(id).map_or(0, |s| s.items.len());
        let rows = count.div_ceil(cols);
        let edge = (THUMB * scale).round() as u32;
        let p = p.clone();
        let scroll = self.scroll.clone();
        let this = cx.entity().downgrade();
        let spacing = (w - 2.0 * BODY_PAD - cols as f32 * TILE_W) / cols.max(1) as f32;
        uniform_list("stash-grid", rows, move |range, window, cx| {
            let scroll_y = f32::from(scroll.0.borrow().base_handle.offset().y);
            range
                .map(|row| {
                    let tiles = ShelfWindow::tiles(id, row * cols..((row + 1) * cols).min(count), edge, cx);
                    div()
                        .h(px(TILE_H + TILE_GAP))
                        .px(px(BODY_PAD))
                        .row()
                        .gap(px(spacing.max(0.0)))
                        .children(tiles.into_iter().map(|t| {
                            let col = t.index % cols;
                            // Where the tile sits in the window, to fly in from the drop.
                            let centre = (PAD + BODY_PAD + col as f32 * (TILE_W + spacing) + TILE_W / 2.0, HEADER_H + PAD + row as f32 * (TILE_H + TILE_GAP) + scroll_y + TILE_H / 2.0);
                            tile(id, t, centre, &p, this.clone(), window)
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .pt(px(BODY_PAD / 2.0))
        .h(px(body_h))
        .w_full()
        .into_any_element()
    }

    fn list(&self, _w: f32, body_h: f32, scale: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = self.id;
        let count = entity(cx).read(cx).state.shelf(id).map_or(0, |s| s.items.len());
        let edge = (LIST_ICON * scale).round() as u32;
        let p = p.clone();
        let this = cx.entity().downgrade();
        uniform_list("stash-list", count, move |range, window, cx| {
            ShelfWindow::tiles(id, range, edge, cx).into_iter().map(|t| list_row(id, t, &p, this.clone(), window)).collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .px(px(BODY_PAD / 2.0))
        .h(px(body_h))
        .w_full()
        .into_any_element()
    }

    /// A fanned pile of the newest few; a click spreads it into the grid.
    fn stack(&self, count: usize, w: f32, body_h: f32, scale: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = self.id;
        let shown = count.min(4);
        let card = (w * 0.5).clamp(110.0, 170.0);
        let edge = (card * scale).round() as u32;
        let tiles = ShelfWindow::tiles(id, count - shown..count, edge, cx);
        let all: Rc<[u64]> = entity(cx).read(cx).state.shelf(id).map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_else(|| Rc::from(Vec::new()));
        let title: SharedString = tf("{count} items", &[("count", &count)]).into();
        let (cx0, cy0) = ((w - card) / 2.0, (body_h - card) / 2.0 - 10.0);
        div()
            .id("stash-stack")
            .size_full()
            .relative()
            .cursor_pointer()
            .on_drag(DragItems { shelf: id, ids: all, title }, |d, _, _, cx| cx.new(|_| Ghost { count: d.ids.len(), title: d.title.clone() }))
            .external_drag_payload(|d: &DragItems, _, cx| payload(d, cx))
            .on_click(move |_, _, cx| super::set_view(id, ShelfView::Grid, cx))
            .children(tiles.into_iter().enumerate().map(|(i, t)| {
                let k = (shown - 1 - i) as f32;
                let off = k * 7.0;
                let fade = 1.0 - k * 0.14;
                let landing = t.landing.map_or(0.0, |(d, _)| d);
                div()
                    .absolute()
                    .left(px(cx0 + off * if i.is_multiple_of(2) { 1.0 } else { -1.0 }))
                    .top(px(cy0 - off + landing * 50.0))
                    .size(px(card))
                    .opacity(fade * (1.0 - landing).clamp(0.0, 1.0).max(0.15))
                    .rounded(px(crate::style::radius(16.0)))
                    .overflow_hidden()
                    .bg(p.surface_container_high.hsla())
                    .shadow(soft_shadow(p, 0.22))
                    .child(look_el(&t.look, card, p))
            }))
            .child(div().absolute().bottom(px(STACK_BADGE_INSET)).left_0().right_0().flex().justify_center().child(badge(count, p)))
            .into_any_element()
    }
}

/// What a tile shows inside its picture box.
fn look_el(look: &Look, side: f32, p: &MandaraPalette) -> AnyElement {
    match look {
        Look::Picture(Some(image)) => img(ImageSource::Render(image.clone())).size_full().object_fit(ObjectFit::Cover).into_any_element(),
        Look::Picture(None) => div().size_full().bg(p.surface_container_high.hsla()).flex().items_center().justify_center().child(icon("image", side * 0.32, p.on_surface_variant.with_alpha(0.5))).into_any_element(),
        Look::Vector(path) => div().size_full().p(px(side * 0.12)).child(img(crate::icon_raster::source(path, side * VECTOR_FRAC)).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
        Look::Icon(Some(path), _) => div().size_full().flex().items_center().justify_center().child(img(crate::icon_raster::source(path, side * ICON_FRAC)).size(px(side * ICON_FRAC))).into_any_element(),
        Look::Icon(None, glyph) => div().size_full().flex().items_center().justify_center().child(icon(glyph, side * 0.44, p.primary)).into_any_element(),
        Look::Text(preview) => div()
            .size_full()
            .p(px(side * 0.09))
            .bg(p.surface_container_high.lighten(0.03).hsla())
            .overflow_hidden()
            .text_size(px((side * 0.12).clamp(7.5, 11.0)))
            .line_height(px((side * 0.15).clamp(9.0, 14.0)))
            .text_color(p.on_surface_variant.hsla())
            .child(preview.clone())
            .into_any_element(),
        Look::Link { host, icon: favicon } => div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(LINK_GAP))
            .bg(mandara_core::oklab_gradient(160.0, p.primary.with_alpha(0.22), p.tertiary.with_alpha(0.14)))
            .child(match favicon {
                Some(image) => img(ImageSource::Render(image.clone())).size(px(side * 0.42)).rounded(px(FAVICON_RADIUS)).into_any_element(),
                None => monogram(host, side * 0.42, p),
            })
            .when(side > 60.0, |el| el.child(div().max_w(px(side - LINK_HOST_INSET)).truncate().text_size(px(LINK_HOST_TEXT)).text_color(p.on_surface_variant.hsla()).child(host.clone())))
            .into_any_element(),
    }
}

/// A site's first letter on a disc tinted by its name (no network needed).
fn monogram(host: &str, side: f32, p: &MandaraPalette) -> AnyElement {
    let name = host.trim_start_matches("www.");
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "·".into());
    let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))) % 360;
    let tint = p.primary.rotate_hue(hue as f32);
    div()
        .size(px(side))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(tint.hsla())
        .text_color(tint.best_on(p.on_primary, p.on_surface).hsla())
        .text_size(px(side * 0.5))
        .font_weight(FontWeight::BOLD)
        .child(letter)
        .into_any_element()
}

/// One grid tile.
fn tile(shelf: u64, t: TileData, centre: (f32, f32), p: &MandaraPalette, this: gpui::WeakEntity<ShelfWindow>, _window: &mut Window) -> AnyElement {
    let id = t.id;
    let (dx, dy, alpha) = match t.landing {
        Some((d, from)) => {
            let (vx, vy) = ((from.0 - centre.0).clamp(-LAND_REACH, LAND_REACH), (from.1 - centre.1).clamp(-LAND_REACH, LAND_REACH));
            (vx * d, vy * d, (1.0 - d.abs()).clamp(0.0, 1.0).max(0.2))
        }
        None => (0.0, 0.0, 1.0),
    };
    let lift = gpui::black().opacity(0.28);
    let ring = p.primary;
    let hover_bg = p.on_surface.with_alpha(0.06);
    let title = t.title.clone();
    let this_hover = this.clone();
    div()
        .id(("stash-tile", id as usize))
        .relative()
        .left(px(dx))
        .top(px(dy))
        .opacity(alpha)
        .w(px(TILE_W))
        .h(px(TILE_H))
        .p(px(TILE_PAD))
        .rounded(px(crate::style::radius(14.0)))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(TILE_INNER_GAP))
        .when(t.selected, |el| el.bg(ring.with_alpha(0.16).hsla()))
        .hover(move |s| s.bg(hover_bg.hsla()).top(px(dy - 2.0)))
        .child(
            div()
                .size(px(THUMB))
                .flex_none()
                .rounded(px(crate::style::radius(THUMB_RADIUS)))
                .overflow_hidden()
                .bg(p.surface_container.hsla())
                .shadow(vec![BoxShadow { color: lift, offset: point(px(ZERO), px(THUMB_SHADOW_Y)), blur_radius: px(THUMB_SHADOW_BLUR), spread_radius: px(THUMB_SHADOW_SPREAD), inset: false }])
                .when(t.selected, |el| el.border_2().border_color(ring.hsla()))
                .when(t.missing, |el| el.opacity(0.4))
                .child(look_el(&t.look, THUMB, p)),
        )
        .child(div().w_full().truncate().text_center().text_size(px(text::CAPTION)).text_color(p.on_surface.hsla()).child(t.title))
        .on_hover(move |hovered, _, cx| {
            let _ = this_hover.update(cx, |v, cx| v.hover_tile(id, *hovered, cx));
        })
        .on_click(move |e: &ClickEvent, _, cx| click_tile(shelf, id, e, cx))
        .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            ui(shelf, cx, |u| {
                u.menu = Some((id, e.position));
                u.shelf_menu = false;
                if !u.selected.contains(&id) {
                    u.selected = vec![id];
                }
            });
        })
        .on_drag(drag_value(shelf, id, title), |d, _, _, cx| cx.new(|_| Ghost { count: d.ids.len(), title: d.title.clone() }))
        .external_drag_payload(|d: &DragItems, _, cx| payload(&with_selection(d, cx), cx))
        .into_any_element()
}

/// The dragged items: the selection when the tile is in it, else the tile.
fn drag_value(shelf: u64, id: u64, title: SharedString) -> DragItems {
    DragItems { shelf, ids: Rc::from(vec![id]), title }
}

/// One list row.
fn list_row(shelf: u64, t: TileData, p: &MandaraPalette, this: gpui::WeakEntity<ShelfWindow>, _window: &mut Window) -> AnyElement {
    let id = t.id;
    let alpha = t.landing.map_or(1.0, |(d, _)| (1.0 - d.abs()).clamp(0.2, 1.0));
    let dx = t.landing.map_or(0.0, |(d, _)| d * 30.0);
    let hover_bg = p.on_surface.with_alpha(0.06);
    let ring = p.primary;
    let title = t.title.clone();
    div()
        .id(("stash-row", id as usize))
        .h(px(LIST_ROW_H))
        .relative()
        .left(px(dx))
        .opacity(alpha)
        .px(px(tokens::SM))
        .rounded(px(crate::style::radius(10.0)))
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .when(t.selected, |el| el.bg(ring.with_alpha(0.16).hsla()))
        .hover(move |s| s.bg(hover_bg.hsla()))
        .child(div().size(px(LIST_ICON)).flex_none().rounded(px(LIST_ICON_RADIUS)).overflow_hidden().bg(p.surface_container.hsla()).child(look_el(&t.look, LIST_ICON, p)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().truncate().text_size(px(text::BODY_S)).text_color(p.on_surface.hsla()).child(t.title))
                .child(div().truncate().text_size(px(text::CAPTION)).text_color(p.on_surface_variant.hsla()).child(t.subtitle)),
        )
        .on_hover(move |hovered, _, cx| {
            let _ = this.update(cx, |v, cx| v.hover_tile(id, *hovered, cx));
        })
        .on_click(move |e: &ClickEvent, _, cx| click_tile(shelf, id, e, cx))
        .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            ui(shelf, cx, |u| {
                u.menu = Some((id, e.position));
                u.shelf_menu = false;
                if !u.selected.contains(&id) {
                    u.selected = vec![id];
                }
            });
        })
        .on_drag(drag_value(shelf, id, title), |d, _, _, cx| cx.new(|_| Ghost { count: d.ids.len(), title: d.title.clone() }))
        .external_drag_payload(|d: &DragItems, _, cx| payload(&with_selection(d, cx), cx))
        .into_any_element()
}

/// A drag of a selected tile takes the whole selection.
fn with_selection(d: &DragItems, cx: &App) -> DragItems {
    let selected = entity_ref(cx).and_then(|e| e.read(cx).ui.get(&d.shelf).map(|u| u.selected.clone())).unwrap_or_default();
    if d.ids.len() == 1 && selected.len() > 1 && selected.contains(&d.ids[0]) {
        return DragItems { shelf: d.shelf, ids: Rc::from(selected), title: d.title.clone() };
    }
    d.clone()
}

/// Selection: a click picks, Ctrl adds or removes, Shift extends; a double
/// click opens.
fn click_tile(shelf: u64, id: u64, e: &ClickEvent, cx: &mut App) {
    if e.click_count() >= 2 {
        if let Some(item) = items_of(shelf, &[id], cx).into_iter().next() {
            super::open_item(&item, shelf, cx);
        }
        return;
    }
    let order: Vec<u64> = entity(cx).read(cx).state.shelf(shelf).map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_default();
    let m = e.modifiers();
    ui(shelf, cx, |u| {
        u.menu = None;
        u.shelf_menu = false;
        if m.control || m.platform {
            if let Some(at) = u.selected.iter().position(|i| *i == id) {
                u.selected.remove(at);
            } else {
                u.selected.push(id);
            }
        } else if m.shift
            && let Some(anchor) = u.selected.last().and_then(|a| order.iter().position(|i| i == a))
            && let Some(here) = order.iter().position(|i| *i == id)
        {
            let (a, b) = (anchor.min(here), anchor.max(here));
            u.selected = order[a..=b].to_vec();
        } else {
            u.selected = vec![id];
        }
    });
}

fn empty_state(p: &MandaraPalette, busy: bool) -> impl IntoElement {
    div()
        .size_full()
        .p(px(BODY_PAD + 4.0))
        .child(
            div()
                .size_full()
                .rounded(px(crate::style::radius(16.0)))
                .border_2()
                .border_dashed()
                .border_color(p.on_surface_variant.with_alpha(0.25).hsla())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(tokens::SM))
                .child(div().size(px(EMPTY_BADGE)).rounded_full().bg(p.primary.with_alpha(0.14).hsla()).flex().items_center().justify_center().child(icon("download", 24.0, p.primary)))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(if busy { t("Adding…") } else { t("Drop anything here") }))
                .child(div().px(px(tokens::MD)).text_center().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("Files, pictures, text and links wait here until you drag them out"))),
        )
}

fn note_bar(shelf: u64, note: String, p: &MandaraPalette) -> impl IntoElement {
    div()
        .id("stash-note")
        .absolute()
        .left(px(BANNER_INSET))
        .right(px(BANNER_INSET))
        .bottom(px(BANNER_INSET))
        .p(px(tokens::SM))
        .rounded(px(BANNER_RADIUS))
        .bg(p.urgent_coral.with_alpha(0.92).hsla())
        .text_color(p.urgent_coral.best_on(p.on_primary, p.on_surface).hsla())
        .text_size(px(text::LABEL))
        .cursor_pointer()
        .child(note)
        .on_click(move |_, _, cx| ui(shelf, cx, |u| u.note = None))
}

fn over_bar(bytes: u64, p: &MandaraPalette) -> impl IntoElement {
    div()
        .id("stash-over")
        .absolute()
        .left(px(BANNER_INSET))
        .right(px(BANNER_INSET))
        .top(px(HEADER_H))
        .p(px(tokens::SM))
        .rounded(px(BANNER_RADIUS))
        .bg(p.warning_amber.with_alpha(0.92).hsla())
        .text_color(p.warning_amber.best_on(p.on_primary, p.on_surface).hsla())
        .text_size(px(text::LABEL))
        .cursor_pointer()
        .child(tf("Open shelves hold {size}, over Stash's cache limit. Remove items or raise the limit.", &[("size", &human_bytes(bytes))]))
        .on_click(|_, _, cx| crate::state::open_studio_at(Some(format!("plugins/{}/settings", super::ID)), cx))
}

fn grip(p: &MandaraPalette, cx: &mut Context<ShelfWindow>) -> impl IntoElement {
    let dot = p.on_surface_variant.with_alpha(0.45);
    div()
        .id("stash-grip")
        .absolute()
        .right_0()
        .bottom_0()
        .size(px(GRIP))
        .cursor(gpui::CursorStyle::ResizeUpLeftDownRight)
        .children([(11.0, 5.0), (11.0, 9.0), (7.0, 9.0)].map(|(x, y)| div().absolute().left(px(x - GRIP_DOT_HALF)).top(px(y - GRIP_DOT_HALF)).size(px(GRIP_DOT)).rounded_full().bg(dot.hsla())))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let Some(card) = this.card(cx) else { return };
                this.resizing = Some(Press { at: (f32::from(e.position.x), f32::from(e.position.y)), grab: (0.0, 0.0), card, started: false });
                cx.notify();
            }),
        )
}

fn menu_box(p: &MandaraPalette) -> gpui::Div {
    div()
        .absolute()
        .w(px(MENU_W))
        .p(px(MENU_PAD))
        .rounded(px(MENU_RADIUS))
        .bg(p.surface_container_high.hsla())
        .shadow(soft_shadow(p, 0.35))
        .flex()
        .flex_col()
}

fn menu_row(id: (&'static str, usize), glyph: &'static str, label: SharedString, p: &MandaraPalette, on_click: impl Fn(&mut App) + 'static) -> AnyElement {
    let hover = p.on_surface.with_alpha(0.08);
    div()
        .id(id)
        .h(px(MENU_ROW_H))
        .px(px(tokens::SM))
        .rounded(px(MENU_ROW_RADIUS))
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .text_size(px(text::BODY_S))
        .text_color(p.on_surface.hsla())
        .child(icon(glyph, 14.0, p.on_surface_variant))
        .child(label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

fn item_menu(shelf: u64, item: &Item, at: Point<Pixels>, w: f32, h: f32, p: &MandaraPalette) -> impl IntoElement {
    let id = item.id;
    let (x, y) = (f32::from(at.x).min(w - 200.0).max(4.0), f32::from(at.y).min(h - 230.0).max(HEADER_H));
    let selection = move |cx: &App| entity_ref(cx).and_then(|e| e.read(cx).ui.get(&shelf).map(|u| u.selected.clone())).filter(|s| !s.is_empty()).unwrap_or_else(|| vec![id]);
    let is_link = matches!(item.kind, ItemKind::Url { .. });
    let picture_link = matches!(&item.kind, ItemKind::Url { url, .. } if super::model::looks_like_image(url) && super::model::fetchable(url));
    let has_icon = matches!(&item.kind, ItemKind::Url { icon: Some(_), .. });
    let n = id as usize;
    menu_box(p)
        .left(px(x))
        .top(px(y))
        .child(menu_row(("stash-m-open", n), "external", t("Open").into(), p, move |cx| {
            for it in items_of(shelf, &selection(cx), cx) {
                super::open_item(&it, shelf, cx);
            }
            ui(shelf, cx, |u| u.menu = None);
        }))
        .when(!is_link, |el| {
            el.child(menu_row(("stash-m-reveal", n), "folder", t("Show in folder").into(), p, move |cx| {
                if let Some(it) = items_of(shelf, &[id], cx).into_iter().next() {
                    super::reveal(&it, shelf, cx);
                }
                ui(shelf, cx, |u| u.menu = None);
            }))
        })
        .child(menu_row(("stash-m-copy", n), "copy", t("Copy").into(), p, move |cx| {
            let ids = selection(cx);
            super::copy(shelf, &ids, cx);
            ui(shelf, cx, |u| u.menu = None);
        }))
        .child(menu_row(("stash-m-look", n), "eye", t("Quick look").into(), p, move |cx| {
            ui(shelf, cx, |u| u.menu = None);
            super::quicklook::open(shelf, id, cx);
        }))
        .when(picture_link, |el| {
            el.child(menu_row(("stash-m-fetch", n), "download", t("Fetch the picture").into(), p, move |cx| {
                ui(shelf, cx, |u| u.menu = None);
                super::fetch_picture(shelf, id, cx);
            }))
        })
        .when(is_link && !has_icon, |el| {
            el.child(menu_row(("stash-m-icon", n), "globe", t("Load the site's icon").into(), p, move |cx| {
                ui(shelf, cx, |u| u.menu = None);
                super::fetch_icon(shelf, id, cx);
            }))
        })
        .child(menu_row(("stash-m-remove", n), "trash", t("Remove from shelf").into(), p, move |cx| {
            let ids = selection(cx);
            super::remove_items(shelf, &ids, cx);
        }))
}

fn shelf_menu_el(shelf: u64, move_out: bool, count: usize, p: &MandaraPalette) -> impl IntoElement {
    let n = shelf as usize;
    menu_box(p)
        .right(px(MENU_EDGE))
        .top(px(HEADER_H - MENU_PAD))
        .child(menu_row(("stash-s-move", n), if move_out { "check" } else { "arrow-up" }, t("Move items out (don't keep them)").into(), p, move |cx| {
            super::change(cx, |s, _| {
                if let Some(sh) = s.state.shelf_mut(shelf) {
                    sh.move_out = !sh.move_out;
                }
                s.ui.entry(shelf).or_default().shelf_menu = false;
            });
        }))
        .when(count > 0, |el| {
            el.child(menu_row(("stash-s-all", n), "list-check", t("Select all").into(), p, move |cx| {
                let all: Vec<u64> = entity(cx).read(cx).state.shelf(shelf).map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_default();
                ui(shelf, cx, |u| {
                    u.selected = all;
                    u.shelf_menu = false;
                });
            }))
            .child(menu_row(("stash-s-copy", n), "copy", t("Copy everything").into(), p, move |cx| {
                let all: Vec<u64> = entity(cx).read(cx).state.shelf(shelf).map(|s| s.items.iter().map(|i| i.id).collect()).unwrap_or_default();
                super::copy(shelf, &all, cx);
                ui(shelf, cx, |u| u.shelf_menu = false);
            }))
            .child(menu_row(("stash-s-clear", n), "eraser", t("Empty the shelf").into(), p, move |cx| {
                ui(shelf, cx, |u| u.shelf_menu = false);
                let centres = vec![(160.0, 160.0)];
                super::clear(shelf, centres, cx);
            }))
        })
        .child(menu_row(("stash-s-new", n), "plus", t("New shelf").into(), p, move |cx| {
            ui(shelf, cx, |u| u.shelf_menu = false);
            super::new_shelf(None, None, Origin::Asked, cx);
        }))
        .child(menu_row(("stash-s-settings", n), "sliders", t("Stash settings").into(), p, move |cx| {
            ui(shelf, cx, |u| u.shelf_menu = false);
            crate::state::open_studio_at(Some(format!("plugins/{}/settings", super::ID)), cx);
        }))
}

fn confirm_close(shelf: u64, count: usize, p: &MandaraPalette) -> impl IntoElement {
    let button = |id: &'static str, label: SharedString, strong: bool, on_click: Box<dyn Fn(&mut App)>| {
        let bg = if strong { p.primary } else { p.surface_container_high.lighten(0.03) };
        let fg = if strong { p.on_primary } else { p.on_surface };
        div()
            .id((id, shelf as usize))
            .h(px(DIALOG_BUTTON_H))
            .px(px(tokens::MD))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(bg.hsla())
            .text_color(fg.hsla())
            .text_size(px(text::BODY_S))
            .font_weight(FontWeight::MEDIUM)
            .hover(|s| s.opacity(0.9))
            .child(label)
            .on_click(move |_, _, cx| on_click(cx))
    };
    div()
        .id("stash-confirm")
        .absolute()
        .inset_0()
        .bg(gpui::black().opacity(0.45))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .m(px(tokens::LG))
                .p(px(tokens::LG))
                .rounded(px(DIALOG_RADIUS))
                .bg(p.surface_container_high.hsla())
                .shadow(soft_shadow(p, 0.4))
                .flex()
                .flex_col()
                .gap(px(tokens::MD))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(t("Close this shelf?")))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(tf("It holds {count} items. Closed shelves can be brought back with Restore until the cache limit clears them.", &[("count", &count)])))
                .child(
                    div()
                        .row()
                        .flex_wrap()
                        .gap(px(tokens::SM))
                        .child(button("stash-c-keep", t("Keep it").into(), false, Box::new(move |cx| ui(shelf, cx, |u| u.confirm_close = false))))
                        .child(button("stash-c-empty", t("Empty & close").into(), false, Box::new(move |cx| super::close(shelf, true, cx))))
                        .child(button("stash-c-close", t("Close").into(), true, Box::new(move |cx| super::close(shelf, false, cx)))),
                ),
        )
}

/// Sparks flying out of where the items were.
fn poof_el(elapsed: Duration, centres: &[(f32, f32)], (w, h): (f32, f32), p: &MandaraPalette) -> impl IntoElement {
    let t = (elapsed.as_secs_f32() / POOF_FOR.as_secs_f32()).clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - t).powi(3);
    let centres: Vec<(f32, f32)> = if centres.is_empty() { vec![(w / 2.0, h / 2.0)] } else { centres.to_vec() };
    let colors: [Color; 3] = [p.primary, p.tertiary, p.secondary];
    div().absolute().inset_0().children(centres.into_iter().flat_map(move |(cx0, cy0)| {
        (0..POOF_SPARKS).map(move |i| {
            let a = i as f32 / POOF_SPARKS as f32 * std::f32::consts::TAU + 0.4;
            let r = 10.0 + 46.0 * ease;
            let s = (7.0 * (1.0 - t)).max(1.0);
            div()
                .absolute()
                .left(px(cx0 + a.cos() * r - s / 2.0))
                .top(px(cy0 + a.sin() * r - s / 2.0))
                .size(px(s))
                .rounded_full()
                .bg(colors[i % 3].with_alpha(1.0 - t).hsla())
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_hold_the_card_and_stay_on_the_output() {
        // In the middle: shadow room on every side.
        assert_eq!(surface_rect((500.0, 300.0), (320.0, 380.0), (2560.0, 1440.0)), ((474.0, 274.0), (372.0, 432.0)));
        // Flush with the top-left corner: the surface starts at 0.
        assert_eq!(surface_rect((8.0, 0.0), (320.0, 380.0), (2560.0, 1440.0)).0, (0.0, 0.0));
        // Against the bottom-right: kept inside the output.
        let ((ox, oy), (sw, sh)) = surface_rect((2560.0 - 320.0, 1440.0 - 380.0), (320.0, 380.0), (2560.0, 1440.0));
        assert!(ox + sw <= 2560.0 && oy + sh <= 1440.0);
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(short_type("application/vnd.oasis.opendocument.text"), "TEXT");
        assert_eq!(short_type("image/svg+xml"), "SVG");
    }
}
