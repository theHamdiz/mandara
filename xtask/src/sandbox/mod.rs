//! `cargo xtask sandbox <command>`: a private, headless Mandara desktop for
//! screenshots, demos and end-to-end tests — never the real session.
//!
//! The sandbox is a headless MangoWM (its own Wayland socket), a private
//! session bus, a dead system bus, and the shell running as a demo user on a
//! demo machine (bwrap: own PID namespace, hostname `mandara-demo`, account
//! `demo` / "Mandara Demo", the real /home hidden, the sandbox home mounted
//! at /home/demo, host Flatpak / Snap apps hidden). Demo apps, Downloads,
//! wallpapers, album art and a music track are generated on first use.
//!
//! Safety: every command builds its children's environment from scratch
//! (nothing of the real session's Wayland or D-Bus is ever read or passed
//! on), talks only to sockets inside the sandbox runtime dir, and refuses to
//! act unless that dir carries this sandbox's marker. Processes are signalled
//! only when this tool recorded them and their environment names the sandbox
//! runtime dir.
//!
//! Files live in `target/sandbox/` (home, logs, generated data); the runtime
//! dir is `$MANDARA_SANDBOX_RUNTIME` or `/tmp/mandara-sandbox-<uid>`.

mod demo;
mod matte;
mod paint;
mod player;
// The virtual-pointer driver shared with `mandara-services` (its
// `vpointer` example); compiled here directly so xtask does not depend on
// the whole services crate.
#[path = "../../../crates/mandara-services/src/vpointer.rs"]
mod vpointer;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::headless::{self, DEAD_SYSTEM_BUS, SESSION_VARS};
use crate::proc::{self, Lifetime};
use crate::{Result, flag, opt};

/// Overrides the sandbox runtime dir.
const RUNTIME_ENV: &str = "MANDARA_SANDBOX_RUNTIME";
/// Overrides the sandbox's time zone.
const TZ_ENV: &str = "SANDBOX_TZ";
/// The sandbox's time zone (El-Mandara).
const DEFAULT_TZ: &str = "Africa/Cairo";
/// Marks a runtime dir as this sandbox's; holds the sandbox root's path.
const MARKER: &str = ".mandara-sandbox";
/// The running sandbox's settings, inside the runtime dir.
const STATE: &str = "sandbox.toml";
/// Recorded process ids, one file per name, inside the runtime dir.
const PIDS: &str = "pids";
/// Mode of the runtime dir.
const RUNTIME_MODE: u32 = 0o700;
/// Mode bits compared against `RUNTIME_MODE`.
const MODE_BITS: u32 = 0o777;
/// A Unix socket path must fit `sun_path` (108 bytes with its NUL).
const MAX_SOCKET_PATH: usize = 107;
/// The longest socket name created in the runtime dir.
const LONGEST_SOCKET: &str = "/pulse/native";
/// Output size, scale and refresh rate unless `up` is told otherwise.
const DEFAULT_SIZE: (u32, u32) = (2560, 1440);
const DEFAULT_SCALE: f64 = 1.5;
const REFRESH: u32 = 60;
/// Studio's floating window size and corner radius in the sandbox.
const STUDIO_SIZE: (u32, u32) = (1203, 803);
const STUDIO_RADIUS: u32 = 12;
/// The headless output's name.
pub const OUTPUT: &str = "HEADLESS-1";
/// Processes stopped by `down`, in order.
const STOP_ORDER: [&str; 7] = ["shell", "player", "wireplumber", "pipewire-pulse", "pipewire", "mango", "dbus"];
/// Audio processes stopped by `audio stop`, in order.
const AUDIO_ORDER: [&str; 4] = ["player", "wireplumber", "pipewire-pulse", "pipewire"];
/// How long the sound server's sockets may take to appear.
const AUDIO_TIMEOUT: Duration = Duration::from_secs(5);
/// Settle time for WirePlumber after its start.
const WIREPLUMBER_SETTLE: Duration = Duration::from_millis(1500);
/// Timeout of one `pactl` call.
const PACTL_TIMEOUT: Duration = Duration::from_secs(5);
/// Volume of the demo speakers.
const SPEAKER_VOLUME: &str = "64%";
/// Pause between stopping Studio and opening it again.
const STUDIO_RESTART_PAUSE: Duration = Duration::from_millis(800);
/// How long MangoWM gets to answer an IPC command.
const MANGO_REPLY_TIMEOUT: Duration = Duration::from_secs(1);
/// Variables of the real environment passed through (no session data).
const PASSTHROUGH: [&str; 9] = ["PATH", "LANG", "LC_ALL", "LC_MESSAGES", "TERM", "RUST_LOG", "RUST_BACKTRACE", "WLR_RENDERER", "WLR_RENDER_DRM_DEVICE"];
/// `PATH` when the real environment has none.
const FALLBACK_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Flags that force dry runs of everything that would change the system.
const DRY_RUN_VARS: [&str; 4] = ["MANDARA_RESOURCES_DEMO", "MANDARA_UNINSTALL_DRY_RUN", "MANDARA_GREETER_DRY_RUN", "MANDARA_INSTALL_DRY_RUN"];
/// The demo account inside bwrap.
const DEMO_USER: &str = "demo";
const DEMO_HOME: &str = "/home/demo";
const DEMO_HOST: &str = "mandara-demo";
/// The built-in config presets.
const CONFIG_DEMO: &str = include_str!("config.demo.toml");
const CONFIG_HERO: &str = include_str!("config.hero.toml");

