//! Linker options shared with the build-policy regression tests.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// First glibc whose dynamic loader handles RELR (`GLIBC_ABI_DT_RELR`).
const RELR_MIN_GLIBC: (u32, u32) = (2, 36);

/// LLD can separate cold code, but GNU BFD warns and ignores this option.
pub const KEEP_TEXT_SECTION_PREFIX: &str = "-Wl,-z,keep-text-section-prefix";
/// An optional layout optimization must not wait indefinitely on a wrapper.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll only while the tiny compiler probe is alive.
const PROBE_POLL: Duration = Duration::from_millis(10);
/// Compiler diagnostics fit comfortably here; stop a noisy wrapper promptly.
const PROBE_STDERR_LIMIT: u64 = 64 * 1024;

/// GNU ld supports `-z pack-relative-relocs` on i386 and x86-64.
pub fn packed_relocations(os: &str, env: &str, arch: &str, glibc: Option<(u32, u32)>) -> bool {
    os == "linux" && env == "gnu" && matches!(arch, "x86" | "x86_64") && glibc.is_some_and(|version| version >= RELR_MIN_GLIBC)
}

/// Tests the exact target linker selected by Cargo, without running its output.
/// Failure only omits the optional section-layout flag; RELR is independent.
pub fn keep_text_sections(get: impl FnMut(&str) -> Option<OsString>) -> bool {
    LinkerProbe::from_vars(get).is_some_and(|probe| probe.supports(KEEP_TEXT_SECTION_PREFIX))
}

struct LinkerProbe {
    rustc: OsString,
    target: OsString,
    linker: Option<OsString>,
    wrapper: Option<OsString>,
    workspace_wrapper: Option<OsString>,
    flags: Vec<OsString>,
    out_dir: PathBuf,
}

impl LinkerProbe {
    /// Cargo's encoded flags are arguments, not shell text. Empty wrapper
    /// values reset wrapping, and flags may override the configured linker.
    fn from_vars(mut get: impl FnMut(&str) -> Option<OsString>) -> Option<Self> {
        let rustc = get("RUSTC").filter(|value| !value.is_empty())?;
        let target = get("TARGET").filter(|value| !value.is_empty())?;
        let out_dir = get("OUT_DIR").filter(|value| !value.is_empty())?.into();
        let encoded = get("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
        let encoded = encoded.to_str()?;
        let flags = if encoded.is_empty() { Vec::new() } else { encoded.split('\x1f').map(OsString::from).collect() };
        Some(Self {
            rustc, target, out_dir, flags,
            linker: get("RUSTC_LINKER").filter(|value| !value.is_empty()),
            wrapper: get("RUSTC_WRAPPER").filter(|value| !value.is_empty()),
            workspace_wrapper: get("RUSTC_WORKSPACE_WRAPPER").filter(|value| !value.is_empty()),
        })
    }

    /// Mirrors Cargo's wrapper nesting and target/linker selection. The final
    /// arguments force a real link and make ignored linker options fail.
    fn command(&self, source: &Path, binary: &Path, flag: &str) -> Command {
        let mut tools = self.wrapper.iter().chain(self.workspace_wrapper.iter()).chain(std::iter::once(&self.rustc));
        let mut command = Command::new(tools.next().expect("compiler is present"));
        command.args(tools).args(["--crate-name=mandara_link_option_probe", "--crate-type=bin", "--edition=2024", "--target"]).arg(&self.target);
        if let Some(linker) = &self.linker {
            let mut value = OsString::from("linker=");
            value.push(linker);
            command.arg("-C").arg(value);
        }
        command.args(&self.flags).arg(format!("-Clink-arg={flag}")).arg("-Clink-arg=-Wl,--fatal-warnings")
            .arg("--emit=link").arg("-o").arg(binary).arg(source).stdin(Stdio::null()).stdout(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        command
    }

    /// Owns a fresh directory and accepts only a quiet, successful link.
    /// Some linkers exit successfully after warning that a flag was ignored.
    fn supports(&self, flag: &str) -> bool {
        let directory = self.out_dir.join(format!("mandara-link-probe-{}", std::process::id()));
        if std::fs::create_dir(&directory).is_err() { return false; }
        let files = ProbeFiles(directory);
        let source = files.0.join("probe.rs");
        let binary = files.0.join("probe");
        let diagnostics = files.0.join("stderr");
        if std::fs::write(&source, "//! Linker capability probe.\nfn main() {}\n").is_err() { return false; }
        let Ok(stderr) = std::fs::File::create(&diagnostics) else { return false; };
        let Ok(mut child) = self.command(&source, &binary, flag).stderr(stderr).spawn() else { return false; };
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {},
                Err(_) => break None,
            }
            if Instant::now() >= deadline || std::fs::metadata(&diagnostics).is_ok_and(|file| file.len() > PROBE_STDERR_LIMIT) { break None; }
            std::thread::sleep(PROBE_POLL);
        };
        if status.is_none() { stop_probe(&mut child); }
        status.is_some_and(|status| status.success())
            && std::fs::metadata(diagnostics).is_ok_and(|file| file.len() == 0)
            && std::fs::metadata(binary).is_ok_and(|file| file.is_file() && file.len() > 0)
    }
}

