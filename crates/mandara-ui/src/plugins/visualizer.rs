//! The media panel's audio visualizer styles (Studio → Audio & media):
//! mirrored bars, a liquid wave and an album ring, with a configurable bar
//! count and temporal smoothing. Settings live in `[plugins.settings.media]`
//! (`visualizer_style`, `visualizer_bars`, `visualizer_smoothing`).
//!
//! Every shape is drawn with rounded quads (SDF edges), never tessellated
//! paths. Silence draws a resting baseline — no fake motion.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Instant;

use gpui::{AnyElement, App, ImageSource, IntoElement, ParentElement, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_theme::{Color, MandaraPalette};

use super::panel_fx::{WASH_IDLE_BAR, wash};
use crate::icons::icon;

/// Settings key prefix owner (`[plugins.settings.media]`).
const SETTINGS_PLUGIN: &str = "media";
/// Bar count range and default.
const BARS_MIN: f64 = 8.0;
const BARS_MAX: f64 = 64.0;
const DEFAULT_BARS: usize = 32;
/// Smoothing range upper bound and default.
const SMOOTHING_MAX: f64 = 0.95;
const DEFAULT_SMOOTHING: f32 = 0.35;
/// Attack (rise) smoothing as a share of the decay smoothing, and its cap.
const ATTACK_SHARE: f32 = 0.35;
const ATTACK_MAX: f32 = 0.9;
/// Smoothing factors are per frame at this rate; other frame intervals
/// scale them so motion speed doesn't depend on the refresh rate.
const SMOOTHING_REFERENCE_HZ: f32 = 60.0;
/// Longest step one smoothing update may cover (a stalled frame).
const MAX_SMOOTH_DT_S: f32 = 1.0 / 20.0;

/// Bars: perceptual curve, minimum bar height / width, alpha range from
/// quiet to loud, and the gap for dense / sparse bar counts.
const BAR_GAMMA: f32 = 0.85;
const BAR_MIN_H_PX: f32 = 3.0;
const BAR_MIN_W_PX: f32 = 1.5;
const BAR_ALPHA: (f32, f32) = (0.55, 0.45);
const DENSE_BARS: usize = 40;
const DENSE_GAP_PX: f32 = 2.0;
const SPARSE_GAP_PX: f32 = 3.0;

/// Wave: columns, curve, resting height, column radius, echo layer
/// (scale, alpha, phase shift in bands) and front layer scale, plus the
/// alpha range of a column.
const WAVE_COLS: usize = 96;
const WAVE_GAMMA: f32 = 0.8;
const WAVE_REST_H_PX: f32 = 2.0;
const WAVE_COL_RADIUS_PX: f32 = 1.0;
const WAVE_ECHO: (f32, f32, f32) = (1.0, 0.35, 0.6);
const WAVE_FRONT_SCALE: f32 = 0.82;
const WAVE_ALPHA: (f32, f32) = (0.6, 0.4);

/// Ring: art diameter as a share of the height, most dots, gap between
/// the art and the dots, dot size range, outer margin, colour alpha range,
/// side bars (height share, gap, minimum count) and the placeholder.
const RING_ART_FRAC: f32 = 0.6;
const RING_MAX_DOTS: usize = 48;
const RING_ART_GAP_PX: f32 = 3.5;
const RING_DOT_PX: (f32, f32) = (2.0, 1.5);
const RING_MARGIN_PX: f32 = 1.5;
const RING_ALPHA: (f32, f32) = (0.5, 0.5);
const RING_SIDE_H_FRAC: f32 = 0.8;
const RING_SIDE_GAP_PX: f32 = 2.0;
const RING_SIDE_MIN_BARS: usize = 4;
const RING_ROW_GAP_PX: f32 = 8.0;
const RING_PLACEHOLDER_ALPHA: f32 = 0.3;
const RING_PLACEHOLDER_ICON_FRAC: f32 = 0.5;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Style {
    /// Mirrored, rounded bars (the default).
    Bars,
    /// A smooth, filled, mirrored wave with a softer echo layer behind it.
    Wave,
    /// A ring of dots around the album art, with bars bursting to both sides.
    Ring,
}

impl Style {
    pub const ALL: [(Style, &'static str); 3] = [(Style::Bars, "bars"), (Style::Wave, "wave"), (Style::Ring, "ring")];

    pub fn parse(s: &str) -> Style {
        match s {
            "wave" | "liquid-wave" => Style::Wave,
            "ring" | "album-ring" => Style::Ring,
            _ => Style::Bars,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Settings {
    pub style: Style,
    /// Bars / dots drawn (`8..=64`).
    pub bars: usize,
    /// `0` = raw levels, up to `0.95` = very smooth decay.
    pub smoothing: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { style: Style::Bars, bars: DEFAULT_BARS, smoothing: DEFAULT_SMOOTHING }
    }
}

impl Settings {
    pub fn from_config(cfg: &ShellConfig) -> Self {
        let d = Self::default();
        Self {
            style: Style::parse(&cfg.plugins.setting::<String>(SETTINGS_PLUGIN, "visualizer_style", "bars".into())),
            bars: cfg.plugins.setting::<f64>(SETTINGS_PLUGIN, "visualizer_bars", d.bars as f64).round().clamp(BARS_MIN, BARS_MAX) as usize,
            smoothing: cfg.plugins.setting::<f64>(SETTINGS_PLUGIN, "visualizer_smoothing", d.smoothing as f64).clamp(0.0, SMOOTHING_MAX) as f32,
        }
    }

    /// The settings for `cfg`, parsed once per config change (parsing
    /// allocates and the media panel asks every frame).
    pub fn cached(cfg: &Arc<ShellConfig>) -> Self {
        thread_local! {
            static CACHE: RefCell<Option<(Weak<ShellConfig>, Settings)>> = const { RefCell::new(None) };
        }
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            match c.as_ref() {
                Some((seen, s)) if Weak::ptr_eq(seen, &Arc::downgrade(cfg)) => *s,
                _ => {
                    let s = Self::from_config(cfg);
                    *c = Some((Arc::downgrade(cfg), s));
                    s
                }
            }
        })
    }
}

/// Linearly resamples `bands` to `n` values.
pub fn resample(bands: &[f32], n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    resample_into(bands, n, &mut out);
    out
}

/// [`resample`] into a reused buffer (no allocation once it has grown).
pub fn resample_into(bands: &[f32], n: usize, out: &mut Vec<f32>) {
    out.clear();
    if bands.is_empty() || n == 0 {
        return;
    }
    if bands.len() == 1 {
        out.resize(n, bands[0]);
        return;
    }
    out.extend((0..n).map(|i| {
        let x = i as f32 / (n - 1).max(1) as f32 * (bands.len() - 1) as f32;
        let (a, t) = (x.floor() as usize, x.fract());
        let b = (a + 1).min(bands.len() - 1);
        bands[a] * (1.0 - t) + bands[b] * t
    }));
}

/// One smoothing step: rises follow the signal quickly, falls decay by
/// `smoothing` per frame (so peaks stay readable without lag on attacks).
pub fn smooth(state: &mut Vec<f32>, next: &[f32], smoothing: f32) {
    smooth_dt(state, next, smoothing, 1.0 / SMOOTHING_REFERENCE_HZ);
}

/// [`smooth`] for a frame `dt` seconds long: the per-frame factors are
/// defined at [`SMOOTHING_REFERENCE_HZ`] and rescaled (`k^(dt·hz)`), with
/// `dt` clamped, so the decay looks the same at 60 and 144 Hz.
pub fn smooth_dt(state: &mut Vec<f32>, next: &[f32], smoothing: f32, dt: f32) {
    if state.len() != next.len() {
        state.clear();
        state.extend_from_slice(next);
        return;
    }
    let frames = dt.clamp(0.0, MAX_SMOOTH_DT_S) * SMOOTHING_REFERENCE_HZ;
    let attack = (smoothing * ATTACK_SHARE).clamp(0.0, ATTACK_MAX).powf(frames);
    let decay = smoothing.powf(frames);
    for (s, &v) in state.iter_mut().zip(next) {
        let k = if v > *s { attack } else { decay };
        *s = *s * k + v * (1.0 - k);
    }
}

/// Catmull-Rom sample of `values` at fractional index `x`.
fn spline(values: &[f32], x: f32) -> f32 {
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    let i = x.floor() as isize;
    let t = x - i as f32;
    let at = |k: isize| values[k.clamp(0, n as isize - 1) as usize];
    let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let v = 0.5 * ((2.0 * p1) + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
    v.clamp(0.0, 1.0)
}

/// The media panel's live levels between frames.
struct Live {
    /// Smoothed levels (empty while silent).
    levels: Vec<f32>,
    /// Resampling scratch buffer.
    scratch: Vec<f32>,
    last: Option<Instant>,
}

thread_local! {
    static LIVE: RefCell<Live> = const { RefCell::new(Live { levels: Vec::new(), scratch: Vec::new(), last: None }) };
}

/// Base tint of each of `n` bars along `from → to`. OKLab mixes are
/// recomputed only when the art theme or bar count changes, not per frame.
fn tints(from: Color, to: Color, n: usize) -> std::rc::Rc<[Color]> {
    type Tints = Option<(Color, Color, std::rc::Rc<[Color]>)>;
    thread_local! {
        static CACHE: RefCell<Tints> = const { RefCell::new(None) };
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((f, t, v)) = c.as_ref()
            && *f == from
            && *t == to
            && v.len() == n
        {
            return v.clone();
        }
        let v: std::rc::Rc<[Color]> = (0..n).map(|i| from.mix(to, i as f32 / (n - 1).max(1) as f32)).collect();
        *c = Some((from, to, v.clone()));
        v
    })
}

fn rest_color(palette: &MandaraPalette) -> Color {
    wash(WASH_IDLE_BAR, palette)
}

/// Mirrored bars for `levels` (empty = resting baseline of `n` bars).
/// `mirror` draws the row right-to-left (levels and tint reversed), for the
/// left wing of the ring style.
#[allow(clippy::too_many_arguments)]
fn bars(levels: &[f32], n: usize, from: Color, to: Color, palette: &MandaraPalette, height: f32, gap: f32, mirror: bool) -> gpui::Div {
    let rest = rest_color(palette);
    let tints = tints(from, to, n);
    div().h(px(height)).flex_1().min_w(px(0.0)).flex().items_center().gap(px(gap)).children((0..n).map(move |i| {
        let k = if mirror { n - 1 - i } else { i };
        let (h, c) = match levels.get(k) {
            Some(v) => {
                let v = v.clamp(0.0, 1.0).powf(BAR_GAMMA);
                ((v * height).max(BAR_MIN_H_PX), tints[k].with_alpha(BAR_ALPHA.0 + BAR_ALPHA.1 * v))
            }
            None => (BAR_MIN_H_PX, rest),
        };
        div().flex_1().min_w(px(BAR_MIN_W_PX)).h(px(h)).rounded_full().bg(c.hsla())
    }))
}

/// A filled, mirrored wave: many slim columns following a spline through
/// the levels, over a wider, softer echo of itself.
fn wave(levels: &[f32], from: Color, to: Color, palette: &MandaraPalette, height: f32) -> AnyElement {
    let rest = rest_color(palette);
    let tints = tints(from, to, WAVE_COLS);
    let layer = |scale: f32, alpha: f32, shift: f32| {
        let tints = tints.clone();
        div().absolute().top_0().left_0().size_full().flex().items_center().children((0..WAVE_COLS).map(move |i| {
            let x = i as f32 / (WAVE_COLS - 1) as f32;
            let (h, c) = if levels.is_empty() {
                (WAVE_REST_H_PX, rest)
            } else {
                let last = (levels.len() - 1) as f32;
                let at = (x * last + shift).clamp(0.0, last);
                let v = spline(levels, at).powf(WAVE_GAMMA) * scale;
                ((v * height).max(WAVE_REST_H_PX), tints[i].with_alpha(alpha * (WAVE_ALPHA.0 + WAVE_ALPHA.1 * v)))
            };
            div().flex_1().h(px(h)).rounded(px(crate::style::radius(WAVE_COL_RADIUS_PX))).bg(c.hsla())
        }))
    };
    div()
        .h(px(height))
        .w_full()
        .relative()
        .when(!levels.is_empty(), |el| el.child(layer(WAVE_ECHO.0, WAVE_ECHO.1, WAVE_ECHO.2)))
        .child(layer(WAVE_FRONT_SCALE, 1.0, 0.0))
        .into_any_element()
}

/// Dots on a ring around the album art, bars spreading to both sides.
#[allow(clippy::too_many_arguments)]
fn ring(levels: &[f32], n: usize, from: Color, to: Color, palette: &MandaraPalette, height: f32, art: Option<ImageSource>) -> AnyElement {
    let d = height;
    let art_d = (d * RING_ART_FRAC).round();
    let dots = n.min(RING_MAX_DOTS);
    let ring_levels = resample(levels, dots);
    let (c0, r0) = (d / 2.0, art_d / 2.0 + RING_ART_GAP_PX);
    let rest = rest_color(palette);
    let mut core = div().relative().size(px(d)).flex_none().flex().items_center().justify_center();
    core = core.child(match art {
        Some(source) => img(source).size(px(art_d)).rounded_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
        None => div()
            .size(px(art_d))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(from.mix(to, 0.5).with_alpha(RING_PLACEHOLDER_ALPHA).hsla())
            .child(icon("music", art_d * RING_PLACEHOLDER_ICON_FRAC, to))
            .into_any_element(),
    });
    for i in 0..dots {
        let a = i as f32 / dots as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let (v, color) = match ring_levels.get(i) {
            Some(v) => {
                let v = v.clamp(0.0, 1.0);
                (v, from.mix(to, (a.sin() + 1.0) / 2.0).with_alpha(RING_ALPHA.0 + RING_ALPHA.1 * v))
            }
            None => (0.0, rest),
        };
        let r = r0 + v * (d / 2.0 - r0 - RING_MARGIN_PX);
        let s = RING_DOT_PX.0 + v * RING_DOT_PX.1;
        core = core.child(div().absolute().left(px(c0 + r * a.cos() - s / 2.0)).top(px(c0 + r * a.sin() - s / 2.0)).size(px(s)).rounded_full().bg(color.hsla()));
    }
    // Low frequencies sit next to the ring on both sides.
    let side = (n / 2).max(RING_SIDE_MIN_BARS);
    let wing = resample(levels, side);
    div()
        .h(px(height))
        .w_full()
        .flex()
        .items_center()
        .gap(px(RING_ROW_GAP_PX))
        .child(bars(&wing, side, from, to, palette, height * RING_SIDE_H_FRAC, RING_SIDE_GAP_PX, true))
        .child(core)
        .child(bars(&wing, side, from, to, palette, height * RING_SIDE_H_FRAC, RING_SIDE_GAP_PX, false))
        .into_any_element()
}

/// The media panel's visualizer in the configured style. `bands` are live
/// levels `0..=1` (empty = silence / paused / no capture); `art` is the
/// panel's decoded cover for the ring style. Levels are smoothed once per
/// frame with the real frame interval, into reused buffers.
#[allow(clippy::too_many_arguments)]
pub fn media_visualizer(bands: &[f32], art: Option<ImageSource>, from: Color, to: Color, palette: &MandaraPalette, height: f32, cx: &App) -> AnyElement {
    let settings = Settings::cached(&crate::state::config(cx).read(cx).snapshot());
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let Live { levels, scratch, last } = &mut *live;
        if bands.is_empty() {
            levels.clear();
            *last = None;
        } else {
            let now = Instant::now();
            let dt = last.map_or(1.0 / SMOOTHING_REFERENCE_HZ, |t| now.saturating_duration_since(t).as_secs_f32());
            *last = Some(now);
            resample_into(bands, settings.bars, scratch);
            smooth_dt(levels, scratch, settings.smoothing, dt);
        }
        draw_source(&settings, levels, art, from, to, palette, height)
    })
}

/// Draws already resampled / smoothed `levels` (empty = resting baseline)
/// in `settings.style`; `art` is the album cover for the ring.
pub fn draw(settings: &Settings, levels: &[f32], art: Option<PathBuf>, from: Color, to: Color, palette: &MandaraPalette, height: f32) -> AnyElement {
    draw_source(settings, levels, art.map(ImageSource::from), from, to, palette, height)
}

fn draw_source(settings: &Settings, levels: &[f32], art: Option<ImageSource>, from: Color, to: Color, palette: &MandaraPalette, height: f32) -> AnyElement {
    let n = settings.bars;
    match settings.style {
        Style::Bars => {
            let gap = if n > DENSE_BARS { DENSE_GAP_PX } else { SPARSE_GAP_PX };
            div().w_full().flex().child(bars(levels, n, from, to, palette, height, gap, false)).into_any_element()
        }
        Style::Wave => wave(levels, from, to, palette, height),
        Style::Ring => ring(levels, n, from, to, palette, height, art),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_endpoints_and_length() {
        let b = [0.0, 1.0, 0.0];
        let r = resample(&b, 5);
        assert_eq!(r.len(), 5);
        assert_eq!((r[0], r[2], r[4]), (0.0, 1.0, 0.0));
        assert!((r[1] - 0.5).abs() < 1e-6);
        assert!(resample(&[], 8).is_empty());
        assert_eq!(resample(&[0.4], 3), vec![0.4; 3]);
    }

    #[test]
    fn smoothing_attacks_fast_and_decays_slowly() {
        let mut s = vec![0.0; 2];
        smooth(&mut s, &[1.0, 1.0], 0.8);
        assert!(s[0] > 0.7, "rises quickly: {}", s[0]);
        smooth(&mut s, &[0.0, 0.0], 0.8);
        assert!(s[0] > 0.55, "falls slowly: {}", s[0]);
        let mut raw = vec![0.5];
        smooth(&mut raw, &[0.1], 0.0);
        assert_eq!(raw, vec![0.1], "no smoothing = raw levels");
        let mut fresh = Vec::new();
        smooth(&mut fresh, &[0.3, 0.2], 0.5);
        assert_eq!(fresh, vec![0.3, 0.2], "a new bar count starts from the signal");
    }

    #[test]
    fn smoothing_is_frame_rate_independent() {
        let run = |hz: f32| {
            let mut s = vec![1.0];
            for _ in 0..(hz as usize) {
                smooth_dt(&mut s, &[0.0], 0.8, 1.0 / hz);
            }
            s[0]
        };
        assert!((run(60.0) - run(144.0)).abs() < 1e-3, "one second of decay matches at 60 and 144 Hz");
    }

    #[test]
    fn bar_tints_are_cached_per_theme() {
        let (a, b) = (Color::hex("#ff0000"), Color::hex("#0000ff"));
        let t1 = tints(a, b, 8);
        assert!(std::rc::Rc::ptr_eq(&t1, &tints(a, b, 8)));
        assert_eq!(tints(a, b, 4).len(), 4);
    }

    #[test]
    fn spline_passes_through_points_and_stays_in_range() {
        let v = [0.0, 1.0, 0.2, 0.9];
        for (i, want) in v.iter().enumerate() {
            assert!((spline(&v, i as f32) - want).abs() < 1e-5);
        }
        for k in 0..30 {
            let y = spline(&v, k as f32 * 0.1);
            assert!((0.0..=1.0).contains(&y));
        }
    }

    #[test]
    fn settings_parse_with_defaults_and_clamps() {
        let mut cfg = ShellConfig::default();
        assert_eq!(Settings::from_config(&cfg), Settings::default());
        let media = cfg.plugins.settings.entry("media".into()).or_default();
        media.insert("visualizer_style".into(), toml::Value::String("ring".into()));
        media.insert("visualizer_bars".into(), toml::Value::Integer(500));
        media.insert("visualizer_smoothing".into(), toml::Value::Float(0.6));
        let s = Settings::from_config(&cfg);
        assert_eq!((s.style, s.bars), (Style::Ring, 64));
        assert!((s.smoothing - 0.6).abs() < 1e-6);
        assert_eq!(Style::parse("liquid-wave"), Style::Wave);
        assert_eq!(Style::parse("nonsense"), Style::Bars);
    }
}
