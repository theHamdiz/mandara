//! Frame-accurate Wayland input-region masking.
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
    pending: SmallVec<[IRect; 8]>,
    /// Normalized region last sent to the compositor.
    committed: Option<Vec<IRect>>,
    /// Pixel bounds reused when applying a changed region to GPUI.
    bounds: Vec<Bounds<Pixels>>,
    commits: u64,
}

impl Inner {
    fn commit_pending(&mut self) -> bool {
        normalize(&mut self.pending);
        if self.committed.as_deref() == Some(self.pending.as_slice()) {
            self.pending.clear();
            return false;
        }
        let committed = self.committed.get_or_insert_default();
        committed.clear();
        committed.extend_from_slice(&self.pending);
        self.pending.clear();
        self.commits += 1;
        true
    }
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
            let x0 = f32::from(bounds.origin.x).floor() as i32;
            let y0 = f32::from(bounds.origin.y).floor() as i32;
            let x1 = (f32::from(bounds.origin.x) + f32::from(bounds.size.width)).ceil() as i32;
            let y1 = (f32::from(bounds.origin.y) + f32::from(bounds.size.height)).ceil() as i32;
            let rect = (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0));
            if rect.2 > 0 && rect.3 > 0 {
                self.inner.borrow_mut().pending.push(rect);
            }
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
        self.flush_with(|bounds| window.set_input_region(Some(bounds)));
    }

    fn flush_with(&self, apply: impl FnOnce(&[Bounds<Pixels>])) {
        let mut inner = self.inner.borrow_mut();
        if inner.commit_pending() {
            let Inner { committed, bounds, .. } = &mut *inner;
            bounds.clear();
            bounds.extend(committed.iter().flatten().map(|&(x, y, w, h)| Bounds {
                origin: point(px(x as f32), px(y as f32)),
                size: size(px(w as f32), px(h as f32)),
            }));
            apply(bounds);
        }
    }

    /// Normalizes and diffs the pending rects; returns the new region when it
    /// differs from the last committed one. Clears pending state.
    pub fn take_changed_region(&self) -> Option<Vec<IRect>> {
        let mut inner = self.inner.borrow_mut();
        inner.commit_pending().then(|| inner.committed.clone().unwrap_or_default())
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

/// Drops contained rects and sorts for stable diffing without a second copy.
fn normalize(rects: &mut SmallVec<[IRect; 8]>) {
    rects.sort_unstable();
    rects.dedup();
    let contains = |outer: &IRect, inner: &IRect| {
        outer.0 <= inner.0
            && outer.1 <= inner.1
            && i64::from(outer.0) + i64::from(outer.2) >= i64::from(inner.0) + i64::from(inner.2)
            && i64::from(outer.1) + i64::from(outer.3) >= i64::from(inner.1) + i64::from(inner.3)
    };
    for index in 0..rects.len() {
        let rect = rects[index];
        if rects.iter().any(|outer| outer != &rect && contains(outer, &rect)) {
            // A removed container is covered by another surviving container,
            // so it is safe to stop using it for later containment checks.
            rects[index].2 = 0;
        }
    }
    rects.retain(|rect| rect.2 > 0);
}

#[cfg(test)]
mod tests;
