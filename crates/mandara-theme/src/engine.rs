//! Theme resolution and the 450 ms OKLCH palette crossfade.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::material::{SchemeVariant, SeedColor, extract_seeds, palette_from_seed};
use crate::palette::{MandaraPalette, PaletteAdjustments};
use crate::catalog::{self, DEFAULT_THEME};

/// Duration of a full-shell palette crossfade.
pub const THEME_TRANSITION: Duration = Duration::from_millis(450);

/// Where the active palette comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ThemeSource {
    /// A catalog theme by id (built-in or user TOML).
    Preset { preset: String },
    Wallpaper {
        path: PathBuf,
        variant: SchemeVariant,
        /// Which extracted seed leads (0 = Material's top-scored color).
        #[serde(default)]
        seed: usize,
    },
    Seed { color: Color, variant: SchemeVariant },
}

impl Default for ThemeSource {
    fn default() -> Self {
        ThemeSource::Preset { preset: DEFAULT_THEME.to_string() }
    }
}

/// The default theme's palette (always available: it is built in).
pub fn default_palette(dark: bool) -> MandaraPalette {
    catalog::find(DEFAULT_THEME).expect("default theme is built in").palette(dark)
}

/// Result of resolving a [`ThemeSource`].
#[derive(Clone, Debug)]
pub struct ResolvedTheme {
    pub palette: MandaraPalette,
    /// Ranked seed clusters (wallpaper sources only), for Studio badges.
    pub seeds: Vec<SeedColor>,
}

impl ThemeSource {
    /// Resolves to a dark or light palette. Wallpaper decoding is blocking;
    /// call from a background executor.
    pub fn resolve(&self, dark: bool) -> anyhow::Result<ResolvedTheme> {
        Ok(match self {
            ThemeSource::Preset { preset } => {
                let def = catalog::find(preset).ok_or_else(|| anyhow::anyhow!("unknown theme {preset:?}"))?;
                ResolvedTheme { palette: def.palette(dark), seeds: Vec::new() }
            }
            ThemeSource::Seed { color, variant } => {
                ResolvedTheme { palette: palette_from_seed(*color, *variant, dark), seeds: Vec::new() }
            }
            ThemeSource::Wallpaper { path, variant, seed } => {
                let seeds = extract_seeds(path, 4)?;
                let lead = seeds.get(*seed).or(seeds.first()).map(|s| s.color).unwrap_or(Color::hex("#4285f4"));
                let mut palette = palette_from_seed(lead, *variant, dark);
                palette.name = format!(
                    "{} · {}",
                    path.file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper"),
                    variant.display_name()
                );
                ResolvedTheme { palette, seeds }
            }
        })
    }
}

/// Emphasized-decelerate cubic Bézier, `cubic-bezier(0.05, 0.7, 0.1, 1.0)`.
pub const EMPHASIZED_DECELERATE: CubicBezier = CubicBezier::new(0.05, 0.7, 0.1, 1.0);
/// Standard Material easing, `cubic-bezier(0.2, 0.0, 0.0, 1.0)`.
pub const STANDARD: CubicBezier = CubicBezier::new(0.2, 0.0, 0.0, 1.0);

/// CSS-style cubic Bézier timing function.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CubicBezier {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn sample(a1: f32, a2: f32, t: f32) -> f32 {
        // B(t) = 3(1-t)²t·a1 + 3(1-t)t²·a2 + t³
        let mt = 1.0 - t;
        3.0 * mt * mt * t * a1 + 3.0 * mt * t * t * a2 + t * t * t
    }

    fn sample_derivative(a1: f32, a2: f32, t: f32) -> f32 {
        let mt = 1.0 - t;
        3.0 * mt * mt * a1 + 6.0 * mt * t * (a2 - a1) + 3.0 * t * t * (1.0 - a2)
    }

    /// Maps linear progress `x` to eased progress.
    pub fn ease(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if x == 0.0 || x == 1.0 {
            return x;
        }
        // Newton-Raphson, falling back to bisection when the slope is flat.
        let mut t = x;
        for _ in 0..8 {
            let err = Self::sample(self.x1, self.x2, t) - x;
            if err.abs() < 1e-5 {
                return Self::sample(self.y1, self.y2, t);
            }
            let d = Self::sample_derivative(self.x1, self.x2, t);
            if d.abs() < 1e-6 {
                break;
            }
            t = (t - err / d).clamp(0.0, 1.0);
        }
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        t = x;
        for _ in 0..32 {
            let v = Self::sample(self.x1, self.x2, t);
            if (v - x).abs() < 1e-5 {
                break;
            }
            if v < x {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) * 0.5;
        }
        Self::sample(self.y1, self.y2, t)
    }
}

