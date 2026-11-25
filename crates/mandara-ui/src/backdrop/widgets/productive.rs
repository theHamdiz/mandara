//! Productivity on the desktop: a to-do checklist, a sticky note, the
//! focus timer, a quote card and notebook tabs. Data comes from the shared
//! productivity store (the To-do, Notes and Pomodoro plugins) through
//! [`super::WidgetData`]; clicks tick tasks, open notes and run the timer.

use std::f32::consts::{FRAC_PI_2, TAU};

use chrono::{DateTime, Datelike as _, Local, Timelike as _};
use mandara_config::desktop_widgets::{NoteRuling, NoteSheet, NoteWidget, NotebookWidget, PlateShape, PomodoroWidget, QuoteEvery, QuoteWidget, TimerStyle, TodoWidget};
use mandara_config::widget_kinds::{count, ranges, within};
use mandara_core::shape::Shape;
use mandara_i18n::tk;

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, TextStyle, caps, face, fit, style, tracked, wrap};
use super::lingo::Lingo;
use super::sayings::SAYINGS;
use super::{NoteItem, PomodoroState, TodoItem};

/// Reference sizes.
const ROW_GAP: f32 = 12.0;
const CHECK: f32 = 1.15;
const CHECK_GAP: f32 = 12.0;
const CHECK_STROKE: f32 = 2.0;
const TITLE_SCALE: f32 = 1.25;
const TITLE_GAP: f32 = 14.0;
const RULE_ALPHA: f32 = 0.16;
const DONE_ALPHA: f32 = 0.5;
const STRIKE_W: f32 = 1.5;
const NOTE_PAD: f32 = 22.0;
const NOTE_LINE: f32 = 1.55;
const GRID_ALPHA: f32 = 0.12;
const DOT_R: f32 = 1.4;
const COUNT_SCALE: f32 = 0.65;
const TIMER_RING_W: f32 = 0.05;
const TIMER_TIME: f32 = 0.24;
const TIMER_LABEL: f32 = 0.075;
const TIMER_DOT: f32 = 0.03;
const TIMER_DOT_GAP: f32 = 0.05;
const BUTTON: f32 = 0.16;
const BUTTON_GAP: f32 = 0.06;
const TRACK_ALPHA: f32 = 0.18;
pub(super) const QUOTE_MARK: f32 = 2.6;
pub(super) const QUOTE_AUTHOR: f32 = 0.62;
pub(super) const QUOTE_GAP: f32 = 14.0;
pub(super) const QUOTE_LINES: usize = 12;
const LABEL_TRACKING: f32 = 0.12;
pub(super) const AUTHOR_TRACKING: f32 = 0.06;
/// The words start this share of the quotation mark's height below its top.
pub(super) const QUOTE_MARK_DROP: f32 = 0.62;
const TAB_W: f32 = 46.0;
const TAB_TEXT: f32 = 16.0;
const TAB_PAD: f32 = 18.0;
const TAB_GAP: f32 = 6.0;
const TAB_MAX: f32 = 220.0;
/// Pastel hue steps between notebook tabs, degrees.
const TAB_HUE_STEP: u16 = 47;
/// Fraction of the check box the tick spans.
const TICK: [(f32, f32); 3] = [(0.25, 0.52), (0.43, 0.7), (0.76, 0.32)];
const SECONDS_PER_MINUTE: i64 = 60;
const MS_PER_SECOND: i64 = 1000;

/// A box of size `s` at `(x, y)` as a hit rectangle.
fn rect(x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    (x, y, w, h)
}

// ── To-do ────────────────────────────────────────────────────────────────

