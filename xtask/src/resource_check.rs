//! `cargo xtask resource-check <BIN> [config.toml]`: the resource budget of
//! a Mandara binary (docs/releasing.md, "Resource budget").
//!
//! Runs the shell in a throw-away headless MangoWM session — its own runtime
//! dir, session bus and XDG dirs, and a dead system bus so nothing reaches
//! the real NetworkManager / logind — lets it settle, then checks resident
//! memory, anonymous memory, threads, idle CPU and that no OpenGL driver was
//! loaded on a Vulkan machine. Exit status 1 when over budget.
//!
//! Budgets (override through the environment): `RSS_MAX_KIB`,
//! `ANON_MAX_KIB`, `THREADS_MAX`, `CPU_MAX_PCT`. `SETTLE_SECS` (default 180)
//! and `CPU_SECS` (60). Needs `mango` and `dbus-daemon`;
//! `WLR_RENDER_DRM_DEVICE` picks the GPU. `MESA_CACHE` (default
//! `~/.cache/mandara-resource-check`) keeps the shader cache.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use crate::headless::{self, DEAD_SYSTEM_BUS, SESSION_VARS};
use crate::proc::{self, Lifetime};
use crate::{EXIT_FAILURE, Result};

/// Default budgets, overridable through the environment.
const RSS_MAX_KIB: &str = "65536";
const ANON_MAX_KIB: &str = "28672";
const THREADS_MAX: &str = "32";
const CPU_MAX_PCT: &str = "0.5";
/// Seconds the shell settles before measuring.
const SETTLE_SECS: &str = "180";
/// Seconds over which idle CPU is averaged.
const CPU_SECS: &str = "60";
/// The headless output: 4K at 144 Hz, scale 1.5.
const OUTPUT: (u32, u32, u32, f64) = (3840, 2160, 144, 1.5);
/// The compositor's renderer and GPU unless the environment picks others.
const DEFAULT_RENDERER: &str = "gles2";
const DEFAULT_DRM_DEVICE: &str = "/dev/dri/renderD128";
/// Log lines shown when the shell exits early.
const LOG_TAIL: usize = 20;
/// Mode of the private runtime dir.
const RUNTIME_MODE: u32 = 0o700;
/// Percent.
const PERCENT: f64 = 100.0;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