/// Owns the active palette and animates between palettes in OKLCH.
///
/// This type is UI-agnostic: the GPUI layer wraps it in an `Entity` and calls
/// [`ThemeEngine::sample`] once per animation frame.
#[derive(Clone, Debug)]
pub struct ThemeEngine {
    source: ThemeSource,
    dark: bool,
    adjustments: PaletteAdjustments,
    /// Unadjusted palette for the current source.
    base: MandaraPalette,
    seeds: Vec<SeedColor>,
    /// Palette on screen when the current transition started.
    from: MandaraPalette,
    /// Adjusted target palette.
    to: MandaraPalette,
    started: Option<Instant>,
    duration: Duration,
}

impl ThemeEngine {
    pub fn new(source: ThemeSource, dark: bool, adjustments: PaletteAdjustments) -> Self {
        let resolved = source.resolve(dark).unwrap_or_else(|err| {
            tracing::warn!("theme source failed to resolve ({err:#}); falling back to Mandara Corniche");
            ResolvedTheme { palette: default_palette(dark), seeds: Vec::new() }
        });
        let to = resolved.palette.adjusted(&adjustments);
        Self {
            source,
            dark,
            adjustments,
            base: resolved.palette,
            seeds: resolved.seeds,
            from: to.clone(),
            to,
            started: None,
            duration: THEME_TRANSITION,
        }
    }

    pub fn source(&self) -> &ThemeSource {
        &self.source
    }

    pub fn is_dark(&self) -> bool {
        self.dark
    }

    pub fn adjustments(&self) -> &PaletteAdjustments {
        &self.adjustments
    }

    pub fn seeds(&self) -> &[SeedColor] {
        &self.seeds
    }

    /// The palette the engine is heading towards.
    pub fn target(&self) -> &MandaraPalette {
        &self.to
    }

    pub fn set_duration(&mut self, duration: Duration) {
        self.duration = duration;
    }

    /// Starts a crossfade to an already-resolved theme.
    pub fn apply_resolved(&mut self, source: ThemeSource, dark: bool, resolved: ResolvedTheme, now: Instant) {
        self.source = source;
        self.dark = dark;
        self.base = resolved.palette;
        self.seeds = resolved.seeds;
        self.retarget(now);
    }

    /// Resolves synchronously and crossfades. Prefer
    /// [`ThemeSource::resolve`] on a background thread for wallpapers.
    pub fn set_source(&mut self, source: ThemeSource, dark: bool, now: Instant) -> anyhow::Result<()> {
        let resolved = source.resolve(dark)?;
        self.apply_resolved(source, dark, resolved, now);
        Ok(())
    }

    /// Updates Studio adjustments. Slider drags apply instantly (no tween) so
    /// the live preview tracks the pointer at full frame rate.
    pub fn set_adjustments(&mut self, adjustments: PaletteAdjustments, animate: bool, now: Instant) {
        self.adjustments = adjustments;
        if animate {
            self.retarget(now);
        } else {
            self.to = self.base.adjusted(&self.adjustments);
            self.from = self.to.clone();
            self.started = None;
        }
    }

    fn retarget(&mut self, now: Instant) {
        // Start from whatever is on screen right now so interrupted
        // transitions never jump.
        let (current, _) = self.sample(now);
        self.from = current;
        self.to = self.base.adjusted(&self.adjustments);
        self.started = Some(now);
    }

