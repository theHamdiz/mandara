//! You and your shortcuts on the desktop: a profile card, launch tiles for
//! apps, folders and links, quick toggles, volume and brightness sliders
//! and the theme's palette. Every tile, toggle and swatch takes a click.

use mandara_config::desktop_widgets::{PaletteWidget, ProfileWidget, ShortcutsWidget, SlidersWidget, TileLayout, TogglesWidget};
use mandara_config::widget_kinds::{count, ranges, within};
use mandara_core::shape::Shape;
use mandara_i18n::tk;

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, caps, face, fit, style, tracked};
use super::lingo::Lingo;
use super::{MeInfo, TileItem};

/// Reference sizes.
const AVATAR: f32 = 96.0;
const NAME: f32 = 28.0;
const SMALL: f32 = 16.0;
const GAP: f32 = 16.0;
const LINE_GAP: f32 = 4.0;
const PILL_PAD_X: f32 = 12.0;
const PILL_PAD_Y: f32 = 4.0;
const STATUS_DOT: f32 = 4.0;
const TILE: f32 = 72.0;
const TILE_ICON: f32 = 0.56;
const TILE_GAP: f32 = 14.0;
const TILE_LABEL: f32 = 13.0;
const LIST_ROW: f32 = 46.0;
const LIST_ICON: f32 = 30.0;
const LIST_TEXT: f32 = 17.0;
const LIST_W: f32 = 240.0;
const TOGGLE: f32 = 64.0;
const TOGGLE_ICON: f32 = 0.42;
const TRACK_ALPHA: f32 = 0.16;
const OFF_ALPHA: f32 = 0.14;
const SWATCH: f32 = 56.0;
const SWATCH_LABEL: f32 = 11.0;
const SLIDER_ICON: f32 = 0.5;
/// Click targets along a slider: one per this many percent.
const SLIDER_STEPS: usize = 20;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
/// Hue step between colourful tiles, degrees.
const HUE_STEP: u16 = 41;
const PERCENT: f32 = 100.0;
const TRACKING: f32 = 0.08;

/// `3 h 17 min` of uptime in `l`'s words and digits.
fn uptime(minutes: u64, l: &Lingo) -> String {
    let (d, h, m) = (minutes / (MINUTES_PER_HOUR * HOURS_PER_DAY), minutes / MINUTES_PER_HOUR % HOURS_PER_DAY, minutes % MINUTES_PER_HOUR);
    let text = match (d, h) {
        (0, 0) => l.word(tk("{minutes} min")),
        (0, _) => l.word(tk("{hours} h {minutes} min")),
        _ => l.word(tk("{days} d {hours} h")),
    };
    l.digits(text.replace("{days}", &d.to_string()).replace("{hours}", &h.to_string()).replace("{minutes}", &m.to_string()))
}

