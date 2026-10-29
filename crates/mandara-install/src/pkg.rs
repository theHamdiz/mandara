//! The package-manager abstraction shared by `mandara install` and
//! `mandara greeter use --install`: which manager a system uses, the argv
//! that installs packages unattended, and how to get root for it (sudo,
//! doas, run0 or pkexec). Everything is argv; nothing goes through a shell.

use crate::family::Family;

/// A package manager Mandara can drive.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PackageManager {
    Pacman,
    AptGet,
    Dnf,
    Zypper,
    Xbps,
    Apk,
    Emerge,
    Eopkg,
}

impl PackageManager {
    /// Every manager, in the order an unknown system is probed.
    pub const ALL: [PackageManager; 8] = [PackageManager::Pacman, PackageManager::AptGet, PackageManager::Dnf, PackageManager::Zypper, PackageManager::Xbps, PackageManager::Apk, PackageManager::Emerge, PackageManager::Eopkg];

    /// The program that identifies it.
    pub fn binary(self) -> &'static str {
        match self {
            PackageManager::Pacman => "pacman",
            PackageManager::AptGet => "apt-get",
            PackageManager::Dnf => "dnf",
            PackageManager::Zypper => "zypper",
            PackageManager::Xbps => "xbps-install",
            PackageManager::Apk => "apk",
            PackageManager::Emerge => "emerge",
            PackageManager::Eopkg => "eopkg",
        }
    }

    /// The family whose package names it takes.
    pub fn family(self) -> Family {
        match self {
            PackageManager::Pacman => Family::Arch,
            PackageManager::AptGet => Family::Debian,
            PackageManager::Dnf => Family::Fedora,
            PackageManager::Zypper => Family::Suse,
            PackageManager::Xbps => Family::Void,
            PackageManager::Apk => Family::Alpine,
            PackageManager::Emerge => Family::Gentoo,
            PackageManager::Eopkg => Family::Solus,
        }
    }

    /// The manager of `family` (`None` for NixOS and unknown systems).
    pub fn for_family(family: Family) -> Option<PackageManager> {
        PackageManager::ALL.into_iter().find(|pm| pm.family() == family)
    }

    /// The manager to use: `family`'s own when installed, else the first
    /// installed one. `None` when no known manager is installed.
    pub fn detect(family: Family, has_program: impl Fn(&str) -> bool) -> Option<PackageManager> {
        PackageManager::for_family(family).filter(|pm| has_program(pm.binary())).or_else(|| PackageManager::ALL.into_iter().find(|pm| has_program(pm.binary())))
    }

    /// The argv prefix that installs packages without asking (the package
    /// names are appended). Already-installed packages are kept as they are.
    fn install_prefix(self) -> &'static [&'static str] {
        match self {
            PackageManager::Pacman => &["pacman", "-S", "--needed", "--noconfirm"],
            PackageManager::AptGet => &["env", "DEBIAN_FRONTEND=noninteractive", "apt-get", "install", "-y", "--no-install-recommends"],
            PackageManager::Dnf => &["dnf", "install", "-y"],
            PackageManager::Zypper => &["zypper", "--non-interactive", "install", "--no-recommends"],
            PackageManager::Xbps => &["xbps-install", "-Sy"],
            PackageManager::Apk => &["apk", "add"],
            PackageManager::Emerge => &["emerge", "--noreplace"],
            PackageManager::Eopkg => &["eopkg", "install", "-y"],
        }
    }

    /// argv installing `packages` (run it as root).
    pub fn install_argv<S: AsRef<str>>(self, packages: &[S]) -> Vec<String> {
        self.install_prefix().iter().map(|a| a.to_string()).chain(packages.iter().map(|p| p.as_ref().to_string())).collect()
    }

    /// argv refreshing the package index before installing, for managers
    /// whose install command doesn't (run it as root; a failure is harmless).
    /// Never pacman: `pacman -Sy` followed by `-S` is a partial upgrade
    /// (new packages built against libraries newer than the installed
    /// ones), which Arch doesn't support; `-S --needed` uses the current
    /// database, and a full `-Syu` is the user's call, not the installer's.
    pub fn refresh_argv(self) -> Option<Vec<String>> {
        let argv: &[&str] = match self {
            PackageManager::AptGet => &["apt-get", "update", "-qq"],
            _ => return None,
        };
        Some(argv.iter().map(|a| a.to_string()).collect())
    }
}

/// How a user process gets root for one command.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Privilege {
    /// Already root: commands run as they are.
    Root,
    Sudo,
    Doas,
    Run0,
    Pkexec,
    /// No helper installed: root commands can't run.
    Unavailable,
}

impl Privilege {
    /// Helpers in order of preference.
    pub const HELPERS: [Privilege; 4] = [Privilege::Sudo, Privilege::Doas, Privilege::Run0, Privilege::Pkexec];

    /// The helper to use: none when `is_root`, else the first installed.
    pub fn detect(is_root: bool, has_program: impl Fn(&str) -> bool) -> Privilege {
        if is_root {
            return Privilege::Root;
        }
        Privilege::HELPERS.into_iter().find(|p| p.program().is_some_and(&has_program)).unwrap_or(Privilege::Unavailable)
    }

