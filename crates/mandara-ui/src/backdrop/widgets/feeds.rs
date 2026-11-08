//! Widgets fed from outside the shell: GitHub's contribution calendar and
//! exchange rates (online, off until switched on), Claude Code's token use
//! (local logs, off until switched on) and your Steam games shelf.

use chrono::{Datelike as _, Local, NaiveDate};
use mandara_config::desktop_widgets::{AiUsageWidget, CurrencyWidget, GamesWidget, GithubWidget};
use mandara_config::widget_kinds::{count, ranges, within};
use mandara_core::shape::Shape;
use mandara_i18n::tk;
use mandara_services::desktop_feeds::{ContributionDay, DayTokens, Rates};

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, Picture, caps, face, fit, style, tracked};
use super::lingo::Lingo;
use super::empty::{self, Empty, Hint};
use super::{Feed, GameItem};

/// Reference sizes.
const CELL: f32 = 12.0;
const CELL_GAP: f32 = 3.0;
const TEXT: f32 = 15.0;
const SMALL: f32 = 12.0;
const BIG: f32 = 40.0;
const GAP: f32 = 10.0;
const ROW: f32 = 34.0;
const RATES_W: f32 = 240.0;
const BAR_W: f32 = 20.0;
const BAR_H: f32 = 90.0;
const BAR_GAP: f32 = 8.0;
const METER_H: f32 = 8.0;
pub const COVER_W: f32 = 96.0;
const COVER_RATIO: f32 = 1.5;
const COVER_GAP: f32 = 12.0;
const TRACKING: f32 = 0.1;
/// Strength of each contribution level (0 = empty).
const LEVEL_ALPHA: [f32; 5] = [0.1, 0.35, 0.55, 0.78, 1.0];
const DAYS_PER_WEEK: usize = 7;
/// Days the AI usage feed reads at least (the week's total).
pub const WEEK_DAYS: u32 = 7;
const MILLION: f64 = 1_000_000.0;
const THOUSAND: f64 = 1_000.0;
const PERCENT: f32 = 100.0;

/// What a feed widget shows instead of its data: switched off (with the
/// setting that fixes it, `off` its hint), on its way, or why it failed
/// (a click asks again).
fn status<T>(feed: &Feed<T>, off: Option<&'static str>, icon: &'static str, title: &'static str, id: &str) -> Option<Empty> {
    if let Some(hint) = off {
        return Some(Empty::new(icon, title, hint).setup(empty::widget_settings(id)));
    }
    match (&feed.value, &feed.error) {
        (Some(_), _) => None,
        (None, Some(e)) => Some(Empty { hint: Hint::Text(e.clone()), click: Some(Action::Refresh), ..Empty::new(icon, tk("Couldn't load it"), tk("Loading…")) }),
        (None, None) => Some(Empty::new(icon, title, tk("Loading…"))),
    }
}

/// Why a GitHub widget can't ask yet: no user, or fetching is off.
pub(super) fn github_off(w: &GithubWidget) -> Option<&'static str> {
    if w.user.trim().is_empty() {
        Some(tk("Add your GitHub user name to see your year"))
    } else if !w.online {
        Some(tk("Allow fetching from GitHub to see your year"))
    } else {
        None
    }
}

