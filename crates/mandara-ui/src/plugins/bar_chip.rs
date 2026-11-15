//! What live bar chips share (`[plugins.settings.<id>] bar_style`, see
//! [`mandara_config::bar_display`]): a text slot that formats again only
//! when its source text changes, a truncating label, a short trail of
//! samples drawn as a tiny sparkline or an inline area chart of SDF quads,
//! and the `bar display`
//! command that switches a plugin's look.
//!
//! A chip is content only: the bar's own capsules, tiles and shapes go
//! under it as under every other item, so it looks native in every bar
//! style. A click still opens the plugin's panel.

use gpui::{App, Bounds, Div, IntoElement, ParentElement, SharedString, Styled, canvas, div, fill, point, px, size};
use mandara_config::bar_display::{self, BarDisplay, BluetoothBar, NetworkBar, NotificationsBar, ResourcesBar};
use mandara_core::ToGpui as _;
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_theme::Color;
use serde_json::json;

/// A chip's glyph (designed size; the bar's icon scale applies).
pub const GLYPH_PX: f32 = 15.0;
/// Gap between a glyph and its text, and between two readings.
pub const GLYPH_GAP_PX: f32 = 5.0;
pub const PART_GAP_PX: f32 = 10.0;

/// The newest samples a chip's sparkline keeps (fixed size: no heap).
pub const TRAIL: usize = 10;
/// The sparkline's size (designed; the height follows the icon scale),
/// the gap between its columns, and the shortest column.
const SPARK_W_PX: f32 = 34.0;
const SPARK_H_PX: f32 = 13.0;
const SPARK_GAP_PX: f32 = 1.0;
const SPARK_MIN_PX: f32 = 1.5;
/// How much the oldest column fades.
const SPARK_AGE_FADE: f32 = 0.55;
/// The samples the inline chart spans (fixed size: no heap).
pub const CHART_TRAIL: usize = 30;
/// The inline chart: its size (designed; the height follows the icon
/// scale), corner radius, column width, edge thickness, the inset that
/// keeps 0 % and 100 % inside the well, and its midline.
const CHART_W_PX: f32 = 64.0;
const CHART_H_PX: f32 = 16.0;
const CHART_RADIUS_PX: f32 = 4.0;
const CHART_COLUMN_PX: f32 = 1.0;
const CHART_LINE_PX: f32 = 1.5;
const CHART_INSET_PX: f32 = 2.0;
const CHART_GRID_PX: f32 = 1.0;
/// The area's fill under its edge, and the second reading's line.
const CHART_AREA_ALPHA: f32 = 0.22;
const CHART_SECOND_ALPHA: f32 = 0.85;

/// A label kept as shared text: re-made only when the text it shows
/// changes, compared borrowed so an unchanged render allocates nothing.
#[derive(Default)]
pub struct TextSlot(SharedString);

impl TextSlot {
    pub fn get(&mut self, source: &str) -> SharedString {
        if self.0.as_ref() != source {
            self.0 = SharedString::from(source.to_owned());
        }
        self.0.clone()
    }
}

/// One line of chip text, at most `max_w` px wide, ending in an ellipsis
/// when it doesn't fit (it shrinks before its neighbours do).
pub fn label(text: SharedString, max_w: f32, color: Color) -> Div {
    div().min_w(px(0.0)).max_w(px(max_w)).truncate().text_color(color.hsla()).child(text)
}

/// The newest `N` samples of one reading, oldest first ([`TRAIL`] for
/// the sparkline, [`CHART_TRAIL`] for the inline chart; fixed size, no
/// heap).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Trail<const N: usize = TRAIL> {
    values: [f32; N],
    len: usize,
}

impl<const N: usize> Default for Trail<N> {
    fn default() -> Self {
        Self { values: [0.0; N], len: 0 }
    }
}