/// Creates a fresh directory `<base>.<unique>`.
fn unique_dir(base: &str) -> Result<PathBuf> {
    let pid = std::process::id();
    for n in 0u32.. {
        let dir = PathBuf::from(format!("{base}.{pid}{n}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    unreachable!("u32 range exhausted")
}

/// The session's children and directories, torn down on drop (also on
/// error paths). Children also die with this process (`Lifetime::Bound`).
struct Session {
    work: PathBuf,
    runtime: PathBuf,
    children: Vec<Child>,
}

impl Drop for Session {
    fn drop(&mut self) {
        for child in self.children.iter_mut().rev() {
            proc::stop_child(child);
        }
        let _ = std::fs::remove_dir_all(&self.work);
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// `resource-check` entry point.
pub fn resource_check(args: &[String]) -> Result<()> {
    if !run(args)? {
        std::process::exit(EXIT_FAILURE);
    }
    Ok(())
}

fn run(args: &[String]) -> Result<bool> {
    let usage = || "usage: cargo xtask resource-check BIN [config.toml]".to_string();
    let bin = std::fs::canonicalize(args.first().ok_or_else(usage)?).map_err(|e| format!("{}: {e}", args[0]))?;
    let config = args.get(1).map(PathBuf::from);
    let rss_max = env_or("RSS_MAX_KIB", RSS_MAX_KIB);
    let anon_max = env_or("ANON_MAX_KIB", ANON_MAX_KIB);
    let threads_max = env_or("THREADS_MAX", THREADS_MAX);
    let cpu_max = env_or("CPU_MAX_PCT", CPU_MAX_PCT);
    let settle: u64 = env_or("SETTLE_SECS", SETTLE_SECS).parse().map_err(|e| format!("SETTLE_SECS: {e}"))?;
    let cpu_secs: u64 = env_or("CPU_SECS", CPU_SECS).parse().map_err(|e| format!("CPU_SECS: {e}"))?;

    // Mesa's shader cache persists between runs: a cold cache makes the
    // driver start extra disk-cache threads (10 instead of 4) that a real
    // session, whose cache is warm, never has. Run twice after a driver
    // update.
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let cache_home = std::env::var("XDG_CACHE_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{home}/.cache"));
    let mesa_cache = env_or("MESA_CACHE", &format!("{cache_home}/mandara-resource-check"));
    std::fs::create_dir_all(&mesa_cache).map_err(|e| format!("{mesa_cache}: {e}"))?;

    // The Wayland socket path must stay under 108 bytes: a short runtime dir.
    let tmp = std::env::var("TMPDIR").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/tmp".into());
    let work = unique_dir(&format!("{tmp}/mandara-rc"))?;
    let runtime = match unique_dir("/tmp/mrc") {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&work);
            return Err(e);
        }
    };
    let mut session = Session { work: work.clone(), runtime: runtime.clone(), children: Vec::new() };
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(RUNTIME_MODE)).map_err(|e| e.to_string())?;
    let h = work.join("home");
    for d in ["config/mandara", "config/mango", "data", "cache", "state"] {
        std::fs::create_dir_all(h.join(d)).map_err(|e| e.to_string())?;
    }
    if let Some(config) = &config {
        std::fs::copy(config, h.join("config/mandara/config.toml")).map_err(|e| format!("{}: {e}", config.display()))?;
    }
    let mango_conf = h.join("config/mango/config.conf");
    let (w, hgt, refresh, scale) = OUTPUT;
    std::fs::write(&mango_conf, headless::monitor_rule(w, hgt, refresh, scale) + "\n").map_err(|e| e.to_string())?;
    let bus_conf = work.join("bus.conf");
    std::fs::write(&bus_conf, headless::bus_config(&runtime)).map_err(|e| e.to_string())?;

    let env: BTreeMap<&str, String> = BTreeMap::from([
        ("MESA_SHADER_CACHE_DIR", mesa_cache.clone()),
        ("HOME", path_str(&h)),
        ("XDG_CONFIG_HOME", path_str(&h.join("config"))),
        ("XDG_DATA_HOME", path_str(&h.join("data"))),
        ("XDG_CACHE_HOME", path_str(&h.join("cache"))),
        ("XDG_STATE_HOME", path_str(&h.join("state"))),
        ("XDG_RUNTIME_DIR", path_str(&runtime)),
        ("DBUS_SYSTEM_BUS_ADDRESS", DEAD_SYSTEM_BUS.to_string()),
        ("DBUS_SESSION_BUS_ADDRESS", headless::bus_address(&runtime)),
        ("XDG_CURRENT_DESKTOP", "mango".to_string()),
    ]);
    let command = |program: &Path| {
        let mut cmd = Command::new(program);
        for var in SESSION_VARS {
            cmd.env_remove(var);
        }
        cmd.envs(&env);
        cmd
    };

    let mut dbus = command(Path::new("dbus-daemon"));
    dbus.arg(format!("--config-file={}", bus_conf.display())).arg("--nofork");
    session.children.push(proc::spawn(&mut dbus, &work.join("dbus.log"), Lifetime::Bound)?);
    let bus_socket = runtime.join(headless::BUS_SOCKET);
    if !proc::wait_until(headless::COMPOSITOR_TIMEOUT, || bus_socket.exists()) {
        return Err(format!("dbus-daemon did not start:\n{}", proc::tail(&work.join("dbus.log"), LOG_TAIL)));
    }

    let mut mango = command(Path::new("mango"));
    mango
        .env("WLR_RENDERER", env_or("WLR_RENDERER", DEFAULT_RENDERER))
        .env("WLR_RENDER_DRM_DEVICE", env_or("WLR_RENDER_DRM_DEVICE", DEFAULT_DRM_DEVICE))
        .env("WLR_BACKENDS", "headless")
        .env("WLR_LIBINPUT_NO_DEVICES", "1")
        .env("WLR_HEADLESS_OUTPUTS", "1")
        .arg("-c")
        .arg(&mango_conf);
    session.children.push(proc::spawn(&mut mango, &work.join("mango.log"), Lifetime::Bound)?);
    let _ = proc::wait_until(headless::COMPOSITOR_TIMEOUT, || headless::wayland_socket(&runtime).is_some());
    let (Some(wayland), Some(mango_sock)) = (headless::wayland_socket(&runtime), headless::mango_socket(&runtime)) else {
        return Err(format!("mango did not start:\n{}", proc::tail(&work.join("mango.log"), LOG_TAIL)));
    };

    let mut shell = command(&bin);
    shell.env("WAYLAND_DISPLAY", wayland.file_name().unwrap_or_default()).env("MANGO_INSTANCE_SIGNATURE", &mango_sock).arg("run");
    let shell_log = work.join("shell.log");
    session.children.push(proc::spawn(&mut shell, &shell_log, Lifetime::Bound)?);
    let shell = session.children.last_mut().expect("just pushed");
    let pid = i32::try_from(shell.id()).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_secs(settle));
    if !matches!(shell.try_wait(), Ok(None)) {
        println!("shell exited:");
        println!("{}", proc::tail(&shell_log, LOG_TAIL));
        return Ok(false);
    }

    let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
    let rss = rollup_kib(&rollup, "Rss");
    let anon = rollup_kib(&rollup, "Anonymous");
    let threads = std::fs::read_dir(format!("/proc/{pid}/task")).map(|d| d.count()).unwrap_or(0);
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap_or_default();
    let gl = maps.lines().filter(|l| l.contains("libgallium") || l.contains("_dri.so")).count();
    let t0 = proc::cpu_ticks(pid).unwrap_or(0);
    std::thread::sleep(Duration::from_secs(cpu_secs));
    let t1 = proc::cpu_ticks(pid).unwrap_or(t0);
    let hz = rustix::param::clock_ticks_per_second() as f64;
    let cpu = format!("{:.2}", t1.saturating_sub(t0) as f64 * PERCENT / hz / cpu_secs.max(1) as f64);

    let mut ok = true;
    ok &= check("rss_kib", &rss, &rss_max);
    ok &= check("anon_kib", &anon, &anon_max);
    ok &= check("threads", &threads.to_string(), &threads_max);
    ok &= check("idle_cpu_pct", &cpu, &cpu_max);
    if maps.contains("libvulkan") && gl > 0 {
        println!("FAIL a Vulkan renderer also loaded the OpenGL driver ({gl} mappings)");
        ok = false;
    } else {
        println!("ok   no OpenGL driver mapped");
    }
    if !ok {
        println!("threads by name:");
        for (count, name) in thread_names(pid) {
            println!("{count:>7} {name}");
        }
    }
    drop(session);
    Ok(ok)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A `Key:  N kB` line of `smaps_rollup` (empty when absent).
fn rollup_kib(rollup: &str, key: &str) -> String {
    rollup
        .lines()
        .find_map(|l| {
            let mut f = l.split_whitespace();
            (f.next()? == format!("{key}:")).then(|| f.next().map(String::from)).flatten()
        })
        .unwrap_or_default()
}

/// Prints one budget line; false when `value` is over `max`.
fn check(name: &str, value: &str, max: &str) -> bool {
    let num = |s: &str| s.parse::<f64>().unwrap_or(0.0);
    if num(value) > num(max) {
        println!("FAIL {name} {value} > {max}");
        false
    } else {
        println!("ok   {name} {value} (budget {max})");
        true
    }
}

/// Thread names with trailing digits stripped, counted, most frequent first.
fn thread_names(pid: i32) -> Vec<(usize, String)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in std::fs::read_dir(format!("/proc/{pid}/task")).into_iter().flatten().flatten() {
        let comm = std::fs::read_to_string(t.path().join("comm")).unwrap_or_default();
        *counts.entry(strip_digits(comm.trim_end_matches('\n')).to_string()).or_default() += 1;
    }
    let mut rows: Vec<(usize, String)> = counts.into_iter().map(|(n, c)| (c, n)).collect();
    rows.sort_by(|a, b| b.cmp(a));
    rows
}

fn strip_digits(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rollup_fields() {
        let rollup = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss:              41200 kB\nPss:  30000 kB\nAnonymous:        19000 kB\n";
        assert_eq!(rollup_kib(rollup, "Rss"), "41200");
        assert_eq!(rollup_kib(rollup, "Anonymous"), "19000");
        assert_eq!(rollup_kib(rollup, "Swap"), "");
    }

    #[test]
    fn budget_comparison_is_numeric() {
        assert!(check("idle_cpu_pct", "0.12", "0.5"));
        assert!(!check("threads", "33", "32"));
        assert!(check("threads", "32", "32"));
    }

    #[test]
    fn thread_names_lose_trailing_digits() {
        assert_eq!(strip_digits("tokio-worker12"), "tokio-worker");
        assert_eq!(strip_digits("gdrv0"), "gdrv");
        assert_eq!(strip_digits("main"), "main");
    }
}