/// GitHub's contribution calendar: weeks as columns, a cell per day in
/// the accent by level, the total and the current streak under it.
pub fn github(w: &GithubWidget, id: &str, feed: &Feed<Vec<ContributionDay>>, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    if let Some(e) = status(feed, github_off(w), "github", tk("GitHub contributions"), id) {
        return empty::layout(&e, w.look.font, l, k, m);
    }
    let days = feed.value.as_deref().unwrap_or_default();
    let weeks = count(w.weeks, ranges::WEEKS) as usize;
    let today = Local::now().date_naive();
    // The last `weeks` columns, ending with this week (Sunday first, as
    // GitHub draws it).
    let end_col = today + chrono::Duration::days(i64::from(6 - today.weekday().num_days_from_sunday()));
    let first = end_col - chrono::Duration::days((weeks * DAYS_PER_WEEK) as i64 - 1);
    let (c, g) = (px(CELL), px(CELL_GAP));
    let shape = match shape_of(w.cell, &w.look, k) {
        Shape::Rect { .. } => Shape::Rect { radius: c * 0.25 },
        s => s,
    };
    let mut marks = Vec::new();
    for d in days.iter().filter(|d| d.date >= first && d.date <= today) {
        let i = (d.date - first).num_days() as usize;
        let (col, row) = (i / DAYS_PER_WEEK, i % DAYS_PER_WEEK);
        let col = if l.rtl { weeks - 1 - col } else { col };
        let ink = if d.level == 0 { InkRole::Fg } else { InkRole::Accent };
        marks.push(Mark::new(MarkKind::Shaped { x: col as f32 * (c + g), y: row as f32 * (c + g), w: c, h: c, shape, width: 0.0 }, ink).dim(LEVEL_ALPHA[usize::from(d.level.min(4))]));
    }
    // The grid's full size even where days are missing.
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: weeks as f32 * (c + g) - g, h: DAYS_PER_WEEK as f32 * (c + g) - g, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    if w.summary {
        let year: u32 = days.iter().map(|d| d.count).sum();
        let streak = days.iter().rev().skip_while(|d| d.date > today || (d.date == today && d.level == 0)).take_while(|d| d.level > 0).count();
        let text = l.digits(l.word(tk("{count} contributions in the last year · {days}-day streak")).replace("{count}", &year.to_string()).replace("{days}", &streak.to_string()));
        let st = style(w.look.font, px(SMALL), face::MEDIUM);
        let (tw, th) = m.measure(&text, &st);
        let y = DAYS_PER_WEEK as f32 * (c + g) + px(GAP) / 2.0;
        marks.push(Mark::new(MarkKind::Text { text, style: st, x: if l.rtl { weeks as f32 * (c + g) - g - tw } else { 0.0 }, y, w: tw, h: th }, InkRole::Fg).dim(face::DIM));
    }
    let mut comp = Composition::of(marks);
    comp.hits.push(Hit { rect: (0.0, 0.0, comp.size.0, comp.size.1), action: Action::Open(format!("https://github.com/{}", w.user.trim())) });
    comp
}

/// Exchange rates: what one unit of the base buys, one currency a row.
pub fn currency(w: &CurrencyWidget, id: &str, feed: &Feed<Rates>, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let off = (!w.online).then_some(tk("Allow fetching rates to see today's prices"));
    if let Some(e) = status(feed, off, "globe", tk("Exchange rates"), id) {
        return empty::layout(&e, w.look.font, l, k, m);
    }
    let Some(r) = feed.value.as_ref() else { return Composition::default() };
    let width = px(RATES_W);
    let hs = tracked(style(w.look.font, px(SMALL), face::BOLD), TRACKING);
    let head = caps(&l.digits(format!("1 {}", r.base)));
    let (hw, hh) = m.measure(&head, &hs);
    let mut marks = vec![Mark::new(MarkKind::Text { text: head, style: hs, x: if l.rtl { width - hw } else { 0.0 }, y: 0.0, w: hw, h: hh }, InkRole::Accent)];
    let cs = style(w.look.font, px(TEXT), face::SEMIBOLD);
    let vs = style(mandara_config::desktop_widgets::FontRole::Mono, px(TEXT) * 1.2, face::MEDIUM);
    let decimals = count(w.decimals, ranges::DECIMALS) as usize;
    let mut y = hh + px(GAP) / 2.0;
    for code in &w.pairs {
        let code = code.trim().to_ascii_uppercase();
        let value = r.rates.get(&code).map_or_else(|| "—".to_string(), |v| l.digits(format!("{v:.decimals$}")));
        let (cw, ch) = m.measure(&code, &cs);
        let (vw, vh) = m.measure(&value, &vs);
        let row = px(ROW).max(ch).max(vh);
        let (cx, vx) = if l.rtl { (width - cw, 0.0) } else { (0.0, width - vw) };
        marks.push(Mark::new(MarkKind::Text { text: code, style: cs.clone(), x: cx, y: y + (row - ch) / 2.0, w: cw, h: ch }, InkRole::Fg));
        marks.push(Mark::new(MarkKind::Text { text: value, style: vs.clone(), x: vx, y: y + (row - vh) / 2.0, w: vw, h: vh }, InkRole::Fg));
        marks.push(Mark::new(MarkKind::Line { a: (0.0, y + row), b: (width, y + row), width: 1.0 }, InkRole::Fg).dim(LEVEL_ALPHA[0]));
        y += row;
    }
    let when = chrono::DateTime::from_timestamp(r.updated, 0).map(|t| l.date(&t.with_timezone(&Local).date_naive(), tk("%-d %b"))).unwrap_or_default();
    let foot = l.word(tk("Rates by ExchangeRate-API · {date}")).replace("{date}", &when);
    let fs = style(w.look.font, px(SMALL) * 0.9, face::MEDIUM);
    let foot = fit(&foot, &fs, width, m);
    let (fw, fh) = m.measure(&foot, &fs);
    marks.push(Mark::new(MarkKind::Text { text: foot, style: fs, x: if l.rtl { width - fw } else { 0.0 }, y: y + px(GAP) / 2.0, w: fw, h: fh }, InkRole::Fg).dim(face::DIM));
    let mut c = Composition::of(marks);
    c.hits.push(Hit { rect: (0.0, 0.0, c.size.0, c.size.1), action: Action::Refresh });
    c
}

