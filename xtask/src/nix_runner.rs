//! Physical-memory isolation for Nix builds on hosted CI runners.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Result;

/// Reclaim build pages before the hard limit leaves the agent short of RAM.
const MEMORY_HIGH: &str = "75%";
/// Keep the daemon and every builder below the runner's physical capacity.
const MEMORY_MAX: &str = "85%";
/// Matches the adaptive swap action's largest file.
const SWAP_MAX: &str = "32G";
/// The agent, evaluator and operating system need memory outside the builders.
const MIN_HEADROOM: u64 = 2 * 1024 * 1024 * 1024;
/// Bound journal output even when a runner has unrelated kernel messages.
const JOURNAL_LINES: &str = "1000";
/// Keep the most recent relevant events readable in CI logs.
const OOM_LINES: usize = 30;

trait Host {
    fn output(&self, program: &str, args: &[&str]) -> Result<String>;
    fn read(&self, path: &Path) -> Result<String>;
    fn emit(&self, text: &str);
}

struct System;

impl Host for System {
    fn output(&self, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .stderr(Stdio::inherit())
            .output()
            .map_err(|error| format!("{program}: {error}"))?;
        if !output.status.success() {
            return Err(format!("{program} {args:?}: {}", output.status));
        }
        String::from_utf8(output.stdout).map_err(|error| format!("{program}: {error}"))
    }

    fn read(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn emit(&self, text: &str) {
        println!("{text}");
    }
}

pub fn nix_runner(args: &[String]) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err("nix-runner is only available inside GitHub Actions".into());
    }
    match args.first().map(String::as_str) {
        Some("prepare") if args.len() == 1 => prepare(&System),
        Some("report") if args.len() == 1 => report(&System),
        _ => Err("usage: cargo xtask nix-runner <prepare | report>".into()),
    }
}

fn group_root(group: &str) -> Result<PathBuf> {
    let group = Path::new(group.trim());
    if !group.is_absolute()
        || group == Path::new("/")
        || group
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("Nix has no dedicated cgroup".into());
    }
    Ok(Path::new("/sys/fs/cgroup")
        .join(group.strip_prefix("/").map_err(|error| error.to_string())?))
}

fn mem_total(text: &str) -> Result<u64> {
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("MemTotal:") {
            continue;
        }
        let kib = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or("invalid MemTotal")?;
        if fields.next() != Some("kB") || fields.next().is_some() {
            return Err("invalid MemTotal unit".into());
        }
        return kib
            .checked_mul(1024)
            .ok_or_else(|| "MemTotal overflow".into());
    }
    Err("missing MemTotal".into())
}

fn check_boundary(high: &str, limit: &str, total: u64) -> Result<()> {
    let high = high
        .trim()
        .parse::<u64>()
        .map_err(|_| "Nix memory.high is unlimited or invalid")?;
    let limit = limit
        .trim()
        .parse::<u64>()
        .map_err(|_| "Nix memory.max is unlimited or invalid")?;
    if high == 0
        || high >= limit
        || total
            .checked_sub(limit)
            .is_none_or(|free| free < MIN_HEADROOM)
    {
        return Err(format!(
            "Nix memory boundary leaves insufficient headroom: high={high} max={limit} total={total}"
        ));
    }
    Ok(())
}

fn prepare(host: &impl Host) -> Result<()> {
    host.read(Path::new("/sys/fs/cgroup/cgroup.controllers"))?;
    host.output(
        "sudo",
        &[
            "--non-interactive",
            "systemctl",
            "start",
            "nix-daemon.service",
        ],
    )?;
    // The daemon owns the builders; a limit on the nix client misses them.
    host.output(
        "sudo",
        &[
            "--non-interactive",
            "systemctl",
            "set-property",
            "--runtime",
            "nix-daemon.service",
            "MemoryAccounting=yes",
            &format!("MemoryHigh={MEMORY_HIGH}"),
            &format!("MemoryMax={MEMORY_MAX}"),
            &format!("MemorySwapMax={SWAP_MAX}"),
        ],
    )?;
    let group = host.output(
        "systemctl",
        &[
            "show",
            "nix-daemon.service",
            "--property=ControlGroup",
            "--value",
        ],
    )?;
    let root = group_root(&group)?;
    let high = host.read(&root.join("memory.high"))?;
    let limit = host.read(&root.join("memory.max"))?;
    check_boundary(
        &high,
        &limit,
        mem_total(&host.read(Path::new("/proc/meminfo"))?)?,
    )?;
    host.emit(&host.output(
        "systemctl",
        &[
            "show",
            "nix-daemon.service",
            "--property=ControlGroup,MemoryHigh,MemoryMax,MemorySwapMax",
        ],
    )?);
    host.emit(&host.output("free", &["-h"])?);
    Ok(())
}

