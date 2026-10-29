//! Shell-wide interface style (`[style]`, `[motion]`,
//! `typography.weight`), mirrored into process globals so leaf helpers
//! (cards, elevation, panel edges) follow it without threading the config
//! through every call.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use gpui::{App, FontWeight};
use mandara_config::ShellConfig;
use mandara_config::surfaces;

/// Corner-radius multiplier bounds.
const MIN_RADIUS_SCALE: f32 = 0.0;
const MAX_RADIUS_SCALE: f32 = 2.0;
/// CSS font-weight bounds.
const MIN_WEIGHT: u16 = 100;
const MAX_WEIGHT: u16 = 900;

/// `1.0f32` as bits (the default radius scale).
const ONE_BITS: u32 = 0x3f80_0000;

static RADIUS_SCALE: AtomicU32 = AtomicU32::new(ONE_BITS);
/// `bar.icon_scale`.
static BAR_ICON_SCALE: AtomicU32 = AtomicU32::new(ONE_BITS);
/// `sidebar.look.card_radius` / `card_padding` (before the radius scale).
static CARD_RADIUS: AtomicU32 = AtomicU32::new(surfaces::defaults::CARD_RADIUS_PX.to_bits());
static CARD_PADDING: AtomicU32 = AtomicU32::new(surfaces::defaults::CARD_PADDING_PX.to_bits());
static SHADOWS: AtomicBool = AtomicBool::new(true);
static BORDERS: AtomicBool = AtomicBool::new(true);
/// Body text weight, CSS scale.
static WEIGHT: AtomicU32 = AtomicU32::new(mandara_config::defaults::FONT_WEIGHT_REGULAR as u32);

/// Clipping children to a container's rounded shape.
///
/// GPUI clips the children of an element whose overflow is not visible on
/// both axes to its rounded rectangle (Mandara's rounded content masks,
/// `vendor/gpui/MANDARA-PATCHES.md`), so covers, art, colour bands and
/// hover fills never spill past a card's corners. `clip_rounded` asks for
/// that clip with `Overflow::Clip`, which keeps the element's content-based
/// minimum size: unlike `overflow_hidden` it never lets a flex item shrink
/// below its content, so adding it to a card changes nothing but the
/// clipping.
pub trait ClipRounded: gpui::Styled + Sized {
    /// Clips children to this element's (rounded) bounds without changing layout.
    fn clip_rounded(mut self) -> Self {
        self.style().overflow.x = Some(gpui::Overflow::Clip);
        self.style().overflow.y = Some(gpui::Overflow::Clip);
        self
    }
}

impl<E: gpui::Styled> ClipRounded for E {}

/// Multiplier for card / tile corner radii.
pub fn radius_scale() -> f32 {
    f32::from_bits(RADIUS_SCALE.load(Ordering::Relaxed))
}

/// `r` scaled by the global corner-radius setting.
pub fn radius(r: f32) -> f32 {
    r * radius_scale()
}

/// Corner radius of an edge nested `pad` inside an edge rounded by
/// `outer` (both already final): concentric corners share one center, so
/// the inner radius is the outer minus the gap, never negative.
pub fn concentric(outer: f32, pad: f32) -> f32 {
    (outer - pad).max(0.0)
}

/// [`concentric`] for a designed (unscaled) outer radius.
pub fn inner_radius(outer: f32, pad: f32) -> f32 {
    concentric(radius(outer), pad)
}

/// Corner radius of drop-down panels (`bar.panel_radius`, scaled).
pub fn panel_radius(bar: &mandara_config::BarConfig) -> f32 {
    radius(surfaces::ranges::PANEL_RADIUS.clamp(bar.panel_radius, mandara_config::defaults::PANEL_RADIUS_PX))
}

/// Concave flare radius where panels meet the edge frame (scaled).
pub fn flare_radius(bar: &mandara_config::BarConfig) -> f32 {
    radius(surfaces::ranges::BAR_FLARE_RADIUS.clamp(bar.flare_radius, mandara_config::defaults::BAR_FLARE_RADIUS_PX))
}

/// Outer corner radius of the side panels (sidebar, power dock; scaled).
pub fn side_panel_radius(look: &surfaces::SidebarLook) -> f32 {
    radius(surfaces::ranges::SIDE_PANEL_RADIUS.clamp(look.radius, surfaces::defaults::SIDE_PANEL_RADIUS_PX))
}

