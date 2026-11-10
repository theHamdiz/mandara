//! System updates that Mandara runs itself, with live progress, through
//! Mandara's own password prompt.
//!
//! The read-only check stays in [`crate::updates`]. This module applies
//! the updates:
//!
//! * [`plan`]: one [`plan::SystemBackend`] per distribution family, each
//!   a fixed list of argv steps (refresh, download, install) that run
//!   without questions, and the parser that turns their output into
//!   progress;
//! * [`helper`]: the root side. `mandara system-update apply` (and the
//!   `nixos …` actions) runs through pkexec with the root-owned admin
//!   helper, so the shell's polkit agent shows Mandara's password dialog.
//!   It accepts only a few fixed actions, detects the system itself and
//!   runs only the fixed per-family commands: no command string ever
//!   crosses the privilege boundary;
//! * [`protocol`]: the marker lines the helper writes between the
//!   package manager's own output;
//! * [`job`]: the user side. One owned worker thread starts the helper (and
//!   the rootless Flatpak step), reads its output incrementally into a
//!   bounded state, and keeps running while the panel is closed;
//! * [`nixos`]: NixOS generations, flake inputs, closure diffs and clean-up;
//! * [`aur`]: the AUR step on Arch, run as you by the installed AUR helper,
//!   with [`askpass`] answering its `sudo` through Mandara's password dialog.

pub mod askpass;
pub mod aur;
pub mod helper;
pub mod job;
pub mod nixos;
pub mod parse;
pub mod plan;
pub mod protocol;
#[cfg(any(test, all(feature = "fake-system-update", debug_assertions)))]
pub mod fake;

use std::path::{Path, PathBuf};

pub use mandara_install::Family;
pub use mandara_install::family::OsRelease;
use mandara_i18n::t;
use serde::{Deserialize, Serialize};

/// Folders the root side looks for programs in, in order. Only root-owned
/// files there are run as root; `$PATH` is never consulted.
#[cfg(not(target_os = "freebsd"))]
pub const SAFE_DIRS: &[&str] = &["/run/current-system/sw/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"];
/// FreeBSD: packages live in `/usr/local` (`/usr/local/sbin/pkg`; the
/// `/usr/sbin/pkg` in base only bootstraps it, so it comes second), the
/// base system's tools (`freebsd-update`) in `/usr/sbin`.
#[cfg(target_os = "freebsd")]
pub const SAFE_DIRS: &[&str] = &["/usr/local/sbin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"];
/// `PATH` given to every program the root side runs.
#[cfg(not(target_os = "freebsd"))]
pub const SAFE_PATH: &str = "/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin";
/// FreeBSD root's own default `PATH` (package scripts call tools from
/// `/usr/local/bin`).
#[cfg(target_os = "freebsd")]
pub const SAFE_PATH: &str = "/sbin:/bin:/usr/sbin:/usr/bin:/usr/local/sbin:/usr/local/bin";
/// Where `os-release` is read from (the first that exists).
const OS_RELEASE: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];
/// Present on rpm-ostree (Fedora Atomic) systems.
const OSTREE_BOOTED: &str = "/run/ostree-booted";
/// Ids of image-based openSUSE systems (transactional-update).
const TRANSACTIONAL_IDS: [&str; 4] = ["microos", "aeon", "kalpa", "sle-micro"];
/// openSUSE ids that update with `zypper dup` (rolling releases).
const SUSE_ROLLING_IDS: [&str; 2] = ["opensuse-tumbleweed", "opensuse-slowroll"];

/// How this system is updated.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum System {
    /// A package manager Mandara drives ([`plan::backend`]).
    Managed(plan::BackendId),
    /// NixOS: generations, rebuilt from the system's own configuration.
    NixOs,
    /// Image-based: updated by its own tool, never by Mandara.
    ImageBased(ImageTool),
    /// A family Mandara can't update (Gentoo, Solus, unknown).
    Unsupported,
}

