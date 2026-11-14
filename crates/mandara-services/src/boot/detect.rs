//! Which bootloader this machine boots with, and where every tool keeps its
//! files: read-only probes, no root needed for most of it.
//!
//! The bootloader in use comes from, in order:
//! 1. the Boot Loader Interface's `LoaderInfo` EFI variable (set by
//!    Limine, systemd-boot and GRUB 2.12's `bli` module);
//! 2. the firmware's `BootCurrent` entry and the EFI file it started
//!    (`efibootmgr`);
//! 3. the one bootloader whose config is installed (BIOS machines).
//!
//! Anything else (two installed, none answering) is reported as unknown,
//! and nothing is applied: guessing which config a machine boots from is
//! how a theme ends up in a file no bootloader reads.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::Sys;
use super::edit::ShellVars;

/// The Boot Loader Interface vendor GUID.
const BLI_GUID: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";
/// The EFI global variable GUID (`SecureBoot`).
const EFI_GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// Where a FAT system partition is mounted, in the order the distribution
/// tools search.
const ESP_MOUNTS: [&str; 4] = ["/efi", "/boot", "/boot/efi", "/limine"];
/// Bytes of a vendor `grub.cfg` on the ESP above which it is the real
/// config rather than a stub that loads `/boot/grub2/grub.cfg`.
const GRUB_STUB_MAX: u64 = 4096;
/// Largest config file read during detection.
const READ_MAX: u64 = 1 << 20;
/// Programs are only taken from these folders (root-owned system folders;
/// never `/usr/local`, where wrappers may prompt or differ).
pub const TOOL_DIRS: &[&str] = &["/usr/sbin", "/usr/bin", "/sbin", "/bin"];
/// Plymouth's plugin folders across distributions.
const PLYMOUTH_PLUGIN_DIRS: &[&str] = &["/usr/lib/plymouth", "/usr/lib64/plymouth", "/usr/lib/x86_64-linux-gnu/plymouth", "/usr/lib/aarch64-linux-gnu/plymouth"];
pub const PLYMOUTH_THEMES: &str = "/usr/share/plymouth/themes";

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Loader {
    Grub,
    Limine,
    Refind,
    SystemdBoot,
}