struct ProbeFiles(PathBuf);

impl Drop for ProbeFiles {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

/// Stops the probe's isolated process group, including a timed-out linker.
fn stop_probe(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: this unreaped child owns the process group created above.
        unsafe { libc::kill(-pid, libc::SIGKILL); }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_relocations_require_both_linker_and_loader_support() {
        assert!(packed_relocations("linux", "gnu", "x86_64", Some((2, 36))));
        assert!(packed_relocations("linux", "gnu", "x86", Some((2, 44))));
        assert!(!packed_relocations("linux", "gnu", "aarch64", Some((2, 44))));
        assert!(!packed_relocations("linux", "gnu", "x86_64", Some((2, 35))));
        assert!(!packed_relocations("linux", "musl", "x86_64", None));
        assert!(!packed_relocations("freebsd", "", "x86_64", None));
    }

    /// Configured paths containing spaces stay single arguments, including
    /// Cargo's outer wrapper → workspace wrapper → compiler nesting.
    #[test]
    fn probe_preserves_target_linker_wrappers_and_encoded_arguments() {
        let values = std::collections::BTreeMap::from([
            ("RUSTC", "/tool chain/rustc"), ("TARGET", "aarch64-unknown-linux-gnu"), ("OUT_DIR", "/build output"),
            ("RUSTC_LINKER", "/link wrapper/cc"), ("RUSTC_WRAPPER", "/outer cache"), ("RUSTC_WORKSPACE_WRAPPER", "/workspace cache"),
            ("CARGO_ENCODED_RUSTFLAGS", "-C\x1flink-arg=-fuse-ld=bfd\x1f--cfg\x1fprobe=\"with spaces\""),
        ]);
        let probe = LinkerProbe::from_vars(|key| values.get(key).map(|value| OsString::from(*value))).unwrap();
        let command = probe.command(Path::new("/source path/probe.rs"), Path::new("/output path/probe"), KEEP_TEXT_SECTION_PREFIX);
        assert_eq!(command.get_program(), "/outer cache");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(&args[..2], ["/workspace cache", "/tool chain/rustc"]);
        assert!(args.windows(2).any(|pair| pair == ["--target", "aarch64-unknown-linux-gnu"]));
        assert!(args.windows(2).any(|pair| pair == ["-C", "linker=/link wrapper/cc"]));
        assert!(args.windows(4).any(|pair| pair == ["-C", "link-arg=-fuse-ld=bfd", "--cfg", "probe=\"with spaces\""]));
        assert!(args.contains(&std::ffi::OsStr::new("-Clink-arg=-Wl,--fatal-warnings")));
        assert_eq!(args.last().unwrap(), &std::ffi::OsStr::new("/source path/probe.rs"));
        let unwrapped = LinkerProbe::from_vars(|key| {
            if matches!(key, "RUSTC_WRAPPER" | "RUSTC_WORKSPACE_WRAPPER") { Some(OsString::new()) }
            else { values.get(key).map(|value| OsString::from(*value)) }
        }).unwrap();
        assert_eq!(unwrapped.command(Path::new("probe.rs"), Path::new("probe"), KEEP_TEXT_SECTION_PREFIX).get_program(), "/tool chain/rustc");
    }

    /// Missing Cargo context cannot prove that the eventual linker supports it.
    #[test]
    fn missing_probe_context_omits_the_optional_flag() {
        for missing in ["RUSTC", "TARGET", "OUT_DIR"] {
            assert!(LinkerProbe::from_vars(|key| (key != missing).then(|| "value".into())).is_none());
        }
        assert!(!keep_text_sections(|_| None));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            assert!(LinkerProbe::from_vars(|key| Some(if key == "CARGO_ENCODED_RUSTFLAGS" { OsString::from_vec(vec![0xff]) } else { "value".into() })).is_none());
        }
    }

