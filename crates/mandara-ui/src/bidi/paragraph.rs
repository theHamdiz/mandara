//! Wrapped paragraphs that may contain right-to-left text.
//!
//! GPUI wraps a paragraph by walking the glyphs of its single shaped line in
//! storage order and cutting where `x` passes the wrap width
//! (`LineLayout::compute_wrap_boundaries`). That assumes `x` grows with the
//! glyph index. A right-to-left paragraph is shaped with `x` *falling* from
//! the line width to 0, so the second glyph already "passes" the width (a
//! one-letter first row) and every later width comes out negative (the rest
//! never wraps and overflows). Its char-width line wrapper also treats every
//! Arabic letter as a break opportunity (`is_word_char` knows Latin,
//! Cyrillic and Bengali only) and measures letters in their isolated forms.
//!
//! [`paragraph`] wraps by itself instead: it measures whole words with the
//! real shaper (Arabic joins inside words, never across spaces), breaks
//! rows at spaces (characters only inside a word wider than the row), and
//! shapes each row as its own line, so the bidi algorithm orders every row
//! on its own, as UAX #9 asks. A row that opens with a word of the other
//! direction gets an invisible mark so it keeps the paragraph's direction.
//! It follows the inherited text style (font, size, color, line height,
//! alignment, `line_clamp`, `white_space`, `text_overflow`) like GPUI text.
//! Text without right-to-left characters stays plain GPUI text.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, HighlightStyle,
    InspectorElementId, InteractiveText, IntoElement, LayoutId, MouseButton, MouseUpEvent, Pixels, Point, ShapedLine,
    SharedString, Size, StyledText, TextAlign, TextRun, TextStyle, WhiteSpace, Window, point, px,
};
use smallvec::SmallVec;

/// Right-to-left / left-to-right marks (zero width, strong direction).
const RLM: char = '\u{200F}';
const LRM: char = '\u{200E}';
/// Appended to a clamped paragraph's last row.
const ELLIPSIS: &str = "…";
/// Width slack before a laid-out paragraph is re-wrapped at paint time.
const REWRAP_EPSILON_PX: f32 = 0.5;

/// A wrapping paragraph of `text` (see the module docs).
pub fn paragraph(text: impl Into<SharedString>) -> AnyElement {
    styled_paragraph(text, Vec::new())
}

/// [`paragraph`] with styled byte ranges (bold, links, …), sorted and
/// non-overlapping, on char boundaries.
pub fn styled_paragraph(text: impl Into<SharedString>, highlights: Vec<(Range<usize>, HighlightStyle)>) -> AnyElement {
    let text = text.into();
    if !super::has_rtl(&text) {
        return StyledText::new(text).with_highlights(highlights).into_any_element();
    }
    Paragraph { text, highlights, families: Vec::new(), links: None, layout: Rc::default(), handle: None }.into_any_element()
}

/// The laid-out rows of an editable paragraph, shared with its editor:
/// caret positions, hit tests and selection rectangles in window space,
/// valid after the paragraph's prepaint (e.g. in a later sibling's paint).
#[derive(Clone, Default)]
pub struct ParagraphLayout {
    rows: Rc<RefCell<Option<Rows>>>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// An editor's text: always this module's own layout (whatever the
/// direction), reporting its rows through `layout`.
pub fn editable_paragraph(text: impl Into<SharedString>, layout: &ParagraphLayout) -> AnyElement {
    Paragraph { text: text.into(), highlights: Vec::new(), families: Vec::new(), links: None, layout: layout.rows.clone(), handle: Some(layout.clone()) }.into_any_element()
}

impl ParagraphLayout {
    /// Where a caret before byte `index` is drawn: a `width`-wide bar the
    /// height of a line.
    pub fn caret_bounds(&self, index: usize, width: Pixels) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds.get()?;
        let rows = self.rows.borrow();
        let rows = rows.as_ref()?;
        let r = rows.row_of(index);
        let x = rows.caret_x(r, index, bounds);
        Some(Bounds::new(point(x - width / 2.0, bounds.origin.y + rows.line_height * r as f32), gpui::size(width, rows.line_height)))
    }

