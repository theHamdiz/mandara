//! `mandara greeter …` command-line interface.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

use crate::install::{InstallPlan, Kiosk, UninstallPlan};
use crate::keyring_pam::{Action, KeyringPlan};
use crate::manager::{self, RestorePlan, Runner as _, SwitchPlan, SysRoot, SystemRunner};

#[derive(Args, Debug, Clone)]
pub struct GreeterArgs {
    /// Preview on the current session: no greetd, the password isn't
    /// checked, nothing is started; Esc Esc or "Exit demo" closes it.
    #[arg(long)]
    pub demo: bool,
    /// Language override (en, ar, es, fr, de, tr, ru, hi, zh, ja).
    #[arg(long)]
    pub lang: Option<String>,
    /// Theme override (see `mandara themes`).
    #[arg(long)]
    pub theme: Option<String>,
    /// Use an overlay layer surface instead of a full-screen window.
    #[arg(long)]
    pub layer_shell: bool,
    /// Backdrop override: auto, video, stills or aurora.
    #[arg(long)]
    pub background: Option<String>,
    /// With --demo: play a scripted wrong-then-right login (screenshots).
    #[arg(long, hide = true)]
    pub demo_autoplay: bool,
    #[command(subcommand)]
    pub action: Option<GreeterAction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum GreeterAction {
    /// Register Mandara Login as the login manager (via greetd). Prints the
    /// plan; writes files only under sudo; switches services only with --apply.
    Install {
        /// Also run the `systemctl disable <old>` / `enable greetd` commands.
        #[arg(long)]
        apply: bool,
        /// Kiosk compositor: cage (default), niri or hyprland.
        #[arg(long, default_value = "cage")]
        compositor: String,
        /// With --apply, leave greetd's PAM file alone (by default GNOME
        /// Keyring / KWallet are set to unlock at login: `keyring-unlock`).
        #[arg(long)]
        no_keyring_unlock: bool,
    },
    /// Unlock GNOME Keyring and KWallet when you log in with your password
    /// through Mandara Login: adds their PAM lines to greetd's PAM file
    /// (only installed modules, in marked blocks, backed up). Prints the
    /// plan; --apply adds the lines, --remove takes them out (root only).
    KeyringUnlock {
        /// Add the lines.
        #[arg(long, conflicts_with = "remove")]
        apply: bool,
        /// Take Mandara's lines out again.
        #[arg(long)]
        remove: bool,
        /// Only print the plan.
        #[arg(long)]
        dry_run: bool,
    },
    /// Validate a greeter config (default: your Studio draft,
    /// ~/.config/mandara/greeter.toml) and install it as
    /// /etc/mandara/greeter.toml. Needs root (sudo or pkexec).
    ApplyConfig {
        #[arg(long)]
        from: Option<std::path::PathBuf>,
    },
    /// Restore the previous greetd config and print how to re-enable the
    /// previous display manager (--apply runs it).
    Uninstall {
        #[arg(long)]
        apply: bool,
    },
    /// Show the active login screen and every one you can switch to.
    List,
    /// Switch the login screen (default: Mandara Login). Prints the plan;
    /// writes files (with a timestamped backup) only as root; switches
    /// display-manager.service only with --apply. Nothing is started or
    /// stopped: it takes effect at the next boot.
    Use {
        /// mandara, tuigreet, regreet, gtkgreet, nwg-hello, qtgreet,
        /// agreety, sddm, gdm, lightdm, ly or lxdm (see `greeter list`).
        #[arg(default_value = manager::MANDARA)]
        name: String,
        /// Only print the plan.
        #[arg(long)]
        dry_run: bool,
        /// Also run the `systemctl disable <old>` / `enable <new>` commands.
        #[arg(long)]
        apply: bool,
        /// First install what the choice is missing (the login manager or
        /// greeter, greetd, its kiosk compositor) with the system's package
        /// manager.
        #[arg(long)]
        install: bool,
        /// Mandara Login's kiosk compositor: cage (default), niri or hyprland.
        #[arg(long, default_value = "cage")]
        compositor: String,
    },
    /// Undo a `greeter use` switch from its backup (`latest` or an id).
    Restore {
        #[arg(default_value = "latest")]
        which: String,
        /// Only print the plan.
        #[arg(long)]
        dry_run: bool,
        /// Also switch display-manager.service back.
        #[arg(long)]
        apply: bool,
    },
}

pub fn main(args: GreeterArgs) -> Result<()> {
    match args.action {
        Some(GreeterAction::Install { apply, compositor, no_keyring_unlock }) => {
            let Some(kiosk) = Kiosk::parse(&compositor) else { bail!("unknown compositor {compositor:?} (cage, niri or hyprland)") };
            let plan = InstallPlan::build(kiosk);
            println!("{}", plan.describe());
            plan.execute(apply).map_err(anyhow::Error::msg)?;
            if apply && !no_keyring_unlock {
                // Offered by default; a problem here never fails the install.
                let root = SysRoot::system();
                let keyrings = KeyringPlan::build(&root, Action::Add);
                if keyrings.changes() {
                    println!("\n{}", keyrings.describe());
                    match keyrings.execute(&root) {
                        Ok(out) => println!("{out}"),
                        Err(e) => println!("warning: keyrings were not set up: {e}\n(run `sudo mandara greeter keyring-unlock --apply` later)"),
                    }
                }
            }
            Ok(())
        }
        Some(GreeterAction::KeyringUnlock { apply, remove, dry_run }) => {
            let root = SysRoot::system();
            let plan = KeyringPlan::build(&root, if remove { Action::Remove } else { Action::Add });
            println!("{}", plan.describe());
            if dry_run || manager::forced_dry_run() || !(apply || remove) {
                if !(apply || remove) && !dry_run && plan.changes() {
                    println!("Run with --apply (as root) to make this change.");
                }
                return if plan.errors.is_empty() { Ok(()) } else { bail!("this change can't run (see the errors above)") };
            }
            println!("{}", plan.execute(&root).map_err(anyhow::Error::msg)?);
            Ok(())
        }
        Some(GreeterAction::ApplyConfig { from }) => {
            let from = from.unwrap_or_else(crate::config::user_draft_path);
            let msg = crate::config::apply_config(&from).map_err(anyhow::Error::msg)?;
            println!("{msg}");
            Ok(())
        }
        Some(GreeterAction::Uninstall { apply }) => {
            let plan = UninstallPlan::build();
            println!("{}", plan.describe());
            plan.execute(apply).map_err(anyhow::Error::msg)
        }
        Some(GreeterAction::List) => {
            print!("{}", manager::describe_list(&manager::detect(&SysRoot::system())));
            Ok(())
        }
        Some(GreeterAction::Use { name, dry_run, apply, compositor, install }) => {
            let Some(kiosk) = Kiosk::parse(&compositor) else { bail!("unknown compositor {compositor:?} (cage, niri or hyprland)") };
            let exe = std::env::current_exe().unwrap_or_else(|_| "mandara".into());
            let root = SysRoot::system();
            let mut plan = SwitchPlan::build(&root, &name, kiosk, exe.clone()).map_err(anyhow::Error::msg)?;
            // `--install`: what's missing is installed first, so it no longer
            // blocks the plan.
            let missing = if install { manager::missing_for(&root, plan.target, kiosk, plan.detection.family) } else { Vec::new() };
            let packages = manager::packages_for(&missing);
            let install_cmd = manager::install_command(&root, plan.detection.family, &packages).and_then(|argv| manager::as_root(&root, argv));
            if !packages.is_empty() {
                match &install_cmd {
                    Some(cmd) => {
                        println!("Installs first: {}\n$ {}", packages.join(" "), cmd.join(" "));
                        let resolved: Vec<String> = missing.iter().map(manager::Missing::describe).collect();
                        plan.errors.retain(|e| !resolved.contains(e));
                    }
                    None => println!("No known package manager: install {} yourself.", packages.join(" ")),
                }
            }
            println!("{}", plan.describe());
            if dry_run || manager::forced_dry_run() {
                println!("Dry run: nothing was changed.");
                return if plan.errors.is_empty() { Ok(()) } else { bail!("this switch can't run yet (see the errors above)") };
            }
            if let Some(cmd) = install_cmd {
                SystemRunner.run(&cmd).map_err(anyhow::Error::msg)?;
                // Plan again against what's installed now.
                plan = SwitchPlan::build(&root, &name, kiosk, exe).map_err(anyhow::Error::msg)?;
            }
            println!("{}", plan.execute(&root, apply, &mut SystemRunner).map_err(anyhow::Error::msg)?);
            Ok(())
        }
        Some(GreeterAction::Restore { which, dry_run, apply }) => {
            let root = SysRoot::system();
            let plan = RestorePlan::build(&root, &which).map_err(anyhow::Error::msg)?;
            println!("{}", plan.describe());
            if dry_run || manager::forced_dry_run() {
                println!("Dry run: nothing was changed.");
                return Ok(());
            }
            println!("{}", plan.execute(&root, apply, &mut SystemRunner).map_err(anyhow::Error::msg)?);
            Ok(())
        }
        None => crate::run(crate::GreeterOptions {
            demo: args.demo,
            layer_shell: args.layer_shell.then_some(true),
            language: args.lang,
            theme: args.theme,
            background: args.background,
            autoplay: args.demo_autoplay,
        }),
    }
}
