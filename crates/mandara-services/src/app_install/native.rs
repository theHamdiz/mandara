//! Native packages (`.deb`, `.rpm`, `.pkg.tar.zst`, `.apk`, `.xbps`):
//! installed only by the system's own package manager, through `pkexec`,
//! after showing that manager's own simulation (what it pulls in, what
//! conflicts). A package for another distribution family is refused with
//! an explanation: Mandara never converts packages or uses containers.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::Serialize;

use super::archive;
use super::detect::{Arch, Format};
use crate::app_uninstall::{CmdOutput, Runner, Source, system_manager};

/// Package metadata files read from packages are capped at this size.
const MAX_CONTROL_BYTES: u64 = 1 << 20;
/// Simulations are read-only and quick.
pub const SIMULATE_TIMEOUT: Duration = Duration::from_secs(120);
const AR_MAGIC: &[u8] = b"!<arch>\n";
const AR_HEADER: usize = 60;
/// Conflict / error lines shown from a simulation.
const MAX_PROBLEM_LINES: usize = 6;

/// Name, version and summary of a package.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PackageMeta {
    pub name: String,
    pub version: String,
    pub summary: String,
    /// `None` for architecture-independent packages.
    pub arch: Option<Arch>,
    /// The long description (Debian's continuation lines), plain text.
    pub description: String,
    /// Maintainer / packager, without the e-mail address.
    pub maintainer: String,
    /// Installed size in bytes, when the package says.
    pub installed_size: Option<u64>,
    /// Declared dependencies (names, version constraints dropped).
    pub depends: Vec<String>,
}

/// Every `key: value` / `key = value` line's value.
fn fields(text: &str, key: &str, sep: char) -> Vec<String> {
    text.lines().filter_map(|l| l.split_once(sep).filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())).filter(|v| !v.is_empty()).collect()
}

/// `Jane Doe <jane@x.org>` → `Jane Doe`.
fn person(raw: &str) -> String {
    raw.split('<').next().unwrap_or_default().trim().to_string()
}

/// `libc6 (>= 2.34), libgtk-3-0 | libgtk-4-1` → `libc6`, `libgtk-3-0`.
fn dependency_names(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|d| d.split('|').next())
        .map(|d| d.split(['(', '<', '>', '=', ':']).next().unwrap_or_default().trim().to_string())
        .filter(|d| !d.is_empty())
        .collect()
}

/// Debian's `Description:` continuation lines (` .` is a blank line).
fn debian_long_description(control: &str) -> String {
    let mut lines = control.lines().skip_while(|l| !l.starts_with("Description:")).skip(1);
    let mut out: Vec<String> = Vec::new();
    for l in lines.by_ref() {
        if !l.starts_with(' ') && !l.starts_with('\t') {
            break;
        }
        let t = l.trim();
        out.push(if t == "." { String::new() } else { t.to_string() });
    }
    // Paragraphs fold their lines; blank lines separate them.
    out.split(String::is_empty).map(|p| p.join(" ")).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n")
}

/// `key: value` / `key = value` lines.
fn field(text: &str, key: &str, sep: char) -> String {
    text.lines().find_map(|l| l.split_once(sep).filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())).unwrap_or_default()
}

/// One member of an ar archive (`.deb`).
fn ar_member(path: &Path, prefix: &str) -> Option<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let mut at = AR_MAGIC.len();
    if !bytes.starts_with(AR_MAGIC) {
        return None;
    }
    while at + AR_HEADER <= bytes.len() {
        let header = &bytes[at..at + AR_HEADER];
        let name = String::from_utf8_lossy(&header[..16]).trim().trim_end_matches('/').to_string();
        let size: usize = String::from_utf8_lossy(&header[48..58]).trim().parse().ok()?;
        let start = at + AR_HEADER;
        let end = start.checked_add(size)?;
        if end > bytes.len() {
            return None;
        }
        if name.starts_with(prefix) {
            return Some((name, bytes[start..end].to_vec()));
        }
        // Members are 2-byte aligned.
        at = end + (size % 2);
    }
    None
}

