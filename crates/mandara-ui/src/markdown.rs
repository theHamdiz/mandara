//! Markdown for notes: a CommonMark + GFM parse (`pulldown-cmark`) into a
//! small block tree, drawn natively with GPUI elements in the theme, plus
//! the end-of-text editing helpers the notes editor binds to keys.
//!
//! Why `pulldown-cmark`: pure Rust, CommonMark-conformant, GFM tables /
//! task lists / strikethrough behind flags, and a pull parser with source
//! offsets — which is how a clicked `- [ ]` rewrites exactly its own
//! characters in the note's text. Parsing happens once per edit (the notes
//! view caches a [`Doc`] per note keyed by its text), never per frame.
//!
//! Supported: headings, paragraphs, **bold**, *italic*, ~~strike~~,
//! `inline code`, fenced / indented code blocks (monospace, left to right
//! in every language), bullet / numbered / task lists (nested), block
//! quotes, links (opened with the session's `open_uri`), rules and simple
//! tables. Raw HTML shows as text. Paragraphs whose first strong letter is
//! Arabic (or another RTL script) align right.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, StrikethroughStyle, Styled, UnderlineStyle, Window, div, prelude::FluentBuilder as _,
    px,
};
use mandara_core::ToGpui;
use mandara_theme::MandaraPalette;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::kit::{text, tokens};

/// Inline style flags of a run.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// A styled run inside an [`Inline`] (byte range of its text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
    pub link: Option<String>,
}

/// Text with styled runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<Span>,
}

impl Inline {
    fn push(&mut self, s: &str, style: Style, link: Option<&String>) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        let range = start..self.text.len();
        // Merge with the previous run when nothing changes.
        if let Some(last) = self.spans.last_mut()
            && last.range.end == start
            && last.style == style
            && last.link.as_ref() == link
        {
            last.range.end = range.end;
            return;
        }
        self.spans.push(Span { range, style, link: link.cloned() });
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// One list item; `task` is `(checked, source range of "[ ]")`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub task: Option<(bool, Range<usize>)>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: Inline },
    Paragraph(Inline),
    Code { lang: String, text: String },
    Quote(Vec<Block>),
    /// `start` is the first number of an ordered list.
    List { start: Option<u64>, items: Vec<Item> },
    Rule,
    Table { head: Vec<Inline>, rows: Vec<Vec<Inline>> },
}

/// A parsed note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

enum Frame {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    List { start: Option<u64>, items: Vec<Item> },
    Item(Item),
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum InlineKind {
    Paragraph,
    Heading(u8),
    Cell,
}

#[derive(Default)]
struct TableBuild {
    head: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    in_head: bool,
}

struct Builder {
    stack: Vec<Frame>,
    inline: Option<(InlineKind, Inline)>,
    style: Style,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
    code: Option<(String, String)>,
    table: Option<TableBuild>,
}

impl Builder {
    fn push_block(&mut self, b: Block) {
        match self.stack.last_mut() {
            Some(Frame::Root(v) | Frame::Quote(v)) => v.push(b),
            Some(Frame::Item(it)) => it.blocks.push(b),
            // A block straight in a list (never from the parser) — keep it.
            Some(Frame::List { items, .. }) => items.push(Item { task: None, blocks: vec![b] }),
            None => {}
        }
    }

    fn flush(&mut self) {
        let Some((kind, inl)) = self.inline.take() else { return };
        match kind {
            InlineKind::Paragraph if !inl.is_empty() => self.push_block(Block::Paragraph(inl)),
            InlineKind::Heading(level) => self.push_block(Block::Heading { level, text: inl }),
            InlineKind::Cell => {
                if let Some(t) = &mut self.table {
                    if t.in_head {
                        t.head.push(inl);
                    } else if let Some(row) = t.rows.last_mut() {
                        row.push(inl);
                    }
                }
            }
            InlineKind::Paragraph => {}
        }
    }

    fn text(&mut self, s: &str, code: bool) {
        if let Some((_, buf)) = &mut self.code {
            buf.push_str(s);
            return;
        }
        let inl = &mut self.inline.get_or_insert_with(|| (InlineKind::Paragraph, Inline::default())).1;
        let style = Style { code, ..self.style };
        inl.push(s, style, self.links.last());
    }

