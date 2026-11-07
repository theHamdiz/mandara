//! Spring presets and surface transition speeds. Older files retain their tuning.

use serde::{Deserialize, Serialize};

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
        }
    }
}

/// Named spring tuples, rather than a second setting that overrides manual edits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    Default,
    Snappy,
    Smooth,
    Bouncy,
    Subtle,
}

impl Preset {
    pub const ALL: [Self; 5] = [Self::Default, Self::Snappy, Self::Smooth, Self::Bouncy, Self::Subtle];

    /// Stiffness, damping, mass and playback speed for each feel.
    pub fn values(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Default => {
                let m = MotionConfig::default();
                (m.stiffness, m.damping, m.mass, m.speed)
            }
            Self::Snappy => (420.0, 36.0, 1.0, 1.3),
            Self::Smooth => (180.0, 28.0, 1.0, 1.0),
            Self::Bouncy => (280.0, 16.0, 1.0, 1.0),
            Self::Subtle => (280.0, 34.0, 1.0, 1.4),
        }
    }

    /// Change only spring tuning, preserving accessibility and surface choices.
    pub fn apply(self, motion: &mut MotionConfig) {
        (motion.stiffness, motion.damping, motion.mass, motion.speed) = self.values();
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
        Preset::ALL.into_iter().find(|p| p.values() == (self.stiffness, self.damping, self.mass, self.speed))
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