    /// The caret index (a char boundary) nearest to `pos`.
    pub fn caret_index_at(&self, pos: Point<Pixels>) -> Option<usize> {
        let bounds = self.bounds.get()?;
        let rows = self.rows.borrow();
        rows.as_ref().map(|rows| rows.caret_index_at(bounds, pos))
    }

    /// Rectangles covering the byte range `range`, one per row it touches.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        self.rows.borrow().as_ref().map_or_else(Vec::new, |r| r.selection_rects(self.bounds.get(), &range))
    }
}

/// Called with the index of the clicked link range.
pub type LinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// [`styled_paragraph`] with font families for byte ranges (`families`,
/// e.g. inline code in the monospace font) and clickable `links` ranges:
/// `on_click` gets the index of the range clicked (`id` keys GPUI's
/// interactive text for left-to-right paragraphs). Both lists are sorted
/// and non-overlapping, on char boundaries.
pub fn linked_paragraph(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    families: Vec<(Range<usize>, SharedString)>,
    links: Vec<Range<usize>>,
    on_click: LinkHandler,
) -> AnyElement {
    let text = text.into();
    if !super::has_rtl(&text) {
        let styled = StyledText::new(text).with_highlights(highlights);
        let styled = if families.is_empty() { styled } else { styled.with_font_family_overrides(families) };
        if links.is_empty() {
            return styled.into_any_element();
        }
        return InteractiveText::new(id, styled).on_click(links, move |ix, window, cx| on_click(ix, window, cx)).into_any_element();
    }
    let links = (!links.is_empty()).then_some((links, on_click));
    Paragraph { text, highlights, families, links, layout: Rc::default(), handle: None }.into_any_element()
}

/// The element behind [`paragraph`] for text with right-to-left characters.
pub struct Paragraph {
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    families: Vec<(Range<usize>, SharedString)>,
    links: Option<(Vec<Range<usize>>, LinkHandler)>,
    layout: Rc<RefCell<Option<Rows>>>,
    /// Set for an editor's text ([`editable_paragraph`]).
    handle: Option<ParagraphLayout>,
}

/// What a layout pass needs to (re)wrap the paragraph.
struct Inputs {
    text: SharedString,
    runs: Vec<TextRun>,
    font_size: Pixels,
    rtl: bool,
    wrap: bool,
    max_lines: Option<usize>,
    ellipsize: bool,
}

/// The paragraph wrapped at one width.
struct Rows {
    inputs: Rc<Inputs>,
    wrap_width: Option<Pixels>,
    lines: Vec<ShapedLine>,
    /// Per row: where its text starts in the paragraph, and the bytes of
    /// direction mark shaped in front of it.
    starts: Vec<(usize, usize)>,
    /// Per row: where its text (before any ellipsis) ends.
    ends: Vec<usize>,
    line_height: Pixels,
    align: TextAlign,
    size: Size<Pixels>,
}

