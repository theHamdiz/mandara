//! Shell surfaces follow the lifetime and logical size of each output.

use std::time::Duration;

use gpui::{AnyWindowHandle, App, AppContext as _, DisplayId, WindowHandle, px, size};
use mandara_core::{DisplayKey, LayerSurfaceSpec, OutputGeometry};
use mandara_ui::ShellView;

/// Used only if neither output protocol nor GPUI supplies an initial size.
const FALLBACK_SIZE: (f32, f32) = (1920.0, 1080.0);
/// Give a compositor time to publish geometry after replacing an output.
const OUTPUT_RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq)]
struct Spec {
    display: Option<DisplayId>,
    monitor: String,
    size: (f32, f32),
}

struct SurfaceSet {
    spec: Spec,
    shell: AnyWindowHandle,
    wallpaper: Option<AnyWindowHandle>,
    strips: mandara_ui::shell::ExclusionStrips,
}

#[derive(Default)]
struct Surfaces {
    sets: Vec<SurfaceSet>,
    gpui_wallpapers: bool,
}

impl gpui::Global for Surfaces {}

/// The latest output measurement asked for; only its result applies. Kept
/// apart from [`Surfaces`], which [`reconcile`] takes while it opens windows.
#[derive(Default)]
struct Remeasure(u64);

impl gpui::Global for Remeasure {}

pub(super) fn init(outputs: Vec<OutputGeometry>, cx: &mut App) {
    cx.set_global(Surfaces { gpui_wallpapers: !mandara_ui::backdrop::software_enabled(), ..Default::default() });
    cx.set_global(Remeasure::default());
    reconcile(&outputs, true, cx);
    cx.observe_displays(|_, cx| remeasure(cx)).detach();
}

/// Measures the outputs again off the UI thread, then fits the surfaces to
/// them. A newer request supersedes one still measuring.
fn remeasure(cx: &mut App) {
    let generation = &mut cx.global_mut::<Remeasure>().0;
    *generation = generation.wrapping_add(1);
    let current = *generation;
    let displays: Vec<_> = cx.displays().iter().map(|display| DisplayKey::of(display.as_ref())).collect();
    cx.spawn(async move |cx| {
        let outputs = cx.background_executor().spawn(async move {
            query_after_change(&displays, mandara_core::query_outputs, || std::thread::sleep(OUTPUT_RETRY_DELAY))
        }).await;
        cx.update(|cx| {
            if cx.global::<Remeasure>().0 != current {
                return;
            }
            reconcile(&available_geometry(outputs), false, cx);
        });
    }).detach();
}

/// Follows the render scale the compositor gives a shell surface.
///
/// An output's logical size is its mode divided by its (fractional) scale,
/// but GPUI reports a display change only when the display's bounds change,
/// and it derives those from `wl_output`'s *integer* scale (the fractional
/// scale rounded up). Going from 1.5 to 1.75 keeps that at 2, so no display
/// change came, the full-output shell surface kept its old size, and the
/// compositor centred it on the resized output: past the top edge, with the
/// bar (or shifted down, on a smaller scale). The compositor does send each
/// surface its new preferred scale, so a scale change re-measures the
/// outputs. The first scale a new surface receives re-checks the size it was
/// opened with (a no-op unless the output changed meanwhile).
fn follow_scale(handle: WindowHandle<ShellView>, cx: &mut App) {
    let _ = handle.update(cx, |_, window, cx| {
        let mut scale = ScaleWatch::new(window.scale_factor());
        cx.observe_window_bounds(window, move |_, window, cx| {
            if scale.changed(window.scale_factor()) {
                remeasure(cx);
            }
        })
        .detach();
    });
}

/// The last render scale of a surface; resizes at the same scale don't count.
#[derive(Clone, Copy, Debug)]
struct ScaleWatch(f32);

impl ScaleWatch {
    fn new(scale: f32) -> Self {
        Self(scale)
    }

    /// Whether `scale` differs from the last one seen (and remembers it).
    fn changed(&mut self, scale: f32) -> bool {
        let changed = scale != self.0;
        self.0 = scale;
        changed
    }
}

