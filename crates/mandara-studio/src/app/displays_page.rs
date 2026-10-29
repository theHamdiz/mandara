//! Studio → Displays: arrangement canvas (drag to place, edges snap),
//! per-output mode / refresh / scale / rotation / VRR / on-off, "apply with
//! revert countdown", saved profiles (auto-applied on hotplug by the shell),
//! persistence to the compositor drop-ins, which displays show the bar /
//! OSD / notifications / wallpaper, external-monitor brightness over DDC/CI,
//! and the night light.
//!
//! Display changes go through `wlr-output-management` from this process
//! (the countdown thread lives here too); night-light state is the
//! shell's, read with `mandara msg nightlight status`.

use mandara_ui::bidi::Bidi as _;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};
use mandara_compositor::ClientInfo;
use mandara_config::ShellConfig;
use mandara_config::compositor::Target;
use mandara_config::outputs::{DisplayProfile, NightSchedule, OutputConfig, ScreenChoice, SurfaceKind, Transform, VrrMode, user_output_rules};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_services::gamma::GammaSupport;
use mandara_services::{ddc, outputs};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;
use std::sync::Mutex;

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title, heading, tip};
use crate::registry::{self, Section};
use crate::tokens::*;

// ── Page constants ──────────────────────────────────────────────────────────

/// Arrangement canvas height and the padding around the monitors in it.
const CANVAS_H_PX: f32 = 240.0;
const CANVAS_PAD_PX: f32 = 28.0;
/// Edges closer than this (logical px) snap together while dragging.
const SNAP_LOGICAL_PX: f32 = 48.0;
/// Fallback logical size of an output with no known mode.
const FALLBACK_W: f32 = 1920.0;
const FALLBACK_H: f32 = 1080.0;
/// Scales offered as chips (the current one is added when different).
const SCALES: &[f32] = &[1.0, 1.25, 1.5, 1.75, 2.0];
/// Resolutions and refresh rates shown before the list is cut.
const MAX_RESOLUTIONS: usize = 8;
const MAX_RATES: usize = 6;
/// Scales closer than this are the same chip.
const SCALE_EPSILON: f32 = 0.005;
/// Re-read outputs this often while the page shows (hotplug, other tools).
const HEADS_REFRESH: Duration = Duration::from_secs(10);
/// Night-light status refresh.
const NIGHT_REFRESH: Duration = Duration::from_secs(5);
/// Page tick (countdown text, refreshes).
const TICK: Duration = Duration::from_millis(500);
/// Millihertz per hertz.
const MHZ_PER_HZ: f32 = 1000.0;
/// Percent per whole.
const PERCENT: f32 = 100.0;
/// Preview length of the night temperature, seconds.
const PREVIEW_SECONDS: &str = "4";

/// One DDC monitor's slider: latest wanted percent while a write runs.
type DdcWants = Arc<Mutex<HashMap<String, u32>>>;

#[derive(Default)]
struct DisplaysStudio {
    heads: Option<Result<Vec<outputs::Head>, String>>,
    heads_at: Option<Instant>,
    /// Edited arrangement (one entry per head, same order).
    draft: Vec<OutputConfig>,
    selected: usize,
    /// Dragging output `index` from pointer `start` (window px) and its
    /// logical position then.
    drag: Option<(usize, gpui::Point<gpui::Pixels>, (i32, i32))>,
    busy: bool,
    message: Option<(String, bool)>,
    /// A tested change reverts at this instant unless kept.
    revert_at: Option<Instant>,
    support: Option<GammaSupport>,
    night: Option<serde_json::Value>,
    night_at: Option<Instant>,
    ddc: Option<(ddc::Access, Vec<ddc::DdcMonitor>)>,
    ddc_wants: DdcWants,
    ddc_shown: HashMap<String, u32>,
    clients: Option<Vec<String>>,
    user_rules: Option<BTreeSet<String>>,
    /// The page loop runs.
    alive: bool,
}
impl Global for DisplaysStudio {}

fn st(cx: &mut gpui::App) -> &mut DisplaysStudio {
    cx.default_global::<DisplaysStudio>()
}

// ── Pure geometry (unit-tested) ─────────────────────────────────────────────

/// Logical rectangle `(x, y, w, h)` of an output config.
pub(crate) fn logical_rect(c: &OutputConfig) -> (f32, f32, f32, f32) {
    let s = c.scale.filter(|s| *s > 0.0).unwrap_or(1.0);
    let (w, h) = (c.width.map(|w| w as f32).unwrap_or(FALLBACK_W) / s, c.height.map(|h| h as f32).unwrap_or(FALLBACK_H) / s);
    let (w, h) = if c.transform.is_some_and(Transform::swaps_axes) { (h, w) } else { (w, h) };
    (c.x.unwrap_or(0) as f32, c.y.unwrap_or(0) as f32, w, h)
}

/// Maps logical space onto the canvas: `(scale, origin_x, origin_y)` such
/// that canvas = (logical − origin) × scale + centring offset.
pub(crate) fn fit_canvas(rects: &[(f32, f32, f32, f32)], w: f32, h: f32, pad: f32) -> (f32, f32, f32) {
    if rects.is_empty() {
        return (1.0, 0.0, 0.0);
    }
    let (min_x, min_y) = rects.iter().fold((f32::MAX, f32::MAX), |(a, b), r| (a.min(r.0), b.min(r.1)));
    let (max_x, max_y) = rects.iter().fold((f32::MIN, f32::MIN), |(a, b), r| (a.max(r.0 + r.2), b.max(r.1 + r.3)));
    let (span_w, span_h) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
    let scale = ((w - 2.0 * pad) / span_w).min((h - 2.0 * pad) / span_h).max(f32::EPSILON);
    // Centre the arrangement.
    let ox = min_x - ((w / scale) - span_w) / 2.0;
    let oy = min_y - ((h / scale) - span_h) / 2.0;
    (scale, ox, oy)
}

/// Snaps a moving rectangle's position to the edges of `others`.
pub(crate) fn snap(x: f32, y: f32, w: f32, h: f32, others: &[(f32, f32, f32, f32)], threshold: f32) -> (f32, f32) {
    let best = |value: f32, candidates: &mut dyn Iterator<Item = f32>| -> f32 {
        candidates.filter(|c| (c - value).abs() <= threshold).min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs())).unwrap_or(value)
    };
    let sx = best(x, &mut others.iter().flat_map(|o| [o.0, o.0 + o.2, o.0 - w, o.0 + o.2 - w]));
    let sy = best(y, &mut others.iter().flat_map(|o| [o.1, o.1 + o.3, o.1 - h, o.1 + o.3 - h]));
    (sx, sy)
}