/// The tool an image-based system updates with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageTool {
    RpmOstree,
    TransactionalUpdate,
}

impl ImageTool {
    pub fn command(self) -> &'static str {
        match self {
            ImageTool::RpmOstree => "rpm-ostree upgrade",
            ImageTool::TransactionalUpdate => "transactional-update dup",
        }
    }
}

/// The facts [`System::detect`] decides from (a fixture root in tests).
pub struct Probe<'a> {
    pub os: &'a OsRelease,
    pub has_program: &'a dyn Fn(&str) -> bool,
    pub ostree_booted: bool,
}

impl System {
    /// From `os-release`, the installed package manager and image markers.
    pub fn detect_with(p: &Probe<'_>) -> System {
        if p.ostree_booted {
            return System::ImageBased(ImageTool::RpmOstree);
        }
        let ids = || std::iter::once(p.os.id.as_str()).chain(p.os.id_like.iter().map(String::as_str));
        if ids().any(|id| TRANSACTIONAL_IDS.iter().any(|t| id.contains(t))) {
            return System::ImageBased(ImageTool::TransactionalUpdate);
        }
        let family = Family::detect(p.os, |b| (p.has_program)(b));
        let id = match family {
            Family::Arch => plan::BackendId::Pacman,
            Family::Debian => plan::BackendId::Apt,
            Family::Fedora => plan::BackendId::Dnf,
            Family::Suse if SUSE_ROLLING_IDS.contains(&p.os.id.as_str()) => plan::BackendId::ZypperDup,
            Family::Suse => plan::BackendId::ZypperUpdate,
            Family::Void => plan::BackendId::Xbps,
            Family::Alpine => plan::BackendId::Apk,
            Family::FreeBSD => plan::BackendId::Pkg,
            Family::NixOS => return System::NixOs,
            Family::Gentoo | Family::Solus | Family::Unknown => return System::Unsupported,
        };
        System::Managed(id)
    }

    /// This system (reads `os-release` and the safe program folders; on
    /// FreeBSD, whether pkg installed the base system).
    pub fn detect() -> System {
        let os = read_os_release();
        System::detect_with(&Probe { os: &os, has_program: &|p| find_safe(p).is_some(), ostree_booted: Path::new(OSTREE_BOOTED).exists() }).with_packaged_base(packaged_base)
    }

    /// FreeBSD whose base system came from packages (FreeBSD 15's default
    /// install) updates it with pkg, never freebsd-update. `packaged` is
    /// asked only on FreeBSD.
    pub fn with_packaged_base(self, packaged: impl FnOnce() -> bool) -> System {
        match self {
            System::Managed(plan::BackendId::Pkg) if packaged() => System::Managed(plan::BackendId::PkgBase),
            other => other,
        }
    }

    /// Shown in the panel ("Arch Linux", "NixOS"…): the distribution's own
    /// name from `os-release`.
    pub fn label(self) -> &'static str {
        match self {
            System::Managed(id) => id.manager_label(),
            System::NixOs => "NixOS",
            System::ImageBased(ImageTool::RpmOstree) => "rpm-ostree",
            System::ImageBased(ImageTool::TransactionalUpdate) => "transactional-update",
            System::Unsupported => "",
        }
    }

    /// Why Mandara won't update this system itself, and what to do.
    pub fn explanation(self) -> Option<String> {
        match self {
            System::Managed(_) | System::NixOs => None,
            System::ImageBased(tool) => Some(mandara_i18n::tf("This system is image-based: it updates with {command}, which Mandara leaves to it", &[("command", &tool.command())])),
            System::Unsupported => Some(t("Mandara can't update this distribution itself yet; use its own package manager").to_string()),
        }
    }
}

/// The file freebsd-update asks pkg about: pkg itself reads it for the
/// system's ABI, so it exists everywhere, and on a packaged base a
/// `FreeBSD-*` package owns it.
#[cfg(target_os = "freebsd")]
const BASE_PROBE_FILE: &str = "/usr/bin/uname";
/// The prefix of every package of a packaged base.
const BASE_PACKAGE_PREFIX: &str = "FreeBSD-";

