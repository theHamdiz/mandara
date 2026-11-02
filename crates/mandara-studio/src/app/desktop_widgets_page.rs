//! Studio → Desktop widgets: add, arrange and style the widgets on the
//! desktop (`[desktop.widgets]`).
//!
//! * The preview is a scaled display: the wallpaper's thumbnail with every
//!   widget drawn by the shell's own painters. Each widget is its own
//!   picture ([`mandara_ui::backdrop::widgets::preview_layer`]), so dragging
//!   one only moves its picture: nothing is repainted or written while the
//!   pointer moves, and the config is written once, on release, when the
//!   widget re-anchors to the nearest of the nine anchors. A setting
//!   repaints only the widget it belongs to.
//! * The gallery lists the widget kinds of the desktop widget plugins that
//!   are on (the registry's [`mandara_plugins::DesktopWidgetType`]s,
//!   third-party plugins' too), with live thumbnails from the same
//!   painters.
//! * Sliders write at most once per frame (`slider_model::next_frame`):
//!   a fast mouse sends many pointer events a frame, each one used to copy,
//!   compare, serialise and mirror the whole config.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Bounds, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, RenderImage,
    SharedString, StatefulInteractiveElement, Styled, canvas, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::desktop_cards::{self as dc, PluginCardOptions};
use mandara_config::widget_kinds::ranges as kr;
use mandara_config::widget_style::ranges as style;
use mandara_config::desktop_widgets::{
    self as dw, Align, Anchor, Chrome, PlateShape, ShadowKind, Calendar, ClockFace, DecorShape, Decoration, Depth, DesktopWidget, DesktopWidgetsConfig, Effect, FontRole, MonitorRow, MonitorStyle, Numerals, Placement, Plate, PlaneVisualizer,
    RingProgress, Sideways, TextOptions, Tone, VisualizerShape, WeatherLayout, WidgetKind, ranges,
};
use mandara_core::ToGpui;
use mandara_i18n::{Lang, t, tf, tk, tr};
use mandara_plugins::{DesktopWidgetType, PluginRegistry};
use mandara_theme::MandaraPalette;
use mandara_ui::backdrop::paint::FontNames;
use mandara_ui::backdrop::widgets::{PreviewLayer, render_image};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::desktop_plugins::{self as dp, face_label, vis_label};
use mandara_ui::lock::paint::TextPainter;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{button, card, group_title, heading};
use super::font_picker::FontSelect;
use crate::tokens::*;

/// The preview is 16:9 like a screen, at most this wide…
const PREVIEW_W_PX: f32 = 640.0;
const PREVIEW_ASPECT: f32 = 9.0 / 16.0;
/// …and rendered at this many physical pixels per preview pixel (crisp on
/// HiDPI).
const PREVIEW_DENSITY: f32 = 2.0;
/// Gallery tiles: size, and the display they are rendered on (the widget
/// is shown at a scale that fills the tile).
const TILE_W_PX: f32 = 132.0;
const TILE_H_PX: f32 = 74.0;
const TILE_RENDER: (u32, u32) = (528, 296);
const TILE_SCALE: f32 = 1.9;
/// Opacity of the box outlines of unselected widgets on the preview.
const BOX_ALPHA: f32 = 0.35;
/// Opacity of a switched-off widget's chip.
const DISABLED_OPACITY: f32 = 0.55;
/// Track width of the sliders.
const TRACK_W_PX: f32 = DIM_220_PX;
/// Seconds per minute: clocks in the preview are repainted with the minute.
const SECS_PER_MINUTE: i64 = 60;

/// One widget's picture on the preview, and the key it was painted for.
struct Layer {
    key: u64,
    image: Arc<RenderImage>,
    geom: PreviewLayer,
}

/// The page's state (kept across renders; one Studio process).
#[derive(Default)]
struct PageState {
    selected: Option<String>,
    drag: Option<Drag>,
    painter: Option<TextPainter>,
    /// Each gallery kind's tile (by kind key) and the key it was painted for.
    tiles: HashMap<String, (u64, Arc<RenderImage>)>,
    /// Each painted widget's picture, by widget id.
    layers: HashMap<String, Layer>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The wallpaper's thumbnail `(wallpaper, thumbnail)`; one that failed
    /// isn't tried again until the wallpaper changes.
    thumb: Option<(PathBuf, PathBuf)>,
    thumb_failed: Option<PathBuf>,
    thumb_loading: bool,
    /// The font picker of the selected widget's own font.
    font_select: Option<(String, Entity<FontSelect>)>,
}

impl Global for PageState {}

/// A widget being dragged: where the pointer holds its box, the box's size
/// (preview px) and where it is now (written on release).
#[derive(Clone)]
struct Drag {
    id: String,
    grab: (f32, f32),
    size: (f32, f32),
    place: Placement,
}

fn key(v: impl std::fmt::Debug) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}

/// The shell's families as installed (the configured names fall back).
fn fonts(cx: &gpui::App) -> FontNames {
    FontNames::from(&mandara_ui::fonts::fonts(cx))
}

/// Applies `f` to widget `id` (migrating an old config first) and keeps
/// the depth clock's `[wallpaper]` keys in step.
fn edit(cx: &mut gpui::App, id: &str, f: impl FnOnce(&mut DesktopWidget) + 'static) {
    let id = id.to_string();
    update_config(cx, move |c| {
        dw::migrate_depth_clock(c);
        if let Some(w) = c.desktop.widgets.get_mut(&id) {
            f(w);
        }
        dw::sync_depth_clock(c);
    });
}

/// Adds `widgets` on top; returns the last one's id (to select it).
fn add(cx: &mut gpui::App, widgets: Vec<DesktopWidget>) -> Option<String> {
    let last = Rc::new(std::cell::RefCell::new(None::<String>));
    let out = last.clone();
    update_config(cx, move |c| {
        dw::migrate_depth_clock(c);
        for w in widgets {
            if let Some(id) = c.desktop.widgets.add_new(w) {
                *out.borrow_mut() = Some(id);
            }
        }
        dw::sync_depth_clock(c);
    });
    last.take()
}

// ── Labels ───────────────────────────────────────────────────────────────

fn row_label(r: MonitorRow) -> &'static str {
    match r {
        MonitorRow::Cpu => tk("CPU"),
        MonitorRow::CpuGraph => tk("CPU graph"),
        MonitorRow::Gpu => tk("GPU"),
        MonitorRow::Memory => tk("Memory"),
        MonitorRow::GpuPower => tk("GPU power"),
        MonitorRow::CpuTemp => tk("CPU temp"),
        MonitorRow::GpuTemp => tk("GPU temp"),
        MonitorRow::Fan => tk("Fan"),
        MonitorRow::Battery => tk("Battery"),
        MonitorRow::Disk => tk("Disk"),
        MonitorRow::Download => tk("Download"),
        MonitorRow::Upload => tk("Upload"),
        MonitorRow::NetGraph => tk("Network graph"),
    }
}

fn shape_label(s: DecorShape) -> &'static str {
    match s {
        DecorShape::Text => tk("Text"),
        DecorShape::Disc => tk("Disc"),
        DecorShape::Circle => tk("Circle"),
        DecorShape::Stripes => tk("Stripes"),
        DecorShape::Waves => tk("Waves"),
        DecorShape::Rule => tk("Rule"),
        DecorShape::Frame => tk("Frame"),
        DecorShape::Tape => tk("Washi tape"),
    }
}

/// A widget's name in the list.
fn widget_label(w: &DesktopWidget, cx: &gpui::App) -> String {
    match &w.kind {
        WidgetKind::Clock(c) => tr(face_label(c.face)),
        WidgetKind::Decoration(d) if d.shape == DecorShape::Text => {
            let first: String = d.text.lines().next().unwrap_or_default().chars().take(ranges::REPEAT_DEFAULT as usize * 3).collect();
            if first.is_empty() { tr(shape_label(d.shape)) } else { first }
        }
        WidgetKind::Decoration(d) => tr(shape_label(d.shape)),
        WidgetKind::Visualizer(v) => tr(vis_label(v.shape)),
        // A plugin's widget: its kind's name.
        WidgetKind::Card(c) if dc::parse_plugin_card(&c.card).is_some() => {
            let (plugin, id) = dc::parse_plugin_card(&c.card).unwrap_or_default();
            cx.try_global::<PluginRegistry>().and_then(|r| r.desktop_widget_type(&format!("{plugin}/{id}"))).map(|t| mandara_ui::desktop_menu::widget_type_name(t, cx)).unwrap_or_else(|| id.to_string())
        }
        _ => tr(dp::label(w).0),
    }
}

const ANCHOR_GLYPHS: [&str; 9] = ["↖", "↑", "↗", "←", "•", "→", "↙", "↓", "↘"];
const TONES: [(Tone, &str); 4] = [(Tone::Auto, tk("Automatic")), (Tone::OnSurface, tk("Text colour")), (Tone::Accent, tk("Accent")), (Tone::Custom, tk("Custom"))];
const FONTS: [(FontRole, &str); 6] =
    [(FontRole::Widgets, tk("Widgets font")), (FontRole::Display, tk("Display")), (FontRole::Ui, tk("Interface")), (FontRole::Arabic, tk("Arabic")), (FontRole::Mono, tk("Monospace")), (FontRole::Custom, tk("Choose…"))];
const EFFECTS: [(Effect, &str); 3] = [(Effect::None, tk("None")), (Effect::Shadow, tk("Shadow")), (Effect::Glow, tk("Glow"))];
const ALIGNS: [(Align, &str); 4] = [(Align::Auto, tk("With the anchor")), (Align::Start, tk("Start")), (Align::Center, tk("Centre")), (Align::End, tk("End"))];
const PLATES: [(Plate, &str); 3] = [(Plate::None, tk("None")), (Plate::Glass, tk("Glass")), (Plate::Solid, tk("Solid"))];
const SHAPES: [(PlateShape, &str); 9] = [
    (PlateShape::Rect, tk("Rectangle")),
    (PlateShape::Circle, tk("Circle")),
    (PlateShape::Squircle, tk("Squircle")),
    (PlateShape::Cookie, tk("Cookie")),
    (PlateShape::Flower, tk("Flower")),
    (PlateShape::Pentagon, tk("Pentagon")),
    (PlateShape::Chamfer, tk("Chamfer")),
    (PlateShape::Skew, tk("Slanted")),
    (PlateShape::Deckle, tk("Torn paper")),
];
const PERIODS: [(dw::Period, &str); 5] = [(dw::Period::Hour, tk("Hour")), (dw::Period::Day, tk("Day")), (dw::Period::Week, tk("Week")), (dw::Period::Month, tk("Month")), (dw::Period::Year, tk("Year"))];
const PROGRESS_STYLES: [(dw::ProgressStyle, &str); 4] = [(dw::ProgressStyle::Ring, tk("Ring")), (dw::ProgressStyle::WavyRing, tk("Wavy ring")), (dw::ProgressStyle::Bar, tk("Bar")), (dw::ProgressStyle::Dots, tk("Dots"))];
const DIAL_MARKS: [(dw::DialMarks, &str); 4] = [(dw::DialMarks::None, tk("None")), (dw::DialMarks::Dots, tk("Dots")), (dw::DialMarks::Ticks, tk("Ticks")), (dw::DialMarks::Numbers, tk("Numbers"))];
const HANDS: [(dw::HandStyle, &str); 3] = [(dw::HandStyle::Classic, tk("Classic")), (dw::HandStyle::Fill, tk("Blade")), (dw::HandStyle::Hollow, tk("Hollow"))];
const SECONDS_STYLES: [(dw::SecondsStyle, &str); 2] = [(dw::SecondsStyle::Hand, tk("Hand")), (dw::SecondsStyle::Dot, tk("Dot"))];
const DATE_STYLES: [(dw::DateStyle, &str); 3] = [(dw::DateStyle::Under, tk("Under the dial")), (dw::DateStyle::Bubble, tk("Bubble")), (dw::DateStyle::Box, tk("Box"))];
/// Weights offered for a template line.
const LINE_WEIGHTS: [(u16, &str); 5] = [(200, tk("Thin")), (300, tk("Light")), (400, tk("Regular")), (600, tk("Semibold")), (800, tk("Heavy"))];
/// Size range of a template line's text, reference px.
const LINE_SIZE_PX: (f32, f32) = (10.0, 240.0);
const SHADOWS: [(ShadowKind, &str); 3] = [(ShadowKind::None, tk("None")), (ShadowKind::Soft, tk("Soft")), (ShadowKind::Hard, tk("Hard"))];
const CHROMES: [(Chrome, &str); 4] = [(Chrome::None, tk("None")), (Chrome::Window, tk("Window")), (Chrome::Tape, tk("Washi tape")), (Chrome::Polaroid, tk("Polaroid"))];
const NUMERALS: [(Numerals, &str); 3] = [(Numerals::Auto, tk("From the language")), (Numerals::Western, tk("Western (123)")), (Numerals::EasternArabic, tk("Eastern Arabic (١٢٣)"))];
const CALENDARS: [(Calendar, &str); 4] = [(Calendar::Gregorian, tk("Gregorian")), (Calendar::Hijri, tk("Hijri")), (Calendar::Both, tk("Both")), (Calendar::None, tk("No date"))];
/// Which way vertical text turns its sideways runs (Arabic, Latin, …).
const SIDEWAYS: [(Sideways, &str); 3] = [(Sideways::Auto, tk("Toward the middle")), (Sideways::Clockwise, tk("Clockwise")), (Sideways::CounterClockwise, tk("Counter-clockwise"))];

// ── Page ─────────────────────────────────────────────────────────────────

type Set<T> = Rc<dyn Fn(&mut DesktopWidget, T)>;

