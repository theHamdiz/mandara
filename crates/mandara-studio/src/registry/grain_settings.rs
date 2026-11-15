//! Grain for Studio, cards, bars, the launcher and panels (`[grain.*]`,
//! [`mandara_config::grain::SurfaceGrain`]): one group per surface under
//! Themes → Components, each with the same bounded options. A surface's
//! numbers are locked until it has a style.

use mandara_config::ShellConfig;
use mandara_config::grain::{GrainStyle, ranges};
use mandara_i18n::tk;

use super::{Control, GRAIN_STYLES, GRAIN_TINTS, Section, Setting, s, span};

/// The groups, in the order they show.
pub(crate) const GROUPS: [&str; 5] = [STUDIO, CARDS, BARS, LAUNCHER, PANELS];
const STUDIO: &str = tk("Studio grain");
const CARDS: &str = tk("Card grain");
const BARS: &str = tk("Bar grain");
const LAUNCHER: &str = tk("Launcher grain");
const PANELS: &str = tk("Panel grain");

/// One surface's five rows.
macro_rules! surface {
    ($group:expr, $key:literal, $what:expr) => {
        [
            s(Section::Themes, $group, concat!("/grain/", $key, "/style"), tk("Grain"), $what, Control::Choice(GRAIN_STYLES)),
            s(Section::Themes, $group, concat!("/grain/", $key, "/amount"), tk("Grain strength"), tk("How visible the grain is; it always stays subtle"), span(ranges::AMOUNT, "")),
            s(Section::Themes, $group, concat!("/grain/", $key, "/scale"), tk("Grain size"), tk("Size of each speck"), span(ranges::SCALE, " px")),
            s(Section::Themes, $group, concat!("/grain/", $key, "/contrast"), tk("Grain contrast"), tk("Higher makes strong specks stand out from faint ones"), span(ranges::CONTRAST, "")),
            s(Section::Themes, $group, concat!("/grain/", $key, "/tint"), tk("Grain colour"), tk("Neutral specks, or specks in the hue of the theme's surfaces"), Control::Choice(GRAIN_TINTS)),
        ]
    };
}

pub(super) fn entries() -> impl Iterator<Item = Setting> {
    [
        surface!(STUDIO, "studio", tk("A fine texture behind Studio's pages")),
        surface!(CARDS, "cards", tk("A fine texture on cards and grouped lists, in the shell and in Studio")),
        surface!(BARS, "bars", tk("A fine texture on the bars and their capsules")),
        surface!(LAUNCHER, "launcher", tk("A fine texture on the launcher")),
        surface!(PANELS, "panels", tk("A fine texture on drop-down and plugin panels")),
    ]
    .into_iter()
    .flatten()
}

/// A surface's strength, size, contrast and colour wait for a style.
pub(super) fn locked(path: &str, cfg: &ShellConfig) -> Option<&'static str> {
    let rest = path.strip_prefix("/grain/")?;
    let (surface, field) = rest.split_once('/')?;
    if field == "style" {
        return None;
    }
    let g = &cfg.grain;
    let grain = match surface {
        "studio" => g.studio,
        "cards" => g.cards,
        "bars" => g.bars,
        "launcher" => g.launcher,
        "panels" => g.panels,
        _ => return None,
    };
    (grain.style == GrainStyle::None).then_some(tk("Pick a grain style first"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_has_the_same_five_bounded_rows() {
        let all: Vec<Setting> = entries().collect();
        assert_eq!(all.len(), GROUPS.len() * 5);
        for group in GROUPS {
            assert_eq!(all.iter().filter(|s| s.group == group).count(), 5, "{group}");
        }
        let cfg = ShellConfig::default();
        assert!(locked("/grain/bars/amount", &cfg).is_some(), "off: the numbers wait for a style");
        assert!(locked("/grain/bars/style", &cfg).is_none());
        let mut on = cfg.clone();
        on.grain.bars.style = GrainStyle::Film;
        assert!(locked("/grain/bars/amount", &on).is_none());
        assert!(locked("/grain/cards/amount", &on).is_some(), "each surface on its own");
    }
}