/// Whether `pkg which -q` answered with a base-system package (exit 0
/// and its `FreeBSD-runtime-15.1`-style name): the same test
/// freebsd-update uses to refuse a packaged base.
pub fn is_base_package_owner(success: bool, stdout: &str) -> bool {
    success && stdout.trim().starts_with(BASE_PACKAGE_PREFIX)
}

/// Whether this system's base came from packages (asked once per process:
/// one `pkg which` run, on FreeBSD only).
#[cfg(target_os = "freebsd")]
pub fn packaged_base() -> bool {
    static PACKAGED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PACKAGED.get_or_init(|| {
        let Some(pkg) = find_safe("pkg") else { return false };
        std::process::Command::new(pkg)
            .args(["which", "-q", BASE_PROBE_FILE])
            .env_clear()
            .env("PATH", SAFE_PATH)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| is_base_package_owner(o.status.success(), &String::from_utf8_lossy(&o.stdout)))
    })
}

/// Only FreeBSD has a packaged base.
#[cfg(not(target_os = "freebsd"))]
pub fn packaged_base() -> bool {
    false
}

/// `os-release` of this system (empty when unreadable).
pub fn read_os_release() -> OsRelease {
    OS_RELEASE.iter().find_map(|p| std::fs::read_to_string(p).ok()).map(|text| OsRelease::parse(&text)).unwrap_or_default()
}

/// `name` in [`SAFE_DIRS`]: the first executable regular file.
pub fn find_safe(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    if name.contains('/') {
        return None;
    }
    SAFE_DIRS.iter().map(|d| Path::new(d).join(name)).find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// Whether the sandbox's recorded-output backend replaces the real one
/// (`MANDARA_FAKE_SYSTEM_UPDATE`; only in debug builds with the
/// `fake-system-update` feature, never in a release).
pub fn fake_active() -> bool {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    {
        fake::from_env().is_some()
    }
    #[cfg(not(all(feature = "fake-system-update", debug_assertions)))]
    {
        false
    }
}

#[cfg(all(feature = "fake-system-update", debug_assertions))]
fn fake_launcher() -> Option<&'static fake::FakeLauncher> {
    static FAKE: std::sync::OnceLock<Option<fake::FakeLauncher>> = std::sync::OnceLock::new();
    FAKE.get_or_init(fake::launcher).as_ref()
}

/// How this system updates (the sandbox fake's system when it is on).
pub fn current_system() -> System {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    if let Some(f) = fake_launcher() {
        return f.system;
    }
    System::detect()
}

/// The launcher jobs run with: pkexec and the admin helper (the recorded
/// fake in the sandbox).
pub fn launcher() -> std::sync::Arc<dyn job::Launcher> {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    if fake_launcher().is_some() {
        struct Shared;
        impl job::Launcher for Shared {
            fn prepare(&self) -> Result<PathBuf, Failure> {
                fake_launcher().map_or_else(|| Err(Failure { kind: FailureKind::Helper, detail: String::new() }), |f| f.prepare())
            }
            fn root(&self, admin: &Path, args: &[String]) -> std::io::Result<job::RootProc> {
                fake_launcher().ok_or_else(|| std::io::Error::other("no fake"))?.root(admin, args)
            }
            fn user(&self) -> &dyn helper::Runner {
                fake_launcher().map_or(&job::UserRunner as &dyn helper::Runner, |f| f.user())
            }
            fn nix_paths(&self) -> nixos::NixPaths {
                fake_launcher().map_or_else(nixos::NixPaths::system, |f| f.nix_paths())
            }
            fn has_flatpak(&self) -> bool {
                true
            }
            fn restart_facts(&self) -> (String, PathBuf, PathBuf) {
                fake_launcher().map_or_else(|| (String::new(), PathBuf::new(), PathBuf::new()), |f| f.restart_facts())
            }
            fn prompter(&self) -> Option<std::sync::Arc<dyn askpass::Prompter>> {
                askpass::prompter()
            }
            fn aur(&self, step: &plan::Step, session: &askpass::Session, line: &mut dyn FnMut(&str), cancel: &helper::CancelSignal) -> std::io::Result<Option<i32>> {
                match fake_launcher() {
                    Some(f) => f.aur(step, session, line, cancel),
                    None => job::run_aur(step, session, line, cancel),
                }
            }
        }
        return std::sync::Arc::new(Shared);
    }
    std::sync::Arc::new(job::PkexecLauncher)
}