fn report(host: &impl Host) -> Result<()> {
    host.emit(&host.output(
        "systemctl",
        &[
            "show",
            "nix-daemon.service",
            "--property=Result,MemoryPeak,MemorySwapPeak,MemoryHigh,MemoryMax",
        ],
    )?);
    let group = host.output(
        "systemctl",
        &[
            "show",
            "nix-daemon.service",
            "--property=ControlGroup",
            "--value",
        ],
    )?;
    if !group.trim().is_empty() {
        let root = group_root(&group)?;
        for file in ["memory.events", "memory.pressure"] {
            host.emit(file);
            host.emit(&host.read(&root.join(file))?);
        }
    }
    host.emit(&host.output("free", &["-h"])?);
    host.emit(&host.output("df", &["-h", "/", "/nix"])?);
    let journal = host.output(
        "sudo",
        &[
            "--non-interactive",
            "journalctl",
            "-k",
            "--no-pager",
            "--output=short-monotonic",
            "--lines",
            JOURNAL_LINES,
        ],
    )?;
    for line in journal
        .lines()
        .rev()
        .filter(|line| {
            line.contains("oom")
                || line.contains("Out of memory")
                || line.contains("Killed process")
        })
        .take(OOM_LINES)
    {
        host.emit(line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fixture {
        high: &'static str,
        limit: &'static str,
        applies: bool,
        calls: RefCell<Vec<String>>,
        journal: String,
        emitted: RefCell<Vec<String>>,
    }

    impl Host for Fixture {
        fn output(&self, program: &str, args: &[&str]) -> Result<String> {
            self.calls
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            if args.contains(&"set-property") && !self.applies {
                return Err("set-property denied".into());
            }
            if args.contains(&"journalctl") {
                return Ok(self.journal.clone());
            }
            Ok(if args.contains(&"--property=ControlGroup") {
                "/system.slice/nix-daemon.service\n".into()
            } else {
                String::new()
            })
        }

        fn read(&self, path: &Path) -> Result<String> {
            self.calls
                .borrow_mut()
                .push(format!("read {}", path.display()));
            match path.to_str().unwrap() {
                "/sys/fs/cgroup/cgroup.controllers" => Ok("memory\n".into()),
                "/proc/meminfo" => Ok("MemTotal: 16777216 kB\n".into()),
                "/sys/fs/cgroup/system.slice/nix-daemon.service/memory.high" => {
                    Ok(self.high.into())
                }
                "/sys/fs/cgroup/system.slice/nix-daemon.service/memory.max" => {
                    Ok(self.limit.into())
                }
                "/sys/fs/cgroup/system.slice/nix-daemon.service/memory.events" => {
                    Ok("oom_kill 0\n".into())
                }
                "/sys/fs/cgroup/system.slice/nix-daemon.service/memory.pressure" => {
                    Ok("some avg10=0.00\n".into())
                }
                _ => Err(format!("unexpected read: {}", path.display())),
            }
        }

        fn emit(&self, text: &str) {
            self.emitted.borrow_mut().push(text.into());
        }
    }

    #[test]
    fn setup_checks_the_effective_kernel_boundary_after_applying_daemon_limits() {
        for (high, limit, applies, accepted) in [
            ("12884901888", "14602888806", true, true),
            ("max", "14602888806", true, false),
            ("12884901888", "max", true, false),
            ("12884901888", "17179869184", true, false),
            ("14602888806", "12884901888", true, false),
            ("12884901888", "14602888806", false, false),
        ] {
            let host = Fixture {
                high,
                limit,
                applies,
                calls: RefCell::default(),
                journal: String::new(),
                emitted: RefCell::default(),
            };
            assert_eq!(
                prepare(&host).is_ok(),
                accepted,
                "high={high} max={limit} applies={applies}"
            );
            let calls = host.calls.borrow();
            assert_eq!(
                calls[1],
                "sudo --non-interactive systemctl start nix-daemon.service"
            );
            assert_eq!(
                calls[2],
                "sudo --non-interactive systemctl set-property --runtime nix-daemon.service MemoryAccounting=yes MemoryHigh=75% MemoryMax=85% MemorySwapMax=32G"
            );
            if !applies {
                assert_eq!(calls.len(), 3);
            }
        }
    }

    #[test]
    fn procfs_numbers_require_bytes_and_checked_arithmetic() {
        assert_eq!(
            mem_total("MemFree: 1 kB\nMemTotal: 16777216 kB\n").unwrap(),
            16 * 1024 * 1024 * 1024
        );
        for text in [
            "",
            "MemTotal: max kB",
            "MemTotal: 16 GB",
            "MemTotal: 18446744073709551615 kB",
        ] {
            assert!(mem_total(text).is_err(), "{text}");
        }
        assert!(check_boundary("1", "18446744073709551615", 1).is_err());
        assert!(check_boundary("0", "2", MIN_HEADROOM + 2).is_err());
    }

    #[test]
    fn a_boundary_requires_a_dedicated_group() {
        for group in ["", "/", "nix-daemon.service", "/../nix-daemon.service"] {
            assert!(group_root(group).is_err(), "{group}");
        }
        assert_eq!(
            group_root("/system.slice/nix-daemon.service\n").unwrap(),
            Path::new("/sys/fs/cgroup/system.slice/nix-daemon.service")
        );
    }

    #[test]
    fn reports_keep_a_bounded_oom_tail_and_accept_an_empty_journal() {
        let mut host = Fixture {
            high: "",
            limit: "",
            applies: true,
            calls: RefCell::default(),
            journal: String::new(),
            emitted: RefCell::default(),
        };
        report(&host).unwrap();
        assert!(
            host.emitted
                .borrow()
                .iter()
                .any(|line| line == "oom_kill 0\n")
        );
        host.emitted.borrow_mut().clear();
        for i in 0..OOM_LINES + 1 {
            use std::fmt::Write as _;
            writeln!(host.journal, "unrelated kernel event\nKilled process {i}").unwrap();
        }
        report(&host).unwrap();
        let emitted = host.emitted.borrow();
        let mut events = emitted
            .iter()
            .filter(|line| line.starts_with("Killed process"));
        assert_eq!(
            events.next().unwrap(),
            &format!("Killed process {OOM_LINES}")
        );
        assert_eq!(events.count(), OOM_LINES - 1);
        assert!(
            !emitted
                .iter()
                .any(|line| line == "Killed process 0" || line.contains("unrelated kernel event"))
        );
    }
}
