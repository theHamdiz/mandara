//! Folded reading text: a verse, a hadith or a reminder shown in a few
//! lines on a small surface, with a clear way to read all of it.
//!
//! [`fold`] sets each block (the Arabic, its meaning, …) clamped to its own
//! number of lines, ending after a whole word with an ellipsis, never
//! inside a word or between a letter and its marks
//! ([`crate::bidi::reading_paragraph`]). When a block was cut, *Read more*
//! (a link with a chevron, or the text itself) unfolds them in place: the
//! card grows smoothly to the full text, and the surface around it
//! scrolls once it is taller than the room. *Show less* folds it again.
//! Both work from the keyboard (Tab, then Enter or Space). The words are
//! never shortened: only how many lines show changes.
//!
//! The state lives with the surface while it is drawn (GPUI keyed state):
//! another item folds again, and a surface that stops drawing forgets it.
//! Nothing animates once the height has arrived.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Div, Entity, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_i18n::t;
use mandara_theme::Color;

use crate::bidi::{Bidi as _, Clip, reading_paragraph};
use crate::icons::icon;
use crate::kit::{text, tokens};

/// Blocks one fold holds at most (the Arabic, the chain, the meaning).
pub(crate) const MAX_BLOCKS: usize = 3;
/// The unfold's spring (critically damped: the card never grows past its
/// text or folds below its lines) and the longest it may run.
const UNFOLD_STIFFNESS: f32 = 260.0;
const UNFOLD_FOR: Duration = Duration::from_millis(650);
/// Closer than this, the height has arrived.
const REST_PX: f32 = 0.5;

/// One block of text in a fold: its styled container (font, size, line
/// height, alignment), the words, and how many lines it shows folded.
pub(crate) struct Block {
    pub style: Div,
    pub text: SharedString,
    pub lines: usize,
}

impl Block {
    pub(crate) fn new(style: Div, text: impl Into<SharedString>, lines: usize) -> Self {
        Self { style, text: text.into(), lines }
    }
}

/// A fold's state on one surface.
#[derive(Default)]
pub(crate) struct FoldState {
    /// The item it is folded or unfolded for.
    key: u64,
    expanded: bool,
    /// Per block: whether its folded layout left words out.
    clips: [Clip; MAX_BLOCKS],
    /// The folded and the full height, as last measured.
    short_h: Rc<Cell<f32>>,
    full_h: Rc<Cell<f32>>,
    /// Since when the height moves, and from which height.
    anim: Option<(Instant, f32)>,
}

impl FoldState {
    /// Another item starts folded.
    fn follow(&mut self, key: u64) {
        if self.key != key {
            *self = FoldState { key, ..FoldState::default() };
        }
    }

    /// Unfolds or folds, moving from the height shown now.
    fn toggle(&mut self, now: Instant) {
        let from = if self.expanded { self.full_h.get() } else { self.short_h.get() };
        self.expanded = !self.expanded;
        self.anim = (from > 0.0).then_some((now, from));
    }

    /// Whether any of the first `n` blocks was cut while folded.
    fn cut(&self, n: usize) -> bool {
        self.clips[..n.min(MAX_BLOCKS)].iter().any(|c| c.clipped() == Some(true))
    }

    /// The height to clip to now (`None`: no clip, the move is over).
    fn height(&mut self, now: Instant, reduce: bool) -> Option<f32> {
        let (at, from) = self.anim?;
        let to = if self.expanded { self.full_h.get() } else { self.short_h.get() };
        let h = moving_height(from, to, now.saturating_duration_since(at), reduce);
        if h.is_none() {
            self.anim = None;
        }
        h
    }
}

/// The height `elapsed` into a move from `from` to `to` (`to` 0: not yet
/// measured, so it holds); `None` once it has arrived, ran out of time,
/// or motion is reduced.
pub(crate) fn moving_height(from: f32, to: f32, elapsed: Duration, reduce: bool) -> Option<f32> {
    if reduce || elapsed >= UNFOLD_FOR {
        return None;
    }
    if to <= 0.0 {
        return Some(from);
    }
    let (x, _) = mandara_core::SpringConfig::critical(UNFOLD_STIFFNESS).solve(from - to, 0.0, elapsed.as_secs_f32());
    (x.abs() > REST_PX).then_some(to + x)
}

/// The item key's hash (a fold follows one item).
fn key_of(item: &str) -> u64 {
    let mut h = DefaultHasher::new();
    item.hash(&mut h);
    h.finish()
}

/// Flips `state` (from a click or a key) and redraws its surface.
fn flip(state: &Entity<FoldState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.toggle(Instant::now());
        cx.notify();
    });
}