impl IntoElement for Paragraph {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Paragraph {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, _: &mut App) -> (LayoutId, ()) {
        let style = window.text_style();
        let rem = window.rem_size();
        let font_size = style.font_size.to_pixels(rem);
        let line_height = window.pixel_snap(style.line_height.to_pixels(font_size.into(), rem));
        let rtl = super::text_is_rtl(&self.text);
        let inputs = Rc::new(Inputs {
            runs: with_families(runs(&self.text, &style, std::mem::take(&mut self.highlights)), &self.families),
            text: self.text.clone(),
            font_size,
            rtl,
            wrap: style.white_space == WhiteSpace::Normal,
            max_lines: style.line_clamp,
            ellipsize: style.line_clamp.is_some() || style.text_overflow.is_some(),
        });
        let align = paragraph_align(style.text_align, super::rtl(), rtl);
        let state = self.layout.clone();
        let id = window.request_measured_layout(Default::default(), move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                AvailableSpace::MinContent => Some(Pixels::ZERO),
                AvailableSpace::MaxContent => None,
            });
            let rows = layout_rows(&inputs, width, line_height, align, window);
            let size = rows.size;
            state.replace(Some(rows));
            size
        });
        (id, ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, _: &mut App) {
        if let Some(handle) = &self.handle {
            handle.bounds.set(Some(bounds));
        }
        let mut state = self.layout.borrow_mut();
        let Some(rows) = state.as_ref() else { return };
        let stale = rows.inputs.wrap && rows.wrap_width.is_none_or(|w| (w - bounds.size.width).abs() > px(REWRAP_EPSILON_PX));
        if stale {
            let rewrapped = layout_rows(&rows.inputs.clone(), Some(bounds.size.width), rows.line_height, rows.align, window);
            *state = Some(rewrapped);
        }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        {
            let state = self.layout.borrow();
            let Some(rows) = state.as_ref() else { return };
            for (i, line) in rows.lines.iter().enumerate() {
                let origin = point(bounds.origin.x, bounds.origin.y + rows.line_height * i as f32);
                if let Err(e) = line.paint(origin, rows.line_height, rows.align, Some(bounds.size.width), window, cx) {
                    tracing::debug!("paragraph row failed to paint: {e:#}");
                }
            }
        }
        if let Some((links, on_click)) = self.links.take() {
            let layout = self.layout.clone();
            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left || !bounds.contains(&ev.position) {
                    return;
                }
                let hit = layout.borrow().as_ref().and_then(|rows| rows.index_at(bounds, ev.position));
                if let Some(link) = hit.and_then(|ix| links.iter().position(|r| r.contains(&ix))) {
                    on_click(link, window, cx);
                }
            });
        }
    }
}

/// One glyph of a row: the paragraph byte index of its character and its
/// horizontal extent within the row.
#[derive(Clone, Copy, Debug)]
struct GlyphSpan {
    index: usize,
    x0: f32,
    x1: f32,
}

impl Rows {
    /// The row's left edge in window space (alignment applied).
    fn row_left(&self, row: usize, bounds: Bounds<Pixels>) -> Pixels {
        let w = self.lines.get(row).map_or(Pixels::ZERO, |l| l.width);
        match self.align {
            TextAlign::Left => bounds.origin.x,
            TextAlign::Center => bounds.origin.x + (bounds.size.width - w) / 2.0,
            TextAlign::Right => bounds.origin.x + bounds.size.width - w,
        }
    }

    /// The row's glyphs of paragraph text (not its mark or ellipsis),
    /// left to right.
    fn glyphs(&self, row: usize) -> Vec<GlyphSpan> {
        let (Some(line), Some(&(start, mark)), Some(&end)) = (self.lines.get(row), self.starts.get(row), self.ends.get(row)) else { return Vec::new() };
        let mut xs: Vec<(f32, usize)> = line.runs.iter().flat_map(|r| r.glyphs.iter()).map(|g| (f32::from(g.position.x), g.index)).collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let width = f32::from(line.width);
        let mut out = Vec::with_capacity(xs.len());
        for (i, &(x0, ix)) in xs.iter().enumerate() {
            let x1 = xs.get(i + 1).map_or(width, |n| n.0);
            if ix < mark || ix - mark >= end - start {
                continue;
            }
            out.push(GlyphSpan { index: start + ix - mark, x0, x1 });
        }
        out
    }

    /// The row a caret at byte `index` sits on.
    fn row_of(&self, index: usize) -> usize {
        let last = self.lines.len().saturating_sub(1);
        (0..self.lines.len()).find(|&r| index <= self.ends[r] && (r == last || index < self.starts[r + 1].0)).unwrap_or(last)
    }

