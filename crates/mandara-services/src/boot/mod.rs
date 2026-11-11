//! Boot look: the boot menu (GRUB, Limine, rEFInd) and the boot splash
//! (Plymouth), restyled with curated open-source themes or your own image.
//!
//! This touches what a machine boots with, so it is built around one rule:
//! nothing changes that wasn't shown first, backed up, checked by the
//! tool's own checker and undoable in one step.
//!
//! * [`detect`] reads, without changing anything, which bootloader this
//!   machine boots with (the Boot Loader Interface's `LoaderInfo`, then the
//!   firmware's `BootCurrent`, then the installed configs), where each
//!   tool keeps its files, and how the initramfs is rebuilt.
//! * [`catalog`] pins popular themes to their authors' release or commit
//!   archives by SHA-256; [`archive`] unpacks them with pure-Rust readers
//!   that refuse unsafe paths and keep only theme files (images, fonts,
//!   theme descriptions): nothing in an archive is ever run.
//! * [`image`] turns your SVG or PNG into a Plymouth splash, a GRUB theme
//!   and a Limine wallpaper; [`theme`] checks every theme before use.
//! * [`plan`] turns a [`plan::Request`] into the exact files and commands
//!   (shown to you first); [`helper`] is the root side (`mandara
//!   boot-admin`), which detects the system again with admin rights,
//!   rebuilds the plan, refuses when it differs from the one you approved,
//!   backs up every file it changes to [`BACKUP_DIR`], runs the tools'
//!   own checkers before replacing anything and rolls back on failure.
//!   [`helper::restore`] undoes an apply.
//! * [`stage`] is the user side: downloads, imports and generated themes
//!   are prepared in Mandara's cache, never in system folders.
//!
//! FreeBSD's loader is out of scope: [`detect`] reports it as unsupported.

pub mod archive;
pub mod catalog;
pub mod detect;
pub mod edit;
pub mod helper;
pub mod image;
pub mod plan;
pub mod stage;
pub mod theme;
#[cfg(test)]
mod vm_tests;

use std::path::{Path, PathBuf};

/// Root-owned state: backups and the record of installed themes.
pub const STATE_DIR: &str = "/var/lib/mandara-boot";
/// One folder per apply, each with a `record.json` (readable by everyone,
/// so Studio can offer Restore) and the backed-up files (root only).
pub const BACKUP_DIR: &str = "/var/lib/mandara-boot/backups";
/// Staged themes are copied here (root-owned) and checked again before use.
pub const INCOMING_DIR: &str = "/var/lib/mandara-boot/incoming";
/// The theme folders Mandara installed (the only ones it replaces or
/// removes).
pub const INSTALLED_FILE: &str = "/var/lib/mandara-boot/installed.json";
/// Prefix of the GRUB / generated theme folders Mandara names itself.
pub const PREFIX: &str = "mandara-";
/// The generated splash theme's name.
pub const SPLASH_NAME: &str = "mandara-splash";
/// The generated GRUB theme's name.
pub const GRUB_IMAGE_NAME: &str = "mandara-image";
/// Marker on lines Mandara writes or retires in a config file.
pub const MARK: &str = "mandara-boot-look";

/// Where files are read and written: `/` for real, a temporary folder in
/// tests (fixtures of whole distribution layouts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sys {
    pub root: PathBuf,
}

impl Sys {
    pub fn real() -> Self {
        Self { root: PathBuf::from("/") }
    }

    pub fn under(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }

    /// `abs` (an absolute system path) under this root.
    pub fn path(&self, abs: impl AsRef<Path>) -> PathBuf {
        let abs = abs.as_ref();
        self.root.join(abs.strip_prefix("/").unwrap_or(abs))
    }

    /// The system path of `p` (a path under this root).
    pub fn system_path(&self, p: &Path) -> PathBuf {
        match p.strip_prefix(&self.root) {
            Ok(rel) => Path::new("/").join(rel),
            Err(_) => p.to_path_buf(),
        }
    }

    pub fn is_real(&self) -> bool {
        self.root == Path::new("/")
    }
}

/// A theme folder or file name Mandara accepts: letters, digits, `.`, `_`
/// and `-`, not starting with `.` or `-`, at most 64 characters.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with(['.', '-']) && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file (`None` when it can't be read).
pub fn sha256_file(path: &Path) -> Option<String> {
    crate::cursors::install::sha256_file(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain() {
        for ok in ["rings", "catppuccin-mocha", "mandara-vimix_2k", "Dracula.v2"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".hidden", "-flag", "a/b", "..", "a b", "x$(id)", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn sys_maps_paths_both_ways() {
        let sys = Sys::under(Path::new("/tmp/fx"));
        assert_eq!(sys.path("/etc/default/grub"), Path::new("/tmp/fx/etc/default/grub"));
        assert_eq!(sys.system_path(Path::new("/tmp/fx/boot/grub")), Path::new("/boot/grub"));
        assert!(Sys::real().is_real() && !sys.is_real());
        assert_eq!(Sys::real().path("/boot"), Path::new("/boot"));
    }
}
