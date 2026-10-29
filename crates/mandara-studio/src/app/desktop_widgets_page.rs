//! Studio → Desktop widgets: add, arrange and style the
//! widgets painted on the wallpaper (`[desktop.widgets]`).
//!
//! * The preview is a scaled display: the wallpaper's thumbnail with every
//!   widget drawn by the shell's own painters on top. Drag a widget to move
//!   it; on release it re-anchors to the nearest of the nine anchors and
//!   keeps its place. Click to select it and edit it below.
//! * The gallery adds a clock face (live thumbnails from the same
//!   painters), the system monitor, a visualizer shape or a decoration
//!   preset. Everything starts on top of the list (painted last).
//! * Each edit goes straight to the config (the shell repaints the plane at
//!   once); the depth clock keeps the old `[wallpaper]` keys in step.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, RenderImage, SharedString,
    StatefulInteractiveElement, Styled, canvas, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::desktop_widgets::{
    self as dw, Anchor, ClockFace, ClockWidget, DecorShape, Decoration, Depth, DesktopWidget, DesktopWidgetsConfig, Effect, FontRole, MonitorRow, MonitorWidget, Placement, PlaneVisualizer, Preset, RingProgress, Sideways, Tone,
    VisualizerShape, WidgetKind, ranges,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::backdrop::paint::FontNames;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::lock::paint::TextPainter;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{button, card, group_title, heading};
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
/// A new radial visualizer: inner diameter and spike length, reference px.
const RADIAL_DIAMETER_PX: f32 = 260.0;
const RADIAL_SPIKE_PX: f32 = 60.0;
/// Opacity of a switched-off widget's chip.
const DISABLED_OPACITY: f32 = 0.55;
/// Track width of the sliders.
const TRACK_W_PX: f32 = DIM_220_PX;

/// The page's state (kept across renders; one Studio process).
#[derive(Default)]
struct PageState {
    selected: Option<String>,
    drag: Option<Drag>,
    painter: Option<TextPainter>,
    tiles: HashMap<u64, Arc<RenderImage>>,
    /// Widgets overlay of the preview, keyed by the widgets' hash, and the
    /// boxes (fractions of the frame).
    overlay: Option<(u64, Arc<RenderImage>, Vec<mandara_ui::backdrop::widgets::WidgetBox>)>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    thumb: Option<(std::path::PathBuf, std::path::PathBuf)>,
    thumb_loading: bool,
}

impl Global for PageState {}

/// A widget being dragged: the pointer's offset inside its box and the box
/// size, in preview pixels.
#[derive(Clone)]
struct Drag {
    id: String,
    grab: (f32, f32),
    size: (f32, f32),
}

fn key(v: impl std::fmt::Debug) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}

use mandara_ui::backdrop::widgets::render_image;

fn fonts(cfg: &ShellConfig) -> FontNames {
    let t = &cfg.typography;
    FontNames { ui: t.ui_font.clone(), display: t.display_font.clone(), arabic: t.arabic_font.clone(), mono: t.mono_font.clone() }
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

fn face_label(f: ClockFace) -> &'static str {
    match f {
        ClockFace::Stacked => tk("Depth · stacked"),
        ClockFace::Classic => tk("Depth · classic"),
        ClockFace::Arabic => tk("Depth · Arabic"),
        ClockFace::Superscript => tk("Superscript"),
        ClockFace::Headline => tk("Headline"),
        ClockFace::DayBlock => tk("Day block"),
        ClockFace::Ring => tk("Ring"),
        ClockFace::Greeting => tk("Greeting"),
        ClockFace::DayNumeral => tk("Day numeral"),
        ClockFace::Minimal => tk("Minimal"),
    }
}

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
    }
}

fn vis_label(s: VisualizerShape) -> &'static str {
    match s {
        VisualizerShape::Bars => tk("Bars"),
        VisualizerShape::Mirrored => tk("Mirrored bars"),
        VisualizerShape::Wave => tk("Wave"),
        VisualizerShape::Radial => tk("Radial"),
    }
}

fn preset_label(p: Preset) -> &'static str {
    match p {
        Preset::AlexandriaLine => tk("Alexandria line"),
        Preset::VerticalPoem => tk("Vertical poem"),
        Preset::Geometric => tk("Geometric set"),
    }
}

