//! More ways to show the system monitor's readings: wavy or dotted rings,
//! segmented meters, tall bars and thermal arcs with ticks.

use std::f32::consts::TAU;

use mandara_config::desktop_widgets::{MonitorRow, MonitorWidget, ranges};

use super::layout::{Composition, InkRole, Mark, MarkKind, Measure, MonitorLine, TextStyle, caps, face, fit, monitor, style, tracked};

/// Reference sizes.
const LABEL: f32 = 13.0;
const VALUE: f32 = 17.0;
const GAP: f32 = 10.0;
const ROW_GAP: f32 = 14.0;
const METER_H: f32 = 14.0;
const SEGMENT_GAP: f32 = 0.25;
const BAR_W: f32 = 30.0;
const BAR_H: f32 = 140.0;
const BAR_GAP: f32 = 22.0;
const ARC_R: f32 = 52.0;
const ARC_W: f32 = 7.0;
const ARC_SWEEP: f32 = 0.7;
const TICKS: usize = 10;
const TICK: f32 = 6.0;
const TRACK_ALPHA: f32 = 0.18;
const LABEL_ALPHA: f32 = 0.7;
const TRACKING: f32 = 0.12;
/// Where a reading that is bad when high turns amber, then coral.
const WARN_HIGH: f32 = 0.7;
const URGENT_HIGH: f32 = 0.85;
/// Where a reading that is bad when low (the battery) turns amber, then
/// coral.
const WARN_LOW: f32 = 0.3;
const URGENT_LOW: f32 = 0.15;
/// Least room between two bar labels, reference px, and the smaller label
/// size tried before cutting a label short.
pub const LABEL_SPACE: f32 = 6.0;
pub const LABEL_SMALL: f32 = 11.0;
/// The dots and waves of a ring.
const DOTS_PER_TURN: f32 = 48.0;
const WAVES_PER_TURN: f32 = 11.0;
const WAVE_AMP: f32 = 0.3;
const POINTS_PER_WAVE: f32 = 12.0;
const MIN_POINTS: usize = 8;
/// The coral of urgent readings and the amber of warnings, OKLCH (the
/// theme's `urgent_coral` and `warning_amber` hues).
const CORAL: (f32, f32, f32) = (0.68, 0.19, 25.0);
const AMBER: (f32, f32, f32) = (0.8, 0.16, 75.0);

/// How a ring is drawn.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RingStyle {
    pub wavy: bool,
    pub dotted: bool,
}

/// An arc of a ring gauge: a plain stroke, a wave riding the radius, or a
/// row of dots.
pub fn ring(c: (f32, f32), r: f32, width: f32, start: f32, sweep: f32, style: RingStyle, ink: InkRole) -> Vec<Mark> {
    if style.dotted {
        let n = ((sweep.abs() / TAU * DOTS_PER_TURN).round() as usize).max(1);
        return (0..=n)
            .map(|i| {
                let a = start + sweep * i as f32 / n as f32;
                Mark::new(MarkKind::Disc { c: (c.0 + r * a.cos(), c.1 + r * a.sin()), r: width / 2.0 }, ink)
            })
            .collect();
    }
    if style.wavy {
        let n = ((sweep.abs() / TAU * WAVES_PER_TURN * POINTS_PER_WAVE).ceil() as usize).max(MIN_POINTS);
        let pts = (0..=n)
            .map(|i| {
                let f = i as f32 / n as f32;
                let a = start + sweep * f;
                let rr = r + width * WAVE_AMP * (a * WAVES_PER_TURN).sin();
                (c.0 + rr * a.cos(), c.1 + rr * a.sin())
            })
            .collect();
        return vec![Mark::new(MarkKind::Polyline { pts, width: width * (1.0 - WAVE_AMP) }, ink)];
    }
    vec![Mark::new(MarkKind::Arc { c, r, width, start, sweep }, ink)]
}

/// How a reading stands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Normal,
    Warning,
    Urgent,
}

/// Whether more of `row` is better: the battery's charge is; load, use,
/// heat and the fan's speed aren't.
fn higher_is_better(row: Option<MonitorRow>) -> bool {
    matches!(row, Some(MonitorRow::Battery))
}

