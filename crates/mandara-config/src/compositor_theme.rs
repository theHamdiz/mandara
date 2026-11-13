//! Imported compositor colors live beside the managed drop-in.

mod inline_displays;

use crate::compositor::{self, Target};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    AppColors,
    Displays,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::AppColors => "app colors",
            Self::Displays => "saved displays",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::AppColors => "app-theme",
            Self::Displays => "outputs",
        }
    }
}

pub fn supported(target: Target) -> bool {
    matches!(
        target,
        Target::Mango | Target::Niri | Target::Hyprland | Target::Umbriel | Target::Sway
    )
}

pub fn file(target: Target, kind: Kind) -> PathBuf {
    let generated = target.generated_file();
    generated.with_file_name(format!(
        "{}-{}",
        kind.prefix(),
        generated.file_name().unwrap_or_default().to_string_lossy()
    ))
}

pub fn include(target: Target, kind: Kind,path: &Path) -> String {
    let path = path.to_string_lossy();
    match target {
        Target::Mango => format!("source_optional={path}"),
        Target::Niri => compositor::niri_include_line(&path, compositor::installed_version(target)),
        Target::Hyprland if Target::hyprland_lua() => lua_include(kind,&path),
        Target::Hyprland => format!("source = {path}"),
        Target::Umbriel => format!(
            "[include.optional]\nfiles = [{}]",
            compositor::toml_str(&path)
        ),
        Target::Sway => compositor::sway_include_line(&path),
        _ => String::new(),
    }
}

fn lua_include(kind: Kind,path: &str) -> String {
    let path = serde_json::to_string(path).unwrap_or_default();
    match kind {
        Kind::AppColors => format!("local app_theme = dofile({path}); app_theme.apply_theme()"),
        Kind::Displays => format!("dofile({path})"),
    }
}

fn umbriel_remove_include(body: &str,path: &str) -> anyhow::Result<String> {
    use toml_edit::Item;
    let mut document:toml_edit::DocumentMut = body.parse()?;
    let Some(include) = document.get_mut("include").and_then(Item::as_table_like_mut) else { return Ok(body.into()) };
    let Some(optional) = include.get_mut("optional").and_then(Item::as_table_like_mut) else { return Ok(body.into()) };
    let Some(files) = optional.get_mut("files").and_then(Item::as_array_mut) else { return Ok(body.into()) };
    let before = files.len();
    files.retain(|value|value.as_str() != Some(path));
    if before == files.len() { return Ok(body.into()); }
    if files.is_empty() { optional.remove("files"); }
    if optional.is_empty() { include.remove("optional"); }
    if include.is_empty() { document.remove("include"); }
    Ok(document.to_string())
}