/// The AUR helper the sandbox's fake pretends is installed
/// (`MANDARA_FAKE_AUR`; `None` outside the sandbox).
pub fn fake_aur() -> Option<aur::AurHelper> {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    {
        fake_launcher().and_then(|f| f.aur)
    }
    #[cfg(not(all(feature = "fake-system-update", debug_assertions)))]
    {
        None
    }
}

/// The NixOS files read for generations (the fake's tree in the sandbox).
pub fn nix_paths() -> nixos::NixPaths {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    if let Some(f) = fake_launcher() {
        return f.nix.clone();
    }
    nixos::NixPaths::system()
}

/// Where home-manager's generations are looked for.
pub fn home_profile_dirs() -> Vec<PathBuf> {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    if fake_launcher().is_some() {
        return vec![fake::fake_root().join("home/.local/state/nix/profiles")];
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    nixos::home_profile_dirs(&home, &user).into_iter().collect()
}

/// What the progress UI shows as the current stage.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Preparing,
    /// Waiting for the password dialog.
    Authorizing,
    Refreshing,
    Downloading,
    /// Checking signatures, integrity and file conflicts.
    Verifying,
    Installing,
    /// Post-transaction hooks, triggers and scriptlets.
    Hooks,
    /// NixOS: evaluating and building the new generation.
    Building,
    /// NixOS: switching to it.
    Activating,
    /// NixOS: deleting generations and collecting garbage.
    Cleaning,
    Flatpak,
    /// The AUR helper builds packages.
    AurBuild,
    Finished,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Preparing => t("Preparing…"),
            Phase::Authorizing => t("Waiting for your password…"),
            Phase::Refreshing => t("Refreshing package databases"),
            Phase::Downloading => t("Downloading"),
            Phase::Verifying => t("Checking packages"),
            Phase::Installing => t("Installing"),
            Phase::Hooks => t("Running hooks"),
            Phase::Building => t("Building the new generation"),
            Phase::Activating => t("Switching to it"),
            Phase::Cleaning => t("Cleaning up"),
            Phase::Flatpak => t("Updating Flatpak apps"),
            Phase::AurBuild => t("Building AUR packages"),
            Phase::Finished => t("Finished"),
        }
    }
}

/// Why an update stopped.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    /// Another program holds the package database lock.
    Locked,
    Network,
    /// File or package conflicts, broken dependencies: needs a person.
    Conflict,
    DiskFull,
    /// Signatures or keys.
    Keyring,
    /// The password dialog was dismissed or the password was wrong.
    NotAuthorized,
    /// No polkit agent answered, or pkexec is missing.
    NoPrompt,
    /// The admin helper is missing or too old and couldn't be set up.
    Helper,
    /// An AUR package didn't build.
    BuildFailed,
    Other,
}

