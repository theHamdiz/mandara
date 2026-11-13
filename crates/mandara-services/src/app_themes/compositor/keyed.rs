//! Theme-key ownership for compositors that do not read generated includes.

use super::check::Native;
use crate::app_themes::{paths::Locations,preferences::{self,Format,Sections}};
use anyhow::{Result,ensure};
use mandara_theme::templates::Ledger;
use serde::{Deserialize,Serialize};
use std::path::PathBuf;

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="lowercase")]
pub enum Extra { Jay,Driftwm,Labwc,Scroll }

impl Extra {
    pub fn parse(id: &str) -> Option<Self> { match id { "jay"=>Some(Self::Jay),"driftwm"=>Some(Self::Driftwm),"labwc"=>Some(Self::Labwc),"scroll"=>Some(Self::Scroll),_=>None } }
    pub fn id(self) -> &'static str { match self { Self::Jay=>"jay",Self::Driftwm=>"driftwm",Self::Labwc=>"labwc",Self::Scroll=>"scroll" } }
    pub(super) fn format(self) -> Format { if self == Self::Labwc { Format::Xresources } else { Format::Toml } }
    pub(super) fn primary(self,locations: &Locations) -> PathBuf {
        locations.config.join(self.id()).join(if self == Self::Labwc { "themerc-override" } else { "config.toml" })
    }
    pub fn generated(self,locations: &Locations) -> PathBuf {
        locations.config.join("mandara/generated").join(format!("app-theme-{}.{}",self.id(),if matches!(self,Self::Jay|Self::Driftwm) { "toml" } else { "conf" }))
    }
}

// Only these public native color keys are supplied by the imported integrations.
const JAY_COLORS: &[&str] = &["border-color","focused-border-color","focused-title-bg-color","focused-title-text-color","unfocused-title-bg-color","unfocused-title-text-color","focused-inactive-title-bg-color","focused-inactive-title-text-color","attention-requested-bg-color","separator-color","highlight-color"];
const DRIFT_COLORS: &[&str] = &["bg_color","fg_color","border_color","border_color_focused"];
const LABWC_COLORS: &[&str] = &["window.active.border.color","window.inactive.border.color","window.active.indicator.toggled-keybind.color","window.active.title.bg.color","window.inactive.title.bg.color","window.active.label.text.color","window.inactive.label.text.color","window.button.hover.bg.color","window.active.button.unpressed.image.color","window.inactive.button.unpressed.image.color","menu.border.color","menu.items.bg.color","menu.items.text.color","menu.items.active.bg.color","menu.items.active.text.color","menu.separator.color","menu.title.bg.color","menu.title.text.color","osd.bg.color","osd.border.color","osd.label.text.color","osd.window-switcher.preview.border.color"];

fn color(value: &str) -> Result<()> {
    ensure!(mandara_theme::Color::from_hex(value).is_ok(),"invalid compositor theme color: {value}");
    Ok(())
}

pub(super) fn keys(extra: Extra,body: &str) -> Result<Sections> {
    let mut keys = Sections::new();
    if extra == Extra::Labwc {
        let group = keys.entry(String::new()).or_default();
        for line in body.lines().map(str::trim).filter(|line|!line.is_empty() && !line.starts_with('#')) {
            let (key,value) = line.split_once(':').ok_or_else(||anyhow::anyhow!("invalid compositor theme line"))?;
            let (key,value) = (key.trim(),value.trim());
            if key == "window.label.text.justify" { ensure!(["left","center","right"].contains(&value),"invalid title alignment"); }
            else { ensure!(LABWC_COLORS.contains(&key),"unsupported compositor theme key: {key}");color(value)?; }
            ensure!(group.insert(key.into(),value.into()).is_none(),"duplicate compositor theme key: {key}");
        }
    } else {
        let document:toml::Table = toml::from_str(body)?;
        let (section,values,allowed) = if extra == Extra::Jay {
            ensure!(document.len() == 1,"only the compositor theme table can be changed");
            ("theme",document.get("theme").and_then(toml::Value::as_table).ok_or_else(||anyhow::anyhow!("missing compositor theme table"))?,JAY_COLORS)
        } else {
            ensure!(extra == Extra::Driftwm,"this compositor uses an include");
            // Older catalogs supplied these four decoration colors at the root.
            let values = if let Some(group) = document.get("decorations") {
                ensure!(document.len() == 1,"only compositor theme colors can be changed");
                group.as_table().ok_or_else(||anyhow::anyhow!("compositor decorations are not a table"))?
            } else { &document };
            ("decorations",values,DRIFT_COLORS)
        };
        let group = keys.entry(section.into()).or_default();
        for (key,value) in values {
            ensure!(allowed.contains(&key.as_str()),"unsupported compositor theme key: {key}");
            let value = value.as_str().ok_or_else(||anyhow::anyhow!("compositor theme color is not a string"))?;
            color(value)?;group.insert(key.clone(),value.into());
        }
    }
    ensure!(keys.values().any(|group|!group.is_empty()),"compositor theme has no colors");
    Ok(keys)
}

