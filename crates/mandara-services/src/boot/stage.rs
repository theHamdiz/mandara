//! The user side: themes are downloaded, imported or made from your image
//! into Mandara's cache (`~/.cache/mandara/boot/`), checked there, and
//! handed to the root side by path. Nothing here needs or asks for root
//! except [`run_admin`], which runs `mandara boot-admin` through `pkexec`
//! and the root-owned admin helper.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use super::catalog::{BootTheme, Copy, Layout, Variant};
use super::image::{self, Options};
use super::plan::{Approved, Staged};
use super::theme::{self, Kind};
use super::{archive, helper};

/// Largest download accepted (every catalog file is far below it).
pub const MAX_DOWNLOAD: u64 = 64 << 20;
/// The largest share of the screen height a logo takes on Limine's wallpaper.
const LIMINE_LOGO_SCALE: f32 = 0.2;
/// How deep an imported archive's theme may sit.
const FIND_DEPTH: usize = 4;

/// Where staged themes live.
#[derive(Clone, Debug)]
pub struct Env {
    pub root: PathBuf,
}

impl Env {
    pub fn current() -> Self {
        Self { root: mandara_config::paths::cache_dir().join("boot") }
    }

    pub fn under(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }

    fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    /// Staged theme folders, by kind.
    pub fn stage(&self, kind: Kind) -> PathBuf {
        self.root.join("stage").join(match kind {
            Kind::Grub => "grub",
            Kind::Refind => "refind",
            Kind::Plymouth => "plymouth",
        })
    }

    pub fn wallpaper(&self) -> PathBuf {
        self.root.join("stage/limine/wallpaper.png")
    }

    fn requests(&self) -> PathBuf {
        self.root.join("requests")
    }