    /// The window x of a caret before byte `index` on `row`.
    fn caret_x(&self, row: usize, index: usize, bounds: Bounds<Pixels>) -> Pixels {
        let left = self.row_left(row, bounds);
        let text = self.inputs.text.as_ref();
        let glyphs = self.glyphs(row);
        let at = |i: usize| glyphs.iter().filter(|g| g.index == i).copied().reduce(|a, b| GlyphSpan { index: a.index, x0: a.x0.min(b.x0), x1: a.x1.max(b.x1) });
        let rtl_at = |i: usize| text[i..].chars().next().is_some_and(super::is_rtl_char);
        // The leading edge of the character after the caret…
        if let Some(g) = at(index) {
            return left + px(if rtl_at(index) { g.x1 } else { g.x0 });
        }
        // …else the trailing edge of the one before it.
        let prev = text[..index].char_indices().next_back().map(|(i, _)| i).filter(|&i| i >= self.starts[row].0);
        if let Some(g) = prev.and_then(|p| at(p).map(|g| (p, g))) {
            return left + px(if rtl_at(g.0) { g.1.x0 } else { g.1.x1 });
        }
        // An empty row: its start edge.
        let w = self.lines.get(row).map_or(Pixels::ZERO, |l| l.width);
        if self.inputs.rtl { left + w } else { left }
    }

    /// The char boundary nearest to `pos` (rows painted in `bounds`).
    fn caret_index_at(&self, bounds: Bounds<Pixels>, pos: Point<Pixels>) -> usize {
        let last = self.lines.len().saturating_sub(1);
        let row = (((pos.y - bounds.origin.y) / self.line_height).floor().max(0.0) as usize).min(last);
        let (start, end) = (self.starts.get(row).map_or(0, |s| s.0), self.ends.get(row).copied().unwrap_or(0));
        let x = f32::from(pos.x - self.row_left(row, bounds));
        let text = self.inputs.text.as_ref();
        let glyphs = self.glyphs(row);
        let (Some(first), Some(last_g)) = (glyphs.first(), glyphs.last()) else { return start };
        // Past the row's left / right edge: its start or end by direction.
        let (left_end, right_end) = if self.inputs.rtl { (end, start) } else { (start, end) };
        if x < first.x0 {
            return left_end;
        }
        if x >= last_g.x1 {
            return right_end;
        }
        let Some(g) = glyphs.iter().find(|g| x >= g.x0 && x < g.x1) else { return right_end };
        let c = text[g.index..].chars().next().unwrap_or(' ');
        let after = x > (g.x0 + g.x1) / 2.0;
        if after != super::is_rtl_char(c) { g.index + c.len_utf8() } else { g.index }
    }

    /// Selection rectangles for `range` (window space).
    fn selection_rects(&self, bounds: Option<Bounds<Pixels>>, range: &Range<usize>) -> Vec<Bounds<Pixels>> {
        let Some(bounds) = bounds else { return Vec::new() };
        let mut out = Vec::new();
        for row in 0..self.lines.len() {
            let left = self.row_left(row, bounds);
            let sel = self.glyphs(row).into_iter().filter(|g| range.contains(&g.index));
            let span = sel.fold(None::<(f32, f32)>, |acc, g| Some(acc.map_or((g.x0, g.x1), |(a, b)| (a.min(g.x0), b.max(g.x1)))));
            if let Some((a, b)) = span {
                out.push(Bounds::new(point(left + px(a), bounds.origin.y + self.line_height * row as f32), gpui::size(px(b - a), self.line_height)));
            }
        }
        out
    }

