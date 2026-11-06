//! A native home favourites panel. Names, values and availability come
//! from the configured server; actions always wait for its response.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, px, size};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::credentials::{Access, Key, Keyring, Provider, Secret, Store as _};
use mandara_services::home_assistant::{self as ha, Backend as _, Entity};
use mandara_theme::MandaraPalette;

use super::model_view as fn_view;
use super::integration::{self, Data};
use super::panel_fx::{EXPIRY_MARGIN, glass_card, header, pill_button, readable, redraw_in};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};

pub const ID: &str = "home-assistant";
/// A small control grid leaves room for entity names and sensor units.
const PANEL_W: f32 = 440.0;
const PANEL_H: f32 = 480.0;
const BAR_ICON_PX: f32 = 16.0;
const ENTITY_ICON_PX: f32 = 24.0;
const TILE_MIN_W: f32 = 180.0;
pub const DEFAULT_REFRESH_SECS: i64 = 30;
pub const MIN_REFRESH_SECS: i64 = 5;
pub const MAX_REFRESH_SECS: i64 = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub server: String,
    pub favourites: Vec<String>,
}

/// Read the configured server and favourite order without contacting the server.
pub fn settings(cfg: &ShellConfig) -> Settings {
    Settings {
        server: cfg.plugins.setting_str(ID, "server").unwrap_or("").trim().trim_end_matches('/').into(),
        favourites: cfg.plugins.setting_str(ID, "entities").unwrap_or("").split_whitespace().map(str::to_owned).collect(),
    }
}

/// Validate the server base before using it to identify a saved token.
pub fn credential_key(settings: &Settings) -> Result<Key, String> {
    let server = mandara_services::web_api::endpoint(&settings.server, "")?;
    Ok(Key { provider: Provider::HomeAssistant, server: server.trim_end_matches('/').into() })
}

/// Read only the token scoped to the currently configured Home Assistant server.
fn token(settings: &Settings) -> Result<Secret, String> {
    Keyring.read(&credential_key(settings)?, Access::Quiet)?.ok_or_else(|| t("Add your Home Assistant token in Studio").to_string())
}

fn refresh_interval(cfg: &ShellConfig) -> Duration {
    Duration::from_secs(cfg.plugins.setting(ID, "refresh_seconds", DEFAULT_REFRESH_SECS).clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS) as u64)
}

type HomeData = Rc<RefCell<Data<Settings, Vec<Entity>>>>;
thread_local! { static DATA: RefCell<Option<HomeData>> = const { RefCell::new(None) }; }
fn data() -> HomeData { DATA.with(|slot| slot.borrow_mut().get_or_insert_with(Rc::default).clone()) }

/// Discard a disabled server's data without creating a cold cache.
pub(super) fn stop() {
    if let Some(data) = DATA.with(|slot| slot.borrow_mut().take()) { data.borrow_mut().clear(); }
}

/// Refresh the selected server outside the UI thread, coalescing pending reads.
fn refresh(settings: Settings, force: bool, cfg: &ShellConfig, cx: &mut App) {
    if !super::lifecycle::is_on(ID, cx) { return; }
    let address = settings.clone();
    integration::load(&data(), settings, force, refresh_interval(cfg), move |_| {
        let key = token(&address)?;
        ha::Http::new().states(&address.server, &key, &address.favourites)
    }, cx);
}

type HomeAction = Rc<RefCell<integration::Action<Settings>>>;
thread_local! { static ACTIONS: HomeAction = Rc::default(); }

/// Run one explicit entity action and refresh state after it completes.
fn act(settings: Settings, entity: String, action: ha::Action, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let actions = ACTIONS.with(Rc::clone);
    let address = settings.clone();
    let views = data().borrow().views.clone();
    integration::act(&actions, settings.clone(), views, move || {
        let key = token(&address)?;
        ha::Http::new().act(&address.server, &key, &entity, action).map(|()| serde_json::json!({"ok":true}))
    }, move |cx| {
        let cfg = crate::state::config(cx).read(cx).snapshot();
        if super::lifecycle::is_on(ID, cx) && self::settings(&cfg) == settings {
            data().borrow_mut().invalidate();
            refresh(settings, true, &cfg, cx);
        }
    }, cx)
}

