//! `mandara` — the Mandara Shell binary.
//!
//! * `mandara` — start the shell under a supervisor that restarts it if it
//!   ever dies (crash-loop protected); `mandara run` starts it directly.
//!   The shell is one full-screen `mandara-bar` layer per output (bar,
//!   plugin panels, sidebar) plus exclusion strips and wallpaper layers.
//! * `mandara msg <command…>` — run any shell command over IPC
//!   (`--json` prints the raw `{"ok", "data"|"error"}` response).
//! * `mandara commands [--json]` — the documented command catalog.
//! * `mandara themes` — list built-in palettes.
//! * `mandara studio [section] [--plugin ID [--settings]]` — open Mandara
//!   Studio (its own process, single instance; live-applies settings to the
//!   shell over IPC), optionally on a plugin's detail or settings modal.
//! * `mandara update [check|apply|rollback|plugins]` — self-update by binary
//!   patch (full download as the fallback) and marketplace plugin updates.
//! * `mandara install` / `mandara uninstall` — install this binary (or a
//!   downloaded release, or a source build) with its system packages,
//!   compositor integration and login screen; remove it again.

mod alloc;
mod changelog_cli;
mod ipc;
mod residency;
mod update_cli;

use std::time::Duration;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use gpui::{App, AppContext as _, px, size};
use mandara_config::{ConfigStore, paths};
use mandara_core::LayerSurfaceSpec;
use mandara_plugins::PluginRegistry;
use mandara_theme::catalog;
use mandara_ui::{Fonts, MandaraAssets, ShellView};

use crate::ipc::{Request, WireResponse};

#[derive(Parser)]
#[command(name = "mandara", version, about = "Mandara Shell — a design-first Wayland desktop shell")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the shell directly, without the restart supervisor.
    Run,
    /// Run a shell command, e.g. `mandara msg audio volume set 40`.
    Msg {
        /// Print the raw JSON response (for scripts and agents).
        #[arg(long)]
        json: bool,
        /// Command words.
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List every command with arguments and examples.
    Commands {
        /// Machine-readable catalog.
        #[arg(long)]
        json: bool,
    },
    /// List built-in theme presets.
    Themes,
    /// Open Mandara Studio, optionally on a section (e.g. `themes`).
    Studio {
        section: Option<String>,
        /// Open this plugin's detail modal (`mandara studio plugins --plugin weather`).
        #[arg(long, value_name = "ID")]
        plugin: Option<String>,
        /// With `--plugin`: open its settings modal instead.
        #[arg(long, requires = "plugin")]
        settings: bool,
        /// Open a tab: Plugins (discover, installed, updates, community,
        /// develop — `mandara studio plugins --tab develop`; the older
        /// `marketplace` / `local` still work) or Applications (installed,
        /// defaults, startup, launching — `mandara studio applications --tab
        /// startup`).
        #[arg(long, value_name = "TAB", conflicts_with = "plugin")]
        tab: Option<String>,
        /// With `--tab develop`: scroll to a section (folders, format,
        /// architecture, contract, quick-start, validate).
        #[arg(long, value_name = "SECTION", requires = "tab")]
        scroll: Option<String>,
        /// Demo data instead of the real system (`mandara studio users
        /// --demo`: fictional accounts; `mandara studio date --demo`: a
        /// fictional clock). Nothing on the system changes.
        #[arg(long)]
        demo: bool,
    },
    /// Hook Mandara into your compositor without a running shell: writes
    /// the generated drop-in and adds the single include line to your
    /// config (backup kept). Used by `mandara install`.
    Setup {
        /// Also start Mandara with the compositor (from the drop-in).
        #[arg(long)]
        autostart: bool,
        /// mango, niri or hyprland (default: the running one, else every
        /// compositor that has a config file).
        #[arg(long)]
        compositor: Option<String>,
        /// Only report conflicts (old launcher binds, other bars, notification
        /// or wallpaper daemons); change nothing. Exits 10 when there are any.
        #[arg(long)]
        check: bool,
        /// Retire conflicting lines in your config (commented out with a
        /// `mandara-retired:` marker, every touched file backed up first),
        /// validate with the compositor, and roll back if it objects.
        #[arg(long)]
        fix_conflicts: bool,
        /// Undo a retire: restore a backup folder (or `latest`).
        #[arg(long, value_name = "BACKUP|latest")]
        restore: Option<String>,
    },
    /// Mandara Login, the greetd greeter: `mandara greeter` (started by
    /// greetd), `--demo` (preview on this session), `install` / `uninstall`.
    Greeter(mandara_greeter::cli::GreeterArgs),
    /// Lock the screen (ext-session-lock-v1 + PAM). `--demo` previews the
    /// lock screen without locking anything.
    Lock {
        /// Preview on overlay surfaces: nothing is locked, any password works.
        #[arg(long)]
        demo: bool,
        /// Demo only: close after this many seconds (default 60).
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
    },
    /// What changed in each release, in plain words: this version by
    /// default, `mandara changelog 0.1.1`, `--all`, or `--since 0.1.0` for
    /// everything an update brought.
    Changelog(changelog_cli::ChangelogArgs),
    /// Update Mandara (binary patch first, full download as the fallback),
    /// roll back, or update marketplace plugins: `mandara update apply`.
    Update(update_cli::UpdateArgs),
    /// Install Mandara: this binary into ~/.local/bin (checked against its
    /// release's SHA-256), the system packages it needs (sudo / doas / run0 /
    /// pkexec, asked), compositor integration, Mandara Login and keyring
    /// unlock. `--dry-run` shows everything and changes nothing.
    Install(mandara_install::InstallArgs),
    /// Remove the binary and desktop entry and empty the compositor
    /// drop-ins (your settings are kept).
    Uninstall(mandara_install::UninstallArgs),
}