/// `1.2 M`, `840 K`, `512` tokens.
pub fn tokens(n: u64, l: &Lingo) -> String {
    let n = n as f64;
    l.digits(if n >= MILLION {
        format!("{:.1} M", n / MILLION)
    } else if n >= THOUSAND {
        format!("{:.0} K", n / THOUSAND)
    } else {
        format!("{n:.0}")
    })
}

/// Claude Code's tokens: today big, the last days as bars, the week
/// against a budget when there is one.
pub fn ai_usage(w: &AiUsageWidget, id: &str, feed: &Feed<std::collections::BTreeMap<NaiveDate, DayTokens>>, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let off = (!w.read_logs).then_some(tk("Allow reading Claude Code's local logs to see your tokens"));
    if let Some(e) = status(feed, off, "sparkles", tk("AI usage"), id) {
        return empty::layout(&e, w.look.font, l, k, m);
    }
    let per_day = feed.value.clone().unwrap_or_default();
    let today = Local::now().date_naive();
    let days = count(w.days, ranges::AI_DAYS) as i64;
    let series: Vec<(NaiveDate, u64)> = (0..days).rev().map(|back| today - chrono::Duration::days(back)).map(|d| (d, per_day.get(&d).map_or(0, DayTokens::total))).collect();
    let ls = tracked(style(w.look.font, px(SMALL), face::BOLD), TRACKING);
    let label = caps(&l.word(tk("Claude Code today")));
    let (lw, lh) = m.measure(&label, &ls);
    let bs = style(w.look.font, px(BIG), face::SEMIBOLD);
    let big = tokens(series.last().map_or(0, |s| s.1), l);
    let (bw, bh) = m.measure(&big, &bs);
    let n = series.len();
    let width = (n as f32 * (px(BAR_W) + px(BAR_GAP))).max(lw).max(bw);
    let ax = |w0: f32| if l.rtl { width - w0 } else { 0.0 };
    let mut marks = vec![
        Mark::new(MarkKind::Text { text: label, style: ls, x: ax(lw), y: 0.0, w: lw, h: lh }, InkRole::Accent),
        Mark::new(MarkKind::Text { text: big, style: bs, x: ax(bw), y: lh, w: bw, h: bh }, InkRole::Fg),
    ];
    let y = lh + bh + px(GAP);
    let most = series.iter().map(|s| s.1).max().unwrap_or(1).max(1) as f32;
    for (i, (d, v)) in series.iter().enumerate() {
        let x = i as f32 * (px(BAR_W) + px(BAR_GAP));
        let h = (px(BAR_H) * *v as f32 / most).max(px(BAR_W) / 3.0);
        marks.push(Mark::new(MarkKind::Rect { x, y, w: px(BAR_W), h: px(BAR_H), radius: px(BAR_W) / 3.0, width: 0.0 }, InkRole::Fg).dim(LEVEL_ALPHA[0]));
        marks.push(Mark::new(MarkKind::Rect { x, y: y + px(BAR_H) - h, w: px(BAR_W), h, radius: px(BAR_W) / 3.0, width: 0.0 }, if *d == today { InkRole::Accent } else { InkRole::Fg }).dim(if *d == today { 1.0 } else { face::DIM }));
    }
    let mut bottom = y + px(BAR_H) + px(GAP);
    let week: u64 = series.iter().rev().take(DAYS_PER_WEEK).map(|s| s.1).sum();
    let ss = style(w.look.font, px(SMALL), face::MEDIUM);
    let budget = within(w.budget_millions, ranges::AI_BUDGET_M, 0.0) as f64 * MILLION;
    let line = if budget > 0.0 {
        let f = (week as f64 / budget) as f32;
        marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: bottom, w: width, h: px(METER_H), radius: px(METER_H) / 2.0, width: 0.0 }, InkRole::Fg).dim(LEVEL_ALPHA[0]));
        let fw = (width * f.clamp(0.0, 1.0)).max(px(METER_H));
        marks.push(Mark::new(MarkKind::Rect { x: if l.rtl { width - fw } else { 0.0 }, y: bottom, w: fw, h: px(METER_H), radius: px(METER_H) / 2.0, width: 0.0 }, InkRole::Accent));
        bottom += px(METER_H) + px(GAP) / 2.0;
        l.word(tk("This week {used} of {budget} ({percent}%)")).replace("{used}", &tokens(week, l)).replace("{budget}", &tokens(budget as u64, l)).replace("{percent}", &l.digits(((f * PERCENT).round() as i32).to_string()))
    } else {
        l.word(tk("This week {used}")).replace("{used}", &tokens(week, l))
    };
    let (tw, th) = m.measure(&line, &ss);
    marks.push(Mark::new(MarkKind::Text { text: line, style: ss, x: ax(tw), y: bottom, w: tw, h: th }, InkRole::Fg).dim(face::DIM));
    Composition::of(marks)
}

