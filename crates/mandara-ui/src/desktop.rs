//! Depth layer 2 of the wallpaper canvas: the typographic desktop clock
//! (Alexandria / Cairo / Inter, tabular numerals), a Gregorian + Hijri date
//! pill, a compact weather summary and a subtle audio ring around the clock
//! that breathes with the music — all tucked between the wallpaper and its
//! foreground subject mask ([`crate::depth`]).
//!
//! Layout and ink decisions are plain functions shared by the
//! GPUI layer (`MANDARA_WALLPAPER_RENDERER=gpui`), the software renderer
//! ([`crate::backdrop`], the default) and [`render_preview`], which composes
//! all three layers offscreen with the software renderer's painters
//! (`mandara msg wallpaper render-preview <png>`).

use std::path::PathBuf;

use chrono::{Local, NaiveDateTime, TimeDelta, Timelike};
use gpui::{AnyElement, App, BoxShadow, FontWeight, IntoElement, ParentElement, SharedString, Styled, canvas, div, point, prelude::FluentBuilder as _, px};
use image::RgbImage;
use mandara_config::desktop_widgets::Insets;
use mandara_config::{ClockStyle, ShellConfig, WallpaperConfig};
use mandara_core::ToGpui;
use mandara_services::spectrum::BANDS;
use mandara_theme::Color;

use crate::fonts::{Fonts, tabular_nums};
use crate::icons::icon;
use crate::widgets::clock::{arabic_numerals, localized};

// ── Layout ───────────────────────────────────────────────────────────────

/// Where everything sits, in logical pixels of the output.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClockLayout {
    pub center: (f32, f32),
    /// Digit font size.
    pub size: f32,
    pub line_h: f32,
    pub lines: usize,
    pub block_h: f32,
    pub ring_r: f32,
    /// Top edge of the date pill.
    pub pill_top: f32,
    /// Scale for the pill (1.0 on a 1080 px tall output).
    pub k: f32,
}

/// [`layout_in`] with nothing reserved: the clock stays on the output.
pub fn layout(w: f32, h: f32, cfg: &WallpaperConfig) -> ClockLayout {
    layout_in(w, h, cfg, Insets::default())
}

/// Where the clock goes on a `w × h` output, moved (never resized) so its
/// digits and date pill stay fully on the output and clear of `safe` (the
/// bar's exclusive zone): an offset or scale that would push it past an
/// edge pins it to that edge instead.
pub fn layout_in(w: f32, h: f32, cfg: &WallpaperConfig, safe: Insets) -> ClockLayout {
    let mut l = placed(w, h, cfg);
    let (dx0, dy0, dx1, dy1) = digit_box(&l, cfg.clock_style);
    let (x0, y0, x1, y1) = if cfg.show_date {
        let (px0, _, px1, py1) = pill_box(&l);
        (dx0.min(px0), dy0, dx1.max(px1), dy1.max(py1))
    } else {
        (dx0, dy0, dx1, dy1)
    };
    let dx = shift_into(x0, x1, safe.left, w - safe.right);
    let dy = shift_into(y0, y1, safe.top, h - safe.bottom);
    l.center = (l.center.0 + dx, l.center.1 + dy);
    l.pill_top += dy;
    l
}

/// How far the span `a..b` must move to lie inside `lo..hi` (centred on it
/// when it is longer).
fn shift_into(a: f32, b: f32, lo: f32, hi: f32) -> f32 {
    if b - a > hi - lo {
        lo + (hi - lo - (b - a)) / 2.0 - a
    } else if a < lo {
        lo - a
    } else if b > hi {
        hi - b
    } else {
        0.0
    }
}