const USAGE: &str = "usage: cargo xtask sandbox <command>
  up [--bin PATH] [--size WxH] [--scale S] [--config demo|hero|FILE] [--studio-size WxH]
  down
  shot OUT.png
  matte OUT.png [--region X0 Y0 X1 Y1] [--margin PX]
  msg ARGS…
  pointer move X Y | click X Y [left|right|middle] | drag X1 Y1 X2 Y2 | flyby X Y MS | scroll X Y STEPS
  studio [--size WxH] SECTION [ARGS…] | studio stop
  shell restart | stop
  audio play | stop
  notify
  env [--fish]";

/// Settings of the running sandbox, recorded by `up`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct State {
    bin: PathBuf,
    width: u32,
    height: u32,
    scale: f64,
}

impl State {
    /// The output layout in logical pixels.
    fn layout(&self) -> (u32, u32) {
        ((f64::from(self.width) / self.scale).round() as u32, (f64::from(self.height) / self.scale).round() as u32)
    }
}

/// The sandbox's paths and safety checks.
#[derive(Clone)]
pub struct Sandbox {
    root: PathBuf,
    runtime: PathBuf,
}

impl Sandbox {
    /// The sandbox root and runtime dir, without touching either.
    fn locate() -> Result<Self> {
        let root = crate::root().join("target/sandbox");
        let uid = rustix::process::getuid().as_raw();
        let runtime = std::env::var_os(RUNTIME_ENV).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/tmp/mandara-sandbox-{uid}")));
        check_runtime_path(&runtime)?;
        Ok(Self { root, runtime })
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    fn share(&self) -> PathBuf {
        self.root.join("demo-share")
    }
    fn etc(&self) -> PathBuf {
        self.root.join("demo-etc")
    }
    fn mango_conf(&self) -> PathBuf {
        self.home().join("config/mango/config.conf")
    }
    fn bus_socket(&self) -> PathBuf {
        self.runtime.join(headless::BUS_SOCKET)
    }

    /// Fails unless the runtime dir exists, is private to this user and
    /// carries this sandbox's marker.
    fn verify(&self) -> Result<()> {
        let refuse = |why: &str| Err(format!("refusing: {} {why} (start the sandbox with `cargo xtask sandbox up`)", self.runtime.display()));
        let Ok(meta) = std::fs::symlink_metadata(&self.runtime) else { return refuse("does not exist") };
        if !meta.is_dir() {
            return refuse("is not a directory");
        }
        if meta.uid() != rustix::process::getuid().as_raw() || meta.mode() & MODE_BITS != RUNTIME_MODE {
            return refuse("is not a private (0700) directory of this user");
        }
        match std::fs::read_to_string(self.runtime.join(MARKER)) {
            Ok(m) if Path::new(m.trim()) == self.root => Ok(()),
            _ => refuse("is not this sandbox's runtime dir"),
        }
    }

    /// True when `pid` runs with this sandbox's runtime dir.
    fn ours(&self, pid: i32) -> bool {
        let want = format!("XDG_RUNTIME_DIR={}", self.runtime.display());
        pid != std::process::id() as i32 && proc::environ(pid).contains(&want)
    }

    fn state(&self) -> Result<State> {
        let text = std::fs::read_to_string(self.runtime.join(STATE)).map_err(|_| "the sandbox is not up (cargo xtask sandbox up)".to_string())?;
        toml::from_str(&text).map_err(|e| format!("{STATE}: {e}"))
    }

    /// The sandbox environment, built from scratch.
    fn env(&self) -> BTreeMap<String, String> {
        let p = |p: PathBuf| p.to_string_lossy().into_owned();
        let rt = self.runtime.display().to_string();
        let home = self.home();
        let mut env = BTreeMap::new();
        for var in PASSTHROUGH {
            if let Ok(v) = std::env::var(var) {
                env.insert(var.to_string(), v);
            }
        }
        env.entry("PATH".to_string()).or_insert_with(|| FALLBACK_PATH.to_string());
        let pairs = [
            (RUNTIME_ENV, rt.clone()),
            ("TZ", std::env::var(TZ_ENV).unwrap_or_else(|_| DEFAULT_TZ.to_string())),
            ("HOME", p(home.clone())),
            ("XDG_CONFIG_HOME", p(home.join("config"))),
            ("XDG_DATA_HOME", p(home.join("data"))),
            ("XDG_CACHE_HOME", p(home.join("cache"))),
            ("XDG_STATE_HOME", p(home.join("state"))),
            ("XDG_RUNTIME_DIR", rt.clone()),
            ("XDG_DATA_DIRS", p(self.share())),
            ("DBUS_SYSTEM_BUS_ADDRESS", DEAD_SYSTEM_BUS.to_string()),
            ("DBUS_SESSION_BUS_ADDRESS", headless::bus_address(&self.runtime)),
            ("XDG_CURRENT_DESKTOP", "mango".to_string()),
            ("XDG_SESSION_TYPE", "wayland".to_string()),
            ("PIPEWIRE_REMOTE", format!("{rt}/pipewire-0")),
            ("PULSE_SERVER", format!("unix:{rt}/pulse/native")),
            ("PULSE_RUNTIME_PATH", format!("{rt}/pulse")),
        ];
        env.extend(pairs.into_iter().map(|(k, v)| (k.to_string(), v)));
        for var in DRY_RUN_VARS {
            env.insert(var.to_string(), "1".to_string());
        }
        if let Some(wayland) = headless::wayland_socket(&self.runtime) {
            env.insert("WAYLAND_DISPLAY".to_string(), wayland.file_name().unwrap_or_default().to_string_lossy().into_owned());
        }
        if let Some(mango) = headless::mango_socket(&self.runtime) {
            env.insert("MANGO_INSTANCE_SIGNATURE".to_string(), p(mango));
        }
        env
    }

    /// `program` with only the sandbox environment.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear().envs(self.env()).current_dir(&self.root);
        cmd
    }