impl FailureKind {
    /// A plain explanation with what to do next.
    pub fn advice(self) -> &'static str {
        match self {
            FailureKind::Locked => t("Another program is using the package manager. Wait for it to finish (or close it), then try again."),
            FailureKind::Network => t("The package servers couldn't be reached. Check your connection and try again."),
            FailureKind::Conflict => t("The package manager needs a decision from you (a conflict or broken dependency). Nothing was forced; read the log and finish it in a terminal."),
            FailureKind::DiskFull => t("There isn't enough free disk space. Free some space, then try again."),
            FailureKind::Keyring => t("A package signature couldn't be verified. Update your distribution's keyring, then try again."),
            FailureKind::NotAuthorized => t("The update needs an administrator's password, and it wasn't given."),
            FailureKind::NoPrompt => t("No password prompt is available: make sure polkit (pkexec) is installed and Mandara's polkit agent is running."),
            FailureKind::Helper => t("Mandara's admin helper isn't set up, so the update can't run as administrator."),
            FailureKind::BuildFailed => t("An AUR package didn't build. The log shows why; it often needs a fix from its maintainer. Nothing else was changed by this step."),
            FailureKind::Other => t("The update stopped with an error. The log shows what the package manager said."),
        }
    }
}

/// A stopped update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// The most telling line of the output.
    pub detail: String,
}

/// Package-name patterns whose update needs a restart to take effect: the
/// kernel, the init system and the C library.
const RESTART_EXACT: [&str; 14] = ["linux", "linux-lts", "linux-zen", "linux-hardened", "kernel", "kernel-core", "kernel-default", "systemd", "glibc", "libc6", "musl", "linux-virt", "linux-firmware", "dbus"];
/// Prefixes of kernel packages (`linux-cachyos`, `linux-image-6.1.0-18-amd64`,
/// `linux6.12` on Void, `kernel-6.11`).
const RESTART_PREFIXES: [&str; 5] = ["linux-image-", "linux-cachyos", "kernel-default-", "linux6.", "nvidia"];

/// Whether updating `package` calls for a restart.
pub fn needs_restart(package: &str) -> bool {
    RESTART_EXACT.contains(&package) || RESTART_PREFIXES.iter().any(|p| package.starts_with(p))
}

/// Whether the running kernel's modules are gone (replaced by an update).
pub fn kernel_modules_missing(release: &str, modules_root: &Path) -> bool {
    modules_root.is_dir() && !release.is_empty() && !modules_root.join(release).exists()
}

