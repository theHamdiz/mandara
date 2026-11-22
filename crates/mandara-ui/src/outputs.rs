//! Which GPUI display shows which output, and the output full-screen
//! overlays open on.
//!
//! GPUI and the xdg-output query ([`mandara_core::query_outputs`]) bind
//! outputs on separate Wayland connections, so their lists can come in
//! different orders (a compositor reload re-announces outputs). Pairing
//! them by position gave surfaces the other monitor's size; everything
//! pairs by output name here ([`mandara_core::pair_displays`]).

use gpui::{App, DisplayId};
use mandara_config::outputs::{DisplaysConfig, SurfaceKind, assignment};
use mandara_core::{DisplayKey, OutputGeometry};

/// Used when no output reports a size.
const FALLBACK_SIZE: (f32, f32) = (1920.0, 1080.0);

/// The measured outputs. [`crate::overlays::Outputs`] is measured off the UI
/// thread at startup and again on every display or scale change; asking the
/// compositor here would block the UI thread on a Wayland round trip each
/// time an overlay opens. Only the shell sets that global: Studio (and any
/// other process using these helpers) has none, so it asks the compositor
/// through [`mandara_core::query_outputs`], whose connection is kept between
/// calls.
pub fn geometries(cx: &App) -> Vec<OutputGeometry> {
    match cx.try_global::<crate::overlays::Outputs>() {
        Some(measured) => measured.0.clone(),
        None => mandara_core::query_outputs().unwrap_or_default(),
    }
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

/// The compositor's focused monitor as of the last navigation or focus
/// change (see [`watch_focused_monitor`]).
#[derive(Default)]
struct ActiveMonitor {
    name: Option<String>,
    /// The newest refresh asked for; older answers are dropped.
    generation: u64,
}
impl gpui::Global for ActiveMonitor {}

/// Keeps the focused monitor current: asks the compositor off the UI thread
/// whenever navigation or focus changes, so [`focused_monitor`] never has to
/// block on the compositor's IPC socket.
pub fn watch_focused_monitor(hub: &gpui::Entity<crate::hub::Hub>, cx: &mut App) {
    use crate::hub::Domains;
    cx.set_global(ActiveMonitor::default());
    let mut last = None;
    cx.observe(hub, move |hub, cx| {
        let token = hub.read(cx).revision(Domains::NAVIGATION.union(Domains::FOCUS));
        if last == Some(token) {
            return;
        }
        last = Some(token);
        let Some(backend) = hub.read(cx).compositor.clone() else { return };
        let generation = {
            let state = cx.global_mut::<ActiveMonitor>();
            state.generation = state.generation.wrapping_add(1);
            state.generation
        };
        cx.spawn(async move |cx| {
            let name = cx.background_executor().spawn(async move { backend.get_monitors().ok().and_then(|list| list.into_iter().find(|m| m.active).map(|m| m.name)) }).await;
            cx.update(|cx| {
                let state = cx.global_mut::<ActiveMonitor>();
                if state.generation == generation {
                    state.name = name;
                }
            });
        })
        .detach();
    })
    .detach();
}

/// The monitor you're on: the compositor's focused monitor (an empty
/// workspace has no focused window), else the focused window's.
pub fn focused_monitor(cx: &App) -> Option<String> {
    let hub = cx.try_global::<crate::hub::GlobalHub>()?.0.read(cx);
    cx.try_global::<ActiveMonitor>().and_then(|a| a.name.clone()).or_else(|| hub.focused.as_ref().map(|c| c.monitor.clone()))
}

/// The output a surface of `kind` opens on: the monitor you're on when
/// `kind` may show there, else the main display, else the first output it
/// may show on. A choice that allows no connected output allows them all.
pub fn pick_output<'a>(outputs: &'a [OutputGeometry], focused: Option<&str>, displays: &DisplaysConfig, kind: SurfaceKind) -> Option<&'a OutputGeometry> {
    let allowed: Vec<&OutputGeometry> = outputs.iter().filter(|o| assignment(displays, kind, &o.name)).collect();
    let allowed = if allowed.is_empty() { outputs.iter().collect() } else { allowed };
    let named = |name: Option<&str>| name.and_then(|n| allowed.iter().copied().find(|o| o.name.eq_ignore_ascii_case(n.trim())));
    named(focused).or_else(|| named(displays.primary.as_deref())).or_else(|| allowed.first().copied())
}