/// Hide activation timestamps while preserving real sensor states and units.
fn state_label(entity: &Entity) -> String {
    match entity.state.as_str() {
        "unavailable" => t("Unavailable").into(),
        "unknown" => t("Unknown").into(),
        "on" => t("On").into(),
        "off" => t("Off").into(),
        _ if matches!(entity.domain(), "scene" | "button" | "input_button") => t("Available").into(),
        state => match &entity.unit { Some(unit) => format!("{state} {unit}"), None => state.into() },
    }
}

/// Show the selected server’s state and offer only supported, available actions.
fn panel(context: &PanelCx, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let settings = settings(&cfg);
    let data = data();
    data.borrow_mut().select(settings.clone());
    data.borrow().views.track(window);
    if context.progress() > 0.0 && !settings.server.is_empty() {
        refresh(settings.clone(), false, &cfg, cx);
        redraw_in(refresh_interval(&cfg) + EXPIRY_MARGIN, window, cx);
    }
    let model = data.borrow();
    let actions = ACTIONS.with(Rc::clone);
    let actions = actions.borrow();
    let accent = readable(p.primary, p);
    let setup = pill_button("home-settings", Some("sliders"), t("Settings"), accent, p,
        |_, cx| crate::state::open_studio_at(Some("plugins/home-assistant/settings".into()), cx)).focusable().tab_index(0).into_any_element();
    let mut column = div().size_full().p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
        .child(header("home", accent, t("Home Assistant"), t("A little closer to home"), p, Some(setup)));
    let error = model.error.clone().or_else(|| actions.error_for(&settings).map(str::to_owned));
    if let Some(error) = error {
        column = column.child(glass_card(p).p(px(tokens::SM)).text_size(px(text::BODY_S)).text_color(readable(p.warning_amber, p).hsla()).child(crate::bidi::paragraph(error)));
    }
    let mut content = div().id("home-entities").flex_1().min_h_0().overflow_y_scroll().row().flex_wrap().items_start().content_start().gap(px(tokens::SM));
    match model.value.as_deref() {
        Some(entities) if !entities.is_empty() => for entity in entities.iter().take(ha::MAX_FAVOURITES) {
            let glyph = match entity.domain() { "light" => "sun", "sensor" => "gauge", "fan" => "wind", "scene" => "tiles", "script" => "terminal", _ => "power" };
            let state = state_label(entity);
            let tone = if entity.state == "on" { accent } else { p.on_surface_variant };
            let mut card = glass_card(p).min_w(px(TILE_MIN_W)).flex_1().p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM))
                .child(div().row().items_center().gap(px(tokens::SM)).child(div().flex_none().child(icon(glyph, ENTITY_ICON_PX, tone)))
                    .child(div().flex_1().min_w_0().text_right().truncate().text_size(px(text::BODY)).font_features(crate::fonts::tabular_nums()).child(state)))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).line_clamp(2).child(crate::bidi::paragraph(entity.name.clone())));
            if let Some(action) = entity.action() {
                let settings = settings.clone();
                let id = entity.id.clone();
                if actions.busy() { card = card.child(div().text_size(px(text::LABEL)).child(t("Working…"))); }
                else { card = card.child(pill_button(gpui::SharedString::from(format!("home-{}", entity.id)), None,
                    if action == ha::Action::Activate { t("Run") } else if entity.state == "on" { t("Turn off") } else { t("Turn on") }, accent, p,
                    move |_, cx| { drop(act(settings.clone(), id.clone(), action, cx)); }).focusable().tab_index(0)); }
            }
            content = content.child(card);
        },
        _ => { content = content.child(div().size_full().row().flex_col().items_center().justify_center().gap(px(tokens::SM))
            .child(icon("home", ENTITY_ICON_PX, p.on_surface_variant)).child(if settings.server.is_empty() { t("Connect your home in Studio") } else if model.busy() { t("Reading your home…") } else { t("No entities to show") })); }
    }
    let refresh_settings = settings.clone();
    column.child(content).child(pill_button("home-refresh", Some("refresh"), t("Refresh"), accent, p, move |_, cx| {
        let cfg = crate::state::config(cx).read(cx).snapshot();
        refresh(refresh_settings.clone(), true, &cfg, cx);
    }).focusable().tab_index(0)).into_any_element()
}

