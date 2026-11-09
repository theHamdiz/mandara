//! The desktop background (`mandara-wallpaper` layer) drawn in software
//! into `wl_shm` buffers on its own Wayland connection and thread — no GPU
//! context, no swapchain.
//!
//! A GPUI window per output cost ~131 MiB of VRAM (four 3840 × 2160
//! swapchain images) plus the wallpaper texture, to show a picture that
//! changes once a minute. Here every output gets a
//! [`mandara_core::shm_canvas`] canvas whose planes are:
//!
//! | plane | content | redrawn |
//! |---|---|---|
//! | `WALLPAPER_A/B` | the wallpaper pre-scaled to the output with its fill mode | on wallpaper / fill / size change |
//! | `VIDEO_FILL` | one opaque pixel stretched behind a non-covering video | on fill / size change |
//! | `RING` | audio ring (its bounding box only) | ≤ 30 fps while music plays and the desktop shows |
//! | `HALO` | the clock's legibility glow, 1/8 resolution, scaled by the compositor | when the clock moves or the ink changes |
//! | `DIGITS` | the time | once a minute |
//! | `PILL` | Gregorian · Hijri date · weather | when they change |
//! | `CUT_A/B` | the depth subject, cropped to where the clock layer can be | when the clock moves or the mask changes |
//!
//! A video wallpaper ([`video`]) shows its first frame on the wallpaper
//! plane like a still, then plays there: each decoded frame is converted
//! straight into the plane's buffer at the clip's frame rate (capped),
//! paused whenever the desktop can't be seen.
//!
//! Wallpaper crossfades run in the compositor (`wp_alpha_modifier_v1` on the
//! incoming plane, zero CPU); wipes and grows crop it with `wp_viewporter`.
//! The clock layer sits *between* a wallpaper and its depth subject, and no
//! stack of per-plane alphas fades such a sandwich as one picture. So during
//! a transition the live planes keep showing the outgoing desktop, and the
//! incoming wallpaper plane, on top of them all, carries the whole incoming
//! desktop baked in once ([`bake`]): the screen is exactly the old desktop
//! fading to the new one. When it ends, the plane gets the wallpaper's own
//! pixels back and the live planes switch over in the same commit.
//! Static planes keep no copy once the compositor released their buffer.
//!
//! The GPUI side ([`init`]) only gathers what the shell knows — config,
//! theme, fonts, date line, depth masks, whether the desktop is visible and
//! music plays — into a [`Scene`] and posts it when it changes. Set
//! `MANDARA_WALLPAPER_RENDERER=gpui` for the old GPUI windows.
//!
//! The other way, the renderer posts right-clicks on the desktop (the
//! canvas root takes the pointer; [`crate::desktop_menu`] opens there) and,
//! only while [`crate::desktop_edit`] is active, where every widget was
//! painted ([`OutputBoxes`]), so the edit outlines match the pixels.

pub mod paint;
pub mod video;
pub mod widgets;

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Global};
use image::RgbImage;
use mandara_config::desktop::{WallpaperFill, WallpaperTransition, effective_wallpaper};
use mandara_config::outputs::{DisplaysConfig, SurfaceKind, assignment_any};
use mandara_config::desktop_widgets::Insets;
use mandara_config::WallpaperConfig;
use mandara_core::shm_canvas::{CanvasLayer, FRAME_TIMEOUT, Host, OutputInfo, Place, Rect, View, Waker, snap};
use mandara_theme::Color;

use self::paint::{CutSource, DecorGeometry, DecorInput, FontNames};
use crate::desktop::{self, DateLine, PILL_LIFT};
use crate::lock::paint::TextPainter;

/// `MANDARA_WALLPAPER_RENDERER=gpui` selects the GPUI windows instead.
pub const RENDERER_ENV: &str = "MANDARA_WALLPAPER_RENDERER";

/// Whether the software renderer draws the desktop (the default).
pub fn software_enabled() -> bool {
    std::env::var(RENDERER_ENV).map(|v| !v.trim().eq_ignore_ascii_case("gpui")).unwrap_or(true)
}

/// Linux input codes of the left and right mouse buttons (`BTN_LEFT`,
/// `BTN_RIGHT`).
/// How long a wallpaper that failed to load (a missing or unreadable file)
/// rests before it's tried again.
const LOAD_RETRY: Duration = Duration::from_secs(30);
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

// Plane indices on each canvas.
const WALLPAPER: [usize; 2] = [0, 1];
const RING: usize = 2;
const HALO: usize = 3;
const DIGITS: usize = 4;
const PILL: usize = 5;
const CUT: [usize; 2] = [6, 7];

/// Audio ring: frame period while drawn, poll period while waiting for
/// sound, easing rate (1/s), rest threshold, longest easing step, the
/// visibility below which it is not drawn, and the spectrum retry period.
const RING_FRAME: Duration = Duration::from_millis(33);
const RING_POLL: Duration = Duration::from_millis(250);
const RING_EASE_RATE: f32 = 5.0;
const RING_REST: f32 = 0.005;
const RING_MAX_DT_S: f32 = 0.25;
const RING_DRAW_MIN: f32 = 0.01;
const SPECTRUM_RETRY: Duration = Duration::from_secs(2);
/// Milliseconds per minute, and the slack after a minute boundary before
/// the clock redraws (so the new minute is already current).
const MS_PER_MINUTE: i64 = 60_000;
const MINUTE_WAKE_SLACK: Duration = Duration::from_millis(20);
/// Fade of a depth cut-out arriving, and of one leaving because depth was
/// switched off / became unavailable.
const CUTOUT_FADE: Duration = Duration::from_millis(400);
const CUTOUT_EXIT_FADE: Duration = Duration::from_millis(350);
/// Free the depth subject's pixels once its plane is painted (the
/// compositor holds the buffer; a repaint cuts them out again). Never
/// while the desktop is being edited: every drag step repaints the
/// subject, and cutting it out of a 4K wallpaper again each time held
/// edit mode to a frame or two a second.
const RELEASE_CUT_PIXELS: bool = true;
/// Shortest crossfade honoured, ms.
const MIN_TRANSITION_MS: u64 = 1;
/// Gap between the preview's two system samples (rates need a delta).
const PREVIEW_SAMPLE_GAP: Duration = Duration::from_millis(250);
/// Region around the clock (in em / block heights) sampled for its ink.
const TONE_HALF_W_EM: f32 = 1.3;
const TONE_HALF_H_BLOCK: f32 = 0.6;

// ── Scene (GPUI → renderer) ──────────────────────────────────────────────

/// Everything the renderer needs from the shell.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub enabled: bool,
    pub wallpaper: WallpaperConfig,
    pub dark: bool,
    pub h24: bool,
    pub rtl: bool,
    pub displays: DisplaysConfig,
    pub primary: Color,
    pub fonts: FontNames,
    pub date: Option<DateLine>,
    /// Ready depth masks per wallpaper, and the masks' generation.
    pub masks: HashMap<PathBuf, PathBuf>,
    pub mask_generation: u64,
    /// Outputs (connector names) whose audio ring should listen.
    pub ring: Vec<String>,
    /// Reduced motion: wallpapers switch without a transition.
    pub reduce_motion: bool,
    /// Displays the depth clock shows on (empty = all).
    pub depth_clock_displays: Vec<String>,
    /// Desktop widgets (`[desktop.widgets]`).
    pub widgets: widgets::WidgetScene,
    /// Edges the clock and widgets keep clear of (logical px) on outputs
    /// showing the bar, and on the others.
    pub safe_bar: Insets,
    pub safe_bare: Insets,
    /// Desktop edit mode is on: report where the widgets are painted.
    pub editing: bool,
    /// Outputs (connector names) whose video wallpaper may play now
    /// ([`video::may_play`]); empty when no output shows a video.
    pub video_play: Vec<String>,
    /// The depth clock's own digits and font.
    pub depth: DepthLook,
}

/// How the depth clock writes its time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DepthLook {
    /// Eastern Arabic digits.
    pub eastern: bool,
    /// The family of the stacked and classic digits (`None`: the display
    /// face).
    pub family: Option<String>,
}

/// Where a widget is painted on an output, logical px `[x, y, w, h]`:
/// its visible bounds and the box its placement positions (they differ by
/// the effect margin and anti-aliasing).
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetGeom {
    pub id: String,
    pub bounds: [f32; 4],
    pub outer: [f32; 4],
}

/// The painted widgets of one output (edit mode).
#[derive(Clone, Debug, PartialEq)]
pub struct OutputBoxes {
    /// Connector name.
    pub output: String,
    /// Logical size.
    pub size: (f32, f32),
    /// What the clock and widgets keep clear of here (logical px).
    pub safe: Insets,
    /// The depth clock's digits and date pill.
    pub clock: Option<[f32; 4]>,
    pub widgets: Vec<WidgetGeom>,
}

impl Scene {
    /// What the desktop keeps clear of on the output known by `names`
    /// (logical px).
    fn safe_for(&self, names: &[String]) -> Insets {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        if assignment_any(&self.displays, SurfaceKind::Bar, &names) { self.safe_bar } else { self.safe_bare }
    }
}

enum Msg {
    Scene(Box<Scene>),
    Edit(String, widgets::actions::Action),
    TextEditing(bool),
}

enum Event {
    /// The renderer's outputs (connector names).
    Outputs(Vec<String>),
    /// A right-click on the desktop: output, logical position, and the
    /// output's logical size as the canvas knows it (the menu covers
    /// exactly that output, even one connected after startup).
    Menu(String, f32, f32, Option<(f32, f32)>),
    /// Painted widget geometry (edit mode).
    Boxes(Vec<OutputBoxes>),
    /// A click (or a scroll step) on a widget's clickable area.
    Widget(String, widgets::actions::Action),
    Edit(String, widgets::actions::Action, crate::desktop_text::placement::Placement),
    FinishText,
    /// The renderer could not start; the shell falls back to GPUI.
    Failed(String),
}

// ── GPUI side ────────────────────────────────────────────────────────────

struct Driver {
    tx: mpsc::Sender<Msg>,
    waker: Arc<Waker>,
    outputs: Vec<String>,
    last: Option<Scene>,
    /// The idle manager / the power policy are observed (once a video
    /// wallpaper needs to know about screens off / battery).
    watching_idle: bool,
    watching_power: bool,
}

#[derive(Clone)]
struct GlobalBackdrop(Rc<RefCell<Driver>>);
impl Global for GlobalBackdrop {}

/// The outputs (connector names) the desktop is drawn on.
pub(crate) fn outputs(cx: &App) -> Vec<String> {
    cx.try_global::<GlobalBackdrop>().map(|g| g.0.borrow().outputs.clone()).unwrap_or_default()
}

/// Whether the software renderer is running in this process.
pub fn running(cx: &App) -> bool {
    cx.has_global::<GlobalBackdrop>()
}

/// Starts the renderer on the background layer. `fallback` runs (once) if
/// it cannot start, e.g. to open the GPUI windows instead.
pub fn init(fallback: impl FnOnce(&mut App) + 'static, cx: &mut App) {
    init_on(CanvasLayer::Background, fallback, cx);
}

/// [`init`] on a chosen layer (`Overlay` for screenshots).
pub fn init_on(layer: CanvasLayer, fallback: impl FnOnce(&mut App) + 'static, cx: &mut App) {
    if running(cx) {
        return;
    }
    let waker = match Waker::new() {
        Ok(w) => Arc::new(w),
        Err(err) => {
            tracing::error!("wallpaper renderer: {err:#}");
            fallback(cx);
            return;
        }
    };
    let (tx, rx) = mpsc::channel();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    let thread_waker = waker.clone();
    let spawned = std::thread::Builder::new().name("mandara-wallpaper".into()).spawn(move || {
        if let Err(err) = run(layer, rx, thread_waker, ev_tx.clone()) {
            tracing::error!("wallpaper renderer stopped: {err:#}");
            let _ = ev_tx.send_blocking(Event::Failed(format!("{err:#}")));
        }
    });
    if let Err(err) = spawned {
        tracing::error!("wallpaper renderer thread: {err}");
        fallback(cx);
        return;
    }
    let driver = Rc::new(RefCell::new(Driver { tx, waker, outputs: Vec::new(), last: None, watching_idle: false, watching_power: false }));
    cx.set_global(GlobalBackdrop(driver));

    let mut fallback = Some(fallback);
    cx.spawn(async move |cx| {
        while let Ok(event) = ev_rx.recv().await {
            cx.update(|cx| match event {
                Event::Outputs(names) => {
                    if let Some(g) = cx.try_global::<GlobalBackdrop>() {
                        g.0.borrow_mut().outputs = names;
                    }
                    refresh(cx);
                }
                Event::Menu(output, x, y, size) => {
                    crate::desktop_text::after_save(cx, move |cx| crate::desktop_menu::open_sized(&output, (x, y), size, cx));
                }
                Event::Boxes(boxes) => crate::desktop_edit::on_boxes(boxes, cx),
                Event::Widget(id, action) => {
                    crate::desktop_text::after_save(cx, move |cx| crate::desktop_actions::run(&id, action, cx));
                }
                Event::Edit(id, action, placement) => crate::desktop_text::open(id, action, placement, cx),
                Event::FinishText => { crate::desktop_text::finish(cx); }
                Event::Failed(why) => {
                    tracing::warn!("wallpaper: software renderer unavailable ({why}); using GPUI windows");
                    cx.remove_global::<GlobalBackdrop>();
                    if let Some(f) = fallback.take() {
                        f(cx);
                    }
                }
            });
        }
    })
    .detach();

    let config = crate::state::config(cx);
    cx.observe(&config, |_, cx| refresh(cx)).detach();
    let theme = mandara_core::theme_entity(cx);
    cx.observe(&theme, |_, cx| refresh(cx)).detach();
    let masks = crate::depth::masks(cx);
    cx.observe(&masks, |_, cx| refresh(cx)).detach();
    if let Some(hub) = cx.try_global::<crate::hub::GlobalHub>().map(|g| g.0.clone()) {
        cx.observe(&hub, |_, cx| refresh(cx)).detach();
    }
    cx.observe_global::<crate::fonts::Fonts>(refresh).detach();
    // Calendar events (the calendar widgets).
    if let Some(store) = crate::calendar::store(cx) {
        cx.observe(&store, |_, cx| refresh(cx)).detach();
    }
    // To-dos, notes and the focus timer (their desktop widgets).
    if let Some(p) = cx.try_global::<crate::productivity::GlobalProductivity>().map(|g| g.0.clone()) {
        cx.observe(&p, |_, cx| refresh(cx)).detach();
    }
    // Video wallpapers stop while the session is locked (a channel wait:
    // no cost until a lock happens).
    let locks = crate::lock::subscribe();
    cx.spawn(async move |cx| {
        while locks.recv().await.is_ok() {
            cx.update(refresh);
        }
    })
    .detach();
    // The date pill changes at minute boundaries (the Hijri day at Maghrib).
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(until_next_minute()).await;
            cx.update(refresh);
        }
    })
    .detach();
    refresh(cx);
}

fn until_next_minute() -> Duration {
    let into = u64::try_from(chrono::Local::now().timestamp_millis().rem_euclid(MS_PER_MINUTE)).unwrap_or(0);
    Duration::from_millis(MS_PER_MINUTE.unsigned_abs() - into) + MINUTE_WAKE_SLACK
}

/// Wakes the renderer so time-dependent widgets (the desktop clock) redraw
/// now: the clock was set, or its time zone changed.
pub(crate) fn wake(cx: &mut App) {
    if let Some(driver) = cx.try_global::<GlobalBackdrop>().map(|g| g.0.clone()) {
        driver.borrow().waker.wake();
    }
    refresh(cx);
}

