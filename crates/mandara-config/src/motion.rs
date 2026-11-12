//! Spring presets, choreography and surface transition speeds. Older files
//! retain their tuning.

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use crate::dock::DockMotion;

/// Zero makes a surface transition instant; other values scale its playback.
pub const SURFACE_SPEED_RANGE: (f32, f32) = (0.0, 3.0);
/// Motion intensity scales bounce from critically damped to the chosen spring.
pub const INTENSITY_RANGE: (f32, f32) = (0.0, 1.0);

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceMotion {
    pub bar: f32,
    pub panels: f32,
    pub sidebar: f32,
    pub launcher: f32,
    pub dock: f32,
    pub notifications: f32,
    pub osd: f32,
}

impl Default for SurfaceMotion {
    fn default() -> Self {
        Self {
            bar: 1.0,
            panels: 1.0,
            sidebar: 1.0,
            launcher: 1.0,
            dock: 1.0,
            notifications: 1.0,
            osd: 1.0,
        }
    }
}

/// Sanitize persisted surface speeds at the point of use.
pub fn surface_speed(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(SURFACE_SPEED_RANGE.0, SURFACE_SPEED_RANGE.1)
    } else {
        1.0
    }
}

/// Where entering content comes from (`[motion.choreography] entrance`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Entrance {
    /// Each surface's own path: out of its button, bar or screen edge.
    #[default]
    Anchor,
    /// Rises from below.
    Rise,
    /// Falls from above and lands on the spring's overshoot.
    Drop,
    /// Sweeps in from the side where reading starts.
    Sweep,
    /// Drifts in on a slant from the side where reading ends.
    Drift,
}

impl Entrance {
    pub const ALL: [Self; 5] = [Self::Anchor, Self::Rise, Self::Drop, Self::Sweep, Self::Drift];
}

/// Shared range of the choreography amounts.
pub const CHOREOGRAPHY_RANGE: (f32, f32) = (0.0, 1.0);

/// How entering content moves on top of the spring (`[motion.choreography]`).
/// Every amount is a pure function of a surface's own spring, so it adds no
/// timers; zero everywhere (the default) is the classic motion.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choreography {
    pub entrance: Entrance,
    /// Lists and tiles cascade in one after another (0: all at once).
    pub stagger: f32,
    /// Moving shapes stretch along their direction of travel.
    pub stretch: f32,
    /// Content trails its surface, settling a beat later.
    pub parallax: f32,
}

impl Default for Choreography {
    fn default() -> Self {
        Self::CLASSIC
    }
}

impl Choreography {
    /// No choreography: each surface's own motion, unchanged.
    pub const CLASSIC: Self = Self { entrance: Entrance::Anchor, stagger: 0.0, stretch: 0.0, parallax: 0.0 };
}

/// Bound a persisted choreography amount, treating invalid values as off.
pub fn choreography_amount(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(CHOREOGRAPHY_RANGE.0, CHOREOGRAPHY_RANGE.1)
    } else {
        0.0
    }
}

/// Damped-harmonic-oscillator parameters shared by every spring.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionConfig {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
    pub reduce_motion: bool,
    pub speed: f32,
    /// Respect the portal's reduced-motion preference without changing this file.
    pub follow_system: bool,
    /// Zero applies transitions instantly; lower values suppress overshoot.
    pub intensity: f32,
    pub surfaces: SurfaceMotion,
    pub choreography: Choreography,
    /// Choosing one of the six choreographed presets also sets its dock
    /// animations and wallpaper transition.
    pub preset_desktop: bool,
    /// …and its window animations (written to the compositor drop-in).
    pub preset_windows: bool,
}

impl Default for MotionConfig {
    fn default() -> Self {
        use crate::schema::defaults;
        Self {
            stiffness: defaults::SPRING_STIFFNESS,
            damping: defaults::SPRING_DAMPING,
            mass: defaults::SPRING_MASS,
            reduce_motion: false,
            speed: 1.0,
            follow_system: false,
            intensity: 1.0,
            surfaces: SurfaceMotion::default(),
            choreography: Choreography::CLASSIC,
            preset_desktop: true,
            preset_windows: false,
        }
    }
}

/// Named spring tuples and choreographies, rather than a second setting that
/// overrides manual edits: applying one writes its values, and editing any of
/// them afterwards reads as Custom.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    Default,
    Snappy,
    Smooth,
    Bouncy,
    Subtle,
    Tide,
    Lighthouse,
    Felucca,
    Magnetic,
    Mercury,
    Raindrop,
}