/// How a reading of `row` at `f` (0..=1) stands, in the right direction
/// for what it measures.
pub fn status(row: Option<MonitorRow>, f: f32) -> Status {
    if higher_is_better(row) {
        if f <= URGENT_LOW {
            Status::Urgent
        } else if f <= WARN_LOW {
            Status::Warning
        } else {
            Status::Normal
        }
    } else if f >= URGENT_HIGH {
        Status::Urgent
    } else if f >= WARN_HIGH {
        Status::Warning
    } else {
        Status::Normal
    }
}

/// The ink of a reading's fill: the accent, amber or coral.
pub fn status_ink(row: Option<MonitorRow>, f: f32) -> InkRole {
    let oklch = match status(row, f) {
        Status::Normal => return InkRole::Accent,
        Status::Warning => AMBER,
        Status::Urgent => CORAL,
    };
    let [r, g, b, _] = mandara_theme::Color::oklch(oklch.0, oklch.1, oklch.2).to_rgba8();
    InkRole::Rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

/// The label that fits a bar's slot `slot` wide: the full one, else the
/// short one, else the short one smaller, else the short one cut with an
/// ellipsis. Returns the text and its style.
pub fn slot_label(full: &str, short: Option<&str>, slot: f32, st: &TextStyle, small: &TextStyle, m: &mut dyn Measure) -> (String, TextStyle) {
    let full = caps(full);
    if m.measure(&full, st).0 <= slot {
        return (full, st.clone());
    }
    let short = short.map_or_else(|| full.clone(), caps);
    if m.measure(&short, st).0 <= slot {
        return (short, st.clone());
    }
    if m.measure(&short, small).0 <= slot {
        return (short, small.clone());
    }
    (fit(&short, small, slot, m), small.clone())
}

/// The readings that aren't shown as meters, bars or arcs, as the column.
fn rest(w: &MonitorWidget, lines: Vec<&MonitorLine>, k: f32, rtl: bool, y: f32, marks: &mut Vec<Mark>, m: &mut dyn Measure) {
    if lines.is_empty() {
        return;
    }
    let column = MonitorWidget { title: String::new(), ..w.clone() };
    let lines: Vec<MonitorLine> = lines.into_iter().cloned().collect();
    for mut mk in monitor(&column, &lines, k, rtl, m).marks {
        mk.shift(0.0, y);
        marks.push(mk);
    }
}

/// Segmented meters: label and value over a row of cells.
pub fn meters(w: &MonitorWidget, lines: &[MonitorLine], k: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(w.width.clamp(ranges::MONITOR_WIDTH_PX.0, ranges::MONITOR_WIDTH_PX.1));
    let (shares, others): (Vec<&MonitorLine>, Vec<&MonitorLine>) = lines.iter().partition(|l| l.share.is_some());
    let ls = tracked(style(w.look.font, px(LABEL), face::SEMIBOLD), TRACKING);
    let vs = style(w.look.font, px(VALUE), face::SEMIBOLD);
    let n = w.segments() as usize;
    let cell = width / n as f32;
    let mut marks = Vec::new();
    let mut y = 0.0;
    for l in shares {
        let label = caps(&l.label);
        let (lw, lh) = m.measure(&label, &ls);
        let (vw, vh) = m.measure(&l.value, &vs);
        let h = lh.max(vh);
        let (lx, vx) = if rtl { (width - lw, 0.0) } else { (0.0, width - vw) };
        marks.push(Mark::new(MarkKind::Text { text: label, style: ls.clone(), x: lx, y: y + h - lh, w: lw, h: lh }, InkRole::Fg).dim(LABEL_ALPHA));
        marks.push(Mark::new(MarkKind::Text { text: l.value.clone(), style: vs.clone(), x: vx, y: y + h - vh, w: vw, h: vh }, InkRole::Fg).live());
        y += h + px(GAP) / 2.0;
        let f = l.share.unwrap_or(0.0).clamp(0.0, 1.0);
        let lit = (f * n as f32).round() as usize;
        for i in 0..n {
            let col = if rtl { n - 1 - i } else { i };
            let x = col as f32 * cell;
            let on = i < lit;
            let mark = Mark::new(MarkKind::Rect { x, y, w: cell * (1.0 - SEGMENT_GAP), h: px(METER_H), radius: px(METER_H) * SEGMENT_GAP, width: 0.0 }, if on { status_ink(l.row, f) } else { InkRole::Fg }).dim(if on { 1.0 } else { TRACK_ALPHA });
            marks.push(mark.live());
        }
        y += px(METER_H) + px(ROW_GAP);
    }
    rest(w, others, k, rtl, y, &mut marks, m);
    Composition::of(marks)
}

/// Tall bars side by side, the value over each and the label under. Each
/// bar owns a slot of the bar pitch (the widget's width shared out, never
/// narrower than a bar and its gap), and its label is centred in it and
/// shortened to fit, so labels never touch.
pub fn bars(w: &MonitorWidget, lines: &[MonitorLine], k: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let (shares, others): (Vec<&MonitorLine>, Vec<&MonitorLine>) = lines.iter().partition(|l| l.share.is_some());
    let ls = tracked(style(w.look.font, px(LABEL), face::SEMIBOLD), TRACKING);
    let small = tracked(style(w.look.font, px(LABEL_SMALL), face::SEMIBOLD), TRACKING);
    let vs = style(w.look.font, px(LABEL), face::SEMIBOLD);
    let (bw, bh) = (px(BAR_W), px(BAR_H));
    let n = shares.len();
    let width = px(w.width.clamp(ranges::MONITOR_WIDTH_PX.0, ranges::MONITOR_WIDTH_PX.1));
    let pitch = bar_pitch(width, n, bw + px(BAR_GAP));
    let label_room = pitch - px(LABEL_SPACE);
    let mut marks = Vec::new();
    let value_h = m.measure("0", &vs).1;
    let mut bottom: f32 = 0.0;
    for (i, l) in shares.into_iter().enumerate() {
        let col = if rtl { n - 1 - i } else { i };
        let centre = (col as f32 + 0.5) * pitch;
        let x = centre - bw / 2.0;
        let top = value_h + px(GAP) / 2.0;
        let f = l.share.unwrap_or(0.0).clamp(0.0, 1.0);
        marks.push(Mark::new(MarkKind::Rect { x, y: top, w: bw, h: bh, radius: bw / 2.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA));
        if f > 0.0 {
            let fh = (bh * f).max(bw);
            marks.push(Mark::new(MarkKind::Rect { x, y: top + bh - fh, w: bw, h: fh, radius: bw / 2.0, width: 0.0 }, status_ink(l.row, f)).live());
        }
        marks.push(Mark::new(MarkKind::Rect { x, y: top, w: bw, h: bh, radius: bw / 2.0, width: 0.0 }, InkRole::Fg).dim(0.0).live());
        let value = fit(&l.value, &vs, label_room, m);
        let (vw, vh) = m.measure(&value, &vs);
        marks.push(Mark::new(MarkKind::Text { text: value, style: vs.clone(), x: centre - vw / 2.0, y: 0.0, w: vw, h: vh }, InkRole::Fg).live());
        let (label, st) = slot_label(&l.label, l.short.as_deref(), label_room, &ls, &small, m);
        let (lw, lh) = m.measure(&label, &st);
        marks.push(Mark::new(MarkKind::Text { text: label, style: st, x: centre - lw / 2.0, y: top + bh + px(GAP) / 2.0, w: lw, h: lh }, InkRole::Fg).dim(LABEL_ALPHA));
        bottom = bottom.max(top + bh + px(GAP) / 2.0 + lh);
    }
    // The slots' full width, so the outer ones keep their room too.
    if n > 0 {
        marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: pitch * n as f32, h: bottom, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    }
    rest(w, others, k, rtl, if n > 0 { bottom + px(ROW_GAP) } else { 0.0 }, &mut marks, m);
    Composition::of(marks)
}

/// The distance between bar centres: `width` shared by `n` bars, never
/// less than `least` (a bar and its gap).
pub fn bar_pitch(width: f32, n: usize, least: f32) -> f32 {
    if n == 0 { least } else { (width / n as f32).max(least) }
}

/// Arc gauges with ticks for the temperatures and the fan (from their
/// usual range), the other readings as the column.
pub fn thermals(w: &MonitorWidget, lines: &[MonitorLine], k: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let heat = |l: &&MonitorLine| matches!(l.row, Some(MonitorRow::CpuTemp | MonitorRow::GpuTemp | MonitorRow::Fan)) && l.level.is_some();
    let (arcs, others): (Vec<&MonitorLine>, Vec<&MonitorLine>) = lines.iter().partition(heat);
    let (r, aw) = (px(ARC_R), px(ARC_W));
    let cell = 2.0 * r + px(BAR_GAP);
    let n = arcs.len();
    let vs = style(w.look.font, px(VALUE), face::SEMIBOLD);
    let ls = tracked(style(w.look.font, px(LABEL), face::SEMIBOLD), TRACKING);
    let small = tracked(style(w.look.font, px(LABEL_SMALL), face::SEMIBOLD), TRACKING);
    let sweep = ARC_SWEEP * TAU;
    let start = -std::f32::consts::FRAC_PI_2 - sweep / 2.0;
    let mut marks = Vec::new();
    let mut bottom: f32 = 0.0;
    for (i, l) in arcs.into_iter().enumerate() {
        let col = if rtl { n - 1 - i } else { i };
        let c = (r + aw + col as f32 * cell, r + aw);
        let f = l.level.unwrap_or(0.0).clamp(0.0, 1.0);
        marks.push(Mark::new(MarkKind::Arc { c, r, width: aw, start, sweep }, InkRole::Fg).dim(TRACK_ALPHA));
        if f > 0.0 {
            marks.push(Mark::new(MarkKind::Arc { c, r, width: aw, start, sweep: sweep * f }, status_ink(l.row, f)).live());
        }
        marks.push(Mark::new(MarkKind::Arc { c, r, width: aw * 2.0, start, sweep }, InkRole::Fg).dim(0.0).live());
        for t in 0..=TICKS {
            let a = start + sweep * t as f32 / TICKS as f32;
            let (r0, r1) = (r - aw - px(TICK), r - aw);
            marks.push(Mark::new(MarkKind::Line { a: (c.0 + r0 * a.cos(), c.1 + r0 * a.sin()), b: (c.0 + r1 * a.cos(), c.1 + r1 * a.sin()), width: 1.0 }, InkRole::Fg).dim(LABEL_ALPHA));
        }
        let (vw, vh) = m.measure(&l.value, &vs);
        marks.push(Mark::new(MarkKind::Text { text: l.value.clone(), style: vs.clone(), x: c.0 - vw / 2.0, y: c.1 - vh / 2.0, w: vw, h: vh }, InkRole::Fg).live());
        let (label, st) = slot_label(&l.label, l.short.as_deref(), cell - px(LABEL_SPACE), &ls, &small, m);
        let (lw, lh) = m.measure(&label, &st);
        let ly = c.1 + r * (sweep / 2.0).cos().abs().max(0.4);
        marks.push(Mark::new(MarkKind::Text { text: label, style: st, x: c.0 - lw / 2.0, y: ly, w: lw, h: lh }, InkRole::Fg).dim(LABEL_ALPHA));
        bottom = bottom.max(ly + lh);
    }
    rest(w, others, k, rtl, if n > 0 { bottom + px(ROW_GAP) } else { 0.0 }, &mut marks, m);
    Composition::of(marks)
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn lines() -> Vec<MonitorLine> {
        vec![
            MonitorLine { row: Some(MonitorRow::Cpu), label: "CPU".into(), value: "42%".into(), share: Some(0.42), ..Default::default() },
            MonitorLine { row: Some(MonitorRow::Memory), label: "Memory".into(), value: "91%".into(), share: Some(0.91), ..Default::default() },
            MonitorLine { row: Some(MonitorRow::CpuTemp), label: "CPU temp".into(), value: "61°".into(), level: Some(0.61), ..Default::default() },
            MonitorLine { row: Some(MonitorRow::Download), label: "Download".into(), value: "2 MB/s".into(), ..Default::default() },
        ]
    }

    #[test]
    fn every_style_lays_out_and_rings_take_any_style() {
        let w = MonitorWidget::default();
        for c in [meters(&w, &lines(), 1.0, false, &mut Fixed), bars(&w, &lines(), 1.0, true, &mut Fixed), thermals(&w, &lines(), 1.0, false, &mut Fixed)] {
            assert!(c.size.0 > 50.0 && c.size.1 > 50.0, "{:?}", c.size);
        }
        assert_eq!(ring((0.0, 0.0), 10.0, 2.0, 0.0, TAU, RingStyle { wavy: false, dotted: true }, InkRole::Fg).len(), DOTS_PER_TURN as usize + 1);
    }

    #[test]
    fn each_reading_is_judged_in_its_own_direction() {
        // Load, use and heat are bad when high.
        for row in [MonitorRow::Cpu, MonitorRow::Memory, MonitorRow::Disk, MonitorRow::Gpu, MonitorRow::CpuTemp, MonitorRow::GpuTemp, MonitorRow::Fan] {
            assert_eq!(status(Some(row), 0.2), Status::Normal, "{row:?}");
            assert_eq!(status(Some(row), 0.75), Status::Warning, "{row:?}");
            assert_eq!(status(Some(row), 0.9), Status::Urgent, "{row:?}");
        }
        // The battery is bad when low: full is fine.
        let bat = Some(MonitorRow::Battery);
        assert_eq!(status(bat, 0.86), Status::Normal);
        assert_eq!(status(bat, 1.0), Status::Normal);
        assert_eq!(status(bat, 0.25), Status::Warning);
        assert_eq!(status(bat, 0.1), Status::Urgent);
        assert_eq!(status_ink(bat, 0.86), InkRole::Accent);
        assert_ne!(status_ink(bat, 0.1), InkRole::Accent);
        assert_eq!(status_ink(Some(MonitorRow::Cpu), 0.9), status_ink(bat, 0.1));
    }

    #[test]
    fn bar_labels_keep_to_their_slots_at_any_width() {
        let ls = style(Default::default(), LABEL, face::SEMIBOLD);
        let small = style(Default::default(), LABEL_SMALL, face::SEMIBOLD);
        let mut m = Fixed;
        // Wide enough: the full label; tight: the short one; tighter: cut.
        assert_eq!(slot_label("Memory", Some("RAM"), 500.0, &ls, &small, &mut m).0, "MEMORY");
        let full_w = m.measure("MEMORY", &ls).0;
        assert_eq!(slot_label("Memory", Some("RAM"), full_w - 1.0, &ls, &small, &mut m).0, "RAM");
        let cut = slot_label("Battery", None, m.measure("BAT", &small).0, &small, &small, &mut m).0;
        assert!(cut.ends_with('…'), "{cut}");
        // Thermal arcs keep their labels apart too.
        let heat: Vec<MonitorLine> = ["CPU temperature", "GPU temperature", "Fan"].iter().map(|l| MonitorLine { row: Some(MonitorRow::CpuTemp), label: (*l).into(), value: "50°".into(), level: Some(0.5), ..Default::default() }).collect();
        let c = thermals(&MonitorWidget::default(), &heat, 1.0, false, &mut m);
        let mut spans: Vec<(f32, f32)> = c.marks.iter().filter_map(|mk| match &mk.kind {
            MarkKind::Text { text, x, w, .. } if !text.contains('°') => Some((*x, x + w)),
            _ => None,
        }).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(spans.windows(2).all(|p| p[0].1 < p[1].0), "{spans:?}");
        // The pitch shares the width out but never squeezes bars together.
        assert_eq!(bar_pitch(260.0, 4, 52.0), 65.0);
        assert_eq!(bar_pitch(100.0, 5, 52.0), 52.0);
        // Laid out, no two labels overlap, whatever the width and labels.
        let labels = ["CPU", "Memory", "Disk", "Battery", "GPU"];
        for width in [ranges::MONITOR_WIDTH_PX.0, 260.0, ranges::MONITOR_WIDTH_PX.1] {
            let w = MonitorWidget { width, ..MonitorWidget::default() };
            let lines: Vec<MonitorLine> = labels.iter().map(|l| MonitorLine { label: (*l).into(), value: "100%".into(), share: Some(0.5), ..Default::default() }).collect();
            let c = bars(&w, &lines, 1.0, false, &mut m);
            let mut boxes: Vec<(f32, f32)> = c.marks.iter().filter_map(|mk| match &mk.kind {
                MarkKind::Text { text, x, w, .. } if !text.contains('%') => Some((*x, x + w)),
                _ => None,
            }).collect();
            boxes.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(boxes.len(), labels.len());
            for pair in boxes.windows(2) {
                assert!(pair[0].1 < pair[1].0, "labels touch at width {width}: {boxes:?}");
            }
        }
    }
}
