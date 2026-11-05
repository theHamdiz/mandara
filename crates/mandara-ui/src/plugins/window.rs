//! Active window plugin: the focused window's app and title.

use std::cell::RefCell;

use gpui::{IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_plugins::{CommandSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use serde_json::json;

use super::domain_view;
use crate::hub::Domains;
use crate::hub::hub;
use crate::kit::{ellipsize, tokens};

pub const ID: &str = "active-window";

/// Widest the app + title may grow in the bar (`max_width`), and the
/// range the setting is clamped to.
pub const MAX_W_PX: f32 = 420.0;
pub const MAX_W_RANGE: (f64, f64) = (120.0, 720.0);
/// Title characters shown before ellipsizing.
const TITLE_CHARS: usize = 48;

/// The labels last derived for `(app id, title)`: resolving the app name
/// reads desktop entries, so it runs once per focus / title change.
#[derive(Default)]
struct Labels {
    app_id: String,
    title: String,
    app: SharedString,
    shown_title: SharedString,
}

/// The app's name: its desktop entry's `Name` (so web apps show e.g.
/// "WhatsApp Web", not `chrome-<id>-Default`), else the app id made
/// friendly (`com.anthropic.Claude` → `Claude`).
pub fn app_name(app_id: &str) -> String {
    crate::apps::name_for_app_id(app_id).unwrap_or_else(|| friendly_id(app_id))
}

/// `com.anthropic.Claude` → `Claude`, `org.gnome.text-editor` → `Text editor`.
pub fn friendly_id(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect::<String>().replace(['-', '_'], " "),
        None => String::new(),
    }
}

/// What the bar item shows (`[plugins.settings.active-window] show`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Show {
    AppAndTitle,
    App,
    Title,
}

impl Show {
    fn parse(s: &str) -> Self {
        match s {
            "app" => Show::App,
            "title" => Show::Title,
            _ => Show::AppAndTitle,
        }
    }

    /// `(app name, title)` to draw; a title-only item falls back to the
    /// app's name for windows without a title of their own.
    fn parts(self, app: &SharedString, title: &SharedString) -> (Option<SharedString>, Option<SharedString>) {
        match self {
            Show::AppAndTitle => (Some(app.clone()), Some(title.clone()).filter(|t| !t.is_empty())),
            Show::App => (Some(app.clone()), None),
            Show::Title if title.is_empty() => (Some(app.clone()), None),
            Show::Title => (None, Some(title.clone())),
        }
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Active window"), mandara_i18n::tk("Focused window title"),Some(Zone::Left)), |window, cx, _| {
        let labels = RefCell::new(Labels::default());
        let bar = domain_view(Domains::FOCUS,
            None,
            move |palette, _, cx| {
                let Some(client) = hub(cx).read(cx).focused.as_ref() else { return div().into_any_element() };
                let mut l = labels.borrow_mut();
                if l.app_id != client.app_id || l.title != client.title || l.app.is_empty() {
                    let app = app_name(&client.app_id);
                    l.shown_title = if client.title.is_empty() || client.title == app { SharedString::default() } else { ellipsize(&client.title, TITLE_CHARS).into() };
                    l.app = app.into();
                    l.app_id.clone_from(&client.app_id);
                    l.title.clone_from(&client.title);
                }
                let cfg = crate::state::config(cx).read(cx).snapshot();
                let show = Show::parse(&cfg.plugins.setting(ID, "show", String::from("app-and-title")));
                let max_w = cfg.plugins.setting(ID, "max_width", f64::from(MAX_W_PX)).clamp(MAX_W_RANGE.0, MAX_W_RANGE.1) as f32;
                let (app, title) = show.parts(&l.app, &l.shown_title);
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::SM))
                    .max_w(px(max_w))
                    // Shrinks into the room its bar zone leaves (the title
                    // ellipsizes) rather than overlapping the next zone.
                    .min_w(px(0.0))
                    .when_some(app, |el, app| el.child(div().flex_none().font_weight(gpui::FontWeight::MEDIUM).child(app)))
                    .when_some(title, |el, title| el.child(div().min_w(px(0.0)).truncate().text_color(palette.on_surface_variant.hsla()).child(title)))
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance { bar: Some(bar), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new("window active", mandara_i18n::tk("The focused window"), |_, cx| {
        Ok(serde_json::to_value(&hub(cx).read(cx).focused).unwrap_or(json!(null)))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_item_shows_the_app_the_title_or_both() {
        let (app, title): (SharedString, SharedString) = ("Zed".into(), "main.rs".into());
        assert_eq!(Show::parse("app-and-title").parts(&app, &title), (Some(app.clone()), Some(title.clone())));
        assert_eq!(Show::parse("app").parts(&app, &title), (Some(app.clone()), None));
        assert_eq!(Show::parse("title").parts(&app, &title), (None, Some(title.clone())));
        assert_eq!(Show::Title.parts(&app, &SharedString::default()), (Some(app.clone()), None), "an untitled window still shows something");
        assert_eq!(Show::parse("nonsense"), Show::AppAndTitle);
    }

    #[test]
    fn friendly_app_names() {
        assert_eq!(friendly_id("com.anthropic.Claude"), "Claude");
        assert_eq!(friendly_id("dev.zed.Zed"), "Zed");
        assert_eq!(friendly_id("firefox"), "Firefox");
        assert_eq!(friendly_id("org.gnome.text-editor"), "Text editor");
        assert_eq!(friendly_id(""), "");
    }
}
