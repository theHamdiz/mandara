//! What plays, painted on the desktop: a turning record, a pocket player
//! with an LCD, the cover cut to a shape per track, a tall side pill, a
//! halftone cover, and synced lyrics. Shown only while a player is loaded;
//! the record turns and the progress moves only while music plays and the
//! desktop shows.

use std::f32::consts::TAU;

use chrono::{DateTime, Local};
use mandara_config::desktop_widgets::{MediaLayout, MediaWidget, PlateShape};
use mandara_config::widget_kinds::{count, ranges, within};
use mandara_core::shape::Shape;
use mandara_i18n::tk;

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, Picture, caps, face, fit, reads_rtl, style, tracked};
use super::lingo::Lingo;
use super::MediaInfo;

/// Frames per second of the turning record.
pub const VINYL_FPS: u32 = 20;
/// Degrees per second of a record at 33⅓ rpm.
const RPM_DEG_PER_S: f32 = 200.0;
/// Reference sizes and proportions.
const TITLE: f32 = 22.0;
const ARTIST: f32 = 16.0;
const GAP: f32 = 18.0;
const TEXT_W: f32 = 260.0;
const BUTTON: f32 = 40.0;
const BUTTON_GAP: f32 = 10.0;
const PROGRESS_H: f32 = 4.0;
const LABEL_SIZE: f32 = 0.36;
const SPINDLE: f32 = 0.025;
const GROOVE_ALPHA: f32 = 0.22;
const SHEEN_ALPHA: f32 = 0.12;
/// The record's vinyl: near black, faintly warm.
const VINYL: (f32, f32, f32) = (0.16, 0.01, 60.0);
const LCD_PAD: f32 = 14.0;
const LCD_H: f32 = 0.42;
const BODY_ASPECT: f32 = 1.3;
const PILL_W: f32 = 0.34;
const HALFTONE_MAX: f32 = 0.95;
const LYRIC: f32 = 22.0;
const LYRIC_W: f32 = 520.0;
const LYRIC_DIM: f32 = 0.42;
const TRACK_ALPHA: f32 = 0.22;
const TRACKING: f32 = 0.08;
const MS_PER_S: f32 = 1000.0;
/// One-bit dither threshold matrix (Bayer 4 × 4).
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
const BAYER_LEVELS: f32 = 16.0;

/// Milliseconds into the track at `now`.
pub fn position_ms(m: &MediaInfo, now: &DateTime<Local>) -> i64 {
    if m.playing { (m.position_ms + ((now.timestamp_millis() - m.at_ms) as f64 * m.rate) as i64).clamp(0, m.length_ms.max(0).max(m.position_ms)) } else { m.position_ms }
}

fn mmss(ms: i64, l: &Lingo) -> String {
    let s = ms.max(0) / 1000;
    l.digits(format!("{}:{:02}", s / 60, s % 60))
}

/// Play / pause, previous, next as round buttons from `x` (left to
/// right whatever the language: transport stays as on a device).
#[allow(clippy::too_many_arguments)]
fn transport(x: f32, y: f32, k: f32, playing: bool, marks: &mut Vec<Mark>, hits: &mut Vec<Hit>, filled: InkRole, on: InkRole, ink: InkRole) {
    let b = BUTTON * k;
    for (i, (icon, verb)) in [("previous", "previous"), (if playing { "pause" } else { "play" }, "play-pause"), ("next", "next")].into_iter().enumerate() {
        let bx = x + i as f32 * (b + BUTTON_GAP * k);
        let main = i == 1;
        marks.push(Mark::new(MarkKind::Disc { c: (bx + b / 2.0, y + b / 2.0), r: b / 2.0 }, if main { filled } else { ink }).dim(if main { 1.0 } else { TRACK_ALPHA / 2.0 }));
        marks.push(Mark::new(MarkKind::Icon { name: icon, x: bx + b * 0.28, y: y + b * 0.28, size: b * 0.44 }, if main { on } else { ink }));
        hits.push(Hit { rect: (bx, y, b, b), action: Action::Media(verb.into()) });
    }
}