/// Reads a package's metadata (from its control data when Mandara can
/// read it, else from the file name).
pub fn meta(path: &Path, format: Format) -> PackageMeta {
    let from_name = from_file_name(path, format);
    let read = match format {
        Format::Deb => ar_member(path, "control.tar").and_then(|(name, data)| {
            let reader: Box<dyn std::io::Read + Send> = if name.ends_with(".gz") {
                Box::new(flate2::read::GzDecoder::new(std::io::Cursor::new(data)))
            } else if name.ends_with(".zst") {
                Box::new(archive::ZstdFrames::new(std::io::Cursor::new(data)))
            } else if name.ends_with(".xz") {
                let mut out = Vec::new();
                lzma_rs::xz_decompress(&mut std::io::Cursor::new(data), &mut out).ok()?;
                Box::new(std::io::Cursor::new(out))
            } else {
                Box::new(std::io::Cursor::new(data))
            };
            let control = String::from_utf8_lossy(&archive::read_member(reader, "control", MAX_CONTROL_BYTES)?).into_owned();
            Some(PackageMeta {
                name: field(&control, "Package", ':'),
                version: field(&control, "Version", ':'),
                summary: field(&control, "Description", ':'),
                arch: Arch::from_package_word(&field(&control, "Architecture", ':')),
                description: debian_long_description(&control),
                maintainer: person(&field(&control, "Maintainer", ':')),
                installed_size: field(&control, "Installed-Size", ':').parse::<u64>().ok().map(|kib| kib * 1024),
                depends: dependency_names(&field(&control, "Depends", ':')),
            })
        }),
        Format::Pacman | Format::Apk => archive::tar_reader(path, format).ok().and_then(|r| archive::read_member(r, ".PKGINFO", MAX_CONTROL_BYTES)).map(|b| {
            let info = String::from_utf8_lossy(&b).into_owned();
            PackageMeta {
                name: field(&info, "pkgname", '='),
                version: field(&info, "pkgver", '='),
                summary: field(&info, "pkgdesc", '='),
                arch: Arch::from_package_word(&field(&info, "arch", '=')),
                maintainer: person(&field(&info, "packager", '=')).replace("Unknown Packager", ""),
                installed_size: field(&info, "size", '=').parse().ok(),
                depends: fields(&info, "depend", '=').iter().flat_map(|d| dependency_names(d)).collect(),
                ..Default::default()
            }
        }),
        Format::Xbps => archive::tar_reader(path, format).ok().and_then(|r| archive::read_member(r, "props.plist", MAX_CONTROL_BYTES)).map(|b| {
            let plist = String::from_utf8_lossy(&b).into_owned();
            let value = |key: &str| {
                let k = format!("<key>{key}</key>");
                plist.find(&k).and_then(|at| {
                    let rest = &plist[at + k.len()..];
                    let s = rest.find("<string>")? + "<string>".len();
                    Some(rest[s..s + rest[s..].find("</string>")?].trim().to_string())
                })
            };
            let pkgver = value("pkgver").unwrap_or_default();
            let (name, version) = pkgver.rsplit_once('-').map(|(n, v)| (n.to_string(), v.to_string())).unwrap_or((pkgver.clone(), String::new()));
            PackageMeta { name, version, summary: value("short_desc").unwrap_or_default(), arch: value("architecture").and_then(|a| Arch::from_package_word(&a)), maintainer: person(&value("maintainer").unwrap_or_default()), ..Default::default() }
        }),
        _ => None,
    };
    match read {
        Some(mut m) => {
            if m.name.is_empty() {
                m.name = from_name.name.clone();
            }
            if m.version.is_empty() {
                m.version = from_name.version;
            }
            m
        }
        None => from_name,
    }
}

