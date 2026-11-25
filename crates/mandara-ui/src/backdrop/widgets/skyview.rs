//! The sky widgets: the sun on its arc from sunrise to sunset (the moon on
//! it at night), tonight's moon, and a globe lit where the sun really is.
//! All from the time and your location ([`super::sky`]); they redraw with
//! the minute.

use std::f32::consts::{PI, TAU};

use chrono::{DateTime, Local, Utc};
use mandara_config::desktop_widgets::{SkyLayout, SkyWidget};
use mandara_config::widget_kinds::{ranges, within};
use mandara_i18n::tk;

use super::faces::moon;
use super::layout::{Composition, InkRole, Mark, MarkKind, Measure, Picture, caps, face, style, tracked};
use super::lingo::Lingo;
use super::sky;

/// Reference sizes and proportions.
const LABEL: f32 = 18.0;
const HEADLINE: f32 = 24.0;
const LINE_GAP: f32 = 10.0;
const PATH_W: f32 = 3.0;
const DASH: f32 = 0.035;
const SUN: f32 = 0.05;
const TICK: f32 = 8.0;
const HORIZON_ALPHA: f32 = 0.35;
const GLOW_ALPHA: f32 = 0.25;
const GLOW: f32 = 1.9;
const MOON_EARTHSHINE: f32 = 0.32;
/// Soft terminator: extra phases drawn faintly on either side.
const TERMINATOR_STEPS: usize = 4;
const TERMINATOR_SPREAD: f32 = 0.012;
const TERMINATOR_ALPHA: f32 = 0.22;
/// Globe: the mask's resolution at most, the graticule's spacing and line
/// width (degrees) and the softness of the terminator (sine of the sun's
/// altitude across which day fades into the night side, the plate).
const GLOBE_MAX_PX: u32 = 360;
const GRATICULE_DEG: f64 = 30.0;
const GRATICULE_LINE_DEG: f64 = 0.9;
const TWILIGHT: f64 = 0.12;
const GRID_ALPHA: f32 = 0.22;
/// The dimmest a lit point of the globe gets (where the sun is low).
const SHADE_FLOOR: f64 = 0.45;
const PLACE_DOT: f32 = 0.022;
const MOON_DOT: f32 = 0.045;
const HOURS_PER_DAY: i64 = 24;
const SECONDS_PER_HOUR: i64 = 3600;
const SECONDS_PER_MINUTE: i64 = 60;
const PERCENT: f32 = 100.0;
/// Steps of the arc path.
const ARC_STEPS: usize = 96;

/// `14:05` of a UTC instant in local time, in the widget's digits.
fn hm(t: DateTime<Utc>, l: &Lingo, h24: bool) -> String {
    let local = t.with_timezone(&Local);
    l.digits(if h24 { local.format("%H:%M").to_string() } else { local.format("%-I:%M").to_string() })
}

/// `2 h 13 min` until `secs`.
fn span(secs: i64, l: &Lingo) -> String {
    let (h, m) = (secs / SECONDS_PER_HOUR, secs % SECONDS_PER_HOUR / SECONDS_PER_MINUTE);
    let text = if h > 0 { l.word(tk("{hours} h {minutes} min")).replace("{hours}", &h.to_string()) } else { l.word(tk("{minutes} min")) };
    l.digits(text.replace("{minutes}", &m.to_string()))
}

/// A sky widget at `now`; `place` is the location (latitude, longitude),
/// `None` before one is set.
#[allow(clippy::too_many_arguments)]
pub fn sky_widget(w: &SkyWidget, place: Option<(f64, f64)>, now: &DateTime<Local>, l: &Lingo, h24: bool, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let size = within(w.size, ranges::SKY_SIZE_PX, ranges::SKY_SIZE_DEFAULT_PX) * k;
    let (lat, lon) = place.unwrap_or((0.0, 0.0));
    let marks = match w.layout {
        SkyLayout::SunArc => sun_arc(w, (lat, lon), now, l, h24, size, k, m),
        SkyLayout::Moon => moon_view(w, now, l, size, k, align, m),
        SkyLayout::Globe => globe(w, (lat, lon), place.is_some(), now, size),
    };
    Composition::of(marks)
}

