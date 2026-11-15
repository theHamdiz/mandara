//! The Resources bar item in each look (`[plugins.settings.resources]
//! bar_style`): the classic rings, a single glyph, labelled numbers, or a
//! tiny live graph beside each number; `bar_metrics` picks CPU, memory or
//! both.
//!
//! Samples come from the hub's CPU / memory sampler. The Numbers and
//! Sparkline chips ask it for their own cadence (`bar_refresh_ms`) while
//! they are drawn on a bar that shows; once none is (the bar slid away, the
//! item left the bar, another look picked) the request lapses and the
//! sampler falls back to the panel's `refresh_ms`, as the rings always used.
//! The icon look follows no samples at all: they are still recorded for the
//! panel's history, but the bar item doesn't redraw.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::bar_display::{BarDisplay, RESOURCES_BAR_REFRESH_MS, RESOURCES_BAR_REFRESH_MS_RANGE, ResourcesBar, ResourcesMetrics};
use mandara_core::ToGpui as _;
use mandara_i18n::{Lang, t};
use mandara_theme::{Color, MandaraPalette};

use super::super::bar_chip::{self, CHART_TRAIL, GLYPH_PX, Trail};
use super::super::panel_fx::{PERCENT_STEP, Readout, percent_text, readable};
use super::super::settings_cache::{ConfigSettings, SettingsCache};
use super::{CHIP_GAP_PX, CHIP_RING_D_PX, CHIP_RING_GAP_PX, CHIP_RING_STROKE_PX, CHIP_SNAP_BELOW, CHIP_SPRING, CHIP_TRACK_ALPHA, CHIP_VALUE_W_PX, Live};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::{icon, ring};
use crate::kit::text;

/// A chip's request for faster samples lapses after this many of its own
/// periods without a render (it renders on every sample while shown).
const DEMAND_LAPSE: u32 = 3;
/// Readings past these turn a chip's figure (and graph) amber.
const HOT_CPU: f32 = 0.85;
const HOT_MEM: f32 = 0.9;
/// The sparkline columns' wells: quieter than the rings' track, there
/// are many of them.
const SPARK_TRACK_ALPHA: f32 = 0.07;
/// Gap between a Numbers chip's label and its figure.
const LABEL_GAP_PX: f32 = 4.0;

/// The bar item's look, resolved once per config snapshot.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct BarLook {
    pub style: ResourcesBar,
    pub metrics: ResourcesMetrics,
    /// How often a ticking chip wants samples (`bar_refresh_ms`).
    pub every: Duration,
}

impl ConfigSettings for BarLook {
    type Scope = ();

    fn resolve(config: &ShellConfig, _: Lang, _: ()) -> Self {
        let p = &config.plugins;
        let ms = p.setting(crate::plugins::resources::ID, "bar_refresh_ms", RESOURCES_BAR_REFRESH_MS).clamp(RESOURCES_BAR_REFRESH_MS_RANGE.0, RESOURCES_BAR_REFRESH_MS_RANGE.1);
        Self {
            style: ResourcesBar::parse(p.setting_str(crate::plugins::resources::ID, "bar_style")),
            metrics: ResourcesMetrics::parse(p.setting_str(crate::plugins::resources::ID, "bar_metrics")),
            every: Duration::from_millis(ms as u64),
        }
    }
}

thread_local! {
    static LOOK: RefCell<SettingsCache<BarLook>> = const { RefCell::new(SettingsCache::new()) };
    /// When a ticking chip last rendered on a bar that shows, and the
    /// cadence it wants.
    static DEMAND: Cell<Option<(Instant, Duration)>> = const { Cell::new(None) };
}

fn look_in(config: &Arc<ShellConfig>) -> BarLook {
    *LOOK.with(|l| l.borrow_mut().get(config, mandara_i18n::language(), ()))
}

pub(super) fn look(cx: &App) -> BarLook {
    look_in(&crate::state::config(cx).read(cx).snapshot())
}