/// A bar icon's designed size `px` scaled by `bar.icon_scale`.
pub fn bar_icon(px: f32) -> f32 {
    px * f32::from_bits(BAR_ICON_SCALE.load(Ordering::Relaxed))
}

/// Corner radius of cards (`sidebar.look.card_radius`, scaled).
pub fn card_radius() -> f32 {
    radius(f32::from_bits(CARD_RADIUS.load(Ordering::Relaxed)))
}

/// Inner padding of cards (`sidebar.look.card_padding`).
pub fn card_padding() -> f32 {
    f32::from_bits(CARD_PADDING.load(Ordering::Relaxed))
}

/// Soft shadows under panels, toasts and popups.
pub fn shadows() -> bool {
    SHADOWS.load(Ordering::Relaxed)
}

/// Hairline borders around panels and popups.
pub fn borders() -> bool {
    BORDERS.load(Ordering::Relaxed)
}

/// Body text weight.
pub fn body_weight() -> FontWeight {
    FontWeight(WEIGHT.load(Ordering::Relaxed) as f32)
}

/// Copies the style settings of `cfg` into the globals.
pub fn apply(cfg: &ShellConfig) {
    let scale = if cfg.style.radius_scale.is_finite() { cfg.style.radius_scale.clamp(MIN_RADIUS_SCALE, MAX_RADIUS_SCALE) } else { 1.0 };
    RADIUS_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    BAR_ICON_SCALE.store(cfg.bar.icon_scale().to_bits(), Ordering::Relaxed);
    let look = &cfg.sidebar.look;
    CARD_RADIUS.store(surfaces::ranges::CARD_RADIUS.clamp(look.card_radius, surfaces::defaults::CARD_RADIUS_PX).to_bits(), Ordering::Relaxed);
    CARD_PADDING.store(surfaces::ranges::CARD_PADDING.clamp(look.card_padding, surfaces::defaults::CARD_PADDING_PX).to_bits(), Ordering::Relaxed);
    SHADOWS.store(cfg.style.shadows, Ordering::Relaxed);
    BORDERS.store(cfg.style.borders, Ordering::Relaxed);
    WEIGHT.store(u32::from(cfg.typography.weight.clamp(MIN_WEIGHT, MAX_WEIGHT)), Ordering::Relaxed);
    mandara_core::spring::set_time_scale(cfg.motion.speed);
    mandara_core::spring::set_motion_profile(motion_profile(cfg));
    crate::widgets::clock::set_locale_override(&cfg.datetime.time_locale);
}

/// The spring tuning and reduced motion of `cfg` (`[motion]`).
pub fn motion_profile(cfg: &ShellConfig) -> mandara_core::spring::MotionProfile {
    let m = &cfg.motion;
    mandara_core::spring::MotionProfile::new(m.stiffness, m.damping, m.mass, m.reduce_motion)
}

/// Everything that changes how surfaces look without a plugin noticing:
/// cached views (bar chips, panel content, separate overlay windows) must
/// redraw when any of it changes, so a Studio slider shows at once.
#[derive(Clone, PartialEq)]
struct LookKey {
    style: mandara_config::desktop::StyleConfig,
    typography: mandara_config::TypographyConfig,
    motion: mandara_config::MotionConfig,
    time_locale: String,
    appearance: mandara_config::AppearanceConfig,
    bar: mandara_config::BarConfig,
    sidebar: mandara_config::SidebarConfig,
    panels: surfaces::PanelsConfig,
    osd: mandara_config::alerts::OsdConfig,
    dock: mandara_config::workflow::DockConfig,
    switcher: mandara_config::workflow::SwitcherConfig,
    launcher: mandara_config::LauncherConfig,
}

impl LookKey {
    fn of(c: &ShellConfig) -> Self {
        Self {
            style: c.style.clone(),
            typography: c.typography.clone(),
            motion: c.motion,
            time_locale: c.datetime.time_locale.clone(),
            appearance: c.appearance.clone(),
            bar: c.bar.clone(),
            sidebar: c.sidebar.clone(),
            panels: c.panels.clone(),
            osd: c.osd.clone(),
            dock: c.dock.clone(),
            switcher: c.switcher.clone(),
            launcher: c.launcher.clone(),
        }
    }
}