/// `blocks` of `item` (any stable key: the reference), folded on surface
/// `id`, `gap` apart; the *Read more* / *Show less* row in `tint` carries
/// `trailing` (e.g. a way into the full reader) at its end.
pub(crate) fn fold(id: &'static str, item: &str, blocks: impl IntoIterator<Item = Block>, gap: f32, tint: Color, trailing: Option<AnyElement>, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state((id, 0usize), cx, |_, _| FoldState::default());
    let reduce = mandara_core::spring::motion_profile().reduce;
    let (expanded, clips, short_h, full_h, height) = state.update(cx, |s, _| {
        s.follow(key_of(item));
        let height = s.height(Instant::now(), reduce);
        (s.expanded, s.clips.clone(), s.short_h.clone(), s.full_h.clone(), height)
    });
    if height.is_some() {
        window.request_animation_frame();
    }
    // Unclamped while unfolded and while folding back.
    let open = expanded || height.is_some();
    let mut body = div().relative().flex_none().w_full().flex().flex_col().gap(px(gap));
    let mut n = 0;
    for (b, clip) in blocks.into_iter().zip(clips.iter()) {
        body = body.child(b.style.when(!open, |el| el.line_clamp(b.lines)).child(reading_paragraph(b.text, clip)));
        n += 1;
    }
    let cut = state.read(cx).cut(n);
    let measured = if open { full_h } else { short_h };
    let body = body.child(
        canvas(
            move |bounds, _, _| {
                measured.set(f32::from(bounds.size.height));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    );
    let can_open = cut && !expanded;
    let text_box = div()
        .id((id, 1usize))
        .w_full()
        .when_some(height, |el, h| el.h(px(h)).overflow_hidden())
        .when(can_open, |el| {
            let st = state.clone();
            el.cursor_pointer().on_click(move |_, _, cx| {
                flip(&st, cx);
                cx.stop_propagation();
            })
        })
        .child(body);
    let toggle = (cut || expanded).then(|| {
        let label: SharedString = if expanded { t("Show less").into() } else { t("Read more").into() };
        let (on_click, on_key) = (state.clone(), state.clone());
        div()
            .id((id, 2usize))
            .tab_index(0)
            .aria_label(label.clone())
            .row()
            .items_center()
            .gap(px(tokens::XS))
            .cursor_pointer()
            .text_size(px(text::LABEL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tint.hsla())
            .child(label)
            .child(icon(if expanded { "chevron-up" } else { "chevron-down" }, text::LABEL + 1.0, tint))
            .on_click(move |_, _, cx| {
                flip(&on_click, cx);
                cx.stop_propagation();
            })
            .on_key_down(move |event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.keystroke.modifiers.modified() {
                    flip(&on_key, cx);
                    cx.stop_propagation();
                }
            })
    });
    let row = (toggle.is_some() || trailing.is_some()).then(|| {
        div().w_full().row().items_center().gap(px(tokens::SM)).when_some(toggle, |el, t_| el.child(t_)).child(div().flex_1()).when_some(trailing, |el, t_| el.child(t_))
    });
    div().w_full().flex().flex_col().gap(px(tokens::XS)).child(text_box).when_some(row, |el, r| el.child(r)).into_any_element()
}

/// A small focusable text link with a reading-direction chevron (e.g.
/// "Open in the reader"), in `tint`.
pub(crate) fn link(id: &'static str, label: impl Into<SharedString>, tint: Color, on_activate: impl Fn(&mut App) + 'static) -> AnyElement {
    let label = label.into();
    let on_activate = Rc::new(on_activate);
    let on_key = on_activate.clone();
    div()
        .id(id)
        .tab_index(0)
        .aria_label(label.clone())
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .cursor_pointer()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label)
        .child(icon(crate::bidi::dir_icon("chevron-right"), text::LABEL + 1.0, tint))
        .on_click(move |_, _, cx| {
            on_activate(cx);
            cx.stop_propagation();
        })
        .on_key_down(move |event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.keystroke.modifiers.modified() {
                on_key(cx);
                cx.stop_propagation();
            }
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fold_follows_its_item_and_toggles() {
        let mut s = FoldState::default();
        s.follow(key_of("2:255"));
        assert!(!s.expanded && !s.cut(2));
        // Nothing measured yet: it opens without a move.
        s.toggle(Instant::now());
        assert!(s.expanded && s.anim.is_none());
        // Measured: folding back moves from the full height.
        s.full_h.set(240.0);
        s.short_h.set(80.0);
        let t0 = Instant::now();
        s.toggle(t0);
        assert!(!s.expanded);
        assert_eq!(s.anim.map(|(_, from)| from), Some(240.0));
        let h = s.height(t0 + Duration::from_millis(60), false).unwrap();
        assert!(h < 240.0 && h > 80.0, "{h}");
        // It arrives and stops asking for frames.
        assert_eq!(s.height(t0 + UNFOLD_FOR, false), None);
        assert!(s.anim.is_none());
        // Another item starts folded, its cut unknown.
        s.toggle(t0);
        s.clips[0].set(Some(true));
        assert!(s.expanded && s.cut(1));
        s.follow(key_of("2:256"));
        assert!(!s.expanded && !s.cut(MAX_BLOCKS) && s.full_h.get() == 0.0);
        // The same item keeps its state.
        s.toggle(t0);
        s.follow(key_of("2:256"));
        assert!(s.expanded);
    }

    #[test]
    fn the_height_moves_without_overshoot_and_respects_reduced_motion() {
        let mut last = 80.0;
        for ms in (0..600).step_by(20) {
            let Some(h) = moving_height(80.0, 300.0, Duration::from_millis(ms), false) else { break };
            assert!(h >= last - 0.01 && h <= 300.0, "{ms} ms: {h}");
            last = h;
        }
        assert!(last > 250.0);
        // Folding back never dips below the folded height.
        for ms in (0..600).step_by(20) {
            if let Some(h) = moving_height(300.0, 80.0, Duration::from_millis(ms), false) {
                assert!(h >= 80.0, "{ms} ms: {h}");
            }
        }
        assert_eq!(moving_height(80.0, 300.0, Duration::from_millis(10), true), None);
        // Not measured yet: hold the starting height.
        assert_eq!(moving_height(80.0, 0.0, Duration::from_millis(10), false), Some(80.0));
    }
}