/// Rebuilds the scene and posts it if it changed.
pub(crate) fn refresh(cx: &mut App) {
    let Some(driver) = cx.try_global::<GlobalBackdrop>().map(|g| g.0.clone()) else { return };
    let outputs = driver.borrow().outputs.clone();
    let scene = build_scene(&outputs, cx);
    if !scene.video_play.is_empty() || scene_has_video(&scene, &outputs) {
        watch_power(&driver, cx);
    }
    let mut d = driver.borrow_mut();
    if d.last.as_ref() == Some(&scene) {
        return;
    }
    d.last = Some(scene.clone());
    if d.tx.send(Msg::Scene(Box::new(scene))).is_ok() {
        d.waker.wake();
    }
}

/// Locate a widget only when its text is requested; idle rendering keeps
/// no additional geometry table or editor window.
pub(crate) fn edit_widget(widget: &str, action: widgets::actions::Action, cx: &App) -> Result<(), String> {
    let missing = || mandara_i18n::tf("No desktop widget {id}", &[("id", &widget)]);
    let driver = cx.try_global::<GlobalBackdrop>().ok_or_else(missing)?.0.borrow();
    driver.tx.send(Msg::Edit(widget.to_owned(), action)).map_err(|_| missing())?;
    driver.waker.wake();
    Ok(())
}

pub(crate) fn text_editing(active: bool, cx: &App) {
    if let Some(driver) = cx.try_global::<GlobalBackdrop>() {
        let driver = driver.0.borrow();
        if driver.tx.send(Msg::TextEditing(active)).is_ok() { driver.waker.wake(); }
    }
}

/// Whether any output's wallpaper is a video.
fn scene_has_video(scene: &Scene, outputs: &[String]) -> bool {
    let w = &scene.wallpaper;
    w.enabled && outputs.iter().map(Some).chain([None]).any(|o| effective_wallpaper(w.path.as_ref(), &w.variants, o.map(String::as_str), scene.dark).is_some_and(|p| video::is_video(&p)))
}

/// Observes the screens-off and battery state (from the first video
/// wallpaper on; still wallpapers never pay for it).
fn watch_power(driver: &Rc<RefCell<Driver>>, cx: &mut App) {
    let (idle, power) = {
        let d = driver.borrow();
        (d.watching_idle, d.watching_power)
    };
    if !idle && let Some(m) = crate::powerctl::idle::manager(cx) {
        cx.observe(&m, |_, cx| refresh(cx)).detach();
        driver.borrow_mut().watching_idle = true;
    }
    if !power && let Some(p) = crate::powerctl::battery::policy(cx) {
        cx.observe(&p, |_, cx| refresh(cx)).detach();
        driver.borrow_mut().watching_power = true;
    }
}

/// Outputs whose video wallpaper may play now: none unless one shows a
/// video (so stills never look anything up).
fn video_outputs(cfg: &mandara_config::ShellConfig, outputs: &[String], dark: bool, cx: &App) -> Vec<String> {
    let w = &cfg.wallpaper;
    if !w.enabled || w.video.still_frame {
        return Vec::new();
    }
    let videos: Vec<&String> = outputs.iter().filter(|o| effective_wallpaper(w.path.as_ref(), &w.variants, Some(o), dark).is_some_and(|p| video::is_video(&p))).collect();
    if videos.is_empty() {
        return Vec::new();
    }
    let base = video::Conditions {
        locked: crate::lock::is_locked(),
        screens_off: crate::powerctl::idle::manager(cx).is_some_and(|m| m.read(cx).screens_off()),
        on_battery: crate::powerctl::battery::on_battery(cx),
        ..video::Conditions::default()
    };
    videos
        .into_iter()
        .filter(|o| video::may_play(&w.video, &video::Conditions { covered: !crate::wallpaper::desktop_visible(o, cx), fullscreen: crate::wallpaper::fullscreen_on(o, cx), ..base }))
        .cloned()
        .collect()
}

/// Resolve one desktop scene, including any widget position currently being dragged.
fn build_scene(outputs: &[String], cx: &mut App) -> Scene {
    let mut cfg = crate::state::config(cx).read(cx).snapshot();
    let editing = crate::desktop_edit::active(cx);
    // A widget being dragged in edit mode is drawn where the pointer has
    // it, ahead of the config (written on release).
    let mut moving = None;
    if let Some((id, place)) = crate::desktop_edit::live_place(cx) {
        let c = std::sync::Arc::make_mut(&mut cfg);
        if let Some(w) = c.desktop.widgets.get_mut(&id) {
            w.place = place;
            mandara_config::desktop_widgets::sync_depth_clock(c);
        }
        moving = Some(id);
    }
    // Hidden widgets (the depth clock too) are shown again while editing.
    if cfg.desktop.widgets.hidden && !editing {
        let c = std::sync::Arc::make_mut(&mut cfg);
        c.desktop.widgets.items.clear();
        c.wallpaper.desktop_clock = false;
    }
    // The clocks plugin switched off takes the depth clock with it.
    if cfg.wallpaper.desktop_clock && !crate::desktop_plugins::enabled(mandara_config::desktop_widgets::plugin_ids::CLOCKS, &cfg, cx) {
        std::sync::Arc::make_mut(&mut cfg).wallpaper.desktop_clock = false;
    }
    let w = &cfg.wallpaper;
    let dark = cfg.theme.mode.is_dark();
    let mut widgets = widget_scene(&cfg, outputs, cx);
    widgets.moving = moving.filter(|_| !widgets.items.is_empty());
    let depth = (w.desktop_clock || widgets.any_behind()) && w.depth_effect && w.fill == WallpaperFill::Cover;
    let request = if depth { crate::depth::Request::of(w) } else { None };
    let entity = crate::depth::masks(cx);
    let mut masks = HashMap::new();
    if w.enabled && request.is_some() {
        let mut paths: Vec<PathBuf> = outputs.iter().filter_map(|o| effective_wallpaper(w.path.as_ref(), &w.variants, Some(o), dark)).collect();
        if paths.is_empty() {
            paths.extend(effective_wallpaper(w.path.as_ref(), &w.variants, None, dark));
        }
        paths.sort();
        paths.dedup();
        // Videos have no depth subject (it moves).
        paths.retain(|p| !video::is_video(p));
        for p in paths {
            if let crate::depth::MaskStatus::Ready(r) = entity.update(cx, |m, cx| m.status(&p, request, cx)) {
                masks.insert(p, r.path);
            }
        }
    }
    let date = (w.enabled && w.desktop_clock && w.show_date).then(|| desktop::date_line(&cfg, w.show_weather, cx));
    let video_play = video_outputs(&cfg, outputs, dark, cx);
    let ring = if w.enabled && w.desktop_clock && w.audio_ring && crate::wallpaper::music_playing(cx) {
        outputs.iter().filter(|o| crate::wallpaper::desktop_visible(o, cx)).cloned().collect()
    } else {
        Vec::new()
    };
    let fonts = FontNames::from(&crate::fonts::fonts(cx));
    let ui_rtl = cfg.general.locale.is_rtl();
    let depth = cfg
        .desktop
        .widgets
        .depth_clock()
        .map(|(_, c)| DepthLook {
            eastern: c.eastern(widgets::lingo::Lingo::of(&c.locale, ui_rtl, None).arabic()),
            family: (c.look.font != mandara_config::desktop_widgets::FontRole::Display).then(|| widgets::draw::family(&fonts.for_look(Some(&c.look)), c.look.font).to_string()),
        })
        .unwrap_or_default();
    // Slideshow changes may have their own transition.
    let mut wallpaper = w.clone();
    crate::slideshow::scene_transition(&mut wallpaper, cx);
    Scene {
        depth,
        enabled: w.enabled,
        wallpaper,
        dark,
        h24: cfg.general.clock_24h,
        rtl: cfg.general.locale.is_rtl(),
        displays: cfg.displays.clone(),
        primary: mandara_core::theme_entity(cx).read(cx).target().primary,
        fonts,
        date,
        masks,
        mask_generation: entity.read(cx).generation,
        ring,
        reduce_motion: cfg.motion.reduced(mandara_core::spring::system_reduced_motion()),
        depth_clock_displays: cfg.desktop.widgets.depth_clock().map(|(w, _)| w.place.displays.clone()).unwrap_or_default(),
        widgets,
        safe_bar: widgets::safe_insets(&cfg, true),
        safe_bare: widgets::safe_insets(&cfg, false),
        editing,
        video_play,
    }
}

/// The widgets part of the scene. Visibility and playback are looked up
/// only while a widget needs them.
fn widget_scene(cfg: &mandara_config::ShellConfig, outputs: &[String], cx: &mut App) -> widgets::WidgetScene {
    use mandara_config::desktop_widgets::WidgetKind;
    // A widget whose plugin is switched off isn't drawn.
    let items: Vec<_> = cfg.desktop.widgets.painted().filter(|w| crate::desktop_plugins::shows(w, cfg, cx)).cloned().collect();
    if items.is_empty() {
        return widgets::WidgetScene::default();
    }
    let mut ws = widgets::WidgetScene { items, units: cfg.units(), h24: cfg.general.clock_24h, rtl: cfg.general.locale.is_rtl(), week_start: crate::plugins::clock::week_start_days(cx), ..widgets::WidgetScene::default() };
    let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
    ws.on_surface = palette.on_surface;
    ws.surface = palette.surface_container;
    let weather_wanted = ws.items.iter().any(|w| matches!(&w.kind, WidgetKind::Clock(c) if c.weather));
    if weather_wanted {
        ws.weather = crate::hub::hub(cx).read(cx).weather.weather.clone().map(|w| (crate::plugins::weather::condition_icon(w.condition, w.is_day), cfg.units().format_degrees(w.temp)));
    }
    if ws.items.iter().any(|w| matches!(w.kind, WidgetKind::Weather(_))) {
        ws.forecast = crate::hub::hub(cx).read(cx).weather.weather.as_ref().map(|w| widgets::Forecast::from_weather(w, &crate::plugins::weather::location_name(cfg)));
    }
    // The Hijri date, while a widget may show it (the day turns at Maghrib).
    let hijri = ws.items.iter().any(|w| {
        w.kind.locale().is_some_and(|l| l.calendar.hijri()) || matches!(&w.kind, WidgetKind::Countdown(_)) || matches!(&w.kind, WidgetKind::Clock(c) if c.face == mandara_config::desktop_widgets::ClockFace::Template && c.lines.iter().any(|l| l.text.contains("{hijri}")))
    });
    if hijri {
        ws.hijri = Some(desktop::hijri_today(cfg, cx));
    }
    if ws.items.iter().any(|w| matches!(&w.kind, WidgetKind::WorldClock(c) if c.cities.is_empty())) {
        ws.world_cities = crate::plugins::world_clock::city_entries(cfg);
    }
    ws.data = widget_data(&ws.items, cfg, outputs, cx);
    if ws.any_live() {
        let screens_off = crate::powerctl::idle::manager(cx).is_some_and(|m| m.read(cx).screens_off());
        if !screens_off && !crate::lock::is_locked() {
            ws.visible = outputs.iter().filter(|o| crate::wallpaper::desktop_visible(o, cx)).cloned().collect();
        }
    }
    if ws.any_visualizer() || ws.items.iter().any(|w| matches!(w.kind, WidgetKind::Pet(_))) {
        ws.music = crate::wallpaper::music_playing(cx);
    }
    ws
}

/// Most notes the scene carries, and the characters of each one's text.
const SCENE_NOTES: usize = 24;
const NOTE_BODY_CHARS: usize = 1200;

/// What the widget collection's kinds show, read only for the kinds
/// placed.
fn widget_data(items: &[mandara_config::desktop_widgets::DesktopWidget], cfg: &mandara_config::ShellConfig, outputs: &[String], cx: &mut App) -> widgets::WidgetData {
    use mandara_config::desktop_widgets::WidgetKind as K;
    let has = |f: fn(&K) -> bool| items.iter().any(|w| f(&w.kind));
    let mut d = widgets::WidgetData::default();
    if has(|k| matches!(k, K::Sky(_))) {
        let q = crate::plugins::weather::query(cfg);
        d.place = Some((q.latitude, q.longitude));
    }
    if has(|k| matches!(k, K::Date(_))) {
        d.events = calendar_events(cfg, cx);
    }
    let Some(p) = cx.try_global::<crate::productivity::GlobalProductivity>().map(|g| g.0.clone()) else { return d };
    let p = p.read(cx);
    if has(|k| matches!(k, K::Todo(_))) {
        let mut todos: Vec<widgets::TodoItem> = p.book.todos.iter().map(|t| widgets::TodoItem { id: t.id, text: t.text.clone(), done: t.done, list: t.list }).collect();
        // Open tasks first, newest first within each.
        todos.sort_by_key(|t| t.done);
        d.todos = todos;
        d.lists = p.book.lists.iter().map(|l| (l.id, l.name.clone())).collect();
    }
    if has(|k| matches!(k, K::Note(_) | K::Notebook(_))) {
        d.notes = p.notes.notes.iter().filter(|n| !n.text.trim().is_empty()).take(SCENE_NOTES).map(|n| widgets::NoteItem { title: n.title(), body: n.preview().chars().take(NOTE_BODY_CHARS).collect() }).collect();
    }
    if has(|k| matches!(k, K::Pomodoro(_))) {
        let t = &p.pomodoro;
        let now_ms = chrono::Local::now().timestamp_millis();
        let phase = match t.phase {
            crate::productivity::Phase::Focus => mandara_i18n::tk("Focus"),
            crate::productivity::Phase::ShortBreak => mandara_i18n::tk("Short break"),
            crate::productivity::Phase::LongBreak => mandara_i18n::tk("Long break"),
        };
        let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        // Rounded to whole seconds so a scene rebuilt mid-second compares equal.
        let ends = t.running().then(|| (now_ms + ms(t.left())).div_euclid(SECOND_MS) * SECOND_MS);
        d.pomodoro = Some(widgets::PomodoroState { phase, ends_ms: ends, paused_ms: ms(t.paused_left), length_ms: ms(t.length), running: t.running(), completed: t.completed, long_every: crate::productivity::PomodoroRules::from_config(cx).long_every });
    }
    if let Some(turns) = cx.try_global::<crate::desktop_actions::Turns>() {
        let ours = turns.0.iter().filter(|(id, _)| items.iter().any(|w| &w.id == *id));
        d.turns = ours.clone().map(|(k, v)| (k.clone(), v.0)).collect();
        d.poked = ours.map(|(k, v)| (k.clone(), v.1)).collect();
    }
    identity_data(&mut d, items, cfg, cx);
    media_data(&mut d, items, cx);
    if has(|k| matches!(k, K::Fetch(_))) {
        d.fetch = crate::desktop_info::fetch(cx);
        if d.palette.is_empty() {
            d.palette = theme_palette(cx);
        }
    }
    // Only while the Screen time plugin records: otherwise the widget says
    // it's off (and how to turn it on) instead of showing stale days.
    if let Some(days) = items.iter().find_map(|w| if let K::ScreenTime(s) = &w.kind { Some(s.days) } else { None })
        && crate::desktop_plugins::enabled(crate::plugins::screen_time::ID, cfg, cx)
    {
        d.screen = crate::desktop_info::screen(days.clamp(mandara_config::widget_kinds::ranges::SCREEN_DAYS.0, mandara_config::widget_kinds::ranges::SCREEN_DAYS.1), cx);
    }
    let prayer_on = crate::desktop_plugins::enabled(crate::plugins::prayer::ID, cfg, cx);
    if has(|k| matches!(k, K::Prayer(_))) && prayer_on {
        d.prayer = Some(crate::desktop_info::prayer(cfg, cx));
    }
    if has(|k| matches!(k, K::Jannah(_))) && crate::desktop_plugins::enabled(crate::plugins::jannah::ID, cfg, cx) {
        d.jannah = crate::plugins::jannah::widget_info(cx);
    }
    if has(|k| matches!(k, K::Hadith(_))) && crate::desktop_plugins::enabled(crate::plugins::hadith::ID, cfg, cx) {
        d.hadith = crate::plugins::hadith::widget_info(cx);
    }
    if has(|k| matches!(k, K::Quran(_))) && crate::desktop_plugins::enabled(crate::plugins::quran::ID, cfg, cx) {
        d.quran = crate::plugins::quran::widget_info(cx);
    }
    if has(|k| matches!(k, K::Glance(_))) {
        d.glance = Some(crate::desktop_info::glance(cfg, prayer_on, cx));
    }
    feeds_data(&mut d, items, outputs, cx);
    crate::desktop_media::retain_asked(cx);
    d
}