/// Applies the style now and on every config change (shell process), and
/// redraws every window (cached views included) when the look changed.
pub fn init(cx: &mut App) {
    let store = crate::state::config(cx);
    apply(&store.read(cx).snapshot());
    let mut last = LookKey::of(&store.read(cx).snapshot());
    cx.observe(&store, move |store, cx| {
        let cfg = store.read(cx).snapshot();
        let next = LookKey::of(&cfg);
        if next != last {
            if next.appearance.icon_theme != last.appearance.icon_theme {
                crate::plugins::workspaces::clear_icon_cache();
            }
            apply(&cfg);
            last = next;
            cx.refresh_windows();
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The style lives in process globals: tests that set it run one at a time.
    static GLOBALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn style_globals_follow_the_config() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = ShellConfig::default();
        cfg.style.radius_scale = 5.0;
        cfg.style.shadows = false;
        cfg.typography.weight = 1200;
        apply(&cfg);
        assert_eq!(radius_scale(), MAX_RADIUS_SCALE);
        assert!(!shadows() && borders());
        assert_eq!(body_weight(), FontWeight(f32::from(MAX_WEIGHT)));
        apply(&ShellConfig::default());
        assert_eq!(radius(10.0), 10.0);
        assert!(shadows());
        assert_eq!(mandara_core::spring::time_scale(), 1.0);
    }

    #[test]
    fn motion_settings_become_the_spring_profile() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        use mandara_core::spring::{DEFAULT_DAMPING, DEFAULT_MASS, DEFAULT_STIFFNESS, MotionProfile};
        let defaults = motion_profile(&ShellConfig::default());
        assert_eq!(defaults, MotionProfile::default(), "the default config leaves springs as designed");
        let mut cfg = ShellConfig::default();
        cfg.motion.stiffness = DEFAULT_STIFFNESS * 2.0;
        cfg.motion.damping = DEFAULT_DAMPING / 2.0;
        cfg.motion.mass = DEFAULT_MASS * 1.5;
        cfg.motion.reduce_motion = true;
        let p = motion_profile(&cfg);
        assert_eq!((p.feel.stiffness, p.feel.damping, p.feel.mass), (cfg.motion.stiffness, cfg.motion.damping, cfg.motion.mass));
        assert!(p.reduce);
        // A shell spring is retuned by it.
        let tuned = p.tune(mandara_core::SpringConfig::SNAPPY);
        assert!(tuned.stiffness > mandara_core::SpringConfig::SNAPPY.stiffness);
        assert!(tuned.damping_ratio() < mandara_core::SpringConfig::SNAPPY.damping_ratio());
    }

    #[test]
    fn radius_math_is_concentric_and_bounded() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(concentric(26.0, 6.0), 20.0);
        assert_eq!(concentric(4.0, 10.0), 0.0, "an inner corner never goes negative");
        let mut cfg = ShellConfig::default();
        cfg.style.radius_scale = 0.5;
        apply(&cfg);
        assert_eq!(radius(20.0), 10.0);
        assert_eq!(inner_radius(26.0, 6.0), 7.0, "scale the outer, then subtract the padding");
        assert_eq!(panel_radius(&cfg.bar), mandara_config::defaults::PANEL_RADIUS_PX * 0.5);
        cfg.style.radius_scale = f32::NAN;
        apply(&cfg);
        assert_eq!(radius_scale(), 1.0, "a broken value falls back to 1");
        cfg.style.radius_scale = -3.0;
        apply(&cfg);
        assert_eq!(radius(18.0), 0.0);
        cfg.style.radius_scale = 1.0;
        cfg.bar.icon_scale = 1.5;
        cfg.sidebar.look.card_radius = 4.0;
        cfg.sidebar.look.card_padding = 200.0;
        apply(&cfg);
        assert_eq!(bar_icon(14.0), 21.0);
        assert_eq!(card_radius(), 4.0);
        assert_eq!(card_padding(), surfaces::ranges::CARD_PADDING.max);
        apply(&ShellConfig::default());
        assert_eq!((bar_icon(14.0), card_radius(), card_padding()), (14.0, surfaces::defaults::CARD_RADIUS_PX, surfaces::defaults::CARD_PADDING_PX));
    }
}