/// Your Steam games as a shelf of covers; a click starts one.
pub fn games(w: &GamesWidget, feed: &Feed<Vec<GameItem>>, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let list = feed.value.as_deref().unwrap_or_default();
    if list.is_empty() {
        let e = if feed.loading { Empty::new("dice", tk("Games"), tk("Loading…")) } else { Empty::new("dice", tk("No Steam games found"), tk("Games you install in Steam show up here")) };
        return empty::layout(&e, w.look.font, l, k, m);
    }
    let (cw, ch) = (px(COVER_W), px(COVER_W) * COVER_RATIO);
    let shape = match shape_of(w.cover, &w.look, k) {
        Shape::Rect { radius } => Shape::Rect { radius: radius.min(cw / 6.0) },
        s => s,
    };
    let ns = style(w.look.font, px(SMALL), face::MEDIUM);
    let mut marks = Vec::new();
    let mut hits = Vec::new();
    let shown: Vec<&GameItem> = list.iter().take(count(w.count, ranges::GAMES) as usize).collect();
    let n = shown.len();
    for (i, g) in shown.into_iter().enumerate() {
        let col = if l.rtl { n - 1 - i } else { i };
        let x = col as f32 * (cw + px(COVER_GAP));
        cover(g.cover.as_ref(), x, cw, ch, shape, &g.name, &mut marks, &ns, m);
        let mut h = ch;
        if w.names {
            let name = fit(&g.name, &ns, cw, m);
            let (nw, nh) = m.measure(&name, &ns);
            marks.push(Mark::new(MarkKind::Text { text: name, style: ns.clone(), x: x + (cw - nw) / 2.0, y: ch + px(GAP) / 2.0, w: nw, h: nh }, InkRole::Fg).dim(face::DIM));
            h += px(GAP) / 2.0 + nh;
        }
        hits.push(Hit { rect: (x, 0.0, cw, h), action: Action::Open(format!("steam://rungameid/{}", g.appid)) });
    }
    Composition::with_hits(marks, hits)
}

