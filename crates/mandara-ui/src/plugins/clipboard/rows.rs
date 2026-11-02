//! One row per history entry, drawn by kind: colour swatches with their
//! value, image thumbnails, code and CSS in monospace with a light tint,
//! Markdown rendered, files with an icon, name and folder, links by host.
//! What is costly to derive (titles, tinted lines, a parsed Markdown head)
//! is computed once per entry in [`RowText`] and cached by the panel.

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, FontWeight, HighlightStyle, ImageSource, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled,
    StyledImage as _, StyledText, div, img, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::clipboard::{ClipClass, ClipCollection, ClipContent, ClipEntry, Kind};
use mandara_theme::{Color, MandaraPalette};

use super::tint::{self, Tok};
use crate::icons::icon;
use crate::kit::{icon_button, text, tokens};
use crate::markdown;
use crate::plugins::panel_fx::{WASH_SOFT, alpha, readable, wash};

// ── Geometry ─────────────────────────────────────────────────────────────

/// One-line rows (text, links, colours, files).
pub const TEXT_H: f32 = 52.0;
/// Image rows (room for the thumbnail).
pub const IMAGE_H: f32 = 84.0;
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_PAD_Y_PX: f32 = 7.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_GAP_PX: f32 = 12.0;
const ROW_TEXT_GAP_PX: f32 = 2.0;
/// Row title: half a step above the small body size.
const ROW_TITLE_PX: f32 = text::BODY_S + 0.5;
/// Height of the meta line under a preview.
const META_H_PX: f32 = 15.0;
/// Image preview tile.
pub const THUMB_W_PX: f32 = 104.0;
pub const THUMB_H_PX: f32 = IMAGE_H - 16.0;
const THUMB_RADIUS_PX: f32 = 10.0;
/// Kind badge (and colour swatch).
const BADGE_PX: f32 = 34.0;
const BADGE_RADIUS_PX: f32 = 10.0;
const BADGE_ICON_PX: f32 = 15.0;
const BADGE_ALPHA: (f32, f32) = (0.14, 0.1);
/// Ring around a colour swatch, so near-surface colours stay visible.
const SWATCH_RING_ALPHA: f32 = 0.18;
/// Row buttons (collection, pin, edit, delete).
pub const ROW_BUTTON_PX: f32 = 28.0;
const BUTTON_ALPHA: f32 = 0.8;
/// The floating action pill shown on hover: inner padding, gap between its
/// buttons and its soft shadow.
const ACTIONS_PAD_PX: f32 = 3.0;
const ACTIONS_GAP_PX: f32 = 1.0;
const ACTIONS_SHADOW_ALPHA: f32 = 0.5;
const ACTIONS_SHADOW_BLUR_PX: f32 = 10.0;
/// The quiet marks a pinned / collected row keeps at its end: pin glyph,
/// collection dots (at most this many), and the room kept for them.
const MARK_ICON_PX: f32 = 12.0;
const MARK_DOT_PX: f32 = 6.0;
const MARK_GAP_PX: f32 = 4.0;
const MAX_MARK_DOTS: usize = 3;
const MARK_ROOM_PX: f32 = 44.0;
/// Code / CSS preview lines: text size and line height, box padding.
const CODE_TEXT_PX: f32 = text::LABEL + 0.5;
const CODE_LINE_PX: f32 = 16.0;
/// Markdown previews keep the notes' line height.
const MD_LINE_PX: f32 = 19.0;
/// Gap between Markdown blocks (the notes' block gap).
const MD_BLOCK_GAP_PX: f32 = tokens::SM;
const PREVIEW_PAD_X_PX: f32 = 8.0;
const PREVIEW_PAD_Y_PX: f32 = 4.0;
const PREVIEW_RADIUS_PX: f32 = 8.0;
const PREVIEW_BG: (f32, f32) = (0.05, 0.04);
/// Comment runs are muted this much.
const COMMENT_ALPHA: f32 = 0.75;
/// Characters kept per previewed line (the rest is clipped anyway).
const LINE_CHARS: usize = 160;
/// Spaces per tab in previews.
const TAB_SPACES: usize = 4;
/// Source lines parsed for a Markdown preview, per line shown.
const MD_SOURCE_LINES_PER_ROW: usize = 2;
/// Bytes of source parsed for a Markdown preview at most.
const MD_SOURCE_BYTES: usize = 2048;
/// Characters of a text entry shown as its title.
const PREVIEW_CHARS: usize = 90;
/// Collection picker strip under a row.
pub const PICKER_H: f32 = 36.0;
const PICKER_CHIP_H_PX: f32 = 26.0;
const PICKER_PAD_X_PX: f32 = 10.0;
const PICKER_DOT_PX: f32 = 7.0;
const PICKER_GAP_PX: f32 = 6.0;
const PICKER_ICON_PX: f32 = 12.0;
const PICKER_IDLE_ALPHA: f32 = 0.06;
const PICKER_ACTIVE_ALPHA: f32 = 0.22;

