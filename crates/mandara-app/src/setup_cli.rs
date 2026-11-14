//! `mandara setup`: compositor integration from the command line — the
//! drop-in, the single include line, conflict detection / retirement and the
//! Config doctor, every change checked with the compositor's own checker and
//! rolled back when it makes the config worse.

use std::cell::RefCell;
use std::path::PathBuf;

use anyhow::Result;
use mandara_config::compositor::{Emitted, Target, install_include, user_autostarts};
use mandara_config::conflicts;
use mandara_config::doctor::{self, CheckEnv, CheckerStatus, Report, fix as dfix, state};
use mandara_config::paths;

use crate::ipc;
mod mango;

/// `mandara setup --check` exit code when conflicts were found (scripts and
/// `mandara install` test for it).
pub const EXIT_CONFLICTS: i32 = 10;
/// `mandara setup --check` exit code when the compositor reports errors in
/// the config (and there are no conflicts).
pub const EXIT_CONFIG_ERRORS: i32 = 11;
/// A required library is unavailable; no configuration was inspected or changed.
pub const EXIT_PREREQUISITE: i32 = 12;
/// The requested compositor could not pass its availability check.
pub const EXIT_AVAILABILITY: i32 = 13;

pub struct Options {
    pub autostart: bool,
    pub compositor: Option<String>,
    pub check: bool,
    pub fix_conflicts: bool,
    pub restore: Option<String>,
    pub fix: Vec<String>,
}


/// Whether sway is installed (on `PATH`).
fn which_sway() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join("sway").is_file()))
}

fn setup_targets(compositor: Option<&str>) -> Result<Vec<Target>> {
    let targets = match compositor {
        Some(name) => vec![Target::parse(name).ok_or_else(|| anyhow::anyhow!("unknown compositor {name:?} ({})", Target::ALL.map(Target::id).join(", ")))?],
        None => match mandara_compositor::detect() {
            Some(kind) => vec![kind.target()],
            // Sway runs its system-wide config until the user has one.
            None => Target::ALL.into_iter().filter(|t| t.user_config().exists() || (*t == Target::Sway && std::path::Path::new(mandara_config::compositor::SWAY_SYSTEM_CONFIG).exists() && which_sway())).collect(),
        },
    };
    Ok(targets)
}