fn main() -> Result<()> {
    alloc::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,naga=warn,wgpu=warn,wgpu_hal=error,blade=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    mandara_plugins::install_panic_hook();
    // A write past RLIMIT_FSIZE (e.g. a large screencopy buffer under a
    // restrictive ulimit) must fail with EFBIG, not kill the shell.
    // SAFETY: setting a signal disposition to SIG_IGN has no preconditions.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }

    let long = Box::leak(mandara_version::long_version().into_boxed_str()) as &'static str;
    let matches = <Cli as clap::CommandFactory>::command().version(mandara_version::version()).long_version(long).get_matches();
    let cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    match cli.command {
        Some(Cmd::Setup { autostart, compositor, check, fix_conflicts, restore }) => setup(autostart, compositor.as_deref(), check, fix_conflicts, restore.as_deref()),
        Some(Cmd::Msg { json, args }) => {
            use std::io::Write as _;
            let args = absolute_install_paths(args);
            let response = ipc::send(&paths::ipc_socket(), &args)?;
            // Locked writes that stop quietly on a closed pipe (`| head`).
            let mut out = std::io::stdout().lock();
            if json {
                let _ = writeln!(out, "{}", serde_json::to_string(&response)?);
            } else if response.ok {
                let _ = match response.data {
                    Some(serde_json::Value::String(s)) => writeln!(out, "{s}"),
                    Some(serde_json::Value::Null) | None => writeln!(out, "ok"),
                    Some(other) => writeln!(out, "{}", serde_json::to_string_pretty(&other)?),
                };
            } else {
                eprintln!("error: {}", response.error.clone().unwrap_or_default());
            }
            if !response.ok {
                std::process::exit(1);
            }
            Ok(())
        }
        Some(Cmd::Commands { json }) => {
            let registry = build_registry();
            if json {
                println!("{}", serde_json::to_string_pretty(&registry.commands.catalog_json())?);
            } else {
                use std::io::Write as _;
                // Write through a locked handle and stop quietly on a closed
                // pipe (`mandara commands | head`).
                let mut out = std::io::stdout().lock();
                let mut owner = String::new();
                for c in registry.commands.all() {
                    if c.owner != owner {
                        owner = c.owner.clone();
                        if writeln!(out, "\n[{owner}]").is_err() {
                            return Ok(());
                        }
                    }
                    if writeln!(out, "  {:<48} {}", c.usage(), c.summary).is_err() {
                        return Ok(());
                    }
                }
                let _ = writeln!(out, "\nRun with `mandara msg <command>`; add --json for structured output.");
            }
            Ok(())
        }
        Some(Cmd::Themes) => {
            for t in catalog::all() {
                let (d, l) = (t.palette(true), t.palette(false));
                println!(
                    "{:<18} {:<18} dark {} on {}   light {} on {}{}",
                    t.id,
                    t.name,
                    d.primary,
                    d.surface_base,
                    l.primary,
                    l.surface_base,
                    if t.signature { "  ★" } else { "" }
                );
            }
            Ok(())
        }
        Some(Cmd::Studio { section, plugin, settings, tab, scroll, demo }) => {
            if demo {
                mandara_studio::app::set_users_demo();
                mandara_studio::app::set_time_demo();
            }
            run_studio(match (plugin, tab) {
                // Studio's target syntax: `plugins/<id>[/settings]` and
                // `plugins/<tab>[/<section>]`.
                (Some(id), _) => Some(format!("plugins/{id}{}", if settings { "/settings" } else { "" })),
                // `applications --tab startup` → `applications/startup`;
                // without a section the tab is a Plugins tab.
                (None, Some(tab)) => Some(format!("{}/{tab}{}", section.as_deref().filter(|s| !s.eq_ignore_ascii_case("plugins")).unwrap_or("plugins"), scroll.map(|s| format!("/{s}")).unwrap_or_default())),
                (None, None) => section,
            })
        }
        Some(Cmd::Greeter(args)) => mandara_greeter::cli::main(args),
        Some(Cmd::Lock { demo, timeout }) => mandara_ui::lock::run(mandara_ui::lock::LockOptions { demo, timeout: timeout.map(Duration::from_secs) }),
        Some(Cmd::Update(args)) => update_cli::main(args),
        Some(Cmd::Changelog(args)) => changelog_cli::main(args),
        Some(Cmd::Install(args)) => mandara_install::install(args, &mandara_greeter::install_probe::InstallerProbe).map_err(anyhow::Error::msg),
        Some(Cmd::Uninstall(args)) => mandara_install::uninstall(args, &mandara_greeter::install_probe::InstallerProbe).map_err(anyhow::Error::msg),
        Some(Cmd::Run) => run_shell(),
        None => supervise(),
    }
}