/// The sun's path over the horizon, the sun on it (or tonight's moon), and
/// sunrise, sunset and what comes next.
#[allow(clippy::too_many_arguments)]
fn sun_arc(w: &SkyWidget, (lat, lon): (f64, f64), now: &DateTime<Local>, l: &Lingo, h24: bool, width: f32, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let height = width * within(w.arc_height, ranges::ARC_HEIGHT, ranges::ARC_HEIGHT_DEFAULT);
    let day = sky::sun_day(now.date_naive(), lat, lon);
    let t = now.with_timezone(&Utc);
    let (rise, set) = (day.rise, day.set);
    // Where along the path (0 at sunrise, 1 at sunset); at night the moon
    // walks it from sunset to the next sunrise.
    let (frac, daytime) = match (rise, set) {
        (Some(r), Some(s)) if t >= r && t <= s => (((t - r).num_seconds() as f32 / (s - r).num_seconds().max(1) as f32), true),
        (Some(r), Some(s)) => {
            let next_rise = if t < r { r } else { r + chrono::Duration::hours(HOURS_PER_DAY) };
            let last_set = if t > s { s } else { s - chrono::Duration::hours(HOURS_PER_DAY) };
            (((t - last_set).num_seconds() as f32 / (next_rise - last_set).num_seconds().max(1) as f32), false)
        }
        _ => (0.5, sky::sun_altitude(t.timestamp(), lat, lon) > 0.0),
    };
    let base = height + px(PATH_W);
    let at = |f: f32| {
        let a = PI - PI * f.clamp(0.0, 1.0);
        (width / 2.0 + width / 2.0 * a.cos(), base - height * a.sin())
    };
    let mut marks = vec![Mark::new(MarkKind::Line { a: (0.0, base), b: (width, base), width: 1.0 }, InkRole::Fg).dim(HORIZON_ALPHA)];
    let pts: Vec<(f32, f32)> = (0..=ARC_STEPS).map(|i| at(i as f32 / ARC_STEPS as f32)).collect();
    if w.dashed {
        let dash = (ARC_STEPS as f32 * DASH).max(1.0) as usize;
        for chunk in pts.chunks(dash * 2) {
            marks.push(Mark::new(MarkKind::Polyline { pts: chunk.iter().take(dash + 1).copied().collect(), width: px(PATH_W) }, InkRole::Fg).dim(face::DIM));
        }
    } else {
        marks.push(Mark::new(MarkKind::Polyline { pts: pts.clone(), width: px(PATH_W) }, InkRole::Fg).dim(face::DIM));
    }
    // The travelled part in the accent.
    let done: Vec<(f32, f32)> = pts.iter().copied().take(((ARC_STEPS as f32 * frac).round() as usize + 1).min(pts.len())).collect();
    if done.len() > 1 && daytime {
        marks.push(Mark::new(MarkKind::Polyline { pts: done, width: px(PATH_W) }, InkRole::Accent));
    }
    if w.ticks {
        for i in 1..12 {
            let p = at(i as f32 / 12.0);
            marks.push(Mark::new(MarkKind::Disc { c: p, r: px(TICK) / 4.0 }, InkRole::Fg).dim(face::DIM));
        }
    }
    let body = at(frac);
    let r = width * SUN;
    if daytime {
        marks.push(Mark::new(MarkKind::Disc { c: body, r: r * GLOW }, InkRole::Accent).dim(GLOW_ALPHA));
        marks.push(Mark::new(MarkKind::Disc { c: body, r }, InkRole::Accent));
    } else {
        marks.extend(moon(body, r, sky::moon_phase(t.timestamp()), InkRole::Fg));
    }
    if w.labels {
        let ls = style(w.look.font, px(LABEL), face::MEDIUM);
        let hs = style(w.look.font, px(HEADLINE), face::SEMIBOLD);
        let y = base + px(LINE_GAP);
        let (left, right) = match (rise, set) {
            (Some(r0), Some(s0)) => (format!("↑ {}", hm(r0, l, h24)), format!("{} ↓", hm(s0, l, h24))),
            _ => (String::new(), String::new()),
        };
        // The sky doesn't mirror: facing the equator the sun rises on the
        // left and sets on the right, in any language.
        let (lw, lh) = m.measure(&left, &ls);
        let (rw, _) = m.measure(&right, &ls);
        marks.push(Mark::new(MarkKind::Text { text: left, style: ls.clone(), x: 0.0, y, w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
        marks.push(Mark::new(MarkKind::Text { text: right, style: ls, x: width - rw, y, w: rw, h: lh }, InkRole::Fg).dim(face::DIM));
        let next = match (rise, set) {
            (Some(_), Some(s0)) if daytime => l.word(tk("Sunset in {time}")).replace("{time}", &span((s0 - t).num_seconds(), l)),
            (Some(r0), Some(_)) => {
                let next_rise = if t < r0 { r0 } else { r0 + chrono::Duration::hours(HOURS_PER_DAY) };
                l.word(tk("Sunrise in {time}")).replace("{time}", &span((next_rise - t).num_seconds(), l))
            }
            _ if daytime => l.word(tk("The sun doesn't set today")),
            _ => l.word(tk("The sun doesn't rise today")),
        };
        let (nw, nh) = m.measure(&next, &hs);
        marks.push(Mark::new(MarkKind::Text { text: next, style: hs, x: (width - nw) / 2.0, y: y + lh + px(LINE_GAP) / 2.0, w: nw, h: nh }, InkRole::Fg));
    }
    marks
}

/// Tonight's moon with its phase name and how much of it is lit.
fn moon_view(w: &SkyWidget, now: &DateTime<Local>, l: &Lingo, size: f32, k: f32, align: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let phase = sky::moon_phase(now.timestamp());
    let r = size / 2.0;
    let c = (r, r);
    let mut marks = moon(c, r, phase, InkRole::Fg);
    if w.shaded {
        // Earthshine on the dark side, and a soft terminator.
        marks[0].opacity = MOON_EARTHSHINE;
        for i in 1..=TERMINATOR_STEPS {
            for side in [-1.0, 1.0] {
                let p = (phase + side * TERMINATOR_SPREAD * i as f32).rem_euclid(1.0);
                if let Some(lit) = moon(c, r, p, InkRole::Fg).into_iter().nth(1) {
                    marks.push(lit.dim(TERMINATOR_ALPHA / i as f32));
                }
            }
        }
    }
    if w.labels {
        let name = caps(&l.word(sky::moon_name(phase)));
        let ns = tracked(style(w.look.font, px(LABEL), face::BOLD), HEADER_TRACKING);
        let (nw, nh) = m.measure(&name, &ns);
        let y = size + px(LINE_GAP);
        marks.push(Mark::new(MarkKind::Text { text: name, style: ns, x: (size - nw) * align, y, w: nw, h: nh }, InkRole::Accent));
        let lit = l.word(tk("{percent} lit")).replace("{percent}", &l.digits(format!("{}%", (sky::moon_lit(phase) * PERCENT).round() as i32)));
        let ls = style(w.look.font, px(LABEL), face::MEDIUM);
        let (lw, lh) = m.measure(&lit, &ls);
        marks.push(Mark::new(MarkKind::Text { text: lit, style: ls, x: (size - lw) * align, y: y + nh, w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
    }
    marks
}

/// Letter spacing of the moon's name.
const HEADER_TRACKING: f32 = 0.12;

/// A unit vector for latitude / longitude (degrees).
fn vec3(lat: f64, lon: f64) -> [f64; 3] {
    let (la, lo) = (lat.to_radians(), lon.to_radians());
    [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A mask picture of `side × side` whose alpha `f` gives for each point of
/// the visible hemisphere (`None` outside the disc).
fn sphere_mask(side: u32, key: u64, f: impl Fn(f64, f64) -> f32) -> Option<Picture> {
    let mut pm = tiny_skia::Pixmap::new(side, side)?;
    let n = f64::from(side);
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let (x, y) = ((i as u32 % side) as f64 + 0.5, (i as u32 / side) as f64 + 0.5);
        let (u, v) = (x / n * 2.0 - 1.0, 1.0 - y / n * 2.0);
        let d = u * u + v * v;
        if d > 1.0 {
            continue;
        }
        // Anti-aliased rim.
        let edge = ((1.0 - d.sqrt()) * n / 2.0).clamp(0.0, 1.0) as f32;
        let a = (f(u, v) * edge).clamp(0.0, 1.0);
        let a8 = (a * f32::from(u8::MAX)).round() as u8;
        if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(a8, a8, a8, a8) {
            *px = p;
        }
    }
    Some(Picture { key, pixmap: std::sync::Arc::new(pm) })
}

/// The Earth seen over your longitude: lit where the sun is now, night
/// shaded, a graticule, your place and the moon above the point it stands
/// over.
fn globe(w: &SkyWidget, (lat, lon): (f64, f64), has_place: bool, now: &DateTime<Local>, size: f32) -> Vec<Mark> {
    let t = now.timestamp();
    let side = (size.round() as u32).clamp(1, GLOBE_MAX_PX);
    // The view: centred on your longitude, tilted toward your latitude.
    let (vlat, vlon) = (lat.clamp(-60.0, 60.0) / 2.0, lon);
    let (cl, sl) = (vlat.to_radians().cos(), vlat.to_radians().sin());
    let to_world = |u: f64, v: f64| {
        let z = (1.0 - u * u - v * v).max(0.0).sqrt();
        // View frame: x right, y up, z toward the viewer; tilt by the
        // view latitude, then turn to the view longitude.
        let (x, y, zz) = (u, v * cl + z * sl, -v * sl + z * cl);
        let (s, c) = (vlon.to_radians().sin(), vlon.to_radians().cos());
        [zz * c - x * s, zz * s + x * c, y]
    };
    let (sun_lat, sun_lon) = sky::subsolar(t);
    let sun = vec3(sun_lat, sun_lon);
    let key = super::key(("globe", side, t / SECONDS_PER_MINUTE, (lat * PERCENT_F).round() as i64, (lon * PERCENT_F).round() as i64));
    let mut marks = Vec::new();
    let draw = |m: Option<Picture>, ink: InkRole, alpha: f32| m.map(|mask| Mark::new(MarkKind::Mask { x: 0.0, y: 0.0, w: size, h: size, mask }, ink).dim(alpha));
    marks.extend(draw(sphere_mask(side, key ^ 1, |_, _| 1.0), InkRole::Plate, 1.0));
    marks.extend(draw(
        sphere_mask(side, key ^ 2, |u, v| {
            let p = to_world(u, v);
            let light = dot(p, sun);
            // Day fades out across the twilight band, and is shaded like a
            // lit ball: brightest under the sun, dimmer toward the edge.
            let day = ((light + TWILIGHT) / (2.0 * TWILIGHT)).clamp(0.0, 1.0);
            (day * (SHADE_FLOOR + (1.0 - SHADE_FLOOR) * light.max(0.0))) as f32
        }),
        InkRole::Accent,
        1.0,
    ));
    marks.extend(draw(
        sphere_mask(side, key ^ 3, |u, v| {
            let p = to_world(u, v);
            let la = p[2].asin().to_degrees();
            let lo = p[1].atan2(p[0]).to_degrees();
            let near = |deg: f64| {
                let r = deg.rem_euclid(GRATICULE_DEG);
                r.min(GRATICULE_DEG - r)
            };
            // Meridians thin out toward the poles: compare along the ground.
            let lo_dist = near(lo) * la.to_radians().cos();
            if near(la) < GRATICULE_LINE_DEG || lo_dist < GRATICULE_LINE_DEG { 1.0 } else { 0.0 }
        }),
        InkRole::Fg,
        GRID_ALPHA,
    ));
    let project = |la: f64, lo: f64| -> Option<(f32, f32)> {
        let p = vec3(la, lo);
        // Inverse of `to_world`.
        let (s, c) = (vlon.to_radians().sin(), vlon.to_radians().cos());
        let (zz, x) = (p[0] * c + p[1] * s, -p[0] * s + p[1] * c);
        let y = p[2];
        let (v, z) = (y * cl - zz * sl, y * sl + zz * cl);
        (z > 0.0).then(|| (((x + 1.0) / 2.0) as f32 * size, ((1.0 - v) / 2.0) as f32 * size))
    };
    if has_place && let Some(c) = project(lat, lon) {
        marks.push(Mark::new(MarkKind::Disc { c, r: size * PLACE_DOT * 1.8 }, InkRole::Fg).dim(GLOW_ALPHA));
        marks.push(Mark::new(MarkKind::Disc { c, r: size * PLACE_DOT }, InkRole::Fg));
    }
    if w.moon {
        let (mla, mlo) = sky::sublunar(t);
        if let Some(c) = project(mla, mlo) {
            marks.extend(moon(c, size * MOON_DOT, sky::moon_phase(t), InkRole::OnAccent));
        }
    }
    // The plane covers the whole disc (a moon at the limb stays inside).
    marks.push(Mark::new(MarkKind::Arc { c: (size / 2.0, size / 2.0), r: size / 2.0, width: 1.0, start: 0.0, sweep: TAU }, InkRole::Fg).dim(HORIZON_ALPHA));
    marks
}

/// Coordinates are keyed to a hundredth of a degree.
const PERCENT_F: f64 = 100.0;

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn every_sky_lays_out_by_day_and_night() {
        let l = Lingo::of(&Default::default(), false, None);
        for hour in [3, 12, 22] {
            let now = Local.with_ymd_and_hms(2026, 6, 15, hour, 30, 0).unwrap();
            for layout in [SkyLayout::SunArc, SkyLayout::Moon, SkyLayout::Globe] {
                let w = SkyWidget { layout, size: 200.0, ..Default::default() };
                let c = sky_widget(&w, Some((31.2, 29.9)), &now, &l, true, 1.0, 0.5, &mut Fixed);
                assert!(c.size.0 >= 150.0 && c.size.1 >= 50.0, "{layout:?} {hour}: {:?}", c.size);
            }
        }
    }

    #[test]
    fn the_globe_is_lit_on_the_suns_side() {
        let mask = sphere_mask(64, 0, |u, _| if u > 0.0 { 1.0 } else { 0.0 }).unwrap();
        let px = |x: u32, y: u32| mask.pixmap.pixel(x, y).unwrap().alpha();
        assert!(px(48, 32) > 200 && px(16, 32) == 0 && px(0, 0) == 0);
    }
}