/// The clock where its config puts it (may reach past the edges).
fn placed(w: f32, h: f32, cfg: &WallpaperConfig) -> ClockLayout {
    let base = h * cfg.clock_scale.clamp(0.3, 3.0);
    let (size, line_h, lines) = match cfg.clock_style {
        ClockStyle::Stacked => (0.19 * base, 0.19 * base * 0.84, 2),
        ClockStyle::Classic => (0.14 * base, 0.14 * base * 1.05, 1),
        ClockStyle::Arabic => (0.15 * base, 0.15 * base * 1.3, 1),
    };
    let block_h = line_h * lines as f32;
    let center = (cfg.clock_x.clamp(0.0, 1.0) * w, cfg.clock_y.clamp(0.0, 1.0) * h);
    let k = (base / 1080.0).clamp(0.55, 2.2);
    let ring_r = if lines == 2 { block_h * 0.62 } else { size * 1.35 };
    ClockLayout { center, size, line_h, lines, block_h, ring_r, pill_top: center.1 + block_h / 2.0 + 18.0 * k, k }
}

/// The digits' box `(x0, y0, x1, y1)` in logical pixels (approximate
/// widths: tabular digits are ~0.6 em).
pub fn digit_box(l: &ClockLayout, style: ClockStyle) -> (f32, f32, f32, f32) {
    let half_w = l.size * match style {
        ClockStyle::Stacked => 0.62,
        ClockStyle::Classic => 1.4,
        ClockStyle::Arabic => 1.25,
    };
    (l.center.0 - half_w, l.center.1 - l.block_h / 2.0, l.center.0 + half_w, l.center.1 + l.block_h / 2.0)
}

/// The clock's lines for `now`.
pub fn clock_lines(now: &NaiveDateTime, style: ClockStyle, h24: bool) -> Vec<String> {
    let hour = if h24 {
        format!("{:02}", now.hour())
    } else {
        let h = now.hour() % 12;
        let h = if h == 0 { 12 } else { h };
        // Stacked digits balance better as two.
        if style == ClockStyle::Stacked { format!("{h:02}") } else { h.to_string() }
    };
    let minute = format!("{:02}", now.minute());
    match style {
        ClockStyle::Stacked => vec![hour, minute],
        ClockStyle::Classic => vec![format!("{hour}:{minute}")],
        ClockStyle::Arabic => vec![arabic_numerals(&format!("{hour}:{minute}"))],
    }
}

// ── Ink ──────────────────────────────────────────────────────────────────

/// Clock colours chosen against what is behind the clock.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ink {
    pub fg: Color,
    /// Soft glow behind the block, opposite of `fg`, for legibility.
    pub halo: Color,
    pub accent: Color,
    pub light: bool,
}

/// Mean colour of the wallpaper around the clock (`thumb` is a small copy;
/// `center` and `half` are fractions of the frame).
pub fn region_tone(thumb: &RgbImage, center: (f32, f32), half: (f32, f32)) -> Color {
    let (w, h) = (thumb.width() as f32, thumb.height() as f32);
    let x0 = ((center.0 - half.0) * w).clamp(0.0, w - 1.0) as u32;
    let x1 = ((center.0 + half.0) * w).clamp(1.0, w) as u32;
    let y0 = ((center.1 - half.1) * h).clamp(0.0, h - 1.0) as u32;
    let y1 = ((center.1 + half.1) * h).clamp(1.0, h) as u32;
    let (mut sum, mut n) = ([0.0f64; 3], 0.0f64);
    for y in y0..y1.max(y0 + 1) {
        for x in x0..x1.max(x0 + 1) {
            let p = thumb.get_pixel(x.min(thumb.width() - 1), y.min(thumb.height() - 1)).0;
            sum = [sum[0] + p[0] as f64, sum[1] + p[1] as f64, sum[2] + p[2] as f64];
            n += 1.0;
        }
    }
    let avg = |c: f64| (c / n.max(1.0)).round() as u8;
    Color::from_rgba8(avg(sum[0]), avg(sum[1]), avg(sum[2]), 255)
}

