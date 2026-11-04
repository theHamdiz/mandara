//! Shell surfaces follow the lifetime and logical size of each output.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, AppContext as _, DisplayId, px, size};
use mandara_core::{DisplayKey, LayerSurfaceSpec, OutputGeometry};
use mandara_ui::ShellView;

/// Used only if neither output protocol nor GPUI supplies an initial size.
const FALLBACK_SIZE: (f32, f32) = (1920.0, 1080.0);

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

pub(super) fn init(outputs: Vec<OutputGeometry>, cx: &mut App) {
    cx.set_global(Surfaces { gpui_wallpapers: !mandara_ui::backdrop::software_enabled(), ..Default::default() });
    reconcile(&outputs, true, cx);
    let generation = Rc::new(Cell::new(0u64));
    cx.observe_displays(move |_, cx| {
        let current = generation.get().wrapping_add(1);
        generation.set(current);
        let generation = generation.clone();
        cx.spawn(async move |cx| {
            let outputs = cx.background_executor().spawn(async { mandara_core::query_outputs() }).await;
            cx.update(|cx| {
                if generation.get() != current {
                    return;
                }
                match outputs {
                    Ok(outputs) => reconcile(&outputs, false, cx),
                    Err(err) => {
                        tracing::warn!("output geometry after display change: {err:#}");
                        reconcile(&[], false, cx);
                    }
                }
            });
        }).detach();
    }).detach();
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

fn snapshots(outputs: &[OutputGeometry], initial: bool, cx: &App) -> Vec<Spec> {
    let displays = cx.displays();
    let keys: Vec<_> = displays.iter().map(|d| DisplayKey::of(d.as_ref())).collect();
    let mut specs: Vec<_> = mandara_core::pair_displays(&keys, outputs).into_iter().map(|(id, geo)| {
        let fallback = displays.iter().find(|d| d.id() == id).map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))).unwrap_or(FALLBACK_SIZE);
        Spec {
            display: Some(id),
            monitor: geo.as_ref().map(|g| g.name.clone()).unwrap_or_default(),
            size: geo.filter(|g| g.width > 0 && g.height > 0).map(|g| (g.width as f32, g.height as f32)).unwrap_or(fallback),
        }
    }).collect();
    if initial && specs.is_empty() {
        let geo = outputs.first();
        specs.push(Spec { display: None, monitor: geo.map(|g| g.name.clone()).unwrap_or_default(), size: geo.map(|g| (g.width as f32, g.height as f32)).unwrap_or(FALLBACK_SIZE) });
    }
    specs.retain(|s| s.size.0 > 0.0 && s.size.1 > 0.0);
    specs
}

fn reconcile(outputs: &[OutputGeometry], initial: bool, cx: &mut App) {
    let desired = snapshots(outputs, initial, cx);
    cx.set_global(mandara_ui::overlays::Outputs(outputs.to_vec()));
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
        Ok(handle) => handle.into(),
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
    use super::*;

    fn spec(id: u64, monitor: &str, size: (f32, f32)) -> Spec {
        Spec { display: Some(DisplayId::new(id)), monitor: monitor.into(), size }
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