pub fn setup(o: Options) -> Result<()> {
    let restore_only = o.restore.is_some() && !o.autostart && !o.check && !o.fix_conflicts && o.fix.is_empty();
    let mut targets = setup_targets(o.compositor.as_deref())?;
    // Without an explicit or running compositor, a restored file can create
    // another target. Check prerequisites before a combined restore/check.
    let uncertain_restore = o.restore.is_some() && o.compositor.is_none() && mandara_compositor::detect().is_none();
    if !restore_only && (uncertain_restore || targets.iter().any(|target| !target.self_configured())) && !mandara_config::mango_keys::is_available() {
        eprintln!("{}", mandara_i18n::t("Install libxkbcommon before checking or changing compositor shortcuts."));
        std::process::exit(EXIT_PREREQUISITE);
    }

    if let Some(which) = &o.restore {
        let backup = if which == "latest" { conflicts::backups().into_iter().next().ok_or_else(|| anyhow::anyhow!("no retire backups in {}", conflicts::backups_root().display()))? } else { PathBuf::from(which) };
        // Doctor backups and retire backups share the manifest format; a
        // doctor backup may also record a file that didn't exist.
        let restored = if backup.starts_with(dfix::backups_root()) { dfix::restore(&backup)? } else { conflicts::restore(&backup)? };
        for f in restored {
            println!("restored {}", f.display());
        }
    }
    if restore_only {
        if std::env::var_os("MANDARA_NO_IPC").is_none() {
            let _ = ipc::send(&paths::ipc_socket(), &["compositor".to_string(), "reload".to_string()]);
        }
        return Ok(());
    }
    if o.restore.is_some() { targets = setup_targets(o.compositor.as_deref())?; }
    if targets.is_empty() {
        println!("No {} config found — start your compositor once, then run `mandara setup` again.", Target::ALL.map(Target::name).join(", "));
        return Ok(());
    }
    let mut store = mandara_config::ConfigStore::open_default()?;
    if o.autostart && !o.check && !store.get().compositor.autostart {
        store.update(|c| c.compositor.autostart = true);
        store.flush()?;
    }
    let mut cfg = store.snapshot();
    let mut total_conflicts = 0;
    let mut config_errors = 0;
    let mut unavailable = false;
    // `--fix` ids that matched a problem of some compositor.
    let mut fixed_ids: Vec<String> = Vec::new();
    for target in targets {
        if target == Target::KWin {
            if o.check {
                println!("{}", mandara_i18n::t("Mandara uses native Plasma protocols; KWin’s configuration stays unchanged"));
                let report = doctor::check(target);
                print_report(&report);
                unavailable |= report.errors() > 0 || report.status != CheckerStatus::Ran;
            } else if let Some(path) = mandara_config::kwin::sync_autostart(cfg.compositor.autostart, &mandara_config::paths::self_exe())? {
                println!("{}", mandara_i18n::tf("KWin: startup entry {path}", &[("path", &path.display())]));
            }
            continue;
        }
        if matches!(target, Target::Aqueous | Target::Labwc | Target::Cosmic) {
            // No drop-in, no include line, no startup entry: these read only
            // their own settings, and Mandara follows them over their socket
            // (Aqueous) or standard Wayland protocols (labwc, COSMIC).
            if let Some(note) = target.settings_elsewhere() {
                println!("{}", mandara_i18n::t(note));
            }
            if cfg.compositor.autostart && !o.check {
                let command = mandara_config::compositor::shell_quote(&mandara_config::paths::self_exe().display().to_string());
                let hint = match target {
                    Target::Labwc => mandara_i18n::tf("To start Mandara with labwc, add the line “{command} &” to ~/.config/labwc/autostart.", &[("command", &command)]),
                    Target::Cosmic => mandara_i18n::tf("To start Mandara with COSMIC, add {command} in COSMIC Settings → Applications → Startup applications.", &[("command", &command)]),
                    _ => mandara_i18n::tf("To start Mandara with Aqueous, add {command} to Aqueous's own startup settings.", &[("command", &command)]),
                };
                println!("{hint}");
            }
            if o.check {
                let report = doctor::check(target);
                print_report(&report);
                unavailable |= report.errors() > 0 || report.status != CheckerStatus::Ran;
            }
            continue;
        }
        // Sway without a config of its own runs the system one: the user's
        // starts as a copy of it (sway's documented first step), so the
        // include line has somewhere to go and sway's defaults stay.
        if target == Target::Mango {
            mango::check_path()?;
            if !o.check && o.fix.is_empty() { mandara_config::compositor::seed_mango_user_config()?; }
        }
        if target == Target::Sway && !o.check && o.fix.is_empty() && mandara_config::compositor::seed_sway_user_config()? {
            println!("Sway: started {} as a copy of {}", target.user_config().display(), mandara_config::compositor::SWAY_SYSTEM_CONFIG);
        }
        let has_config = target.user_config().exists();
        if target == Target::Mango && !has_config && !o.check {
            anyhow::bail!("{}", mandara_i18n::tf("No compositor configuration was found at {path}.", &[("path", &target.user_config().display())]));
        }
        let env = CheckEnv::default();
        // The config as the compositor reads it before Mandara touches it.
        let before = doctor::check(target);
        // The last drop-in written (its skipped binds are reported).
        let last: RefCell<Option<(PathBuf, Emitted)>> = RefCell::new(None);
        let effective = cfg.effective_compositor().into_owned();
        let write = || -> Result<()> {
            let written = mandara_compositor::backend::write_dropin(target, &effective, None, mandara_config::compositor::installed_version(target))?;
            *last.borrow_mut() = Some(written);
            Ok(())
        };
        let mut report = before.clone();
        if !o.check && o.fix.is_empty() {
            if !has_config {
                write()?;
                let path = last.borrow().as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| target.generated_file());
                println!("{target:?}: drop-in {} · {} doesn't exist yet — add this line to your config: {}", path.display(), target.user_config().display(), target.include_line());
                continue;
            }
            let guarded = dfix::guarded_write(&before, &env, &write)?;
            for option in &guarded.left_out {
                println!("  note: {} {} rejects `{option}`, so Mandara leaves it out for this version", target.name(), guarded.after.version.as_deref().unwrap_or("?"));
            }
            if guarded.rolled_back {
                println!("{target:?}: the new drop-in made {} reject your config, so the previous one was kept:", target.name());
                print_report(&guarded.after);
                config_errors += guarded.after.errors();
                state::record(guarded.after);
                continue;
            }
            report = guarded.after;
            if target == Target::River {
                // River's init is a program Mandara never edits: the shell
                // sends the settings over river-control when it runs.
                let path = last.borrow().as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| target.generated_file());
                println!("River: settings recorded in {}; the shell sends them to River when it starts and on every change", path.display());
                if !mandara_config::compositor::include_installed(target) {
                    println!("  to start Mandara with River, add this line to {}: {}", target.user_config().display(), target.include_line());
                }
                for note in last.borrow().as_ref().map(|(_, e)| e.skipped.clone()).unwrap_or_default() {
                    println!("  note: {note}");
                }
            } else {
                let user_before = std::fs::read_to_string(target.user_config())?;
                let added = install_include(target)?;
                if added {
                    let after = doctor::check(target);
                    if dfix::is_worse(&report, &after) {
                        // Our include made it worse: undo exactly that.
                        mandara_config::write_atomic(&target.user_config(), user_before.as_bytes())?;
                        println!("{target:?}: {} rejects the config with Mandara's include, so it was removed again:", target.name());
                        print_report(&after);
                        config_errors += after.errors();
                        continue;
                    }
                    report = after;
                }
                let path = last.borrow().as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| target.generated_file());
                println!("{target:?}: drop-in {} · include {} {}", path.display(), if added { "added to" } else { "already in" }, target.user_config().display());
                if cfg.compositor.autostart {
                    println!("  autostart: {}", if user_autostarts(target) { "your config already starts Mandara" } else { "on (from the drop-in)" });
                }
                for note in last.borrow().as_ref().map(|(_, e)| e.skipped.clone()).unwrap_or_default() {
                    println!("  note: {note}");
                }
            }
        }
        if !has_config {
            print_report(&report);
            config_errors += 1;
            continue;
        }
        // Fixes asked for by id, bottom of each file first: a removed line
        // never shifts the lines of the fixes still to come.
        let mut wanted: Vec<(doctor::Diagnostic, dfix::Fix)> = Vec::new();
        for spec in &o.fix {
            let Some((id, action)) = spec.split_once('=') else {
                anyhow::bail!("--fix takes ID=comment, ID=remove or ID=regenerate, not {spec:?}");
            };
            let Some(action) = dfix::Fix::parse(action) else { anyhow::bail!("unknown fix {action:?} (comment, remove or regenerate)") };
            if let Some(d) = report.find(id) {
                fixed_ids.push(id.to_string());
                wanted.push((d.clone(), action));
            }
        }
        wanted.sort_by(|(a, _), (b, _)| (&b.file, b.line).cmp(&(&a.file, a.line)));
        for (d, action) in wanted {
            // The same problem in the latest check (its id is stable while
            // the lines above it are).
            let Some(id) = report.diagnostics.iter().find(|x| x.file == d.file && x.line == d.line && x.message == d.message).map(|x| x.id.clone()) else {
                println!("  {}: already gone", d.id);
                continue;
            };
            let outcome = dfix::apply(&report, &id, action, &env, &write)?;
            let backup = outcome.backup.as_ref().map(|b| b.display().to_string()).unwrap_or_default();
            if outcome.rolled_back {
                println!("  {id}: {} made the config worse, so it was undone (backup {backup})", action.id());
            } else {
                println!("  {id}: {}{} · backup {backup} · undo: mandara setup --restore {backup}", action.id(), if outcome.resolved { " — fixed" } else { " — the problem is still reported" });
            }
            report = outcome.after;
        }
        let report = state::record(report);
        print_report(&report);
        config_errors += report.errors();
        if o.check && target == Target::Mango && cfg.compositor.autostart
            && !user_autostarts(target)
            && (!mandara_config::compositor::include_installed(target)
                || !std::fs::read_to_string(target.generated_file()).is_ok_and(|text| mandara_config::compositor::starts_mandara(&text)))
        {
            println!("{}", mandara_i18n::t("Autostart is enabled but its generated configuration is not connected. Run `mandara setup` to repair it."));
            config_errors += 1;
        }
        let found = conflicts::scan(target, &cfg.compositor);
        let fixable: Vec<conflicts::Conflict> = found.iter().filter(|c| c.is_conflict()).cloned().collect();
        total_conflicts += fixable.len();
        for c in &found {
            println!("  {} {}:{} — {}", if c.is_conflict() { "conflict:" } else { "note:" }, c.file.display(), c.lines[0] + 1, c.summary());
            println!("      {}", c.text);
        }
        if o.fix_conflicts && !o.check && !fixable.is_empty() && target == Target::River {
            // River's init is a program Mandara never edits.
            println!("  River: Mandara doesn't edit {}; remove or comment the lines above yourself", target.user_config().display());
        } else if o.fix_conflicts && !o.check && !fixable.is_empty() {
            let backup = conflicts::retire(target, &fixable)?;
            let after = doctor::check(target);
            if dfix::is_worse(&report, &after) {
                conflicts::restore(&backup)?;
                println!("  {} rejected the result, so nothing was changed:", target.name());
                print_report(&after);
            } else {
                // Keys an old tool used now open its Mandara equivalent.
                let takeovers = conflicts::takeover_keybinds(&fixable);
                if !takeovers.is_empty() {
                    store.update(|c| {
                        for k in takeovers {
                            if !c.compositor.keybinds.iter().any(|x| x.chord() == k.chord()) {
                                c.compositor.keybinds.push(k);
                            }
                        }
                    });
                    store.flush()?;
                    cfg = store.snapshot();
                }
                // Freed chords: regenerate so Mandara's binds take them.
                mandara_compositor::backend::write_dropin(target, &cfg.effective_compositor(), None, mandara_config::compositor::installed_version(target))?;
                total_conflicts -= fixable.len();
                let checked = match &after.status {
                    CheckerStatus::Ran => format!(", checked with {}", target.name()),
                    _ => String::new(),
                };
                println!("  retired {} line(s){checked} · backup {} · undo: mandara setup --restore {}", fixable.len(), backup.display(), backup.display());
            }
        } else if !fixable.is_empty() && !o.check {
            println!("  run `mandara setup --fix-conflicts` to retire them (backed up, validated, reversible)");
        }
        if !o.check && std::env::var_os("MANDARA_NO_IPC").is_none() {
            // Ask a running shell to reload the compositor with the new files.
            let _ = ipc::send(&paths::ipc_socket(), &["compositor".to_string(), "reload".to_string()]);
        }
    }
    for spec in &o.fix {
        let id = spec.split_once('=').map_or(spec.as_str(), |(id, _)| id);
        if !fixed_ids.iter().any(|f| f == id) {
            println!("no problem {id} in the current check — run `mandara setup --check` for the current ids");
        }
    }
    if o.check && unavailable { std::process::exit(EXIT_AVAILABILITY); }
    if o.check && total_conflicts > 0 {
        std::process::exit(EXIT_CONFLICTS);
    }
    if o.check && config_errors > 0 {
        std::process::exit(EXIT_CONFIG_ERRORS);
    }
    Ok(())
}