/// Light ink over dark backdrops and vice versa, tinted toward the backdrop
/// hue; the accent (audio ring) comes from the theme's primary.
pub fn ink_for(tone: Option<Color>, primary: Color) -> Ink {
    let tone = tone.unwrap_or(Color::oklch(0.3, 0.02, 250.0));
    let hue = tone.hue();
    let tint = (tone.chroma() * 0.35).min(0.025);
    let light = tone.lightness() < 0.66;
    if light {
        Ink {
            fg: Color::oklch(0.97, tint, hue).with_alpha(0.95),
            halo: Color::oklch(0.12, tint, hue).with_alpha(0.16),
            accent: primary.with_lightness(primary.lightness().clamp(0.78, 0.88)),
            light,
        }
    } else {
        Ink {
            fg: Color::oklch(0.2, tint.max(0.015), hue).with_alpha(0.92),
            halo: Color::oklch(0.98, tint, hue).with_alpha(0.22),
            accent: primary.with_lightness(primary.lightness().clamp(0.38, 0.5)),
            light,
        }
    }
}

// ── Date & weather ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct DateLine {
    pub gregorian: String,
    /// Arabic `(day, month, year)` with Eastern Arabic digits.
    pub hijri_ar: (String, &'static str, String),
    /// e.g. `5 Jumada al-Ula 1448` (for the preview renderer).
    pub hijri_latin: String,
    /// `(icon, temperature, condition)`.
    pub weather: Option<(&'static str, String, String)>,
}

/// Today's date line. The Islamic day begins at Maghrib (prayer times from
/// the Prayer plugin's settings, including its Hijri correction).
pub fn date_line(cfg: &ShellConfig, show_weather: bool, cx: &App) -> DateLine {
    let now = Local::now().naive_local();
    let maghrib = crate::plugins::prayer::today(cx).get(mandara_services::prayer::Prayer::Maghrib);
    let offset: i64 = cfg.plugins.setting(crate::plugins::prayer::ID, "hijri_offset", 0_i64);
    let date = now.date() + TimeDelta::days(offset + (now.time() >= maghrib) as i64);
    let (y, m, d) = crate::plugins::prayer::hijri(date);
    let (latin, arabic) = crate::plugins::prayer::HIJRI_MONTHS[m.clamp(1, 12) - 1];
    let weather = show_weather
        .then(|| crate::hub::hub(cx).read(cx).weather.weather.clone())
        .flatten()
        .map(|w| (crate::plugins::weather::condition_icon(w.condition, w.is_day), cfg.units().format_degrees(w.temp), w.condition.label().to_string()));
    DateLine {
        gregorian: localized(&now, mandara_i18n::t("%A, %-d %B")),
        hijri_ar: (arabic_numerals(&d.to_string()), arabic, arabic_numerals(&y.to_string())),
        hijri_latin: format!("{d} {} {y}", mandara_i18n::tr(latin)),
        weather,
    }
}

// ── Live layer (GPUI) ────────────────────────────────────────────────────

/// Everything one frame of the clock layer needs.
pub struct ClockFrame {
    pub layout: ClockLayout,
    pub lines: Vec<String>,
    pub style: ClockStyle,
    pub ink: Ink,
    pub date: Option<DateLine>,
    /// Live spectrum (while music plays and the desktop is visible).
    pub bands: Option<[f32; BANDS]>,
    /// Ring visibility `0..=1` (fades in / out with the music).
    pub ring: f32,
    pub fonts: Fonts,
    pub width: f32,
}

/// Radius of the audio ring at angle fraction `t` (`0..1`, clockwise from
/// the top), mirrored left / right so bass sits at the top.
pub fn ring_radius(r: f32, bands: &[f32], t: f32, amp: f32) -> f32 {
    if bands.is_empty() {
        return r;
    }
    let u = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
    let f = u * (bands.len() - 1) as f32;
    let (i, frac) = (f.floor() as usize, f.fract());
    let v = bands[i] * (1.0 - frac) + bands[(i + 1).min(bands.len() - 1)] * frac;
    r + amp * v.clamp(0.0, 1.0).powf(1.3)
}

fn pill_dot(ink: &Ink) -> impl IntoElement {
    div().size(px(3.0)).rounded_full().bg(ink.fg.with_alpha(0.45).hsla())
}

fn date_pill(d: &DateLine, ink: &Ink, fonts: &Fonts, k: f32) -> AnyElement {
    let surface = if ink.light { Color::oklch(0.1, 0.0, 0.0).with_alpha(0.2) } else { Color::oklch(1.0, 0.0, 0.0).with_alpha(0.38) };
    div()
        .h(px(34.0 * k))
        .px(px(16.0 * k))
        .rounded_full()
        .bg(surface.hsla())
        .border_1()
        .border_color(ink.fg.with_alpha(0.14).hsla())
        .flex()
        .items_center()
        .gap(px(10.0 * k))
        .text_size(px(14.0 * k))
        .text_color(ink.fg.hsla())
        .font_family(fonts.ui.clone())
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular_nums())
        .whitespace_nowrap()
        .child(d.gregorian.clone())
        .child(pill_dot(ink))
        // Right-to-left by hand (day · month · year): no bidi surprises.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0 * k))
                .font_family(fonts.arabic.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .child(d.hijri_ar.2.clone())
                .child(d.hijri_ar.1)
                .child(d.hijri_ar.0.clone()),
        )
        .when_some(d.weather.clone(), |el, (icon_name, temp, label)| {
            el.child(pill_dot(ink)).child(div().flex().items_center().gap(px(6.0 * k)).child(icon(icon_name, 15.0 * k, ink.fg)).child(temp).child(div().opacity(0.75).child(label)))
        })
        .into_any_element()
}

