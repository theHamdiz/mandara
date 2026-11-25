//! Theme labels follow the interface language; user titles stay as written.

use mandara_theme::catalog::{ThemeDef, ThemeOrigin};

/// Shared label policy for catalog entries and their cached menu previews.
pub trait ThemeName {
    fn name(&self) -> &str;
    fn is_builtin(&self) -> bool;

    /// Resolve the current language when drawing, without caching translated
    /// text or changing the theme's stable identifier.
    fn localized_name(&self) -> String {
        localized_metadata(self.name(), self.is_builtin())
    }
}

/// Built-in descriptions are catalog text; user theme files retain their prose.
pub fn localized_description(theme: &ThemeDef) -> String {
    localized_metadata(&theme.description, theme.is_builtin())
}

/// Both fields follow the same origin policy, including copied built-in text.
fn localized_metadata(text: &str, builtin: bool) -> String {
    if builtin { mandara_i18n::tr(text) } else { text.to_owned() }
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
        with_language(Some(Lang::Ar), || {
            assert_eq!(theme.localized_name(), "كورنيش المندرة");
            assert_ne!(localized_description(theme), theme.description);
        });
        with_language(Some(Lang::En), || {
            assert_eq!(theme.localized_name(), "Mandara Corniche");
            assert_eq!(localized_description(theme), theme.description);
        });
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
            with_language(Some(Lang::Ar), || {
                assert_eq!(theme.localized_name(), name);
                assert_eq!(localized_description(&theme), theme.description);
            });
        }
    }

    /// TOML-provided labels need pack entries just as Rust literals do.
    #[test]
    fn every_builtin_theme_name_and_description_is_present_in_every_language_pack() {
        for theme in mandara_theme::catalog::builtin() {
            for language in Lang::ALL.into_iter().filter(|language| *language != Lang::En) {
                for text in [&theme.name, &theme.description] {
                    assert!(mandara_i18n::lookup(language, text).is_some(), "{}: {}", language.code(), text);
                }
            }
        }
    }
}
