//! Boot look: restyle the boot menu (GRUB, Limine, rEFInd) and the boot
//! splash (Plymouth) with curated open-source themes or your own SVG / PNG
//! (Studio → Themes → Boot, and `boot status|list|install|import|apply|
//! restore`, registered with `system_admin`). Opt-in: switched off,
//! Mandara reads and changes nothing about how the machine boots.
//!
//! Every change goes through `mandara_services::boot`: a plan you see
//! first (`--dry-run`), your approval, backups, the tools' own checkers
//! and one-step Restore.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mandara_i18n::{tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::boot::detect::{self, Detection, Loader};
use mandara_services::boot::image::{Options, Position, Spinner};
use mandara_services::boot::plan::{self, Approved, LimineStyle, Request, SplashChoice, Staged};
use mandara_services::boot::theme::Kind;
use mandara_services::boot::{catalog, helper, stage};
use serde_json::{Value, json};

pub const ID: &str = "boot-look";
/// Resolution used when none is given (GRUB and Limine menus, the splash).
pub const DEFAULT_SCREEN: (u32, u32) = (1920, 1080);

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Boot look"), tk("Restyle the boot menu and the boot splash with popular themes or your own image, with a plan preview, backups and one-step restore"), None).opt_in(), |_, _, _| PluginInstance::empty());
}

/// Runs `f` on a worker thread; the command resolves to its JSON.
fn job(f: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-boot".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| match v {
            Value::String(s) => s,
            other => serde_json::to_string_pretty(&other).unwrap_or_default(),
        }));
    });
    Output::Pending(rx)
}

/// `--name value` / `--flag` options after the positional words.
#[derive(Default, Debug, PartialEq)]
pub struct Flags {
    pub words: Vec<String>,
    pub values: Vec<(String, String)>,
    pub switches: Vec<String>,
}

/// Options that take a value.
const VALUED: [&str; 10] = ["--gfxmode", "--background", "--accent", "--spinner", "--scale", "--position", "--screen", "--menu", "--wallpaper", "--branding"];

pub fn flags(raw: &str) -> Result<Flags, String> {
    let mut out = Flags::default();
    let mut it = raw.split_whitespace();
    while let Some(w) = it.next() {
        if VALUED.contains(&w) {
            let v = it.next().ok_or_else(|| tf("{option} needs a value", &[("option", &w)]))?;
            out.values.push((w.to_string(), v.to_string()));
        } else if w.starts_with("--") {
            out.switches.push(w.to_string());
        } else {
            out.words.push(w.to_string());
        }
    }
    Ok(out)
}

impl Flags {
    fn get(&self, name: &str) -> Option<&str> {
        self.values.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

fn rgb(s: &str) -> Result<[u8; 3], String> {
    let s = s.trim_start_matches('#');
    let ok = s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit());
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0);
    if ok { Ok([byte(0), byte(2), byte(4)]) } else { Err(tf("{value} isn't a colour like 0b1026", &[("value", &s)])) }
}

fn size(s: &str) -> Result<(u32, u32), String> {
    s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).filter(|_| plan::resolution_ok(s)).ok_or_else(|| tf("{value} isn't a resolution like 1920x1080", &[("value", &s)]))
}

/// "Use my image" options from command flags.
pub fn image_options(f: &Flags) -> Result<Options, String> {
    let mut o = Options::default();
    if let Some(v) = f.get("--background") {
        o.background = rgb(v)?;
    }
    if let Some(v) = f.get("--accent") {
        o.accent = rgb(v)?;
    }
    if let Some(v) = f.get("--spinner") {
        o.spinner = Spinner::parse(v).ok_or_else(|| tf("{value} isn't ring, dots or none", &[("value", &v)]))?;
    }
    if let Some(v) = f.get("--scale") {
        o.scale = v.parse().map_err(|_| tf("{value} isn't a number", &[("value", &v)]))?;
    }
    if let Some(v) = f.get("--position") {
        o.position = Position::parse(v).ok_or_else(|| tf("{value} isn't center or upper", &[("value", &v)]))?;
    }
    if f.has("--fill") {
        o.fill = Some(true);
    }
    if f.has("--place") {
        o.fill = Some(false);
    }
    Ok(o.clamped())
}