/// Title and artist lines from `(x, y)`, `w` wide; returns the height.
fn titles(m: &MediaInfo, w: &MediaWidget, x: f32, y: f32, width: f32, k: f32, marks: &mut Vec<Mark>, meas: &mut dyn Measure) -> f32 {
    let ts = style(w.look.font, TITLE * k, face::SEMIBOLD);
    let as_ = style(w.look.font, ARTIST * k, face::MEDIUM);
    let title = fit(&m.title, &ts, width, meas);
    let artist = fit(&m.artist, &as_, width, meas);
    let rtl = reads_rtl(&m.title).unwrap_or(false);
    let (tw, th) = meas.measure(&title, &ts);
    let (aw, ah) = meas.measure(&artist, &as_);
    marks.push(Mark::new(MarkKind::Text { text: title, style: ts, x: if rtl { x + width - tw } else { x }, y, w: tw, h: th }, InkRole::Fg));
    marks.push(Mark::new(MarkKind::Text { text: artist, style: as_, x: if reads_rtl(&m.artist).unwrap_or(rtl) { x + width - aw } else { x }, y: y + th, w: aw, h: ah }, InkRole::Fg).dim(face::DIM));
    th + ah
}

/// A thin progress line, live (it moves each second while playing).
fn progress(m: &MediaInfo, now: &DateTime<Local>, x: f32, y: f32, width: f32, k: f32, ink: InkRole, marks: &mut Vec<Mark>) {
    let h = PROGRESS_H * k;
    marks.push(Mark::new(MarkKind::Rect { x, y, w: width, h, radius: h / 2.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA).live());
    let f = if m.length_ms > 0 { position_ms(m, now) as f32 / m.length_ms as f32 } else { 0.0 };
    if f > 0.0 {
        marks.push(Mark::new(MarkKind::Rect { x, y, w: (width * f.clamp(0.0, 1.0)).max(h), h, radius: h / 2.0, width: 0.0 }, ink).live());
    }
}

/// The cover, or a disc in the accent with a note when there's none.
fn cover(art: Option<&Picture>, x: f32, y: f32, s: f32, shape: Shape, turn: f32, marks: &mut Vec<Mark>) {
    match art {
        Some(p) => marks.push(Mark::new(MarkKind::Image { x, y, w: s, h: s, pic: p.clone(), shape, turn }, InkRole::Fg)),
        None => {
            marks.push(Mark::new(MarkKind::Shaped { x, y, w: s, h: s, shape, width: 0.0 }, InkRole::Accent));
            marks.push(Mark::new(MarkKind::Icon { name: "music", x: x + s * 0.3, y: y + s * 0.3, size: s * 0.4 }, InkRole::OnAccent));
        }
    }
}

/// A media widget at `now` (nothing while no player is loaded).
pub fn media(w: &MediaWidget, m: Option<&MediaInfo>, now: &DateTime<Local>, l: &Lingo, k: f32, meas: &mut dyn Measure) -> Composition {
    let Some(m) = m else { return Composition::default() };
    let s = within(w.size, ranges::MEDIA_SIZE_PX, ranges::MEDIA_SIZE_DEFAULT_PX) * k;
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    match w.layout {
        MediaLayout::Vinyl => {
            let c = (s / 2.0, s / 2.0);
            let rgb = {
                let [r, g, b, _] = mandara_theme::Color::oklch(VINYL.0, VINYL.1, VINYL.2).to_rgba8();
                (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
            };
            marks.push(Mark::new(MarkKind::Disc { c, r: s / 2.0 }, InkRole::Rgb(rgb)));
            let label = s * LABEL_SIZE;
            for i in 0..count(w.grooves, ranges::GROOVES) {
                let r = label / 2.0 + (s / 2.0 - label / 2.0) * (i as f32 + 0.5) / w.grooves.max(1) as f32;
                marks.push(Mark::new(MarkKind::Arc { c, r, width: 1.0, start: 0.0, sweep: TAU }, InkRole::Paper).dim(GROOVE_ALPHA * 0.5));
            }
            marks.push(Mark::new(MarkKind::Arc { c, r: s * 0.36, width: s * 0.1, start: -2.4, sweep: 0.9 }, InkRole::Paper).dim(SHEEN_ALPHA));
            let turn = if w.spin && m.playing { (now.timestamp_millis() % 1_000_000) as f32 / MS_PER_S * RPM_DEG_PER_S % 360.0 } else { 0.0 };
            let mut label_marks = Vec::new();
            cover(m.art.as_ref(), c.0 - label / 2.0, c.1 - label / 2.0, label, Shape::Circle, turn, &mut label_marks);
            for mk in label_marks {
                // The label turns with the record (its own plane).
                marks.push(if w.spin && m.playing { mk.live() } else { mk });
            }
            marks.push(Mark::new(MarkKind::Disc { c, r: s * SPINDLE }, InkRole::Rgb(rgb)).live());
            let tx = s + GAP * k;
            let h = titles(m, w, tx, s * 0.2, TEXT_W * k, k, &mut marks, meas);
            progress(m, now, tx, s * 0.2 + h + GAP * k / 2.0, TEXT_W * k, k, InkRole::Accent, &mut marks);
            if w.controls {
                transport(tx, s * 0.2 + h + GAP * k, k, m.playing, &mut marks, &mut hits, InkRole::Accent, InkRole::OnAccent, InkRole::Fg);
            }
        }
        MediaLayout::Device => {
            let (bw, bh) = (s, s * BODY_ASPECT);
            let shape = match shape_of(w.look.plate_shape, &w.look, k) {
                Shape::Rect { radius } => Shape::Rect { radius: radius.max(bw * 0.12) },
                sh => sh,
            };
            marks.push(Mark::new(MarkKind::Shaped { x: 0.0, y: 0.0, w: bw, h: bh, shape, width: 0.0 }, InkRole::Plate));
            marks.push(Mark::new(MarkKind::Shaped { x: 0.0, y: 0.0, w: bw, h: bh, shape, width: 2.0 * k }, InkRole::OnPlate).dim(TRACK_ALPHA));
            let pad = LCD_PAD * k;
            let (lx, ly, lw, lh) = (pad, pad, bw - 2.0 * pad, bh * LCD_H);
            marks.push(Mark::new(MarkKind::Rect { x: lx, y: ly, w: lw, h: lh, radius: pad / 2.0, width: 0.0 }, InkRole::Accent).dim(face::DIM));
            let lcd = tracked(style(mandara_config::desktop_widgets::FontRole::Mono, ARTIST * k, face::BOLD), TRACKING);
            let title = caps(&fit(&m.title, &lcd, lw - pad, meas));
            let (tw, th) = meas.measure(&title, &lcd);
            marks.push(Mark::new(MarkKind::Text { text: title, style: lcd.clone(), x: lx + (lw - tw) / 2.0, y: ly + pad / 2.0, w: tw, h: th }, InkRole::OnAccent));
            let artist = caps(&fit(&m.artist, &lcd, lw - pad, meas));
            let (aw, ah) = meas.measure(&artist, &lcd);
            marks.push(Mark::new(MarkKind::Text { text: artist, style: lcd.clone(), x: lx + (lw - aw) / 2.0, y: ly + pad / 2.0 + th, w: aw, h: ah }, InkRole::OnAccent).dim(face::DIM));
            let time = mmss(position_ms(m, now), l);
            let big = style(mandara_config::desktop_widgets::FontRole::Mono, TITLE * k * 1.6, face::BOLD);
            let (gw, gh) = meas.measure(&time, &big);
            marks.push(Mark::new(MarkKind::Text { text: time, style: big, x: lx + (lw - gw) / 2.0, y: ly + lh - gh - pad / 3.0, w: gw, h: gh }, InkRole::OnAccent).live());
            // The time's box is redrawn whole each second.
            marks.push(Mark::new(MarkKind::Rect { x: lx, y: ly + lh - gh - pad / 3.0, w: lw, h: gh, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0).live());
            if w.controls {
                let b = BUTTON * k;
                let total = 3.0 * b + 2.0 * BUTTON_GAP * k;
                transport((bw - total) / 2.0, ly + lh + (bh - ly - lh - b) / 2.0, k, m.playing, &mut marks, &mut hits, InkRole::Accent, InkRole::OnAccent, InkRole::OnPlate);
            }
        }
        MediaLayout::Shaped => {
            // One shape per track, picked by its title.
            let shapes: Vec<PlateShape> = if w.shapes.is_empty() { vec![PlateShape::Cookie] } else { w.shapes.clone() };
            let pick = shapes[(super::key(&m.title) % shapes.len() as u64) as usize];
            let shape = shape_of(pick, &w.look, k);
            cover(m.art.as_ref(), 0.0, 0.0, s, shape, 0.0, &mut marks);
            let h = titles(m, w, 0.0, s + GAP * k, s, k, &mut marks, meas);
            progress(m, now, 0.0, s + GAP * k * 1.5 + h, s, k, InkRole::Accent, &mut marks);
            if w.controls {
                let b = BUTTON * k;
                transport((s - 3.0 * b - 2.0 * BUTTON_GAP * k) / 2.0, s + GAP * k * 2.0 + h, k, m.playing, &mut marks, &mut hits, InkRole::Accent, InkRole::OnAccent, InkRole::Fg);
            }
        }
        MediaLayout::SidePill => {
            let pw = s * PILL_W;
            let ph = s * 2.0;
            marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: pw, h: ph, radius: pw / 2.0, width: 0.0 }, InkRole::Plate));
            let art = pw * 0.8;
            cover(m.art.as_ref(), (pw - art) / 2.0, (pw - art) / 2.0, art, Shape::Circle, 0.0, &mut marks);
            let ts = style(w.look.font, ARTIST * k, face::SEMIBOLD);
            let text = fit(&format!("{} · {}", m.title, m.artist), &ts, ph - pw * 2.2, meas);
            let (tw, th) = meas.measure(&text, &ts);
            // Written down the pill, read with the head turned right.
            marks.push(Mark::new(MarkKind::SideText { text, style: ts, x: (pw - th) / 2.0, y: pw * 1.05, w: th, h: tw, clockwise: true }, InkRole::OnPlate));
            if w.controls {
                let b = pw * 0.7;
                let y = ph - pw * 0.15 - b;
                marks.push(Mark::new(MarkKind::Disc { c: (pw / 2.0, y + b / 2.0), r: b / 2.0 }, InkRole::Accent));
                marks.push(Mark::new(MarkKind::Icon { name: if m.playing { "pause" } else { "play" }, x: pw / 2.0 - b * 0.22, y: y + b * 0.28, size: b * 0.44 }, InkRole::OnAccent));
                hits.push(Hit { rect: (0.0, y, pw, b), action: Action::Media("play-pause".into()) });
            }
        }
        MediaLayout::Halftone => {
            if let Some(p) = m.art.as_ref() {
                halftone(p, s, within(w.dot_pitch, ranges::DOT_PITCH_PX, ranges::DOT_PITCH_DEFAULT_PX) * k, w.dither, &mut marks);
            } else {
                cover(None, 0.0, 0.0, s, Shape::Circle, 0.0, &mut marks);
            }
            let h = titles(m, w, 0.0, s + GAP * k, s.max(TEXT_W * k), k, &mut marks, meas);
            progress(m, now, 0.0, s + GAP * k * 1.5 + h, s, k, InkRole::Accent, &mut marks);
        }
        MediaLayout::Lyrics => lyrics(w, m, now, l, k, &mut marks, meas),
    }
    hits.push(Hit { rect: (0.0, 0.0, s, s), action: Action::Panel(crate::plugins::media::ID.into()) });
    // Buttons win over the cover's panel click.
    hits.rotate_right(1);
    Composition::with_hits(marks, hits)
}

