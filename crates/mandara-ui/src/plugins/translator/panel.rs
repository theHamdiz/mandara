//! The translation editor and searchable provider language picker.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyElement, App, AppContext as _, Entity, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_plugins::PanelCx;
use mandara_services::translator::{self as service, Backend as _, Language, Translation};
use mandara_theme::MandaraPalette;

use super::{Preferences, settings};
use crate::bidi::Bidi as _;
use crate::kit::{text, tokens};
use crate::plugins::integration::{self, Data};
use crate::plugins::panel_fx::{glass_card, header, pill_button, readable};
use crate::text_field::TextField;

/// Catalogs change rarely; a failed read has an explicit retry button.
const CATALOG_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Keep menu construction bounded; search reaches every advertised language.
const VISIBLE_LANGUAGES: usize = 40;
/// The input keeps room for several lines without pushing the result away.
const INPUT_H: f32 = 140.0;

#[derive(Clone, Copy)]
enum Side { Source, Target }

#[derive(Clone, PartialEq, Eq)]
struct Work { preferences: Preferences, text: String, source: Option<String>, target: String }

struct Model {
    preferences: Option<Preferences>,
    source: Option<String>,
    target: String,
    picker: Option<Side>,
    query: String,
    catalog: Rc<RefCell<Data<Preferences, Vec<Language>>>>,
    output: Rc<RefCell<Data<Work, Translation>>>,
}

pub(super) struct State {
    model: Rc<RefCell<Model>>,
    input: Entity<TextField>,
    search: Entity<TextField>,
}

impl State {
    pub fn new(cx: &mut App) -> Self {
        let model = Rc::new(RefCell::new(Model { preferences: None, source: None, target: super::DEFAULT_TARGET.into(), picker: None, query: String::new(), catalog: Rc::default(), output: Rc::default() }));
        let change = model.clone();
        let input = cx.new(|cx| TextField::new(String::new(), t("Text to translate"), true, Rc::new(move |_, _, cx| notify(&change, cx)), cx).multiline());
        let change = model.clone();
        let search = cx.new(|cx| TextField::new(String::new(), t("Search languages"), true, Rc::new(move |value, _, cx| {
            change.borrow_mut().query = value;
            notify(&change, cx);
        }), cx));
        Self { model, input, search }
    }
}

fn notify(model: &Rc<RefCell<Model>>, cx: &mut App) { model.borrow().output.borrow().views.notify(cx); }

/// Respect provider source-to-target pairs and omit source-only languages.
fn targets<'a>(languages: &'a [Language], source: Option<&str>) -> impl Iterator<Item = &'a Language> {
    let allowed = source.and_then(|code| languages.iter().find(|language| language.code.eq_ignore_ascii_case(code))).map(|language| &language.targets);
    languages.iter().filter(move |language| language.target && allowed.is_none_or(|allowed| allowed.is_empty() || allowed.iter().any(|code| code.eq_ignore_ascii_case(&language.code))))
}

/// Keep a valid choice; otherwise prefer the default target before the first result.
fn normalize(model: &mut Model, languages: &[Language]) {
    if model.source.as_ref().is_some_and(|source| !languages.iter().any(|language| language.source && language.code.eq_ignore_ascii_case(source))) { model.source = None; }
    let available: Vec<_> = targets(languages, model.source.as_deref()).collect();
    if let Some(current) = available.iter().find(|language| language.code.eq_ignore_ascii_case(&model.target)) {
        model.target = current.code.clone();
    } else if let Some(default) = available.iter().find(|language| language.code.eq_ignore_ascii_case(super::DEFAULT_TARGET))
        .or_else(|| available.iter().find(|language| language.code.split('-').next().is_some_and(|base| base.eq_ignore_ascii_case(super::DEFAULT_TARGET))))
        .or_else(|| available.first()) {
        model.target = default.code.clone();
    }
}

fn load_languages(model: &Rc<RefCell<Model>>, preferences: Preferences, force: bool, cx: &mut App) {
    let catalog = model.borrow().catalog.clone();
    let request = preferences.clone();
    integration::load(&catalog, preferences, force, CATALOG_AGE, move |_| {
        let key = super::api_key(&request)?;
        service::Http::new().languages(&request.connection, key.as_ref())
    }, cx);
}