/// The staged theme a spec names: a catalog id[/variant] or a staged folder
/// name.
fn staged_theme(spec: &str, kind: Kind, env: &stage::Env) -> Result<Staged, String> {
    if let Some((t, v)) = catalog::resolve(spec).filter(|(t, _)| t.kind == kind || (kind == Kind::Grub && t.kind == Kind::Refind)) {
        let k = t.kind;
        return stage::staged(env, k, v.install).map(|dir| Staged { name: v.install.to_string(), dir, include: v.include.map(str::to_string) }).ok_or_else(|| tf("{theme} isn't downloaded yet (run `mandara msg boot install {spec}`)", &[("theme", &t.name), ("spec", &spec)]));
    }
    for k in [kind, Kind::Refind] {
        if let Some(dir) = stage::staged(env, k, spec) {
            let include = (k == Kind::Refind).then(|| catalog::by_install(spec).and_then(|(_, v)| v.include).unwrap_or("theme.conf").to_string());
            return Ok(Staged { name: spec.to_string(), dir, include });
        }
    }
    Err(tf("{theme} isn't downloaded or imported (see `mandara msg boot list`)", &[("theme", &spec)]))
}

/// The request `boot apply <target> <what> [options]` asks for.
pub fn request(target: &str, f: &Flags, det: &Detection, env: &stage::Env, palette: impl Fn(&str) -> Option<mandara_theme::MandaraPalette>) -> Result<Request, String> {
    let what = f.words.first().map(String::as_str).unwrap_or_default();
    match target {
        "menu" => {
            if what.is_empty() {
                return Err(tf("Expected a theme, e.g. {example}", &[("example", &"boot apply menu vimix/2k")]));
            }
            let gfxmode = match f.get("--gfxmode") {
                Some(g) => Some(size(g).map(|_| g.to_string())?),
                None => catalog::resolve(what).and_then(|(_, v)| match v.id {
                    "2k" => Some("2560x1440".to_string()),
                    "4k" => Some("3840x2160".to_string()),
                    "1080p" => Some("1920x1080".to_string()),
                    _ => None,
                }),
            };
            let theme = staged_theme(what, Kind::Grub, env)?;
            Ok(Request::MenuTheme { gfxmode: if theme.include.is_some() { None } else { gfxmode }, theme })
        }
        "splash" => {
            if what.is_empty() {
                return Err(tf("Expected a theme, e.g. {example}", &[("example", &"boot apply splash adi1090x/rings")]));
            }
            let installed = det.plymouth.as_ref().is_some_and(|p| p.installed.iter().any(|n| n == what));
            // A theme you downloaded or made wins over a package's of the same name.
            match staged_theme(what, Kind::Plymouth, env) {
                Ok(s) => Ok(Request::Splash { theme: SplashChoice::Staged(s) }),
                Err(_) if installed => Ok(Request::Splash { theme: SplashChoice::Installed { name: what.to_string() } }),
                Err(e) => Err(e),
            }
        }
        "limine" => {
            let id = if what.is_empty() { "mandara-corniche" } else { what };
            let p = palette(id).ok_or_else(|| tf("{theme} isn't a Mandara theme (see `mandara themes`)", &[("theme", &id)]))?;
            let wallpaper = match f.get("--wallpaper") {
                Some("image") => Some(env.wallpaper()).filter(|w| w.is_file()).ok_or_else(|| tf("Make the wallpaper first: {command}", &[("command", &"mandara msg boot import <image>")]))?.into(),
                Some(path) => Some(PathBuf::from(path)),
                None => None,
            };
            let mut style = LimineStyle::from_palette(&p, wallpaper);
            if let Some(b) = f.get("--branding") {
                style.branding = Some(b.replace('_', " "));
            }
            Ok(Request::LimineStyle { style })
        }
        other => Err(tf("Unknown target {target}; expected menu, splash or limine", &[("target", &other)])),
    }
}

