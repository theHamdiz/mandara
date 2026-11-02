//! A plugin's own language and the peek bar, shared by Jannah, Qur'an,
//! Hadith and Prayer times.
//!
//! Each of those plugins has a `language` setting: `follow` (the shell's
//! language) or one of Mandara's languages. Their views, commands and
//! desktop-widget data are built inside [`mandara_i18n::scoped`], so every
//! `t`/`tf`/`tr`, every reading-direction choice ([`crate::bidi`]) and the
//! content language they pick (meanings, editions, prayer names) follow it,
//! while the rest of the shell keeps its own.
//!
//! The peek bar steps a panel's "today" item back and forth for a look
//! only: it never marks anything done or read and is forgotten on restart.

use std::rc::Rc;

use chrono::NaiveDate;
use gpui::{AnyElement, App, InteractiveElement as _, IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{Lang, t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::panel_fx::{PILL_H_PX, pill_button, readable};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};

/// What the `language` setting holds when the plugin follows the shell.
pub const FOLLOW: &str = "follow";

/// The `language` setting's values: [`FOLLOW`] and every language code.
pub const CHOICES: [&str; 16] = [FOLLOW, "en", "ar", "fa", "ur", "es", "fr", "de", "tr", "ru", "hi", "bn", "id", "zh", "ja", "ko"];

/// A plugin's own language from its `language` setting (`None`: follow
/// the shell; unknown codes follow it too).
pub fn own_language(code: &str) -> Option<Lang> {
    let code = code.trim();
    if code.is_empty() || code.eq_ignore_ascii_case(FOLLOW) { None } else { Lang::from_code(code) }
}

/// The language a plugin speaks: its own, else the shell's.
pub fn resolved(own: Option<Lang>) -> Lang {
    own.unwrap_or_else(mandara_i18n::shell_language)
}

/// `render`, built in the plugin's language (`lang` reads its setting).
pub fn in_language(lang: fn(&App) -> Option<Lang>, render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static {
    move |palette, window, cx| {
        let _scope = mandara_i18n::scoped(lang(cx));
        render(palette, window, cx)
    }
}

/// `spec` with its handler run in the plugin's language.
pub fn speaking(mut spec: CommandSpec, lang: fn(&App) -> Option<Lang>) -> CommandSpec {
    let handler = spec.handler.clone();
    spec.handler = Rc::new(move |args, cx| {
        let _scope = mandara_i18n::scoped(lang(cx));
        handler(args, cx)
    });
    spec
}

/// `<plugin> language <code|follow>`: sets (or with no argument shows) the
/// plugin's own language.
pub fn language_command(id: &'static str, path: &'static str, example: &'static str) -> CommandSpec {
    CommandSpec::new(path, tk("The plugin's own language (a code such as ar or en), or follow to use Mandara's"), move |a, cx| {
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let mut code = cfg.plugins.setting(id, "language", FOLLOW.to_string());
        if let Some(wanted) = a.get(0).map(str::trim).filter(|w| !w.is_empty()) {
            let wanted = wanted.to_ascii_lowercase();
            if wanted != FOLLOW && Lang::from_code(&wanted).is_none() {
                return Err(mandara_i18n::tf("Unknown language {code}: use follow or one of {codes}", &[("code", &wanted), ("codes", &CHOICES[1..].join(", "))]));
            }
            code = wanted;
            let value = toml::Value::String(code.clone());
            crate::state::update_config(cx, |c| {
                c.plugins.settings.entry(id.to_string()).or_default().insert("language".into(), value);
            });
        }
        let own = own_language(&code);
        Ok(json!({ "language": if own.is_some() { code.as_str() } else { FOLLOW }, "speaks": resolved(own).code(), "follows_mandara": own.is_none() }))
    })
    .arg(ArgSpec::optional("language", ArgKind::Enum, tk("follow, or a language code: en, ar, fa, ur, es, fr, de, tr, ru, hi, bn, id, zh, ja, ko")).choices(&CHOICES))
    .example(example)
}

/// `date` with a strftime `pattern` (a catalog key) in the current
/// language, day and month names included.
pub fn date(date: NaiveDate, pattern: &'static str) -> String {
    let lang = mandara_i18n::language();
    date.format_localized(t(pattern), chrono::Locale::try_from(lang.posix()).unwrap_or(chrono::Locale::en_US)).to_string()
}

// ── The peek bar ─────────────────────────────────────────────────────────

/// One step of the peek bar: -1, +1, or back to today (`-offset`).
pub type Step = Rc<dyn Fn(i64, &mut App)>;

/// The bar: "Previous" at the reading start, "Next" at the end, and in the
/// middle `here` (at today) or, while peeking, `away` and the way `back`
/// (a catalog key: "Back to today").
pub fn peek_bar(id: &'static str, offset: i64, here: SharedString, away: Option<SharedString>, back: &'static str, tint: Color, palette: &MandaraPalette, step: Step) -> AnyElement {
    let ink = palette.on_surface_variant;
    let button = |key: (&'static str, usize), label: &'static str, glyph: &'static str, start: bool, by: i64| {
        let step = step.clone();
        div()
            .id(key)
            .h(px(PILL_H_PX))
            .px(px(tokens::SM))
            .flex_none()
            .rounded_full()
            .row()
            .items_center()
            .gap(px(tokens::XS))
            .cursor_pointer()
            .hover(move |s| s.bg(ink.with_alpha(crate::kit::alpha::HOVER).hsla()))
            .text_size(px(text::LABEL))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(ink.hsla())
            .when(start, |el| el.child(icon(crate::bidi::dir_icon(glyph), text::LABEL + 1.0, ink)).child(t(label)))
            .when(!start, |el| el.child(t(label)).child(icon(crate::bidi::dir_icon(glyph), text::LABEL + 1.0, ink)))
            .on_click(move |_, _, cx| step(by, cx))
    };
    let middle: AnyElement = if offset == 0 {
        div().flex_1().min_w(px(0.0)).flex().justify_center().text_size(px(text::LABEL)).text_color(ink.hsla()).child(div().min_w(px(0.0)).truncate().child(here)).into_any_element()
    } else {
        let step = step.clone();
        let back = pill_button((id, 2usize), Some("undo"), t(back), readable(tint, palette), palette, move |_, cx| step(-offset, cx));
        div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .items_center()
            .justify_center()
            .gap(px(tokens::SM))
            .when_some(away, |el, a| el.child(div().min_w(px(0.0)).truncate().text_size(px(text::LABEL)).text_color(ink.hsla()).child(a)))
            .child(back)
            .into_any_element()
    };
    div()
        .w_full()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .child(button((id, 0), tk("Previous"), "chevron-left", true, -1))
        .child(middle)
        .child(button((id, 1), tk("Next"), "chevron-right", false, 1))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_languages_follow_unless_set() {
        assert_eq!(own_language(""), None);
        assert_eq!(own_language("follow"), None);
        assert_eq!(own_language(" Follow "), None);
        assert_eq!(own_language("xx"), None);
        assert_eq!(own_language("ar"), Some(Lang::Ar));
        assert_eq!(resolved(Some(Lang::Ja)), Lang::Ja);
        assert_eq!(resolved(None), mandara_i18n::shell_language());
        // Every language is offered, once.
        assert_eq!(CHOICES.len(), Lang::ALL.len() + 1);
        assert!(Lang::ALL.iter().all(|l| CHOICES.contains(&l.code())));
    }
}