/// The fetched feeds (GitHub, rates, AI usage, games): each widget's last
/// answer, and a new fetch only while the desktop can be seen and the
/// widget may ask (its online or log switch on).
fn feeds_data(d: &mut widgets::WidgetData, items: &[mandara_config::desktop_widgets::DesktopWidget], outputs: &[String], cx: &mut App) {
    use mandara_config::desktop_widgets::WidgetKind as K;
    use mandara_config::widget_kinds::{count, ranges};
    let feeds = items.iter().filter(|w| matches!(w.kind, K::Github(_) | K::Currency(_) | K::AiUsage(_) | K::Games(_)));
    let mut feeds = feeds.peekable();
    if feeds.peek().is_none() {
        return;
    }
    let seen = !crate::lock::is_locked() && outputs.iter().any(|o| crate::wallpaper::desktop_visible(o, cx));
    for w in feeds {
        match &w.kind {
            K::Github(g) if !g.user.trim().is_empty() => {
                let f = crate::desktop_feeds::github(&g.user, seen && g.online, cx);
                d.github.insert(w.id.clone(), f);
            }
            K::Currency(c) => {
                let f = crate::desktop_feeds::rates(&c.base, seen && c.online, cx);
                d.rates.insert(w.id.clone(), f);
            }
            K::AiUsage(a) if a.read_logs => {
                let f = crate::desktop_feeds::ai_usage(count(a.days, ranges::AI_DAYS).max(widgets::feeds::WEEK_DAYS), seen, cx);
                d.ai_usage.insert(w.id.clone(), f);
            }
            K::Games(g) => {
                let f = crate::desktop_feeds::games(seen, cx);
                let edge = (widgets::feeds::COVER_W * w.place.scale() * PICTURE_DENSITY).round().max(1.0) as u32;
                let list = f.value.as_ref().map(|list| {
                    list.iter()
                        .take(count(g.count, ranges::GAMES) as usize)
                        .map(|s| widgets::GameItem { appid: s.appid, name: s.name.clone(), cover: s.cover.as_deref().and_then(|p| crate::desktop_media::picture(p, edge, cx)) })
                        .collect()
                });
                d.games = widgets::Feed { value: list, error: f.error, loading: f.loading };
            }
            _ => {}
        }
    }
}

/// What plays, for the media widgets: the track, its art at the largest
/// widget's size and, when a lyrics widget shows, its synced lines.
fn media_data(d: &mut widgets::WidgetData, items: &[mandara_config::desktop_widgets::DesktopWidget], cx: &mut App) {
    use mandara_config::desktop_widgets::{MediaLayout, WidgetKind as K};
    let media: Vec<&mandara_config::desktop_widgets::MediaWidget> = items.iter().filter_map(|w| if let K::Media(m) = &w.kind { Some(m) } else { None }).collect();
    if media.is_empty() {
        return;
    }
    let Some(state) = crate::hub::try_hub(cx).and_then(|h| h.read(cx).media.clone()) else { return };
    if state.status == mandara_services::PlaybackStatus::Stopped && state.title.is_empty() {
        return;
    }
    let size = items.iter().filter_map(|w| if let K::Media(m) = &w.kind { Some(m.size * w.place.scale()) } else { None }).fold(0.0, f32::max);
    let art = state.art_path.as_deref().and_then(|p| crate::desktop_media::picture(p, (size * PICTURE_DENSITY).round().max(1.0) as u32, cx));
    let now_ms = chrono::Local::now().timestamp_millis();
    let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    let wants_lyrics = media.iter().any(|m| m.layout == MediaLayout::Lyrics);
    let (lyrics, loading) = if wants_lyrics { crate::desktop_media::lyrics(&state, cx) } else { (Vec::new(), false) };
    d.media = Some(widgets::MediaInfo {
        title: state.title.clone(),
        artist: state.artist.clone(),
        playing: state.status == mandara_services::PlaybackStatus::Playing,
        position_ms: ms(state.position),
        at_ms: now_ms - ms(state.position_at.elapsed()),
        rate: state.rate,
        length_ms: state.length.map(ms).unwrap_or(0),
        art,
        lyrics,
        lyrics_loading: loading,
    });
}

/// Physical px a picture is decoded for: its reference size at the
/// tallest display's density (a 4K screen is twice the reference).
const PICTURE_DENSITY: f32 = 2.0;

/// The profile, tiles, toggles, levels, palette and pictures the placed
/// widgets show.
fn identity_data(d: &mut widgets::WidgetData, items: &[mandara_config::desktop_widgets::DesktopWidget], cfg: &mandara_config::ShellConfig, cx: &mut App) {
    use mandara_config::desktop_widgets::WidgetKind as K;
    use widgets::actions::Action;
    let edge = |size: f32, scale: f32| (size * scale * PICTURE_DENSITY).round().max(1.0) as u32;
    for w in items {
        match &w.kind {
            K::Profile(_) if d.me.is_none() => {
                let me = crate::plugins::main_menu::me();
                let avatar = me.picture.as_deref().and_then(|p| crate::desktop_media::picture(p, edge(PROFILE_AVATAR_PX, w.place.scale()), cx));
                let dnd = crate::hub::try_hub(cx).is_some_and(|h| h.read(cx).notifications.dnd);
                d.me = Some(widgets::MeInfo { name: me.name, login: me.login, host: me.host, uptime_s: crate::plugins::main_menu::uptime_secs().unwrap_or(0), avatar, dnd });
            }
            K::Shortcuts(sw) => {
                let mut tiles = Vec::new();
                if sw.folders {
                    // The home folders that exist (named by the user-dirs
                    // config, else the usual English names).
                    let defaults = || {
                        let home = dirs::home_dir().unwrap_or_default();
                        let named = [(dirs::document_dir(), "Documents"), (dirs::download_dir(), "Downloads"), (dirs::picture_dir(), "Pictures"), (dirs::audio_dir(), "Music"), (dirs::video_dir(), "Videos")];
                        std::iter::once(home.clone()).chain(named.into_iter().map(|(d, name)| d.unwrap_or_else(|| home.join(name)))).filter(|p| p.is_dir()).map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>()
                    };
                    let list = if sw.items.is_empty() { defaults() } else { sw.items.clone() };
                    for item in list {
                        let path = mandara_config::paths::expand_tilde(item.trim());
                        let label = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| item.clone());
                        tiles.push(widgets::TileItem { label, action: Action::Open(path.to_string_lossy().to_string()), icon: None, glyph: "folder" });
                    }
                } else {
                    let list = if sw.items.is_empty() { cfg.dock.pinned.clone() } else { sw.items.clone() };
                    let apps = crate::desktop_media::apps(cx);
                    for item in list {
                        let item = item.trim().to_string();
                        if item.contains("://") {
                            let label = item.split("://").nth(1).unwrap_or(&item).trim_end_matches('/').to_string();
                            tiles.push(widgets::TileItem { label, action: Action::Open(item), icon: None, glyph: "globe" });
                            continue;
                        }
                        let id = item.trim_end_matches(".desktop").to_string();
                        let Some((name, icon)) = apps.as_ref().and_then(|a| a.get(&id).cloned()) else { continue };
                        let icon = icon.and_then(|p| crate::desktop_media::picture(&p, edge(TILE_ICON_PX, w.place.scale()), cx));
                        tiles.push(widgets::TileItem { label: name, action: Action::Launch(id), icon, glyph: "grid" });
                    }
                }
                d.tiles.insert(w.id.clone(), tiles);
            }
            K::Toggles(_) if d.toggles.is_empty() => {
                d.toggles = crate::desktop_actions::TOGGLES.iter().filter_map(|t| crate::desktop_actions::toggle_state(t, cx).map(|on| (t.to_string(), on))).collect();
            }
            K::Sliders(_) if d.levels.is_empty() => {
                d.levels = ["volume", "brightness"].into_iter().filter_map(|t| crate::desktop_actions::level(t, cx).map(|v| (t.to_string(), v))).collect();
            }
            K::Palette(_) if d.palette.is_empty() => d.palette = theme_palette(cx),
            K::Photo(p) => {
                let folder = if p.folder.trim().is_empty() { dirs::picture_dir().unwrap_or_default() } else { mandara_config::paths::expand_tilde(p.folder.trim()) };
                let list = crate::desktop_media::folder_pictures(&folder, cx);
                if !list.is_empty() {
                    let minutes = u64::from(p.minutes.clamp(mandara_config::widget_kinds::ranges::PHOTO_MINUTES.0, mandara_config::widget_kinds::ranges::PHOTO_MINUTES.1));
                    let slot = chrono::Local::now().timestamp().max(0) as u64 / (minutes * SECS_PER_MINUTE);
                    let turn = u64::from(d.turns.get(&w.id).copied().unwrap_or(0));
                    let path = list[((slot + turn) % list.len() as u64) as usize].clone();
                    let size = p.width.max(p.height) * w.place.scale();
                    if let Some(pic) = crate::desktop_media::picture(&path, edge(size, 1.0), cx) {
                        d.pictures.insert(w.id.clone(), pic);
                    }
                }
            }
            K::Sticker(st) if !st.image.trim().is_empty() => {
                let path = mandara_config::paths::expand_tilde(st.image.trim());
                if let Some(pic) = crate::desktop_media::picture(&path, edge(st.size, w.place.scale()), cx) {
                    d.pictures.insert(w.id.clone(), pic);
                }
            }
            _ => {}
        }
    }
}

/// The theme's colours with their names (catalog keys), as `0xRRGGBB`.
fn theme_palette(cx: &App) -> Vec<(String, u32)> {
    let p = mandara_core::theme_entity(cx).read(cx).target().clone();
    palette_colors(&p)
}