fn available_geometry(outputs: anyhow::Result<Vec<OutputGeometry>>) -> Vec<OutputGeometry> {
    outputs.unwrap_or_else(|err| {
        tracing::warn!("output geometry after display change: {err:#}");
        Vec::new()
    })
}

fn geometry_score(displays: &[DisplayKey], outputs: &[OutputGeometry]) -> (usize, usize) {
    let named = displays.iter().filter_map(|display| display.uuid).filter(|uuid| outputs.iter().any(|output| output.has_size() && !output.name.is_empty() && mandara_core::output_uuid(&output.name) == *uuid)).count();
    (named, outputs.iter().filter(|output| output.has_size()).count())
}

fn query_after_change(displays: &[DisplayKey], mut query: impl FnMut() -> anyhow::Result<Vec<OutputGeometry>>, wait: impl FnOnce()) -> anyhow::Result<Vec<OutputGeometry>> {
    let first = query();
    let expected_named = displays.iter().filter(|display| display.uuid.is_some()).count();
    if first.as_ref().is_ok_and(|outputs| {
        let (named, sized) = geometry_score(displays, outputs);
        named == expected_named && sized >= displays.len()
    }) {
        return first;
    }
    wait();
    match (first, query()) {
        (Ok(first), Ok(retry)) if geometry_score(displays, &first) > geometry_score(displays, &retry) => Ok(first),
        (_, Ok(retry)) => Ok(retry),
        (first, Err(_)) if first.is_ok() => first,
        (_, Err(err)) => Err(err),
    }
}

/// The software renderer calls this if it cannot start. Future outputs
/// then receive the same fallback as the outputs connected now.
pub(super) fn enable_gpui_wallpapers(cx: &mut App) {
    let mut state = std::mem::take(cx.global_mut::<Surfaces>());
    state.gpui_wallpapers = true;
    for set in &mut state.sets {
        if set.wallpaper.is_none() {
            set.wallpaper = open_wallpaper(&set.spec, cx);
        }
    }
    cx.set_global(state);
}

fn display_spec(id: DisplayId, geo: Option<OutputGeometry>, fallback: (f32, f32), existing: &[Spec]) -> Spec {
    match geo.filter(OutputGeometry::has_size) {
        Some(geo) => Spec { display: Some(id), monitor: geo.name, size: (geo.width as f32, geo.height as f32) },
        None => existing.iter().find(|s| s.display == Some(id)).cloned()
            .unwrap_or_else(|| Spec { display: Some(id), monitor: String::new(), size: fallback }),
    }
}

fn same_display(spec: &Spec, display: &DisplayKey) -> bool {
    spec.display == Some(display.id) && display.uuid.is_none_or(|uuid| !spec.monitor.is_empty() && mandara_core::output_uuid(&spec.monitor) == uuid)
}

fn preserve_known_outputs(live: &[DisplayKey], queried: &[OutputGeometry], existing: &[Spec], previous: &[OutputGeometry]) -> Vec<OutputGeometry> {
    let mut outputs = queried.to_vec();
    for spec in existing.iter().filter(|spec| live.iter().any(|display| same_display(spec, display))) {
        if !spec.monitor.is_empty() && !outputs.iter().any(|g| g.name == spec.monitor)
            && let Some(geo) = previous.iter().find(|g| g.name == spec.monitor && g.has_size())
        {
            outputs.push(geo.clone());
        }
    }
    outputs
}

fn snapshots(outputs: &[OutputGeometry], existing: &[Spec], initial: bool, cx: &App) -> Vec<Spec> {
    let displays = cx.displays();
    let keys: Vec<_> = displays.iter().map(|d| DisplayKey::of(d.as_ref())).collect();
    let existing: Vec<_> = existing.iter().filter(|spec| keys.iter().any(|key| same_display(spec, key))).cloned().collect();
    let mut specs: Vec<_> = mandara_core::pair_displays(&keys, outputs).into_iter().map(|(id, geo)| {
        let fallback = displays.iter().find(|d| d.id() == id).map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))).unwrap_or(FALLBACK_SIZE);
        display_spec(id, geo, fallback, &existing)
    }).collect();
    if initial && specs.is_empty() {
        let geo = outputs.first();
        specs.push(Spec { display: None, monitor: geo.map(|g| g.name.clone()).unwrap_or_default(), size: geo.map(|g| (g.width as f32, g.height as f32)).unwrap_or(FALLBACK_SIZE) });
    }
    specs.retain(|s| s.size.0 > 0.0 && s.size.1 > 0.0);
    specs
}