impl StudioView {
    pub(super) fn desktop_widgets_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if !cx.has_global::<PageState>() {
            cx.set_global(PageState::default());
        }
        let mut cfg = cfg.clone();
        dw::migrate_depth_clock(&mut cfg);
        let items = cfg.desktop.widgets.items.clone();
        let selected = cx.global::<PageState>().selected.clone().filter(|id| items.iter().any(|w| w.id == *id));
        let preview = self.widgets_preview(&cfg, p, cx);
        let gallery = self.widgets_gallery(&cfg, p, cx);
        let list = self.widgets_list(&items, selected.as_deref(), p, cx);
        let editor = selected.as_ref().and_then(|id| items.iter().find(|w| w.id == *id)).map(|w| self.widget_editor(w, &cfg, p, cx));
        card(p)
            .child(heading(t("Desktop widgets"), t("Clocks, weather, a system monitor, decorations and more on the wallpaper — behind its subject or in front. Each kind comes from a plugin; nothing runs until you add one."), p))
            .child(
                // Arrange them on the desktop itself (also: right-click the desktop → Edit desktop).
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(button("widgets-edit-on-desktop", "pencil", t("Edit on desktop"), true, p).on_click(|_, _, cx| mandara_ui::desktop_edit::enter(cx)))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Or right-click the desktop and choose Edit desktop")))),
            )
            .child(preview)
            .child(gallery)
            .child(list)
            .children(editor)
            .into_any_element()
    }

    /// Starts loading the wallpaper's thumbnail (once per wallpaper; a
    /// failed one isn't retried).
    fn load_thumb(&mut self, cfg: &ShellConfig, cx: &mut Context<Self>) {
        let w = &cfg.wallpaper;
        let wall = mandara_config::desktop::effective_wallpaper(w.path.as_ref(), &w.variants, cfg.displays.primary.as_deref(), cfg.theme.mode.is_dark());
        let s = cx.global::<PageState>();
        let Some(path) = wall else { return };
        let have = s.thumb.as_ref().is_some_and(|(w, _)| *w == path);
        if have || s.thumb_loading || s.thumb_failed.as_ref() == Some(&path) {
            return;
        }
        cx.global_mut::<PageState>().thumb_loading = true;
        let task = cx.background_executor().spawn(async move {
            let made = mandara_ui::wallpaper::thumbnail(&path);
            (path, made.ok())
        });
        cx.spawn(async move |this, cx| {
            let (path, thumb) = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = cx.global_mut::<PageState>();
                s.thumb_loading = false;
                match thumb {
                    Some(t) => s.thumb = Some((path, t)),
                    None => s.thumb_failed = Some(path),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Paints the pictures of the widgets that changed (and drops those of
    /// widgets that went away).
    fn refresh_layers(&mut self, painted: &[DesktopWidget], frame: (u32, u32), cx: &mut Context<Self>) {
        let primary = mandara_core::theme_entity(cx).read(cx).target().primary;
        let fonts = fonts(cx);
        let minute = chrono::Local::now().timestamp().div_euclid(SECS_PER_MINUTE);
        let lang = mandara_i18n::language();
        let mut retired = Vec::new();
        let s = cx.global_mut::<PageState>();
        s.layers.retain(|id, layer| {
            let keep = painted.iter().any(|w| w.id == *id);
            if !keep {
                retired.push(layer.image.clone());
            }
            keep
        });
        for w in painted {
            let k = key((mandara_ui::backdrop::widgets::content_key(w), primary.to_rgba8(), &fonts, lang, frame, minute));
            if s.layers.get(&w.id).is_some_and(|l| l.key == k) {
                continue;
            }
            let text = s.painter.get_or_insert_with(TextPainter::new);
            match mandara_ui::backdrop::widgets::preview_layer(w, frame, &fonts, primary, text) {
                Some((rgba, geom)) => {
                    let image = render_image(rgba);
                    if let Some(old) = s.layers.insert(w.id.clone(), Layer { key: k, image, geom }) {
                        retired.push(old.image);
                    }
                }
                None => {
                    if let Some(old) = s.layers.remove(&w.id) {
                        retired.push(old.image);
                    }
                }
            }
        }
        if let Some(text) = s.painter.as_mut() {
            text.release_glyphs();
        }
        // Replaced pictures leave the GPU atlas at once.
        for image in retired {
            cx.drop_image(image, None);
        }
    }

    fn widgets_preview(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (pw, ph) = (PREVIEW_W_PX, PREVIEW_W_PX * PREVIEW_ASPECT);
        let frame = ((pw * PREVIEW_DENSITY) as u32, (ph * PREVIEW_DENSITY) as u32);
        let frame_f = (frame.0 as f32, frame.1 as f32);
        self.load_thumb(cfg, cx);
        let painted: Vec<DesktopWidget> = cfg.desktop.widgets.items.iter().filter(|w| w.enabled && w.kind.on_wallpaper_plane() && dp::shows(w, cfg, cx)).cloned().collect();
        self.refresh_layers(&painted, frame, cx);
        let s = cx.global::<PageState>();
        let thumb = s.thumb.as_ref().map(|(_, t)| t.clone());
        let (selected, drag, bounds) = (s.selected.clone(), s.drag.clone(), s.bounds.clone());
        let mut pictures = Vec::new();
        let mut boxes = Vec::new();
        for w in &painted {
            let Some(layer) = s.layers.get(&w.id) else { continue };
            // A dragged widget is drawn where the pointer has it.
            let place = drag.as_ref().filter(|d| d.id == w.id).map_or(&w.place, |d| &d.place);
            let (x, y) = layer.geom.at(place, frame_f);
            let r = &layer.geom.rect;
            let (lx, ly, lw, lh) = (x / PREVIEW_DENSITY, y / PREVIEW_DENSITY, r.w as f32 / PREVIEW_DENSITY, r.h as f32 / PREVIEW_DENSITY);
            pictures.push(img(layer.image.clone()).absolute().left(px(lx)).top(px(ly)).w(px(lw)).h(px(lh)).into_any_element());
            let (ow, oh) = (layer.geom.outer.2 / PREVIEW_DENSITY, layer.geom.outer.3 / PREVIEW_DENSITY);
            let on = selected.as_deref() == Some(w.id.as_str());
            let (grab_id, grab_place) = (w.id.clone(), w.place.clone());
            boxes.push(
                div()
                    .id(SharedString::from(format!("dw-box-{}", w.id)))
                    .absolute()
                    .left(px(lx))
                    .top(px(ly))
                    .w(px(lw.max(DIM_8_PX)))
                    .h(px(lh.max(DIM_8_PX)))
                    .rounded(px(RADIUS_6_PX))
                    .border_1()
                    .border_color(if on { p.primary.hsla() } else { p.on_surface.with_alpha(BOX_ALPHA).hsla() })
                    .when(on, |el| el.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, e: &MouseDownEvent, _, cx| {
                            let b = cx.global::<PageState>().bounds.get();
                            let (px_, py_) = (f32::from(e.position.x - b.origin.x), f32::from(e.position.y - b.origin.y));
                            let (ox, oy) = grab_place.origin((pw, ph), (ow, oh));
                            let s = cx.global_mut::<PageState>();
                            s.selected = Some(grab_id.clone());
                            s.drag = Some(Drag { id: grab_id.clone(), grab: (px_ - ox, py_ - oy), size: (ow, oh), place: grab_place.clone() });
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            );
        }
        div()
            .id("dw-preview")
            .relative()
            .w(px(pw))
            .h(px(ph))
            .flex_none()
            .rounded(px(RADIUS_14_PX))
            .overflow_hidden()
            .bg(p.surface_container_high.hsla())
            .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().inset_0())
            .when_some(thumb, |el, t| {
                use gpui::StyledImage as _;
                el.child(img(t).absolute().inset_0().object_fit(gpui::ObjectFit::Cover))
            })
            .children(pictures)
            .children(boxes)
            .on_mouse_move(cx.listener(move |_, e: &MouseMoveEvent, _, cx| {
                // Only the picture moves: nothing is painted or written.
                let Some(mut d) = cx.global::<PageState>().drag.clone() else { return };
                let b = cx.global::<PageState>().bounds.get();
                let left = f32::from(e.position.x - b.origin.x) - d.grab.0;
                let top = f32::from(e.position.y - b.origin.y) - d.grab.1;
                (d.place.x, d.place.y) = d.place.offset_for((pw, ph), d.size, (left, top));
                cx.global_mut::<PageState>().drag = Some(d);
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(move |_, _: &MouseUpEvent, _, cx| drop_drag(pw, ph, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(move |_, _: &MouseUpEvent, _, cx| drop_drag(pw, ph, cx)))
            .into_any_element()
    }

    /// A gallery tile's picture (painted kinds), cached.
    fn tile_image(&mut self, t: &DesktopWidgetType, widgets: &[DesktopWidget], cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if widgets.is_empty() || widgets.iter().any(|w| !w.kind.on_wallpaper_plane()) {
            return None;
        }
        let primary = mandara_core::theme_entity(cx).read(cx).target().primary;
        let fonts = fonts(cx);
        // Tiles that tell the time are painted again with the minute.
        let timed = widgets.iter().any(|w| matches!(w.kind, WidgetKind::Clock(_) | WidgetKind::WorldClock(_) | WidgetKind::Countdown(_) | WidgetKind::Progress(_)));
        let minute = timed.then(|| chrono::Local::now().timestamp().div_euclid(SECS_PER_MINUTE));
        let k = key((t.key(), primary.to_rgba8(), &fonts, mandara_i18n::language(), minute));
        if let Some((_, img)) = cx.global::<PageState>().tiles.get(&t.key()).filter(|(old, _)| *old == k) {
            return Some(img.clone());
        }
        let s = cx.global_mut::<PageState>();
        let text = s.painter.get_or_insert_with(TextPainter::new);
        // Centred and enlarged so it fills the tile.
        let shown: Vec<DesktopWidget> = widgets
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let mut w = w.clone();
                if widgets.len() == 1 {
                    w.place = Placement { scale: TILE_SCALE, ..Placement::default() };
                }
                w.id = format!("{}-{i}", t.key());
                w
            })
            .collect();
        let (rgba, _) = mandara_ui::backdrop::widgets::preview(&shown, TILE_RENDER, &fonts, primary, text)?;
        text.release_glyphs();
        let image = render_image(rgba);
        // The tile it replaces leaves the GPU atlas.
        if let Some((_, old)) = s.tiles.insert(t.key(), (k, image.clone())) {
            cx.drop_image(old, None);
        }
        Some(image)
    }

    fn widgets_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let kinds: Vec<DesktopWidgetType> = cx.try_global::<PluginRegistry>().map(|r| r.enabled_desktop_widgets(&cfg.plugins).cloned().collect()).unwrap_or_default();
        let has_depth = cfg.desktop.widgets.items.iter().any(|w| matches!(&w.kind, WidgetKind::Clock(c) if c.face.depth_style().is_some()));
        // Grouped by the plugin that offers them, in registration order.
        let mut groups: Vec<(String, Vec<AnyElement>)> = Vec::new();
        for t in kinds {
            let widgets = (t.make)();
            // One depth clock at a time.
            if has_depth && widgets.iter().any(|w| matches!(&w.kind, WidgetKind::Clock(c) if c.face.depth_style().is_some())) {
                continue;
            }
            let tile = self.tile_image(&t, &widgets, cx);
            let label = mandara_ui::desktop_menu::widget_type_name(&t, cx);
            // Already on the desktop (one widget, same settings): the tile
            // selects it instead of stacking an identical copy on top.
            let existing = match widgets.as_slice() {
                [only] => cfg.desktop.widgets.find_same(&only.kind).map(|w| w.id.clone()),
                _ => None,
            };
            let on_desktop = existing.is_some();
            let glyph = mandara_ui::desktop_menu::icon_named(&t.icon);
            let el = div()
                .id(SharedString::from(format!("dw-add-{}", t.key())))
                .w(px(TILE_W_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_4_PX))
                .cursor_pointer()
                .child(
                    div()
                        .w(px(TILE_W_PX))
                        .h(px(TILE_H_PX))
                        .relative()
                        .rounded(px(RADIUS_10_PX))
                        .overflow_hidden()
                        .bg(p.surface_dim.hsla())
                        .hover(|s| s.bg(p.surface_base.hsla()))
                        .row()
                        .items_center()
                        .justify_center()
                        .when_some(tile.clone(), |el, t| {
                            use gpui::StyledImage as _;
                            el.child(img(t).absolute().top_0().left_0().w(px(TILE_W_PX)).h(px(TILE_H_PX)).object_fit(gpui::ObjectFit::Contain))
                        })
                        // Cards are live views: their tile shows their icon.
                        .when(tile.is_none(), |el| el.child(mandara_ui::icons::icon(glyph, ICON_26_PX, p.primary)))
                        .when(on_desktop, |el| {
                            el.border_1().border_color(p.primary.with_alpha(ALPHA_55).hsla()).child(
                                div()
                                    .absolute()
                                    .top(px(SPACE_4_PX))
                                    .right(px(SPACE_4_PX))
                                    .size(px(DIM_18_PX))
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .justify_center()
                                    .bg(p.primary.hsla())
                                    .child(mandara_ui::icons::icon("check", ICON_11_PX, p.on_primary)),
                            )
                        }),
                )
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(if on_desktop { tf("{name} · on the desktop", &[("name", &label)]) } else { label }))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let id = match &existing {
                        Some(id) => Some(id.clone()),
                        None => add(cx, widgets.clone()),
                    };
                    cx.global_mut::<PageState>().selected = id;
                    cx.notify();
                }))
                .into_any_element();
            let plugin_name = cx.try_global::<PluginRegistry>().and_then(|r| r.get(&t.plugin)).map(|f| f.manifest.display_name()).unwrap_or_else(|| t.plugin.clone());
            match groups.iter_mut().find(|(n, _)| *n == plugin_name) {
                Some((_, tiles)) => tiles.push(el),
                None => groups.push((plugin_name, vec![el])),
            }
        }
        let full = cfg.desktop.widgets.items.len() >= ranges::MAX_WIDGETS;
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(if full { t("The list is full — remove a widget to add another") } else { t("Add a widget") }, p))
            .children(groups.into_iter().map(|(name, tiles)| {
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(div().px(px(SPACE_10_PX)).text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(name))
                    .child(div().row().flex_wrap().gap(px(SPACE_10_PX)).children(tiles))
            }))
            .child(div().px(px(SPACE_10_PX)).text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Widget kinds come from plugins: switch them on or off in Studio → Plugins (Desktop widgets)."))))
            .into_any_element()
    }

    fn widgets_list(&mut self, items: &[DesktopWidget], selected: Option<&str>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if items.is_empty() {
            return div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No widgets yet.")).into_any_element();
        }
        // Same name more than once: "Clock 2", "Clock 3" tell them apart.
        let mut seen: HashMap<String, usize> = HashMap::new();
        let names: Vec<String> = items
            .iter()
            .map(|w| {
                let base = widget_label(w, cx);
                let n = seen.entry(base.clone()).and_modify(|n| *n += 1).or_insert(1);
                if *n > 1 { format!("{base} {n}") } else { base }
            })
            .collect();
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        let duplicates = DesktopWidgetsConfig { items: items.to_vec(), ..DesktopWidgetsConfig::default() }.duplicates().len();
        let chips: Vec<AnyElement> = items
            .iter()
            .zip(names)
            .map(|(w, name)| {
                let on = selected == Some(w.id.as_str());
                let (sel, tog, del) = (w.id.clone(), w.id.clone(), w.id.clone());
                let enabled = w.enabled;
                // Its plugin is off: listed, not drawn.
                let plugin_off = !dp::shows(w, &cfg, cx);
                div()
                    .id(SharedString::from(format!("dw-item-{}", w.id)))
                    .h(px(DIM_34_PX))
                    .px(px(SPACE_10_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .bg(if on { p.primary.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
                    .when(!enabled || plugin_off, |el| el.opacity(DISABLED_OPACITY))
                    .child(
                        div()
                            .id(SharedString::from(format!("dw-on-{}", w.id)))
                            .child(mandara_ui::plugins::network::switch(enabled, p))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                edit(cx, &tog, |w| w.enabled = !w.enabled);
                                cx.stop_propagation();
                            })),
                    )
                    .child(div().text_size(px(TEXT_12_5_PX)).child(name))
                    .when(w.place.depth == Depth::Front, |el| el.child(div().text_size(px(TEXT_10_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("front"))))
                    .when(plugin_off, |el| el.child(div().text_size(px(TEXT_10_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("plugin off"))))
                    .child(
                        div()
                            .id(SharedString::from(format!("dw-del-{}", w.id)))
                            .size(px(DIM_22_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .child(mandara_ui::icons::icon("trash", ICON_12_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                let id = del.clone();
                                update_config(cx, move |c| {
                                    dw::migrate_depth_clock(c);
                                    c.desktop.widgets.items.retain(|w| w.id != id);
                                    dw::sync_depth_clock(c);
                                });
                                cx.stop_propagation();
                            })),
                    )
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.global_mut::<PageState>().selected = Some(sel.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let dedupe = (duplicates > 0).then(|| {
            button("dw-remove-duplicates", "trash", tf("Remove duplicates ({n})", &[("n", &duplicates)]), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                update_config(cx, |c| {
                    dw::migrate_depth_clock(c);
                    c.desktop.widgets.remove_duplicates();
                    dw::sync_depth_clock(c);
                });
                cx.notify();
            }))
        });
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(div().row().items_center().justify_between().gap(px(SPACE_10_PX)).child(group_title(t("On the desktop (later ones on top)"), p)).children(dedupe))
            .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(chips))
            .into_any_element()
    }

    // ── Editor ───────────────────────────────────────────────────────────

    fn widget_editor(&mut self, w: &DesktopWidget, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = w.id.clone();
        let place = vec![
            row(p, t("Depth"), choice(&id, "depth", w.place.depth, &[(Depth::Behind, tk("Behind the subject")), (Depth::Front, tk("In front"))], Rc::new(|w, v| w.place.depth = v), p, cx)),
            row(p, t("Anchor"), anchors(&id, w.place.anchor, p, cx)),
            row(p, t("Horizontal offset"), slider(&id, "x", w.place.x, ranges::OFFSET, 0.005, "", Rc::new(|w, v| w.place.x = v), p)),
            row(p, t("Vertical offset"), slider(&id, "y", w.place.y, ranges::OFFSET, 0.005, "", Rc::new(|w, v| w.place.y = v), p)),
            row(p, t("Size"), slider(&id, "scale", w.place.scale, ranges::SCALE, 0.05, "×", Rc::new(|w, v| w.place.scale = v), p)),
            row(p, t("Opacity"), slider(&id, "opacity", w.place.opacity, ranges::OPACITY, 0.05, "", Rc::new(|w, v| w.place.opacity = v), p)),
            row(p, t("Displays"), displays(&id, &w.place.displays, cfg, p, cx)),
        ];
        let mut place = place;
        if w.kind.look().is_some() {
            place.push(row(p, t("Rotation"), slider(&id, "rotation", w.place.rotation, (style::ROTATION_DEG.min, style::ROTATION_DEG.max), style::ROTATION_DEG.step, "°", Rc::new(|w, v| w.place.rotation = v), p)));
            place.push(row(p, t("Mirror left to right"), toggle(&id, "flip-x", w.place.flip_x, Rc::new(|w, v| w.place.flip_x = v), p, cx)));
            place.push(row(p, t("Mirror top to bottom"), toggle(&id, "flip-y", w.place.flip_y, Rc::new(|w, v| w.place.flip_y = v), p, cx)));
        } else {
            place.push(hint(t("Interactive cards stay upright: they can't be turned or mirrored."), p));
        }
        let content = self.content_rows(w, p, cx);
        let text = w.kind.locale().map(|l| text_rows(&id, &w.kind, l, p, cx)).unwrap_or_default();
        let look = match &w.kind {
            WidgetKind::Clock(c) if c.face.depth_style().is_some() => self.font_rows(&id, &c.look, cfg, p, cx),
            k => k.look().map(|l| self.look_rows(&id, l, cfg, p, cx)).unwrap_or_default(),
        };
        let shape = match &w.kind {
            WidgetKind::Clock(c) if c.face.depth_style().is_some() => Vec::new(),
            k => k.look().map(|l| self.shape_rows(&id, l, p, cx)).unwrap_or_default(),
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
            .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(widget_label(w, cx)))
            .child(section(t("Place"), place, p))
            .when(!content.is_empty(), |el| el.child(section(t("Content"), content, p)))
            .when(!text.is_empty(), |el| el.child(section(t("Numbers, dates and language"), text, p)))
            .when(!look.is_empty(), |el| el.child(section(t("Look"), look, p)))
            .when(!shape.is_empty(), |el| el.child(section(t("Shape and frame"), shape, p)))
            .into_any_element()
    }

    /// A clock face's own options.
    fn face_rows(&mut self, id: &str, c: &dw::ClockWidget, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let f = &c.face_options;
        let mut rows = Vec::new();
        match c.face {
            ClockFace::Analog | ClockFace::Numerals => {
                let a = &c.analog;
                if c.face == ClockFace::Analog {
                    rows.push(row(p, t("Dial"), choice(id, "dial", a.dial, &SHAPES, analog_set(|a, v| a.dial = v), p, cx)));
                    rows.push(row(p, t("Filled dial"), toggle(id, "dial-fill", a.dial_fill, analog_set(|a, v| a.dial_fill = v), p, cx)));
                    rows.push(row(p, t("Turn the dial"), slider(id, "dial-turn", a.dial_turn, (style::ROTATION_DEG.min, style::ROTATION_DEG.max), style::ROTATION_DEG.step, "°", analog_set(|a, v| a.dial_turn = v), p)));
                    rows.push(row(p, t("Hour marks"), choice(id, "marks", a.marks, &DIAL_MARKS, analog_set(|a, v| a.marks = v), p, cx)));
                    if c.seconds {
                        rows.push(row(p, t("Seconds as"), choice(id, "secstyle", a.seconds_style, &SECONDS_STYLES, analog_set(|a, v| a.seconds_style = v), p, cx)));
                    }
                    if c.date {
                        rows.push(row(p, t("Date style"), choice(id, "datestyle", a.date_style, &DATE_STYLES, analog_set(|a, v| a.date_style = v), p, cx)));
                    }
                }
                rows.push(row(p, t("Hands"), choice(id, "hands", a.hour_hand, &HANDS, analog_set(|a, v| a.hour_hand = v), p, cx)));
                rows.push(row(p, t("Hand width"), slider(id, "handw", a.hand_width, ranges::HAND_WIDTH_PX, 0.5, " px", analog_set(|a, v| a.hand_width = v), p)));
                if c.face == ClockFace::Numerals {
                    rows.push(row(p, t("Numerals strength"), slider(id, "numerals", f.numerals, ranges::NUMERALS, 0.05, "", face_set(|f, v| f.numerals = v), p)));
                }
            }
            ClockFace::Shape => {
                rows.push(row(p, t("Shape"), choice(id, "fshape", f.shape, &SHAPES, face_set(|f, v| f.shape = v), p, cx)));
                rows.push(row(p, t("Turn every minute"), toggle(id, "spin", f.spin, face_set(|f, v| f.spin = v), p, cx)));
            }
            ClockFace::DotMatrix => {
                rows.push(row(p, t("Square dots"), toggle(id, "sqdots", f.square_dots, face_set(|f, v| f.square_dots = v), p, cx)));
                rows.push(row(p, t("Unlit dots"), slider(id, "unlit", f.unlit, ranges::UNLIT, 0.01, "", face_set(|f, v| f.unlit = v), p)));
            }
            ClockFace::Stack => rows.push(row(p, t("Line gap"), slider(id, "gap", f.line_gap, ranges::LINE_GAP, 0.01, "", face_set(|f, v| f.line_gap = v), p))),
            ClockFace::Boxes => {
                rows.push(row(p, t("Box shape"), choice(id, "bshape", f.box_shape, &SHAPES, face_set(|f, v| f.box_shape = v), p, cx)));
                rows.push(row(p, t("Week strip"), toggle(id, "week", f.week_strip, face_set(|f, v| f.week_strip = v), p, cx)));
            }
            ClockFace::Hud => {
                rows.push(row(p, t("Slant"), slider(id, "slant", f.slant, ranges::SLANT_DEG, 1.0, "°", face_set(|f, v| f.slant = v), p)));
                rows.push(row(p, t("Moon"), toggle(id, "moon", f.moon, face_set(|f, v| f.moon = v), p, cx)));
            }
            ClockFace::Orbit => rows.push(row(p, t("Hour numbers"), toggle(id, "labels", f.labels, face_set(|f, v| f.labels = v), p, cx))),
            ClockFace::Template => {
                rows.push(row(p, t("First line beside the rest"), toggle(id, "divider", f.divider, face_set(|f, v| f.divider = v), p, cx)));
                rows.push(hint(t("Placeholders: {HH} {mm} {ss} {ampm} {time} {weekday} {wd} {day} {month} {mon} {MM} {year} {greeting} {daypart} {hijri}"), p));
                for (i, l) in c.lines.iter().enumerate() {
                    let field = self.list_field(format!("dw-line-{id}-{i}"), &l.text, tk("Text with placeholders"), Rc::new(edit_field(id, move |w, v| if let WidgetKind::Clock(c) = &mut w.kind && let Some(l) = c.lines.get_mut(i) { l.text = v })), cx);
                    rows.push(row(p, &tf("Line {n}", &[("n", &(i + 1))]), div().w(px(TRACK_W_PX)).child(field).into_any_element()));
                    rows.push(row(p, t("Size"), slider(&format!("{id}-{i}"), "lsize", l.size, LINE_SIZE_PX, 1.0, " px", line_set(i, |l, v| l.size = v), p)));
                    rows.push(row(p, t("Weight"), choice(&format!("{id}-{i}"), "lweight", l.weight, &LINE_WEIGHTS, line_set(i, |l, v| l.weight = v), p, cx)));
                    rows.push(row(p, t("Opacity"), slider(&format!("{id}-{i}"), "lop", l.opacity, ranges::OPACITY, 0.05, "", line_set(i, |l, v| l.opacity = v), p)));
                    rows.push(row(p, t("Accent colour"), toggle(&format!("{id}-{i}"), "lacc", l.accent, line_set(i, |l, v| l.accent = v), p, cx)));
                    rows.push(row(p, t("Capitals"), toggle(&format!("{id}-{i}"), "lcaps", l.caps, line_set(i, |l, v| l.caps = v), p, cx)));
                }
                let wid = id.to_string();
                let can_add = c.lines.len() < ranges::TEMPLATE_LINES;
                let can_remove = c.lines.len() > 1;
                let wid2 = wid.clone();
                rows.push(
                    div()
                        .row()
                        .gap(px(SPACE_8_PX))
                        .when(can_add, |el| {
                            el.child(button(format!("dw-line-add-{id}"), "plus", t("Add a line"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                edit(cx, &wid, |w| {
                                    if let WidgetKind::Clock(c) = &mut w.kind {
                                        c.lines.push(dw::TemplateLine { text: "{time}".into(), ..dw::TemplateLine::default() });
                                    }
                                })
                            })))
                        })
                        .when(can_remove, |el| {
                            el.child(button(format!("dw-line-remove-{id}"), "minus", t("Remove the last line"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                edit(cx, &wid2, |w| {
                                    if let WidgetKind::Clock(c) = &mut w.kind {
                                        c.lines.pop();
                                    }
                                })
                            })))
                        })
                        .into_any_element(),
                );
            }
            ClockFace::Greeting => {
                for (i, label) in [tk("Morning greeting"), tk("Afternoon greeting"), tk("Evening greeting"), tk("Night greeting")].into_iter().enumerate() {
                    let current = c.phrases.get(i).cloned().unwrap_or_default();
                    let field = self.list_field(format!("dw-phrase-{id}-{i}"), &current, tk("Empty: the usual greeting"), Rc::new(edit_field(id, move |w, v| {
                        if let WidgetKind::Clock(c) = &mut w.kind {
                            if c.phrases.len() <= i {
                                c.phrases.resize(i + 1, String::new());
                            }
                            c.phrases[i] = v;
                        }
                    })), cx);
                    rows.push(row(p, &tr(label), div().w(px(TRACK_W_PX)).child(field).into_any_element()));
                }
                let field = self.list_field(format!("dw-subtitle-{id}"), &c.subtitle, tk("A line under the greeting (optional)"), Rc::new(edit_field(id, |w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.subtitle = v })), cx);
                rows.push(row(p, t("Second line"), div().w(px(TRACK_W_PX)).child(field).into_any_element()));
            }
            _ => {}
        }
        if matches!(c.face, ClockFace::Stack | ClockFace::Shape | ClockFace::DotMatrix | ClockFace::Pixel | ClockFace::Hud) {
            rows.push(row(p, t("Date"), toggle(id, "date", c.date, clock_set(|c, v| c.date = v), p, cx)));
        }
        rows
    }

    /// The shared shape controls: the plate's shape and its parameters, an
    /// outline, a directional shadow, a header and a chrome.
    fn shape_rows(&mut self, id: &str, look: &dw::Look, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![row(p, t("Shape"), choice(id, "pshape", look.plate_shape, &SHAPES, look_set(|l, v| l.plate_shape = v), p, cx))];
        if look.plate == Plate::None && look.outline_width() == 0.0 && look.plate_shape != PlateShape::Rect {
            rows.push(hint(t("Pick a plate or an outline to see the shape."), p));
        }
        if matches!(look.plate_shape, PlateShape::Cookie | PlateShape::Flower) {
            rows.push(row(p, if look.plate_shape == PlateShape::Cookie { t("Lobes") } else { t("Petals") }, slider(id, "lobes", look.plate_lobes as f32, (style::LOBES.min, style::LOBES.max), style::LOBES.step, "", look_f32(|l, v| l.plate_lobes = v as u32), p)));
        }
        if matches!(look.plate_shape, PlateShape::Cookie | PlateShape::Flower | PlateShape::Deckle) {
            rows.push(row(p, t("Depth"), slider(id, "depth", look.plate_depth, (style::DEPTH.min, style::DEPTH.max), style::DEPTH.step, "", look_f32(|l, v| l.plate_depth = v), p)));
        }
        if look.plate_shape == PlateShape::Skew {
            rows.push(row(p, t("Slant"), slider(id, "skew", look.plate_skew, (style::SKEW_DEG.min, style::SKEW_DEG.max), style::SKEW_DEG.step, "°", look_f32(|l, v| l.plate_skew = v), p)));
        }
        if look.plate_shape == PlateShape::Chamfer {
            rows.push(row(p, t("Corner cut"), slider(id, "chamfer", look.plate_chamfer, (style::CHAMFER_PX.min, style::CHAMFER_PX.max), style::CHAMFER_PX.step, " px", look_f32(|l, v| l.plate_chamfer = v), p)));
        }
        rows.push(row(p, t("Outline"), slider(id, "outline", look.outline_width, (style::OUTLINE_PX.min, style::OUTLINE_PX.max), style::OUTLINE_PX.step, " px", look_f32(|l, v| l.outline_width = v), p)));
        if look.outline_width() > 0.0 {
            rows.push(row(p, t("Outline colour"), choice(id, "otone", look.outline_tone, &TONES, look_set(|l, v| l.outline_tone = v), p, cx)));
            if look.outline_tone == Tone::Custom {
                let f = self.list_field(format!("dw-ocolor-{id}"), &look.outline_color, "#1e1a17", Rc::new(edit_field(id, |w, v| if let Some(l) = w.kind.look_mut() { l.outline_color = v })), cx);
                rows.push(row(p, t("Custom outline"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
            }
        }
        rows.push(row(p, t("Shadow"), choice(id, "shadow", look.shadow, &SHADOWS, look_set(|l, v| l.shadow = v), p, cx)));
        if look.shadow != ShadowKind::None {
            rows.push(row(p, t("Shadow across"), slider(id, "sx", look.shadow_x, (style::SHADOW_OFFSET_PX.min, style::SHADOW_OFFSET_PX.max), style::SHADOW_OFFSET_PX.step, " px", look_f32(|l, v| l.shadow_x = v), p)));
            rows.push(row(p, t("Shadow down"), slider(id, "sy", look.shadow_y, (style::SHADOW_OFFSET_PX.min, style::SHADOW_OFFSET_PX.max), style::SHADOW_OFFSET_PX.step, " px", look_f32(|l, v| l.shadow_y = v), p)));
            if look.shadow == ShadowKind::Soft {
                rows.push(row(p, t("Shadow blur"), slider(id, "sblur", look.shadow_blur, (style::SHADOW_BLUR_PX.min, style::SHADOW_BLUR_PX.max), style::SHADOW_BLUR_PX.step, " px", look_f32(|l, v| l.shadow_blur = v), p)));
            }
            rows.push(row(p, t("Shadow opacity"), slider(id, "sop", look.shadow_opacity, (style::SHADOW_OPACITY.min, style::SHADOW_OPACITY.max), style::SHADOW_OPACITY.step, "", look_f32(|l, v| l.shadow_opacity = v), p)));
            rows.push(row(p, t("Shadow colour"), choice(id, "stone", look.shadow_tone, &TONES, look_set(|l, v| l.shadow_tone = v), p, cx)));
            if look.shadow_tone == Tone::Custom {
                let f = self.list_field(format!("dw-scolor-{id}"), &look.shadow_color, "#1f6bff", Rc::new(edit_field(id, |w, v| if let Some(l) = w.kind.look_mut() { l.shadow_color = v })), cx);
                rows.push(row(p, t("Custom shadow"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
            }
        }
        rows.push(row(p, t("Chrome"), choice(id, "chrome", look.chrome, &CHROMES, look_set(|l, v| l.chrome = v), p, cx)));
        if look.chrome == Chrome::Tape {
            rows.push(row(p, t("Tape colour"), choice(id, "ctone", look.chrome_tone, &TONES, look_set(|l, v| l.chrome_tone = v), p, cx)));
            if look.chrome_tone == Tone::Custom {
                let f = self.list_field(format!("dw-ccolor-{id}"), &look.chrome_color, "#ffe08a", Rc::new(edit_field(id, |w, v| if let Some(l) = w.kind.look_mut() { l.chrome_color = v })), cx);
                rows.push(row(p, t("Custom tape"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
            }
            rows.push(row(p, t("Tape angle"), slider(id, "tape", look.tape_angle, (style::TAPE_ANGLE_DEG.min, style::TAPE_ANGLE_DEG.max), style::TAPE_ANGLE_DEG.step, "°", look_f32(|l, v| l.tape_angle = v), p)));
        }
        let header_label = match look.chrome {
            Chrome::Window => tk("Window title"),
            Chrome::Polaroid => tk("Caption"),
            _ => tk("Header"),
        };
        let header = self.list_field(format!("dw-header-{id}"), &look.header, tk("Optional, e.g. SYS.CLOCK // الساعة"), Rc::new(edit_field(id, |w, v| if let Some(l) = w.kind.look_mut() { l.header = v })), cx);
        rows.push(row(p, &tr(header_label), div().w(px(TRACK_W_PX)).child(header).into_any_element()));
        if !look.header.trim().is_empty() && matches!(look.chrome, Chrome::None | Chrome::Tape) {
            rows.push(row(p, t("Header size"), slider(id, "hsize", look.header_size, (style::HEADER_SIZE_PX.min, style::HEADER_SIZE_PX.max), style::HEADER_SIZE_PX.step, " px", look_f32(|l, v| l.header_size = v), p)));
            rows.push(row(p, t("Rule under the header"), toggle(id, "hrule", look.header_rule, look_set(|l, v| l.header_rule = v), p, cx)));
        }
        rows
    }

    /// What the widget shows: its kind's own settings.
    fn content_rows(&mut self, w: &DesktopWidget, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let id = w.id.clone();
        let mut rows = Vec::new();
        match &w.kind {
            WidgetKind::Clock(c) => {
                let faces: Vec<(ClockFace, &'static str)> = ClockFace::ALL.iter().map(|f| (*f, face_label(*f))).collect();
                rows.push(row(p, t("Style"), choice(&id, "face", c.face, &faces, clock_set(|c, v| c.face = v), p, cx)));
                if matches!(c.face, ClockFace::Superscript | ClockFace::Analog) {
                    rows.push(row(p, t("Seconds"), toggle(&id, "seconds", c.seconds, clock_set(|c, v| c.seconds = v), p, cx)));
                }
                if c.face == ClockFace::Analog {
                    rows.push(row(p, t("Hour numbers"), toggle(&id, "dial-numbers", c.dial_numbers, clock_set(|c, v| c.dial_numbers = v), p, cx)));
                }
                if c.face == ClockFace::Ring {
                    rows.push(row(p, t("Ring shows"), choice(&id, "ring", c.ring_progress, &[(RingProgress::Hour, tk("The hour")), (RingProgress::Day, tk("The day"))], clock_set(|c, v| c.ring_progress = v), p, cx)));
                }
                if c.face.depth_style().is_some() || c.face == ClockFace::Analog {
                    rows.push(row(p, if c.face == ClockFace::Analog { t("Date") } else { t("Date pill") }, toggle(&id, "date", c.date, clock_set(|c, v| c.date = v), p, cx)));
                }
                if c.face.depth_style().is_some() {
                    rows.push(row(p, t("Audio ring"), toggle(&id, "ring-audio", c.audio_ring, clock_set(|c, v| c.audio_ring = v), p, cx)));
                }
                if c.face.depth_style().is_some() || c.face == ClockFace::DayBlock {
                    rows.push(row(p, t("Weather"), toggle(&id, "weather", c.weather, clock_set(|c, v| c.weather = v), p, cx)));
                }
                rows.extend(self.face_rows(&id, c, p, cx));
            }
            WidgetKind::Progress(pw) => {
                rows.push(row(p, t("Period"), choice(&id, "period", pw.period, &PERIODS, progress_set(|w, v| w.period = v), p, cx)));
                rows.push(row(p, t("Style"), choice(&id, "pstyle", pw.style, &PROGRESS_STYLES, progress_set(|w, v| w.style = v), p, cx)));
                rows.push(row(p, t("Line width"), slider(&id, "pthick", pw.thickness, ranges::PROGRESS_THICKNESS_PX, 1.0, " px", progress_set(|w, v| w.thickness = v), p)));
                rows.push(row(p, t("Text under it"), toggle(&id, "plabel", pw.label, progress_set(|w, v| w.label = v), p, cx)));
            }
            WidgetKind::Todo(x) => {
                let list = self.list_field(format!("dw-tlist-{id}"), &x.list, tk("A list's name (empty: every list)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Todo(x) = &mut w.kind { x.list = v })), cx);
                rows.push(row(p, t("List"), div().w(px(TRACK_W_PX)).child(list).into_any_element()));
                rows.push(row(p, t("Rows"), slider(&id, "trows", x.rows as f32, rng(kr::ROWS), 1.0, "", todo_set(|x, v: f32| x.rows = as_count(v)), p)));
                rows.push(row(p, t("Width"), slider(&id, "twidth", x.width, kr::WIDTH_PX, 4.0, " px", todo_set(|x, v| x.width = v), p)));
                rows.push(row(p, t("Text size"), slider(&id, "ttext", x.text_size, kr::TEXT_PX, 1.0, " px", todo_set(|x, v| x.text_size = v), p)));
                rows.push(row(p, t("Check box"), choice(&id, "tbox", x.checkbox, &SHAPES, todo_set(|x, v| x.checkbox = v), p, cx)));
                rows.push(row(p, t("Keep ticked tasks"), toggle(&id, "tdone", x.show_done, todo_set(|x, v| x.show_done = v), p, cx)));
                rows.push(row(p, t("Lined paper"), toggle(&id, "truled", x.ruled, todo_set(|x, v| x.ruled = v), p, cx)));
                rows.push(row(p, t("Title"), toggle(&id, "ttitle", x.title, todo_set(|x, v| x.title = v), p, cx)));
                rows.push(row(p, t("“Add a task” row"), toggle(&id, "tadd", x.add_row, todo_set(|x, v| x.add_row = v), p, cx)));
            }
            WidgetKind::Note(x) => {
                let note = self.list_field(format!("dw-note-{id}"), &x.note, tk("A note's title (empty: the latest)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Note(x) = &mut w.kind { x.note = v })), cx);
                rows.push(row(p, t("Note"), div().w(px(TRACK_W_PX)).child(note).into_any_element()));
                rows.push(row(p, t("Sheet"), choice(&id, "nsheet", x.sheet, &[(dw::NoteSheet::Theme, tk("Plate")), (dw::NoteSheet::Pastel, tk("Pastel")), (dw::NoteSheet::Paper, tk("Paper")), (dw::NoteSheet::Custom, tk("Custom"))], note_set(|x, v| x.sheet = v), p, cx)));
                if x.sheet == dw::NoteSheet::Custom {
                    let f = self.list_field(format!("dw-nsheet-{id}"), &x.sheet_color, "#ffe08a", Rc::new(edit_field(&id, |w, v| if let WidgetKind::Note(x) = &mut w.kind { x.sheet_color = v })), cx);
                    rows.push(row(p, t("Sheet colour"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                }
                rows.push(row(p, t("Lines"), choice(&id, "nruling", x.ruling, &[(dw::NoteRuling::Plain, tk("Plain")), (dw::NoteRuling::Ruled, tk("Ruled")), (dw::NoteRuling::Grid, tk("Grid")), (dw::NoteRuling::Dots, tk("Dots"))], note_set(|x, v| x.ruling = v), p, cx)));
                rows.push(row(p, t("Rows"), slider(&id, "nrows", x.lines as f32, rng(kr::ROWS), 1.0, "", note_set(|x, v: f32| x.lines = as_count(v)), p)));
                rows.push(row(p, t("Width"), slider(&id, "nwidth", x.width, kr::WIDTH_PX, 4.0, " px", note_set(|x, v| x.width = v), p)));
                rows.push(row(p, t("Text size"), slider(&id, "ntext", x.text_size, kr::TEXT_PX, 1.0, " px", note_set(|x, v| x.text_size = v), p)));
                rows.push(row(p, t("Word count"), toggle(&id, "nwords", x.word_count, note_set(|x, v| x.word_count = v), p, cx)));
            }
            WidgetKind::Pomodoro(x) => {
                rows.push(row(p, t("Style"), choice(&id, "pstyle2", x.style, &[(dw::TimerStyle::Ring, tk("Ring")), (dw::TimerStyle::Dots, tk("Dots"))], timer_set(|x, v| x.style = v), p, cx)));
                rows.push(row(p, t("Size"), slider(&id, "psize", x.size, kr::TIMER_SIZE_PX, 4.0, " px", timer_set(|x, v| x.size = v), p)));
                rows.push(row(p, t("Skip and reset buttons"), toggle(&id, "pctl", x.controls, timer_set(|x, v| x.controls = v), p, cx)));
                rows.push(hint(t("Focus and break lengths are the Pomodoro plugin's settings."), p));
            }
            WidgetKind::Quote(x) => {
                let quotes = self.list_field(format!("dw-quotes-{id}"), &x.quotes.join(" | "), tk("Your quotes: text — author | next one (empty: built-in sayings)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Quote(x) = &mut w.kind { x.quotes = v.split('|').map(str::trim).filter(|q| !q.is_empty()).map(String::from).collect() })), cx);
                rows.push(row(p, t("Quotes"), div().w(px(TRACK_W_PX)).child(quotes).into_any_element()));
                rows.push(row(p, t("Changes"), choice(&id, "qevery", x.every, &[(dw::QuoteEvery::Day, tk("Every day")), (dw::QuoteEvery::Hour, tk("Every hour")), (dw::QuoteEvery::Click, tk("On a click"))], quote_set(|x, v| x.every = v), p, cx)));
                rows.push(row(p, t("Quotation mark"), toggle(&id, "qmark", x.mark, quote_set(|x, v| x.mark = v), p, cx)));
                rows.push(row(p, t("Width"), slider(&id, "qwidth", x.width, kr::WIDTH_PX, 4.0, " px", quote_set(|x, v| x.width = v), p)));
                rows.push(row(p, t("Text size"), slider(&id, "qtext", x.text_size, kr::TEXT_PX, 1.0, " px", quote_set(|x, v| x.text_size = v), p)));
            }
            WidgetKind::Notebook(x) => {
                rows.push(row(p, t("Tabs"), slider(&id, "ntabs", x.tabs as f32, rng(kr::TABS), 1.0, "", notebook_set(|x, v: f32| x.tabs = as_count(v)), p)));
                rows.push(row(p, t("Tab shape"), choice(&id, "ntshape", x.tab_shape, &SHAPES, notebook_set(|x, v| x.tab_shape = v), p, cx)));
            }
            WidgetKind::Media(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "mlayout", x.layout, &[(dw::MediaLayout::Vinyl, tk("Record")), (dw::MediaLayout::Device, tk("Pocket player")), (dw::MediaLayout::Shaped, tk("Shaped cover")), (dw::MediaLayout::SidePill, tk("Side pill")), (dw::MediaLayout::Halftone, tk("Halftone")), (dw::MediaLayout::Lyrics, tk("Lyrics"))], media_set(|x, v| x.layout = v), p, cx)));
                if x.layout != dw::MediaLayout::Lyrics {
                    rows.push(row(p, t("Size"), slider(&id, "msize", x.size, kr::MEDIA_SIZE_PX, 2.0, " px", media_set(|x, v| x.size = v), p)));
                }
                match x.layout {
                    dw::MediaLayout::Vinyl => {
                        rows.push(row(p, t("Turns while playing"), toggle(&id, "mspin", x.spin, media_set(|x, v| x.spin = v), p, cx)));
                        rows.push(row(p, t("Grooves"), slider(&id, "mgroove", x.grooves as f32, rng(kr::GROOVES), 1.0, "", media_set(|x, v: f32| x.grooves = as_count(v)), p)));
                    }
                    dw::MediaLayout::Shaped => {
                        for (i, label) in [tk("First shape"), tk("Second shape"), tk("Third shape")].into_iter().enumerate() {
                            let current = x.shapes.get(i).copied().unwrap_or(dw::PlateShape::Circle);
                            rows.push(row(p, &tr(label), choice(&format!("{id}-{i}"), "mshape", current, &SHAPES, Rc::new(move |w, v| {
                                if let WidgetKind::Media(x) = &mut w.kind {
                                    if x.shapes.len() <= i {
                                        x.shapes.resize(i + 1, dw::PlateShape::Circle);
                                    }
                                    x.shapes[i] = v;
                                }
                            }), p, cx)));
                        }
                    }
                    dw::MediaLayout::Halftone => {
                        rows.push(row(p, t("Dot pitch"), slider(&id, "mpitch", x.dot_pitch, kr::DOT_PITCH_PX, 0.5, " px", media_set(|x, v| x.dot_pitch = v), p)));
                        rows.push(row(p, t("One-bit dither"), toggle(&id, "mdither", x.dither, media_set(|x, v| x.dither = v), p, cx)));
                    }
                    dw::MediaLayout::Lyrics => rows.push(row(p, t("Lines"), slider(&id, "mlines", x.lines as f32, rng(kr::LYRIC_LINES), 1.0, "", media_set(|x, v: f32| x.lines = as_count(v)), p))),
                    _ => {}
                }
                if !matches!(x.layout, dw::MediaLayout::Lyrics | dw::MediaLayout::Halftone) {
                    rows.push(row(p, t("Buttons"), toggle(&id, "mctl", x.controls, media_set(|x, v| x.controls = v), p, cx)));
                }
                rows.push(hint(t("Shows only while a player is loaded."), p));
            }
            WidgetKind::Fetch(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "flayout", x.layout, &[(dw::FetchLayout::Terminal, tk("Terminal")), (dw::FetchLayout::Rows, tk("Rows")), (dw::FetchLayout::Grid, tk("Grid"))], fetch_set(|x, v| x.layout = v), p, cx)));
                rows.push(row(p, t("Logo"), toggle(&id, "flogo", x.logo, fetch_set(|x, v| x.logo = v), p, cx)));
                rows.push(row(p, t("Colour row"), toggle(&id, "fcolors", x.colors, fetch_set(|x, v| x.colors = v), p, cx)));
            }
            WidgetKind::ScreenTime(x) => {
                rows.push(row(p, t("Days"), slider(&id, "sdays", x.days as f32, rng(kr::SCREEN_DAYS), 1.0, "", screen_set(|x, v: f32| x.days = as_count(v)), p)));
                rows.push(row(p, t("Apps"), slider(&id, "sapps", x.apps as f32, rng(kr::SCREEN_APPS), 1.0, "", screen_set(|x, v: f32| x.apps = as_count(v)), p)));
                rows.push(hint(t("Needs the Screen time plugin."), p));
            }
            WidgetKind::Prayer(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "prlayout", x.layout, &[(dw::PrayerLayout::Ring, tk("Ring")), (dw::PrayerLayout::List, tk("List"))], prayer_set(|x, v| x.layout = v), p, cx)));
                rows.push(row(p, t("Names in Arabic"), toggle(&id, "prar", x.arabic_names, prayer_set(|x, v| x.arabic_names = v), p, cx)));
                rows.push(hint(t("Needs the Prayer times plugin; times follow its settings."), p));
            }
            WidgetKind::Jannah(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "jlayout", x.layout, &[(dw::JannahLayout::Verse, tk("Verse under an arch")), (dw::JannahLayout::Quote, tk("Quote card")), (dw::JannahLayout::Step, tk("Today's step"))], jannah_set(|x, v| x.layout = v), p, cx)));
                if x.layout != dw::JannahLayout::Step {
                    if x.layout == dw::JannahLayout::Quote {
                        rows.push(row(p, t("Quotation mark"), toggle(&id, "jmark", x.mark, jannah_set(|x, v| x.mark = v), p, cx)));
                    }
                    rows.push(row(p, t("Width"), slider(&id, "jwidth", x.width, kr::WIDTH_PX, 4.0, " px", jannah_set(|x, v| x.width = v), p)));
                    rows.push(row(p, t("Text size"), slider(&id, "jtext", x.text_size, kr::TEXT_PX, 1.0, " px", jannah_set(|x, v| x.text_size = v), p)));
                }
                rows.push(hint(t("Needs the Jannah plugin; today's reminder, its rotation and the meaning follow its settings."), p));
            }
            WidgetKind::Hadith(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "hlayout", x.layout, &[(dw::HadithLayout::Hadith, tk("Hadith of the day")), (dw::HadithLayout::Collection, tk("Reading progress"))], hadith_set(|x, v| x.layout = v), p, cx)));
                if x.layout == dw::HadithLayout::Collection {
                    const COLLECTIONS: [(dw::HadithCollection, &str); 10] = [
                        (dw::HadithCollection::Bukhari, tk("Bukhari")),
                        (dw::HadithCollection::Muslim, tk("Muslim")),
                        (dw::HadithCollection::Abudawud, tk("Abu Dawud")),
                        (dw::HadithCollection::Tirmidhi, tk("Tirmidhi")),
                        (dw::HadithCollection::Nasai, tk("Nasa'i")),
                        (dw::HadithCollection::Ibnmajah, tk("Ibn Majah")),
                        (dw::HadithCollection::Malik, tk("Malik")),
                        (dw::HadithCollection::Nawawi, tk("Nawawi")),
                        (dw::HadithCollection::Qudsi, tk("Qudsi")),
                        (dw::HadithCollection::Dehlawi, tk("Dehlawi")),
                    ];
                    rows.push(row(p, t("Collection"), choice(&id, "hcol", x.collection, &COLLECTIONS, hadith_set(|x, v| x.collection = v), p, cx)));
                } else {
                    rows.push(row(p, t("Width"), slider(&id, "hwidth", x.width, kr::WIDTH_PX, 4.0, " px", hadith_set(|x, v| x.width = v), p)));
                    rows.push(row(p, t("Text size"), slider(&id, "htext", x.text_size, kr::TEXT_PX, 1.0, " px", hadith_set(|x, v| x.text_size = v), p)));
                }
                rows.push(hint(t("Needs the Hadith plugin; today's hadith, its rotation and the meaning follow its settings."), p));
            }
            WidgetKind::Quran(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "qlayout", x.layout, &[(dw::QuranLayout::Ayah, tk("Ayah under an arch")), (dw::QuranLayout::Quote, tk("Quote card")), (dw::QuranLayout::Progress, tk("Khatm progress"))], quran_set(|x, v| x.layout = v), p, cx)));
                if x.layout != dw::QuranLayout::Progress {
                    if x.layout == dw::QuranLayout::Quote {
                        rows.push(row(p, t("Quotation mark"), toggle(&id, "qmark", x.mark, quran_set(|x, v| x.mark = v), p, cx)));
                    }
                    rows.push(row(p, t("Width"), slider(&id, "qwidth", x.width, kr::WIDTH_PX, 4.0, " px", quran_set(|x, v| x.width = v), p)));
                    rows.push(row(p, t("Text size"), slider(&id, "qtext", x.text_size, kr::TEXT_PX, 1.0, " px", quran_set(|x, v| x.text_size = v), p)));
                }
                rows.push(hint(t("Needs the Qur'an plugin; the ayah of the day and your khatm follow it, the meaning its translation setting."), p));
            }
            WidgetKind::Search(x) => {
                rows.push(row(p, t("Width"), slider(&id, "swidth", x.width, kr::WIDTH_PX, 4.0, " px", search_set(|x, v| x.width = v), p)));
                rows.push(row(p, t("Shape"), choice(&id, "sshape", x.shape, &SHAPES, search_set(|x, v| x.shape = v), p, cx)));
                let f = self.list_field(format!("dw-sph-{id}"), &x.placeholder, tk("Your words in the field (optional)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Search(x) = &mut w.kind { x.placeholder = v })), cx);
                rows.push(row(p, t("Placeholder"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
            }
            WidgetKind::Glance(x) => {
                rows.push(row(p, t("Weather"), toggle(&id, "gweather", x.weather, glance_set(|x, v| x.weather = v), p, cx)));
                rows.push(hint(t("Shows the most pressing of: an event soon, the next prayer, a low battery, unread notifications, what plays."), p));
            }
            WidgetKind::Github(x) => {
                let f = self.list_field(format!("dw-ghuser-{id}"), &x.user, tk("Your GitHub user name"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Github(x) = &mut w.kind { x.user = v.trim().to_string() })), cx);
                rows.push(row(p, t("User"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                rows.push(row(p, t("Fetch from GitHub"), toggle(&id, "ghonline", x.online, github_set(|x, v| x.online = v), p, cx)));
                rows.push(hint(t("Asks github.com for your public contribution calendar every few hours, only while the desktop shows. Nothing is sent but the user name."), p));
                rows.push(row(p, t("Weeks"), slider(&id, "ghweeks", x.weeks as f32, rng(kr::WEEKS), 1.0, "", github_set(|x, v: f32| x.weeks = as_count(v)), p)));
                rows.push(row(p, t("Cell shape"), choice(&id, "ghcell", x.cell, &SHAPES, github_set(|x, v| x.cell = v), p, cx)));
                rows.push(row(p, t("Total and streak"), toggle(&id, "ghsum", x.summary, github_set(|x, v| x.summary = v), p, cx)));
            }
            WidgetKind::Currency(x) => {
                let base = self.list_field(format!("dw-cbase-{id}"), &x.base, "USD", Rc::new(edit_field(&id, |w, v| if let WidgetKind::Currency(x) = &mut w.kind { x.base = v.trim().to_ascii_uppercase() })), cx);
                rows.push(row(p, t("Base currency"), div().w(px(TRACK_W_PX)).child(base).into_any_element()));
                let pairs = self.list_field(format!("dw-cpairs-{id}"), &x.pairs.join(", "), "EUR, GBP, EGP", Rc::new(edit_field(&id, |w, v| if let WidgetKind::Currency(x) = &mut w.kind { x.pairs = v.split([',', ' ']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_ascii_uppercase).collect() })), cx);
                rows.push(row(p, t("Currencies"), div().w(px(TRACK_W_PX)).child(pairs).into_any_element()));
                rows.push(row(p, t("Decimals"), slider(&id, "cdec", x.decimals as f32, rng(kr::DECIMALS), 1.0, "", currency_set(|x, v: f32| x.decimals = as_count(v)), p)));
                rows.push(row(p, t("Fetch rates"), toggle(&id, "conline", x.online, currency_set(|x, v| x.online = v), p, cx)));
                rows.push(hint(t("Asks open.er-api.com (ExchangeRate-API) twice a day, only while the desktop shows. A click fetches again."), p));
            }
            WidgetKind::AiUsage(x) => {
                rows.push(row(p, t("Read Claude Code's logs"), toggle(&id, "ailogs", x.read_logs, ai_set(|x, v| x.read_logs = v), p, cx)));
                rows.push(hint(t("Counts the tokens in ~/.claude/projects on this computer. Nothing leaves it."), p));
                rows.push(row(p, t("Days"), slider(&id, "aidays", x.days as f32, rng(kr::AI_DAYS), 1.0, "", ai_set(|x, v: f32| x.days = as_count(v)), p)));
                rows.push(row(p, t("Weekly budget"), slider(&id, "aibudget", x.budget_millions, kr::AI_BUDGET_M, 5.0, " M", ai_set(|x, v| x.budget_millions = v), p)));
            }
            WidgetKind::Games(x) => {
                rows.push(row(p, t("Games"), slider(&id, "gcount", x.count as f32, rng(kr::GAMES), 1.0, "", games_set(|x, v: f32| x.count = as_count(v)), p)));
                rows.push(row(p, t("Cover shape"), choice(&id, "gcover", x.cover, &SHAPES, games_set(|x, v| x.cover = v), p, cx)));
                rows.push(row(p, t("Names"), toggle(&id, "gnames", x.names, games_set(|x, v| x.names = v), p, cx)));
                rows.push(hint(t("Your installed Steam games, the most recently played first."), p));
            }
            WidgetKind::Profile(x) => {
                rows.push(row(p, t("Picture shape"), choice(&id, "pavatar", x.avatar, &SHAPES, profile_set(|x, v| x.avatar = v), p, cx)));
                rows.push(row(p, t("user@host"), toggle(&id, "plogin", x.login, profile_set(|x, v| x.login = v), p, cx)));
                rows.push(row(p, t("Uptime"), toggle(&id, "puptime", x.uptime, profile_set(|x, v| x.uptime = v), p, cx)));
                rows.push(row(p, t("Status"), toggle(&id, "pstatus", x.show_status, profile_set(|x, v| x.show_status = v), p, cx)));
                if x.show_status {
                    let f = self.list_field(format!("dw-pstatus-{id}"), &x.status, tk("Your status (empty: Online / Do not disturb)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Profile(x) = &mut w.kind { x.status = v })), cx);
                    rows.push(row(p, t("Status text"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                }
                rows.push(hint(t("Your picture is ~/.face or your account picture."), p));
            }
            WidgetKind::Shortcuts(x) => {
                let placeholder = if x.folders { tk("Folders, comma separated (empty: your home folders)") } else { tk("Apps (desktop ids) or web addresses, comma separated (empty: your dock's apps)") };
                let f = self.list_field(format!("dw-tiles-{id}"), &x.items.join(", "), placeholder, Rc::new(edit_field(&id, |w, v| if let WidgetKind::Shortcuts(x) = &mut w.kind { x.items = v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect() })), cx);
                rows.push(row(p, if x.folders { t("Folders") } else { t("Shortcuts") }, div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                rows.push(row(p, t("Folders"), toggle(&id, "tfolders", x.folders, tiles_set(|x, v| x.folders = v), p, cx)));
                rows.push(row(p, t("Layout"), choice(&id, "tlayout", x.layout, &[(dw::TileLayout::Grid, tk("Grid")), (dw::TileLayout::List, tk("List"))], tiles_set(|x, v| x.layout = v), p, cx)));
                if x.layout == dw::TileLayout::Grid {
                    rows.push(row(p, t("Columns"), slider(&id, "tcols", x.columns as f32, rng(kr::COLUMNS), 1.0, "", tiles_set(|x, v: f32| x.columns = as_count(v)), p)));
                    rows.push(row(p, t("Names"), toggle(&id, "tlabels", x.labels, tiles_set(|x, v| x.labels = v), p, cx)));
                }
                rows.push(row(p, t("Tile shape"), choice(&id, "ttile", x.tile, &SHAPES, tiles_set(|x, v| x.tile = v), p, cx)));
                rows.push(row(p, t("Colourful tiles"), toggle(&id, "tcolor", x.colorful, tiles_set(|x, v| x.colorful = v), p, cx)));
            }
            WidgetKind::Toggles(x) => {
                let f = self.list_field(format!("dw-toggles-{id}"), &x.toggles.join(", "), tk("wifi, bluetooth, dnd, night-light, keep-awake, dark, mute"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Toggles(x) = &mut w.kind { x.toggles = v.split(',').map(str::trim).filter(|s| mandara_ui::desktop_actions::TOGGLES.contains(s)).map(String::from).collect() })), cx);
                rows.push(row(p, t("Toggles"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                rows.push(row(p, t("Shape when off"), choice(&id, "toff", x.shape_off, &SHAPES, toggles_set(|x, v| x.shape_off = v), p, cx)));
                rows.push(row(p, t("Shape when on"), choice(&id, "ton", x.shape_on, &SHAPES, toggles_set(|x, v| x.shape_on = v), p, cx)));
                rows.push(row(p, t("Columns"), slider(&id, "tgcols", x.columns as f32, rng(kr::COLUMNS), 1.0, "", toggles_set(|x, v: f32| x.columns = as_count(v)), p)));
                rows.push(row(p, t("Names"), toggle(&id, "tglabels", x.labels, toggles_set(|x, v| x.labels = v), p, cx)));
            }
            WidgetKind::Sliders(x) => {
                rows.push(row(p, t("Upright"), toggle(&id, "svert", x.vertical, sliders_set(|x, v| x.vertical = v), p, cx)));
                rows.push(row(p, t("Length"), slider(&id, "slen", x.length, kr::SLIDER_LENGTH_PX, 2.0, " px", sliders_set(|x, v| x.length = v), p)));
                rows.push(row(p, t("Thickness"), slider(&id, "sthick", x.thickness, kr::SLIDER_THICKNESS_PX, 1.0, " px", sliders_set(|x, v| x.thickness = v), p)));
                rows.push(row(p, t("Volume"), toggle(&id, "svol", x.levels.is_empty() || x.levels.iter().any(|l| l == "volume"), sliders_set(|x, v| set_level_list(&mut x.levels, "volume", v)), p, cx)));
                rows.push(row(p, t("Brightness"), toggle(&id, "sbright", x.levels.is_empty() || x.levels.iter().any(|l| l == "brightness"), sliders_set(|x, v| set_level_list(&mut x.levels, "brightness", v)), p, cx)));
            }
            WidgetKind::Palette(x) => {
                rows.push(row(p, t("Swatch shape"), choice(&id, "pswatch", x.swatch, &SHAPES, palette_set(|x, v| x.swatch = v), p, cx)));
                rows.push(row(p, t("Columns"), slider(&id, "pcols", x.columns as f32, rng(kr::COLUMNS), 1.0, "", palette_set(|x, v: f32| x.columns = as_count(v)), p)));
                rows.push(row(p, t("Names and codes"), toggle(&id, "plabels", x.labels, palette_set(|x, v| x.labels = v), p, cx)));
            }
            WidgetKind::Pet(x) => {
                let f = self.list_field(format!("dw-petname-{id}"), &x.name, tk("Its name (optional)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Pet(x) = &mut w.kind { x.name = v })), cx);
                rows.push(row(p, t("Name"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                rows.push(row(p, t("Size"), slider(&id, "petsize", x.size, kr::PET_SIZE_PX, 2.0, " px", pet_set(|x, v| x.size = v), p)));
                rows.push(hint(t("It moves only while the desktop shows, a few frames a second."), p));
            }
            WidgetKind::Bubbles(x) => {
                rows.push(row(p, t("Bubbles"), slider(&id, "bcount", x.count as f32, rng(kr::BUBBLES), 1.0, "", bubbles_set(|x, v: f32| x.count = as_count(v)), p)));
                rows.push(row(p, t("Smallest"), slider(&id, "bmin", x.min_size, kr::BUBBLE_PX, 1.0, " px", bubbles_set(|x, v| x.min_size = v), p)));
                rows.push(row(p, t("Largest"), slider(&id, "bmax", x.max_size, kr::BUBBLE_PX, 1.0, " px", bubbles_set(|x, v| x.max_size = v), p)));
                rows.push(row(p, t("Speed"), slider(&id, "bspeed", x.speed, kr::BUBBLE_SPEED, 1.0, "", bubbles_set(|x, v| x.speed = v), p)));
                rows.push(row(p, t("Width"), slider(&id, "bw", x.width, kr::AREA_PX, 4.0, " px", bubbles_set(|x, v| x.width = v), p)));
                rows.push(row(p, t("Height"), slider(&id, "bh", x.height, kr::AREA_PX, 4.0, " px", bubbles_set(|x, v| x.height = v), p)));
            }
            WidgetKind::Photo(x) => {
                let f = self.list_field(format!("dw-pfolder-{id}"), &x.folder, tk("A folder of pictures (empty: ~/Pictures)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Photo(x) = &mut w.kind { x.folder = v })), cx);
                rows.push(row(p, t("Folder"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                rows.push(row(p, t("Minutes each"), slider(&id, "pmin", x.minutes as f32, rng(kr::PHOTO_MINUTES), 1.0, " min", photo_set(|x, v: f32| x.minutes = as_count(v)), p)));
                rows.push(row(p, t("Crop shape"), choice(&id, "pcrop", x.crop, &SHAPES, photo_set(|x, v| x.crop = v), p, cx)));
                rows.push(row(p, t("Width"), slider(&id, "pw2", x.width, kr::AREA_PX, 4.0, " px", photo_set(|x, v| x.width = v), p)));
                rows.push(row(p, t("Height"), slider(&id, "ph2", x.height, kr::AREA_PX, 4.0, " px", photo_set(|x, v| x.height = v), p)));
            }
            WidgetKind::Sticker(x) => {
                let f = self.list_field(format!("dw-simage-{id}"), &x.image, tk("A picture file (PNG with a transparent background looks best)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Sticker(x) = &mut w.kind { x.image = v })), cx);
                rows.push(row(p, t("Picture"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
                rows.push(row(p, t("Size"), slider(&id, "ssz", x.size, kr::AREA_PX, 4.0, " px", sticker_set(|x, v| x.size = v), p)));
                rows.push(row(p, t("Border"), slider(&id, "sbord", x.border, kr::STICKER_BORDER_PX, 0.5, " px", sticker_set(|x, v| x.border = v), p)));
            }
            WidgetKind::Monitor(m) => {
                let set = |f: fn(&mut mandara_config::desktop_widgets::MonitorWidget, f32)| -> Set<f32> { Rc::new(move |w, v| if let WidgetKind::Monitor(m) = &mut w.kind { f(m, v) }) };
                rows.push(row(p, t("Style"), choice(&id, "mstyle", m.style, &[(MonitorStyle::Column, tk("Column")), (MonitorStyle::Gauges, tk("Gauges")), (MonitorStyle::Meters, tk("Meters")), (MonitorStyle::Bars, tk("Bars")), (MonitorStyle::Thermals, tk("Thermals"))], Rc::new(|w, v| if let WidgetKind::Monitor(m) = &mut w.kind { m.style = v }), p, cx)));
                if m.style == MonitorStyle::Gauges {
                    rows.push(row(p, t("Ring width"), slider(&id, "mring", m.ring_width, ranges::RING_WIDTH_PX, 0.5, " px", monitor_set(|m, v| m.ring_width = v), p)));
                    rows.push(row(p, t("Wavy ring"), toggle(&id, "mwavy", m.wavy, monitor_set(|m, v| m.wavy = v), p, cx)));
                    rows.push(row(p, t("Dotted ring"), toggle(&id, "mdots", m.dotted, monitor_set(|m, v| m.dotted = v), p, cx)));
                    rows.push(row(p, t("Ring starts at"), slider(&id, "mstart", m.start_angle, ranges::ANGLE_DEG, 1.0, "°", monitor_set(|m, v| m.start_angle = v), p)));
                }
                if m.style == MonitorStyle::Meters {
                    rows.push(row(p, t("Segments"), slider(&id, "msegs", m.segments as f32, (ranges::SEGMENTS.0 as f32, ranges::SEGMENTS.1 as f32), 1.0, "", monitor_set(|m, v: f32| m.segments = as_count(v)), p)));
                }
                rows.push(row(p, t("Rows"), monitor_rows(&id, &m.rows, p, cx)));
                rows.push(row(p, t("Refresh"), slider(&id, "interval", m.interval_s as f32, (ranges::MONITOR_INTERVAL_S.0 as f32, ranges::MONITOR_INTERVAL_S.1 as f32), 1.0, " s", set(|m, v| m.interval_s = v as u32), p)));
                rows.push(row(p, t("Width"), slider(&id, "width", m.width, ranges::MONITOR_WIDTH_PX, 2.0, " px", set(|m, v| m.width = v), p)));
                let title = self.list_field(format!("dw-title-{id}"), &m.title, tk("Title (optional)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Monitor(m) = &mut w.kind { m.title = v })), cx);
                rows.push(row(p, t("Title"), div().w(px(TRACK_W_PX)).child(title).into_any_element()));
            }
            WidgetKind::Decoration(d) => {
                let shapes: Vec<(DecorShape, &'static str)> = [DecorShape::Text, DecorShape::Disc, DecorShape::Circle, DecorShape::Stripes, DecorShape::Waves, DecorShape::Rule, DecorShape::Frame, DecorShape::Tape].iter().map(|s| (*s, shape_label(*s))).collect();
                rows.push(row(p, t("Shape"), choice(&id, "shape", d.shape, &shapes, Rc::new(|w, v| if let WidgetKind::Decoration(d) = &mut w.kind { d.shape = v }), p, cx)));
                let set = |f: fn(&mut Decoration, f32)| -> Set<f32> { Rc::new(move |w, v| if let WidgetKind::Decoration(d) = &mut w.kind { f(d, v) }) };
                if d.shape == DecorShape::Text {
                    let text = self.list_field(format!("dw-text-{id}"), &d.text.replace('\n', " / "), tk("Your text (“ / ” starts a new line or column)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Decoration(d) = &mut w.kind { d.text = v.split(" / ").collect::<Vec<_>>().join("\n") })), cx);
                    rows.push(row(p, t("Text"), div().w(px(DIM_320_PX)).child(text).into_any_element()));
                    rows.push(row(p, t("Vertical"), toggle(&id, "vertical", d.vertical, Rc::new(|w, v| if let WidgetKind::Decoration(d) = &mut w.kind { d.vertical = v }), p, cx)));
                    if d.vertical {
                        rows.push(row(p, t("Sideways text"), choice(&id, "sideways", d.sideways, &SIDEWAYS, Rc::new(|w, v| if let WidgetKind::Decoration(d) = &mut w.kind { d.sideways = v }), p, cx)));
                        rows.push(row(p, t("Upright Latin letters"), toggle(&id, "upright-latin", d.upright_latin, Rc::new(|w, v| if let WidgetKind::Decoration(d) = &mut w.kind { d.upright_latin = v }), p, cx)));
                    }
                    rows.push(row(p, t("Text size"), slider(&id, "size", d.size, ranges::TEXT_SIZE_PX, 1.0, " px", set(|d, v| d.size = v), p)));
                    rows.push(row(p, t("Weight"), slider(&id, "weight", d.weight as f32, (ranges::WEIGHT.0 as f32, ranges::WEIGHT.1 as f32), 100.0, "", set(|d, v| d.weight = v as u16), p)));
                    rows.push(row(p, t("Letter spacing"), slider(&id, "tracking", d.tracking, ranges::TRACKING_EM, 0.01, " em", set(|d, v| d.tracking = v), p)));
                } else {
                    rows.push(row(p, t("Width"), slider(&id, "w", d.width, ranges::SHAPE_SIZE_PX, 2.0, " px", set(|d, v| d.width = v), p)));
                    if matches!(d.shape, DecorShape::Stripes | DecorShape::Waves | DecorShape::Frame | DecorShape::Tape) {
                        rows.push(row(p, t("Height"), slider(&id, "h", d.height, ranges::SHAPE_SIZE_PX, 2.0, " px", set(|d, v| d.height = v), p)));
                    }
                    if matches!(d.shape, DecorShape::Stripes | DecorShape::Waves | DecorShape::Tape) {
                        rows.push(row(p, t("Lines"), slider(&id, "count", d.count as f32, (ranges::REPEAT.0 as f32, ranges::REPEAT.1 as f32), 1.0, "", set(|d, v| d.count = v as u32), p)));
                    }
                    if matches!(d.shape, DecorShape::Circle | DecorShape::Waves | DecorShape::Rule | DecorShape::Frame | DecorShape::Tape) {
                        rows.push(row(p, t("Line width"), slider(&id, "stroke", d.stroke, ranges::STROKE_PX, 0.5, " px", set(|d, v| d.stroke = v), p)));
                    }
                }
                rows.push(row(p, t("Rotation"), slider(&id, "rot", d.rotation, ranges::ANGLE_DEG, 1.0, "°", set(|d, v| d.rotation = v), p)));
            }
            WidgetKind::Visualizer(v) => {
                let shapes: Vec<(VisualizerShape, &'static str)> = [VisualizerShape::Bars, VisualizerShape::Mirrored, VisualizerShape::Wave, VisualizerShape::Radial, VisualizerShape::Led].iter().map(|s| (*s, vis_label(*s))).collect();
                rows.push(row(p, t("Shape"), choice(&id, "vshape", v.shape, &shapes, Rc::new(|w, x| if let WidgetKind::Visualizer(v) = &mut w.kind { v.shape = x }), p, cx)));
                let set = |f: fn(&mut PlaneVisualizer, f32)| -> Set<f32> { Rc::new(move |w, x| if let WidgetKind::Visualizer(v) = &mut w.kind { f(v, x) }) };
                rows.push(row(p, t("Bars"), slider(&id, "bars", v.bars as f32, (ranges::VISUALIZER_BARS.0 as f32, ranges::VISUALIZER_BARS.1 as f32), 1.0, "", set(|v, x| v.bars = x as u32), p)));
                rows.push(row(p, if v.shape == VisualizerShape::Radial { t("Diameter") } else { t("Length") }, slider(&id, "len", v.length, ranges::VISUALIZER_LENGTH_PX, 2.0, " px", set(|v, x| v.length = x), p)));
                rows.push(row(p, t("Height"), slider(&id, "vh", v.height, ranges::VISUALIZER_HEIGHT_PX, 1.0, " px", set(|v, x| v.height = x), p)));
                rows.push(row(p, t("Angle"), slider(&id, "angle", v.angle, ranges::ANGLE_DEG, 1.0, "°", set(|v, x| v.angle = x), p)));
                rows.push(row(p, t("Gap"), slider(&id, "gap", v.gap, ranges::VISUALIZER_GAP, 0.05, "", set(|v, x| v.gap = x), p)));
                rows.push(row(p, t("Frame rate"), slider(&id, "fps", v.fps as f32, (ranges::VISUALIZER_FPS.0 as f32, ranges::VISUALIZER_FPS.1 as f32), 1.0, " fps", set(|v, x| v.fps = x as u32), p)));
                rows.push(row(p, t("Stop after silence"), slider(&id, "silence", v.silence_s as f32, (ranges::VISUALIZER_SILENCE_S.0 as f32, ranges::VISUALIZER_SILENCE_S.1 as f32), 1.0, " s", set(|v, x| v.silence_s = x as u32), p)));
            }
            WidgetKind::Weather(ww) => {
                rows.push(row(p, t("Layout"), choice(&id, "wlayout", ww.layout, &[(WeatherLayout::Hero, tk("Large")), (WeatherLayout::Compact, tk("One line")), (WeatherLayout::Sentence, tk("Sentence")), (WeatherLayout::Disc, tk("Disc")), (WeatherLayout::Hourly, tk("Hourly curve"))], Rc::new(|w, v| if let WidgetKind::Weather(x) = &mut w.kind { x.layout = v }), p, cx)));
                match ww.layout {
                    WeatherLayout::Hero => {
                        rows.push(row(p, t("Place name"), toggle(&id, "wplace", ww.location, weather_set(|x, v| x.location = v), p, cx)));
                        rows.push(row(p, t("Details"), toggle(&id, "wdetails", ww.details, weather_set(|x, v| x.details = v), p, cx)));
                        rows.push(row(p, t("Days ahead"), slider(&id, "wdays", ww.days as f32, (ranges::WEATHER_DAYS.0 as f32, ranges::WEATHER_DAYS.1 as f32), 1.0, "", weather_set(|x, v: f32| x.days = as_count(v)), p)));
                    }
                    WeatherLayout::Sentence => {
                        rows.push(row(p, t("Place name"), toggle(&id, "wplace", ww.location, weather_set(|x, v| x.location = v), p, cx)));
                        for (i, label) in [tk("Clear"), tk("Partly cloudy"), tk("Cloudy"), tk("Fog"), tk("Drizzle"), tk("Rain"), tk("Snow"), tk("Thunderstorm")].into_iter().enumerate() {
                            let current = ww.sentences.get(i).cloned().unwrap_or_default();
                            let field = self.list_field(format!("dw-wsent-{id}-{i}"), &current, tk("Empty: the built-in sentence"), Rc::new(edit_field(&id, move |w, v| {
                                if let WidgetKind::Weather(x) = &mut w.kind {
                                    if x.sentences.len() <= i {
                                        x.sentences.resize(i + 1, String::new());
                                    }
                                    x.sentences[i] = v;
                                }
                            })), cx);
                            rows.push(row(p, &tr(label), div().w(px(TRACK_W_PX)).child(field).into_any_element()));
                        }
                    }
                    WeatherLayout::Disc => {
                        rows.push(row(p, t("Size"), slider(&id, "wdisc", ww.disc, kr::DISC_PX, 4.0, " px", weather_set(|x, v| x.disc = v), p)));
                        rows.push(row(p, t("Dotted ring"), toggle(&id, "wring", ww.dotted_ring, weather_set(|x, v| x.dotted_ring = v), p, cx)));
                    }
                    WeatherLayout::Hourly => {
                        rows.push(row(p, t("Hours"), slider(&id, "whours", ww.hours as f32, rng(kr::HOURS), 1.0, "", weather_set(|x, v: f32| x.hours = as_count(v)), p)));
                        rows.push(row(p, t("Fill under the curve"), toggle(&id, "wfill", ww.fill, weather_set(|x, v| x.fill = v), p, cx)));
                    }
                    WeatherLayout::Compact => {}
                }
            }
            WidgetKind::Date(x) => {
                rows.push(row(p, t("Layout"), choice(&id, "dlayout", x.layout, &[(dw::DateLayout::Month, tk("Month")), (dw::DateLayout::Week, tk("Week strip")), (dw::DateLayout::Agenda, tk("Coming up")), (dw::DateLayout::Tile, tk("Date tile"))], date_set(|x, v| x.layout = v), p, cx)));
                if x.layout != dw::DateLayout::Tile {
                    rows.push(row(p, t("Marker shape"), choice(&id, "dmarker", x.marker, &SHAPES, date_set(|x, v| x.marker = v), p, cx)));
                }
                match x.layout {
                    dw::DateLayout::Month => {
                        rows.push(row(p, t("Week numbers"), toggle(&id, "dweeks", x.week_numbers, date_set(|x, v| x.week_numbers = v), p, cx)));
                        rows.push(row(p, t("Event dots"), toggle(&id, "ddots", x.event_dots, date_set(|x, v| x.event_dots = v), p, cx)));
                    }
                    dw::DateLayout::Week => rows.push(row(p, t("Event dots"), toggle(&id, "ddots", x.event_dots, date_set(|x, v| x.event_dots = v), p, cx))),
                    dw::DateLayout::Agenda => rows.push(row(p, t("Rows"), slider(&id, "drows", x.rows as f32, rng(kr::AGENDA_ROWS), 1.0, "", date_set(|x, v: f32| x.rows = as_count(v)), p))),
                    dw::DateLayout::Tile => {}
                }
                rows.push(hint(t("Events come from Studio → Calendar."), p));
            }
            WidgetKind::Sky(x) => {
                rows.push(row(p, t("Shows"), choice(&id, "slayout", x.layout, &[(dw::SkyLayout::SunArc, tk("Sun arc")), (dw::SkyLayout::Moon, tk("Moon phase")), (dw::SkyLayout::Globe, tk("Day and night globe"))], sky_set(|x, v| x.layout = v), p, cx)));
                rows.push(row(p, t("Size"), slider(&id, "ssize", x.size, kr::SKY_SIZE_PX, 4.0, " px", sky_set(|x, v| x.size = v), p)));
                match x.layout {
                    dw::SkyLayout::SunArc => {
                        rows.push(row(p, t("Arc height"), slider(&id, "sarc", x.arc_height, kr::ARC_HEIGHT, 0.01, "", sky_set(|x, v| x.arc_height = v), p)));
                        rows.push(row(p, t("Dashed path"), toggle(&id, "sdash", x.dashed, sky_set(|x, v| x.dashed = v), p, cx)));
                        rows.push(row(p, t("Hour ticks"), toggle(&id, "sticks", x.ticks, sky_set(|x, v| x.ticks = v), p, cx)));
                    }
                    dw::SkyLayout::Moon => rows.push(row(p, t("Shaded"), toggle(&id, "sshade", x.shaded, sky_set(|x, v| x.shaded = v), p, cx))),
                    dw::SkyLayout::Globe => rows.push(row(p, t("Moon"), toggle(&id, "smoon", x.moon, sky_set(|x, v| x.moon = v), p, cx))),
                }
                if x.layout != dw::SkyLayout::Globe {
                    rows.push(row(p, t("Labels"), toggle(&id, "slabels", x.labels, sky_set(|x, v| x.labels = v), p, cx)));
                }
                rows.push(hint(t("Uses your location from Studio → Date, time & location. Nothing is fetched from the network."), p));
            }
            WidgetKind::WorldClock(wc) => {
                let cities = self.list_field(format!("dw-cities-{id}"), &wc.cities.join(", "), tk("Cities, comma separated (empty: your World clock cities)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::WorldClock(x) = &mut w.kind { x.cities = v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect() })), cx);
                rows.push(row(p, t("Cities"), div().w(px(DIM_320_PX)).child(cities).into_any_element()));
                rows.push(row(p, t("Hours ahead or behind"), toggle(&id, "woffsets", wc.offsets, Rc::new(|w, v| if let WidgetKind::WorldClock(x) = &mut w.kind { x.offsets = v }), p, cx)));
                rows.push(row(p, t("Side by side"), toggle(&id, "wrow", wc.horizontal, Rc::new(|w, v| if let WidgetKind::WorldClock(x) = &mut w.kind { x.horizontal = v }), p, cx)));
            }
            WidgetKind::Countdown(c) => {
                let title = self.list_field(format!("dw-ctitle-{id}"), &c.title, tk("What is coming (empty: New Year)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Countdown(x) = &mut w.kind { x.title = v })), cx);
                rows.push(row(p, t("Title"), div().w(px(TRACK_W_PX)).child(title).into_any_element()));
                let date = self.list_field(format!("dw-cdate-{id}"), &c.date, tk("YYYY-MM-DD (empty: next New Year)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Countdown(x) = &mut w.kind { x.date = v.trim().to_string() })), cx);
                rows.push(row(p, t("Date"), div().w(px(TRACK_W_PX)).child(date).into_any_element()));
                rows.push(row(p, t("Show the date"), toggle(&id, "cshow", c.show_date, Rc::new(|w, v| if let WidgetKind::Countdown(x) = &mut w.kind { x.show_date = v }), p, cx)));
                rows.push(row(p, t("Count the days since"), toggle(&id, "cup", c.count_up, Rc::new(|w, v| if let WidgetKind::Countdown(x) = &mut w.kind { x.count_up = v }), p, cx)));
            }
            WidgetKind::Card(c) if dc::parse_plugin_card(&c.card).is_some() => {
                let opts: PluginCardOptions = dc::options(c);
                let set = |f: fn(&mut PluginCardOptions, f32)| -> Set<f32> { Rc::new(move |w, v| plugin_card_edit(w, |o| f(o, v))) };
                rows.push(row(p, t("Width"), slider(&id, "pw", opts.width, (dc::ranges::PLUGIN_CARD_WIDTH.min, dc::ranges::PLUGIN_CARD_WIDTH.max), dc::ranges::PLUGIN_CARD_WIDTH.step, " px", set(|o, v| o.width = v), p)));
                rows.push(row(p, t("Height"), slider(&id, "ph", opts.height, (dc::ranges::PLUGIN_CARD_HEIGHT.min, dc::ranges::PLUGIN_CARD_HEIGHT.max), dc::ranges::PLUGIN_CARD_HEIGHT.step, " px", set(|o, v| o.height = v), p)));
                let locale = TextOptions { numerals: opts.numerals, calendar: opts.calendar, language: opts.language.clone(), ..TextOptions::default() };
                rows.extend(plugin_text_rows(&id, &locale, p, cx));
            }
            WidgetKind::Card(_) => rows.push(hint(t("An interactive card — its own settings are with the cards."), p)),
        }
        rows
    }

    /// The font: a role, or a family of its own from the font picker.
    fn font_rows(&mut self, id: &str, look: &dw::Look, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![row(p, t("Font"), choice(id, "font", look.font, &FONTS, look_set(|l, v| l.font = v), p, cx))];
        if look.font == FontRole::Custom {
            let picker = self.widget_font_picker(id, cx);
            let fallback = mandara_config::TypographyConfig::or(&cfg.typography.widgets_font, &cfg.typography.display_font).to_string();
            picker.update(cx, |f, _| f.sync(&look.font_family, Some(&fallback)));
            rows.push(row(p, t("Family"), picker.into_any_element()));
        }
        rows
    }

    /// The selected widget's font picker (one at a time).
    fn widget_font_picker(&mut self, id: &str, cx: &mut Context<Self>) -> Entity<FontSelect> {
        if let Some((_, f)) = cx.global::<PageState>().font_select.as_ref().filter(|(w, _)| w == id) {
            return f.clone();
        }
        let wid = id.to_string();
        let f = cx.new(|_| {
            FontSelect::new(
                format!("dw-font-{id}"),
                Rc::new(move |family, _, cx| {
                    edit(cx, &wid, move |w| {
                        if let Some(l) = w.kind.look_mut() {
                            l.font_family = family;
                        }
                    })
                }),
            )
        });
        cx.global_mut::<PageState>().font_select = Some((id.to_string(), f.clone()));
        f
    }

    fn look_rows(&mut self, id: &str, look: &dw::Look, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = self.font_rows(id, look, cfg, p, cx);
        rows.push(row(p, t("Colour"), choice(id, "tone", look.tone, &TONES, look_set(|l, v| l.tone = v), p, cx)));
        if look.tone == Tone::Custom {
            let f = self.list_field(format!("dw-color-{id}"), &look.custom_color, "#e8d9b0 / oklch(0.85 0.06 85)", Rc::new(edit_field(id, |w, v| if let Some(l) = w.kind.look_mut() { l.custom_color = v })), cx);
            rows.push(row(p, t("Custom colour"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
        }
        rows.push(row(p, t("Accent"), choice(id, "accent", look.accent, &TONES, look_set(|l, v| l.accent = v), p, cx)));
        if look.accent == Tone::Custom {
            let f = self.list_field(format!("dw-accent-{id}"), &look.custom_accent, "#4fd1c5 / oklch(0.78 0.12 190)", Rc::new(edit_field(id, |w, v| if let Some(l) = w.kind.look_mut() { l.custom_accent = v })), cx);
            rows.push(row(p, t("Custom accent"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
        }
        rows.push(row(p, t("Alignment"), choice(id, "align", look.align, &ALIGNS, look_set(|l, v| l.align = v), p, cx)));
        rows.push(row(p, t("Effect"), choice(id, "effect", look.effect, &EFFECTS, look_set(|l, v| l.effect = v), p, cx)));
        if look.effect != Effect::None {
            rows.push(row(p, t("Effect strength"), slider(id, "strength", look.effect_strength, ranges::EFFECT_STRENGTH, 0.05, "×", look_f32(|l, v| l.effect_strength = v), p)));
        }
        rows.push(row(p, t("Plate"), choice(id, "plate", look.plate, &PLATES, look_set(|l, v| l.plate = v), p, cx)));
        if look.plate != Plate::None {
            if look.plate == Plate::Glass {
                rows.push(row(p, t("Plate opacity"), slider(id, "plate-op", look.plate_opacity, ranges::PLATE_OPACITY, 0.05, "", look_f32(|l, v| l.plate_opacity = v), p)));
            }
            rows.push(row(p, t("Plate corners"), slider(id, "plate-r", look.plate_radius, ranges::PLATE_RADIUS_PX, 1.0, " px", look_f32(|l, v| l.plate_radius = v), p)));
            rows.push(row(p, t("Plate padding"), slider(id, "plate-pad", look.plate_padding, ranges::PLATE_PADDING_PX, 1.0, " px", look_f32(|l, v| l.plate_padding = v), p)));
        }
        rows
    }
}

/// A titled group of rows in the editor.
fn section(title: &str, rows: Vec<AnyElement>, p: &MandaraPalette) -> AnyElement {
    div().row().flex_col().gap(px(SPACE_8_PX)).child(group_title(title.to_string(), p)).children(rows).into_any_element()
}

fn hint(text: &str, p: &MandaraPalette) -> AnyElement {
    div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(text)).into_any_element()
}

fn clock_set<T: 'static>(f: fn(&mut mandara_config::desktop_widgets::ClockWidget, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let WidgetKind::Clock(c) = &mut w.kind {
            f(c, v)
        }
    })
}

/// `fn name<T>(f: fn(&mut Type, T)) -> Set<T>`: edits a widget of kind
/// `Variant`.
macro_rules! kind_setter {
    ($($name:ident: $variant:ident($ty:ty)),* $(,)?) => {$(
        fn $name<T: 'static>(f: fn(&mut $ty, T)) -> Set<T> {
            Rc::new(move |w, v| {
                if let WidgetKind::$variant(x) = &mut w.kind {
                    f(x, v)
                }
            })
        }
    )*};
}

kind_setter!(media_set: Media(dw::MediaWidget), fetch_set: Fetch(dw::FetchWidget), screen_set: ScreenTime(dw::ScreenTimeWidget), prayer_set: Prayer(dw::PrayerWidget), search_set: Search(dw::SearchWidget), glance_set: Glance(dw::GlanceWidget), monitor_set: Monitor(dw::MonitorWidget), profile_set: Profile(dw::ProfileWidget), tiles_set: Shortcuts(dw::ShortcutsWidget), toggles_set: Toggles(dw::TogglesWidget), sliders_set: Sliders(dw::SlidersWidget), palette_set: Palette(dw::PaletteWidget), pet_set: Pet(dw::PetWidget), bubbles_set: Bubbles(dw::BubblesWidget), photo_set: Photo(dw::PhotoWidget), sticker_set: Sticker(dw::StickerWidget), weather_set: Weather(dw::WeatherWidget), date_set: Date(dw::DateWidget), sky_set: Sky(dw::SkyWidget), todo_set: Todo(dw::TodoWidget), note_set: Note(dw::NoteWidget), timer_set: Pomodoro(dw::PomodoroWidget), quote_set: Quote(dw::QuoteWidget), notebook_set: Notebook(dw::NotebookWidget), github_set: Github(dw::GithubWidget), currency_set: Currency(dw::CurrencyWidget), ai_set: AiUsage(dw::AiUsageWidget), games_set: Games(dw::GamesWidget), jannah_set: Jannah(dw::JannahWidget), hadith_set: Hadith(dw::HadithWidget), quran_set: Quran(dw::QuranWidget));

/// A count slider's setter (the slider works in `f32`).
/// Adds or removes `level` from a sliders widget's list (empty = both).
fn set_level_list(levels: &mut Vec<String>, level: &str, on: bool) {
    if levels.is_empty() {
        *levels = vec!["volume".into(), "brightness".into()];
    }
    levels.retain(|l| l != level);
    // One of them always shows (an empty list means both).
    if on || levels.is_empty() {
        levels.push(level.into());
        levels.sort();
        levels.reverse();
    }
}

fn rng((lo, hi): (u32, u32)) -> (f32, f32) {
    (lo as f32, hi as f32)
}

fn as_count(v: f32) -> u32 {
    v.round().max(0.0) as u32
}

fn progress_set<T: 'static>(f: fn(&mut dw::ProgressWidget, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let WidgetKind::Progress(x) = &mut w.kind {
            f(x, v)
        }
    })
}

fn face_set<T: 'static>(f: fn(&mut dw::FaceOptions, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let WidgetKind::Clock(c) = &mut w.kind {
            f(&mut c.face_options, v)
        }
    })
}

fn analog_set<T: 'static>(f: fn(&mut dw::AnalogOptions, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let WidgetKind::Clock(c) = &mut w.kind {
            f(&mut c.analog, v)
        }
    })
}

/// Edits template line `i` of a clock.
fn line_set<T: 'static>(i: usize, f: fn(&mut dw::TemplateLine, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let WidgetKind::Clock(c) = &mut w.kind
            && let Some(l) = c.lines.get_mut(i)
        {
            f(l, v)
        }
    })
}

fn look_set<T: 'static>(f: fn(&mut dw::Look, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let Some(l) = w.kind.look_mut() {
            f(l, v)
        }
    })
}

fn look_f32(f: fn(&mut dw::Look, f32)) -> Set<f32> {
    look_set(f)
}

fn locale_set<T: 'static>(f: fn(&mut TextOptions, T)) -> Set<T> {
    Rc::new(move |w, v| {
        if let Some(l) = w.kind.locale_mut() {
            f(l, v)
        }
    })
}

/// Edits a plugin card's options.
fn plugin_card_edit(w: &mut DesktopWidget, f: impl FnOnce(&mut PluginCardOptions)) {
    if let WidgetKind::Card(c) = &mut w.kind {
        let mut o: PluginCardOptions = dc::options(c);
        f(&mut o);
        dc::set_options(c, &o);
    }
}

/// The language choices: the interface's, then each language by name.
fn languages() -> Vec<(String, String)> {
    let mut out = vec![(String::new(), t("The interface's").to_string())];
    out.extend(Lang::ALL.iter().map(|l| (l.code().to_string(), l.native_name().to_string())));
    out
}

/// Digits, calendar and language of a widget that writes them.
fn text_rows(id: &str, kind: &WidgetKind, l: &TextOptions, p: &MandaraPalette, cx: &mut Context<StudioView>) -> Vec<AnyElement> {
    let mut rows = vec![row(p, t("Numerals"), choice(id, "numerals", l.numerals, &NUMERALS, locale_set(|l, v| l.numerals = v), p, cx))];
    // Dates: the clocks and the countdown.
    if matches!(kind, WidgetKind::Clock(_) | WidgetKind::Countdown(_)) {
        rows.push(row(p, t("Calendar"), choice(id, "calendar", l.calendar, &CALENDARS, locale_set(|l, v| l.calendar = v), p, cx)));
        if l.calendar.hijri() {
            rows.push(row(p, t("Hijri date in Arabic"), toggle(id, "hijri-ar", l.hijri_in_arabic, locale_set(|l, v| l.hijri_in_arabic = v), p, cx)));
        }
    }
    rows.push(row(p, t("Language"), language_choice(id, &l.language, locale_set(|l, v: String| l.language = v), p, cx)));
    rows
}

/// The same for a plugin's widget (told to the plugin).
fn plugin_text_rows(id: &str, l: &TextOptions, p: &MandaraPalette, cx: &mut Context<StudioView>) -> Vec<AnyElement> {
    vec![
        row(p, t("Numerals"), choice(id, "numerals", l.numerals, &NUMERALS, Rc::new(|w, v| plugin_card_edit(w, |o| o.numerals = v)), p, cx)),
        row(p, t("Calendar"), choice(id, "calendar", l.calendar, &CALENDARS, Rc::new(|w, v| plugin_card_edit(w, |o| o.calendar = v)), p, cx)),
        row(p, t("Language"), language_choice(id, &l.language, Rc::new(|w, v: String| plugin_card_edit(w, |o| o.language = v)), p, cx)),
    ]
}

fn language_choice(id: &str, current: &str, set: Set<String>, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    div()
        .row()
        .flex_wrap()
        .gap(px(SPACE_3_PX))
        .p(px(SPACE_3_PX))
        .rounded(px(RADIUS_12_PX))
        .bg(p.surface_container_high.hsla())
        .children(languages().into_iter().map(|(code, name)| {
            let active = code == current;
            let (set, wid) = (set.clone(), id.to_string());
            div()
                .id(SharedString::from(format!("dw-lang-{code}-{id}")))
                .px(px(SPACE_10_PX))
                .h(px(DIM_28_PX))
                .rounded(px(RADIUS_9_PX))
                .row()
                .items_center()
                .cursor_pointer()
                .text_size(px(TEXT_12_PX))
                .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).font_weight(FontWeight::MEDIUM))
                .when(!active, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla())))
                .child(name)
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let (set, code) = (set.clone(), code.clone());
                    edit(cx, &wid, move |w| set(w, code));
                }))
        }))
        .into_any_element()
}

/// Ends a drag: the widget re-anchors to the anchor nearest its centre,
/// keeps its place, and the config is written (once).
fn drop_drag(pw: f32, ph: f32, cx: &mut Context<StudioView>) {
    let Some(d) = cx.global_mut::<PageState>().drag.take() else { return };
    let place = d.place.clone();
    edit(cx, &d.id, move |w| {
        let (left, top) = place.origin((pw, ph), d.size);
        let (cx_f, cy_f) = ((left + d.size.0 / 2.0) / pw, (top + d.size.1 / 2.0) / ph);
        let pick = |f: f32| if f < 1.0 / 3.0 { 0 } else if f > 2.0 / 3.0 { 2 } else { 1 };
        w.place.anchor = Anchor::ALL[pick(cy_f) * 3 + pick(cx_f)];
        let (x, y) = w.place.offset_for((pw, ph), d.size, (left, top));
        w.place.x = x;
        w.place.y = y;
    });
    cx.notify();
}

fn edit_field(id: &str, f: impl Fn(&mut DesktopWidget, String) + 'static) -> impl Fn(&mut ShellConfig, String) + 'static {
    let id = id.to_string();
    move |c, v| {
        dw::migrate_depth_clock(c);
        if let Some(w) = c.desktop.widgets.get_mut(&id) {
            f(w, v);
        }
        dw::sync_depth_clock(c);
    }
}

fn row(p: &MandaraPalette, label: &str, control: AnyElement) -> AnyElement {
    div()
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .child(div().w(px(DIM_150_PX)).flex_none().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(label.to_string()))
        .child(div().flex_1().min_w(px(0.0)).child(control))
        .into_any_element()
}

fn toggle(id: &str, what: &str, on: bool, set: Set<bool>, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let id = id.to_string();
    div()
        .id(SharedString::from(format!("dw-t-{what}-{id}")))
        .cursor_pointer()
        .child(mandara_ui::plugins::network::switch(on, p))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            let set = set.clone();
            edit(cx, &id, move |w| set(w, !on));
        }))
        .into_any_element()
}

/// A slider that writes at most once a frame (the latest value).
#[allow(clippy::too_many_arguments)]
fn slider(id: &str, what: &str, value: f32, (min, max): (f32, f32), step: f32, unit: &str, set: Set<f32>, p: &MandaraPalette) -> AnyElement {
    let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let wid = id.to_string();
    let slider_id = SharedString::from(format!("dw-s-{what}-{id}"));
    // The readout follows the fill while the slider is held.
    let snap = move |f: f32| ((min + (max - min) * f) / step).round() * step;
    let text = {
        let unit = unit.to_string();
        move |v: f32| if step >= 1.0 { format!("{v:.0}{unit}") } else if step >= 0.05 { format!("{v:.2}{unit}") } else { format!("{v:.3}{unit}") }
    };
    let shown = text(mandara_ui::slider_model::held(&gpui::ElementId::Name(slider_id.clone())).map_or(value, snap));
    div()
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .child(div().w(px(TRACK_W_PX)).child(mandara_ui::kit::slider_with_label(slider_id, frac, p, move |f| text(snap(f)).into(), move |f, window, cx| {
            let v = ((min + (max - min) * f) / step).round() * step;
            let set = set.clone();
            edit(cx, &wid, move |w| set(w, v));
            mandara_ui::slider_model::next_frame(window)
        })))
        .child(div().w(px(DIM_76_PX)).text_end().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(shown))
        .into_any_element()
}

fn choice<T: Copy + PartialEq + 'static>(id: &str, what: &str, current: T, options: &[(T, &'static str)], set: Set<T>, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    div()
        .row()
        .flex_wrap()
        .gap(px(SPACE_3_PX))
        .p(px(SPACE_3_PX))
        .rounded(px(RADIUS_12_PX))
        .bg(p.surface_container_high.hsla())
        .children(options.iter().enumerate().map(|(i, (v, label))| {
            let active = *v == current;
            let (v, set, id) = (*v, set.clone(), id.to_string());
            div()
                .id(SharedString::from(format!("dw-c-{what}-{i}-{id}")))
                .px(px(SPACE_10_PX))
                .h(px(DIM_28_PX))
                .rounded(px(RADIUS_9_PX))
                .row()
                .items_center()
                .cursor_pointer()
                .text_size(px(TEXT_12_PX))
                .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).font_weight(FontWeight::MEDIUM))
                .when(!active, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla())))
                .child(tr(label).to_string())
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let set = set.clone();
                    edit(cx, &id, move |w| set(w, v));
                }))
        }))
        .into_any_element()
}

fn anchors(id: &str, current: Anchor, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let cells = Anchor::ALL.into_iter().zip(ANCHOR_GLYPHS).map(|(a, glyph)| {
        let on = a == current;
        let id = id.to_string();
        div()
            .id(SharedString::from(format!("dw-a-{a:?}-{id}")))
            .size(px(DIM_24_PX))
            .rounded(px(RADIUS_6_PX))
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_size(px(TEXT_12_PX))
            .when(on, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()))
            .when(!on, |el| el.bg(p.surface_container_high.hsla()).text_color(p.on_surface_variant.hsla()))
            .child(glyph)
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                // Keep the widget where it is: recompute the offset for the new anchor.
                edit(cx, &id, move |w| {
                    let (fx, fy) = w.place.anchor.fraction();
                    let (nx, ny) = a.fraction();
                    w.place.x += fx - nx;
                    w.place.y += fy - ny;
                    w.place.anchor = a;
                });
            }))
    });
    div().w(px(DIM_88_PX)).row().flex_wrap().gap(px(SPACE_3_PX)).children(cells).into_any_element()
}

fn displays(id: &str, current: &[String], cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let mut names: Vec<String> = cfg.displays.profiles.iter().flat_map(|pr| pr.outputs.iter().map(|o| o.name.clone())).chain(cfg.displays.primary.clone()).collect();
    names.extend(current.iter().cloned());
    names.sort();
    names.dedup();
    let all = current.is_empty();
    type EditList = Rc<dyn Fn(&mut Vec<String>)>;
    let chip = |label: String, on: bool, idk: String, f: EditList, cx: &mut Context<StudioView>| {
        let id = id.to_string();
        div()
            .id(SharedString::from(format!("dw-d-{idk}-{id}")))
            .px(px(SPACE_10_PX))
            .h(px(DIM_28_PX))
            .rounded_full()
            .row()
            .items_center()
            .cursor_pointer()
            .text_size(px(TEXT_12_PX))
            .when(on, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()))
            .when(!on, |el| el.bg(p.surface_container_high.hsla()).text_color(p.on_surface_variant.hsla()))
            .child(label)
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let f = f.clone();
                edit(cx, &id, move |w| f(&mut w.place.displays));
            }))
            .into_any_element()
    };
    let mut chips = vec![chip(t("Every display").to_string(), all, "all".into(), Rc::new(|d: &mut Vec<String>| d.clear()), cx)];
    for n in names {
        let on = current.iter().any(|c| c == &n);
        let name = n.clone();
        chips.push(chip(n.clone(), on, n, Rc::new(move |d: &mut Vec<String>| {
            if let Some(i) = d.iter().position(|x| *x == name) {
                d.remove(i);
            } else {
                d.push(name.clone());
            }
        }), cx));
    }
    div().row().flex_wrap().gap(px(SPACE_4_PX)).children(chips).into_any_element()
}

fn monitor_rows(id: &str, rows: &[MonitorRow], p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let chips = MonitorRow::ALL.into_iter().map(|r| {
        let on = rows.contains(&r);
        let id = id.to_string();
        div()
            .id(SharedString::from(format!("dw-r-{r:?}-{id}")))
            .px(px(SPACE_10_PX))
            .h(px(DIM_28_PX))
            .rounded_full()
            .row()
            .items_center()
            .cursor_pointer()
            .text_size(px(TEXT_12_PX))
            .when(on, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()))
            .when(!on, |el| el.bg(p.surface_container_high.hsla()).text_color(p.on_surface_variant.hsla()))
            .child(tr(row_label(r)).to_string())
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                edit(cx, &id, move |w| {
                    if let WidgetKind::Monitor(m) = &mut w.kind {
                        if let Some(i) = m.rows.iter().position(|x| *x == r) {
                            m.rows.remove(i);
                        } else {
                            // Keep the canonical order.
                            m.rows.push(r);
                            m.rows.sort_by_key(|x| MonitorRow::ALL.iter().position(|y| y == x));
                        }
                    }
                });
            }))
    });
    div().row().flex_wrap().gap(px(SPACE_4_PX)).children(chips).into_any_element()
}
