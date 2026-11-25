//! Lifecycle follows plugin switches without retaining their settings tables.

use mandara_config::PluginsConfig;

struct Selection {
    enabled: Vec<String>,
    disabled: Vec<String>,
    removed: Vec<String>,
}

impl From<&PluginsConfig> for Selection {
    fn from(plugins: &PluginsConfig) -> Self {
        Self { enabled: plugins.enabled.clone(), disabled: plugins.disabled.clone(), removed: plugins.removed.clone() }
    }
}

/// Remembers only the lists that change lifecycle behavior. The first sync
/// always runs; subsequent settings or layout edits compare borrowed lists.
#[derive(Default)]
pub(super) struct Switches {
    previous: Option<Selection>,
}

impl Switches {
    pub const fn new() -> Self {
        Self { previous: None }
    }

    /// Preserve exact list order and reuse owned strings after actual switch edits.
    pub fn changed(&mut self, plugins: &PluginsConfig) -> bool {
        if let Some(previous) = &mut self.previous {
            if previous.enabled == plugins.enabled && previous.disabled == plugins.disabled && previous.removed == plugins.removed {
                return false;
            }
            previous.enabled.clone_from(&plugins.enabled);
            previous.disabled.clone_from(&plugins.disabled);
            previous.removed.clone_from(&plugins.removed);
        } else {
            self.previous = Some(plugins.into());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::{LayoutLane, LayoutSlot, ShellConfig};

    /// Ordinary settings and remembered placement edits do not rerun plugin stops.
    #[test]
    fn initial_sync_runs_once_without_retaining_unrelated_settings() {
        let mut config = ShellConfig::default();
        let mut switches = Switches::new();
        assert!(switches.changed(&config.plugins));
        assert!(!switches.changed(&config.plugins));
        config.plugins.settings.entry("feeds".into()).or_default().insert("subscriptions".into(), toml::Value::String("https://example.org/feed".into()));
        config.plugins.removed_places.insert("feeds".into(), vec![LayoutSlot::new(LayoutLane::Sidebar, 2)]);
        config.plugins.defaults_version += 1;
        assert!(!switches.changed(&config.plugins));
        eprintln!("plugin lifecycle key: previous PluginsConfig={} B plus settings, switch lists={} B", size_of::<PluginsConfig>(), size_of::<Switches>());
    }

    /// The real enable, disable, remove and restore operations each invalidate once.
    #[test]
    fn lifecycle_transitions_keep_the_previous_three_list_contract() {
        use mandara_plugins::lifecycle::{remove, restore, set_enabled};
        let mut config = ShellConfig::default();
        let mut switches = Switches::new();
        assert!(switches.changed(&config.plugins));
        set_enabled(&mut config, "feeds", "Feeds", true, None).unwrap();
        assert!(switches.changed(&config.plugins));
        assert!(!switches.changed(&config.plugins));
        set_enabled(&mut config, "feeds", "Feeds", false, None).unwrap();
        assert!(switches.changed(&config.plugins));
        remove(&mut config, "feeds", "Feeds").unwrap();
        assert!(switches.changed(&config.plugins));
        restore(&mut config, "feeds", None);
        assert!(switches.changed(&config.plugins));
        assert!(!switches.changed(&config.plugins));
        config.plugins.enabled.push("notes".into());
        assert!(switches.changed(&config.plugins));
        config.plugins.enabled.reverse();
        assert!(switches.changed(&config.plugins), "ordering stays part of the existing lifecycle key");
    }
}
