//! What an apply will do, step by step: the exact files and commands,
//! shown to you before anything runs, and run as is by the root side.
//!
//! The plan is a pure function of what was detected and what you asked
//! for ([`build`]). The root side detects again with admin rights,
//! rebuilds the plan and compares its [`Plan::digest`] with the one you
//! approved: a system that looks different with admin rights (a config
//! only root can read, a changed setup) gets no changes, and the new plan
//! comes back for you to review.

use std::path::{Path, PathBuf};

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use super::detect::{Detection, Initramfs, Loader};
use super::theme::{self, Kind};

/// The GRUB drop-in Mandara owns, where `grub-mkconfig` reads drop-ins.
pub const GRUB_DROPIN: &str = "99-mandara-boot-look.cfg";
/// Limine's wallpaper, on the partition holding its config.
pub const LIMINE_WALLPAPER: &str = "mandara-boot-look/wallpaper.png";
/// Plymouth's config `plymouth-set-default-theme` writes.
pub const PLYMOUTHD_CONF: &str = "/etc/plymouth/plymouthd.conf";
/// Where `grub-mkconfig` writes before the result is checked and moved in.
pub const GRUB_NEW_SUFFIX: &str = "mandara-new";

/// A theme folder prepared on your side (in Mandara's cache).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    /// The folder name it installs as.
    pub name: String,
    /// Where it was prepared (absolute).
    pub dir: PathBuf,
    /// rEFInd: the theme file `refind.conf` includes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "kebab-case")]
pub enum SplashChoice {
    /// A theme already installed (from a package, or earlier).
    Installed { name: String },
    Staged(Staged),
}

/// Limine's look: global options (`None` leaves an option as it is).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimineStyle {
    /// A prepared PNG copied next to the config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallpaper: Option<PathBuf>,
    /// `stretched`, `centered` or `tiled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallpaper_style: Option<String>,
    /// `RRGGBB` around a centred wallpaper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branding_colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    /// `TTRRGGBB` (TT: transparency).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Eight `RRGGBB` colours: black, red, green, brown, blue, magenta, cyan, gray.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette_bright: Option<Vec<String>>,
    /// `WxH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

