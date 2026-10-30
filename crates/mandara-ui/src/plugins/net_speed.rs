//! Network speed plugin: live download / upload rates in the bar; a panel
//! with springy rate tiles and sparklines, a mirrored 60 s / 10 min graph,
//! per-interface breakdown with local addresses, session totals, the public
//! address (looked up when the panel opens) and an on-demand Cloudflare
//! download speed test with an animated gauge.
//!
//! Counters are sampled once a second off the UI thread, only while the
//! bar chip or the panel is on screen: one sampler task runs while a view
//! rendered recently and stops (dropping its handle) once none has. Every
//! derived label is formatted once per sample, not per frame. Settings
//! (`[plugins.settings.net-speed]`): `unit` (`bytes` / `bits`),
//! `public_ip_url`, `speed_test_mb`, `bar_upload`.

use std::cell::{Cell, RefCell};
use std::net::IpAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Task, Window,
    div, prelude::FluentBuilder as _, px, size,
};
use mandara_config::ShellConfig;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::net_speed::{self as ns, IfaceRate, LinkKind, LocalAddr, RateHistory, Sample, SpeedProgress, SpeedResult, Unit};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{Flash, Segment, caption, segmented, toast};
use super::fn_view;
use super::panel_fx::{
    Cached, HEADER_H, History, PanelShape, SECTION_H_PX, SMALL_ICON_PX, ViewSet, WASH_FAINT, chip, glass_card, glow, header, partner, pill_button,
    readable, redraw_in, section, sparkline, wash,
};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "net-speed";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 404.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const TILE_H: f32 = 118.0;
const GRAPH_H: f32 = 150.0;
const CHART_H: f32 = 84.0;
const IFACE_H: f32 = 38.0;
const TOTALS_H: f32 = 44.0;
/// The totals card: its content row plus the card's vertical padding.
const TOTALS_CARD_H: f32 = TOTALS_H + tokens::MD;
const TEST_H: f32 = 124.0;
/// Inner padding of the glass cards.
const CARD_PAD_PX: f32 = tokens::MD;
/// Gap between a section label and its content.
const SECTION_GAP_PX: f32 = tokens::XS;
/// Gap inside a card's label row (icon · text).
const ROW_GAP_PX: f32 = 6.0;
/// Gap between the two rate tiles / the totals cells.
const TILE_GAP_PX: f32 = 10.0;
/// Glyph size of tile and interface icons.
const ICON_PX: f32 = 14.0;
/// Rate arrow glyph next to interface / totals figures.
const RATE_ICON_PX: f32 = 11.0;
/// Gap between a rate arrow and its figure.
const RATE_ICON_GAP_PX: f32 = 3.0;
/// Tile figure: size, line height and the unit's baseline nudge.
const TILE_VALUE_PX: f32 = 28.0;
const TILE_VALUE_LINE_PX: f32 = 32.0;
const TILE_UNIT_PAD_PX: f32 = tokens::XS;
/// Tile sparkline height and the alpha of its low end.
const TILE_SPARK_H_PX: f32 = 30.0;
const TILE_SPARK_FROM_ALPHA: f32 = 0.7;

/// Mirrored chart: inner padding, corner radius, column gap / radius, the
/// gap at the midline, and the fade of its oldest columns.
const CHART_PAD_PX: f32 = 3.0;
const CHART_RADIUS_PX: f32 = 10.0;
const CHART_COL_GAP_PX: f32 = 2.0;
const CHART_COL_RADIUS_PX: f32 = 1.5;
const CHART_MIDLINE_PX: f32 = tokens::XS;
const CHART_AGE_FADE: f32 = 0.55;
/// Height of an idle (≈ 0) chart column and its tick alpha.
const IDLE_BAR: f32 = 1.0;
const IDLE_TICK_ALPHA: f32 = 0.12;
/// Graph legend: dot size and gap; width of the 60 s / 10 min toggle.
const LEGEND_DOT_PX: f32 = 7.0;
const LEGEND_GAP_PX: f32 = 5.0;
const WINDOW_TOGGLE_W_PX: f32 = 116.0;

/// Interface rows: badge side / radius / alpha, the gap after it, and the
/// fixed width of each rate column.
const IFACE_BADGE_PX: f32 = 26.0;
const IFACE_BADGE_RADIUS_PX: f32 = 9.0;
const IFACE_BADGE_ALPHA: f32 = 0.14;
const IFACE_GAP_PX: f32 = 9.0;
const IFACE_RATE_W_PX: f32 = 84.0;
/// Totals: divider size and alpha.
const DIVIDER_W_PX: f32 = 1.0;
const DIVIDER_H_PX: f32 = 26.0;
const DIVIDER_ALPHA: f32 = 0.08;

/// Gauge: diameter, dot count / size, dot inset (× dot size) from the rim,
/// head glow (alpha, blur), track alpha `(dark, light)`.
const GAUGE_D: f32 = 96.0;
const GAUGE_DOTS: usize = 34;
const GAUGE_DOT_PX: f32 = 4.0;
const GAUGE_DOT_INSET: f32 = 1.4;
const GAUGE_HEAD_GLOW: (f32, f32) = (0.6, 9.0);
const GAUGE_TRACK_ALPHA: (f32, f32) = (0.12, 0.10);
/// The gauge spans 270° (from 7:30 to 4:30 on a clock face).
const GAUGE_SWEEP: f32 = 0.75;
/// Gauge scale: log from 1 Mb/s to 1 Gb/s (bytes per second).
const GAUGE_MIN_BPS: f64 = 125_000.0;
const GAUGE_MAX_BPS: f64 = 125_000_000.0;
/// Below the minimum the gauge creeps linearly up to this fraction.
const GAUGE_FLOOR_FRAC: f64 = 0.04;
/// Gauge figure size / line height and its unit size.
const GAUGE_VALUE_PX: f32 = 20.0;
const GAUGE_VALUE_LINE_PX: f32 = 22.0;
const GAUGE_UNIT_PX: f32 = 10.0;
/// Gap between the gauge and the test text, and above the button.
const TEST_GAP_PX: f32 = 14.0;
const TEST_BUTTON_GAP_PX: f32 = 6.0;
/// Bar chip: gap between the two readouts, and the fixed width of a rate
/// readout (no jitter as digits change).
const BAR_GAP_PX: f32 = 6.0;
const BAR_VALUE_W: f32 = 62.0;
/// Panel size changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;