/// `name_1.2-1_amd64.deb`, `name-1.2-1.x86_64.rpm`, `name-1.2-1-x86_64.pkg.tar.zst`,
/// `name-1.2-r0.apk`, `name-1.2_1.x86_64.xbps`.
pub fn from_file_name(path: &Path, format: Format) -> PackageMeta {
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let base = file.split(".pkg.tar.").next().unwrap_or(&file);
    let base = base.trim_end_matches(".deb").trim_end_matches(".rpm").trim_end_matches(".apk").trim_end_matches(".xbps");
    let mut m = PackageMeta::default();
    match format {
        Format::Deb => {
            let parts: Vec<&str> = base.split('_').collect();
            m.name = parts.first().unwrap_or(&base).to_string();
            m.version = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
            m.arch = parts.get(2).and_then(|a| Arch::from_package_word(a));
        }
        Format::Rpm | Format::Xbps => {
            let (rest, arch) = base.rsplit_once('.').unwrap_or((base, ""));
            m.arch = Arch::from_package_word(arch);
            let parts: Vec<&str> = rest.rsplitn(if format == Format::Rpm { 3 } else { 2 }, '-').collect();
            m.name = parts.last().unwrap_or(&rest).to_string();
            // `rsplitn` yields release, version, name: rejoin the middle.
            m.version = parts[..parts.len().saturating_sub(1)].iter().rev().copied().collect::<Vec<_>>().join("-");
        }
        Format::Pacman => {
            let parts: Vec<&str> = base.rsplitn(4, '-').collect();
            if parts.len() == 4 {
                m.arch = Arch::from_package_word(parts[0]);
                m.version = format!("{}-{}", parts[2], parts[1]);
                m.name = parts[3].to_string();
            } else {
                m.name = base.to_string();
            }
        }
        Format::Apk => {
            let parts: Vec<&str> = base.rsplitn(3, '-').collect();
            if parts.len() == 3 {
                m.version = format!("{}-{}", parts[1], parts[0]);
                m.name = parts[2].to_string();
            } else {
                m.name = base.to_string();
            }
        }
        _ => m.name = base.to_string(),
    }
    m
}

/// The distribution family a format belongs to (for the foreign-package
/// explanation).
fn family_name(format: Format) -> &'static str {
    match format {
        Format::Deb => "Debian / Ubuntu",
        Format::Rpm => "Fedora / openSUSE",
        Format::Pacman => "Arch Linux",
        Format::Apk => "Alpine Linux",
        Format::Xbps => "Void Linux",
        _ => "",
    }
}

/// Package managers that install `format`, in preference order.
fn managers_for(format: Format) -> &'static [Source] {
    match format {
        Format::Deb => &[Source::Apt],
        Format::Rpm => &[Source::Dnf, Source::Zypper, Source::Yum, Source::Rpm],
        Format::Pacman => &[Source::Pacman],
        Format::Apk => &[Source::Apk],
        Format::Xbps => &[Source::Xbps],
        _ => &[],
    }
}

/// The system's package manager for `format`, or why it can't install it.
pub fn manager(format: Format, runner: &dyn Runner) -> Result<(Source, &'static str), String> {
    let system = system_manager(runner);
    let ok = system.filter(|s| managers_for(format).contains(s));
    let program = match ok {
        Some(Source::Apt) if runner.has("apt-get") => "apt-get",
        Some(Source::Dnf) => ["dnf5", "dnf"].into_iter().find(|p| runner.has(p)).unwrap_or("dnf"),
        Some(Source::Yum) => "yum",
        Some(Source::Zypper) => "zypper",
        Some(Source::Rpm) => "rpm",
        Some(Source::Pacman) => "pacman",
        Some(Source::Apk) => "apk",
        Some(Source::Xbps) if runner.has("xbps-install") && runner.has("xbps-rindex") => "xbps-install",
        _ => return Err(foreign(format, system)),
    };
    Ok((ok.unwrap_or(Source::Unknown), program))
}

/// The explanation for a package from another distribution family.
pub fn foreign(format: Format, system: Option<Source>) -> String {
    let what = tf("This is a {family} package ({extension}).", &[("family", &family_name(format)), ("extension", &format.extension())]);
    let uses = match system {
        Some(s) => tf("This computer installs software with {manager}, which can't install it.", &[("manager", &s.label())]),
        None => t("No package manager on this computer can install it.").to_string(),
    };
    let never = t("Mandara installs native packages only with your system's own package manager: it never converts them or runs them in a container.");
    let mut alternatives = t("Look for this app as a Flatpak (Flathub) or an AppImage instead.").to_string();
    if system == Some(Source::Pacman) {
        alternatives.push(' ');
        alternatives.push_str(t("On Arch-based systems it may also be in the AUR."));
    }
    format!("{what} {uses} {never} {alternatives}")
}