impl Loader {
    pub fn name(self) -> &'static str {
        match self {
            Loader::Grub => "GRUB",
            Loader::Limine => "Limine",
            Loader::Refind => "rEFInd",
            Loader::SystemdBoot => "systemd-boot",
        }
    }

    /// What a loader name or EFI path names.
    pub fn classify(text: &str) -> Option<Loader> {
        let s = text.to_ascii_lowercase();
        if s.contains("limine") {
            Some(Loader::Limine)
        } else if s.contains("refind") {
            Some(Loader::Refind)
        } else if s.contains("systemd-boot") || s.contains("systemd") || s.contains("linux boot manager") {
            Some(Loader::SystemdBoot)
        } else if s.contains("grub") || s.contains("shim") {
            Some(Loader::Grub)
        } else {
            None
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Firmware {
    Uefi,
    Bios,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Os {
    pub id: String,
    pub id_like: Vec<String>,
    pub pretty: String,
}

impl Os {
    /// Whether this is (or derives from) one of `ids`.
    pub fn is(&self, ids: &[&str]) -> bool {
        ids.iter().any(|i| self.id == *i || self.id_like.iter().any(|l| l == i))
    }

    /// Distributions whose GRUB is `grub2-*` with `/boot/grub2`.
    pub fn grub2_naming(&self) -> bool {
        self.is(&["fedora", "rhel", "centos", "suse", "opensuse", "opensuse-tumbleweed", "opensuse-leap", "sles", "mageia"])
    }
}

/// GRUB's files on this system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Grub {
    /// `/boot/grub` or `/boot/grub2`.
    pub dir: PathBuf,
    /// The config GRUB reads (`<dir>/grub.cfg`).
    pub cfg: PathBuf,
    /// `/etc/default/grub`.
    pub defaults: PathBuf,
    /// `/etc/default/grub.d` when this system's `grub-mkconfig` reads it
    /// (Debian, Ubuntu, Mint): settings then go in a drop-in.
    pub dropin_dir: Option<PathBuf>,
    pub mkconfig: PathBuf,
    pub script_check: Option<PathBuf>,
    pub mkfont: Option<PathBuf>,
    /// A full `grub.cfg` on the ESP (older layouts): unsupported.
    pub efi_full_config: Option<PathBuf>,
    /// Whether `/boot/grub*` could be read (without root it often can't).
    pub readable: bool,
    /// The settings in effect now.
    pub current: GrubSettings,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GrubSettings {
    pub theme: Option<String>,
    pub background: Option<String>,
    pub gfxmode: Option<String>,
    pub terminal_output: Option<String>,
    pub terminal_input: Option<String>,
    pub terminal: Option<String>,
    /// `GRUB_TIMEOUT_STYLE=hidden` or `GRUB_TIMEOUT=0`: the menu is never
    /// shown, so its theme isn't either.
    pub menu_hidden: bool,
}

/// Limine's files on this system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Limine {
    /// The FAT partition holding the config.
    pub esp: PathBuf,
    /// The config Limine reads.
    pub config: PathBuf,
    /// Whether `config` was seen (an unreadable `/boot` hides it).
    pub config_seen: bool,
    /// CachyOS's `limine-entry-tool` manages the entries (and the
    /// initramfs is rebuilt with `limine-mkinitcpio`).
    pub entry_tool: bool,
    /// `ENABLE_ENROLL_LIMINE_CONFIG=yes`: the config's hash is enrolled in
    /// the Limine binary, so every change must be enrolled again.
    pub enroll: bool,
    pub enroll_tool: Option<PathBuf>,
    /// `ENABLE_VERIFICATION=yes`: entries carry file hashes.
    pub verification: bool,
    /// A config Limine finds before [`Self::config`] (so changing that one
    /// would change nothing, or the wrong file).
    pub shadowed_by: Option<PathBuf>,
}

/// The config Limine reads instead of the one Mandara would change.
pub fn limine_shadow(l: &Limine) -> Option<&PathBuf> {
    l.shadowed_by.as_ref()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refind {
    pub dir: PathBuf,
    pub conf: PathBuf,
}

/// How this system rebuilds its initramfs (Plymouth themes live in it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "tool", content = "path", rename_all = "kebab-case")]
pub enum Initramfs {
    /// CachyOS + Limine: rebuilds and re-hashes the Limine entries.
    LimineMkinitcpio(PathBuf),
    Mkinitcpio(PathBuf),
    Dracut(PathBuf),
    UpdateInitramfs(PathBuf),
}

impl Initramfs {
    /// The fixed argv that rebuilds every initramfs.
    pub fn argv(&self) -> Vec<String> {
        let p = |p: &PathBuf| p.display().to_string();
        match self {
            Initramfs::LimineMkinitcpio(t) => vec![p(t)],
            Initramfs::Mkinitcpio(t) => vec![p(t), "-P".into()],
            Initramfs::Dracut(t) => vec![p(t), "--force".into(), "--regenerate-all".into()],
            Initramfs::UpdateInitramfs(t) => vec![p(t), "-u".into(), "-k".into(), "all".into()],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plymouth {
    pub themes_dir: PathBuf,
    pub set_default: PathBuf,
    /// The theme in effect (`plymouthd.conf`, else the distribution's default).
    pub current: Option<String>,
    /// Installed theme folders (each with a `.plymouth` file).
    pub installed: Vec<String>,
    /// Splash plugins available (`two-step`, `script`, …).
    pub modules: Vec<String>,
    pub initramfs: Option<Initramfs>,
    /// Whether the initramfs includes Plymouth (`None`: can't tell).
    pub in_initramfs: Option<bool>,
    /// `splash` on the running kernel's command line.
    pub splash_on_cmdline: bool,
}

/// Everything detected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Detection {
    /// `false` on FreeBSD (its loader is out of scope).
    pub supported: bool,
    pub os: Os,
    pub firmware: Firmware,
    pub secure_boot: Option<bool>,
    /// The bootloader this machine booted with.
    pub in_use: Option<Loader>,
    /// How [`Self::in_use`] was found.
    pub evidence: String,
    /// The bootloaders with a config here.
    pub installed: Vec<Loader>,
    pub grub: Option<Grub>,
    pub limine: Option<Limine>,
    pub refind: Option<Refind>,
    pub systemd_boot: bool,
    pub plymouth: Option<Plymouth>,
    /// The first connected display's native mode (what the splash draws at).
    pub screen: Option<(u32, u32)>,
}

impl Detection {
    /// The bootloader whose menu Mandara may restyle: the one in use, and
    /// only when its config was found.
    pub fn menu_target(&self) -> Option<Loader> {
        self.in_use.filter(|l| self.installed.contains(l))
    }

    /// The FreeBSD answer.
    pub fn unsupported() -> Self {
        Self { supported: false, os: Os::default(), firmware: Firmware::Bios, secure_boot: None, in_use: None, evidence: "FreeBSD's loader is not supported".into(), installed: Vec::new(), grub: None, limine: None, refind: None, systemd_boot: false, plymouth: None, screen: None }
    }
}

/// Reads a small text file under `sys` (`None` if missing, unreadable or
/// too large).
pub fn read(sys: &Sys, abs: impl AsRef<Path>) -> Option<String> {
    read_path(&sys.path(abs))
}

fn read_path(p: &Path) -> Option<String> {
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() > READ_MAX {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

/// `name` in [`TOOL_DIRS`] under `sys` (its system path).
pub fn tool(sys: &Sys, name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    if name.contains('/') {
        return None;
    }
    TOOL_DIRS.iter().map(|d| Path::new(d).join(name)).find(|p| std::fs::metadata(sys.path(p)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// `/etc/os-release`.
pub fn os(sys: &Sys) -> Os {
    let text = read(sys, "/etc/os-release").or_else(|| read(sys, "/usr/lib/os-release")).unwrap_or_default();
    let vars = ShellVars::parse(&text);
    let get = |k: &str| vars.get(k).unwrap_or_default();
    Os { id: get("ID").to_ascii_lowercase(), id_like: get("ID_LIKE").split_whitespace().map(str::to_ascii_lowercase).collect(), pretty: get("PRETTY_NAME") }
}

/// `(mount point, fs type)` of every mount (`/proc/self/mountinfo`).
pub fn mounts(sys: &Sys) -> Vec<(PathBuf, String)> {
    let text = read(sys, "/proc/self/mountinfo").unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let target = left.split(' ').nth(4)?;
            let fstype = right.split(' ').next()?;
            Some((PathBuf::from(unescape_mount(target)), fstype.to_string()))
        })
        .collect()
}

/// mountinfo escapes space, tab, newline and backslash as `\ooo`.
pub(crate) fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let n = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(n);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mounted FAT partitions among [`ESP_MOUNTS`], in search order.
pub fn esps(sys: &Sys) -> Vec<PathBuf> {
    let mounts = mounts(sys);
    ESP_MOUNTS.iter().map(PathBuf::from).filter(|m| mounts.iter().any(|(t, fs)| t == m && fs == "vfat")).collect()
}

/// An EFI variable's payload (after the 4 attribute bytes).
fn efivar(sys: &Sys, name: &str, guid: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(sys.path(format!("/sys/firmware/efi/efivars/{name}-{guid}"))).ok()?;
    (bytes.len() > 4).then(|| bytes[4..].to_vec())
}

/// `LoaderInfo` ("Limine 12.9.6", "systemd-boot 257").
pub fn loader_info(sys: &Sys) -> Option<String> {
    let payload = efivar(sys, "LoaderInfo", BLI_GUID)?;
    let units: Vec<u16> = payload.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|u| *u != 0).collect();
    let s = String::from_utf16_lossy(&units).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The label and EFI file of `BootCurrent` in `efibootmgr` output.
pub fn boot_current(efibootmgr: &str) -> Option<(String, Option<String>)> {
    let current = efibootmgr.lines().find_map(|l| l.strip_prefix("BootCurrent:"))?.trim().to_string();
    let line = efibootmgr.lines().find(|l| l.starts_with(&format!("Boot{current}")))?;
    let rest = line[4 + current.len()..].trim_start_matches('*').trim();
    let (label, path) = match rest.split_once('\t') {
        Some((label, path)) => (label.trim(), Some(path)),
        None => (rest, None),
    };
    let file = path.and_then(|p| {
        let start = p.find("File(")? + 5;
        let end = p[start..].find(')')? + start;
        Some(p[start..end].to_string())
    });
    Some((label.to_string(), file))
}

/// Runs `efibootmgr` (read-only listing) when it exists.
fn efibootmgr_output(sys: &Sys) -> Option<String> {
    if !sys.is_real() {
        return read(sys, "/fixture/efibootmgr.txt");
    }
    let tool = tool(sys, "efibootmgr")?;
    let out = std::process::Command::new(tool).env_clear().env("LC_ALL", "C").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn grub(sys: &Sys, os: &Os) -> Option<Grub> {
    let defaults = PathBuf::from("/etc/default/grub");
    if !sys.path(&defaults).is_file() {
        return None;
    }
    let (mkconfig, two) = match (tool(sys, "grub-mkconfig"), tool(sys, "grub2-mkconfig")) {
        (Some(t), None) => (t, false),
        (None, Some(t)) => (t, true),
        // Both: the distribution's naming decides.
        (Some(a), Some(b)) => if os.grub2_naming() { (b, true) } else { (a, false) },
        (None, None) => return None,
    };
    let prefix = if two { "grub2" } else { "grub" };
    let boot_dir = |name: &str| sys.path(format!("/boot/{name}")).is_dir();
    let dir = if boot_dir(prefix) || !boot_dir(if two { "grub" } else { "grub2" }) { PathBuf::from(format!("/boot/{prefix}")) } else { PathBuf::from(format!("/boot/{}", if two { "grub" } else { "grub2" })) };
    let readable = std::fs::read_dir(sys.path(&dir)).is_ok();
    let script = read(sys, &mkconfig).unwrap_or_default();
    let dropin_dir = script.contains("default/grub.d").then(|| PathBuf::from("/etc/default/grub.d"));
    // A full config on the ESP means GRUB reads that one (older Fedora
    // UEFI layouts); a stub only `configfile`s the real one.
    let efi_full_config = esps(sys).into_iter().find_map(|esp| {
        let efi = sys.path(esp.join("EFI"));
        std::fs::read_dir(efi).ok()?.flatten().find_map(|vendor| {
            let cfg = vendor.path().join("grub.cfg");
            let meta = std::fs::metadata(&cfg).ok()?;
            let text = read_path(&cfg)?;
            (meta.len() > GRUB_STUB_MAX && !text.contains("configfile")).then(|| sys.system_path(&cfg))
        })
    });
    let mut vars = ShellVars::parse(&read(sys, &defaults).unwrap_or_default());
    if let Some(d) = &dropin_dir {
        let mut files: Vec<PathBuf> = std::fs::read_dir(sys.path(d)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "cfg")).collect();
        files.sort();
        for f in files {
            vars.overlay(&ShellVars::parse(&read_path(&f).unwrap_or_default()));
        }
    }
    let current = GrubSettings {
        theme: vars.get("GRUB_THEME"),
        background: vars.get("GRUB_BACKGROUND"),
        gfxmode: vars.get("GRUB_GFXMODE"),
        terminal_output: vars.get("GRUB_TERMINAL_OUTPUT"),
        terminal_input: vars.get("GRUB_TERMINAL_INPUT"),
        terminal: vars.get("GRUB_TERMINAL"),
        menu_hidden: vars.get("GRUB_TIMEOUT_STYLE").is_some_and(|s| s == "hidden") || vars.get("GRUB_TIMEOUT").is_some_and(|s| s.trim() == "0"),
    };
    Some(Grub {
        cfg: dir.join("grub.cfg"),
        dir,
        defaults,
        dropin_dir,
        mkconfig,
        script_check: tool(sys, &format!("{prefix}-script-check")),
        mkfont: tool(sys, &format!("{prefix}-mkfont")),
        efi_full_config,
        readable,
        current,
    })
}

/// The settings `limine-entry-tool` reads, lowest priority first.
fn limine_settings(sys: &Sys) -> ShellVars {
    let mut vars = ShellVars::default();
    let conf_dir = |d: &str| {
        let mut files: Vec<PathBuf> = std::fs::read_dir(sys.path(d)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "conf")).collect();
        files.sort();
        files
    };
    for f in conf_dir("/usr/share/limine-entry-tool.d").into_iter().chain([sys.path("/etc/limine-entry-tool.conf")]).chain(conf_dir("/etc/limine-entry-tool.d")) {
        vars.overlay(&ShellVars::parse(&read_path(&f).unwrap_or_default()));
    }
    // Only /etc/default/limine may switch enrolment on.
    vars.remove("ENABLE_ENROLL_LIMINE_CONFIG");
    vars.overlay(&ShellVars::parse(&read(sys, "/etc/default/limine").unwrap_or_default()));
    vars
}

/// Where Limine looks for its config on `esp`, in its own order (the EFI
/// app's own folder first).
pub fn limine_candidates(esp: &Path, efi_file: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(file) = efi_file {
        let rel = file.replace('\\', "/");
        if let Some(dir) = Path::new(rel.trim_start_matches('/')).parent() {
            out.push(esp.join(dir).join("limine.conf"));
        }
    }
    for rel in ["EFI/BOOT/limine.conf", "boot/limine/limine.conf", "boot/limine.conf", "limine/limine.conf", "limine.conf"] {
        let p = esp.join(rel);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn limine(sys: &Sys, efi_file: Option<&str>) -> Option<Limine> {
    let has_tool = tool(sys, "limine").is_some() || sys.path("/usr/share/limine").is_dir();
    let defaults = sys.path("/etc/default/limine").is_file();
    if !has_tool && !defaults {
        return None;
    }
    let vars = limine_settings(sys);
    let entry_tool = defaults && tool(sys, "limine-entry-tool").is_some();
    let esp = vars.get("ESP_PATH").map(|p| PathBuf::from(p.trim())).filter(|p| p.is_absolute()).or_else(|| esps(sys).into_iter().next())?;
    let candidates = limine_candidates(&esp, efi_file);
    let seen = candidates.iter().find(|c| sys.path(c).is_file()).cloned();
    let config = if entry_tool { esp.join("limine.conf") } else { seen.clone().unwrap_or_else(|| esp.join("limine.conf")) };
    let yes = |k: &str| vars.get(k).is_some_and(|v| v.trim() == "yes");
    let shadowed_by = seen.clone().filter(|s| *s != config);
    Some(Limine { config_seen: seen.is_some(), esp, config, entry_tool, enroll: yes("ENABLE_ENROLL_LIMINE_CONFIG"), enroll_tool: tool(sys, "limine-enroll-config"), verification: yes("ENABLE_VERIFICATION"), shadowed_by })
}

fn refind(sys: &Sys) -> Option<Refind> {
    esps(sys).into_iter().find_map(|esp| {
        let dir = esp.join("EFI/refind");
        let conf = dir.join("refind.conf");
        sys.path(&conf).is_file().then_some(Refind { dir, conf })
    })
}

fn systemd_boot(sys: &Sys) -> bool {
    esps(sys).iter().any(|esp| sys.path(esp.join("loader/loader.conf")).is_file() || sys.path(esp.join("EFI/systemd")).is_dir())
}

/// The value of `key` in an INI `section` (`[Daemon] Theme=`).
pub fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line.trim_matches(['[', ']']).eq_ignore_ascii_case(section);
        } else if inside && let Some((k, v)) = line.split_once('=') && k.trim().eq_ignore_ascii_case(key) {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// The last `HOOKS=(…)` across the mkinitcpio config files.
fn mkinitcpio_hooks(sys: &Sys) -> Option<Vec<String>> {
    let mut files = vec![sys.path("/etc/mkinitcpio.conf")];
    let mut extra: Vec<PathBuf> = std::fs::read_dir(sys.path("/etc/mkinitcpio.conf.d")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "conf")).collect();
    extra.sort();
    files.extend(extra);
    let mut hooks = None;
    for f in files {
        for line in read_path(&f).unwrap_or_default().lines().map(str::trim) {
            if let Some(list) = line.strip_prefix("HOOKS=(").and_then(|l| l.split(')').next()) {
                hooks = Some(list.split_whitespace().map(str::to_string).collect());
            }
        }
    }
    hooks
}

fn plymouth(sys: &Sys, limine: Option<&Limine>) -> Option<Plymouth> {
    let set_default = tool(sys, "plymouth-set-default-theme")?;
    let themes_dir = PathBuf::from(PLYMOUTH_THEMES);
    if !sys.path(&themes_dir).is_dir() {
        return None;
    }
    let current = read(sys, "/etc/plymouth/plymouthd.conf").and_then(|t| ini_get(&t, "Daemon", "Theme")).or_else(|| read(sys, "/usr/share/plymouth/plymouthd.defaults").and_then(|t| ini_get(&t, "Daemon", "Theme"))).filter(|t| !t.is_empty());
    let mut installed: Vec<String> = std::fs::read_dir(sys.path(&themes_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            e.path().join(format!("{name}.plymouth")).is_file().then_some(name)
        })
        .collect();
    installed.sort();
    let modules: Vec<String> = PLYMOUTH_PLUGIN_DIRS
        .iter()
        .find_map(|d| std::fs::read_dir(sys.path(d)).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".so").map(str::to_string))
        .collect();
    let limine_hook = limine.is_some_and(|l| l.entry_tool) && sys.path("/etc/default/limine").is_file();
    let initramfs = match (limine_hook.then(|| tool(sys, "limine-mkinitcpio")).flatten(), tool(sys, "mkinitcpio"), tool(sys, "dracut"), tool(sys, "update-initramfs")) {
        (Some(t), ..) => Some(Initramfs::LimineMkinitcpio(t)),
        (None, Some(t), ..) => Some(Initramfs::Mkinitcpio(t)),
        (None, None, Some(t), _) => Some(Initramfs::Dracut(t)),
        (None, None, None, Some(t)) => Some(Initramfs::UpdateInitramfs(t)),
        _ => None,
    };
    let in_initramfs = match &initramfs {
        Some(Initramfs::LimineMkinitcpio(_) | Initramfs::Mkinitcpio(_)) => mkinitcpio_hooks(sys).map(|h| h.iter().any(|x| x == "plymouth" || x == "sd-plymouth")),
        Some(Initramfs::Dracut(_)) => Some(std::fs::read_dir(sys.path("/usr/lib/dracut/modules.d")).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().contains("plymouth"))),
        Some(Initramfs::UpdateInitramfs(_)) => Some(sys.path("/usr/share/initramfs-tools/hooks/plymouth").is_file()),
        None => None,
    };
    let splash_on_cmdline = read(sys, "/proc/cmdline").is_some_and(|c| c.split_whitespace().any(|w| w == "splash"));
    Some(Plymouth { themes_dir, set_default, current, installed, modules, initramfs, in_initramfs, splash_on_cmdline })
}

/// The preferred mode of the first connected display (`/sys/class/drm`,
/// readable without root): built-in panels first.
pub fn native_screen(sys: &Sys) -> Option<(u32, u32)> {
    let mut connectors: Vec<PathBuf> = std::fs::read_dir(sys.path("/sys/class/drm")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("status").is_file()).collect();
    connectors.sort_by_key(|p| (!p.to_string_lossy().contains("eDP"), p.clone()));
    connectors.iter().filter(|c| read_path(&c.join("status")).is_some_and(|s| s.trim() == "connected")).find_map(|c| {
        let modes = read_path(&c.join("modes"))?;
        let (w, h) = modes.lines().next()?.trim().split_once('x')?;
        let h: String = h.chars().take_while(char::is_ascii_digit).collect();
        Some((w.parse().ok()?, h.parse().ok()?))
    })
}

/// Detects everything under `sys`.
pub fn detect(sys: &Sys) -> Detection {
    if cfg!(target_os = "freebsd") && sys.is_real() {
        return Detection::unsupported();
    }
    let os = os(sys);
    let uefi = sys.path("/sys/firmware/efi").is_dir();
    let firmware = if uefi { Firmware::Uefi } else { Firmware::Bios };
    let secure_boot = uefi.then(|| efivar(sys, "SecureBoot", EFI_GLOBAL_GUID).and_then(|b| b.first().copied()).map(|b| b == 1)).flatten();
    let info = if uefi { loader_info(sys) } else { None };
    let current = if uefi { efibootmgr_output(sys).as_deref().and_then(boot_current) } else { None };
    let efi_file = current.as_ref().and_then(|(_, f)| f.clone());
    let grub = grub(sys, &os);
    let limine = limine(sys, efi_file.as_deref());
    let refind = refind(sys);
    let systemd_boot = systemd_boot(sys);
    let mut installed = Vec::new();
    if grub.is_some() {
        installed.push(Loader::Grub);
    }
    if limine.is_some() {
        installed.push(Loader::Limine);
    }
    if refind.is_some() {
        installed.push(Loader::Refind);
    }
    if systemd_boot {
        installed.push(Loader::SystemdBoot);
    }
    let (in_use, evidence) = if let Some(l) = info.as_deref().and_then(Loader::classify) {
        (Some(l), format!("LoaderInfo: {}", info.unwrap_or_default()))
    } else if let Some(l) = current.as_ref().and_then(|(label, file)| file.as_deref().and_then(Loader::classify).or_else(|| Loader::classify(label))) {
        let (label, file) = current.clone().unwrap_or_default();
        (Some(l), format!("BootCurrent: {label}{}", file.map(|f| format!(" ({f})")).unwrap_or_default()))
    } else if installed.len() == 1 {
        (installed.first().copied(), "the only bootloader configured here".to_string())
    } else if installed.is_empty() {
        (None, "no supported bootloader configuration found".to_string())
    } else {
        (None, format!("several bootloaders are configured ({}) and the firmware doesn't say which one started", installed.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ")))
    };
    let plymouth = plymouth(sys, limine.as_ref());
    Detection { supported: true, os, firmware, secure_boot, in_use, evidence, installed, grub, limine, refind, systemd_boot, plymouth, screen: native_screen(sys) }
}

/// [`detect`] on this machine. Blocking (reads files, runs `efibootmgr`):
/// call it off the UI thread.
///
/// Debug builds in the headless sandbox can point it at a fixture tree
/// (`MANDARA_BOOT_FIXTURE=/path`) to show another distribution's layout;
/// release builds and the root side always look at the real system.
pub fn detect_now() -> Detection {
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::var_os("MANDARA_BOOT_FIXTURE").map(PathBuf::from).filter(|p| p.is_absolute() && p.is_dir()) {
        return detect(&Sys::under(&root));
    }
    detect(&Sys::real())
}

#[cfg(test)]
pub(crate) mod tests;