/// Seconds per minute / hour / day for the "copied … ago" label.
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 60 * SECS_PER_MIN;
const SECS_PER_DAY: i64 = 24 * SECS_PER_HOUR;

pub fn ago(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    let secs = Local::now().timestamp() - ts;
    match secs {
        ..SECS_PER_MIN => t("now").to_string(),
        SECS_PER_MIN..SECS_PER_HOUR => tf("{n}m", &[("n", &(secs / SECS_PER_MIN))]),
        SECS_PER_HOUR..SECS_PER_DAY => tf("{n}h", &[("n", &(secs / SECS_PER_HOUR))]),
        _ => Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d %b").to_string()).unwrap_or_default(),
    }
}

// ── Kinds ────────────────────────────────────────────────────────────────

/// The icon and (translated) plural label of a kind, for chips and badges.
pub fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "note",
        Kind::Link => "globe",
        Kind::Markdown => "list-check",
        Kind::Code => "terminal",
        Kind::Css => "brush",
        Kind::Color => "palette",
        Kind::Image => "image",
        Kind::Files => "folder",
    }
}

/// English key of a kind's chip label (`tr` it).
pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => mandara_i18n::tk("Text"),
        Kind::Link => mandara_i18n::tk("Links"),
        Kind::Markdown => mandara_i18n::tk("Markdown"),
        Kind::Code => mandara_i18n::tk("Code"),
        Kind::Css => mandara_i18n::tk("CSS"),
        Kind::Color => mandara_i18n::tk("Colors"),
        Kind::Image => mandara_i18n::tk("Images"),
        Kind::Files => mandara_i18n::tk("Files"),
    }
}

/// A language id as people write it (`javascript` → JavaScript).
pub fn lang_name(lang: &str) -> String {
    match lang {
        "" => t("Code").to_string(),
        "javascript" => "JavaScript".into(),
        "typescript" => "TypeScript".into(),
        "csharp" => "C#".into(),
        "cpp" => "C++".into(),
        "php" | "sql" | "css" | "json" | "yaml" | "toml" | "html" | "xml" => lang.to_uppercase(),
        "powershell" => "PowerShell".into(),
        "dockerfile" => "Dockerfile".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// An icon for a file by its extension (folders have none).
fn file_icon(path: &std::path::Path) -> &'static str {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "" => "folder",
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "avif" | "bmp" | "heic" => "image",
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" => "music",
        "mp4" | "mkv" | "webm" | "mov" | "avi" => "camera",
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" | "deb" | "rpm" => "box",
        "rs" | "py" | "js" | "ts" | "go" | "c" | "h" | "cpp" | "java" | "sh" | "toml" | "json" | "yaml" | "yml" | "css" | "html" => "terminal",
        _ => "note",
    }
}

/// `~/…` for paths under the home folder.
fn tilde(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(std::path::Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

// ── Per-entry derived text ───────────────────────────────────────────────

/// A tinted preview line.
pub struct Line {
    pub text: SharedString,
    pub runs: Vec<(Range<usize>, Tok)>,
}

/// Everything about an entry that is costly to derive, computed once per
/// entry (and again only when the preview length changes).
pub struct RowText {
    /// The title (one-line rows).
    pub title: SharedString,
    /// Kind details for the meta line (the age is added when drawn).
    pub meta: SharedString,
    /// Code / CSS preview lines.
    pub lines: Vec<Line>,
    /// Parsed head of a Markdown entry.
    pub doc: Option<Rc<markdown::Doc>>,
    /// Lead icon for files.
    pub file_icon: &'static str,
    /// The preview length this was derived for.
    pub preview_lines: usize,
}

/// The first `n` lines worth showing of `text`: leading blank lines
/// skipped, common indentation removed, tabs expanded, long lines cut.
fn head_lines(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().skip_while(|l| l.trim().is_empty()).take(n).collect();
    let indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            let l = l.get(indent..).unwrap_or(l.trim_start()).replace('\t', &" ".repeat(TAB_SPACES));
            l.chars().take(LINE_CHARS).collect()
        })
        .collect()
}