    /// Starts `cmd` detached as `name`, logging to `logs/<name>.log`, and
    /// records its pid.
    fn start(&self, name: &str, cmd: &mut Command) -> Result<i32> {
        let child = proc::spawn(cmd, &self.logs().join(format!("{name}.log")), Lifetime::Detached)?;
        let pid = i32::try_from(child.id()).map_err(|e| e.to_string())?;
        let dir = self.runtime.join(PIDS);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name)).map_err(|e| e.to_string())?;
        writeln!(f, "{pid}").map_err(|e| e.to_string())?;
        Ok(pid)
    }

    /// Stops the process trees recorded as `name` (only processes of this
    /// sandbox), children first.
    fn stop(&self, name: &str) {
        let file = self.runtime.join(PIDS).join(name);
        let pids: Vec<i32> = std::fs::read_to_string(&file).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).collect();
        for pid in pids {
            if !proc::alive(pid) || !self.ours(pid) {
                continue;
            }
            for p in proc::tree(pid).into_iter().filter(|&p| self.ours(p)) {
                proc::terminate(p);
                println!("stopped {name} {p}");
            }
        }
        let _ = std::fs::remove_file(file);
    }

    /// True while a recorded `name` process is alive.
    fn running(&self, name: &str) -> bool {
        std::fs::read_to_string(self.runtime.join(PIDS).join(name))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .any(|pid| proc::alive(pid) && self.ours(pid))
    }

    /// A Wayland connection to the sandbox compositor (never the session's).
    fn wayland(&self) -> Result<wayland_client::Connection> {
        let socket = headless::wayland_socket(&self.runtime).ok_or("the sandbox compositor is not running")?;
        let stream = UnixStream::connect(&socket).map_err(|e| format!("{}: {e}", socket.display()))?;
        wayland_client::Connection::from_socket(stream).map_err(|e| e.to_string())
    }

    /// Sends one IPC command to the sandbox MangoWM and returns its reply.
    fn mango(&self, command: &str) -> Result<String> {
        let socket = headless::mango_socket(&self.runtime).ok_or("the sandbox compositor is not running")?;
        let mut s = UnixStream::connect(&socket).map_err(|e| format!("{}: {e}", socket.display()))?;
        s.set_read_timeout(Some(MANGO_REPLY_TIMEOUT)).map_err(|e| e.to_string())?;
        s.write_all(format!("{command}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).unwrap_or(0);
        Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
    }

    /// Runs `mandara msg ARGS` in the sandbox; returns its exit code.
    fn msg(&self, args: &[String], quiet: bool) -> Result<i32> {
        let state = self.state()?;
        let mut cmd = self.command(&state.bin);
        cmd.arg("msg").args(args);
        if quiet {
            cmd.stdout(std::process::Stdio::null());
        }
        let status = cmd.status().map_err(|e| format!("{}: {e}", state.bin.display()))?;
        Ok(status.code().unwrap_or(crate::EXIT_FAILURE))
    }
}

/// Rejects runtime dirs that could be a real session's.
fn check_runtime_path(runtime: &Path) -> Result<()> {
    let bad = |why: &str| Err(format!("refusing sandbox runtime dir {}: {why}", runtime.display()));
    if !runtime.is_absolute() || runtime.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return bad("must be an absolute, normalized path");
    }
    if runtime.starts_with("/run") || runtime.components().count() < 3 {
        return bad("a real session's runtime dir or a top-level directory");
    }
    if runtime.as_os_str().len() + LONGEST_SOCKET.len() > MAX_SOCKET_PATH {
        return bad("too long for its sockets (108 bytes)");
    }
    Ok(())
}