/// Which display a transient surface (toasts, OSD) of `kind` opens on.
/// `None` lets the compositor pick (the focused output) when every output
/// may show it; `focused` is asked only otherwise.
pub fn transient_display(kind: SurfaceKind, focused: impl FnOnce(&App) -> Option<String>, cx: &App) -> Option<DisplayId> {
    let outputs = &cx.try_global::<crate::overlays::Outputs>()?.0;
    let displays = &crate::state::config(cx).read(cx).get().displays;
    if outputs.iter().all(|o| assignment(displays, kind, &o.name)) {
        return None;
    }
    let name = pick_output(outputs, focused(cx).as_deref(), displays, kind)?.name.clone();
    display_for(&name, cx)
}

/// Where a full-screen overlay (launcher, window switcher, prompts) opens:
/// [`pick_output`] for [`SurfaceKind::Overlays`] over the current
/// measurement ([`geometries`]), with that output's display and logical size. The
/// display is passed explicitly, so the size always matches the output the
/// compositor puts the surface on.
pub fn overlay_target(cx: &App) -> (Option<DisplayId>, (f32, f32)) {
    let (display, size, _) = overlay_target_with_output(cx);
    (display, size)
}

/// The overlay target with its connector name retained for surface settings.
pub fn overlay_target_with_output(cx: &App) -> (Option<DisplayId>, (f32, f32), Option<String>) {
    let outputs = geometries(cx);
    let geo = overlay_output(&outputs, cx);
    let Some(geo) = geo else { return (None, FALLBACK_SIZE, None) };
    let display = paired_with(&outputs, cx).into_iter().find(|(_, g)| g.as_ref().is_some_and(|g| g.name == geo.name)).map(|(id, _)| id);
    (display, logical_size(geo), Some(geo.name.clone()))
}

/// An output's logical size (xdg-output: already divided by its scale), the
/// size a full-screen overlay on it takes.
pub fn logical_size(geo: &OutputGeometry) -> (f32, f32) {
    (geo.width as f32, geo.height as f32)
}

/// The output among `outputs` that full-screen overlays open on.
pub fn overlay_output<'a>(outputs: &'a [OutputGeometry], cx: &App) -> Option<&'a OutputGeometry> {
    pick_output(outputs, focused_monitor(cx).as_deref(), &crate::state::config(cx).read(cx).get().displays, SurfaceKind::Overlays)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::outputs::ScreenChoice;

    fn out(name: &str) -> OutputGeometry {
        OutputGeometry { name: name.into(), x: 0, y: 0, width: 2560, height: 1440 }
    }

    fn names(o: Option<&OutputGeometry>) -> Option<&str> {
        o.map(|o| o.name.as_str())
    }

    #[test]
    fn overlays_open_on_the_monitor_you_are_on() {
        let outputs = [out("DP-1"), out("DP-2")];
        let cfg = DisplaysConfig::default();
        assert_eq!(names(pick_output(&outputs, Some("DP-2"), &cfg, SurfaceKind::Overlays)), Some("DP-2"));
        assert_eq!(names(pick_output(&outputs, Some("DP-1"), &cfg, SurfaceKind::Overlays)), Some("DP-1"));
        assert_eq!(names(pick_output(&outputs, None, &cfg, SurfaceKind::Overlays)), Some("DP-1"), "focus unknown: the first output");
        let main = DisplaysConfig { primary: Some("DP-2".into()), ..Default::default() };
        assert_eq!(names(pick_output(&outputs, None, &main, SurfaceKind::Overlays)), Some("DP-2"), "focus unknown: the main display");
    }

    #[test]
    fn overlays_stay_on_the_displays_they_are_given() {
        // Issue #18: the main display is DP-2 and on-screen displays show
        // only there; the launcher and switcher follow even with DP-1 focused.
        let outputs = [out("DP-1"), out("DP-2")];
        let mut cfg = DisplaysConfig { primary: Some("DP-2".into()), ..Default::default() };
        cfg.surfaces.osd.show_on = ScreenChoice::Primary;
        assert_eq!(names(pick_output(&outputs, Some("DP-1"), &cfg, SurfaceKind::Overlays)), Some("DP-2"));
        assert_eq!(names(pick_output(&outputs, Some("DP-1"), &cfg, SurfaceKind::Osd)), Some("DP-2"));
        cfg.surfaces.get_mut(SurfaceKind::Overlays).show_on = ScreenChoice::All;
        assert_eq!(names(pick_output(&outputs, Some("DP-1"), &cfg, SurfaceKind::Overlays)), Some("DP-1"), "overlays set on their own");
        cfg.surfaces.osd = mandara_config::outputs::ScreenSet { show_on: ScreenChoice::Selected, outputs: vec!["HDMI-A-1".into()] };
        assert_eq!(names(pick_output(&outputs, Some("DP-1"), &cfg, SurfaceKind::Osd)), Some("DP-1"), "a disconnected choice allows every output");
    }
}
