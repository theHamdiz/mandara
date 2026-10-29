//! A small text field (focus, caret, selection, clipboard, Enter/blur to
//! commit), shared by Studio and the shell's plugins. Multi-line fields
//! insert newlines on Enter and commit on Ctrl+Enter, Escape or blur.
//!
//! Editing keys: ←/→ (Ctrl: by word), Home/End (Ctrl: whole text), ↑/↓ in
//! multi-line fields, Shift with any of them to select, Ctrl+A, Ctrl+C /
//! Ctrl+X / Ctrl+V, Backspace / Delete (Ctrl: a word). Click places the
//! caret, Shift+click extends, drag selects, double-click selects a word.
//! Masked (password) fields never copy or cut to the clipboard.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, ClipboardItem, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Render, Styled, Window, canvas, div, fill, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;

use crate::bidi::ParagraphLayout;

/// Drawn per character of a masked field.
const BULLET: char = '•';
/// Caret bar width and selection tint.
const CARET_W_PX: f32 = 1.5;
const SELECTION_ALPHA: f32 = 0.3;

pub type OnCommit = Rc<dyn Fn(String, &mut Window, &mut App)>;

pub struct TextField {
    pub value: String,
    /// Caret byte offset into `value`, and the other end of the selection.
    caret: usize,
    anchor: Option<usize>,
    /// The text's rows from the last frame (caret, clicks, selection).
    layout: ParagraphLayout,
    /// A drag selection is in progress.
    dragging: bool,
    placeholder: String,
    focus: FocusHandle,
    on_commit: OnCommit,
    /// Commit on every keystroke (search boxes) instead of Enter/blur.
    live: bool,
    multiline: bool,
    /// Empty the field after each Enter commit ("Add a task…" inputs).
    clear_on_commit: bool,
    /// Password entry: shows dots, never the text.
    masked: bool,
    /// Markdown editing keys (lists continue on Enter, Ctrl+B/I/K,
    /// Ctrl+Shift+8, Tab / Shift+Tab nest list items).
    markdown: bool,
    /// Editor font family and size (`None`: the surface's).
    font: Option<gpui::SharedString>,
    font_px: Option<f32>,
    dirty: bool,
}

impl TextField {
    pub fn new(value: String, placeholder: impl Into<String>, live: bool, on_commit: OnCommit, cx: &mut Context<Self>) -> Self {
        Self {
            caret: value.len(),
            value,
            anchor: None,
            layout: ParagraphLayout::default(),
            dragging: false,
            // Keep the English key so the placeholder follows live
            // language switches (callers usually pass `t("…")`).
            placeholder: mandara_i18n::untranslate(&placeholder.into()),
            focus: cx.focus_handle(),
            on_commit,
            live,
            multiline: false,
            clear_on_commit: false,
            masked: false,
            markdown: false,
            font: None,
            font_px: None,
            dirty: false,
        }
    }

    /// Markdown editing keys (see [`crate::markdown`]'s helpers).
    pub fn markdown(mut self) -> Self {
        self.markdown = true;
        self
    }

    /// Sets the editor's font family and size.
    pub fn set_font(&mut self, family: Option<gpui::SharedString>, size: Option<f32>) {
        self.font = family;
        self.font_px = size;
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Shows `•` per character instead of the text (passwords).
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn clear_on_commit(mut self) -> Self {
        self.clear_on_commit = true;
        self
    }

    /// Replaces the value unless the user is editing.
    pub fn sync(&mut self, value: &str) {
        if !self.dirty && self.value != value {
            self.value = value.to_string();
            self.caret = self.value.len();
            self.anchor = None;
        }
    }

    /// Replaces the value unconditionally (e.g. switching the edited note).
    pub fn set(&mut self, value: &str) {
        self.value = value.to_string();
        self.caret = self.value.len();
        self.anchor = None;
        self.dirty = false;
    }

    /// The caret / selection over the current value (clamped: callers may
    /// replace `value` directly).
    fn edit(&mut self) -> Edit {
        let mut e = Edit { text: std::mem::take(&mut self.value), caret: self.caret, anchor: self.anchor };
        e.clamp();
        e
    }

    fn store(&mut self, e: Edit) {
        self.value = e.text;
        self.caret = e.caret;
        self.anchor = e.anchor;
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dirty = false;
        (self.on_commit)(self.value.clone(), window, cx);
    }

    fn edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dirty = true;
        if self.live {
            self.commit(window, cx);
        }
    }
}

