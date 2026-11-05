//! Local AI: what a local Ollama server holds. The bar chip shows a glyph
//! and, while a model is loaded, the VRAM it takes. The panel shows the
//! Ollama version, the models loaded now (a ring of how much sits in VRAM,
//! the GPU / CPU split, memory, a countdown until it unloads, Unload) and
//! the installed models (parameters, quantisation, family, size on disk,
//! Load to warm one up). Data comes from Ollama's HTTP API
//! (`mandara_services::local_ai`), read off the UI thread.
//!
//! No timer runs: a chip render refreshes data older than 30 s and
//! schedules its own redraw for then, the open panel does the same every
//! 3 s, so nothing is read while neither is on screen. Setting
//! (`[plugins.settings.local-ai]`): `url`. Commands:
//! `ai models|loaded|load|unload`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt as _, AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::local_ai::{self as ai, Installed, Loaded, Snapshot};
use mandara_theme::MandaraPalette;
use serde_json::{Value, json};

use super::extras::{Flash, caption, toast};
use super::model_view as fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, PILL_H_PX, SECTION_H_PX, SMALL_ICON_PX, ViewSet, WASH_FAINT, WASH_TRACK, alpha, dot_ring, duration_short, empty_state, glass_card,
    header, partner, pill_button, readable, redraw_in, section, wash,
};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "local-ai";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 420.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const EMPTY_H: f32 = 170.0;
const SECTION_GAP_PX: f32 = tokens::XS + 2.0;
/// Loaded-model cards: height, padding, ring.
const CARD_H: f32 = 84.0;
const CARD_GAP_PX: f32 = 8.0;
const CARD_PAD_PX: f32 = 12.0;
const CARD_FILL: (f32, f32) = (0.16, 0.05);
const RING_D_PX: f32 = 58.0;
const RING_DOTS: usize = 28;
const RING_DOT_PX: f32 = 3.5;
const RING_TEXT_PX: f32 = 12.0;
/// The unlit (CPU) part of the ring: `(dark, light)` alpha.
const RING_TRACK: (f32, f32) = (0.35, 0.3);
const NAME_PX: f32 = 14.0;
const LINE_GAP_PX: f32 = 2.0;
/// The "nothing loaded" line.
const IDLE_H: f32 = 40.0;
const IDLE_RADIUS_PX: f32 = 12.0;
/// Installed rows.
const ROW_H: f32 = 50.0;
const ROW_GAP_PX: f32 = 4.0;
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_RADIUS_PX: f32 = 12.0;
const VISIBLE_ROWS: f32 = 5.5;
const TAG_H_PX: f32 = 18.0;
const TAG_PAD_X_PX: f32 = 6.0;
const TAG_TEXT_PX: f32 = 10.0;
const SIZE_W_PX: f32 = 64.0;
/// Spinner turn and the busy pill's padding.
const SPIN: Duration = Duration::from_millis(900);
const BUSY_PAD_X_PX: f32 = 12.0;
/// Bar chip.
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_GAP_PX: f32 = 5.0;
const CHIP_IDLE_ALPHA: f32 = 0.6;
const SIZE_EPSILON_PX: f32 = 0.5;
/// Gradient partners fade to this share of the accent's alpha.
const PARTNER_FADE: f32 = 0.3;
/// Gap between an installed row's name and its tags.
const ROW_LINE_GAP_PX: f32 = 3.0;
/// Bytes per GiB.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// Fraction → percent.
const PERCENT: f32 = 100.0;

// ── Timing ───────────────────────────────────────────────────────────────

const CHIP_EVERY: Duration = Duration::from_secs(30);
const PANEL_EVERY: Duration = Duration::from_secs(3);

fn url(cx: &App) -> String {
    ai::base(&crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "url", ai::DEFAULT_URL.to_string()))
}

// ── Model (one per shell process) ────────────────────────────────────────

/// Labels formatted once per fetch.
struct Labels {
    chip: Option<SharedString>,
    subtitle: SharedString,
}

#[derive(Default)]
struct Model {
    data: Option<Rc<Result<Snapshot, String>>>,
    labels: Option<Labels>,
    /// The address the data came from (a new `url` refetches).
    from: String,
    fetched: Option<Instant>,
    fetching: bool,
    loading: BTreeSet<String>,
    unloading: BTreeSet<String>,
    flash: Flash,
    views: ViewSet,
}