/// The date pill's box `(x0, y0, x1, y1)` in logical pixels (its width
/// estimated: the live pill sizes to its text).
pub fn pill_box(l: &ClockLayout) -> (f32, f32, f32, f32) {
    let half = 230.0 * l.k;
    (l.center.0 - half, l.pill_top, l.center.0 + half, l.pill_top + 34.0 * l.k)
}

/// The clock layer (digits, halo, ring) filling its full-output parent,
/// and the date pill as a separate full-output layer so the caller can lift
/// it above the subject when the subject would hide it.
pub fn element(f: ClockFrame) -> (AnyElement, Option<AnyElement>) {
    let ClockFrame { layout: l, lines, style, ink, date, bands, ring, fonts, width } = f;
    let (cx0, cy0) = l.center;
    let family: SharedString = if style == ClockStyle::Arabic { fonts.arabic.clone() } else { fonts.display.clone() };
    let weight = match style {
        ClockStyle::Stacked => FontWeight::BOLD,
        ClockStyle::Classic => FontWeight::LIGHT,
        ClockStyle::Arabic => FontWeight::SEMIBOLD,
    };
    let box_w = width * 0.9;
    let accent = ink.accent;
    let ring_el = (ring > 0.01).then(|| {
        let bands = bands.map(|b| b.to_vec()).unwrap_or_default();
        canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let (ox, oy) = (f32::from(b.origin.x) + cx0, f32::from(b.origin.y) + cy0);
                let steps = 144;
                let circle = |r_of: &dyn Fn(f32) -> f32, width: f32| {
                    let mut pb = gpui::PathBuilder::stroke(px(width));
                    for i in 0..=steps {
                        let t = i as f32 / steps as f32;
                        let a = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                        let r = r_of(t);
                        let p = point(px(ox + r * a.cos()), px(oy + r * a.sin()));
                        if i == 0 { pb.move_to(p) } else { pb.line_to(p) }
                    }
                    pb.build().ok()
                };
                if let Some(path) = circle(&|_| l.ring_r, 1.0) {
                    window.paint_path(path, accent.with_alpha(0.16 * ring).hsla());
                }
                let amp = l.ring_r * 0.09;
                if let Some(path) = circle(&|t| ring_radius(l.ring_r, &bands, t, amp), 2.0) {
                    window.paint_path(path, accent.with_alpha(0.5 * ring).hsla());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    });
    let clock = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .when_some(ring_el, |el, r| el.child(r))
        // Legibility halo (shadow only; the box itself is transparent).
        .child(
            div()
                .absolute()
                .left(px(cx0 - l.size * 1.2))
                .top(px(cy0 - l.block_h * 0.42))
                .w(px(l.size * 2.4))
                .h(px(l.block_h * 0.84))
                .rounded(px(crate::style::radius(l.size)))
                .shadow(vec![BoxShadow { color: ink.halo.hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(l.size * 0.9), spread_radius: px(0.0), inset: false }]),
        )
        .child(
            div()
                .absolute()
                .left(px(cx0 - box_w / 2.0))
                .top(px(cy0 - l.block_h / 2.0))
                .w(px(box_w))
                .h(px(l.block_h))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .font_family(family)
                .font_weight(weight)
                .font_features(tabular_nums())
                .text_size(px(l.size))
                .line_height(px(l.line_h))
                .text_color(ink.fg.hsla())
                .whitespace_nowrap()
                .children(lines.into_iter().map(|s| div().child(s))),
        )
        .into_any_element();
    let pill = date.map(|d| {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(div().absolute().left(px(cx0 - box_w / 2.0)).top(px(l.pill_top)).w(px(box_w)).flex().justify_center().child(date_pill(&d, &ink, &fonts, l.k)))
            .into_any_element()
    });
    (clock, pill)
}

// ── Offscreen preview ────────────────────────────────────────────────────

/// Inputs for [`render_preview`] (gathered on the UI thread).
pub struct PreviewInput {
    pub wallpaper: PathBuf,
    pub out: PathBuf,
    /// Physical pixels, and the output scale they stand for.
    pub size: (u32, u32),
    pub scale: f32,
    pub cfg: WallpaperConfig,
    pub lines: Vec<String>,
    pub date: Option<DateLine>,
    pub fonts: Fonts,
    pub primary: Color,
    /// Desktop widgets drawn with the clock.
    pub widgets: crate::backdrop::widgets::WidgetScene,
}

/// Subject coverage of the date pill above which it is lifted over layer 3.
pub const PILL_LIFT: f32 = 0.2;

/// Composes the three depth layers offscreen into a PNG at `input.out`
/// with the live software renderer's painters ([`crate::backdrop`]), so
/// the preview is what the desktop shows. Returns a JSON summary.
pub fn render_preview(input: PreviewInput) -> Result<serde_json::Value, String> {
    crate::backdrop::render_preview(&input)
}

/// Parses `WIDTHxHEIGHT`.
pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let s = s.split_once('@').map_or(s, |(size, _)| size);
    let (a, b) = s.split_once(['x', 'X'])?;
    let (w, h) = (a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?);
    ((64..=8192).contains(&w) && (64..=8192).contains(&h)).then_some((w, h))
}

