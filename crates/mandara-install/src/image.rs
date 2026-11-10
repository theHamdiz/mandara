//! Image-based (immutable) systems: how to tell one, and how packages get
//! onto it.
//!
//! These systems boot a read-only image of `/usr` that their own tool
//! replaces as a whole, so a package manager can't install into the running
//! system. Mandara itself goes into your home folder (`~/.local`) as
//! everywhere; system packages it is missing are added to the next image
//! with the system's own tool and arrive with the next boot.
//!
//! | System | Detected by | Packages added with | Root-owned files that survive updates |
//! |---|---|---|---|
//! | Fedora Atomic (Silverblue, Kinoite, Sway Atomic…), Bazzite, Bluefin, Aurora | `/run/ostree-booted` | `rpm-ostree install --idempotent` (layering) | `/etc`, `/var`, `/usr/local` (a link to `/var/usrlocal`) |
//! | openSUSE Aeon, Kalpa, MicroOS, SL Micro | `os-release` (`microos`, `aeon`, `kalpa`, `sl-micro`, `sle-micro` in `ID`/`ID_LIKE`) | `transactional-update pkg install` (a new snapshot) | `/etc`, `/var`, `/usr/local` (its own subvolume) |
//! | Vanilla OS 2 | the `abroot` program, or `NAME="Vanilla OS"` | `abroot pkg add`, then `abroot pkg apply` (the other root) | `/etc`, `/var` |
//! | SteamOS 3 | `ID=steamos` | nothing: the image is read-only, and packages put into it are removed by the next update | `/etc` (an overlay), `/var`, `/home` |
//!
//! NixOS is declarative rather than image-based and has its own path (the
//! flake and module), so it isn't one of these.
//!
//! The Updates panel uses the same detection (`mandara_services::sysupdate`)
//! and leaves these systems' updates to their tool.

use crate::family::OsRelease;

/// Present on every booted ostree system (Fedora Atomic and its
/// derivatives, bootc images).
pub const OSTREE_BOOTED: &str = "/run/ostree-booted";
/// `os-release` ids (substrings of `ID` or an `ID_LIKE` entry) of systems
/// that update with transactional-update.
const TRANSACTIONAL_IDS: [&str; 5] = ["microos", "aeon", "kalpa", "sl-micro", "sle-micro"];
/// SteamOS's `os-release` id.
const STEAMOS_ID: &str = "steamos";
/// Vanilla OS's `os-release` name (its `ID` is its base's).
const VANILLA_NAME: &str = "Vanilla OS";
/// `os-release` `VARIANT_ID`s of Fedora Atomic images, which mark one even
/// where `/run/ostree-booted` can't exist (a container of the image).
const ATOMIC_VARIANTS: &[&str] = &["silverblue", "kinoite", "sway-atomic", "budgie-atomic", "cosmic-atomic", "lxqt-atomic", "xfce-atomic", "sericea", "onyx", "bazzite", "bazzite-deck", "bazzite-gnome", "bluefin", "bluefin-dx", "aurora", "aurora-dx"];

/// An image-based system.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageSystem {
    /// rpm-ostree: Fedora Atomic and the images built on it.
    RpmOstree,
    /// transactional-update: openSUSE Aeon, Kalpa, MicroOS, SL Micro.
    Transactional,
    /// ABRoot: Vanilla OS 2.
    Abroot,
    /// SteamOS 3: read-only, takes no packages.
    SteamOs,
}

/// What [`ImageSystem::detect`] decides from (fixtures in tests).
pub struct Probe<'a> {
    pub os: &'a OsRelease,
    /// [`OSTREE_BOOTED`] exists.
    pub ostree_booted: bool,
    pub has_program: &'a dyn Fn(&str) -> bool,
}