// ── Timing & units ───────────────────────────────────────────────────────

/// Counter sampling cadence in seconds (`interval_s`).
pub const DEFAULT_INTERVAL_S: i64 = 1;
pub const INTERVAL_S_RANGE: (i64, i64) = (1, 10);
/// The sampler stops once no view has rendered for this long (plus one
/// sampling interval).
const SAMPLER_IDLE: Duration = Duration::from_millis(2500);
/// Local addresses refresh at most this often (while the panel shows).
const ADDR_EVERY: Duration = Duration::from_secs(10);
/// The panel counts as on screen for this long after its last render
/// (plus one sampling interval).
const PANEL_SEEN: Duration = Duration::from_millis(1500);
/// The public address is looked up again after this long.
const PUBLIC_IP_TTL: Duration = Duration::from_secs(600);
/// The panel tiles' spring.
const TILE_SPRING: SpringConfig = SpringConfig::new(120.0, 20.0, 1.0);
/// A tile spring rests within this fraction of its target (below the
/// three significant digits a rate is shown with) …
const TILE_SETTLE_REL: f32 = 0.002;
/// … or within this many bytes per second of it.
const TILE_SETTLE_BPS: f32 = 1.0;
/// The speed-test gauge spring.
const GAUGE_SPRING: SpringConfig = SpringConfig::new(140.0, 18.0, 1.0);
/// Interfaces listed at most.
const MAX_IFACES: usize = 5;
/// Speed test redraw cadence while running (panel visible only).
const TEST_FRAME: Duration = Duration::from_millis(50);
/// Window of the two samples `net-speed status` takes when nothing is live.
const PROBE: Duration = Duration::from_millis(300);
/// Bytes in a (decimal) megabyte, bits in a byte and in a megabit.
const BYTES_PER_MB: u64 = 1_000_000;
const BITS_PER_BYTE: f64 = 8.0;
const BITS_PER_MBIT: f64 = 1e6;
/// Fraction → percent.
const PERCENT: f32 = 100.0;

// ── Settings ─────────────────────────────────────────────────────────────

fn unit(cfg: &ShellConfig) -> Unit {
    Unit::parse(&cfg.plugins.setting(ID, "unit", String::from("bytes")))
}

fn endpoint(cfg: &ShellConfig) -> String {
    let url = cfg.plugins.setting(ID, "public_ip_url", String::from(ns::DEFAULT_PUBLIC_IP_URL));
    if url.trim().is_empty() { ns::DEFAULT_PUBLIC_IP_URL.into() } else { url.trim().to_string() }
}

fn test_megabytes(cfg: &ShellConfig) -> u64 {
    let mb = cfg.plugins.setting(ID, "speed_test_mb", i64::try_from(ns::DEFAULT_TEST_MEGABYTES).unwrap_or(i64::MAX));
    u64::try_from(mb).unwrap_or(1).clamp(1, ns::MAX_TEST_MEGABYTES)
}

// ── Live state (one per shell process, shared by every view) ─────────────

#[derive(Clone, Debug, PartialEq)]
enum PublicIp {
    Idle,
    Loading,
    Done(IpAddr, Instant),
    Failed(String, Instant),
}

#[derive(Clone)]
enum SpeedTest {
    Idle,
    Running(Arc<SpeedProgress>),
    Done(SpeedResult),
    Failed(String),
}

/// What the speed-test texts were formatted from.
#[derive(Clone, Debug, Default, PartialEq)]
enum TestKey {
    #[default]
    Idle,
    Running(u64, u64),
    Done(u64, u64),
    Failed,
}

/// Labels and series derived from one sample, rebuilt only when a sample,
/// the addresses, the graph window or the unit change.
struct Derived {
    key: (u64, Unit),
    ifaces: Vec<IfaceRate>,
    iface_text: Vec<IfaceText>,
    virtuals: Option<SharedString>,
    subtitle: SharedString,
    peak_rx: SharedString,
    peak_tx: SharedString,
    down_series: History,
    up_series: History,
    samples: Vec<(f64, f64)>,
    capacity: usize,
    gpeak: f64,
    gpeak_text: SharedString,
    session: (SharedString, SharedString),
    boot: (SharedString, SharedString),
}

struct IfaceText {
    addr: SharedString,
    rx: SharedString,
    tx: SharedString,
}

struct Live {
    /// The sampling interval in use (`interval_s`).
    every: Duration,
    last: Option<Sample>,
    /// Interfaces listed by the latest counters (sizes the panel before
    /// the first rates exist).
    iface_hint: usize,
    rates: Vec<IfaceRate>,
    total: (f64, f64),
    /// Rate history for the panel's graphs: kept only while the panel is
    /// on screen (the bar needs just the current rates).
    history: Option<RateHistory>,
    /// Bytes moved while sampled (rx, tx).
    session: (f64, f64),
    addresses: Vec<LocalAddr>,
    addresses_at: Option<Instant>,
    addresses_loading: bool,
    public_ip: PublicIp,
    test: SpeedTest,
    long_window: bool,
    /// Bumped whenever the data behind [`Derived`] changes.
    generation: u64,
    derived: Option<Derived>,
    test_status: Cached<TestKey>,
    test_detail: Cached<TestKey>,
    /// Displayed tile values spring toward the samples.
    shown: (SpringValue<f32>, SpringValue<f32>, FrameClock),
    gauge: (SpringValue<f32>, FrameClock),
    /// The 1 Hz sampler (dropped = cancelled) and the last view render.
    sampler: Option<Task<()>>,
    seen: Option<Instant>,
    panel_seen: Option<Instant>,
    views: ViewSet,
}

/// Counter sampling cadence (`interval_s`).
fn sample_every(cx: &App) -> Duration {
    let s: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "interval_s", DEFAULT_INTERVAL_S);
    Duration::from_secs(s.clamp(INTERVAL_S_RANGE.0, INTERVAL_S_RANGE.1) as u64)
}