/// Lines in a text (counting stops early on huge copies).
fn line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

/// "Rust · 12 lines", or "· 1 line" for one.
fn lines_meta(kind: &str, text: &str) -> SharedString {
    match line_count(text) {
        1 => tf("{kind} · 1 line", &[("kind", &kind)]).into(),
        n => tf("{kind} · {lines} lines", &[("kind", &kind), ("lines", &n)]).into(),
    }
}

/// How many lines a code preview of `text` shows (at most `n`): the box
/// is as tall as what it holds.
fn shown_lines(text: &str, n: usize) -> usize {
    text.lines().skip_while(|l| l.trim().is_empty()).take(n).count().clamp(1, n.max(1))
}

impl RowText {
    pub fn of(e: &ClipEntry, preview_lines: usize) -> Self {
        let mut row = Self { title: SharedString::default(), meta: SharedString::default(), lines: Vec::new(), doc: None, file_icon: "note", preview_lines };
        match (&e.content, &e.class) {
            (ClipContent::Image { mime, .. }, ClipClass::Image { width, height, screenshot }) => {
                row.title = if *screenshot { t("Screenshot") } else { t("Image") }.into();
                let format = mime.trim_start_matches("image/").to_uppercase();
                row.meta = if *width > 0 { format!("{format} · {width}×{height}").into() } else { format.into() };
            }
            (ClipContent::Image { mime, .. }, _) => {
                row.title = t("Image").into();
                row.meta = mime.trim_start_matches("image/").to_uppercase().into();
            }
            (ClipContent::Files { .. }, _) => {
                let paths = e.file_paths();
                let first = paths.first().cloned().unwrap_or_default();
                let name = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| first.display().to_string());
                row.title = if paths.len() > 1 { tf("{name} and {count} more", &[("name", &name), ("count", &(paths.len() - 1))]).into() } else { name.into() };
                row.meta = first.parent().map(tilde).unwrap_or_default().into();
                row.file_icon = if paths.len() > 1 { "folder" } else { file_icon(&first) };
            }
            (ClipContent::Text { text }, class) => {
                let chars = text.chars().count();
                let kind_meta = |kind: &str| -> SharedString { tf("{kind} · {chars} chars", &[("kind", &kind), ("chars", &chars)]).into() };
                match class {
                    ClipClass::Color { notation, rgba } => {
                        row.title = text.trim().trim_end_matches(';').trim().to_string().into();
                        let hex = format!("#{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2]);
                        let shown = if notation == "hex" { format!("rgb({} {} {})", rgba[0], rgba[1], rgba[2]) } else { hex };
                        row.meta = format!("{} · {shown}", notation.to_uppercase()).into();
                    }
                    ClipClass::Link { host } => {
                        row.title = host.clone().into();
                        let rest = text.trim().split_once("://").map_or(text.trim(), |(_, r)| r);
                        row.meta = rest.to_string().into();
                    }
                    ClipClass::Files { .. } => {
                        let paths: Vec<PathBuf> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(mandara_services::clipboard::kind::uri_path).collect();
                        let first = paths.first().cloned().unwrap_or_default();
                        row.title = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into();
                        row.meta = first.parent().map(tilde).unwrap_or_default().into();
                        row.file_icon = file_icon(&first);
                    }
                    ClipClass::Code { lang } => {
                        row.title = e.preview(PREVIEW_CHARS).into();
                        row.meta = lines_meta(&lang_name(lang), text);
                        row.lines = tinted(text, lang, preview_lines);
                    }
                    ClipClass::Css => {
                        row.title = e.preview(PREVIEW_CHARS).into();
                        row.meta = lines_meta("CSS", text);
                        row.lines = tinted(text, "css", preview_lines);
                    }
                    ClipClass::Markdown => {
                        row.title = markdown::title_line(text).unwrap_or_else(|| e.preview(PREVIEW_CHARS)).into();
                        row.meta = kind_meta(t("Markdown"));
                        if preview_lines > 0 {
                            let source: String = text.lines().take(preview_lines * MD_SOURCE_LINES_PER_ROW).collect::<Vec<_>>().join("\n");
                            let cut = (0..=source.len().min(MD_SOURCE_BYTES)).rev().find(|i| source.is_char_boundary(*i)).unwrap_or(0);
                            row.doc = Some(Rc::new(compact(markdown::parse(&source[..cut]), preview_lines)));
                        }
                    }
                    _ => {
                        row.title = e.preview(PREVIEW_CHARS).into();
                        row.meta = kind_meta(t("Text"));
                    }
                }
            }
        }
        row
    }
}