/// Runs `mandara run` as a child and restarts it whenever it exits
/// abnormally (panic that escaped, signal, GPU loss…). A clean exit
/// (`mandara msg quit`) ends supervision; too many crashes in a short time
/// stop restarts so a broken build can't spin forever.
fn supervise() -> Result<()> {
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::time::Instant;

    static STOP: AtomicBool = AtomicBool::new(false);
    /// The running shell's pid (0 between runs), for the signal handler.
    static CHILD: AtomicI32 = AtomicI32::new(0);
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
        let pid = CHILD.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill(2) is async-signal-safe; the pid is our own child.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    // SAFETY: the handler only stores to an atomic (async-signal-safe).
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
    }
    let exe = paths::self_exe();
    let mut crashes: Vec<Instant> = Vec::new();
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("run").env("MANDARA_SUPERVISED", "1");
        // SAFETY: prctl in the forked child before exec; no allocation.
        unsafe {
            cmd.pre_exec(|| {
                // The shell dies with its supervisor.
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn().context("starting the shell")?;
        let pid = libc::pid_t::try_from(child.id()).context("shell pid out of range")?;
        CHILD.store(pid, Ordering::SeqCst);
        // A signal that landed before the pid was published.
        if STOP.load(Ordering::SeqCst) {
            // SAFETY: plain kill(2) on our own child's pid.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        // Blocks without polling: the signal handler forwards SIGTERM to
        // the shell, and its exit ends the wait.
        let status = child.wait().context("waiting for the shell")?;
        CHILD.store(0, Ordering::SeqCst);
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        if status.success() {
            return Ok(());
        }
        // `mandara msg restart`: come straight back, not counted as a crash.
        if status.code() == Some(RESTART_CODE) {
            continue;
        }
        let auto_restart = ConfigStore::open_default().map(|s| s.get().general.auto_restart).unwrap_or(true);
        if !auto_restart {
            tracing::warn!("Mandara exited abnormally; auto-restart is off (Studio → General), so it stays stopped");
            return Ok(());
        }
        let why = match (status.code(), status.signal()) {
            (_, Some(sig)) => format!("signal {sig}"),
            (Some(code), _) => format!("exit code {code}"),
            _ => "unknown reason".into(),
        };
        let now = Instant::now();
        crashes.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
        crashes.push(now);
        if crashes.len() > MAX_CRASHES {
            tracing::error!("Mandara crashed {} times in a minute ({why}); not restarting. Run `mandara run` to see the error.", crashes.len());
            anyhow::bail!("shell crash loop");
        }
        let doublings = u32::try_from(crashes.len().saturating_sub(1)).unwrap_or(u32::MAX);
        let backoff = BACKOFF_FIRST.saturating_mul(2u32.saturating_pow(doublings)).min(BACKOFF_MAX);
        tracing::warn!("Mandara exited ({why}); restarting in {} ms", backoff.as_millis());
        std::thread::sleep(backoff);
    }
}

/// Background workers GPUI starts. It sizes its pool by the CPUs the process
/// may run on — 32 idle threads on a 16-core machine, each a stack and a
/// potential malloc arena — while a shell's background work (image decode,
/// IPC replies, file reads) never needs more than a few at once.
const GPUI_WORKERS: usize = 4;

/// The GPUI application, with its worker pool capped at [`GPUI_WORKERS`]:
/// the platform is created while this thread may only run on that many
/// CPUs (GPUI reads the affinity mask to size the pool), then every thread
/// gets the full mask back, so nothing is pinned.
fn application() -> gpui::Application {
    use std::mem::{size_of, zeroed};
    // SAFETY: cpu_set_t is plain data; the libc calls get valid pointers and
    // its exact size, and only change scheduling affinity.
    unsafe {
        let mut full: libc::cpu_set_t = zeroed();
        if libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut full) != 0 {
            return gpui_platform::application();
        }
        let mut capped: libc::cpu_set_t = zeroed();
        let cpus = (0..libc::CPU_SETSIZE as usize).filter(|cpu| libc::CPU_ISSET(*cpu, &full)).take(GPUI_WORKERS);
        let mut count = 0;
        for cpu in cpus {
            libc::CPU_SET(cpu, &mut capped);
            count += 1;
        }
        if count < GPUI_WORKERS || libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &capped) != 0 {
            return gpui_platform::application();
        }
        let app = gpui_platform::application();
        // Threads started meanwhile inherited the capped mask: restore all.
        let tids = std::fs::read_dir("/proc/self/task").into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse::<libc::pid_t>().ok());
        for tid in tids {
            libc::sched_setaffinity(tid, size_of::<libc::cpu_set_t>(), &full);
        }
        app
    }
}