impl Live {
    fn new() -> Self {
        Self {
            every: Duration::from_secs(DEFAULT_INTERVAL_S as u64),
            last: None,
            iface_hint: 0,
            rates: Vec::new(),
            total: (0.0, 0.0),
            history: None,
            session: (0.0, 0.0),
            addresses: Vec::new(),
            addresses_at: None,
            addresses_loading: false,
            public_ip: PublicIp::Idle,
            test: SpeedTest::Idle,
            long_window: false,
            generation: 0,
            derived: None,
            test_status: Cached::default(),
            test_detail: Cached::default(),
            shown: (SpringValue::new(0.0, TILE_SPRING), SpringValue::new(0.0, TILE_SPRING), FrameClock::new()),
            gauge: (SpringValue::new(0.0, GAUGE_SPRING), FrameClock::new()),
            sampler: None,
            seen: None,
            panel_seen: None,
            views: ViewSet::default(),
        }
    }

    /// Frees what only the panel shows — graph history, derived labels,
    /// local addresses and the speed-test texts — once it has closed.
    fn release_panel(&mut self) {
        if self.history.is_none() && self.derived.is_none() && self.addresses.capacity() == 0 {
            return;
        }
        self.history = None;
        self.derived = None;
        self.addresses = Vec::new();
        self.addresses_at = None;
        self.test_status = Cached::default();
        self.test_detail = Cached::default();
        self.generation += 1;
    }

    /// Frees the counter snapshot and rates once no view is on screen.
    fn release_all(&mut self) {
        self.release_panel();
        self.last = None;
        self.rates = Vec::new();
    }

    /// Folds a counter sample taken off the UI thread into the rates;
    /// history accumulates only while the panel shows.
    fn apply(&mut self, next: Sample, panel: bool) {
        self.iface_hint = iface_hint(&next);
        if let Some(prev) = &self.last {
            self.rates = ns::rates(prev, &next, LinkKind::classify);
            self.total = ns::total(&self.rates);
            let dt = next.at.saturating_duration_since(prev.at).as_secs_f64();
            // Only count continuous sampling toward the session.
            if dt < ns::SHORT_GAP.as_secs_f64() {
                self.session.0 += self.total.0 * dt;
                self.session.1 += self.total.1 * dt;
            }
            if panel {
                self.history.get_or_insert_with(RateHistory::default).push(self.total.0, self.total.1, next.at);
            }
        }
        self.last = Some(next);
        self.generation += 1;
    }
}

/// Interfaces the panel will list for a counter snapshot.
fn iface_hint(sample: &Sample) -> usize {
    sample.counters.iter().filter(|c| LinkKind::classify(&c.name) != LinkKind::Virtual).count().min(MAX_IFACES)
}

thread_local! {
    static LIVE: Rc<RefCell<Live>> = Rc::new(RefCell::new(Live::new()));
}

fn live() -> Rc<RefCell<Live>> {
    LIVE.with(|l| l.clone())
}

/// Marks a view as on screen and keeps the 1 Hz sampler running while any
/// is. The sampler reads `/proc/net/dev` (and, while the panel shows, the
/// local addresses) on the background executor, applies the result and
/// notifies just this plugin's views; it ends — and its handle drops —
/// once no view has rendered for [`SAMPLER_IDLE`].
fn keep_sampling(l: &Rc<RefCell<Live>>, window: &Window, cx: &mut App) {
    let mut lb = l.borrow_mut();
    lb.views.track(window);
    lb.seen = Some(Instant::now());
    if lb.sampler.is_some() {
        return;
    }
    let live = l.clone();
    lb.sampler = Some(cx.spawn(async move |cx| {
        loop {
            let every = cx.update(|cx| sample_every(cx));
            let (idle, panel, want_addr) = {
                let mut lb = live.borrow_mut();
                lb.every = every;
                let idle = lb.seen.is_none_or(|t| t.elapsed() > SAMPLER_IDLE + every);
                let panel = lb.panel_seen.is_some_and(|t| t.elapsed() < PANEL_SEEN + every);
                (idle, panel, panel && !lb.addresses_loading && lb.addresses_at.is_none_or(|t| t.elapsed() > ADDR_EVERY))
            };
            if idle {
                break;
            }
            if !panel {
                live.borrow_mut().release_panel();
            }
            if want_addr {
                live.borrow_mut().addresses_loading = true;
            }
            let next = cx.background_executor().spawn(async move { (Sample::now(), want_addr.then(ns::local_addresses)) }).await;
            let views = {
                let mut lb = live.borrow_mut();
                lb.apply(next.0, panel);
                if let Some(addresses) = next.1 {
                    lb.addresses = addresses;
                    lb.addresses_at = Some(Instant::now());
                    lb.addresses_loading = false;
                }
                lb.views.clone()
            };
            views.notify_async(cx);
            cx.background_executor().timer(every).await;
        }
        let mut lb = live.borrow_mut();
        lb.release_all();
        lb.sampler = None;
    }));
}