thread_local! {
    static MODEL: Rc<RefCell<Model>> = Rc::new(RefCell::new(Model::default()));
}

fn model() -> Rc<RefCell<Model>> {
    MODEL.with(Rc::clone)
}

fn notify(cx: &mut App) {
    let views = model().borrow().views.clone();
    views.notify(cx);
}

pub fn gib(bytes: u64) -> String {
    tf("{value} GiB", &[("value", &format!("{:.1}", bytes as f64 / GIB))])
}

/// `100% GPU`, `38% GPU / 62% CPU` or `100% CPU`.
fn split_label(share: f32) -> String {
    let gpu = (share * PERCENT).round() as i64;
    match gpu {
        100 => t("100% GPU").into(),
        0 => t("100% CPU").into(),
        g => tf("{gpu}% GPU / {cpu}% CPU", &[("gpu", &g), ("cpu", &(100 - g))]),
    }
}

fn labels(data: &Result<Snapshot, String>) -> Labels {
    match data {
        Ok(s) => Labels {
            chip: (!s.loaded.is_empty()).then(|| gib(s.vram()).into()),
            subtitle: match s.loaded.len() {
                0 => tf("Ollama {version} · nothing loaded", &[("version", &s.version)]),
                n => tf("Ollama {version} · {count} loaded", &[("version", &s.version), ("count", &n)]),
            }
            .into(),
        },
        Err(_) => Labels { chip: None, subtitle: t("Not reachable").into() },
    }
}