impl ImageSystem {
    /// The image-based system this is, if it is one.
    pub fn detect(p: &Probe<'_>) -> Option<ImageSystem> {
        let ids = || std::iter::once(p.os.id.as_str()).chain(p.os.id_like.iter().map(String::as_str));
        if p.ostree_booted || (ATOMIC_VARIANTS.contains(&p.os.variant_id.as_str()) && (p.has_program)(ImageSystem::RpmOstree.program())) {
            return Some(ImageSystem::RpmOstree);
        }
        if ids().any(|id| TRANSACTIONAL_IDS.iter().any(|t| id.contains(t))) {
            return Some(ImageSystem::Transactional);
        }
        if p.os.id == STEAMOS_ID {
            return Some(ImageSystem::SteamOs);
        }
        if p.os.name.starts_with(VANILLA_NAME) || (p.has_program)(ImageSystem::Abroot.program()) {
            return Some(ImageSystem::Abroot);
        }
        None
    }

    /// The tool that changes the image (`rpm-ostree`…; `steamos-update`
    /// on SteamOS, which only updates it).
    pub fn program(self) -> &'static str {
        match self {
            ImageSystem::RpmOstree => "rpm-ostree",
            ImageSystem::Transactional => "transactional-update",
            ImageSystem::Abroot => "abroot",
            ImageSystem::SteamOs => "steamos-update",
        }
    }

    /// How the system updates itself (the Updates panel's hint).
    pub fn update_command(self) -> &'static str {
        match self {
            ImageSystem::RpmOstree => "rpm-ostree upgrade",
            ImageSystem::Transactional => "transactional-update dup",
            ImageSystem::Abroot => "abroot upgrade",
            ImageSystem::SteamOs => "steamos-update",
        }
    }

    /// Whether packages can be added to the image (and survive updates).
    pub fn takes_packages(self) -> bool {
        self != ImageSystem::SteamOs
    }

    /// Whether root-owned files in `/usr/local` survive updates: the admin
    /// helper and Mandara Login's copy live there. SteamOS's `/usr` (with
    /// `/usr/local`) is replaced by every update; ABRoot's `/usr/local`
    /// belongs to the root it is in, so the other root lacks it.
    pub fn keeps_usr_local(self) -> bool {
        matches!(self, ImageSystem::RpmOstree | ImageSystem::Transactional)
    }

    /// argv adding `packages` to the next image (run as root). Packages
    /// already requested are no error (`--idempotent`). ABRoot only
    /// records them: [`Self::finish_argv`] builds the image.
    pub fn add_argv<S: AsRef<str>>(self, packages: &[S]) -> Option<Vec<String>> {
        let prefix: &[&str] = match self {
            ImageSystem::RpmOstree => &["rpm-ostree", "install", "--idempotent"],
            ImageSystem::Transactional => &["transactional-update", "--non-interactive", "pkg", "install", "--no-recommends"],
            ImageSystem::Abroot => &["abroot", "pkg", "add"],
            ImageSystem::SteamOs => return None,
        };
        Some(prefix.iter().map(|a| a.to_string()).chain(packages.iter().map(|p| p.as_ref().to_string())).collect())
    }

    /// argv that builds the next image from what [`Self::add_argv`]
    /// recorded, when the tool needs one (ABRoot's `pkg apply`).
    pub fn finish_argv(self) -> Option<Vec<String>> {
        (self == ImageSystem::Abroot).then(|| ["abroot", "pkg", "apply"].iter().map(|a| a.to_string()).collect())
    }

    /// argv asking which of `packages` the running image has, read by
    /// [`missing_packages`] (run with `LC_ALL=C`, as the user).
    pub fn query_argv<S: AsRef<str>>(self, packages: &[S]) -> Vec<String> {
        let prefix: &[&str] = match self {
            ImageSystem::RpmOstree | ImageSystem::Transactional => &["rpm", "-q", "--qf", "%{NAME}\\n"],
            ImageSystem::Abroot => &["dpkg-query", "-W", "-f", "${Package} ${db:Status-Abbrev}\\n"],
            ImageSystem::SteamOs => &["pacman", "-Q"],
        };
        prefix.iter().map(|a| a.to_string()).chain(packages.iter().map(|p| p.as_ref().to_string())).collect()
    }
}