fn reconcile(outputs: &[OutputGeometry], initial: bool, cx: &mut App) {
    let existing: Vec<_> = cx.global::<Surfaces>().sets.iter().map(|set| set.spec.clone()).collect();
    let live_displays: Vec<_> = cx.displays().iter().map(|display| DisplayKey::of(display.as_ref())).collect();
    let previous = cx.try_global::<mandara_ui::overlays::Outputs>().map(|outputs| outputs.0.as_slice()).unwrap_or_default();
    let outputs = preserve_known_outputs(&live_displays, outputs, &existing, previous);
    let desired = snapshots(&outputs, &existing, initial, cx);
    cx.set_global(mandara_ui::overlays::Outputs(outputs));
    let mut state = std::mem::take(cx.global_mut::<Surfaces>());
    let mut kept = Vec::new();
    for set in state.sets.drain(..) {
        if desired.contains(&set.spec) && live(set.shell, cx) {
            kept.push(set);
        } else {
            close(set.shell, cx);
            if let Some(wallpaper) = set.wallpaper {
                close(wallpaper, cx);
            }
            set.strips.close(cx);
        }
    }
    for spec in missing(&desired, &kept.iter().map(|s| s.spec.clone()).collect::<Vec<_>>()) {
        if let Some(set) = open(spec, state.gpui_wallpapers, cx) {
            kept.push(set);
        }
    }
    state.sets = kept;
    cx.set_global(state);
}

fn missing(desired: &[Spec], existing: &[Spec]) -> Vec<Spec> {
    desired.iter().filter(|s| !existing.contains(s)).cloned().collect()
}

fn open(spec: Spec, wallpaper: bool, cx: &mut App) -> Option<SurfaceSet> {
    let (w, h) = spec.size;
    let mut main = LayerSurfaceSpec::shell(size(px(w), px(h)), spec.display);
    if mandara_ui::state::config(cx).read(cx).get().bar.over_fullscreen {
        main.layer = gpui::layer_shell::Layer::Overlay;
    }
    let monitor = spec.monitor.clone();
    let shell = match cx.open_window(main.window_options(), move |window, cx| cx.new(|cx| ShellView::new(monitor, (w, h), window, cx))) {
        Ok(handle) => {
            follow_scale(handle, cx);
            handle.into()
        }
        Err(err) => {
            tracing::error!("shell surface on {}: {err:#}", spec.monitor);
            return None;
        }
    };
    let strips = mandara_ui::shell::open_exclusion_strips(spec.display, spec.size, spec.monitor.clone(), cx);
    let wallpaper = wallpaper.then(|| open_wallpaper(&spec, cx)).flatten();
    tracing::info!("mandara shell surface on {} ({w}×{h} logical)", spec.monitor);
    Some(SurfaceSet { spec, shell, wallpaper, strips })
}

fn open_wallpaper(spec: &Spec, cx: &mut App) -> Option<AnyWindowHandle> {
    let (w, h) = spec.size;
    let background = LayerSurfaceSpec::background(size(px(w), px(h)), spec.display);
    let monitor = spec.monitor.clone();
    cx.open_window(background.window_options(), move |_, cx| cx.new(|cx| mandara_ui::wallpaper::WallpaperView::on_monitor(monitor, cx)))
        .map(Into::into).map_err(|err| tracing::error!("wallpaper surface: {err:#}")).ok()
}

fn live(handle: AnyWindowHandle, cx: &App) -> bool {
    cx.windows().iter().any(|w| w.window_id() == handle.window_id())
}

