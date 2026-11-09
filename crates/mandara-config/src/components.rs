//! Component designs (`[style]`): how the shell's own controls look.
//!
//! Every component has one design setting whose first design, *Classic*,
//! is exactly the look Mandara shipped before designs existed, and is the
//! default, so a config without these keys draws as it always did. Studio
//! → Themes → **Components** shows each design as a live preview, and its
//! **presets** ([`ComponentPreset`]) set every component at once.
//!
//! The designs only change shape, outline, elevation and motion: the roles
//! (a filled button stays the primary action, a tonal one secondary) and
//! the palette tokens stay the same.

use serde::{Deserialize, Serialize};

use crate::desktop::{SliderDesign, StyleConfig};

/// Buttons (`style.button_design`): the shell's text buttons in panels,
/// the sidebar and the main menu, Studio's buttons and the round icon
/// buttons.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ButtonDesign {
    /// Fully rounded pills; icon buttons are circles.
    #[default]
    Classic,
    /// Rounded squares (Material's square button shape).
    Square,
    /// Round at rest; the corners square off while you press, then spring
    /// back (Material 3 Expressive's shape morph).
    Expressive,
    /// Corners cut at 45 degrees.
    Cut,
    /// Soft raised buttons that lift on hover and settle when pressed.
    Elevated,
}

impl ButtonDesign {
    pub const ALL: [ButtonDesign; 5] = [ButtonDesign::Classic, ButtonDesign::Square, ButtonDesign::Expressive, ButtonDesign::Cut, ButtonDesign::Elevated];
}

/// Switches, check boxes and radio buttons (`style.toggle_design`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToggleDesign {
    /// The gradient pill switch with a glowing knob, and each surface's own
    /// check boxes and radio buttons.
    #[default]
    Classic,
    /// Material 3: an outlined track whose small thumb grows and carries a
    /// check when on, square-ish check boxes and ringed radio buttons.
    Material,
    /// Material 3 Expressive: the thumb stretches while it travels, a check
    /// box rounds into a circle as it fills, the radio dot springs in.
    Expressive,
    /// Soft and round: circular check boxes and a filled thumb.
    Round,
}

impl ToggleDesign {
    pub const ALL: [ToggleDesign; 4] = [ToggleDesign::Classic, ToggleDesign::Material, ToggleDesign::Expressive, ToggleDesign::Round];
}

/// Text fields and search fields (`style.input_design`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputDesign {
    /// A rounded tonal field with an accent edge while focused.
    #[default]
    Classic,
    /// Material 3 filled: a tonal field with square bottom corners and an
    /// underline that thickens in the accent colour while focused.
    Filled,
    /// Material 3 outlined: an outline that thickens while focused, with
    /// the hint floating up into it once you type.
    Outlined,
    /// A fully rounded field with an accent ring while focused.
    Pill,
}

impl InputDesign {
    pub const ALL: [InputDesign; 4] = [InputDesign::Classic, InputDesign::Filled, InputDesign::Outlined, InputDesign::Pill];
}

/// Chips and segmented choices (`style.chip_design`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChipDesign {
    /// Options as pills on one tonal track; filled chips.
    #[default]
    Classic,
    /// Material 3: outlined segmented buttons with a check on the chosen
    /// one, outlined filter chips.
    Material,
    /// Material 3 Expressive: separate connected buttons with small inner
    /// corners; the chosen one rounds fully.
    Connected,
}

impl ChipDesign {
    pub const ALL: [ChipDesign; 3] = [ChipDesign::Classic, ChipDesign::Material, ChipDesign::Connected];
}

/// Progress bars and meters (`style.progress_design`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressDesign {
    /// A rounded track with the fill over it.
    #[default]
    Classic,
    /// Material 3: the fill, a small gap, then the track ending in a stop
    /// dot.
    Material,
    /// Material 3 Expressive: the fill drawn as a gentle wave.
    Wavy,
    /// Ten rounded steps.
    Segmented,
}

impl ProgressDesign {
    pub const ALL: [ProgressDesign; 4] = [ProgressDesign::Classic, ProgressDesign::Material, ProgressDesign::Wavy, ProgressDesign::Segmented];
}

/// The corner family of cards and panels' inner surfaces
/// (`style.surface_corners`); `style.radius_scale` still sets how large
/// the corners are.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceCorners {
    /// Every corner rounded alike.
    #[default]
    Rounded,
    /// Material 3 Expressive's mixed corners: the leading top and trailing
    /// bottom corners large, the other two small (mirrored right to left).
    Asymmetric,
    /// Small, crisp corners.
    Square,
    /// Continuous superellipse corners, smoother than a circle's.
    Squircle,
    /// Corners cut at 45 degrees.
    Cut,
}

