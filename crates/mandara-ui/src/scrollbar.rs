//! A slim, auto-hiding overlay scroll bar for any vertical scroller tracked
//! by a [`ScrollHandle`] (Studio's pages, sidebar and lists, and the shell's
//! long panels).
//!
//! It draws over the scroller instead of taking layout space: place it as
//! an absolutely positioned sibling *after* the scroll container inside a
//! `relative` wrapper ([`with_scrollbar`] does that). It reads the handle's
//! viewport bounds and offsets, which the container records while it
//! prepaints, so it always matches the frame being drawn.
//!
//! * Invisible while the content fits.
//! * Appears while the content scrolls, or while the pointer is over the
//!   scroller's end edge or the thumb; then holds for a moment and fades.
//!   Hiding costs one timer wake-up plus a short fade: no frames are
//!   requested while idle. Reduce motion hides it without the fade.
//! * The thumb is a rounded pill in the palette's text colour at a low
//!   alpha, a touch wider and brighter under the pointer, with a faint
//!   track. Drag it to scroll; click the track to page toward the click.
//!   Wheel scrolling still reaches the scroller underneath.
//! * Sits on the end edge: right, or left in right-to-left languages.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Position,
    ScrollHandle, SharedString, Style, Styled as _, Window, div, fill, point, px, size,
};
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

/// Thumb thickness at rest: slim, so it never competes with content.
pub const THUMB_W_PX: f32 = 4.0;
/// Thumb thickness under the pointer or while dragged: easier to grab.
pub const THUMB_W_HOVER_PX: f32 = 7.0;
/// Gap between the thumb and the scroller's edge (and its ends).
pub const INSET_PX: f32 = 3.0;
/// Width of the edge strip that reveals the bar and takes its clicks.
pub const HIT_W_PX: f32 = 14.0;
/// The thumb never gets shorter than this, however long the content.
pub const THUMB_MIN_PX: f32 = 28.0;
/// Thumb alpha at rest, under the pointer and while dragged.
pub const ALPHA_REST: f32 = 0.28;
pub const ALPHA_HOVER: f32 = 0.45;
pub const ALPHA_DRAG: f32 = 0.6;
/// Track alpha while the pointer is over the bar.
pub const ALPHA_TRACK: f32 = 0.06;
/// Room to leave between the bar and a window's outer edge: compositors
/// take presses that close to a window's border for resizing (MangoWM
/// does within about 10 px), so a bar there couldn't be grabbed.
pub const WINDOW_EDGE_CLEARANCE_PX: f32 = 12.0;
/// How long the bar stays after the last scroll or hover.
pub const HOLD: Duration = Duration::from_millis(900);
/// How long it takes to fade out after that.
pub const FADE: Duration = Duration::from_millis(220);

/// Per-scroller state; keep one beside the [`ScrollHandle`] (cheap to
/// clone, shared with the event handlers).
#[derive(Default)]
pub struct ScrollbarState {
    /// The vertical offset seen last frame (scrolling shows the bar).
    last_offset: Cell<Option<Pixels>>,
    /// When the bar was last needed (scrolled, hovered, dragged).
    active_at: Cell<Option<Instant>>,
    hovered: Cell<bool>,
    /// While dragging: where the pointer grabbed the thumb, from its top.
    grab: Cell<Option<Pixels>>,
    /// The hide wake-up already scheduled, so frames don't pile up timers.
    wake_at: Cell<Option<Instant>>,
}

impl ScrollbarState {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    fn dragging(&self) -> bool {
        self.grab.get().is_some()
    }

    /// Marks the bar as needed now.
    fn touch(&self) {
        self.active_at.set(Some(Instant::now()));
    }
}

/// The bar's opacity `elapsed` after it was last needed: full while it
/// holds, then a linear fade to nothing.
pub fn opacity_after(elapsed: Duration, hold: Duration, fade: Duration) -> f32 {
    if elapsed <= hold {
        return 1.0;
    }
    if fade.is_zero() {
        return 0.0;
    }
    (1.0 - (elapsed - hold).as_secs_f32() / fade.as_secs_f32()).clamp(0.0, 1.0)
}

