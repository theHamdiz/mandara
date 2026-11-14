//! An explicitly selected MangoWM config must not be shadowed by a new default.

use anyhow::{Result, ensure};
use mandara_config::compositor::Target;
use std::path::Path;

pub(super) fn check_path() -> Result<()> {
    if mandara_compositor::detect().is_none_or(|kind| kind.target() != Target::Mango) {
        return Ok(());
    }
    let native = mandara_compositor::native_config::running(Target::Mango)?;
    let Some(actual) = native.file else {
        return Ok(());
    };
    ensure!(
        supported(&Target::Mango.user_config(), &actual, native.explicit_file),
        "{}",
        mandara_i18n::tf(
            "MangoWM uses a custom config at {path}. Automatic setup requires its default config location. Your compositor files were left unchanged.",
            &[("path", &actual.display())]
        )
    );
    Ok(())
}

fn supported(user: &Path, actual: &Path, explicit: bool) -> bool {
    if !explicit
        && [
            Path::new("/etc/mango/config.conf"),
            Path::new("/etc/mango/config.toml"),
        ]
        .contains(&actual)
    {
        return true;
    }
    user.is_file()
        && (user == actual
            || user.extension() == actual.extension()
                && std::fs::canonicalize(user).is_ok_and(|user| {
                    std::fs::canonicalize(actual).is_ok_and(|actual| actual == user)
                }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_to_seed_but_explicit_and_foreign_roots_are_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("config.conf");
        let system = Path::new("/etc/mango/config.conf");
        assert!(supported(&user, system, false));
        assert!(!supported(&user, system, true));
        assert!(!supported(&user, &user, true));
        let custom = directory.path().join("other.conf");
        std::fs::write(&user, "border_radius=6\n").unwrap();
        std::fs::write(&custom, "border_radius=12\n").unwrap();
        assert!(supported(&user, &user, true));
        assert!(!supported(&user, &custom, true));
        let alias = directory.path().join("alias.conf");
        std::os::unix::fs::symlink(&user, &alias).unwrap();
        assert!(supported(&user, &alias, true));
        let other_format = directory.path().join("alias.toml");
        std::os::unix::fs::symlink(&user, &other_format).unwrap();
        assert!(!supported(&user, &other_format, true));
        assert_eq!(
            std::fs::read_to_string(custom).unwrap(),
            "border_radius=12\n"
        );
    }
}