/// Removes the last word (and any whitespace after it), like Ctrl+Backspace.
pub fn delete_word(s: &mut String) {
    let trimmed = s.trim_end().len();
    s.truncate(trimmed);
    let cut = s.char_indices().rev().find(|(_, c)| c.is_whitespace()).map(|(i, c)| i + c.len_utf8()).unwrap_or(0);
    s.truncate(cut);
}

/// A text with a caret and an optional selection anchor (byte offsets on
/// char boundaries). Pure editing logic, separate from GPUI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub caret: usize,
    pub anchor: Option<usize>,
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

impl Edit {
    pub fn new(text: &str, caret: usize) -> Self {
        let mut e = Edit { text: text.to_string(), caret, anchor: None };
        e.clamp();
        e
    }

    fn clamp(&mut self) {
        self.caret = floor_boundary(&self.text, self.caret);
        self.anchor = self.anchor.map(|a| floor_boundary(&self.text, a)).filter(|a| *a != self.caret);
    }

    /// The selected byte range, if any.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor.filter(|a| *a != self.caret).map(|a| a.min(self.caret)..a.max(self.caret))
    }

    pub fn selected(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(p, _)| p)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    /// Start of the word before `i` (skipping whitespace first).
    fn word_left(&self, i: usize) -> usize {
        let before = &self.text[..i];
        let trimmed = before.trim_end().len();
        before[..trimmed].char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(p, c)| p + c.len_utf8())
    }

    /// End of the word after `i` (skipping whitespace first).
    fn word_right(&self, i: usize) -> usize {
        let after = &self.text[i..];
        let lead = after.len() - after.trim_start().len();
        after[lead..].char_indices().find(|(_, c)| c.is_whitespace()).map_or(self.text.len(), |(p, _)| i + lead + p)
    }

    fn line_start(&self, i: usize) -> usize {
        self.text[..i].rfind('\n').map_or(0, |p| p + 1)
    }

    fn line_end(&self, i: usize) -> usize {
        self.text[i..].find('\n').map_or(self.text.len(), |p| i + p)
    }

    /// Moves the caret to `to`, extending the selection or dropping it.
    pub fn move_to(&mut self, to: usize, extend: bool) {
        let to = floor_boundary(&self.text, to);
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to;
        self.clamp();
    }

    /// One step backwards (a char or a word); without `extend` a selection
    /// collapses to its start.
    pub fn back(&mut self, word: bool, extend: bool) {
        if let (Some(r), false) = (self.selection(), extend) {
            return self.move_to(r.start, false);
        }
        let to = if word { self.word_left(self.caret) } else { self.prev(self.caret) };
        self.move_to(to, extend);
    }

    /// One step forwards (a char or a word).
    pub fn forward(&mut self, word: bool, extend: bool) {
        if let (Some(r), false) = (self.selection(), extend) {
            return self.move_to(r.end, false);
        }
        let to = if word { self.word_right(self.caret) } else { self.next(self.caret) };
        self.move_to(to, extend);
    }

    /// Home / End: the line's start or end (`all`: the whole text's).
    pub fn home(&mut self, all: bool, extend: bool) {
        let to = if all { 0 } else { self.line_start(self.caret) };
        self.move_to(to, extend);
    }

    pub fn end(&mut self, all: bool, extend: bool) {
        let to = if all { self.text.len() } else { self.line_end(self.caret) };
        self.move_to(to, extend);
    }

    /// ↑ / ↓: the same column (in chars) on the previous / next line.
    pub fn vertical(&mut self, down: bool, extend: bool) {
        let start = self.line_start(self.caret);
        let col = self.text[start..self.caret].chars().count();
        let target_line = if down {
            let end = self.line_end(self.caret);
            if end >= self.text.len() {
                return self.move_to(self.text.len(), extend);
            }
            end + 1
        } else {
            if start == 0 {
                return self.move_to(0, extend);
            }
            self.line_start(start - 1)
        };
        let line_end = self.line_end(target_line);
        let to = self.text[target_line..line_end].char_indices().nth(col).map_or(line_end, |(p, _)| target_line + p);
        self.move_to(to, extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
        self.clamp();
    }

    /// The word around the caret (double-click).
    pub fn select_word(&mut self) {
        let at = self.caret;
        let start = self.text[..at].char_indices().rev().take_while(|(_, c)| !c.is_whitespace()).last().map_or(at, |(p, _)| p);
        let end = self.text[at..].char_indices().find(|(_, c)| c.is_whitespace()).map_or(self.text.len(), |(p, _)| at + p);
        self.anchor = Some(start);
        self.caret = end;
        self.clamp();
    }

    /// Types `s` over the selection (or at the caret).
    pub fn insert(&mut self, s: &str) {
        let r = self.selection().unwrap_or(self.caret..self.caret);
        self.text.replace_range(r.clone(), s);
        self.caret = r.start + s.len();
        self.anchor = None;
    }

    /// Backspace: the selection, else the char (or word) before the caret.
    pub fn backspace(&mut self, word: bool) {
        if self.selection().is_none() {
            let from = if word { self.word_left(self.caret) } else { self.prev(self.caret) };
            self.anchor = Some(from);
        }
        self.insert("");
    }

    /// Delete: the selection, else the char (or word) after the caret.
    pub fn delete(&mut self, word: bool) {
        if self.selection().is_none() {
            let to = if word { self.word_right(self.caret) } else { self.next(self.caret) };
            self.anchor = Some(to);
        }
        self.insert("");
    }

    /// Runs a Markdown helper written for "the end of the text" at the
    /// caret: on the text before it, then puts the rest back.
    fn at_caret(&mut self, f: impl FnOnce(&mut String)) {
        let mut before = self.text[..self.caret].to_string();
        let after = self.text[self.caret..].to_string();
        f(&mut before);
        self.caret = before.len();
        self.text = before + &after;
        self.anchor = None;
    }

    /// Runs a line helper (bullet, indent) on the caret's whole line,
    /// keeping the caret on the same character.
    fn on_line<R>(&mut self, f: impl FnOnce(&mut String) -> R) -> R {
        let end = self.line_end(self.caret);
        let start = self.line_start(self.caret);
        let mut head = self.text[..end].to_string();
        let tail = self.text[end..].to_string();
        let before = head.len();
        let out = f(&mut head);
        let delta = head.len() as isize - before as isize;
        self.text = head + &tail;
        self.caret = floor_boundary(&self.text, (self.caret as isize + delta).max(start as isize) as usize);
        self.anchor = None;
        out
    }

    /// Ctrl+B / Ctrl+I: wraps the selection in `mark`, else opens or
    /// closes it at the caret.
    pub fn toggle_mark(&mut self, mark: &str) {
        match self.selection() {
            Some(r) => {
                let inner = self.text[r.clone()].to_string();
                self.text.replace_range(r.clone(), &format!("{mark}{inner}{mark}"));
                self.anchor = Some(r.start + mark.len());
                self.caret = r.start + mark.len() + inner.len();
            }
            None => self.at_caret(|t| crate::markdown::toggle_mark(t, mark)),
        }
    }

    /// Ctrl+K: makes the selection a link label (`[label](` + the address
    /// to type), else steps `[` → `](` → `)` at the caret.
    pub fn link_step(&mut self) {
        match self.selection() {
            Some(r) => {
                let label = self.text[r.clone()].to_string();
                let link = format!("[{label}](");
                self.text.replace_range(r.clone(), &link);
                self.caret = r.start + link.len();
                self.anchor = None;
            }
            None => self.at_caret(crate::markdown::link_step),
        }
    }

    /// Enter in Markdown: continues a list item at the caret.
    pub fn newline_markdown(&mut self) {
        if self.selection().is_some() {
            self.insert("");
        }
        let mut continued = false;
        self.at_caret(|t| continued = crate::markdown::continue_list(t));
        if !continued {
            self.insert("\n");
        }
    }

    pub fn toggle_bullet(&mut self) {
        self.on_line(crate::markdown::toggle_bullet);
    }

    /// Tab / Shift+Tab on a list item; `false` off one.
    pub fn indent(&mut self, outdent: bool) -> bool {
        self.on_line(|t| crate::markdown::indent(t, outdent))
    }
}

