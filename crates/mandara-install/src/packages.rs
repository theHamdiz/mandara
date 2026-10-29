//! What `mandara install` installs on each distribution family.
//!
//! * **run** — runtime libraries (always; a failure stops the install);
//! * **build** — the toolchain for source builds (`--from-source` only);
//! * **opt** — fonts, desktop portals, extra Vulkan drivers, colour emoji,
//!   icons and `xdg-utils`;
//! * **svc** — the system services Studio and the plugins talk to, the same
//!   set on every family: AccountsService (Users), GeoClue (location), the
//!   freedesktop sound theme (event sounds), UDisks2 (drives), UPower +
//!   power-profiles-daemon (battery, profiles), i2c-tools (DDC/CI monitor
//!   brightness), NetworkManager + its OpenVPN plugin (Network panel,
//!   `.ovpn` import), PipeWire + WirePlumber (sound), BlueZ (Bluetooth),
//!   polkit (admin prompts) and GNOME Keyring with its PAM module (secrets
//!   unlocked at login);
//! * **greeter** — Mandara Login's runtime: greetd and the cage kiosk.
//!
//! opt and svc are tried one package at a time, so a name a release doesn't
//! carry never blocks the rest. Gentoo names are full atoms
//! (`category/name`).

use crate::family::Family;

/// One family's package groups.
#[derive(Debug)]
pub struct PackageSet {
    pub run: &'static [&'static str],
    pub build: &'static [&'static str],
    pub opt: &'static [&'static str],
    pub svc: &'static [&'static str],
    pub greeter: &'static [&'static str],
}