/// A native install: commands and the manager's simulation.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NativePlan {
    pub manager: Option<Source>,
    pub package: String,
    /// Unprivileged preparation (XBPS: index a private copy).
    pub prepare: Vec<Vec<String>>,
    /// The privileged install (starts with `pkexec`).
    pub command: Vec<String>,
    pub simulation: Simulation,
}

/// What the package manager's dry run said.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Simulation {
    /// Packages it would install (the package itself and dependencies).
    pub installs: Vec<String>,
    /// Conflicts / unsatisfiable dependencies, in its own words.
    pub conflicts: Vec<String>,
    /// Shown when the output couldn't be read.
    pub note: Option<String>,
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|s| s.to_string()).collect()
}

/// The install argv (without `pkexec`) and the simulation argv.
pub fn commands(source: Source, program: &str, file: &Path, package: &str, repo: Option<&Path>) -> (Vec<String>, Option<Vec<String>>) {
    let f = file.to_string_lossy().to_string();
    let repo_flag = repo.map(|r| format!("--repository={}", r.display())).unwrap_or_default();
    match source {
        Source::Apt => (words(&["apt-get", "install", "-y", &f]), Some(words(&["apt-get", "-s", "install", &f]))),
        Source::Dnf | Source::Yum => (words(&[program, "install", "-y", &f]), Some(words(&[program, "install", "--assumeno", &f]))),
        Source::Zypper => (words(&["zypper", "--non-interactive", "install", "--allow-unsigned-rpm", &f]), Some(words(&["zypper", "--non-interactive", "install", "--dry-run", "--allow-unsigned-rpm", &f]))),
        Source::Rpm => (words(&["rpm", "-i", &f]), Some(words(&["rpm", "-i", "--test", &f]))),
        Source::Pacman => (words(&["pacman", "-U", "--noconfirm", &f]), Some(words(&["pacman", "-U", "--print", "--print-format", "%n", &f]))),
        Source::Apk => (words(&["apk", "add", "--allow-untrusted", &f]), Some(words(&["apk", "add", "--simulate", "--allow-untrusted", &f]))),
        Source::Xbps => (words(&["xbps-install", "-y", &repo_flag, package]), Some(words(&["xbps-install", "-n", &repo_flag, package]))),
        _ => (Vec::new(), None),
    }
}

/// Reads a simulation's output.
pub fn parse_simulation(source: Source, out: &CmdOutput) -> Simulation {
    let all = format!("{}\n{}", out.stdout, out.stderr);
    let mut s = Simulation::default();
    let problem = |l: &str| {
        let lower = l.to_ascii_lowercase();
        lower.contains("conflict") || lower.contains("broken") || lower.contains("not installable") || lower.contains("nothing provides") || lower.contains("could not satisfy") || lower.contains("unsatisfiable") || lower.contains("is needed by") || lower.starts_with("e:") || lower.starts_with("error") || lower.starts_with("problem")
    };
    match source {
        Source::Apt => s.installs = out.stdout.lines().filter_map(|l| l.strip_prefix("Inst ")).filter_map(|r| r.split_whitespace().next()).map(String::from).collect(),
        Source::Dnf | Source::Yum => {
            let mut on = false;
            for line in out.stdout.lines() {
                let l = line.trim();
                let lower = l.to_ascii_lowercase();
                if lower.starts_with("installing") || lower.starts_with("upgrading") {
                    on = true;
                    continue;
                }
                if l.is_empty() || lower.starts_with("transaction summary") || lower.starts_with("operation aborted") || lower.starts_with("removing") || lower.starts_with("skipping") {
                    on = false;
                    continue;
                }
                if on && let Some(name) = l.split_whitespace().next().filter(|w| !w.starts_with('=') && !w.eq_ignore_ascii_case("package")) {
                    s.installs.push(name.to_string());
                }
            }
        }
        Source::Zypper => {
            let mut on = false;
            for line in out.stdout.lines() {
                let l = line.trim();
                if l.contains("going to be installed") || l.contains("going to be upgraded") {
                    on = true;
                    continue;
                }
                if l.is_empty() {
                    on = false;
                    continue;
                }
                if on {
                    s.installs.extend(l.split_whitespace().map(String::from));
                }
            }
        }
        Source::Rpm => {
            if out.success() {
                s.installs.push(String::new());
            }
        }
        Source::Pacman => {
            if out.success() {
                s.installs = out.stdout.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(':') && !l.contains(' ')).map(String::from).collect();
            }
        }
        Source::Apk => s.installs = all.lines().filter_map(|l| l.split_once("Installing ").and_then(|(_, r)| r.split_whitespace().next())).map(String::from).collect(),
        Source::Xbps => {
            s.installs = out
                .stdout
                .lines()
                .filter(|l| l.split_whitespace().nth(1).is_some_and(|a| a == "install" || a == "update"))
                .filter_map(|l| l.split_whitespace().next())
                .map(|v| v.rsplit_once('-').map(|(n, _)| n).unwrap_or(v).to_string())
                .collect()
        }
        _ => {}
    }
    s.installs.retain(|n| !n.is_empty());
    s.installs.dedup();
    s.conflicts = all.lines().map(str::trim).filter(|l| !l.is_empty() && problem(l)).take(MAX_PROBLEM_LINES).map(String::from).collect();
    if s.installs.is_empty() && s.conflicts.is_empty() && !out.success() {
        let last = all.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_string();
        s.note = Some(tf("Couldn't preview what gets installed: {error}", &[("error", &last)]));
    }
    s
}