/// The Config doctor's findings for a terminal: grouped by file, each with
/// its line, explanation and the commands that fix it.
fn print_report(report: &Report) {
    let name = report.target.name();
    let version = report.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
    match &report.status {
        CheckerStatus::NoConfig => {
            println!("{}", mandara_i18n::tf("No compositor configuration was found at {path}.", &[("path", &report.root.display())]));
            return;
        }
        CheckerStatus::NotInstalled => {
            println!("  config doctor: {name}'s checker isn't installed here, so the config wasn't checked");
            return;
        }
        CheckerStatus::TimedOut => {
            println!("  config doctor: `{}` didn't finish in time; the config wasn't checked", report.checker);
            return;
        }
        CheckerStatus::Failed(e) => {
            println!("  config doctor: `{}` couldn't run: {e}", report.checker);
            return;
        }
        CheckerStatus::Ran => {}
    }
    if report.diagnostics.is_empty() {
        println!("  config doctor: all good — {name}{version} accepts the config");
        return;
    }
    let unverified = if report.verified_parser { "" } else { " (reading of its output not verified against a real binary)" };
    println!("  config doctor: {name}{version} reports {} error(s), {} warning(s){unverified}", report.errors(), report.warnings());
    for (file, list) in report.by_file() {
        match &file {
            Some(f) if mandara_config::compositor::is_generated(f) => println!("    {} (Mandara's drop-in)", f.display()),
            Some(f) => println!("    {}", f.display()),
            None => println!("    (no file named)"),
        }
        for d in list {
            let line = d.line.map(|l| format!("line {l}")).unwrap_or_default();
            println!("      [{}] {} {line} — {}", d.id, d.kind.label(), d.message);
            if let Some(text) = &d.text {
                println!("          {text}");
            }
            println!("          {}", doctor::explain(d, report.target, report.version.as_deref()));
            let fixes: Vec<String> = doctor::fixes_for(d).iter().map(|f| format!("mandara setup --fix {}={}", d.id, f.id())).collect();
            if !fixes.is_empty() {
                println!("          fix: {}", fixes.join("  or  "));
            }
        }
    }
}
