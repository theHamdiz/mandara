//! Right-to-left mirroring. GPUI's layout (taffy) has no `direction`, so
//! views say what is *reading order* and what is *physical*:
//!
//! * [`Bidi::row`] is a flex row in reading order: start → end, so it runs
//!   right to left in Arabic. Use it for anything a reader scans (label and
//!   control, icon and text, button groups, segmented options, cards).
//!   Inside it, `justify_start`/`justify_end` and `gap` mirror on their own.
//! * [`Bidi::ltr_row`] stays left to right in every language: clock times,
//!   key chords, code, media transport, charts, QR codes.
//! * [`Bidi::text_start`], [`Bidi::cross_start`] and the `*_start`/`*_end`
//!   insets and margins replace `text_left`, `items_start` (in a column),
//!   `left`, `ml_auto`, … wherever the side depends on reading direction.
//!
//! Windows set [`Bidi::text_start`] at their root, so every text block aligns
//! to the reading start unless it asks otherwise.

use gpui::{DefiniteLength, Length, Styled};

mod paragraph;
pub use paragraph::{LinkHandler, Paragraph, ParagraphLayout, editable_paragraph, linked_paragraph, paragraph, styled_paragraph};

/// Whether the UI language reads right to left.
pub fn rtl() -> bool {
    mandara_i18n::language().is_rtl()
}

/// `name` mirrored for the reading direction: forward/back chevrons point
/// the other way in right-to-left languages.
pub fn dir_icon(name: &'static str) -> &'static str {
    mirrored_icon(name, rtl())
}

fn mirrored_icon(name: &'static str, rtl: bool) -> &'static str {
    if !rtl {
        return name;
    }
    match name {
        "chevron-right" => "chevron-left",
        "chevron-left" => "chevron-right",
        other => other,
    }
}

/// Hebrew, Arabic, Syriac, Thaana, NKo, Samaritan, Mandaic and Arabic
/// Extended blocks, plus the Hebrew/Arabic presentation forms.
const RTL_RANGES: [std::ops::RangeInclusive<u32>; 3] = [0x0590..=0x08FF, 0xFB1D..=0xFDFF, 0xFE70..=0xFEFF];

/// Whether `text` reads right to left: its first letter is from a
/// right-to-left script (UAX #9 P2, simplified to letters). Text without
/// letters (numbers, paths of symbols) follows the UI language.
pub fn text_is_rtl(text: &str) -> bool {
    text.chars().find(|c| c.is_alphabetic()).map_or_else(rtl, is_rtl_char)
}

/// Whether `c` is from a right-to-left script.
pub fn is_rtl_char(c: char) -> bool {
    RTL_RANGES.iter().any(|r| r.contains(&(c as u32)))
}

/// Whether `text` has any right-to-left character (GPUI's own wrapping
/// breaks such paragraphs, see [`paragraph`]).
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(is_rtl_char)
}

/// Unicode Left-to-Right Isolate and Pop Directional Isolate (UAX #9).
const LRI: char = '\u{2066}';
const PDI: char = '\u{2069}';

/// `text` (a number with its unit, a path, a command) kept left to right
/// inside a right-to-left sentence, e.g. "26 px" or "mandara msg". A no-op
/// in left-to-right languages.
pub fn ltr_isolate(text: &str) -> String {
    isolate(text, rtl())
}

fn isolate(text: &str, rtl: bool) -> String {
    if rtl { format!("{LRI}{text}{PDI}") } else { text.to_string() }
}

/// Reading-direction aware layout for any GPUI element.
pub trait Bidi: Styled + Sized {
    /// A flex row in reading order (reversed in right-to-left languages).
    fn row(self) -> Self {
        let el = self.flex().flex_row();
        if rtl() { el.flex_row_reverse() } else { el }
    }

    /// A flex row that stays left to right in every language.
    fn ltr_row(self) -> Self {
        self.flex().flex_row()
    }

    /// Aligns text to the reading start (left, or right in RTL).
    fn text_start(self) -> Self {
        if rtl() { self.text_right() } else { self.text_left() }
    }

    /// Aligns text to the reading end (right, or left in RTL).
    fn text_end(self) -> Self {
        if rtl() { self.text_left() } else { self.text_right() }
    }

    /// In a column: children hug the reading start.
    fn cross_start(self) -> Self {
        if rtl() { self.items_end() } else { self.items_start() }
    }

    /// In a column: children hug the reading end.
    fn cross_end(self) -> Self {
        if rtl() { self.items_start() } else { self.items_end() }
    }

    /// Pushes this child (and those after it) to the end of a [`Bidi::row`].
    fn push_end(self) -> Self {
        if rtl() { self.mr_auto() } else { self.ml_auto() }
    }

    /// Absolute offset from the reading-start edge.
    fn inset_start(self, length: impl Clone + Into<Length>) -> Self {
        if rtl() { self.right(length) } else { self.left(length) }
    }

    /// Absolute offset from the reading-end edge.
    fn inset_end(self, length: impl Clone + Into<Length>) -> Self {
        if rtl() { self.left(length) } else { self.right(length) }
    }

    /// Padding on the reading-start side.
    fn pad_start(self, length: impl Clone + Into<DefiniteLength>) -> Self {
        if rtl() { self.pr(length) } else { self.pl(length) }
    }

    /// Padding on the reading-end side.
    fn pad_end(self, length: impl Clone + Into<DefiniteLength>) -> Self {
        if rtl() { self.pl(length) } else { self.pr(length) }
    }
}

impl<T: Styled + Sized> Bidi for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_and_isolates_follow_the_direction() {
        assert_eq!(mirrored_icon("chevron-right", true), "chevron-left");
        assert_eq!(mirrored_icon("chevron-left", true), "chevron-right");
        assert_eq!(mirrored_icon("check", true), "check");
        assert_eq!(mirrored_icon("chevron-right", false), "chevron-right");
        assert_eq!(isolate("26 px", true), "\u{2066}26 px\u{2069}");
        assert_eq!(isolate("26 px", false), "26 px");
    }

    #[test]
    fn text_direction_follows_the_first_letter() {
        assert!(text_is_rtl("مرحبا world"));
        assert!(!text_is_rtl("~/Downloads/وثائق"));
        assert!(text_is_rtl("«الاسم» (اختياري)"));
    }
}