/// The cover as a dot screen in the ink (darker art, bigger dots), or a
/// one-bit ordered dither.
fn halftone(p: &Picture, s: f32, pitch: f32, dither: bool, marks: &mut Vec<Mark>) {
    let src = &p.pixmap;
    let (iw, ih) = (src.width() as f32, src.height() as f32);
    let side = iw.min(ih);
    let (ox, oy) = ((iw - side) / 2.0, (ih - side) / 2.0);
    let lum = |x: f32, y: f32| {
        let px = src.pixel((ox + x / s * side).clamp(0.0, iw - 1.0) as u32, (oy + y / s * side).clamp(0.0, ih - 1.0) as u32);
        px.map_or(0.0, |c| {
            let c = c.demultiply();
            (0.2126 * f32::from(c.red()) + 0.7152 * f32::from(c.green()) + 0.0722 * f32::from(c.blue())) / f32::from(u8::MAX)
        })
    };
    if dither {
        let n = (s / pitch.max(1.0) * 2.0).round().max(8.0) as u32;
        let Some(mut pm) = tiny_skia::Pixmap::new(n, n) else { return };
        for y in 0..n {
            for x in 0..n {
                let v = lum((x as f32 + 0.5) * s / n as f32, (y as f32 + 0.5) * s / n as f32);
                let t = (f32::from(BAYER[(y % 4) as usize][(x % 4) as usize]) + 0.5) / BAYER_LEVELS;
                if v > t
                    && let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(u8::MAX, u8::MAX, u8::MAX, u8::MAX)
                {
                    pm.pixels_mut()[(y * n + x) as usize] = c;
                }
            }
        }
        marks.push(Mark::new(MarkKind::Mask { x: 0.0, y: 0.0, w: s, h: s, mask: Picture { key: p.key ^ u64::from(n), pixmap: std::sync::Arc::new(pm) } }, InkRole::Fg));
        return;
    }
    let n = (s / pitch).floor().max(1.0) as usize;
    for j in 0..n {
        for i in 0..n {
            let (x, y) = ((i as f32 + 0.5) * pitch, (j as f32 + 0.5) * pitch);
            let r = pitch / 2.0 * HALFTONE_MAX * lum(x, y).sqrt();
            if r > pitch * 0.06 {
                marks.push(Mark::new(MarkKind::Disc { c: (x, y), r }, InkRole::Fg));
            }
        }
    }
}

