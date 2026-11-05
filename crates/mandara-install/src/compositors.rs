//! The compositors Mandara runs on, and where each distribution family
//! ships them: what `mandara install` offers when none is installed, or
//! installs for `--compositor NAME`.
//!
//! Only official repositories: a compositor a family carries only in a
//! third-party source (the AUR, Copr, a PPA, Gentoo's GURU) isn't offered
//! there, and the installer never adds a repository. Names were checked
//! against each family's repositories (Arch extra, Debian sid and Ubuntu
//! 26.04, Fedora 44, openSUSE Tumbleweed, Void, Alpine 3.24); where a
//! release may lack a package (Debian stable has no Hyprland), the
//! installer also asks the package manager before offering it
//! ([`crate::index::available_query`]).
//!
//! A new compositor (once `mandara setup` supports it) is one more entry
//! in [`SUPPORTED`].

use crate::family::Family;

/// A compositor Mandara supports.
#[derive(Debug)]
pub struct Compositor {
    /// The name `mandara setup --compositor` takes.
    pub id: &'static str,
    /// Other names `--compositor` accepts for it.
    pub aliases: &'static [&'static str],
    /// How it is shown.
    pub label: &'static str,
    /// Programs whose presence means it is installed.
    pub programs: &'static [&'static str],
    /// Its package in each family's official repositories.
    pub packages: &'static [(Family, &'static str)],
}

/// Every supported compositor, in the order the installer recommends them
/// (the first one available is the default choice).
pub const SUPPORTED: &[Compositor] = &[
    // Plasma is an existing desktop session, not an extra standalone
    // compositor package the installer offers to add to another desktop.
    Compositor { id: "kwin", aliases: &["kde", "plasma"], label: "KWin (Plasma)", programs: &["kwin_wayland"], packages: &[] },
    Compositor { id: "mango", aliases: &["mangowm"], label: "MangoWM", programs: &["mango"], packages: &[(Family::Arch, "mangowm"), (Family::Alpine, "mangowm")] },
    Compositor { id: "niri", aliases: &[], label: "Niri", programs: &["niri"], packages: &[(Family::Arch, "niri"), (Family::Fedora, "niri"), (Family::Suse, "niri"), (Family::Void, "niri"), (Family::Alpine, "niri")] },
    Compositor { id: "hyprland", aliases: &["hypr"], label: "Hyprland", programs: &["Hyprland", "hyprland"], packages: &[(Family::Arch, "hyprland"), (Family::Debian, "hyprland"), (Family::Suse, "hyprland"), (Family::Alpine, "hyprland")] },
    Compositor { id: "sway", aliases: &[], label: "Sway", programs: &["sway"], packages: &[(Family::Arch, "sway"), (Family::Debian, "sway"), (Family::Fedora, "sway"), (Family::Suse, "sway"), (Family::Void, "sway"), (Family::Alpine, "sway")] },
    // River's classic line (0.3): Arch packages it as `river-classic`;
    // elsewhere `river` may be 0.4+, which Mandara can't drive.
    Compositor { id: "river", aliases: &[], label: "River Classic", programs: &["river"], packages: &[(Family::Arch, "river-classic")] },
    // Not packaged by any distribution yet: supported once installed.
    Compositor { id: "umbriel", aliases: &[], label: "Umbriel", programs: &["umbriel"], packages: &[] },
];

impl Compositor {
    /// Its package on `family`, when an official repository carries it.
    pub fn package(&self, family: Family) -> Option<&'static str> {
        self.packages.iter().find(|(f, _)| *f == family).map(|(_, p)| *p)
    }

    /// Whether one of its programs is installed.
    pub fn installed(&self, has_program: impl Fn(&str) -> bool) -> bool {
        self.programs.iter().any(|p| has_program(p))
    }
}

/// The compositor `--compositor` names (its id or an alias).
pub fn by_name(name: &str) -> Option<&'static Compositor> {
    let name = name.to_ascii_lowercase();
    SUPPORTED.iter().find(|c| c.id == name || c.aliases.contains(&name.as_str()))
}

/// Whether any supported compositor is installed.
pub fn any_installed(has_program: impl Fn(&str) -> bool) -> bool {
    SUPPORTED.iter().any(|c| c.installed(&has_program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setup_name_is_a_compositor() {
        for name in crate::args::COMPOSITORS {
            assert!(by_name(name).is_some(), "{name}");
        }
        for c in SUPPORTED {
            assert!(crate::args::COMPOSITORS.contains(&c.id), "{}", c.id);
            assert!(c.aliases.iter().all(|a| crate::args::COMPOSITORS.contains(a)), "{}", c.id);
        }
        assert_eq!(by_name("HYPR").map(|c| c.id), Some("hyprland"));
        assert!(by_name("weston").is_none());
    }

    #[test]
    fn package_map() {
        let names = |family| SUPPORTED.iter().filter_map(|c| c.package(family).map(|p| (c.id, p))).collect::<Vec<_>>();
        assert_eq!(names(Family::Arch), [("mango", "mangowm"), ("niri", "niri"), ("hyprland", "hyprland"), ("sway", "sway"), ("river", "river-classic")]);
        assert_eq!(names(Family::Alpine), [("mango", "mangowm"), ("niri", "niri"), ("hyprland", "hyprland"), ("sway", "sway")]);
        assert_eq!(names(Family::Debian), [("hyprland", "hyprland"), ("sway", "sway")]);
        assert_eq!(names(Family::Fedora), [("niri", "niri"), ("sway", "sway")]);
        assert_eq!(names(Family::Suse), [("niri", "niri"), ("hyprland", "hyprland"), ("sway", "sway")]);
        assert_eq!(names(Family::Void), [("niri", "niri"), ("sway", "sway")]);
        // Overlays and hand-built systems: nothing offered.
        for f in [Family::Gentoo, Family::Solus, Family::NixOS, Family::Unknown] {
            assert!(names(f).is_empty(), "{f}");
        }
    }

    #[test]
    fn detection() {
        assert!(!any_installed(|_| false));
        assert!(any_installed(|p| p == "Hyprland"));
        assert!(any_installed(|p| p == "umbriel"));
        assert!(any_installed(|p| p == "kwin_wayland"));
        assert_eq!(by_name("plasma").unwrap().id, "kwin");
        assert!(by_name("hyprland").unwrap().installed(|p| p == "hyprland"));
        assert!(!by_name("niri").unwrap().installed(|p| p == "mango"));
    }
}
