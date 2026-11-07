//! Accessibility and per-surface playback controls.
use super::{Control::*, Section::Motion, Setting, s};
use mandara_i18n::tk;

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
