//! The styles and their parameter tables.

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use crate::params::{ParamSpec, count, pct, spec};

/// A wallpaper style. Ids are stable (they live in saved recipes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Style {
    /// Soft mesh gradient of a few drifting color points.
    Mesh,
    /// Layered mountain ridges in atmospheric haze, with a sun or moon.
    Landscape,
    /// The sea from the Alexandria Corniche: horizon, waves, sun glitter.
    Corniche,
    /// Curtains of aurora light over a starry night and dark hills.
    Aurora,
    /// Sand dunes raked by low sunlight.
    Dunes,
    /// Silk threads following a flow field.
    Flow,
    /// A topographic map: tinted terrain and contour lines.
    Contours,
    /// Out-of-focus lights in layered depth.
    Bokeh,
    /// A faceted, lit triangulation.
    LowPoly,
    /// Glossy liquid blobs merging with smooth minimums.
    Liquid,
    /// Stacked glassy wave bands.
    Waves,
    /// Frosted glass prisms with refraction and caustics.
    Glass,
}

impl Style {
    /// Every style, in gallery order.
    pub const ALL: [Style; 12] = [
        Style::Mesh,
        Style::Landscape,
        Style::Corniche,
        Style::Aurora,
        Style::Dunes,
        Style::Flow,
        Style::Contours,
        Style::Bokeh,
        Style::LowPoly,
        Style::Liquid,
        Style::Waves,
        Style::Glass,
    ];

    /// Stable kebab-case id.
    pub fn id(self) -> &'static str {
        match self {
            Style::Mesh => "mesh",
            Style::Landscape => "landscape",
            Style::Corniche => "corniche",
            Style::Aurora => "aurora",
            Style::Dunes => "dunes",
            Style::Flow => "flow",
            Style::Contours => "contours",
            Style::Bokeh => "bokeh",
            Style::LowPoly => "low-poly",
            Style::Liquid => "liquid",
            Style::Waves => "waves",
            Style::Glass => "glass",
        }
    }

    pub fn from_id(id: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.id() == id)
    }

    /// English label, marked for translation (`mandara_i18n::tr` it).
    pub fn label(self) -> &'static str {
        match self {
            Style::Mesh => tk("Mesh gradient"),
            Style::Landscape => tk("Landscape"),
            Style::Corniche => tk("Corniche"),
            Style::Aurora => tk("Aurora"),
            Style::Dunes => tk("Dunes"),
            Style::Flow => tk("Silk flow"),
            Style::Contours => tk("Contours"),
            Style::Bokeh => tk("Bokeh"),
            Style::LowPoly => tk("Low poly"),
            Style::Liquid => tk("Liquid"),
            Style::Waves => tk("Waves"),
            Style::Glass => tk("Glass"),
        }
    }

    /// One-line description, marked for translation.
    pub fn description(self) -> &'static str {
        match self {
            Style::Mesh => tk("Soft color fields blending into one another"),
            Style::Landscape => tk("Mountain ridges fading into haze under a glowing sun"),
            Style::Corniche => tk("The Mediterranean horizon with waves and sun glitter"),
            Style::Aurora => tk("Curtains of northern light over a starry night"),
            Style::Dunes => tk("Sand dunes lit by a low sun"),
            Style::Flow => tk("Silk threads drifting along a flow field"),
            Style::Contours => tk("A topographic map with tinted terrain"),
            Style::Bokeh => tk("Out-of-focus lights at different depths"),
            Style::LowPoly => tk("A faceted, lit triangle mesh"),
            Style::Liquid => tk("Glossy liquid blobs melting together"),
            Style::Waves => tk("Layered glassy waves"),
            Style::Glass => tk("Frosted glass prisms with light caustics"),
        }
    }

    /// This style's own parameters (see also [`crate::COMMON_PARAMS`]).
    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            Style::Mesh => MESH,
            Style::Landscape => LANDSCAPE,
            Style::Corniche => CORNICHE,
            Style::Aurora => AURORA,
            Style::Dunes => DUNES,
            Style::Flow => FLOW,
            Style::Contours => CONTOURS,
            Style::Bokeh => BOKEH,
            Style::LowPoly => LOW_POLY,
            Style::Liquid => LIQUID,
            Style::Waves => WAVES,
            Style::Glass => GLASS,
        }
    }

    /// Style parameter `key`, then common parameter `key`.
    pub fn spec(self, key: &str) -> Option<&'static ParamSpec> {
        self.params().iter().chain(crate::params::COMMON_PARAMS).find(|p| p.key == key)
    }

    /// A per-style salt so the same seed lays out differently per style.
    pub(crate) fn salt(self) -> u64 {
        self.id().bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3))
    }
}

impl std::fmt::Display for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

