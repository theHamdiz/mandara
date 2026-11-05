//! Diagnose the selected icon theme without changing the user's choice.

use mandara_config::ShellConfig;
use mandara_i18n::{t, tf};
use mandara_ui::icon_theme;

use super::{Health, Roots};

pub(super) fn check(roots: &Roots) -> Health {
    let config = std::fs::read_to_string(roots.config_file()).ok().and_then(|text| toml::from_str::<ShellConfig>(&text).ok()).unwrap_or_default();
    selected(config.appearance.icon_theme.as_deref())
}

pub(super) fn selected(theme: Option<&str>) -> Health {
    inspect(theme, icon_theme::desktop_icon_theme(), icon_theme::theme_installed)
}

fn inspect(selected: Option<&str>, desktop: Option<String>, installed: impl FnOnce(&str) -> bool) -> Health {
    let theme = selected.filter(|name| !name.trim().is_empty()).or(desktop.as_deref());
    match theme {
        Some(theme) if installed(theme) => Health::Ok(tf("Icon theme {theme} is installed", &[("theme", &theme)])),
        Some(theme) => Health::Warn(tf("Icon theme {theme} is missing; install it or choose another in Themes → Icons", &[("theme", &theme)])),
        None => Health::Ok(t("No icon theme is selected; standard fallbacks are in use").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_directions_use_the_navigation_labels_in_every_language() {
        for lang in mandara_i18n::Lang::ALL {
            let message = mandara_i18n::t_in(lang, "Icon theme {theme} is missing; install it or choose another in Themes → Icons");
            for label in ["Themes", "Icons"] {
                assert!(message.contains(mandara_i18n::t_in(lang, label)), "{}: {label}", lang.code());
            }
        }
    }

    #[test]
    fn an_explicit_choice_takes_precedence_and_is_not_silently_replaced() {
        let health = inspect(Some("Missing"), Some("Installed".into()), |name| name == "Installed");
        assert!(matches!(health, Health::Warn(detail) if detail.contains("Missing")));
        assert!(inspect(None, Some("Installed".into()), |name| name == "Installed").is_ok());
        assert!(inspect(Some(""), None, |_| panic!("no theme selected")).is_ok());
    }

    #[test]
    fn a_missing_theme_check_and_repair_preserve_the_config() {
        let (_dir, roots) = super::super::tests::sandbox();
        let text = "[appearance]\nicon_theme = 'mandara-test-missing-theme-7c8946'\n";
        super::super::tests::write(&roots.config_file(), text);
        assert!(matches!(check(&roots), Health::Warn(detail) if detail.contains("mandara-test-missing-theme-7c8946")));
        assert!(super::super::fix_in(&roots, super::super::CheckId::Icons, &[], &|_, _| {}).is_err());
        assert_eq!(std::fs::read_to_string(roots.config_file()).unwrap(), text);
    }
}
