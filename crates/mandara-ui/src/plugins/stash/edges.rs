//! Stash on the shell surface: the edge catchers and the shake detector.
//!
//! **Edge catchers** are hairline strips in the shell surface's input region
//! (no surface of their own) along the edges you enabled in Studio. Wayland
//! has no "drags only" input region, so a strip takes clicks too; it is two
//! logical pixels thick (`CATCHER_THICKNESS_PX`), leaves a gap at each end
//! (wider at hot corners),
//! skips stretches shared with another output, is never drawn on an edge
//! another feature uses (`mandara_config::stash::catcher_states`) and is
//! gone while a fullscreen window shows on that output. A drag reaching a
//! strip springs a shelf open there, under the pointer, and the drop lands
//! in it; dropping on the strip itself lands there too.
//!
//! **Shake**: [`shake_hook`] follows the pointer events a surface already
//! receives (motion, and drags over it) with a cheap pure detector
//! ([`mandara_core::shake`]); nothing is registered while shaking is off.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, DragMoveEvent, ExternalData, ExternalPaths, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, Styled, Window, canvas, div, px};
use mandara_config::stash::{self as cfg_stash, Rect, ScreenEdge};
use mandara_core::InputMaskRegistry;
use mandara_core::shake::ShakeDetector;

use super::{Origin, running};

/// One spring per edge and drag: a second arrival this soon is the same drag.
const SPRING_COOLDOWN: Duration = Duration::from_millis(1500);

thread_local! {
    /// Output names handed to shake hooks.
    static NAMES: RefCell<Vec<gpui::SharedString>> = const { RefCell::new(Vec::new()) };
    /// Shake detectors, one per window that feeds them.
    static DETECTORS: RefCell<Vec<(gpui::WindowId, ShakeDetector)>> = const { RefCell::new(Vec::new()) };
    /// When each edge last sprang a shelf.
    static SPRUNG: RefCell<[Option<Instant>; 4]> = const { RefCell::new([None; 4]) };
    /// The last strips worked out, with what they depend on.
    static STRIPS: RefCell<Option<Strips>> = const { RefCell::new(None) };
}

/// Catcher strips with every input they were worked out from (the config
/// snapshot by identity, the output, its surface and the output layout).
struct Strips {
    /// Weak: keeps the snapshot's address from being reused while cached.
    config: std::sync::Weak<mandara_config::ShellConfig>,
    monitor: String,
    surface: (f32, f32),
    outputs: Vec<mandara_core::OutputGeometry>,
    rects: smallvec::SmallVec<[(ScreenEdge, Rect); 4]>,
}

/// The catcher strips of `monitor`'s shell surface (`surface`: its logical
/// size), each in the input region, springing a shelf on a drag.
pub fn catchers(monitor: &str, surface: (f32, f32), mask: &InputMaskRegistry, cx: &mut App) -> Vec<AnyElement> {
    if !running(cx) {
        return Vec::new();
    }
    let cfg = crate::state::config(cx).read(cx).snapshot();
    // Most renders end here: no catcher switched on (a borrowed read).
    if !cfg_stash::edge_flags(&cfg).iter().any(|on| *on) {
        return Vec::new();
    }
    // Never over a fullscreen game or video.
    if cx.try_global::<crate::hub::GlobalHub>().is_some_and(|hub| crate::bar::autohide::coverage(hub.0.read(cx), monitor).1) {
        return Vec::new();
    }
    let Some(outputs) = cx.try_global::<crate::overlays::Outputs>().map(|o| &o.0) else { return Vec::new() };
    // Strips are worked out once per config, output layout and surface size.
    let hit = STRIPS.with_borrow(|c| c.as_ref().filter(|c| std::ptr::eq(c.config.as_ptr(), std::sync::Arc::as_ptr(&cfg)) && c.monitor == monitor && c.surface == surface && c.outputs == *outputs).map(|c| c.rects.clone()));
    let rects = match hit {
        Some(rects) => rects,
        None => {
            let ctx = super::edge_context(cx);
            let active = cfg_stash::catcher_states(&cfg, ctx).map(|s| s.is_active());
            let layout: Vec<Rect> = outputs.iter().filter(|o| o.has_size()).map(|o| Rect::new(o.x as f32, o.y as f32, o.width as f32, o.height as f32)).collect();
            let here = outputs.iter().find(|o| o.name == monitor).map_or(Rect::new(0.0, 0.0, surface.0, surface.1), |o| Rect::new(o.x as f32, o.y as f32, surface.0, surface.1));
            let rects = cfg_stash::catcher_rects(here, &layout, active, |c| cfg_stash::corner_gap(&cfg, c, ctx));
            tracing::info!("stash: edge catchers on {monitor}: {:?}", rects.iter().map(|(e, r)| (e.id(), r.x, r.y, r.w, r.h)).collect::<Vec<_>>());
            let outputs = outputs.clone();
            STRIPS.with_borrow_mut(|c| *c = Some(Strips { config: std::sync::Arc::downgrade(&cfg), monitor: monitor.to_string(), surface, outputs, rects: rects.clone() }));
            rects
        }
    };
    rects
        .into_iter()
        .map(|(edge, r)| {
            let m1 = monitor.to_string();
            let m2 = monitor.to_string();
            let m3 = monitor.to_string();
            let m4 = monitor.to_string();
            div()
                .id(("stash-catcher", edge.index()))
                .absolute()
                .left(px(r.x))
                .top(px(r.y))
                .w(px(r.w))
                .h(px(r.h))
                .on_drag_move::<ExternalPaths>(move |e: &DragMoveEvent<ExternalPaths>, _, cx| arrive(&m1, edge, e.bounds.contains(&e.event.position), e.event.position, cx))
                .on_drag_move::<ExternalData>(move |e: &DragMoveEvent<ExternalData>, _, cx| arrive(&m2, edge, e.bounds.contains(&e.event.position), e.event.position, cx))
                .on_drop(move |paths: &ExternalPaths, window, cx| {
                    if let Some(id) = shelf_for_drop(&m3, edge, window, cx) {
                        super::drop_paths(id, paths, None, cx);
                    }
                })
                .on_drop(move |data: &ExternalData, window, cx| {
                    if let Some(id) = shelf_for_drop(&m4, edge, window, cx) {
                        super::drop_data(id, data, None, cx);
                    }
                })
                .child(mask.region())
                .into_any_element()
        })
        .collect()
}

