//! Accessibility and per-surface playback controls.
use super::{Control::*, Section::Motion, Setting, s};
use mandara_i18n::tk;

/// `[motion.choreography] entrance`, in `Entrance::ALL`'s order.
const ENTRANCES: &[(&str, &str)] = &[
    ("anchor", tk("From its button or edge")),
    ("rise", tk("Rise from below")),
    ("drop", tk("Drop from above")),
    ("sweep", tk("Sweep from the reading side")),
    ("drift", tk("Drift in on a slant")),
];

/// Declare bounded global and per-surface controls for the shared settings engine.
pub(super) fn entries() -> Vec<Setting> {
    let range = mandara_config::motion::SURFACE_SPEED_RANGE;
    let intensity = mandara_config::motion::INTENSITY_RANGE;
    let mut entries = vec![
        s(
            Motion,
            tk("Accessibility"),
            "/motion/follow_system",
            tk("Follow system preference"),
            tk("Respect the desktop's reduced-motion preference"),
            Toggle,
        ),
        s(
            Motion,
            tk("Accessibility"),
            "/motion/intensity",
            tk("Motion intensity"),
            tk("Lower values soften bounce; zero makes transitions instant"),
            slider(intensity.0, intensity.1, 0.05, ""),
        ),
    ];
    let amount = mandara_config::motion::CHOREOGRAPHY_RANGE;
    entries.push(s(Motion, tk("Choreography"), "/motion/choreography/entrance", tk("Entrance"), tk("Where entering content comes from"), Choice(ENTRANCES)));
    for (path, label, detail) in [
        ("/motion/choreography/stagger", tk("Cascade"), tk("Lists, tiles and cards arrive one after another instead of all at once")),
        ("/motion/choreography/stretch", tk("Liquid stretch"), tk("Moving shapes stretch along their travel and round off at rest")),
        ("/motion/choreography/parallax", tk("Parallax"), tk("Content trails its surface and settles a beat later")),
    ] {
        entries.push(s(Motion, tk("Choreography"), path, label, detail, slider(amount.0, amount.1, 0.05, "")));
    }
    for (path, label) in [
        ("/motion/surfaces/bar", tk("Bar")),
        ("/motion/surfaces/panels", tk("Panels")),
        ("/motion/surfaces/sidebar", tk("Sidebar")),
        ("/motion/surfaces/launcher", tk("Launcher")),
        ("/motion/surfaces/dock", tk("Dock")),
        ("/motion/surfaces/notifications", tk("Notifications")),
        ("/motion/surfaces/osd", tk("On-screen displays")),
    ] {
        entries.push(s(
            Motion,
            tk("Surface transitions"),
            path,
            label,
            tk("Playback speed for this surface; zero applies transitions instantly"),
            slider(range.0, range.1, 0.1, "×"),
        ));
    }
    entries
}

fn slider(min: f32, max: f32, step: f32, unit: &'static str) -> super::Control {
    super::Control::Slider {
        min: min.into(),
        max: max.into(),
        step: step.into(),
        unit,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn entrance_choices_match_the_config() {
        let ids: Vec<String> = mandara_config::motion::Entrance::ALL.iter().map(|e| serde_json::to_value(e).unwrap().as_str().unwrap().to_string()).collect();
        assert_eq!(ids, super::ENTRANCES.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>());
    }
}