fn widget_label(w: &DesktopWidget) -> String {
    match &w.kind {
        WidgetKind::Clock(c) => tr(face_label(c.face)).to_string(),
        WidgetKind::Monitor(_) => t("System monitor").to_string(),
        WidgetKind::Decoration(d) if d.shape == DecorShape::Text => {
            let first: String = d.text.lines().next().unwrap_or_default().chars().take(ranges::REPEAT_DEFAULT as usize * 3).collect();
            if first.is_empty() { tr(shape_label(d.shape)).to_string() } else { first }
        }
        WidgetKind::Decoration(d) => tr(shape_label(d.shape)).to_string(),
        WidgetKind::Visualizer(v) => tr(vis_label(v.shape)).to_string(),
        WidgetKind::Card(c) => c.card.clone(),
    }
}

const ANCHOR_GLYPHS: [&str; 9] = ["↖", "↑", "↗", "←", "•", "→", "↙", "↓", "↘"];
const TONES: [(Tone, &str); 4] = [(Tone::Auto, tk("Automatic")), (Tone::OnSurface, tk("Text colour")), (Tone::Accent, tk("Accent")), (Tone::Custom, tk("Custom"))];
const FONTS: [(FontRole, &str); 4] = [(FontRole::Display, tk("Display")), (FontRole::Ui, tk("Interface")), (FontRole::Arabic, tk("Arabic")), (FontRole::Mono, tk("Monospace"))];
const EFFECTS: [(Effect, &str); 3] = [(Effect::None, tk("None")), (Effect::Shadow, tk("Shadow")), (Effect::Glow, tk("Glow"))];
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
            .child(heading(t("Desktop widgets"), t("Clocks, a system monitor, decorations and an audio visualizer drawn on the wallpaper — behind its subject or in front. Nothing runs until you add one."), p))
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

    fn widgets_preview(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (pw, ph) = (PREVIEW_W_PX, PREVIEW_W_PX * PREVIEW_ASPECT);
        // The wallpaper thumbnail, off the UI thread once.
        let wall = cfg.wallpaper.path.clone();
        let state = cx.global::<PageState>();
        let have_thumb = state.thumb.as_ref().is_some_and(|(w, _)| Some(w) == wall.as_ref());
        if let Some(path) = wall.clone().filter(|_| !have_thumb && !state.thumb_loading) {
            cx.global_mut::<PageState>().thumb_loading = true;
            let task = cx.background_executor().spawn(async move { mandara_ui::wallpaper::thumbnail(&path).ok().map(|t| (path, t)) });
            cx.spawn(async move |this, cx| {
                let thumb = task.await;
                let _ = this.update(cx, |_, cx| {
                    let s = cx.global_mut::<PageState>();
                    s.thumb_loading = false;
                    s.thumb = thumb;
                    cx.notify();
                });
            })
            .detach();
        }
        // The widgets drawn by the shell's painters, re-rendered on change.
        let painted: Vec<DesktopWidget> = cfg.desktop.widgets.items.iter().filter(|w| w.enabled && w.kind.on_wallpaper_plane()).cloned().collect();
        let primary = mandara_core::theme_entity(cx).read(cx).target().primary;
        let fonts = fonts(cfg);
        let k = key((&painted, primary.to_rgba8(), &fonts, mandara_i18n::language()));
        let s = cx.global_mut::<PageState>();
        if s.overlay.as_ref().is_none_or(|(old, _, _)| *old != k) {
            let text = s.painter.get_or_insert_with(TextPainter::new);
            let size = ((pw * PREVIEW_DENSITY) as u32, (ph * PREVIEW_DENSITY) as u32);
            if let Some((rgba, boxes)) = mandara_ui::backdrop::widgets::preview(&painted, size, &fonts, primary, text) {
                text.release_glyphs();
                s.overlay = Some((k, render_image(rgba), boxes));
            }
        }
        let s = cx.global::<PageState>();
        let thumb = s.thumb.as_ref().map(|(_, t)| t.clone());
        let (overlay, boxes) = match &s.overlay {
            Some((_, img, boxes)) => (Some(img.clone()), boxes.clone()),
            None => (None, Vec::new()),
        };
        let selected = s.selected.clone();
        let bounds = s.bounds.clone();
        let boxes_el = boxes.into_iter().map(|(id, [x, y, w, h])| {
            let on = selected.as_deref() == Some(id.as_str());
            let grab_id = id.clone();
            div()
                .id(SharedString::from(format!("dw-box-{id}")))
                .absolute()
                .left(px(x * pw))
                .top(px(y * ph))
                .w(px((w * pw).max(DIM_8_PX)))
                .h(px((h * ph).max(DIM_8_PX)))
                .rounded(px(RADIUS_6_PX))
                .border_1()
                .border_color(if on { p.primary.hsla() } else { p.on_surface.with_alpha(BOX_ALPHA).hsla() })
                .when(on, |el| el.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                .cursor_grab()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, e: &MouseDownEvent, _, cx| {
                        let b = cx.global::<PageState>().bounds.get();
                        let (ox, oy) = (f32::from(e.position.x - b.origin.x), f32::from(e.position.y - b.origin.y));
                        let s = cx.global_mut::<PageState>();
                        s.selected = Some(grab_id.clone());
                        s.drag = Some(Drag { id: grab_id.clone(), grab: (ox - x * pw, oy - y * ph), size: (w * pw, h * ph) });
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
        });
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
            .when_some(overlay, |el, o| {
                use gpui::StyledImage as _;
                // Rendered at PREVIEW_DENSITY: shown at the preview's size.
                el.child(img(o).absolute().top_0().left_0().w(px(pw)).h(px(ph)).object_fit(gpui::ObjectFit::Fill))
            })
            .children(boxes_el)
            .on_mouse_move(cx.listener(move |_, e: &MouseMoveEvent, _, cx| {
                let Some(d) = cx.global::<PageState>().drag.clone() else { return };
                let b = cx.global::<PageState>().bounds.get();
                let left = f32::from(e.position.x - b.origin.x) - d.grab.0;
                let top = f32::from(e.position.y - b.origin.y) - d.grab.1;
                edit(cx, &d.id, move |w| {
                    let (x, y) = w.place.offset_for((pw, ph), d.size, (left, top));
                    w.place.x = x;
                    w.place.y = y;
                });
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(move |_, _: &MouseUpEvent, _, cx| drop_drag(pw, ph, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(move |_, _: &MouseUpEvent, _, cx| drop_drag(pw, ph, cx)))
            .into_any_element()
    }

    fn widgets_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let primary = mandara_core::theme_entity(cx).read(cx).target().primary;
        let fonts = fonts(cfg);
        let mut tiles = Vec::new();
        let has_depth = cfg.desktop.widgets.depth_clock().is_some() || cfg.desktop.widgets.items.iter().any(|w| matches!(&w.kind, WidgetKind::Clock(c) if c.face.depth_style().is_some()));
        let faces = ClockFace::ALL.into_iter().filter(|f| f.depth_style().is_none() || (*f == ClockFace::Stacked && !has_depth));
        let mut entries: Vec<(String, String, Vec<DesktopWidget>)> = Vec::new();
        for f in faces {
            let w = DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face: f, ..ClockWidget::default() }), ..DesktopWidget::default() };
            entries.push((format!("face-{}", f.id()), tr(face_label(f)).to_string(), vec![w]));
        }
        entries.push(("monitor".into(), t("System monitor").into(), vec![DesktopWidget { kind: WidgetKind::Monitor(MonitorWidget::default()), place: Placement { anchor: Anchor::TopRight, x: -0.03, y: 0.06, ..Placement::default() }, ..DesktopWidget::default() }]));
        for s in [VisualizerShape::Bars, VisualizerShape::Mirrored, VisualizerShape::Wave, VisualizerShape::Radial] {
            // A radial ring starts small: its plane is a square around it.
            let v = match s {
                VisualizerShape::Radial => PlaneVisualizer { shape: s, length: RADIAL_DIAMETER_PX, height: RADIAL_SPIKE_PX, ..PlaneVisualizer::default() },
                _ => PlaneVisualizer { shape: s, ..PlaneVisualizer::default() },
            };
            entries.push((format!("vis-{s:?}"), tr(vis_label(s)).to_string(), vec![DesktopWidget { kind: WidgetKind::Visualizer(v), ..DesktopWidget::default() }]));
        }
        for pr in Preset::ALL {
            entries.push((format!("preset-{pr:?}"), tr(preset_label(pr)).to_string(), pr.widgets()));
        }
        for (id, label, widgets) in entries {
            let k = key((&id, primary.to_rgba8(), &fonts, mandara_i18n::language()));
            if !cx.global::<PageState>().tiles.contains_key(&k) {
                let s = cx.global_mut::<PageState>();
                let text = s.painter.get_or_insert_with(TextPainter::new);
                // Centred and enlarged so it fills the tile.
                let shown: Vec<DesktopWidget> = widgets
                    .iter()
                    .map(|w| {
                        let mut w = w.clone();
                        if widgets.len() == 1 {
                            w.place = Placement { scale: TILE_SCALE, ..Placement::default() };
                        }
                        w.id = id.clone();
                        w
                    })
                    .collect();
                if let Some((rgba, _)) = mandara_ui::backdrop::widgets::preview(&shown, TILE_RENDER, &fonts, primary, text) {
                    text.release_glyphs();
                    s.tiles.insert(k, render_image(rgba));
                }
            }
            let tile = cx.global::<PageState>().tiles.get(&k).cloned();
            // Already on the desktop (one widget, same settings): the tile
            // selects it instead of stacking an identical copy on top.
            let existing = match widgets.as_slice() {
                [only] => cfg.desktop.widgets.find_same(&only.kind).map(|w| w.id.clone()),
                _ => None,
            };
            let on_desktop = existing.is_some();
            let add_widgets = widgets.clone();
            tiles.push(
                div()
                    .id(SharedString::from(format!("dw-add-{id}")))
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
                            .when_some(tile, |el, t| {
                                use gpui::StyledImage as _;
                                el.child(img(t).absolute().top_0().left_0().w(px(TILE_W_PX)).h(px(TILE_H_PX)).object_fit(gpui::ObjectFit::Contain))
                            })
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
                            None => add(cx, add_widgets.clone()),
                        };
                        cx.global_mut::<PageState>().selected = id;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let full = cfg.desktop.widgets.items.len() >= ranges::MAX_WIDGETS;
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(group_title(if full { t("The list is full — remove a widget to add another") } else { t("Add a widget") }, p))
            .child(div().row().flex_wrap().gap(px(SPACE_10_PX)).children(tiles))
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
                let base = widget_label(w);
                let n = seen.entry(base.clone()).and_modify(|n| *n += 1).or_insert(1);
                if *n > 1 { format!("{base} {n}") } else { base }
            })
            .collect();
        let duplicates = DesktopWidgetsConfig { items: items.to_vec(), ..DesktopWidgetsConfig::default() }.duplicates().len();
        let chips = items.iter().zip(names).map(|(w, name)| {
            let on = selected == Some(w.id.as_str());
            let (sel, tog, del) = (w.id.clone(), w.id.clone(), w.id.clone());
            let enabled = w.enabled;
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
                .when(!enabled, |el| el.opacity(DISABLED_OPACITY))
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
        });
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
        // Placement.
        let mut rows: Vec<AnyElement> = vec![
            row(p, t("Depth"), choice(&id, "depth", w.place.depth, &[(Depth::Behind, tk("Behind the subject")), (Depth::Front, tk("In front"))], Rc::new(|w, v| w.place.depth = v), p, cx)),
            row(p, t("Anchor"), anchors(&id, w.place.anchor, p, cx)),
            row(p, t("Horizontal offset"), slider(&id, "x", w.place.x, ranges::OFFSET, 0.005, "", Rc::new(|w, v| w.place.x = v), p, cx)),
            row(p, t("Vertical offset"), slider(&id, "y", w.place.y, ranges::OFFSET, 0.005, "", Rc::new(|w, v| w.place.y = v), p, cx)),
            row(p, t("Size"), slider(&id, "scale", w.place.scale, ranges::SCALE, 0.05, "×", Rc::new(|w, v| w.place.scale = v), p, cx)),
            row(p, t("Opacity"), slider(&id, "opacity", w.place.opacity, ranges::OPACITY, 0.05, "", Rc::new(|w, v| w.place.opacity = v), p, cx)),
            row(p, t("Displays"), displays(&id, &w.place.displays, cfg, p, cx)),
        ];
        match &w.kind {
            WidgetKind::Clock(c) => {
                let faces: Vec<(ClockFace, &'static str)> = ClockFace::ALL.iter().map(|f| (*f, face_label(*f))).collect();
                rows.push(row(p, t("Style"), choice(&id, "face", c.face, &faces, Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.face = v }), p, cx)));
                if c.face == ClockFace::Superscript {
                    rows.push(row(p, t("Seconds"), toggle(&id, "seconds", c.seconds, Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.seconds = v }), p, cx)));
                }
                if c.face == ClockFace::Ring {
                    rows.push(row(p, t("Ring shows"), choice(&id, "ring", c.ring_progress, &[(RingProgress::Hour, tk("The hour")), (RingProgress::Day, tk("The day"))], Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.ring_progress = v }), p, cx)));
                }
                if c.face.depth_style().is_some() {
                    rows.push(row(p, t("Date pill"), toggle(&id, "date", c.date, Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.date = v }), p, cx)));
                    rows.push(row(p, t("Audio ring"), toggle(&id, "ring-audio", c.audio_ring, Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.audio_ring = v }), p, cx)));
                } else {
                    rows.push(row(p, t("Eastern Arabic numerals"), toggle(&id, "eastern", c.eastern_numerals, Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.eastern_numerals = v }), p, cx)));
                }
                rows.push(row(p, t("Weather"), toggle(&id, "weather", c.weather, Rc::new(|w, v| if let WidgetKind::Clock(c) = &mut w.kind { c.weather = v }), p, cx)));
                if c.face.depth_style().is_none() {
                    rows.extend(self.look_rows(&id, &c.look, p, cx));
                }
            }
            WidgetKind::Monitor(m) => {
                rows.push(row(p, t("Rows"), monitor_rows(&id, &m.rows, p, cx)));
                rows.push(row(p, t("Refresh"), slider(&id, "interval", m.interval_s as f32, (ranges::MONITOR_INTERVAL_S.0 as f32, ranges::MONITOR_INTERVAL_S.1 as f32), 1.0, " s", Rc::new(|w, v| if let WidgetKind::Monitor(m) = &mut w.kind { m.interval_s = v as u32 }), p, cx)));
                rows.push(row(p, t("Width"), slider(&id, "width", m.width, ranges::MONITOR_WIDTH_PX, 2.0, " px", Rc::new(|w, v| if let WidgetKind::Monitor(m) = &mut w.kind { m.width = v }), p, cx)));
                let title = self.list_field(format!("dw-title-{id}"), &m.title, tk("Title (optional)"), Rc::new(edit_field(&id, |w, v| if let WidgetKind::Monitor(m) = &mut w.kind { m.title = v })), cx);
                rows.push(row(p, t("Title"), div().w(px(TRACK_W_PX)).child(title).into_any_element()));
                rows.extend(self.look_rows(&id, &m.look, p, cx));
            }
            WidgetKind::Decoration(d) => {
                let shapes: Vec<(DecorShape, &'static str)> = [DecorShape::Text, DecorShape::Disc, DecorShape::Circle, DecorShape::Stripes, DecorShape::Waves, DecorShape::Rule, DecorShape::Frame].iter().map(|s| (*s, shape_label(*s))).collect();
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
                    rows.push(row(p, t("Text size"), slider(&id, "size", d.size, ranges::TEXT_SIZE_PX, 1.0, " px", set(|d, v| d.size = v), p, cx)));
                    rows.push(row(p, t("Weight"), slider(&id, "weight", d.weight as f32, (ranges::WEIGHT.0 as f32, ranges::WEIGHT.1 as f32), 100.0, "", set(|d, v| d.weight = v as u16), p, cx)));
                    rows.push(row(p, t("Letter spacing"), slider(&id, "tracking", d.tracking, ranges::TRACKING_EM, 0.01, " em", set(|d, v| d.tracking = v), p, cx)));
                } else {
                    rows.push(row(p, t("Width"), slider(&id, "w", d.width, ranges::SHAPE_SIZE_PX, 2.0, " px", set(|d, v| d.width = v), p, cx)));
                    if matches!(d.shape, DecorShape::Stripes | DecorShape::Waves | DecorShape::Frame) {
                        rows.push(row(p, t("Height"), slider(&id, "h", d.height, ranges::SHAPE_SIZE_PX, 2.0, " px", set(|d, v| d.height = v), p, cx)));
                    }
                    if matches!(d.shape, DecorShape::Stripes | DecorShape::Waves) {
                        rows.push(row(p, t("Lines"), slider(&id, "count", d.count as f32, (ranges::REPEAT.0 as f32, ranges::REPEAT.1 as f32), 1.0, "", set(|d, v| d.count = v as u32), p, cx)));
                    }
                    if matches!(d.shape, DecorShape::Circle | DecorShape::Waves | DecorShape::Rule | DecorShape::Frame) {
                        rows.push(row(p, t("Line width"), slider(&id, "stroke", d.stroke, ranges::STROKE_PX, 0.5, " px", set(|d, v| d.stroke = v), p, cx)));
                    }
                }
                rows.push(row(p, t("Rotation"), slider(&id, "rot", d.rotation, ranges::ANGLE_DEG, 1.0, "°", set(|d, v| d.rotation = v), p, cx)));
                rows.extend(self.look_rows(&id, &d.look, p, cx));
            }
            WidgetKind::Visualizer(v) => {
                let shapes: Vec<(VisualizerShape, &'static str)> = [VisualizerShape::Bars, VisualizerShape::Mirrored, VisualizerShape::Wave, VisualizerShape::Radial].iter().map(|s| (*s, vis_label(*s))).collect();
                rows.push(row(p, t("Shape"), choice(&id, "vshape", v.shape, &shapes, Rc::new(|w, x| if let WidgetKind::Visualizer(v) = &mut w.kind { v.shape = x }), p, cx)));
                let set = |f: fn(&mut PlaneVisualizer, f32)| -> Set<f32> { Rc::new(move |w, x| if let WidgetKind::Visualizer(v) = &mut w.kind { f(v, x) }) };
                rows.push(row(p, t("Bars"), slider(&id, "bars", v.bars as f32, (ranges::VISUALIZER_BARS.0 as f32, ranges::VISUALIZER_BARS.1 as f32), 1.0, "", set(|v, x| v.bars = x as u32), p, cx)));
                rows.push(row(p, if v.shape == VisualizerShape::Radial { t("Diameter") } else { t("Length") }, slider(&id, "len", v.length, ranges::VISUALIZER_LENGTH_PX, 2.0, " px", set(|v, x| v.length = x), p, cx)));
                rows.push(row(p, t("Height"), slider(&id, "vh", v.height, ranges::VISUALIZER_HEIGHT_PX, 1.0, " px", set(|v, x| v.height = x), p, cx)));
                rows.push(row(p, t("Angle"), slider(&id, "angle", v.angle, ranges::ANGLE_DEG, 1.0, "°", set(|v, x| v.angle = x), p, cx)));
                rows.push(row(p, t("Gap"), slider(&id, "gap", v.gap, ranges::VISUALIZER_GAP, 0.05, "", set(|v, x| v.gap = x), p, cx)));
                rows.push(row(p, t("Frame rate"), slider(&id, "fps", v.fps as f32, (ranges::VISUALIZER_FPS.0 as f32, ranges::VISUALIZER_FPS.1 as f32), 1.0, " fps", set(|v, x| v.fps = x as u32), p, cx)));
                rows.push(row(p, t("Stop after silence"), slider(&id, "silence", v.silence_s as f32, (ranges::VISUALIZER_SILENCE_S.0 as f32, ranges::VISUALIZER_SILENCE_S.1 as f32), 1.0, " s", set(|v, x| v.silence_s = x as u32), p, cx)));
                rows.extend(self.look_rows(&id, &v.look, p, cx));
            }
            WidgetKind::Card(_) => rows.push(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("An interactive card — its own settings are with the cards.")).into_any_element()),
        }
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
            .child(group_title(widget_label(w), p))
            .children(rows)
            .into_any_element()
    }

    fn look_rows(&mut self, id: &str, look: &dw::Look, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        fn on_look(w: &mut DesktopWidget) -> Option<&mut dw::Look> {
            match &mut w.kind {
                WidgetKind::Clock(c) => Some(&mut c.look),
                WidgetKind::Monitor(m) => Some(&mut m.look),
                WidgetKind::Decoration(d) => Some(&mut d.look),
                WidgetKind::Visualizer(v) => Some(&mut v.look),
                WidgetKind::Card(_) => None,
            }
        }
        let mut rows = vec![
            row(p, t("Font"), choice(id, "font", look.font, &FONTS, Rc::new(|w, v| if let Some(l) = on_look(w) { l.font = v }), p, cx)),
            row(p, t("Colour"), choice(id, "tone", look.tone, &TONES, Rc::new(|w, v| if let Some(l) = on_look(w) { l.tone = v }), p, cx)),
        ];
        if look.tone == Tone::Custom {
            let f = self.list_field(format!("dw-color-{id}"), &look.custom_color, "#e8d9b0 / oklch(0.85 0.06 85)", Rc::new(edit_field(id, |w, v| if let Some(l) = on_look(w) { l.custom_color = v })), cx);
            rows.push(row(p, t("Custom colour"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
        }
        rows.push(row(p, t("Accent"), choice(id, "accent", look.accent, &TONES, Rc::new(|w, v| if let Some(l) = on_look(w) { l.accent = v }), p, cx)));
        if look.accent == Tone::Custom {
            let f = self.list_field(format!("dw-accent-{id}"), &look.custom_accent, "#4fd1c5 / oklch(0.78 0.12 190)", Rc::new(edit_field(id, |w, v| if let Some(l) = on_look(w) { l.custom_accent = v })), cx);
            rows.push(row(p, t("Custom accent"), div().w(px(TRACK_W_PX)).child(f).into_any_element()));
        }
        rows.push(row(p, t("Effect"), choice(id, "effect", look.effect, &EFFECTS, Rc::new(|w, v| if let Some(l) = on_look(w) { l.effect = v }), p, cx)));
        rows
    }
}

/// Ends a drag: the widget re-anchors to the anchor nearest its centre and
/// keeps its place.
fn drop_drag(pw: f32, ph: f32, cx: &mut Context<StudioView>) {
    let Some(d) = cx.global_mut::<PageState>().drag.take() else { return };
    edit(cx, &d.id, move |w| {
        let (left, top) = w.place.origin((pw, ph), d.size);
        let (cx_f, cy_f) = ((left + d.size.0 / 2.0) / pw, (top + d.size.1 / 2.0) / ph);
        let pick = |f: f32| if f < 1.0 / 3.0 { 0 } else if f > 2.0 / 3.0 { 2 } else { 1 };
        w.place.anchor = Anchor::ALL[pick(cy_f) * 3 + pick(cx_f)];
        let (x, y) = w.place.offset_for((pw, ph), d.size, (left, top));
        w.place.x = x;
        w.place.y = y;
    });
    cx.notify();
}

fn edit_field(id: &str, f: fn(&mut DesktopWidget, String)) -> impl Fn(&mut ShellConfig, String) + 'static {
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

#[allow(clippy::too_many_arguments)]
fn slider(id: &str, what: &str, value: f32, (min, max): (f32, f32), step: f32, unit: &str, set: Set<f32>, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let wid = id.to_string();
    let shown = if step >= 1.0 { format!("{value:.0}{unit}") } else if step >= 0.05 { format!("{value:.2}{unit}") } else { format!("{value:.3}{unit}") };
    let _ = cx;
    div()
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .child(div().w(px(TRACK_W_PX)).child(mandara_ui::kit::slider(SharedString::from(format!("dw-s-{what}-{id}")), frac, p, move |f, _, cx| {
            let v = ((min + (max - min) * f) / step).round() * step;
            let set = set.clone();
            edit(cx, &wid, move |w| set(w, v));
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
