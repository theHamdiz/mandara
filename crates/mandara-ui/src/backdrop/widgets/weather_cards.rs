//! Forecast ribbons and dials built only from measured weather data.

use std::f32::consts::{FRAC_PI_2, TAU};
use mandara_config::desktop_widgets::{WeatherLayout, WeatherWidget};
use mandara_i18n::tk;

use super::Forecast;
use super::layout::{InkRole, Mark, MarkKind, Measure, WeatherStrings, face, fit, style};
use super::lingo::Lingo;

/// Reference geometry at 1080 px, scaled with the widget's placement.
const WIDTH: f32 = 380.0;
const PAD: f32 = 18.0;
const GAP: f32 = 12.0;
const ICON: f32 = 48.0;
const TEMP: f32 = 54.0;
const TEXT: f32 = 16.0;
const SMALL: f32 = 13.0;
const RULE: f32 = 1.0;
const TUBE_W: f32 = 24.0;
const TUBE_H: f32 = 144.0;
const BULB: f32 = 20.0;
const ROSE_R: f32 = 100.0;
const ROSE_INNER: f32 = 66.0;
const PETALS: usize = 24;
const PETAL_ANGLE: f32 = 0.32;
const MUTED: f32 = 0.16;

fn label(out: &mut Vec<Mark>, value: &str, w: &WeatherWidget, size: f32, x: f32, y: f32, width: f32, ink: InkRole, m: &mut dyn Measure) -> f32 {
    let font = style(w.look.font, size, face::MEDIUM);
    let value = fit(value, &font, width, m);
    let (tw, th) = m.measure(&value, &font);
    let rtl = Lingo::of(&w.locale, mandara_i18n::language().is_rtl(), None).rtl;
    out.push(Mark::new(MarkKind::Text { text: value, style: font, x: if rtl { x + width - tw } else { x }, y, w: tw, h: th }, ink));
    th
}

fn range_fraction(f: &Forecast) -> Option<f32> {
    let (lo, hi) = f.today.filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo <= hi)?;
    if !f.temp.is_finite() { return None; }
    Some(if lo == hi { 0.5 } else { ((f.temp - lo) / (hi - lo)).clamp(0.0, 1.0) })
}

