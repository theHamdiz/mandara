//! Desktop cards and GPU visualizers, each on its **own small layer
//! surface** on the Bottom layer (`mandara-desktop`), sized to the piece.
//!
//! Resource rules (they are what makes these affordable):
//!
//! * nothing exists while no card is placed (`[[desktop.widgets.items]]`
//!   of kind `card`, see [`mandara_config::desktop_cards`]);
//! * a surface is created only while its card is *visible* and destroyed
//!   when it hides: the now-playing card while a player is active (plus a
//!   grace period after it stops), a visualizer while music plays (plus
//!   the silence window), the calendar whenever it is placed;
//! * surfaces are as small as the card (a full-output GPUI surface costs
//!   ~130 MiB of VRAM at 4K; a 2560 × 140 strip a few MiB) and take input
//!   only on the card itself (visualizers take none);
//! * visualizers sample the spectrum only while audio plays and stop
//!   drawing after the configured silence (see [`visualizer`]).
//!
//! The host reconciles the open surfaces with the config, the outputs and
//! the media state whenever one of them changes (and once more when a
//! grace period ends) — never per frame.

pub mod calendar_card;
pub mod now_playing;
pub mod visualizer;

use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Pixels, Task, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, point, px, size,
};
use mandara_config::ShellConfig;
use mandara_config::desktop_cards::{self as dc, CALENDAR, FRAMED_BARS, MIRRORED_BARS, NOW_PLAYING, WAVE};
use mandara_config::desktop_widgets::{DesktopWidget, Placement};
use mandara_core::SurfaceRole;
use mandara_services::PlaybackStatus;

use crate::bar::layout::{self, Metrics};
use crate::state::config;

/// Layer-shell exclusive zone `-1`: placed against the whole output,
/// ignoring other surfaces' reserved zones.
const IGNORE_ZONES: f32 = -1.0;

/// Transparent room around a card for its soft shadow (outside the input
/// region, so it never catches clicks).
pub const SHADOW_PAD_PX: f32 = 14.0;

/// What a visible card needs from the media state.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaPresence {
    /// A player is loaded (playing or paused).
    pub active: bool,
    pub playing: bool,
}

impl MediaPresence {
    pub fn of(cx: &App) -> Self {
        let hub = crate::hub::hub(cx);
        let m = hub.read(cx).media.as_ref();
        Self { active: m.is_some_and(|m| m.status != PlaybackStatus::Stopped), playing: m.is_some_and(|m| m.status == PlaybackStatus::Playing) }
    }
}

/// When the media state last changed in a way cards care about.
#[derive(Copy, Clone, Debug, Default)]
pub struct MediaClock {
    pub seen: MediaPresence,
    /// No active player since then.
    pub inactive_since: Option<Instant>,
    /// Not playing since then.
    pub quiet_since: Option<Instant>,
}

impl MediaClock {
    /// Follows `now_state` (call on every media change).
    pub fn observe(&mut self, state: MediaPresence, now: Instant) {
        if state.active {
            self.inactive_since = None;
        } else if self.inactive_since.is_none() {
            self.inactive_since = Some(now);
        }
        if state.playing {
            self.quiet_since = None;
        } else if self.quiet_since.is_none() {
            self.quiet_since = Some(now);
        }
        self.seen = state;
    }
}

/// Whether a card is visible now, and when that may next change.
pub fn visible(card: &str, widget: &DesktopWidget, clock: &MediaClock, cfg: &ShellConfig, now: Instant) -> (bool, Option<Instant>) {
    let within = |since: Option<Instant>, keep: Duration| match since {
        None => (true, None),
        Some(t) if now < t + keep => (true, Some(t + keep)),
        Some(_) => (false, None),
    };
    match card {
        NOW_PLAYING => {
            let opts: dc::NowPlayingOptions = card_opts(widget);
            // Never shown for a player that was never active.
            if !clock.seen.active && clock.inactive_since.is_none() {
                return (false, None);
            }
            within(clock.inactive_since, opts.grace())
        }
        WAVE | FRAMED_BARS | MIRRORED_BARS => {
            if !clock.seen.playing && clock.quiet_since.is_none() {
                return (false, None);
            }
            within(clock.quiet_since, cfg.visualizers.silence())
        }
        _ => (true, None),
    }
}

fn card_opts<T: serde::de::DeserializeOwned + Default>(w: &DesktopWidget) -> T {
    dc::as_card(w).map(dc::options).unwrap_or_default()
}