    fn restyle(&mut self) {
        self.style = Style { bold: self.bold > 0, italic: self.italic > 0, strike: self.strike > 0, code: false };
    }
}

fn level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Parses `source` (CommonMark with GFM tables, task lists, strikethrough).
pub fn parse(source: &str) -> Doc {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    let mut b = Builder {
        stack: vec![Frame::Root(Vec::new())],
        inline: None,
        style: Style::default(),
        bold: 0,
        italic: 0,
        strike: 0,
        links: Vec::new(),
        code: None,
        table: None,
    };
    for (event, range) in Parser::new_ext(source, opts).into_offset_iter() {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    b.flush();
                    b.inline = Some((InlineKind::Paragraph, Inline::default()));
                }
                Tag::Heading { level: l, .. } => {
                    b.flush();
                    b.inline = Some((InlineKind::Heading(level(l)), Inline::default()));
                }
                Tag::BlockQuote(_) => {
                    b.flush();
                    b.stack.push(Frame::Quote(Vec::new()));
                }
                Tag::CodeBlock(kind) => {
                    b.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    b.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    b.flush();
                    b.stack.push(Frame::List { start, items: Vec::new() });
                }
                Tag::Item => {
                    b.flush();
                    b.stack.push(Frame::Item(Item::default()));
                }
                Tag::Table(_) => {
                    b.flush();
                    b.table = Some(TableBuild::default());
                }
                Tag::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.in_head = true;
                    }
                }
                Tag::TableRow => {
                    if let Some(t) = &mut b.table {
                        t.rows.push(Vec::new());
                    }
                }
                Tag::TableCell => b.inline = Some((InlineKind::Cell, Inline::default())),
                Tag::Emphasis => {
                    b.italic += 1;
                    b.restyle();
                }
                Tag::Strong => {
                    b.bold += 1;
                    b.restyle();
                }
                Tag::Strikethrough => {
                    b.strike += 1;
                    b.restyle();
                }
                Tag::Link { dest_url, .. } => b.links.push(dest_url.to_string()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => b.flush(),
                TagEnd::BlockQuote(_) => {
                    b.flush();
                    if let Some(Frame::Quote(v)) = b.stack.pop() {
                        b.push_block(Block::Quote(v));
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, mut text)) = b.code.take() {
                        while text.ends_with('\n') {
                            text.pop();
                        }
                        b.push_block(Block::Code { lang, text });
                    }
                }
                TagEnd::Item => {
                    b.flush();
                    if let Some(Frame::Item(it)) = b.stack.pop()
                        && let Some(Frame::List { items, .. }) = b.stack.last_mut()
                    {
                        items.push(it);
                    }
                }
                TagEnd::List(_) => {
                    b.flush();
                    if let Some(Frame::List { start, items }) = b.stack.pop() {
                        b.push_block(Block::List { start, items });
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.in_head = false;
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = b.table.take() {
                        b.push_block(Block::Table { head: t.head, rows: t.rows });
                    }
                }
                TagEnd::Emphasis => {
                    b.italic = b.italic.saturating_sub(1);
                    b.restyle();
                }
                TagEnd::Strong => {
                    b.bold = b.bold.saturating_sub(1);
                    b.restyle();
                }
                TagEnd::Strikethrough => {
                    b.strike = b.strike.saturating_sub(1);
                    b.restyle();
                }
                TagEnd::Link => {
                    b.links.pop();
                }
                _ => {}
            },
            Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => b.text(&s, false),
            Event::Code(s) => b.text(&s, true),
            Event::SoftBreak => b.text(" ", false),
            Event::HardBreak => b.text("\n", false),
            Event::Rule => {
                b.flush();
                b.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item(it)) = b.stack.last_mut() {
                    it.task = Some((checked, range));
                }
            }
            _ => {}
        }
    }
    b.flush();
    while b.stack.len() > 1 {
        match b.stack.pop() {
            Some(Frame::Quote(v)) => b.push_block(Block::Quote(v)),
            Some(Frame::List { start, items }) => b.push_block(Block::List { start, items }),
            Some(Frame::Item(it)) => {
                if let Some(Frame::List { items, .. }) = b.stack.last_mut() {
                    items.push(it);
                }
            }
            _ => {}
        }
    }
    match b.stack.pop() {
        Some(Frame::Root(blocks)) => Doc { blocks },
        _ => Doc::default(),
    }
}

