//! Permission generations survive coalescing, stale patches and file reloads.

use crate::{PluginGrant, PluginsConfig};

pub(crate) fn changed(before: &PluginsConfig, after: &PluginsConfig) -> bool {
    before.grants != after.grants || before.enabled != after.enabled || before.disabled != after.disabled
}

/// Keep empty revoked records: deleting their generation would make a later regrant reuse it.
pub(crate) fn reconcile(before: &PluginsConfig, after: &mut PluginsConfig) {
    if !changed(before, after) { return; }
    for (plugin, previous) in &before.grants {
        let enablement_changed = before.enabled.contains(plugin) != after.enabled.contains(plugin)
            || before.disabled.contains(plugin) != after.disabled.contains(plugin);
        let next = after.grants.entry(plugin.clone()).or_default();
        let authority_changed = previous.capabilities != next.capabilities || previous.folders != next.folders || enablement_changed;
        let minimum = if authority_changed { previous.generation.saturating_add(1) } else { previous.generation };
        next.generation = next.generation.max(minimum).min(PluginGrant::EXHAUSTED_GENERATION);
    }
    for (plugin, next) in &mut after.grants {
        if before.grants.contains_key(plugin) { continue; }
        next.generation = next.generation.max(u64::from(!next.is_empty())).min(PluginGrant::EXHAUSTED_GENERATION);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> PluginsConfig {
        let mut plugins = PluginsConfig::default();
        plugins.set_granted("workshop", "wallpaper.live", true);
        reconcile(&PluginsConfig::default(), &mut plugins);
        plugins
    }

    #[test]
    fn revoke_and_regrant_never_reuse_an_earlier_generation() {
        let first = allow();
        assert_eq!(first.grants["workshop"].generation, 1);
        let mut revoked = first.clone();
        revoked.grants.remove("workshop");
        reconcile(&first, &mut revoked);
        assert!(revoked.grants["workshop"].is_empty());
        assert_eq!(revoked.grants["workshop"].generation, 2);
        let mut restored = revoked.clone();
        restored.set_granted("workshop", "wallpaper.live", true);
        reconcile(&revoked, &mut restored);
        assert_eq!(restored.grants["workshop"].generation, 3);
        assert_eq!(restored.granted("workshop"), first.granted("workshop"));
    }

    #[test]
    fn stale_edits_cannot_rewind_generations_or_invalidate_another_plugin() {
        let first = allow();
        let mut current = first.clone();
        current.grants.get_mut("workshop").unwrap().generation = 8;
        let mut stale = first.clone();
        reconcile(&current, &mut stale);
        assert_eq!(stale.grants["workshop"].generation, 8);
        let mut edited = current.clone();
        edited.set_granted("other", "net:example.com", true);
        reconcile(&current, &mut edited);
        assert_eq!(edited.grants["workshop"].generation, 8);
        assert_eq!(edited.grants["other"].generation, 1);
    }

    #[test]
    fn disabling_and_reenabling_a_plugin_advances_its_permission_generation() {
        let first = allow();
        let mut off = first.clone();
        off.disabled.push("workshop".into());
        reconcile(&first, &mut off);
        assert_eq!(off.grants["workshop"].generation, 2);
        let mut on = off.clone();
        on.disabled.clear();
        reconcile(&off, &mut on);
        assert_eq!(on.grants["workshop"].generation, 3);
    }

    #[test]
    fn old_files_remain_unchanged_until_a_permission_edit_and_exhaustion_never_wraps() {
        let grant: PluginGrant = toml::from_str("capabilities = ['wallpaper.live']\n").unwrap();
        assert_eq!(grant.generation, 0);
        assert!(!toml::to_string(&grant).unwrap().contains("generation"));
        let mut previous = allow();
        previous.grants.get_mut("workshop").unwrap().generation = PluginGrant::EXHAUSTED_GENERATION;
        let mut next = previous.clone();
        next.grants.remove("workshop");
        reconcile(&previous, &mut next);
        assert_eq!(next.grants["workshop"].generation, PluginGrant::EXHAUSTED_GENERATION);
        assert!(toml::to_string(&next).is_ok());
    }
}