impl SurfaceCorners {
    pub const ALL: [SurfaceCorners; 5] = [SurfaceCorners::Rounded, SurfaceCorners::Asymmetric, SurfaceCorners::Square, SurfaceCorners::Squircle, SurfaceCorners::Cut];
}

/// How cards stand off their panel (`style.card_style`). Panels and
/// popups keep `style.shadows` and `style.borders`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CardStyle {
    /// A tonal surface with a soft specular rim.
    #[default]
    Tonal,
    /// Raised on a soft shadow (none while `style.shadows` is off).
    Raised,
    /// A flat surface with a fine outline.
    Outlined,
}

impl CardStyle {
    pub const ALL: [CardStyle; 3] = [CardStyle::Tonal, CardStyle::Raised, CardStyle::Outlined];
}

/// A one-click set of component designs (Studio → Themes → Components).
/// Presets set only the design settings: corner size, shadows, panel edges
/// and the slider handle stay yours.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ComponentPreset {
    Classic,
    MaterialYou,
    Expressive,
    Minimal,
    BrutalistSoft,
}

/// The design settings a preset writes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ComponentDesigns {
    pub slider: SliderDesign,
    pub button: ButtonDesign,
    pub toggle: ToggleDesign,
    pub input: InputDesign,
    pub chip: ChipDesign,
    pub progress: ProgressDesign,
    pub corners: SurfaceCorners,
    pub card: CardStyle,
}

impl ComponentPreset {
    pub const ALL: [ComponentPreset; 5] = [ComponentPreset::Classic, ComponentPreset::MaterialYou, ComponentPreset::Expressive, ComponentPreset::Minimal, ComponentPreset::BrutalistSoft];

    /// The designs this preset picks.
    pub const fn designs(self) -> ComponentDesigns {
        use ComponentPreset as P;
        match self {
            P::Classic => ComponentDesigns {
                slider: SliderDesign::Classic,
                button: ButtonDesign::Classic,
                toggle: ToggleDesign::Classic,
                input: InputDesign::Classic,
                chip: ChipDesign::Classic,
                progress: ProgressDesign::Classic,
                corners: SurfaceCorners::Rounded,
                card: CardStyle::Tonal,
            },
            P::MaterialYou => ComponentDesigns {
                slider: SliderDesign::Continuous,
                button: ButtonDesign::Classic,
                toggle: ToggleDesign::Material,
                input: InputDesign::Filled,
                chip: ChipDesign::Material,
                progress: ProgressDesign::Material,
                corners: SurfaceCorners::Rounded,
                card: CardStyle::Tonal,
            },
            P::Expressive => ComponentDesigns {
                slider: SliderDesign::Wavy,
                button: ButtonDesign::Expressive,
                toggle: ToggleDesign::Expressive,
                input: InputDesign::Pill,
                chip: ChipDesign::Connected,
                progress: ProgressDesign::Wavy,
                corners: SurfaceCorners::Asymmetric,
                card: CardStyle::Raised,
            },
            P::Minimal => ComponentDesigns {
                slider: SliderDesign::Continuous,
                button: ButtonDesign::Square,
                toggle: ToggleDesign::Round,
                input: InputDesign::Outlined,
                chip: ChipDesign::Material,
                progress: ProgressDesign::Material,
                corners: SurfaceCorners::Squircle,
                card: CardStyle::Outlined,
            },
            P::BrutalistSoft => ComponentDesigns {
                slider: SliderDesign::Segmented,
                button: ButtonDesign::Cut,
                toggle: ToggleDesign::Material,
                input: InputDesign::Outlined,
                chip: ChipDesign::Connected,
                progress: ProgressDesign::Segmented,
                corners: SurfaceCorners::Cut,
                card: CardStyle::Outlined,
            },
        }
    }

    /// Writes this preset's designs into `style`, nothing else.
    pub fn apply(self, style: &mut StyleConfig) {
        let d = self.designs();
        style.slider_design = d.slider;
        style.button_design = d.button;
        style.toggle_design = d.toggle;
        style.input_design = d.input;
        style.chip_design = d.chip;
        style.progress_design = d.progress;
        style.surface_corners = d.corners;
        style.card_style = d.card;
    }

    /// The preset `style` matches exactly, if any.
    pub fn current(style: &StyleConfig) -> Option<ComponentPreset> {
        Self::ALL.into_iter().find(|p| ComponentDesigns::of(style) == p.designs())
    }
}