/// Flips the task marker at `range` (`[ ]` ↔ `[x]`) in `source`.
pub fn toggle_task(source: &str, range: Range<usize>) -> Option<String> {
    let marker = source.get(range.clone())?;
    let next = match marker {
        "[ ]" => "[x]",
        "[x]" | "[X]" => "[ ]",
        _ => return None,
    };
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..range.start]);
    out.push_str(next);
    out.push_str(&source[range.end..]);
    Some(out)
}

/// The plain text of a block tree (search, previews).
pub fn plain(blocks: &[Block], out: &mut String) {
    for b in blocks {
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        match b {
            Block::Heading { text, .. } | Block::Paragraph(text) => out.push_str(&text.text),
            Block::Code { text, .. } => out.push_str(text),
            Block::Quote(v) => plain(v, out),
            Block::List { items, .. } => items.iter().for_each(|i| plain(&i.blocks, out)),
            Block::Rule => {}
            Block::Table { head, rows } => {
                for c in head.iter().chain(rows.iter().flatten()) {
                    out.push_str(&c.text);
                    out.push(' ');
                }
            }
        }
    }
}

/// A note's title: its first line without Markdown markers.
pub fn title_line(source: &str) -> Option<String> {
    let line = source.lines().map(str::trim).find(|l| !l.is_empty())?;
    let doc = parse(line);
    let mut s = String::new();
    plain(&doc.blocks, &mut s);
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

// ── Editing at the end of the text (the editor's caret lives there) ──────

/// The last line of `text` (what the caret is on).
fn last_line(text: &str) -> &str {
    text.rsplit('\n').next().unwrap_or(text)
}

/// The list marker opening `line` (indent, marker incl. trailing space):
/// `"  - [ ] "`, `"3. "`, `"* "`.
fn list_marker(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let bullet = ["- ", "* ", "+ "].into_iter().find(|b| rest.starts_with(b));
    let (marker, after) = match bullet {
        Some(b) => (b.to_string(), &rest[b.len()..]),
        None => {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits == 0 || !(rest[digits..].starts_with(". ") || rest[digits..].starts_with(") ")) {
                return None;
            }
            (rest[..digits + 2].to_string(), &rest[digits + 2..])
        }
    };
    let task = ["[ ] ", "[x] ", "[X] "].into_iter().find(|t| after.starts_with(t));
    Some((indent, format!("{marker}{}", task.unwrap_or(""))))
}

/// Enter: continues a list (next number, open task) or, on an empty item,
/// ends it. Returns `false` when Enter should just insert a newline.
pub fn continue_list(text: &mut String) -> bool {
    let line = last_line(text).to_string();
    let Some((indent, marker)) = list_marker(&line) else { return false };
    if line.trim_end().len() <= indent + marker.trim_end().len() {
        // Empty item: drop the marker, leaving a blank line.
        let cut = text.len() - line.len();
        text.truncate(cut);
        return true;
    }
    let next = match marker.split_once(['.', ')']) {
        Some((n, rest)) if n.chars().all(|c| c.is_ascii_digit()) => {
            let sep = &marker[n.len()..n.len() + 1];
            format!("{}{sep}{}", n.parse::<u64>().unwrap_or(0) + 1, rest)
        }
        _ => marker.clone(),
    };
    let next = next.replace("[x] ", "[ ] ").replace("[X] ", "[ ] ");
    text.push('\n');
    text.push_str(&" ".repeat(indent));
    text.push_str(&next);
    true
}

/// Ctrl+B / Ctrl+I: opens `mark` at the caret, or closes it when the line
/// has an unclosed one (`**bold**`, `_italic_`).
pub fn toggle_mark(text: &mut String, mark: &str) {
    let open = last_line(text).matches(mark).count() % 2 == 1;
    if !open && !text.is_empty() && !text.ends_with([' ', '\n']) {
        text.push(' ');
    }
    text.push_str(mark);
}

/// Ctrl+K: `[` → `](` → `)` — type the label, press again, type the
/// address, press again.
pub fn link_step(text: &mut String) {
    let line = last_line(text);
    let opened = line.rfind('[');
    let middle = line.rfind("](");
    let closed = line.rfind(')');
    match (opened, middle) {
        (Some(o), m) if m.is_none_or(|m| m < o) => text.push_str("]("),
        (_, Some(m)) if closed.is_none_or(|c| c < m) => text.push(')'),
        _ => {
            if !text.is_empty() && !text.ends_with([' ', '\n']) {
                text.push(' ');
            }
            text.push('[');
        }
    }
}

/// Ctrl+Shift+8: makes the current line a bullet, or plain again.
pub fn toggle_bullet(text: &mut String) {
    let line = last_line(text).to_string();
    let start = text.len() - line.len();
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let new = match rest.strip_prefix("- ") {
        Some(plain) => format!("{}{plain}", &line[..indent]),
        None => format!("{}- {rest}", &line[..indent]),
    };
    text.replace_range(start.., &new);
}

/// Spaces one Tab indents a list item by.
const INDENT: &str = "  ";

/// Tab / Shift+Tab on a list item: nests it one level deeper / shallower.
/// Returns `false` off a list item (Tab then moves focus as usual).
pub fn indent(text: &mut String, outdent: bool) -> bool {
    let line = last_line(text).to_string();
    if list_marker(&line).is_none() {
        return false;
    }
    let start = text.len() - line.len();
    if outdent {
        if let Some(stripped) = line.strip_prefix(INDENT) {
            text.replace_range(start.., stripped);
        }
    } else {
        text.insert_str(start, INDENT);
    }
    true
}

// ── Rendering ────────────────────────────────────────────────────────────

const H_SIZES: [f32; 3] = [text::TITLE + 3.0, text::TITLE + 1.0, text::TITLE];
const BLOCK_GAP_PX: f32 = tokens::SM;
const LINE_PX: f32 = 19.0;
const CODE_PAD_PX: f32 = 10.0;
const CODE_RADIUS_PX: f32 = 10.0;
const CODE_TEXT_PX: f32 = text::BODY_S;
const CODE_BG_ALPHA: f32 = 0.06;
const INLINE_CODE_ALPHA: f32 = 0.1;
const QUOTE_BAR_PX: f32 = 3.0;
const QUOTE_PAD_PX: f32 = 10.0;
const LIST_INDENT_PX: f32 = 18.0;
const LIST_GAP_PX: f32 = 4.0;
const BULLET_W_PX: f32 = 14.0;
const CHECK_PX: f32 = 16.0;
const CHECK_ICON_PX: f32 = 11.0;
const RULE_ALPHA: f32 = 0.12;
const TABLE_CELL_PAD_X_PX: f32 = 8.0;
const TABLE_CELL_PAD_Y_PX: f32 = 4.0;
const TABLE_BORDER_ALPHA: f32 = 0.1;
const MUTED_ALPHA: f32 = 0.7;

/// Flips the task whose `[ ]` marker sits at the given source range.
pub type OnToggle = Rc<dyn Fn(Range<usize>, &mut Window, &mut App)>;

/// What a rendered note can do: toggle a task (by its source range).
pub struct RenderCx {
    /// Element-id namespace (unique per surface and note).
    pub id: SharedString,
    pub on_toggle: Option<OnToggle>,
    /// The monospace family.
    pub mono: SharedString,
}

fn highlights(inl: &Inline, palette: &MandaraPalette) -> Vec<(Range<usize>, HighlightStyle)> {
    inl.spans
        .iter()
        .filter(|s| s.style != Style::default() || s.link.is_some())
        .map(|s| {
            let mut h = HighlightStyle::default();
            if s.style.bold {
                h.font_weight = Some(FontWeight::BOLD);
            }
            if s.style.italic {
                h.font_style = Some(FontStyle::Italic);
            }
            if s.style.strike {
                h.strikethrough = Some(StrikethroughStyle { thickness: px(1.0), color: None });
            }
            if s.style.code {
                h.background_color = Some(palette.on_surface.with_alpha(INLINE_CODE_ALPHA).hsla());
                h.color = Some(palette.tertiary.hsla());
            }
            if s.link.is_some() {
                h.color = Some(palette.primary.hsla());
                h.underline = Some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false });
            }
            (s.range.clone(), h)
        })
        .collect()
}