/// The to-do checklist.
pub fn todo(w: &TodoWidget, items: &[TodoItem], lists: &[(u64, String)], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(within(w.width, ranges::WIDTH_PX, ranges::TODO_WIDTH_DEFAULT_PX));
    let size = px(within(w.text_size, ranges::TEXT_PX, ranges::TODO_TEXT_DEFAULT_PX));
    let ts = style(w.look.font, size, face::REGULAR);
    let list = lists.iter().find(|(_, n)| !w.list.trim().is_empty() && n.eq_ignore_ascii_case(w.list.trim())).map(|(id, _)| *id);
    let shown: Vec<&TodoItem> = items.iter().filter(|t| list.is_none_or(|id| t.list == id) && (w.show_done || !t.done)).take(count(w.rows, ranges::ROWS) as usize).collect();
    let open = items.iter().filter(|t| list.is_none_or(|id| t.list == id) && !t.done).count();
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    let mut y = 0.0;
    // The whole width is laid out even when the rows are short.
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    if w.title {
        let name = list.and_then(|id| lists.iter().find(|(i, _)| *i == id)).map_or_else(|| l.word(tk("To-do")), |(_, n)| n.clone());
        let hs = style(w.look.font, size * TITLE_SCALE, face::SEMIBOLD);
        let cs = style(w.look.font, size * COUNT_SCALE, face::MEDIUM);
        let count_text = l.digits(l.word(tk("{count} open")).replace("{count}", &open.to_string()));
        let (cw, ch) = m.measure(&count_text, &cs);
        let name = fit(&name, &hs, width - cw - px(CHECK_GAP), m);
        let (nw, nh) = m.measure(&name, &hs);
        let (nx, cx) = if l.rtl { (width - nw, 0.0) } else { (0.0, width - cw) };
        marks.push(Mark::new(MarkKind::Text { text: name, style: hs, x: nx, y: 0.0, w: nw, h: nh }, InkRole::Fg));
        marks.push(Mark::new(MarkKind::Text { text: count_text, style: cs, x: cx, y: nh - ch, w: cw, h: ch }, InkRole::Accent));
        y += nh + px(TITLE_GAP);
    }
    let shape = match shape_of(w.checkbox, &w.look, k) {
        Shape::Rect { .. } => Shape::Rect { radius: size * CHECK * 0.22 },
        s => s,
    };
    let row_h = m.measure("Ag", &ts).1;
    let bx = size * CHECK;
    let text_w = width - bx - px(CHECK_GAP);
    if shown.is_empty() {
        let text = l.word(tk("Nothing to do"));
        let (tw, th) = m.measure(&text, &ts);
        marks.push(Mark::new(MarkKind::Text { text, style: ts.clone(), x: if l.rtl { width - tw } else { 0.0 }, y, w: tw, h: th }, InkRole::Fg).dim(face::DIM));
        y += th + px(ROW_GAP);
    }
    for t in shown {
        let h = row_h.max(bx);
        let (box_x, text_x) = if l.rtl { (width - bx, 0.0) } else { (0.0, bx + px(CHECK_GAP)) };
        let by = y + (h - bx) / 2.0;
        if t.done {
            marks.push(Mark::new(MarkKind::Shaped { x: box_x, y: by, w: bx, h: bx, shape, width: 0.0 }, InkRole::Accent));
            let tick: Vec<(f32, f32)> = TICK.iter().map(|(fx, fy)| (box_x + fx * bx, by + fy * bx)).collect();
            marks.push(Mark::new(MarkKind::Polyline { pts: tick, width: px(CHECK_STROKE) }, InkRole::OnAccent));
        } else {
            marks.push(Mark::new(MarkKind::Shaped { x: box_x + px(CHECK_STROKE) / 2.0, y: by + px(CHECK_STROKE) / 2.0, w: bx - px(CHECK_STROKE), h: bx - px(CHECK_STROKE), shape, width: px(CHECK_STROKE) }, InkRole::Fg).dim(face::DIM));
        }
        let text = fit(&t.text, &ts, text_w, m);
        let (tw, th) = m.measure(&text, &ts);
        let tx = if l.rtl { text_w - tw } else { text_x };
        let ty = y + (h - th) / 2.0;
        marks.push(Mark::new(MarkKind::Text { text, style: ts.clone(), x: tx, y: ty, w: tw, h: th }, InkRole::Fg).dim(if t.done { DONE_ALPHA } else { 1.0 }));
        if t.done {
            marks.push(Mark::new(MarkKind::Line { a: (tx, ty + th * 0.55), b: (tx + tw, ty + th * 0.55), width: px(STRIKE_W) }, InkRole::Fg).dim(DONE_ALPHA));
        }
        if w.ruled {
            let ry = y + h + px(ROW_GAP) / 2.0;
            marks.push(Mark::new(MarkKind::Line { a: (0.0, ry), b: (width, ry), width: 1.0 }, InkRole::Fg).dim(RULE_ALPHA));
        }
        let hit_y = y - px(ROW_GAP) / 2.0;
        let hit_h = h + px(ROW_GAP);
        hits.push(Hit { rect: rect(box_x, hit_y, bx, hit_h), action: Action::Todo(t.id) });
        hits.push(Hit { rect: rect(if l.rtl { 0.0 } else { text_x }, hit_y, text_w, hit_h), action: Action::TodoEdit(t.id) });
        y += h + px(ROW_GAP);
    }
    if w.add_row {
        let text = format!("+  {}", l.word(tk("Add a task")));
        let (tw, th) = m.measure(&text, &ts);
        marks.push(Mark::new(MarkKind::Text { text, style: ts, x: if l.rtl { width - tw } else { 0.0 }, y, w: tw, h: th }, InkRole::Accent).dim(face::DIM));
        hits.push(Hit { rect: rect(0.0, y, width, th), action: Action::TodoAdd });
    }
    Composition::with_hits(marks, hits)
}