/// A card's own size (logical px, without the shadow pad) on an `output`.
pub fn card_size(card: &str, widget: &DesktopWidget, output: (f32, f32)) -> (f32, f32) {
    let k = widget.place.scale();
    match card {
        NOW_PLAYING => now_playing::card_size(&card_opts(widget), k),
        CALENDAR => calendar_card::card_size(&card_opts(widget), k),
        WAVE => (output.0, card_opts::<dc::WaveOptions>(widget).height() * k),
        MIRRORED_BARS => (output.0, card_opts::<dc::MirroredBarsOptions>(widget).height() * k),
        FRAMED_BARS => {
            let (w, h) = card_opts::<dc::FramedBarsOptions>(widget).size();
            (w * k, h * k)
        }
        _ => (0.0, 0.0),
    }
}

/// Strips run edge to edge and carry no shadow.
fn is_strip(card: &str) -> bool {
    matches!(card, WAVE | MIRRORED_BARS)
}

/// The output area a card may use: the output less the bar's reserved
/// band (and a side bar's screen frame) when the bar shows there.
pub fn usable_area(cfg: &ShellConfig, monitor: &str, output: (f32, f32)) -> Bounds<Pixels> {
    let full = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(output.0), px(output.1)) };
    if !cfg.bar_on_output(monitor) || !crate::bar::autohide::reserves_space(cfg, monitor) {
        return full;
    }
    let m = Metrics::of(cfg, crate::style::radius_scale());
    let band = layout::reserved_rect(&m, output);
    let (bx, by, bw, bh) = (f32::from(band.origin.x), f32::from(band.origin.y), f32::from(band.size.width), f32::from(band.size.height));
    let t = m.screen_frame;
    let (mut x0, mut y0, mut x1, mut y1) = (0.0, 0.0, output.0, output.1);
    match cfg.bar.position {
        mandara_config::BarPosition::Top => y0 = by + bh,
        mandara_config::BarPosition::Bottom => y1 = by,
        mandara_config::BarPosition::Left => {
            x0 = bx + bw;
            (y0, y1, x1) = (t, output.1 - t, output.0 - t);
        }
        mandara_config::BarPosition::Right => {
            x1 = bx;
            (y0, y1, x0) = (t, output.1 - t, t);
        }
    }
    Bounds { origin: point(px(x0), px(y0)), size: size(px((x1 - x0).max(0.0)), px((y1 - y0).max(0.0))) }
}

/// The surface rect of a card on an output (logical px, output-relative),
/// including its shadow pad.
pub fn surface_rect(card: &str, place: &Placement, area: Bounds<Pixels>, card_size: (f32, f32), rtl: bool) -> Bounds<Pixels> {
    let (ax, ay, aw, ah) = (f32::from(area.origin.x), f32::from(area.origin.y), f32::from(area.size.width), f32::from(area.size.height));
    if is_strip(card) {
        let h = card_size.1.min(ah);
        let y = if dc::strip_on_top(place.anchor) { ay } else { ay + ah - h };
        return Bounds { origin: point(px(ax), px(y)), size: size(px(aw), px(h)) };
    }
    let (x, y) = dc::card_origin(place, (aw, ah), card_size, rtl);
    let pad = SHADOW_PAD_PX;
    Bounds { origin: point(px(ax + x - pad), px(ay + y - pad)), size: size(px(card_size.0 + pad * 2.0), px(card_size.1 + pad * 2.0)) }
}

/// Layer-surface options of a card surface at `rect` on its output.
fn options(rect: Bounds<Pixels>, display: Option<gpui::DisplayId>) -> WindowOptions {
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: rect.size })),
        focus: false,
        show: true,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: SurfaceRole::Desktop.namespace().to_string(),
            layer: Layer::Bottom,
            anchor: Anchor::TOP | Anchor::LEFT,
            // Placed against the whole output (the usable area is already
            // taken into account by the rect).
            exclusive_zone: Some(px(IGNORE_ZONES)),
            exclusive_edge: None,
            margin: Some((rect.origin.y, px(0.0), px(0.0), rect.origin.x)),
            keyboard_interactivity: KeyboardInteractivity::None,
        }),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(mandara_core::SHELL_APP_ID.to_string()),
        // Cards pace themselves (timers / their own frame cap).
        inactive_frame_interval: None,
        ..Default::default()
    }
}

/// One open card surface.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    widget: String,
    card: String,
    output: String,
    /// Surface rect, rounded to whole px (a change re-creates the surface:
    /// layer surfaces can't move).
    rect: (i32, i32, i32, i32),
}