/// The thumb on a track `track` long, for a viewport `viewport` tall over
/// content that scrolls `max` further, scrolled `scrolled` (0..=max):
/// `(top from the track's start, length)`, or `None` when nothing scrolls.
pub fn thumb(track: f32, viewport: f32, max: f32, scrolled: f32, min_len: f32) -> Option<(f32, f32)> {
    if max <= 0.0 || track <= 0.0 || viewport <= 0.0 {
        return None;
    }
    let len = (track * viewport / (viewport + max)).max(min_len.min(track)).min(track);
    let travel = track - len;
    let top = travel * (scrolled / max).clamp(0.0, 1.0);
    Some((top, len))
}

/// The scroll amount (0..=max) that puts the thumb's top at `top` on a
/// track `track` long with a thumb `len` long.
pub fn scrolled_for(top: f32, track: f32, len: f32, max: f32) -> f32 {
    let travel = track - len;
    if travel <= 0.0 {
        return 0.0;
    }
    (top / travel).clamp(0.0, 1.0) * max
}

/// The overlay bar for `handle` (see the module docs for where it goes).
pub fn overlay_scrollbar(handle: &ScrollHandle, state: &Rc<ScrollbarState>, palette: &MandaraPalette) -> Scrollbar {
    Scrollbar { handle: handle.clone(), state: state.clone(), ink: palette.on_surface, end_inset: Pixels::ZERO }
}

/// `scroller` (a container with `.overflow_y_scroll().track_scroll(handle)`)
/// in a `relative` wrapper with its overlay bar on top. The wrapper takes
/// the place of the scroller, so give it the scroller's sizing
/// (`flex_1`, `min_h_0`, `h_full`, …) when that came from its parent.
pub fn with_scrollbar(scroller: impl IntoElement, handle: &ScrollHandle, state: &Rc<ScrollbarState>, palette: &MandaraPalette) -> gpui::Div {
    div().relative().child(scroller).child(overlay_scrollbar(handle, state, palette))
}

thread_local! {
    /// Scroll handles and bar states by key, for views that live once per
    /// process (Studio's pages, lists and modals).
    static KEYED: RefCell<HashMap<SharedString, (ScrollHandle, Rc<ScrollbarState>)>> = RefCell::new(HashMap::new());
}

/// The scroll handle and bar state kept under `key` (made on first use).
/// For a scroller that exists once per process, such as a Studio list; a
/// view drawn on several outputs keeps its own pair instead.
pub fn keyed(key: impl Into<SharedString>) -> (ScrollHandle, Rc<ScrollbarState>) {
    KEYED.with(|map| map.borrow_mut().entry(key.into()).or_insert_with(|| (ScrollHandle::new(), ScrollbarState::new())).clone())
}

/// [`keyed`]'s bar state alone, for a scroller that already has a handle.
pub fn keyed_state(key: impl Into<SharedString>) -> Rc<ScrollbarState> {
    keyed(key).1
}

/// The overlay bar element.
pub struct Scrollbar {
    handle: ScrollHandle,
    state: Rc<ScrollbarState>,
    ink: Color,
    /// Extra distance from the scroller's end edge.
    end_inset: Pixels,
}

impl Scrollbar {
    /// Moves the bar `inset` in from the scroller's end edge, e.g.
    /// [`WINDOW_EDGE_CLEARANCE_PX`] when that edge is a window's border.
    pub fn end_inset(mut self, inset: Pixels) -> Self {
        self.end_inset = inset;
        self
    }
}