/// Builds the native plan: manager, commands, its simulation. `scratch`
/// holds XBPS's private repository index.
pub fn plan(file: &Path, format: Format, package: &str, scratch: &Path, runner: &dyn Runner) -> Result<NativePlan, String> {
    let (source, program) = manager(format, runner)?;
    let mut plan = NativePlan { manager: Some(source), package: package.to_string(), ..Default::default() };
    let mut repo: Option<PathBuf> = None;
    if source == Source::Xbps {
        // xbps-install takes repositories, not files: index a private copy.
        let dir = scratch.join("xbps-repo");
        let copy = dir.join(file.file_name().unwrap_or_default());
        plan.prepare.push(words(&["xbps-rindex", "-a", &copy.to_string_lossy()]));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::copy(file, &copy).map_err(|e| e.to_string())?;
        for cmd in &plan.prepare {
            let out = runner.run(cmd, SIMULATE_TIMEOUT)?;
            if !out.success() {
                return Err(out.stderr.lines().last().unwrap_or_default().to_string());
            }
        }
        repo = Some(dir);
    }
    let (install, simulate) = commands(source, program, file, package, repo.as_deref());
    plan.command = std::iter::once(super::PKEXEC.to_string()).chain(install).collect();
    if let Some(cmd) = simulate {
        match runner.run(&cmd, SIMULATE_TIMEOUT) {
            Ok(out) => plan.simulation = parse_simulation(source, &out),
            Err(e) => plan.simulation.note = Some(tf("Couldn't preview what gets installed: {error}", &[("error", &e)])),
        }
    }
    Ok(plan)
}