/// A card window's root: hosts the card view as a child so the card renders
/// inside layout like every other view (GPUI's view-local helpers —
/// redraw timers, image slots — need a view on the render stack).
pub struct Root(gpui::AnyView);

impl gpui::Render for Root {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        gpui::div().size_full().child(self.0.clone())
    }
}

enum Handle {
    NowPlaying(WindowHandle<Root>, gpui::Entity<now_playing::NowPlayingView>),
    Other(WindowHandle<Root>),
}

impl Handle {
    fn any(&self) -> AnyWindowHandle {
        match self {
            Handle::NowPlaying(h, _) | Handle::Other(h) => (*h).into(),
        }
    }

    /// Closes the surface (the now-playing card fades out first).
    fn close(&self, cx: &mut App) {
        match self {
            Handle::NowPlaying(h, view) => {
                let view = view.clone();
                let _ = h.update(cx, move |_, window, cx| view.update(cx, |v, cx| v.leave(window, cx)));
            }
            Handle::Other(h) => {
                let _ = h.update(cx, |_, window, _| window.remove_window());
            }
        }
    }
}

#[derive(Default)]
struct Host {
    open: Vec<(Key, Handle)>,
    clock: MediaClock,
    /// Re-check once the earliest grace period ends.
    timer: Option<(Instant, Task<()>)>,
}

impl Global for Host {}

fn alive(h: AnyWindowHandle, cx: &App) -> bool {
    cx.windows().iter().any(|w| w.window_id() == h.window_id())
}

fn outputs(cx: &App) -> Vec<mandara_core::OutputGeometry> {
    cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.clone()).filter(|o| !o.is_empty()).unwrap_or_else(|| mandara_core::query_outputs().unwrap_or_default())
}

/// Number of card surfaces open right now (tests, diagnostics).
pub fn open_count(cx: &App) -> usize {
    cx.try_global::<Host>().map_or(0, |h| h.open.len())
}

/// Every wanted surface `(key, output index)` and the next time
/// visibility may change.
fn plan(cfg: &ShellConfig, outs: &[mandara_core::OutputGeometry], clock: &MediaClock, rtl: bool, now: Instant) -> (Vec<(Key, usize)>, Option<Instant>) {
    let mut want = Vec::new();
    let mut next: Option<Instant> = None;
    for (w, c) in dc::enabled_cards(&cfg.desktop.widgets) {
        let (shown, until) = visible(&c.card, w, clock, cfg, now);
        if let Some(t) = until {
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        if !shown {
            continue;
        }
        for (i, o) in outs.iter().enumerate() {
            if !w.place.shows_on(&[o.name.as_str()]) {
                continue;
            }
            let output = (o.width as f32, o.height as f32);
            let area = usable_area(cfg, &o.name, output);
            let card = card_size(&c.card, w, (f32::from(area.size.width), output.1));
            if card.0 < 1.0 || card.1 < 1.0 {
                continue;
            }
            let r = surface_rect(&c.card, &w.place, area, card, rtl);
            let rect = (f32::from(r.origin.x).round() as i32, f32::from(r.origin.y).round() as i32, f32::from(r.size.width).round() as i32, f32::from(r.size.height).round() as i32);
            want.push((Key { widget: w.id.clone(), card: c.card.clone(), output: o.name.clone(), rect }, i));
        }
    }
    (want, next)
}

/// Re-plans the card surfaces now (desktop edit mode started or ended:
/// hidden cards show while editing).
pub(crate) fn refresh(cx: &mut App) {
    if cx.has_global::<Host>() {
        reconcile(cx);
    }
}

fn reconcile(cx: &mut App) {
    let mut cfg = config(cx).read(cx).snapshot();
    if cfg.desktop.widgets.hidden && !crate::desktop_edit::active(cx) {
        std::sync::Arc::make_mut(&mut cfg).desktop.widgets.items.retain(|w| dc::as_card(w).is_none());
    }
    let now = Instant::now();
    let presence = MediaPresence::of(cx);
    let mut host = cx.remove_global::<Host>();
    host.clock.observe(presence, now);
    let outs = outputs(cx);
    let (want, next) = plan(&cfg, &outs, &host.clock, crate::bidi::rtl(), now);
    // Close what is no longer wanted (or whose window went away).
    let mut keep = Vec::new();
    for (key, handle) in std::mem::take(&mut host.open) {
        if !alive(handle.any(), cx) {
            continue;
        }
        if want.iter().any(|(k, _)| *k == key) {
            keep.push((key, handle));
        } else {
            tracing::info!("desktop card {} on {}: hidden", key.card, key.output);
            handle.close(cx);
        }
    }
    host.open = keep;
    for (key, _) in want {
        if host.open.iter().any(|(k, _)| *k == key) {
            continue;
        }
        let rect = Bounds { origin: point(px(key.rect.0 as f32), px(key.rect.1 as f32)), size: size(px(key.rect.2 as f32), px(key.rect.3 as f32)) };
        let display = crate::outputs::display_for(&key.output, cx);
        let opts = options(rect, display);
        let widget = key.widget.clone();
        let opened = match key.card.as_str() {
            NOW_PLAYING => {
                let mut inner = None;
                let slot = &mut inner;
                cx.open_window(opts, move |window, cx| {
                    let view = cx.new(|cx| now_playing::NowPlayingView::new(widget, window, cx));
                    *slot = Some(view.clone());
                    cx.new(|_| Root(view.into()))
                })
                .and_then(|h| inner.map(|v| Handle::NowPlaying(h, v)).ok_or_else(|| anyhow::anyhow!("no card view")))
            }
            CALENDAR => cx.open_window(opts, move |window, cx| {
                let view = cx.new(|cx| calendar_card::CalendarView::new(widget, window, cx));
                cx.new(|_| Root(view.into()))
            })
            .map(Handle::Other),
            card => {
                let kind = visualizer::Kind::of(card);
                cx.open_window(opts, move |window, cx| {
                    let view = cx.new(|cx| visualizer::VisualizerView::new(widget, kind, window, cx));
                    cx.new(|_| Root(view.into()))
                })
                .map(Handle::Other)
            }
        };
        match opened {
            Ok(h) => {
                tracing::info!("desktop card {} on {}: {}×{} surface", key.card, key.output, key.rect.2, key.rect.3);
                host.open.push((key, h));
            }
            Err(err) => tracing::error!("desktop card surface: {err:#}"),
        }
    }
    // Wake up when the earliest grace period ends.
    host.timer = next.filter(|t| *t > now).map(|t| {
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(t.saturating_duration_since(Instant::now()) + GRACE_SLACK).await;
            cx.update(reconcile);
        });
        (t, task)
    });
    cx.set_global(host);
}