/// A drag moved over a catcher: the first arrival springs a shelf there.
fn arrive(monitor: &str, edge: ScreenEdge, inside: bool, at: gpui::Point<gpui::Pixels>, cx: &mut App) {
    if !inside {
        return;
    }
    let now = Instant::now();
    let fresh = SPRUNG.with_borrow_mut(|s| {
        let slot = &mut s[edge.index()];
        if slot.is_some_and(|t| now.duration_since(t) < SPRING_COOLDOWN) {
            return false;
        }
        *slot = Some(now);
        true
    });
    if fresh {
        tracing::info!("stash: a drag reached the {} edge of {monitor}", edge.id());
        super::new_shelf(Some(monitor.to_string()), Some((f32::from(at.x), f32::from(at.y))), Origin::Edge(edge), cx);
    }
}

/// The shelf a drop on the strip itself goes to: the one it just sprang,
/// else a new one there.
fn shelf_for_drop(monitor: &str, edge: ScreenEdge, window: &Window, cx: &mut App) -> Option<u64> {
    let latest = super::entity_ref(cx).and_then(|e| {
        let s = e.read(cx);
        s.state.shelves.iter().rev().find(|sh| s.ui.get(&sh.id).is_some_and(|u| u.origin == Some(Origin::Edge(edge)))).map(|sh| sh.id)
    });
    latest.or_else(|| {
        let at = window.mouse_position();
        super::new_shelf(Some(monitor.to_string()), Some((f32::from(at.x), f32::from(at.y))), Origin::Edge(edge), cx)
    })
}

/// Follows pointer motion on this window for the shake gesture: `monitor`
/// is its output, `origin` where the window sits on it.
pub fn shake_hook(monitor: &str, origin: (f32, f32)) -> Option<impl IntoElement> {
    mandara_core::shake::current_tuning()?;
    // The output's name, shared (no copy per render).
    let monitor: gpui::SharedString = NAMES.with_borrow_mut(|names| match names.iter().find(|n| n.as_ref() == monitor) {
        Some(n) => n.clone(),
        None => {
            let n: gpui::SharedString = monitor.to_string().into();
            names.push(n.clone());
            n
        }
    });
    Some(
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let monitor = monitor.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        feed(&monitor, origin, e, window, cx);
                    }
                });
            },
        )
        .absolute()
        .size_0(),
    )
}

fn feed(monitor: &str, origin: (f32, f32), e: &MouseMoveEvent, window: &Window, cx: &mut App) {
    let Some(tuning) = mandara_core::shake::current_tuning() else { return };
    let id = window.window_handle().window_id();
    let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
    let fired = DETECTORS.with_borrow_mut(|d| {
        let at = match d.iter().position(|(w, _)| *w == id) {
            Some(at) => at,
            None => {
                // Detectors of closed windows go (a handful at most).
                d.retain(|(w, _)| cx.windows().iter().any(|h| h.window_id() == *w));
                d.push((id, ShakeDetector::new()));
                d.len() - 1
            }
        };
        d[at].1.feed(x, y, Instant::now(), &tuning)
    });
    if fired {
        super::shake_at(monitor, (origin.0 + x, origin.1 + y), cx);
    }
}

/// Forgets every shake detector (Stash stopped).
pub fn reset() {
    DETECTORS.with_borrow_mut(|d| {
        d.clear();
        d.shrink_to_fit();
    });
    STRIPS.with_borrow_mut(|s| *s = None);
    NAMES.with_borrow_mut(|n| {
        n.clear();
        n.shrink_to_fit();
    });
}