fn hex_ok(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `WxH` with sane bounds.
pub fn resolution_ok(s: &str) -> bool {
    s.split_once('x').is_some_and(|(w, h)| w.parse::<u32>().is_ok_and(|w| (320..=16384).contains(&w)) && h.parse::<u32>().is_ok_and(|h| (200..=16384).contains(&h)))
}

impl LimineStyle {
    /// Limine in a Mandara theme's colours: the menu text, the branding
    /// line, the help line and the terminal's sixteen colours from the
    /// palette, and the backdrop around a centred wallpaper.
    pub fn from_palette(p: &mandara_theme::MandaraPalette, wallpaper: Option<PathBuf>) -> Self {
        let hex = |c: mandara_theme::Color| c.to_hex_rgb_bare();
        let base = [p.surface_dim, p.urgent_coral, p.success_emerald, p.warning_amber, p.primary, p.tertiary, p.secondary, p.on_surface_variant];
        let mut bright: Vec<String> = base.iter().map(|c| hex(c.lighten(0.08))).collect();
        bright[0] = hex(p.surface_container_high);
        bright[7] = hex(p.on_surface);
        // Over a wallpaper the terminal is mostly see-through (the
        // wallpaper's own colour stays readable behind the text); without
        // one it is the palette's surface.
        let background = format!("{}{}", if wallpaper.is_some() { "c0" } else { "00" }, hex(p.surface_dim));
        Self {
            wallpaper_style: wallpaper.as_ref().map(|_| "stretched".to_string()),
            wallpaper,
            backdrop: Some(hex(p.surface_dim)),
            branding: None,
            branding_colour: Some(hex(p.primary)),
            help_colour: Some(hex(p.secondary)),
            foreground: Some(hex(p.on_surface)),
            background: Some(background),
            palette: Some(base.iter().map(|c| hex(*c)).collect()),
            palette_bright: Some(bright),
            resolution: None,
        }
    }

    /// The global options, checked (`wallpaper` as Limine reads it).
    pub fn options(&self) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        let colour = |name: &str, v: &Option<String>, len: usize, out: &mut Vec<(String, String)>| -> Result<(), String> {
            if let Some(v) = v {
                if !hex_ok(v, len) {
                    return Err(format!("{name}: {v:?} isn't a {len}-digit hex colour"));
                }
                out.push((name.to_string(), v.to_ascii_lowercase()));
            }
            Ok(())
        };
        if self.wallpaper.is_some() {
            out.push(("wallpaper".into(), format!("boot():/{LIMINE_WALLPAPER}")));
        }
        if let Some(s) = &self.wallpaper_style {
            if !["stretched", "centered", "tiled"].contains(&s.as_str()) {
                return Err(format!("wallpaper_style: {s:?}"));
            }
            out.push(("wallpaper_style".into(), s.clone()));
        }
        colour("backdrop", &self.backdrop, 6, &mut out)?;
        if let Some(b) = &self.branding {
            let clean: String = b.chars().filter(|c| !c.is_control()).take(64).collect();
            out.push(("interface_branding".into(), clean));
        }
        colour("interface_branding_colour", &self.branding_colour, 6, &mut out)?;
        colour("interface_help_colour", &self.help_colour, 6, &mut out)?;
        colour("term_foreground", &self.foreground, 6, &mut out)?;
        colour("term_background", &self.background, 8, &mut out)?;
        for (name, list) in [("term_palette", &self.palette), ("term_palette_bright", &self.palette_bright)] {
            if let Some(list) = list {
                if list.len() != 8 || !list.iter().all(|c| hex_ok(c, 6)) {
                    return Err(format!("{name}: eight RRGGBB colours"));
                }
                out.push((name.into(), list.iter().map(|c| c.to_ascii_lowercase()).collect::<Vec<_>>().join(";")));
            }
        }
        if let Some(r) = &self.resolution {
            if !resolution_ok(r) {
                return Err(format!("interface_resolution: {r:?}"));
            }
            out.push(("interface_resolution".into(), r.clone()));
        }
        if out.is_empty() {
            return Err("nothing to change".into());
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Request {
    /// A GRUB or rEFInd theme.
    MenuTheme {
        theme: Staged,
        /// GRUB's resolution (`WxH`), when the theme is made for one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gfxmode: Option<String>,
    },
    LimineStyle { style: LimineStyle },
    Splash { theme: SplashChoice },
}

impl Request {
    pub fn slug(&self) -> &'static str {
        match self {
            Request::MenuTheme { .. } => "menu",
            Request::LimineStyle { .. } => "limine",
            Request::Splash { .. } => "splash",
        }
    }

    /// The staged folder or file the root side copies, if any.
    pub fn staged(&self) -> Option<(&Staged, Kind)> {
        match self {
            Request::MenuTheme { theme, .. } => Some((theme, if theme.include.is_some() { Kind::Refind } else { Kind::Grub })),
            Request::Splash { theme: SplashChoice::Staged(s) } => Some((s, Kind::Plymouth)),
            _ => None,
        }
    }

    /// Field checks that need no system access.
    pub fn validate(&self) -> Result<(), String> {
        let staged = |s: &Staged| -> Result<(), String> {
            if !super::valid_name(&s.name) {
                return Err(format!("{:?} isn't a theme folder name", s.name));
            }
            if !s.dir.is_absolute() {
                return Err("the staged theme must be an absolute path".into());
            }
            if let Some(i) = &s.include && (!i.ends_with(".conf") || !super::archive::safe_relative(Path::new(i))) {
                return Err(format!("{i:?} isn't a theme file"));
            }
            Ok(())
        };
        match self {
            Request::MenuTheme { theme, gfxmode } => {
                staged(theme)?;
                if let Some(g) = gfxmode && !resolution_ok(g) {
                    return Err(format!("{g:?} isn't a resolution like 1920x1080"));
                }
                Ok(())
            }
            Request::LimineStyle { style } => {
                if let Some(w) = &style.wallpaper && !w.is_absolute() {
                    return Err("the wallpaper must be an absolute path".into());
                }
                style.options().map(|_| ())
            }
            Request::Splash { theme: SplashChoice::Installed { name } } => {
                if super::valid_name(name) { Ok(()) } else { Err(format!("{name:?} isn't a theme name")) }
            }
            Request::Splash { theme: SplashChoice::Staged(s) } => staged(s),
        }
    }
}

/// The request you approved, with its plan's digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approved {
    pub request: Request,
    pub digest: String,
}

/// One thing an apply does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case")]
pub enum Step {
    /// Run `grub-mkconfig` into a scratch file and check it, before
    /// anything changes: a system whose GRUB config can't be rebuilt now
    /// gets nothing.
    GrubPreflight { mkconfig: PathBuf, check: Option<PathBuf>, cfg: PathBuf },
    /// Copy the file into this apply's backup folder (or note it didn't exist).
    Backup { path: PathBuf },
    /// Install a staged theme folder (replacing only Mandara's own).
    InstallTheme { from: PathBuf, to: PathBuf, kind: Kind },
    /// `grub-mkfont -s <size> -o <out> <font>` for a theme that ships only
    /// a TrueType / OpenType font.
    MakeFont { tool: PathBuf, size: u32, font: PathBuf, out: PathBuf },
    /// Set keys in `/etc/default/grub` (`None`: retire the line).
    GrubKeys { file: PathBuf, keys: Vec<(String, Option<String>)> },
    /// Write Mandara's own GRUB drop-in.
    GrubDropIn { file: PathBuf, keys: Vec<(String, Option<String>)> },
    /// `grub-mkconfig -o <cfg>.mandara-new`, `grub-script-check` it, then
    /// move it over `cfg`.
    GrubConfig { mkconfig: PathBuf, check: Option<PathBuf>, cfg: PathBuf },
    LimineOptions { file: PathBuf, options: Vec<(String, String)> },
    CopyFile { from: PathBuf, to: PathBuf },
    /// `limine-enroll-config`: enrol the changed config's hash.
    LimineEnroll { tool: PathBuf },
    RefindInclude { file: PathBuf, include: String },
    /// `plymouth-set-default-theme <name>`, then read back.
    PlymouthDefault { tool: PathBuf, name: String },
    Initramfs { argv: Vec<String> },
}

impl Step {
    /// What it does, in a few words (`tk` text; display with `tr`).
    pub fn label(&self) -> &'static str {
        match self {
            Step::GrubPreflight { .. } => tk("Check that GRUB's menu can be rebuilt"),
            Step::Backup { .. } => tk("Back up"),
            Step::InstallTheme { .. } => tk("Install the theme"),
            Step::MakeFont { .. } => tk("Make a GRUB font from the theme's own font"),
            Step::GrubKeys { .. } => tk("Change GRUB settings"),
            Step::GrubDropIn { .. } => tk("Write Mandara's GRUB settings file"),
            Step::GrubConfig { .. } => tk("Rebuild GRUB's menu, checked before it replaces the old one"),
            Step::LimineOptions { .. } => tk("Change Limine's look options"),
            Step::CopyFile { .. } => tk("Copy the wallpaper"),
            Step::LimineEnroll { .. } => tk("Enrol the changed Limine config"),
            Step::RefindInclude { .. } => tk("Point rEFInd at the theme"),
            Step::PlymouthDefault { .. } => tk("Make it the boot splash"),
            Step::Initramfs { .. } => tk("Rebuild the initramfs"),
        }
    }

    /// The exact paths and commands.
    pub fn detail(&self) -> String {
        let p = |p: &Path| p.display().to_string();
        let keys = |k: &[(String, Option<String>)]| k.iter().map(|(k, v)| match v { Some(v) => format!("{k}=\"{v}\""), None => format!("{k} (retired)") }).collect::<Vec<_>>().join(", ");
        match self {
            Step::GrubPreflight { mkconfig, check, cfg } => format!("{} -o {}.{GRUB_NEW_SUFFIX}{}", p(mkconfig), p(cfg), check.as_ref().map(|c| format!(" && {} …", p(c))).unwrap_or_default()),
            Step::Backup { path } => p(path),
            Step::InstallTheme { to, .. } => p(to),
            Step::MakeFont { tool, size, font, out } => format!("{} -s {size} -o {} {}", p(tool), p(out), p(font)),
            Step::GrubKeys { file, keys: k } | Step::GrubDropIn { file, keys: k } => format!("{}: {}", p(file), keys(k)),
            Step::GrubConfig { mkconfig, check, cfg } => format!("{} -o {}.{GRUB_NEW_SUFFIX}{} → {}", p(mkconfig), p(cfg), check.as_ref().map(|c| format!(" && {} {}.{GRUB_NEW_SUFFIX}", p(c), p(cfg))).unwrap_or_default(), p(cfg)),
            Step::LimineOptions { file, options } => format!("{}: {}", p(file), options.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ")),
            Step::CopyFile { to, .. } => p(to),
            Step::LimineEnroll { tool } => p(tool),
            Step::RefindInclude { file, include } => format!("{}: include {include}", p(file)),
            Step::PlymouthDefault { tool, name } => format!("{} {name}", p(tool)),
            Step::Initramfs { argv } => argv.join(" "),
        }
    }

    /// Steps that only read (shown, but they change nothing).
    pub fn read_only(&self) -> bool {
        matches!(self, Step::GrubPreflight { .. } | Step::Backup { .. })
    }
}

/// Notes shown with a plan (`tk` text keys; display with `tr`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Note {
    GrubMenuHidden,
    SecureBootUnaffected,
    PlymouthNotInInitramfs,
    NoSplashOnCmdline,
    LimineEntryTool,
    LimineEnrolled,
}