    /// The paragraph byte index of the glyph under `pos` (rows painted in
    /// `bounds`), whatever order the row's glyphs are stored in.
    fn index_at(&self, bounds: Bounds<Pixels>, pos: Point<Pixels>) -> Option<usize> {
        let row = ((pos.y - bounds.origin.y) / self.line_height).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        let line = self.lines.get(row)?;
        let (start, mark) = *self.starts.get(row)?;
        let left = match self.align {
            TextAlign::Left => bounds.origin.x,
            TextAlign::Center => bounds.origin.x + (bounds.size.width - line.width) / 2.0,
            TextAlign::Right => bounds.origin.x + bounds.size.width - line.width,
        };
        let x = pos.x - left;
        if x < Pixels::ZERO || x >= line.width {
            return None;
        }
        let glyph = line.runs.iter().flat_map(|r| r.glyphs.iter()).filter(|g| g.position.x <= x).max_by(|a, b| f32::from(a.position.x).total_cmp(&f32::from(b.position.x)))?;
        Some((start + glyph.index).saturating_sub(mark))
    }
}

/// The alignment a paragraph paints with: text aligned to the UI's reading
/// start goes to the *paragraph's* start (an Arabic body in an English UI
/// hugs the right); centered and end-aligned text stays as asked.
fn paragraph_align(inherited: TextAlign, ui_rtl: bool, paragraph_rtl: bool) -> TextAlign {
    let ui_start = if ui_rtl { TextAlign::Right } else { TextAlign::Left };
    if inherited != ui_start {
        return inherited;
    }
    if paragraph_rtl { TextAlign::Right } else { TextAlign::Left }
}

/// Text runs for `text` in `style` with `highlights` applied (as
/// `StyledText::with_highlights` computes them).
fn runs(text: &str, style: &TextStyle, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut ix = 0;
    for (range, highlight) in highlights {
        if range.start < ix || range.end > text.len() || range.is_empty() {
            continue;
        }
        if ix < range.start {
            out.push(style.to_run(range.start - ix));
        }
        out.push(style.clone().highlight(highlight).to_run(range.len()));
        ix = range.end;
    }
    if ix < text.len() || out.is_empty() {
        out.push(style.to_run(text.len() - ix));
    }
    out
}

/// `runs` with the font family of each `families` range replaced (runs are
/// split at the ranges' edges).
fn with_families(runs: Vec<TextRun>, families: &[(Range<usize>, SharedString)]) -> Vec<TextRun> {
    if families.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + families.len() * 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        // Cut points inside this run.
        let mut cuts: SmallVec<[usize; 4]> = families.iter().flat_map(|(r, _)| [r.start, r.end]).filter(|&c| c > start && c < end).collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut from = start;
        for to in cuts.into_iter().chain([end]) {
            let mut piece = TextRun { len: to - from, ..run.clone() };
            if let Some((_, family)) = families.iter().find(|(r, _)| r.start <= from && to <= r.end) {
                piece.font.family = family.clone();
            }
            out.push(piece);
            from = to;
        }
        start = end;
    }
    out
}

/// The runs covering `range` of the text, clipped to it.
fn slice_runs(runs: &[TextRun], range: Range<usize>) -> SmallVec<[TextRun; 4]> {
    let mut out = SmallVec::new();
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let (a, b) = (start.max(range.start), end.min(range.end));
        if a < b {
            out.push(TextRun { len: b - a, ..run.clone() });
        }
        start = end;
        if start >= range.end {
            break;
        }
    }
    if out.is_empty()
        && let Some(last) = runs.iter().rev().find(|r| r.len > 0).or(runs.last())
    {
        out.push(TextRun { len: range.len(), ..last.clone() });
    }
    out
}

