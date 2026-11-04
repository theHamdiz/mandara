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
    Some(widget)
}

impl Widgets {
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
    fn non_finite_placement_is_normalized_before_painting() {
        let widget = DesktopWidget { place: Placement { x: f32::NAN, y: f32::INFINITY, scale: f32::NAN, opacity: f32::NEG_INFINITY, ..Default::default() }, ..Default::default() };
        let copied = copy(&widget).unwrap();
        assert_eq!(copied.place.x, 0.0);
        assert_eq!(copied.place.y, 0.0);
        assert_eq!(copied.place.scale, 1.0);
        assert_eq!(copied.place.opacity, 1.0);
    }
}
