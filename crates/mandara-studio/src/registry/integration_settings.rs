//! Settings for integrations that load only when their panel is visible.

use mandara_i18n::tk;
use mandara_ui::plugins::feeds;
use mandara_ui::plugins::home_assistant as home;
use mandara_ui::plugins::phone_connect as phone;
use serde_json::Value;

use super::{Control, Section, Setting, sd};

pub(super) fn entries() -> Vec<Setting> {
    vec![
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/provider", tk("Provider"), tk("Text is sent only when you press Translate"), Control::Choice(const { &[("deepl-free", tk("DeepL Free")), ("deepl-pro", tk("DeepL Pro")), ("libretranslate", tk("LibreTranslate"))] }), Value::from("deepl-free")),
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/server", tk("Server address"), tk("Your LibreTranslate server, including a reverse proxy path if needed"), Control::Text { placeholder: "http://localhost:5000" }, Value::from("")),
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/use_api_key", tk("Use an API key"), tk("Enable this when your LibreTranslate server requires a key; save it in the keyring below"), Control::Toggle, Value::from(false)),
        sd(Section::Plugins, tk("Translator"), "/plugins/settings/translator/target", tk("Default target language"), tk("Initial target language code; choose from the provider's language list in the panel"), Control::Text { placeholder: "EN" }, Value::from(mandara_ui::plugins::translator::DEFAULT_TARGET)),
        sd(Section::Plugins, tk("Feeds"), "/plugins/settings/feeds/subscriptions", tk("Feed addresses"), tk("Separate RSS, Atom or JSON feed addresses with spaces; up to 16 subscriptions"), Control::Text { placeholder: "https://example.org/feed.xml" }, Value::from("")),
        sd(Section::Plugins, tk("Feeds"), "/plugins/settings/feeds/refresh_seconds", tk("Refresh interval"), tk("Only the selected feed refreshes while its panel is open"), Control::Slider { min: feeds::MIN_REFRESH_SECS as f64, max: feeds::MAX_REFRESH_SECS as f64, step: feeds::MIN_REFRESH_SECS as f64, unit: " s" }, Value::from(feeds::DEFAULT_REFRESH_SECS)),
        sd(Section::Plugins, tk("Home Assistant"), "/plugins/settings/home-assistant/server", tk("Server address"), tk("Your Home Assistant address, including a reverse proxy path if needed"), Control::Text { placeholder: "http://192.168.1.10:8123" }, Value::from("")),
        sd(Section::Plugins, tk("Home Assistant"), "/plugins/settings/home-assistant/entities", tk("Favourite entities"), tk("Separate up to 24 entity ids with spaces; empty shows the first available entities"), Control::Text { placeholder: "light.studio sensor.temperature scene.evening" }, Value::from("")),
        sd(Section::Plugins, tk("Home Assistant"), "/plugins/settings/home-assistant/refresh_seconds", tk("Refresh interval"), tk("Read your home while the panel is open; actions also refresh its state"), Control::Slider { min: home::MIN_REFRESH_SECS as f64, max: home::MAX_REFRESH_SECS as f64, step: home::MIN_REFRESH_SECS as f64, unit: " s" }, Value::from(home::DEFAULT_REFRESH_SECS)),
        sd(Section::Plugins, tk("Phone Connect"), "/plugins/settings/phone-connect/refresh_seconds", tk("Refresh interval"), tk("Check KDE Connect while the phone panel is open"), Control::Slider { min: phone::MIN_REFRESH_SECS as f64, max: phone::MAX_REFRESH_SECS as f64, step: phone::MIN_REFRESH_SECS as f64, unit: " s" }, Value::from(phone::DEFAULT_REFRESH_SECS)),
        sd(Section::Plugins, tk("Phone Connect"), "/plugins/settings/phone-connect/show_unpaired", tk("Show available devices"), tk("Include nearby devices that can be paired; pairing always needs your confirmation"), Control::Toggle, Value::from(true)),
    ]
}

/// Explain controls that do not apply to the selected integration provider.
pub(super) fn locked(path: &str, cfg: &mandara_config::ShellConfig) -> Option<&'static str> {
    (matches!(path, "/plugins/settings/translator/server" | "/plugins/settings/translator/use_api_key")
        && mandara_ui::plugins::translator::settings(cfg).connection.provider != mandara_services::translator::Provider::LibreTranslate)
        .then_some(tk("DeepL uses its own server and requires an API key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_specific_controls_explain_when_they_do_not_apply() {
        let mut cfg = mandara_config::ShellConfig::default();
        assert!(locked("/plugins/settings/translator/server", &cfg).is_some());
        assert!(locked("/plugins/settings/translator/provider", &cfg).is_none());
        cfg.plugins.settings.entry("translator".into()).or_default().insert("provider".into(), "libretranslate".into());
        assert!(locked("/plugins/settings/translator/server", &cfg).is_none());
        assert!(locked("/plugins/settings/translator/use_api_key", &cfg).is_none());
    }
}