/// Which of `wanted` [`ImageSystem::query_argv`]'s output doesn't list as
/// installed. `rpm -q` prints installed names (and "package X is not
/// installed" for the others), `dpkg-query` `name ii`, `pacman -Q`
/// `name version`.
pub fn missing_packages<'a>(system: ImageSystem, stdout: &str, wanted: &[&'a str]) -> Vec<&'a str> {
    let installed: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            match system {
                ImageSystem::RpmOstree | ImageSystem::Transactional => (name != "package").then_some(name),
                ImageSystem::Abroot => words.next().is_some_and(|status| status.starts_with("ii")).then_some(name),
                ImageSystem::SteamOs => Some(name),
            }
        })
        .collect();
    wanted.iter().copied().filter(|p| !installed.contains(p)).collect()
}

/// What `rpm-ostree status --json` says about the next boot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OstreeStatus {
    /// The deployment the next boot starts isn't the running one (a staged
    /// or newer deployment waits for a reboot).
    pub reboot_pending: bool,
    /// The packages the next boot's deployment layers.
    pub requested: Vec<String>,
}

impl OstreeStatus {
    /// From `rpm-ostree status --json`: its first deployment is the one
    /// the next boot starts.
    pub fn parse(json: &str) -> Option<OstreeStatus> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let next = value.get("deployments")?.as_array()?.first()?;
        let booted = next.get("booted").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let requested = next.get("requested-packages").and_then(serde_json::Value::as_array).map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
        Some(OstreeStatus { reboot_pending: !booted, requested })
    }

    /// argv of the query (as the user; read-only).
    pub fn argv() -> Vec<String> {
        ["rpm-ostree", "status", "--json"].iter().map(|a| a.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SILVERBLUE: &str = include_str!("../fixtures/image/silverblue.os-release");
    const BAZZITE: &str = include_str!("../fixtures/image/bazzite.os-release");
    const BLUEFIN: &str = include_str!("../fixtures/image/bluefin.os-release");
    const AEON: &str = include_str!("../fixtures/image/aeon.os-release");
    const KALPA: &str = include_str!("../fixtures/image/kalpa.os-release");
    const MICROOS: &str = include_str!("../fixtures/image/microos.os-release");
    const VANILLA: &str = include_str!("../fixtures/image/vanillaos.os-release");
    const STEAMOS: &str = include_str!("../fixtures/image/steamos.os-release");
    const TUMBLEWEED: &str = "NAME=\"openSUSE Tumbleweed\"\nID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";
    const FEDORA: &str = "NAME=\"Fedora Linux\"\nID=fedora\nVARIANT_ID=workstation\n";
    const STATUS_PENDING: &str = include_str!("../fixtures/image/rpm-ostree-status-pending.json");
    const STATUS_BOOTED: &str = include_str!("../fixtures/image/rpm-ostree-status-booted.json");

    fn detect(os_release: &str, ostree_booted: bool, programs: &[&str]) -> Option<ImageSystem> {
        let os = OsRelease::parse(os_release);
        let has = |p: &str| programs.contains(&p);
        ImageSystem::detect(&Probe { os: &os, ostree_booted, has_program: &has })
    }

    #[test]
    fn fedora_atomic_by_the_ostree_flag_or_its_variant() {
        for os in [SILVERBLUE, BAZZITE, BLUEFIN] {
            assert_eq!(detect(os, true, &[]), Some(ImageSystem::RpmOstree));
            // A container of the image: no flag, but the variant and the tool.
            assert_eq!(detect(os, false, &["rpm-ostree"]), Some(ImageSystem::RpmOstree));
            assert_eq!(detect(os, false, &[]), None, "the variant alone, without rpm-ostree");
        }
        // Any booted ostree system, whatever its os-release says.
        assert_eq!(detect(FEDORA, true, &[]), Some(ImageSystem::RpmOstree));
        assert_eq!(detect(FEDORA, false, &["rpm-ostree"]), None, "Workstation with rpm-ostree installed is not image-based");
    }

    #[test]
    fn transactional_systems_by_their_ids() {
        for os in [AEON, KALPA, MICROOS] {
            assert_eq!(detect(os, false, &[]), Some(ImageSystem::Transactional), "{os}");
        }
        assert_eq!(detect("ID=\"sl-micro\"\nID_LIKE=\"suse\"", false, &[]), Some(ImageSystem::Transactional));
        assert_eq!(detect(TUMBLEWEED, false, &["transactional-update"]), None, "Tumbleweed can install the tool without being transactional");
    }

    #[test]
    fn vanilla_and_steamos() {
        assert_eq!(detect(VANILLA, false, &[]), Some(ImageSystem::Abroot), "by its NAME");
        assert_eq!(detect("ID=debian", false, &["abroot"]), Some(ImageSystem::Abroot), "by its tool");
        assert_eq!(detect(STEAMOS, false, &[]), Some(ImageSystem::SteamOs));
        assert!(!ImageSystem::SteamOs.takes_packages());
        assert_eq!(ImageSystem::SteamOs.add_argv(&["greetd"]), None);
        for plain in ["ID=arch", "ID=debian\nNAME=\"Debian GNU/Linux\"", FEDORA, TUMBLEWEED, "ID=nixos", ""] {
            assert_eq!(detect(plain, false, &["pacman", "apt-get", "dnf", "zypper"]), None, "{plain}");
        }
    }

    #[test]
    fn every_tool_has_its_command_plan() {
        let pkgs = ["greetd", "cage"];
        assert_eq!(ImageSystem::RpmOstree.add_argv(&pkgs).unwrap(), ["rpm-ostree", "install", "--idempotent", "greetd", "cage"]);
        assert_eq!(ImageSystem::Transactional.add_argv(&pkgs).unwrap(), ["transactional-update", "--non-interactive", "pkg", "install", "--no-recommends", "greetd", "cage"]);
        assert_eq!(ImageSystem::Abroot.add_argv(&pkgs).unwrap(), ["abroot", "pkg", "add", "greetd", "cage"]);
        assert_eq!(ImageSystem::Abroot.finish_argv().unwrap(), ["abroot", "pkg", "apply"]);
        for s in [ImageSystem::RpmOstree, ImageSystem::Transactional, ImageSystem::SteamOs] {
            assert_eq!(s.finish_argv(), None, "{s:?}");
        }
        assert!(ImageSystem::RpmOstree.keeps_usr_local() && ImageSystem::Transactional.keeps_usr_local());
        assert!(!ImageSystem::SteamOs.keeps_usr_local() && !ImageSystem::Abroot.keeps_usr_local());
        assert_eq!(ImageSystem::RpmOstree.update_command(), "rpm-ostree upgrade");
    }

    #[test]
    fn missing_packages_from_each_query() {
        let wanted = ["fontconfig", "greetd", "cage"];
        let rpm = "fontconfig\npackage greetd is not installed\npackage cage is not installed\n";
        assert_eq!(ImageSystem::RpmOstree.query_argv(&wanted), ["rpm", "-q", "--qf", "%{NAME}\\n", "fontconfig", "greetd", "cage"]);
        assert_eq!(missing_packages(ImageSystem::RpmOstree, rpm, &wanted), ["greetd", "cage"]);
        assert_eq!(missing_packages(ImageSystem::Transactional, "fontconfig\ngreetd\ncage\n", &wanted), Vec::<&str>::new());
        // dpkg-query lists removed-but-configured packages as `rc`.
        let dpkg = "fontconfig ii \ngreetd rc \n";
        assert_eq!(missing_packages(ImageSystem::Abroot, dpkg, &wanted), ["greetd", "cage"]);
        assert_eq!(missing_packages(ImageSystem::SteamOs, "fontconfig 2.15.0-1\n", &wanted), ["greetd", "cage"]);
    }

    #[test]
    fn rpm_ostree_status_tells_the_next_boot() {
        let pending = OstreeStatus::parse(STATUS_PENDING).unwrap();
        assert!(pending.reboot_pending);
        assert_eq!(pending.requested, ["greetd", "cage"]);
        let booted = OstreeStatus::parse(STATUS_BOOTED).unwrap();
        assert!(!booted.reboot_pending);
        assert_eq!(booted.requested, ["gnome-keyring-pam"]);
        assert_eq!(OstreeStatus::parse("error: not booted"), None);
        assert_eq!(OstreeStatus::parse("{\"deployments\": []}"), None);
    }
}
