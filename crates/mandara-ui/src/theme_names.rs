//! Theme labels follow the interface language; user titles stay as written.

use mandara_theme::catalog::{ThemeDef, ThemeOrigin};

/// Shared label policy for catalog entries and their cached menu previews.
pub trait ThemeName {
    fn name(&self) -> &str;
    fn is_builtin(&self) -> bool;

    /// Resolve the current language when drawing, without caching translated
    /// text or changing the theme's stable identifier.
    fn localized_name(&self) -> String {
        if self.is_builtin() { mandara_i18n::tr(self.name()) } else { self.name().to_owned() }
    }
}

impl ThemeName for ThemeDef {
    fn name(&self) -> &str { &self.name }
    fn is_builtin(&self) -> bool { matches!(self.origin, ThemeOrigin::Builtin) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_i18n::{Lang, with_language};

    /// A retained definition follows locale changes without losing its id.
    #[test]
    fn builtin_labels_follow_the_current_language() {
        let theme = mandara_theme::catalog::builtin().iter().find(|theme| theme.id == "mandara-corniche").unwrap();
        with_language(Some(Lang::Ar), || assert_eq!(theme.localized_name(), "كورنيش المندرة"));
        with_language(Some(Lang::En), || assert_eq!(theme.localized_name(), "Mandara Corniche"));
        assert_eq!(theme.id, "mandara-corniche");
        assert_eq!(theme.name, "Mandara Corniche");
    }

    /// An override's title is user content even when it matches a built-in key.
    #[test]
    fn user_overrides_keep_their_exact_titles() {
        let mut theme = mandara_theme::catalog::builtin()[0].clone();
        theme.origin = ThemeOrigin::User { path: "custom-theme.toml".into() };
        for name in ["Mandara Corniche", "My evening colors", "ألواني"] {
            theme.name = name.into();
            with_language(Some(Lang::Ar), || assert_eq!(theme.localized_name(), name));
        }
    }

    /// TOML-provided labels need pack entries just as Rust literals do.
    #[test]
    fn every_builtin_theme_name_is_present_in_every_language_pack() {
        for theme in mandara_theme::catalog::builtin() {
            for language in Lang::ALL.into_iter().filter(|language| *language != Lang::En) {
                assert!(mandara_i18n::lookup(language, &theme.name).is_some(), "{}: {}", language.code(), theme.name);
            }
        }
    }
}