/// Shared palette swatches keep catalog keys until the widget chooses its language.
fn palette_colors(p: &mandara_theme::MandaraPalette) -> Vec<(String, u32)> {
    let rgb = |c: mandara_theme::Color| {
        let [r, g, b, _] = c.to_rgba8();
        (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    };
    [
        (mandara_i18n::tk("Primary"), p.primary),
        (mandara_i18n::tk("Secondary"), p.secondary),
        (mandara_i18n::tk("Tertiary"), p.tertiary),
        (mandara_i18n::tk("Surface"), p.surface_container),
        (mandara_i18n::tk("Text"), p.on_surface),
        (mandara_i18n::tk("Urgent"), p.urgent_coral),
        (mandara_i18n::tk("Success"), p.success_emerald),
        (mandara_i18n::tk("Warning"), p.warning_amber),
    ]
    .into_iter()
    .map(|(n, c)| (n.to_string(), rgb(c)))
    .collect()
}

/// Reference sizes of the avatar and app icons the identity widgets draw.
const PROFILE_AVATAR_PX: f32 = 96.0;
const TILE_ICON_PX: f32 = 48.0;
const SECS_PER_MINUTE: u64 = 60;

/// Milliseconds in a second.
const SECOND_MS: i64 = 1000;
/// Days of events the calendar widgets look ahead (the month grid's
/// remaining weeks).
const EVENT_DAYS: i64 = 42;
/// Most events the scene carries.
const SCENE_EVENTS: usize = 64;

/// Calendar events from the start of this month on, soonest first.
fn calendar_events(cfg: &mandara_config::ShellConfig, cx: &App) -> Vec<widgets::EventItem> {
    use chrono::Datelike as _;
    if !cfg.calendar.enabled {
        return Vec::new();
    }
    let Some(store) = crate::calendar::store(cx) else { return Vec::new() };
    let now = chrono::Local::now();
    let from = now.with_day(1).and_then(|d| d.date_naive().and_hms_opt(0, 0, 0)).and_then(|d| d.and_local_timezone(chrono::Local).earliest()).unwrap_or(now);
    let colors: Vec<Option<u32>> = cfg.calendar.sources.iter().filter(|s| s.enabled).map(|s| mandara_theme::Color::from_hex(s.color.trim()).ok().map(|c| {
        let [r, g, b, _] = c.to_rgba8();
        (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    })).collect();
    let mut out: Vec<widgets::EventItem> = store
        .read(cx)
        .between(from, now + chrono::Duration::days(EVENT_DAYS))
        .into_iter()
        .map(|o| widgets::EventItem {
            day: o.start.date_naive(),
            // An all-day event ends at the next midnight.
            last_day: (o.end - chrono::Duration::seconds(1)).date_naive().max(o.start.date_naive()),
            start_ms: o.start.timestamp_millis(),
            end_ms: o.end.timestamp_millis(),
            all_day: o.all_day,
            title: o.summary.clone(),
            color: colors.get(o.source).copied().flatten(),
        })
        .collect();
    out.sort_by_key(|e| e.start_ms);
    out.truncate(SCENE_EVENTS);
    out
}

// ── Renderer thread ──────────────────────────────────────────────────────

/// What a wallpaper plane should show.
#[derive(Clone, Debug, PartialEq)]
struct Want {
    path: PathBuf,
    fill: WallpaperFill,
    fill_color: String,
    size: (u32, u32),
}

/// A depth mask and the masks' generation it was resolved in.
type CutKey = (PathBuf, u64);

enum Job {
    Loaded { out: u32, serial: u64, want: Want, result: Result<(paint::Background, Option<(CutSource, CutKey)>), String> },
    Cut { out: u32, want: Want, key: CutKey, result: Result<Option<CutSource>, String> },
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum CutFade {
    Shown,
    In(Instant),
    Out(Instant),
}

struct Slot {
    want: Want,
    tone: RgbImage,
    cut: Option<Arc<CutSource>>,
    cut_key: Option<CutKey>,
    cut_fade: CutFade,
    /// A cut-only job for this key is running.
    cut_loading: Option<CutKey>,
    /// The cut's pixels were released after painting and a repaint needs
    /// them again ([`Self::cut_loading`] then re-cuts the same key).
    cut_refresh: bool,
}

struct Trans {
    from: usize,
    kind: WallpaperTransition,
    start: Instant,
    dur: Duration,
    rtl: bool,
    /// The incoming wallpaper's own pixels while its plane shows the baked
    /// incoming desktop ([`bake`]); painted back when the transition ends.
    raw: Option<tiny_skia::Pixmap>,
    /// The baked desktop includes the incoming depth subject (one cut out
    /// later fades in once the transition is over).
    baked_cut: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Geom {
    logical: (u32, u32),
    physical: (u32, u32),
    scale120: u32,
}

/// Content keys of the planes last painted (repaint only on change).
#[derive(Default)]
struct Painted {
    halo: Option<u64>,
    digits: Option<u64>,
    pill: Option<u64>,
    cut: [Option<u64>; 2],
    order: Vec<usize>,
    geo: Option<DecorGeometry>,
    accent: Option<Color>,
}

struct Out {
    id: u32,
    name: String,
    aliases: Vec<String>,
    geom: Option<Geom>,
    slots: [Option<Slot>; 2],
    cur: usize,
    serial: u64,
    loading: Option<(u64, Want)>,
    /// The last wallpaper that failed to load and when: not retried before
    /// [`LOAD_RETRY`], so a missing file isn't re-read on every pass.
    failed: Option<(Want, Instant)>,
    trans: Option<Trans>,
    painted: Painted,
    ring: f32,
    ring_at: Instant,
    ring_painted: Option<Instant>,
    widgets: widgets::OutWidgets,
}

impl Out {
    fn new(info: &OutputInfo) -> Self {
        Self {
            id: info.id,
            name: info.name.clone(),
            aliases: Vec::new(),
            geom: None,
            slots: [None, None],
            cur: 0,
            serial: 0,
            loading: None,
            failed: None,
            trans: None,
            painted: Painted::default(),
            ring: 0.0,
            ring_at: Instant::now(),
            ring_painted: None,
            widgets: widgets::OutWidgets::default(),
        }
    }

    fn reset(&mut self) {
        self.slots = [None, None];
        self.trans = None;
        self.loading = None;
        self.painted = Painted::default();
        self.ring = 0.0;
        self.ring_painted = None;
        self.widgets.clear();
    }

    fn current(&self) -> Option<&Slot> {
        self.slots[self.cur].as_ref()
    }

    /// The slot whose desktop the live planes (clock layer, widgets, depth
    /// subject) show: the outgoing one during a transition (the incoming
    /// desktop is baked into its wallpaper plane), the current one otherwise.
    fn live_slot(&self) -> usize {
        self.trans.as_ref().map_or(self.cur, |t| t.from)
    }
}

struct Renderer {
    text_editing: bool,
    scene: Option<Scene>,
    outs: Vec<Out>,
    text: Option<TextPainter>,
    /// Arabic fallback family set on `text`.
    arabic_font: Option<String>,
    alpha: bool,
    viewport: bool,
    jobs_tx: mpsc::Sender<Job>,
    waker: Arc<Waker>,
    tap: Option<mandara_services::spectrum::Spectrum>,
    tap_tried: Option<Instant>,
    reported: Option<Vec<String>>,
    /// Widget geometry last posted in edit mode.
    boxes: Option<Vec<OutputBoxes>>,
    /// Desktop widget services (system monitor sampler, spectrum).
    wshared: widgets::Shared,
    /// Playing video wallpapers, and what each output last painted of one.
    videos: video::Videos,
    video_states: HashMap<u32, video::OutVideo>,
}

fn key(parts: impl std::fmt::Debug) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{parts:?}").hash(&mut h);
    h.finish()
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn run(layer: CanvasLayer, rx: mpsc::Receiver<Msg>, waker: Arc<Waker>, events: async_channel::Sender<Event>) -> anyhow::Result<()> {
    let mut host = Host::connect(layer, mandara_core::SurfaceRole::Wallpaper.namespace())?;
    tracing::info!("wallpaper: software renderer (alpha modifier: {}, viewporter: {})", host.supports_alpha(), host.supports_viewport());
    let (jobs_tx, jobs_rx) = mpsc::channel();
    let mut r = Renderer {
        text_editing: false,
        scene: None,
        outs: Vec::new(),
        text: None,
        arabic_font: None,
        alpha: host.supports_alpha(),
        viewport: host.supports_viewport(),
        jobs_tx,
        waker: waker.clone(),
        tap: None,
        tap_tried: None,
        reported: None,
        boxes: None,
        wshared: widgets::Shared::default(),
        videos: video::Videos::default(),
        video_states: HashMap::new(),
    };
    loop {
        let mut new_scene = false;
        loop {
            match rx.try_recv() {
                Ok(Msg::Scene(s)) => {
                    r.scene = Some(*s);
                    new_scene = true;
                }
                Ok(Msg::Edit(widget, action)) => {
                    if !r.report_edit(&widget, &action, None, &events) {
                        tracing::warn!(widget, "desktop widget is not painted on a connected output");
                    }
                }
                Ok(Msg::TextEditing(active)) => r.text_editing = active,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if host.take_changed() {
            r.sync_outputs(&host, &events);
        }
        while let Ok(job) = jobs_rx.try_recv() {
            r.on_job(job, &mut host);
        }
        let now = Instant::now();
        r.step(&mut host, now);
        if new_scene {
            // What a change of the desktop (a widget dragged, a setting
            // moved) costs here: `diagnostics frames` → `desktop_ms`.
            crate::plugins::diagnostics::record_desktop(now.elapsed());
        }
        r.report_boxes(&events);
        r.report_presses(&mut host, &events);
        host.release_idle();
        let deadline = r.next_wake(&host, Instant::now()).map(|d| Instant::now() + d);
        // Pointer motion over the desktop only moves the stored position:
        // sleep on until something else happens (or a press arrives).
        while !host.wait(deadline.map(|d| d.saturating_duration_since(Instant::now())), &waker)? {}
    }
}

impl Renderer {
    fn report_edit(&self, widget: &str, action: &widgets::actions::Action, output: Option<&str>, events: &async_channel::Sender<Event>) -> bool {
        let Some((out, bounds, geom)) = self.outs.iter().filter(|out| output.is_none_or(|name| out.name == name)).find_map(|out| Some((out, out.widgets.bounds(widget)?, out.geom?))) else { return false };
        let k = mandara_core::shm_canvas::SCALE_DENOMINATOR as f32 / geom.scale120.max(1) as f32;
        let placement = crate::desktop_text::placement::Placement {
            output: out.name.clone(),
            bounds: [bounds.x as f32 * k, bounds.y as f32 * k, bounds.w as f32 * k, bounds.h as f32 * k],
            output_size: (geom.logical.0 as f32, geom.logical.1 as f32),
        };
        events.send_blocking(Event::Edit(widget.to_owned(), action.clone(), placement)).is_ok()
    }

    /// Right-clicks on the desktop go to the shell; left clicks and scroll
    /// steps on a widget's clickable area run its action (scrolling over a
    /// level nudges it); other buttons do nothing.
    fn report_presses(&self, host: &mut Host, events: &async_channel::Sender<Event>) {
        use mandara_core::shm_canvas::{SCALE_DENOMINATOR, SCROLL_DOWN, SCROLL_UP};
        use widgets::actions::Action;
        for p in host.take_presses() {
            let Some(out) = self.outs.iter().find(|o| o.id == p.output && !o.name.is_empty()) else { continue };
            match p.button {
                BTN_RIGHT => {
                    let size = out.geom.map(|g| (g.logical.0 as f32, g.logical.1 as f32));
                    let _ = events.send_blocking(Event::Menu(out.name.clone(), p.x as f32, p.y as f32, size));
                }
                BTN_LEFT | SCROLL_UP | SCROLL_DOWN => {
                    let Some(geom) = out.geom else { continue };
                    let k = geom.scale120 as f32 / SCALE_DENOMINATOR as f32;
                    let Some((id, action)) = out.widgets.hit((p.x as f32 * k, p.y as f32 * k)) else {
                        if p.button == BTN_LEFT && self.text_editing { let _ = events.send_blocking(Event::FinishText); }
                        continue;
                    };
                    let action = match (p.button, action) {
                        (BTN_LEFT, a) => a,
                        (b, Action::Level(what, _) | Action::Step(what, _)) => Action::Step(what, if b == SCROLL_UP { 1 } else { -1 }),
                        _ => continue,
                    };
                    if action.edits_text() {
                        self.report_edit(&id, &action, Some(&out.name), events);
                    } else {
                        let _ = events.send_blocking(Event::Widget(id, action));
                    }
                }
                _ => {}
            }
        }
    }

    /// In edit mode, posts where the widgets were painted when it changed.
    fn report_boxes(&mut self, events: &async_channel::Sender<Event>) {
        let Some(scene) = self.scene.as_ref().filter(|s| s.editing) else {
            self.boxes = None;
            return;
        };
        let mut all = Vec::new();
        for out in &self.outs {
            let Some(geom) = out.geom else { continue };
            let k = mandara_core::shm_canvas::SCALE_DENOMINATOR as f32 / geom.scale120.max(1) as f32;
            let logical = |r: Rect| [r.x as f32 * k, r.y as f32 * k, r.w as f32 * k, r.h as f32 * k];
            let clock = out.painted.geo.as_ref().map(|g| g.pill.map_or(g.digits, |p| g.digits.union(&p))).filter(|r| !r.is_empty()).map(logical);
            let widgets = out
                .widgets
                .geometry()
                .into_iter()
                .map(|(id, bounds, (x, y, w, h))| WidgetGeom { id, bounds: logical(bounds), outer: [x * k, y * k, w * k, h * k] })
                .collect();
            all.push(OutputBoxes { output: out.name.clone(), size: (geom.logical.0 as f32, geom.logical.1 as f32), safe: scene.safe_for(&out.aliases), clock, widgets });
        }
        if self.boxes.as_ref() != Some(&all) {
            self.boxes = Some(all.clone());
            let _ = events.send_blocking(Event::Boxes(all));
        }
    }

    fn sync_outputs(&mut self, host: &Host, events: &async_channel::Sender<Event>) {
        let infos = host.outputs();
        self.outs.retain(|o| infos.iter().any(|i| i.id == o.id));
        for info in &infos {
            let out = match self.outs.iter_mut().position(|o| o.id == info.id) {
                Some(i) => &mut self.outs[i],
                None => {
                    self.outs.push(Out::new(info));
                    self.outs.last_mut().expect("just pushed")
                }
            };
            out.name = info.name.clone();
            out.aliases = [&info.name, &info.identity, &info.description].into_iter().filter(|s| !s.is_empty()).cloned().collect();
            let geom = match (info.logical, info.physical) {
                (Some(logical), Some(physical)) => Some(Geom { logical, physical, scale120: info.scale120 }),
                _ => None,
            };
            if !info.has_canvas && out.geom.is_some() {
                out.reset();
            }
            if geom != out.geom {
                out.geom = geom;
                out.painted = Painted { order: Vec::new(), ..Painted::default() };
            }
        }
        let names: Vec<String> = infos.iter().map(|i| i.name.clone()).filter(|n| !n.is_empty()).collect();
        if self.reported.as_ref() != Some(&names) {
            self.reported = Some(names.clone());
            let _ = events.send_blocking(Event::Outputs(names));
        }
    }

    fn step(&mut self, host: &mut Host, now: Instant) {
        let Some(scene) = self.scene.clone() else { return };
        for i in 0..self.outs.len() {
            if let Err(err) = self.step_output(host, i, &scene, now) {
                tracing::warn!("wallpaper on {}: {err:#}", self.outs[i].name);
            }
        }
        self.step_video(host, &scene, now);
        self.step_ring(host, &scene, now);
        if !scene.widgets.items.is_empty() || self.wshared.sampler.is_some() || self.wshared.listening() {
            self.step_widgets(host, &scene, now);
        }
    }

    /// Plays the video wallpapers on screen (nothing at all without one).
    fn step_video(&mut self, host: &mut Host, scene: &Scene, now: Instant) {
        let mut shown: Vec<(PathBuf, Vec<video::Consumer>)> = Vec::new();
        for out in &self.outs {
            let (Some(geom), Some(slot)) = (out.geom, out.current()) else { continue };
            if !video::is_video(&slot.want.path) {
                continue;
            }
            // Mid-transition the plane holds its poster (or last frame).
            let allowed = out.trans.is_none() && scene.video_play.contains(&out.name);
            let [r, g, b, _] = crate::wallpaper_fill::fill_color(&slot.want.fill_color).to_rgba8();
            let c = video::Consumer { id: out.id, plane: WALLPAPER[out.cur], physical: geom.physical, scale120: geom.scale120, fill: slot.want.fill, fill_color: [r, g, b, u8::MAX], allowed };
            match shown.iter_mut().find(|(p, _)| *p == slot.want.path) {
                Some((_, list)) => list.push(c),
                None => shown.push((slot.want.path.clone(), vec![c])),
            }
        }
        if shown.is_empty() && self.videos.is_empty() {
            return;
        }
        let outs = &self.outs;
        self.video_states.retain(|id, _| outs.iter().any(|o| o.id == *id));
        self.videos.step(host, &shown, &mut self.video_states, scene.wallpaper.video.fps_cap(), &self.waker, now);
    }

    fn step_output(&mut self, host: &mut Host, i: usize, scene: &Scene, now: Instant) -> anyhow::Result<()> {
        let out = &mut self.outs[i];
        let id = out.id;
        let names: Vec<&str> = out.aliases.iter().map(String::as_str).collect();
        let assigned = scene.enabled && !out.name.is_empty() && assignment_any(&scene.displays, SurfaceKind::Wallpaper, &names);
        if !assigned {
            host.remove_canvas(id);
            out.reset();
            out.geom = None;
            return Ok(());
        }
        host.ensure_canvas(id);
        let Some(geom) = out.geom else { return Ok(()) };
        let w = &scene.wallpaper;
        let want = effective_wallpaper(w.path.as_ref(), &w.variants, Some(&out.name), scene.dark)
            .map(|path| Want { fill: video::effective_fill(&path, w.fill), path, fill_color: w.fill_color.clone(), size: geom.physical });
        let Some(want) = want else {
            // Nothing to show: a transparent canvas (another daemon may draw).
            for p in WALLPAPER.into_iter().chain(CUT).chain([RING, HALO, DIGITS, PILL, video::FILL_PLANE]) {
                host.hide(id, p);
            }
            out.widgets.remove_all(host, id);
            out.reset();
            host.set_base(id, None);
            host.commit(id, false)?;
            return Ok(());
        };
        // Load what should be shown.
        let shown = out.current().map(|s| &s.want) == Some(&want);
        let pending = out.loading.as_ref().is_some_and(|(_, w)| *w == want);
        let resting = out.failed.as_ref().is_some_and(|(w, at)| *w == want && at.elapsed() < LOAD_RETRY);
        if !shown && !pending && !resting {
            out.serial += 1;
            out.loading = Some((out.serial, want.clone()));
            let mask = mask_for(scene, &want);
            spawn_load(id, out.serial, want.clone(), mask, self.jobs_tx.clone(), self.waker.clone());
        }
        // Depth masks of the shown slots.
        let wanted_cut = out.current().and_then(|s| mask_for(scene, &s.want));
        let cur = out.cur;
        if let Some(slot) = out.slots[cur].as_mut() {
            match &wanted_cut {
                None if slot.cut.is_some() && !matches!(slot.cut_fade, CutFade::Out(_)) => {
                    slot.cut_fade = CutFade::Out(if self.alpha { now } else { now.checked_sub(CUTOUT_EXIT_FADE).unwrap_or(now) });
                    slot.cut_key = None;
                }
                Some(k) if slot.cut_key.as_ref() != Some(k) && slot.cut_loading.as_ref() != Some(k) => {
                    slot.cut_loading = Some(k.clone());
                    spawn_cut(id, slot.want.clone(), k.clone(), self.jobs_tx.clone(), self.waker.clone());
                }
                _ => {}
            }
        }
        // Animations and the clock layer.
        let animating = out.trans.is_some() || out.slots.iter().flatten().any(|s| matches!(s.cut_fade, CutFade::In(_) | CutFade::Out(_)));
        if animating && !host.frame_ready(id) {
            return Ok(());
        }
        self.step_transition(host, i, now)?;
        let text = text_painter(&mut self.text, &mut self.arabic_font, scene);
        decor(host, &mut self.outs[i], scene, text, self.viewport, &self.wshared)?;
        // The planes hold the pixels; the next repaint is a minute away.
        text.release_glyphs();
        let (tx, waker) = (&self.jobs_tx, &self.waker);
        for slot in self.outs[i].slots.iter_mut().flatten() {
            if std::mem::take(&mut slot.cut_refresh) && slot.cut_loading.is_none()
                && let Some(k) = slot.cut_key.clone()
            {
                tracing::debug!("depth cut-out: cutting the released subject again for a repaint");
                slot.cut_loading = Some(k.clone());
                spawn_cut(id, slot.want.clone(), k, tx.clone(), waker.clone());
            }
        }
        apply_alphas(host, &mut self.outs[i], self.alpha, now);
        let out = &mut self.outs[i];
        let animating = out.trans.is_some() || out.slots.iter().flatten().any(|s| matches!(s.cut_fade, CutFade::In(_) | CutFade::Out(_)));
        host.commit(id, animating)?;
        Ok(())
    }

    /// Advances a running wallpaper transition.
    fn step_transition(&mut self, host: &mut Host, i: usize, now: Instant) -> anyhow::Result<()> {
        let alpha = self.alpha;
        let out = &mut self.outs[i];
        let (id, to) = (out.id, out.cur);
        let Some(t) = out.trans.as_ref() else { return Ok(()) };
        let Some(geom) = out.geom else { return Ok(()) };
        let p = (now.saturating_duration_since(t.start).as_secs_f32() / t.dur.as_secs_f32().max(f32::EPSILON)).clamp(0.0, 1.0);
        let e = smoothstep(p);
        match t.kind {
            WallpaperTransition::Fade if alpha => host.set_alpha(id, WALLPAPER[to], e),
            WallpaperTransition::Wipe | WallpaperTransition::Grow => host.set_view(id, WALLPAPER[to], Some(reveal(t.kind, e, geom, t.rtl))),
            _ => {}
        }
        // No free buffer for the wallpaper's own pixels: the baked desktop
        // stays at full strength and the next frame tries again.
        if p >= 1.0 && !finish_transition(host, out, alpha)? {
            tracing::debug!("wallpaper transition: no buffer to restore the wallpaper yet");
        }
        Ok(())
    }

    fn on_job(&mut self, job: Job, host: &mut Host) {
        match job {
            Job::Loaded { out, serial, want, result } => {
                let Some(o) = self.outs.iter_mut().find(|o| o.id == out) else { return };
                if o.loading.as_ref().is_none_or(|(g, _)| *g != serial) {
                    return;
                }
                o.loading = None;
                let (bg, cut) = match result {
                    Ok(v) => v,
                    Err(err) => {
                        tracing::warn!("wallpaper: {err} (trying again in {} s)", LOAD_RETRY.as_secs());
                        o.failed = Some((want, Instant::now()));
                        return;
                    }
                };
                if o.geom.is_none_or(|g| g.physical != want.size) {
                    return;
                }
                let Some(scene) = self.scene.as_ref() else { return };
                let ring_bands = self.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands().to_vec()).unwrap_or_default();
                let text = text_painter(&mut self.text, &mut self.arabic_font, scene);
                let mut cx = BakeCx { text, viewport: self.viewport, wshared: &self.wshared, ring_bands: &ring_bands };
                if let Err(err) = show(host, o, want, bg, cut, scene, self.alpha, &mut cx) {
                    tracing::warn!("wallpaper on {}: {err:#}", o.name);
                }
            }
            Job::Cut { out, want, key, result } => {
                let alpha = self.alpha;
                let Some(o) = self.outs.iter_mut().find(|o| o.id == out) else { return };
                let Some(index) = o.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.want == want)) else { return };
                let Some(slot) = o.slots[index].as_mut() else { return };
                if slot.cut_loading.as_ref() != Some(&key) {
                    return;
                }
                slot.cut_loading = None;
                // A re-cut of the shown subject (pixels released) keeps its fade.
                let refresh = slot.cut_key.as_ref() == Some(&key) && slot.cut.as_ref().is_some_and(|c| c.pixmap.is_none());
                match result {
                    Ok(Some(cut)) => {
                        slot.cut = Some(Arc::new(cut));
                        if !refresh {
                            slot.cut_fade = if alpha { CutFade::In(Instant::now()) } else { CutFade::Shown };
                        }
                    }
                    Ok(None) => slot.cut = None,
                    Err(err) => {
                        tracing::warn!("depth cut-out: {err}");
                        slot.cut = None;
                    }
                }
                slot.cut_key = Some(key);
                // Only this slot's plane changes (the other's key covers
                // what it depends on).
                o.painted.cut[index] = None;
            }
        }
    }

    /// Desktop widgets: starts / stops their services and repaints their
    /// live planes (seconds, readings, visualizer frames) where due.
    fn step_widgets(&mut self, host: &mut Host, scene: &Scene, now: Instant) {
        let names: Vec<Vec<String>> = self.outs.iter().filter(|o| o.geom.is_some()).map(|o| o.aliases.clone()).collect();
        let refs: Vec<Vec<&str>> = names.iter().map(|n| n.iter().map(String::as_str).collect()).collect();
        widgets::step_services(&mut self.wshared, &scene.widgets, &refs, now);
        if scene.widgets.items.is_empty() {
            return;
        }
        let text = self.text.get_or_insert_with(TextPainter::new);
        let clock = chrono::Local::now();
        for out in &mut self.outs {
            let Some(geom) = out.geom else { continue };
            let Some(slot) = out.slots[out.live_slot()].as_ref() else { continue };
            let aliases: Vec<&str> = out.aliases.iter().map(String::as_str).collect();
            let o = widgets::OutputCx {
                id: out.id,
                names: &aliases,
                logical: (geom.logical.0 as f32, geom.logical.1 as f32),
                physical: geom.physical,
                scale120: geom.scale120,
                tone: Some(&slot.tone),
                primary: scene.primary,
                fonts: &scene.fonts,
                depth: scene.wallpaper.depth_effect && scene.wallpaper.fill == WallpaperFill::Cover,
                subject: slot.cut.as_deref().map(|c| &c.map),
                safe: scene.safe_for(&out.aliases).scaled(geom.scale120 as f32 / mandara_core::shm_canvas::SCALE_DENOMINATOR as f32),
            };
            let started = Instant::now();
            match widgets::step_live(host, &mut out.widgets, &o, &scene.widgets, &self.wshared, text, clock, now) {
                Ok(true) => {
                    // What one live frame (seconds, readings, an animation)
                    // costs: `diagnostics frames` → `desktop_live_ms`.
                    crate::plugins::diagnostics::record_desktop_live(started.elapsed());
                    let _ = host.commit(out.id, true);
                }
                Ok(false) => {}
                Err(err) => tracing::warn!("desktop widgets on {}: {err:#}", out.name),
            }
        }
    }

    fn step_ring(&mut self, host: &mut Host, scene: &Scene, now: Instant) {
        let wants: Vec<bool> = self
            .outs
            .iter()
            .map(|o| scene.wallpaper.audio_ring && o.painted.geo.is_some() && scene.ring.contains(&o.name))
            .collect();
        if wants.iter().any(|w| *w) {
            if self.tap.is_none() && self.tap_tried.is_none_or(|t| t.elapsed() > SPECTRUM_RETRY) {
                self.tap_tried = Some(now);
                self.tap = mandara_services::spectrum::subscribe();
            }
        } else {
            self.tap = None;
        }
        let bands = self.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands());
        for (out, want) in self.outs.iter_mut().zip(wants) {
            let id = out.id;
            let target = if want && bands.is_some() { 1.0 } else { 0.0 };
            let dt = now.saturating_duration_since(out.ring_at).as_secs_f32().min(RING_MAX_DT_S);
            let due = out.ring_painted.is_none_or(|t| now.saturating_duration_since(t) >= RING_FRAME);
            if !due && out.ring > 0.0 {
                continue;
            }
            out.ring_at = now;
            out.ring += (target - out.ring) * (1.0 - (-dt * RING_EASE_RATE).exp());
            if out.ring < RING_REST && target == 0.0 {
                out.ring = 0.0;
            }
            let (Some(geo), Some(accent)) = (out.painted.geo.clone(), out.painted.accent) else {
                host.hide(id, RING);
                continue;
            };
            if out.ring > RING_DRAW_MIN {
                if !host.frame_ready(id) {
                    continue;
                }
                let levels = bands.map(|b| b.to_vec()).unwrap_or_default();
                let ring = out.ring;
                match host.paint(id, RING, Place::Rect(geo.ring), None, true, true, |pm| paint::paint_ring(pm, &geo, &levels, ring, accent)) {
                    Ok(true) => {
                        out.ring_painted = Some(now);
                        let _ = host.commit(id, true);
                    }
                    Ok(false) => {}
                    Err(err) => tracing::warn!("audio ring: {err:#}"),
                }
            } else if host.is_mapped(id, RING) {
                host.hide(id, RING);
                out.ring_painted = None;
                let _ = host.commit(id, false);
            }
        }
    }

    fn next_wake(&self, host: &Host, now: Instant) -> Option<Duration> {
        let scene = self.scene.as_ref()?;
        let mut wake: Option<Duration> = None;
        let mut at = |d: Duration| wake = Some(wake.map_or(d, |w| w.min(d)));
        if let Some(d) = self.videos.next_wake(now) {
            at(d);
        }
        for out in &self.outs {
            let animating = out.trans.is_some() || out.slots.iter().flatten().any(|s| matches!(s.cut_fade, CutFade::In(_) | CutFade::Out(_)));
            if animating {
                // Woken by the frame callback; the timeout covers a hidden desktop.
                at(if host.frame_ready(out.id) { Duration::ZERO } else { FRAME_TIMEOUT });
            }
            if out.ring > 0.0 {
                at(out.ring_painted.map_or(Duration::ZERO, |t| RING_FRAME.saturating_sub(now.saturating_duration_since(t))));
            } else if scene.ring.contains(&out.name) && scene.wallpaper.audio_ring {
                at(RING_POLL);
            }
            if out.painted.digits.is_some() {
                at(until_next_minute());
            }
            if !scene.widgets.items.is_empty() && out.geom.is_some() {
                let names: Vec<&str> = out.aliases.iter().map(String::as_str).collect();
                if let Some(d) = widgets::next_wake(&out.widgets, &names, &scene.widgets, &self.wshared, chrono::Local::now(), now) {
                    at(d);
                }
            }
        }
        wake
    }
}

/// The ready depth mask for `want`, if depth applies to it.
fn mask_for(scene: &Scene, want: &Want) -> Option<CutKey> {
    let w = &scene.wallpaper;
    if !((w.desktop_clock || scene.widgets.any_behind()) && w.depth_effect && w.fill == WallpaperFill::Cover) || video::is_video(&want.path) {
        return None;
    }
    scene.masks.get(&want.path).map(|m| (m.clone(), scene.mask_generation))
}

fn spawn_load(out: u32, serial: u64, want: Want, mask: Option<CutKey>, tx: mpsc::Sender<Job>, waker: Arc<Waker>) {
    let job = move || {
        let result = (|| {
            let bg = load_background(&want)?;
            let cut = match &mask {
                Some(k) => paint::cut_source(&bg.pixmap, &k.0).unwrap_or_else(|e| {
                    tracing::warn!("depth cut-out: {e}");
                    None
                }),
                None => None,
            }
            .zip(mask);
            Ok((bg, cut))
        })();
        let _ = tx.send(Job::Loaded { out, serial, want, result });
        waker.wake();
    };
    if let Err(err) = std::thread::Builder::new().name("mandara-wp-load".into()).spawn(job) {
        tracing::warn!("wallpaper loader: {err}");
    }
}

fn spawn_cut(out: u32, want: Want, key: CutKey, tx: mpsc::Sender<Job>, waker: Arc<Waker>) {
    let job = move || {
        let result = load_background(&want).and_then(|bg| paint::cut_source(&bg.pixmap, &key.0));
        let _ = tx.send(Job::Cut { out, want, key, result });
        waker.wake();
    };
    if let Err(err) = std::thread::Builder::new().name("mandara-wp-cut".into()).spawn(job) {
        tracing::warn!("depth cut-out loader: {err}");
    }
}

/// Decodes the (cached, output-sized) copy of the wallpaper and places it.
fn load_background(want: &Want) -> Result<paint::Background, String> {
    let (w, h) = want.size;
    let (prepared, _) = crate::wallpaper_fill::prepare(&want.path, want.fill, w, h)?;
    let img = image::open(&prepared).map_err(|e| format!("{}: {e}", prepared.display()))?;
    paint::background(img, want.fill, want.size, crate::wallpaper_fill::fill_color(&want.fill_color))
}

/// What [`bake`] needs besides the output and the scene.
struct BakeCx<'a> {
    text: &'a mut TextPainter,
    /// The halo plane is scaled up by the compositor (`wp_viewporter`).
    viewport: bool,
    wshared: &'a widgets::Shared,
    /// Live audio levels for the ring (empty while silent).
    ring_bands: &'a [f32],
}

/// The shared text painter, with the Arabic fallback family of `scene`
/// (a scan of the font list: only when it changes).
fn text_painter<'a>(text: &'a mut Option<TextPainter>, arabic_font: &mut Option<String>, scene: &Scene) -> &'a mut TextPainter {
    let text = text.get_or_insert_with(TextPainter::new);
    // Arabic text the requested family lacks falls back to the user's
    // Arabic font first.
    if arabic_font.as_deref() != Some(scene.fonts.arabic.as_str()) {
        text.set_arabic_font(&scene.fonts.arabic);
        *arabic_font = Some(scene.fonts.arabic.clone());
    }
    text
}