/// Entry point of `cargo xtask sandbox`.
pub fn sandbox(args: &[String]) -> Result<()> {
    let rest = args.get(1..).unwrap_or_default();
    let sb = Sandbox::locate()?;
    match args.first().map(String::as_str) {
        Some("up") => up(&sb, rest),
        Some("down") => down(&sb),
        Some("shot") => shot(&sb, rest.first().ok_or(USAGE)?).map(|_| ()),
        Some("matte") => matte::matte(&sb, rest),
        Some("msg") => {
            sb.verify()?;
            let code = sb.msg(rest, false)?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Some("pointer") => pointer(&sb, rest),
        Some("studio") => studio(&sb, rest),
        Some("shell") => match rest.first().map(String::as_str) {
            Some("restart") => restart_shell(&sb),
            Some("stop") => {
                sb.verify()?;
                sb.stop("shell");
                Ok(())
            }
            _ => Err(USAGE.into()),
        },
        Some("audio") => match rest.first().map(String::as_str) {
            Some("play") => audio_play(&sb),
            Some("stop") => {
                sb.verify()?;
                AUDIO_ORDER.iter().for_each(|n| sb.stop(n));
                Ok(())
            }
            _ => Err(USAGE.into()),
        },
        Some("notify") => {
            sb.verify()?;
            player::notify(&sb)
        }
        Some("env") => print_env(&sb, flag(rest, "--fish")),
        // Internal: the demo MPRIS player, started by `audio play`.
        Some("player") => {
            sb.verify()?;
            player::run(&sb)
        }
        _ => Err(USAGE.into()),
    }
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    vpointer::parse_size(s).ok_or_else(|| format!("bad size {s:?} (want WxH)"))
}

/// Prepares a fresh runtime dir: absent, empty, or a previous run of this
/// sandbox (then wiped). Anything else is refused.
fn fresh_runtime(sb: &Sandbox) -> Result<()> {
    if sb.runtime.exists() {
        let empty = std::fs::read_dir(&sb.runtime).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !empty {
            sb.verify()?;
            if headless::wayland_socket(&sb.runtime).is_some() && (sb.running("mango") || sb.running("shell")) {
                return Err("the sandbox is already up (cargo xtask sandbox down)".into());
            }
            std::fs::remove_dir_all(&sb.runtime).map_err(|e| e.to_string())?;
        }
    }
    std::fs::create_dir_all(&sb.runtime).map_err(|e| format!("{}: {e}", sb.runtime.display()))?;
    std::fs::set_permissions(&sb.runtime, std::fs::Permissions::from_mode(RUNTIME_MODE)).map_err(|e| e.to_string())?;
    std::fs::write(sb.runtime.join(MARKER), sb.root.display().to_string()).map_err(|e| e.to_string())?;
    sb.verify()
}

fn mango_config(state: &State, studio: (u32, u32)) -> String {
    format!(
        "{}\nanimations=0\nborder_px=0\nshadows=0\nborder_radius={STUDIO_RADIUS}\n{}\n",
        headless::monitor_rule(state.width, state.height, REFRESH, state.scale),
        studio_rule(studio)
    )
}

fn studio_rule((w, h): (u32, u32)) -> String {
    format!("window_rule=is_floating:1,width:{w},height:{h},app_id:^org\\.mandara\\.studio$")
}

fn up(sb: &Sandbox, args: &[String]) -> Result<()> {
    let bin = opt(args, "--bin").map(PathBuf::from).unwrap_or_else(|| crate::root().join("target/release/mandara"));
    let bin = std::fs::canonicalize(&bin).map_err(|e| format!("{}: {e} (build it, or pass --bin PATH)", bin.display()))?;
    let (width, height) = opt(args, "--size").map(|s| parse_size(&s)).transpose()?.unwrap_or(DEFAULT_SIZE);
    let scale = opt(args, "--scale").map(|s| s.parse::<f64>().map_err(|e| format!("--scale: {e}"))).transpose()?.unwrap_or(DEFAULT_SCALE);
    let studio = opt(args, "--studio-size").map(|s| parse_size(&s)).transpose()?.unwrap_or(STUDIO_SIZE);
    let state = State { bin, width, height, scale };

    fresh_runtime(sb)?;
    demo::ensure(sb)?;
    let config = sb.home().join("config/mandara/config.toml");
    match opt(args, "--config").as_deref() {
        Some("demo") => write(&config, CONFIG_DEMO)?,
        Some("hero") => write(&config, CONFIG_HERO)?,
        Some(file) => {
            std::fs::copy(file, &config).map_err(|e| format!("{file}: {e}"))?;
        }
        None if !config.exists() => write(&config, CONFIG_DEMO)?,
        None => {}
    }
    write(&sb.mango_conf(), &mango_config(&state, studio))?;
    write(&sb.runtime.join(STATE), &toml::to_string(&state).map_err(|e| e.to_string())?)?;

    let bus_conf = sb.runtime.join("bus.conf");
    write(&bus_conf, &headless::bus_config(&sb.runtime))?;
    let mut dbus = sb.command("dbus-daemon");
    dbus.arg(format!("--config-file={}", bus_conf.display())).arg("--nofork");
    sb.start("dbus", &mut dbus)?;
    if !proc::wait_until(headless::COMPOSITOR_TIMEOUT, || sb.bus_socket().exists()) {
        return Err(format!("dbus-daemon did not start (see {})", sb.logs().join("dbus.log").display()));
    }

    let mut mango = sb.command("mango");
    mango.env("WLR_BACKENDS", "headless").env("WLR_LIBINPUT_NO_DEVICES", "1").env("WLR_HEADLESS_OUTPUTS", "1").arg("-c").arg(sb.mango_conf());
    sb.start("mango", &mut mango)?;
    let ready = || headless::wayland_socket(&sb.runtime).is_some() && headless::mango_socket(&sb.runtime).is_some();
    if !proc::wait_until(headless::COMPOSITOR_TIMEOUT, ready) {
        return Err(format!("mango did not start (see {})", sb.logs().join("mango.log").display()));
    }
    let env = sb.env();
    println!("up {} {}", env.get("WAYLAND_DISPLAY").map_or("", String::as_str), env.get("MANGO_INSTANCE_SIGNATURE").map_or("", String::as_str));
    start_shell(sb, &state)
}

/// The shell as the demo user (bwrap), recorded as `shell`.
fn start_shell(sb: &Sandbox, state: &State) -> Result<()> {
    if headless::wayland_socket(&sb.runtime).is_none() {
        return Err("the sandbox compositor is not running".into());
    }
    let mut cmd = sb.command("bwrap");
    cmd.args(bwrap_args(sb, &state.bin)).arg(&state.bin).arg("run").env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()));
    let pid = sb.start("shell", &mut cmd)?;
    println!("shell {pid}");
    Ok(())
}