/// Inline text: styled spans, `code` spans in the monospace family, links.
fn inline(inl: &Inline, palette: &MandaraPalette, id: ElementId, mono: &SharedString) -> AnyElement {
    let links: Vec<(Range<usize>, String)> = inl.spans.iter().filter_map(|s| s.link.clone().map(|l| (s.range.clone(), l))).collect();
    let rtl = crate::bidi::text_is_rtl(&inl.text);
    let el = div().w_full().when(rtl, |el| el.text_right()).when(!rtl, |el| el.text_left());
    let ranges = links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = links.into_iter().map(|(_, u)| u).collect();
    let open = std::rc::Rc::new(move |ix: usize, _: &mut gpui::Window, _: &mut gpui::App| {
        if let Some(url) = urls.get(ix)
            && let Err(err) = mandara_services::session::open_uri(url)
        {
            tracing::warn!("opening {url}: {err:#}");
        }
    });
    let families = inl.spans.iter().filter(|s| s.style.code && !s.range.is_empty()).map(|s| (s.range.clone(), mono.clone())).collect();
    el.child(crate::bidi::linked_paragraph(id, inl.text.clone(), highlights(inl, palette), families, ranges, open)).into_any_element()
}

fn blocks(bs: &[Block], palette: &MandaraPalette, rcx: &RenderCx, path: &mut Vec<usize>) -> Vec<AnyElement> {
    bs.iter()
        .enumerate()
        .map(|(i, b)| {
            path.push(i);
            let id = ElementId::Name(format!("{}-{}", rcx.id, path.iter().map(usize::to_string).collect::<Vec<_>>().join(".")).into());
            let el = block(b, palette, rcx, path, id);
            path.pop();
            el
        })
        .collect()
}