// Shared specs (same key and label in several styles).
const fn horizon(min: f32, max: f32, default: f32, nice: (f32, f32)) -> ParamSpec {
    spec("horizon", tk("Horizon height"), min, max, 1.0, default, "%", nice)
}
const SUN_X: ParamSpec = spec("sun_x", tk("Sun position"), -100.0, 100.0, 1.0, 25.0, "%", (-55.0, 55.0));
const SUN_SIZE: ParamSpec = pct("sun_size", tk("Sun size"), 35.0, (25.0, 50.0));
const GLOW: ParamSpec = pct("glow", tk("Glow"), 60.0, (45.0, 75.0));
const HAZE: ParamSpec = pct("haze", tk("Atmospheric haze"), 55.0, (40.0, 70.0));
const STARS: ParamSpec = pct("stars", tk("Stars"), 35.0, (20.0, 55.0));

const MESH: &[ParamSpec] = &[
    count("points", tk("Color points"), 3.0, 9.0, 6.0, (5.0, 8.0)),
    pct("spread", tk("Blend size"), 42.0, (32.0, 55.0)),
    pct("warp", tk("Organic warp"), 45.0, (25.0, 65.0)),
    spec("swirl", tk("Swirl"), -100.0, 100.0, 1.0, 0.0, "%", (-35.0, 35.0)),
    pct("depth", tk("Shadow depth"), 50.0, (35.0, 65.0)),
    pct("sheen", tk("Silk sheen"), 35.0, (15.0, 50.0)),
];

const LANDSCAPE: &[ParamSpec] = &[
    count("layers", tk("Ridge layers"), 2.0, 8.0, 5.0, (4.0, 7.0)),
    pct("ruggedness", tk("Ruggedness"), 50.0, (30.0, 70.0)),
    horizon(30.0, 85.0, 58.0, (50.0, 66.0)),
    SUN_X,
    pct("sun_height", tk("Sun height"), 30.0, (12.0, 45.0)),
    SUN_SIZE,
    GLOW,
    HAZE,
    pct("mist", tk("Valley mist"), 35.0, (20.0, 55.0)),
    STARS,
];

const CORNICHE: &[ParamSpec] = &[
    horizon(30.0, 80.0, 52.0, (45.0, 60.0)),
    SUN_X,
    pct("sun_height", tk("Sun height"), 14.0, (6.0, 28.0)),
    SUN_SIZE,
    GLOW,
    pct("clouds", tk("Clouds"), 40.0, (20.0, 60.0)),
    pct("waves", tk("Wave detail"), 55.0, (40.0, 70.0)),
    pct("glitter", tk("Sun glitter"), 60.0, (45.0, 80.0)),
    pct("coast", tk("Distant coast"), 50.0, (0.0, 70.0)),
];

const AURORA: &[ParamSpec] = &[
    count("curtains", tk("Curtains"), 1.0, 5.0, 3.0, (2.0, 4.0)),
    pct("height", tk("Curtain height"), 55.0, (45.0, 70.0)),
    pct("intensity", tk("Intensity"), 70.0, (55.0, 85.0)),
    pct("rays", tk("Ray detail"), 60.0, (45.0, 75.0)),
    pct("waviness", tk("Waviness"), 50.0, (30.0, 70.0)),
    STARS,
    horizon(60.0, 98.0, 84.0, (78.0, 90.0)),
];

const DUNES: &[ParamSpec] = &[
    pct("scale", tk("Dune size"), 55.0, (40.0, 70.0)),
    pct("sharpness", tk("Crest sharpness"), 60.0, (45.0, 75.0)),
    pct("ripples", tk("Wind ripples"), 40.0, (25.0, 60.0)),
    horizon(15.0, 60.0, 34.0, (26.0, 42.0)),
    SUN_X,
    pct("sun_height", tk("Sun height"), 18.0, (8.0, 30.0)),
    GLOW,
    HAZE,
];

const FLOW: &[ParamSpec] = &[
    pct("density", tk("Line density"), 55.0, (40.0, 70.0)),
    pct("thickness", tk("Line weight"), 35.0, (20.0, 50.0)),
    pct("turbulence", tk("Turbulence"), 50.0, (30.0, 70.0)),
    pct("scale", tk("Field scale"), 45.0, (30.0, 60.0)),
    GLOW,
    pct("fade", tk("Thread fade"), 55.0, (40.0, 70.0)),
    pct("color_spread", tk("Color spread"), 60.0, (40.0, 80.0)),
];

const CONTOURS: &[ParamSpec] = &[
    pct("scale", tk("Terrain scale"), 45.0, (30.0, 60.0)),
    count("levels", tk("Contour lines"), 4.0, 60.0, 22.0, (14.0, 30.0)),
    pct("line_width", tk("Line weight"), 35.0, (20.0, 50.0)),
    pct("relief", tk("Hillshade"), 55.0, (40.0, 70.0)),
    pct("fill", tk("Elevation tint"), 70.0, (50.0, 85.0)),
    pct("index", tk("Index lines"), 60.0, (40.0, 80.0)),
    pct("warp", tk("Organic warp"), 40.0, (25.0, 55.0)),
];