/// `inline` with every run bold (a heading drawn at body size).
fn emboldened(mut inline: markdown::Inline) -> markdown::Inline {
    if inline.spans.is_empty() && !inline.text.is_empty() {
        inline.spans.push(markdown::Span { range: 0..inline.text.len(), style: markdown::Style::default(), link: None });
    }
    for s in &mut inline.spans {
        s.style.bold = true;
    }
    inline
}

/// A Markdown head shrunk to at most `n` one-line blocks, so a row's
/// preview never cuts a line in half: headings become bold lines, a list
/// keeps one line per item (nested lists dropped), a code block or table
/// shows its first line, rules are skipped. With the box set to not wrap,
/// each kept block is exactly one line tall.
fn compact(doc: markdown::Doc, n: usize) -> markdown::Doc {
    use markdown::Block;
    let first_line = |inline: markdown::Inline| {
        let end = inline.text.find('\n').unwrap_or(inline.text.len());
        let spans = inline.spans.into_iter().filter(|s| s.range.start < end).map(|mut s| {
            s.range.end = s.range.end.min(end);
            s
        });
        markdown::Inline { spans: spans.collect(), text: inline.text[..end].to_string() }
    };
    let mut left = n;
    let mut out = Vec::new();
    for block in doc.blocks {
        if left == 0 {
            break;
        }
        let kept = match block {
            Block::Heading { text, .. } => Block::Paragraph(emboldened(first_line(text))),
            Block::Paragraph(text) => Block::Paragraph(first_line(text)),
            Block::Code { text, .. } => {
                let line = text.lines().next().unwrap_or_default().to_string();
                let style = markdown::Style { code: true, ..Default::default() };
                Block::Paragraph(markdown::Inline { spans: vec![markdown::Span { range: 0..line.len(), style, link: None }], text: line })
            }
            Block::Quote(inner) => match inner.into_iter().find_map(|b| if let Block::Paragraph(p) = b { Some(p) } else { None }) {
                Some(p) => Block::Quote(vec![Block::Paragraph(first_line(p))]),
                None => continue,
            },
            Block::List { start, items } => {
                let items: Vec<markdown::Item> = items
                    .into_iter()
                    .take(left)
                    .map(|item| {
                        let lead = item.blocks.into_iter().find_map(|b| match b {
                            Block::Paragraph(p) | Block::Heading { text: p, .. } => Some(Block::Paragraph(first_line(p))),
                            _ => None,
                        });
                        markdown::Item { task: item.task, blocks: lead.into_iter().collect() }
                    })
                    .collect();
                left -= items.len().saturating_sub(1);
                Block::List { start, items }
            }
            Block::Table { head, .. } => {
                let text = head.into_iter().map(|c| c.text).collect::<Vec<_>>().join(" · ");
                Block::Paragraph(emboldened(markdown::Inline { text, spans: Vec::new() }))
            }
            Block::Rule => continue,
        };
        out.push(kept);
        left -= 1;
    }
    markdown::Doc { blocks: out }
}

/// Height of a compact Markdown preview of `n` lines: the lines plus, at
/// most, a block gap between each (list items sit closer).
fn md_box_h(n: usize) -> f32 {
    MD_LINE_PX * n as f32 + MD_BLOCK_GAP_PX * n.saturating_sub(1) as f32
}

fn tinted(text: &str, lang: &str, n: usize) -> Vec<Line> {
    let mut block = false;
    head_lines(text, n)
        .into_iter()
        .map(|l| {
            let runs = tint::line(&l, lang, &mut block);
            Line { text: l.into(), runs }
        })
        .collect()
}

/// Whether the entry draws a multi-line preview at `preview_lines`.
fn previews(e: &ClipEntry, preview_lines: usize) -> bool {
    preview_lines > 0 && matches!(e.content, ClipContent::Text { .. }) && matches!(e.class.kind(), Kind::Code | Kind::Css | Kind::Markdown)
}