    /// Use the installed compiler's own host tuple, never an assumed GNU triple.
    fn native_probe(out_dir: &Path) -> LinkerProbe {
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let version = Command::new(&rustc).arg("-vV").output().unwrap();
        assert!(version.status.success());
        let version = String::from_utf8(version.stdout).unwrap();
        let target = version.lines().find_map(|line| line.strip_prefix("host: ")).unwrap().into();
        LinkerProbe { rustc, target, linker: None, wrapper: None, workspace_wrapper: None, flags: Vec::new(), out_dir: out_dir.into() }
    }

    /// Both accepted links and compiler failures leave no probe artefacts.
    #[test]
    fn real_link_success_and_failure_are_detected_and_cleaned() {
        let out = tempfile::tempdir().unwrap();
        let mut probe = native_probe(out.path());
        assert!(probe.supports("-Wl,--as-needed"));
        assert!(!probe.supports("-Wl,--mandara-unsupported-linker-option"));
        probe.rustc = out.path().join("missing-rustc").into();
        assert!(!probe.supports(KEEP_TEXT_SECTION_PREFIX));
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }

    /// Explicit LLD selection retains the existing cold-code layout option.
    #[test]
    fn lld_accepts_the_cold_section_option() {
        if Command::new("ld.lld").arg("--version").output().is_err() { return; }
        let out = tempfile::tempdir().unwrap();
        let mut probe = native_probe(out.path());
        probe.linker = Some("cc".into());
        probe.flags = vec!["-Clink-arg=-fuse-ld=lld".into()];
        assert!(probe.supports(KEEP_TEXT_SECTION_PREFIX));
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }

    /// GNU BFD accepts unknown -z options with a warning unless warnings are
    /// fatal. A successful exit by itself would reproduce the CI mistake.
    #[test]
    fn an_ignored_bfd_option_is_not_a_supported_option() {
        if Command::new("ld.bfd").arg("--version").output().is_err() { return; }
        let out = tempfile::tempdir().unwrap();
        let mut probe = native_probe(out.path());
        probe.linker = Some("cc".into());
        probe.flags = vec!["-Clink-arg=-fuse-ld=bfd".into()];
        let source = out.path().join("counterexample.rs");
        let binary = out.path().join("counterexample");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let flag = "-Wl,-z,mandara-capability-probe-unknown";
        let command = probe.command(&source, &binary, flag);
        let output = Command::new(command.get_program())
            .args(command.get_args().filter(|arg| *arg != "-Clink-arg=-Wl,--fatal-warnings"))
            .env("LC_ALL", "C").output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stderr).contains("ignored"));
        assert!(!probe.supports(flag));
    }
}