fn block(b: &Block, palette: &MandaraPalette, rcx: &RenderCx, path: &mut Vec<usize>, id: ElementId) -> AnyElement {
    match b {
        Block::Heading { level, text: t } => {
            let size = H_SIZES.get(usize::from(*level).saturating_sub(1)).copied().unwrap_or(text::BODY);
            div().text_size(px(size)).font_weight(FontWeight::SEMIBOLD).child(inline(t, palette, id, &rcx.mono)).into_any_element()
        }
        Block::Paragraph(t) => div().line_height(px(LINE_PX)).child(inline(t, palette, id, &rcx.mono)).into_any_element(),
        // Code reads left to right in every language.
        Block::Code { text: code, .. } => div()
            .id(id)
            .w_full()
            .p(px(CODE_PAD_PX))
            .rounded(px(crate::style::radius(CODE_RADIUS_PX)))
            .bg(palette.on_surface.with_alpha(CODE_BG_ALPHA).hsla())
            .overflow_x_scroll()
            .font_family(rcx.mono.clone())
            .text_size(px(CODE_TEXT_PX))
            .text_left()
            .child(div().flex().flex_col().children(code.lines().map(|l| div().font_family(rcx.mono.clone()).whitespace_nowrap().child(if l.is_empty() { " ".to_string() } else { l.to_string() }))))
            .into_any_element(),
        Block::Quote(inner) => {
            use crate::bidi::Bidi as _;
            div()
                .row()
                .gap(px(QUOTE_PAD_PX))
                .child(div().w(px(QUOTE_BAR_PX)).flex_none().rounded_full().bg(palette.primary.with_alpha(MUTED_ALPHA).hsla()))
                .child(div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(BLOCK_GAP_PX)).text_color(palette.on_surface_variant.hsla()).children(blocks(inner, palette, rcx, path)))
                .into_any_element()
        }
        Block::List { start, items } => {
            use crate::bidi::Bidi as _;
            let nested = path.len() > 1;
            let rows = items.iter().enumerate().map(|(n, item)| {
                path.push(n);
                let marker: AnyElement = match (&item.task, start) {
                    (Some((checked, range)), _) => {
                        let (checked, range) = (*checked, range.clone());
                        let toggle = rcx.on_toggle.clone();
                        div()
                            .id(ElementId::Name(format!("{}-task-{}", rcx.id, range.start).into()))
                            .mt(px((LINE_PX - CHECK_PX) / 2.0))
                            .size(px(CHECK_PX))
                            .flex_none()
                            .rounded(px(crate::style::radius(tokens::XS)))
                            .border_2()
                            .border_color(if checked { palette.primary.hsla() } else { palette.outline.hsla() })
                            .when(checked, |el| el.bg(palette.primary.hsla()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(toggle.is_some(), |el| el.cursor_pointer())
                            .when(checked, |el| el.child(crate::icons::icon("check", CHECK_ICON_PX, palette.on_primary)))
                            .when_some(toggle, |el, toggle| el.on_click(move |_, window, cx| toggle(range.clone(), window, cx)))
                            .into_any_element()
                    }
                    (None, Some(first)) => div().w(px(BULLET_W_PX)).flex_none().font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(format!("{}.", first + n as u64)).into_any_element(),
                    (None, None) => div().w(px(BULLET_W_PX)).flex_none().text_color(palette.primary.hsla()).child("•").into_any_element(),
                };
                let done = matches!(item.task, Some((true, _)));
                let body = div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(LIST_GAP_PX)).when(done, |el| el.text_color(palette.on_surface_variant.with_alpha(MUTED_ALPHA).hsla())).children(blocks(&item.blocks, palette, rcx, path));
                path.pop();
                div().row().items_start().gap(px(tokens::SM)).child(marker).child(body)
            });
            div().flex().flex_col().gap(px(LIST_GAP_PX)).when(nested, |el| el.pl(px(LIST_INDENT_PX / 2.0))).line_height(px(LINE_PX)).children(rows.collect::<Vec<_>>()).into_any_element()
        }
        Block::Rule => div().h(px(1.0)).w_full().bg(palette.on_surface.with_alpha(RULE_ALPHA).hsla()).into_any_element(),
        Block::Table { head, rows } => {
            let border = palette.on_surface.with_alpha(TABLE_BORDER_ALPHA).hsla();
            let cell = |inl: &Inline, n: String, bold: bool| {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(TABLE_CELL_PAD_X_PX))
                    .py(px(TABLE_CELL_PAD_Y_PX))
                    .when(bold, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(inline(inl, palette, ElementId::Name(n.into()), &rcx.mono))
            };
            let row = |cells: &[Inline], r: usize, bold: bool| {
                div().flex().border_b_1().border_color(border).children(cells.iter().enumerate().map(|(c, inl)| cell(inl, format!("{}-t{r}-{c}", rcx.id), bold)))
            };
            div()
                .id(id)
                .w_full()
                .rounded(px(crate::style::radius(CODE_RADIUS_PX)))
                .border_1()
                .border_color(border)
                .text_size(px(text::BODY_S))
                .flex()
                .flex_col()
                .child(row(head, 0, true))
                .children(rows.iter().enumerate().map(|(r, cells)| row(cells, r + 1, false)))
                .into_any_element()
        }
    }
}