/// Starts the public IP lookup if it's missing or stale.
fn ensure_public_ip(cx: &mut App) {
    let l = live();
    let due = match &l.borrow().public_ip {
        PublicIp::Idle => true,
        PublicIp::Loading => false,
        PublicIp::Done(_, at) | PublicIp::Failed(_, at) => at.elapsed() > PUBLIC_IP_TTL,
    };
    if !due {
        return;
    }
    l.borrow_mut().public_ip = PublicIp::Loading;
    let url = endpoint(&crate::state::config(cx).read(cx).snapshot());
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-public-ip".into()).spawn(move || {
        let _ = tx.send_blocking(ns::public_ip(&url));
    });
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("lookup failed").into()));
        let views = {
            let mut lb = l.borrow_mut();
            lb.public_ip = match result {
                Ok(ip) => PublicIp::Done(ip, Instant::now()),
                Err(e) => PublicIp::Failed(e, Instant::now()),
            };
            lb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

fn start_test(cx: &mut App) {
    let l = live();
    if matches!(l.borrow().test, SpeedTest::Running(_)) {
        return;
    }
    let mb = test_megabytes(&crate::state::config(cx).read(cx).snapshot());
    let progress = Arc::new(SpeedProgress::default());
    l.borrow_mut().test = SpeedTest::Running(progress.clone());
    let (tx, rx) = async_channel::bounded(1);
    let p = progress.clone();
    let _ = std::thread::Builder::new().name("mandara-speed-test".into()).spawn(move || {
        let _ = tx.send_blocking(ns::speed_test(mb, p));
    });
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The speed test stopped").into()));
        let views = {
            let mut lb = l.borrow_mut();
            lb.test = match result {
                Ok(r) => SpeedTest::Done(r),
                Err(e) => SpeedTest::Failed(e),
            };
            lb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

fn cancel_test() {
    if let SpeedTest::Running(p) = &live().borrow().test {
        p.cancel.store(true, Ordering::Relaxed);
    }
}

/// Re-renders this plugin's views (click handlers).
fn notify_views(cx: &mut App) {
    let views = live().borrow().views.clone();
    views.notify(cx);
}

// ── Views ────────────────────────────────────────────────────────────────

fn colors(palette: &MandaraPalette) -> ((Color, Color), (Color, Color)) {
    let pair = |c: Color| (readable(c, palette), readable(partner(c), palette));
    (pair(palette.primary), pair(palette.tertiary))
}

fn rate_text(bps: f64, unit: Unit) -> String {
    let (v, u) = ns::format_rate(bps, unit);
    format!("{v} {u}")
}

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let l = live();
    keep_sampling(&l, window, cx);
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let unit = unit(&cfg);
    let show_up = cfg.plugins.setting(ID, "bar_upload", true);
    let ((down, _), (up, _)) = colors(palette);
    let (rx, tx) = l.borrow().total;
    let value = |icon_name: &'static str, tint: Color, bps: f64| {
        div()
            .flex()
            .items_center()
            .gap(px(RATE_ICON_GAP_PX))
            .child(icon(icon_name, SMALL_ICON_PX, tint))
            .child(div().min_w(px(BAR_VALUE_W)).text_color(palette.on_surface.hsla()).child(rate_text(bps, unit)))
    };
    div()
        .flex()
        .items_center()
        .gap(px(BAR_GAP_PX))
        .font_features(tabular_nums())
        .child(value("arrow-down", down, rx))
        .when(show_up, |el| el.child(value("arrow-up", up, tx)))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn tile(
    icon_name: &'static str,
    label: &'static str,
    shown: f32,
    peak: SharedString,
    series: &History,
    (from, to): (Color, Color),
    unit: Unit,
    palette: &MandaraPalette,
    display: SharedString,
    radius: f32,
) -> AnyElement {
    let (v, u) = ns::format_rate(f64::from(shown), unit);
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(TILE_H))
        .p(px(CARD_PAD_PX))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(ROW_GAP_PX))
                .child(icon(icon_name, ICON_PX, from))
                .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(label))
                .child(caption(peak, palette).font_features(tabular_nums())),
        )
        .child(
            div()
                .mt(px(tokens::XS))
                .flex()
                .items_end()
                .gap(px(tokens::XS))
                .font_family(display)
                .font_features(tabular_nums())
                .child(div().text_size(px(TILE_VALUE_PX)).line_height(px(TILE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(v))
                .child(div().pb(px(TILE_UNIT_PAD_PX)).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(u)),
        )
        .child(div().flex_1())
        .child(sparkline(series, TILE_SPARK_H_PX, from.with_alpha(TILE_SPARK_FROM_ALPHA), to, palette))
        .into_any_element()
}

fn normalized(values: impl Iterator<Item = f64>, peak: f64, capacity: usize) -> History {
    let mut h = History::new(capacity);
    let now = Instant::now();
    for v in values {
        h.push_every(if peak > 0.0 { (v / peak) as f32 } else { 0.0 }, Duration::ZERO, now);
    }
    h
}

/// Mirrored columns: download grows up from the midline, upload down.
fn mirrored_chart(samples: &[(f64, f64)], capacity: usize, peak: f64, palette: &MandaraPalette) -> AnyElement {
    let ((down, down2), (up, up2)) = colors(palette);
    let half = (CHART_H - CHART_MIDLINE_PX) / 2.0;
    let idle_tick = palette.on_surface.with_alpha(IDLE_TICK_ALPHA);
    let empty = capacity.saturating_sub(samples.len());
    let column = |sample: Option<(f64, f64)>, age: f32| {
        let col = div().flex_1().min_w(px(1.0)).h_full().flex().flex_col();
        match sample {
            Some((rx, tx)) => {
                let (a, b) = if peak > 0.0 { ((rx / peak) as f32, (tx / peak) as f32) } else { (0.0, 0.0) };
                let fade = 1.0 - age * CHART_AGE_FADE;
                // Idle seconds leave a faint baseline tick, not a bar.
                let bar = |v: f32, from: Color, to: Color| {
                    let idle = v * half < IDLE_BAR;
                    div()
                        .w_full()
                        .h(px(if idle { IDLE_BAR } else { v * half }))
                        .rounded(px(crate::style::radius(CHART_COL_RADIUS_PX)))
                        .bg(if idle { idle_tick } else { from.mix(to, v).with_alpha(fade) }.hsla())
                };
                col.child(div().flex_1().flex().flex_col().justify_end().child(bar(a, down, down2)))
                    .child(div().h(px(CHART_MIDLINE_PX)))
                    .child(div().flex_1().child(bar(b, up, up2)))
            }
            None => col,
        }
    };
    let cols = (0..capacity).map(|slot| {
        let sample = slot.checked_sub(empty).and_then(|i| samples.get(i).copied());
        column(sample, 1.0 - slot as f32 / capacity.max(1) as f32)
    });
    div()
        .relative()
        .h(px(CHART_H))
        .w_full()
        .p(px(CHART_PAD_PX))
        .rounded(px(crate::style::radius(CHART_RADIUS_PX)))
        .bg(wash(WASH_FAINT, palette).hsla())
        .flex()
        .gap(px(CHART_COL_GAP_PX))
        .children(cols)
        .into_any_element()
}

fn link_icon(kind: LinkKind) -> &'static str {
    match kind {
        LinkKind::Wifi => "wifi-4",
        LinkKind::Ethernet => "ethernet",
        LinkKind::Tunnel => "shield",
        LinkKind::Virtual => "box",
        LinkKind::Other => "globe",
    }
}

fn link_label(kind: LinkKind) -> &'static str {
    match kind {
        LinkKind::Wifi => t("Wi-Fi"),
        LinkKind::Ethernet => t("Ethernet"),
        LinkKind::Tunnel => t("VPN / tunnel"),
        LinkKind::Virtual => t("Virtual"),
        LinkKind::Other => t("Network"),
    }
}