/// A row's height (without an open collection picker).
pub fn row_h(e: &ClipEntry, preview_lines: usize) -> f32 {
    if matches!(e.content, ClipContent::Image { .. }) {
        return IMAGE_H;
    }
    if !previews(e, preview_lines) {
        return TEXT_H;
    }
    let shown = match &e.content {
        ClipContent::Text { text } => shown_lines(text, preview_lines),
        _ => preview_lines,
    };
    let body = if e.class.kind() == Kind::Markdown { md_box_h(preview_lines) } else { CODE_LINE_PX * shown as f32 };
    ROW_PAD_Y_PX * 2.0 + PREVIEW_PAD_Y_PX * 2.0 + body + ROW_TEXT_GAP_PX + META_H_PX
}

// ── Thumbnails ───────────────────────────────────────────────────────────

/// A downscaled image preview, decoded off the UI thread.
pub enum Thumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

// ── Drawing ──────────────────────────────────────────────────────────────

/// What a row needs from the panel besides its entry.
pub struct RowCx<'a> {
    pub index: usize,
    pub text: Option<&'a RowText>,
    pub thumb: Option<&'a Thumb>,
    pub mono: SharedString,
    pub preview_lines: usize,
    /// An image editor command is set (shows the edit button on images).
    pub can_edit: bool,
    /// The collection picker is open under this row.
    pub picking: bool,
}

/// What clicking a row's parts does (the panel wires them to its state).
pub struct RowActions {
    pub restore: Rc<dyn Fn(bool, &mut gpui::App)>,
    pub pin: Rc<dyn Fn(&mut gpui::App)>,
    pub delete: Rc<dyn Fn(&mut gpui::App)>,
    pub edit: Rc<dyn Fn(&mut gpui::App)>,
    pub pick: Rc<dyn Fn(&mut gpui::App)>,
}

fn tok_color(tok: Tok, palette: &MandaraPalette) -> Color {
    match tok {
        Tok::Keyword => readable(palette.primary, palette),
        Tok::Str => readable(palette.tertiary, palette),
        Tok::Number => readable(palette.warning_amber, palette),
        Tok::Comment => palette.on_surface_variant.with_alpha(COMMENT_ALPHA),
        Tok::Key => readable(palette.secondary, palette),
    }
}

fn preview_box(e: &ClipEntry, r: &RowText, rcx: &RowCx, palette: &MandaraPalette) -> AnyElement {
    // As many lines as the entry has, up to the setting (see `row_h`).
    let n = r.lines.len().clamp(1, rcx.preview_lines.max(1)) as f32;
    let shell = div()
        .w_full()
        .px(px(PREVIEW_PAD_X_PX))
        .py(px(PREVIEW_PAD_Y_PX))
        .rounded(px(crate::style::radius(PREVIEW_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(alpha(PREVIEW_BG, palette)).hsla())
        .overflow_hidden();
    if let Some(doc) = &r.doc {
        let rcx_md = markdown::RenderCx { id: SharedString::from(format!("clip-md-{}", e.id)), on_toggle: None, mono: rcx.mono.clone() };
        return shell.h(px(md_box_h(rcx.preview_lines) + PREVIEW_PAD_Y_PX * 2.0)).whitespace_nowrap().child(markdown::render(doc, palette, &rcx_md)).into_any_element();
    }
    // Code reads left to right in every language.
    shell
        .h(px(CODE_LINE_PX * n + PREVIEW_PAD_Y_PX * 2.0))
        .font_family(rcx.mono.clone())
        .text_size(px(CODE_TEXT_PX))
        .line_height(px(CODE_LINE_PX))
        .text_left()
        .flex()
        .flex_col()
        .children(r.lines.iter().map(|l| {
            let highlights: Vec<(Range<usize>, HighlightStyle)> = l.runs.iter().map(|(range, tok)| (range.clone(), HighlightStyle { color: Some(tok_color(*tok, palette).hsla()), ..Default::default() })).collect();
            div().h(px(CODE_LINE_PX)).whitespace_nowrap().overflow_hidden().child(StyledText::new(if l.text.is_empty() { SharedString::from(" ") } else { l.text.clone() }).with_highlights(highlights))
        }))
        .into_any_element()
}

/// The leading tile: thumbnail, colour swatch, file icon or kind badge.
fn lead(e: &ClipEntry, rcx: &RowCx, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let badge = |icon_name: &'static str| {
        div()
            .size(px(BADGE_PX))
            .flex_none()
            .rounded(px(crate::style::radius(BADGE_RADIUS_PX)))
            .bg(accent.with_alpha(alpha(BADGE_ALPHA, palette)).hsla())
            .flex()
            .items_center()
            .justify_center()
            .child(icon(icon_name, BADGE_ICON_PX, accent))
            .into_any_element()
    };
    match (&e.content, &e.class) {
        (ClipContent::Image { .. }, _) => {
            let tile = div().w(px(THUMB_W_PX)).h(px(THUMB_H_PX)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS_PX)));
            match rcx.thumb {
                Some(Thumb::Ready(image)) => img(ImageSource::Render(image.clone())).w(px(THUMB_W_PX)).h(px(THUMB_H_PX)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS_PX))).object_fit(ObjectFit::Cover).into_any_element(),
                // Same-size skeleton while decoding: the row never reflows.
                Some(Thumb::Loading) | None => tile.bg(wash(WASH_SOFT, palette).hsla()).into_any_element(),
                Some(Thumb::Failed) => tile.bg(wash(WASH_SOFT, palette).hsla()).flex().items_center().justify_center().child(icon("image", BADGE_ICON_PX, palette.on_surface_variant)).into_any_element(),
            }
        }
        (_, ClipClass::Color { rgba, .. }) => {
            let fill = Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
            div()
                .size(px(BADGE_PX))
                .flex_none()
                .rounded(px(crate::style::radius(BADGE_RADIUS_PX)))
                .bg(fill.hsla())
                .border_1()
                .border_color(palette.on_surface.with_alpha(SWATCH_RING_ALPHA).hsla())
                .into_any_element()
        }
        (ClipContent::Files { .. }, _) | (_, ClipClass::Files { .. }) => badge(rcx.text.map_or("folder", |r| r.file_icon)),
        (_, class) => badge(kind_icon(class.kind())),
    }
}