fn plan_json(det: &Detection, plan: &plan::Plan) -> Value {
    json!({
        "bootloader": det.in_use.map(Loader::name),
        "steps": plan.steps.iter().map(|s| json!({ "do": mandara_i18n::tr(s.label()), "detail": s.detail(), "changes": !s.read_only() })).collect::<Vec<_>>(),
        "notes": plan.notes.iter().map(|n| mandara_i18n::tr(n.text()).to_string()).collect::<Vec<_>>(),
        "digest": plan.digest(),
    })
}

/// `boot status|list|install|import|apply|restore`.
pub fn register_commands(r: &mut PluginRegistry) {
    r.command(
        ID,
        CommandSpec::new_output("boot status", tk("Which bootloader and splash this machine uses, where their files are, and the backups Boot look kept"), |_, _| {
            Ok(job(|| {
                let det = detect::detect_now();
                let backups: Vec<Value> = helper::records(&mandara_services::boot::Sys::real()).iter().map(|r| json!({ "id": r.id, "what": r.what, "state": r.state })).collect();
                Ok(json!({ "detected": det, "backups": backups }))
            }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("boot list", tk("The boot theme catalog (licence, pinned download, downloaded) and the splash themes installed here"), |a, _| {
            let only = a.get(0).map(str::to_string);
            Ok(job(move || {
                let env = stage::Env::current();
                let det = detect::detect_now();
                let kinds: &[Kind] = match only.as_deref() {
                    Some("menu") => &[Kind::Grub, Kind::Refind],
                    Some("splash") => &[Kind::Plymouth],
                    _ => &[Kind::Grub, Kind::Refind, Kind::Plymouth],
                };
                let themes: Vec<Value> = catalog::CATALOG
                    .iter()
                    .filter(|t| kinds.contains(&t.kind))
                    .map(|t| {
                        let variants: Vec<Value> = t.variants.iter().map(|v| json!({ "id": format!("{}/{}", t.id, v.id), "installs_as": v.install, "downloaded": stage::staged(&env, t.kind, v.install).is_some(), "download_bytes": v.asset.size, "url": v.asset.url, "sha256": v.asset.sha256 })).collect();
                        json!({ "id": t.id, "name": t.name, "for": t.kind, "author": t.author, "license": t.license, "homepage": t.homepage, "stars": t.stars, "variants": variants })
                    })
                    .collect();
                Ok(json!({ "bootloader": det.in_use.map(Loader::name), "catalog": themes, "splash_installed": det.plymouth.map(|p| p.installed).unwrap_or_default() }))
            }))
        })
        .arg(ArgSpec::optional("what", ArgKind::Enum, tk("menu or splash (default: both)")).choices(&["menu", "splash"])),
    );
    r.command(
        ID,
        CommandSpec::new_output("boot install", tk("Download a catalog boot theme, check its pinned SHA-256 and prepare it in Mandara's cache (nothing on the system changes until you apply it)"), |a, _| {
            let spec = a.get(0).ok_or("expected a theme id, e.g. vimix/2k (see `boot list`)")?.to_string();
            let (theme, variant) = catalog::resolve(&spec).ok_or_else(|| format!("unknown boot theme {spec:?} (see `boot list`)"))?;
            Ok(job(move || {
                let s = stage::stage_catalog(theme, variant, &stage::Env::current(), &|_| {}, &AtomicBool::new(false))?;
                Ok(json!({ "staged": s.name, "dir": s.dir, "apply": format!("mandara msg boot apply {} {spec}", if theme.kind == Kind::Plymouth { "splash" } else { "menu" }) }))
            }))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("catalog id[/variant]")))
        .example("boot install adi1090x/rings"),
    );
    r.command(
        ID,
        CommandSpec::new_output("boot import", tk("Prepare a theme from an archive, a folder or an https link (unverified: add --yes), or make a splash, a GRUB theme and a Limine wallpaper from your SVG or PNG"), |a, _| {
            let f = flags(&a.rest(0))?;
            let source = f.words.first().cloned().ok_or("expected a file, folder or https link")?;
            let is_image = ["svg", "png", "jpg", "jpeg", "webp"].iter().any(|x| source.to_ascii_lowercase().ends_with(&format!(".{x}"))) && !source.starts_with("https://");
            let yes = f.has("--yes");
            if is_image {
                let opts = image_options(&f)?;
                let screen = f.get("--screen").map(size).transpose()?.unwrap_or(DEFAULT_SCREEN);
                let menu = f.get("--menu").map(size).transpose()?.unwrap_or(DEFAULT_SCREEN);
                return Ok(job(move || {
                    let made = stage::from_image(std::path::Path::new(&source), &opts, screen, menu, &stage::Env::current())?;
                    Ok(json!({ "splash": made.splash.name, "grub": made.grub.name, "limine_wallpaper": made.wallpaper, "apply": ["mandara msg boot apply splash mandara-splash", "mandara msg boot apply menu mandara-image", "mandara msg boot apply limine <theme> --wallpaper image"] }))
                }));
            }
            Ok(job(move || {
                let got = stage::import(&source, &stage::Env::current(), yes, &AtomicBool::new(false))?;
                Ok(json!({ "staged": got.staged.name, "for": got.kind, "dir": got.staged.dir }))
            }))
        })
        .arg(ArgSpec::required("source", ArgKind::String, tk("archive, folder, https link or image, then options: --yes, --background RRGGBB, --accent RRGGBB, --spinner ring|dots|none, --scale 0.1–0.8, --position center|upper, --fill, --screen WxH, --menu WxH")))
        .example("boot import ~/Pictures/logo.svg --background 0b1026 --spinner ring"),
    );
    r.command(
        ID,
        CommandSpec::new_output("boot apply", tk("Apply a prepared boot menu theme, splash or Limine look with your approval: backs up every file, checks with the tools' own checkers and can be restored; --dry-run shows the plan only"), |a, _| {
            let target = a.get(0).ok_or("expected menu, splash or limine")?.to_string();
            let f = flags(&a.rest(1))?;
            Ok(job(move || {
                let env = stage::Env::current();
                let det = detect::detect_now();
                let req = request(&target, &f, &det, &env, |id| mandara_theme::catalog::find(id).map(|t| t.palette(true)))?;
                let plan = plan::build(&det, &req, None)?;
                if f.has("--dry-run") {
                    return Ok(plan_json(&det, &plan));
                }
                let approved = Approved { digest: plan.digest(), request: req };
                let path = stage::write_request(&env, &approved)?;
                let out = stage::run_admin(&["apply", &path.display().to_string()]);
                let _ = std::fs::remove_file(&path);
                let out = out?;
                if out.ok { Ok(json!({ "applied": true, "backup": out.backup, "message": out.message })) } else { Err(out.message) }
            }))
        })
        .arg(ArgSpec::required("target", ArgKind::Enum, tk("menu, splash or limine")).choices(&["menu", "splash", "limine"]))
        .arg(ArgSpec::optional("theme", ArgKind::String, tk("theme (catalog id[/variant], a prepared or installed name, or a Mandara theme for limine), then --dry-run, --gfxmode WxH, --wallpaper image|PATH, --branding TEXT")))
        .example("boot apply menu vimix/2k --dry-run"),
    );
    r.command(
        ID,
        CommandSpec::new_output("boot restore", tk("Put back what a Boot look apply changed (default: the latest), with your approval; --dry-run lists the files"), |a, _| {
            let f = flags(&a.rest(0))?;
            let id = f.words.first().cloned().unwrap_or_else(|| "latest".into());
            if !(id == "latest" || helper::valid_id(&id)) {
                return Err(tf("{id} isn't a backup (see `boot status`)", &[("id", &id)]));
            }
            Ok(job(move || {
                let sys = mandara_services::boot::Sys::real();
                let record = if id == "latest" { helper::latest(&sys) } else { helper::records(&sys).into_iter().find(|r| r.id == id) }.ok_or("There is nothing to restore")?;
                if f.has("--dry-run") {
                    return Ok(json!({ "backup": record.id, "what": record.what, "files": record.files.iter().map(|f| &f.path).collect::<Vec<_>>(), "folders_removed": record.dirs.iter().map(|d| &d.path).collect::<Vec<_>>(), "rebuilt": record.regenerate.iter().map(|s| s.detail()).collect::<Vec<_>>() }));
                }
                let out = stage::run_admin(&["restore", &record.id])?;
                if out.ok { Ok(json!({ "restored": out.backup, "message": out.message })) } else { Err(out.message) }
            }))
        })
        .arg(ArgSpec::optional("backup", ArgKind::String, tk("latest (default) or a backup id from `boot status`, then --dry-run")))
        .example("boot restore latest --dry-run"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_split_words_values_and_switches() {
        let f = flags("logo.svg --background 0b1026 --dry-run --spinner dots").unwrap();
        assert_eq!(f.words, ["logo.svg"]);
        assert_eq!(f.get("--background"), Some("0b1026"));
        assert!(f.has("--dry-run") && !f.has("--yes"));
        assert!(flags("--scale").is_err());
        let o = image_options(&flags("--background #101418 --scale 9 --position upper --spinner none").unwrap()).unwrap();
        assert_eq!((o.background, o.scale, o.position, o.spinner), ([0x10, 0x14, 0x18], 0.8, Position::Upper, Spinner::None));
        assert!(image_options(&flags("--background red").unwrap()).is_err());
        assert!(image_options(&flags("--spinner swirl").unwrap()).is_err());
        assert!(size("1920x1080").is_ok() && size("19x10").is_err() && size("big").is_err());
    }

    #[test]
    fn apply_requests_follow_what_is_prepared() {
        let dir = tempfile::tempdir().unwrap();
        let env = stage::Env::under(dir.path());
        let det = Detection::unsupported();
        let none = |_: &str| None;
        assert!(request("menu", &flags("vimix/2k").unwrap(), &det, &env, none).unwrap_err().contains("isn't downloaded"));
        let staged = env.stage(Kind::Grub).join("mandara-vimix-2k");
        std::fs::create_dir_all(&staged).unwrap();
        let Request::MenuTheme { theme, gfxmode } = request("menu", &flags("vimix/2k").unwrap(), &det, &env, none).unwrap() else { panic!() };
        assert_eq!((theme.name.as_str(), gfxmode.as_deref()), ("mandara-vimix-2k", Some("2560x1440")), "the theme's own resolution");
        let Request::MenuTheme { gfxmode, .. } = request("menu", &flags("mandara-vimix-2k --gfxmode 1920x1080").unwrap(), &det, &env, none).unwrap() else { panic!() };
        assert_eq!(gfxmode.as_deref(), Some("1920x1080"));
        assert!(request("limine", &flags("nope").unwrap(), &det, &env, none).is_err());
        let p = mandara_theme::catalog::builtin()[0].palette(true);
        let Request::LimineStyle { style } = request("limine", &flags("x --branding My_Machine").unwrap(), &det, &env, |_| Some(p.clone())).unwrap() else { panic!() };
        assert_eq!(style.branding.as_deref(), Some("My Machine"));
        assert!(request("kernel", &flags("x").unwrap(), &det, &env, none).is_err());
    }
}
