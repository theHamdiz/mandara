//! Active window plugin: the focused window's app and title.

use std::cell::RefCell;

use gpui::{IntoElement, ParentElement, SharedString, Styled, div, px};
use mandara_core::ToGpui;
use mandara_plugins::{CommandSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use serde_json::json;

use super::fn_view;
use crate::hub::hub;
use crate::kit::{ellipsize, tokens};

pub const ID: &str = "active-window";

/// Widest the app + title may grow in the bar.
const MAX_W_PX: f32 = 420.0;
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

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Active window"), mandara_i18n::tk("Focused window title"),Some(Zone::Left)), |window, cx, _| {
        let labels = RefCell::new(Labels::default());
        let bar = fn_view(
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
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::SM))
                    .max_w(px(MAX_W_PX))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(l.app.clone()))
                    .child(div().truncate().text_color(palette.on_surface_variant.hsla()).child(l.shown_title.clone()))
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
    fn friendly_app_names() {
        assert_eq!(friendly_id("com.anthropic.Claude"), "Claude");
        assert_eq!(friendly_id("dev.zed.Zed"), "Zed");
        assert_eq!(friendly_id("firefox"), "Firefox");
        assert_eq!(friendly_id("org.gnome.text-editor"), "Text editor");
        assert_eq!(friendly_id(""), "");
    }
}
