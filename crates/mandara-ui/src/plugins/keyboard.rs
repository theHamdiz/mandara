//! Keyboard layout plugin: the active layout as a short code (EN, AR, …)
//! in the bar; click switches to the next layout (MangoWM).

use std::cell::RefCell;

use gpui::{IntoElement, ParentElement, Styled, div, px};
use mandara_core::ToGpui;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry, Zone};

use super::domain_view;
use crate::hub::Domains;
use crate::hub::hub;

pub const ID: &str = "keyboard-layout";

/// Layout badge geometry.
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_H_PX: f32 = 18.0;
const BADGE_RADIUS_PX: f32 = 6.0;
const BADGE_ALPHA: f32 = 0.08;
const BADGE_TEXT_PX: f32 = 10.5;
mod label;
pub use label::short_code;
use label::LayoutLabel;

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Keyboard layout"), mandara_i18n::tk("The active keyboard layout; click to switch"), Some(Zone::Right)), |window, cx, _| {
        // (layout name, its short code), derived once per layout change.
        let code: RefCell<LayoutLabel> = RefCell::default();
        let bar = domain_view(Domains::KEYBOARD,
            None,
            move |palette, _, cx| {
                let layout = hub(cx).read(cx).keyboard_layout.as_str();
                if layout.is_empty() {
                    return div().into_any_element();
                }
                let mut code = code.borrow_mut();
                // A label set in Studio → Input (EN, ع…) wins over the derived code.
                let config = crate::state::config(cx);
                let config = config.read(cx);
                let custom = config.get().compositor.input.keyboard.label_for_reported(layout);
                let shown = code.label(layout, custom);
                div()
                    .px(px(BADGE_PAD_X_PX))
                    .h(px(BADGE_H_PX))
                    .rounded(px(crate::style::radius(BADGE_RADIUS_PX)))
                    .flex()
                    .items_center()
                    .bg(palette.on_surface.with_alpha(BADGE_ALPHA).hsla())
                    .text_size(px(BADGE_TEXT_PX))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(shown)
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            on_click: Some(std::rc::Rc::new(|_, cx| {
                if let Some(b) = hub(cx).read(cx).compositor.clone()
                    && let Err(err) = b.switch_keyboard_layout() {
                        tracing::warn!("switch keyboard layout: {err:#}");
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