/// Exit code a shell uses to ask its supervisor for an immediate restart.
pub const RESTART_CODE: i32 = 75;
/// `mandara setup --check` exit code when conflicts were found (scripts and
/// `mandara install` test for it).
const EXIT_CONFLICTS: i32 = 10;
/// Crashes are counted over this window…
const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// …and supervision gives up after more than this many in it.
const MAX_CRASHES: usize = 6;
/// Restart delay after the first crash; it doubles with each further one…
const BACKOFF_FIRST: Duration = Duration::from_millis(250);
/// …up to this.
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// Debounce of the config file watcher.
const CONFIG_WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// Output size assumed when neither xdg-output nor GPUI reports one.
const FALLBACK_OUTPUT: (f32, f32) = (1920.0, 1080.0);

/// Every shipped plugin and command.
fn build_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::default();
    mandara_ui::plugins::register_builtin(&mut registry);
    registry
}

fn run_shell() -> Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        anyhow::bail!("Mandara Shell requires a Wayland session (WAYLAND_DISPLAY is not set)");
    }
    // The output query is a Wayland round trip of its own: run it while
    // the config is read and the IPC socket bound.
    let outputs = std::thread::Builder::new().name("mandara-outputs".into()).spawn(mandara_core::query_outputs).context("spawning the output query")?;
    let store = ConfigStore::open_default().context("opening config")?;
    tracing::info!("config: {} (allocator: {})", store.path().display(), alloc::NAME);
    alloc::start_upkeep();
    let watcher = store.watch(CONFIG_WATCH_DEBOUNCE).context("watching config")?;
    let requests = ipc::serve(&paths::ipc_socket())?;
    let outputs = match outputs.join() {
        Ok(Ok(outputs)) => outputs,
        Ok(Err(err)) => {
            tracing::warn!("xdg-output query failed: {err:#}");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!("xdg-output query panicked");
            Vec::new()
        }
    };

    application().with_assets(MandaraAssets).run(move |cx: &mut App| {
        cx.set_global(build_info());
        let cfg = store.snapshot();
        cx.set_global(Fonts::resolve(&cfg.typography, cx));
        cx.set_global(build_registry());
        let (config, _shell) = mandara_ui::init_state(store, cx);
        mandara_ui::hub::init_hub(cx);
        cx.set_global(mandara_ui::overlays::Outputs(outputs.clone()));
        mandara_ui::overlays::init_overlays(cx);
        mandara_ui::lock::init(cx);
        mandara_ui::powerctl::init(cx);
        // Interface style, calendar reminders, shared location, slideshow,
        // the wallpaper generator's opt-in schedule.
        mandara_ui::style::init(cx);
        mandara_ui::calendar::init(cx);
        mandara_ui::location::init(cx);
        mandara_ui::slideshow::init(cx);
        mandara_ui::plugins::wallpaper_generator::init(cx);
        mandara_ui::polkit::init(cx);
        mandara_ui::keyring_prompt::init(cx);
        mandara_ui::keepassxc_prompt::init(cx);
        mandara_ui::bt_pairing::init(cx);
        mandara_ui::plugins::system_admin::init(cx);
        mandara_ui::plugins::compositor::init(cx);
        mandara_ui::workflow::init(cx);
        // Studio is its own process; the shell only launches it.
        cx.set_global(mandara_ui::state::StudioHook(std::rc::Rc::new(|section, _| spawn_studio(section))));

        mandara_ui::icon_theme::set_theme_override(cfg.appearance.icon_theme.clone());
        // Re-resolve fonts / icon theme when appearance settings change.
        let mut typography = cfg.typography.clone();
        cx.observe(&config, move |config, cx| {
            mandara_ui::icon_theme::set_theme_override(config.read(cx).get().appearance.icon_theme.clone());
            let next = config.read(cx).get().typography.clone();
            if next != typography {
                cx.set_global(Fonts::resolve(&next, cx));
                typography = next;
                cx.refresh_windows();
            }
        })
        .detach();

        open_surfaces(&outputs, cx);
        if mandara_ui::backdrop::software_enabled() {
            // Falls back to the GPUI windows if it cannot start.
            let fallback_outputs = outputs.clone();
            mandara_ui::backdrop::init(move |cx| open_gpui_wallpapers(&fallback_outputs, cx), cx);
        }

        // External config edits → live reload.
        let changes = watcher.receiver();
        cx.spawn(async move |cx| {
            let _watcher = watcher;
            while changes.recv().await.is_ok() {
                cx.update(|cx| {
                    let _ = mandara_ui::reload_config_from_disk(cx);
                });
            }
        })
        .detach();

        // IPC: resolve on the foreground thread, await pending service
        // results without blocking the UI.
        cx.spawn(async move |cx| {
            while let Ok(Request { args, reply }) = requests.recv().await {
                let output = cx.update(|cx| {
                    if args.first().map(String::as_str) == Some("commands") {
                        return Ok(mandara_plugins::Output::Value(cx.global::<PluginRegistry>().commands.catalog_json()));
                    }
                    let registry = cx.global::<PluginRegistry>().commands.clone();
                    mandara_ui::dismiss::with_origin(mandara_ui::dismiss::Origin::External, cx, |cx| registry.execute(&args, cx))
                });
                cx.background_executor()
                    .spawn(async move {
                        let response = match output {
                            Ok(output) => match output.resolve().await {
                                Ok(value) => WireResponse::ok(value),
                                Err(err) => WireResponse::err(err),
                            },
                            Err(err) => WireResponse::err(err),
                        };
                        let _ = reply.send(response).await;
                    })
                    .detach();
            }
        })
        .detach();
    });
    let _ = std::fs::remove_file(paths::ipc_socket());
    Ok(())
}

