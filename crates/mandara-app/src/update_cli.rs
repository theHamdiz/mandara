//! `mandara update`: Mandara's own updates and marketplace plugin updates.
//! Binary patches are the main method; the full download is the fallback
//! (logic in `mandara_services::self_update`).

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use mandara_services::marketplace::{Finding, Vetting};
use mandara_services::self_update::{self as su, ApplyOptions, ApplyReport, Method, Progress, install, plugins};

#[derive(Args)]
pub struct UpdateArgs {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Check for a newer release (the default).
    Check {
        /// Machine-readable answer.
        #[arg(long)]
        json: bool,
    },
    /// Install the newest release: binary patch first, full download as the fallback.
    Apply {
        /// Skip the patch and download the full release.
        #[arg(long)]
        full: bool,
        /// Show what would be downloaded without installing.
        #[arg(long)]
        dry_run: bool,
        /// Also replace a cargo build (`target/…/mandara`).
        #[arg(long)]
        force_dev: bool,
        /// Don't restart the running shell afterwards.
        #[arg(long)]
        no_restart: bool,
    },
    /// Put the previous binary (`mandara.old`) back.
    Rollback {
        #[arg(long)]
        no_restart: bool,
        #[arg(long)]
        force_dev: bool,
    },
    /// Marketplace plugin updates (first-party plugins update with the shell).
    Plugins {
        #[command(subcommand)]
        action: Option<PluginAction>,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List installed marketplace plugins with a newer release (the default).
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Update one plugin, or all with an update.
    Apply {
        /// Plugin id (all when omitted).
        id: Option<String>,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        dry_run: bool,
        /// Install a release that fails the compliance check anyway (it stays flagged).
        #[arg(long)]
        force: bool,
    },
    /// Put a plugin's previous version back.
    Rollback { id: String },
}

fn print_progress(p: Progress) {
    if let Progress::Step(s) = p {
        eprintln!("  {s}");
    }
}

fn describe(r: &ApplyReport) -> String {
    let how = match r.method {
        Method::Delta => "binary patch",
        Method::Full => "full download",
    };
    let saved = if r.full_size > 0 && r.downloaded < r.full_size { format!(" instead of {}", su::human_size(r.full_size)) } else { String::new() };
    let verb = if r.dry_run { "would download" } else { "downloaded" };
    let mut s = format!("{} via {how}: {verb} {}{saved}", r.version, su::human_size(r.downloaded));
    if let Some(reason) = &r.fallback {
        s.push_str(&format!(" (patch skipped: {reason})"));
    }
    s
}

fn restart_shell(exe: &std::path::Path) {
    if !install::running_from(exe) {
        return;
    }
    match crate::ipc::send(&mandara_config::paths::ipc_socket(), &["restart".to_string()]) {
        Ok(r) if r.ok => println!("restarted the running shell on the new version"),
        _ => println!("restart Mandara to use the new version (`mandara msg restart`)"),
    }
}

pub fn main(args: UpdateArgs) -> Result<()> {
    match args.action.unwrap_or(Action::Check { json: false }) {
        Action::Check { json } => {
            let info = su::check(true, su::MIN_INTERVAL).map_err(anyhow::Error::msg)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
                return Ok(());
            }
            if !info.available {
                println!("Mandara {} is up to date (latest release: {})", info.current, info.latest);
                return Ok(());
            }
            println!("Mandara {} is available (you have {}) — {}", info.latest, info.current, info.notes_url);
            match (info.patch.as_ref(), info.full_size()) {
                (Some(p), Some(full)) => println!("smart update: {} instead of {}", su::human_size(p.download.size), su::human_size(full)),
                (None, Some(full)) => println!("full update: {} (no patch from {})", su::human_size(full), info.current),
                _ => println!("no download for {} in this release", info.target),
            }
            match install::detect().updatable(false) {
                Ok(_) => println!("install it with `mandara update apply`"),
                Err(why) => println!("{why}"),
            }
            Ok(())
        }
        Action::Apply { full, dry_run, force_dev, no_restart } => {
            let install = install::detect();
            let exe = install.updatable(force_dev).map_err(anyhow::Error::msg)?.to_path_buf();
            let info = su::check(true, su::MIN_INTERVAL).map_err(anyhow::Error::msg)?;
            if !info.available {
                println!("Mandara {} is up to date", info.current);
                return Ok(());
            }
            let report = su::apply(&info, &exe, ApplyOptions { full_only: full, dry_run }, &print_progress).map_err(anyhow::Error::msg)?;
            println!("{}", describe(&report));
            if !dry_run {
                println!("installed {} (previous version kept as {})", exe.display(), install::backup_path(&exe).display());
                if !no_restart {
                    restart_shell(&exe);
                }
            }
            Ok(())
        }
        Action::Rollback { no_restart, force_dev } => {
            let install = install::detect();
            let exe = install.updatable(force_dev).map_err(anyhow::Error::msg)?.to_path_buf();
            install::rollback(&exe).map_err(anyhow::Error::msg)?;
            println!("restored the previous version of {}", exe.display());
            if !no_restart {
                restart_shell(&exe);
            }
            Ok(())
        }
        Action::Plugins { action } => plugins_main(action.unwrap_or(PluginAction::Check { json: false })),
    }
}

fn plugins_main(action: PluginAction) -> Result<()> {
    match action {
        PluginAction::Check { json } => {
            let updates = plugins::check_all(true, su::MIN_INTERVAL).map_err(anyhow::Error::msg)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&updates)?);
                return Ok(());
            }
            if updates.is_empty() {
                println!("no marketplace plugins installed");
            }
            for u in &updates {
                let size = u.download_size(false).map(su::human_size).unwrap_or_default();
                if u.available {
                    let kind = if u.patch.is_some() && u.meta.is_some() { "patch" } else { "download" };
                    println!("{:<24} {} → {}  ({size} {kind})", u.id, u.installed, u.latest);
                } else {
                    println!("{:<24} {} (up to date)", u.id, u.installed);
                }
            }
            Ok(())
        }
        PluginAction::Apply { id, full, dry_run, force } => {
            let updates = plugins::check_all(true, su::MIN_INTERVAL).map_err(anyhow::Error::msg)?;
            let chosen: Vec<_> = updates.iter().filter(|u| u.available && id.as_ref().is_none_or(|i| *i == u.id)).collect();
            if chosen.is_empty() {
                if let Some(id) = id
                    && !updates.iter().any(|u| u.id == id)
                {
                    bail!("{id} is not an installed marketplace plugin");
                }
                println!("all marketplace plugins are up to date");
                return Ok(());
            }
            let check = vetting_check();
            for u in chosen {
                let report = plugins::apply(u, ApplyOptions { full_only: full, dry_run }, &Vetting { check: &check, force }, &print_progress).map_err(anyhow::Error::msg)?;
                println!("{}: {}", u.id, describe(&report));
            }
            Ok(())
        }
        PluginAction::Rollback { id } => {
            plugins::rollback(&id).map_err(anyhow::Error::msg)?;
            println!("restored the previous version of {id}");
            Ok(())
        }
    }
}

/// The marketplace's compliance check, with the built-in plugin ids a
/// community plugin may not pose as.
fn vetting_check() -> impl Fn(&std::path::Path, &str) -> Result<(), Finding> {
    let mut registry = mandara_plugins::PluginRegistry::default();
    mandara_ui::plugins::register_builtin(&mut registry);
    let reserved = mandara_plugins::vet::builtin_ids(&registry);
    move |dir, repo| mandara_plugins::vet::vet(dir, &reserved.iter().map(String::as_str).collect::<Vec<_>>(), Some(repo))
}