/// The desktop entry an installed package ships (its id without
/// `.desktop`), from the package manager's file list.
pub fn desktop_of(source: Source, package: &str, runner: &dyn Runner) -> Option<String> {
    let cmd = match source {
        Source::Pacman => words(&["pacman", "-Qlq", package]),
        Source::Apt => words(&["dpkg", "-L", package]),
        Source::Dnf | Source::Yum | Source::Zypper | Source::Rpm => words(&["rpm", "-ql", package]),
        Source::Apk => words(&["apk", "info", "-L", package]),
        Source::Xbps => words(&["xbps-query", "-f", package]),
        _ => return None,
    };
    let out = runner.run(&cmd, SIMULATE_TIMEOUT).ok().filter(CmdOutput::success)?;
    out.stdout
        .lines()
        .map(|l| l.trim().trim_start_matches('/'))
        .find(|l| l.starts_with("usr/share/applications/") && l.ends_with(".desktop"))
        .and_then(|l| Path::new(l).file_stem().map(|s| s.to_string_lossy().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_files() {
        let m = from_file_name(Path::new("/d/code_1.92.0-1722473020_amd64.deb"), Format::Deb);
        assert_eq!((m.name.as_str(), m.version.as_str(), m.arch), ("code", "1.92.0-1722473020", Some(Arch::X86_64)));
        let m = from_file_name(Path::new("foo-bar-2.0-1-x86_64.pkg.tar.zst"), Format::Pacman);
        assert_eq!((m.name.as_str(), m.version.as_str()), ("foo-bar", "2.0-1"));
        let m = from_file_name(Path::new("foo-bar-2.0-3.fc40.x86_64.rpm"), Format::Rpm);
        assert_eq!(m.name, "foo-bar");
        let m = from_file_name(Path::new("tool-1.2_1.x86_64.xbps"), Format::Xbps);
        assert_eq!(m.name, "tool");
        let m = from_file_name(Path::new("tool-1.2-r0.apk"), Format::Apk);
        assert_eq!((m.name.as_str(), m.version.as_str()), ("tool", "1.2-r0"));
    }

    #[test]
    fn control_details() {
        let control = "Package: demo\nVersion: 1.0\nMaintainer: Jane Doe <jane@x.org>\nInstalled-Size: 2048\nDepends: libc6 (>= 2.34), libgtk-3-0 | libgtk-4-1, zlib1g:any\nDescription: A demo\n Line one\n continues.\n .\n Second paragraph.\nHomepage: https://x.org\n";
        assert_eq!(debian_long_description(control), "Line one continues.\nSecond paragraph.");
        assert_eq!(person(&field(control, "Maintainer", ':')), "Jane Doe");
        assert_eq!(dependency_names(&field(control, "Depends", ':')), vec!["libc6", "libgtk-3-0", "zlib1g"]);
        assert_eq!(fields("depend = a>=1\ndepend = b\n", "depend", '='), vec!["a>=1", "b"]);
    }

    #[test]
    fn simulations_per_manager() {
        let apt = CmdOutput::ok("Inst libfoo (1.0 Debian:stable)\nInst code (1.92 local)\nConf code\n");
        assert_eq!(parse_simulation(Source::Apt, &apt).installs, vec!["libfoo", "code"]);
        let apk = CmdOutput::ok("(1/2) Installing libx (1.0-r0)\n(2/2) Installing tool (1.2-r0)\nOK\n");
        assert_eq!(parse_simulation(Source::Apk, &apk).installs, vec!["libx", "tool"]);
        let pac = CmdOutput::ok("libfoo\nfoo-bar\n");
        assert_eq!(parse_simulation(Source::Pacman, &pac).installs, vec!["libfoo", "foo-bar"]);
        let pac_conflict = CmdOutput::failed(1, "error: failed to prepare transaction (conflicting dependencies)\n:: foo and bar are in conflict\n");
        let s = parse_simulation(Source::Pacman, &pac_conflict);
        assert!(s.installs.is_empty() && !s.conflicts.is_empty(), "{s:?}");
        let dnf = CmdOutput { code: Some(1), stdout: "Dependencies resolved.\n Package  Arch\nInstalling:\n foo  x86_64  1.0  @commandline  1 M\nInstalling dependencies:\n libbar x86_64 2 fedora 2 M\n\nTransaction Summary\n".into(), stderr: "Operation aborted.\n".into() };
        assert_eq!(parse_simulation(Source::Dnf, &dnf).installs, vec!["foo", "libbar"]);
        let xbps = CmdOutput::ok("libz-1.3_1 install x86_64 /tmp/repo 1MB\ntool-1.2_1 install x86_64 /tmp/repo 2MB\n");
        assert_eq!(parse_simulation(Source::Xbps, &xbps).installs, vec!["libz", "tool"]);
    }

    #[test]
    fn foreign_packages_are_explained() {
        let msg = foreign(Format::Deb, Some(Source::Pacman));
        assert!(msg.contains("Debian") && msg.contains("Pacman") && msg.contains("Flatpak") && msg.contains("AUR"), "{msg}");
    }
}