/// A game's cover, or its name on a plain card when Steam has none.
fn cover(pic: Option<&Picture>, x: f32, w: f32, h: f32, shape: Shape, name: &str, marks: &mut Vec<Mark>, st: &super::layout::TextStyle, m: &mut dyn Measure) {
    match pic {
        Some(p) => marks.push(Mark::new(MarkKind::Image { x, y: 0.0, w, h, pic: p.clone(), shape, turn: 0.0 }, InkRole::Fg)),
        None => {
            marks.push(Mark::new(MarkKind::Shaped { x, y: 0.0, w, h, shape, width: 0.0 }, InkRole::Plate));
            let t = fit(name, st, w * 0.9, m);
            let (tw, th) = m.measure(&t, st);
            marks.push(Mark::new(MarkKind::Text { text: t, style: st.clone(), x: x + (w - tw) / 2.0, y: (h - th) / 2.0, w: tw, h: th }, InkRole::OnPlate));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn l() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    /// Every line a composition writes, joined (a wrapped hint reads whole).
    fn shown(c: &Composition) -> String {
        c.marks.iter().filter_map(|m| if let MarkKind::Text { text, .. } = &m.kind { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn feeds_say_why_they_are_empty_and_lay_out_when_full() {
        let off = github(&GithubWidget::default(), "github-1", &Feed::default(), &l(), 1.0, &mut Fixed);
        assert!(shown(&off).contains("GitHub user name"));
        let today = Local::now().date_naive();
        let days: Vec<ContributionDay> = (0..60).map(|i| ContributionDay { date: today - chrono::Duration::days(i), level: (i % 5) as u8, count: (i % 5) as u32 }).collect();
        let w = GithubWidget { user: "octocat".into(), online: true, ..Default::default() };
        let c = github(&w, "github-1", &Feed { value: Some(days), error: None, loading: false }, &l(), 1.0, &mut Fixed);
        assert!(c.size.0 > 100.0 && c.hits.len() == 1);
        let rates = Rates { base: "USD".into(), rates: [("EGP".to_string(), 48.7), ("EUR".to_string(), 0.92)].into_iter().collect(), updated: 1_700_000_000 };
        let c = currency(&CurrencyWidget { online: true, ..Default::default() }, "currency-1", &Feed { value: Some(rates), error: None, loading: false }, &l(), 1.0, &mut Fixed);
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "48.70")));
        assert_eq!(tokens(1_250_000, &l()), "1.2 M");
        let usage: std::collections::BTreeMap<NaiveDate, DayTokens> = [(today, DayTokens { input: 10, output: 20, cache: 1_000_000 })].into_iter().collect();
        let c = ai_usage(&AiUsageWidget { read_logs: true, budget_millions: 10.0, ..Default::default() }, "ai-1", &Feed { value: Some(usage), error: None, loading: false }, &l(), 1.0, &mut Fixed);
        assert!(c.size.1 > 100.0);
        let g = vec![GameItem { appid: 570, name: "Dota 2".into(), cover: None }];
        let c = games(&GamesWidget::default(), &Feed { value: Some(g), error: None, loading: false }, &l(), 1.0, &mut Fixed);
        assert_eq!(c.hits[0].action, Action::Open("steam://rungameid/570".into()));
    }

    #[test]
    fn empty_feeds_pick_the_state_that_fits() {
        let studio = |c: &Composition| c.hits.iter().find_map(|h| if let Action::Studio(t) = &h.action { Some(t.clone()) } else { None });
        let says = |c: &Composition, words: &str| shown(c).contains(words);
        // Switched off: a button to the exact widget's settings.
        let c = github(&GithubWidget { user: "octocat".into(), ..Default::default() }, "github-2", &Feed::default(), &l(), 1.0, &mut Fixed);
        assert_eq!(studio(&c).as_deref(), Some("desktop-widgets/github-2"));
        assert!(says(&c, "Allow fetching from GitHub"));
        assert_eq!(studio(&currency(&CurrencyWidget::default(), "currency-1", &Feed::default(), &l(), 1.0, &mut Fixed)).as_deref(), Some("desktop-widgets/currency-1"));
        assert_eq!(studio(&ai_usage(&AiUsageWidget::default(), "ai-1", &Feed::default(), &l(), 1.0, &mut Fixed)).as_deref(), Some("desktop-widgets/ai-1"));
        // On and waiting: no button; failed: the reason, and a click asks again.
        let on = CurrencyWidget { online: true, ..Default::default() };
        let loading = currency(&on, "c", &Feed { value: None, error: None, loading: true }, &l(), 1.0, &mut Fixed);
        assert!(studio(&loading).is_none() && says(&loading, "Loading…"));
        let failed = currency(&on, "c", &Feed { value: None, error: Some("timed out".into()), loading: false }, &l(), 1.0, &mut Fixed);
        assert!(says(&failed, "timed out") && says(&failed, "Couldn't load it"));
        assert_eq!(failed.hits.last().map(|h| &h.action), Some(&Action::Refresh));
        // No games: an explanation, nothing to set up.
        let none = games(&GamesWidget::default(), &Feed::default(), &l(), 1.0, &mut Fixed);
        assert!(none.hits.is_empty() && says(&none, "No Steam games found"));
    }
}