/// Added to grace deadlines so the re-check lands after them.
const GRACE_SLACK: Duration = Duration::from_millis(30);

/// What of the config a card host re-plans on (cheap to compare).
fn config_key(cfg: &ShellConfig) -> (mandara_config::desktop_widgets::DesktopWidgetsConfig, mandara_config::desktop_cards::VisualizersConfig, mandara_config::BarConfig, mandara_config::outputs::DisplaysConfig, mandara_config::Locale) {
    (cfg.desktop.widgets.clone(), cfg.visualizers.clone(), cfg.bar.clone(), cfg.displays.clone(), cfg.general.locale)
}

/// Starts the host: it stays idle (no surfaces, no timers) until a card is
/// placed.
pub fn init(cx: &mut App) {
    cx.set_global(Host::default());
    reconcile(cx);
    let mut last = config_key(&config(cx).read(cx).snapshot());
    cx.observe(&config(cx), move |store, cx| {
        let next = config_key(&store.read(cx).snapshot());
        if next != last {
            last = next;
            reconcile(cx);
        }
    })
    .detach();
    let hub = crate::hub::hub(cx);
    cx.observe(&hub, |_, cx| {
        let presence = MediaPresence::of(cx);
        if cx.try_global::<Host>().is_none_or(|h| h.clock.seen == presence) {
            return;
        }
        if dc::enabled_cards(&config(cx).read(cx).get().desktop.widgets).next().is_some() {
            reconcile(cx);
        } else if cx.has_global::<Host>() {
            // Keep the clock in step while no card is placed.
            cx.global_mut::<Host>().clock.observe(presence, Instant::now());
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop_widgets::Anchor;

    fn cfg_with(cards: &[(&str, Anchor)]) -> ShellConfig {
        let mut cfg = ShellConfig::default();
        for (c, a) in cards {
            dc::set_card(&mut cfg.desktop.widgets, c, Some(*a));
        }
        cfg
    }

    fn out(name: &str, w: i32, h: i32) -> mandara_core::OutputGeometry {
        mandara_core::OutputGeometry { name: name.into(), x: 0, y: 0, width: w, height: h }
    }

    #[test]
    fn no_cards_no_surfaces() {
        let (want, next) = plan(&ShellConfig::default(), &[out("DP-1", 2560, 1440)], &MediaClock::default(), false, Instant::now());
        assert!(want.is_empty() && next.is_none());
    }

    #[test]
    fn media_cards_exist_only_while_media_is_active_then_for_the_grace() {
        let cfg = cfg_with(&[(NOW_PLAYING, Anchor::BottomLeft), (WAVE, Anchor::Bottom), (CALENDAR, Anchor::TopRight)]);
        let outs = [out("DP-1", 2560, 1440)];
        let t0 = Instant::now();
        let mut clock = MediaClock::default();
        clock.observe(MediaPresence::default(), t0);
        // Nothing has played yet: only the calendar.
        let (want, _) = plan(&cfg, &outs, &MediaClock::default(), false, t0);
        assert_eq!(want.iter().map(|(k, _)| k.card.as_str()).collect::<Vec<_>>(), [CALENDAR]);
        // Playing: all three.
        clock.observe(MediaPresence { active: true, playing: true }, t0);
        let (want, _) = plan(&cfg, &outs, &clock, false, t0);
        assert_eq!(want.len(), 3);
        // Stopped: both media cards linger, the visualizer for the silence
        // window, the card for its grace period; then both are gone.
        let t1 = t0 + Duration::from_secs(1);
        clock.observe(MediaPresence::default(), t1);
        let (want, next) = plan(&cfg, &outs, &clock, false, t1);
        assert_eq!(want.len(), 3);
        assert_eq!(next, Some(t1 + cfg.visualizers.silence()));
        let (want, _) = plan(&cfg, &outs, &clock, false, t1 + cfg.visualizers.silence() + Duration::from_millis(1));
        assert_eq!(want.iter().map(|(k, _)| k.card.as_str()).collect::<Vec<_>>(), [NOW_PLAYING, CALENDAR]);
        let grace = dc::NowPlayingOptions::default().grace();
        let (want, _) = plan(&cfg, &outs, &clock, false, t1 + grace + Duration::from_millis(1));
        assert_eq!(want.iter().map(|(k, _)| k.card.as_str()).collect::<Vec<_>>(), [CALENDAR]);
    }

    #[test]
    fn surfaces_are_sized_to_the_card_never_the_output() {
        let cfg = cfg_with(&[(NOW_PLAYING, Anchor::BottomLeft), (FRAMED_BARS, Anchor::BottomRight), (WAVE, Anchor::Bottom), (CALENDAR, Anchor::TopRight)]);
        let mut clock = MediaClock::default();
        clock.observe(MediaPresence { active: true, playing: true }, Instant::now());
        let (want, _) = plan(&cfg, &[out("DP-1", 2560, 1440)], &clock, false, Instant::now());
        for (k, _) in &want {
            let (w, h) = (k.rect.2, k.rect.3);
            assert!(w as i64 * h as i64 <= 2560 * 1440 / 8, "{} is {w}×{h}", k.card);
            assert!(k.rect.0 >= -(SHADOW_PAD_PX as i32) && k.rect.1 >= 0, "{k:?}");
        }
        let wave = want.iter().find(|(k, _)| k.card == WAVE).unwrap();
        assert_eq!(wave.0.rect.2, 2560, "the wave spans the output");
        assert_eq!(wave.0.rect.1 + wave.0.rect.3, 1440, "on the bottom edge");
        // Below a top bar: the calendar keeps clear of it.
        let cal = want.iter().find(|(k, _)| k.card == CALENDAR).unwrap();
        let zone = layout::exclusive_zone(&Metrics::of(&cfg, 1.0));
        assert!(cal.0.rect.1 as f32 + SHADOW_PAD_PX >= zone, "{:?}", cal.0.rect);
    }

    #[test]
    fn side_bar_keeps_cards_beside_it() {
        let mut cfg = cfg_with(&[(CALENDAR, Anchor::TopLeft)]);
        cfg.bar.position = mandara_config::BarPosition::Left;
        let area = usable_area(&cfg, "DP-1", (2560.0, 1440.0));
        assert_eq!(f32::from(area.origin.x), cfg.bar.vertical_width());
        let (want, _) = plan(&cfg, &[out("DP-1", 2560, 1440)], &MediaClock::default(), false, Instant::now());
        assert!(want[0].0.rect.0 as f32 + SHADOW_PAD_PX > cfg.bar.vertical_width());
    }
}