/// Draws a parsed note.
pub fn render(doc: &Doc, palette: &MandaraPalette, rcx: &RenderCx) -> AnyElement {
    let mut path = Vec::new();
    div().w_full().flex().flex_col().gap(px(BLOCK_GAP_PX)).text_size(px(text::BODY)).children(blocks(&doc.blocks, palette, rcx, &mut path)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(b: &Block) -> &Inline {
        match b {
            Block::Paragraph(i) => i,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    #[test]
    fn parses_the_supported_blocks() {
        let src = "# Plan\n\nSome **bold**, *it*, ~~old~~ and `code` with a [link](https://x.org).\n\n> quoted\n\n- one\n- two\n  1. nested\n\n```rust\nfn main() {}\n```\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let d = parse(src);
        assert!(matches!(&d.blocks[0], Block::Heading { level: 1, text } if text.text == "Plan"));
        let p = para(&d.blocks[1]);
        assert_eq!(p.text, "Some bold, it, old and code with a link.");
        let styled = |s: &str| p.spans.iter().find(|sp| &p.text[sp.range.clone()] == s).cloned().unwrap();
        assert!(styled("bold").style.bold && styled("it").style.italic && styled("old").style.strike && styled("code").style.code);
        assert_eq!(styled("link").link.as_deref(), Some("https://x.org"));
        assert!(matches!(&d.blocks[2], Block::Quote(v) if para(&v[0]).text == "quoted"));
        let Block::List { start: None, items } = &d.blocks[3] else { panic!("{:?}", d.blocks[3]) };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1].blocks[1], Block::List { start: Some(1), .. }), "{:?}", items[1]);
        assert!(matches!(&d.blocks[4], Block::Code { lang, text } if lang == "rust" && text == "fn main() {}"));
        assert_eq!(d.blocks[5], Block::Rule);
        let Block::Table { head, rows } = &d.blocks[6] else { panic!() };
        assert_eq!(head.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(rows[0][1].text, "2");
    }

    #[test]
    fn task_lists_toggle_their_own_marker() {
        let src = "Shopping\n\n- [ ] milk\n- [x] bread\n";
        let d = parse(src);
        let Block::List { items, .. } = &d.blocks[1] else { panic!() };
        let (open, r0) = items[0].task.clone().unwrap();
        let (done, r1) = items[1].task.clone().unwrap();
        assert!(!open && done);
        assert_eq!(&src[r0.clone()], "[ ]");
        let once = toggle_task(src, r0).unwrap();
        assert_eq!(once, "Shopping\n\n- [x] milk\n- [x] bread\n");
        assert_eq!(toggle_task(&once, r1).unwrap(), "Shopping\n\n- [x] milk\n- [ ] bread\n");
        assert_eq!(toggle_task(src, 0..3), None, "only real markers flip");
    }

    #[test]
    fn titles_and_plain_text() {
        assert_eq!(title_line("\n\n## **Trip** to Siwa\nbody").as_deref(), Some("Trip to Siwa"));
        assert_eq!(title_line("   "), None);
        let mut s = String::new();
        plain(&parse("a *b*\n\n- c\n").blocks, &mut s);
        assert_eq!(s, "a b c");
        // Arabic text parses untouched.
        assert_eq!(para(&parse("مرحبا **بالعالم**").blocks[0]).text, "مرحبا بالعالم");
    }

    #[test]
    fn editing_helpers() {
        let mut t = String::from("- [x] done");
        assert!(continue_list(&mut t));
        assert_eq!(t, "- [x] done\n- [ ] ");
        assert!(continue_list(&mut t), "an empty item ends the list");
        assert_eq!(t, "- [x] done\n");
        let mut n = String::from("  9. nine");
        continue_list(&mut n);
        assert_eq!(n, "  9. nine\n  10. ");
        let mut p = String::from("plain");
        assert!(!continue_list(&mut p));
        let mut b = String::from("say");
        toggle_mark(&mut b, "**");
        b.push_str("hi");
        toggle_mark(&mut b, "**");
        assert_eq!(b, "say **hi**");
        let mut l = String::new();
        link_step(&mut l);
        l.push_str("Mandara");
        link_step(&mut l);
        l.push_str("https://x.org");
        link_step(&mut l);
        assert_eq!(l, "[Mandara](https://x.org)");
        let mut bl = String::from("a\nitem");
        toggle_bullet(&mut bl);
        assert_eq!(bl, "a\n- item");
        toggle_bullet(&mut bl);
        assert_eq!(bl, "a\nitem");
        let mut i = String::from("- x");
        assert!(indent(&mut i, false));
        assert_eq!(i, "  - x");
        assert!(indent(&mut i, true));
        assert_eq!(i, "- x");
        assert!(!indent(&mut String::from("x"), false));
    }
}