/// The running kernel release (`uname -r`).
#[cfg(not(target_os = "freebsd"))]
pub fn kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The running kernel release (`uname -r`, `kern.osrelease`).
#[cfg(target_os = "freebsd")]
pub fn kernel_release() -> String {
    // SAFETY: uname fills the zeroed struct it is given; its fields are
    // NUL-terminated within their fixed size.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(u.release.as_ptr()).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(os_release: &str, programs: &[&str], ostree: bool) -> System {
        let os = OsRelease::parse(os_release);
        let has = |p: &str| programs.contains(&p);
        System::detect_with(&Probe { os: &os, has_program: &has, ostree_booted: ostree })
    }

    #[test]
    fn detects_every_family_and_variant() {
        use plan::BackendId::*;
        assert_eq!(detect("ID=cachyos\nID_LIKE=arch", &[], false), System::Managed(Pacman));
        assert_eq!(detect("ID=ubuntu\nID_LIKE=debian", &[], false), System::Managed(Apt));
        assert_eq!(detect("ID=fedora", &[], false), System::Managed(Dnf));
        assert_eq!(detect("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"", &[], false), System::Managed(ZypperDup));
        assert_eq!(detect("ID=\"opensuse-leap\"\nID_LIKE=\"suse opensuse\"", &[], false), System::Managed(ZypperUpdate));
        assert_eq!(detect("ID=void", &[], false), System::Managed(Xbps));
        assert_eq!(detect("ID=alpine", &[], false), System::Managed(Apk));
        assert_eq!(detect("ID=nixos", &[], false), System::NixOs);
        assert_eq!(detect("NAME=FreeBSD\nVERSION=\"14.4-RELEASE\"\nVERSION_ID=\"14.4\"\nID=freebsd\nANSI_COLOR=\"0;31\"\nPRETTY_NAME=\"FreeBSD 14.4-RELEASE\"\n", &[], false), System::Managed(Pkg));
        assert_eq!(System::Managed(Pkg).label(), "pkg");
        assert_eq!(System::Managed(PkgBase).label(), "pkg");
        assert_eq!(detect("ID=gentoo", &[], false), System::Unsupported);
        assert_eq!(detect("ID=mystery", &["xbps-install"], false), System::Managed(Xbps));
        assert_eq!(detect("", &[], false), System::Unsupported);
    }

    /// FreeBSD 15.1's default install has a packaged base: pkg updates it
    /// and freebsd-update is left out. Only FreeBSD asks.
    #[test]
    fn a_packaged_freebsd_base_updates_through_pkg() {
        use plan::BackendId::*;
        const FREEBSD_15: &str = "NAME=FreeBSD\nVERSION=\"15.1-RELEASE\"\nVERSION_ID=\"15.1\"\nID=freebsd\nANSI_COLOR=\"0;31\"\nPRETTY_NAME=\"FreeBSD 15.1-RELEASE\"\nCPE_NAME=\"cpe:/o:freebsd:freebsd:15.1\"\n";
        let freebsd = detect(FREEBSD_15, &[], false);
        assert_eq!(freebsd, System::Managed(Pkg));
        assert_eq!(freebsd.with_packaged_base(|| true), System::Managed(PkgBase));
        assert_eq!(freebsd.with_packaged_base(|| false), System::Managed(Pkg), "a traditional install keeps freebsd-update");
        let linux = detect("ID=arch", &[], false);
        assert_eq!(linux.with_packaged_base(|| panic!("asked on Linux")), System::Managed(Pacman));
        // `pkg which -q /usr/bin/uname`, recorded on FreeBSD 15.1: a packaged
        // base, a traditional one (exit 1, nothing printed), and a stray answer.
        assert!(is_base_package_owner(true, "FreeBSD-runtime-15.1\n"));
        assert!(!is_base_package_owner(false, ""));
        assert!(!is_base_package_owner(true, "coreutils-9.5\n"));
        assert!(!packaged_base() || cfg!(target_os = "freebsd"));
    }

    #[test]
    fn image_based_systems_are_left_to_their_tool() {
        assert_eq!(detect("ID=fedora\nVARIANT_ID=silverblue", &[], true), System::ImageBased(ImageTool::RpmOstree));
        assert_eq!(detect("ID=\"opensuse-microos\"\nID_LIKE=\"suse opensuse opensuse-tumbleweed\"", &[], false), System::ImageBased(ImageTool::TransactionalUpdate));
        assert!(System::ImageBased(ImageTool::RpmOstree).explanation().unwrap().contains("rpm-ostree upgrade"));
        assert!(System::Managed(plan::BackendId::Pacman).explanation().is_none());
    }

    #[test]
    fn restart_packages() {
        for p in ["linux", "linux-cachyos-lts", "linux-image-6.1.0-18-amd64", "systemd", "glibc", "libc6", "musl", "kernel-core", "linux6.12", "nvidia-open"] {
            assert!(needs_restart(p), "{p}");
        }
        for p in ["linux-api-headers", "systemd-libs", "firefox", "glib2", "musl-utils", "libc-bin"] {
            assert!(!needs_restart(p), "{p}");
        }
    }

    #[test]
    fn missing_kernel_modules_mean_restart() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("6.17.1-arch1-1")).unwrap();
        assert!(!kernel_modules_missing("6.17.1-arch1-1", dir.path()));
        assert!(kernel_modules_missing("6.17.0-arch1-1", dir.path()));
        assert!(!kernel_modules_missing("6.17.0-arch1-1", &dir.path().join("absent")), "a container has no modules folder");
    }

    #[test]
    fn safe_lookup_ignores_paths() {
        assert!(find_safe("sh").is_some());
        assert!(find_safe("../bin/sh").is_none());
        assert!(find_safe("/bin/sh").is_none());
    }
}