const BOKEH: &[ParamSpec] = &[
    pct("density", tk("Density"), 50.0, (35.0, 65.0)),
    pct("size", tk("Size"), 50.0, (35.0, 65.0)),
    pct("blur", tk("Defocus"), 60.0, (45.0, 75.0)),
    pct("rim", tk("Rim brightness"), 40.0, (25.0, 55.0)),
    pct("brightness", tk("Light intensity"), 60.0, (45.0, 75.0)),
    pct("band", tk("Diagonal band"), 50.0, (30.0, 70.0)),
];

const LOW_POLY: &[ParamSpec] = &[
    count("cells", tk("Facets"), 4.0, 40.0, 12.0, (8.0, 18.0)),
    pct("jitter", tk("Irregularity"), 70.0, (55.0, 85.0)),
    pct("relief", tk("Relief"), 55.0, (40.0, 70.0)),
    pct("edges", tk("Edge lines"), 12.0, (0.0, 25.0)),
    pct("variation", tk("Color variation"), 35.0, (20.0, 50.0)),
    spec("gradient_angle", tk("Gradient angle"), 0.0, 360.0, 1.0, 60.0, "°", (20.0, 160.0)),
];

const LIQUID: &[ParamSpec] = &[
    count("blobs", tk("Blobs"), 2.0, 16.0, 7.0, (6.0, 9.0)),
    pct("size", tk("Blob size"), 50.0, (40.0, 60.0)),
    pct("merge", tk("Merge"), 65.0, (50.0, 80.0)),
    pct("gloss", tk("Gloss"), 60.0, (45.0, 75.0)),
    pct("depth", tk("Depth"), 55.0, (40.0, 70.0)),
    pct("shadow", tk("Shadow"), 45.0, (30.0, 60.0)),
];

const WAVES: &[ParamSpec] = &[
    count("bands", tk("Bands"), 3.0, 12.0, 6.0, (5.0, 8.0)),
    pct("amplitude", tk("Amplitude"), 45.0, (30.0, 60.0)),
    pct("frequency", tk("Frequency"), 40.0, (25.0, 55.0)),
    pct("rim", tk("Glass rim"), 60.0, (45.0, 75.0)),
    pct("shadow", tk("Shadow"), 50.0, (35.0, 65.0)),
    spec("tilt", tk("Tilt"), -100.0, 100.0, 1.0, 20.0, "%", (-35.0, 35.0)),
    pct("translucency", tk("Translucency"), 35.0, (20.0, 50.0)),
];

const GLASS: &[ParamSpec] = &[
    count("shapes", tk("Shapes"), 1.0, 8.0, 4.0, (3.0, 5.0)),
    pct("size", tk("Shape size"), 50.0, (40.0, 65.0)),
    pct("roundness", tk("Roundness"), 70.0, (50.0, 90.0)),
    pct("refraction", tk("Refraction"), 50.0, (35.0, 65.0)),
    pct("dispersion", tk("Prism dispersion"), 45.0, (30.0, 60.0)),
    pct("frost", tk("Frost"), 55.0, (40.0, 70.0)),
    pct("rim", tk("Glass rim"), 60.0, (45.0, 75.0)),
    pct("caustics", tk("Caustics"), 45.0, (30.0, 60.0)),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::COMMON_PARAMS;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for s in Style::ALL {
            assert_eq!(Style::from_id(s.id()), Some(s));
            assert!(seen.insert(s.id()));
            assert!(s.id().chars().all(|c| c.is_ascii_lowercase() || c == '-'));
            let toml = toml::to_string(&std::collections::BTreeMap::from([("s", s)])).unwrap();
            assert!(toml.contains(&format!("\"{}\"", s.id())), "{toml}");
        }
        assert_eq!(Style::from_id("nope"), None);
    }

    #[test]
    fn param_tables_are_sane() {
        for s in Style::ALL {
            let mut keys = std::collections::BTreeSet::new();
            assert!(s.params().len() >= 5, "{s}");
            for p in s.params().iter().chain(COMMON_PARAMS) {
                assert!(keys.insert(p.key), "{s}: duplicate {}", p.key);
                assert!(p.min < p.max && p.step > 0.0, "{s}.{}", p.key);
                assert!((p.min..=p.max).contains(&p.default), "{s}.{} default", p.key);
                assert!(p.min <= p.nice.0 && p.nice.0 <= p.nice.1 && p.nice.1 <= p.max, "{s}.{} nice", p.key);
                assert_eq!(p.clamp(p.default), p.default, "{s}.{} default off-step", p.key);
                assert!(!p.label.is_empty());
            }
        }
    }
}