/// Shifts the arrangement so its top-left corner is at 0,0.
pub(crate) fn normalize(draft: &mut [OutputConfig]) {
    let on: Vec<&OutputConfig> = draft.iter().filter(|c| c.enabled).collect();
    let (Some(min_x), Some(min_y)) = (on.iter().filter_map(|c| c.x).min(), on.iter().filter_map(|c| c.y).min()) else { return };
    for c in draft.iter_mut().filter(|c| c.enabled) {
        c.x = c.x.map(|x| x - min_x);
        c.y = c.y.map(|y| y - min_y);
    }
}

/// Rates within this of a whole number print without decimals, Hz.
const WHOLE_HZ_EPSILON: f32 = 0.005;

fn rate_label(mhz: u32) -> String {
    let hz = mhz as f32 / MHZ_PER_HZ;
    if (hz - hz.round()).abs() < WHOLE_HZ_EPSILON { format!("{:.0} Hz", hz.round()) } else { format!("{hz:.2} Hz") }
}

/// `3840×2160 · 60 Hz · 150%` for the canvas.
fn mode_label(c: &OutputConfig) -> String {
    let mut parts = Vec::new();
    if let (Some(w), Some(h)) = (c.width, c.height) {
        parts.push(format!("{w}×{h}"));
    }
    if let Some(r) = c.refresh_mhz {
        parts.push(rate_label(r));
    }
    if let Some(s) = c.scale {
        parts.push(format!("{:.0}%", s * PERCENT));
    }
    parts.join(" · ")
}

/// Which group comes first (`MANDARA_STUDIO_DISPLAYS_GROUP=arrangement|
/// screens|brightness|night`): screenshots and layout checks of the lower
/// groups without scrolling.
fn first_group() -> &'static str {
    static GROUP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    GROUP.get_or_init(|| std::env::var("MANDARA_STUDIO_DISPLAYS_GROUP").unwrap_or_default())
}

fn transform_label(t: Transform) -> &'static str {
    match t {
        Transform::Normal => tk("Normal"),
        Transform::Rotate90 => tk("90°"),
        Transform::Rotate180 => tk("180°"),
        Transform::Rotate270 => tk("270°"),
        Transform::Flipped => tk("Flipped"),
        Transform::Flipped90 => tk("Flipped 90°"),
        Transform::Flipped180 => tk("Flipped 180°"),
        Transform::Flipped270 => tk("Flipped 270°"),
    }
}

fn surface_label(k: SurfaceKind) -> &'static str {
    match k {
        SurfaceKind::Bar => tk("Bar"),
        SurfaceKind::Osd => tk("On-screen displays"),
        SurfaceKind::Notifications => tk("Notifications"),
        SurfaceKind::Wallpaper => tk("Wallpaper"),
        SurfaceKind::Dock => tk("Dock"),
    }
}

/// What a surface's assignment currently reads as: "All displays", "Main
/// display" or the listed connectors.
fn assignment_summary(cfg: &ShellConfig, kind: SurfaceKind) -> String {
    let set = cfg.displays.surfaces.get(kind);
    // The dock's own older "first display" choice still applies until one is
    // picked here (`ShellConfig::dock_on_output`).
    if kind == SurfaceKind::Dock && *set == Default::default() && cfg.dock.displays == mandara_config::workflow::DockDisplays::Primary {
        return t("First display").into();
    }
    match set.show_on {
        ScreenChoice::Selected if !set.outputs.is_empty() => set.outputs.join(", "),
        ScreenChoice::Primary => t("Main display").into(),
        _ => t("All displays").into(),
    }
}

impl StudioView {
    /// A compact card on a surface's own page (Bar, Dock) pointing at the
    /// Displays page, where every surface's displays are chosen in one place.
    pub(super) fn displays_link(&mut self, kind: SurfaceKind, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rtl = mandara_i18n::language().is_rtl();
        card(p)
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_14_PX))
                    .child(icon("monitor", ICON_17_PX, p.primary))
                    .child(div().flex_1().min_w(px(DIM_150_PX)).row().flex_col().when(rtl, |el| el.items_end()).child(heading(
                        tf("Shows on: {displays}", &[("displays", &assignment_summary(cfg, kind))]),
                        tf("Pick the displays for the {surface} under Displays → Shell on each display", &[("surface", &tr(surface_label(kind)).to_lowercase())]),
                        p,
                    )))
                    .child(button(SharedString::from(format!("displays-link-{}", kind.id())), "monitor", t("Choose displays"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Displays, cx)))),
            )
            .into_any_element()
    }
}

fn running_target() -> Option<Target> {
    match std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase().as_str() {
        d if d.contains("mango") => Some(Target::Mango),
        d if d.contains("niri") => Some(Target::Niri),
        d if d.contains("hyprland") => Some(Target::Hyprland),
        _ => None,
    }
}

