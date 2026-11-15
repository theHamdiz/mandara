//! Plasma's login session. Most distributions package KWin on its own:
//! that package brings `kwin_wayland` but no session file, so Mandara
//! Login (and every other login screen) has nothing to offer until the
//! Plasma workspace package adds `wayland-sessions/plasma.desktop`.
//! `mandara setup --compositor kwin` and `mandara install` both check for
//! it and name the package to install.

use std::path::{Path, PathBuf};

use crate::family::{Family, OsRelease};
use crate::packages;

/// KWin's Wayland compositor.
pub const KWIN_PROGRAM: &str = "kwin_wayland";

/// Plasma's Wayland session files: `plasma.desktop` since Plasma 6,
/// `plasmawayland.desktop` in Plasma 5 and in openSUSE's `plasma6-session`.
pub const SESSION_FILES: &[&str] = &["plasma.desktop", "plasmawayland.desktop"];

/// The data directories login screens read sessions from: every entry of
/// `$XDG_DATA_DIRS`, then `/usr/local/share` and `/usr/share` when it
/// doesn't list them. Mandara Login reads the same ones.
pub fn data_roots(xdg_data_dirs: Option<&str>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = xdg_data_dirs.unwrap_or_default().split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    for d in ["/usr/local/share", "/usr/share"] {
        if !roots.iter().any(|r| r == Path::new(d)) {
            roots.push(PathBuf::from(d));
        }
    }
    roots
}

/// Whether one of `roots` (below `sysroot`: `/`, or a fixture tree in
/// tests) has Plasma's Wayland session.
pub fn has_session(sysroot: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| {
        let dir = sysroot.join(r.strip_prefix("/").unwrap_or(r)).join("wayland-sessions");
        SESSION_FILES.iter().any(|f| dir.join(f).is_file())
    })
}

/// What to tell someone whose KWin has no login session, or `None` when
/// KWin isn't installed or its session is there.
pub fn missing_session_hint(family: Family, os: &OsRelease, has_program: impl Fn(&str) -> bool, has_session: impl FnOnce() -> bool) -> Option<String> {
    if !has_program(KWIN_PROGRAM) || has_session() {
        return None;
    }
    Some(match packages::plasma_session_package(family, os) {
        Some(package) => mandara_i18n::tf(
            "KWin is installed, but Plasma’s login session isn’t, so the login screen can’t offer it. Install {package} from your distribution, then choose Plasma at login.",
            &[("package", &package)],
        ),
        None => mandara_i18n::t("KWin is installed, but Plasma’s login session isn’t, so the login screen can’t offer it. Install your distribution’s Plasma workspace package, then choose Plasma at login.").to_string(),
    })
}

/// [`missing_session_hint`] for this system: the family from
/// `os-release`, programs on `$PATH`, sessions in `$XDG_DATA_DIRS`.
pub fn missing_session_hint_here() -> Option<String> {
    let has_program = |p: &str| crate::host::find_program(p, std::env::var_os("PATH").as_deref()).is_some();
    let os = ["/etc/os-release", "/usr/lib/os-release"].iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let os = OsRelease::parse(&os);
    let family = Family::detect(&os, has_program);
    let roots = data_roots(std::env::var("XDG_DATA_DIRS").ok().as_deref());
    missing_session_hint(family, &os, has_program, || has_session(Path::new("/"), &roots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_roots_follow_xdg_then_the_system_folders_once() {
        assert_eq!(data_roots(None), [PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]);
        assert_eq!(data_roots(Some("/nix/share::/usr/share")), [PathBuf::from("/nix/share"), PathBuf::from("/usr/share"), PathBuf::from("/usr/local/share")]);
    }

    #[test]
    fn finds_plasma_6_and_plasma_5_sessions_in_any_root() {
        let dir = tempfile::tempdir().unwrap();
        let roots = data_roots(Some("/opt/share"));
        assert!(!has_session(dir.path(), &roots));
        // Another compositor's session isn't Plasma's.
        let usr = dir.path().join("usr/share/wayland-sessions");
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(usr.join("niri.desktop"), "[Desktop Entry]\n").unwrap();
        assert!(!has_session(dir.path(), &roots));
        std::fs::write(usr.join("plasmawayland.desktop"), "[Desktop Entry]\n").unwrap();
        assert!(has_session(dir.path(), &roots));
        // FreeBSD's ports install into /usr/local/share.
        let other = tempfile::tempdir().unwrap();
        let local = other.path().join("usr/local/share/wayland-sessions");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("plasma.desktop"), "[Desktop Entry]\n").unwrap();
        assert!(has_session(other.path(), &roots));
    }

    #[test]
    fn hint_names_the_package_only_when_kwin_lacks_its_session() {
        let kwin = |p: &str| p == KWIN_PROGRAM;
        let os = OsRelease::default();
        assert_eq!(missing_session_hint(Family::Arch, &os, |_| false, || false), None);
        assert_eq!(missing_session_hint(Family::Arch, &os, kwin, || true), None);
        let arch = missing_session_hint(Family::Arch, &os, kwin, || false).unwrap();
        assert!(arch.contains("plasma-workspace") && arch.contains("login screen"), "{arch}");
        assert!(missing_session_hint(Family::Suse, &os, kwin, || false).unwrap().contains("plasma6-session"));
        assert!(missing_session_hint(Family::FreeBSD, &os, kwin, || false).unwrap().contains("plasma6-plasma-workspace"));
        let unknown = missing_session_hint(Family::Unknown, &os, kwin, || false).unwrap();
        assert!(unknown.contains("Plasma workspace package"), "{unknown}");
    }

    #[test]
    fn ubuntu_and_its_derivatives_use_their_own_session_package() {
        let debian = OsRelease::parse("ID=debian\n");
        let ubuntu = OsRelease::parse("ID=ubuntu\nID_LIKE=debian\n");
        let neon = OsRelease::parse("ID=neon\nID_LIKE=\"ubuntu debian\"\n");
        assert_eq!(packages::plasma_session_package(Family::Debian, &debian), Some("plasma-workspace"));
        for os in [&ubuntu, &neon] {
            assert_eq!(packages::plasma_session_package(Family::Debian, os), Some("plasma-session-wayland"));
        }
    }

    #[test]
    fn every_family_with_packages_names_a_plasma_session_package() {
        for family in Family::ALL {
            assert_eq!(packages::plasma_session_package(family, &OsRelease::default()).is_some(), packages::for_family(family).is_some(), "{family}");
        }
    }
}