impl<const N: usize> Trail<N> {
    /// Appends a sample (clamped to `0..=1`), dropping the oldest when full.
    pub fn push(&mut self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if self.len == N {
            self.values.copy_within(1.., 0);
            self.values[N - 1] = v;
        } else {
            self.values[self.len] = v;
            self.len += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The samples, oldest first.
    pub fn values(&self) -> &[f32] {
        &self.values[..self.len]
    }

    /// The trail's value at `t` along its full width (0 = the oldest slot,
    /// 1 = the newest sample), linearly between samples; `None` left of
    /// the first sample while the trail is still filling.
    fn at(&self, t: f32) -> Option<f32> {
        if self.len == 0 {
            return None;
        }
        let pos = t.clamp(0.0, 1.0) * (N - 1) as f32 - (N - self.len) as f32;
        if pos < 0.0 {
            return None;
        }
        let i = pos.floor() as usize;
        let next = (i + 1).min(self.len - 1);
        let f = pos - i as f32;
        Some(self.values[i] * (1.0 - f) + self.values[next] * f)
    }
}

/// `trail` as right-aligned columns (the newest always at the right:
/// charts stay left to right), older ones fading, each in a faint `track`
/// well. Painted as rounded quads; the trail is copied
/// into the paint closure, nothing is allocated.
pub fn sparkline(trail: Trail, color: Color, track: Color) -> impl IntoElement {
    let h = crate::style::bar_icon(SPARK_H_PX);
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let col = ((w - SPARK_GAP_PX * (TRAIL - 1) as f32) / TRAIL as f32).max(SPARK_MIN_PX);
            let radius = col / 2.0;
            for slot in 0..TRAIL {
                let x = bounds.origin.x + px(slot as f32 * (col + SPARK_GAP_PX));
                // Every column has its full-height well, so low readings
                // still draw a readable graph.
                window.paint_quad(fill(Bounds::new(point(x, bounds.origin.y), size(px(col), px(h))), track.hsla()).corner_radii(px(radius)));
                let Some(v) = slot.checked_sub(TRAIL - trail.len).map(|i| trail.values[i]) else { continue };
                let c = color.with_alpha(1.0 - (1.0 - slot as f32 / TRAIL as f32) * SPARK_AGE_FADE);
                let ch = (v * h).max(SPARK_MIN_PX);
                let at = point(x, bounds.origin.y + px(h - ch));
                window.paint_quad(fill(Bounds::new(at, size(px(col), px(ch))), c.hsla()).corner_radii(px(radius)));
            }
        },
    )
    .w(px(SPARK_W_PX))
    .h(px(h))
    .flex_none()
}

/// An inline area chart of up to two readings over [`CHART_TRAIL`]
/// samples, newest at the right (charts stay left to right): the first as
/// a soft filled area under a bright edge, the second as a fine line over
/// it. One-pixel columns of SDF quads, interpolated between samples, so
/// the edge reads as a smooth line; nothing is allocated.
pub fn chart(area: Option<(Trail<CHART_TRAIL>, Color)>, line: Option<(Trail<CHART_TRAIL>, Color)>, track: Color) -> impl IntoElement {
    let h = crate::style::bar_icon(CHART_H_PX);
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let (x0, y0) = (bounds.origin.x, bounds.origin.y);
            // The well, then a faint midline for scale.
            window.paint_quad(fill(bounds, track.hsla()).corner_radii(px(CHART_RADIUS_PX)));
            window.paint_quad(fill(Bounds::new(point(x0, y0 + px((h / 2.0).round())), size(px(w), px(CHART_GRID_PX))), track.hsla()));
            let columns = (w / CHART_COLUMN_PX).floor().max(1.0) as usize;
            for c in 0..columns {
                let t = if columns > 1 { c as f32 / (columns - 1) as f32 } else { 1.0 };
                let x = x0 + px(c as f32 * CHART_COLUMN_PX);
                let top = |v: f32| (h - CHART_INSET_PX - v * (h - 2.0 * CHART_INSET_PX)).clamp(0.0, h);
                if let Some((trail, color)) = area
                    && let Some(v) = trail.at(t)
                {
                    let y = top(v);
                    window.paint_quad(fill(Bounds::new(point(x, y0 + px(y)), size(px(CHART_COLUMN_PX), px(h - y))), color.with_alpha(CHART_AREA_ALPHA).hsla()));
                    window.paint_quad(fill(Bounds::new(point(x, y0 + px(y - CHART_LINE_PX / 2.0)), size(px(CHART_COLUMN_PX), px(CHART_LINE_PX))), color.hsla()));
                }
                if let Some((trail, color)) = line
                    && let Some(v) = trail.at(t)
                {
                    let y = top(v);
                    window.paint_quad(fill(Bounds::new(point(x, y0 + px(y - CHART_LINE_PX / 2.0)), size(px(CHART_COLUMN_PX), px(CHART_LINE_PX))), color.with_alpha(CHART_SECOND_ALPHA).hsla()));
                }
            }
        },
    )
    .w(px(CHART_W_PX))
    .h(px(h))
    .flex_none()
    .rounded(px(CHART_RADIUS_PX))
}

// ── `bar display` ────────────────────────────────────────────────────────

/// The plugins whose bar item has chips.
const CHIP_PLUGINS: [&str; 4] = [super::resources::ID, super::notifications::ID, super::network::ID, super::bluetooth::ID];

/// The looks of `plugin`'s bar item, the classic one first.
fn looks(plugin: &str) -> Option<Vec<&'static str>> {
    Some(match plugin {
        super::resources::ID => ResourcesBar::ids().collect(),
        super::notifications::ID => NotificationsBar::ids().collect(),
        super::network::ID => NetworkBar::ids().collect(),
        super::bluetooth::ID => BluetoothBar::ids().collect(),
        _ => return None,
    })
}

/// `plugin`'s look in `cfg`, as its config id.
fn current(plugin: &str, cfg: &mandara_config::ShellConfig) -> &'static str {
    let raw = cfg.plugins.setting_str(plugin, bar_display::KEY);
    match plugin {
        super::resources::ID => ResourcesBar::parse(raw).id(),
        super::notifications::ID => NotificationsBar::parse(raw).id(),
        super::network::ID => NetworkBar::parse(raw).id(),
        super::bluetooth::ID => BluetoothBar::parse(raw).id(),
        _ => "",
    }
}