/// Use the provider’s display name, with an explicit automatic-source label.
fn name(languages: &[Language], code: Option<&str>) -> String {
    code.map(|code| languages.iter().find(|language| language.code.eq_ignore_ascii_case(code)).map(|language| language.name.clone()).unwrap_or_else(|| code.into()))
        .unwrap_or_else(|| t("Detect automatically").into())
}

/// Keep the visible language list bounded; search still reaches the full catalog.
fn picker(state: &State, side: Side, languages: &[Language], p: &MandaraPalette) -> AnyElement {
    let accent = readable(p.primary, p);
    let model = state.model.borrow();
    let query = model.query.to_lowercase();
    let matching: Vec<_> = match side { Side::Source => languages.iter().filter(|language| language.source).collect(), Side::Target => targets(languages, model.source.as_deref()).collect() };
    let matching: Vec<_> = matching.into_iter().filter(|language| language.name.to_lowercase().contains(&query) || language.code.to_lowercase().contains(&query)).collect();
    let choose = |code: Option<String>| {
        let model = state.model.clone();
        move |_: &mut Window, cx: &mut App| {
            { let mut model = model.borrow_mut(); match side { Side::Source => model.source = code.clone(), Side::Target => model.target = code.clone().unwrap_or_default() } model.picker = None; }
            notify(&model, cx);
        }
    };
    let mut rows = div().id("translator-languages").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::XS));
    if matches!(side, Side::Source) { rows = rows.child(pill_button("translation-auto", None, t("Detect automatically"), accent, p, choose(None)).focusable().tab_index(0)); }
    for language in matching.iter().take(VISIBLE_LANGUAGES) {
        rows = rows.child(pill_button(SharedString::from(format!("translation-language-{}", language.code)), None, language.name.clone(), accent, p, choose(Some(language.code.clone()))).w_full().min_w_0().truncate().focusable().tab_index(0));
    }
    if matching.len() > VISIBLE_LANGUAGES { rows = rows.child(div().text_size(px(text::BODY_S)).child(t("Search to find more languages"))); }
    let close = state.model.clone();
    div().flex_1().min_h_0().row().flex_col().gap(px(tokens::SM)).child(state.search.clone()).child(rows)
        .child(pill_button("translation-picker-close", Some("close"), t("Back"), accent, p, move |_, cx| { close.borrow_mut().picker = None; notify(&close, cx); }).focusable().tab_index(0)).into_any_element()
}

/// Count Unicode characters against the provider's limit as one counter value.
fn counter(text: &str) -> String {
    tf("{counter}", &[("counter", &format!("{} / {}", text.chars().count(), service::MAX_TEXT_CHARS))])
}

