//! Frame-accurate Wayland input-region masking ("the Caelestia law").
//!
//! Layer surfaces are allocated large and transparent so islands can morph
//! without resizing the surface. Every frame, interactive containers register
//! their *animated* bounds during prepaint via [`InputMaskRegistry::region`];
//! a [`InputMaskRegistry::commit`] element placed last in the tree diffs the
//! union against the previous frame and calls `wl_surface.set_input_region`
//! only when it changed, so clicks on transparent pixels fall through to the
//! windows underneath.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Bounds, IntoElement, ParentElement as _, Pixels, Styled, Window, canvas, div, point, px, size};
use smallvec::SmallVec;

/// Integer device-independent rectangle `(x, y, w, h)`; the compositor works in
/// surface-local integer coordinates.
type IRect = (i32, i32, i32, i32);

#[derive(Default)]
struct Inner {
    /// Rects registered during the frame being built.
    pending: SmallVec<[Bounds<Pixels>; 8]>,
    /// Normalized region last sent to the compositor.
    committed: Option<Vec<IRect>>,
    commits: u64,
}

/// Per-window registry of interactive bounds. Cheap to clone (shared handle).
#[derive(Clone, Default)]
pub struct InputMaskRegistry {
    inner: Rc<RefCell<Inner>>,
}

impl InputMaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers interactive bounds for the current frame.
    pub fn register(&self, bounds: Bounds<Pixels>) {
        if f32::from(bounds.size.width) > 0.0 && f32::from(bounds.size.height) > 0.0 {
            self.inner.borrow_mut().pending.push(bounds);
        }
    }

    /// An absolutely-positioned, invisible element that fills its parent and
    /// registers the parent's laid-out (animated) bounds during prepaint.
    /// Add it as a child of any interactive container.
    pub fn region(&self) -> impl IntoElement {
        let registry = self.clone();
        canvas(move |bounds, _, _| registry.register(bounds), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
    }

    /// Like [`Self::region`] but grows the registered rect by `pad` on every
    /// side (forgiving hit targets for thin pills).
    pub fn padded_region(&self, pad: Pixels) -> impl IntoElement {
        let registry = self.clone();
        canvas(
            move |bounds, _, _| {
                registry.register(Bounds {
                    origin: point(bounds.origin.x - pad, bounds.origin.y - pad),
                    size: size(bounds.size.width + pad * 2.0, bounds.size.height + pad * 2.0),
                })
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// An invisible element that applies the frame's region. Prepaint of the
    /// whole tree finishes before any paint runs, so placing this anywhere in
    /// the tree (conventionally last) sees every registration of the frame.
    pub fn commit(&self) -> impl IntoElement {
        let registry = self.clone();
        div().absolute().size_0().child(
            canvas(|_, _, _| {}, move |_, _, window, _| registry.flush(window)).size_0(),
        )
    }

    /// Diffs the pending rects against the last commit and updates the
    /// surface's input region if needed.
    pub fn flush(&self, window: &mut Window) {
        if let Some(rects) = self.take_changed_region() {
            let bounds: Vec<Bounds<Pixels>> = rects
                .iter()
                .map(|&(x, y, w, h)| Bounds {
                    origin: point(px(x as f32), px(y as f32)),
                    size: size(px(w as f32), px(h as f32)),
                })
                .collect();
            window.set_input_region(Some(&bounds));
        }
    }

    /// Normalizes and diffs the pending rects; returns the new region when it
    /// differs from the last committed one. Clears pending state.
    pub fn take_changed_region(&self) -> Option<Vec<IRect>> {
        let mut inner = self.inner.borrow_mut();
        let pending = std::mem::take(&mut inner.pending);
        let region = normalize(&pending);
        if inner.committed.as_ref() == Some(&region) {
            return None;
        }
        inner.committed = Some(region.clone());
        inner.commits += 1;
        Some(region)
    }

    /// Number of times the compositor region was updated (diagnostics).
    pub fn commit_count(&self) -> u64 {
        self.inner.borrow().commits
    }

    /// The region most recently sent to the compositor.
    pub fn committed_region(&self) -> Option<Vec<IRect>> {
        self.inner.borrow().committed.clone()
    }
}

/// Snaps rects outward to whole pixels, drops rects fully contained in
/// another, and sorts for stable diffing.
fn normalize(rects: &[Bounds<Pixels>]) -> Vec<IRect> {
    let mut out: Vec<IRect> = rects
        .iter()
        .map(|b| {
            let x0 = f32::from(b.origin.x).floor() as i32;
            let y0 = f32::from(b.origin.y).floor() as i32;
            let x1 = (f32::from(b.origin.x) + f32::from(b.size.width)).ceil() as i32;
            let y1 = (f32::from(b.origin.y) + f32::from(b.size.height)).ceil() as i32;
            (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
        })
        .filter(|r| r.2 > 0 && r.3 > 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    let contains = |outer: &IRect, inner: &IRect| {
        outer.0 <= inner.0
            && outer.1 <= inner.1
            && outer.0 + outer.2 >= inner.0 + inner.2
            && outer.1 + outer.3 >= inner.1 + inner.3
    };
    let snapshot = out.clone();
    out.retain(|r| !snapshot.iter().any(|o| o != r && contains(o, r)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    #[test]
    fn snaps_outward_and_drops_contained() {
        let r = normalize(&[b(10.4, 5.6, 100.2, 30.0), b(20.0, 10.0, 5.0, 5.0), b(300.0, 0.0, 50.0, 40.0)]);
        assert_eq!(r, vec![(10, 5, 101, 31), (300, 0, 50, 40)]);
    }

    #[test]
    fn only_commits_on_change() {
        let reg = InputMaskRegistry::new();
        reg.register(b(0.0, 0.0, 100.0, 40.0));
        assert!(reg.take_changed_region().is_some());
        reg.register(b(0.0, 0.0, 100.0, 40.0));
        assert!(reg.take_changed_region().is_none(), "identical frame must not re-commit");
        reg.register(b(0.0, 0.0, 120.0, 40.0));
        assert_eq!(reg.take_changed_region(), Some(vec![(0, 0, 120, 40)]));
        assert_eq!(reg.commit_count(), 2);
    }

    #[test]
    fn empty_frame_yields_empty_region() {
        let reg = InputMaskRegistry::new();
        reg.register(b(0.0, 0.0, 10.0, 10.0));
        reg.take_changed_region();
        // Nothing registered → empty region (all clicks pass through).
        assert_eq!(reg.take_changed_region(), Some(vec![]));
        // Zero-size rects are ignored.
        reg.register(b(5.0, 5.0, 0.0, 10.0));
        assert_eq!(reg.take_changed_region(), None);
    }
}
