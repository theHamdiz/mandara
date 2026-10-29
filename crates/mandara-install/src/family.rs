//! Distribution families: which package names and which package manager a
//! system uses, from `/etc/os-release` (`ID`, then each `ID_LIKE`).

use std::fmt;

use crate::pkg::PackageManager;

/// A distribution family. Derivatives map to their parent (`cachyos` →
/// Arch, `pop` → Debian, `rocky` → Fedora…).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Arch,
    Debian,
    Fedora,
    Suse,
    Void,
    Alpine,
    Gentoo,
    Solus,
    /// Declarative: packages go into the system configuration (hints only).
    NixOS,
    Unknown,
}

/// `os-release` ids of each family. An entry ending in `*` matches any id
/// with that prefix (`opensuse-tumbleweed`).
const IDS: &[(Family, &[&str])] = &[
    (Family::Arch, &["arch", "archarm", "endeavouros", "manjaro", "cachyos", "garuda", "artix"]),
    (Family::Debian, &["debian", "ubuntu", "linuxmint", "pop", "elementary", "zorin", "kali", "raspbian", "neon"]),
    (Family::Fedora, &["fedora", "rhel", "centos", "rocky", "almalinux", "nobara", "ultramarine"]),
    (Family::Suse, &["opensuse*", "suse", "sles"]),
    (Family::Void, &["void"]),
    (Family::Alpine, &["alpine", "postmarketos"]),
    (Family::Gentoo, &["gentoo", "funtoo"]),
    (Family::NixOS, &["nixos"]),
    (Family::Solus, &["solus"]),
];

/// The fields of `os-release` the installer uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub id_like: Vec<String>,
    pub pretty_name: String,
}

impl OsRelease {
    /// Parses `os-release` text (`KEY=value`, values optionally quoted).
    pub fn parse(text: &str) -> OsRelease {
        let mut out = OsRelease::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            match key.trim() {
                "ID" => out.id = value.to_ascii_lowercase(),
                "ID_LIKE" => out.id_like = value.split_whitespace().map(str::to_ascii_lowercase).collect(),
                "PRETTY_NAME" => out.pretty_name = value.to_string(),
                _ => {}
            }
        }
        out
    }

    /// `PRETTY_NAME`, else `ID`, else `unknown`.
    pub fn display_name(&self) -> &str {
        if !self.pretty_name.is_empty() {
            &self.pretty_name
        } else if !self.id.is_empty() {
            &self.id
        } else {
            "unknown"
        }
    }
}

impl Family {
    /// Every family with package tables, then the hint-only ones.
    pub const ALL: [Family; 10] = [Family::Arch, Family::Debian, Family::Fedora, Family::Suse, Family::Void, Family::Alpine, Family::Gentoo, Family::Solus, Family::NixOS, Family::Unknown];

    /// The family of one `os-release` id.
    pub fn from_id(id: &str) -> Option<Family> {
        IDS.iter().find(|(_, ids)| ids.iter().any(|pattern| matches_id(pattern, id))).map(|(f, _)| *f)
    }

    /// From `os-release` (`ID` first, then each `ID_LIKE` in order); when
    /// none is known, from the package manager that is installed.
    pub fn detect(os: &OsRelease, has_program: impl Fn(&str) -> bool) -> Family {
        std::iter::once(os.id.as_str())
            .chain(os.id_like.iter().map(String::as_str))
            .find_map(Family::from_id)
            .or_else(|| PackageManager::ALL.iter().find(|pm| has_program(pm.binary())).map(|pm| pm.family()))
            .unwrap_or(Family::Unknown)
    }

    /// Lower-case name (`arch`, `debian`, …), as the installer prints it.
    pub fn name(self) -> &'static str {
        match self {
            Family::Arch => "arch",
            Family::Debian => "debian",
            Family::Fedora => "fedora",
            Family::Suse => "suse",
            Family::Void => "void",
            Family::Alpine => "alpine",
            Family::Gentoo => "gentoo",
            Family::Solus => "solus",
            Family::NixOS => "nixos",
            Family::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

fn matches_id(pattern: &str, id: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => id.starts_with(prefix),
        None => pattern == id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(text: &str) -> OsRelease {
        OsRelease::parse(text)
    }

    #[test]
    fn parses_os_release() {
        let o = os("NAME=\"CachyOS Linux\"\nPRETTY_NAME=\"CachyOS\"\nID=cachyos\nID_LIKE=\"arch\"\n# comment\n");
        assert_eq!(o.id, "cachyos");
        assert_eq!(o.id_like, ["arch"]);
        assert_eq!(o.display_name(), "CachyOS");
        assert_eq!(OsRelease::default().display_name(), "unknown");
    }

    #[test]
    fn derivatives_map_to_their_family() {
        let none = |_: &str| false;
        assert_eq!(Family::detect(&os("ID=cachyos\nID_LIKE=arch"), none), Family::Arch);
        assert_eq!(Family::detect(&os("ID=pop\nID_LIKE=\"ubuntu debian\""), none), Family::Debian);
        assert_eq!(Family::detect(&os("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\""), none), Family::Suse);
        assert_eq!(Family::detect(&os("ID=rocky\nID_LIKE=\"rhel centos fedora\""), none), Family::Fedora);
        assert_eq!(Family::detect(&os("ID=postmarketos\nID_LIKE=alpine"), none), Family::Alpine);
        assert_eq!(Family::detect(&os("ID=nixos"), none), Family::NixOS);
        assert_eq!(Family::detect(&os("ID=solus"), none), Family::Solus);
        assert_eq!(Family::detect(&os("ID=funtoo"), none), Family::Gentoo);
        // An unknown id falls through to ID_LIKE.
        assert_eq!(Family::detect(&os("ID=mything\nID_LIKE=void"), none), Family::Void);
    }

    #[test]
    fn unknown_ids_fall_back_to_the_package_manager() {
        assert_eq!(Family::detect(&os("ID=mystery"), |b| b == "xbps-install"), Family::Void);
        assert_eq!(Family::detect(&os(""), |b| b == "emerge"), Family::Gentoo);
        assert_eq!(Family::detect(&os(""), |_| false), Family::Unknown);
    }
}
