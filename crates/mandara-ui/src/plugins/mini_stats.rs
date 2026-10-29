//! Mini stats: CPU, memory, network, temperature and battery as dense
//! icon + number pairs — the hairline bar's signature content (off until
//! you place it or pick a look that does).
//!
//! It samples only while placed: one read of `/proc` + hwmon + the battery
//! every [`SAMPLE_EVERY`] on the background executor, formatted once per
//! sample; the chip itself redraws only when a sample lands. In a side bar
//! the pairs stack (icon over value).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_i18n::tk;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::net_speed::{self as ns, LinkKind, Sample, Unit};
use mandara_core::ToGpui;
use mandara_theme::MandaraPalette;

use super::fn_view;
use super::panel_fx::redraw_in;
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::text;
use crate::widgets::system::{CpuTimes, cpu_usage, read_cpu_times, read_mem_usage};

pub const ID: &str = mandara_config::looks::MINI_STATS_ID;

/// How often the readings refresh (they are glanceable, not a monitor).
const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// Icon size and the gaps inside / between pairs.
const ICON_PX: f32 = 12.0;
const PAIR_GAP_PX: f32 = 3.0;
const GAP_PX: f32 = 9.0;
/// Readings past these turn the value to the warning colour.
const HOT_CPU: f32 = 0.85;
const HOT_MEM: f32 = 0.9;
const HOT_TEMP_C: f32 = 85.0;
const LOW_BATTERY: u8 = 15;
const PERCENT: f32 = 100.0;

/// One sample's readings, already formatted.
#[derive(Clone, Default)]
struct Readings {
    cpu: (SharedString, bool),
    mem: (SharedString, bool),
    net: SharedString,
    temp: Option<(SharedString, bool)>,
    battery: Option<(SharedString, bool)>,
}

#[derive(Default)]
struct Live {
    last_cpu: Option<CpuTimes>,
    last_net: Option<Sample>,
    readings: Option<Readings>,
    due: Option<Instant>,
    busy: bool,
}

struct Raw {
    cpu: Option<CpuTimes>,
    mem: Option<f32>,
    net: Sample,
    temp: Option<f32>,
    battery: Option<u8>,
}

fn read_raw() -> Raw {
    let temps = crate::widgets::sensors::read_temperatures(std::path::Path::new("/sys/class/hwmon"));
    Raw {
        cpu: read_cpu_times(),
        mem: read_mem_usage(),
        net: Sample::now(),
        temp: crate::widgets::sensors::cpu_temperature(&temps),
        battery: crate::widgets::battery::read_battery().map(|b| b.percent),
    }
}

fn pct(v: f32) -> SharedString {
    format!("{:.0}%", (v * PERCENT).clamp(0.0, PERCENT)).into()
}

impl Live {
    fn apply(&mut self, raw: Raw) {
        let cpu = match (self.last_cpu, raw.cpu) {
            (Some(a), Some(b)) => cpu_usage(a, b),
            _ => 0.0,
        };
        self.last_cpu = raw.cpu;
        let down = self.last_net.as_ref().map(|old| ns::total(&ns::rates(old, &raw.net, LinkKind::classify)).0).unwrap_or(0.0);
        self.last_net = Some(raw.net);
        let (value, unit) = ns::format_rate(down, Unit::Bytes);
        let mem = raw.mem.unwrap_or(0.0);
        self.readings = Some(Readings {
            cpu: (pct(cpu), cpu >= HOT_CPU),
            mem: (pct(mem), mem >= HOT_MEM),
            net: format!("{value} {unit}").into(),
            temp: raw.temp.map(|c| (SharedString::from(format!("{c:.0}°")), c >= HOT_TEMP_C)),
            battery: raw.battery.map(|b| (SharedString::from(format!("{b}%")), b <= LOW_BATTERY)),
        });
    }
}

fn pair(glyph: &'static str, value: SharedString, hot: bool, vertical: bool, palette: &MandaraPalette) -> AnyElement {
    let ink = if hot { palette.warning_amber } else { palette.on_surface };
    div()
        .flex()
        .when(vertical, |el| el.flex_col())
        .items_center()
        .gap(px(PAIR_GAP_PX))
        .child(icon(glyph, crate::style::bar_icon(ICON_PX), palette.on_surface_variant))
        .child(div().text_color(ink.hsla()).child(value))
        .into_any_element()
}

fn view(live: &Rc<RefCell<Live>>, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let now = Instant::now();
    let due = live.borrow().due.is_none_or(|d| now >= d);
    if due && !live.borrow().busy {
        live.borrow_mut().busy = true;
        let view_id = window.current_view();
        let task = cx.background_executor().spawn(async { read_raw() });
        let live2 = live.clone();
        cx.spawn(async move |cx| {
            let raw = task.await;
            {
                let mut l = live2.borrow_mut();
                l.apply(raw);
                l.busy = false;
                l.due = Some(Instant::now() + SAMPLE_EVERY);
            }
            cx.update(|cx| cx.notify(view_id));
        })
        .detach();
    }
    if let Some(d) = live.borrow().due {
        redraw_in(d.saturating_duration_since(now), window, cx);
    }
    let Some(r) = live.borrow().readings.clone() else { return div().into_any_element() };
    let vertical = crate::bar::vertical(cx);
    div()
        .flex()
        .when(vertical, |el| el.flex_col().text_size(px(text::LABEL)))
        .items_center()
        .gap(px(GAP_PX))
        .font_features(tabular_nums())
        .child(pair("cpu", r.cpu.0, r.cpu.1, vertical, palette))
        .child(pair("memory", r.mem.0, r.mem.1, vertical, palette))
        .when(!vertical, |el| el.child(pair("arrow-down", r.net, false, vertical, palette)))
        .when_some(r.temp, |el, (t, hot)| el.child(pair("thermometer", t, hot, vertical, palette)))
        .when_some(r.battery, |el, (b, low)| el.child(pair("battery", b, low, vertical, palette)))
        .into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("Mini stats"), tk("CPU, memory, network, temperature and battery as compact numbers"), Some(Zone::Left)).opt_in(),
        |window, cx, _| {
            let live = Rc::new(RefCell::new(Live::default()));
            let bar = fn_view(None, move |palette, window, cx| view(&live, palette, window, cx), window, cx);
            PluginInstance { bar: Some(bar), ..PluginInstance::empty() }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_format_and_flag_hot_values() {
        let mut l = Live::default();
        let raw = |busy: u64| Raw { cpu: Some(CpuTimes { busy, total: 100 + busy }), mem: Some(0.95), net: Sample { at: Instant::now(), counters: Vec::new() }, temp: Some(91.0), battery: Some(9) };
        l.apply(raw(0));
        l.apply(raw(100));
        let r = l.readings.unwrap();
        assert!(r.mem.1, "95% memory is hot");
        assert_eq!(r.mem.0.as_ref(), "95%");
        assert!(r.temp.unwrap().1 && r.battery.unwrap().1);
    }
}