/// The scale in `WIDTHxHEIGHT@SCALE` (1 when absent or invalid).
pub fn parse_scale(s: &str) -> f32 {
    s.split_once('@').and_then(|(_, k)| k.trim().parse::<f32>().ok()).filter(|k| (0.5..=4.0).contains(k)).unwrap_or(1.0)
}

/// `~`-expanded output path for the preview.
pub fn preview_path(raw: &str) -> PathBuf {
    let p = mandara_config::paths::expand_tilde(raw);
    if p.extension().is_none() { p.with_extension("png") } else { p }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn clock_lines_follow_style_and_hour_cycle() {
        let t = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap().and_hms_opt(21, 5, 0).unwrap();
        assert_eq!(clock_lines(&t, ClockStyle::Stacked, true), ["21", "05"]);
        assert_eq!(clock_lines(&t, ClockStyle::Classic, false), ["9:05"]);
        assert_eq!(clock_lines(&t, ClockStyle::Arabic, true), ["٢١:٠٥"]);
        let midnight = t.date().and_hms_opt(0, 30, 0).unwrap();
        assert_eq!(clock_lines(&midnight, ClockStyle::Classic, false), ["12:30"]);
    }

    #[test]
    fn layout_scales_and_anchors() {
        let cfg = WallpaperConfig::default();
        let l = layout(1920.0, 1080.0, &cfg);
        assert_eq!(l.center, (960.0, 1080.0 * cfg.clock_y));
        assert!(l.pill_top > l.center.1 + l.block_h / 2.0);
        let big = layout(1920.0, 1080.0, &WallpaperConfig { clock_scale: 2.0, ..cfg.clone() });
        assert!((big.size / l.size - 2.0).abs() < 1e-4);
        let classic = layout(1920.0, 1080.0, &WallpaperConfig { clock_style: ClockStyle::Classic, ..cfg });
        assert_eq!(classic.lines, 1);
    }

    #[test]
    fn the_clock_stays_on_screen_and_clear_of_the_bar() {
        // An Eastern Arabic depth clock pinned near the top edge (as an
        // earlier look placed it): its digits must not reach under the bar.
        let cfg = WallpaperConfig { clock_style: ClockStyle::Arabic, clock_y: 0.07, show_date: true, ..WallpaperConfig::default() };
        for (w, h, zone) in [(1920.0, 1080.0, 40.0), (3840.0, 2160.0, 40.0), (1280.0, 720.0, 32.0)] {
            let safe = Insets { top: zone, ..Insets::default() };
            let l = layout_in(w, h, &cfg, safe);
            let (x0, y0, x1, y1) = digit_box(&l, cfg.clock_style);
            let (_, _, _, pill_bottom) = pill_box(&l);
            assert!(y0 >= zone - 1e-3, "{w}x{h}: digits start at {y0}, under the bar");
            assert!(x0 >= 0.0 && x1 <= w && y1 <= h && pill_bottom <= h, "{w}x{h}");
            assert!((l.pill_top - (l.center.1 + l.block_h / 2.0 + 18.0 * l.k)).abs() < 1e-3, "the pill moves with the digits");
        }
        // A clock already inside is left where it is.
        let centred = WallpaperConfig { clock_style: ClockStyle::Arabic, ..WallpaperConfig::default() };
        assert_eq!(layout_in(1920.0, 1080.0, &centred, Insets { top: 40.0, ..Insets::default() }), layout(1920.0, 1080.0, &centred));
        // The bottom edge holds it in too (date pill included).
        let low = WallpaperConfig { clock_y: 1.0, show_date: true, ..WallpaperConfig::default() };
        let l = layout(1920.0, 1080.0, &low);
        assert!((pill_box(&l).3 - 1080.0).abs() < 1e-3, "{:?}", pill_box(&l));
    }

    #[test]
    fn ink_contrasts_with_the_backdrop() {
        let primary = Color::oklch(0.6, 0.15, 200.0);
        let dark = ink_for(Some(Color::oklch(0.25, 0.05, 250.0)), primary);
        assert!(dark.light && dark.fg.lightness() > 0.9);
        let bright = ink_for(Some(Color::oklch(0.9, 0.03, 80.0)), primary);
        assert!(!bright.light && bright.fg.lightness() < 0.3);
        let thumb = RgbImage::from_fn(16, 16, |x, _| if x < 8 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) });
        assert!(region_tone(&thumb, (0.25, 0.5), (0.2, 0.4)).lightness() < 0.1);
        assert!(region_tone(&thumb, (0.75, 0.5), (0.2, 0.4)).lightness() > 0.9);
    }

    #[test]
    fn ring_breathes_with_bands() {
        assert_eq!(ring_radius(100.0, &[], 0.3, 10.0), 100.0);
        let bands = [1.0, 0.0];
        assert!((ring_radius(100.0, &bands, 0.0, 10.0) - 110.0).abs() < 1e-3, "bass at the top");
        assert!((ring_radius(100.0, &bands, 0.5, 10.0) - 100.0).abs() < 1e-3);
        assert!((ring_radius(100.0, &bands, 0.25, 10.0) - ring_radius(100.0, &bands, 0.75, 10.0)).abs() < 1e-3, "mirrored");
    }

    #[test]
    fn parses_sizes_and_paths() {
        assert_eq!(parse_size("2560x1440"), Some((2560, 1440)));
        assert_eq!(parse_size("10x10"), None);
        assert_eq!(parse_size("3840x2160@1.5"), Some((3840, 2160)));
        assert_eq!(parse_scale("3840x2160@1.5"), 1.5);
        assert_eq!(parse_scale("2560x1440"), 1.0);
        assert!(preview_path("/tmp/x").ends_with("x.png"));
    }
}