/// Puts a freshly loaded wallpaper on screen (starting its transition).
#[allow(clippy::too_many_arguments)]
fn show(host: &mut Host, out: &mut Out, want: Want, bg: paint::Background, cut: Option<(CutSource, CutKey)>, scene: &Scene, alpha: bool, cx: &mut BakeCx<'_>) -> anyhow::Result<()> {
    let id = out.id;
    // An interrupted transition ends where it is. If its wallpaper cannot
    // get its own pixels back right now, its plane still holds a baked
    // desktop that must not become the outgoing one: cut to the new one.
    let mut cut_in = false;
    if out.trans.is_some() && !finish_transition(host, out, alpha)? {
        out.trans = None;
        cut_in = true;
    }
    let first = out.current().is_none();
    let size_changed = out.current().is_some_and(|s| s.want.size != want.size);
    let incoming = if first { out.cur } else { 1 - out.cur };
    let from = out.cur;
    let kind = scene.wallpaper.transition;
    let instant = cut_in || first || size_changed || scene.reduce_motion || kind == WallpaperTransition::None || (kind == WallpaperTransition::Fade && !alpha) || (kind != WallpaperTransition::Fade && !host.supports_viewport());
    let animated = !instant && incoming != from;
    host.hide(id, WALLPAPER[incoming]);
    host.hide(id, CUT[incoming]);
    host.set_view(id, WALLPAPER[incoming], None);
    host.set_alpha(id, WALLPAPER[incoming], 1.0);
    let (cut, cut_key) = match cut {
        Some((c, k)) => (Some(Arc::new(c)), Some(k)),
        None => (None, None),
    };
    let slot = Slot { want: want.clone(), tone: bg.tone, cut, cut_key, cut_fade: CutFade::Shown, cut_loading: None, cut_refresh: false };
    let pixmap = bg.pixmap;
    // A transition shows the whole incoming desktop on the incoming plane
    // (module docs); the wallpaper's own pixels wait for its end.
    let mut baked = None;
    let painted = host.paint(id, WALLPAPER[incoming], Place::Full, None, false, false, |pm| {
        pm.data_mut().copy_from_slice(pixmap.data());
        if animated {
            baked = bake(pm, out, &slot, scene, cx);
        }
    })?;
    if !painted {
        anyhow::bail!("no free buffer for the wallpaper");
    }
    let raw = baked.is_some().then_some(pixmap);
    out.slots[incoming] = Some(slot);
    out.cur = incoming;
    out.painted.cut[incoming] = None;
    if !animated {
        out.trans = None;
        host.hide(id, video::FILL_PLANE);
        if incoming != from {
            host.hide(id, WALLPAPER[from]);
            host.hide(id, CUT[from]);
            out.slots[from] = None;
        }
    } else {
        let geom = out.geom.unwrap_or(Geom { logical: want.size, physical: want.size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR });
        let dur = Duration::from_millis(scene.wallpaper.crossfade_ms.max(MIN_TRANSITION_MS));
        match kind {
            WallpaperTransition::Fade => host.set_alpha(id, WALLPAPER[incoming], 0.0),
            _ => host.set_view(id, WALLPAPER[incoming], Some(reveal(kind, 0.0, geom, scene.rtl))),
        }
        out.trans = Some(Trans { from, kind, start: Instant::now(), dur, rtl: scene.rtl, raw, baked_cut: baked == Some(true) });
    }
    // Posters include their fill; decoded video keeps it on a child plane.
    // Keep the root transparent so
    // Umbriel's overview, which mirrors only the root, has no solid tile.
    host.set_base(id, None);
    Ok(())
}

