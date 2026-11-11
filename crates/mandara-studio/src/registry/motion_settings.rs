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

/// `[dock.motion]` choices, each in its enum's `ALL` order.
const DOCK_LAUNCH: &[(&str, &str)] = &[("bounce", tk("Bounce")), ("pulse", tk("Pulse")), ("ripple", tk("Ripple")), ("none", tk("Off"))];
const DOCK_ATTENTION: &[(&str, &str)] = &[("bounce", tk("Bounce")), ("pulse", tk("Pulse")), ("wiggle", tk("Wiggle")), ("none", tk("Off"))];
const DOCK_INDICATOR: &[(&str, &str)] = &[("spring", tk("Grow")), ("fade", tk("Fade")), ("none", tk("Instant"))];
const DOCK_MINIMIZE: &[(&str, &str)] = &[("dip", tk("Dip")), ("pulse", tk("Pulse")), ("none", tk("Off"))];
const DOCK_ITEMS: &[(&str, &str)] = &[("spring", tk("Grow")), ("fade", tk("Fade")), ("none", tk("Instant"))];
const DOCK_REVEAL: &[(&str, &str)] = &[("slide", tk("Slide")), ("fade", tk("Fade")), ("scale", tk("Zoom"))];

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
    // The dock's own animations; its speed is the Dock slider above.
    for (path, label, detail, choices) in [
        ("/dock/motion/launch", tk("Opening apps"), tk("While an app starts, its icon bounces, pulses or sends out ripples (Dock → Animate opening applications switches it off)"), DOCK_LAUNCH),
        ("/dock/motion/attention", tk("Apps asking for attention"), tk("How an icon calls you when its app has an urgent window or a critical notification"), DOCK_ATTENTION),
        ("/dock/motion/indicator", tk("Running indicators"), tk("Dots grow in when an app opens a window and shrink away when it closes one"), DOCK_INDICATOR),
        ("/dock/motion/minimize", tk("Minimizing"), tk("The app's icon answers when one of its windows minimizes"), DOCK_MINIMIZE),
        ("/dock/motion/items", tk("Adding and removing"), tk("New icons grow in, removed ones shrink out and the others slide over, also when you reorder them"), DOCK_ITEMS),
        ("/dock/motion/reveal", tk("Showing and hiding"), tk("How the dock comes and goes, auto-hide included"), DOCK_REVEAL),
    ] {
        entries.push(s(Motion, tk("Dock"), path, label, detail, Choice(choices)));
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

    #[test]
    fn dock_motion_choices_match_the_config() {
        use mandara_config::dock::{AttentionFeedback, DockReveal, IndicatorMotion, ItemMotion, LaunchFeedback, MinimizeFeedback};
        fn ids<T: serde::Serialize>(all: &[T]) -> Vec<String> {
            all.iter().map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().to_string()).collect()
        }
        fn shown(choices: &[(&str, &str)]) -> Vec<String> {
            choices.iter().map(|(id, _)| id.to_string()).collect()
        }
        assert_eq!(ids(&LaunchFeedback::ALL), shown(super::DOCK_LAUNCH));
        assert_eq!(ids(&AttentionFeedback::ALL), shown(super::DOCK_ATTENTION));
        assert_eq!(ids(&IndicatorMotion::ALL), shown(super::DOCK_INDICATOR));
        assert_eq!(ids(&MinimizeFeedback::ALL), shown(super::DOCK_MINIMIZE));
        assert_eq!(ids(&ItemMotion::ALL), shown(super::DOCK_ITEMS));
        assert_eq!(ids(&DockReveal::ALL), shown(super::DOCK_REVEAL));
    }
}
