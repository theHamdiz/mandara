//! Which text controls affect a widget's current layout.

use crate::desktop_widgets::{
    Chrome, ClockFace, DateLayout, DecorShape, Look, Plate, SkyLayout, WeatherLayout, WidgetKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Content,
    Header,
    Fixed,
}

pub trait TextControls {
    fn alignment(&self) -> Alignment;
    fn has_text(&self) -> bool;
    fn has_foreground(&self) -> bool;
    fn has_calendar(&self) -> bool;
}

fn header(look: &Look) -> bool {
    !look.header.trim().is_empty() && matches!(look.chrome, Chrome::None | Chrome::Tape)
}

impl TextControls for WidgetKind {
    fn has_calendar(&self) -> bool {
        matches!(self,Self::Clock(clock) if clock.face != ClockFace::Word)
            || matches!(self, Self::Countdown(_))
    }
    fn alignment(&self) -> Alignment {
        let content = match self {
            Self::Clock(clock) => {
                matches!(
                    clock.face,
                    ClockFace::Superscript
                        | ClockFace::Greeting
                        | ClockFace::DayBlock
                        | ClockFace::DayNumeral
                        | ClockFace::Minimal
                        | ClockFace::Stack
                        | ClockFace::DotMatrix
                        | ClockFace::Pixel
                        | ClockFace::Template
                ) || (clock.face == ClockFace::Analog && clock.date)
            }
            Self::Decoration(decoration) => decoration.shape == DecorShape::Text,
            Self::Weather(weather) => matches!(
                weather.layout,
                WeatherLayout::Hero | WeatherLayout::Sentence
            ),
            Self::Countdown(_)
            | Self::Quote(_)
            | Self::Jannah(_)
            | Self::Hadith(_)
            | Self::Quran(_)
            | Self::Glance(_) => true,
            Self::Progress(progress) => progress.label,
            Self::Date(date) => date.layout == DateLayout::Tile,
            Self::Sky(sky) => sky.layout == SkyLayout::Moon && sky.labels,
            _ => false,
        };
        if content {
            Alignment::Content
        } else if self.look().is_some_and(header) {
            Alignment::Header
        } else {
            Alignment::Fixed
        }
    }

    fn has_foreground(&self) -> bool {
        !matches!(self, Self::Visualizer(_)) || self.has_text()
    }

    fn has_text(&self) -> bool {
        let content = match self {
            Self::Visualizer(_) | Self::Bubbles(_) => false,
            Self::Decoration(decoration) => decoration.shape == DecorShape::Text,
            Self::Card(_) => false,
            _ => true,
        };
        content
            || self.look().is_some_and(|look| {
                !look.header.trim().is_empty()
                    || (look.chrome == Chrome::Window && look.plate != Plate::None)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_widgets::{ClockWidget, DateWidget, PlaneVisualizer};
    #[test]
    fn time_only_faces_do_not_offer_a_calendar_that_they_cannot_show() {
        let word = WidgetKind::Clock(ClockWidget {
            face: ClockFace::Word,
            ..Default::default()
        });
        assert!(!word.has_calendar());
        assert!(
            WidgetKind::Clock(ClockWidget {
                face: ClockFace::SplitFlap,
                ..Default::default()
            })
            .has_calendar()
        );
        assert!(WidgetKind::Countdown(Default::default()).has_calendar());
    }
    #[test]
    fn fixed_layouts_explain_alignment_until_an_alignable_header_is_added() {
        let mut visualizer = WidgetKind::Visualizer(PlaneVisualizer::default());
        assert_eq!(visualizer.alignment(), Alignment::Fixed);
        assert!(!visualizer.has_text());
        assert!(!visualizer.has_foreground());
        visualizer.look_mut().unwrap().header = "Audio".into();
        assert_eq!(visualizer.alignment(), Alignment::Header);
        assert!(visualizer.has_text());
        assert!(visualizer.has_foreground());
        assert!(WidgetKind::Bubbles(Default::default()).has_foreground());
        assert_eq!(
            WidgetKind::Date(DateWidget::default()).alignment(),
            Alignment::Fixed
        );
        for face in [ClockFace::Greeting, ClockFace::DayNumeral] {
            assert_eq!(
                WidgetKind::Clock(ClockWidget {
                    face,
                    ..Default::default()
                })
                .alignment(),
                Alignment::Content
            );
        }
    }
}