/// Ends the running transition: the incoming wallpaper stays alone, with
/// its own pixels painted back over the baked desktop (the live planes
/// switch to it in the same commit). `false` while no buffer is free for
/// that: the transition stays, at its end, for the next frame to finish.
fn finish_transition(host: &mut Host, out: &mut Out, alpha: bool) -> anyhow::Result<bool> {
    let id = out.id;
    let Some(t) = out.trans.as_ref() else { return Ok(true) };
    // A plane of another size is replaced by the reload that size change
    // started; it must not be filled from these pixels.
    let fits = |raw: &tiny_skia::Pixmap| out.geom.is_some_and(|g| g.physical == (raw.width(), raw.height()));
    if let Some(raw) = t.raw.as_ref().filter(|r| fits(r))
        && !host.paint(id, WALLPAPER[out.cur], Place::Full, None, false, false, |pm| pm.data_mut().copy_from_slice(raw.data()))?
    {
        return Ok(false);
    }
    let Some(t) = out.trans.take() else { return Ok(true) };
    host.set_alpha(id, WALLPAPER[out.cur], 1.0);
    host.set_view(id, WALLPAPER[out.cur], None);
    host.hide(id, video::FILL_PLANE);
    if t.from != out.cur {
        host.hide(id, WALLPAPER[t.from]);
        host.hide(id, CUT[t.from]);
        out.slots[t.from] = None;
        out.painted.cut[t.from] = None;
    }
    // A subject cut out after the desktop was baked was not on screen:
    // it fades in now, as one arriving at rest does.
    if !t.baked_cut
        && alpha
        && let Some(slot) = out.slots[out.cur].as_mut()
        && slot.cut.is_some()
        && !matches!(slot.cut_fade, CutFade::Out(_))
    {
        slot.cut_fade = CutFade::In(Instant::now());
    }
    out.painted.cut[out.cur] = None;
    Ok(true)
}

/// The incoming wallpaper's crop at eased progress `e` (wipe from the
/// reading side; grow from the centre).
fn reveal(kind: WallpaperTransition, e: f32, g: Geom, rtl: bool) -> View {
    let (lw, lh) = (g.logical.0.max(1) as i32, g.logical.1.max(1) as i32);
    let (pw, ph) = (g.physical.0 as f64, g.physical.1 as f64);
    let (x, y, w, h) = match kind {
        WallpaperTransition::Grow => {
            let (w, h) = (((lw as f32 * e).round() as i32).max(1), ((lh as f32 * e).round() as i32).max(1));
            ((lw - w) / 2, (lh - h) / 2, w, h)
        }
        _ => {
            let w = ((lw as f32 * e).round() as i32).max(1);
            (if rtl { lw - w } else { 0 }, 0, w, lh)
        }
    };
    let (sx, sy) = (pw / lw as f64, ph / lh as f64);
    let src_x = (x as f64 * sx).clamp(0.0, pw - 1.0);
    let src_y = (y as f64 * sy).clamp(0.0, ph - 1.0);
    let src_w = (w as f64 * sx).clamp(1.0, pw - src_x);
    let src_h = (h as f64 * sy).clamp(1.0, ph - src_y);
    View { source: (src_x, src_y, src_w, src_h), dest: (x, y, w, h) }
}

/// Lays out and paints the clock layer (only the planes whose content
/// changed) and the depth cut-out over it, for the live slot
/// ([`Out::live_slot`]).
fn decor(host: &mut Host, out: &mut Out, scene: &Scene, text: &mut TextPainter, viewport: bool, wshared: &widgets::Shared) -> anyhow::Result<()> {
    let id = out.id;
    let cfg = &scene.wallpaper;
    let has_widgets = !scene.widgets.items.is_empty();
    let live = out.live_slot();
    let geom = match out.geom {
        Some(g) if (cfg.desktop_clock || has_widgets) && out.slots[live].is_some() => g,
        _ => {
            hide_decor(host, out);
            return Ok(());
        }
    };
    let names: Vec<String> = out.aliases.clone();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let legacy = if clock_shows_here(scene, &names) {
        depth_clock(host, out, scene, text, viewport, geom)?
    } else {
        if out.painted.geo.is_some() || out.painted.digits.is_some() || out.painted.halo.is_some() || out.painted.pill.is_some() {
            for p in [RING, HALO, DIGITS, PILL] {
                host.hide(id, p);
            }
            out.painted.halo = None;
            out.painted.digits = None;
            out.painted.pill = None;
        }
        None
    };

    // Desktop widgets: their planes, and where the subject must cover them.
    // Run with an empty list too: the planes of widgets that went away (or
    // were switched off) are removed there, not just forgotten.
    let layers = {
        let slot = out.slots[live].as_ref();
        let tone = slot.map(|s| &s.tone);
        let subject = slot.and_then(|s| s.cut.as_deref()).map(|c| &c.map);
        let wallpaper_key = key(slot.map(|s| &s.want.path));
        let o = output_cx(out, scene, geom, &names, tone, subject);
        widgets::step_static(host, &mut out.widgets, &o, &scene.widgets, wshared, text, chrono::Local::now(), wallpaper_key)?
    };

    // Layer 3: the live slot's subject over the clock layer and the widgets
    // behind it. The other slot has no plane: during a transition it is
    // the incoming one, baked into its wallpaper plane.
    let clock = legacy.as_ref().map(|(geo, _)| geo);
    let cut_now = out.slots[live].as_ref().and_then(|s| s.cut.as_deref());
    // While editing, the subject covers all it can at once: a widget
    // dragged behind it then never makes it cut a larger area again (that
    // repaint cost most of each drag step at 4K). Drawing the subject over
    // the wallpaper it comes from changes no pixel.
    let reach = match cut_now {
        Some(c) if scene.editing && !layers.behind.is_empty() => layers.reach.union(&c.rect),
        _ => layers.reach,
    };
    let plan = subject_plan(clock, cfg, cut_now, reach, geom);
    let release = RELEASE_CUT_PIXELS && !scene.editing;
    for (s, &plane) in CUT.iter().enumerate() {
        let shown = if s == live { out.slots[s].as_ref().and_then(|slot| slot.cut.clone()) } else { None };
        let Some(cut) = shown else {
            host.hide(id, plane);
            out.painted.cut[s] = None;
            continue;
        };
        let (rect, limit) = (plan.rect, plan.limit);
        if rect.is_empty() || limit <= 0.0 {
            host.hide(id, plane);
            out.painted.cut[s] = None;
            // Nothing on this layer sits under the subject: don't keep its
            // pixels (a 4K subject is ~14 MiB); a later overlap cuts it again.
            if release && cut.pixmap.is_some() && let Some(slot) = out.slots[s].as_mut() {
                slot.cut = Some(Arc::new(cut.released()));
            }
            continue;
        }
        let k = out.slots[s].as_ref().map(|slot| key((rect, limit, &slot.want, &slot.cut_key)));
        if out.painted.cut[s] == k {
            // Kept through edit mode: freed on the first pass after it.
            if release && cut.pixmap.is_some() && let Some(slot) = out.slots[s].as_mut() {
                slot.cut = Some(Arc::new(cut.released()));
            }
            continue;
        }
        if cut.pixmap.is_none() {
            // Released after the last paint: the loop re-cuts it (the
            // plane keeps its previous pixels meanwhile).
            if let Some(slot) = out.slots[s].as_mut() {
                slot.cut_refresh = true;
            }
            continue;
        }
        if host.paint(id, plane, Place::Rect(rect), None, false, true, |pm| paint::paint_cut(pm, &cut, rect, limit))? {
            out.painted.cut[s] = k;
            if release && let Some(slot) = out.slots[s].as_mut() {
                slot.cut = Some(Arc::new(cut.released()));
            }
        }
    }

    let order = stack_order(out.trans.as_ref().map(|t| t.from), out.cur, plan.pill_on_top, &layers.behind, &layers.front);
    if out.painted.order != order {
        host.set_order(id, &order);
        out.painted.order = order;
    }
    match legacy {
        Some((geo, ink)) => {
            out.painted.accent = Some(ink.accent);
            out.painted.geo = Some(geo);
        }
        None => {
            out.painted.accent = None;
            out.painted.geo = None;
        }
    }
    Ok(())
}

/// Whether the depth clock shows on the output known by `names`.
fn clock_shows_here(scene: &Scene, names: &[&str]) -> bool {
    scene.wallpaper.desktop_clock && (scene.depth_clock_displays.is_empty() || scene.depth_clock_displays.iter().any(|d| names.iter().any(|n| n.eq_ignore_ascii_case(d))))
}

/// The output as the desktop widgets see it, over the wallpaper `tone`
/// with the subject `subject`.
fn output_cx<'a>(out: &Out, scene: &'a Scene, geom: Geom, names: &'a [&'a str], tone: Option<&'a RgbImage>, subject: Option<&'a image::GrayImage>) -> widgets::OutputCx<'a> {
    let cfg = &scene.wallpaper;
    widgets::OutputCx {
        id: out.id,
        names,
        logical: (geom.logical.0 as f32, geom.logical.1 as f32),
        physical: geom.physical,
        scale120: geom.scale120,
        tone,
        primary: scene.primary,
        fonts: &scene.fonts,
        depth: cfg.depth_effect && cfg.fill == WallpaperFill::Cover,
        subject,
        safe: scene.safe_for(&out.aliases).scaled(geom.scale120 as f32 / mandara_core::shm_canvas::SCALE_DENOMINATOR as f32),
    }
}

/// How a depth subject goes over the clock layer.
#[derive(Copy, Clone, Debug, PartialEq)]
struct SubjectPlan {
    /// The date pill rises above the subject (it would be hidden).
    pill_on_top: bool,
    /// The subject plane (physical px; empty = no plane).
    rect: Rect,
    /// Its opacity: a subject may overlap the clock, never swallow it.
    limit: f32,
}

/// The subject plane for `cut` over the clock `clock` (when it shows here)
/// and widgets behind the subject reaching `widgets_reach`.
fn subject_plan(clock: Option<&DecorGeometry>, cfg: &WallpaperConfig, cut: Option<&CutSource>, widgets_reach: Rect, geom: Geom) -> SubjectPlan {
    let Some(cut) = cut else { return SubjectPlan { pill_on_top: false, rect: Rect::default(), limit: 0.0 } };
    let limit = match clock {
        Some(geo) => crate::depth::overlap_alpha(crate::depth::coverage(&cut.map, geo.digits_fraction(cfg.clock_style))),
        // Widgets alone: the subject is drawn in full.
        None => 1.0,
    };
    let pill_on_top = clock.is_some_and(|geo| geo.pill.is_some() && crate::depth::coverage(&cut.map, geo.pill_fraction()) * limit > PILL_LIFT);
    let reach = clock.map(|geo| geo.reach(!pill_on_top, cfg.audio_ring)).unwrap_or_default().union(&widgets_reach);
    let output = Rect::new(0, 0, geom.physical.0, geom.physical.1);
    SubjectPlan { pill_on_top, rect: snap(reach.intersect(&cut.rect), geom.scale120).intersect(&output), limit }
}

/// Plane stacking, bottom to top. At rest: the wallpaper, audio ring, halo,
/// digits, the date pill (below the subject unless lifted), widgets behind
/// the subject, the subject, widgets in front. During a transition from
/// slot `from` that is the outgoing desktop, and the incoming wallpaper
/// plane (the whole incoming desktop baked in, [`bake`]) goes over it all.
fn stack_order(from: Option<usize>, cur: usize, pill_on_top: bool, behind: &[usize], front: &[usize]) -> Vec<usize> {
    let live = from.unwrap_or(cur);
    let mut order = vec![WALLPAPER[1 - live], WALLPAPER[live], RING, HALO, DIGITS];
    if from.is_some() {
        // The incoming plane goes on top instead.
        order.remove(0);
    }
    order.insert(0, video::FILL_PLANE);
    if !pill_on_top {
        order.push(PILL);
    }
    order.extend(behind);
    order.extend([CUT[1 - live], CUT[live]]);
    if pill_on_top {
        order.push(PILL);
    }
    order.extend(front);
    if from.is_some() {
        order.push(WALLPAPER[1 - live]);
    }
    order
}

/// The depth clock's layout and ink over a wallpaper.
struct ClockFrame {
    geo: DecorGeometry,
    ink: desktop::Ink,
    lines: Vec<String>,
    safe: Insets,
    /// The families, the digits set in the clock's own font.
    fonts: FontNames,
}

impl ClockFrame {
    /// The clock of `scene` on an output known by `aliases`, inked for the
    /// wallpaper whose small copy is `tone`.
    fn new(scene: &Scene, aliases: &[String], geom: Geom, tone: &RgbImage, text: &mut TextPainter) -> Self {
        let cfg = &scene.wallpaper;
        let (lw, lh) = (geom.logical.0 as f32, geom.logical.1 as f32);
        let safe = scene.safe_for(aliases);
        let l = desktop::layout_in(lw, lh, cfg, safe);
        let tone = desktop::region_tone(tone, (l.center.0 / lw, l.center.1 / lh), (l.size * TONE_HALF_W_EM / lw, l.block_h * TONE_HALF_H_BLOCK / lh));
        let ink = desktop::ink_for(Some(tone), scene.primary);
        let digits = |s: &String| if scene.depth.eastern { crate::widgets::clock::arabic_numerals(s) } else { crate::widgets::clock::western_numerals(s) };
        let lines: Vec<String> = desktop::clock_lines(&chrono::Local::now().naive_local(), cfg.clock_style, scene.h24).iter().map(digits).collect();
        let fonts = FontNames { display: scene.depth.family.clone().unwrap_or_else(|| scene.fonts.display.clone()), ..scene.fonts.clone() };
        let input = DecorInput { logical: (lw, lh), scale120: geom.scale120, cfg, lines: &lines, date: scene.date.as_ref(), ink, fonts: &fonts, safe };
        let geo = DecorGeometry::new(&input, text);
        Self { geo, ink, lines, safe, fonts }
    }