/// The sampler's period: the panel's `base`, or a visible chip's faster
/// cadence while its request is fresh.
pub(super) fn cadence(base: Duration) -> Duration {
    cadence_at(DEMAND.with(Cell::get), base, Instant::now())
}

fn cadence_at(demand: Option<(Instant, Duration)>, base: Duration, now: Instant) -> Duration {
    match demand {
        Some((at, every)) if now.saturating_duration_since(at) <= every.max(base).saturating_mul(DEMAND_LAPSE) => every.min(base),
        _ => base,
    }
}

/// What the bar item keeps between samples: the rings' springs and the
/// graphs' trails (fixed size, no heap).
pub(super) struct Chip {
    rings: (Readout, Readout),
    trails: (Trail, Trail),
    /// The inline chart's longer history.
    charts: (Trail<CHART_TRAIL>, Trail<CHART_TRAIL>),
}

impl Chip {
    pub fn new() -> Self {
        Self {
            rings: (Readout::new(CHIP_SPRING, CHIP_SNAP_BELOW, PERCENT_STEP), Readout::new(CHIP_SPRING, CHIP_SNAP_BELOW, PERCENT_STEP)),
            trails: (Trail::default(), Trail::default()),
            charts: (Trail::default(), Trail::default()),
        }
    }

    /// Records one sample; whether the bar item shows samples at all.
    pub fn on_sample(&mut self, live: &RefCell<Live>, (cpu, mem): (f32, f32), look: BarLook) -> bool {
        // History accumulates for the panel whatever the bar shows.
        live.borrow_mut().record(cpu, mem);
        self.trails.0.push(cpu);
        self.trails.1.push(mem);
        self.charts.0.push(cpu);
        self.charts.1.push(mem);
        look.style.live()
    }
}

/// A figure in its fixed-width slot, so `9%` → `10%` never moves a
/// neighbour.
fn figure(value: f32, color: Color) -> gpui::Div {
    div().w(px(CHIP_VALUE_W_PX)).flex_none().text_color(color.hsla()).child(percent_text(value))
}

/// `(cpu, mem)` figure colours: muted, amber when hot.
fn figure_colors(cpu: f32, mem: f32, palette: &MandaraPalette) -> (Color, Color) {
    let hot = readable(palette.warning_amber, palette);
    (if cpu >= HOT_CPU { hot } else { palette.on_surface }, if mem >= HOT_MEM { hot } else { palette.on_surface })
}