/// Regeneration keeps this include while its color file exists. Disabling
/// removes just this block; the rest of the generated settings stay intact.
pub fn compose(target: Target, kind: Kind, body: &str, path: &Path, enabled: bool) -> anyhow::Result<String> {
    let inline;
    let body = if enabled && kind == Kind::Displays {
        inline = inline_displays::remove(target, body, path)?;
        inline.as_str()
    } else { body };
    if target == Target::Umbriel {
        let path = path.to_string_lossy();
        return if enabled { Ok(compositor::umbriel_add_include(body,&path)?.unwrap_or_else(||body.into())) } else { umbriel_remove_include(body,&path) };
    }
    let prefix = if target == Target::Niri {
        "//"
    } else if target == Target::Hyprland && Target::hyprland_lua() {
        "--"
    } else {
        "#"
    };
    let start = format!("{prefix} Mandara Shell {}", kind.label());
    let end = format!("{prefix} End Mandara Shell {}", kind.label());
    let mut inside = false;
    let mut result = String::with_capacity(body.len());
    for line in body.lines() {
        if line == start {
            inside = true;
            continue;
        }
        if inside {
            if line == end {
                inside = false;
            }
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    if enabled && supported(target) {
        result.push_str(&start);
        result.push('\n');
        result.push_str(&include(target,kind, path));
        result.push('\n');
        result.push_str(&end);
        result.push('\n');
    }
    Ok(result)
}

pub fn lock() -> std::io::Result<std::fs::File> {
    let path = crate::paths::config_dir().join("generated/compositor.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    lock.lock()?;
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_lua_runs_directly_while_imported_color_modules_call_their_entrypoint() {
        assert_eq!(lua_include(Kind::Displays,"/tmp/displays.lua"),"dofile(\"/tmp/displays.lua\")");
        assert!(lua_include(Kind::AppColors,"/tmp/colors.lua").ends_with("app_theme.apply_theme()"));
    }
    #[test]
    fn umbriel_combines_color_and_display_includes_without_duplicate_toml_tables() {
        let original = "# own setting\n[general]\nborder_width = 3\n[include.optional]\nfiles = ['user.toml']\n";
        let colors = compose(Target::Umbriel,Kind::AppColors,original,Path::new("/colors.toml"),true).unwrap();
        let both = compose(Target::Umbriel,Kind::Displays,&colors,Path::new("/outputs.toml"),true).unwrap();
        let parsed:toml::Value = toml::from_str(&both).unwrap();
        assert_eq!(parsed["include"]["optional"]["files"].as_array().unwrap().len(),3);
        assert_eq!(both.matches("[include.optional]").count(),1);
        let retired = compose(Target::Umbriel,Kind::AppColors,&both,Path::new("/colors.toml"),false).unwrap();
        assert_eq!(compositor::umbriel_includes(&retired),["user.toml","/outputs.toml"]);
        assert!(retired.contains("# own setting"));
        assert_eq!(toml::from_str::<toml::Value>(&retired).unwrap()["general"]["border_width"].as_integer(),Some(3));
    }

    #[test]
    fn regenerated_settings_keep_one_theme_include_and_disabling_preserves_other_settings() {
        for target in [
            Target::Mango,
            Target::Niri,
            Target::Hyprland,
            Target::Umbriel,
            Target::Sway,
        ] {
            let path = Path::new("/tmp/colors");
            let first = compose(
                target,
                Kind::AppColors,
                "# user's generated settings\n",
                path,
                true,
            ).unwrap();
            let again = compose(target, Kind::AppColors, &first, path, true).unwrap();
            assert_eq!(first, again);
            assert_eq!(
                compose(target, Kind::AppColors, &again, path, false).unwrap(),
                "# user's generated settings\n"
            );
        }
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::doctor::{self,CheckEnv,CheckerStatus};
    use crate::outputs::{OutputConfig,Transform};
    use std::collections::BTreeMap;

    #[test]
    #[ignore = "set MANDARA_COMPOSITOR_TEMPLATE_FIXTURES to downloaded assets and install the native checker"]
    fn imported_colors_and_saved_displays_pass_the_native_checker_together() {
        let root = PathBuf::from(std::env::var_os("MANDARA_COMPOSITOR_TEMPLATE_FIXTURES").expect("template asset directory"));
        let requested = std::env::var("MANDARA_CHECK_COMPOSITOR").expect("one compositor name");
        let target = Target::parse(&requested).filter(|target|supported(*target)).expect("supported compositor");
        let input = match target {
            Target::Mango => "mango/mango.conf",
            Target::Niri => "niri/niri.kdl",
            Target::Hyprland if Target::hyprland_lua() => "hyprland/hyprland.lua",
            Target::Hyprland => "hyprland/hyprland.conf",
            Target::Umbriel => "umbriel/umbriel.toml",
            Target::Sway => "sway/sway",
            _ => unreachable!(),
        };
        let source = std::fs::read_to_string(root.join(input)).unwrap();
        let outputs = [OutputConfig { name:"HEADLESS-1".into(),width:Some(2560),height:Some(1440),refresh_mhz:Some(60000),scale:Some(1.25),transform:Some(Transform::Rotate90),x:Some(-120),y:Some(80),..Default::default() }];
        for dark in [true,false] {
            let render = mandara_theme::templates::custom::Renderer::new(&mandara_theme::engine::default_palette(dark),&mandara_theme::engine::default_palette(!dark),&BTreeMap::new()).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let generated = target.generated_file();
            let extension = if target == Target::Hyprland && Target::hyprland_lua() { "lua" } else { generated.extension().unwrap().to_str().unwrap() };
            let colors = dir.path().join(format!("colors.{extension}"));
            let displays = dir.path().join(format!("displays.{extension}"));
            let main = dir.path().join(format!("main.{extension}"));
            std::fs::write(&colors,render.render(&source).unwrap()).unwrap();
            std::fs::write(&displays,crate::outputs::emit_outputs(target,&outputs,target == Target::Hyprland && Target::hyprland_lua())).unwrap();
            let inline = crate::outputs::emit_outputs(target,&outputs,target == Target::Hyprland && Target::hyprland_lua());
            let mut legacy = inline.lines().filter(|line|!line.contains("End Displays")).map(|line|format!("{line}\n")).collect::<String>();
            if target == Target::Niri { legacy.push_str("output \"UNPLUGGED\" { scale 2; }\n"); }
            for original in [String::new(),inline,legacy] {
                let body = compose(target,Kind::AppColors,&original,&colors,true).unwrap();
                let body = compose(target,Kind::Displays,&body,&displays,true).unwrap();
                std::fs::write(&main,body).unwrap();
                let report = doctor::check_with(target,&main,&CheckEnv { home:Some(dir.path().into()),..Default::default() });
                assert_eq!(report.status,CheckerStatus::Ran,"{}",report.raw);
                assert_eq!(report.problems(),0,"{target:?}, dark={dark}: {}",report.raw);
            }
        }
    }
}