    fn input<'a>(&'a self, scene: &'a Scene, geom: Geom) -> DecorInput<'a> {
        let logical = (geom.logical.0 as f32, geom.logical.1 as f32);
        DecorInput { logical, scale120: geom.scale120, cfg: &scene.wallpaper, lines: &self.lines, date: scene.date.as_ref(), ink: self.ink, fonts: &self.fonts, safe: self.safe }
    }
}

/// The depth clock's planes (glow, digits, date pill); returns its
/// geometry and ink for the subject and the audio ring.
fn depth_clock(host: &mut Host, out: &mut Out, scene: &Scene, text: &mut TextPainter, viewport: bool, geom: Geom) -> anyhow::Result<Option<(DecorGeometry, desktop::Ink)>> {
    let id = out.id;
    let cfg = &scene.wallpaper;
    let Some(slot) = out.slots[out.live_slot()].as_ref() else { return Ok(None) };
    let frame = ClockFrame::new(scene, &out.aliases, geom, &slot.tone, text);
    let input = frame.input(scene, geom);
    let (geo, ink, l) = (&frame.geo, frame.ink, &frame.geo.layout);

    let k = key((geo.halo, geo.halo_buffer, ink.halo.to_rgba8(), l.center, l.size, l.block_h));
    if out.painted.halo != Some(k) {
        let place = if viewport { Place::Scaled { rect: geo.halo, buffer: geo.halo_buffer } } else { Place::Rect(geo.halo) };
        if !geo.halo.is_empty() && host.paint(id, HALO, place, None, false, true, |pm| paint::paint_halo(pm, geo, &ink))? {
            out.painted.halo = Some(k);
        }
    }
    let k = key((geo.digits, &frame.lines, ink.fg.to_rgba8(), cfg.clock_style, &frame.fonts, l.size, l.line_h));
    if out.painted.digits != Some(k) && !geo.digits.is_empty() && host.paint(id, DIGITS, Place::Rect(geo.digits), None, false, true, |pm| paint::paint_digits(pm, text, geo, &input))? {
        out.painted.digits = Some(k);
    }
    match geo.pill {
        Some(rect) => {
            let k = key((rect, &scene.date, ink.fg.to_rgba8(), ink.light, &scene.fonts, l.k));
            if out.painted.pill != Some(k) && host.paint(id, PILL, Place::Rect(rect), None, false, true, |pm| paint::paint_pill(pm, text, geo, &input))? {
                out.painted.pill = Some(k);
            }
        }
        None => {
            host.hide(id, PILL);
            out.painted.pill = None;
        }
    }
    Ok(Some((frame.geo.clone(), ink)))
}

/// Paints, over `slot`'s wallpaper already in `pm`, everything the live
/// planes would show for it — audio ring, halo, digits, date pill, widgets
/// behind the subject, the subject, a lifted pill, widgets in front — with
/// the painters and decisions of [`decor`], composed as the compositor
/// stacks those planes ([`paint::compose`]). Run once when a transition
/// starts; `None` when there is nothing to draw, else whether the subject
/// was drawn.
fn bake(pm: &mut tiny_skia::PixmapMut<'_>, out: &Out, slot: &Slot, scene: &Scene, cx: &mut BakeCx<'_>) -> Option<bool> {
    let geom = out.geom?;
    let cfg = &scene.wallpaper;
    let has_widgets = !scene.widgets.items.is_empty();
    if !(cfg.desktop_clock || has_widgets) {
        return None;
    }
    let names: Vec<&str> = out.aliases.iter().map(String::as_str).collect();
    let clock = clock_shows_here(scene, &names).then(|| ClockFrame::new(scene, &out.aliases, geom, &slot.tone, cx.text));
    let cut = slot.cut.as_deref();
    let o = output_cx(out, scene, geom, &names, Some(&slot.tone), cut.map(|c| &c.map));
    let now = chrono::Local::now();
    let layer = |r: Rect, paint: &mut dyn FnMut(&mut tiny_skia::PixmapMut<'_>)| {
        let mut plane = tiny_skia::Pixmap::new(r.w.max(1), r.h.max(1))?;
        paint(&mut plane.as_mut());
        Some(plane)
    };
    let plan_pill = subject_plan(clock.as_ref().map(|c| &c.geo), cfg, cut, Rect::default(), geom).pill_on_top;
    let mut pill = None;
    if let Some(frame) = &clock {
        let (geo, ink, input) = (&frame.geo, frame.ink, frame.input(scene, geom));
        let text = &mut *cx.text;
        if out.ring > RING_DRAW_MIN && let Some(p) = layer(geo.ring, &mut |p| paint::paint_ring(p, geo, cx.ring_bands, out.ring, ink.accent)) {
            paint::compose(pm, &p, geo.ring, 1.0);
        }
        if !geo.halo.is_empty() {
            let (w, h) = if cx.viewport { geo.halo_buffer } else { (geo.halo.w, geo.halo.h) };
            if let Some(p) = layer(Rect::new(0, 0, w, h), &mut |p| paint::paint_halo(p, geo, &ink)) {
                paint::compose(pm, &p, geo.halo, 1.0);
            }
        }
        if !geo.digits.is_empty() && let Some(p) = layer(geo.digits, &mut |p| paint::paint_digits(p, text, geo, &input)) {
            paint::compose(pm, &p, geo.digits, 1.0);
        }
        pill = geo.pill.and_then(|r| layer(r, &mut |p| paint::paint_pill(p, text, geo, &input)).map(|p| (p, r)));
    }
    if !plan_pill && let Some((p, r)) = &pill {
        paint::compose(pm, p, *r, 1.0);
    }
    let behind = if has_widgets { widgets::compose_side(pm, &o, &scene.widgets, cx.wshared, cx.text, &now, true) } else { Rect::default() };
    let plan = subject_plan(clock.as_ref().map(|c| &c.geo), cfg, cut, behind, geom);
    let mut drew_cut = false;
    if let Some(c) = cut.filter(|c| c.pixmap.is_some())
        && !plan.rect.is_empty()
        && plan.limit > 0.0
        && let Some(p) = layer(plan.rect, &mut |p| paint::paint_cut(p, c, plan.rect, plan.limit))
    {
        paint::compose(pm, &p, plan.rect, 1.0);
        drew_cut = true;
    }
    if plan_pill && let Some((p, r)) = &pill {
        paint::compose(pm, p, *r, 1.0);
    }
    if has_widgets {
        widgets::compose_side(pm, &o, &scene.widgets, cx.wshared, cx.text, &now, false);
    }
    Some(drew_cut)
}

fn hide_decor(host: &mut Host, out: &mut Out) {
    for p in [RING, HALO, DIGITS, PILL].into_iter().chain(CUT) {
        host.hide(out.id, p);
    }
    out.widgets.remove_all(host, out.id);
    let order = stack_order(out.trans.as_ref().map(|transition| transition.from), out.cur, false, &[], &[]);
    if out.painted.order != order {
        host.set_order(out.id, &order);
        out.painted.order = order;
    }
    let order = std::mem::take(&mut out.painted.order);
    out.painted = Painted { order, ..Painted::default() };
}

/// Alpha multipliers of the subject planes: their own fade in / out (a
/// wallpaper transition never fades them: the outgoing desktop stays whole
/// under the incoming one, [`stack_order`]).
fn apply_alphas(host: &mut Host, out: &mut Out, alpha: bool, now: Instant) {
    let id = out.id;
    for (s, &plane) in CUT.iter().enumerate() {
        let Some(slot) = out.slots[s].as_mut() else { continue };
        let fade = match slot.cut_fade {
            CutFade::Shown => 1.0,
            CutFade::In(t0) => {
                let f = now.saturating_duration_since(t0).as_secs_f32() / CUTOUT_FADE.as_secs_f32();
                if f >= 1.0 {
                    slot.cut_fade = CutFade::Shown;
                }
                smoothstep(f)
            }
            CutFade::Out(t0) => {
                let f = now.saturating_duration_since(t0).as_secs_f32() / CUTOUT_EXIT_FADE.as_secs_f32();
                if f >= 1.0 {
                    slot.cut = None;
                    slot.cut_fade = CutFade::Shown;
                    host.hide(id, plane);
                    out.painted.cut[s] = None;
                    continue;
                }
                1.0 - smoothstep(f)
            }
        };
        if alpha {
            host.set_alpha(id, plane, fade);
        }
    }
}

// ── Offscreen preview (same painters) ────────────────────────────────────