    /// The helper's program (`None` for root and when unavailable).
    pub fn program(self) -> Option<&'static str> {
        match self {
            Privilege::Sudo => Some("sudo"),
            Privilege::Doas => Some("doas"),
            Privilege::Run0 => Some("run0"),
            Privilege::Pkexec => Some("pkexec"),
            Privilege::Root | Privilege::Unavailable => None,
        }
    }

    /// `argv` run as root, or `None` when no helper is available.
    pub fn wrap(self, argv: Vec<String>) -> Option<Vec<String>> {
        match self {
            Privilege::Root => Some(argv),
            Privilege::Unavailable => None,
            helper => Some(helper.program().into_iter().map(str::to_string).chain(argv).collect()),
        }
    }

    /// How a root command is shown to the user (`sudo …`, or bare as root).
    pub fn display(self, argv: &[String]) -> String {
        match self.program() {
            Some(p) => format!("{p} {}", argv.join(" ")),
            None => argv.join(" "),
        }
    }
}

/// Whether this process runs as root.
pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_argv_per_manager() {
        let pkgs = ["greetd", "cage"];
        assert_eq!(PackageManager::Pacman.install_argv(&pkgs), ["pacman", "-S", "--needed", "--noconfirm", "greetd", "cage"]);
        assert_eq!(PackageManager::AptGet.install_argv(&pkgs), ["env", "DEBIAN_FRONTEND=noninteractive", "apt-get", "install", "-y", "--no-install-recommends", "greetd", "cage"]);
        assert_eq!(PackageManager::Dnf.install_argv(&pkgs), ["dnf", "install", "-y", "greetd", "cage"]);
        assert_eq!(PackageManager::Zypper.install_argv(&pkgs), ["zypper", "--non-interactive", "install", "--no-recommends", "greetd", "cage"]);
        assert_eq!(PackageManager::Xbps.install_argv(&pkgs), ["xbps-install", "-Sy", "greetd", "cage"]);
        assert_eq!(PackageManager::Apk.install_argv(&pkgs), ["apk", "add", "greetd", "cage"]);
        assert_eq!(PackageManager::Emerge.install_argv(&["gui-libs/greetd"]), ["emerge", "--noreplace", "gui-libs/greetd"]);
        assert_eq!(PackageManager::Eopkg.install_argv(&pkgs), ["eopkg", "install", "-y", "greetd", "cage"]);
    }

    #[test]
    fn refresh_only_where_install_doesnt() {
        assert!(PackageManager::Pacman.refresh_argv().is_none(), "no partial upgrades on Arch");
        assert_eq!(PackageManager::AptGet.refresh_argv().unwrap(), ["apt-get", "update", "-qq"]);
        assert!(PackageManager::Dnf.refresh_argv().is_none());
    }

    #[test]
    fn every_family_with_packages_has_a_manager() {
        for f in Family::ALL {
            let expected = !matches!(f, Family::NixOS | Family::Unknown);
            assert_eq!(PackageManager::for_family(f).is_some(), expected, "{f}");
            if let Some(pm) = PackageManager::for_family(f) {
                assert_eq!(pm.family(), f);
            }
        }
    }

    #[test]
    fn detect_prefers_the_familys_manager() {
        let both = |b: &str| b == "pacman" || b == "dnf";
        assert_eq!(PackageManager::detect(Family::Arch, both), Some(PackageManager::Pacman));
        assert_eq!(PackageManager::detect(Family::Fedora, both), Some(PackageManager::Dnf));
        assert_eq!(PackageManager::detect(Family::Unknown, |b| b == "zypper"), Some(PackageManager::Zypper));
        // The family's manager missing: whichever is installed.
        assert_eq!(PackageManager::detect(Family::Debian, |b| b == "apk"), Some(PackageManager::Apk));
        assert_eq!(PackageManager::detect(Family::Arch, |_| false), None);
    }

    #[test]
    fn privilege_helpers() {
        let argv = vec!["pacman".to_string(), "-S".to_string()];
        assert_eq!(Privilege::detect(true, |_| true), Privilege::Root);
        assert_eq!(Privilege::detect(false, |b| b == "doas" || b == "pkexec"), Privilege::Doas);
        assert_eq!(Privilege::detect(false, |b| b == "pkexec"), Privilege::Pkexec);
        assert_eq!(Privilege::detect(false, |b| b == "run0"), Privilege::Run0);
        assert_eq!(Privilege::detect(false, |_| false), Privilege::Unavailable);
        assert_eq!(Privilege::Sudo.wrap(argv.clone()).unwrap(), ["sudo", "pacman", "-S"]);
        assert_eq!(Privilege::Root.wrap(argv.clone()).unwrap(), ["pacman", "-S"]);
        assert_eq!(Privilege::Unavailable.wrap(argv.clone()), None);
        assert_eq!(Privilege::Run0.display(&argv), "run0 pacman -S");
        assert_eq!(Privilege::Root.display(&argv), "pacman -S");
    }
}
