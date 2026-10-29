//! Mandara Shell's installer, in Rust: `mandara install` and
//! `mandara uninstall` (no shell scripts).
//!
//! Getting Mandara is download, extract, run: the release tarball's
//! `bin/mandara install` copies itself to `~/.local/bin` (checking its own
//! SHA-256 against the release's `update.json` or `SHA256SUMS` when they can
//! be reached), installs the runtime packages of your distribution, hooks
//! into your compositor (`mandara setup`, one include line), offers Mandara
//! Login (greetd + cage) and keyring unlock, and offers DDC/CI monitor
//! brightness. `--version` / `--latest` download a release instead and
//! `--from-source` builds one.
//!
//! * [`family`] — distribution families from `/etc/os-release`;
//! * [`packages`] — what each family installs (data tables);
//! * [`pkg`] — package managers and privilege helpers, shared with
//!   `mandara greeter use --install`;
//! * [`installer`] — the flows, against a [`host::Host`] so tests (and
//!   `--dry-run`) change nothing.

pub mod args;
pub mod family;
pub mod files;
pub mod host;
pub mod installer;
pub mod packages;
pub mod pkg;
pub mod platform;
pub mod release;

pub use args::{InstallArgs, UninstallArgs};
pub use family::Family;
pub use installer::{Facts, Installer, LoginProbe};
pub use pkg::{PackageManager, Privilege};

use host::SystemHost;

/// `mandara install`.
pub fn install(args: InstallArgs, probe: &dyn LoginProbe) -> Result<(), String> {
    let mut host = SystemHost::new();
    let facts = Facts::detect(&host, args.repo.as_deref());
    Installer::new(&mut host, probe, facts, args).install()
}

/// `mandara uninstall`.
pub fn uninstall(args: UninstallArgs, probe: &dyn LoginProbe) -> Result<(), String> {
    let mut host = SystemHost::new();
    let facts = Facts::detect(&host, None);
    let args = InstallArgs { prefix: args.prefix, dry_run: args.dry_run, uninstall: true, ..Default::default() };
    Installer::new(&mut host, probe, facts, args).uninstall()
}