/// `--ro-bind /dev/null /dev/i2c-N` for every I2C bus node under `dev`.
fn i2c_masks(dev: &Path) -> Vec<String> {
    let mut nodes: Vec<String> = std::fs::read_dir(dev).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("i2c-")).collect();
    nodes.sort();
    nodes.into_iter().flat_map(|n| ["--ro-bind".to_string(), "/dev/null".to_string(), dev.join(n).to_string_lossy().into_owned()]).collect()
}

/// bwrap arguments for running `bin` as the demo user on the demo machine.
fn bwrap_args(sb: &Sandbox, bin: &Path) -> Vec<String> {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let etc = sb.etc();
    let mut a: Vec<String> = ["--dev-bind", "/", "/", "--unshare-pid", "--proc", "/proc", "--unshare-uts", "--hostname", DEMO_HOST].map(String::from).to_vec();
    for f in ["passwd", "group", "hostname"] {
        a.extend(["--ro-bind".into(), s(etc.join(f)), format!("/etc/{f}")]);
    }
    a.extend(["--tmpfs", "/var/lib/flatpak", "--tmpfs", "/var/lib/snapd", "--tmpfs", "/home"].map(String::from));
    // The real /home is hidden: keep the sandbox and the binary reachable
    // when they live under it.
    for keep in [sb.root.clone(), bin.to_path_buf()] {
        if keep.starts_with("/home") {
            a.extend(["--bind".into(), s(keep.clone()), s(keep)]);
        }
    }
    a.extend(["--bind".into(), s(sb.home()), DEMO_HOME.into()]);
    // The real monitors stay out of reach: DDC/CI talks to them over
    // /dev/i2c-*, so each node is masked with /dev/null (opening works,
    // the I2C ioctls fail, and the shell sees no DDC monitors). Backlight
    // writes go through logind, which the dead system bus already blocks.
    a.extend(i2c_masks(Path::new("/dev")));
    let vars = [
        ("USER", DEMO_USER.to_string()),
        ("LOGNAME", DEMO_USER.to_string()),
        ("HOME", DEMO_HOME.to_string()),
        ("XDG_CONFIG_HOME", format!("{DEMO_HOME}/config")),
        ("XDG_DATA_HOME", format!("{DEMO_HOME}/data")),
        ("XDG_CACHE_HOME", format!("{DEMO_HOME}/cache")),
        ("XDG_STATE_HOME", format!("{DEMO_HOME}/state")),
    ];
    for (k, v) in vars {
        a.extend(["--setenv".into(), k.into(), v]);
    }
    a
}

