//! Explicit translation with a provider chosen in Studio. Text stays in
//! memory and is sent only by a button press or an IPC command.

mod panel;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{IntoElement, px, size};
use mandara_config::ShellConfig;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::credentials::{Access, Key, Keyring, Secret, Store as _};
use mandara_services::translator::{self as service, Backend as _, Provider};

use super::{model_view as fn_view, panel_fx::readable};
use crate::icons::icon;

pub const ID: &str = "translator";
/// Space for editable source text, a language picker and a readable result.
const PANEL_W: f32 = 480.0;
const PANEL_H: f32 = 600.0;
const BAR_ICON_PX: f32 = 16.0;
pub const DEFAULT_TARGET: &str = "EN";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub connection: service::Settings,
    pub use_api_key: bool,
    pub target: String,
}

/// Read provider, server and target choices without opening the keyring.
pub fn settings(cfg: &ShellConfig) -> Preferences {
    let provider = match cfg.plugins.setting_str(ID, "provider").unwrap_or("") {
        "deepl-pro" => Provider::DeepLPro,
        "libretranslate" => Provider::LibreTranslate,
        _ => Provider::DeepLFree,
    };
    Preferences {
        connection: service::Settings { provider, server: cfg.plugins.setting_str(ID, "server").unwrap_or("").trim().trim_end_matches('/').into() },
        use_api_key: provider != Provider::LibreTranslate || cfg.plugins.setting(ID, "use_api_key", false),
        target: cfg.plugins.setting_str(ID, "target").unwrap_or(DEFAULT_TARGET).trim().to_string(),
    }
}

impl Preferences {
    /// Anonymous local servers never contact the desktop keyring.
    fn credential_key(&self) -> Result<Option<Key>, String> {
        self.use_api_key.then(|| self.connection.credential_key()).transpose()
    }
}

/// Return no keyring identity for an explicitly anonymous provider.
pub fn credential_key(cfg: &ShellConfig) -> Result<Option<Key>, String> { settings(cfg).credential_key() }

/// Read a key only when this provider requires one, scoped to its current server.
fn api_key(preferences: &Preferences) -> Result<Option<Secret>, String> {
    match preferences.credential_key()? {
        Some(key) => Keyring.read(&key, Access::Quiet)?.ok_or_else(|| t("Add your translation API key in Studio").to_string()).map(Some),
        None => Ok(None),
    }
}

/// Read this provider’s key quietly and submit only the supplied text.
fn translate(preferences: &Preferences, text: &str, source: Option<&str>, target: &str) -> Result<service::Translation, String> {
    let key = api_key(preferences)?;
    service::Http::new().translate(&preferences.connection, &service::Request { text, source, target }, key.as_ref())
}

/// Register explicit translation commands and a panel that submits only on request.
pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Translator"), tk("Translate text with your chosen provider"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        let state = panel::State::new(cx);
        let bar = fn_view(None, |p, _, _| icon("translate", crate::style::bar_icon(BAR_ICON_PX), readable(p.primary, p)).into_any_element(), window, cx);
        let context = context.clone();
        let view = fn_view(None, move |p, window, cx| panel::render(&state, &context, p, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view, size: Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H)))) }), ..PluginInstance::empty() }
    });
    registry.command(ID, CommandSpec::new_output("translate text", tk("Translate text into a language with your configured provider"), |args, cx| {
        let preferences = settings(&crate::state::config(cx).read(cx).snapshot());
        let target = args.str(0)?.to_owned();
        let text = args.rest(1);
        Ok(crate::powerctl::pending(move || translate(&preferences, &text, None, &target).and_then(|result| serde_json::to_value(result).map_err(|error| error.to_string()))))
    }).arg(ArgSpec::required("target", ArgKind::String, tk("Target language code"))).arg(ArgSpec::required("text", ArgKind::String, tk("Text to translate"))).example("translate text ar Good morning"));
    registry.command(ID, CommandSpec::new_output("translate languages", tk("List the configured translation provider's languages"), |_, cx| {
        let preferences = settings(&crate::state::config(cx).read(cx).snapshot());
        Ok(crate::powerctl::pending(move || {
            let key = api_key(&preferences)?;
            service::Http::new().languages(&preferences.connection, key.as_ref()).and_then(|languages| serde_json::to_value(languages).map_err(|error| error.to_string()))
        }))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mistyped text settings keep the previous provider, server and target defaults.
    #[test]
    fn borrowed_preferences_keep_invalid_value_fallbacks() {
        let mut config = ShellConfig::default();
        let values = config.plugins.settings.entry(ID.into()).or_default();
        for key in ["provider", "server", "target"] { values.insert(key.into(), toml::Value::Boolean(true)); }
        let preferences = settings(&config);
        assert_eq!(preferences.connection.provider, Provider::DeepLFree);
        assert!(preferences.connection.server.is_empty());
        assert_eq!(preferences.target, DEFAULT_TARGET);
    }

    #[test]
    fn an_anonymous_server_does_not_need_a_keyring_and_keys_follow_the_provider() {
        let mut cfg = ShellConfig::default();
        let values = cfg.plugins.settings.entry(ID.into()).or_default();
        values.insert("provider".into(), "libretranslate".into());
        values.insert("server".into(), " http://localhost:5000/proxy/ ".into());
        assert_eq!(credential_key(&cfg).unwrap(), None);
        cfg.plugins.settings.get_mut(ID).unwrap().insert("use_api_key".into(), true.into());
        assert_eq!(credential_key(&cfg).unwrap().unwrap().server, "http://localhost:5000/proxy");
        cfg.plugins.settings.get_mut(ID).unwrap().insert("provider".into(), "deepl-free".into());
        assert_eq!(credential_key(&cfg).unwrap().unwrap().server, "https://api-free.deepl.com");
    }
}