/// Wraps and shapes `inputs` at `width` (`None`: unbounded).
fn layout_rows(inputs: &Rc<Inputs>, width: Option<Pixels>, line_height: Pixels, align: TextAlign, window: &mut Window) -> Rows {
    let text = inputs.text.as_ref();
    let font_size = inputs.font_size;
    let ts = window.text_system().clone();
    let mut measure = |r: Range<usize>| -> f32 {
        if r.is_empty() {
            return 0.0;
        }
        f32::from(ts.layout_line(&text[r.clone()], font_size, &slice_runs(&inputs.runs, r), None).width)
    };
    let wrap_width = if inputs.wrap { width } else { None };
    let mut ranges = break_rows(text, wrap_width.map(f32::from), &mut measure);
    let mut clipped_last = false;
    if let Some(max) = inputs.max_lines.filter(|&m| m > 0)
        && ranges.len() > max
    {
        ranges.truncate(max);
        clipped_last = true;
    }
    let limit = width.map(f32::from);
    let mut lines = Vec::with_capacity(ranges.len());
    let mut starts = Vec::with_capacity(ranges.len());
    let mut ends = Vec::with_capacity(ranges.len());
    let mut widest = Pixels::ZERO;
    let last = ranges.len().saturating_sub(1);
    for (i, range) in ranges.into_iter().enumerate() {
        let overflows = limit.is_some_and(|l| measure(range.clone()) > l + REWRAP_EPSILON_PX);
        let ellipsis = inputs.ellipsize && i == last && (clipped_last || overflows);
        let (body, suffix) = if ellipsis {
            let room = limit.map_or(f32::INFINITY, |l| l - measure_str(&ts, ELLIPSIS, &inputs.runs, range.end, font_size));
            (fit_prefix(text, range, room, &mut measure), ELLIPSIS)
        } else {
            (range, "")
        };
        starts.push((body.start, row_mark(&text[body.clone()], inputs.rtl).map_or(0, char::len_utf8)));
        ends.push(body.end);
        let line = shape_row(&ts, inputs, body, suffix);
        widest = widest.max(line.width);
        lines.push(line);
    }
    let size = Size { width: widest.ceil(), height: line_height * lines.len().max(1) as f32 };
    Rows { inputs: inputs.clone(), wrap_width, lines, starts, ends, line_height, align, size }
}

/// Width of `s` styled like the text at byte `at`.
fn measure_str(ts: &gpui::WindowTextSystem, s: &str, runs: &[TextRun], at: usize, font_size: Pixels) -> f32 {
    let style = run_at(runs, at);
    f32::from(ts.layout_line(s, font_size, &[TextRun { len: s.len(), ..style }], None).width)
}

/// The run style at byte `at` (the last run past the end).
fn run_at(runs: &[TextRun], at: usize) -> TextRun {
    let mut start = 0;
    for run in runs {
        if at < start + run.len {
            return run.clone();
        }
        start += run.len;
    }
    runs.last().cloned().expect("a paragraph has at least one run")
}

/// The longest prefix of `range` (ending on a char boundary, trailing
/// spaces dropped) whose width fits `room`.
fn fit_prefix(text: &str, range: Range<usize>, room: f32, measure: &mut impl FnMut(Range<usize>) -> f32) -> Range<usize> {
    let ends: Vec<usize> = text[range.clone()].char_indices().skip(1).map(|(i, _)| range.start + i).chain([range.end]).collect();
    let n = ends.partition_point(|&end| measure(range.start..end) <= room);
    let end = if n == 0 { range.start } else { ends[n - 1] };
    range.start..range.start + text[range.start..end].trim_end().len()
}

/// Shapes `body` (+ `suffix`) as one row, opening it with a direction mark
/// when its first letter reads the other way than the paragraph.
fn shape_row(ts: &gpui::WindowTextSystem, inputs: &Inputs, body: Range<usize>, suffix: &str) -> ShapedLine {
    let row = &inputs.text[body.clone()];
    let mark = row_mark(row, inputs.rtl);
    let mut runs = slice_runs(&inputs.runs, body.clone());
    let mut s = String::with_capacity(row.len() + suffix.len() + RLM.len_utf8());
    if let Some(m) = mark {
        s.push(m);
        if let Some(first) = runs.first_mut() {
            first.len += m.len_utf8();
        }
    }
    s.push_str(row);
    if !suffix.is_empty() {
        s.push_str(suffix);
        match runs.last_mut() {
            Some(r) => r.len += suffix.len(),
            None => runs.push(TextRun { len: suffix.len(), ..run_at(&inputs.runs, body.end) }),
        }
    }
    // A row that is only the mark still needs a run covering it.
    let total: usize = runs.iter().map(|r| r.len).sum();
    if total < s.len() {
        match runs.last_mut() {
            Some(r) => r.len += s.len() - total,
            None => runs.push(TextRun { len: s.len(), ..run_at(&inputs.runs, body.start) }),
        }
    }
    ts.shape_line(s.into(), inputs.font_size, &runs, None)
}

