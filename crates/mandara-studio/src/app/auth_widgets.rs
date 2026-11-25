//! The same passive-widget editor for login and lock appearance.

use std::collections::HashSet;
use std::rc::Rc;
use gpui::{AnyElement, App, Global, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_config::desktop_widgets::{self as dw, Anchor, DesktopWidget, DesktopWidgetsConfig};
use mandara_config::preauth::{self, PublicWeather, Widgets};
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use super::compositor_page::{button, card, heading};
use crate::tokens::*;

/// A new public widget starts away from the authentication card.
const NEW_SCALE: f32 = 0.65;

#[derive(Default)]
struct Open(HashSet<String>);
impl Global for Open {}

#[derive(Clone)]
pub(super) enum Value { Anchor(Anchor), X(f32), Y(f32), Scale(f32), Opacity(f32) }

#[derive(Clone)]
pub(super) enum Edit {
    Add(Box<DesktopWidget>, bool),
    Remove(String),
    Place(String, Value),
    Weather(Option<PublicWeather>),
    Units(mandara_config::Units),
}

impl Edit {
    /// Normalize each edit against the public-widget rules before saving it.
    pub(super) fn apply(self, cfg: &mut Widgets) {
        match self {
            Self::Add(widget, keep_place) if cfg.items.len() < preauth::MAX_WIDGETS => {
                if let Some(mut widget) = preauth::copy(&widget) {
                    widget.enabled = true;
                    if !keep_place {
                        widget.place.anchor = Anchor::TopLeft;
                        (widget.place.x, widget.place.y) = Anchor::TopLeft.inset();
                        widget.place.scale = NEW_SCALE;
                    }
                    let mut list = DesktopWidgetsConfig { items: std::mem::take(&mut cfg.items), ..Default::default() };
                    list.add(widget);
                    cfg.items = list.items;
                }
            }
            Self::Remove(id) => cfg.items.retain(|w| w.id != id),
            Self::Place(id, value) => if let Some(w) = cfg.items.iter_mut().find(|w| w.id == id) {
                match value {
                    Value::Anchor(anchor) => { w.place.anchor = anchor; (w.place.x, w.place.y) = anchor.inset(); }
                    Value::X(v) => w.place.x = v.clamp(dw::ranges::OFFSET.0, dw::ranges::OFFSET.1),
                    Value::Y(v) => w.place.y = v.clamp(dw::ranges::OFFSET.0, dw::ranges::OFFSET.1),
                    Value::Scale(v) => w.place.scale = v.clamp(dw::ranges::SCALE.0, dw::ranges::SCALE.1),
                    Value::Opacity(v) => w.place.opacity = v.clamp(dw::ranges::OPACITY.0, dw::ranges::OPACITY.1),
                }
            },
            Self::Weather(value) => cfg.weather = value.filter(PublicWeather::valid),
            Self::Units(units) => cfg.units = units,
            Self::Add(_, _) => {},
        }
        cfg.normalize();
    }
}

pub(super) type Dispatch = Rc<dyn Fn(Edit, &mut App)>;

/// Toggle a disclosure independently for each screen and widget id.
fn fold(id: String, cx: &mut App) {
    let state = cx.default_global::<Open>();
    if !state.0.remove(&id) { state.0.insert(id); }
    cx.refresh_windows();
}

fn row(label: impl Into<SharedString>, control: impl IntoElement, p: &MandaraPalette) -> AnyElement {
    div().row().items_center().flex_wrap().gap(px(SPACE_12_PX)).child(div().min_w(px(DIM_120_PX)).text_color(p.on_surface_variant.hsla()).child(label.into())).child(control).into_any_element()
}

fn slider(prefix: &str, id: &str, what: &'static str, value: f32, range: (f32, f32), set: fn(f32) -> Value, dispatch: Dispatch, p: &MandaraPalette) -> AnyElement {
    let widget_id = id.to_string();
    let slider_id: SharedString = format!("auth-widget-{prefix}-{id}-{what}").into();
    let (min, max) = range;
    let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let control = mandara_ui::kit::slider_with_label(slider_id, frac, p, move |f| format!("{:.2}", min + f * (max - min)).into(), move |f, window, cx| {
        dispatch(Edit::Place(widget_id.clone(), set(min + f * (max - min))), cx);
        mandara_ui::slider_model::next_frame(window)
    });
    row(tr(what), div().w(px(DIM_240_PX)).child(control), p)
}

/// Exclude agenda cards before making the same sanitized copy used on save.
fn gallery(widget: &DesktopWidget) -> Option<DesktopWidget> {
    if matches!(&widget.kind, dw::WidgetKind::Date(d) if d.layout == dw::DateLayout::Agenda) { return None; }
    preauth::copy(widget)
}

/// Edit this screen’s own widget collection and explicitly chosen public weather.
pub(super) fn panel(prefix: &'static str, cfg: &Widgets, dispatch: Dispatch, p: &MandaraPalette, cx: &mut App) -> AnyElement {
    let gallery_key = format!("auth-gallery-{prefix}");
    let gallery_open = cx.default_global::<Open>().0.contains(&gallery_key);
    let mut body = card(p).p(px(SPACE_16_PX)).gap(px(SPACE_12_PX)).child(heading(t("Widgets"), t("Public decorations only. Notes, messages, account data and widget actions stay out of sign-in screens."), p));
    for widget in &cfg.items {
        let id = widget.id.clone();
        let key = format!("auth-{prefix}-{id}");
        let open = cx.default_global::<Open>().0.contains(&key);
        let label = match &widget.kind { dw::WidgetKind::Clock(c) => tr(mandara_ui::desktop_plugins::face_label(c.face)), _ => tr(mandara_ui::desktop_plugins::label(widget).0) };
        let remove = dispatch.clone();
        let remove_id = id.clone();
        let mut part = div().row().flex_col().gap(px(SPACE_10_PX)).child(div().row().items_center().gap(px(SPACE_8_PX))
            .child(button(format!("auth-edit-{prefix}-{id}"), "sliders", label, open, p).on_click(move |_, _, cx| fold(key.clone(), cx)))
            .child(button(format!("auth-remove-{prefix}-{id}"), "close", t("Remove"), false, p).on_click(move |_, _, cx| remove(Edit::Remove(remove_id.clone()), cx))));
        if open {
            let anchor_dispatch = dispatch.clone();
            let anchor_id = id.clone();
            let anchors = super::desktop_widgets_page::anchor_grid(&format!("auth-{prefix}-{id}"), widget.place.anchor, p, Rc::new(move |anchor, cx| anchor_dispatch(Edit::Place(anchor_id.clone(), Value::Anchor(anchor)), cx)));
            part = part.child(row(t("Anchor"), anchors, p));
            for (name, value, range, set) in [(tk("Horizontal offset"), widget.place.x, dw::ranges::OFFSET, Value::X as fn(f32) -> Value), (tk("Vertical offset"), widget.place.y, dw::ranges::OFFSET, Value::Y), (tk("Scale"), widget.place.scale(), dw::ranges::SCALE, Value::Scale), (tk("Opacity"), widget.place.opacity(), dw::ranges::OPACITY, Value::Opacity)] {
                part = part.child(slider(prefix, &id, name, value, range, set, dispatch.clone(), p));
            }
        }
        body = body.child(part);
    }
    body = body.child(button(format!("auth-add-{prefix}"), "plus", t("Add a widget"), false, p).on_click(move |_, _, cx| fold(gallery_key.clone(), cx)));
    if gallery_open && cfg.items.len() < preauth::MAX_WIDGETS {
        let mut presets = Vec::new();
        if let Some(registry) = cx.try_global::<PluginRegistry>() {
            for kind in registry.desktop_widgets() {
                let styles = if kind.styles.is_empty() { vec![(kind.name.clone(), kind.make.clone())] } else { kind.styles.iter().map(|s| (s.name.clone(), s.make.clone())).collect() };
                for (name, make) in styles {
                    if let Some(widget) = make().first().and_then(gallery) { presets.push((name, widget)); }
                }
            }
        }
        let choices = presets.into_iter().enumerate().map(|(index, (name, widget))| {
            let dispatch = dispatch.clone();
            button(format!("auth-preset-{prefix}-{index}"), "plus", tr(&name), false, p).on_click(move |_, _, cx| dispatch(Edit::Add(Box::new(widget.clone()), false), cx)).into_any_element()
        }).collect::<Vec<_>>();
        body = body.child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(choices));
        let desktop = mandara_ui::state::config(cx).read(cx).get().desktop.widgets.items.clone();
        let copies = desktop.into_iter().filter_map(|w| preauth::copy(&w)).enumerate().map(|(i, widget)| {
            let dispatch = dispatch.clone(); let label = format!("{} · {}", tr(mandara_ui::desktop_plugins::label(&widget).0), widget.id);
            button(format!("auth-copy-{prefix}-{i}"), "plus", label, false, p).on_click(move |_, _, cx| dispatch(Edit::Add(Box::new(widget.clone()), true), cx)).into_any_element()
        }).collect::<Vec<_>>();
        if !copies.is_empty() { body = body.child(heading(t("Copy from desktop"), t("Copies its appearance and position. Later desktop edits do not change this screen."), p)).child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(copies)); }
    }
    let shell = mandara_ui::state::config(cx).read(cx).snapshot();
    let query = mandara_ui::plugins::weather::query(&shell);
    let location = PublicWeather { latitude: query.latitude, longitude: query.longitude, place: mandara_ui::plugins::weather::location_name(&shell) };
    let use_location = dispatch.clone();
    let clear_location = dispatch.clone();
    body = body.child(heading(t("Public weather location"), t("The location you add here is public and allows weather requests on this screen. Studio previews do not contact the weather service."), p));
    if let Some(location) = &cfg.weather {
        body = body.child(row(format!("{} · {:.4}, {:.4}", location.place, location.latitude, location.longitude), button(format!("auth-weather-clear-{prefix}"), "close", t("Remove"), false, p).on_click(move |_, _, cx| clear_location(Edit::Weather(None), cx)), p));
    } else {
        body = body.child(row(format!("{} · {:.4}, {:.4}", location.place, location.latitude, location.longitude), button(format!("auth-weather-add-{prefix}"), "plus", t("Use this location publicly"), false, p).on_click(move |_, _, cx| use_location(Edit::Weather(Some(location.clone())), cx)), p));
    }
    let unit_choices = [(mandara_config::Units::Metric, t("Metric")), (mandara_config::Units::Imperial, t("Imperial"))].into_iter().map(|(unit, label)| {
        let dispatch = dispatch.clone();
        button(format!("auth-unit-{prefix}-{unit:?}"), "thermometer", label, unit == cfg.units, p).on_click(move |_, _, cx| dispatch(Edit::Units(unit), cx)).into_any_element()
    }).collect::<Vec<_>>();
    body.child(row(t("Units"), div().row().gap(px(SPACE_6_PX)).children(unit_choices), p)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_have_unique_ids_and_can_be_edited_independently() {
        let mut cfg = Widgets::default();
        let source = DesktopWidget::default();
        Edit::Add(Box::new(source.clone()), false).apply(&mut cfg);
        Edit::Add(Box::new(source.clone()), false).apply(&mut cfg);
        let first = cfg.items[0].id.clone();
        assert_ne!(first, cfg.items[1].id);
        Edit::Place(first.clone(), Value::Scale(2.0)).apply(&mut cfg);
        assert_eq!(cfg.items[0].place.scale, 2.0);
        assert_eq!(cfg.items[1].place.scale, NEW_SCALE);
        assert_eq!(source.place.scale, 1.0);
        Edit::Remove(first).apply(&mut cfg);
        assert_eq!(cfg.items.len(), 1);
    }
    #[test]
    fn gallery_does_not_offer_an_agenda_without_events() {
        let widget = DesktopWidget { kind: dw::WidgetKind::Date(dw::DateWidget { layout: dw::DateLayout::Agenda, ..Default::default() }), ..Default::default() };
        assert!(gallery(&widget).is_none());
        assert!(gallery(&DesktopWidget::default()).is_some());
    }

}