/// Composes exactly what the live canvas shows for `input` (wallpaper,
/// halo, digits, pill, depth subject; no audio ring) into a PNG.
pub fn render_preview(input: &desktop::PreviewInput) -> Result<serde_json::Value, String> {
    let (pw, ph) = input.size;
    let scale120 = ((input.scale.max(0.25)) * mandara_core::shm_canvas::SCALE_DENOMINATOR as f32).round() as u32;
    let scale = scale120 as f32 / mandara_core::shm_canvas::SCALE_DENOMINATOR as f32;
    let (lw, lh) = (pw as f32 / scale, ph as f32 / scale);
    let cfg = &input.cfg;
    let want = Want { path: input.wallpaper.clone(), fill: cfg.fill, fill_color: cfg.fill_color.clone(), size: (pw, ph) };
    let bg = load_background(&want)?;
    let mut canvas = bg.pixmap.clone();
    let fonts = FontNames::from(&input.fonts);
    let mut info = serde_json::json!({ "path": input.out, "size": [pw, ph], "scale": scale, "clock": input.lines, "renderer": "software" });
    let ws = &input.widgets;
    let behind_widgets = ws.any_behind();
    let mut text = TextPainter::new();
    text.set_arabic_font(&fonts.arabic);
    let depth = cfg.depth_effect && cfg.fill == WallpaperFill::Cover;
    let mask = if cfg.desktop_clock || behind_widgets {
        crate::depth::resolve(&input.wallpaper, if depth { crate::depth::Request::of(cfg) } else { None }, false)
    } else {
        Err("no clock or widget behind the subject".to_string())
    };
    let cut = match &mask {
        Ok(r) => paint::cut_source(&bg.pixmap, &r.path)?,
        Err(_) => None,
    };
    let now = chrono::Local::now();
    let names: [&str; 0] = [];
    let o = widgets::OutputCx { id: 0, names: &names, logical: (lw, lh), physical: (pw, ph), scale120, tone: Some(&bg.tone), primary: input.primary, fonts: &fonts, depth, subject: None, safe: Insets::default() };
    let sampler = ws.items.iter().any(|w| matches!(w.kind, mandara_config::desktop_widgets::WidgetKind::Monitor(_))).then(|| {
        let mut s = widgets::sysmon::Sampler::new();
        let mut wants = widgets::sysmon::Wants::default();
        for w in &ws.items {
            if let mandara_config::desktop_widgets::WidgetKind::Monitor(m) = &w.kind {
                wants.add(&m.rows);
            }
        }
        // Two samples: rates and CPU load need a delta.
        s.sample(wants, Instant::now());
        std::thread::sleep(PREVIEW_SAMPLE_GAP);
        s.sample(wants, Instant::now());
        s
    });
    let bands = widgets::preview_bands();
    let compose_widgets = |canvas: &mut tiny_skia::Pixmap, text: &mut TextPainter, behind: bool| {
        for w in ws.items.iter().filter(|w| (w.place.depth == mandara_config::desktop_widgets::Depth::Behind && depth) == behind) {
            widgets::compose(&mut canvas.as_mut(), w, &o, ws, sampler.as_ref(), text, &now, &bands);
        }
    };
    let mut limit = 1.0;
    let mut pill_on_top = false;
    let mut pill = None;
    let mut reach = None;
    if cfg.desktop_clock {
        let l = desktop::layout(lw, lh, cfg);
        let tone = desktop::region_tone(&bg.tone, (l.center.0 / lw, l.center.1 / lh), (l.size * TONE_HALF_W_EM / lw, l.block_h * TONE_HALF_H_BLOCK / lh));
        let ink = desktop::ink_for(Some(tone), input.primary);
        let date = input.date.as_ref();
        let dinput = DecorInput { logical: (lw, lh), scale120, cfg, lines: &input.lines, date, ink, fonts: &fonts, safe: Default::default() };
        let geo = DecorGeometry::new(&dinput, &mut text);
        let plane = |w: u32, h: u32| tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or_else(|| "bad plane size".to_string());
        let mut halo = plane(geo.halo_buffer.0, geo.halo_buffer.1)?;
        paint::paint_halo(&mut halo.as_mut(), &geo, &ink);
        paint::compose(&mut canvas.as_mut(), &halo, geo.halo, 1.0);
        let mut digits = plane(geo.digits.w, geo.digits.h)?;
        paint::paint_digits(&mut digits.as_mut(), &mut text, &geo, &dinput);
        paint::compose(&mut canvas.as_mut(), &digits, geo.digits, 1.0);
        pill = match geo.pill {
            Some(r) => {
                let mut pm = plane(r.w, r.h)?;
                paint::paint_pill(&mut pm.as_mut(), &mut text, &geo, &dinput);
                Some((pm, r))
            }
            None => None,
        };
        // Same decisions as the live canvas: the subject may overlap the
        // clock but never swallow it; the pill rises above a subject that
        // would hide it.
        limit = cut.as_ref().map(|c| crate::depth::overlap_alpha(crate::depth::coverage(&c.map, geo.digits_fraction(cfg.clock_style)))).unwrap_or(0.0);
        let pill_cover = match (&cut, geo.pill) {
            (Some(c), Some(_)) => crate::depth::coverage(&c.map, geo.pill_fraction()),
            _ => 0.0,
        };
        pill_on_top = pill_cover * limit > PILL_LIFT;
        if !pill_on_top && let Some((pm, r)) = &pill {
            paint::compose(&mut canvas.as_mut(), pm, *r, 1.0);
        }
        reach = Some(geo.reach(!pill_on_top, cfg.audio_ring));
        let mask_info = match &mask {
            Ok(r) => serde_json::json!({ "mask": r.path, "source": r.source, "detail": r.detail, "drawn": limit, "pill_coverage": pill_cover, "pill_on_top": pill_on_top }),
            Err(e) => serde_json::json!({ "mask": null, "reason": e }),
        };
        info["depth"] = mask_info;
        info["ink"] = serde_json::json!(if ink.light { "light" } else { "dark" });
        info["planes"] = serde_json::json!({
            "halo": [geo.halo.x, geo.halo.y, geo.halo.w, geo.halo.h, geo.halo_buffer.0, geo.halo_buffer.1],
            "digits": [geo.digits.x, geo.digits.y, geo.digits.w, geo.digits.h],
            "pill": geo.pill.map(|r| vec![r.x as i64, r.y as i64, r.w as i64, r.h as i64]),
            "ring": [geo.ring.x, geo.ring.y, geo.ring.w, geo.ring.h],
        });
    }
    compose_widgets(&mut canvas, &mut text, true);
    // The subject over the clock layer (all of it when widgets sit behind:
    // where nothing is under it, it repeats the wallpaper's own pixels).
    if let Some(c) = &cut {
        let full = Rect::new(0, 0, pw, ph);
        let area = if behind_widgets { full } else { reach.unwrap_or_default() };
        let rect = snap(area.intersect(&c.rect), scale120).intersect(&full);
        if !rect.is_empty() && limit > 0.0 {
            let mut pm = tiny_skia::Pixmap::new(rect.w.max(1), rect.h.max(1)).ok_or("bad plane size")?;
            paint::paint_cut(&mut pm.as_mut(), c, rect, limit);
            paint::compose(&mut canvas.as_mut(), &pm, rect, 1.0);
            info["cut_plane"] = serde_json::json!([rect.x, rect.y, rect.w, rect.h]);
        }
    }
    if pill_on_top && let Some((pm, r)) = &pill {
        paint::compose(&mut canvas.as_mut(), pm, *r, 1.0);
    }
    compose_widgets(&mut canvas, &mut text, false);
    info["widgets"] = serde_json::json!(ws.items.iter().map(|w| w.id.clone()).collect::<Vec<_>>());
    // Straight alpha for PNG (the canvas is opaque).
    let out = image::RgbaImage::from_raw(pw, ph, canvas.take()).ok_or("bad output")?;
    if let Some(dir) = input.out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    out.save_with_format(&input.out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom() -> Geom {
        Geom { logical: (2560, 1440), physical: (3840, 2160), scale120: 180 }
    }

    #[test]
    fn wipes_follow_the_reading_direction() {
        let v = reveal(WallpaperTransition::Wipe, 0.25, geom(), false);
        assert_eq!(v.dest, (0, 0, 640, 1440));
        assert_eq!(v.source, (0.0, 0.0, 960.0, 2160.0));
        let v = reveal(WallpaperTransition::Wipe, 0.25, geom(), true);
        assert_eq!(v.dest, (1920, 0, 640, 1440));
        assert_eq!(v.source.0, 2880.0);
        // Never an empty viewport (protocol error).
        let v = reveal(WallpaperTransition::Wipe, 0.0, geom(), false);
        assert!(v.dest.2 >= 1 && v.source.2 >= 1.0);
    }

    #[test]
    fn grow_reveals_from_the_centre_inside_the_buffer() {
        let v = reveal(WallpaperTransition::Grow, 0.5, geom(), false);
        assert_eq!(v.dest, (640, 360, 1280, 720));
        assert_eq!(v.source, (960.0, 540.0, 1920.0, 1080.0));
        let full = reveal(WallpaperTransition::Grow, 1.0, geom(), false);
        assert!(full.source.0 + full.source.2 <= 3840.0 && full.source.1 + full.source.3 <= 2160.0);
    }

    // ── Crossfade compositing (synthetic planes) ─────────────────────────

    /// Frame size of the synthetic desktops.
    const W: u32 = 48;
    const H: u32 = 32;
    /// Largest deviation from the exact blend allowed (8-bit rounding of the
    /// compositor's over operator, twice).
    const BLEND_TOLERANCE: u8 = 2;
    /// What the old stacking (subjects faded with the transition over a
    /// shared clock layer) got wrong by, at least, in these scenes.
    const OLD_STACK_ERROR: u8 = 20;
    /// Mid-fade progress values checked.
    const PROGRESS: [f32; 3] = [0.25, 0.5, 0.75];
    /// The clock: a soft halo and opaque digit blocks, frame px.
    const HALO_RECT: Rect = Rect::new(6, 2, 36, 26);
    const DIGITS_RECT: Rect = Rect::new(14, 7, 20, 14);

    /// An opaque wallpaper whose channels follow `f(x, y)`.
    fn wallpaper(f: impl Fn(u32, u32) -> [u8; 3]) -> tiny_skia::Pixmap {
        let mut pm = tiny_skia::Pixmap::new(W, H).unwrap();
        for (i, px) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let [r, g, b] = f(i as u32 % W, i as u32 / W);
            px.copy_from_slice(&[r, g, b, u8::MAX]);
        }
        pm
    }

    /// A premultiplied plane of `r`'s size, colour `rgb` at coverage `a(x, y)`.
    fn plane(r: Rect, rgb: [u8; 3], a: impl Fn(u32, u32) -> f32) -> tiny_skia::Pixmap {
        let mut pm = tiny_skia::Pixmap::new(r.w, r.h).unwrap();
        for (i, px) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let cov = a(i as u32 % r.w, i as u32 / r.w).clamp(0.0, 1.0);
            let k = |c: u8| (c as f32 * cov).round() as u8;
            px.copy_from_slice(&[k(rgb[0]), k(rgb[1]), k(rgb[2]), (cov * 255.0).round() as u8]);
        }
        pm
    }

    /// One desktop: wallpaper, clock layer (in its ink) and depth subject.
    struct Desk {
        wallpaper: tiny_skia::Pixmap,
        halo: tiny_skia::Pixmap,
        digits: tiny_skia::Pixmap,
        /// The subject plane and where it goes.
        cut: Option<(tiny_skia::Pixmap, Rect)>,
    }

    impl Desk {
        fn new(wallpaper: tiny_skia::Pixmap, ink: [u8; 3], mask: Option<image::GrayImage>, limit: f32) -> Self {
            let (hw, hh) = (HALO_RECT.w as f32 / 2.0, HALO_RECT.h as f32 / 2.0);
            let halo = plane(HALO_RECT, ink, |x, y| 0.55 * (1.0 - ((x as f32 - hw) / hw).powi(2) - ((y as f32 - hh) / hh).powi(2)));
            let digits = plane(DIGITS_RECT, ink, |x, y| if (x / 3 + y / 4) % 2 == 0 { 1.0 } else { 0.0 });
            let cut = mask.and_then(|m| paint::cut_from_alpha(&wallpaper, &m)).and_then(|c| {
                // As the live canvas: the subject only where the clock layer reaches.
                let rect = snap(HALO_RECT.union(&DIGITS_RECT).intersect(&c.rect), SCALE_1X);
                let mut pm = tiny_skia::Pixmap::new(rect.w, rect.h)?;
                paint::paint_cut(&mut pm.as_mut(), &c, rect, limit);
                Some((pm, rect))
            });
            Self { wallpaper, halo, digits, cut }
        }

        /// The planes over the wallpaper, bottom to top, by plane index.
        fn layers(&self, slot: usize) -> Vec<(usize, &tiny_skia::Pixmap, Rect)> {
            let mut l = vec![(HALO, &self.halo, HALO_RECT), (DIGITS, &self.digits, DIGITS_RECT)];
            l.extend(self.cut.as_ref().map(|(pm, r)| (CUT[slot], pm, *r)));
            l
        }

        /// The desktop alone, composed as the compositor stacks its planes.
        fn composite(&self) -> tiny_skia::Pixmap {
            let mut pm = self.wallpaper.clone();
            self.bake(&mut pm);
            pm
        }

        /// What a transition paints on the incoming plane ([`bake`]): the
        /// clock layer and the subject over the wallpaper already there.
        fn bake(&self, pm: &mut tiny_skia::Pixmap) {
            for (_, p, r) in self.layers(0) {
                paint::compose(&mut pm.as_mut(), p, r, 1.0);
            }
        }
    }

    /// Scale 1 (grid step 1).
    const SCALE_1X: u32 = mandara_core::shm_canvas::SCALE_DENOMINATOR;

    /// The compositor: `planes` (index → pixels, place, alpha multiplier)
    /// stacked in `order` over black.
    fn screen(order: &[usize], planes: &[(usize, &tiny_skia::Pixmap, Rect, f32)]) -> tiny_skia::Pixmap {
        let mut out = tiny_skia::Pixmap::new(W, H).unwrap();
        out.fill(tiny_skia::Color::BLACK);
        for i in order {
            for (_, pm, r, a) in planes.iter().filter(|p| p.0 == *i) {
                paint::compose(&mut out.as_mut(), pm, *r, *a);
            }
        }
        out
    }

    /// Largest channel difference between `got` and `(1 − e)·a + e·b` in `r`.
    fn deviation(got: &tiny_skia::Pixmap, a: &tiny_skia::Pixmap, b: &tiny_skia::Pixmap, e: f32, r: Rect) -> u8 {
        let mut worst = 0;
        for y in r.y as u32..r.bottom() as u32 {
            for x in r.x as u32..r.right() as u32 {
                let i = ((y * W + x) * 4) as usize;
                for c in 0..3 {
                    let want = ((1.0 - e) * a.data()[i + c] as f32 + e * b.data()[i + c] as f32).round() as i32;
                    worst = worst.max((got.data()[i + c] as i32 - want).unsigned_abs() as u8);
                }
            }
        }
        worst
    }

    /// The renderer's planes mid-transition from slot 0 (`old`) to slot 1
    /// (`new`, baked into its wallpaper plane) at eased progress `e`.
    fn mid_fade(old: &Desk, new: &Desk, e: f32) -> tiny_skia::Pixmap {
        let (from, cur) = (0, 1);
        let mut baked = new.wallpaper.clone();
        new.bake(&mut baked);
        let full = Rect::new(0, 0, W, H);
        let mut planes = vec![(WALLPAPER[from], &old.wallpaper, full, 1.0), (WALLPAPER[cur], &baked, full, e)];
        planes.extend(old.layers(from).into_iter().map(|(i, p, r)| (i, p, r, 1.0)));
        screen(&stack_order(Some(from), cur, false, &[], &[]), &planes)
    }

    /// What the stacking before baking showed: both wallpapers, one clock
    /// layer (the incoming ink), each subject faded with the transition.
    fn old_mid_fade(old: &Desk, new: &Desk, e: f32) -> tiny_skia::Pixmap {
        let full = Rect::new(0, 0, W, H);
        let mut planes = vec![(WALLPAPER[0], &old.wallpaper, full, 1.0), (WALLPAPER[1], &new.wallpaper, full, e)];
        planes.extend(new.layers(1).into_iter().filter(|(i, ..)| !CUT.contains(i)).map(|(i, p, r)| (i, p, r, 1.0)));
        planes.extend(old.cut.as_ref().map(|(p, r)| (CUT[0], p, *r, 1.0 - e)));
        planes.extend(new.cut.as_ref().map(|(p, r)| (CUT[1], p, *r, e)));
        screen(&[WALLPAPER[0], WALLPAPER[1], HALO, DIGITS, CUT[0], CUT[1]], &planes)
    }

    /// A subject filling the lower part, with a feathered top edge.
    fn dunes() -> image::GrayImage {
        image::GrayImage::from_fn(W, H, |x, y| image::Luma([((y as f32 - 14.0 - (x as f32 / 6.0).sin() * 3.0) * 60.0).clamp(0.0, 255.0) as u8]))
    }

    /// A round subject on the left.
    fn blob() -> image::GrayImage {
        image::GrayImage::from_fn(W, H, |x, y| image::Luma([((10.0 - ((x as f32 - 16.0).powi(2) + (y as f32 - 14.0).powi(2)).sqrt()) * 50.0).clamp(0.0, 255.0) as u8]))
    }

    fn desks(old_mask: Option<image::GrayImage>, new_mask: Option<image::GrayImage>) -> (Desk, Desk) {
        let violet = wallpaper(|x, y| [60 + x as u8, 40 + y as u8 * 2, 120]);
        let dune = wallpaper(|x, y| [240 - x as u8, 190 - y as u8 * 3, 150 + (x as u8 % 7) * 5]);
        // Light ink on the dark wallpaper, dark ink on the light one.
        (Desk::new(violet, [245, 240, 255], old_mask, 1.0), Desk::new(dune, [30, 20, 15], new_mask, 0.8))
    }

    #[test]
    fn crossfades_blend_the_whole_desktops() {
        let clock = HALO_RECT.union(&DIGITS_RECT);
        let full = Rect::new(0, 0, W, H);
        for (name, old_mask, new_mask) in [("to a subject", None, Some(dunes())), ("from a subject", Some(dunes()), None), ("subject to subject", Some(blob()), Some(dunes()))] {
            let (old, new) = desks(old_mask, new_mask);
            let (a, b) = (old.composite(), new.composite());
            let mut old_worst = 0;
            for e in PROGRESS {
                let got = mid_fade(&old, &new, e);
                let d = deviation(&got, &a, &b, e, clock);
                assert!(d <= BLEND_TOLERANCE, "{name} at {e}: the clock area is {d} levels off the blend");
                assert!(deviation(&got, &a, &b, e, full) <= BLEND_TOLERANCE, "{name} at {e}: the frame is off the blend");
                old_worst = old_worst.max(deviation(&old_mid_fade(&old, &new, e), &a, &b, e, clock));
            }
            // The check catches the rectangle the old stacking showed.
            assert!(old_worst >= OLD_STACK_ERROR, "{name}: the old stacking was only {old_worst} levels off");
        }
    }

    #[test]
    fn a_transition_ends_on_the_live_desktop() {
        // At the end the incoming plane gets its own pixels back and the live
        // planes show the new desktop: no pop from the baked picture.
        for new_mask in [Some(dunes()), None] {
            let (old, new) = desks(Some(blob()), new_mask);
            let full = Rect::new(0, 0, W, H);
            let baked = mid_fade(&old, &new, 1.0);
            let mut planes = vec![(WALLPAPER[1], &new.wallpaper, full, 1.0)];
            planes.extend(new.layers(1).into_iter().map(|(i, p, r)| (i, p, r, 1.0)));
            let live = screen(&stack_order(None, 1, false, &[], &[]), &planes);
            assert!(deviation(&live, &baked, &baked, 0.0, full) <= BLEND_TOLERANCE);
        }
        // A mask cut out mid-fade was not baked: its plane starts the usual
        // fade-in from nothing, so the first live frame is the baked one.
        let (old, new) = desks(None, None);
        let (_, late) = desks(None, Some(dunes()));
        let baked = mid_fade(&old, &new, 1.0);
        let full = Rect::new(0, 0, W, H);
        let (cut, rect) = late.cut.as_ref().unwrap();
        let fade_in = smoothstep(0.0);
        let mut planes = vec![(WALLPAPER[1], &new.wallpaper, full, 1.0), (CUT[1], cut, *rect, fade_in)];
        planes.extend(new.layers(1).into_iter().map(|(i, p, r)| (i, p, r, 1.0)));
        let live = screen(&stack_order(None, 1, false, &[], &[]), &planes);
        assert!(deviation(&live, &baked, &baked, 0.0, full) <= BLEND_TOLERANCE);
    }

    #[test]
    fn stacking_puts_the_baked_incoming_plane_on_top() {
        let (behind, front) = ([PLANE_A], [PLANE_B]);
        // At rest: as it always was.
        assert_eq!(stack_order(None, 1, false, &behind, &front), vec![video::FILL_PLANE, WALLPAPER[0], WALLPAPER[1], RING, HALO, DIGITS, PILL, PLANE_A, CUT[0], CUT[1], PLANE_B]);
        assert_eq!(stack_order(None, 0, true, &[], &[]), vec![video::FILL_PLANE, WALLPAPER[1], WALLPAPER[0], RING, HALO, DIGITS, CUT[1], CUT[0], PILL]);
        // Mid-transition from slot 0 to 1: the outgoing desktop, whole, under
        // the incoming wallpaper plane.
        let order = stack_order(Some(0), 1, false, &behind, &front);
        assert_eq!(order.first(), Some(&video::FILL_PLANE));
        assert_eq!(order.get(1), Some(&WALLPAPER[0]));
        assert_eq!(order.last(), Some(&WALLPAPER[1]));
        assert!(order.windows(2).any(|w| w == [CUT[1], CUT[0]]), "the outgoing subject is the top one: {order:?}");
    }

    /// Widget plane indices for the stacking test.
    const PLANE_A: usize = widgets::PLANE_BASE;
    const PLANE_B: usize = widgets::PLANE_BASE + 1;
}