fn restart_shell(sb: &Sandbox) -> Result<()> {
    sb.verify()?;
    let state = sb.state()?;
    sb.stop("shell");
    start_shell(sb, &state)
}

fn down(sb: &Sandbox) -> Result<()> {
    sb.verify()?;
    stop_studio(sb)?;
    for name in STOP_ORDER {
        sb.stop(name);
    }
    std::fs::remove_dir_all(&sb.runtime).map_err(|e| format!("{}: {e}", sb.runtime.display()))?;
    println!("down");
    Ok(())
}

/// Screenshots the sandbox output into `out` (grim on the sandbox socket).
fn shot(sb: &Sandbox, out: &str) -> Result<PathBuf> {
    sb.verify()?;
    if headless::wayland_socket(&sb.runtime).is_none() {
        return Err("the sandbox compositor is not running".into());
    }
    let out = std::env::current_dir().map_err(|e| e.to_string())?.join(out);
    let status = sb.command("grim").arg(&out).status().map_err(|e| format!("grim: {e}"))?;
    if !status.success() {
        return Err(format!("grim failed ({status})"));
    }
    Ok(out)
}

fn pointer(sb: &Sandbox, args: &[String]) -> Result<()> {
    sb.verify()?;
    let gesture = vpointer::Gesture::parse(args).map_err(|e| e.to_string())?;
    let options = vpointer::Options { layout: sb.state()?.layout(), ..vpointer::Options::from_env() };
    vpointer::perform(&sb.wayland()?, &gesture, &options).map_err(|e| e.to_string())
}

/// Stops Studio processes of this sandbox (only `<bin> studio …` children).
fn stop_studio(sb: &Sandbox) -> Result<()> {
    let Ok(state) = sb.state() else { return Ok(()) };
    let bin_name = state.bin.file_name().map(|n| n.to_os_string());
    for pid in proc::all_pids() {
        let argv = proc::cmdline(pid);
        let is_studio = argv.get(1).is_some_and(|a| a == "studio") && argv.first().and_then(|a| Path::new(a).file_name().map(|n| n.to_os_string())) == bin_name;
        if is_studio && proc::alive(pid) && sb.ours(pid) {
            proc::terminate(pid);
            println!("stopped studio {pid}");
        }
    }
    Ok(())
}