/// Everything a preset writes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PresetValues {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
    pub speed: f32,
    pub choreography: Choreography,
}

impl PresetValues {
    const fn spring(stiffness: f32, damping: f32, mass: f32, speed: f32) -> Self {
        Self { stiffness, damping, mass, speed, choreography: Choreography::CLASSIC }
    }

    const fn with(self, entrance: Entrance, stagger: f32, stretch: f32, parallax: f32) -> Self {
        Self { choreography: Choreography { entrance, stagger, stretch, parallax }, ..self }
    }

    fn of(m: &MotionConfig) -> Self {
        Self { stiffness: m.stiffness, damping: m.damping, mass: m.mass, speed: m.speed, choreography: m.choreography }
    }
}

impl Preset {
    pub const ALL: [Self; 11] = [
        Self::Default,
        Self::Snappy,
        Self::Smooth,
        Self::Bouncy,
        Self::Subtle,
        Self::Tide,
        Self::Lighthouse,
        Self::Felucca,
        Self::Magnetic,
        Self::Mercury,
        Self::Raindrop,
    ];

    /// The ids `mandara msg motion preset` takes, in [`Self::ALL`]'s order.
    pub const IDS: [&'static str; 11] = ["default", "snappy", "smooth", "bouncy", "subtle", "tide", "lighthouse", "felucca", "magnetic", "mercury", "raindrop"];

    pub fn id(self) -> &'static str {
        Self::IDS[self as usize]
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id().eq_ignore_ascii_case(id.trim()))
    }

    /// The preset's name (`tk` text).
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => tk("Default"),
            Self::Snappy => tk("Snappy"),
            Self::Smooth => tk("Smooth"),
            Self::Bouncy => tk("Bouncy"),
            Self::Subtle => tk("Subtle"),
            Self::Tide => tk("Corniche tide"),
            Self::Lighthouse => tk("Lighthouse sweep"),
            Self::Felucca => tk("Felucca drift"),
            Self::Magnetic => tk("Magnetic"),
            Self::Mercury => tk("Mercury"),
            Self::Raindrop => tk("Raindrop"),
        }
    }

    /// How the preset feels, in one line (`tk` text).
    pub fn feel(self) -> &'static str {
        match self {
            Self::Default => tk("Mandara's balanced spring with a soft landing"),
            Self::Snappy => tk("Quick and firm, settling almost at once"),
            Self::Smooth => tk("Slow and silky, with no bounce"),
            Self::Bouncy => tk("Playful springs that swing past and back"),
            Self::Subtle => tk("Brisk and calm, barely any overshoot"),
            Self::Tide => tk("Everything rises like a swell, lists rolling in wave after wave"),
            Self::Lighthouse => tk("A quick beam sweeps across, lighting each row in turn"),
            Self::Felucca => tk("Heavy and unhurried: surfaces glide in on a slant, content trailing like a sail"),
            Self::Magnetic => tk("Pulled into place with one crisp tick past and back"),
            Self::Mercury => tk("Liquid metal: shapes stretch as they travel and round off at rest"),
            Self::Raindrop => tk("Drops in from above and lands with an elastic bounce"),
        }
    }

    /// Stiffness, damping, mass, playback speed and choreography for each feel.
    pub fn values(self) -> PresetValues {
        match self {
            Self::Default => PresetValues::of(&MotionConfig::default()),
            Self::Snappy => PresetValues::spring(420.0, 36.0, 1.0, 1.3),
            Self::Smooth => PresetValues::spring(180.0, 28.0, 1.0, 1.0),
            Self::Bouncy => PresetValues::spring(280.0, 16.0, 1.0, 1.0),
            Self::Subtle => PresetValues::spring(280.0, 34.0, 1.0, 1.4),
            // ζ ≈ 0.77 with a little extra mass: a long swell that crests once.
            Self::Tide => PresetValues::spring(200.0, 24.0, 1.2, 1.0).with(Entrance::Rise, 0.55, 0.0, 0.4),
            // ζ ≈ 0.82 and fast: the cascade is the beam, not a bounce.
            Self::Lighthouse => PresetValues::spring(480.0, 36.0, 1.0, 1.15).with(Entrance::Sweep, 0.8, 0.0, 0.0),
            // ζ ≈ 0.79 with a heavy hull: slow travel and deep parallax.
            Self::Felucca => PresetValues::spring(130.0, 22.0, 1.5, 0.9).with(Entrance::Drift, 0.2, 0.0, 0.8),
            // ζ ≈ 0.55 at high stiffness: a fast pull and one short tick.
            Self::Magnetic => PresetValues::spring(640.0, 28.0, 1.0, 1.1).with(Entrance::Anchor, 0.0, 0.3, 0.0),
            // ζ ≈ 0.55 at the default pace, with strong stretch.
            Self::Mercury => PresetValues::spring(300.0, 19.0, 1.0, 1.0).with(Entrance::Anchor, 0.15, 0.9, 0.2),
            // ζ ≈ 0.41: the drop lands past its rest and bounces back.
            Self::Raindrop => PresetValues::spring(340.0, 15.0, 1.0, 1.0).with(Entrance::Drop, 0.35, 0.4, 0.0),
        }
    }

    /// The dock animations that match the preset's personality, or `None`
    /// for the classic presets, which leave the dock as it is.
    pub fn dock(self) -> Option<DockMotion> {
        use crate::dock::{AttentionFeedback as A, DockReveal as R, IndicatorMotion as I, ItemMotion as It, LaunchFeedback as L, MinimizeFeedback as M};
        let m = |launch, attention, indicator, minimize, items, reveal| DockMotion { launch, attention, indicator, minimize, items, reveal };
        match self {
            Self::Default | Self::Snappy | Self::Smooth | Self::Bouncy | Self::Subtle => None,
            // A swell: icons breathe, everything rolls in on springs.
            Self::Tide => Some(m(L::Pulse, A::Pulse, I::Spring, M::Dip, It::Spring, R::Slide)),
            // A beam: rings of light, things light up and fade.
            Self::Lighthouse => Some(m(L::Ripple, A::Pulse, I::Fade, M::Pulse, It::Fade, R::Fade)),
            // A sail: slow fades and a sway for attention.
            Self::Felucca => Some(m(L::Pulse, A::Wiggle, I::Fade, M::Dip, It::Fade, R::Fade)),
            // A crisp pull: hops, a tick for attention, a snap into the dock.
            Self::Magnetic => Some(m(L::Bounce, A::Wiggle, I::Spring, M::Dip, It::Spring, R::Slide)),
            // Liquid metal: swelling icons and a dock that grows out of its edge.
            Self::Mercury => Some(m(L::Pulse, A::Pulse, I::Spring, M::Pulse, It::Spring, R::Scale)),
            // A drop: it lands, bounces and splashes into the dock.
            Self::Raindrop => Some(m(L::Bounce, A::Bounce, I::Spring, M::Dip, It::Spring, R::Scale)),
        }
    }

    /// The wallpaper transition that matches the preset, or `None` for the
    /// classic presets.
    pub fn wallpaper(self) -> Option<crate::desktop::WallpaperTransition> {
        use crate::desktop::WallpaperTransition as W;
        match self {
            Self::Default | Self::Snappy | Self::Smooth | Self::Bouncy | Self::Subtle => None,
            Self::Tide => Some(W::Tide),
            Self::Lighthouse => Some(W::Wipe),
            Self::Felucca => Some(W::Dissolve),
            Self::Magnetic => Some(W::Glitch),
            Self::Mercury => Some(W::Shatter),
            Self::Raindrop => Some(W::Ripple),
        }
    }

    /// The window animation style that matches the preset, or `None` for
    /// the classic presets.
    pub fn windows(self) -> Option<crate::window_motion::WindowStyle> {
        use crate::window_motion::WindowStyle as S;
        match self {
            Self::Default | Self::Snappy | Self::Smooth | Self::Bouncy | Self::Subtle => None,
            Self::Tide => Some(S::Swell),
            Self::Lighthouse => Some(S::Beam),
            Self::Felucca => Some(S::Drift),
            Self::Magnetic => Some(S::Snap),
            Self::Mercury => Some(S::Liquid),
            Self::Raindrop => Some(S::Drop),
        }
    }

    /// [`Self::apply`], plus the preset's dock animations and wallpaper
    /// transition when `motion.preset_desktop` is on and its window
    /// animations when `motion.preset_windows` is on. The classic presets
    /// change only `[motion]`.
    pub fn apply_to(self, cfg: &mut crate::ShellConfig) {
        self.apply(&mut cfg.motion);
        if cfg.motion.preset_desktop {
            if let Some(d) = self.dock() {
                cfg.dock.motion = d;
            }
            if let Some(w) = self.wallpaper() {
                cfg.wallpaper.transition = w;
            }
        }
        if cfg.motion.preset_windows
            && let Some(style) = self.windows()
        {
            cfg.compositor.look.windows.style = style;
        }
    }

    /// Change only spring tuning and choreography, preserving accessibility
    /// and surface choices.
    pub fn apply(self, motion: &mut MotionConfig) {
        let v = self.values();
        (motion.stiffness, motion.damping, motion.mass, motion.speed) = (v.stiffness, v.damping, v.mass, v.speed);
        motion.choreography = v.choreography;
    }
}

