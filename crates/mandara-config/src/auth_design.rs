//! Login and lock designs share their catalog and preset proportions.

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

/// Visual language of either authentication screen; each keeps its own selection.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthDesign {
    #[default]
    Mandara,
    MidnightCard,
    MaterialYou,
    Ghostline,
    ChromeHalo,
    Meridian,
}

/// Ready-made looks for the whole screen: `(id, English name, summary)`.
/// Applying one (`GreeterConfig::apply_look`) changes how the screen
/// looks and where things sit; who is listed, sessions, folders, language
/// and accessibility stay as they are.
pub const LOOKS: [(&str, &str, &str); 6] = [
    (
        "mandara",
        tk("Mandara"),
        tk("A big clock, your picture over a glass field, everything floating on the backdrop"),
    ),
    (
        "midnight-card",
        tk("Midnight card"),
        tk(
            "A quiet clock over one outlined card with the user, password and session, in midnight navy and citrine",
        ),
    ),
    (
        "material-you",
        tk("Material You"),
        tk(
            "Colours from your wallpaper, a big stacked clock and a tonal card with a scalloped avatar, chips and tonal buttons",
        ),
    ),
    (
        "ghostline",
        tk("Ghostline"),
        tk(
            "Graphite and ice-white hairlines: a wireframe sign-in panel on the left with live readouts, a terminal password line and a ruler clock, in cold cyan",
        ),
    ),
    (
        "chrome-halo",
        tk("Chrome Halo"),
        tk(
            "Deep ink and holographic light: a luminous ring around a light clock, and a machined sign-in module on the right with a pill button",
        ),
    ),
    (
        "meridian",
        tk("Meridian"),
        tk(
            "No boxes: your name and password ride on one hairline across a calm dawn field, over enormous whisper-thin numerals; light or dark",
        ),
    ),
];

/// Picker labels use the same order as the greeter catalog.
pub const CHOICES: [(&str, &str); 6] = [
    (LOOKS[0].0, LOOKS[0].1),
    (LOOKS[1].0, LOOKS[1].1),
    (LOOKS[2].0, LOOKS[2].1),
    (LOOKS[3].0, LOOKS[3].1),
    (LOOKS[4].0, LOOKS[4].1),
    (LOOKS[5].0, LOOKS[5].1),
];

/// Preset measurements in logical pixels at the login layout's reference height.
/// These are shared defaults, independent of the shell's active Look.
#[derive(Copy, Clone, Debug)]
pub struct DesignTokens {
    pub preset: &'static str,
    pub accent: &'static str,
    pub backdrop: &'static str,
    pub light: bool,
    pub card_width: f32,
    pub card_radius: f32,
    pub avatar: f32,
    pub name: f32,
    pub field_height: f32,
    pub clock: f32,
    pub clock_weight: u16,
}

impl AuthDesign {
    pub const ALL: [Self; 6] = [
        Self::Mandara,
        Self::MidnightCard,
        Self::MaterialYou,
        Self::Ghostline,
        Self::ChromeHalo,
        Self::Meridian,
    ];

    pub fn id(self) -> &'static str {
        LOOKS[Self::ALL
            .iter()
            .position(|design| *design == self)
            .expect("cataloged design")]
        .0
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|design| design.id() == id)
    }

    /// The original glass and wallpaper-derived Material design retain their own tokens.
    pub fn tokens(self) -> Option<DesignTokens> {
        let (
            preset,
            accent,
            backdrop,
            light,
            card_width,
            card_radius,
            avatar,
            name,
            field_height,
            clock,
            clock_weight,
        ) = match self {
            Self::Mandara | Self::MaterialYou => return None,
            Self::MidnightCard => (
                "night-citrine",
                "",
                "#070722",
                false,
                440.0,
                18.0,
                64.0,
                16.0,
                40.0,
                96.0,
                400,
            ),
            Self::Ghostline => (
                "vesper", "#5fd7ff", "#0b0d10", false, 420.0, 2.0, 52.0, 18.0, 46.0, 168.0, 200,
            ),
            Self::ChromeHalo => (
                "pharos-night",
                "",
                "#06070e",
                false,
                404.0,
                34.0,
                60.0,
                20.0,
                52.0,
                124.0,
                200,
            ),
            Self::Meridian => (
                "mandara-corniche",
                "#c0623a",
                "#f3efe6",
                true,
                440.0,
                0.0,
                crate::login_layout::AVATAR,
                40.0,
                48.0,
                150.0,
                100,
            ),
        };
        Some(DesignTokens {
            preset,
            accent,
            backdrop,
            light,
            card_width,
            card_radius,
            avatar,
            name,
            field_height,
            clock,
            clock_weight,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_design_names_and_summaries_are_translated_for_both_pickers() {
        for language in mandara_i18n::Lang::ALL
            .into_iter()
            .filter(|language| *language != mandara_i18n::Lang::En)
        {
            for &(_, name, summary) in &LOOKS {
                assert!(
                    mandara_i18n::lookup(language, name).is_some(),
                    "{language:?}: {name}"
                );
                assert!(
                    mandara_i18n::lookup(language, summary).is_some(),
                    "{language:?}: {summary}"
                );
            }
        }
    }

    #[test]
    fn every_login_design_roundtrips_as_a_lock_design() {
        for (design, &(id, _, _)) in AuthDesign::ALL.into_iter().zip(&LOOKS) {
            assert_eq!(design.id(), id);
            assert_eq!(AuthDesign::parse(id), Some(design));
            let mut cfg = crate::ShellConfig::default();
            cfg.lock.design = design;
            let encoded = toml::to_string(&cfg).unwrap();
            assert_eq!(
                toml::from_str::<crate::ShellConfig>(&encoded)
                    .unwrap()
                    .lock
                    .design,
                design
            );
            if let Some(tokens) = design.tokens() {
                assert!(mandara_theme::catalog::find(tokens.preset).is_some());
                assert!(mandara_theme::Color::from_hex(tokens.backdrop).is_ok());
                assert!(
                    tokens.accent.is_empty()
                        || mandara_theme::Color::from_hex(tokens.accent).is_ok()
                );
            }
        }
        assert_eq!(AuthDesign::parse("unknown"), None);
    }
}