/// Launches `mandara studio [section]` detached from the shell's process
/// group (a running Studio just focuses itself).
fn spawn_studio(section: Option<String>) {
    use std::os::unix::process::CommandExt as _;
    let exe = paths::self_exe();
    let mut cmd = std::process::Command::new(exe);
    // `section` may carry flags after the page (`date --demo`); the page
    // itself can have spaces ("Login screen").
    let (flags, words): (Vec<&str>, Vec<&str>) = section.as_deref().unwrap_or_default().split_whitespace().partition(|w| w.starts_with("--"));
    cmd.arg("studio");
    if !words.is_empty() {
        cmd.arg(words.join(" "));
    }
    cmd.args(flags).stdin(std::process::Stdio::null()).process_group(0);
    match cmd.spawn() {
        // Reap it in the background so it never lingers as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(err) => tracing::error!("launching Mandara Studio: {err}"),
    }
}

fn studio_socket() -> std::path::PathBuf {
    paths::ipc_socket().with_file_name("mandara-studio.sock")
}

fn run_studio(section: Option<String>) -> Result<()> {
    use std::io::{BufRead as _, Write as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        anyhow::bail!("Mandara Studio requires a Wayland session (WAYLAND_DISPLAY is not set)");
    }
    // Single instance: hand the section to a running Studio and exit.
    let sock = studio_socket();
    if let Ok(mut stream) = UnixStream::connect(&sock) {
        writeln!(stream, "{}", section.unwrap_or_default())?;
        return Ok(());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;
    let (show_tx, show_rx) = async_channel::unbounded::<String>();
    std::thread::Builder::new().name("mandara-studio-ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if std::io::BufReader::new(stream).read_line(&mut line).is_ok() && show_tx.send_blocking(line.trim().to_string()).is_err() {
                break;
            }
        }
    })?;

    let store = ConfigStore::open_default().context("opening config")?;
    let watcher = store.watch(CONFIG_WATCH_DEBOUNCE).context("watching config")?;
    let shell_sock = paths::ipc_socket();
    // Whether the shell answers decides who saves the config. Asked off the
    // startup path (a hung shell would otherwise hold the window back for the
    // whole IPC timeout); until it answers Studio saves edits itself, which
    // is harmless (the shell ignores a file identical to its own).
    let shell_alive = Arc::new(AtomicBool::new(false));
    {
        let alive = shell_alive.clone();
        let sock = shell_sock.clone();
        std::thread::Builder::new().name("mandara-studio-ping".into()).spawn(move || {
            alive.store(ipc::send(&sock, &["ping".to_string()]).is_ok_and(|r| r.ok), Ordering::Relaxed);
        })?;
    }

    // Latest-wins mirror: slider drags coalesce into one IPC call per send.
    let (mirror_tx, mirror_rx) = std::sync::mpsc::channel::<String>();
    {
        let alive = shell_alive.clone();
        let shell_sock = shell_sock.clone();
        std::thread::Builder::new().name("mandara-studio-mirror".into()).spawn(move || {
            while let Ok(mut json) = mirror_rx.recv() {
                while let Ok(newer) = mirror_rx.try_recv() {
                    json = newer;
                }
                let ok = ipc::send(&shell_sock, &["config".into(), "apply".into(), json]).is_ok_and(|r| r.ok);
                alive.store(ok, Ordering::Relaxed);
            }
        })?;
    }

    application().with_assets(MandaraAssets).run(move |cx: &mut App| {
        cx.set_global(build_info());
        let cfg = store.snapshot();
        cx.set_global(Fonts::resolve(&cfg.typography, cx));
        cx.set_global(build_registry());
        let (config, _shell) = mandara_ui::init_state(store, cx);
        mandara_ui::icon_theme::set_theme_override(cfg.appearance.icon_theme.clone());
        let mut typography = cfg.typography.clone();
        cx.observe(&config, move |config, cx| {
            mandara_ui::icon_theme::set_theme_override(config.read(cx).get().appearance.icon_theme.clone());
            let next = config.read(cx).get().typography.clone();
            if next != typography {
                cx.set_global(Fonts::resolve(&next, cx));
                typography = next;
                cx.refresh_windows();
            }
        })
        .detach();

        // Every edit goes to the shell; when it's down, Studio saves itself.
        let alive = shell_alive.clone();
        cx.set_global(mandara_ui::state::ConfigMirror(std::rc::Rc::new(move |cfg| match serde_json::to_string(cfg) {
            Ok(json) => mirror_tx.send(json).is_ok() && alive.load(Ordering::Relaxed),
            Err(_) => false,
        })));
        let pid_sock = shell_sock.clone();
        let pid_alive = shell_alive.clone();
        cx.set_global(mandara_studio::ShellLink(Arc::new(move || {
            let response = ipc::send(&pid_sock, &["info".to_string()]).ok().filter(|r| r.ok);
            pid_alive.store(response.is_some(), Ordering::Relaxed);
            response?.data?.get("pid")?.as_u64().map(|p| p as u32)
        })));

        let ipc_sock = shell_sock.clone();
        cx.set_global(mandara_studio::ShellIpc(Arc::new(move |args| match ipc::send(&ipc_sock, &args) {
            Ok(r) if r.ok => Ok(()),
            Ok(r) => Err(r.error.unwrap_or_default()),
            Err(e) => Err(format!("{e:#}")),
        })));
        let query_sock = shell_sock.clone();
        cx.set_global(mandara_studio::ShellQuery(Arc::new(move |args| match ipc::send(&query_sock, &args) {
            Ok(r) if r.ok => Ok(r.data.unwrap_or(serde_json::Value::Null)),
            Ok(r) => Err(r.error.unwrap_or_default()),
            Err(e) => Err(format!("{e:#}")),
        })));

        mandara_studio::run(section.as_deref(), cx);

        let changes = watcher.receiver();
        cx.spawn(async move |cx| {
            let _watcher = watcher;
            while changes.recv().await.is_ok() {
                cx.update(|cx| {
                    let _ = mandara_ui::reload_config_from_disk(cx);
                });
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            while let Ok(section) = show_rx.recv().await {
                cx.update(|cx| mandara_studio::open_at((!section.is_empty()).then_some(section.as_str()), cx));
            }
        })
        .detach();
    });
    let _ = std::fs::remove_file(studio_socket());
    Ok(())
}

