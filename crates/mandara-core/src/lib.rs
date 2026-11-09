//! Mandara core runtime: layer-shell surface specs, analytical spring
//! physics, OKLCH tweening, frame-synced Wayland input-region masking and the
//! chromatic-glass surface material.

pub mod avatar_shape;
pub mod frame;
pub mod input_mask;
pub mod layer;
pub mod liquid;
pub mod material;
pub mod motion;
pub mod outputs;
pub mod password_symbols;
pub mod shape;
pub mod session_lock;
pub mod shm_canvas;
pub mod spring;
pub mod theme;

pub use frame::{DrawerEnd, Edge, FlareCenter, FrameShape, PanelShape, concave_flare, concave_flare_overlapping, edge_drawer, edge_drawer_ends, edge_drawer_snapped, snap, snap_rect};
pub use input_mask::InputMaskRegistry;
pub use layer::{EXCLUSION_STRIP_THICKNESS_PX, LayerSurfaceSpec, SHELL_APP_ID, STUDIO_APP_ID, SurfaceRole};
pub use material::{ToGpui, glass_fill, glass_rim, glass_rim_width, grain_overlay, oklab_gradient};
pub use motion::FrameClock;
pub use outputs::{DisplayKey, OutputGeometry, output_uuid, pair_displays, query_outputs};
pub use spring::{SpringConfig, SpringValue, Springable};
pub use theme::{GlobalTheme, frame_palette, init_theme, theme_entity};

/// The running build's identity, set once by the binary (from
/// `mandara-version`) so UI crates can show it without depending on a
/// crate that rebuilds on every source change.
#[derive(Clone, Debug, Default)]
pub struct BuildInfo {
    /// Semantic version, e.g. `0.4.2` or `0.4.3-dev` for changed sources.
    pub version: String,
    /// `0.4.2 (3f9c2a1b7e40, content 9f8e…, 2026-10-27, x86_64-…)`.
    pub long: String,
    /// Every component as `(name, version)`.
    pub components: Vec<(String, String)>,
}

/// The process's [`BuildInfo`], shared: readers take a reference count
/// instead of copying its strings and component list on every frame.
struct BuildInfoGlobal(std::sync::Arc<BuildInfo>);

impl gpui::Global for BuildInfoGlobal {}

/// Records the build identity once at startup ([`build_info`] reads it).
pub fn set_build_info(cx: &mut gpui::App, info: BuildInfo) {
    cx.set_global(BuildInfoGlobal(std::sync::Arc::new(info)));
}

/// The build identity (`CARGO_PKG_VERSION` until the binary sets it).
/// Shared, so calling it from a render path copies nothing.
pub fn build_info(cx: &gpui::App) -> std::sync::Arc<BuildInfo> {
    static FALLBACK: std::sync::OnceLock<std::sync::Arc<BuildInfo>> = std::sync::OnceLock::new();
    cx.try_global::<BuildInfoGlobal>().map(|g| g.0.clone()).unwrap_or_else(|| {
        FALLBACK.get_or_init(|| std::sync::Arc::new(BuildInfo { version: env!("CARGO_PKG_VERSION").into(), long: env!("CARGO_PKG_VERSION").into(), components: Vec::new() })).clone()
    })
}
