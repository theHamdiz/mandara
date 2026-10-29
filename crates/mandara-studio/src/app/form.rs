//! Small building blocks shared by the Network, Input, Applications and
//! Profiles pages: page-local text inputs (values kept outside the
//! config), background jobs, an on/off switch, a status note and a Wi-Fi
//! QR code.

use mandara_ui::bidi::Bidi as _;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AnyElement, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement, Styled, div, px};
use mandara_core::ToGpui;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;

use super::StudioView;
use crate::text_field::TextField;
use crate::tokens::*;

/// Page-local inputs by key: the field and its current text.
#[derive(Default)]
struct Inputs {
    fields: HashMap<String, (Entity<TextField>, String)>,
}
impl Global for Inputs {}

/// A text input whose value lives in Studio (not the config): read it with
/// [`value`], reset it with [`clear`].
pub(super) fn input(key: &str, placeholder: &'static str, masked: bool, cx: &mut Context<StudioView>) -> Entity<TextField> {
    if let Some((f, _)) = cx.default_global::<Inputs>().fields.get(key) {
        return f.clone();
    }
    let k = key.to_string();
    let field = cx.new(|cx| {
        let f = TextField::new(
            String::new(),
            placeholder,
            true,
            Rc::new(move |value, _, cx| {
                if let Some(entry) = cx.default_global::<Inputs>().fields.get_mut(&k) {
                    entry.1 = value;
                }
            }),
            cx,
        );
        if masked { f.masked() } else { f }
    });
    cx.default_global::<Inputs>().fields.insert(key.to_string(), (field.clone(), String::new()));
    field
}

/// The text typed into input `key` (trimmed).
pub(super) fn value(key: &str, cx: &App) -> String {
    cx.try_global::<Inputs>().and_then(|i| i.fields.get(key)).map(|(_, v)| v.trim().to_string()).unwrap_or_default()
}

/// Empties input `key`.
pub(super) fn clear(key: &str, cx: &mut App) {
    let field = cx.try_global::<Inputs>().and_then(|i| i.fields.get(key)).map(|(f, _)| f.clone());
    if let Some(f) = field {
        f.update(cx, |f, _| f.set(""));
    }
    if let Some(entry) = cx.default_global::<Inputs>().fields.get_mut(key) {
        entry.1.clear();
    }
}

/// Sets input `key`'s text (pre-filling an editor).
pub(super) fn fill(key: &str, text: &str, cx: &mut App) {
    let field = cx.try_global::<Inputs>().and_then(|i| i.fields.get(key)).map(|(f, _)| f.clone());
    if let Some(f) = field {
        f.update(cx, |f, _| f.set(text));
    }
    if let Some(entry) = cx.default_global::<Inputs>().fields.get_mut(key) {
        entry.1 = text.to_string();
    }
}

/// Runs blocking `work` on the background executor, then `done` on the UI
/// thread with its result.
pub(super) fn job<T: Send + 'static>(
    cx: &mut Context<StudioView>,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&mut StudioView, Result<T, String>, &mut Context<StudioView>) + 'static,
) {
    let task = cx.background_executor().spawn(async move { work() });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        let _ = this.update(cx, |view, cx| {
            done(view, result, cx);
            cx.notify();
        });
    })
    .detach();
}

/// Runs a shell command over IPC (`["bluetooth", "trust", "on", addr]`)
/// and hands back its JSON; an error when the shell isn't running.
pub(super) fn shell(args: Vec<String>, cx: &mut Context<StudioView>, done: impl FnOnce(&mut StudioView, Result<serde_json::Value, String>, &mut Context<StudioView>) + 'static) {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let task = crate::perf::query(&words, cx);
    cx.spawn(async move |this, cx| {
        let result = match task {
            Some(task) => task.await,
            None => Err(mandara_i18n::t("Studio isn't connected to the shell").to_string()),
        };
        let _ = this.update(cx, |view, cx| {
            done(view, result, cx);
            cx.notify();
        });
    })
    .detach();
}

/// A pill switch (matches the shell's panel switches).
pub(super) fn switch(on: bool, p: &MandaraPalette) -> AnyElement {
    mandara_ui::plugins::network::switch(on, p).into_any_element()
}

/// A one-line result note (green when fine, coral on errors).
pub(super) fn note(text: &str, error: bool, p: &MandaraPalette) -> AnyElement {
    let color = if error { p.urgent_coral } else { p.success_emerald };
    div()
        .px(px(SPACE_14_PX))
        .py(px(SPACE_10_PX))
        .rounded(px(RADIUS_12_PX))
        .bg(color.with_alpha(ALPHA_12).hsla())
        .text_color(color.hsla())
        .text_size(px(TEXT_12_5_PX))
        .row()
        .items_center()
        .gap(px(SPACE_8_PX))
        .child(icon(if error { "close" } else { "check" }, ICON_14_PX, color))
        .child(div().flex_1().min_w_0().child(mandara_ui::bidi::paragraph(text.to_string())))
        .into_any_element()
}

