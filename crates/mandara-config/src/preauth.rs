//! Explicitly public, passive widgets for authentication screens.

use serde::{Deserialize, Serialize};
use crate::desktop_widgets::{DesktopWidget, WidgetKind};

/// Authentication screens keep a small, bounded collection of decorations.
pub const MAX_WIDGETS: usize = 24;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublicWeather {
    pub latitude: f64,
    pub longitude: f64,
    pub place: String,
}

impl PublicWeather {
    /// Accept only finite coordinates within the latitude and longitude ranges.
    pub fn valid(&self) -> bool {
        self.latitude.is_finite() && self.longitude.is_finite() && (-90.0..=90.0).contains(&self.latitude) && (-180.0..=180.0).contains(&self.longitude)
    }
}

/// Empty on existing and new installations. Adding a widget is an explicit
/// choice; no desktop content or location is copied automatically.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Widgets {
    pub items: Vec<DesktopWidget>,
    pub weather: Option<PublicWeather>,
    pub units: crate::Units,
}

/// Only kinds that render without personal files, accounts or actions.
pub fn supported(kind: &WidgetKind) -> bool {
    matches!(kind, WidgetKind::Clock(_) | WidgetKind::Date(_) | WidgetKind::Weather(_) | WidgetKind::WorldClock(_) | WidgetKind::Countdown(_) | WidgetKind::Progress(_) | WidgetKind::Sky(_) | WidgetKind::Decoration(_) | WidgetKind::Palette(_))
}

/// Copy appearance and public text while removing private data and output bindings.
pub fn copy(widget: &DesktopWidget) -> Option<DesktopWidget> {
    if !supported(&widget.kind) { return None; }
    let mut widget = widget.clone();
    let defaults = crate::desktop_widgets::Placement::default();
    widget.place.x = crate::widget_kinds::within(widget.place.x, crate::desktop_widgets::ranges::OFFSET, defaults.x);
    widget.place.y = crate::widget_kinds::within(widget.place.y, crate::desktop_widgets::ranges::OFFSET, defaults.y);
    widget.place.scale = crate::widget_kinds::within(widget.place.scale, crate::desktop_widgets::ranges::SCALE, defaults.scale);
    widget.place.opacity = crate::widget_kinds::within(widget.place.opacity, crate::desktop_widgets::ranges::OPACITY, defaults.opacity);
    widget.place.displays.clear();
    widget.place.depth = crate::desktop_widgets::Depth::Front;
    if let WidgetKind::Date(date) = &mut widget.kind {
        // Calendar accounts and event text are never exported.
        date.event_dots = false;
        if date.layout == crate::widget_kinds::DateLayout::Agenda {
            date.layout = crate::widget_kinds::DateLayout::Month;
        }
    }
    if let WidgetKind::Countdown(countdown) = &mut widget.kind {
        countdown.title.clear();
        countdown.date.clear();
    }
    Some(widget)
}

impl Widgets {
    /// Apply public-copy rules to saved entries and bound the collection size.
    pub fn normalize(&mut self) {
        self.items = self.items.iter().filter_map(copy).take(MAX_WIDGETS).collect();
        if self.weather.as_ref().is_some_and(|w| !w.valid()) { self.weather = None; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_widgets::*;

    #[test]
    fn copied_decorations_keep_the_text_the_user_chose_to_make_public() {
        let source = DesktopWidget { kind: WidgetKind::Decoration(crate::desktop_widgets::Decoration { text: "Welcome home".into(), ..Default::default() }), ..Default::default() };
        let copy = copy(&source).unwrap();
        assert!(matches!(copy.kind, WidgetKind::Decoration(d) if d.text == "Welcome home"));
    }

    #[test]
    fn private_and_active_widgets_are_not_public_decorations() {
        for kind in [WidgetKind::Note(NoteWidget::default()), WidgetKind::Todo(TodoWidget::default()), WidgetKind::Media(MediaWidget::default()), WidgetKind::Photo(PhotoWidget::default())] {
            assert!(copy(&DesktopWidget { kind, ..Default::default() }).is_none());
        }
        let source = DesktopWidget { kind: WidgetKind::Date(crate::widget_kinds::DateWidget { layout: crate::widget_kinds::DateLayout::Agenda, event_dots: true, ..Default::default() }), ..Default::default() };
        assert!(matches!(copy(&source).unwrap().kind, WidgetKind::Date(d) if d.layout == crate::widget_kinds::DateLayout::Month && !d.event_dots));
        assert!(matches!(source.kind, WidgetKind::Date(d) if d.event_dots));
    }

    #[test]
    fn defaults_are_empty_and_invalid_locations_do_not_make_requests() {
        assert!(Widgets::default().items.is_empty());
        let mut widgets = Widgets { weather: Some(PublicWeather { latitude: f64::NAN, longitude: 10.0, place: "fixture".into() }), ..Default::default() };
        widgets.normalize();
        assert!(widgets.weather.is_none());
        let old: Widgets = toml::from_str("").unwrap();
        assert_eq!(old, Widgets::default());
    }

    #[test]
    fn public_countdowns_do_not_export_personal_titles_or_dates() {
        let source = DesktopWidget { kind: WidgetKind::Countdown(CountdownWidget { title: "Private appointment".into(), date: "2027-03-19".into(), show_date: false, ..Default::default() }), ..Default::default() };
        let copy = copy(&source).unwrap();
        assert!(matches!(copy.kind, WidgetKind::Countdown(c) if c.title.is_empty() && c.date.is_empty() && !c.show_date));
        assert!(matches!(&source.kind, WidgetKind::Countdown(c) if c.title == "Private appointment" && c.date == "2027-03-19"));
        let mut config = Widgets { items: vec![source], ..Default::default() };
        config.normalize();
        assert!(matches!(&config.items[0].kind, WidgetKind::Countdown(c) if c.title.is_empty() && c.date.is_empty()));
    }

    #[test]
    fn non_finite_placement_is_normalized_before_painting() {
        let widget = DesktopWidget { place: Placement { x: f32::NAN, y: f32::INFINITY, scale: f32::NAN, opacity: f32::NEG_INFINITY, ..Default::default() }, ..Default::default() };
        let copied = copy(&widget).unwrap();
        assert_eq!(copied.place.x, 0.0);
        assert_eq!(copied.place.y, 0.0);
        assert_eq!(copied.place.scale, 1.0);
        assert_eq!(copied.place.opacity, 1.0);
    }
}