impl StudioView {
    /// Re-reads the heads (and resets the draft) off the UI thread.
    fn load_heads(cx: &mut Context<Self>) {
        st(cx).heads_at = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async { outputs::heads() }).await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                if s.drag.is_some() || s.busy {
                    return;
                }
                if let Ok(heads) = &result {
                    let fresh: Vec<OutputConfig> = heads.iter().map(outputs::Head::to_config).collect();
                    // Keep unapplied edits unless the monitors changed.
                    let same_set = s.draft.iter().map(|c| c.name.clone()).collect::<Vec<_>>() == fresh.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
                    let untouched = s.heads.as_ref().and_then(|h| h.as_ref().ok()).is_none_or(|old| old.iter().map(outputs::Head::to_config).collect::<Vec<_>>() == s.draft);
                    if !same_set || untouched {
                        s.draft = fresh;
                    }
                    s.selected = s.selected.min(heads.len().saturating_sub(1));
                }
                s.heads = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_once(cx: &mut Context<Self>) {
        let s = st(cx);
        let (need_support, need_ddc) = (s.support.is_none(), s.ddc.is_none());
        if need_support {
            s.support = Some(GammaSupport::Unknown);
        }
        if need_ddc {
            s.ddc = Some((ddc::Access::Ready, Vec::new()));
        }
        if need_support {
            cx.spawn(async move |this, cx| {
                let support = cx.background_executor().spawn(async { mandara_services::gamma::probe() }).await;
                let _ = this.update(cx, |_, cx| {
                    st(cx).support = Some(support);
                    cx.notify();
                });
            })
            .detach();
        }
        if need_ddc {
            cx.spawn(async move |this, cx| {
                let found = cx
                    .background_executor()
                    .spawn(async {
                        let access = ddc::access();
                        let list = if access == ddc::Access::Ready { ddc::monitors() } else { Vec::new() };
                        (access, list)
                    })
                    .await;
                let _ = this.update(cx, |_, cx| {
                    let s = st(cx);
                    s.ddc_shown = found.1.iter().map(|m| (m.connector.clone(), m.percent())).collect();
                    s.ddc = Some(found);
                    cx.notify();
                });
            })
            .detach();
        }
        let s = st(cx);
        if s.user_rules.is_none() {
            s.user_rules = Some(running_target().map(user_output_rules).unwrap_or_default());
        }
        if s.clients.is_none() {
            s.clients = Some(Vec::new());
            if let Some(task) = crate::perf::query(&["compositor", "clients"], cx) {
                cx.spawn(async move |this, cx| {
                    let list = task.await.and_then(|v| serde_json::from_value::<Vec<ClientInfo>>(v).map_err(|e| e.to_string())).unwrap_or_default();
                    let _ = this.update(cx, |_, cx| {
                        let mut ids: Vec<String> = list.into_iter().map(|c| c.app_id).filter(|a| !a.is_empty() && a != mandara_core::STUDIO_APP_ID).collect();
                        ids.sort();
                        ids.dedup();
                        st(cx).clients = Some(ids);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    fn load_night(cx: &mut Context<Self>) {
        st(cx).night_at = Some(Instant::now());
        let Some(task) = crate::perf::query(&["nightlight", "status"], cx) else { return };
        cx.spawn(async move |this, cx| {
            let v = task.await.ok();
            let _ = this.update(cx, |_, cx| {
                st(cx).night = v;
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs while the page is visible: countdown, refreshes.
    fn ensure_loop(cx: &mut Context<Self>) {
        if st(cx).alive {
            return;
        }
        st(cx).alive = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let on_page = this.update(cx, |v, _| v.section == Section::Displays && v.query.is_empty()).unwrap_or(false);
            if !on_page {
                let _ = this.update(cx, |_, cx| {
                    let s = st(cx);
                    s.alive = false;
                    // Fresh probes next visit.
                    s.ddc = None;
                    s.clients = None;
                    s.user_rules = None;
                });
                return;
            }
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                if s.revert_at.is_some_and(|at| Instant::now() >= at) {
                    s.revert_at = None;
                    s.message = Some((t("Reverted to the previous arrangement").into(), false));
                    s.heads_at = None;
                }
                let (heads_due, night_due) = (s.heads_at.is_none_or(|t| t.elapsed() >= HEADS_REFRESH), s.night_at.is_none_or(|t| t.elapsed() >= NIGHT_REFRESH));
                if heads_due {
                    Self::load_heads(cx);
                }
                if night_due {
                    Self::load_night(cx);
                }
                if st(cx).revert_at.is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn displays_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        Self::ensure_loop(cx);
        Self::load_once(cx);
        if st(cx).heads.is_none() && st(cx).heads_at.is_none() {
            Self::load_heads(cx);
        }
        let arrangement = self.arrangement(cfg, p, cx);
        let night = self.night_light_section(cfg, p, cx);
        let screens = self.surface_screens(cfg, p, cx);
        let brightness = self.external_brightness(cfg, p, cx);
        let rest = self.rows(registry::in_section(Section::Displays).filter(|s| !s.path.starts_with("/night_light/")), cfg, p, cx);
        let mut groups = vec![("arrangement", arrangement), ("screens", screens), ("brightness", brightness), ("night", night), ("rest", rest)];
        if let Some(i) = groups.iter().position(|(name, _)| *name == first_group()) {
            let g = groups.remove(i);
            groups.insert(0, g);
        }
        div().row().flex_col().gap(px(SPACE_26_PX)).children(groups.into_iter().map(|(_, g)| g)).into_any_element()
    }

    // ── Arrangement ─────────────────────────────────────────────────────────

    fn arrangement(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let heads = match st(cx).heads.clone() {
            None => return div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Arrangement"), p)).child(card(p).child(div().text_color(p.on_surface_variant.hsla()).child(t("Reading displays…")))).into_any_element(),
            Some(Err(e)) => {
                return div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Arrangement"), p)).child(card(p).child(heading(t("Display configuration isn't available"), e, p))).into_any_element();
            }
            Some(Ok(h)) => h,
        };
        let s = st(cx);
        let draft = s.draft.clone();
        let selected = s.selected.min(draft.len().saturating_sub(1));
        let dirty = heads.iter().map(outputs::Head::to_config).collect::<Vec<_>>() != draft;
        let (busy, message, revert_at) = (s.busy, s.message.clone(), s.revert_at);

        // Canvas.
        let canvas_w = (self.page.column_w - 2.0 * SPACE_18_PX).max(DIM_200_PX);
        let rects: Vec<(f32, f32, f32, f32)> = draft.iter().map(logical_rect).collect();
        let shown: Vec<(f32, f32, f32, f32)> = draft.iter().zip(&rects).filter(|(c, _)| c.enabled).map(|(_, r)| *r).collect();
        let (scale, ox, oy) = fit_canvas(&shown, canvas_w, CANVAS_H_PX, CANVAS_PAD_PX);
        let primary = cfg.displays.primary.clone();
        let mut canvas = div()
            .id("display-canvas")
            .relative()
            .w(px(canvas_w))
            .h(px(CANVAS_H_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_dim.hsla())
            .overflow_hidden()
            .on_mouse_move(cx.listener(move |_, e: &MouseMoveEvent, _, cx| {
                let s = st(cx);
                let Some((i, start, (x0, y0))) = s.drag else { return };
                let dx = f32::from(e.position.x - start.x) / scale;
                let dy = f32::from(e.position.y - start.y) / scale;
                let Some(c) = s.draft.get(i) else { return };
                let (_, _, w, h) = logical_rect(c);
                let others: Vec<_> = s.draft.iter().enumerate().filter(|(j, o)| *j != i && o.enabled).map(|(_, o)| logical_rect(o)).collect();
                let (x, y) = snap(x0 as f32 + dx, y0 as f32 + dy, w, h, &others, SNAP_LOGICAL_PX);
                if let Some(c) = s.draft.get_mut(i) {
                    (c.x, c.y) = (Some(x.round() as i32), Some(y.round() as i32));
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _: &MouseUpEvent, _, cx| {
                    let s = st(cx);
                    if s.drag.take().is_some() {
                        normalize(&mut s.draft);
                        cx.notify();
                    }
                }),
            );
        if shown.is_empty() {
            canvas = canvas.child(div().size_full().row().items_center().justify_center().text_color(p.on_surface_variant.hsla()).child(t("Every display is off")));
        }
        for (i, (c, r)) in draft.iter().zip(&rects).enumerate().filter(|(_, (c, _))| c.enabled) {
            let (x, y, w, h) = ((r.0 - ox) * scale, (r.1 - oy) * scale, r.2 * scale, r.3 * scale);
            let active = i == selected;
            let is_primary = primary.as_deref() == Some(c.name.as_str());
            let mode = mode_label(c);
            let pos = (c.x.unwrap_or(0), c.y.unwrap_or(0));
            canvas = canvas.child(
                div()
                    .id(("display-rect", i))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w.max(1.0)))
                    .h(px(h.max(1.0)))
                    .p(px(SPACE_3_PX))
                    .child(
                        div()
                            .size_full()
                            .rounded(px(RADIUS_10_PX))
                            .bg(if active { p.primary.with_alpha(ALPHA_22).hsla() } else { p.surface_container_high.hsla() })
                            .border_2()
                            .border_color(if active { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_30).hsla() })
                            .cursor_grab()
                            .row()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(SPACE_3_PX))
                            .overflow_hidden()
                            .child(div().row().items_center().gap(px(SPACE_5_PX)).when(is_primary, |el| el.child(icon("pin", ICON_11_PX, p.primary))).child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::SEMIBOLD).child(c.name.clone())))
                            .child(div().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(mode)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, e: &MouseDownEvent, _, cx| {
                            let s = st(cx);
                            s.selected = i;
                            s.drag = Some((i, e.position, pos));
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }

        // Countdown banner or action row.
        let actions = if let Some(at) = revert_at {
            let left = at.saturating_duration_since(Instant::now()).as_secs() + 1;
            div()
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_10_PX))
                .p(px(SPACE_12_PX))
                .rounded(px(RADIUS_14_PX))
                .bg(p.warning_amber.with_alpha(ALPHA_14).hsla())
                .child(icon("timer", ICON_16_PX, p.warning_amber))
                .child(div().flex_1().min_w(px(DIM_200_PX)).font_weight(FontWeight::MEDIUM).font_features(mandara_ui::fonts::tabular_nums()).child(mandara_ui::bidi::paragraph(tf("Keep these display settings? Reverting in {seconds} s", &[("seconds", &left)]))))
                .child(button("display-keep", "check", t("Keep"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    outputs::confirm();
                    let persisted = !mandara_ui::state::config(cx).read(cx).get().compositor.outputs.is_empty();
                    let s = st(cx);
                    s.revert_at = None;
                    s.message = Some((t("Kept").into(), false));
                    s.heads_at = None;
                    if persisted {
                        // Keep the drop-in in step with what's on screen: the
                        // live heads, not the draft (after a profile apply or
                        // with unapplied edits the draft is something else).
                        let fallback = s.draft.clone();
                        cx.spawn(async move |_, cx| {
                            let live = cx.background_executor().spawn(async { outputs::heads() }).await;
                            let kept = match live {
                                Ok(heads) if !heads.is_empty() => heads.iter().map(outputs::Head::to_config).collect(),
                                _ => fallback,
                            };
                            let _ = cx.update(|cx| update_config(cx, move |c| c.compositor.outputs = kept));
                        })
                        .detach();
                    }
                    cx.notify();
                })))
                .child(button("display-revert", "undo", t("Revert"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    st(cx).revert_at = None;
                    cx.spawn(async move |this, cx| {
                        let r = cx.background_executor().spawn(async { outputs::revert_now() }).await;
                        let _ = this.update(cx, |_, cx| {
                            let s = st(cx);
                            s.message = Some(match r {
                                Ok(_) => (t("Reverted").into(), false),
                                Err(e) => (e, true),
                            });
                            s.heads_at = None;
                            cx.notify();
                        });
                    })
                    .detach();
                })))
        } else {
            let secs = cfg.displays.revert_secs();
            div()
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(
                    button("display-apply", "check", if busy { t("Applying…") } else { t("Apply") }, dirty && !busy, p)
                        .when(!dirty || busy, |el| el.opacity(ALPHA_50))
                        .tooltip(tip(tf("Applies now; reverts after {seconds} s unless you keep it", &[("seconds", &secs)]), p))
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            let s = st(cx);
                            if s.busy || !dirty {
                                return;
                            }
                            s.busy = true;
                            s.message = None;
                            let draft = s.draft.clone();
                            cx.spawn(async move |this, cx| {
                                let r = cx.background_executor().spawn(async move { outputs::apply_with_revert(&draft, secs) }).await;
                                let _ = this.update(cx, |_, cx| {
                                    let s = st(cx);
                                    s.busy = false;
                                    match r {
                                        Ok(secs) => s.revert_at = Some(Instant::now() + Duration::from_secs(secs as u64)),
                                        Err(e) => s.message = Some((e, true)),
                                    }
                                    s.heads_at = None;
                                    cx.notify();
                                });
                            })
                            .detach();
                        })),
                )
                .child(button("display-reset", "refresh", t("Reset"), false, p).when(!dirty, |el| el.opacity(ALPHA_50)).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    let s = st(cx);
                    if let Some(Ok(h)) = &s.heads {
                        s.draft = h.iter().map(outputs::Head::to_config).collect();
                    }
                    s.message = None;
                    cx.notify();
                })))
                .child(div().flex_1())
                .when_some(message, |el, (text, error)| el.child(badge(text, if error { p.urgent_coral } else { p.success_emerald })))
        };

        let selected_card = draft.get(selected).zip(heads.iter().find(|h| draft.get(selected).is_some_and(|c| c.name == h.name))).map(|(c, h)| self.output_card(selected, c, h, cfg, p, cx));
        let profiles = self.profiles_card(&heads, cfg, p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Arrangement"), p))
            .child(card(p).child(heading(t("Drag displays to arrange them"), t("Edges snap together. Changes apply only when you press Apply, and come back on their own unless you keep them."), p)).child(canvas).child(actions))
            .when_some(selected_card, |el, c| el.child(c))
            .child(profiles)
            .into_any_element()
    }

    /// Edits draft entry `i` with `f`.
    fn edit(i: usize, f: impl FnOnce(&mut OutputConfig) + 'static) -> impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
        let f = std::cell::RefCell::new(Some(f));
        move |_, _, cx| {
            if let (Some(c), Some(f)) = (st(cx).draft.get_mut(i), f.borrow_mut().take()) {
                f(c);
                normalize(&mut st(cx).draft);
            }
            cx.refresh_windows();
        }
    }

    fn output_card(&mut self, i: usize, c: &OutputConfig, h: &outputs::Head, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let row = |label: &'static str, body: AnyElement| {
            div().row().flex_wrap().items_center().gap(px(SPACE_12_PX)).child(div().w(px(DIM_110_PX)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(label)))).child(div().flex_1().min_w(px(0.0)).child(body))
        };
        let chips = || div().row().flex_wrap().gap(px(SPACE_6_PX));
        let enabled_count = st(cx).draft.iter().filter(|d| d.enabled).count();

        let power = chips()
            .child(chip(SharedString::from(format!("d{i}-on")), t("On"), c.enabled, p).on_click(Self::edit(i, |c| c.enabled = true)))
            .child(chip(SharedString::from(format!("d{i}-off")), t("Off"), !c.enabled, p).when(c.enabled && enabled_count <= 1, |el| el.opacity(ALPHA_50).tooltip(tip(t("At least one display must stay on"), p))).on_click({
                let last = c.enabled && enabled_count <= 1;
                let set = Self::edit(i, |c| c.enabled = false);
                move |e, w, cx| {
                    if !last {
                        set(e, w, cx)
                    }
                }
            }));
        let mut res = chips();
        for (n, (w, hgt)) in h.resolutions().into_iter().take(MAX_RESOLUTIONS).enumerate() {
            let active = c.width == Some(w as u32) && c.height == Some(hgt as u32);
            let best = h.refresh_rates(w, hgt).first().copied();
            res = res.child(chip(SharedString::from(format!("d{i}-res-{n}")), format!("{w}×{hgt}"), active, p).font_features(mandara_ui::fonts::tabular_nums()).on_click(Self::edit(i, move |c| {
                (c.width, c.height) = (Some(w as u32), Some(hgt as u32));
                c.refresh_mhz = best.map(|r| r as u32);
            })));
        }
        let mut rates = chips();
        if let (Some(w), Some(hgt)) = (c.width, c.height) {
            let offered = h.refresh_rates(w as i32, hgt as i32);
            // The offered rate closest to the chosen one is the active chip.
            let closest = c.refresh_mhz.and_then(|cur| offered.iter().copied().min_by_key(|r| (*r as i64 - cur as i64).abs()));
            for (n, r) in offered.into_iter().take(MAX_RATES).enumerate() {
                let active = closest == Some(r);
                rates = rates.child(chip(SharedString::from(format!("d{i}-hz-{n}")), rate_label(r as u32), active, p).font_features(mandara_ui::fonts::tabular_nums()).on_click(Self::edit(i, move |c| c.refresh_mhz = Some(r as u32))));
            }
        }
        let current_scale = c.scale.unwrap_or(1.0);
        let mut scales: Vec<f32> = SCALES.to_vec();
        if !scales.iter().any(|s| (s - current_scale).abs() < SCALE_EPSILON) {
            scales.push(current_scale);
            scales.sort_by(f32::total_cmp);
        }
        let mut scale_row = chips();
        for (n, s) in scales.into_iter().enumerate() {
            scale_row = scale_row.child(chip(SharedString::from(format!("d{i}-scale-{n}")), format!("{:.0}%", s * PERCENT), (s - current_scale).abs() < SCALE_EPSILON, p).on_click(Self::edit(i, move |c| c.scale = Some(s))));
        }
        let mut rot = chips();
        let current_t = c.transform.unwrap_or_default();
        for (n, tr_) in Transform::ALL.into_iter().enumerate() {
            rot = rot.child(chip(SharedString::from(format!("d{i}-rot-{n}")), tr(transform_label(tr_)), tr_ == current_t, p).on_click(Self::edit(i, move |c| c.transform = Some(tr_))));
        }
        let vrr = h.adaptive_sync.map(|_| {
            let cur = c.vrr.unwrap_or_default();
            chips()
                .child(chip(SharedString::from(format!("d{i}-vrr-off")), t("Off"), cur == VrrMode::Off, p).on_click(Self::edit(i, |c| c.vrr = Some(VrrMode::Off))))
                .child(chip(SharedString::from(format!("d{i}-vrr-on")), t("On"), cur == VrrMode::On, p).on_click(Self::edit(i, |c| c.vrr = Some(VrrMode::On))))
                .child(chip(SharedString::from(format!("d{i}-vrr-fs")), t("Fullscreen only"), cur == VrrMode::Fullscreen, p).tooltip(tip(t("Hyprland and Niri switch it per fullscreen window; MangoWM treats it as on"), p)).on_click(Self::edit(i, |c| c.vrr = Some(VrrMode::Fullscreen))))
        });
        let name = c.name.clone();
        let is_primary = cfg.displays.primary.as_deref() == Some(c.name.as_str());
        let primary = chip(SharedString::from(format!("d{i}-primary")), if is_primary { t("Main display") } else { t("Make main display") }, is_primary, p).on_click(move |_, _, cx| {
            let name = name.clone();
            update_config(cx, move |c| c.displays.primary = Some(name));
        });
        let title = if h.make.is_empty() && h.model.is_empty() { h.description.clone() } else { format!("{} {}", h.make, h.model).trim().to_string() };
        let mm = (h.physical_mm.0 > 0).then(|| tf("{width} × {height} mm", &[("width", &h.physical_mm.0), ("height", &h.physical_mm.1)]));
        card(p)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(icon("monitor", ICON_17_PX, p.primary))
                    .child(div().flex_1().min_w(px(0.0)).child(heading(c.name.clone(), [Some(title), mm].into_iter().flatten().collect::<Vec<_>>().join(" · "), p)))
                    .child(primary),
            )
            .child(row(tk("Power"), power.into_any_element()))
            .when(c.enabled, |el| {
                el.child(row(tk("Resolution"), res.into_any_element()))
                    .child(row(tk("Refresh rate"), rates.into_any_element()))
                    .child(row(tk("Scale"), scale_row.into_any_element()))
                    .child(row(tk("Rotation"), rot.into_any_element()))
                    .when_some(vrr, |el, v| el.child(row(tk("Adaptive sync"), v.into_any_element())))
            })
            .into_any_element()
    }

    fn profiles_card(&mut self, heads: &[outputs::Head], cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let connected = outputs::identities(heads);
        let mut list = div().row().flex_wrap().gap(px(SPACE_8_PX));
        for (n, prof) in cfg.displays.profiles.iter().enumerate() {
            let matches = prof.matches(&connected);
            let (apply_p, del_name) = (prof.clone(), prof.name.clone());
            list = list.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_4_PX))
                    .pad_start(px(SPACE_12_PX))
                    .pad_end(px(SPACE_4_PX))
                    .h(px(DIM_34_PX))
                    .rounded_full()
                    .bg(if matches { p.primary.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
                    .child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(prof.name.clone()))
                    .when(matches, |el| el.child(badge(t("Connected"), p.success_emerald)))
                    .child(
                        div()
                            .id(("profile-apply", n))
                            .size(px(DIM_26_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_16).hsla()))
                            .tooltip(tip(t("Apply this profile"), p))
                            .child(icon("check", ICON_12_PX, p.primary))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                let profile = apply_p.clone();
                                let secs = mandara_ui::state::config(cx).read(cx).get().displays.revert_secs();
                                st(cx).busy = true;
                                cx.spawn(async move |this, cx| {
                                    let r = cx.background_executor().spawn(async move { outputs::apply_with_revert(&profile.outputs, secs) }).await;
                                    let _ = this.update(cx, |_, cx| {
                                        let s = st(cx);
                                        s.busy = false;
                                        match r {
                                            Ok(secs) => s.revert_at = Some(Instant::now() + Duration::from_secs(secs as u64)),
                                            Err(e) => s.message = Some((e, true)),
                                        }
                                        s.heads_at = None;
                                        cx.notify();
                                    });
                                })
                                .detach();
                            })),
                    )
                    .child(
                        div()
                            .id(("profile-del", n))
                            .size(px(DIM_26_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .tooltip(tip(t("Delete"), p))
                            .child(icon("close", ICON_11_PX, p.on_surface_variant))
                            .on_click(move |_, _, cx| {
                                let name = del_name.clone();
                                update_config(cx, move |c| c.displays.profiles.retain(|p| p.name != name));
                            }),
                    ),
            );
        }
        if cfg.displays.profiles.is_empty() {
            list = list.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No profiles yet")));
        }
        let snapshot: Vec<OutputConfig> = heads.iter().map(outputs::Head::to_config).collect();
        let default_name = heads.iter().filter(|h| h.enabled).map(|h| if h.model.is_empty() { h.name.clone() } else { h.model.clone() }).collect::<Vec<_>>().join(" + ");
        let save = button("profile-save", "plus", t("Save as profile"), false, p).on_click(move |_, _, cx| {
            let profile = DisplayProfile { name: default_name.clone(), outputs: snapshot.clone() };
            update_config(cx, move |c| {
                c.displays.profiles.retain(|p| p.name != profile.name);
                c.displays.profiles.push(profile);
            });
        });

        // Persistence to the compositor drop-in.
        let persisted = !cfg.compositor.outputs.is_empty();
        let target = running_target();
        let file = target.map(|t| t.generated_file().file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).unwrap_or_default();
        let overlap: Vec<String> = st(cx).user_rules.clone().unwrap_or_default().into_iter().filter(|n| heads.iter().any(|h| &h.name == n)).collect();
        let current: Vec<OutputConfig> = heads.iter().map(outputs::Head::to_config).collect();
        let keep = if persisted {
            button("display-unpersist", "close", t("Stop saving"), false, p).on_click(|_, _, cx| update_config(cx, |c| c.compositor.outputs.clear()))
        } else {
            button("display-persist", "download", t("Keep after restart"), true, p).on_click(move |_, _, cx| {
                let list = current.clone();
                update_config(cx, move |c| c.compositor.outputs = list);
            })
        };
        card(p)
            .child(heading(t("Profiles"), t("Save the current arrangement; when the same monitors connect again the shell switches to it on its own."), p))
            .child(list)
            .child(div().row().child(save))
            .child(div().h(px(BORDER_1_PX)).bg(p.outline.with_alpha(ALPHA_12).hsla()))
            .child(heading(
                t("Keep after restart"),
                if !cfg.compositor.manage {
                    // Nothing is written while the drop-in is off.
                    t("Turn on “Manage compositor drop-in” (Compositor page) first: Mandara writes no drop-in while it's off").into()
                } else if persisted {
                    tf("Saved as monitor lines in Mandara's {file} drop-in (your own config isn't touched)", &[("file", &file)])
                } else {
                    t("Off: the compositor's own config decides at the next start").into()
                },
                p,
            ))
            .when(!overlap.is_empty(), |el| {
                el.child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_8_PX))
                        .child(icon("alert", ICON_14_PX, p.warning_amber))
                        .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tf("Your own config also sets {outputs}; your line may win over the drop-in", &[("outputs", &overlap.join(", "))])))),
                )
            })
            .child(div().row().gap(px(SPACE_8_PX)).child(keep))
            .into_any_element()
    }

    // ── Which displays show what ────────────────────────────────────────────

    fn surface_screens(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let names: Vec<String> = st(cx).heads.as_ref().and_then(|h| h.as_ref().ok()).map(|h| h.iter().filter(|h| h.enabled).map(|h| h.name.clone()).collect()).unwrap_or_default();
        let mut body = card(p).child(heading(t("Show on"), t("Pick the displays each part of the shell appears on. Main display follows the one marked as main above."), p));
        for kind in SurfaceKind::ALL {
            let set = cfg.displays.surfaces.get(kind).clone();
            let mut chips = div().row().flex_wrap().gap(px(SPACE_6_PX));
            chips = chips
                .child(chip(SharedString::from(format!("show-{}-all", kind.id())), t("All"), set.show_on == ScreenChoice::All, p).on_click(move |_, _, cx| update_config(cx, move |c| c.displays.surfaces.get_mut(kind).show_on = ScreenChoice::All)))
                .child(chip(SharedString::from(format!("show-{}-main", kind.id())), t("Main display"), set.show_on == ScreenChoice::Primary, p).on_click(move |_, _, cx| update_config(cx, move |c| c.displays.surfaces.get_mut(kind).show_on = ScreenChoice::Primary)));
            for (n, name) in names.iter().enumerate() {
                let on = set.show_on == ScreenChoice::Selected && set.outputs.iter().any(|o| o == name);
                let name = name.clone();
                chips = chips.child(chip(SharedString::from(format!("show-{}-{n}", kind.id())), name.clone(), on, p).on_click(move |_, _, cx| {
                    let name = name.clone();
                    update_config(cx, move |c| {
                        let s = c.displays.surfaces.get_mut(kind);
                        if s.show_on != ScreenChoice::Selected {
                            s.show_on = ScreenChoice::Selected;
                            s.outputs.clear();
                        }
                        if let Some(i) = s.outputs.iter().position(|o| *o == name) {
                            s.outputs.remove(i);
                        } else {
                            s.outputs.push(name);
                        }
                    });
                }));
            }
            body = body.child(div().row().flex_wrap().items_center().gap(px(SPACE_12_PX)).child(div().w(px(DIM_150_PX)).text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(surface_label(kind))))).child(chips));
        }
        div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Shell on each display"), p)).child(body).into_any_element()
    }

    // ── External brightness (DDC/CI) ────────────────────────────────────────

    fn external_brightness(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let s = st(cx);
        let wants = s.ddc_wants.clone();
        let shown = s.ddc_shown.clone();
        let min = cfg.displays.brightness.floor_percent();
        let body = match s.ddc.clone() {
            _ if !cfg.displays.brightness.ddc => card(p).child(heading(t("External monitor brightness is off"), t("Turn on “Control external monitors” below."), p)),
            None => card(p).child(div().text_color(p.on_surface_variant.hsla()).child(t("Looking for monitors…"))),
            Some((ddc::Access::NoModule, _)) => card(p).child(heading(
                t("The i2c-dev kernel module isn't loaded"),
                t("Load it now with `sudo modprobe i2c-dev`, and at every boot with `echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf`."),
                p,
            )),
            Some((ddc::Access::NoPermission { group }, _)) => {
                let g = group.unwrap_or_else(|| "i2c".into());
                card(p).child(heading(t("No permission to talk to your monitors"), tf("Add yourself to the {group} group (`sudo usermod -aG {group} $USER`), then log out and back in.", &[("group", &g)]), p))
            }
            Some((ddc::Access::Ready, list)) if list.is_empty() => card(p).child(heading(t("No external monitor answers DDC/CI"), t("Laptop panels use the brightness keys. For external monitors, turn on DDC/CI in the monitor's own menu."), p)),
            Some((ddc::Access::Ready, list)) => {
                let mut c = card(p).gap(px(SPACE_14_PX));
                for m in list {
                    let percent = shown.get(&m.connector).copied().unwrap_or_else(|| m.percent());
                    let (connector, wants) = (m.connector.clone(), wants.clone());
                    let slider = mandara_ui::kit::slider(SharedString::from(format!("ddc-{}", m.connector)), percent as f32 / PERCENT, p, move |f, _, cx| {
                        let v = ((f * PERCENT).round() as u32).clamp(min, PERCENT as u32);
                        st(cx).ddc_shown.insert(connector.clone(), v);
                        let first = wants.lock().unwrap_or_else(|e| e.into_inner()).insert(connector.clone(), v).is_none();
                        if first {
                            // One writer per monitor; it always sends the newest value.
                            let (connector, wants) = (connector.clone(), wants.clone());
                            std::thread::spawn(move || loop {
                                let Some(v) = wants.lock().unwrap_or_else(|e| e.into_inner()).get(&connector).copied() else { return };
                                let _ = ddc::set_percent(&connector, v as f64, min);
                                let mut w = wants.lock().unwrap_or_else(|e| e.into_inner());
                                if w.get(&connector) == Some(&v) {
                                    w.remove(&connector);
                                    return;
                                }
                            });
                        }
                        cx.refresh_windows();
                    });
                    c = c.child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_12_PX))
                            .child(icon("sun", ICON_16_PX, p.primary))
                            .child(div().w(px(DIM_110_PX)).font_weight(FontWeight::MEDIUM).child(m.connector.clone()))
                            .child(div().flex_1().min_w(px(0.0)).child(slider))
                            .child(div().w(px(DIM_44_PX)).text_end().text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(format!("{percent}%"))),
                    );
                }
                c
            }
        };
        div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Monitor brightness"), p)).child(body).into_any_element()
    }

    // ── Night light ─────────────────────────────────────────────────────────

    fn night_light_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let s = st(cx);
        let support = s.support.unwrap_or_default();
        let night = s.night.clone();
        let clients = s.clients.clone().unwrap_or_default();
        let n = &cfg.night_light;
        let active = night.as_ref().and_then(|v| v["active"].as_bool()).unwrap_or(false);
        let temp = night.as_ref().and_then(|v| v["temperature"].as_u64()).unwrap_or(n.day_temperature as u64);
        let next = night.as_ref().and_then(|v| v["next_change"].as_object().cloned());
        let paused = night.as_ref().and_then(|v| v["paused"].as_str().map(str::to_string));
        let busy: Vec<String> = night.as_ref().and_then(|v| serde_json::from_value(v["held_by_another_app"].clone()).ok()).unwrap_or_default();

        let status_text = if active { tf("Warm now · {kelvin} K", &[("kelvin", &temp)]) } else { t("Not warming now").into() };
        let detail = match (&paused, &next) {
            (Some(why), _) => tf("Paused: {reason}", &[("reason", &tr(why))]),
            (None, Some(next)) => {
                let at = next.get("at").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                if next.get("to_night").and_then(|v| v.as_bool()).unwrap_or(false) { tf("Warms at {time}", &[("time", &at)]) } else { tf("Back to daylight at {time}", &[("time", &at)]) }
            }
            (None, None) if !n.enabled => t("Turn it on below, or toggle it from quick settings").into(),
            _ => String::new(),
        };
        let night_k = n.night_temperature.to_string();
        let header = card(p)
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(div().size(px(DIM_40_PX)).rounded_full().row().items_center().justify_center().bg(if active { p.warning_amber.with_alpha(ALPHA_20).hsla() } else { p.surface_container_high.hsla() }).child(icon("sunset", ICON_17_PX, if active { p.warning_amber } else { p.on_surface_variant })))
                    .child(div().flex_1().min_w(px(DIM_200_PX)).child(heading(status_text, detail, p)))
                    .child(button("night-toggle", "sunset", if active { t("Turn off for now") } else { t("Warm now") }, false, p).tooltip(tip(t("Until the schedule next changes"), p)).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        if let Some(task) = crate::perf::query(&["nightlight", "toggle"], cx) {
                            cx.spawn(async move |this, cx| {
                                let v = task.await.ok();
                                let _ = this.update(cx, |_, cx| {
                                    st(cx).night = v;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                    })))
                    .child(button("night-preview", "play", t("Preview"), false, p).tooltip(tip(tf("Shows {kelvin} K for a few seconds", &[("kelvin", &night_k)]), p)).on_click({
                        let k = night_k.clone();
                        cx.listener(move |_, _: &ClickEvent, _, cx| {
                            if let Some(task) = crate::perf::query(&["nightlight", "preview", &k, PREVIEW_SECONDS], cx) {
                                task.detach();
                            }
                        })
                    })),
            )
            .when(!busy.is_empty(), |el| {
                el.child(div().row().items_center().gap(px(SPACE_8_PX)).child(icon("alert", ICON_14_PX, p.warning_amber)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tf(
                    "Another app controls the colours of {outputs} (wlsunset, gammastep, hyprsunset…); quit it to use Mandara's night light",
                    &[("outputs", &busy.join(", "))],
                ))))
            });
        let body = match support {
            GammaSupport::Unsupported => card(p).child(heading(
                t("This compositor can't change screen colours"),
                t("Night light uses wlr-gamma-control, which MangoWM, Hyprland and Sway offer. Niri doesn't implement it yet; there, use its own settings or a tool built for it."),
                p,
            )),
            GammaSupport::NoDisplay => card(p).child(heading(t("No Wayland display"), t("Studio isn't running inside a Wayland session."), p)),
            _ => header,
        };
        // Rows: only the ones that apply to the chosen schedule.
        let rows = self.rows(
            registry::in_section(Section::Displays).filter(|s| s.path.starts_with("/night_light/")).filter(|s| match s.path {
                "/night_light/start" | "/night_light/end" => n.schedule == NightSchedule::Fixed,
                "/night_light/latitude" | "/night_light/longitude" => n.schedule == NightSchedule::Sun,
                _ => true,
            }),
            cfg,
            p,
            cx,
        );
        // Excluded apps: chips of the list plus running apps to add.
        let mut excl = div().row().flex_wrap().gap(px(SPACE_6_PX));
        for (k, app) in n.excluded_apps.iter().enumerate() {
            let app_c = app.clone();
            excl = excl.child(chip(SharedString::from(format!("nl-ex-{k}")), app.clone(), true, p).child(icon("close", ICON_11_PX, p.on_primary)).on_click(move |_, _, cx| {
                let a = app_c.clone();
                update_config(cx, move |c| c.night_light.excluded_apps.retain(|x| *x != a));
            }));
        }
        for (k, app) in clients.iter().filter(|a| !n.excludes(a)).enumerate() {
            let a = app.clone();
            excl = excl.child(chip(SharedString::from(format!("nl-add-{k}")), format!("+ {app}"), false, p).on_click(move |_, _, cx| {
                let a = a.clone();
                update_config(cx, move |c| c.night_light.excluded_apps.push(a));
            }));
        }
        if n.excluded_apps.is_empty() && clients.is_empty() {
            excl = excl.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Open the app you want to exclude, then come back here")));
        }
        let exclusions = card(p).child(heading(t("Pause for these apps"), t("While one of them is focused, colours stay true (photo and video editing)."), p)).child(excl);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Night light"), p)).child(body))
            .when(support != GammaSupport::Unsupported, |el| el.child(div().row().flex_col().gap(px(SPACE_10_PX)).child(rows).child(exclusions)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(name: &str, x: i32, y: i32, w: u32, h: u32, scale: f32) -> OutputConfig {
        OutputConfig { name: name.into(), x: Some(x), y: Some(y), width: Some(w), height: Some(h), scale: Some(scale), ..Default::default() }
    }

    #[test]
    fn logical_rects_follow_scale_and_rotation() {
        assert_eq!(logical_rect(&out("a", 10, 20, 3840, 2160, 1.5)), (10.0, 20.0, 2560.0, 1440.0));
        let mut r = out("b", 0, 0, 1920, 1080, 1.0);
        r.transform = Some(Transform::Rotate270);
        assert_eq!(logical_rect(&r).2, 1080.0);
    }

    #[test]
    fn canvas_fits_and_centres() {
        let rects = [(0.0, 0.0, 2560.0, 1440.0), (2560.0, 0.0, 1920.0, 1080.0)];
        let (scale, ox, oy) = fit_canvas(&rects, 800.0, 240.0, 20.0);
        let right_edge = (4480.0 - ox) * scale;
        assert!(right_edge <= 780.0 + 0.01, "{right_edge}");
        assert!((0.0 - ox) * scale >= 19.99);
        assert!((0.0 - oy) * scale >= 19.99 && (1440.0 - oy) * scale <= 220.01);
    }

    #[test]
    fn dragging_snaps_to_neighbour_edges() {
        let others = [(0.0, 0.0, 2560.0, 1440.0)];
        // Dropped slightly off the right edge, a bit low: snaps flush + top-aligned.
        assert_eq!(snap(2590.0, 30.0, 1920.0, 1080.0, &others, SNAP_LOGICAL_PX), (2560.0, 0.0));
        // Far away: unchanged.
        assert_eq!(snap(4000.0, 900.0, 1920.0, 1080.0, &others, SNAP_LOGICAL_PX), (4000.0, 900.0));
        let mut d = vec![out("a", 300, -100, 1920, 1080, 1.0), out("b", 2220, 50, 1920, 1080, 1.0)];
        normalize(&mut d);
        assert_eq!((d[0].x, d[0].y, d[1].x, d[1].y), (Some(0), Some(0), Some(1920), Some(150)));
    }

    #[test]
    fn labels() {
        assert_eq!(rate_label(60_000), "60 Hz");
        assert_eq!(rate_label(143_912), "143.91 Hz");
        assert_eq!(rate_label(29_970), "29.97 Hz");
        assert_eq!(mode_label(&out("a", 0, 0, 3840, 2160, 1.5)), "3840×2160 · 150%");
        for t in Transform::ALL {
            assert!(!transform_label(t).is_empty());
        }
    }
}