/// QR modules must contrast whatever the theme: dark on light.
const QR_LIGHT: &str = "#ffffff";
const QR_DARK: &str = "#111111";
/// Quiet zone around the code, in modules (the QR spec asks for 4).
const QR_QUIET_MODULES: f32 = 4.0;

/// A QR code for `payload` (`WIFI:…`), `module_px` per module.
pub(super) fn qr(payload: &str, module_px: f32) -> AnyElement {
    let Ok(code) = qrcodegen::QrCode::encode_text(payload, qrcodegen::QrCodeEcc::Medium) else {
        return div().into_any_element();
    };
    let n = code.size();
    let (light, dark) = (Color::hex(QR_LIGHT).hsla(), Color::hex(QR_DARK).hsla());
    let rows = (0..n).map(|y| {
        // Consecutive dark modules become one bar (fewer elements).
        // Never mirrored: a QR code reads the same in every language.
        let mut row = div().ltr_row().h(px(module_px));
        let mut x = 0;
        while x < n {
            let on = code.get_module(x, y);
            let start = x;
            while x < n && code.get_module(x, y) == on {
                x += 1;
            }
            let w = (x - start) as f32 * module_px;
            row = row.child(div().w(px(w)).h_full().when_dark(on, dark));
        }
        row
    });
    div()
        .p(px(module_px * QR_QUIET_MODULES))
        .bg(light)
        .rounded(px(RADIUS_12_PX))
        .row()
        .flex_col()
        .flex_none()
        .children(rows)
        .into_any_element()
}

trait WhenDark {
    fn when_dark(self, on: bool, color: gpui::Hsla) -> Self;
}

impl WhenDark for gpui::Div {
    fn when_dark(self, on: bool, color: gpui::Hsla) -> Self {
        if on { self.bg(color) } else { self }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn search_finds_pages_by_topic() {
        use crate::registry::Section;
        let pages = |q: &str| super::matching_pages(q).into_iter().map(|(_, s)| s).collect::<Vec<_>>();
        assert!(pages("vpn").contains(&Section::Network));
        assert!(pages("cursor").contains(&Section::Input));
        assert!(pages("autostart").contains(&Section::Applications));
        assert!(pages("undo").contains(&Section::Profiles));
        assert!(pages("zzzz-nothing").is_empty());
    }

    #[test]
    fn wifi_payload_encodes() {
        let payload = mandara_config::connectivity::wifi_qr_payload("Mandara hotspot", Some("k3yp4ss-word"), false);
        let code = qrcodegen::QrCode::encode_text(&payload, qrcodegen::QrCodeEcc::Medium).unwrap();
        assert!(code.size() >= 21);
    }
}

/// `(icon, section)` of every page whose title or topics match `q`
/// (lowercase).
pub(super) fn matching_pages(q: &str) -> Vec<(&'static str, crate::registry::Section)> {
    mandara_ui::overlays::STUDIO_SECTIONS
        .iter()
        .filter(|(_, title, _, keywords)| title.to_lowercase().contains(q) || mandara_i18n::tr(title).to_lowercase().contains(q) || keywords.split_whitespace().any(|k| k.starts_with(q)) || keywords.contains(q))
        .filter_map(|(slug, _, icon_name, _)| crate::resolve_section(slug).map(|s| (*icon_name, s)))
        .collect()
}

/// Pages matched by a search: their title (English or translated) or the
/// topics they cover (the launcher's keywords), as chips that open them.
pub(super) fn page_matches(query: &str, p: &MandaraPalette, cx: &mut Context<StudioView>) -> Option<AnyElement> {
    use gpui::{ClickEvent, InteractiveElement as _, SharedString, StatefulInteractiveElement as _};
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let hits = matching_pages(&q);
    if hits.is_empty() {
        return None;
    }
    let chips = hits.into_iter().map(|(icon_name, section)| {
        div()
            .id(SharedString::from(format!("page-hit-{}", section.title())))
            .h(px(DIM_36_PX))
            .px(px(SPACE_14_PX))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .cursor_pointer()
            .bg(p.surface_container_high.hsla())
            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
            .child(icon(icon_name, ICON_15_PX, p.primary))
            .child(section.label())
            .child(icon(mandara_ui::bidi::dir_icon("chevron-right"), ICON_12_PX, p.on_surface_variant))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.search.update(cx, |f, _| f.set(""));
                this.query.clear();
                this.show(section, cx);
                let _ = window;
            }))
    });
    Some(
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(super::compositor_page::group_title(mandara_i18n::t("Pages"), p))
            .child(div().row().flex_wrap().gap(px(SPACE_8_PX)).children(chips))
            .into_any_element(),
    )
}
