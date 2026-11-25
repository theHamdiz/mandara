//! Native theme parsing without connecting to the current desktop.

use super::Extra;
use crate::app_themes::process;
use anyhow::{Context as _, Result, ensure};
use std::io::Read as _;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

/// A native checker has the same bounded diagnostic allowance as a template command.
const MAX_LOG_BYTES: u64 = 64 * 1024;
/// A private configuration creates only a handful of logs, never an unbounded scan.
const MAX_LOG_FILES: usize = 8;

pub(super) struct Native(pub Extra);

fn command(program: &str, root: &Path) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") { command.env("PATH",path); }
    command.env("HOME",root)
        .env("XDG_CONFIG_HOME",root.join("config"))
        .env("XDG_DATA_HOME",root.join("data"))
        .env("XDG_CACHE_HOME",root.join("cache"))
        .env("XDG_STATE_HOME",root.join("state"))
        .env("XDG_RUNTIME_DIR",root.join("runtime"))
        .env("DBUS_SESSION_BUS_ADDRESS",format!("unix:path={}/no-session",root.display()))
        .env("DBUS_SYSTEM_BUS_ADDRESS",format!("unix:path={}/no-system",root.display()))
        .env("WLR_BACKENDS","headless")
        .env("WLR_RENDERER","pixman")
        .env("WLR_LIBINPUT_NO_DEVICES","1")
        .env("LANG","C.UTF-8");
    command
}

fn jay_log(root: &Path) -> Result<String> {
    let mut text = String::new();
    for (index,entry) in std::fs::read_dir(root.join("data/jay/logs/jay"))?.enumerate() {
        ensure!(index < MAX_LOG_FILES,"native theme checker created too many log files");
        let entry = entry?;
        ensure!(entry.file_type()?.is_file(),"native theme checker log is not a file");
        std::fs::File::open(entry.path())?.take(MAX_LOG_BYTES.saturating_sub(text.len() as u64)+1).read_to_string(&mut text)?;
        ensure!(text.len() as u64 <= MAX_LOG_BYTES,"native theme checker log is too large");
    }
    Ok(text)
}