    fn scratch(&self) -> PathBuf {
        self.root.join(format!("unpack-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)))
    }
}

/// Removes a scratch folder when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The staged folder of `name` (`None` when not staged yet).
pub fn staged(env: &Env, kind: Kind, name: &str) -> Option<PathBuf> {
    let dir = env.stage(kind).join(name);
    (super::valid_name(name) && dir.is_dir()).then_some(dir)
}

/// Copies one [`Copy`] of a layout from `src` (the unpacked archive).
fn copy_part(src: &Path, c: &Copy, to: &Path) -> Result<(), String> {
    let dest = if c.to.is_empty() { to.to_path_buf() } else { to.join(c.to) };
    if let Some((dir, ext)) = c.from.rsplit_once("/*.") {
        let mut n = 0;
        for e in std::fs::read_dir(src.join(dir)).map_err(|e| format!("{dir}: {e}"))?.flatten() {
            let p = e.path();
            if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file()) && p.extension().is_some_and(|x| x == ext) {
                std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
                std::fs::copy(&p, dest.join(e.file_name())).map_err(|e| e.to_string())?;
                n += 1;
            }
        }
        return if n > 0 { Ok(()) } else { Err(format!("the download has no {} files in {dir}", ext)) };
    }
    let from = src.join(c.from.trim_end_matches('/'));
    let meta = std::fs::symlink_metadata(&from).map_err(|_| format!("the download has no {}", c.from))?;
    if meta.is_dir() {
        archive::copy_tree(&from, &dest, archive::INSTALL_LIMITS)
    } else {
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&from, &dest).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Checks a staged folder as the kind it is.
pub fn check(kind: Kind, dir: &Path, name: &str, include: Option<&str>) -> Result<(), String> {
    match kind {
        Kind::Grub => theme::check_grub(dir).map(|_| ()),
        Kind::Refind => theme::check_refind(dir, name, include.unwrap_or("theme.conf")),
        Kind::Plymouth => theme::check_plymouth(dir, name, None).map(|_| ()),
    }
}

/// Moves a finished folder into the stage, replacing an older staging.
fn commit(env: &Env, kind: Kind, name: &str, built: &Path) -> Result<PathBuf, String> {
    let dest = env.stage(kind).join(name);
    std::fs::create_dir_all(env.stage(kind)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(built, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(dest)
}

/// Downloads a catalog theme (pinned SHA-256), unpacks its theme files and
/// stages it. Blocking: call off the UI thread.
pub fn stage_catalog(theme: &BootTheme, variant: &Variant, env: &Env, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Staged, String> {
    let asset = crate::cursors::catalog::Asset { url: variant.asset.url, sha256: variant.asset.sha256, size: variant.asset.size, folder: "" };
    let file = crate::cursors::install::download(&asset, &env.downloads(), &|p| progress(p.fraction() * 0.8), cancel)?;
    let scratch = Scratch(env.scratch());
    let unpacked = scratch.0.join("archive");
    let result = archive::unpack(&file, &unpacked, archive::THEME_LIMITS);
    // Variants sharing one archive download it again; the cache stays small.
    let _ = std::fs::remove_file(&file);
    result?;
    progress(0.9);
    let built = scratch.0.join("theme");
    match &variant.layout {
        Layout::Folder(f) => {
            let from = if f.is_empty() { unpacked.clone() } else { unpacked.join(f) };
            archive::copy_tree(&from, &built, archive::INSTALL_LIMITS)?;
        }
        Layout::Map(parts) => {
            std::fs::create_dir_all(&built).map_err(|e| e.to_string())?;
            for c in *parts {
                copy_part(&unpacked, c, &built)?;
            }
        }
    }
    check(theme.kind, &built, variant.install, variant.include)?;
    let dir = commit(env, theme.kind, variant.install, &built)?;
    progress(1.0);
    Ok(Staged { name: variant.install.to_string(), dir, include: variant.include.map(str::to_string) })
}

/// What an import found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Imported {
    pub kind: Kind,
    pub staged: Staged,
}

/// The theme inside `dir`: the shallowest folder with a `theme.txt`
/// (GRUB), a `<name>.plymouth` (Plymouth) or a rEFInd `theme.conf`.
fn find_theme(dir: &Path) -> Option<(Kind, PathBuf, Option<String>)> {
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..FIND_DEPTH {
        let mut next = Vec::new();
        for d in &level {
            let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if d.join("theme.txt").is_file() {
                return Some((Kind::Grub, d.clone(), None));
            }
            let has_plymouth = d.join(format!("{name}.plymouth")).is_file() || std::fs::read_dir(d).into_iter().flatten().flatten().any(|e| e.path().extension().is_some_and(|e| e == "plymouth"));
            if has_plymouth {
                return Some((Kind::Plymouth, d.clone(), None));
            }
            if d.join("theme.conf").is_file() {
                return Some((Kind::Refind, d.clone(), Some("theme.conf".into())));
            }
            let mut subs: Vec<PathBuf> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())).collect();
            subs.sort();
            next.extend(subs);
        }
        level = next;
    }
    None
}

/// A folder-name slug.
fn slug(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').chars().take(48).collect()
}

/// Imports a theme from a local archive or folder, or an `https://` URL
/// (unverified: the caller must have your explicit confirmation).
/// Blocking.
pub fn import(source: &str, env: &Env, confirmed_unverified: bool, cancel: &AtomicBool) -> Result<Imported, String> {
    let scratch = Scratch(env.scratch());
    std::fs::create_dir_all(&scratch.0).map_err(|e| e.to_string())?;
    let unpacked = scratch.0.join("archive");
    if let Some(rest) = source.strip_prefix("https://") {
        if !confirmed_unverified {
            return Err(t("This download can't be verified against a pinned checksum: confirm to import it anyway").to_string());
        }
        if rest.is_empty() || rest.contains(char::is_whitespace) {
            return Err(t("That isn't a download link").to_string());
        }
        crate::offline::check()?;
        let file = scratch.0.join("download");
        download_unpinned(source, &file, cancel)?;
        archive::unpack(&file, &unpacked, archive::THEME_LIMITS)?;
    } else if source.contains("://") {
        return Err(t("Only https:// links are accepted").to_string());
    } else {
        let path = Path::new(source);
        let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{source}: {e}"))?;
        if meta.is_dir() {
            archive::copy_tree(path, &unpacked, archive::THEME_LIMITS)?;
        } else {
            archive::unpack(path, &unpacked, archive::THEME_LIMITS)?;
        }
    }
    let (kind, dir, include) = find_theme(&unpacked).ok_or_else(|| t("No boot theme was found in it (a GRUB theme.txt, a Plymouth .plymouth file or a rEFInd theme.conf)").to_string())?;
    // A theme at the top of an archive is named after the archive.
    let origin = source.rsplit('/').find(|s| !s.is_empty()).unwrap_or("imported");
    let origin = [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz", ".tar", ".zip"].iter().find_map(|x| origin.strip_suffix(x)).unwrap_or(origin);
    let folder = if dir == unpacked { origin.to_string() } else { dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "imported".into()) };
    let name = match kind {
        Kind::Grub => format!("{}{}", super::PREFIX, slug(&folder)),
        // Plymouth and rEFInd themes name their folder in their files.
        _ => folder.clone(),
    };
    if !super::valid_name(&name) {
        return Err(tf("“{name}” isn't a theme folder name Mandara installs", &[("name", &name)]));
    }
    // A folder named "x" with "y.plymouth": Plymouth themes must match.
    if kind == Kind::Plymouth && !dir.join(format!("{name}.plymouth")).is_file() {
        return Err(t("The Plymouth theme's folder and its .plymouth file have different names").to_string());
    }
    let built = scratch.0.join("theme");
    archive::copy_tree(&dir, &built, archive::INSTALL_LIMITS)?;
    check(kind, &built, &name, include.as_deref())?;
    let dir = commit(env, kind, &name, &built)?;
    Ok(Imported { kind, staged: Staged { name, dir, include } })
}

fn download_unpinned(url: &str, to: &Path, cancel: &AtomicBool) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(std::time::Duration::from_secs(15))).timeout_recv_body(Some(std::time::Duration::from_secs(180))).https_only(true).build().into();
    let mut response = agent.get(url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    let mut reader = response.body_mut().with_config().limit(MAX_DOWNLOAD + 1).reader();
    let mut out = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 << 10];
    let mut got = 0u64;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(t("Cancelled").to_string());
        }
        let n = reader.read(&mut buf).map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > MAX_DOWNLOAD {
            return Err(t("The download is larger than expected").to_string());
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Remembers the picture "Use my image" last used (Studio offers it again).
pub fn remember_image(env: &Env, path: &Path) {
    if path.is_absolute() && std::fs::create_dir_all(&env.root).is_ok() {
        let _ = std::fs::write(env.root.join("last-image"), path.as_os_str().as_encoded_bytes());
    }
}

/// The picture "Use my image" last used, if it still exists.
pub fn last_image(env: &Env) -> Option<PathBuf> {
    let bytes = std::fs::read(env.root.join("last-image")).ok()?;
    let path = PathBuf::from(String::from_utf8(bytes).ok()?);
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// What "Use my image" staged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FromImage {
    pub splash: Staged,
    pub grub: Staged,
    pub wallpaper: PathBuf,
}

/// Makes the splash theme, the GRUB theme and the Limine wallpaper from
/// your image (`screen`: the splash's resolution; `menu`: GRUB's and
/// Limine's). Blocking.
pub fn from_image(path: &Path, opts: &Options, screen: (u32, u32), menu: (u32, u32), env: &Env) -> Result<FromImage, String> {
    let src = image::load(path)?;
    remember_image(env, path);
    let scratch = Scratch(env.scratch());
    let splash_dir = scratch.0.join(super::SPLASH_NAME);
    image::write_splash(&splash_dir, &src, opts, screen)?;
    check(Kind::Plymouth, &splash_dir, super::SPLASH_NAME, None)?;
    let grub_dir = scratch.0.join(super::GRUB_IMAGE_NAME);
    image::write_grub(&grub_dir, &src, opts, menu)?;
    check(Kind::Grub, &grub_dir, super::GRUB_IMAGE_NAME, None)?;
    // Limine draws its entries in the middle of the screen: a logo goes
    // below them, smaller (a photo still fills the screen).
    let wall = scratch.0.join("wallpaper.png");
    let wall_opts = Options { position: image::Position::Lower, scale: opts.scale.min(LIMINE_LOGO_SCALE), ..opts.clone() };
    image::write_wallpaper(&wall, &src, &wall_opts, menu)?;
    let splash = commit(env, Kind::Plymouth, super::SPLASH_NAME, &splash_dir)?;
    let grub = commit(env, Kind::Grub, super::GRUB_IMAGE_NAME, &grub_dir)?;
    let wallpaper = env.wallpaper();
    if let Some(d) = wallpaper.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&wall, &wallpaper).map_err(|e| e.to_string())?;
    Ok(FromImage { splash: Staged { name: super::SPLASH_NAME.into(), dir: splash, include: None }, grub: Staged { name: super::GRUB_IMAGE_NAME.into(), dir: grub, include: None }, wallpaper })
}