impl PackageSet {
    /// Every group, labelled (tests and `--dry-run` listings).
    pub fn groups(&self) -> [(&'static str, &'static [&'static str]); 5] {
        [("run", self.run), ("build", self.build), ("opt", self.opt), ("svc", self.svc), ("greeter", self.greeter)]
    }
}

const GREETD_CAGE: &[&str] = &["greetd", "cage"];

const ARCH: PackageSet = PackageSet {
    run: &["libxkbcommon", "wayland", "vulkan-icd-loader", "mesa", "fontconfig", "freetype2"],
    build: &["base-devel", "git", "curl", "pkgconf", "cmake"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "noto-fonts", "noto-fonts-cjk", "inter-font", "ttf-jetbrains-mono", "pipewire-pulse", "pacman-contrib", "noto-fonts-emoji", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "networkmanager-openvpn", "pipewire", "wireplumber", "networkmanager", "bluez", "bluez-utils", "polkit", "gnome-keyring"],
    greeter: GREETD_CAGE,
};

const DEBIAN: PackageSet = PackageSet {
    run: &["libxkbcommon0", "libwayland-client0", "libvulkan1", "mesa-vulkan-drivers", "libfontconfig1", "libfreetype6", "ca-certificates"],
    build: &["build-essential", "git", "curl", "pkg-config", "cmake", "libxkbcommon-dev", "libwayland-dev", "libfontconfig-dev", "libfreetype-dev"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "fonts-noto-core", "fonts-noto-cjk", "fonts-inter", "fonts-jetbrains-mono", "pipewire-pulse", "fonts-noto-color-emoji", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue-2.0", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "network-manager-openvpn", "pipewire", "wireplumber", "network-manager", "bluez", "polkitd", "pkexec", "gnome-keyring", "libpam-gnome-keyring"],
    greeter: GREETD_CAGE,
};

const FEDORA: PackageSet = PackageSet {
    run: &["libxkbcommon", "libwayland-client", "vulkan-loader", "mesa-vulkan-drivers", "fontconfig", "freetype"],
    build: &["gcc", "gcc-c++", "make", "git", "curl", "pkgconf-pkg-config", "cmake", "libxkbcommon-devel", "wayland-devel", "fontconfig-devel", "freetype-devel"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "google-noto-sans-fonts", "google-noto-sans-arabic-fonts", "google-noto-sans-cjk-fonts", "rsms-inter-fonts", "jetbrains-mono-fonts", "pipewire-pulseaudio", "google-noto-color-emoji-fonts", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue2", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "NetworkManager-openvpn", "pipewire", "wireplumber", "NetworkManager", "bluez", "polkit", "gnome-keyring", "gnome-keyring-pam"],
    greeter: GREETD_CAGE,
};

const SUSE: PackageSet = PackageSet {
    run: &["libxkbcommon0", "libwayland-client0", "libvulkan1", "Mesa-vulkan-device-select", "fontconfig", "libfreetype6"],
    build: &["gcc", "gcc-c++", "make", "git", "curl", "pkgconf", "cmake", "libxkbcommon-devel", "wayland-devel", "fontconfig-devel", "freetype2-devel"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "google-noto-sans-fonts", "noto-sans-arabic-fonts", "noto-sans-cjk-fonts", "inter-fonts", "jetbrains-mono-fonts", "pipewire-pulseaudio", "libvulkan_radeon", "libvulkan_intel", "noto-coloremoji-fonts", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue2", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "NetworkManager-openvpn", "pipewire", "wireplumber", "NetworkManager", "bluez", "polkit", "gnome-keyring", "gnome-keyring-pam"],
    greeter: GREETD_CAGE,
};

// PipeWire itself is in svc, not repeated in opt.
const VOID: PackageSet = PackageSet {
    run: &["libxkbcommon", "wayland", "vulkan-loader", "mesa-dri", "fontconfig", "freetype"],
    build: &["base-devel", "git", "curl", "pkg-config", "cmake", "libxkbcommon-devel", "wayland-devel", "fontconfig-devel", "freetype-devel"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "noto-fonts-ttf", "noto-fonts-cjk", "font-inter", "font-jetbrains-mono", "mesa-vulkan-radeon", "mesa-vulkan-intel", "noto-fonts-emoji", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue2", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "NetworkManager-openvpn", "pipewire", "wireplumber", "NetworkManager", "bluez", "polkit", "gnome-keyring"],
    greeter: GREETD_CAGE,
};

const ALPINE: PackageSet = PackageSet {
    run: &["libxkbcommon", "wayland-libs-client", "vulkan-loader", "mesa-dri-gallium", "fontconfig", "freetype"],
    build: &["build-base", "git", "curl", "pkgconf", "cmake", "libxkbcommon-dev", "wayland-dev", "fontconfig-dev", "freetype-dev"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "font-noto", "font-noto-arabic", "font-noto-cjk", "font-inter", "font-jetbrains-mono", "mesa-vulkan-ati", "mesa-vulkan-intel", "pipewire-pulse", "font-noto-emoji", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "networkmanager-openvpn", "pipewire", "wireplumber", "networkmanager", "bluez", "polkit", "gnome-keyring"],
    greeter: GREETD_CAGE,
};

const GENTOO: PackageSet = PackageSet {
    run: &["x11-libs/libxkbcommon", "dev-libs/wayland", "media-libs/vulkan-loader", "media-libs/mesa", "media-libs/fontconfig", "media-libs/freetype"],
    build: &["dev-vcs/git", "net-misc/curl", "virtual/pkgconfig", "dev-build/cmake"],
    opt: &["sys-apps/xdg-desktop-portal", "sys-apps/xdg-desktop-portal-gtk", "media-fonts/noto", "media-fonts/noto-cjk", "media-fonts/inter", "media-fonts/jetbrains-mono", "media-fonts/noto-emoji", "x11-themes/papirus-icon-theme", "x11-misc/xdg-utils"],
    svc: &[
        "sys-apps/accountsservice",
        "app-misc/geoclue",
        "x11-themes/sound-theme-freedesktop",
        "sys-fs/udisks",
        "sys-power/upower",
        "sys-power/power-profiles-daemon",
        "sys-apps/i2c-tools",
        "net-vpn/networkmanager-openvpn",
        "media-video/pipewire",
        "media-video/wireplumber",
        "net-misc/networkmanager",
        "net-wireless/bluez",
        "sys-auth/polkit",
        "gnome-base/gnome-keyring",
    ],
    greeter: &["gui-libs/greetd", "gui-wm/cage"],
};

// Solus has no greetd package: Mandara Login is set up by hand there.
const SOLUS: PackageSet = PackageSet {
    run: &["libxkbcommon", "wayland", "vulkan", "mesalib", "fontconfig", "freetype2"],
    build: &["git", "curl", "pkgconf", "cmake", "libxkbcommon-devel", "wayland-devel", "fontconfig-devel", "freetype2-devel"],
    opt: &["xdg-desktop-portal", "xdg-desktop-portal-gtk", "noto-sans-ttf", "inter", "jetbrains-mono", "noto-emoji", "papirus-icon-theme", "xdg-utils"],
    svc: &["accountsservice", "geoclue", "sound-theme-freedesktop", "udisks2", "upower", "power-profiles-daemon", "i2c-tools", "networkmanager-openvpn", "pipewire", "wireplumber", "networkmanager", "bluez", "polkit", "gnome-keyring"],
    greeter: &[],
};

/// The package groups of `family` (`None`: NixOS and unknown systems get
/// hints instead).
pub fn for_family(family: Family) -> Option<&'static PackageSet> {
    match family {
        Family::Arch => Some(&ARCH),
        Family::Debian => Some(&DEBIAN),
        Family::Fedora => Some(&FEDORA),
        Family::Suse => Some(&SUSE),
        Family::Void => Some(&VOID),
        Family::Alpine => Some(&ALPINE),
        Family::Gentoo => Some(&GENTOO),
        Family::Solus => Some(&SOLUS),
        Family::NixOS | Family::Unknown => None,
    }
}

/// A compositor's screen-sharing / remote-desktop portal: (compositor
/// program, package, Gentoo atom). MangoWM uses wlr's, Niri GNOME's.
const PORTALS: &[(&str, &str, &str)] = &[
    ("mango", "xdg-desktop-portal-wlr", "gui-libs/xdg-desktop-portal-wlr"),
    ("Hyprland", "xdg-desktop-portal-hyprland", "gui-libs/xdg-desktop-portal-hyprland"),
    ("niri", "xdg-desktop-portal-gnome", "sys-apps/xdg-desktop-portal-gnome"),
];

/// The portal packages of every compositor installed (`has_program`).
pub fn portal_packages(family: Family, has_program: impl Fn(&str) -> bool) -> Vec<&'static str> {
    PORTALS.iter().filter(|(prog, _, _)| has_program(prog)).map(|(_, plain, atom)| if family == Family::Gentoo { *atom } else { *plain }).collect()
}

/// The package providing a login-screen program (`greetd`, `cage`, a kiosk
/// compositor) on `family`: Gentoo needs the full atom.
pub fn login_package(family: Family, program: &str) -> String {
    const GENTOO_ATOMS: &[(&str, &str)] = &[("greetd", "gui-libs/greetd"), ("cage", "gui-wm/cage"), ("niri", "gui-wm/niri"), ("hyprland", "gui-wm/hyprland")];
    let name = program.to_ascii_lowercase();
    if family == Family::Gentoo
        && let Some((_, atom)) = GENTOO_ATOMS.iter().find(|(p, _)| *p == name)
    {
        return atom.to_string();
    }
    name
}

/// NixOS: what to add to `configuration.nix` (then rerun with `--no-deps`).
pub const NIXOS_HINT: &[&str] = &[
    "environment.systemPackages = with pkgs; [ libxkbcommon wayland vulkan-loader fontconfig freetype ];",
    "fonts.packages = with pkgs; [ noto-fonts noto-fonts-cjk-sans inter jetbrains-mono ];",
    "xdg.portal.enable = true;",
    "services.accounts-daemon.enable = true; services.geoclue2.enable = true; services.udisks2.enable = true;",
    "services.upower.enable = true; services.power-profiles-daemon.enable = true; hardware.i2c.enable = true;",
    "networking.networkmanager.plugins = with pkgs; [ networkmanager-openvpn ];",
    "environment.systemPackages = with pkgs; [ sound-theme-freedesktop papirus-icon-theme xdg-utils ];",
    "fonts.packages = with pkgs; [ noto-fonts-color-emoji ];",
    "networking.networkmanager.enable = true; hardware.bluetooth.enable = true; security.polkit.enable = true;",
    "services.pipewire = { enable = true; pulse.enable = true; wireplumber.enable = true; };",
    "services.gnome.gnome-keyring.enable = true;",
];

/// Unknown systems: what to install by hand (then rerun with `--no-deps`).
pub const UNKNOWN_HINT: &[&str] = &[
    "Unknown distribution: install libxkbcommon, wayland, a Vulkan loader + Mesa, fontconfig and",
    "FreeType (and for source builds a C toolchain, git, pkg-config, cmake), then rerun with --no-deps.",
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn sets() -> impl Iterator<Item = (Family, &'static PackageSet)> {
        Family::ALL.into_iter().filter_map(|f| for_family(f).map(|s| (f, s)))
    }

    #[test]
    fn every_family_has_run_and_svc() {
        let with_tables: Vec<Family> = sets().map(|(f, _)| f).collect();
        assert_eq!(with_tables, [Family::Arch, Family::Debian, Family::Fedora, Family::Suse, Family::Void, Family::Alpine, Family::Gentoo, Family::Solus]);
        for (f, s) in sets() {
            assert!(!s.run.is_empty(), "{f}: no runtime packages");
            assert!(!s.svc.is_empty(), "{f}: no service packages");
            assert!(!s.build.is_empty(), "{f}: no build packages");
            assert!(!s.opt.is_empty(), "{f}: no optional packages");
        }
    }

    #[test]
    fn no_duplicates_within_a_family() {
        for (f, s) in sets() {
            let mut seen = HashSet::new();
            for (group, pkgs) in s.groups() {
                for p in pkgs {
                    assert!(seen.insert(*p), "{f}: {p} is listed twice (again in {group})");
                    assert!(!p.is_empty() && !p.contains(char::is_whitespace), "{f}: bad package name {p:?}");
                }
            }
        }
    }

    #[test]
    fn gentoo_atoms_have_a_category() {
        let s = for_family(Family::Gentoo).unwrap();
        for (group, pkgs) in s.groups() {
            for p in pkgs {
                let (cat, name) = p.split_once('/').unwrap_or_else(|| panic!("gentoo {group}: {p} has no category"));
                assert!(!cat.is_empty() && !name.is_empty() && !name.contains('/'), "gentoo {group}: {p}");
            }
        }
        for (_, _, atom) in PORTALS {
            assert!(atom.contains('/'), "{atom}");
        }
        assert_eq!(login_package(Family::Gentoo, "cage"), "gui-wm/cage");
        assert_eq!(login_package(Family::Gentoo, "Hyprland"), "gui-wm/hyprland");
        assert_eq!(login_package(Family::Arch, "Hyprland"), "hyprland");
    }

    #[test]
    fn every_family_covers_the_shared_services() {
        // Roles every svc list must cover (a substring of the package name).
        const ROLES: &[&str] = &["accountsservice", "geoclue", "sound-theme", "udisks", "upower", "power-profiles", "i2c-tools", "openvpn", "pipewire", "wireplumber", "bluez", "polkit", "gnome-keyring"];
        for (f, s) in sets() {
            for role in ROLES {
                assert!(s.svc.iter().any(|p| p.to_ascii_lowercase().contains(role)), "{f}: svc lacks {role}");
            }
            assert!(s.svc.iter().any(|p| p.to_ascii_lowercase().replace('-', "").contains("networkmanager")), "{f}: svc lacks NetworkManager");
            for want in ["xdg-desktop-portal", "papirus-icon-theme", "xdg-utils"] {
                assert!(s.opt.iter().any(|p| p.ends_with(want)), "{f}: opt lacks {want}");
            }
            assert!(s.opt.iter().any(|p| p.contains("emoji")), "{f}: opt lacks a colour emoji font");
        }
    }

    #[test]
    fn portals_follow_installed_compositors() {
        assert!(portal_packages(Family::Arch, |_| false).is_empty());
        assert_eq!(portal_packages(Family::Arch, |p| p == "mango" || p == "niri"), ["xdg-desktop-portal-wlr", "xdg-desktop-portal-gnome"]);
        assert_eq!(portal_packages(Family::Gentoo, |p| p == "Hyprland"), ["gui-libs/xdg-desktop-portal-hyprland"]);
    }
}
