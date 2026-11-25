//! Shell-wide interface style (`[style]`, `[motion]`,
//! `typography.weight`), mirrored into process globals so leaf helpers
//! (cards, elevation, panel edges) follow it without threading the config
//! through every call.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use gpui::{App, FontWeight};
use mandara_config::ShellConfig;
use mandara_config::desktop::{ButtonDesign, CardStyle, ChipDesign, InputDesign, ProgressDesign, SliderDesign, SurfaceCorners, ToggleDesign};
use mandara_config::surfaces;
mod source;
use source::{Desktop, Studio, StyleSource};

struct ActiveStyleSource(fn(&ShellConfig));
impl gpui::Global for ActiveStyleSource {}

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
/// `[style.ink]`: outline width and shadow offset (f32 bits), texture
/// (its index in [`SurfaceTexture::ALL`]).
static INK_OUTLINE: AtomicU32 = AtomicU32::new(0);
static INK_SHADOW: AtomicU32 = AtomicU32::new(0);
static INK_TEXTURE: AtomicU8 = AtomicU8::new(0);
static INK_BUBBLES: AtomicBool = AtomicBool::new(false);
/// `style.slider_handle` is `line`.
static SLIDER_HANDLE: AtomicBool = AtomicBool::new(true);
/// `style.slider_design`, as its index in [`SliderDesign::ALL`].
static SLIDER_DESIGN: AtomicU8 = AtomicU8::new(0);
/// The component designs (Themes → Components), each as its index in
/// its design's `ALL`.
static BUTTON_DESIGN: AtomicU8 = AtomicU8::new(0);
static TOGGLE_DESIGN: AtomicU8 = AtomicU8::new(0);
static INPUT_DESIGN: AtomicU8 = AtomicU8::new(0);
static CHIP_DESIGN: AtomicU8 = AtomicU8::new(0);
static PROGRESS_DESIGN: AtomicU8 = AtomicU8::new(0);
static SURFACE_CORNERS: AtomicU8 = AtomicU8::new(0);
static CARD_STYLE: AtomicU8 = AtomicU8::new(0);
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

/// Inked outlines' width, logical px (0: off).
pub fn ink_outline() -> f32 {
    f32::from_bits(INK_OUTLINE.load(Ordering::Relaxed))
}

/// Hard shadow offset, logical px (0: soft shadows).
pub fn ink_shadow() -> f32 {
    f32::from_bits(INK_SHADOW.load(Ordering::Relaxed))
}

/// Notifications as comic speech bubbles (`[style.ink] bubbles`).
pub fn ink_bubbles() -> bool {
    INK_BUBBLES.load(Ordering::Relaxed)
}

/// The ink texture of surfaces.
pub fn ink_texture() -> mandara_config::desktop::SurfaceTexture {
    mandara_config::desktop::SurfaceTexture::ALL.get(usize::from(INK_TEXTURE.load(Ordering::Relaxed))).copied().unwrap_or_default()
}

/// The thick sliders draw their handle line (`style.slider_handle`).
pub fn slider_handle() -> bool {
    SLIDER_HANDLE.load(Ordering::Relaxed)
}

/// The design of the thick sliders (`style.slider_design`).
pub fn slider_design() -> SliderDesign {
    SliderDesign::ALL.get(usize::from(SLIDER_DESIGN.load(Ordering::Relaxed))).copied().unwrap_or_default()
}

/// `all[i]` for the index stored in `slot` (the first, classic one if
/// out of range).
fn design<T: Copy + Default>(slot: &AtomicU8, all: &[T]) -> T {
    all.get(usize::from(slot.load(Ordering::Relaxed))).copied().unwrap_or_default()
}

/// Stores `value`'s index in `all` into `slot`.
fn store_design<T: PartialEq>(slot: &AtomicU8, all: &[T], value: &T) {
    let i = all.iter().position(|d| d == value).unwrap_or(0);
    slot.store(u8::try_from(i).unwrap_or(0), Ordering::Relaxed);
}

/// The design of buttons (`style.button_design`).
pub fn button_design() -> ButtonDesign {
    design(&BUTTON_DESIGN, &ButtonDesign::ALL)
}

/// The design of switches, check boxes and radio buttons (`style.toggle_design`).
pub fn toggle_design() -> ToggleDesign {
    design(&TOGGLE_DESIGN, &ToggleDesign::ALL)
}

/// The design of text and search fields (`style.input_design`).
pub fn input_design() -> InputDesign {
    design(&INPUT_DESIGN, &InputDesign::ALL)
}

/// The design of chips and segmented choices (`style.chip_design`).
pub fn chip_design() -> ChipDesign {
    design(&CHIP_DESIGN, &ChipDesign::ALL)
}

