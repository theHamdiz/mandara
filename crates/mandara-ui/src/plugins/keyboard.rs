//! Keyboard layout plugin: the active layout as a short code (EN, AR, …)
//! in the bar; click switches to the next layout (MangoWM).

use std::cell::RefCell;

use gpui::{IntoElement, ParentElement, SharedString, Styled, div, px};
use mandara_core::ToGpui;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry, Zone};

use super::fn_view;
use crate::hub::hub;

pub const ID: &str = "keyboard-layout";

/// Layout badge geometry.
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_H_PX: f32 = 18.0;
const BADGE_RADIUS_PX: f32 = 6.0;
const BADGE_ALPHA: f32 = 0.08;
const BADGE_TEXT_PX: f32 = 10.5;
/// Letters kept from an unknown layout's name.
const CODE_LEN: usize = 2;

/// "English (US)" → "EN", "Arabic" → "AR"; else the first two letters.
pub fn short_code(layout: &str) -> String {
    const NAMES: &[(&str, &str)] = &[
        ("english", "EN"),
        ("arabic", "AR"),
        ("french", "FR"),
        ("german", "DE"),
        ("spanish", "ES"),
        ("russian", "RU"),
        ("turkish", "TR"),
        ("japanese", "JA"),
        ("chinese", "ZH"),
        ("hindi", "HI"),
        ("persian", "FA"),
        ("hebrew", "HE"),
        ("italian", "IT"),
        ("portuguese", "PT"),
        ("dutch", "NL"),
        ("polish", "PL"),
        ("korean", "KO"),
        ("urdu", "UR"),
        ("greek", "EL"),
        ("ukrainian", "UK"),
    ];
    let lower = layout.to_lowercase();
    NAMES
        .iter()
        .find(|(name, _)| lower.starts_with(name))
        .map(|(_, code)| code.to_string())
        .unwrap_or_else(|| layout.chars().filter(|c| c.is_alphabetic()).take(CODE_LEN).collect::<String>().to_uppercase())
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Keyboard layout"), mandara_i18n::tk("The active keyboard layout; click to switch"), Some(Zone::Right)), |window, cx, _| {
        // (layout name, its short code), derived once per layout change.
        let code: RefCell<(String, SharedString)> = RefCell::default();
        let bar = fn_view(
            None,
            move |palette, _, cx| {
                let layout = hub(cx).read(cx).keyboard_layout.as_str();
                if layout.is_empty() {
                    return div().into_any_element();
                }
                let mut code = code.borrow_mut();
                // A label set in Studio → Input (EN, ع…) wins over the derived code.
                let custom = crate::state::config(cx).read(cx).get().compositor.input.keyboard.label_for_reported(layout).map(str::to_string);
                let key = format!("{layout}\u{1f}{}", custom.as_deref().unwrap_or_default());
                if code.0 != key {
                    code.1 = custom.unwrap_or_else(|| short_code(layout)).into();
                    code.0 = key;
                }
                div()
                    .px(px(BADGE_PAD_X_PX))
                    .h(px(BADGE_H_PX))
                    .rounded(px(crate::style::radius(BADGE_RADIUS_PX)))
                    .flex()
                    .items_center()
                    .bg(palette.on_surface.with_alpha(BADGE_ALPHA).hsla())
                    .text_size(px(BADGE_TEXT_PX))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(code.1.clone())
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            on_click: Some(std::rc::Rc::new(|_, cx| {
                if let Some(b) = hub(cx).read(cx).compositor.clone() {
                    if let Err(err) = b.switch_keyboard_layout() {
                        tracing::warn!("switch keyboard layout: {err:#}");
                    }
                }
            })),
            ..PluginInstance::empty()
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_codes() {
        assert_eq!(short_code("English (US)"), "EN");
        assert_eq!(short_code("Arabic (Egypt)"), "AR");
        assert_eq!(short_code("Esperanto"), "ES");
    }
}