// ── Sticky note ──────────────────────────────────────────────────────────

/// The note `w` names (else the most recent), on its sheet.
pub fn note(w: &NoteWidget, notes: &[NoteItem], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(within(w.width, ranges::WIDTH_PX, ranges::NOTE_WIDTH_DEFAULT_PX));
    let size = px(within(w.text_size, ranges::TEXT_PX, ranges::NOTE_TEXT_DEFAULT_PX));
    let pad = px(NOTE_PAD);
    let found = if w.note.trim().is_empty() { notes.first() } else { notes.iter().find(|n| n.title.eq_ignore_ascii_case(w.note.trim())) };
    let (title, body) = found.map_or_else(|| (l.word(tk("No notes yet")), l.word(tk("Click to write one."))), |n| (n.title.clone(), n.body.clone()));
    let (sheet, ink) = match w.sheet {
        NoteSheet::Theme => (None, InkRole::Fg),
        NoteSheet::Pastel => (Some(InkRole::Pastel(0)), InkRole::OnPaper),
        NoteSheet::Paper => (Some(InkRole::Paper), InkRole::OnPaper),
        NoteSheet::Custom => (Some(super::draw::parse_color(&w.sheet_color).map_or(InkRole::Pastel(0), |c| InkRole::Rgb(rgb(c)))), InkRole::OnPaper),
    };
    let ink = if w.look.tone == mandara_config::desktop_widgets::Tone::Auto { ink } else { InkRole::Fg };
    let inner = width - 2.0 * pad;
    let hs = style(w.look.font, size * TITLE_SCALE, face::BOLD);
    let bs = style(w.look.font, size, face::REGULAR);
    let line_h = size * NOTE_LINE;
    let title_line = fit(&title, &hs, inner, m);
    let body_lines = wrap(&body, &bs, inner, count(w.lines, ranges::ROWS) as usize, m);
    let rows = body_lines.len().max(count(w.lines, ranges::ROWS) as usize);
    let title_h = m.measure(&title_line, &hs).1;
    let height = pad * 2.0 + title_h + px(TITLE_GAP) + rows as f32 * line_h;
    let mut marks = Vec::new();
    if let Some(ink_sheet) = sheet {
        let shape = shape_of(if w.look.plate_shape == PlateShape::Rect { PlateShape::Rect } else { w.look.plate_shape }, &w.look, k);
        marks.push(Mark::new(MarkKind::Plate { x: 0.0, y: 0.0, w: width, h: height, radius: w.look.plate_radius() * k, rim: 0.0, shape }, ink_sheet));
    } else {
        marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: height, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    }
    let (tw, th) = m.measure(&title_line, &hs);
    // The note's own words decide which side they start on.
    let rtl = super::layout::reads_rtl(&format!("{title} {body}")).unwrap_or(l.rtl);
    let tx = |w0: f32| if rtl { width - pad - w0 } else { pad };
    marks.push(Mark::new(MarkKind::Text { text: title_line, style: hs, x: tx(tw), y: pad, w: tw, h: th }, ink));
    let top = pad + title_h + px(TITLE_GAP);
    for i in 0..rows {
        let base = top + (i as f32 + 1.0) * line_h;
        match w.ruling {
            NoteRuling::Ruled => marks.push(Mark::new(MarkKind::Line { a: (pad, base), b: (width - pad, base), width: 1.0 }, ink).dim(RULE_ALPHA)),
            NoteRuling::Grid => {
                marks.push(Mark::new(MarkKind::Line { a: (pad, base), b: (width - pad, base), width: 1.0 }, ink).dim(GRID_ALPHA));
                if i == 0 {
                    let mut x = pad;
                    while x <= width - pad {
                        marks.push(Mark::new(MarkKind::Line { a: (x, top), b: (x, top + rows as f32 * line_h), width: 1.0 }, ink).dim(GRID_ALPHA));
                        x += line_h;
                    }
                }
            }
            NoteRuling::Dots => {
                let mut x = pad;
                while x <= width - pad {
                    marks.push(Mark::new(MarkKind::Disc { c: (x, base), r: px(DOT_R) }, ink).dim(RULE_ALPHA * 2.0));
                    x += line_h;
                }
            }
            NoteRuling::Plain => {}
        }
    }
    for (i, text) in body_lines.into_iter().enumerate() {
        let (bw, bh) = m.measure(&text, &bs);
        marks.push(Mark::new(MarkKind::Text { text, style: bs.clone(), x: tx(bw), y: top + (i as f32 + 1.0) * line_h - bh - line_h * 0.12, w: bw, h: bh }, ink));
    }
    if w.word_count {
        let words = body.split_whitespace().count() + found.map_or(0, |n| n.title.split_whitespace().count());
        let text = l.digits(l.word(tk("{count} words")).replace("{count}", &words.to_string()));
        let cs = style(w.look.font, size * COUNT_SCALE, face::MEDIUM);
        let (cw, ch) = m.measure(&text, &cs);
        marks.push(Mark::new(MarkKind::Text { text, style: cs, x: if rtl { pad } else { width - pad - cw }, y: height - pad / 2.0 - ch, w: cw, h: ch }, ink).dim(face::DIM));
    }
    let hits = vec![Hit { rect: rect(0.0, 0.0, width, height), action: Action::Note(found.map(|n| n.title.clone()).unwrap_or_default()) }];
    Composition::with_hits(marks, hits)
}