fn reload(extra: Extra) -> Result<()> {
    if extra == Extra::Labwc && mandara_compositor::connect().is_some_and(|backend|backend.target() == mandara_config::compositor::Target::Labwc) {
        let mut command = std::process::Command::new("labwc");command.arg("--reconfigure");
        crate::app_themes::process::native(command,&std::sync::atomic::AtomicBool::new(false),crate::app_themes::process::TIMEOUT)?;
    }
    Ok(())
}

pub(super) fn apply(extra: Extra,owner: &str,body: &str,locations: &Locations,ledger: &mut Ledger) -> Result<Vec<PathBuf>> {
    let keys = keys(extra,body)?;
    let generated = extra.generated(locations);
    ledger.put_file(owner,&generated,body)?;
    let primary = extra.primary(locations);
    let validator = Native(extra);
    crate::outputs::reload_preserving(&[],true,crate::outputs::current_revision(),|| {
        preferences::set_with(owner,&primary,&keys,extra.format(),locations,&|path,original,next|mandara_config::doctor::theme::guarded_write(path,original,next,&validator)).map_err(|error|format!("{error:#}"))?;
        reload(extra).map_err(|error|format!("{error:#}"))
    }).map_err(anyhow::Error::msg)?;
    Ok(vec![generated,primary])
}

pub(super) fn undo(extra: Extra,owner: &str,locations: &Locations) -> Result<()> {
    let validator = Native(extra);
    crate::outputs::reload_preserving(&[],true,crate::outputs::current_revision(),|| {
        preferences::undo_with(owner,locations,&|path,original,next|mandara_config::doctor::theme::guarded_write(path,original,next,&validator)).map_err(|error|format!("{error:#}"))?;
        reload(extra).map_err(|error|format!("{error:#}"))
    }).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_generated_file_conflict_is_reported_before_any_primary_preference_edit() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations { home: root.path().into(), config: root.path().join("config"), data: root.path().join("data"), cache: root.path().join("cache"), state: root.path().join("state") };
        let extra = Extra::Labwc; let primary = extra.primary(&locations);
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::write(&primary, "window.active.border.color: #123456\n").unwrap();
        let mut ledger = Ledger::default();
        ledger.put_file("another", &extra.generated(&locations), "existing").unwrap();
        assert!(apply(extra, "new", "window.active.border.color: #abcdef\n", &locations, &mut ledger).is_err());
        assert_eq!(std::fs::read_to_string(primary).unwrap(), "window.active.border.color: #123456\n");
    }

    #[test]
    fn imports_can_only_change_known_theme_keys() {
        assert!(keys(Extra::Jay,"[theme]\nborder-color='#123456'\n").is_ok());
        assert!(keys(Extra::Jay,"on-startup='quit'\n[theme]\nborder-color='#123456'\n").is_err());
        assert_eq!(keys(Extra::Driftwm,"bg_color='#123456'\n").unwrap()["decorations"]["bg_color"],"#123456");
        assert!(keys(Extra::Driftwm,"bg_color='#123456'\nmod_key='alt'\n").is_err());
        assert!(keys(Extra::Labwc,"window.active.border.color: #abcdef\nwindow.label.text.justify: center\n").is_ok());
        assert!(keys(Extra::Labwc,"window.active.border.color: invalid\n").is_err());
        assert!(keys(Extra::Labwc,"border.width: 12\n").is_err());
    }
}