impl mandara_config::doctor::theme::Validator for Native {
    fn validate(&self,body: &str) -> Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::Builder::new().prefix("mdr-theme-").tempdir_in("/tmp")?;
        for directory in ["config","data","cache","state","runtime"] { std::fs::create_dir(root.path().join(directory))?; }
        std::fs::set_permissions(root.path().join("runtime"),std::fs::Permissions::from_mode(0o700))?;
        let mut native = command(self.0.id(),root.path());
        match self.0 {
            Extra::Jay => {
                let document:toml::Value = toml::from_str(body)?;
                let theme = document.get("theme").cloned().unwrap_or_else(||toml::Value::Table(Default::default()));
                let mut config = toml::map::Map::new();
                config.insert("on-startup".into(),"quit".into());
                config.insert("shortcuts".into(),toml::Value::Table([("alt-q".into(),"quit".into())].into_iter().collect()));
                config.insert("theme".into(),theme);
                std::fs::write(root.path().join("config/config.toml"),toml::to_string(&config)?)?;
                native.args(["--config-dir"]).arg(root.path().join("config")).args(["run","--backends","headless"]);
            }
            Extra::Driftwm => {
                std::fs::write(root.path().join("config/config.toml"),body)?;
                native.args(["--check-config","--config"]).arg(root.path().join("config/config.toml"));
            }
            Extra::Labwc => {
                std::fs::write(root.path().join("config/rc.xml"),"<labwc_config/>\n")?;
                std::fs::write(root.path().join("config/themerc-override"),body)?;
                native.arg("-C").arg(root.path().join("config")).args(["-S","true","-V"]);
            }
            Extra::Scroll => anyhow::bail!("scroll uses its configuration checker"),
        }
        let (_,diagnostic) = process::native(native,&AtomicBool::new(false),mandara_config::doctor::CHECK_TIMEOUT).with_context(||format!("checking {} theme",self.0.id()))?;
        match self.0 {
            Extra::Jay => {
                let log = jay_log(root.path())?;
                ensure!(log.contains("jay::config::handler] Quitting"),"the native theme checker did not load the private configuration: {log}");
                ensure!(!log.lines().any(|line|line.contains("jay_toml_config") && (line.contains("WARN") || line.contains("ERROR"))),"the native theme checker rejected the theme: {log}");
            }
            Extra::Labwc => {
                ensure!(diagnostic.contains("read theme") && diagnostic.contains("themerc-override"),"the native theme checker did not read its private theme: {diagnostic}");
                ensure!(!diagnostic.contains("[ERROR]"),"the native theme checker rejected the theme: {diagnostic}");
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_themes::{compositor::{keyed,scroll},paths::Locations};
    use mandara_theme::templates::{Ledger,custom::Renderer};

    #[test]
    #[ignore = "set MANDARA_NATIVE_COMPOSITOR and MANDARA_COMPOSITOR_TEMPLATE_FIXTURES; run with a private runtime and dead buses"]
    fn imported_themes_pass_native_parsers_and_restore_only_owned_keys() {
        let requested = std::env::var("MANDARA_NATIVE_COMPOSITOR").unwrap();
        let extra = Extra::parse(&requested).unwrap();
        let fixtures = std::path::PathBuf::from(std::env::var_os("MANDARA_COMPOSITOR_TEMPLATE_FIXTURES").unwrap());
        let relative = match extra { Extra::Jay=>"jay/jay.toml",Extra::Driftwm=>"driftwm/driftwm-colors",Extra::Labwc=>"labwc/labwc.conf",Extra::Scroll=>"scroll/scroll" };
        let source = std::fs::read_to_string(fixtures.join(relative)).unwrap();
        for dark in [true,false] {
            let root = tempfile::tempdir().unwrap();
            let locations = Locations { home:root.path().into(),config:root.path().join("config"),data:root.path().join("data"),cache:root.path().join("cache"),state:root.path().join("state") };
            let primary = if extra == Extra::Scroll { locations.config.join("scroll/config") } else { extra.primary(&locations) };
            std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
            let target = primary.with_extension("personal");
            let original = match extra {
                Extra::Jay=>"# personal\n[theme]\nborder-color = '#123456'\nborder-width = 3\n",
                Extra::Driftwm=>"# personal\nmod_key = 'alt'\n[decorations]\nbg_color = '#123456'\n",
                Extra::Labwc=>"# personal\nborder.width: 4\nwindow.active.border.color: #123456\n",
                Extra::Scroll=>"# personal\nset $mod Mod4\n",
            };
            std::fs::write(&target,original).unwrap();std::os::unix::fs::symlink(&target,&primary).unwrap();
            let renderer = Renderer::new(&mandara_theme::engine::default_palette(dark),&mandara_theme::engine::default_palette(!dark),&Default::default()).unwrap();
            let body = renderer.render(&source).unwrap();
            let mut ledger = Ledger::default();
            let owner = "test/compositor";
            if extra == Extra::Scroll { scroll::apply(owner,&body,&locations,&mut ledger).unwrap(); }
            else { keyed::apply(extra,owner,&body,&locations,&mut ledger).unwrap(); }
            assert_ne!(std::fs::read_to_string(&primary).unwrap(),original);
            assert_eq!(std::fs::read_link(&primary).unwrap(),target);
            if extra == Extra::Scroll { scroll::undo(owner,&locations).unwrap(); }
            else { keyed::undo(extra,owner,&locations).unwrap(); }
            assert!(ledger.take_back_checked(owner).errors.is_empty());
            let restored = std::fs::read_to_string(&primary).unwrap();
            assert!(restored.contains("# personal"));
            assert_eq!(std::fs::read_link(&primary).unwrap(),target);
            match extra {
                Extra::Jay|Extra::Driftwm => assert_eq!(toml::from_str::<toml::Value>(&restored).unwrap(),toml::from_str::<toml::Value>(original).unwrap()),
                _=>assert_eq!(restored,original),
            }
            assert!(!extra.generated(&locations).exists());
        }
    }
}
