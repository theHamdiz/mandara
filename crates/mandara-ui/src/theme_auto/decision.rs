//! Remembers automatic boundaries independently of the user's selected mode.

use std::path::Path;

use anyhow::{Context as _, Result};
use mandara_config::{ThemeAuto, ThemeConfig, ThemeMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Key {
    auto: ThemeAuto,
    light_at: String,
    dark_at: String,
}

impl From<&ThemeConfig> for Key {
    fn from(theme: &ThemeConfig) -> Self {
        Self { auto: theme.auto, light_at: theme.light_at.clone(), dark_at: theme.dark_at.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Saved {
    key: Key,
    applied: Option<bool>,
}

impl Saved {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading automatic theme state").map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        mandara_config::store::write_atomic(path, &serde_json::to_vec(self)?)
    }
}

pub(super) struct Decision {
    saved: Saved,
    /// Older installs have no boundary record; keep their selected mode.
    preserve_first: bool,
}

#[derive(Default)]
pub(super) struct Change {
    pub mode: Option<ThemeMode>,
    pub save: Option<Saved>,
}

impl Decision {
    pub fn new(theme: &ThemeConfig, saved: Option<Saved>) -> Self {
        let preserve_first = saved.is_none();
        Self { saved: saved.unwrap_or_else(|| Saved { key: theme.into(), applied: None }), preserve_first }
    }

    pub fn evaluate(&mut self, theme: &ThemeConfig, desired: Option<bool>) -> Change {
        let mut change = Change::default();
        let key = Key::from(theme);
        if self.saved.key != key {
            self.saved.key = key;
            self.saved.applied = None;
            self.preserve_first = false;
            change.save = Some(self.saved.clone());
        }
        let Some(dark) = desired else { return change };
        if self.saved.applied == Some(dark) {
            return change;
        }
        self.saved.applied = Some(dark);
        change.save = Some(self.saved.clone());
        if !self.preserve_first && theme.mode.is_dark() != dark {
            change.mode = Some(if dark { ThemeMode::Dark } else { ThemeMode::Light });
        }
        self.preserve_first = false;
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduled_light() -> ThemeConfig {
        ThemeConfig { mode: ThemeMode::Light, auto: ThemeAuto::Schedule, ..ThemeConfig::default() }
    }

    #[test]
    fn upgrade_keeps_selected_mode_until_the_next_boundary() {
        let mut theme = scheduled_light();
        let mut decision = Decision::new(&theme, None);
        assert_eq!(decision.evaluate(&theme, Some(true)).mode, None);
        assert_eq!(decision.evaluate(&theme, Some(false)).mode, None);
        assert_eq!(decision.evaluate(&theme, Some(true)).mode, Some(ThemeMode::Dark));
        theme.mode = ThemeMode::Dark;
        assert_eq!(decision.evaluate(&theme, Some(true)).mode, None);
    }

    #[test]
    fn manual_override_survives_restart_and_state_roundtrip() {
        let theme = scheduled_light();
        let mut decision = Decision::new(&theme, None);
        let saved = decision.evaluate(&theme, Some(true)).save.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/theme-auto.json");
        saved.save(&path).unwrap();
        let mut restarted = Decision::new(&theme, Saved::load(&path).unwrap());
        let change = restarted.evaluate(&theme, Some(true));
        assert_eq!(change.mode, None);
        assert!(change.save.is_none(), "an unchanged decision does no file I/O");
        restarted.evaluate(&theme, Some(false));
        assert_eq!(restarted.evaluate(&theme, Some(true)).mode, Some(ThemeMode::Dark));
    }

    #[test]
    fn boundary_passed_while_stopped_still_applies() {
        let mut theme = scheduled_light();
        let mut decision = Decision::new(&theme, None);
        let saved = decision.evaluate(&theme, Some(true)).save;
        theme.mode = ThemeMode::Dark;
        let mut restarted = Decision::new(&theme, saved);
        assert_eq!(restarted.evaluate(&theme, Some(false)).mode, Some(ThemeMode::Light));
    }

    #[test]
    fn changing_automation_applies_even_after_an_unavailable_portal() {
        let mut theme = ThemeConfig { mode: ThemeMode::Light, ..ThemeConfig::default() };
        let mut decision = Decision::new(&theme, None);
        assert!(decision.evaluate(&theme, None).save.is_none());
        theme.auto = ThemeAuto::System;
        assert_eq!(decision.evaluate(&theme, None).mode, None);
        assert_eq!(decision.evaluate(&theme, Some(true)).mode, Some(ThemeMode::Dark));
    }

    #[test]
    fn schedule_edit_applies_without_waiting_for_a_boundary() {
        let mut theme = scheduled_light();
        let mut decision = Decision::new(&theme, None);
        decision.evaluate(&theme, Some(true));
        theme.dark_at = "18:00".into();
        assert_eq!(decision.evaluate(&theme, Some(true)).mode, Some(ThemeMode::Dark));
    }

    #[test]
    fn absent_and_invalid_state_never_replace_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme-auto.json");
        assert!(Saved::load(&path).unwrap().is_none());
        std::fs::write(&path, "invalid state").unwrap();
        assert!(Saved::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid state");
    }
}