impl Note {
    pub fn text(&self) -> &'static str {
        match self {
            Note::GrubMenuHidden => tk("GRUB's menu is hidden on this system (GRUB_TIMEOUT_STYLE=hidden or GRUB_TIMEOUT=0): the theme shows when you hold Shift or press Esc while it starts."),
            Note::SecureBootUnaffected => tk("Secure Boot is on. Themes are pictures and text, not code, so they don't affect it; Mandara never replaces GRUB, shim or any signed file."),
            Note::PlymouthNotInInitramfs => tk("Plymouth isn't in your initramfs yet (add its hook), so the splash won't show until it is."),
            Note::NoSplashOnCmdline => tk("Your kernel command line has no splash option, so the splash won't show until you add it. Mandara never edits the kernel command line."),
            Note::LimineEntryTool => tk("Your boot entries are managed by limine-entry-tool: Mandara changes only the look options above them, and the tool keeps them."),
            Note::LimineEnrolled => tk("Your Limine config's hash is enrolled, so it is enrolled again after the change."),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub loader: Option<Loader>,
    pub steps: Vec<Step>,
    pub notes: Vec<Note>,
}

impl Plan {
    /// SHA-256 of the steps: what you approve and the root side checks.
    pub fn digest(&self) -> String {
        super::sha256_hex(serde_json::to_string(&self.steps).unwrap_or_default().as_bytes())
    }
}

/// Why a plan can't be made (`tk`-style English for the CLI; Studio shows
/// it as is).
fn refuse(msg: impl Into<String>) -> Result<Plan, String> {
    Err(msg.into())
}

/// The keys a GRUB theme needs, given what is set now.
fn grub_keys(g: &super::detect::Grub, theme_txt: &Path, gfxmode: Option<&str>) -> Vec<(String, Option<String>)> {
    let mut keys = vec![("GRUB_THEME".to_string(), Some(theme_txt.display().to_string()))];
    if let Some(mode) = gfxmode {
        keys.push(("GRUB_GFXMODE".into(), Some(format!("{mode},auto"))));
    }
    // A theme needs the graphical terminal. A text console is swapped for
    // it; other outputs (a serial console) stay alongside, and the input
    // stays what it was. Restore puts every key back.
    let has_gfx = |t: &str| t.split_whitespace().any(|w| w == "gfxterm");
    let with_gfx = |t: &str| std::iter::once("gfxterm").chain(t.split_whitespace().filter(|w| *w != "console" && *w != "gfxterm")).collect::<Vec<_>>().join(" ");
    match (g.current.terminal.as_deref().filter(|t| !t.trim().is_empty()), g.current.terminal_output.as_deref()) {
        // GRUB_TERMINAL sets input and output at once.
        (Some(t), _) if !has_gfx(t) => {
            keys.push(("GRUB_TERMINAL".into(), None));
            if g.current.terminal_input.is_none() {
                keys.push(("GRUB_TERMINAL_INPUT".into(), Some(t.to_string())));
            }
            keys.push(("GRUB_TERMINAL_OUTPUT".into(), Some(with_gfx(t))));
        }
        (Some(_), _) => {}
        (None, Some(out)) if !has_gfx(out) => keys.push(("GRUB_TERMINAL_OUTPUT".into(), Some(with_gfx(out)))),
        _ => {}
    }
    keys
}

fn initramfs(p: &super::detect::Plymouth) -> Result<&Initramfs, String> {
    p.initramfs.as_ref().ok_or_else(|| "Mandara can't tell how this system rebuilds its initramfs (mkinitcpio, dracut or update-initramfs), so the splash can't be changed safely".to_string())
}

/// The plan for `req` on the detected system. `stage` is where the staged
/// theme can be read now (your folder, or the root side's own copy).
pub fn build(det: &Detection, req: &Request, stage: Option<&Path>) -> Result<Plan, String> {
    if !det.supported {
        return refuse("Boot look isn't supported on FreeBSD (its loader is out of scope)");
    }
    req.validate()?;
    let mut notes = Vec::new();
    match req {
        Request::MenuTheme { theme, gfxmode } => {
            let Some(loader) = det.menu_target() else {
                return refuse(format!("Mandara can't tell which bootloader this machine uses ({}), so nothing is changed", det.evidence));
            };
            let stage_dir = stage.unwrap_or(&theme.dir);
            match loader {
                Loader::Grub => {
                    let Some(g) = &det.grub else { return refuse("GRUB's files weren't found") };
                    if let Some(full) = &g.efi_full_config {
                        return refuse(format!("GRUB reads its menu from {} on this system (an older layout Mandara doesn't change)", full.display()));
                    }
                    if theme.include.is_some() {
                        return refuse("that is a rEFInd theme, and this machine boots with GRUB");
                    }
                    let info = theme::check_grub(stage_dir)?;
                    let to = g.dir.join("themes").join(&theme.name);
                    let mut steps = vec![Step::GrubPreflight { mkconfig: g.mkconfig.clone(), check: g.script_check.clone(), cfg: g.cfg.clone() }, Step::InstallTheme { from: theme.dir.clone(), to: to.clone(), kind: Kind::Grub }];
                    if !info.needs_mkfont.is_empty() {
                        let Some(mkfont) = &g.mkfont else { return refuse("the theme ships only a TrueType font and grub-mkfont isn't installed") };
                        for font in &info.needs_mkfont {
                            for size in theme::font_sizes(&info.fonts) {
                                let stem = font.file_stem().unwrap_or_default().to_string_lossy();
                                steps.push(Step::MakeFont { tool: mkfont.clone(), size, font: to.join(font), out: to.join(format!("{stem}-{size}.pf2")) });
                            }
                        }
                    }
                    let keys = grub_keys(g, &to.join("theme.txt"), gfxmode.as_deref());
                    match &g.dropin_dir {
                        Some(d) => {
                            let file = d.join(GRUB_DROPIN);
                            steps.push(Step::Backup { path: file.clone() });
                            steps.push(Step::GrubDropIn { file, keys });
                        }
                        None => {
                            steps.push(Step::Backup { path: g.defaults.clone() });
                            steps.push(Step::GrubKeys { file: g.defaults.clone(), keys });
                        }
                    }
                    steps.push(Step::Backup { path: g.cfg.clone() });
                    steps.push(Step::GrubConfig { mkconfig: g.mkconfig.clone(), check: g.script_check.clone(), cfg: g.cfg.clone() });
                    if g.current.menu_hidden {
                        notes.push(Note::GrubMenuHidden);
                    }
                    if det.secure_boot == Some(true) {
                        notes.push(Note::SecureBootUnaffected);
                    }
                    Ok(Plan { loader: Some(loader), steps, notes })
                }
                Loader::Refind => {
                    let Some(r) = &det.refind else { return refuse("rEFInd's files weren't found") };
                    let Some(include) = &theme.include else { return refuse("that is a GRUB theme, and this machine boots with rEFInd") };
                    theme::check_refind(stage_dir, &theme.name, include)?;
                    let to = r.dir.join("themes").join(&theme.name);
                    if det.secure_boot == Some(true) {
                        notes.push(Note::SecureBootUnaffected);
                    }
                    Ok(Plan {
                        loader: Some(loader),
                        steps: vec![Step::InstallTheme { from: theme.dir.clone(), to, kind: Kind::Refind }, Step::Backup { path: r.conf.clone() }, Step::RefindInclude { file: r.conf.clone(), include: format!("themes/{}/{include}", theme.name) }],
                        notes,
                    })
                }
                Loader::Limine => refuse("Limine has no theme folders: change its look with the Limine options instead"),
                Loader::SystemdBoot => refuse("systemd-boot has no themes or colours to change"),
            }
        }
        Request::LimineStyle { style } => {
            if det.menu_target() != Some(Loader::Limine) {
                return refuse(format!("this machine doesn't boot with Limine ({})", det.evidence));
            }
            let Some(l) = &det.limine else { return refuse("Limine's config wasn't found") };
            if let Some(shadow) = super::detect::limine_shadow(l) {
                return refuse(format!("Limine reads {} before {}, so Mandara doesn't change either", shadow.display(), l.config.display()));
            }
            let options = style.options()?;
            let mut steps = vec![Step::Backup { path: l.config.clone() }];
            if let Some(w) = &style.wallpaper {
                let to = l.esp.join(LIMINE_WALLPAPER);
                steps.push(Step::Backup { path: to.clone() });
                steps.push(Step::CopyFile { from: w.clone(), to });
            }
            steps.push(Step::LimineOptions { file: l.config.clone(), options });
            if l.enroll {
                let Some(tool) = &l.enroll_tool else { return refuse("your Limine config's hash is enrolled and limine-enroll-config isn't installed") };
                steps.push(Step::LimineEnroll { tool: tool.clone() });
                notes.push(Note::LimineEnrolled);
            }
            if l.entry_tool {
                notes.push(Note::LimineEntryTool);
            }
            Ok(Plan { loader: Some(Loader::Limine), steps, notes })
        }
        Request::Splash { theme } => {
            let Some(p) = &det.plymouth else { return refuse("Plymouth isn't installed") };
            let initramfs = initramfs(p)?;
            let mut steps = Vec::new();
            let name = match theme {
                SplashChoice::Installed { name } => {
                    if !p.installed.contains(name) {
                        return refuse(format!("the splash theme {name} isn't installed"));
                    }
                    name.clone()
                }
                SplashChoice::Staged(s) => {
                    theme::check_plymouth(stage.unwrap_or(&s.dir), &s.name, Some(&p.modules))?;
                    steps.push(Step::InstallTheme { from: s.dir.clone(), to: p.themes_dir.join(&s.name), kind: Kind::Plymouth });
                    s.name.clone()
                }
            };
            steps.push(Step::Backup { path: PathBuf::from(PLYMOUTHD_CONF) });
            steps.push(Step::PlymouthDefault { tool: p.set_default.clone(), name });
            steps.push(Step::Initramfs { argv: initramfs.argv() });
            if p.in_initramfs == Some(false) {
                notes.push(Note::PlymouthNotInInitramfs);
            }
            if !p.splash_on_cmdline {
                notes.push(Note::NoSplashOnCmdline);
            }
            Ok(Plan { loader: det.in_use, steps, notes })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