/// Interfaces worth listing: physical links and tunnels (busiest first);
/// bridges and container veths are summarised, not listed.
fn listed(rates: &[IfaceRate]) -> Vec<&IfaceRate> {
    let mut v: Vec<&IfaceRate> = rates.iter().filter(|r| r.kind != LinkKind::Virtual).collect();
    v.sort_by(|a, b| {
        a.kind.counts_toward_total().cmp(&b.kind.counts_toward_total()).reverse().then((b.rx + b.tx).total_cmp(&(a.rx + a.tx))).then(b.rx_total.cmp(&a.rx_total))
    });
    v.truncate(MAX_IFACES);
    v
}

fn virtual_count(rates: &[IfaceRate]) -> usize {
    rates.iter().filter(|r| r.kind == LinkKind::Virtual).count()
}

/// Rebuilds the per-sample labels and series when their inputs changed.
fn derive(l: &mut Live, unit: Unit) {
    let key = (l.generation, unit);
    if l.derived.as_ref().is_some_and(|d| d.key == key) {
        return;
    }
    let ifaces: Vec<IfaceRate> = listed(&l.rates).into_iter().cloned().collect();
    let addr_of = |name: &str| l.addresses.iter().find(|a| a.iface == name).map(|a| a.addr);
    let iface_text = ifaces
        .iter()
        .map(|r| IfaceText {
            addr: addr_of(&r.name).map_or_else(|| SharedString::from(t("no address")), |a| a.to_string().into()),
            rx: rate_text(r.rx, unit).into(),
            tx: rate_text(r.tx, unit).into(),
        })
        .collect();
    let virtuals = virtual_count(&l.rates);
    let subtitle: SharedString = match ifaces.iter().find(|r| r.kind.counts_toward_total()) {
        Some(r) => match addr_of(&r.name) {
            Some(a) => format!("{} · {a}", r.name).into(),
            None => r.name.clone().into(),
        },
        None if l.last.is_none() => t("Measuring…").into(),
        None => t("No active connection").into(),
    };
    let empty;
    let history = match &l.history {
        Some(h) => h,
        None => {
            empty = RateHistory::default();
            &empty
        }
    };
    let peak = history.peak_seconds();
    let (peak_rx, peak_tx) = history.seconds.iter().fold((0.0f64, 0.0f64), |(a, b), (r, t)| (a.max(*r), b.max(*t)));
    let peak_label = |bps: f64| SharedString::from(tf("peak {rate}", &[("rate", &rate_text(bps, unit))]));
    let (samples, capacity, gpeak): (Vec<(f64, f64)>, usize, f64) = if l.long_window {
        (history.buckets.iter().copied().collect(), ns::LONG_BUCKETS, history.peak_buckets())
    } else {
        (history.seconds.iter().copied().collect(), ns::SHORT_WINDOW, peak)
    };
    let (boot_rx, boot_tx) = l.rates.iter().filter(|r| r.kind.counts_toward_total()).fold((0u64, 0u64), |(a, b), r| (a + r.rx_total, b + r.tx_total));
    l.derived = Some(Derived {
        key,
        iface_text,
        virtuals: (virtuals > 0).then(|| tf("+{n} virtual", &[("n", &virtuals)]).into()),
        subtitle,
        peak_rx: peak_label(peak_rx),
        peak_tx: peak_label(peak_tx),
        down_series: normalized(history.seconds.iter().map(|s| s.0), peak, ns::SHORT_WINDOW),
        up_series: normalized(history.seconds.iter().map(|s| s.1), peak, ns::SHORT_WINDOW),
        samples,
        capacity,
        gpeak,
        gpeak_text: peak_label(gpeak),
        session: (ns::format_amount(l.session.0 as u64).into(), ns::format_amount(l.session.1 as u64).into()),
        boot: (ns::format_amount(boot_rx).into(), ns::format_amount(boot_tx).into()),
        ifaces,
    });
}

fn iface_row(r: &IfaceRate, txt: &IfaceText, palette: &MandaraPalette) -> AnyElement {
    let ((down, _), (up, _)) = colors(palette);
    let tint = readable(if r.kind.counts_toward_total() { palette.primary } else { palette.secondary }, palette);
    let rate = |icon_name: &'static str, c: Color, v: SharedString| {
        div()
            .w(px(IFACE_RATE_W_PX))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(RATE_ICON_GAP_PX))
            .child(icon(icon_name, RATE_ICON_PX, c))
            .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(v))
    };
    div()
        .h(px(IFACE_H))
        .flex()
        .items_center()
        .gap(px(IFACE_GAP_PX))
        .font_features(tabular_nums())
        .child(
            div()
                .size(px(IFACE_BADGE_PX))
                .flex_none()
                .rounded(px(crate::style::radius(IFACE_BADGE_RADIUS_PX)))
                .bg(tint.with_alpha(IFACE_BADGE_ALPHA).hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(link_icon(r.kind), ICON_PX, tint)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(ROW_GAP_PX))
                        .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(r.name.clone())))
                        .child(caption(link_label(r.kind), palette)),
                )
                .child(caption(txt.addr.clone(), palette).truncate()),
        )
        .child(rate("arrow-down", down, txt.rx.clone()))
        .child(rate("arrow-up", up, txt.tx.clone()))
        .into_any_element()
}

fn totals(d: &Derived, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let ((down, _), (up, _)) = colors(palette);
    let cell = |label: &'static str, (rx, tx): (SharedString, SharedString)| {
        let figure = |name: &'static str, c: Color, v: SharedString| div().flex().items_center().gap(px(RATE_ICON_GAP_PX)).child(icon(name, RATE_ICON_PX, c)).child(v);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(RATE_ICON_GAP_PX))
            .child(caption(label, palette))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::SM))
                    .font_features(tabular_nums())
                    .text_size(px(text::BODY_S))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(figure("arrow-down", down, rx))
                    .child(figure("arrow-up", up, tx)),
            )
    };
    glass_card(palette)
        .rounded(px(radius))
        .h(px(TOTALS_CARD_H))
        .px(px(CARD_PAD_PX))
        .flex()
        .items_center()
        .gap(px(TILE_GAP_PX))
        .child(cell(t("This session"), d.session.clone()))
        .child(div().w(px(DIVIDER_W_PX)).h(px(DIVIDER_H_PX)).bg(palette.on_surface.with_alpha(DIVIDER_ALPHA).hsla()))
        .child(cell(t("Since the link came up"), d.boot.clone()))
        .into_any_element()
}

