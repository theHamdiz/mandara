//! The taskbar bar item's settings (`[plugins.settings.taskbar]`): what its
//! app buttons look like and how they group windows.
//!
//! Read as one struct ([`TaskbarSettings::from_plugins`]): a missing table
//! or key keeps the taskbar exactly as it always looked (buttons straight
//! on the bar, a line under running apps, a soft fill on the focused one),
//! one bad value never resets the others, and every number is clamped to
//! its named [`Span`] in [`ranges`].

use serde::{Deserialize, Serialize};

use crate::grain::{Grain, GrainStyle};
use crate::surfaces::Span;

/// The taskbar's plugin id (its settings table is `[plugins.settings.taskbar]`).
pub const TASKBAR_SETTINGS: &str = "taskbar";

/// Default values.
pub mod defaults {
    /// An app icon, logical px (before the bar's icon scale).
    pub const ICON_PX: f32 = 18.0;
    /// Gap between buttons, logical px.
    pub const SPACING_PX: f32 = 2.0;
    /// Widest an app label grows, logical px.
    pub const LABEL_WIDTH_PX: f32 = 140.0;
}

/// Named bounds of the taskbar's numbers (logical px). Icons stay small
/// enough for a slim bar.
pub mod ranges {
    use super::Span;

    pub const ICON_SIZE: Span = Span::new(14.0, 24.0, 1.0);
    pub const SPACING: Span = Span::new(0.0, 12.0, 1.0);
    pub const LABEL_WIDTH: Span = Span::new(48.0, 240.0, 4.0);
}

/// What the buttons sit on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskbarMaterial {
    /// Straight on the bar (as it always was).
    #[default]
    None,
    /// A tray of the dock's classic glass.
    Classic,
    /// A tray of liquid glass.
    Liquid,
}

/// The mark under a running app.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskbarIndicator {
    /// A short line that widens on the focused app.
    #[default]
    Line,
    /// One dot.
    Dot,
    /// A dot per window, up to three.
    Dots,
    /// A short line of light.
    Glow,
    None,
}

/// How the focused app's button stands out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FocusStyle {
    /// A soft fill.
    #[default]
    Tint,
    /// A raised glass key with a specular rim.
    Raised,
    /// A wash of the accent colour.
    Accent,
    /// Only its indicator.
    None,
}

/// Which buttons show their app's name.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskbarLabels {
    /// Icons only.
    #[default]
    None,
    /// The focused app.
    Focused,
    /// Every running app.
    Running,
}

/// `[plugins.settings.taskbar]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskbarSettings {
    pub material: TaskbarMaterial,
    /// Grain on the tray (with a material).
    pub grain: GrainStyle,
    pub grain_amount: f32,
    pub indicator: TaskbarIndicator,
    pub focus_style: FocusStyle,
    pub labels: TaskbarLabels,
    pub label_width: f32,
    /// One button per app (off: one per window).
    pub group_windows: bool,
    /// A count on apps with two or more windows.
    pub window_count: bool,
    /// Apps asking for attention turn coral.
    pub urgent: bool,
    pub icon_size: f32,
    pub spacing: f32,
}

impl Default for TaskbarSettings {
    fn default() -> Self {
        Self {
            material: TaskbarMaterial::None,
            grain: GrainStyle::None,
            grain_amount: crate::grain::defaults::AMOUNT,
            indicator: TaskbarIndicator::Line,
            focus_style: FocusStyle::Tint,
            labels: TaskbarLabels::None,
            label_width: defaults::LABEL_WIDTH_PX,
            group_windows: true,
            window_count: false,
            urgent: true,
            icon_size: defaults::ICON_PX,
            spacing: defaults::SPACING_PX,
        }
    }
}

impl TaskbarSettings {
    /// The taskbar's settings from `[plugins.settings.taskbar]`, sanitized.
    pub fn from_plugins(plugins: &crate::PluginsConfig) -> Self {
        let Some(table) = plugins.settings.get("taskbar") else { return Self::default() };
        if let Ok(s) = toml::Value::Table(table.clone()).try_into::<Self>() {
            return s.sanitized();
        }
        // Key by key, so one bad value never resets the others.
        let mut out = toml::Table::try_from(Self::default()).unwrap_or_default();
        for (k, v) in table {
            let mut trial = out.clone();
            trial.insert(k.clone(), v.clone());
            if toml::Value::Table(trial.clone()).try_into::<Self>().is_ok() {
                out = trial;
            }
        }
        toml::Value::Table(out).try_into::<Self>().unwrap_or_default().sanitized()
    }

    /// Every number inside its range.
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        Self {
            grain_amount: crate::grain::ranges::AMOUNT.clamp(self.grain_amount, d.grain_amount),
            label_width: ranges::LABEL_WIDTH.clamp(self.label_width, d.label_width),
            icon_size: ranges::ICON_SIZE.clamp(self.icon_size, d.icon_size),
            spacing: ranges::SPACING.clamp(self.spacing, d.spacing),
            ..self.clone()
        }
    }

    /// The tray's grain.
    pub fn grain(&self) -> Grain {
        Grain { amount: self.grain_amount, ..Grain::with_style(self.grain) }.sanitized()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(table: &str) -> TaskbarSettings {
        let mut plugins = crate::PluginsConfig::default();
        plugins.settings.insert("taskbar".into(), toml::from_str(table).unwrap());
        TaskbarSettings::from_plugins(&plugins)
    }

    #[test]
    fn no_table_is_the_taskbar_as_it_was() {
        assert_eq!(TaskbarSettings::from_plugins(&crate::PluginsConfig::default()), TaskbarSettings::default());
        let d = TaskbarSettings::default();
        assert_eq!((d.material, d.indicator, d.focus_style, d.labels), (TaskbarMaterial::None, TaskbarIndicator::Line, FocusStyle::Tint, TaskbarLabels::None));
        assert!(d.group_windows && !d.window_count);
        assert_eq!(d.icon_size, defaults::ICON_PX);
        assert!(!d.grain().is_on());
        assert_eq!(d.sanitized(), d);
    }

    #[test]
    fn values_are_read_clamped_and_kept_one_by_one() {
        let s = with("material = \"liquid\"\nlabels = \"running\"\nicon_size = 99\nspacing = -4\nindicator = \"bogus\"\ngrain = \"film\"\ngrain_amount = 1.0\n");
        assert_eq!((s.material, s.labels), (TaskbarMaterial::Liquid, TaskbarLabels::Running));
        assert_eq!(s.indicator, TaskbarIndicator::Line, "a bad value falls back alone");
        assert_eq!((s.icon_size, s.spacing), (ranges::ICON_SIZE.max, ranges::SPACING.min));
        assert!(s.grain().is_on() && s.grain().amount() <= crate::grain::ranges::AMOUNT.max);
    }

    /// Every field is a key of the table Studio writes.
    #[test]
    fn keys_are_the_studio_paths() {
        let keys: Vec<String> = toml::Table::try_from(TaskbarSettings::default()).unwrap().keys().cloned().collect();
        for k in ["material", "grain", "grain_amount", "indicator", "focus_style", "labels", "label_width", "group_windows", "window_count", "urgent", "icon_size", "spacing"] {
            assert!(keys.iter().any(|x| x == k), "{k}");
        }
        assert_eq!(keys.len(), 12);
    }
}