/// The direction mark a row needs: RLM when a right-to-left paragraph's row
/// starts with a left-to-right letter (or has no letter), LRM for the
/// reverse; `None` when the row already starts in the paragraph's direction.
fn row_mark(row: &str, paragraph_rtl: bool) -> Option<char> {
    let first = row.chars().find(|c| c.is_alphabetic()).map(super::is_rtl_char);
    match (paragraph_rtl, first) {
        (true, Some(true)) | (false, Some(false) | None) => None,
        (true, _) => Some(RLM),
        (false, Some(true)) => Some(LRM),
    }
}

/// Byte ranges of the rows `text` wraps into at `max_width` (`None`: only
/// at newlines). Rows break after spaces, which stay at the end of the row
/// they follow (and out of its range); a word wider than a row is split
/// between characters. `measure` is the width of a byte range.
pub(crate) fn break_rows(text: &str, max_width: Option<f32>, measure: &mut impl FnMut(Range<usize>) -> f32) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let start = line_start;
        line_start += line.len() + 1;
        let line_end = start + line.trim_end().len();
        let Some(max) = max_width else {
            rows.push(start..line_end);
            continue;
        };
        let words = words(line, start);
        if words.is_empty() {
            rows.push(start..start);
            continue;
        }
        let mut row_start = start;
        let mut row_end: Option<usize> = None;
        // Width of the row's words before the current one, with their spaces.
        let mut acc = 0.0;
        let mut i = 0;
        while i < words.len() {
            let w = &words[i];
            let from = if row_end.is_none() { row_start } else { w.start };
            let word_w = measure(from..w.end);
            if acc + word_w <= max {
                acc += measure(from..w.spaces_end);
                row_end = Some(w.end);
                i += 1;
                continue;
            }
            if let Some(end) = row_end {
                rows.push(row_start..end);
                row_start = w.start;
                row_end = None;
                acc = 0.0;
                continue;
            }
            // A word wider than a whole row: split it between characters.
            let piece = fit_chars(text, from..w.end, max, measure);
            rows.push(from..piece);
            row_start = piece;
            if piece >= w.end {
                row_start = w.spaces_end;
                i += 1;
            }
        }
        if let Some(end) = row_end {
            rows.push(row_start..end);
        }
    }
    rows
}

/// One word of a line: `start..end`, then its spaces up to `spaces_end`.
struct Word {
    start: usize,
    end: usize,
    spaces_end: usize,
}

fn words(line: &str, offset: usize) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut in_word = false;
    for (i, c) in line.char_indices() {
        let space = c == ' ' || c == '\t';
        match (space, in_word) {
            (false, false) => {
                in_word = true;
                out.push(Word { start: offset + i, end: offset + line.len(), spaces_end: offset + line.len() });
            }
            (true, true) => {
                in_word = false;
                if let Some(w) = out.last_mut() {
                    w.end = offset + i;
                }
            }
            _ => {}
        }
        if !space && let Some(w) = out.last_mut() {
            w.end = offset + i + c.len_utf8();
        }
    }
    for i in 0..out.len() {
        out[i].spaces_end = out.get(i + 1).map_or(offset + line.len(), |n| n.start);
    }
    out
}