/// Log position of a rate on the gauge (`0..=1`).
fn gauge_fraction(bps: f64) -> f32 {
    if bps <= GAUGE_MIN_BPS {
        return (bps / GAUGE_MIN_BPS * GAUGE_FLOOR_FRAC).clamp(0.0, GAUGE_FLOOR_FRAC) as f32;
    }
    ((bps / GAUGE_MIN_BPS).ln() / (GAUGE_MAX_BPS / GAUGE_MIN_BPS).ln()).clamp(0.0, 1.0) as f32
}

/// A 270° arc of dots lit up to `frac`, the head dot glowing.
fn gauge(frac: f32, inner: AnyElement, palette: &MandaraPalette) -> AnyElement {
    let ((from, _), (to, _)) = colors(palette);
    let track = palette.on_surface.with_alpha(super::panel_fx::alpha(GAUGE_TRACK_ALPHA, palette));
    let half = GAUGE_D / 2.0;
    let r = half - GAUGE_DOT_PX * GAUGE_DOT_INSET;
    let steps = (GAUGE_DOTS - 1) as f32;
    let lit = (frac.clamp(0.0, 1.0) * steps).round() as usize;
    let start = std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * (1.0 - GAUGE_SWEEP) / 2.0;
    let dots = (0..GAUGE_DOTS).map(move |i| {
        let a = start + std::f32::consts::TAU * GAUGE_SWEEP * i as f32 / steps;
        let on = frac > 0.0 && i <= lit;
        let head = on && i == lit;
        let d = if head { GAUGE_DOT_PX * 2.0 } else { GAUGE_DOT_PX };
        let c = if on { from.mix(to, i as f32 / GAUGE_DOTS as f32) } else { track };
        div()
            .absolute()
            .left(px(half + r * a.cos() - d / 2.0))
            .top(px(half + r * a.sin() - d / 2.0))
            .size(px(d))
            .rounded_full()
            .bg(c.hsla())
            .when(head, |el| el.shadow(vec![glow(c.with_alpha(GAUGE_HEAD_GLOW.0), GAUGE_HEAD_GLOW.1, 1.0, 0.0)]))
    });
    div()
        .relative()
        .size(px(GAUGE_D))
        .flex_none()
        .children(dots)
        .child(div().absolute().top_0().left_0().size_full().flex().flex_col().items_center().justify_center().child(inner))
        .into_any_element()
}

/// Steps a tile spring toward `target`; true while it is visibly moving.
fn step_tile(spring: &mut SpringValue<f32>, target: f32, dt: f32) -> bool {
    spring.set_target(target);
    if !spring.step(dt) {
        return false;
    }
    if (spring.value() - target).abs() <= (target.abs() * TILE_SETTLE_REL).max(TILE_SETTLE_BPS) {
        spring.snap_to_target();
        return false;
    }
    true
}

fn speed_card(l: &mut Live, unit: Unit, mb: u64, palette: &MandaraPalette, display: SharedString, radius: f32, visible: bool, window: &mut Window) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let (target, key, running) = match &l.test {
        SpeedTest::Idle => (0.0, TestKey::Idle, false),
        SpeedTest::Running(p) => (p.rate(), TestKey::Running(p.bytes.load(Ordering::Relaxed), p.expected.load(Ordering::Relaxed)), true),
        SpeedTest::Done(r) => (r.rate, TestKey::Done(r.bytes, r.seconds.to_bits()), false),
        SpeedTest::Failed(_) => (0.0, TestKey::Failed, false),
    };
    let test = l.test.clone();
    let status = l.test_status.get(key.clone(), |_| match &test {
        SpeedTest::Idle => t("Download speed test").to_string(),
        SpeedTest::Running(p) => tf("Testing… {percent}%", &[("percent", &format!("{:.0}", p.fraction() * PERCENT))]),
        SpeedTest::Done(_) => t("Download speed").to_string(),
        SpeedTest::Failed(_) => t("Speed test failed").to_string(),
    });
    let detail = l.test_detail.get(key, |k| match (&test, k) {
        (SpeedTest::Running(_), TestKey::Running(done, total)) => tf("{done} of {total}", &[("done", &ns::format_amount(*done)), ("total", &ns::format_amount(*total))]),
        (SpeedTest::Done(r), _) => tf(
            "{size} in {secs} s · first byte {ms} ms",
            &[("size", &ns::format_amount(r.bytes)), ("secs", &format!("{:.1}", r.seconds)), ("ms", &format!("{:.0}", r.first_byte_ms))],
        ),
        (SpeedTest::Failed(e), _) => e.clone(),
        _ => tf("Downloads {size} from speed.cloudflare.com", &[("size", &ns::format_amount(mb.saturating_mul(BYTES_PER_MB)))]),
    });
    let (spring, clock) = (&mut l.gauge.0, &mut l.gauge.1);
    spring.set_target(gauge_fraction(target));
    let dt = clock.tick(Instant::now());
    if spring.step(dt) && visible {
        window.request_animation_frame();
    } else {
        clock.idle();
    }
    let frac = spring.value();
    let (v, u) = ns::format_rate(target, unit);
    let has_value = target > 0.0;
    let inner = div()
        .flex()
        .flex_col()
        .items_center()
        .font_family(display)
        .font_features(tabular_nums())
        .child(div().text_size(px(GAUGE_VALUE_PX)).line_height(px(GAUGE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(if has_value { v } else { "–".into() }))
        .child(div().text_size(px(GAUGE_UNIT_PX)).text_color(palette.on_surface_variant.hsla()).child(u))
        .into_any_element();
    let failed = matches!(l.test, SpeedTest::Failed(_));
    let button = if running {
        pill_button("ns-cancel", Some("close"), t("Cancel"), palette.on_surface_variant, palette, |_, _| cancel_test()).into_any_element()
    } else {
        let label = if matches!(l.test, SpeedTest::Idle) { t("Run test") } else { t("Run again") };
        pill_button("ns-run", Some("gauge"), label, accent, palette, |_, cx| {
            start_test(cx);
            notify_views(cx);
        })
        .into_any_element()
    };
    glass_card(palette)
        .rounded(px(radius))
        .h(px(TEST_H))
        .px(px(CARD_PAD_PX))
        .flex()
        .items_center()
        .gap(px(TEST_GAP_PX))
        .child(gauge(frac, inner, palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(tokens::XS))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(status))
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .font_features(tabular_nums())
                        .text_color(if failed { readable(palette.urgent_coral, palette).hsla() } else { palette.on_surface_variant.hsla() })
                        .child(detail),
                )
                .child(div().mt(px(TEST_BUTTON_GAP_PX)).flex().child(button)),
        )
        .into_any_element()
}

