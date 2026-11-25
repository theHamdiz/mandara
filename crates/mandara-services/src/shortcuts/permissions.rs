//! Immutable sources stay inspectable without offering an unusable Save action.

use mandara_config::keybinds::Scan;
use std::path::Path;

const READ_ONLY: &str = mandara_i18n::tk(
    "This configuration is read-only. Edit its source or create a writable user override.",
);

fn writable(path: &Path) -> bool {
    use rustix::fs::{Access, access};
    let existing = path.ancestors().find(|path| path.exists());
    let Some(existing) = existing else {
        return false;
    };
    if std::fs::metadata(existing).is_ok_and(|metadata| metadata.permissions().readonly()) {
        return false;
    }
    if access(existing, Access::WRITE_OK).is_err() {
        return false;
    }
    let parent = if existing.is_file() {
        existing.parent()
    } else {
        Some(existing)
    };
    parent.is_some_and(|parent| access(parent, Access::WRITE_OK | Access::EXEC_OK).is_ok())
}

pub(super) fn mark(scan: &mut Scan, destination: Option<&Path>) {
    for source in &mut scan.sources {
        if !writable(destination.unwrap_or(&source.target)) {
            for row in &mut source.bindings {
                row.read_only.get_or_insert(READ_ONLY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::{compositor::Target, keybinds};
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    #[test]
    fn immutable_symlink_targets_are_locked_but_personal_overrides_are_writable() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("immutable");
        let alias = directory.path().join("config.conf");
        std::fs::write(&file, "bind=SUPER,Q,killclient\n").unwrap();
        symlink(&file, &alias).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        let mut scan = keybinds::scan_from(Target::Mango, &alias);
        mark(&mut scan, None);
        assert_eq!(scan.sources[0].bindings[0].read_only, Some(READ_ONLY));
        let mut scan = keybinds::scan_from(Target::Mango, &alias);
        mark(
            &mut scan,
            Some(&directory.path().join("personal/v1/custom")),
        );
        assert!(scan.sources[0].bindings[0].read_only.is_none());
        assert!(std::fs::symlink_metadata(alias).unwrap().is_symlink());
    }
}