fn close(handle: AnyWindowHandle, cx: &mut App) {
    if live(handle, cx) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn a_fractional_scale_change_remeasures_the_outputs() {
        // 1.5 -> 1.75 keeps wl_output's integer scale at 2, so GPUI sees no
        // display change; the surface's own scale must trigger the measure.
        let mut watch = ScaleWatch::new(1.5);
        assert!(!watch.changed(1.5), "a resize at the same scale measures nothing");
        assert!(watch.changed(1.75));
        assert!(!watch.changed(1.75), "one measurement per change");
        assert!(watch.changed(1.25));
        // A new surface starts at GPUI's 1.0 and gets its real scale after
        // opening: that once re-checks the size it was opened with.
        let mut fresh = ScaleWatch::new(1.0);
        assert!(fresh.changed(1.5));
        assert!(!fresh.changed(1.5));
    }

    fn spec(id: u64, monitor: &str, size: (f32, f32)) -> Spec {
        Spec { display: Some(DisplayId::new(id)), monitor: monitor.into(), size }
    }

    fn display_keys(count: usize) -> Vec<DisplayKey> {
        (0..count).map(|id| DisplayKey { id: DisplayId::new(id as u64), uuid: None, origin: (0, 0) }).collect()
    }

    #[test]
    fn same_count_with_stale_names_retries_and_prefers_live_names() {
        let displays = [DisplayKey { id: DisplayId::new(1), uuid: Some(mandara_core::output_uuid("DP-2")), origin: (0, 0) }];
        let geometry = |name: &str| OutputGeometry { name: name.into(), width: 1920, height: 1080, ..Default::default() };
        let mut calls = 0;
        let result = query_after_change(&displays, || {
            calls += 1;
            Ok(if calls == 1 { vec![geometry("DP-1"), geometry("DP-3")] } else { vec![geometry("DP-2")] })
        }, || {}).unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result[0].name, "DP-2");
        let failed = available_geometry(Err(anyhow::anyhow!("both queries failed")));
        let previous = [geometry("DP-2")];
        let retained = [spec(1, "DP-2", (1920.0, 1080.0))];
        assert_eq!(preserve_known_outputs(&[displays[0], DisplayKey { id: DisplayId::new(2), uuid: None, origin: (0, 0) }], &failed, &retained, &previous), previous);
        assert_eq!(display_spec(DisplayId::new(2), None, (1280.0, 720.0), &retained), spec(2, "", (1280.0, 720.0)));
    }

    #[test]
    fn a_transient_query_failure_recovers_the_returning_outputs() {
        let mut calls = 0;
        let waited = Cell::new(false);
        let outputs = query_after_change(&display_keys(1), || {
            calls += 1;
            if calls == 1 { anyhow::bail!("display is changing"); }
            assert!(waited.get());
            Ok(vec![OutputGeometry { name: "DP-3".into(), width: 2560, height: 1440, ..Default::default() }])
        }, || waited.set(true)).unwrap();
        assert_eq!(calls, 2);
        assert_eq!(outputs[0].name, "DP-3");
        assert_eq!(outputs[0].width, 2560);
    }

    #[test]
    fn failed_queries_do_not_become_empty_geometry_and_stop_after_one_retry() {
        let mut calls = 0;
        assert!(query_after_change(&display_keys(1), || {
            calls += 1;
            anyhow::bail!("unavailable")
        }, || {}).is_err());
        assert_eq!(calls, 2);
        assert!(query_after_change(&[], || Ok(Vec::new()), || panic!("complete query must not wait")).unwrap().is_empty());
    }

    #[test]
    fn incomplete_geometry_gets_one_retry_and_survives_a_retry_failure() {
        for retry_succeeds in [false, true] {
            let mut calls = 0;
            let geometry = || OutputGeometry { name: "DP-3".into(), width: 2560, height: 1440, ..Default::default() };
            let outputs = query_after_change(&display_keys(2), || {
                calls += 1;
                if calls == 1 { return Ok(vec![geometry()]); }
                if !retry_succeeds { anyhow::bail!("unavailable"); }
                Ok(vec![geometry(), OutputGeometry { name: "HDMI-A-1".into(), ..geometry() }])
            }, || {}).unwrap();
            assert_eq!(calls, 2);
            assert_eq!(outputs.len(), if retry_succeeds { 2 } else { 1 });
            assert_eq!(outputs[0].name, "DP-3");
        }
    }

    #[test]
    fn an_empty_retry_keeps_the_first_usable_geometry() {
        let mut calls = 0;
        let outputs = query_after_change(&display_keys(2), || {
            calls += 1;
            Ok(if calls == 1 {
                vec![OutputGeometry { name: "DP-3".into(), width: 2560, height: 1440, ..Default::default() }]
            } else {
                Vec::new()
            })
        }, || {}).unwrap();
        assert_eq!(calls, 2);
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "DP-3");
    }

    #[test]
    fn omitted_live_outputs_keep_their_geometry_but_disconnected_outputs_leave() {
        let hdmi = spec(0, "HDMI-A-1", (1920.0, 1080.0));
        let dp = spec(1, "DP-3", (2560.0, 1440.0));
        let previous = [
            OutputGeometry { name: hdmi.monitor.clone(), width: 1920, height: 1080, ..Default::default() },
            OutputGeometry { name: dp.monitor.clone(), x: 1920, width: 2560, height: 1440, ..Default::default() },
        ];
        let existing = [hdmi.clone(), dp.clone()];
        let live = [DisplayKey { id: hdmi.display.unwrap(), uuid: Some(mandara_core::output_uuid(&hdmi.monitor)), origin: (0, 0) }, DisplayKey { id: dp.display.unwrap(), uuid: Some(mandara_core::output_uuid(&dp.monitor)), origin: (1920, 0) }];
        assert_eq!(preserve_known_outputs(&live, &previous[..1], &existing, &previous), previous);
        assert_eq!(preserve_known_outputs(&live[..1], &previous[..1], &existing, &previous), previous[..1]);
        let stale = [OutputGeometry { name: "DP-9".into(), ..previous[1].clone() }, previous[0].clone()];
        assert!(preserve_known_outputs(&live, &stale, &existing, &previous).contains(&previous[1]));
        let replaced = DisplayKey { uuid: Some(mandara_core::output_uuid("DP-9")), ..live[1] };
        assert!(!same_display(&dp, &replaced), "a reused id must not retain another connector's geometry");
        assert!(preserve_known_outputs(&[replaced], &[], &existing, &previous).is_empty());
        // Fractional-scale GPUI bounds must not replace a known logical size.
        assert_eq!(display_spec(live[1].id, None, (1920.0, 1080.0), &existing), dp);
        let new = DisplayId::new(7);
        assert_eq!(display_spec(new, None, (1280.0, 720.0), &existing), spec(7, "", (1280.0, 720.0)));
        let resized = OutputGeometry { width: 1280, height: 720, ..previous[1].clone() };
        assert_eq!(display_spec(live[1].id, Some(resized), (1920.0, 1080.0), &existing), spec(1, "DP-3", (1280.0, 720.0)));
    }

    #[test]
    fn a_kvm_disconnect_and_reconnect_recreates_only_the_returning_output() {
        let hdmi = spec(0, "HDMI-A-1", (1920.0, 1080.0));
        let dp = spec(1, "DP-3", (2560.0, 1440.0));
        let original = [hdmi.clone(), dp.clone()];
        assert!(missing(&original, &original).is_empty());
        let disconnected = [hdmi.clone()];
        assert!(missing(&disconnected, &original).is_empty());
        let reconnected = [hdmi.clone(), spec(7, "DP-3", dp.size)];
        assert_eq!(missing(&reconnected, &disconnected), [reconnected[1].clone()]);
        // A compositor may reuse the old id; the removed set must still be
        // absent from the tracked sets before this reconciliation.
        assert_eq!(missing(&original, &disconnected), [dp]);
        let resized = spec(1, "DP-3", (1280.0, 720.0));
        assert_eq!(missing(&[hdmi, resized.clone()], &original), [resized]);
        assert!(missing(&[], &original).is_empty());
    }
}