/// Preserve typed input while service work runs and submit only from explicit controls.
pub(super) fn render(state: &State, context: &PanelCx, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let preferences = settings(&crate::state::config(cx).read(cx).snapshot());
    let (catalog, output) = {
        let mut model = state.model.borrow_mut();
        if model.preferences.as_ref() != Some(&preferences) {
            model.source = None; model.target = preferences.target.clone(); model.picker = None; model.preferences = Some(preferences.clone());
        }
        (model.catalog.clone(), model.output.clone())
    };
    catalog.borrow_mut().select(preferences.clone());
    catalog.borrow().views.track(window);
    output.borrow().views.track(window);
    if context.progress() > 0.0 { load_languages(&state.model, preferences.clone(), false, cx); }
    let languages = catalog.borrow().value.clone().unwrap_or_default();
    if !languages.is_empty() { normalize(&mut state.model.borrow_mut(), &languages); }
    let model = state.model.borrow();
    let work = Work { preferences: preferences.clone(), text: state.input.read(cx).value.clone(), source: model.source.clone(), target: model.target.clone() };
    output.borrow_mut().select(work.clone());
    let accent = readable(p.primary, p);
    let setup = pill_button("translator-settings", Some("sliders"), t("Settings"), accent, p, |_, cx| crate::state::open_studio_at(Some("plugins/translator/settings".into()), cx)).focusable().tab_index(0).into_any_element();
    let mut column = div().size_full().p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
        .child(header("translate", accent, t("Translator"), t("Text translation"), p, Some(setup)));
    if let Some(side) = model.picker { return column.child(picker(state, side, &languages, p)).into_any_element(); }
    let choices = [(Side::Source, "translation-source", name(&languages, model.source.as_deref())), (Side::Target, "translation-target", name(&languages, Some(&model.target)))];
    drop(model);
    let mut selectors = div().row().gap(px(tokens::SM));
    for (side, id, label) in choices {
        let model = state.model.clone(); let search = state.search.clone();
        selectors = selectors.child(pill_button(id, Some("chevron-right"), label, accent, p, move |window, cx| {
            { let mut model = model.borrow_mut(); model.picker = Some(side); model.query.clear(); }
            search.update(cx, |field, cx| { field.set(""); field.focus(window, cx); cx.notify(); });
            notify(&model, cx);
        }).flex_1().min_w_0().truncate().focusable().tab_index(0));
    }
    column = column.child(selectors);
    if let Some(error) = catalog.borrow().error.clone() {
        let model = state.model.clone(); let preferences = preferences.clone();
        column = column.child(div().text_size(px(text::BODY_S)).text_color(readable(p.warning_amber, p).hsla()).child(crate::bidi::paragraph(error)))
            .child(pill_button("translator-reload", Some("refresh"), t("Reload languages"), accent, p, move |_, cx| load_languages(&model, preferences.clone(), true, cx)).focusable().tab_index(0));
    }
    column = column.child(glass_card(p).id("translation-input").h(px(INPUT_H)).flex_none().p(px(tokens::SM)).overflow_y_scroll().child(state.input.clone()));
    let input = state.input.clone();
    let paste = pill_button("translation-paste", Some("clipboard"), t("Paste"), accent, p, move |_, cx| {
        if let Some(value) = cx.read_from_clipboard().and_then(|item| item.text()) { input.update(cx, |field, cx| { field.set(&value); cx.notify(); }); }
    }).focusable().tab_index(0);
    let send = output.clone(); let request = work.clone();
    let translate = pill_button("translation-submit", Some("translate"), if output.borrow().busy() { t("Translating…") } else { t("Translate") }, accent, p, move |_, cx| {
        let request = request.clone();
        integration::load(&send, request.clone(), true, Duration::ZERO, move |_| super::translate(&request.preferences, &request.text, request.source.as_deref(), &request.target), cx);
    }).focusable().tab_index(0);
    column = column.child(div().row().items_center().justify_between().child(paste).child(div().text_size(px(text::LABEL)).child(counter(&work.text))).child(translate));
    let result = output.borrow();
    if let Some(error) = &result.error { column = column.child(div().text_size(px(text::BODY_S)).text_color(readable(p.warning_amber, p).hsla()).child(crate::bidi::paragraph(error.clone()))); }
    let mut translated = glass_card(p).id("translation-result").flex_1().min_h_0().overflow_y_scroll().p(px(tokens::MD)).text_size(px(text::BODY));
    if let Some(value) = &result.value {
        let text = value.text.clone();
        translated = translated.child(crate::bidi::paragraph(text.clone()));
        column = column.child(translated).child(pill_button("translation-copy", Some("copy"), t("Copy translation"), accent, p, move |_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()))).focusable().tab_index(0));
    } else { column = column.child(translated.child(t("Your translation appears here"))); }
    column.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Text is sent only when you press Translate.")))).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counter_counts_characters_and_fills_its_single_placeholder() {
        let _language = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
        assert_eq!(counter("Hi مرحبا"), "8 / 5000");
        assert_eq!(counter(""), "0 / 5000");
        assert!(!counter("界").contains('{'));
    }

    #[test]
    fn target_choices_respect_provider_pairs_and_ignore_source_only_variants() {
        let languages = vec![
            Language { code: "en".into(), name: "English".into(), source: true, target: true, targets: vec!["ar".into()] },
            Language { code: "ar".into(), name: "Arabic".into(), source: true, target: true, targets: vec!["en".into()] },
            Language { code: "XX".into(), name: "Source only".into(), source: true, target: false, targets: vec![] },
        ];
        assert_eq!(targets(&languages, Some("EN")).map(|language| language.code.as_str()).collect::<Vec<_>>(), ["ar"]);
        assert_eq!(targets(&languages, None).count(), 2);
        let mut model = Model { preferences: None, source: Some("en".into()), target: "EN".into(), picker: None, query: String::new(), catalog: Rc::default(), output: Rc::default() };
        normalize(&mut model, &languages);
        assert_eq!(model.target, "ar");
        model.source = Some("missing".into());
        normalize(&mut model, &languages);
        assert_eq!(model.source, None);
    }
}