/// The design of progress bars and meters (`style.progress_design`).
pub fn progress_design() -> ProgressDesign {
    design(&PROGRESS_DESIGN, &ProgressDesign::ALL)
}

/// The corner family of cards (`style.surface_corners`).
pub fn surface_corners() -> SurfaceCorners {
    design(&SURFACE_CORNERS, &SurfaceCorners::ALL)
}

/// How cards stand off their panel (`style.card_style`).
pub fn card_style() -> CardStyle {
    design(&CARD_STYLE, &CardStyle::ALL)
}

/// Body text weight.
pub fn body_weight() -> FontWeight {
    FontWeight(WEIGHT.load(Ordering::Relaxed) as f32)
}

/// Copies the style settings of `cfg` into the globals.
pub fn apply(cfg: &ShellConfig) {
    apply_from::<Desktop>(cfg);
}

static INDEPENDENT_STUDIO: AtomicBool = AtomicBool::new(false);

/// Selected Studio Looks provide their own card measurements.
pub fn studio_card_metrics() -> Option<(f32, f32)> {
    INDEPENDENT_STUDIO.load(Ordering::Relaxed).then(|| (card_radius(), card_padding()))
}

fn apply_from<S: StyleSource>(cfg: &ShellConfig) {
    INDEPENDENT_STUDIO.store(S::independent_studio(cfg), Ordering::Relaxed);
    let st = S::style(cfg);
    let motion = S::motion(cfg);
    let (card_radius, card_padding) = S::cards(cfg);
    let scale = if st.radius_scale.is_finite() { st.radius_scale.clamp(MIN_RADIUS_SCALE, MAX_RADIUS_SCALE) } else { 1.0 };
    RADIUS_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    BAR_ICON_SCALE.store(S::icon_scale(cfg).to_bits(), Ordering::Relaxed);
    CARD_RADIUS.store(surfaces::ranges::CARD_RADIUS.clamp(card_radius, surfaces::defaults::CARD_RADIUS_PX).to_bits(), Ordering::Relaxed);
    CARD_PADDING.store(surfaces::ranges::CARD_PADDING.clamp(card_padding, surfaces::defaults::CARD_PADDING_PX).to_bits(), Ordering::Relaxed);
    SHADOWS.store(st.shadows, Ordering::Relaxed);
    BORDERS.store(st.borders, Ordering::Relaxed);
    INK_OUTLINE.store(st.ink.outline().to_bits(), Ordering::Relaxed);
    INK_SHADOW.store(st.ink.shadow().to_bits(), Ordering::Relaxed);
    INK_BUBBLES.store(st.ink.bubbles, Ordering::Relaxed);
    INK_TEXTURE.store(mandara_config::desktop::SurfaceTexture::ALL.iter().position(|t| *t == st.ink.texture).unwrap_or(0) as u8, Ordering::Relaxed);
    SLIDER_HANDLE.store(st.slider_handle == mandara_config::desktop::SliderHandle::Line, Ordering::Relaxed);
    let design = SliderDesign::ALL.iter().position(|d| *d == st.slider_design).unwrap_or(0);
    SLIDER_DESIGN.store(design as u8, Ordering::Relaxed);
    store_design(&BUTTON_DESIGN, &ButtonDesign::ALL, &st.button_design);
    store_design(&TOGGLE_DESIGN, &ToggleDesign::ALL, &st.toggle_design);
    store_design(&INPUT_DESIGN, &InputDesign::ALL, &st.input_design);
    store_design(&CHIP_DESIGN, &ChipDesign::ALL, &st.chip_design);
    store_design(&PROGRESS_DESIGN, &ProgressDesign::ALL, &st.progress_design);
    store_design(&SURFACE_CORNERS, &SurfaceCorners::ALL, &st.surface_corners);
    store_design(&CARD_STYLE, &CardStyle::ALL, &st.card_style);
    WEIGHT.store(u32::from(S::typography(cfg).weight.clamp(MIN_WEIGHT, MAX_WEIGHT)), Ordering::Relaxed);
    mandara_core::spring::set_time_scale(motion.speed);
    mandara_core::spring::set_motion_profile(motion_profile_of(&motion));
    mandara_core::choreo::set_choreography(choreography(&motion));
    crate::widgets::clock::set_locale_override(&cfg.datetime.time_locale);
}