impl ComponentDesigns {
    /// The designs set in `style`.
    pub const fn of(style: &StyleConfig) -> Self {
        Self {
            slider: style.slider_design,
            button: style.button_design,
            toggle: style.toggle_design,
            input: style.input_design,
            chip: style.chip_design,
            progress: style.progress_design,
            corners: style.surface_corners,
            card: style.card_style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config from before component designs (v0.23.0, with the slider
    /// design) loads every component in its classic look, keeps what it
    /// had, and reads back unchanged once saved.
    #[test]
    fn older_configs_keep_every_classic_design() {
        let fresh: crate::ShellConfig = toml::from_str("").unwrap();
        assert_eq!(ComponentDesigns::of(&fresh.style), ComponentPreset::Classic.designs());
        assert_eq!(ComponentPreset::current(&fresh.style), Some(ComponentPreset::Classic));
        let old: crate::ShellConfig = toml::from_str("[style]\nradius_scale = 0.8\nshadows = false\nslider_design = \"wavy\"").unwrap();
        assert_eq!((old.style.radius_scale, old.style.shadows, old.style.slider_design), (0.8, false, SliderDesign::Wavy));
        let d = ComponentDesigns::of(&old.style);
        assert_eq!((d.button, d.toggle, d.input, d.chip, d.progress, d.corners, d.card), (ButtonDesign::Classic, ToggleDesign::Classic, InputDesign::Classic, ChipDesign::Classic, ProgressDesign::Classic, SurfaceCorners::Rounded, CardStyle::Tonal));
        assert_eq!(ComponentPreset::current(&old.style), None, "a lone wavy slider is no preset");
        let saved = toml::to_string(&old.style).unwrap();
        let back: StyleConfig = toml::from_str(&saved).unwrap();
        assert_eq!(back, old.style);
    }

    /// Every design reads and writes as its kebab-case name, and an unknown
    /// one is reported rather than guessed.
    #[test]
    fn every_design_round_trips_by_name() {
        fn check<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug + Copy>(all: &[T], names: &[&str]) {
            assert_eq!(all.len(), names.len());
            for (design, name) in all.iter().zip(names) {
                assert_eq!(serde_json::to_value(design).unwrap(), serde_json::Value::from(*name));
                assert_eq!(serde_json::from_value::<T>(serde_json::Value::from(*name)).unwrap(), *design);
            }
            assert!(serde_json::from_value::<T>(serde_json::Value::from("zigzag")).is_err());
        }
        check(&ButtonDesign::ALL, &["classic", "square", "expressive", "cut", "elevated"]);
        check(&ToggleDesign::ALL, &["classic", "material", "expressive", "round"]);
        check(&InputDesign::ALL, &["classic", "filled", "outlined", "pill"]);
        check(&ChipDesign::ALL, &["classic", "material", "connected"]);
        check(&ProgressDesign::ALL, &["classic", "material", "wavy", "segmented"]);
        check(&SurfaceCorners::ALL, &["rounded", "asymmetric", "square", "squircle", "cut"]);
        check(&CardStyle::ALL, &["tonal", "raised", "outlined"]);
        // Each design's default is its first, classic one.
        assert_eq!((ButtonDesign::default(), ToggleDesign::default(), InputDesign::default()), (ButtonDesign::ALL[0], ToggleDesign::ALL[0], InputDesign::ALL[0]));
        assert_eq!((ChipDesign::default(), ProgressDesign::default(), SurfaceCorners::default(), CardStyle::default()), (ChipDesign::ALL[0], ProgressDesign::ALL[0], SurfaceCorners::ALL[0], CardStyle::ALL[0]));
    }

    /// Presets are distinct, each is recognised once applied, and applying
    /// one leaves every other style setting alone.
    #[test]
    fn presets_set_only_the_designs_and_are_recognised() {
        for (i, a) in ComponentPreset::ALL.iter().enumerate() {
            for b in &ComponentPreset::ALL[i + 1..] {
                assert_ne!(a.designs(), b.designs(), "{a:?} and {b:?} differ");
            }
            let mut style = StyleConfig { radius_scale: 1.4, shadows: false, borders: false, backdrop_dim: 0.3, ..StyleConfig::default() };
            style.slider_handle = crate::desktop::SliderHandle::None;
            let before = style.clone();
            a.apply(&mut style);
            assert_eq!(ComponentPreset::current(&style), Some(*a));
            assert_eq!((style.radius_scale, style.shadows, style.borders, style.backdrop_dim, style.slider_handle), (before.radius_scale, before.shadows, before.borders, before.backdrop_dim, before.slider_handle), "{a:?}: only the designs change");
        }
    }
}