impl MotionConfig {
    /// Bound the visual intensity, using the default for invalid config values.
    pub fn intensity(&self) -> f32 {
        if self.intensity.is_finite() {
            self.intensity.clamp(INTENSITY_RANGE.0, INTENSITY_RANGE.1)
        } else {
            1.0
        }
    }

    /// Combine the user's accessibility choices with the desktop preference.
    pub fn reduced(&self, system: bool) -> bool {
        self.reduce_motion || self.intensity() == 0.0 || (self.follow_system && system)
    }

    pub fn preset(&self) -> Option<Preset> {
        let current = PresetValues::of(self);
        Preset::ALL.into_iter().find(|p| p.values() == current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_custom_motion_and_accessibility_survive_presets() {
        let mut m: MotionConfig = serde_json::from_str(r#"{"stiffness":321,"speed":2}"#).unwrap();
        assert_eq!(m.preset(), None);
        assert_eq!(m.surfaces, SurfaceMotion::default());
        m.reduce_motion = true;
        m.follow_system = true;
        m.surfaces.dock = 0.0;
        for p in Preset::ALL {
            p.apply(&mut m);
            assert_eq!(m.preset(), Some(p));
            assert!(m.reduce_motion && m.follow_system);
            assert_eq!(m.surfaces.dock, 0.0);
        }
    }

    #[test]
    fn configs_without_choreography_load_as_classic() {
        // A file from before choreography existed, on a preset's numbers.
        let m: MotionConfig = toml::from_str("stiffness = 420.0\ndamping = 36.0\nmass = 1.0\nspeed = 1.3\n").unwrap();
        assert_eq!(m.choreography, Choreography::CLASSIC);
        assert_eq!(m.preset(), Some(Preset::Snappy));
        assert_eq!(MotionConfig::default().preset(), Some(Preset::Default));
        // A partial table fills the rest from the defaults.
        let m: MotionConfig = toml::from_str("[choreography]\nentrance = \"drop\"\n").unwrap();
        assert_eq!(m.choreography, Choreography { entrance: Entrance::Drop, ..Choreography::CLASSIC });
        assert_eq!(m.preset(), None);
        // Round trip.
        let mut m = MotionConfig::default();
        Preset::Felucca.apply(&mut m);
        let back: MotionConfig = toml::from_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.preset(), Some(Preset::Felucca));
        let json: MotionConfig = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(json.preset(), Some(Preset::Felucca));
    }

    #[test]
    fn every_preset_is_distinct_named_and_in_range() {
        for (i, a) in Preset::ALL.iter().enumerate() {
            let v = a.values();
            assert!((60.0..=900.0).contains(&v.stiffness) && (4.0..=60.0).contains(&v.damping), "{a:?}");
            assert!((0.3..=3.0).contains(&v.mass) && (0.25..=3.0).contains(&v.speed), "{a:?}");
            let c = v.choreography;
            for amount in [c.stagger, c.stretch, c.parallax] {
                assert_eq!(choreography_amount(amount), amount, "{a:?}");
            }
            assert_eq!(Preset::from_id(a.id()), Some(*a));
            assert_eq!(Preset::from_id(&a.id().to_uppercase()), Some(*a));
            assert!(!a.name().is_empty() && !a.feel().is_empty());
            for b in &Preset::ALL[i + 1..] {
                assert_ne!(v, b.values(), "{a:?} and {b:?} write the same values");
                assert_ne!(a.id(), b.id());
            }
        }
        assert_eq!(Preset::from_id("nope"), None);
        // The new presets each carry a choreography; the classic ones don't.
        for p in [Preset::Default, Preset::Snappy, Preset::Smooth, Preset::Bouncy, Preset::Subtle] {
            assert_eq!(p.values().choreography, Choreography::CLASSIC);
        }
        for p in [Preset::Tide, Preset::Lighthouse, Preset::Felucca, Preset::Magnetic, Preset::Mercury, Preset::Raindrop] {
            assert_ne!(p.values().choreography, Choreography::CLASSIC, "{p:?}");
        }
    }

    #[test]
    fn classic_presets_leave_the_dock_and_new_ones_each_bring_their_own() {
        for p in [Preset::Default, Preset::Snappy, Preset::Smooth, Preset::Bouncy, Preset::Subtle] {
            assert_eq!(p.dock(), None, "{p:?}");
        }
        let new = [Preset::Tide, Preset::Lighthouse, Preset::Felucca, Preset::Magnetic, Preset::Mercury, Preset::Raindrop];
        for (i, a) in new.iter().enumerate() {
            let d = a.dock().unwrap_or_else(|| panic!("{a:?} has no dock motion"));
            assert_ne!(d, DockMotion::default(), "{a:?} only repeats the defaults");
            // Every preset keeps the dock answering: launches and attention
            // always show something.
            assert_ne!(d.launch, crate::dock::LaunchFeedback::None, "{a:?}");
            assert_ne!(d.attention, crate::dock::AttentionFeedback::None, "{a:?}");
            for b in &new[i + 1..] {
                assert_ne!(Some(d), b.dock(), "{a:?} and {b:?} animate the dock alike");
            }
        }
        // `apply` keeps writing only `[motion]`: the dock is untouched.
        let mut m = MotionConfig::default();
        Preset::Mercury.apply(&mut m);
        assert_eq!(m.preset(), Some(Preset::Mercury));
    }

    #[test]
    fn editing_a_choreography_reads_as_custom_and_presets_reset_it() {
        let mut m = MotionConfig::default();
        Preset::Tide.apply(&mut m);
        m.choreography.stagger = 0.3;
        assert_eq!(m.preset(), None);
        Preset::Snappy.apply(&mut m);
        assert_eq!(m.choreography, Choreography::CLASSIC);
        assert_eq!(m.preset(), Some(Preset::Snappy));
        assert_eq!(choreography_amount(f32::NAN), 0.0);
        assert_eq!(choreography_amount(4.0), 1.0);
        assert_eq!(choreography_amount(-1.0), 0.0);
    }

    #[test]
    fn presets_reach_the_dock_wallpaper_and_windows_only_when_asked() {
        let mut cfg = crate::ShellConfig::default();
        let (dock, wallpaper, windows) = (cfg.dock.motion, cfg.wallpaper.transition, cfg.compositor.look.windows);
        // Classic presets touch only [motion].
        Preset::Snappy.apply_to(&mut cfg);
        assert_eq!((cfg.dock.motion, cfg.wallpaper.transition, cfg.compositor.look.windows), (dock, wallpaper, windows));
        // Desktop on by default, windows opt-in.
        Preset::Raindrop.apply_to(&mut cfg);
        assert_eq!(cfg.motion.preset(), Some(Preset::Raindrop));
        assert_eq!(Some(cfg.dock.motion), Preset::Raindrop.dock());
        assert_eq!(cfg.wallpaper.transition, crate::desktop::WallpaperTransition::Ripple);
        assert_eq!(cfg.compositor.look.windows, windows);
        cfg.motion.preset_windows = true;
        cfg.motion.preset_desktop = false;
        Preset::Tide.apply_to(&mut cfg);
        assert_eq!(cfg.compositor.look.windows.style, crate::window_motion::WindowStyle::Swell);
        assert_eq!(cfg.wallpaper.transition, crate::desktop::WallpaperTransition::Ripple, "desktop off: left alone");
        for p in Preset::ALL {
            assert_eq!(p.wallpaper().is_some(), p.windows().is_some());
            assert_eq!(p.dock().is_some(), p.windows().is_some());
        }
        let old: MotionConfig = toml::from_str("stiffness = 300.0\n").unwrap();
        assert!(old.preset_desktop && !old.preset_windows);
    }

    #[test]
    fn reduction_and_invalid_values_are_bounded() {
        let mut m = MotionConfig::default();
        assert!(!m.reduced(true));
        m.follow_system = true;
        assert!(m.reduced(true));
        assert!(!m.reduced(false));
        m.intensity = 0.0;
        assert!(m.reduced(false));
        m.intensity = f32::NAN;
        assert_eq!(m.intensity(), 1.0);
        assert_eq!(surface_speed(f32::NAN), 1.0);
        assert_eq!(surface_speed(-1.0), 0.0);
        assert_eq!(surface_speed(99.0), SURFACE_SPEED_RANGE.1);
    }
}
