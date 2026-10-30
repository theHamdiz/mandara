//! Which GPUI display shows which output, and the output full-screen
//! overlays open on.
//!
//! GPUI and the xdg-output query ([`mandara_core::query_outputs`]) bind
//! outputs on separate Wayland connections, so their lists can come in
//! different orders (a compositor reload re-announces outputs). Pairing
//! them by position gave surfaces the other monitor's size; everything
//! pairs by output name here ([`mandara_core::pair_displays`]).

use gpui::{App, DisplayId};
use mandara_core::{DisplayKey, OutputGeometry};

/// Used when no output reports a size.
const FALLBACK_SIZE: (f32, f32) = (1920.0, 1080.0);

/// The outputs measured at startup, else a fresh query.
pub fn geometries(cx: &App) -> Vec<OutputGeometry> {
    cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.clone()).filter(|o| !o.is_empty()).unwrap_or_else(|| mandara_core::query_outputs().unwrap_or_default())
}

fn keys(cx: &App) -> Vec<DisplayKey> {
    cx.displays().iter().map(|d| DisplayKey::of(d.as_ref())).collect()
}

/// Every display with its output's geometry (by name).
pub fn paired(cx: &App) -> Vec<(DisplayId, Option<OutputGeometry>)> {
    mandara_core::pair_displays(&keys(cx), &geometries(cx))
}

/// [`paired`] against a given output list (a fresh measurement).
fn paired_with(outputs: &[OutputGeometry], cx: &App) -> Vec<(DisplayId, Option<OutputGeometry>)> {
    mandara_core::pair_displays(&keys(cx), outputs)
}

/// The GPUI display showing output `name`.
pub fn display_for(name: &str, cx: &App) -> Option<DisplayId> {
    paired(cx).into_iter().find(|(_, g)| g.as_ref().is_some_and(|g| g.name == name)).map(|(id, _)| id)
}

/// The monitor you're on: the compositor's focused monitor (an empty
/// workspace has no focused window), else the focused window's.
pub fn focused_monitor(cx: &App) -> Option<String> {
    let hub = cx.try_global::<crate::hub::GlobalHub>()?.0.read(cx);
    let active = hub.compositor.as_ref().and_then(|c| c.get_monitors().ok()).and_then(|list| list.into_iter().find(|m| m.active).map(|m| m.name));
    active.or_else(|| hub.focused.as_ref().map(|c| c.monitor.clone()))
}

/// Where a full-screen overlay opens: the focused monitor's display and its
/// logical size, measured now (outputs can change after startup). The
/// display is passed explicitly, so the size always matches the output the
/// compositor puts the surface on.
pub fn overlay_target(cx: &App) -> (Option<DisplayId>, (f32, f32)) {
    let outputs = mandara_core::query_outputs().ok().filter(|o| !o.is_empty()).unwrap_or_else(|| geometries(cx));
    let focused = focused_monitor(cx);
    let geo = focused.as_deref().and_then(|n| outputs.iter().find(|g| g.name == n)).or_else(|| outputs.first());
    let Some(geo) = geo else { return (None, FALLBACK_SIZE) };
    let display = paired_with(&outputs, cx).into_iter().find(|(_, g)| g.as_ref().is_some_and(|g| g.name == geo.name)).map(|(id, _)| id);
    (display, (geo.width as f32, geo.height as f32))
}