    /// Returns the palette to render at `now`, and whether a transition is
    /// still running (i.e. another animation frame is needed).
    pub fn sample(&self, now: Instant) -> (MandaraPalette, bool) {
        let Some(start) = self.started else {
            return (self.to.clone(), false);
        };
        let elapsed = now.saturating_duration_since(start);
        if elapsed >= self.duration || self.duration.is_zero() {
            return (self.to.clone(), false);
        }
        let x = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        (self.from.lerp(&self.to, EMPHASIZED_DECELERATE.ease(x)), true)
    }

    /// Clears a finished transition (cheap to call every frame).
    pub fn settle(&mut self, now: Instant) {
        if let Some(start) = self.started
            && now.saturating_duration_since(start) >= self.duration
        {
            self.started = None;
            self.from = self.to.clone();
        }
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.sample(now).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_endpoints_and_monotonic() {
        let e = EMPHASIZED_DECELERATE;
        assert_eq!(e.ease(0.0), 0.0);
        assert_eq!(e.ease(1.0), 1.0);
        let mut prev = 0.0;
        for i in 1..=100 {
            let v = e.ease(i as f32 / 100.0);
            assert!(v >= prev - 1e-4, "not monotonic at {i}");
            prev = v;
        }
        // Decelerate curve front-loads progress.
        assert!(e.ease(0.25) > 0.6);
        // Linear bezier is identity.
        let lin = CubicBezier::new(0.25, 0.25, 0.75, 0.75);
        assert!((lin.ease(0.37) - 0.37).abs() < 1e-3);
    }

    #[test]
    fn transition_runs_for_450ms_and_settles() {
        let t0 = Instant::now();
        let mut engine = ThemeEngine::new(ThemeSource::default(), true, PaletteAdjustments::default());
        let start_primary = engine.target().primary;
        engine
            .set_source(ThemeSource::Preset { preset: "montaza-twilight".into() }, true, t0)
            .unwrap();
        let (mid, animating) = engine.sample(t0 + Duration::from_millis(200));
        assert!(animating);
        assert_ne!(mid.primary.to_hex(), start_primary.to_hex());
        let (end, animating) = engine.sample(t0 + Duration::from_millis(451));
        assert!(!animating);
        assert_eq!(end.primary.to_hex(), catalog::find("montaza-twilight").unwrap().palette(true).primary.to_hex());
        engine.settle(t0 + Duration::from_millis(500));
        assert!(!engine.is_animating(t0 + Duration::from_millis(500)));
    }

    #[test]
    fn interrupted_transition_starts_from_current_frame() {
        let t0 = Instant::now();
        let mut engine = ThemeEngine::new(ThemeSource::default(), true, PaletteAdjustments::default());
        engine
            .set_source(ThemeSource::Preset { preset: "mandara-obsidian".into() }, true, t0)
            .unwrap();
        let t1 = t0 + Duration::from_millis(100);
        let (on_screen, _) = engine.sample(t1);
        engine.set_source(ThemeSource::Preset { preset: "tokyo-night".into() }, true, t1).unwrap();
        let (restart, _) = engine.sample(t1);
        assert_eq!(restart.primary.to_hex(), on_screen.primary.to_hex());
    }

    #[test]
    fn light_mode_crossfades_to_light_palette() {
        let t0 = Instant::now();
        let mut engine = ThemeEngine::new(ThemeSource::default(), true, PaletteAdjustments::default());
        engine.set_source(ThemeSource::default(), false, t0).unwrap();
        assert!(!engine.is_dark());
        let (end, _) = engine.sample(t0 + Duration::from_millis(500));
        assert!(!end.is_dark);
        assert!(end.surface_base.lightness() > 0.9);
    }

    #[test]
    fn missing_wallpaper_falls_back() {
        let source = ThemeSource::Wallpaper {
            path: "/definitely/not/here.png".into(),
            variant: SchemeVariant::Vibrant,
            seed: 0,
        };
        assert!(source.resolve(true).is_err());
        let engine = ThemeEngine::new(source, true, PaletteAdjustments::default());
        assert_eq!(engine.target().name, "Mandara Corniche");
    }
}