fn rgb(c: mandara_theme::Color) -> u32 {
    let [r, g, b, _] = c.to_rgba8();
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

// ── Focus timer ──────────────────────────────────────────────────────────

/// Milliseconds left on the timer at `now`.
pub fn timer_left(p: &PomodoroState, now: &DateTime<Local>) -> i64 {
    match p.ends_ms {
        Some(end) => (end - now.timestamp_millis()).max(0),
        None => p.paused_ms,
    }
}

/// The focus timer: a ring (or dots) with the time left, the phase, the
/// sessions of the set, and skip / reset buttons.
pub fn pomodoro(w: &PomodoroWidget, p: &PomodoroState, now: &DateTime<Local>, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let d = within(w.size, ranges::TIMER_SIZE_PX, ranges::TIMER_SIZE_DEFAULT_PX) * k;
    let left = timer_left(p, now);
    let secs = (left + MS_PER_SECOND - 1) / MS_PER_SECOND;
    let time = l.digits(format!("{:02}:{:02}", secs / SECONDS_PER_MINUTE, secs % SECONDS_PER_MINUTE));
    let frac = if p.length_ms > 0 { (1.0 - left as f32 / p.length_ms as f32).clamp(0.0, 1.0) } else { 0.0 };
    let ring_w = d * TIMER_RING_W;
    let r = d / 2.0 - ring_w;
    let c = (d / 2.0, d / 2.0);
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    let ts = style(w.look.font, d * TIMER_TIME, face::SEMIBOLD);
    let (tw, th) = m.measure(&time, &ts);
    let label = l.word(p.phase);
    let ls = tracked(style(w.look.font, d * TIMER_LABEL, face::BOLD), LABEL_TRACKING);
    let label = caps(&label);
    let (lw, lh) = m.measure(&label, &ls);
    match w.style {
        TimerStyle::Ring => {
            marks.push(Mark::new(MarkKind::Arc { c, r, width: ring_w, start: -FRAC_PI_2, sweep: TAU }, InkRole::Fg).dim(TRACK_ALPHA));
            if frac > 0.0 {
                marks.push(Mark::new(MarkKind::Arc { c, r, width: ring_w, start: -FRAC_PI_2, sweep: TAU * frac }, InkRole::Accent).live());
            }
            // The live plane covers the whole ring and the time.
            marks.push(Mark::new(MarkKind::Arc { c, r, width: ring_w, start: 0.0, sweep: TAU }, InkRole::Fg).dim(0.0).live());
        }
        TimerStyle::Dots => {}
    }
    marks.push(Mark::new(MarkKind::Text { text: time, style: ts, x: c.0 - tw / 2.0, y: c.1 - th / 2.0, w: tw, h: th }, InkRole::Fg).live());
    marks.push(Mark::new(MarkKind::Text { text: label, style: ls, x: c.0 - lw / 2.0, y: c.1 + th / 2.0, w: lw, h: lh }, if p.running { InkRole::Accent } else { InkRole::Fg }).dim(if p.running { 1.0 } else { face::DIM }));
    // One dot per focus session of the set, the done ones filled.
    let n = p.long_every.max(1) as usize;
    let done = p.completed as usize % n;
    let dot = d * TIMER_DOT;
    let gap = d * TIMER_DOT_GAP;
    let row_w = n as f32 * 2.0 * dot + (n as f32 - 1.0) * gap;
    let dy = if w.style == TimerStyle::Ring { c.1 - th / 2.0 - dot * 3.0 } else { c.1 + th / 2.0 + lh + dot * 2.0 };
    for i in 0..n {
        let x = c.0 - row_w / 2.0 + dot + i as f32 * (2.0 * dot + gap);
        let filled = i < done;
        marks.push(Mark::new(MarkKind::Disc { c: (x, dy), r: dot }, if filled { InkRole::Accent } else { InkRole::Fg }).dim(if filled { 1.0 } else { TRACK_ALPHA * 2.0 }));
    }
    hits.push(Hit { rect: rect(0.0, 0.0, d, d), action: Action::Pomodoro });
    if w.controls {
        let b = d * BUTTON;
        let y = d + d * BUTTON_GAP;
        for (i, (icon, action)) in [("refresh", Action::PomodoroReset), ("skip", Action::PomodoroSkip)].into_iter().enumerate() {
            let x = c.0 + if i == 0 { -b - d * BUTTON_GAP / 2.0 } else { d * BUTTON_GAP / 2.0 };
            marks.push(Mark::new(MarkKind::Rect { x, y, w: b, h: b, radius: b / 2.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA / 2.0));
            marks.push(Mark::new(MarkKind::Icon { name: icon, x: x + b * 0.25, y: y + b * 0.25, size: b * 0.5 }, InkRole::Fg));
            hits.push(Hit { rect: rect(x, y, b, b), action });
        }
    }
    Composition::with_hits(marks, hits)
}

// ── Quote ────────────────────────────────────────────────────────────────

/// How far the built-in sayings advance per day, hour or click: coprime to
/// their count, so every saying comes round once per cycle, and large
/// enough that neighbours in the list (same author, same tradition) are not
/// shown one after the other.
const SAYING_STRIDE: usize = 41;

/// The quote shown at `now` after `turns` clicks: `(text, author, work)`,
/// translated. Your own quotes (`text — author`) come in the order you
/// wrote them and have no work; without any, the built-in [`SAYINGS`].
pub fn pick_quote(w: &QuoteWidget, now: &DateTime<Local>, turns: u32, l: &Lingo) -> (String, String, String) {
    let step = match w.every {
        QuoteEvery::Day => now.num_days_from_ce() as u32,
        QuoteEvery::Hour => now.num_days_from_ce() as u32 * 24 + now.hour(),
        QuoteEvery::Click => 0,
    };
    let at = step.wrapping_add(turns) as usize;
    let own: Vec<&str> = w.quotes.iter().map(|q| q.trim()).filter(|q| !q.is_empty()).collect();
    if own.is_empty() {
        let s = &SAYINGS[at % SAYINGS.len() * SAYING_STRIDE % SAYINGS.len()];
        let work = if s.work.is_empty() { String::new() } else { l.word(s.work) };
        return (l.word(s.text), l.word(s.author), work);
    }
    let q = own[at % own.len()];
    match q.rsplit_once(" — ").or_else(|| q.rsplit_once(" - ")) {
        Some((t, a)) => (t.trim().to_string(), a.trim().to_string(), String::new()),
        None => (q.to_string(), String::new(), String::new()),
    }
}

/// The attribution under a quote: "— Author, Work" when that fits on one
/// line of the card (measured in the author's style), else "— Author".
fn attribution(author: &str, work: &str, l: &Lingo, st: &TextStyle, width: f32, m: &mut dyn Measure) -> String {
    if !work.is_empty() {
        let full = format!("— {}", mandara_i18n::fill(&l.word(tk("{author}, {work}")), &[("author", &author), ("work", &work)]));
        if m.measure(&full, st).0 <= width {
            return full;
        }
    }
    format!("— {author}")
}

/// A quote card: a large quotation mark, the words, the author.
pub fn quote(w: &QuoteWidget, now: &DateTime<Local>, turns: u32, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(within(w.width, ranges::WIDTH_PX, ranges::QUOTE_WIDTH_DEFAULT_PX));
    let size = px(within(w.text_size, ranges::TEXT_PX, ranges::QUOTE_TEXT_DEFAULT_PX));
    let (text, author, work) = pick_quote(w, now, turns, l);
    let qs = style(w.look.font, size, face::LIGHT);
    let mut marks = Vec::new();
    let mut y = 0.0;
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    if w.mark {
        let ms = style(w.look.font, size * QUOTE_MARK, face::BLACK);
        let glyph = if l.rtl { "”" } else { "“" };
        let (gw, gh) = m.measure(glyph, &ms);
        marks.push(Mark::new(MarkKind::Text { text: glyph.into(), style: ms, x: (width - gw) * align, y: 0.0, w: gw, h: gh }, InkRole::Accent));
        y += gh * QUOTE_MARK_DROP;
    }
    for line in wrap(&text, &qs, width, QUOTE_LINES, m) {
        let (lw, lh) = m.measure(&line, &qs);
        marks.push(Mark::new(MarkKind::Text { text: line, style: qs.clone(), x: (width - lw) * align, y, w: lw, h: lh }, InkRole::Fg));
        y += lh;
    }
    if !author.is_empty() {
        y += px(QUOTE_GAP);
        let st = tracked(style(w.look.font, size * QUOTE_AUTHOR, face::SEMIBOLD), AUTHOR_TRACKING);
        let a = attribution(&author, &work, l, &st, width, m);
        let (aw, ah) = m.measure(&a, &st);
        marks.push(Mark::new(MarkKind::Text { text: a, style: st, x: (width - aw) * align, y, w: aw, h: ah }, InkRole::Accent).dim(face::DIM));
        y += ah;
    }
    let hits = vec![Hit { rect: rect(0.0, 0.0, width, y), action: Action::Next }];
    Composition::with_hits(marks, hits)
}

// ── Notebook tabs ────────────────────────────────────────────────────────

/// Your most recent notes as tabs, written sideways; turned toward the
/// middle of the screen (`clockwise` on the right half).
pub fn notebook(w: &NotebookWidget, notes: &[NoteItem], l: &Lingo, clockwise: bool, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let ts = style(w.look.font, px(TAB_TEXT), face::SEMIBOLD);
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    let mut y = 0.0;
    let tab_w = px(TAB_W);
    let names: Vec<String> = if notes.is_empty() { vec![l.word(tk("Notes"))] } else { notes.iter().take(count(w.tabs, ranges::TABS) as usize).map(|n| n.title.clone()).collect() };
    for (i, name) in names.into_iter().enumerate() {
        let text = fit(&name, &ts, px(TAB_MAX), m);
        let (tw, th) = m.measure(&text, &ts);
        let h = tw + 2.0 * px(TAB_PAD);
        let shape = match shape_of(w.tab_shape, &w.look, k) {
            Shape::Rect { radius } => Shape::Rect { radius: radius.min(tab_w / 3.0) },
            s => s,
        };
        let ink = InkRole::Pastel((i as u16 * TAB_HUE_STEP) % 360);
        marks.push(Mark::new(MarkKind::Shaped { x: 0.0, y, w: tab_w, h, shape, width: 0.0 }, ink));
        marks.push(Mark::new(MarkKind::SideText { text, style: ts.clone(), x: (tab_w - th) / 2.0, y: y + px(TAB_PAD), w: th, h: tw, clockwise }, InkRole::OnPaper));
        hits.push(Hit { rect: rect(0.0, y, tab_w, h), action: Action::Note(if notes.is_empty() { String::new() } else { name }) });
        y += h + px(TAB_GAP);
    }
    Composition::with_hits(marks, hits)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::layout::tests::Fixed;
    use super::*;
    use chrono::TimeZone as _;
    use mandara_config::desktop_widgets::TextOptions;

    #[test]
    fn notes_keep_automatic_paper_ink_but_honour_an_explicit_text_colour() {
        use mandara_config::desktop_widgets::Tone;
        for sheet in [NoteSheet::Pastel,NoteSheet::Paper,NoteSheet::Custom] {
            let mut widget = NoteWidget { sheet,..Default::default() };
            let text_inks = |widget: &NoteWidget| note(widget,&[],&lingo_in("en"),1.0,&mut Fixed).marks.into_iter()
                .filter(|mark|matches!(mark.kind,MarkKind::Text { .. })).map(|mark|mark.ink).collect::<Vec<_>>();
            let automatic = text_inks(&widget);
            assert!(!automatic.is_empty());
            assert!(automatic.iter().all(|ink|*ink == InkRole::OnPaper));
            widget.look.tone = Tone::Custom;
            widget.look.custom_color = "#553311".into();
            assert!(text_inks(&widget).iter().all(|ink|*ink == InkRole::Fg));
        }
    }
    fn lingo() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    /// A widget language set explicitly, whatever the interface language.
    fn lingo_in(language: &str) -> Lingo {
        Lingo::of(&TextOptions { language: language.into(), ..Default::default() }, false, None)
    }

    fn items() -> Vec<TodoItem> {
        vec![TodoItem { id: 1, text: "Buy bread".into(), done: false, list: 1 }, TodoItem { id: 2, text: "Write the report for the meeting on Thursday afternoon".into(), done: true, list: 2 }]
    }

    #[test]
    fn todo_rows_are_clickable_and_filtered() {
        let lists = vec![(1, "Home".to_string()), (2, "Work".to_string())];
        let c = todo(&TodoWidget::default(), &items(), &lists, &lingo(), 1.0, &mut Fixed);
        assert_eq!(c.hits.iter().filter(|h| matches!(h.action, Action::Todo(_))).count(), 2);
        assert!(c.hits.iter().any(|h| h.action == Action::TodoAdd));
        let home = TodoWidget { list: "home".into(), show_done: false, ..Default::default() };
        let c = todo(&home, &items(), &lists, &lingo(), 1.0, &mut Fixed);
        assert_eq!(c.hits.iter().filter(|h| matches!(h.action, Action::Todo(_))).count(), 1);
        // The long task is cut to the width.
        let c = todo(&TodoWidget::default(), &items(), &lists, &lingo(), 1.0, &mut Fixed);
        assert!(c.size.0 <= ranges::TODO_WIDTH_DEFAULT_PX + 0.5, "{:?}", c.size);
        let hit = c.hits.iter().find(|h| h.action == Action::Todo(2)).unwrap();
        assert_eq!(c.hit((hit.rect.0 + 1.0, hit.rect.1 + 1.0)), Some(&Action::Todo(2)));
    }

    #[test]
    fn checkbox_and_task_text_have_distinct_actions_in_both_reading_directions() {
        for language in ["en", "ar"] {
            let composition = todo(&TodoWidget::default(), &items(), &[], &lingo_in(language), 1.5, &mut Fixed);
            for id in [1, 2] {
                for action in [Action::Todo(id), Action::TodoEdit(id)] {
                    let hit = composition.hits.iter().find(|hit| hit.action == action).unwrap();
                    let (x, y, width, height) = hit.rect;
                    assert_eq!(composition.hit((x + width / 2.0, y + height / 2.0)), Some(&action));
                }
            }
        }
    }

    #[test]
    fn notes_quotes_timer_and_tabs_lay_out() {
        let notes = vec![NoteItem { title: "Ideas".into(), body: "One two three four five six seven eight nine ten eleven twelve".into() }];
        for sheet in [NoteSheet::Theme, NoteSheet::Pastel, NoteSheet::Paper, NoteSheet::Custom] {
            for ruling in [NoteRuling::Plain, NoteRuling::Ruled, NoteRuling::Grid, NoteRuling::Dots] {
                let c = note(&NoteWidget { sheet, ruling, word_count: true, ..Default::default() }, &notes, &lingo(), 1.0, &mut Fixed);
                assert!(c.size.0 > 100.0 && c.hits.len() == 1);
            }
        }
        let now = Local.with_ymd_and_hms(2026, 3, 4, 15, 0, 0).unwrap();
        let p = PomodoroState { phase: tk("Focus"), ends_ms: Some(now.timestamp_millis() + 61_000), paused_ms: 0, length_ms: 1_500_000, running: true, completed: 5, long_every: 4 };
        assert_eq!(timer_left(&p, &now), 61_000);
        for style in [TimerStyle::Ring, TimerStyle::Dots] {
            let c = pomodoro(&PomodoroWidget { style, ..Default::default() }, &p, &now, &lingo(), 1.0, &mut Fixed);
            assert_eq!(c.hits.len(), 3);
            assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "01:01")));
        }
        let q = QuoteWidget { quotes: vec!["Stay hungry — Someone".into(), "Plain words".into()], every: QuoteEvery::Click, ..Default::default() };
        assert_eq!(pick_quote(&q, &now, 0, &lingo()), ("Stay hungry".into(), "Someone".into(), String::new()));
        assert_eq!(pick_quote(&q, &now, 1, &lingo()).0, "Plain words");
        assert!(quote(&QuoteWidget::default(), &now, 0, &lingo(), 1.0, 0.0, &mut Fixed).size.1 > 30.0);
        let c = notebook(&NotebookWidget::default(), &notes, &lingo(), true, 1.0, &mut Fixed);
        assert_eq!(c.hits[0].action, Action::Note("Ideas".into()));
    }

    #[test]
    fn sayings_are_sourced_short_and_all_come_round() {
        let mut texts = HashSet::new();
        for s in SAYINGS {
            assert!(!s.text.is_empty() && !s.author.is_empty(), "{s:?}");
            assert!(s.text.chars().count() <= 125, "too long for a card: {}", s.text);
            assert!(texts.insert(s.text), "twice: {}", s.text);
            // Proverbs have no single source; every other saying names its work.
            assert_eq!(s.work.is_empty(), s.author.ends_with("proverb"), "{s:?}");
        }
        assert!(SAYINGS.len() >= 150);
        // One saying per click, each once per cycle, and never the same
        // author twice in a row.
        let q = QuoteWidget { every: QuoteEvery::Click, ..Default::default() };
        let now = Local.with_ymd_and_hms(2026, 3, 4, 15, 0, 0).unwrap();
        let en = lingo_in("en");
        let shown: Vec<_> = (0..SAYINGS.len() as u32).map(|t| pick_quote(&q, &now, t, &en)).collect();
        assert_eq!(shown.iter().map(|s| &s.0).collect::<HashSet<_>>().len(), SAYINGS.len());
        assert!(shown.windows(2).all(|p| p[0].1 != p[1].1), "same author twice in a row");
        assert!(shown.iter().all(|(t, a, _)| SAYINGS.iter().any(|s| s.text == t && s.author == a)));
        // A day later, another saying.
        let day = QuoteWidget::default();
        assert_ne!(pick_quote(&day, &now, 0, &en).0, pick_quote(&day, &(now + chrono::Duration::days(1)), 0, &en).0);
    }

    #[test]
    fn attribution_names_the_work_when_it_fits() {
        let st = style(QuoteWidget::default().look.font, 20.0, face::SEMIBOLD);
        let en = lingo_in("en");
        assert_eq!(attribution("Seneca", "On Providence", &en, &st, 1000.0, &mut Fixed), "— Seneca, On Providence");
        // Too wide for the card: the author alone.
        assert_eq!(attribution("Seneca", "On Providence", &en, &st, 200.0, &mut Fixed), "— Seneca");
        assert_eq!(attribution("Seneca", "", &en, &st, 1000.0, &mut Fixed), "— Seneca");
        // Each language joins them its own way.
        assert_eq!(attribution("سينيكا", "عن العناية الإلهية", &lingo_in("ar"), &st, 1000.0, &mut Fixed), "— سينيكا، عن العناية الإلهية");
        // The card draws it, and the built-in sayings are translated.
        let q = QuoteWidget { every: QuoteEvery::Click, ..Default::default() };
        let now = Local.with_ymd_and_hms(2026, 3, 4, 15, 0, 0).unwrap();
        let c = quote(&q, &now, 0, &en, 1.0, 0.0, &mut Fixed);
        let (_, author, work) = pick_quote(&q, &now, 0, &en);
        let line = |c: &Composition| c.marks.iter().rev().find_map(|m| match &m.kind {
            MarkKind::Text { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert!(line(&c).is_some_and(|l| l == format!("— {author}, {work}") || l == format!("— {author}")), "{:?}", line(&c));
        let ar = lingo_in("ar");
        for s in SAYINGS {
            assert_ne!(ar.word(s.text), s.text, "untranslated: {}", s.text);
        }
    }
}