/// The choreography of `[motion]`, scaled by its intensity (reduced motion
/// is applied when it is read: [`mandara_core::choreo::choreography`]).
pub fn choreography(m: &mandara_config::MotionConfig) -> mandara_core::choreo::Choreography {
    use mandara_config::motion::Entrance as E;
    use mandara_core::choreo::Entrance;
    let c = &m.choreography;
    let entrance = match c.entrance {
        E::Anchor => Entrance::Anchor,
        E::Rise => Entrance::Rise,
        E::Drop => Entrance::Drop,
        E::Sweep => Entrance::Sweep,
        E::Drift => Entrance::Drift,
    };
    mandara_core::choreo::Choreography::new(entrance, c.stagger, c.stretch, c.parallax, m.intensity())
}

/// The spring tuning and reduced motion of `cfg` (`[motion]`).
pub fn motion_profile(cfg: &ShellConfig) -> mandara_core::spring::MotionProfile {
    motion_profile_of(&cfg.motion)
}

/// [`motion_profile`] of a `[motion]` table alone (Studio previews a preset
/// over the current settings with it).
pub fn motion_profile_of(m: &mandara_config::MotionConfig) -> mandara_core::spring::MotionProfile {
    let mut profile = mandara_core::spring::MotionProfile::new(m.stiffness, m.damping, m.mass, m.reduced(mandara_core::spring::system_reduced_motion()));
    let critical = 2.0 * (profile.feel.stiffness * profile.feel.mass).sqrt();
    profile.feel.damping += (critical - profile.feel.damping).max(0.0) * (1.0 - m.intensity());
    profile
}

/// Apply a portal preference to runtime state, including cached and open views.
pub fn system_motion_changed(reduced: bool, cx: &mut gpui::App) {
    mandara_core::spring::set_system_reduced_motion(reduced);
    let apply = cx.try_global::<ActiveStyleSource>().map_or(apply as fn(&ShellConfig), |source| source.0);
    apply(crate::state::config(cx).read(cx).get());
    crate::state::config(cx).update(cx, |_, cx| cx.notify());
    crate::state::sync_theme_from_config(cx);
    cx.refresh_windows();
}

/// Studio is a separate process: follow accessibility without starting shell services.
pub fn follow_system_motion(cx: &mut gpui::App) {
    let preferences = mandara_services::appearance::watch();
    cx.spawn(async move |cx| {
        while let Ok(preference) = preferences.recv().await {
            cx.update(|cx| system_motion_changed(preference.reduced_motion, cx));
        }
    }).detach();
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
    init_from::<Desktop>(cx);
}

/// Studio selects its own component and motion recipe without changing shell config.
pub fn init_studio(cx: &mut App) {
    init_from::<Studio>(cx);
}

/// Studio's own motion settings with shared accessibility preferences applied.
pub fn studio_motion(config: &ShellConfig) -> mandara_config::MotionConfig {
    Studio::motion(config)
}

fn init_from<S: StyleSource + 'static>(cx: &mut App) {
    let store = crate::state::config(cx);
    cx.set_global(ActiveStyleSource(apply_from::<S>));
    apply_from::<S>(&store.read(cx).snapshot());
    let mut last = S::key(&store.read(cx).snapshot());
    cx.observe(&store, move |store, cx| {
        let cfg = store.read(cx).snapshot();
        let next = S::key(&cfg);
        if next != last {
            if S::icons_changed(&last, &next) {
                crate::plugins::workspaces::clear_icon_cache();
            }
            apply_from::<S>(&cfg);
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
        assert_eq!(slider_design(), SliderDesign::Classic, "the default design is the classic one");
        for design in SliderDesign::ALL {
            cfg.style.slider_design = design;
            apply(&cfg);
            assert_eq!(slider_design(), design);
        }
        apply(&ShellConfig::default());
        let classic = (button_design(), toggle_design(), input_design(), chip_design(), progress_design(), surface_corners(), card_style());
        assert_eq!(classic, (ButtonDesign::Classic, ToggleDesign::Classic, InputDesign::Classic, ChipDesign::Classic, ProgressDesign::Classic, SurfaceCorners::Rounded, CardStyle::Tonal), "every component starts classic");
        for preset in mandara_config::components::ComponentPreset::ALL {
            preset.apply(&mut cfg.style);
            apply(&cfg);
            let d = preset.designs();
            assert_eq!((button_design(), toggle_design(), input_design(), chip_design(), progress_design(), surface_corners(), card_style()), (d.button, d.toggle, d.input, d.chip, d.progress, d.corners, d.card), "{preset:?}");
        }
        apply(&ShellConfig::default());
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
        cfg.motion.reduce_motion = false;
        cfg.motion.intensity = 0.25;
        assert!(motion_profile(&cfg).feel.damping > p.feel.damping);
        cfg.motion.intensity = 0.0;
        assert!(motion_profile(&cfg).reduce);
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