/// The profile card: picture (or initials) in its shape, name, login,
/// uptime and a status pill.
pub fn profile(w: &ProfileWidget, me: &MeInfo, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let a = px(AVATAR);
    let shape = match shape_of(w.avatar, &w.look, k) {
        Shape::Rect { radius } => Shape::Rect { radius: radius.min(a / 3.0) },
        s => s,
    };
    let mut marks = Vec::new();
    let mut col = Vec::new();
    let ns = style(w.look.font, px(NAME), face::SEMIBOLD);
    let (nw, nh) = m.measure(&me.name, &ns);
    col.push((Mark::new(MarkKind::Text { text: me.name.clone(), style: ns, x: 0.0, y: 0.0, w: nw, h: nh }, InkRole::Fg), nw, nh));
    let ss = style(w.look.font, px(SMALL), face::MEDIUM);
    if w.login && !me.login.is_empty() {
        let t = format!("{}@{}", me.login, me.host);
        let (tw, th) = m.measure(&t, &ss);
        col.push((Mark::new(MarkKind::Text { text: t, style: ss.clone(), x: 0.0, y: 0.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM), tw, th));
    }
    if w.uptime {
        let t = l.word(tk("Up {time}")).replace("{time}", &uptime(me.uptime_s / SECONDS_PER_MINUTE, l));
        let (tw, th) = m.measure(&t, &ss);
        col.push((Mark::new(MarkKind::Text { text: t, style: ss.clone(), x: 0.0, y: 0.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM), tw, th));
    }
    let col_w = col.iter().map(|c| c.1).fold(0.0, f32::max);
    let mut y = 0.0;
    let text_x = if l.rtl { 0.0 } else { a + px(GAP) };
    let avatar_x = if l.rtl { col_w + px(GAP) } else { 0.0 };
    for (mut mk, cw, ch) in col {
        mk.shift(text_x + if l.rtl { col_w - cw } else { 0.0 }, y);
        marks.push(mk);
        y += ch + px(LINE_GAP);
    }
    if w.show_status {
        let (text, ink) = match (w.status.trim(), me.dnd) {
            ("", true) => (l.word(tk("Do not disturb")), InkRole::Fg),
            ("", false) => (l.word(tk("Online")), InkRole::Accent),
            (own, _) => (own.to_string(), InkRole::Accent),
        };
        let ps = tracked(style(w.look.font, px(SMALL) * 0.85, face::BOLD), TRACKING);
        let text = caps(&text);
        let (tw, th) = m.measure(&text, &ps);
        let (pw, ph) = (tw + 2.0 * px(PILL_PAD_X) + 3.0 * px(STATUS_DOT), th + 2.0 * px(PILL_PAD_Y));
        let px0 = text_x + if l.rtl { col_w - pw } else { 0.0 };
        let py = y + px(LINE_GAP);
        marks.push(Mark::new(MarkKind::Rect { x: px0, y: py, w: pw, h: ph, radius: ph / 2.0, width: 0.0 }, ink).dim(TRACK_ALPHA * 1.5));
        let dot_x = if l.rtl { px0 + pw - px(PILL_PAD_X) } else { px0 + px(PILL_PAD_X) };
        marks.push(Mark::new(MarkKind::Disc { c: (dot_x, py + ph / 2.0), r: px(STATUS_DOT) }, ink));
        let tx = if l.rtl { px0 + px(PILL_PAD_X) } else { px0 + px(PILL_PAD_X) + 3.0 * px(STATUS_DOT) };
        marks.push(Mark::new(MarkKind::Text { text, style: ps, x: tx, y: py + px(PILL_PAD_Y), w: tw, h: th }, ink));
        y = py + ph;
    }
    // The picture and the text are centred on each other.
    if a > y {
        for mk in &mut marks {
            mk.shift(0.0, (a - y) / 2.0);
        }
    }
    let ay = ((y - a) / 2.0).max(0.0);
    match &me.avatar {
        Some(pic) => marks.push(Mark::new(MarkKind::Image { x: avatar_x, y: ay, w: a, h: a, pic: pic.clone(), shape, turn: 0.0 }, InkRole::Fg)),
        None => {
            marks.push(Mark::new(MarkKind::Shaped { x: avatar_x, y: ay, w: a, h: a, shape, width: 0.0 }, InkRole::Accent));
            let initials = mandara_services::accounts::initials(&me.name);
            let is = style(w.look.font, a * 0.4, face::BOLD);
            let (iw, ih) = m.measure(&initials, &is);
            marks.push(Mark::new(MarkKind::Text { text: initials, style: is, x: avatar_x + (a - iw) / 2.0, y: ay + (a - ih) / 2.0, w: iw, h: ih }, InkRole::OnAccent));
        }
    }
    Composition::of(marks)
}

/// Launch tiles in a grid (icon in a shaped tile, name under) or a list.
pub fn shortcuts(w: &ShortcutsWidget, tiles: &[TileItem], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    if tiles.is_empty() {
        let text = l.word(tk("No shortcuts yet"));
        let st = style(w.look.font, px(LIST_TEXT), face::MEDIUM);
        let (tw, th) = m.measure(&text, &st);
        marks.push(Mark::new(MarkKind::Text { text, style: st, x: 0.0, y: 0.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM));
        return Composition::of(marks);
    }
    let ink = |i: usize| if w.colorful { InkRole::Pastel((i as u16 * HUE_STEP) % 360) } else { InkRole::Plate };
    let on_ink = if w.colorful { InkRole::OnPaper } else { InkRole::OnPlate };
    let draw_icon = |marks: &mut Vec<Mark>, t: &TileItem, x: f32, y: f32, s: f32| match &t.icon {
        Some(pic) => marks.push(Mark::new(MarkKind::Image { x, y, w: s, h: s, pic: pic.clone(), shape: Shape::SQUARE, turn: 0.0 }, InkRole::Fg)),
        None => marks.push(Mark::new(MarkKind::Icon { name: t.glyph, x, y, size: s }, on_ink)),
    };
    match w.layout {
        TileLayout::Grid => {
            let cols = count(w.columns, ranges::COLUMNS) as usize;
            let t = px(TILE);
            let shape = match shape_of(w.tile, &w.look, k) {
                Shape::Rect { radius } => Shape::Rect { radius: radius.min(t / 3.0) },
                s => s,
            };
            let ls = style(w.look.font, px(TILE_LABEL), face::MEDIUM);
            let label_h = if w.labels { m.measure("Ag", &ls).1 + px(LINE_GAP) } else { 0.0 };
            for (i, tile) in tiles.iter().enumerate() {
                let (c, r) = (i % cols, i / cols);
                let c = if l.rtl { cols - 1 - c } else { c };
                let (x, y) = (c as f32 * (t + px(TILE_GAP)), r as f32 * (t + px(TILE_GAP) + label_h));
                marks.push(Mark::new(MarkKind::Shaped { x, y, w: t, h: t, shape, width: 0.0 }, ink(i)));
                let s = t * TILE_ICON;
                draw_icon(&mut marks, tile, x + (t - s) / 2.0, y + (t - s) / 2.0, s);
                if w.labels {
                    let label = fit(&tile.label, &ls, t + px(TILE_GAP) * 0.8, m);
                    let (lw, lh) = m.measure(&label, &ls);
                    marks.push(Mark::new(MarkKind::Text { text: label, style: ls.clone(), x: x + (t - lw) / 2.0, y: y + t + px(LINE_GAP), w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
                }
                hits.push(Hit { rect: (x, y, t, t + label_h), action: tile.action.clone() });
            }
        }
        TileLayout::List => {
            let (row, icon, width) = (px(LIST_ROW), px(LIST_ICON), px(LIST_W));
            let ts = style(w.look.font, px(LIST_TEXT), face::MEDIUM);
            for (i, tile) in tiles.iter().enumerate() {
                let y = i as f32 * row;
                let icon_x = if l.rtl { width - icon } else { 0.0 };
                let chip = icon + px(LINE_GAP) * 2.0;
                let shape = match shape_of(w.tile, &w.look, k) {
                    Shape::Rect { radius } => Shape::Rect { radius: radius.min(chip / 3.0) },
                    s => s,
                };
                marks.push(Mark::new(MarkKind::Shaped { x: icon_x - px(LINE_GAP), y: y + (row - chip) / 2.0, w: chip, h: chip, shape, width: 0.0 }, ink(i)));
                draw_icon(&mut marks, tile, icon_x, y + (row - icon) / 2.0, icon);
                let label = fit(&tile.label, &ts, width - chip - px(GAP), m);
                let (lw, lh) = m.measure(&label, &ts);
                let lx = if l.rtl { width - chip - px(GAP) - lw } else { chip + px(GAP) - px(LINE_GAP) };
                marks.push(Mark::new(MarkKind::Text { text: label, style: ts.clone(), x: lx, y: y + (row - lh) / 2.0, w: lw, h: lh }, InkRole::Fg));
                hits.push(Hit { rect: (0.0, y, width, row), action: tile.action.clone() });
            }
        }
    }
    Composition::with_hits(marks, hits)
}

/// The name and glyph of a quick toggle.
pub fn toggle_face(id: &str, on: bool) -> (&'static str, &'static str) {
    match id {
        "wifi" => (tk("Wi-Fi"), if on { "wifi-4" } else { "wifi-off" }),
        "bluetooth" => (tk("Bluetooth"), if on { "bluetooth" } else { "bluetooth-off" }),
        "dnd" => (tk("Do not disturb"), "moon"),
        "night-light" => (tk("Night light"), "sunset"),
        "keep-awake" => (tk("Keep awake"), "coffee"),
        "dark" => (tk("Dark"), if on { "moon" } else { "sun" }),
        _ => (tk("Mute all"), if on { "volume-off" } else { "volume-high" }),
    }
}

/// Quick toggles: a shape that turns into another when on.
pub fn toggles(w: &TogglesWidget, states: &[(String, bool)], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let cols = count(w.columns, ranges::COLUMNS) as usize;
    let t = px(TOGGLE);
    let ls = style(w.look.font, px(TILE_LABEL), face::MEDIUM);
    let label_h = if w.labels { m.measure("Ag", &ls).1 + px(LINE_GAP) } else { 0.0 };
    let cell = t + px(TILE_GAP) * 1.6;
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    for (i, (id, on)) in states.iter().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let c = if l.rtl { cols - 1 - c } else { c };
        let (x, y) = (c as f32 * cell, r as f32 * (t + px(TILE_GAP) + label_h));
        let kind = if *on { w.shape_on } else { w.shape_off };
        let shape = match shape_of(kind, &w.look, k) {
            Shape::Rect { radius } => Shape::Rect { radius: radius.min(t / 3.0) },
            s => s,
        };
        let ox = (cell - t) / 2.0;
        let (name, glyph) = toggle_face(id, *on);
        if *on {
            marks.push(Mark::new(MarkKind::Shaped { x: x + ox, y, w: t, h: t, shape, width: 0.0 }, InkRole::Accent));
        } else {
            marks.push(Mark::new(MarkKind::Shaped { x: x + ox, y, w: t, h: t, shape, width: 0.0 }, InkRole::Fg).dim(OFF_ALPHA));
        }
        let s = t * TOGGLE_ICON;
        marks.push(Mark::new(MarkKind::Icon { name: glyph, x: x + ox + (t - s) / 2.0, y: y + (t - s) / 2.0, size: s }, if *on { InkRole::OnAccent } else { InkRole::Fg }));
        if w.labels {
            let label = fit(&l.word(name), &ls, cell, m);
            let (lw, lh) = m.measure(&label, &ls);
            marks.push(Mark::new(MarkKind::Text { text: label, style: ls.clone(), x: x + (cell - lw) / 2.0, y: y + t + px(LINE_GAP), w: lw, h: lh }, InkRole::Fg).dim(if *on { 1.0 } else { face::DIM }));
        }
        hits.push(Hit { rect: (x, y, cell, t + label_h), action: Action::Toggle(id.clone()) });
    }
    Composition::with_hits(marks, hits)
}

/// Volume and brightness as fat sliders: click along one to set it,
/// scroll over it to nudge it.
pub fn sliders(w: &SlidersWidget, levels: &[(String, f32)], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let len = px(within(w.length, ranges::SLIDER_LENGTH_PX, ranges::SLIDER_LENGTH_DEFAULT_PX));
    let thick = px(within(w.thickness, ranges::SLIDER_THICKNESS_PX, ranges::SLIDER_THICKNESS_DEFAULT_PX));
    let ls = style(w.look.font, px(TILE_LABEL), face::SEMIBOLD);
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    for (i, (what, v)) in levels.iter().enumerate() {
        let v = v.clamp(0.0, 1.0);
        let pct = l.digits(format!("{}%", (v * PERCENT).round() as i32));
        let (pw, ph) = m.measure(&pct, &ls);
        let icon = if what == "volume" { if v <= 0.0 { "volume-off" } else { "volume-high" } } else { "sun" };
        let off = i as f32 * (thick + px(GAP) + if w.vertical { 0.0 } else { ph });
        let (x, y, bw, bh) = if w.vertical { (off, 0.0, thick, len) } else { (0.0, off, len, thick) };
        marks.push(Mark::new(MarkKind::Rect { x, y, w: bw, h: bh, radius: thick / 2.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA));
        if v > 0.0 {
            let (fx, fy, fw, fh) = if w.vertical {
                let h = (len * v).max(thick);
                (x, y + len - h, thick, h)
            } else {
                let fw = (len * v).max(thick);
                (if l.rtl { x + len - fw } else { x }, y, fw, thick)
            };
            marks.push(Mark::new(MarkKind::Rect { x: fx, y: fy, w: fw, h: fh, radius: thick / 2.0, width: 0.0 }, InkRole::Accent));
        }
        let s = thick * SLIDER_ICON;
        let (ix, iy) = if w.vertical { (x + (thick - s) / 2.0, y + len - thick / 2.0 - s / 2.0) } else if l.rtl { (x + len - thick / 2.0 - s / 2.0, y + (thick - s) / 2.0) } else { (x + thick / 2.0 - s / 2.0, y + (thick - s) / 2.0) };
        marks.push(Mark::new(MarkKind::Icon { name: icon, x: ix, y: iy, size: s }, if v > thick / len { InkRole::OnAccent } else { InkRole::Fg }));
        let (tx, ty) = if w.vertical { (x + (thick - pw) / 2.0, len + px(LINE_GAP)) } else { ((len - pw) / 2.0, y + thick + px(LINE_GAP) / 2.0) };
        marks.push(Mark::new(MarkKind::Text { text: pct, style: ls.clone(), x: tx, y: ty, w: pw, h: ph }, InkRole::Fg).dim(face::DIM));
        // One click target per step along the track.
        for s in 0..SLIDER_STEPS {
            let f0 = s as f32 / SLIDER_STEPS as f32;
            let level = (s + 1) as f32 / SLIDER_STEPS as f32;
            let rect = if w.vertical {
                (x, y + len * (1.0 - f0 - 1.0 / SLIDER_STEPS as f32), thick, len / SLIDER_STEPS as f32)
            } else {
                let fx = if l.rtl { 1.0 - f0 - 1.0 / SLIDER_STEPS as f32 } else { f0 };
                (x + len * fx, y, len / SLIDER_STEPS as f32, thick)
            };
            hits.push(Hit { rect, action: Action::Level(what.clone(), level) });
        }
    }
    Composition::with_hits(marks, hits)
}

/// The theme's colours as swatches with their names and hex codes; a
/// click copies the code.
pub fn palette(w: &PaletteWidget, colors: &[(String, u32)], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let cols = count(w.columns, ranges::COLUMNS) as usize;
    let s = px(SWATCH);
    let shape = match shape_of(w.swatch, &w.look, k) {
        Shape::Rect { radius } => Shape::Rect { radius: radius.min(s / 3.0) },
        sh => sh,
    };
    let ls = style(w.look.font, px(SWATCH_LABEL), face::MEDIUM);
    let hs = style(mandara_config::desktop_widgets::FontRole::Mono, px(SWATCH_LABEL), face::MEDIUM);
    let label_h = if w.labels { 2.0 * m.measure("Ag", &ls).1 + px(LINE_GAP) } else { 0.0 };
    let cell = s + px(TILE_GAP) * 1.8;
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    for (i, (name, rgb)) in colors.iter().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let c = if l.rtl { cols - 1 - c } else { c };
        let (x, y) = (c as f32 * cell, r as f32 * (s + px(TILE_GAP) + label_h));
        let ox = (cell - s) / 2.0;
        marks.push(Mark::new(MarkKind::Shaped { x: x + ox, y, w: s, h: s, shape, width: 0.0 }, InkRole::Rgb(*rgb)));
        marks.push(Mark::new(MarkKind::Shaped { x: x + ox, y, w: s, h: s, shape, width: 1.0 }, InkRole::Fg).dim(TRACK_ALPHA));
        let hex = format!("#{rgb:06X}");
        if w.labels {
            let n = fit(&l.word(name), &ls, cell, m);
            let (nw, nh) = m.measure(&n, &ls);
            marks.push(Mark::new(MarkKind::Text { text: n, style: ls.clone(), x: x + (cell - nw) / 2.0, y: y + s + px(LINE_GAP), w: nw, h: nh }, InkRole::Fg));
            let (hw, hh) = m.measure(&hex, &hs);
            marks.push(Mark::new(MarkKind::Text { text: hex.clone(), style: hs.clone(), x: x + (cell - hw) / 2.0, y: y + s + px(LINE_GAP) + nh, w: hw, h: hh }, InkRole::Fg).dim(face::DIM));
        }
        hits.push(Hit { rect: (x, y, cell, s + label_h), action: Action::Copy(hex) });
    }
    Composition::with_hits(marks, hits)
}

#[cfg(test)]
fn pic() -> super::layout::Picture {
    super::layout::Picture { key: 1, pixmap: std::sync::Arc::new(tiny_skia::Pixmap::new(4, 4).unwrap()) }
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn l() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    #[test]
    fn identity_widgets_lay_out_and_take_clicks() {
        let me = MeInfo { name: "Mandara Demo".into(), login: "demo".into(), host: "mandara-demo".into(), uptime_s: 3 * 3600 + 17 * 60, avatar: None, dnd: false };
        assert_eq!(uptime(197, &l()), "3 h 17 min");
        let c = profile(&ProfileWidget::default(), &me, &l(), 1.0, &mut Fixed);
        assert!(c.size.0 > AVATAR);
        let with_pic = MeInfo { avatar: Some(pic()), ..me };
        assert!(profile(&ProfileWidget::default(), &with_pic, &l(), 1.0, &mut Fixed).marks.iter().any(|m| matches!(m.kind, MarkKind::Image { .. })));
        let tiles = vec![TileItem { label: "Files".into(), action: Action::Launch("files".into()), icon: None, glyph: "folder" }; 5];
        for layout in [TileLayout::Grid, TileLayout::List] {
            let c = shortcuts(&ShortcutsWidget { layout, ..Default::default() }, &tiles, &l(), 1.0, &mut Fixed);
            assert_eq!(c.hits.len(), 5);
        }
        let states = vec![("wifi".to_string(), true), ("dnd".to_string(), false)];
        let c = toggles(&TogglesWidget::default(), &states, &l(), 1.0, &mut Fixed);
        assert_eq!(c.hits.iter().map(|h| h.action.clone()).collect::<Vec<_>>(), vec![Action::Toggle("wifi".into()), Action::Toggle("dnd".into())]);
        let c = sliders(&SlidersWidget::default(), &[("volume".into(), 0.4)], &l(), 1.0, &mut Fixed);
        // The top step of an upright slider sets it full.
        assert_eq!(c.hit((5.0, 1.0)), Some(&Action::Level("volume".into(), 1.0)));
        let c = palette(&PaletteWidget::default(), &[("Primary".into(), 0x1F6BFF)], &l(), 1.0, &mut Fixed);
        assert_eq!(c.hits[0].action, Action::Copy("#1F6BFF".into()));
    }
}
