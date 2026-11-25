//! Checks a theme folder before it is installed (on the user side when it
//! is staged, and again by the root side on its own copy), and finds the
//! picture its preview shows.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::detect::{PLYMOUTH_THEMES, ini_get};

/// Largest theme description read.
const DESCRIPTION_MAX: u64 = 256 << 10;
/// Splash plugins a theme may name (the ones distributions ship).
const PLYMOUTH_MODULES: &[&str] = &["two-step", "script", "fade-throbber", "space-flares", "text", "details", "tribar"];
/// rEFInd tokens whose value is a path inside the theme.
const REFIND_PATH_TOKENS: &[&str] = &["banner", "icons_dir", "selection_big", "selection_small", "font"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Grub,
    Refind,
    Plymouth,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GrubTheme {
    pub desktop_image: Option<String>,
    /// Fonts the theme names ("DejaVu Sans Regular 14").
    pub fonts: Vec<String>,
    /// `.pf2` fonts it ships.
    pub pf2: Vec<PathBuf>,
    /// TrueType / OpenType fonts it ships without a `.pf2` (made into one
    /// with `grub-mkfont` when installed).
    pub needs_mkfont: Vec<PathBuf>,
}

fn read_small(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| format!("{} is missing", path.file_name().unwrap_or_default().to_string_lossy()))?;
    if !meta.is_file() || meta.len() > DESCRIPTION_MAX {
        return Err(format!("{} isn't a theme file", path.display()));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A relative file reference inside a theme (no root, no `..`).
fn inside(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('/') && !value.split('/').any(|c| c == "..") && !value.contains('\\')
}

fn unquote(v: &str) -> &str {
    v.trim().trim_end_matches(';').trim().trim_matches('"')
}

/// Checks a GRUB theme: `theme.txt` parses (balanced components, files it
/// names stay inside and exist), and lists its fonts.
pub fn check_grub(dir: &Path) -> Result<GrubTheme, String> {
    let text = read_small(&dir.join("theme.txt"))?;
    let mut depth = 0i32;
    let mut info = GrubTheme::default();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('+') {
            if !line.ends_with('{') && !line.contains('{') {
                return Err(format!("theme.txt line {}: a component without {{", n + 1));
            }
            depth += 1;
            continue;
        }
        if line == "}" {
            depth -= 1;
            if depth < 0 {
                return Err(format!("theme.txt line {}: an extra }}", n + 1));
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':').or_else(|| line.split_once('=')) else { continue };
        let (key, value) = (key.trim(), unquote(value));
        if key.contains("font") && !value.is_empty() {
            info.fonts.push(value.to_string());
        }
        let file_key = key.ends_with("image") || key.ends_with("pixmap_style") || key == "terminal-box" || key.starts_with("scrollbar_") && !key.ends_with("width") || key == "bar_style" || key == "highlight_style" || key == "file";
        if file_key && !value.is_empty() {
            if !inside(value) {
                return Err(format!("theme.txt line {}: {key} points outside the theme", n + 1));
            }
            if key == "desktop-image" {
                if !dir.join(value).is_file() {
                    return Err(format!("theme.txt: the desktop image {value} is missing"));
                }
                info.desktop_image = Some(value.to_string());
            }
        }
    }
    if depth != 0 {
        return Err("theme.txt: a component isn't closed".into());
    }
    let files = super::archive::files(dir);
    info.pf2 = files.iter().filter(|f| f.extension().is_some_and(|e| e == "pf2")).cloned().collect();
    if info.pf2.is_empty() {
        info.needs_mkfont = files.iter().filter(|f| f.extension().is_some_and(|e| e == "ttf" || e == "otf")).cloned().collect();
    }
    Ok(info)
}

/// The sizes `fonts` asks for (the trailing number of "Name Style 14"),
/// for `grub-mkfont -s` (16 when none is named).
pub fn font_sizes(fonts: &[String]) -> Vec<u32> {
    let mut sizes: Vec<u32> = fonts.iter().filter_map(|f| f.split_whitespace().last()?.parse().ok()).filter(|s| (6..=96).contains(s)).collect();
    if sizes.is_empty() {
        sizes.push(16);
    }
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlymouthTheme {
    pub module: String,
}

fn normalise(p: &str) -> String {
    let mut s = p.trim().to_string();
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    s.trim_end_matches('/').to_string()
}

/// Checks a Plymouth theme folder `dir` that installs as `name`: its
/// `<name>.plymouth` names a known plugin (one this system has, when
/// `modules` is given) and every path in it points into
/// `/usr/share/plymouth/themes/<name>`.
pub fn check_plymouth(dir: &Path, name: &str, modules: Option<&[String]>) -> Result<PlymouthTheme, String> {
    if !super::valid_name(name) {
        return Err(format!("{name:?} isn't a theme name Mandara installs"));
    }
    let text = read_small(&dir.join(format!("{name}.plymouth")))?;
    let module = ini_get(&text, "Plymouth Theme", "ModuleName").ok_or("the .plymouth file names no ModuleName")?;
    if !PLYMOUTH_MODULES.contains(&module.as_str()) {
        return Err(format!("unknown splash plugin {module:?}"));
    }
    if let Some(have) = modules && !have.contains(&module) {
        return Err(format!("this system has no {module} splash plugin"));
    }
    let home = format!("{PLYMOUTH_THEMES}/{name}");
    if let Some(image_dir) = ini_get(&text, &module, "ImageDir") && normalise(&image_dir) != home {
        return Err(format!("ImageDir is {image_dir}, not {home}"));
    }
    if module == "script" {
        let script = ini_get(&text, "script", "ScriptFile").ok_or("a script theme without ScriptFile")?;
        let file = normalise(&script).strip_prefix(&format!("{home}/")).map(str::to_string).ok_or_else(|| format!("ScriptFile {script} is outside {home}"))?;
        if !inside(&file) || !dir.join(&file).is_file() {
            return Err(format!("the script {file} is missing"));
        }
    }
    Ok(PlymouthTheme { module })
}

/// Checks a rEFInd theme folder installed as `themes/<name>`, included
/// through `include` (relative to the folder): every picture and font it
/// names stays inside it and exists, and it includes nothing else.
pub fn check_refind(dir: &Path, name: &str, include: &str) -> Result<(), String> {
    if !super::valid_name(name) || !inside(include) || !include.ends_with(".conf") {
        return Err(format!("{name}/{include} isn't a rEFInd theme Mandara installs"));
    }
    let text = read_small(&dir.join(include))?;
    let prefix = format!("themes/{name}/");
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let token = words.next().unwrap_or_default().to_ascii_lowercase();
        if token == "include" {
            return Err(format!("{include} line {}: a theme may not include other files", n + 1));
        }
        if REFIND_PATH_TOKENS.contains(&token.as_str()) {
            let value = words.next().unwrap_or_default();
            let rel = value.strip_prefix(&prefix).filter(|r| inside(r)).ok_or_else(|| format!("{include} line {}: {token} {value} is outside themes/{name}", n + 1))?;
            if !dir.join(rel).exists() {
                return Err(format!("{include} line {}: {value} is missing", n + 1));
            }
        }
    }
    Ok(())
}

/// The picture a theme card previews and how it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub file: PathBuf,
    /// A picture of the whole screen (a screenshot, a menu background):
    /// drawn to fill. Otherwise a splash element (a logo, an animation
    /// frame) drawn small on the dark boot screen.
    pub screen: bool,
}

/// Pictures some themes ship of themselves (Plymouth packages, many GRUB
/// themes).
const SCREENSHOTS: [&str; 2] = ["preview.png", "screenshot.png"];

/// The picture a theme card previews: a screenshot the theme ships, else
/// GRUB's desktop image, rEFInd's banner, a Plymouth theme's background,
/// watermark or middle animation frame.
pub fn preview(kind: Kind, dir: &Path) -> Option<Preview> {
    let files = super::archive::files(dir);
    let image = |f: &PathBuf| f.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"));
    let named = |n: &str| files.iter().filter(|f| f.file_name().is_some_and(|x| x.eq_ignore_ascii_case(n))).min_by_key(|f| f.components().count()).map(|f| dir.join(f));
    let screen = |file: PathBuf| Preview { file, screen: true };
    let element = |file: PathBuf| Preview { file, screen: false };
    let shot = || SCREENSHOTS.iter().find_map(|n| named(n)).map(screen);
    match kind {
        Kind::Grub => check_grub(dir).ok().and_then(|t| t.desktop_image).map(|d| dir.join(d)).filter(|p| p.is_file()).map(screen).or_else(shot).or_else(|| files.iter().find(|f| f.to_string_lossy().contains("background") && image(f)).map(|f| screen(dir.join(f)))),
        Kind::Refind => shot().or_else(|| files.iter().find(|f| f.file_stem().is_some_and(|s| s.to_string_lossy().contains("background")) && image(f)).map(|f| screen(dir.join(f)))),
        Kind::Plymouth => shot().or_else(|| named("background.png").map(screen)).or_else(|| named("watermark.png").or_else(|| named("logo.png")).map(element)).or_else(|| {
            let frames: Vec<&PathBuf> = files.iter().filter(|f| image(f) && f.file_stem().is_some_and(|s| s.to_string_lossy().chars().any(|c| c.is_ascii_digit()))).collect();
            frames.get(frames.len() / 2).map(|f| element(dir.join(f)))
        }),
    }
}

/// [`preview`]'s file.
pub fn preview_file(kind: Kind, dir: &Path) -> Option<PathBuf> {
    preview(kind, dir).map(|p| p.file)
}

/// Whether `name` is installed on this system as far as `det` tells,
/// without touching the disk: a Plymouth theme in its folder, the GRUB
/// theme in use.
pub fn on_system(det: &super::detect::Detection, kind: Kind, name: &str) -> bool {
    match kind {
        Kind::Plymouth => det.plymouth.as_ref().is_some_and(|p| p.installed.iter().any(|n| n == name)),
        Kind::Grub => grub_theme_in_use(det).is_some_and(|(n, _)| n == name),
        Kind::Refind => false,
    }
}

/// The GRUB theme in use: its folder's name and the folder (`GRUB_THEME`
/// names its `theme.txt`).
pub fn grub_theme_in_use(det: &super::detect::Detection) -> Option<(&str, &Path)> {
    let dir = Path::new(det.grub.as_ref()?.current.theme.as_deref()?).parent()?;
    Some((dir.file_name()?.to_str()?, dir))
}

/// The folder `name` is installed in on this system (not Mandara's staged
/// copy), when it can be read: Plymouth's themes folder, or the folder of
/// the GRUB theme in use. Touches the disk: call it off the UI thread.
pub fn installed_dir(det: &super::detect::Detection, kind: Kind, name: &str) -> Option<PathBuf> {
    if !on_system(det, kind, name) || name.contains('/') || name.starts_with('.') {
        return None;
    }
    let dir = match kind {
        Kind::Plymouth => det.plymouth.as_ref()?.themes_dir.join(name),
        Kind::Grub => grub_theme_in_use(det)?.1.to_path_buf(),
        Kind::Refind => return None,
    };
    std::fs::read_dir(&dir).is_ok().then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, files: &[(&str, &str)]) {
        for (f, body) in files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }

    #[test]
    fn grub_themes_are_parsed_and_contained() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &[("theme.txt", "# x\ndesktop-image: \"background.png\"\nterminal-font: \"Terminus Regular 14\"\n+ boot_menu {\n  item_font = \"Ubuntu Regular 20\"\n  selected_item_pixmap_style = \"select_*.png\"\n}\n"), ("background.png", "p"), ("terminus-14.pf2", "f")]);
        let info = check_grub(d.path()).unwrap();
        assert_eq!(info.desktop_image.as_deref(), Some("background.png"));
        assert_eq!(info.fonts, ["Terminus Regular 14", "Ubuntu Regular 20"]);
        assert_eq!(info.pf2.len(), 1);
        assert_eq!(font_sizes(&info.fonts), [14, 20]);
        assert_eq!(preview_file(Kind::Grub, d.path()), Some(d.path().join("background.png")));
        for bad in ["desktop-image: \"../../etc/shadow\"\n", "desktop-image: \"/boot/x.png\"\n", "+ boot_menu {\n", "}\n", "desktop-image: \"missing.png\"\n"] {
            write(d.path(), &[("theme.txt", bad)]);
            assert!(check_grub(d.path()).is_err(), "{bad}");
        }
        let fonts = tempfile::tempdir().unwrap();
        write(fonts.path(), &[("theme.txt", "title-text: \"\"\n"), ("Inter.ttf", "t")]);
        assert_eq!(check_grub(fonts.path()).unwrap().needs_mkfont, [PathBuf::from("Inter.ttf")]);
        assert_eq!(font_sizes(&[]), [16]);
    }

    #[test]
    fn plymouth_paths_stay_in_their_folder() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &[("rings.plymouth", "[Plymouth Theme]\nName=rings\nModuleName=script\n\n[script]\nImageDir=/usr/share/plymouth/themes/rings\nScriptFile=/usr/share/plymouth/themes/rings/rings.script\n"), ("rings.script", "x")]);
        assert_eq!(check_plymouth(d.path(), "rings", None).unwrap().module, "script");
        assert!(check_plymouth(d.path(), "rings", Some(&["two-step".to_string()])).is_err(), "a plugin this system lacks");
        assert!(check_plymouth(d.path(), "other", None).is_err());
        write(d.path(), &[("rings.plymouth", "[Plymouth Theme]\nModuleName=script\n[script]\nImageDir=/usr/share/plymouth/themes/rings\nScriptFile=/etc/evil.script\n")]);
        assert!(check_plymouth(d.path(), "rings", None).is_err());
        write(d.path(), &[("rings.plymouth", "[Plymouth Theme]\nModuleName=two-step\n[two-step]\nImageDir=/usr/share/plymouth/themes//rings/\n")]);
        assert!(check_plymouth(d.path(), "rings", None).is_ok(), "doubled and trailing slashes are the same folder");
        write(d.path(), &[("rings.plymouth", "[Plymouth Theme]\nModuleName=two-step\n[two-step]\nImageDir=/usr/share/plymouth/themes/spinner\n")]);
        assert!(check_plymouth(d.path(), "rings", None).is_err(), "another theme's folder");
        write(d.path(), &[("rings.plymouth", "[Plymouth Theme]\nModuleName=evil\n")]);
        assert!(check_plymouth(d.path(), "rings", None).is_err());
    }

    #[test]
    fn previews_prefer_a_screenshot_and_know_how_to_draw() {
        let d = tempfile::tempdir().unwrap();
        // An installed Plymouth package: a screenshot and frames in a subfolder.
        write(d.path(), &[("wolf.plymouth", "x"), ("resources/animation-00.png", "p"), ("resources/animation-01.png", "p"), ("resources/animation-02.png", "p")]);
        assert_eq!(preview(Kind::Plymouth, d.path()), Some(Preview { file: d.path().join("resources/animation-01.png"), screen: false }), "a frame, drawn small");
        write(d.path(), &[("watermark.png", "p")]);
        assert_eq!(preview(Kind::Plymouth, d.path()).unwrap(), Preview { file: d.path().join("watermark.png"), screen: false });
        write(d.path(), &[("background.png", "p")]);
        assert_eq!(preview(Kind::Plymouth, d.path()).unwrap(), Preview { file: d.path().join("background.png"), screen: true });
        write(d.path(), &[("Screenshot.png", "p")]);
        assert_eq!(preview(Kind::Plymouth, d.path()).unwrap(), Preview { file: d.path().join("Screenshot.png"), screen: true }, "the theme's own picture wins");
    }

    #[test]
    fn installed_themes_are_found_from_detection() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &[("plymouth/wolf/wolf.plymouth", "x"), ("grub/themes/vimix/theme.txt", "desktop-image: \"bg.png\"\n"), ("grub/themes/vimix/bg.png", "p")]);
        let mut det = super::super::detect::Detection::unsupported();
        det.plymouth = Some(super::super::detect::Plymouth {
            themes_dir: d.path().join("plymouth"),
            set_default: PathBuf::new(),
            current: Some("wolf".into()),
            installed: vec!["wolf".into(), "gone".into()],
            modules: Vec::new(),
            initramfs: None,
            in_initramfs: None,
            splash_on_cmdline: true,
        });
        assert!(on_system(&det, Kind::Plymouth, "wolf"));
        assert_eq!(installed_dir(&det, Kind::Plymouth, "wolf"), Some(d.path().join("plymouth/wolf")));
        assert_eq!(installed_dir(&det, Kind::Plymouth, "gone"), None, "listed but unreadable");
        assert_eq!(installed_dir(&det, Kind::Plymouth, "spinner"), None, "not installed");
        assert_eq!(installed_dir(&det, Kind::Plymouth, "../wolf"), None);
        assert_eq!(installed_dir(&det, Kind::Grub, "vimix"), None, "no GRUB here");
        let theme = d.path().join("grub/themes/vimix/theme.txt");
        det.grub = Some(super::super::detect::Grub {
            dir: d.path().join("grub"),
            cfg: PathBuf::new(),
            defaults: PathBuf::new(),
            dropin_dir: None,
            mkconfig: PathBuf::new(),
            script_check: None,
            mkfont: None,
            efi_full_config: None,
            readable: true,
            current: super::super::detect::GrubSettings { theme: Some(theme.display().to_string()), ..Default::default() },
        });
        assert_eq!(grub_theme_in_use(&det).map(|(n, _)| n), Some("vimix"));
        let dir = installed_dir(&det, Kind::Grub, "vimix").unwrap();
        assert_eq!(preview(Kind::Grub, &dir), Some(Preview { file: dir.join("bg.png"), screen: true }));
    }

    #[test]
    fn refind_themes_name_only_their_own_files() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &[("theme.conf", "hideui hints\nbanner themes/min/background.png\nicons_dir themes/min/icons\nselection_big themes/min/sel.png\n"), ("background.png", "p"), ("icons/os_arch.png", "i"), ("sel.png", "s")]);
        check_refind(d.path(), "min", "theme.conf").unwrap();
        assert_eq!(preview_file(Kind::Refind, d.path()), Some(d.path().join("background.png")));
        write(d.path(), &[("theme.conf", "banner themes/other/background.png\n")]);
        assert!(check_refind(d.path(), "min", "theme.conf").is_err());
        write(d.path(), &[("theme.conf", "include ../../refind.conf\n")]);
        assert!(check_refind(d.path(), "min", "theme.conf").is_err());
        assert!(check_refind(d.path(), "min", "../x.conf").is_err());
    }
}