pub(super) fn render(chip: &RefCell<Chip>, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let look = look(cx);
    if look.style.ticks() && !crate::bar::autohide::slid_away(window.window_handle().window_id()) {
        DEMAND.with(|d| d.set(Some((Instant::now(), look.every))));
    }
    let (cpu, mem) = {
        let s = crate::hub::resources_entity(cx).read(cx);
        (s.cpu, s.mem)
    };
    let vertical = crate::bar::vertical(cx);
    let m = look.metrics;
    match look.style {
        ResourcesBar::Icon => icon("gauge", crate::style::bar_icon(GLYPH_PX), palette.on_surface).into_any_element(),
        ResourcesBar::Rings => {
            let reduced = mandara_core::spring::motion_profile().reduce;
            let (cpu, mem) = {
                let mut c = chip.borrow_mut();
                (c.rings.0.show_reduced(cpu, reduced, window), c.rings.1.show_reduced(mem, reduced, window))
            };
            let track = palette.on_surface.with_alpha(CHIP_TRACK_ALPHA);
            let meter = |value: f32, color| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(CHIP_RING_GAP_PX))
                    .child(ring(value, CHIP_RING_D_PX, CHIP_RING_STROKE_PX, color, track))
                    .child(div().w(px(CHIP_VALUE_W_PX)).text_color(palette.on_surface_variant.hsla()).child(percent_text(value)))
            };
            div()
                .flex()
                .items_center()
                .gap(px(CHIP_GAP_PX))
                .font_features(tabular_nums())
                .when(m.cpu(), |el| el.child(meter(cpu, palette.primary)))
                .when(m.memory(), |el| el.child(meter(mem, palette.tertiary)))
                .into_any_element()
        }
        ResourcesBar::Numbers | ResourcesBar::Sparkline | ResourcesBar::Chart if vertical => {
            // A side bar: each label over its figure.
            let (cpu_ink, mem_ink) = figure_colors(cpu, mem, palette);
            let pair = |label: &'static str, value: f32, ink: Color| {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(div().text_size(px(text::CAPTION)).text_color(palette.on_surface_variant.hsla()).child(label))
                    .child(div().text_color(ink.hsla()).child(percent_text(value)))
            };
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(LABEL_GAP_PX))
                .text_size(px(text::LABEL))
                .font_features(tabular_nums())
                .when(m.cpu(), |el| el.child(pair(t("CPU"), cpu, cpu_ink)))
                .when(m.memory(), |el| el.child(pair(t("RAM"), mem, mem_ink)))
                .into_any_element()
        }
        ResourcesBar::Numbers => {
            let (cpu_ink, mem_ink) = figure_colors(cpu, mem, palette);
            let pair = |label: &'static str, value: f32, ink: Color| {
                div()
                    .row()
                    .items_baseline()
                    .gap(px(LABEL_GAP_PX))
                    .child(div().text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(label))
                    .child(figure(value, ink))
            };
            bar_chip::row(bar_chip::PART_GAP_PX)
                .font_features(tabular_nums())
                .when(m.cpu(), |el| el.child(pair(t("CPU"), cpu, cpu_ink)))
                .when(m.memory(), |el| el.child(pair(t("RAM"), mem, mem_ink)))
                .into_any_element()
        }
        ResourcesBar::Sparkline => {
            let (cpu_ink, mem_ink) = figure_colors(cpu, mem, palette);
            let track = palette.on_surface.with_alpha(SPARK_TRACK_ALPHA);
            let (cpu_trail, mem_trail) = chip.borrow().trails;
            let graph = |trail: Trail, color: Color, value: f32, ink: Color| {
                // Graphs read left to right in every language.
                div().ltr_row().items_center().gap(px(bar_chip::GLYPH_GAP_PX)).child(bar_chip::sparkline(trail, color, track)).child(figure(value, ink))
            };
            let hot = |v: f32, at: f32, c: Color| if v >= at { readable(palette.warning_amber, palette) } else { readable(c, palette) };
            bar_chip::row(bar_chip::PART_GAP_PX)
                .font_features(tabular_nums())
                .when(m.cpu(), |el| el.child(graph(cpu_trail, hot(cpu, HOT_CPU, palette.primary), cpu, cpu_ink)))
                .when(m.memory(), |el| el.child(graph(mem_trail, hot(mem, HOT_MEM, palette.tertiary), mem, mem_ink)))
                .into_any_element()
        }
        ResourcesBar::Chart => {
            let (cpu_ink, mem_ink) = figure_colors(cpu, mem, palette);
            let track = palette.on_surface.with_alpha(SPARK_TRACK_ALPHA);
            let (cpu_trail, mem_trail) = chip.borrow().charts;
            let hot = |v: f32, at: f32, c: Color| if v >= at { readable(palette.warning_amber, palette) } else { readable(c, palette) };
            let cpu_series = m.cpu().then(|| (cpu_trail, hot(cpu, HOT_CPU, palette.primary)));
            let mem_series = m.memory().then(|| (mem_trail, hot(mem, HOT_MEM, palette.tertiary)));
            // With one reading it is the area; with both, CPU fills and memory is the line.
            let (area, line) = match (cpu_series, mem_series) {
                (Some(c), m) => (Some(c), m),
                (None, m) => (m, None),
            };
            // Charts and their figures read left to right in every language.
            div()
                .ltr_row()
                .items_center()
                .gap(px(bar_chip::GLYPH_GAP_PX))
                .font_features(tabular_nums())
                .child(bar_chip::chart(area, line, track))
                .when(m.cpu(), |el| el.child(figure(cpu, cpu_ink)))
                .when(m.memory(), |el| el.child(figure(mem, mem_ink)))
                .into_any_element()
        }
    }
}