fn refresh(cx: &mut App) {
    let m = model();
    if std::mem::replace(&mut m.borrow_mut().fetching, true) {
        return;
    }
    let base = url(cx);
    cx.spawn(async move |cx| {
        let from = base.clone();
        let data = mandara_services::runtime::unblock(move || ai::fetch(&base)).await;
        let views = {
            let mut mb = m.borrow_mut();
            mb.labels = Some(labels(&data));
            mb.data = Some(Rc::new(data));
            mb.from = from;
            mb.fetched = Some(Instant::now());
            mb.fetching = false;
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

fn keep_fresh(every: Duration, window: &mut Window, cx: &mut App) {
    let base = url(cx);
    let stale = {
        let m = model();
        let mb = m.borrow();
        mb.views.track(window);
        mb.from != base || mb.fetched.is_none_or(|at| at.elapsed() >= every)
    };
    if stale {
        refresh(cx);
    }
    redraw_in(every + EXPIRY_MARGIN, window, cx);
}

#[derive(Clone, Debug)]
enum Action {
    Load(String),
    Unload(String),
    UnloadAll,
}

/// Loads or unloads off the UI thread, then refreshes. The reply carries
/// the JSON for commands; the outcome also shows as a toast.
fn act(action: Action, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let m = model();
    {
        let mut mb = m.borrow_mut();
        match &action {
            Action::Load(n) => drop(mb.loading.insert(n.clone())),
            Action::Unload(n) => drop(mb.unloading.insert(n.clone())),
            Action::UnloadAll => {}
        }
    }
    notify(cx);
    let base = url(cx);
    cx.spawn(async move |cx| {
        let work = action.clone();
        let result: Result<Value, String> = mandara_services::runtime::unblock(move || {
            let snap = ai::fetch(&base)?;
            match work {
                Action::Load(q) => {
                    let name = ai::resolve(snap.installed.iter().map(|m| m.name.as_str()), &q)?;
                    ai::load(&base, &name).map(|_| json!({ "loaded": name, "keep_alive": ai::WARM_KEEP_ALIVE }))
                }
                Action::Unload(q) => {
                    let name = ai::resolve(snap.loaded.iter().map(|m| m.name.as_str()), &q)?;
                    ai::unload(&base, &name).map(|_| json!({ "unloaded": [name] }))
                }
                Action::UnloadAll => {
                    let names: Vec<String> = snap.loaded.iter().map(|m| m.name.clone()).collect();
                    for n in &names {
                        ai::unload(&base, n)?;
                    }
                    Ok(json!({ "unloaded": names }))
                }
            }
        })
        .await;
        {
            let mut mb = m.borrow_mut();
            match &action {
                Action::Load(n) => drop(mb.loading.remove(n)),
                Action::Unload(n) => drop(mb.unloading.remove(n)),
                Action::UnloadAll => {}
            }
            match (&result, &action) {
                (Ok(_), Action::Load(n)) => mb.flash.show(true, tf("{model} is ready", &[("model", n)])),
                (Ok(_), Action::Unload(n)) => mb.flash.show(true, tf("Unloaded {model}", &[("model", n)])),
                (Ok(_), Action::UnloadAll) => mb.flash.show(true, t("Unloaded every model")),
                (Err(e), _) => mb.flash.show(false, e.clone()),
            }
            mb.fetched = None;
        }
        let _ = tx.try_send(result.map(|v| v.to_string()));
        cx.update(|cx| {
            refresh(cx);
            notify(cx);
        });
    })
    .detach();
    rx
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn spinner(id: &'static str, tint: mandara_theme::Color) -> AnyElement {
    gpui::svg()
        .path("icons/refresh.svg")
        .size(px(SMALL_ICON_PX))
        .flex_none()
        .text_color(tint.hsla())
        .with_animation(id, Animation::new(SPIN).repeat(), |svg, delta| svg.with_transformation(gpui::Transformation::rotate(gpui::radians(delta * std::f32::consts::TAU))))
        .into_any_element()
}

/// A muted pill with a spinner (an action in flight).
fn busy_pill(id: &'static str, label: &'static str, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    div()
        .h(px(PILL_H_PX))
        .px(px(BUSY_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .bg(wash(WASH_FAINT, palette).hsla())
        .row()
        .items_center()
        .gap(px(SECTION_GAP_PX))
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(spinner(id, accent))
        .child(label)
        .into_any_element()
}

fn tag(label: &str, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(TAG_H_PX))
        .px(px(TAG_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .bg(wash(WASH_TRACK, palette).hsla())
        .flex()
        .items_center()
        .text_size(px(TAG_TEXT_PX))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.on_surface_variant.hsla())
        .child(label.to_string())
        .into_any_element()
}

fn countdown(expires_at: Option<i64>) -> String {
    match expires_at {
        None => t("stays loaded").into(),
        Some(at) => {
            let left = at - chrono::Utc::now().timestamp();
            if left <= 0 { t("unloading…").into() } else { tf("unloads in {span}", &[("span", &duration_short(left))]) }
        }
    }
}

fn loaded_card(i: usize, m: &Loaded, unloading: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let cpu = readable(palette.tertiary, palette);
    let share = m.gpu_share();
    let pct = format!("{}%", (share * PERCENT).round() as i64);
    let ring = dot_ring(
        share,
        RING_D_PX,
        RING_DOTS,
        RING_DOT_PX,
        accent,
        partner(accent),
        cpu.with_alpha(alpha(RING_TRACK, palette)),
        div().text_size(px(RING_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(pct).into_any_element(),
    );
    let name = m.name.clone();
    let action = if unloading {
        busy_pill("ai-unloading", t("Unloading…"), palette)
    } else {
        pill_button(("ai-unload", i), Some("eject"), t("Unload"), readable(palette.urgent_coral, palette), palette, move |_, cx| drop(act(Action::Unload(name.clone()), cx))).into_any_element()
    };
    let detail: Vec<&str> = [m.parameter_size.as_str(), m.quantization.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    glass_card(palette)
        .h(px(CARD_H))
        .p(px(CARD_PAD_PX))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(alpha(CARD_FILL, palette)), partner(accent).with_alpha(alpha(CARD_FILL, palette) * PARTNER_FADE)))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(ring)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(div().row().items_center().gap(px(SECTION_GAP_PX)).child(div().min_w(px(0.0)).truncate().text_size(px(NAME_PX)).font_weight(FontWeight::SEMIBOLD).child(m.name.clone())).children(detail.into_iter().map(|d| tag(d, palette))))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(accent.hsla()).font_features(tabular_nums()).child(tf("{split} · {memory}", &[("split", &split_label(share)), ("memory", &gib(m.size))])))
                .child(caption(countdown(m.expires_at), palette).font_features(tabular_nums())),
        )
        .child(action)
        .into_any_element()
}

fn installed_row(i: usize, m: &Installed, loaded: bool, loading: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let name = m.name.clone();
    let action = if loading {
        busy_pill("ai-loading", t("Loading…"), palette)
    } else if loaded {
        div().h(px(PILL_H_PX)).px(px(BUSY_PAD_X_PX)).flex().items_center().gap(px(tokens::XS)).text_size(px(text::LABEL)).text_color(readable(palette.success_emerald, palette).hsla()).child(icon("check", SMALL_ICON_PX, readable(palette.success_emerald, palette))).child(t("Loaded")).into_any_element()
    } else {
        pill_button(("ai-load", i), Some("play"), t("Load"), accent, palette, move |_, cx| drop(act(Action::Load(name.clone()), cx))).into_any_element()
    };
    let tags: Vec<&str> = [m.parameter_size.as_str(), m.quantization.as_str(), m.family.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    div()
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(wash(WASH_FAINT, palette).hsla())
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(ROW_LINE_GAP_PX))
                .child(div().truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(m.name.clone()))
                .child(div().row().gap(px(tokens::XS)).children(tags.into_iter().map(|t| tag(t, palette)))),
        )
        .child(div().w(px(SIZE_W_PX)).flex_none().text_end().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(gib(m.size)))
        .child(action)
        .into_any_element()
}

// ── Views ────────────────────────────────────────────────────────────────

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    keep_fresh(CHIP_EVERY, window, cx);
    let m = model();
    let mb = m.borrow();
    let reachable = matches!(mb.data.as_deref(), Some(Ok(_)));
    let chip = mb.labels.as_ref().and_then(|l| l.chip.clone());
    let tint = if chip.is_some() { readable(palette.primary, palette) } else if reachable { palette.on_surface } else { palette.on_surface_variant.with_alpha(CHIP_IDLE_ALPHA) };
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(icon("sparkles", crate::style::bar_icon(CHIP_ICON_PX), tint))
        .when_some(chip, |el, label| el.child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(label)))
        .into_any_element()
}

struct View {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn set_height(&self, h: f32) {
        if (f32::from(self.size_cell.get().height) - h).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        if self.p.progress() > 0.0 {
            keep_fresh(PANEL_EVERY, window, cx);
        } else {
            model().borrow().views.track(window);
        }
        let m = model();
        let (data, subtitle, loading, unloading, flash) = {
            let mb = m.borrow();
            (mb.data.clone(), mb.labels.as_ref().map(|l| l.subtitle.clone()), mb.loading.clone(), mb.unloading.clone(), mb.flash.clone())
        };
        let flash = flash.current(window, cx);
        let accent = readable(palette.primary, palette);
        let mut h = PAD * 2.0 + HEADER_H;
        let mut blocks: Vec<AnyElement> = Vec::new();
        match data.as_deref() {
            None => {
                h += GAP + EMPTY_H;
                blocks.push(empty_state("sparkles", t("Looking for Ollama…"), url(cx), EMPTY_H, palette).into_any_element());
            }
            Some(Err(_)) => {
                h += GAP + EMPTY_H;
                blocks.push(empty_state("sparkles", t("Ollama isn't reachable"), t("Start it with ollama serve, or set its address in Studio → Plugins"), EMPTY_H, palette).into_any_element());
            }
            Some(Ok(s)) => {
                let loaded_h = if s.loaded.is_empty() { IDLE_H } else { s.loaded.len() as f32 * (CARD_H + CARD_GAP_PX) - CARD_GAP_PX };
                h += GAP + SECTION_H_PX + SECTION_GAP_PX + loaded_h;
                let loaded: AnyElement = if s.loaded.is_empty() {
                    div()
                        .h(px(IDLE_H))
                        .px(px(ROW_PAD_X_PX))
                        .rounded(px(crate::style::radius(IDLE_RADIUS_PX)))
                        .bg(wash(WASH_FAINT, palette).hsla())
                        .row()
                        .items_center()
                        .gap(px(tokens::SM))
                        .child(icon("moon", SMALL_ICON_PX, palette.on_surface_variant))
                        .child(caption(t("No model in memory: Load one below to warm it up"), palette))
                        .into_any_element()
                } else {
                    div().flex().flex_col().gap(px(CARD_GAP_PX)).children(s.loaded.iter().enumerate().map(|(i, l)| loaded_card(i, l, unloading.contains(&l.name), palette))).into_any_element()
                };
                blocks.push(div().flex().flex_col().gap(px(SECTION_GAP_PX)).child(section(t("Loaded now"), palette, None)).child(loaded).into_any_element());
                if s.installed.is_empty() {
                    h += GAP + EMPTY_H;
                    blocks.push(empty_state("download", t("No models installed"), t("Pull one with: ollama pull llama3.2"), EMPTY_H, palette).into_any_element());
                } else {
                    let list_h = (s.installed.len() as f32).min(VISIBLE_ROWS) * (ROW_H + ROW_GAP_PX) - ROW_GAP_PX;
                    h += GAP + SECTION_H_PX + SECTION_GAP_PX + list_h;
                    let rows: Vec<AnyElement> = s.installed.iter().enumerate().map(|(i, im)| installed_row(i, im, s.is_loaded(&im.name), loading.contains(&im.name), palette)).collect();
                    blocks.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(SECTION_GAP_PX))
                            .child(section(t("Installed"), palette, None))
                            .child(div().h(px(list_h)).child(div().id("ai-installed").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP_PX)).children(rows)))
                            .into_any_element(),
                    );
                }
            }
        }
        self.set_height(h);
        let subtitle = subtitle.unwrap_or_else(|| t("Ollama").into());
        let mut col = div().flex().flex_col().gap(px(GAP)).child(reveal(&self.p, 0, header("sparkles", accent, t("Local AI"), subtitle, palette, None)));
        for (i, b) in blocks.into_iter().enumerate() {
            col = col.child(reveal(&self.p, i + 1, b));
        }
        div().relative().size_full().p(px(PAD)).child(col).when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette))).into_any_element()
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Local AI"), tk("Ollama models in memory and on disk: VRAM, unload timers, load and unload"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(None, bar_view, window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("ai models", tk("Installed Ollama models (JSON)"), |_, cx| {
            let base = url(cx);
            Ok(crate::powerctl::pending(move || ai::fetch(&base).map(|s| json!({ "version": s.version, "models": s.installed }))))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("ai loaded", tk("Models loaded in memory with their VRAM share and unload time (JSON)"), |_, cx| {
            let base = url(cx);
            Ok(crate::powerctl::pending(move || {
                ai::fetch(&base).map(|s| {
                    let models: Vec<Value> = s.loaded.iter().map(|m| json!({ "name": m.name, "size": m.size, "size_vram": m.size_vram, "gpu_share": m.gpu_share(), "expires_at": m.expires_at, "parameter_size": m.parameter_size, "quantization": m.quantization, "family": m.family })).collect();
                    json!({ "version": s.version, "vram": s.vram(), "models": models })
                })
            }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("ai load", tk("Load a model into memory to warm it up (kept for 10 minutes)"), |a, cx| Ok(Output::Pending(act(Action::Load(a.rest(0)), cx))))
            .arg(ArgSpec::required("model", ArgKind::String, tk("installed model name, e.g. llama3.2:3b")))
            .example("ai load llama3.2"),
    );
    r.command(
        ID,
        CommandSpec::new_output("ai unload", tk("Unload a model from memory now, or every model with all"), |a, cx| {
            let q = a.rest(0);
            let action = if q == "all" { Action::UnloadAll } else { Action::Unload(q) };
            Ok(Output::Pending(act(action, cx)))
        })
        .arg(ArgSpec::required("model", ArgKind::String, tk("loaded model name, or all")))
        .example("ai unload qwen2.5:32b")
        .example("ai unload all"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_splits_read_naturally() {
        assert_eq!(split_label(1.0), "100% GPU");
        assert_eq!(split_label(0.38), "38% GPU / 62% CPU");
        assert_eq!(split_label(0.0), "100% CPU");
        assert_eq!(gib(6 * 1024 * 1024 * 1024 + 214_748_364), "6.2 GiB");
    }

    #[test]
    fn chip_label_shows_vram_only_while_loaded() {
        let loaded = Loaded { name: "m".into(), size: 4 << 30, size_vram: 3 << 30, expires_at: None, family: String::new(), parameter_size: String::new(), quantization: String::new() };
        let l = labels(&Ok(Snapshot { version: "0.12.3".into(), loaded: vec![loaded], installed: Vec::new() }));
        assert_eq!(l.chip.as_deref(), Some("3.0 GiB"));
        assert!(labels(&Ok(Snapshot::default())).chip.is_none());
        assert!(labels(&Err("down".into())).chip.is_none());
    }
}