pub fn layout(w: &WeatherWidget, s: &WeatherStrings, forecast: Option<&Forecast>, l: &Lingo, units: mandara_config::Units, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let mut out = Vec::new();
    let Some(f) = forecast else {
        label(&mut out, &s.condition, w, TEXT * k, 0.0, 0.0, WIDTH * k, InkRole::Fg, m);
        return out;
    };
    if w.layout == WeatherLayout::HumidityRose {
        let center = (ROSE_R * k, ROSE_R * k);
        for i in 0..PETALS {
            let angle = i as f32 / PETALS as f32 * TAU - FRAC_PI_2;
            let spread = TAU / PETALS as f32 * PETAL_ANGLE;
            let point = |r: f32, a: f32| (center.0 + r * k * a.cos(), center.1 + r * k * a.sin());
            let pts = vec![point(ROSE_INNER, angle), point((ROSE_R + ROSE_INNER) / 2.0, angle - spread), point(ROSE_R, angle), point((ROSE_R + ROSE_INNER) / 2.0, angle + spread)];
            let fraction = (f32::from(f.humidity.min(100)) / 100.0 * PETALS as f32 - i as f32).clamp(0.0, 1.0);
            out.push(Mark::new(MarkKind::Poly { pts }, InkRole::Accent).dim(MUTED + (1.0 - MUTED) * fraction));
        }
        let value = l.digits(format!("{}%", f.humidity.min(100)));
        let font = style(w.look.font, TEMP * k, face::MEDIUM);
        let (tw, th) = m.measure(&value, &font);
        out.push(Mark::new(MarkKind::Text { text: value, style: font, x: center.0 - tw / 2.0, y: center.1 - th / 2.0, w: tw, h: th }, InkRole::Fg));
        let title = l.word(tk("Humidity"));
        let font = style(w.look.font, TEXT * k, face::MEDIUM);
        let (tw, _) = m.measure(&title, &font);
        label(&mut out, &title, w, TEXT * k, center.0 - tw / 2.0, ROSE_R * 2.0 * k + GAP * k, tw, InkRole::Accent, m);
        return out;
    }
    if w.layout == WeatherLayout::Thermometer {
        let center = BULB * k;
        let tube_x = center - TUBE_W * k / 2.0;
        out.push(Mark::new(MarkKind::Rect { x: tube_x, y: 0.0, w: TUBE_W * k, h: TUBE_H * k, radius: TUBE_W * k / 2.0, width: RULE * k }, InkRole::Fg).dim(MUTED));
        if let Some(fraction) = range_fraction(f) {
            let height = TUBE_H * k * fraction;
            out.push(Mark::new(MarkKind::Rect { x: tube_x, y: TUBE_H * k - height, w: TUBE_W * k, h: height, radius: TUBE_W * k / 2.0, width: 0.0 }, InkRole::Accent));
            out.push(Mark::new(MarkKind::Disc { c: (center, TUBE_H * k), r: BULB * k }, InkRole::Accent));
        } else {
            out.push(Mark::new(MarkKind::Disc { c: (center, TUBE_H * k), r: BULB * k, }, InkRole::Fg).dim(MUTED));
        }
        let x = (BULB * 2.0 + GAP) * k;
        let width = WIDTH * k - x;
        let h = label(&mut out, &s.temp, w, TEMP * k, x, PAD * k, width, InkRole::Fg, m);
        label(&mut out, &s.condition, w, TEXT * k, x, PAD * k + h + GAP * k, width, InkRole::Accent, m);
        if let Some((lo, hi)) = f.today.filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo <= hi) {
            let range = l.digits(format!("{} / {}", units.format_degrees(hi), units.format_degrees(lo)));
            label(&mut out, &range, w, TEXT * k, x, (TUBE_H - TEXT) * k, width, InkRole::Fg, m);
        }
        return out;
    }
    out.push(Mark::new(MarkKind::Icon { name: s.icon, x: 0.0, y: 0.0, size: ICON * k }, InkRole::Accent));
    let x = (ICON + GAP) * k;
    let h = label(&mut out, &s.temp, w, TEMP * k, x, 0.0, (WIDTH - ICON - GAP) * k, InkRole::Fg, m);
    let info_h = label(&mut out, &s.condition, w, TEXT * k, x, h, (WIDTH - ICON - GAP) * k, InkRole::Accent, m);
    let mut y = h + info_h + GAP * k;
    if !s.place.is_empty() {
        y += label(&mut out, &s.place, w, TEXT * k, 0.0, y, WIDTH * k, InkRole::Fg, m) + GAP * k;
    }
    out.push(Mark::new(MarkKind::Line { a: (0.0, y), b: (WIDTH * k, y), width: RULE * k }, InkRole::Fg).dim(MUTED));
    let count = s.days.len().min(4);
    if count > 0 {
        let cell = WIDTH * k / count as f32;
        for (i, (day, icon, temperature)) in s.days.iter().take(count).enumerate() {
            let col = if s.rtl { count - 1 - i } else { i };
            let x = col as f32 * cell;
            let th = label(&mut out, day, w, SMALL * k, x, y + GAP * k, cell - GAP * k, InkRole::Fg, m);
            out.push(Mark::new(MarkKind::Icon { name: icon, x, y: y + GAP * k + th, size: ICON * k / 2.0 }, InkRole::Accent));
            label(&mut out, temperature, w, SMALL * k, x, y + GAP * k + th + ICON * k / 2.0, cell - GAP * k, InkRole::Fg, m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layout::tests::Fixed;

    #[test]
    fn unavailable_weather_has_no_invented_dial_reading() {
        let l = Lingo::of(&Default::default(), false, None);
        for layout_id in [WeatherLayout::Ribbon, WeatherLayout::Thermometer, WeatherLayout::HumidityRose] {
            let w = WeatherWidget { layout: layout_id, ..Default::default() };
            let s = WeatherStrings { condition: "Weather unavailable".into(), ..Default::default() };
            let marks = layout(&w, &s, None, &l, mandara_config::Units::default(), 1.0, &mut Fixed);
            assert_eq!(marks.len(), 1);
            assert!(matches!(&marks[0].kind, MarkKind::Text { text, .. } if text == "Weather unavailable"));
        }
    }
}
