//! `mandara install` / `mandara uninstall` arguments.

use std::path::PathBuf;

use clap::Args;

/// Compositor names `--compositor` accepts (passed on to `mandara setup`).
pub const COMPOSITORS: &[&str] = &["mango", "mangowm", "niri", "hyprland", "hypr"];

#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallArgs {
    /// Install that release (downloaded and checksum-verified) instead of
    /// this binary.
    #[arg(long, value_name = "X.Y.Z", conflicts_with = "latest")]
    pub version: Option<String>,
    /// Install the latest release (downloaded and checksum-verified)
    /// instead of this binary.
    #[arg(long)]
    pub latest: bool,
    /// Build from source (installs Rust and build tools): the checkout this
    /// binary was built in, the current folder if it is one, else a clone
    /// in ~/.local/share/mandara/src.
    #[arg(long)]
    pub from_source: bool,
    /// Install into DIR/bin (default ~/.local).
    #[arg(long, value_name = "DIR")]
    pub prefix: Option<PathBuf>,
    /// Don't install system packages.
    #[arg(long)]
    pub no_deps: bool,
    /// Don't start Mandara with your compositor.
    #[arg(long)]
    pub no_autostart: bool,
    /// Keep your login screen (default: switch to Mandara Login, after asking).
    #[arg(long)]
    pub no_greeter: bool,
    /// mango, niri or hyprland (default: detected).
    #[arg(long, value_name = "NAME", value_parser = clap::builder::PossibleValuesParser::new(COMPOSITORS))]
    pub compositor: Option<String>,
    /// GitHub repository releases and sources come from (default
    /// theHamdiz/mandara, or $MANDARA_REPO). $MANDARA_RELEASE_BASE=<url>
    /// uses a mirror (needs --version).
    #[arg(long, value_name = "OWNER/NAME")]
    pub repo: Option<String>,
    /// Show everything that would happen; change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Don't ask: take every default answer.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Install this binary even when it doesn't match the release's
    /// published SHA-256.
    #[arg(long)]
    pub no_verify: bool,
    /// Remove Mandara instead (same as `mandara uninstall`).
    #[arg(long)]
    pub uninstall: bool,
}

#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallArgs {
    /// The prefix Mandara was installed into (default ~/.local).
    #[arg(long, value_name = "DIR")]
    pub prefix: Option<PathBuf>,
    /// Show what would be removed; change nothing.
    #[arg(long)]
    pub dry_run: bool,
}

impl InstallArgs {
    /// `--version`, without a leading `v`.
    pub fn wanted_version(&self) -> Option<String> {
        self.version.as_deref().map(|v| v.trim().trim_start_matches('v').to_string()).filter(|v| !v.is_empty())
    }
}

impl From<&InstallArgs> for UninstallArgs {
    fn from(a: &InstallArgs) -> Self {
        UninstallArgs { prefix: a.prefix.clone(), dry_run: a.dry_run }
    }
}

#[cfg(test)]
mod tests {
    use clap::{Parser, Subcommand};

    use super::*;

    #[derive(Parser, Debug)]
    #[command(name = "mandara", version)]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[derive(Subcommand, Debug)]
    enum Cmd {
        Install(InstallArgs),
        Uninstall(UninstallArgs),
    }

    fn install(args: &[&str]) -> Result<InstallArgs, clap::Error> {
        let cli = Cli::try_parse_from(["mandara", "install"].iter().chain(args))?;
        match cli.cmd {
            Cmd::Install(a) => Ok(a),
            Cmd::Uninstall(_) => unreachable!(),
        }
    }

    #[test]
    fn parses_every_flag() {
        let a = install(&["--version", "v0.3.1", "--prefix", "/opt/m", "--no-deps", "--no-autostart", "--no-greeter", "--compositor", "niri", "--repo", "me/fork", "--dry-run", "-y"]).unwrap();
        assert_eq!(a.wanted_version().as_deref(), Some("0.3.1"));
        assert_eq!(a.prefix.as_deref(), Some(std::path::Path::new("/opt/m")));
        assert!(a.no_deps && a.no_autostart && a.no_greeter && a.dry_run && a.yes);
        assert_eq!(a.compositor.as_deref(), Some("niri"));
        assert_eq!(a.repo.as_deref(), Some("me/fork"));
        let a = install(&["--from-source", "--uninstall", "--yes", "--prefix=/x", "--compositor=hyprland"]).unwrap();
        assert!(a.from_source && a.uninstall && a.yes);
        assert_eq!(a.compositor.as_deref(), Some("hyprland"));
        assert_eq!(install(&[]).unwrap(), InstallArgs::default());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(install(&["--compositor", "sway"]).is_err());
        assert!(install(&["--version", "1.0.0", "--latest"]).is_err());
        assert!(install(&["--bogus"]).is_err());
        assert!(install(&["--prefix"]).is_err());
    }

    #[test]
    fn uninstall_args() {
        let cli = Cli::try_parse_from(["mandara", "uninstall", "--dry-run", "--prefix", "/p"]).unwrap();
        let Cmd::Uninstall(u) = cli.cmd else { panic!() };
        assert!(u.dry_run);
        assert_eq!(u.prefix.as_deref(), Some(std::path::Path::new("/p")));
        let from = UninstallArgs::from(&InstallArgs { uninstall: true, dry_run: true, ..Default::default() });
        assert!(from.dry_run && from.prefix.is_none());
    }

    #[test]
    fn clap_definition_is_consistent() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }
}