/// Synced lyrics around the current line.
fn lyrics(w: &MediaWidget, m: &MediaInfo, now: &DateTime<Local>, l: &Lingo, k: f32, marks: &mut Vec<Mark>, meas: &mut dyn Measure) {
    let width = LYRIC_W * k;
    let rows = count(w.lines, ranges::LYRIC_LINES) as usize;
    let st = style(w.look.font, LYRIC * k, face::MEDIUM);
    let cur_st = style(w.look.font, LYRIC * k * 1.15, face::BOLD);
    let line_h = meas.measure("Ag", &cur_st).1 * 1.2;
    // The whole column is redrawn as the song moves on.
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: line_h * rows as f32, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0).live());
    if m.lyrics.is_empty() {
        let text = l.word(if m.lyrics_loading { tk("Looking for lyrics…") } else { tk("No lyrics for this song") });
        let (tw, th) = meas.measure(&text, &st);
        marks.push(Mark::new(MarkKind::Text { text, style: st, x: 0.0, y: (line_h * rows as f32 - th) / 2.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM).live());
        return;
    }
    let pos = position_ms(m, now);
    let cur = m.lyrics.iter().rposition(|(t, _)| *t <= pos).unwrap_or(0);
    let first = cur.saturating_sub(rows / 2);
    for (row, (_, text)) in m.lyrics.iter().enumerate().skip(first).take(rows) {
        let is_cur = row == cur;
        let s = if is_cur { &cur_st } else { &st };
        let text = fit(text, s, width, meas);
        let (tw, th) = meas.measure(&text, s);
        let rtl = reads_rtl(&text).unwrap_or(l.rtl);
        let y = (row - first) as f32 * line_h + (line_h - th) / 2.0;
        marks.push(Mark::new(MarkKind::Text { text, style: s.clone(), x: if rtl { width - tw } else { 0.0 }, y, w: tw, h: th }, if is_cur { InkRole::Accent } else { InkRole::Fg }).dim(if is_cur { 1.0 } else { LYRIC_DIM }).live());
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn every_layout_lays_out_and_nothing_shows_without_a_player() {
        let now = Local.with_ymd_and_hms(2026, 3, 1, 14, 0, 0).unwrap();
        let l = Lingo::of(&Default::default(), false, None);
        assert!(media(&MediaWidget::default(), None, &now, &l, 1.0, &mut Fixed).marks.is_empty());
        let pic = Picture { key: 3, pixmap: std::sync::Arc::new(tiny_skia::Pixmap::new(16, 16).unwrap()) };
        let m = MediaInfo { title: "Corniche".into(), artist: "Demo".into(), playing: true, position_ms: 10_000, at_ms: now.timestamp_millis() - 2_000, rate: 1.0, length_ms: 60_000, art: Some(pic), lyrics: vec![(0, "one".into()), (11_000, "two".into()), (20_000, "three".into())], lyrics_loading: false };
        assert_eq!(position_ms(&m, &now), 12_000);
        for layout in [MediaLayout::Vinyl, MediaLayout::Device, MediaLayout::Shaped, MediaLayout::SidePill, MediaLayout::Halftone, MediaLayout::Lyrics] {
            for dither in [false, true] {
                let c = media(&MediaWidget { layout, dither, ..Default::default() }, Some(&m), &now, &l, 1.0, &mut Fixed);
                assert!(c.size.0 > 50.0 && c.size.1 > 50.0, "{layout:?}: {:?}", c.size);
            }
        }
        let c = media(&MediaWidget { layout: MediaLayout::Lyrics, ..Default::default() }, Some(&m), &now, &l, 1.0, &mut Fixed);
        assert!(c.marks.iter().any(|mk| mk.ink == InkRole::Accent && matches!(&mk.kind, MarkKind::Text { text, .. } if text == "two")));
    }
}
