//! Keeps a frozen capture and its interactive overlay on the same output.

use gpui::layer_shell::KeyboardInteractivity;
use gpui::{App, DisplayId, px, size};
use mandara_core::{DisplayKey, LayerSurfaceSpec, OutputGeometry, SurfaceRole};
use mandara_services::screencopy::Capture;

/// Focus at the time capture was requested. Resolving the active monitor
/// happens on the capture worker, alongside the Wayland round trips.
pub(crate) struct Focus {
    compositor: Option<std::sync::Arc<dyn mandara_compositor::CompositorBackend>>,
    window: Option<mandara_compositor::ClientInfo>,
}

impl Focus {
    /// Copies the hub's handles without making compositor requests.
    pub(crate) fn snapshot(cx: &App) -> Self {
        let hub = crate::hub::hub(cx).read(cx);
        Self {
            compositor: hub.compositor.clone(),
            window: hub.focused.clone(),
        }
    }

    /// Chooses the active output even on an empty workspace. Call this
    /// from a worker because the native driver may make an IPC request.
    pub(crate) fn select<'a>(&self, captures: &'a [Capture]) -> Option<&'a Capture> {
        let active = self
            .compositor
            .as_ref()
            .and_then(|c| c.get_monitors().ok())
            .and_then(|monitors| monitors.into_iter().find(|m| m.active).map(|m| m.name));
        let monitor = active
            .as_deref()
            .or_else(|| self.window.as_ref().map(|c| c.monitor.as_str()));
        select(captures, monitor, self.window_geometry())
    }

    /// Moves the chosen frame out of its batch without cloning its pixels.
    /// Reference identity also keeps unnamed outputs distinct.
    pub(crate) fn take(&self, mut captures: Vec<Capture>) -> Option<Capture> {
        let selected = self.select(&captures)?;
        let index = captures
            .iter()
            .position(|capture| std::ptr::eq(capture, selected))?;
        Some(captures.swap_remove(index))
    }

    /// Layout-space bounds for cropping the focused window.
    pub(crate) fn window_geometry(&self) -> Option<(i32, i32, i32, i32)> {
        self.window.as_ref().and_then(|w| w.geometry)
    }
}

/// The focused connector wins. Window geometry is a fallback for drivers
/// without connector names; layout coordinates can be negative.
pub(crate) fn select<'a>(
    captures: &'a [Capture],
    monitor: Option<&str>,
    window: Option<(i32, i32, i32, i32)>,
) -> Option<&'a Capture> {
    monitor
        .and_then(|name| captures.iter().find(|c| c.output == name))
        .or_else(|| {
            let (x, y, width, height) = window?;
            let intersection = |capture: &&Capture| {
                let (ox, oy, ow, oh) = capture.logical;
                let w = (i64::from(x) + i64::from(width)).min(i64::from(ox) + i64::from(ow))
                    - i64::from(x).max(i64::from(ox));
                let h = (i64::from(y) + i64::from(height)).min(i64::from(oy) + i64::from(oh))
                    - i64::from(y).max(i64::from(oy));
                w.max(0) * h.max(0)
            };
            captures
                .iter()
                .filter(|c| intersection(c) > 0)
                .max_by_key(intersection)
        })
        .or(captures.first())
}

/// Binds the overlay to the display that supplied its frozen pixels.
/// Pairing uses the capture's fresh geometry, avoiding stale hotplug data.
pub(crate) fn overlay(capture: &Capture, cx: &App) -> LayerSurfaceSpec {
    let displays: Vec<_> = cx
        .displays()
        .iter()
        .map(|d| DisplayKey::of(d.as_ref()))
        .collect();
    overlay_on(capture, &displays)
}