/// Byte offset in a masked field's bullets for byte `i` of the value, and
/// back.
fn masked_index(value: &str, i: usize) -> usize {
    value[..i].chars().count() * BULLET.len_utf8()
}

fn unmasked_index(value: &str, display: usize) -> usize {
    let n = display / BULLET.len_utf8();
    value.char_indices().nth(n).map_or(value.len(), |(p, _)| p)
}

impl TextField {
    /// Applies a key press; returns whether it was handled.
    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let (ctrl, shift) = (m.control, m.shift);
        let mut ed = self.edit();
        // ←/→ follow the screen: in right-to-left text ← moves forwards.
        let rtl = crate::bidi::text_is_rtl(&ed.text);
        let mut edited = false;
        let mut moved = true;
        if self.markdown {
            let handled = match k.key.as_str() {
                "enter" if !ctrl => {
                    ed.newline_markdown();
                    true
                }
                "b" if ctrl && !shift => {
                    ed.toggle_mark("**");
                    true
                }
                "i" if ctrl && !shift => {
                    ed.toggle_mark("_");
                    true
                }
                "k" if ctrl && !shift => {
                    ed.link_step();
                    true
                }
                "8" | "*" if ctrl && shift => {
                    ed.toggle_bullet();
                    true
                }
                "tab" if !ctrl => ed.indent(shift),
                _ => false,
            };
            if handled {
                self.store(ed);
                cx.stop_propagation();
                self.edited(window, cx);
                return true;
            }
        }
        match k.key.as_str() {
            "left" if rtl => ed.forward(ctrl, shift),
            "left" => ed.back(ctrl, shift),
            "right" if rtl => ed.back(ctrl, shift),
            "right" => ed.forward(ctrl, shift),
            "home" => ed.home(ctrl || !self.multiline, shift),
            "end" => ed.end(ctrl || !self.multiline, shift),
            "up" if self.multiline => ed.vertical(false, shift),
            "down" if self.multiline => ed.vertical(true, shift),
            "a" if ctrl => ed.select_all(),
            "c" | "x" if ctrl => {
                // Passwords never reach the clipboard.
                if let Some(sel) = ed.selected().filter(|_| !self.masked) {
                    cx.write_to_clipboard(ClipboardItem::new_string(sel.to_string()));
                    if k.key == "x" {
                        ed.insert("");
                        edited = true;
                    }
                }
            }
            "v" if ctrl => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let text = if self.multiline { text } else { text.replace('\n', " ") };
                    ed.insert(&text);
                    edited = true;
                }
            }
            "enter" if self.multiline && !ctrl => {
                ed.insert("\n");
                edited = true;
            }
            "enter" => {
                self.store(ed);
                self.commit(window, cx);
                if self.clear_on_commit {
                    self.set("");
                } else {
                    window.blur(cx);
                }
                return true;
            }
            "escape" => {
                self.store(ed);
                if self.dirty {
                    self.commit(window, cx);
                }
                window.blur(cx);
                return true;
            }
            "backspace" => {
                ed.backspace(ctrl);
                edited = true;
            }
            "delete" => {
                ed.delete(ctrl);
                edited = true;
            }
            _ => {
                moved = false;
                if let Some(ch) = k.key_char.as_ref().filter(|_| !ctrl && !m.alt && !m.platform) {
                    ed.insert(ch);
                    edited = true;
                }
            }
        }
        self.store(ed);
        if edited {
            self.edited(window, cx);
        }
        edited || moved
    }

    /// Places the caret at a click (Shift extends the selection; a double
    /// click selects the word).
    fn press(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let mut ed = self.edit();
        if let Some(ix) = self.layout.caret_index_at(e.position) {
            let ix = if self.masked { unmasked_index(&ed.text, ix) } else { ix };
            ed.move_to(ix, e.modifiers.shift);
            if e.click_count == 2 {
                ed.select_word();
            } else if e.click_count >= 3 {
                ed.select_all();
            }
        }
        self.store(ed);
        self.dragging = e.click_count == 1;
        cx.notify();
    }

    fn drag(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging || e.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        let Some(ix) = self.layout.caret_index_at(e.position) else { return };
        let mut ed = self.edit();
        let ix = if self.masked { unmasked_index(&ed.text, ix) } else { ix };
        if ix != ed.caret {
            ed.move_to(ix, true);
            self.store(ed);
            cx.notify();
        } else {
            self.store(ed);
        }
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let focused = self.focus.is_focused(window);
        let empty = self.value.is_empty();
        let multiline = self.multiline;
        let ed = self.edit();
        let (caret, selection) = (ed.caret, ed.selection());
        self.store(ed);
        let shown = if self.masked { BULLET.to_string().repeat(self.value.chars().count()) } else { self.value.clone() };
        let display = |i: usize| if self.masked { masked_index(&self.value, i) } else { i };
        let text_el = if empty && !focused {
            crate::bidi::paragraph(mandara_i18n::tr(&self.placeholder))
        } else {
            let layout = self.layout.clone();
            let caret_d = display(caret);
            let sel_d = selection.filter(|_| focused).map(|r| display(r.start)..display(r.end));
            let (caret_color, sel_color) = (palette.primary.hsla(), palette.primary.with_alpha(SELECTION_ALPHA).hsla());
            div()
                .relative()
                .child(crate::bidi::editable_paragraph(shown, &self.layout))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            if let Some(r) = sel_d.clone() {
                                for b in layout.selection_rects(r) {
                                    window.paint_quad(fill(b, sel_color));
                                }
                            }
                            if focused && let Some(b) = layout.caret_bounds(caret_d, px(CARET_W_PX)) {
                                window.paint_quad(fill(b, caret_color));
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element()
        };
        // Arabic hugs the right edge, Latin (paths, commands) the left.
        let rtl = crate::bidi::text_is_rtl(if empty && !focused { &self.placeholder } else { &self.value });
        div()
            .id("text-field")
            .track_focus(&self.focus)
            .min_w(px(160.0))
            .when(!multiline, |el| el.h(px(32.0)).flex().items_center())
            .when(multiline, |el| el.min_h(px(96.0)).py(px(8.0)).line_height(px(19.0)))
            .px(px(12.0))
            .rounded(px(crate::style::radius(10.0)))
            .bg(palette.surface_container_high.hsla())
            .border_1()
            .border_color(if focused { palette.primary.hsla() } else { palette.surface_container_high.hsla() })
            .cursor_text()
            .when_some(self.font.clone(), |el, f| el.font_family(f))
            .when_some(self.font_px, |el, s| el.text_size(px(s)))
            .text_color(if empty { palette.on_surface_variant.with_alpha(0.6).hsla() } else { palette.on_surface.hsla() })
            .when(!multiline, |el| el.whitespace_nowrap().overflow_hidden())
            .child(div().flex_1().min_w(px(0.0)).when(rtl, |el| el.text_right()).when(!rtl, |el| el.text_left()).child(text_el))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| this.press(e, window, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.drag(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if this.key(e, window, cx) {
                    cx.notify();
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dragging = false;
                if this.dirty {
                    this.commit(window, cx);
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_moves_by_char_word_and_line() {
        let mut e = Edit::new("hello brave world", 0);
        e.forward(false, false);
        assert_eq!(e.caret, 1);
        e.forward(true, false);
        assert_eq!(e.caret, 5);
        e.forward(true, false);
        assert_eq!(e.caret, 11);
        e.back(true, false);
        assert_eq!(e.caret, 6);
        e.end(true, false);
        assert_eq!(e.caret, 17);
        e.home(true, false);
        assert_eq!(e.caret, 0);
        // Multi-byte characters move whole.
        let mut a = Edit::new("مرحبا", 0);
        a.forward(false, false);
        assert_eq!(a.caret, "م".len());
        a.end(true, false);
        a.back(false, false);
        assert_eq!(a.caret, "مرحب".len());
    }

    #[test]
    fn lines_and_columns() {
        let mut e = Edit::new("one\ntwo three\nx", 6);
        e.home(false, false);
        assert_eq!(e.caret, 4);
        e.end(false, false);
        assert_eq!(e.caret, 13);
        e.vertical(true, false);
        assert_eq!(e.caret, 15, "clamped to the short last line");
        e.vertical(false, false);
        assert_eq!(e.caret, 5);
        e.vertical(false, false);
        assert_eq!(e.caret, 1);
    }

    #[test]
    fn selection_edits_and_copies() {
        let mut e = Edit::new("hello world", 0);
        e.forward(true, true);
        assert_eq!(e.selected(), Some("hello"));
        e.insert("bye");
        assert_eq!((e.text.as_str(), e.caret), ("bye world", 3));
        e.select_all();
        assert_eq!(e.selected(), Some("bye world"));
        e.backspace(false);
        assert_eq!(e.text, "");
        let mut d = Edit::new("abc def", 3);
        d.delete(false);
        assert_eq!(d.text, "abcdef");
        d.delete(true);
        assert_eq!(d.text, "abc");
        let mut b = Edit::new("abc def", 7);
        b.backspace(true);
        assert_eq!(b.text, "abc ");
        // A selection collapses to its edge on a plain arrow.
        let mut c = Edit::new("abcdef", 1);
        c.move_to(4, true);
        c.back(false, false);
        assert_eq!((c.caret, c.selection()), (1, None));
        let mut w = Edit::new("one two three", 5);
        w.select_word();
        assert_eq!(w.selected(), Some("two"));
    }

    #[test]
    fn markdown_keys_act_at_the_caret() {
        // Ctrl+B wraps a selection mid-text.
        let mut e = Edit::new("make this bold now", 5);
        e.move_to(9, true);
        e.toggle_mark("**");
        assert_eq!(e.text, "make **this** bold now");
        assert_eq!(e.selected(), Some("this"));
        // Ctrl+K turns the selection into a link label.
        let mut l = Edit::new("see docs here", 4);
        l.move_to(8, true);
        l.link_step();
        assert_eq!(l.text, "see [docs]( here");
        assert_eq!(l.caret, "see [docs](".len());
        l.insert("https://x.y");
        l.link_step();
        assert_eq!(l.text, "see [docs](https://x.y) here");
        // Enter inside the first item of a list continues it there.
        let mut n = Edit::new("- one\n- two", 5);
        n.newline_markdown();
        assert_eq!(n.text, "- one\n- \n- two");
        assert_eq!(n.caret, "- one\n- ".len());
        // The bullet toggles the caret's line, not the last one.
        let mut t = Edit::new("first\nsecond", 2);
        t.toggle_bullet();
        assert_eq!(t.text, "- first\nsecond");
        assert_eq!(t.caret, 4);
        let mut i = Edit::new("- a\n- b", 2);
        assert!(i.indent(false));
        assert_eq!(i.text, "  - a\n- b");
    }

    #[test]
    fn masked_indices_map_to_bullets() {
        assert_eq!(masked_index("pässword", "pä".len()), 2 * BULLET.len_utf8());
        assert_eq!(unmasked_index("pässword", 2 * BULLET.len_utf8()), "pä".len());
        assert_eq!(unmasked_index("ab", 99), 2);
    }

    #[test]
    fn deletes_last_word() {
        let mut s = String::from("buy fresh bread  ");
        delete_word(&mut s);
        assert_eq!(s, "buy fresh ");
        delete_word(&mut s);
        assert_eq!(s, "buy ");
        let mut one = String::from("word");
        delete_word(&mut one);
        assert_eq!(one, "");
    }
}