/// The GPUI desktop background window for one output (transparent until a
/// wallpaper is chosen, so an existing wallpaper daemon shows through).
fn open_gpui_wallpaper(display: Option<gpui::DisplayId>, (w, h): (f32, f32), monitor: String, cx: &mut App) {
    let background = LayerSurfaceSpec::background(size(px(w), px(h)), display);
    if let Err(err) = cx.open_window(background.window_options(), move |_, cx| cx.new(|cx| mandara_ui::wallpaper::WallpaperView::on_monitor(monitor, cx))) {
        tracing::error!("wallpaper surface: {err:#}");
    }
}

/// GPUI desktop backgrounds on every output (the software renderer's fallback).
fn open_gpui_wallpapers(outputs: &[mandara_core::OutputGeometry], cx: &mut App) {
    let displays = cx.displays();
    for i in 0..displays.len().max(1) {
        let display = displays.get(i).map(|d| d.id());
        let geo = outputs.get(i);
        let (w, h) = geo
            .map(|g| (g.width as f32, g.height as f32))
            .or_else(|| displays.get(i).map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))))
            .unwrap_or(FALLBACK_OUTPUT);
        open_gpui_wallpaper(display, (w, h), geo.map(|g| g.name.clone()).unwrap_or_default(), cx);
    }
}