fn studio(sb: &Sandbox, args: &[String]) -> Result<()> {
    sb.verify()?;
    stop_studio(sb)?;
    let mut rest = args;
    if rest.first().is_some_and(|a| a == "stop") {
        return Ok(());
    }
    if rest.first().is_some_and(|a| a == "--size") {
        let size = parse_size(rest.get(1).ok_or(USAGE)?)?;
        set_studio_size(sb, size)?;
        rest = &rest[2..];
    }
    if rest.is_empty() {
        return Err(USAGE.into());
    }
    std::thread::sleep(STUDIO_RESTART_PAUSE);
    let mut msg = vec!["studio".to_string()];
    msg.extend(rest.iter().cloned());
    match sb.msg(&msg, true)? {
        0 => Ok(()),
        code => Err(format!("mandara msg studio exited with {code}")),
    }
}

/// Floats Studio at `size` logical px in the sandbox MangoWM.
fn set_studio_size(sb: &Sandbox, size: (u32, u32)) -> Result<()> {
    let conf = sb.mango_conf();
    let text = std::fs::read_to_string(&conf).map_err(|e| e.to_string())?;
    let mut lines: Vec<String> = text.lines().filter(|l| !l.starts_with("window_rule=")).map(String::from).collect();
    lines.push(studio_rule(size));
    write(&conf, &(lines.join("\n") + "\n"))?;
    println!("{}", sb.mango("dispatch reload_config")?);
    Ok(())
}

/// Private sound server, demo speakers, and the demo player (which loops
/// the demo track).
fn audio_play(sb: &Sandbox) -> Result<()> {
    sb.verify()?;
    if !sb.bus_socket().exists() {
        return Err("start the sandbox first (cargo xtask sandbox up)".into());
    }
    if sb.running("pipewire") {
        return Err("audio is already playing (cargo xtask sandbox audio stop)".into());
    }
    let pw_socket = sb.runtime.join("pipewire-0");
    sb.start("pipewire", &mut sb.command("pipewire"))?;
    if !proc::wait_until(AUDIO_TIMEOUT, || pw_socket.exists()) {
        return Err(format!("pipewire did not start (see {})", sb.logs().join("pipewire.log").display()));
    }
    let pulse_socket = sb.runtime.join("pulse/native");
    sb.start("pipewire-pulse", &mut sb.command("pipewire-pulse"))?;
    if !proc::wait_until(AUDIO_TIMEOUT, || pulse_socket.exists()) {
        return Err(format!("pipewire-pulse did not start (see {})", sb.logs().join("pipewire-pulse.log").display()));
    }
    sb.start("wireplumber", &mut sb.command("wireplumber"))?;
    std::thread::sleep(WIREPLUMBER_SETTLE);
    pactl(sb, &["load-module", "module-null-sink", "sink_name=speakers", "sink_properties=device.description=Speakers"], true)?;
    pactl(sb, &["set-default-sink", "speakers"], true)?;
    pactl(sb, &["set-sink-volume", "speakers", SPEAKER_VOLUME], true)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut player = sb.command(exe);
    player.args(["sandbox", "player"]);
    sb.start("player", &mut player)?;
    std::thread::sleep(player::STARTUP);
    pactl(sb, &["list", "sinks", "short"], false)?;
    pactl(sb, &["list", "sink-inputs", "short"], false)
}

/// Runs `pactl ARGS` against the sandbox sound server, with a timeout.
fn pactl(sb: &Sandbox, args: &[&str], quiet: bool) -> Result<()> {
    let mut cmd = sb.command("pactl");
    cmd.args(args);
    if quiet {
        cmd.stdout(std::process::Stdio::null());
    }
    let mut child = cmd.spawn().map_err(|e| format!("pactl: {e}"))?;
    let done = proc::wait_until(PACTL_TIMEOUT, || !matches!(child.try_wait(), Ok(None)));
    if !done {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("pactl {} timed out", args.join(" ")));
    }
    Ok(())
}