fn panel_height(ifaces: usize) -> f32 {
    let list = if ifaces == 0 { IFACE_H } else { ifaces as f32 * IFACE_H };
    PAD * 2.0 + HEADER_H + GAP + TILE_H + GAP + GRAPH_H + GAP + SECTION_H_PX + SECTION_GAP_PX + list + GAP + TOTALS_CARD_H + GAP + TEST_H
}

struct View {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<Pixels>>>,
    flash: Flash,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let visible = self.p.progress() > 0.0;
        let l = live();
        if visible {
            l.borrow_mut().panel_seen = Some(Instant::now());
            keep_sampling(&l, window, cx);
            ensure_public_ip(cx);
            if matches!(l.borrow().test, SpeedTest::Running(_)) {
                redraw_in(TEST_FRAME, window, cx);
            }
        }
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let unit = unit(&cfg);
        let mb = test_megabytes(&cfg);
        let radius = PanelShape::of(cx).inner(PAD);
        let display = fonts(cx).display;
        let mut lb = l.borrow_mut();
        derive(&mut lb, unit);
        // Rows are reserved for every listed interface from the first
        // counter read, so the panel never grows once rates arrive.
        let rows = lb.derived.as_ref().map_or(0, |d| d.ifaces.len()).max(lb.iface_hint);
        let h = panel_height(rows);
        if (f32::from(self.size_cell.get().height) - h).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }

        // ── Tiles (spring per frame while moving) ────────────────────────
        let (rx, tx) = lb.total;
        let (sd, su) = {
            let shown = &mut lb.shown;
            let (a, b, clock) = (&mut shown.0, &mut shown.1, &mut shown.2);
            let dt = clock.tick(Instant::now());
            let moving = step_tile(a, rx as f32, dt) | step_tile(b, tx as f32, dt);
            if moving && visible {
                window.request_animation_frame();
            } else {
                clock.idle();
            }
            (a.value().max(0.0), b.value().max(0.0))
        };
        let test = speed_card(&mut lb, unit, mb, palette, display.clone(), radius, visible, window);
        let measured = lb.last.is_some();
        let long = lb.long_window;
        let public_ip = lb.public_ip.clone();
        let Some(d) = lb.derived.as_ref() else { return div().into_any_element() };

        // ── Header ───────────────────────────────────────────────────────
        let accent = readable(palette.primary, palette);
        let ip_chip: AnyElement = match public_ip {
            PublicIp::Done(ip, _) => {
                let flash = self.flash.clone();
                div()
                    .id("ns-public-ip")
                    .cursor_pointer()
                    .child(chip("globe", ip.to_string(), accent, palette))
                    .on_click(move |_, _, cx| {
                        match mandara_services::clipboard::serve(ip.to_string().into_bytes(), None) {
                            Ok(()) => flash.show(true, t("Public IP copied")),
                            Err(e) => flash.show(false, e),
                        }
                        notify_views(cx);
                    })
                    .into_any_element()
            }
            PublicIp::Loading | PublicIp::Idle => chip("globe", t("Public IP…"), palette.on_surface_variant, palette).into_any_element(),
            PublicIp::Failed(..) => chip("wifi-off", t("Public IP unavailable"), readable(palette.warning_amber, palette), palette).into_any_element(),
        };
        let head = header("gauge", accent, t("Network speed"), d.subtitle.clone(), palette, Some(ip_chip));

        let (dc, uc) = colors(palette);
        let tiles = div()
            .flex()
            .gap(px(TILE_GAP_PX))
            .child(tile("arrow-down", t("Download"), sd, d.peak_rx.clone(), &d.down_series, dc, unit, palette, display.clone(), radius))
            .child(tile("arrow-up", t("Upload"), su, d.peak_tx.clone(), &d.up_series, uc, unit, palette, display, radius));

        // ── Graph ────────────────────────────────────────────────────────
        let set_window = |long: bool| -> Rc<dyn Fn(&mut Window, &mut App)> {
            Rc::new(move |_, cx| {
                {
                    let l = live();
                    let mut lb = l.borrow_mut();
                    lb.long_window = long;
                    lb.generation += 1;
                }
                notify_views(cx);
            })
        };
        let window_toggle = segmented(
            "ns-window",
            vec![
                Segment { label: "60 s".into(), icon: None, active: !long, on_click: set_window(false) },
                Segment { label: "10 min".into(), icon: None, active: long, on_click: set_window(true) },
            ],
            palette,
        );
        let legend = |c: Color, s: &'static str| div().flex().items_center().gap(px(LEGEND_GAP_PX)).child(div().size(px(LEGEND_DOT_PX)).rounded_full().bg(c.hsla())).child(caption(s, palette));
        let graph = glass_card(palette)
            .rounded(px(radius))
            .h(px(GRAPH_H))
            .p(px(CARD_PAD_PX))
            .flex()
            .flex_col()
            .gap(px(tokens::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(TILE_GAP_PX))
                    .child(legend(dc.0, t("Download")))
                    .child(legend(uc.0, t("Upload")))
                    .child(div().flex_1())
                    .child(caption(d.gpeak_text.clone(), palette).font_features(tabular_nums()))
                    .child(div().w(px(WINDOW_TOGGLE_W_PX)).child(window_toggle)),
            )
            .child(if d.samples.is_empty() {
                div()
                    .h(px(CHART_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(text::BODY_S))
                    .text_color(palette.on_surface_variant.hsla())
                    .child(if long { t("Collecting 10-second averages…") } else { t("Measuring…") })
                    .into_any_element()
            } else {
                mirrored_chart(&d.samples, d.capacity, d.gpeak, palette)
            });

        // ── Interfaces, totals ───────────────────────────────────────────
        let iface_list: AnyElement = if d.ifaces.is_empty() {
            div()
                .h(px(IFACE_H * rows.max(1) as f32))
                .flex()
                .items_center()
                .text_size(px(text::BODY_S))
                .text_color(palette.on_surface_variant.hsla())
                .child(if measured && lb.iface_hint == 0 { t("No network interfaces with traffic") } else { t("Measuring…") })
                .into_any_element()
        } else {
            div().flex().flex_col().children(d.ifaces.iter().zip(&d.iface_text).map(|(r, txt)| iface_row(r, txt, palette))).into_any_element()
        };
        let virtuals = d.virtuals.clone().map(|v| caption(v, palette).into_any_element());
        let totals_card = totals(d, palette, radius);
        drop(lb);
        let p = &self.p;
        let flash = self.flash.current(window, cx);
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(p, 1, tiles))
            .child(div().h(px(GAP)))
            .child(reveal(p, 2, graph))
            .child(div().h(px(GAP)))
            .child(reveal(p, 3, div().flex().flex_col().gap(px(SECTION_GAP_PX)).child(section(t("INTERFACES"), palette, virtuals)).child(iface_list)))
            .child(div().h(px(GAP)))
            .child(reveal(p, 4, totals_card))
            .child(div().h(px(GAP)))
            .child(reveal(p, 5, test))
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }
}

