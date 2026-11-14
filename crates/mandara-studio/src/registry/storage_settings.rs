//! Backup locations belong in plugin settings; secrets stay outside Mandara's config.

use mandara_i18n::tk;
use serde_json::Value;
use super::{Control, Section, Setting, sd};

pub(super) fn entries() -> Vec<Setting> {
    vec![
        sd(Section::Plugins, tk("Backups"), "/plugins/settings/backups/repository", tk("Backup repository"), tk("An absolute path on this computer or a mounted backup drive. Initialize a new repository from the Backups panel."), Control::Text { placeholder: "/media/backup/encrypted" }, Value::from("")),
        sd(Section::Plugins, tk("Backups"), "/plugins/settings/backups/password_file", tk("Backup password file"), tk("An existing password file owned by you with permissions 0600. Mandara stores its path, never its contents."), Control::Text { placeholder: "/home/you/.config/restic/password" }, Value::from("")),
        sd(Section::Plugins, tk("Backups"), "/plugins/settings/backups/sources", tk("Source folders"), tk("Absolute folder paths, separated by commas. The backup repository itself is excluded."), Control::List { placeholder: "/home/you/Documents, /home/you/Pictures" }, Value::Array(Vec::new())),
        sd(Section::Plugins, tk("Backups"), "/plugins/settings/backups/snapshot_source", tk("Local snapshot source"), tk("An existing Btrfs subvolume. A snapshot does not include nested subvolumes."), Control::Text { placeholder: "/" }, Value::from("")),
        sd(Section::Plugins, tk("Backups"), "/plugins/settings/backups/snapshot_directory", tk("Local snapshot folder"), tk("An existing folder on the same mounted Btrfs filesystem. New snapshots are read-only."), Control::Text { placeholder: "/.snapshots" }, Value::from("")),
        sd(Section::Plugins, tk("Backups"), "/plugins/settings/backups/cleanup_age_days", tk("Cache age"), tk("Only cache files older than this are selected. Trash items are reviewed separately."), Control::Slider {
            min: mandara_services::backups::cleanup::AGE_DAYS_RANGE.0 as f64, max: mandara_services::backups::cleanup::AGE_DAYS_RANGE.1 as f64, step: 1.0, unit: " d",
        }, Value::from(mandara_services::backups::cleanup::DEFAULT_AGE_DAYS)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_defaults_do_not_select_private_folders_or_store_a_password() {
        let entries = entries();
        assert_eq!(entries.len(), 6);
        assert!(entries.iter().all(|entry| entry.path.starts_with("/plugins/settings/backups/")));
        assert!(entries.iter().all(|entry| !entry.path.ends_with("/password")));
    }
}