/// Prints the sandbox environment for manual commands.
fn print_env(sb: &Sandbox, fish: bool) -> Result<()> {
    sb.verify()?;
    let quote = |v: &str| format!("'{}'", v.replace('\'', r"'\''"));
    for var in SESSION_VARS {
        if fish {
            println!("set -e {var}");
        } else {
            println!("unset {var}");
        }
    }
    for (k, v) in sb.env() {
        if fish {
            println!("set -gx {k} {}", quote(&v).replace(r"'\''", r"\'"));
        } else {
            println!("export {k}={}", quote(&v));
        }
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_real_runtime_dirs() {
        assert!(check_runtime_path(Path::new("/run/user/1000")).is_err());
        assert!(check_runtime_path(Path::new("/tmp")).is_err());
        assert!(check_runtime_path(Path::new("relative/dir")).is_err());
        assert!(check_runtime_path(Path::new("/tmp/a/../b")).is_err());
        assert!(check_runtime_path(&PathBuf::from(format!("/tmp/{}", "x".repeat(MAX_SOCKET_PATH)))).is_err());
        assert!(check_runtime_path(Path::new("/tmp/mandara-sandbox-1000")).is_ok());
    }

    #[test]
    fn verify_needs_the_marker() {
        let base = std::env::temp_dir().join(format!("xtask-sandbox-test-{}", std::process::id()));
        let sb = Sandbox { root: base.join("root"), runtime: base.join("rt") };
        assert!(sb.verify().is_err(), "missing dir");
        std::fs::create_dir_all(&sb.runtime).unwrap();
        std::fs::set_permissions(&sb.runtime, std::fs::Permissions::from_mode(RUNTIME_MODE)).unwrap();
        assert!(sb.verify().is_err(), "no marker");
        std::fs::write(sb.runtime.join(MARKER), "/somewhere/else").unwrap();
        assert!(sb.verify().is_err(), "another sandbox's marker");
        std::fs::write(sb.runtime.join(MARKER), sb.root.display().to_string()).unwrap();
        assert!(sb.verify().is_ok());
        std::fs::set_permissions(&sb.runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(sb.verify().is_err(), "not private");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn environment_is_built_from_scratch() {
        let sb = Sandbox { root: PathBuf::from("/tmp/sbx-root"), runtime: PathBuf::from("/tmp/sbx-rt") };
        let env = sb.env();
        assert_eq!(env["XDG_RUNTIME_DIR"], "/tmp/sbx-rt");
        assert_eq!(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/tmp/sbx-rt/bus");
        assert_eq!(env["DBUS_SYSTEM_BUS_ADDRESS"], DEAD_SYSTEM_BUS);
        assert_eq!(env["HOME"], "/tmp/sbx-root/home");
        // No sockets in the runtime dir: no display at all.
        assert!(!env.contains_key("WAYLAND_DISPLAY"));
        assert!(!env.contains_key("MANGO_INSTANCE_SIGNATURE"));
        assert!(!env.contains_key("DISPLAY"));
    }

    #[test]
    fn layout_is_logical() {
        let s = State { bin: PathBuf::new(), width: 2560, height: 1440, scale: 1.5 };
        assert_eq!(s.layout(), (1707, 960));
    }

    #[test]
    fn bwrap_hides_home_and_sets_the_demo_identity() {
        let sb = Sandbox { root: PathBuf::from("/mnt/x/target/sandbox"), runtime: PathBuf::from("/tmp/sbx-rt") };
        let a = bwrap_args(&sb, Path::new("/mnt/x/target/release/mandara")).join(" ");
        assert!(a.contains("--unshare-pid"));
        assert!(a.contains("--hostname mandara-demo"));
        assert!(a.contains("--tmpfs /home --bind /mnt/x/target/sandbox/home /home/demo"));
        assert!(a.contains("--setenv USER demo"));
        let under_home = bwrap_args(&sb, Path::new("/home/u/.local/bin/mandara")).join(" ");
        assert!(under_home.contains("--bind /home/u/.local/bin/mandara /home/u/.local/bin/mandara"));
    }

    #[test]
    fn real_monitors_are_masked_from_ddc() {
        let dev = tempfile::tempdir().unwrap();
        for n in ["i2c-7", "i2c-0", "null", "dri"] {
            std::fs::write(dev.path().join(n), "").unwrap();
        }
        let a = i2c_masks(dev.path()).join(" ");
        let d = dev.path().display();
        assert_eq!(a, format!("--ro-bind /dev/null {d}/i2c-0 --ro-bind /dev/null {d}/i2c-7"));
    }

    #[test]
    fn presets_parse() {
        for preset in [CONFIG_DEMO, CONFIG_HERO] {
            let table: toml::Table = toml::from_str(preset).unwrap();
            assert!(table.contains_key("dock"));
        }
    }
}