/// Builds a full-output layer using connector identity rather than list order.
fn overlay_on(capture: &Capture, displays: &[DisplayKey]) -> LayerSurfaceSpec {
    let (x, y, width, height) = capture.logical;
    let output = OutputGeometry {
        name: capture.output.clone(),
        x,
        y,
        width,
        height,
    };
    let display: Option<DisplayId> = mandara_core::pair_displays(displays, &[output])
        .into_iter()
        .find_map(|(id, output)| output.map(|_| id));
    LayerSurfaceSpec::overlay(
        SurfaceRole::Lock,
        size(px(width.max(1) as f32), px(height.max(1) as f32)),
        display,
    )
    .with_keyboard(KeyboardInteractivity::Exclusive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(name: &str, logical: (i32, i32, i32, i32), color: [u8; 4]) -> Capture {
        // A small physical frame suffices to check output selection and
        // local-space cropping without allocating full monitor images.
        Capture {
            output: name.into(),
            logical,
            width: 4,
            height: 4,
            rgba: color.repeat(16),
        }
    }

    #[test]
    fn left_focused_output_does_not_fall_back_to_first_enumerated_output() {
        let captures = [
            capture("HDMI-A-1", (0, 0, 1920, 1080), [255, 0, 0, 255]),
            capture("DP-3", (-2560, 0, 2560, 1440), [0, 0, 255, 255]),
        ];
        let focused = select(&captures, Some("DP-3"), None).unwrap();
        assert_eq!(focused.output, "DP-3");
        assert_eq!(focused.to_image().get_pixel(0, 0).0, [0, 0, 255, 255]);
        let reordered = [captures[1].clone(), captures[0].clone()];
        assert_eq!(
            select(&reordered, Some("DP-3"), None).unwrap().output,
            "DP-3"
        );

        // A numeric monitor id from an older driver must not redirect a
        // window at x=-2400 to the output at x=0.
        let window = (-2400, 120, 800, 600);
        let focused = select(&captures, Some("1"), Some(window)).unwrap();
        assert_eq!(focused.output, "DP-3");
        let crop = focused.crop_logical(
            (window.0 - focused.logical.0) as f32,
            window.1 as f32,
            window.2 as f32,
            window.3 as f32,
        );
        assert_eq!(crop.get_pixel(0, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn overlay_binds_to_the_frozen_output_instead_of_the_default_display() {
        let frame = capture("DP-3", (-2560, 0, 2560, 1440), [0, 0, 255, 255]);
        let displays = [
            DisplayKey {
                id: DisplayId::new(7),
                uuid: Some(mandara_core::output_uuid("HDMI-A-1")),
                origin: (0, 0),
            },
            DisplayKey {
                id: DisplayId::new(19),
                uuid: Some(mandara_core::output_uuid("DP-3")),
                origin: (-2560, 0),
            },
        ];
        let spec = overlay_on(&frame, &displays);
        assert_eq!(spec.display, Some(DisplayId::new(19)));
        assert_eq!(spec.size, size(px(2560.0), px(1440.0)));
        assert_eq!(spec.window_options().display_id, spec.display);
        assert_eq!(
            overlay_on(&frame, &[displays[1], displays[0]]).display,
            spec.display
        );
    }

    #[test]
    fn unknown_focus_preserves_the_first_output_fallback_and_empty_is_none() {
        let captures = [capture("HDMI-A-1", (0, 0, 1920, 1080), [0, 0, 0, 255])];
        assert_eq!(select(&captures, None, None).unwrap().output, "HDMI-A-1");
        assert_eq!(
            select(&captures, Some("disconnected"), Some((9000, 0, 300, 300)))
                .unwrap()
                .output,
            "HDMI-A-1"
        );
        assert!(select(&[], Some("DP-3"), None).is_none());
    }

    #[test]
    fn moving_the_selected_frame_keeps_unnamed_outputs_distinct() {
        let focus = Focus {
            compositor: None,
            window: Some(mandara_compositor::ClientInfo {
                monitor: "1".into(),
                geometry: Some((-2400, 120, 800, 600)),
                ..Default::default()
            }),
        };
        let captures = vec![
            capture("", (0, 0, 1920, 1080), [255, 0, 0, 255]),
            capture("", (-2560, 0, 2560, 1440), [0, 0, 255, 255]),
        ];
        let selected = focus.take(captures).unwrap();
        assert_eq!(selected.logical.0, -2560);
        assert_eq!(selected.to_image().get_pixel(0, 0).0, [0, 0, 255, 255]);
    }
}