pub fn entry_row(e: &ClipEntry, rcx: &RowCx, palette: &MandaraPalette, actions: RowActions) -> AnyElement {
    let i = rcx.index;
    let hover = wash(WASH_SOFT, palette);
    let is_image = matches!(e.content, ClipContent::Image { .. });
    let preview = previews(e, rcx.preview_lines);
    let (title, meta) = match rcx.text {
        Some(r) if r.meta.is_empty() => (r.title.clone(), SharedString::from(ago(e.copied_at))),
        Some(r) => (r.title.clone(), SharedString::from(format!("{} · {}", r.meta, ago(e.copied_at)))),
        None => (SharedString::default(), SharedString::default()),
    };
    let mono_title = matches!(e.class.kind(), Kind::Code | Kind::Css | Kind::Color);
    let meta_el = div().h(px(META_H_PX)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(meta);
    let body = match rcx.text {
        Some(r) if preview => div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(ROW_TEXT_GAP_PX)).child(preview_box(e, r, rcx, palette)).child(meta_el),
        _ => div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(ROW_TEXT_GAP_PX))
            .child(div().text_size(px(ROW_TITLE_PX)).font_weight(FontWeight::MEDIUM).when(mono_title, |el| el.font_family(rcx.mono.clone())).truncate().child(title))
            .child(meta_el),
    };
    let quiet = palette.on_surface_variant.with_alpha(BUTTON_ALPHA);
    let in_collection = !e.collections.is_empty();
    let RowActions { restore, pin, delete, edit, pick } = actions;
    let restore_right = restore.clone();
    // The actions float over the row's end while it is hovered (or its
    // collection picker is open), so the content gets the full width; a
    // pinned or collected entry keeps a quiet mark at the end instead.
    let group = SharedString::from(format!("clip-row-{i}"));
    let marks = div()
        .absolute()
        .right(px(ROW_PAD_X_PX + ACTIONS_PAD_PX))
        .when(preview, |el| el.top(px(ROW_PAD_Y_PX + ACTIONS_PAD_PX)))
        .when(!preview, |el| el.top_0().bottom_0())
        .flex()
        .items_center()
        .gap(px(MARK_GAP_PX))
        .when(e.pinned, |el| el.child(icon("pin", MARK_ICON_PX, readable(palette.primary, palette))))
        .children(e.collections.iter().take(MAX_MARK_DOTS).map(|c| div().size(px(MARK_DOT_PX)).rounded_full().bg(collection_tint(*c, palette).hsla())));
    let actions_pill = div()
        .flex()
        .items_center()
        .gap(px(ACTIONS_GAP_PX))
        .p(px(ACTIONS_PAD_PX))
        .rounded_full()
        .bg(palette.surface_container_high.hsla())
        .shadow(vec![crate::plugins::panel_fx::glow(palette.surface_dim.with_alpha(ACTIONS_SHADOW_ALPHA), ACTIONS_SHADOW_BLUR_PX, 0.0, 2.0)]);
    let actions_pill = actions_pill
        .when(is_image && rcx.can_edit, |el| {
            el.child(icon_button(("clip-edit", i), "crop", ROW_BUTTON_PX, quiet, None, move |_, cx| {
                cx.stop_propagation();
                edit(cx);
            }))
        })
        .child(icon_button(
            ("clip-collect", i),
            "folder",
            ROW_BUTTON_PX,
            if in_collection || rcx.picking { readable(palette.primary, palette) } else { quiet },
            rcx.picking.then(|| palette.primary.with_alpha(alpha(BADGE_ALPHA, palette))),
            move |_, cx| {
                cx.stop_propagation();
                pick(cx);
            },
        ))
        .child(icon_button(("clip-pin", i), if e.pinned { "pin-off" } else { "pin" }, ROW_BUTTON_PX, if e.pinned { readable(palette.primary, palette) } else { quiet }, None, move |_, cx| {
            cx.stop_propagation();
            pin(cx);
        }))
        .child(icon_button(("clip-del", i), "trash", ROW_BUTTON_PX, quiet, None, move |_, cx| {
            // Don't let the row's click (copy) fire too.
            cx.stop_propagation();
            delete(cx);
        }));
    let overlay = div()
        .absolute()
        .right(px(ROW_PAD_X_PX))
        .when(preview, |el| el.top(px(ROW_PAD_Y_PX)))
        .when(!preview, |el| el.top_0().bottom_0())
        .flex()
        .items_center()
        .when(!rcx.picking, |el| el.invisible().group_hover(group.clone(), |s| s.visible()))
        .child(actions_pill);
    div()
        .id(("clip", i))
        .group(group.clone())
        .relative()
        .h(px(row_h(e, rcx.preview_lines)))
        .px(px(ROW_PAD_X_PX))
        .when(preview, |el| el.py(px(ROW_PAD_Y_PX)))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .when(preview, |el| el.items_start())
        .when(!preview, |el| el.items_center())
        .gap(px(ROW_GAP_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(lead(e, rcx, palette))
        .child(body.when(e.pinned || in_collection, |el| el.pr(px(MARK_ROOM_PX))))
        .when(!rcx.picking, |el| el.child(marks.group_hover(group.clone(), |s| s.invisible())))
        .child(overlay)
        .on_click(move |_, _, cx| restore(false, cx))
        .on_mouse_down(MouseButton::Right, move |_, _, cx| restore_right(true, cx))
        .into_any_element()
}

/// A stable tint per collection: the primary hue turned by the golden angle
/// for each id (OKLCH, so every tint keeps the theme's lightness).
pub fn collection_tint(id: u32, palette: &MandaraPalette) -> Color {
    const GOLDEN_ANGLE_DEG: f32 = 137.508;
    readable(palette.primary.rotate_hue(id as f32 * GOLDEN_ANGLE_DEG), palette)
}

/// The strip under a row that adds it to collections (or takes it out).
pub fn picker(e: &ClipEntry, collections: &[ClipCollection], palette: &MandaraPalette, on_toggle: Rc<dyn Fn(u32, bool, &mut gpui::App)>, on_new: Rc<dyn Fn(&mut gpui::Window, &mut gpui::App)>) -> AnyElement {
    let chips = collections.iter().map(|c| {
        let member = e.collections.contains(&c.id);
        let tint = collection_tint(c.id, palette);
        let (id, toggle) = (c.id, on_toggle.clone());
        div()
            .id(("clip-pick", u64::from(c.id)))
            .h(px(PICKER_CHIP_H_PX))
            .px(px(PICKER_PAD_X_PX))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(PICKER_GAP_PX))
            .cursor_pointer()
            .text_size(px(text::BODY_S))
            .font_weight(if member { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .bg(if member { tint.with_alpha(PICKER_ACTIVE_ALPHA).hsla() } else { palette.on_surface.with_alpha(PICKER_IDLE_ALPHA).hsla() })
            .hover(move |s| s.bg(tint.with_alpha(PICKER_ACTIVE_ALPHA).hsla()))
            .child(if member { icon("check", PICKER_ICON_PX, tint).into_any_element() } else { div().size(px(PICKER_DOT_PX)).rounded_full().bg(tint.hsla()).into_any_element() })
            .child(c.name.clone())
            .on_click(move |_, _, cx| toggle(id, !member, cx))
            .into_any_element()
    });
    let new = div()
        .id("clip-pick-new")
        .h(px(PICKER_CHIP_H_PX))
        .px(px(PICKER_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(tokens::XS))
        .cursor_pointer()
        .text_size(px(text::BODY_S))
        .text_color(palette.on_surface_variant.hsla())
        .bg(palette.on_surface.with_alpha(PICKER_IDLE_ALPHA).hsla())
        .hover(|s| s.bg(wash(WASH_SOFT, palette).hsla()))
        .child(icon("plus", PICKER_ICON_PX, palette.on_surface_variant))
        .child(t("New collection"))
        .on_click(move |_, window, cx| on_new(window, cx));
    div()
        .id(("clip-picker", e.id))
        .h(px(PICKER_H))
        .pl(px(ROW_PAD_X_PX + BADGE_PX + ROW_GAP_PX))
        .overflow_x_scroll()
        .flex()
        .items_center()
        .gap(px(PICKER_GAP_PX))
        .children(chips)
        .child(new)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::clipboard::kind::classify_text;

    fn entry(text: &str) -> ClipEntry {
        ClipEntry { id: 1, content: ClipContent::Text { text: text.into() }, copied_at: 0, pinned: false, app_id: String::new(), class: classify_text(text), collections: Vec::new() }
    }

    #[test]
    fn previews_by_kind() {
        let code = entry("\n\n    fn main() {\n    \tprintln!(\"hi\");\n    }\n");
        let r = RowText::of(&code, 2);
        assert_eq!(r.lines.iter().map(|l| l.text.to_string()).collect::<Vec<_>>(), ["fn main() {", "    println!(\"hi\");"]);
        assert!(r.lines[0].runs.iter().any(|(_, t)| *t == Tok::Keyword));
        assert!(row_h(&code, 2) > TEXT_H && row_h(&code, 0) == TEXT_H);
        let colour = entry("oklch(70% 0.15 250);");
        let r = RowText::of(&colour, 3);
        assert_eq!(r.title.as_ref(), "oklch(70% 0.15 250)");
        assert!(r.meta.starts_with("OKLCH · #"));
        assert_eq!(row_h(&colour, 3), TEXT_H);
        let link = entry("https://github.com/theHamdiz/mandara");
        let r = RowText::of(&link, 3);
        assert_eq!((r.title.as_ref(), r.meta.as_ref()), ("github.com", "github.com/theHamdiz/mandara"));
        let md = entry("# Plan\n\n- ship\n- rest");
        assert!(RowText::of(&md, 2).doc.is_some());
        assert!(RowText::of(&md, 0).doc.is_none());
        // Compact previews: a heading becomes a bold line, lists keep one
        // line per item, and the whole fits the line budget.
        let doc = compact(markdown::parse("## Corniche walk\n\n- [x] shells\n- [ ] lighthouse\n  - nested\n- [ ] sunset\n\n---\n\nAfter"), 3);
        let markdown::Block::Paragraph(head) = &doc.blocks[0] else { panic!("{doc:?}") };
        assert!(head.text == "Corniche walk" && head.spans.iter().all(|s| s.style.bold));
        let markdown::Block::List { items, .. } = &doc.blocks[1] else { panic!("{doc:?}") };
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].blocks.len(), 1, "nested lists are dropped");
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(compact(markdown::parse("```rust\nfn a() {}\nfn b() {}\n```\n\n> quote\n\nend"), 8).blocks.len(), 3);
    }

    #[test]
    fn file_rows() {
        let e = ClipEntry {
            id: 2,
            content: ClipContent::Files { uris: vec!["file:///srv/demo/Pictures/harbour.png".into(), "file:///srv/demo/notes.md".into()] },
            copied_at: 0,
            pinned: false,
            app_id: String::new(),
            class: ClipClass::Files { count: 2 },
            collections: Vec::new(),
        };
        let r = RowText::of(&e, 3);
        assert_eq!(r.meta.as_ref(), "/srv/demo/Pictures");
        assert_eq!(r.file_icon, "folder");
        assert_eq!(file_icon(std::path::Path::new("a/b.PNG")), "image");
        assert_eq!(lang_name("javascript"), "JavaScript");
        assert_eq!(lang_name("rust"), "Rust");
        assert_eq!(lang_name("json"), "JSON");
    }
}