/// Writes an approved request for the root side and returns its path.
pub fn write_request(env: &Env, approved: &Approved) -> Result<PathBuf, String> {
    let dir = env.requests();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", &approved.digest[..16.min(approved.digest.len())]));
    std::fs::write(&path, serde_json::to_string_pretty(approved).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Runs `mandara boot-admin <args>` as root (one approval; the admin
/// helper is installed or refreshed first in the same approval when
/// needed) and returns its outcome. Blocking.
pub fn run_admin(args: &[&str]) -> Result<helper::Outcome, String> {
    use mandara_config::admin;
    let args: Vec<OsString> = std::iter::once(OsString::from("boot-admin")).chain(args.iter().map(OsString::from)).collect();
    let (prog, argv) = match admin::current_admin_exe() {
        Some(exe) => admin::pkexec(&exe, args),
        None => admin::setup_and_run(&std::env::current_exe().map_err(|e| e.to_string())?, args),
    };
    let out = std::process::Command::new(&prog).args(&argv).stdin(std::process::Stdio::null()).output().map_err(|e| format!("{}: {e}", prog.to_string_lossy()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let start = stdout.find('{');
    match start.and_then(|s| serde_json::from_str::<helper::Outcome>(&stdout[s..]).ok()) {
        Some(o) => Ok(o),
        // pkexec: 126 = dismissed, 127 = not authorised.
        None if matches!(out.status.code(), Some(126 | 127)) => Err(t("Not authorised: nothing was changed").to_string()),
        None => Err(format!("{}{}", stdout.trim(), String::from_utf8_lossy(&out.stderr).trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_find_the_theme_and_name_it_safely() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::under(&dir.path().join("cache"));
        let src = dir.path().join("My Cool Theme!");
        std::fs::create_dir_all(src.join("inner/icons")).unwrap();
        std::fs::write(src.join("inner/theme.txt"), "desktop-image: \"bg.png\"\n").unwrap();
        std::fs::write(src.join("inner/bg.png"), "p").unwrap();
        std::fs::write(src.join("inner/install.sh"), "rm -rf ~").unwrap();
        let cancel = AtomicBool::new(false);
        let got = import(src.to_str().unwrap(), &env, false, &cancel).unwrap();
        assert_eq!(got.kind, Kind::Grub);
        assert_eq!(got.staged.name, "mandara-inner");
        assert!(got.staged.dir.join("theme.txt").is_file() && !got.staged.dir.join("install.sh").exists());
        // A URL needs your confirmation, and only https.
        assert!(import("https://example.com/t.tar.gz", &env, false, &cancel).is_err());
        assert!(import("http://example.com/t.tar.gz", &env, true, &cancel).is_err());
        assert!(import("file:///etc", &env, true, &cancel).is_err());
        // Nothing theme-like inside.
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(import(empty.to_str().unwrap(), &env, false, &cancel).is_err());
    }

    #[test]
    fn plymouth_imports_keep_their_name_and_must_match() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::under(&dir.path().join("cache"));
        let src = dir.path().join("glow");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("glow.plymouth"), "[Plymouth Theme]\nModuleName=script\n[script]\nImageDir=/usr/share/plymouth/themes/glow\nScriptFile=/usr/share/plymouth/themes/glow/glow.script\n").unwrap();
        std::fs::write(src.join("glow.script"), "x").unwrap();
        let cancel = AtomicBool::new(false);
        let got = import(src.to_str().unwrap(), &env, false, &cancel).unwrap();
        assert_eq!((got.kind, got.staged.name.as_str()), (Kind::Plymouth, "glow"));
        let other = dir.path().join("renamed");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::copy(src.join("glow.plymouth"), other.join("glow.plymouth")).unwrap();
        assert!(import(other.to_str().unwrap(), &env, false, &cancel).is_err());
    }

    #[test]
    fn grub2_themes_layout_is_assembled_like_its_script_would() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        for (f, body) in [("r/common/terminus-16.pf2", "f"), ("r/common/makefont.sh", "x"), ("r/config/theme-1080p.txt", "desktop-image: \"background.jpg\"\n+ boot_menu {\n selected_item_pixmap_style = \"select_*.png\"\n}\n"), ("r/backgrounds/1080p/background-vimix.jpg", "j"), ("r/assets/assets-color/icons-1080p/arch.png", "i"), ("r/assets/assets-select/select-1080p/select_c.png", "s"), ("r/assets/info-1080p.png", "n")] {
            let p = src.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let parts = [
            Copy { from: "r/common/*.pf2", to: "" },
            Copy { from: "r/config/theme-1080p.txt", to: "theme.txt" },
            Copy { from: "r/backgrounds/1080p/background-vimix.jpg", to: "background.jpg" },
            Copy { from: "r/assets/assets-color/icons-1080p/", to: "icons" },
            Copy { from: "r/assets/assets-select/select-1080p/*.png", to: "" },
            Copy { from: "r/assets/info-1080p.png", to: "info.png" },
        ];
        let out = dir.path().join("out");
        for c in &parts {
            copy_part(&src, c, &out).unwrap();
        }
        assert_eq!(archive::files(&out), ["background.jpg", "icons/arch.png", "info.png", "select_c.png", "terminus-16.pf2", "theme.txt"].map(PathBuf::from));
        theme::check_grub(&out).unwrap();
        assert!(copy_part(&src, &Copy { from: "r/missing.txt", to: "x" }, &out).is_err());
    }

    /// Downloads every catalog entry from its pinned URL, checks the
    /// SHA-256 and stages it (network; run with `--ignored`).
    #[test]
    #[ignore = "downloads every catalog theme"]
    fn every_catalog_theme_downloads_verifies_and_stages() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::under(dir.path());
        let cancel = AtomicBool::new(false);
        let mut failed = Vec::new();
        for t in super::super::catalog::CATALOG {
            for v in t.variants {
                if let Err(e) = stage_catalog(t, v, &env, &|_| {}, &cancel) {
                    failed.push(format!("{}/{}: {e}", t.id, v.id));
                }
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