/// The bar item: follows the hub's samples (recording them for the panel)
/// and redraws on them only in a look that shows them.
pub(super) fn bar_view(live: Rc<RefCell<Live>>, window: &mut Window, cx: &mut App) -> gpui::AnyView {
    let chip = Rc::new(RefCell::new(Chip::new()));
    let drawn = chip.clone();
    let bar = super::super::fn_view(None, move |palette, window, cx| render(&drawn, palette, window, cx), window, cx);
    let samples = crate::hub::resources_entity(cx);
    super::super::follow_with(
        &bar,
        &samples,
        move |samples, cx| {
            let reading = {
                let s = samples.read(cx);
                (s.cpu, s.mem)
            };
            let look = look(cx);
            chip.borrow_mut().on_sample(&live, reading, look)
        },
        cx,
    );
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml: &str) -> Arc<ShellConfig> {
        let mut c = ShellConfig::default();
        c.plugins.settings.insert(crate::plugins::resources::ID.into(), toml::from_str(toml).unwrap());
        Arc::new(c)
    }

    #[test]
    fn the_default_look_is_the_classic_rings() {
        let l = look_in(&Arc::new(ShellConfig::default()));
        assert_eq!((l.style, l.metrics), (ResourcesBar::Rings, ResourcesMetrics::Both));
        assert_eq!(l.every, Duration::from_millis(RESOURCES_BAR_REFRESH_MS as u64));
        let l = look_in(&cfg("bar_style = \"sparkline\"\nbar_metrics = \"cpu\"\nbar_refresh_ms = 50"));
        assert_eq!((l.style, l.metrics), (ResourcesBar::Sparkline, ResourcesMetrics::Cpu));
        assert_eq!(l.every, Duration::from_millis(RESOURCES_BAR_REFRESH_MS_RANGE.0 as u64), "clamped to the range");
    }

    #[test]
    fn a_chip_speeds_the_sampler_only_while_it_keeps_rendering() {
        let (base, fast) = (Duration::from_millis(1500), Duration::from_millis(1000));
        let now = Instant::now();
        assert_eq!(cadence_at(None, base, now), base);
        assert_eq!(cadence_at(Some((now, fast)), base, now), fast);
        // A slower chip never slows the panel's own cadence.
        assert_eq!(cadence_at(Some((now, Duration::from_secs(5))), base, now), base);
        // Not drawn for a few periods (bar slid away, item removed): lapsed.
        assert_eq!(cadence_at(Some((now, fast)), base, now + base * (DEMAND_LAPSE + 1)), base);
    }

    #[test]
    fn samples_feed_the_panel_history_and_the_trails_in_every_look() {
        let live = RefCell::new(Live::new());
        let mut chip = Chip::new();
        let icon = BarLook { style: ResourcesBar::Icon, metrics: ResourcesMetrics::Both, every: Duration::ZERO };
        assert!(!chip.on_sample(&live, (0.2, 0.4), icon), "the icon doesn't redraw on samples");
        assert_eq!(live.borrow().cpu.len(), 1);
        assert_eq!(chip.trails.1.values(), &[0.4]);
        assert!(chip.on_sample(&live, (0.3, 0.5), BarLook { style: ResourcesBar::Numbers, ..icon }));
        assert_eq!(chip.trails.0.len(), 2);
        assert_eq!(chip.charts.1.values(), &[0.4, 0.5], "the chart keeps its own, longer history");
    }
}