/// Register lazy views and explicit commands without contacting a home server.
pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Home Assistant"), tk("Your home favourites, sensors and scenes"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        let bar = fn_view(None, |p, _, _| icon("home", crate::style::bar_icon(BAR_ICON_PX), readable(p.primary, p)).into_any_element(), window, cx);
        let context = context.clone();
        let view = fn_view(None, move |p, window, cx| panel(&context, p, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view, size: Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H)))) }), ..PluginInstance::empty() }
    });
    registry.command(ID, CommandSpec::new_output("home states", tk("Read the configured Home Assistant favourites"), |_, cx| {
        let settings = settings(&crate::state::config(cx).read(cx).snapshot());
        Ok(crate::powerctl::pending(move || { let key = token(&settings)?; ha::Http::new().states(&settings.server, &key, &settings.favourites).and_then(|entities| serde_json::to_value(entities).map_err(|error| error.to_string())) }))
    }));
    registry.command(ID, CommandSpec::new_output("home action", tk("Toggle a home control or run a scene, script or button"), |args, cx| {
        let settings = settings(&crate::state::config(cx).read(cx).snapshot());
        let action = match args.str(1)? { "toggle" => ha::Action::Toggle, _ => ha::Action::Activate };
        Ok(mandara_plugins::Output::Pending(act(settings, args.str(0)?.into(), action, cx)))
    }).arg(ArgSpec::required("entity", ArgKind::String, tk("Home Assistant entity id"))).arg(ArgSpec::required("action", ArgKind::String, tk("Action")).choices(&["toggle", "activate"])).example("home action light.studio toggle"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invalid text fields still resolve to an empty, unconfigured server.
    #[test]
    fn borrowed_settings_keep_mistyped_text_fields_empty() {
        let mut config = ShellConfig::default();
        let values = config.plugins.settings.entry(ID.into()).or_default();
        values.insert("server".into(), toml::Value::Boolean(true));
        values.insert("entities".into(), toml::Value::Integer(3));
        assert_eq!(settings(&config), Settings { server: String::new(), favourites: Vec::new() });
    }

    #[test]
    fn disabling_releases_a_snapshot_even_if_a_request_still_holds_the_data_slot() {
        stop();
        assert!(DATA.with(|slot| slot.borrow().is_none()));
        let held = data();
        let snapshot = std::sync::Arc::new(Vec::<Entity>::with_capacity(64));
        let weak = std::sync::Arc::downgrade(&snapshot);
        held.borrow_mut().value = Some(snapshot);
        stop();
        assert!(weak.upgrade().is_none());
        assert!(held.borrow().value.is_none());
        assert!(DATA.with(|slot| slot.borrow().is_none()));
    }

    #[test]
    fn stateless_actions_do_not_expose_timestamps_as_their_current_state() {
        let _english = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
        let scene = Entity { id: "scene.evening".into(), name: "Evening".into(), state: "2026-11-04T16:00:00Z".into(), unit: None };
        assert_eq!(state_label(&scene), "Available");
        assert_eq!(state_label(&Entity { state: "unknown".into(), ..scene.clone() }), "Unknown");
        assert_eq!(state_label(&Entity { state: "unavailable".into(), ..scene.clone() }), "Unavailable");
        assert_eq!(state_label(&Entity { id: "sensor.temperature".into(), state: "23".into(), unit: Some("°C".into()), ..scene }), "23 °C");
    }

    #[test]
    fn home_settings_scope_the_key_and_follow_the_refresh_bounds() {
        let mut cfg = ShellConfig::default();
        let settings = cfg.plugins.settings.entry(ID.into()).or_default();
        settings.insert("server".into(), toml::Value::String(" http://home.local:8123/proxy/ ".into()));
        settings.insert("entities".into(), toml::Value::String("light.studio sensor.temperature".into()));
        settings.insert("refresh_seconds".into(), toml::Value::Integer(0));
        let read = self::settings(&cfg);
        assert_eq!(read.favourites, ["light.studio", "sensor.temperature"]);
        assert_eq!(credential_key(&read).unwrap().server, "http://home.local:8123/proxy");
        assert_eq!(refresh_interval(&cfg), Duration::from_secs(MIN_REFRESH_SECS as u64));
    }
}
