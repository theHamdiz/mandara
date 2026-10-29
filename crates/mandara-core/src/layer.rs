//! `zwlr_layer_shell_v1` surface specifications for the `mandara-*` namespaces.
//!
//! # GPU cost of a surface (GPUI `ec86047`, wgpu backend)
//!
//! Measured with `cargo run -p mandara-core --example vram_probe`. Upstream,
//! every GPUI window cost ~15–25 MB of fixed VRAM plus ~36 bytes per *device*
//! pixel: 4 swapchain images × 4 B, plus a full-size path intermediate (4 B)
//! and 4× MSAA target (16 B) allocated on the first frame even without paths
//! — ~290 MB for a full-output surface at 2560×1440 @1.5.
//!
//! Mandara vendors `gpui_wgpu` (`vendor/gpui_wgpu/MANDARA-PATCHES.md`): path
//! textures are allocated only for frames with paths, at the paths' bounding
//! box, and released when idle; atlases and instance buffers start small. A
//! full-output surface now costs ~131 MB, almost all of it the 4 swapchain
//! images Mesa's Wayland WSI requires (present mode / frame latency cannot
//! lower that). Mandara still keeps full-output surfaces to the minimum
//! (shell + wallpaper), makes invisible strips 1 px thick and creates them
//! on demand, and closes overlays when hidden.

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    Bounds, DisplayId, Pixels, Size, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px,
};

/// Wayland `app_id` shared by every shell surface.
pub const SHELL_APP_ID: &str = "org.mandara.shell";
/// Wayland `app_id` of the Studio `xdg_toplevel`.
pub const STUDIO_APP_ID: &str = "org.mandara.studio";

/// Thickness of invisible exclusive-zone strips, in logical pixels (never 0:
/// a zero dimension trips GPUI's viewport, see `mandara-architecture`).
pub const EXCLUSION_STRIP_THICKNESS_PX: f32 = 1.0;

/// Which shell surface a layer belongs to; determines its namespace so
/// compositor rules (`layer_rule=layer_name:mandara-bar,...`) can target it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceRole {
    Bar,
    Wallpaper,
    Osd,
    Notifications,
    Launcher,
    Lock,
    /// Invisible strip that only reserves the bar's exclusive zone.
    Exclusion,
    /// The app dock.
    Dock,
    /// The Alt-Tab window switcher.
    Switcher,
    /// The app installer's confirmation card and progress pill.
    Installer,
    /// An interactive desktop card or GPU visualizer (Bottom layer, sized
    /// to the card).
    Desktop,
    /// The desktop's right-click menu (Overlay layer, only while open).
    DesktopMenu,
    /// Desktop edit mode: widget outlines and handles (Top layer, one per
    /// output, only while editing).
    DesktopEdit,
}

impl SurfaceRole {
    pub fn namespace(self) -> &'static str {
        match self {
            SurfaceRole::Bar => "mandara-bar",
            SurfaceRole::Wallpaper => "mandara-wallpaper",
            SurfaceRole::Osd => "mandara-osd",
            SurfaceRole::Notifications => "mandara-notifications",
            SurfaceRole::Launcher => "mandara-launcher",
            SurfaceRole::Lock => "mandara-lock",
            SurfaceRole::Exclusion => "mandara-exclusion",
            SurfaceRole::Dock => "mandara-dock",
            SurfaceRole::Switcher => "mandara-switcher",
            SurfaceRole::Installer => "mandara-installer",
            SurfaceRole::Desktop => "mandara-desktop",
            SurfaceRole::DesktopMenu => "mandara-desktop-menu",
            SurfaceRole::DesktopEdit => "mandara-desktop-edit",
        }
    }

    pub const ALL: [SurfaceRole; 13] = [
        SurfaceRole::Exclusion,
        SurfaceRole::Bar,
        SurfaceRole::Wallpaper,
        SurfaceRole::Osd,
        SurfaceRole::Notifications,
        SurfaceRole::Launcher,
        SurfaceRole::Lock,
        SurfaceRole::Dock,
        SurfaceRole::Switcher,
        SurfaceRole::Installer,
        SurfaceRole::Desktop,
        SurfaceRole::DesktopMenu,
        SurfaceRole::DesktopEdit,
    ];
}

/// Declarative description of a layer surface.
#[derive(Clone, Debug)]
pub struct LayerSurfaceSpec {
    pub role: SurfaceRole,
    pub layer: Layer,
    pub anchor: Anchor,
    /// Surface size; a zero dimension with both opposite anchors stretches.
    pub size: Size<Pixels>,
    /// CSS order: top, right, bottom, left.
    pub margin: (Pixels, Pixels, Pixels, Pixels),
    pub exclusive_zone: Option<Pixels>,
    pub keyboard: KeyboardInteractivity,
    pub display: Option<DisplayId>,
}

impl LayerSurfaceSpec {
    /// The bounding surface for the bar plus its expandable island area. It is
    /// anchored to the top edge, spans the output, and reserves only the bar
    /// strip as exclusive zone; everything below is transparent and masked
    /// out of input by the [`crate::InputMaskRegistry`].
    ///
    /// `output_width` must be the *logical* width (see
    /// [`crate::query_outputs`]); GPUI's display bounds are wrong under
    /// fractional scaling, and a zero width (stretch) trips GPUI's viewport.
    pub fn bar(output_width: Pixels, surface_height: Pixels, exclusive: Pixels, display: Option<DisplayId>) -> Self {
        Self {
            role: SurfaceRole::Bar,
            layer: Layer::Top,
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            size: Size { width: output_width, height: surface_height },
            margin: (px(0.0), px(0.0), px(0.0), px(0.0)),
            exclusive_zone: Some(exclusive),
            keyboard: KeyboardInteractivity::OnDemand,
            display,
        }
    }

