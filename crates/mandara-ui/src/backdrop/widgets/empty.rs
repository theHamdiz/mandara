//! The painted desktop widgets' empty state: what a widget shows when it
//! needs setting up, has nothing yet or is still loading.
//!
//! The panels' empty state ([`crate::plugins::panel_fx::empty_state`])
//! drawn with the widget renderer's marks: soft rings of the accent around
//! the widget's icon, a short title, one helpful line and, when a setting
//! fixes it, a "Set up in Studio" button that opens that exact page. The
//! inks follow the widget's own (automatic over the wallpaper, or its
//! plate), so it reads on light and dark wallpapers alike. A composition
//! is built only when the widget's content changes, never per frame.

use mandara_config::desktop_widgets::FontRole;
use mandara_i18n::tk;

use super::actions::Action;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, face, style, wrap};
use super::lingo::Lingo;

/// Ring diameters and strengths around the icon, reference px (the panels'
/// rings, a little stronger: they sit on a wallpaper, not a surface).
const RING_D: [f32; 3] = [72.0, 54.0, 38.0];
const RING_ALPHA: [f32; 3] = [0.1, 0.16, 0.26];
const ICON: f32 = 20.0;
const TITLE: f32 = 17.0;
const HINT: f32 = 13.5;
/// The hint wraps at this width, at most this many lines.
const HINT_W: f32 = 250.0;
const HINT_LINES: usize = 2;
/// A wrapped hint's lines aim for an even share of it, with this slack
/// for word breaks.
const HINT_BALANCE: f32 = 1.12;
const RING_GAP: f32 = 12.0;
const LINE_GAP: f32 = 3.0;
const BUTTON_GAP: f32 = 14.0;
const BUTTON_H: f32 = 32.0;
const BUTTON_PAD: f32 = 14.0;
const BUTTON_ICON: f32 = 14.0;
const BUTTON_ICON_GAP: f32 = 7.0;
const BUTTON_TEXT: f32 = 13.0;

/// Why a widget has nothing to show, in the widget's own words.
#[derive(Clone, Debug, PartialEq)]
pub struct Empty {
    pub icon: &'static str,
    /// Catalog keys, written in the widget's language when laid out.
    pub title: &'static str,
    pub hint: Hint,
    /// The Studio page that fixes it (`desktop/widgets/<id>`,
    /// `plugins/<id>`): a "Set up in Studio" button opens it.
    pub setup: Option<String>,
    /// What a click anywhere on it does (fetch again after an error).
    pub click: Option<Action>,
}

/// The helpful line: a catalog key, or text that comes from elsewhere (an
/// error message), shown as it is.
#[derive(Clone, Debug, PartialEq)]
pub enum Hint {
    Key(&'static str),
    Text(String),
}

impl Empty {
    pub fn new(icon: &'static str, title: &'static str, hint: &'static str) -> Self {
        Self { icon, title, hint: Hint::Key(hint), setup: None, click: None }
    }