fn report(plugin: &str, cx: &App) -> serde_json::Value {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    json!({ "plugin": plugin, "style": current(plugin, &cfg), "choices": looks(plugin).unwrap_or_default() })
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        "core",
        CommandSpec::new("bar display", tk("Show or switch what a plugin's bar item shows: its icon or a live chip"), |a, cx| {
            let Some(plugin) = a.get(0) else {
                return Ok(json!(CHIP_PLUGINS.iter().map(|p| report(p, cx)).collect::<Vec<_>>()));
            };
            let choices = looks(plugin).ok_or_else(|| format!("{plugin:?} has no bar chips (try {})", CHIP_PLUGINS.join(", ")))?;
            if let Some(style) = a.get(1) {
                if !choices.contains(&style) {
                    return Err(format!("{plugin} has no look {style:?} (try {})", choices.join(", ")));
                }
                let (id, style) = (plugin.to_string(), style.to_string());
                crate::state::update_config(cx, move |c| {
                    c.plugins.settings.entry(id).or_default().insert(bar_display::KEY.into(), toml::Value::String(style));
                });
            }
            Ok(report(plugin, cx))
        })
        .arg(ArgSpec::optional("plugin", ArgKind::Enum, tk("plugin with bar chips")).choices(&CHIP_PLUGINS))
        .arg(ArgSpec::optional("style", ArgKind::String, tk("look to switch to (leave out to show the current one)")))
        .example("bar display")
        .example("bar display resources sparkline")
        .example("bar display notifications latest"),
    );
}

/// A row of a chip's parts: glyph, text, figures.
pub fn row(gap: f32) -> Div {
    use crate::bidi::Bidi as _;
    div().row().items_center().gap(px(gap)).flex_none().min_w(px(0.0))
}

/// A chip that may hold text: shrinkable (its label ellipsizes first).
pub fn text_row(gap: f32) -> Div {
    use crate::bidi::Bidi as _;
    div().row().items_center().gap(px(gap)).min_w(px(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trails_keep_the_newest_samples_in_order() {
        let mut t: Trail = Trail::default();
        assert!(t.is_empty());
        for i in 0..TRAIL + 3 {
            t.push(i as f32 / 100.0);
        }
        assert_eq!(t.len(), TRAIL);
        assert_eq!(t.values()[0], 3.0 / 100.0, "the three oldest fell off");
        assert_eq!(*t.values().last().unwrap(), (TRAIL + 2) as f32 / 100.0);
        t.push(7.0);
        assert_eq!(*t.values().last().unwrap(), 1.0, "clamped");
        assert_eq!(std::mem::size_of::<Trail>(), TRAIL * 4 + std::mem::size_of::<usize>(), "inline, no heap");
    }

    #[test]
    fn the_chart_interpolates_between_samples_and_starts_empty_on_the_left() {
        let mut t: Trail<4> = Trail::default();
        assert_eq!(t.at(1.0), None);
        t.push(0.2);
        t.push(0.6);
        // Two samples fill the newest third: nothing left of them.
        assert_eq!(t.at(0.0), None);
        assert!((t.at(1.0).unwrap() - 0.6).abs() < 1e-6);
        let mid = t.at(5.0 / 6.0).unwrap();
        assert!((mid - 0.4).abs() < 1e-6, "halfway between the two samples: {mid}");
        assert_eq!(std::mem::size_of::<Trail<CHART_TRAIL>>(), CHART_TRAIL * 4 + std::mem::size_of::<usize>(), "inline, no heap");
    }

    #[test]
    fn text_slots_reuse_unchanged_text() {
        // Long enough to live on the heap (short ones are stored inline).
        const LONG: &str = "The guest network on the second floor, near the stairs";
        let mut slot = TextSlot::default();
        let a = slot.get(LONG);
        let b = slot.get(LONG);
        assert!(std::ptr::eq(a.as_ref().as_ptr(), b.as_ref().as_ptr()), "same allocation");
        assert_eq!(slot.get("Home-5G").as_ref(), "Home-5G");
        assert_eq!(slot.get("Office").as_ref(), "Office");
    }

    #[test]
    fn every_chip_plugin_lists_its_classic_look_first() {
        let cfg = mandara_config::ShellConfig::default();
        for p in CHIP_PLUGINS {
            let looks = looks(p).expect("has looks");
            assert_eq!(current(p, &cfg), looks[0], "{p}: the default is the classic look");
        }
        assert_eq!(looks("clock"), None);
        let mut cfg = cfg;
        cfg.plugins.settings.entry("network".into()).or_default().insert(bar_display::KEY.into(), toml::Value::String("both".into()));
        assert_eq!(current("network", &cfg), "both");
    }
}