    /// The full-output shell surface hosting the bar, plugin panels and the
    /// sidebar. It ignores exclusive zones (`-1`) and covers the whole output;
    /// a separate [`SurfaceRole::Exclusion`] strip reserves the bar's space,
    /// and the input region masks everything that is not visible UI.
    pub fn shell(output: Size<Pixels>, display: Option<DisplayId>) -> Self {
        Self {
            role: SurfaceRole::Bar,
            layer: Layer::Top,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: output,
            margin: (px(0.0), px(0.0), px(0.0), px(0.0)),
            exclusive_zone: Some(px(-1.0)),
            keyboard: KeyboardInteractivity::OnDemand,
            display,
        }
    }

    /// A full-output background surface (wallpaper depth canvas).
    pub fn background(output: Size<Pixels>, display: Option<DisplayId>) -> Self {
        Self {
            role: SurfaceRole::Wallpaper,
            layer: Layer::Background,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: output,
            margin: (px(0.0), px(0.0), px(0.0), px(0.0)),
            exclusive_zone: Some(px(-1.0)),
            keyboard: KeyboardInteractivity::None,
            display,
        }
    }

    /// A full-output overlay used for OSDs and toasts (fully input-masked
    /// except for the visible cards).
    pub fn overlay(role: SurfaceRole, output: Size<Pixels>, display: Option<DisplayId>) -> Self {
        Self {
            role,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: output,
            margin: (px(0.0), px(0.0), px(0.0), px(0.0)),
            exclusive_zone: Some(px(-1.0)),
            keyboard: KeyboardInteractivity::None,
            display,
        }
    }

    /// An invisible strip that only reserves `zone` logical pixels along the
    /// edge(s) in `anchor` (e.g. `TOP | LEFT | RIGHT` for a top bar,
    /// `TOP | BOTTOM | LEFT` for a left sidebar).
    ///
    /// The exclusive zone is independent of the surface size (wlr-layer-shell
    /// measures it from the anchored edge), so the strip is only
    /// [`EXCLUSION_STRIP_THICKNESS_PX`] thick: every GPUI window costs a
    /// swapchain plus full-size path/MSAA textures, and a full-height
    /// sidebar-wide strip used to hold ~40 MB of VRAM to draw nothing.
    /// `length` is the extent along the edge (the output width or height).
    pub fn exclusion_strip(anchor: Anchor, length: Pixels, zone: Pixels, display: Option<DisplayId>) -> Self {
        let horizontal = anchor.contains(Anchor::LEFT) && anchor.contains(Anchor::RIGHT);
        let thickness = px(EXCLUSION_STRIP_THICKNESS_PX);
        let size = if horizontal { Size { width: length, height: thickness } } else { Size { width: thickness, height: length } };
        Self {
            role: SurfaceRole::Exclusion,
            layer: Layer::Top,
            anchor,
            size,
            margin: (px(0.0), px(0.0), px(0.0), px(0.0)),
            exclusive_zone: Some(zone),
            keyboard: KeyboardInteractivity::None,
            display,
        }
    }

    pub fn with_keyboard(mut self, keyboard: KeyboardInteractivity) -> Self {
        self.keyboard = keyboard;
        self
    }

    /// GPUI window options for this surface. Inactive-frame throttling is
    /// disabled: layer surfaces never become "active", and the shell must
    /// animate at the output's full refresh rate.
    pub fn window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: None,
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: self.size })),
            focus: false,
            show: true,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: self.role.namespace().to_string(),
                layer: self.layer,
                anchor: self.anchor,
                exclusive_zone: self.exclusive_zone,
                exclusive_edge: None,
                margin: Some(self.margin),
                keyboard_interactivity: self.keyboard,
            }),
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: self.display,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some(SHELL_APP_ID.to_string()),
            inactive_frame_interval: None,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_are_prefixed() {
        for role in SurfaceRole::ALL {
            assert!(role.namespace().starts_with("mandara-"));
        }
    }

    #[test]
    fn exclusion_strips_are_thin() {
        let top = LayerSurfaceSpec::exclusion_strip(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, px(2560.0), px(36.0), None);
        assert_eq!(top.size, Size { width: px(2560.0), height: px(EXCLUSION_STRIP_THICKNESS_PX) });
        let side = LayerSurfaceSpec::exclusion_strip(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT, px(1440.0), px(360.0), None);
        assert_eq!(side.size, Size { width: px(EXCLUSION_STRIP_THICKNESS_PX), height: px(1440.0) });
        assert_eq!(side.exclusive_zone, Some(px(360.0)));
        assert_eq!(side.role, SurfaceRole::Exclusion);
    }

    #[test]
    fn bar_options_disable_throttling() {
        let spec = LayerSurfaceSpec::bar(px(2560.0), px(600.0), px(46.0), None);
        let opts = spec.window_options();
        assert!(opts.inactive_frame_interval.is_none());
        match opts.kind {
            WindowKind::LayerShell(o) => {
                assert_eq!(o.namespace, "mandara-bar");
                assert_eq!(o.exclusive_zone, Some(px(46.0)));
                assert!(o.anchor.contains(Anchor::TOP));
            }
            _ => panic!("bar must be a layer surface"),
        }
    }
}