    /// With a "Set up in Studio" button opening `target`.
    pub fn setup(mut self, target: String) -> Self {
        self.setup = Some(target);
        self
    }
}

/// The Studio target of one desktop widget's settings.
pub fn widget_settings(id: &str) -> String {
    format!("desktop/widgets/{id}")
}

/// Lays out `e` centred, in the widget's `font` and language.
pub fn layout(e: &Empty, font: FontRole, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let ring = px(RING_D[0]);
    let ts = style(font, px(TITLE), face::SEMIBOLD);
    let hs = style(font, px(HINT), face::MEDIUM);
    let title = l.word(e.title);
    let (tw, th) = m.measure(&title, &ts);
    let hint = match &e.hint {
        Hint::Key(key) => l.digits(l.word(key)),
        Hint::Text(text) => text.clone(),
    };
    // Two lines of about the same length, not a long line and a lone word.
    let whole = m.measure(&hint, &hs).0;
    let balanced = if whole > px(HINT_W) { (whole / HINT_LINES as f32 * HINT_BALANCE).min(px(HINT_W)) } else { px(HINT_W) };
    let mut wrapped = wrap(&hint, &hs, balanced, HINT_LINES, m);
    if wrapped.last().is_some_and(|l| l.ends_with('…')) {
        wrapped = wrap(&hint, &hs, px(HINT_W), HINT_LINES, m);
    }
    let lines: Vec<(String, (f32, f32))> = wrapped.into_iter().map(|line| {
        let size = m.measure(&line, &hs);
        (line, size)
    }).collect();
    let button = e.setup.as_ref().map(|target| {
        let bs = style(font, px(BUTTON_TEXT), face::SEMIBOLD);
        let label = l.word(tk("Set up in Studio"));
        let (lw, lh) = m.measure(&label, &bs);
        (target, bs, label, (lw, lh), px(BUTTON_PAD) * 2.0 + px(BUTTON_ICON) + px(BUTTON_ICON_GAP) + lw)
    });
    let width = lines.iter().map(|(_, (w, _))| *w).fold(tw.max(ring), f32::max).max(button.as_ref().map_or(0.0, |b| b.4));
    let cx = width / 2.0;
    let mut marks: Vec<Mark> = RING_D.iter().zip(RING_ALPHA).map(|(d, a)| Mark::new(MarkKind::Disc { c: (cx, ring / 2.0), r: px(*d) / 2.0 }, InkRole::Accent).dim(a)).collect();
    marks.push(Mark::new(MarkKind::Icon { name: e.icon, x: cx - px(ICON) / 2.0, y: (ring - px(ICON)) / 2.0, size: px(ICON) }, InkRole::Accent));
    let mut y = ring + px(RING_GAP);
    marks.push(Mark::new(MarkKind::Text { text: title, style: ts, x: cx - tw / 2.0, y, w: tw, h: th }, InkRole::Fg));
    y += th + px(LINE_GAP);
    for (line, (lw, lh)) in lines {
        marks.push(Mark::new(MarkKind::Text { text: line, style: hs.clone(), x: cx - lw / 2.0, y, w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
        y += lh;
    }
    let mut hits = Vec::new();
    if let Some((target, bs, label, (lw, lh), bw)) = button {
        let (bx, by, bh) = (cx - bw / 2.0, y + px(BUTTON_GAP), px(BUTTON_H));
        marks.push(Mark::new(MarkKind::Rect { x: bx, y: by, w: bw, h: bh, radius: bh / 2.0, width: 0.0 }, InkRole::Accent));
        // The icon leads in the reading direction.
        let (ix, lx) = if l.rtl { (bx + bw - px(BUTTON_PAD) - px(BUTTON_ICON), bx + px(BUTTON_PAD)) } else { (bx + px(BUTTON_PAD), bx + px(BUTTON_PAD) + px(BUTTON_ICON) + px(BUTTON_ICON_GAP)) };
        marks.push(Mark::new(MarkKind::Icon { name: "sliders", x: ix, y: by + (bh - px(BUTTON_ICON)) / 2.0, size: px(BUTTON_ICON) }, InkRole::OnAccent));
        marks.push(Mark::new(MarkKind::Text { text: label, style: bs, x: lx, y: by + (bh - lh) / 2.0, w: lw, h: lh }, InkRole::OnAccent));
        hits.push(Hit { rect: (bx, by, bw, bh), action: Action::Studio(target.clone()) });
    }
    let mut comp = Composition::with_hits(marks, hits);
    if let Some(action) = e.click.clone() {
        comp.hits.push(Hit { rect: (0.0, 0.0, comp.size.0, comp.size.1), action });
    }
    comp
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn l() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    #[test]
    fn an_empty_state_has_rings_an_icon_a_title_a_hint_and_its_studio_button() {
        let e = Empty::new("github", tk("GitHub contributions"), tk("Add your GitHub user name to see your year")).setup(widget_settings("github-1"));
        let c = layout(&e, FontRole::Widgets, &l(), 1.0, &mut Fixed);
        assert_eq!(c.marks.iter().filter(|m| matches!(m.kind, MarkKind::Disc { .. })).count(), RING_D.len());
        assert!(c.marks.iter().any(|m| matches!(m.kind, MarkKind::Icon { name: "github", .. }) && m.ink == InkRole::Accent));
        let texts: Vec<&str> = c.marks.iter().filter_map(|m| if let MarkKind::Text { text, .. } = &m.kind { Some(text.as_str()) } else { None }).collect();
        assert!(texts.contains(&"GitHub contributions") && texts.contains(&"Set up in Studio"), "{texts:?}");
        assert_eq!(c.hits.len(), 1);
        assert_eq!(c.hits[0].action, Action::Studio("desktop/widgets/github-1".into()));
        // The button sits inside the composition, under the text.
        let (x, y, w, h) = c.hits[0].rect;
        assert!(x >= -0.01 && y > 0.0 && x + w <= c.size.0 + 0.01 && y + h <= c.size.1 + 0.01);
        // The hint breaks into two even lines, not a line and a lone word.
        let hint: Vec<f32> = c.marks.iter().filter_map(|m| match &m.kind { MarkKind::Text { style, w, .. } if (style.size - HINT).abs() < 0.01 => Some(*w), _ => None }).collect();
        assert_eq!(hint.len(), 2);
        assert!(hint[1] > hint[0] * 0.6, "{hint:?}");
    }

    #[test]
    fn without_a_setting_to_change_there_is_no_button_and_long_hints_wrap() {
        let e = Empty { hint: Hint::Text("word ".repeat(60)), ..Empty::new("dice", tk("Games"), tk("Loading…")) };
        let c = layout(&e, FontRole::Widgets, &l(), 1.0, &mut Fixed);
        assert!(c.hits.is_empty());
        let lines = c.marks.iter().filter(|m| matches!(m.kind, MarkKind::Text { .. })).count();
        assert_eq!(lines, 1 + HINT_LINES, "a title and the hint's capped lines");
        assert!(c.size.0 <= HINT_W.max(RING_D[0]) + 0.01, "{} wide", c.size.0);
    }

    #[test]
    fn a_click_anywhere_can_retry() {
        let e = Empty { click: Some(Action::Refresh), ..Empty::new("globe", tk("Exchange rates"), tk("Loading…")) };
        let c = layout(&e, FontRole::Widgets, &l(), 2.0, &mut Fixed);
        assert_eq!(c.hits.last().map(|h| &h.action), Some(&Action::Refresh));
    }
}
