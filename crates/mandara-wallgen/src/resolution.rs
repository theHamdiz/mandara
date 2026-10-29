//! Output sizes.

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

/// Smallest and largest side a custom size may have (the WebP limit is
/// 16383; 16384 keeps PNG / JPEG renders of 16K panoramas possible).
pub const MIN_SIDE: u32 = 16;
pub const MAX_SIDE: u32 = 16384;

/// A render size: standard height classes, an ultrawide and a portrait
/// option, or anything custom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    /// 1920 × 1080.
    Fhd1080,
    /// 2560 × 1440 ("2K").
    Qhd1440,
    /// 3840 × 2160.
    Uhd4k,
    /// 5120 × 2880.
    Uhd5k,
    /// 7680 × 4320.
    Uhd8k,
    /// 3440 × 1440 ultrawide.
    Ultrawide1440,
    /// 2160 × 3840 portrait (a rotated 4K display).
    Portrait4k,
    Custom { width: u32, height: u32 },
}

impl Default for Resolution {
    fn default() -> Self {
        Resolution::Uhd4k
    }
}

impl Resolution {
    /// The named sizes, smallest first.
    pub const PRESETS: [Resolution; 7] = [
        Resolution::Fhd1080,
        Resolution::Qhd1440,
        Resolution::Uhd4k,
        Resolution::Uhd5k,
        Resolution::Uhd8k,
        Resolution::Ultrawide1440,
        Resolution::Portrait4k,
    ];

    /// `(width, height)` in pixels (custom sizes clamped to the limits).
    pub fn size(self) -> (u32, u32) {
        match self {
            Resolution::Fhd1080 => (1920, 1080),
            Resolution::Qhd1440 => (2560, 1440),
            Resolution::Uhd4k => (3840, 2160),
            Resolution::Uhd5k => (5120, 2880),
            Resolution::Uhd8k => (7680, 4320),
            Resolution::Ultrawide1440 => (3440, 1440),
            Resolution::Portrait4k => (2160, 3840),
            Resolution::Custom { width, height } => (width.clamp(MIN_SIDE, MAX_SIDE), height.clamp(MIN_SIDE, MAX_SIDE)),
        }
    }

    /// Stable id: `1080p`, `1440p`, `4k`, `5k`, `8k`, `uw1440`, `portrait4k`,
    /// or `WxH` for custom sizes.
    pub fn id(self) -> String {
        match self {
            Resolution::Fhd1080 => "1080p".into(),
            Resolution::Qhd1440 => "1440p".into(),
            Resolution::Uhd4k => "4k".into(),
            Resolution::Uhd5k => "5k".into(),
            Resolution::Uhd8k => "8k".into(),
            Resolution::Ultrawide1440 => "uw1440".into(),
            Resolution::Portrait4k => "portrait4k".into(),
            Resolution::Custom { width, height } => format!("{width}x{height}"),
        }
    }

    /// Parses an [`Resolution::id`] (custom `WxH` included; a `WxH` equal
    /// to a preset returns the preset).
    pub fn from_id(id: &str) -> Option<Resolution> {
        let id = id.trim().to_ascii_lowercase();
        if let Some(p) = Resolution::PRESETS.into_iter().find(|p| p.id() == id) {
            return Some(p);
        }
        let (w, h) = id.split_once('x')?;
        let (width, height) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
        if !(MIN_SIDE..=MAX_SIDE).contains(&width) || !(MIN_SIDE..=MAX_SIDE).contains(&height) {
            return None;
        }
        Some(Resolution::PRESETS.into_iter().find(|p| p.size() == (width, height)).unwrap_or(Resolution::Custom { width, height }))
    }

    /// English label, marked for translation (custom sizes: `"Custom"`;
    /// show [`Resolution::size`] next to it).
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Fhd1080 => tk("Full HD (1080p)"),
            Resolution::Qhd1440 => tk("2K (1440p)"),
            Resolution::Uhd4k => tk("4K"),
            Resolution::Uhd5k => tk("5K"),
            Resolution::Uhd8k => tk("8K"),
            Resolution::Ultrawide1440 => tk("Ultrawide (3440 × 1440)"),
            Resolution::Portrait4k => tk("Portrait 4K"),
            Resolution::Custom { .. } => tk("Custom"),
        }
    }

    /// This size's height class fitted to a display's aspect ratio: the
    /// short side of the class (1080, 1440, 2160, …) stays, the long side
    /// follows `width : height`, rounded to even pixels. Portrait displays
    /// keep the class's short side as their width.
    pub fn fit_aspect(self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = self.size();
        let short = w.min(h) as f64;
        if width == 0 || height == 0 {
            return (w, h);
        }
        let even = |v: f64| (((v / 2.0).round() as u32) * 2).clamp(MIN_SIDE, MAX_SIDE);
        if width >= height {
            (even(short * width as f64 / height as f64), even(short))
        } else {
            (even(short), even(short * height as f64 / width as f64))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for r in Resolution::PRESETS {
            assert_eq!(Resolution::from_id(&r.id()), Some(r));
            assert!(!r.label().is_empty());
        }
        assert_eq!(Resolution::from_id("3000x2000"), Some(Resolution::Custom { width: 3000, height: 2000 }));
        assert_eq!(Resolution::from_id("3840X2160"), Some(Resolution::Uhd4k));
        assert_eq!(Resolution::from_id("8x8"), None);
        assert_eq!(Resolution::from_id("wide"), None);
        assert_eq!(Resolution::Uhd8k.size(), (7680, 4320));
        assert_eq!(Resolution::Custom { width: 1, height: 99999 }.size(), (MIN_SIDE, MAX_SIDE));
    }

    #[test]
    fn fits_display_aspect() {
        assert_eq!(Resolution::Uhd4k.fit_aspect(1920, 1080), (3840, 2160));
        assert_eq!(Resolution::Qhd1440.fit_aspect(3440, 1440), (3440, 1440));
        assert_eq!(Resolution::Uhd4k.fit_aspect(1920, 1200), (3456, 2160));
        assert_eq!(Resolution::Fhd1080.fit_aspect(1080, 1920), (1080, 1920));
        assert_eq!(Resolution::Uhd4k.fit_aspect(0, 0), (3840, 2160));
    }

    #[test]
    fn serde_names() {
        let t = toml::to_string(&std::collections::BTreeMap::from([("r", Resolution::Uhd4k)])).unwrap();
        assert!(t.contains("uhd4k"), "{t}");
        let c: std::collections::BTreeMap<String, Resolution> = toml::from_str("r = { custom = { width = 100, height = 50 } }").unwrap();
        assert_eq!(c["r"], Resolution::Custom { width: 100, height: 50 });
    }
}