/// Opens the shell surface + exclusion strip on every output.
fn open_surfaces(outputs: &[mandara_core::OutputGeometry], cx: &mut App) {
    let displays = cx.displays();
    let count = displays.len().max(1);
    for i in 0..count {
        let display = displays.get(i).map(|d| d.id());
        let geo = outputs.get(i);
        let (w, h) = geo
            .map(|g| (g.width as f32, g.height as f32))
            .or_else(|| displays.get(i).map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))))
            .unwrap_or(FALLBACK_OUTPUT);
        let monitor = geo.map(|g| g.name.clone()).unwrap_or_default();

        // Invisible strips reserving the bar's / sidebar's space, created
        // on demand (only the edges that currently reserve).
        mandara_ui::shell::open_exclusion_strips(display, (w, h), monitor.clone(), cx);

        // Desktop background: drawn in software on its own wl_shm surfaces
        // (`mandara_ui::backdrop`, started after this loop) unless
        // MANDARA_WALLPAPER_RENDERER=gpui asks for the GPUI window.
        if !mandara_ui::backdrop::software_enabled() {
            open_gpui_wallpaper(display, (w, h), monitor.clone(), cx);
        }

        let mut main = LayerSurfaceSpec::shell(size(px(w), px(h)), display);
        // "Show over fullscreen": the shell surface joins the overlay layer.
        if mandara_ui::state::config(cx).read(cx).get().bar.over_fullscreen {
            main.layer = gpui::layer_shell::Layer::Overlay;
        }
        let name = monitor.clone();
        match cx.open_window(main.window_options(), move |window, cx| cx.new(|cx| ShellView::new(name, (w, h), window, cx))) {
            Ok(_) => tracing::info!("mandara shell surface on {monitor} ({w}×{h} logical)"),
            Err(err) => tracing::error!("shell surface: {err:#}"),
        }
    }
}

/// `mandara setup`: compositor integration from the command line — the
/// drop-in, the single include line, conflict detection / retirement, all
/// validated by the compositor with automatic rollback.
fn setup(autostart: bool, compositor: Option<&str>, check: bool, fix_conflicts: bool, restore: Option<&str>) -> Result<()> {
    use mandara_config::compositor::{ManagedConfig, Target, emit, existing_chords, install_include, user_autostarts, write_generated};
    use mandara_config::conflicts::{self, Validation};
    if let Some(which) = restore {
        let backup = if which == "latest" { conflicts::backups().into_iter().next().ok_or_else(|| anyhow::anyhow!("no retire backups in {}", conflicts::backups_root().display()))? } else { std::path::PathBuf::from(which) };
        for f in conflicts::restore(&backup)? {
            println!("restored {}", f.display());
        }
    }
    let parse = |name: &str| match name {
        "mango" | "mangowm" => Some(Target::Mango),
        "niri" => Some(Target::Niri),
        "hyprland" | "hypr" => Some(Target::Hyprland),
        _ => None,
    };
    let targets: Vec<Target> = match compositor {
        Some(name) => vec![parse(name).ok_or_else(|| anyhow::anyhow!("unknown compositor {name:?} (mango, niri or hyprland)"))?],
        None => match mandara_compositor::detect() {
            Some(mandara_compositor::CompositorKind::MangoWM) => vec![Target::Mango],
            Some(mandara_compositor::CompositorKind::Niri) => vec![Target::Niri],
            Some(mandara_compositor::CompositorKind::Hyprland) => vec![Target::Hyprland],
            None => [Target::Mango, Target::Niri, Target::Hyprland].into_iter().filter(|t| t.user_config().exists()).collect(),
        },
    };
    if targets.is_empty() {
        println!("No MangoWM, Niri or Hyprland config found — start your compositor once, then run `mandara setup` again.");
        return Ok(());
    }
    let mut store = mandara_config::ConfigStore::open_default()?;
    if autostart && !check && !store.get().compositor.autostart {
        store.update(|c| c.compositor.autostart = true);
        store.flush()?;
    }
    let mut cfg = store.snapshot();
    let write_dropin = |target: Target, compositor: &mandara_config::compositor::CompositorConfig| -> Result<(std::path::PathBuf, mandara_config::compositor::Emitted, bool)> {
        let starts = user_autostarts(target);
        let managed = ManagedConfig { config: compositor, borders: None, existing_chords: existing_chords(target), mandara_bin: mandara_compositor::backend::mandara_binary(), version: mandara_config::compositor::installed_version(target), user_autostarts: starts };
        let emitted = emit(target, &managed);
        Ok((write_generated(target, &emitted)?, emitted, starts))
    };
    let mut total_conflicts = 0;
    for target in targets {
        let has_config = target.user_config().exists();
        if !check {
            let (path, emitted, starts) = write_dropin(target, &cfg.effective_compositor())?;
            if !has_config {
                println!("{target:?}: drop-in {} · {} doesn't exist yet — add this line to your config: {}", path.display(), target.user_config().display(), target.include_line());
                continue;
            }
            let before = std::fs::read_to_string(target.user_config())?;
            let added = install_include(target)?;
            if let Validation::Invalid { tool, report } = conflicts::validate(target) {
                if added {
                    // Our include made it invalid: undo exactly that.
                    mandara_config::write_atomic(&target.user_config(), before.as_bytes())?;
                }
                println!("{target:?}: `{tool}` rejects the config{} — fix it, then run `mandara setup` again:\n{report}", if added { " with Mandara's include (include removed)" } else { "" });
                continue;
            }
            println!("{target:?}: drop-in {} · include {} {}", path.display(), if added { "added to" } else { "already in" }, target.user_config().display());
            if cfg.compositor.autostart {
                println!("  autostart: {}", if starts { "your config already starts Mandara" } else { "on (from the drop-in)" });
            }
            for note in emitted.skipped {
                println!("  note: {note}");
            }
        }
        if !has_config {
            continue;
        }
        let found = conflicts::scan(target, &cfg.compositor);
        let fixable: Vec<conflicts::Conflict> = found.iter().filter(|c| c.is_conflict()).cloned().collect();
        total_conflicts += fixable.len();
        for c in &found {
            println!("  {} {}:{} — {}", if c.is_conflict() { "conflict:" } else { "note:" }, c.file.display(), c.lines[0] + 1, c.summary());
            println!("      {}", c.text);
        }
        if fix_conflicts && !check && !fixable.is_empty() {
            let backup = conflicts::retire(target, &fixable)?;
            match conflicts::validate(target) {
                Validation::Invalid { tool, report } => {
                    conflicts::restore(&backup)?;
                    println!("  `{tool}` rejected the result, so nothing was changed:\n{report}");
                }
                v => {
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
                    write_dropin(target, &cfg.effective_compositor())?;
                    total_conflicts -= fixable.len();
                    println!(
                        "  retired {} line(s){} · backup {} · undo: mandara setup --restore {}",
                        fixable.len(),
                        match v { Validation::Valid { tool } => format!(", validated by {tool}"), _ => String::new() },
                        backup.display(),
                        backup.display()
                    );
                }
            }
        } else if !fixable.is_empty() && !check {
            println!("  run `mandara setup --fix-conflicts` to retire them (backed up, validated, reversible)");
        }
        if !check && std::env::var_os("MANDARA_NO_IPC").is_none() {
            // Ask a running shell to reload the compositor with the new files.
            let _ = ipc::send(&paths::ipc_socket(), &["compositor".to_string(), "reload".to_string()]);
        }
    }
    if check && total_conflicts > 0 {
        std::process::exit(EXIT_CONFLICTS);
    }
    Ok(())
}

