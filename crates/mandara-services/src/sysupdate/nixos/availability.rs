//! Check the default configuration before offering an action; generations do not need a build source.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mandara_i18n::tk;
use serde::Serialize;

use super::{NixAction, NixPaths, Setup};

/// An offline attribute-name lookup should finish promptly even when a flake is broken.
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    MissingConfiguration,
    MissingChannel,
    MissingHost,
    UnverifiedFlake,
    ReadOnlyFlake,
    MissingTools,
    NoPreviousGeneration,
    NoCurrentGeneration,
}

impl Reason {
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingConfiguration => tk(
                "The default NixOS configuration was not found. Use your own rebuild command; Flatpak updates remain available.",
            ),
            Self::MissingChannel => tk(
                "The default root NixOS channel was not found. Use your own rebuild command; Flatpak updates remain available.",
            ),
            Self::MissingHost => tk(
                "The default flake has no configuration matching this hostname. Use your own rebuild command; Flatpak updates remain available.",
            ),
            Self::UnverifiedFlake => tk(
                "The default NixOS flake could not be verified offline. Refresh or use your own rebuild command.",
            ),
            Self::ReadOnlyFlake => tk(
                "This flake cannot be updated here because its folder is read-only or belongs to another user.",
            ),
            Self::MissingTools => tk("Required NixOS update tools or polkit are not installed."),
            Self::NoPreviousGeneration => {
                tk("No earlier system generation is available to roll back to.")
            }
            Self::NoCurrentGeneration => {
                tk("The current NixOS system generation could not be identified.")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Availability {
    pub update: Option<Reason>,
    pub rebuild: Option<Reason>,
    pub rollback: Option<Reason>,
    pub generations: Option<Reason>,
    pub cleanup: Option<Reason>,
}

impl Availability {
    pub fn reason(self, action: NixAction) -> Option<Reason> {
        match action {
            NixAction::Update(_) => self.update,
            NixAction::Rebuild(_) => self.rebuild,
            NixAction::Rollback => self.rollback,
            NixAction::SwitchTo(_) => self.generations,
            NixAction::Clean(_) => self.cleanup,
        }
    }
}

trait Inspector {
    fn tool(&self, name: &str) -> bool;
    fn flake_has_host(&self, dir: &Path, host: &str, cancel: &AtomicBool) -> Result<bool, ()>;
    fn writable_flake(&self, dir: &Path, root_owned: bool) -> bool;
}

struct Native;
impl Inspector for Native {
    fn tool(&self, name: &str) -> bool {
        if name == "pkexec" {
            return crate::updates::find_program(name)
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|path| mandara_config::admin::trusted_root_file(&path));
        }
        super::super::helper::trusted_program(name).is_ok()
    }

    fn flake_has_host(&self, dir: &Path, host: &str, cancel: &AtomicBool) -> Result<bool, ()> {
        let program = super::super::helper::trusted_program("nix").map_err(|_| ())?;
        let mut command = std::process::Command::new(program);
        let trust = if rustix::process::geteuid().is_root() {
            super::git_work_tree(dir)
                .map(|tree| super::super::helper::GitTrust::new(&tree, Path::new("/root")))
                .transpose()
                .map_err(|_| ())?
        } else {
            None
        };
        if let Some(trust) = &trust {
            command.env("XDG_CONFIG_HOME", trust.config_home());
        }
        command
            .args(super::NIX_FEATURES)
            .args([
                "eval",
                "--json",
                "--read-only",
                "--offline",
                "--no-write-lock-file",
                "--option",
                "allow-import-from-derivation",
                "false",
                "--option",
                "accept-flake-config",
                "false",
                "--apply",
                "builtins.attrNames",
            ])
            .arg(format!("{}#nixosConfigurations", dir.display()));
        let (output, _) =
            crate::app_themes::process::native(command, cancel, CHECK_TIMEOUT).map_err(|_| ())?;
        let names: Vec<String> = serde_json::from_str(&output).map_err(|_| ())?;
        Ok(names.iter().any(|name| name == host))
    }

    fn writable_flake(&self, dir: &Path, root_owned: bool) -> bool {
        use std::os::unix::fs::PermissionsExt as _;
        if dir.starts_with("/nix/store") {
            return false;
        }
        if root_owned {
            return std::fs::metadata(dir).is_ok_and(|meta| meta.permissions().mode() & 0o200 != 0);
        }
        rustix::fs::accessat(
            rustix::fs::CWD,
            dir,
            rustix::fs::Access::WRITE_OK,
            rustix::fs::AtFlags::EACCESS,
        )
        .is_ok()
    }
}

/// Runs only on a bounded background job; never downloads inputs or updates a lock file.
pub fn inspect(paths: &NixPaths, cancel: &AtomicBool) -> Availability {
    inspect_with(paths, cancel, &Native)
}

fn inspect_with(paths: &NixPaths, cancel: &AtomicBool, probe: &impl Inspector) -> Availability {
    inspect_actions(paths, cancel, probe, true)
}

fn inspect_actions(
    paths: &NixPaths,
    cancel: &AtomicBool,
    probe: &impl Inspector,
    builds: bool,
) -> Availability {
    let setup = super::setup(paths);
    let rebuild_tools = probe.tool("pkexec") && probe.tool("nixos-rebuild");
    let mut update_only = None;
    let rebuild = if !builds {
        None
    } else {
        match &setup {
            Setup::Flake {
                dir,
                host,
                root_owned,
            } => {
                if !paths.etc_nixos.join("flake.nix").is_file() || !dir.join("flake.nix").is_file()
                {
                    Some(Reason::MissingConfiguration)
                } else if !super::valid_host(host) {
                    Some(Reason::MissingHost)
                } else if !rebuild_tools || !probe.tool("nix") {
                    Some(Reason::MissingTools)
                } else {
                    if !probe.writable_flake(dir, *root_owned) {
                        update_only = Some(Reason::ReadOnlyFlake);
                    }
                    match probe.flake_has_host(dir, host, cancel) {
                        Ok(true) => None,
                        Ok(false) => Some(Reason::MissingHost),
                        Err(()) => Some(Reason::UnverifiedFlake),
                    }
                }
            }
            Setup::Channels { .. } => {
                if !paths.etc_nixos.join("configuration.nix").is_file() {
                    Some(Reason::MissingConfiguration)
                } else if !paths
                    .root_channels
                    .join("nixos/nixos/default.nix")
                    .is_file()
                {
                    Some(Reason::MissingChannel)
                } else if !rebuild_tools || !probe.tool("nix-build") {
                    Some(Reason::MissingTools)
                } else {
                    if !probe.tool("nix-channel") {
                        update_only = Some(Reason::MissingTools);
                    }
                    None
                }
            }
        }
    };
    let generations = super::generations(paths);
    let current = generations.iter().find(|generation| generation.current);
    let generation_tools =
        probe.tool("pkexec") && probe.tool("nix-env") && probe.tool("systemd-run");
    let generation_reason = if !generation_tools {
        Some(Reason::MissingTools)
    } else if current.is_none() {
        Some(Reason::NoCurrentGeneration)
    } else {
        None
    };
    let rollback = generation_reason.or_else(|| {
        current
            .filter(|current| {
                generations
                    .iter()
                    .any(|generation| generation.number < current.number)
            })
            .is_none()
            .then_some(Reason::NoPreviousGeneration)
    });
    let cleanup = generation_reason
        .or_else(|| (!probe.tool("nix-collect-garbage")).then_some(Reason::MissingTools));
    Availability {
        update: rebuild.or(update_only),
        rebuild,
        rollback,
        generations: generation_reason,
        cleanup,
    }
}

pub fn check(paths: &NixPaths, action: NixAction, cancel: &AtomicBool) -> Result<(), String> {
    inspect_actions(
        paths,
        cancel,
        &Native,
        matches!(action, NixAction::Update(_) | NixAction::Rebuild(_)),
    )
    .reason(action)
    .map_or(Ok(()), |reason| {
        Err(mandara_i18n::tr(reason.message()).to_string())
    })
}

#[cfg(test)]
mod tests;