impl IntoElement for Scrollbar {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// What a frame draws and listens on.
pub struct Prepared {
    hitbox: Hitbox,
    /// The track (thumb lane) in window coordinates.
    track: Bounds<Pixels>,
    /// Thumb top from the track's start, and its length.
    thumb: (f32, f32),
    max: f32,
    opacity: f32,
    rtl: bool,
}

impl Element for Scrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepared>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        // No layout space: the bar paints over the scroller's own bounds.
        let style = Style { position: Position::Absolute, ..Style::default() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Option<Prepared> {
        let viewport = self.handle.bounds();
        let max = f32::from(self.handle.max_offset().y);
        let offset = self.handle.offset().y;
        let state = &self.state;
        // Scrolling (by any means) shows the bar.
        if state.last_offset.get().is_some_and(|last| last != offset) {
            state.touch();
        }
        state.last_offset.set(Some(offset));
        let track_len = f32::from(viewport.size.height) - 2.0 * INSET_PX;
        let scrolled = -f32::from(offset);
        let thumb = thumb(track_len, f32::from(viewport.size.height), max, scrolled, THUMB_MIN_PX)?;
        let rtl = mandara_i18n::language().is_rtl();
        let hit_x = if rtl { viewport.origin.x + self.end_inset } else { viewport.origin.x + viewport.size.width - px(HIT_W_PX) - self.end_inset };
        let track = Bounds::new(point(hit_x, viewport.origin.y + px(INSET_PX)), size(px(HIT_W_PX), px(track_len)));
        let opacity = self.opacity(window, cx);
        // Visible, it takes clicks and shows an arrow; hidden, it only
        // watches for the pointer. Wheel events always reach the scroller.
        let behavior = if opacity > 0.0 { HitboxBehavior::BlockMouseExceptScroll } else { HitboxBehavior::Normal };
        let hitbox = window.insert_hitbox(Bounds::new(point(hit_x, viewport.origin.y), size(px(HIT_W_PX), viewport.size.height)), behavior);
        Some(Prepared { hitbox, track, thumb, max, opacity, rtl })
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), prepared: &mut Option<Prepared>, window: &mut Window, _: &mut App) {
        let Some(prep) = prepared.take() else {
            // Nothing scrolls: forget a drag that outlived the content.
            self.state.grab.set(None);
            return;
        };
        let state = self.state.clone();
        let wide = state.hovered.get() || state.dragging();
        if prep.opacity > 0.0 {
            window.set_cursor_style(CursorStyle::Arrow, &prep.hitbox);
            let w = if wide { THUMB_W_HOVER_PX } else { THUMB_W_PX };
            let x = if prep.rtl { prep.track.origin.x + px(INSET_PX) } else { prep.track.origin.x + prep.track.size.width - px(INSET_PX + w) };
            let radius = px(w / 2.0);
            if wide {
                let lane = Bounds::new(point(x, prep.track.origin.y), size(px(w), prep.track.size.height));
                window.paint_quad(fill(lane, self.ink.with_alpha(ALPHA_TRACK * prep.opacity).hsla()).corner_radii(radius));
            }
            let alpha = if state.dragging() {
                ALPHA_DRAG
            } else if state.hovered.get() {
                ALPHA_HOVER
            } else {
                ALPHA_REST
            };
            let (top, len) = prep.thumb;
            let rect = Bounds::new(point(x, prep.track.origin.y + px(top)), size(px(w), px(len)));
            window.paint_quad(fill(rect, self.ink.with_alpha(alpha * prep.opacity).hsla()).corner_radii(radius));
        }

        let (track, thumb, max, hitbox) = (prep.track, prep.thumb, prep.max, prep.hitbox);
        let visible = prep.opacity > 0.0;
        let handle = self.handle.clone();
        // Hover: over the edge strip (not hidden behind something in front).
        let hover_state = state.clone();
        let hover_hitbox = hitbox.clone();
        let drag_handle = handle.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let s = &hover_state;
            if let Some(grab) = s.grab.get() {
                let top = f32::from(e.position.y - track.origin.y - grab);
                let to = scrolled_for(top, f32::from(track.size.height), thumb.1, max);
                drag_handle.set_offset(point(drag_handle.offset().x, px(-to)));
                s.touch();
                window.refresh();
                return;
            }
            let over = hover_hitbox.is_hovered(window);
            if over != s.hovered.get() {
                s.hovered.set(over);
                s.touch();
                window.refresh();
            }
        });
        let down_state = state.clone();
        let down_hitbox = hitbox.clone();
        let down_handle = handle.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || e.button != MouseButton::Left || !(visible || down_state.hovered.get()) || !down_hitbox.is_hovered(window) {
                return;
            }
            let y = f32::from(e.position.y - track.origin.y);
            let (top, len) = thumb;
            if (top..=top + len).contains(&y) {
                down_state.grab.set(Some(px(y - top)));
            } else {
                // Page toward the click: centre the thumb on it, then grab
                // it there so a drag continues smoothly.
                let to = scrolled_for(y - len / 2.0, f32::from(track.size.height), len, max);
                down_handle.set_offset(point(down_handle.offset().x, px(-to)));
                down_state.grab.set(Some(px(len / 2.0)));
            }
            down_state.touch();
            cx.stop_propagation();
            window.refresh();
        });
        let up_state = state;
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || e.button != MouseButton::Left || up_state.grab.get().is_none() {
                return;
            }
            up_state.grab.set(None);
            up_state.touch();
            cx.stop_propagation();
            window.refresh();
        });
    }
}