/// This binary's identity for the UI (About, `info`).
fn build_info() -> mandara_core::BuildInfo {
    mandara_core::BuildInfo {
        version: mandara_version::version().to_string(),
        long: mandara_version::long_version(),
        components: mandara_version::COMPONENTS.iter().map(|c| (c.name.to_string(), c.version.to_string())).collect(),
    }
}

/// `mandara msg apps install ./x.AppImage`, `mandara msg plugins vet ./my-plugin`:
/// the shell resolves paths against its own folder, so relative ones are
/// made absolute here.
fn absolute_install_paths(mut args: Vec<String>) -> Vec<String> {
    if args.len() > 2 && matches!((args[0].as_str(), args[1].as_str()), ("apps", "install") | ("plugins", "vet")) {
        for a in args.iter_mut().skip(2).filter(|a| !a.starts_with("--")) {
            if let Ok(abs) = std::path::absolute(&*a) {
                *a = abs.to_string_lossy().into_owned();
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{Cli, Cmd};

    #[test]
    fn install_and_uninstall_parse() {
        let cli = Cli::try_parse_from(["mandara", "install", "--dry-run", "--no-greeter", "--compositor", "niri"]).unwrap();
        let Some(Cmd::Install(args)) = cli.command else { panic!("not install") };
        assert!(args.dry_run && args.no_greeter);
        assert_eq!(args.compositor.as_deref(), Some("niri"));
        let cli = Cli::try_parse_from(["mandara", "uninstall", "--dry-run"]).unwrap();
        assert!(matches!(cli.command, Some(Cmd::Uninstall(a)) if a.dry_run));
        // `mandara --version` stays the top-level version flag.
        assert!(Cli::try_parse_from(["mandara", "install", "--version", "0.2.0"]).is_ok());
    }

    /// The installer and the updater agree on which release build this is.
    #[test]
    fn installer_and_updater_name_the_same_target() {
        assert_eq!(mandara_install::platform::compiled_target(), mandara_services::self_update::running_target());
    }
}