/// The end of the longest run of whole characters from `range.start` that
/// fits `max` (at least one character, so wrapping always advances).
fn fit_chars(text: &str, range: Range<usize>, max: f32, measure: &mut impl FnMut(Range<usize>) -> f32) -> usize {
    let ends: Vec<usize> = text[range.clone()].char_indices().skip(1).map(|(i, _)| range.start + i).chain([range.end]).collect();
    let n = ends.partition_point(|&end| measure(range.start..end) <= max);
    ends[n.max(1) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is one unit wide.
    fn chars(text: &str) -> impl FnMut(Range<usize>) -> f32 + '_ {
        move |r| text[r].chars().count() as f32
    }

    fn rows(text: &str, max: Option<f32>) -> Vec<&str> {
        break_rows(text, max, &mut chars(text)).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn arabic_wraps_between_words() {
        let text = "يطلب تطبيق صلاحيات المسؤول للمتابعة";
        assert_eq!(rows(text, Some(16.0)), ["يطلب تطبيق", "صلاحيات المسؤول", "للمتابعة"]);
        // No row ends up as a lone letter, and nothing overflows.
        for row in rows(text, Some(16.0)) {
            assert!(row.chars().count() > 1 && row.chars().count() <= 16, "{row:?}");
        }
    }

    #[test]
    fn spaces_newlines_and_unbounded() {
        assert_eq!(rows("aa bb  cc", Some(5.0)), ["aa bb", "cc"]);
        assert_eq!(rows("aa\n\nbb cc", Some(5.0)), ["aa", "", "bb cc"]);
        assert_eq!(rows("aa bb cc", None), ["aa bb cc"]);
        assert_eq!(rows("one two\nthree", None), ["one two", "three"]);
        assert_eq!(rows("  indented word", Some(11.0)), ["  indented", "word"]);
    }

    #[test]
    fn long_words_split_between_characters() {
        assert_eq!(rows("abcdefghij kl", Some(4.0)), ["abcd", "efgh", "ij", "kl"]);
        assert_eq!(rows("ابتثجحخدذر", Some(3.0)), ["ابت", "ثجح", "خدذ", "ر"]);
        // Even a zero-width row advances one character at a time.
        assert_eq!(rows("ab", Some(0.0)), ["a", "b"]);
    }

    #[test]
    fn prefixes_fit_the_room() {
        let text = "hello world";
        assert_eq!(&text[fit_prefix(text, 0..text.len(), 7.0, &mut chars(text))], "hello w");
        assert_eq!(&text[fit_prefix(text, 0..text.len(), 6.0, &mut chars(text))], "hello");
        assert_eq!(&text[fit_prefix(text, 0..text.len(), 0.0, &mut chars(text))], "");
    }

    #[test]
    fn family_overrides_split_runs() {
        let style = TextStyle::default();
        let base = runs("ab code cd", &style, Vec::new());
        let out = with_families(base, &[(3..7, "Mono".into())]);
        assert_eq!(out.iter().map(|r| r.len).collect::<Vec<_>>(), vec![3, 4, 3]);
        assert_eq!(out[1].font.family.as_ref(), "Mono");
        assert_ne!(out[0].font.family.as_ref(), "Mono");
        assert_ne!(out[2].font.family.as_ref(), "Mono");
    }

    #[test]
    fn rows_keep_the_paragraph_direction() {
        assert_eq!(row_mark("Mandara يطلب", true), Some(RLM));
        assert_eq!(row_mark("123", true), Some(RLM));
        assert_eq!(row_mark("يطلب", true), None);
        assert_eq!(row_mark("يطلب hello", false), Some(LRM));
        assert_eq!(row_mark("hello", false), None);
    }

    #[test]
    fn alignment_follows_the_paragraph_start() {
        // English UI, Arabic body: start-aligned text hugs the right.
        assert_eq!(paragraph_align(TextAlign::Left, false, true), TextAlign::Right);
        // Arabic UI, English body.
        assert_eq!(paragraph_align(TextAlign::Right, true, false), TextAlign::Left);
        assert_eq!(paragraph_align(TextAlign::Center, true, false), TextAlign::Center);
        assert_eq!(paragraph_align(TextAlign::Right, false, true), TextAlign::Right);
    }
}