impl Scrollbar {
    /// This frame's opacity; schedules the redraw that starts the fade, or
    /// asks for the next fade frame.
    fn opacity(&self, window: &mut Window, cx: &mut App) -> f32 {
        let state = &self.state;
        if state.hovered.get() || state.dragging() {
            state.touch();
            return 1.0;
        }
        let Some(at) = state.active_at.get() else { return 0.0 };
        let reduce = cx.reduce_motion() || cx.try_global::<crate::state::GlobalConfig>().is_some_and(|g| g.0.read(cx).get().motion.reduce_motion);
        let fade = if reduce { Duration::ZERO } else { FADE };
        let elapsed = at.elapsed();
        let opacity = opacity_after(elapsed, HOLD, fade);
        if elapsed < HOLD {
            // One wake-up when the hold ends.
            let wake = at + HOLD;
            if state.wake_at.get() != Some(wake) {
                state.wake_at.set(Some(wake));
                let wait = HOLD - elapsed;
                window
                    .spawn(cx, async move |cx| {
                        cx.background_executor().timer(wait).await;
                        let _ = cx.update(|window, _| window.refresh());
                    })
                    .detach();
            }
        } else if opacity > 0.0 {
            window.request_animation_frame();
        }
        opacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_scroll_means_no_thumb() {
        assert_eq!(thumb(400.0, 400.0, 0.0, 0.0, THUMB_MIN_PX), None);
        assert_eq!(thumb(0.0, 400.0, 100.0, 0.0, THUMB_MIN_PX), None);
    }

    #[test]
    fn thumb_length_is_the_visible_share_and_travels_the_track() {
        // Half the content visible: half the track.
        let (top, len) = thumb(400.0, 400.0, 400.0, 0.0, THUMB_MIN_PX).unwrap();
        assert_eq!((top, len), (0.0, 200.0));
        let (top, _) = thumb(400.0, 400.0, 400.0, 400.0, THUMB_MIN_PX).unwrap();
        assert_eq!(top, 200.0, "scrolled to the end: the thumb ends at the track's end");
        let (top, _) = thumb(400.0, 400.0, 400.0, 200.0, THUMB_MIN_PX).unwrap();
        assert_eq!(top, 100.0);
        // Very long content: the minimum length, still reaching the end.
        let (top, len) = thumb(400.0, 400.0, 1_000_000.0, 1_000_000.0, THUMB_MIN_PX).unwrap();
        assert_eq!(len, THUMB_MIN_PX);
        assert_eq!(top + len, 400.0);
        // Out-of-range offsets clamp.
        assert_eq!(thumb(400.0, 400.0, 400.0, -50.0, THUMB_MIN_PX).unwrap().0, 0.0);
    }

    #[test]
    fn dragging_maps_back_to_the_same_offset() {
        let (track, viewport, max) = (380.0, 400.0, 1200.0);
        for scrolled in [0.0, 150.0, 600.0, 1200.0] {
            let (top, len) = thumb(track, viewport, max, scrolled, THUMB_MIN_PX).unwrap();
            assert!((scrolled_for(top, track, len, max) - scrolled).abs() < 1e-3, "{scrolled}");
        }
        assert_eq!(scrolled_for(-40.0, track, 100.0, max), 0.0);
        assert_eq!(scrolled_for(10_000.0, track, 100.0, max), max);
        assert_eq!(scrolled_for(10.0, 100.0, 100.0, max), 0.0, "a thumb filling the track can't move");
    }

    #[test]
    fn the_bar_holds_then_fades() {
        assert_eq!(opacity_after(Duration::ZERO, HOLD, FADE), 1.0);
        assert_eq!(opacity_after(HOLD, HOLD, FADE), 1.0);
        let mid = opacity_after(HOLD + FADE / 2, HOLD, FADE);
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
        assert_eq!(opacity_after(HOLD + FADE, HOLD, FADE), 0.0);
        assert_eq!(opacity_after(HOLD + Duration::from_nanos(1), HOLD, Duration::ZERO), 0.0, "reduce motion: gone at once");
    }
}