fn pending<T: serde::Serialize + Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-net-speed-cmd".into()).spawn(move || {
        let _ = tx.send_blocking(work().map(|v| serde_json::to_string(&v).unwrap_or_default()));
    });
    Output::Pending(rx)
}

/// `net-speed status`: the live rates when fresh, otherwise two quick
/// samples [`PROBE`] apart taken on a worker thread (never on the UI one).
fn status_output() -> Output {
    let l = live();
    let recent = {
        let lb = l.borrow();
        (lb.last.as_ref().is_some_and(|s| s.at.elapsed() < lb.every * 2) && !lb.rates.is_empty()).then(|| lb.rates.clone())
    };
    let report = |rates: Vec<IfaceRate>| {
        let (rx, tx) = ns::total(&rates);
        json!({ "download_bps": rx, "upload_bps": tx, "interfaces": rates })
    };
    match recent {
        Some(rates) => Output::Value(report(rates)),
        None => pending(move || {
            let a = Sample::now();
            std::thread::sleep(PROBE);
            Ok(report(ns::rates(&a, &Sample::now(), LinkKind::classify)))
        }),
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Network speed"), tk("Live upload and download rates, public IP and a speed test"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(None, bar_view, window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height(1)))));
        // Learn how many interfaces the panel lists (off the UI thread) so
        // it opens at its final height.
        {
            let (task, size_cell) = (cx.background_executor().spawn(async { iface_hint(&Sample::now()) }), size_cell.clone());
            cx.spawn(async move |_| {
                let hint = task.await;
                let l = live();
                let mut lb = l.borrow_mut();
                if lb.last.is_none() {
                    lb.iface_hint = hint;
                    size_cell.set(size(px(PANEL_W), px(panel_height(hint))));
                }
            })
            .detach();
        }
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone(), flash: Flash::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new_output("net-speed status", tk("Current download / upload rates (bytes per second) and per-interface figures"), |_, _| Ok(status_output())));
    r.command(
        ID,
        CommandSpec::new_output("net-speed addresses", tk("Local IP addresses by interface"), |_, _| Ok(pending(|| Ok(json!(ns::addresses_by_iface(&ns::local_addresses())))))),
    );
    r.command(
        ID,
        CommandSpec::new_output("net-speed public-ip", tk("Look up the public IP address (one request to the configured endpoint)"), |_, cx| {
            let url = endpoint(&crate::state::config(cx).read(cx).snapshot());
            Ok(pending(move || ns::public_ip(&url).map(|ip| json!({ "ip": ip.to_string(), "endpoint": url }))))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("net-speed test", tk("Run a download speed test against speed.cloudflare.com"), |a, cx| {
            let mb = match a.get(0) {
                Some(_) => u64::try_from(a.int(0)?).unwrap_or(1).clamp(1, ns::MAX_TEST_MEGABYTES),
                None => test_megabytes(&crate::state::config(cx).read(cx).snapshot()),
            };
            Ok(pending(move || {
                ns::speed_test(mb, Arc::new(SpeedProgress::default())).map(|r| {
                    json!({ "bytes": r.bytes, "seconds": r.seconds, "download_bps": r.rate, "mbit_per_s": r.rate * BITS_PER_BYTE / BITS_PER_MBIT, "first_byte_ms": r.first_byte_ms })
                })
            }))
        })
        .arg(ArgSpec::optional("megabytes", ArgKind::Integer, tk("download size in MB (default from settings)")))
        .example("net-speed test 10"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_is_logarithmic_and_bounded() {
        assert_eq!(gauge_fraction(0.0), 0.0);
        assert!(gauge_fraction(GAUGE_MIN_BPS * 0.5) <= 0.04);
        assert!((gauge_fraction(GAUGE_MAX_BPS) - 1.0).abs() < 1e-6);
        assert_eq!(gauge_fraction(GAUGE_MAX_BPS * 10.0), 1.0);
        // 10× the minimum lands a third of the way on a 1000× scale.
        assert!((gauge_fraction(GAUGE_MIN_BPS * 10.0) - 1.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn listing_prefers_physical_links_and_summarises_bridges() {
        let r = |name: &str, rx: f64| IfaceRate { name: name.into(), kind: LinkKind::from_name(name), rx, tx: 0.0, rx_total: 0, tx_total: 0 };
        let rates = vec![r("wg0", 500.0), r("docker0", 0.0), r("wlan0", 10.0), r("veth9", 3.0)];
        let names: Vec<&str> = listed(&rates).iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["wlan0", "wg0"]);
        assert_eq!(virtual_count(&rates), 2);
    }

    #[test]
    fn panel_grows_with_interfaces() {
        assert_eq!(panel_height(3) - panel_height(2), IFACE_H);
        assert_eq!(panel_height(0), panel_height(1));
    }

    #[test]
    fn panel_height_matches_the_original_layout() {
        // 14·2 + 38 + 12 + 118 + 12 + 150 + 12 + 18 + 4 + 38 + 12 + 56 + 12 + 124
        assert_eq!(panel_height(1), 634.0);
    }

    #[test]
    fn tile_spring_rests_at_display_resolution() {
        let mut s = SpringValue::new(0.0_f32, TILE_SPRING);
        let mut frames = 0;
        while step_tile(&mut s, 1_000_000.0, 1.0 / 60.0) {
            frames += 1;
            assert!(frames < 240, "spring never settled");
        }
        assert_eq!(s.value(), 1_000_000.0);
    }
}
