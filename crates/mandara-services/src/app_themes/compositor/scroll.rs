//! Scroll's color include uses its own Sway-compatible checker.

use super::Extra;
use crate::app_themes::paths::Locations;
use anyhow::{Result,ensure};
use mandara_config::{compositor::Target,compositor_theme::{self,Kind},doctor::{self,CheckEnv,CheckerStatus}};
use mandara_theme::templates::{Ledger,Placement};
use std::path::{Path,PathBuf};

struct Native<'a> { primary: &'a Path,home: &'a Path }
impl doctor::theme::Validator for Native<'_> {
    fn check_live(&self, path: &Path) -> doctor::theme::LiveCheck {
        doctor::check_with(Target::Sway, path, &CheckEnv {
            home: Some(self.home.into()),
            argv: Some(["scroll", "--validate", "--verbose", "--config", doctor::CONFIG_PLACEHOLDER].map(Into::into).into()),
            ..Default::default()
        }).into()
    }

    fn validate(&self,body: &str) -> Result<()> {
        let directory = self.primary.parent().ok_or_else(||anyhow::anyhow!("compositor configuration has no directory"))?;
        let file = tempfile::Builder::new().prefix(".mandara-check-").suffix(".conf").tempfile_in(directory)?;
        std::fs::write(file.path(),body)?;
        let report = doctor::check_with(Target::Sway,file.path(),&CheckEnv {
            home:Some(self.home.into()),
            argv:Some(["scroll","--validate","--verbose","--config",doctor::CONFIG_PLACEHOLDER].map(Into::into).into()),
            ..Default::default()
        });
        ensure!(report.status == CheckerStatus::Ran && report.problems() == 0,"the compositor rejected its color include: {}",report.raw);
        Ok(())
    }
}

fn primary(locations: &Locations) -> PathBuf { locations.config.join("scroll/config") }

fn block(path: &Path) -> [String;3] {
    ["# Mandara Shell app colors".into(),compositor_theme::include(Target::Sway,Kind::AppColors,path),"# End Mandara Shell app colors".into()]
}

fn reload() -> Result<()> {
    if std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop|desktop.split(':').any(|name|name.eq_ignore_ascii_case("scroll"))) {
        let mut command = std::process::Command::new("scrollmsg");command.arg("reload");
        crate::app_themes::process::native(command,&std::sync::atomic::AtomicBool::new(false),crate::app_themes::process::TIMEOUT)?;
    }
    Ok(())
}

pub(super) fn apply(owner: &str,body: &str,locations: &Locations,ledger: &mut Ledger) -> Result<Vec<PathBuf>> {
    use doctor::theme::Validator as _;
    let primary = primary(locations);
    let original = super::super::apply::read_text(&primary)?;
    let validator = Native { primary:&primary,home:&locations.home };
    validator.validate(body)?;
    let generated = Extra::Scroll.generated(locations);
    ledger.put_file(owner,&generated,body)?;
    let next = compositor_theme::compose(Target::Sway,Kind::AppColors,&original,&generated,true)?;
    crate::outputs::reload_preserving(&[],true,crate::outputs::current_revision(),|| {
        if next != original {
            doctor::fix::backup_files(Target::Sway,std::slice::from_ref(&primary),&doctor::fix::backups_root()).map_err(|error|format!("{error:#}"))?;
            doctor::theme::guarded_write(&primary,&original,&next,&validator).map_err(|error|format!("{error:#}"))?;
        }
        let lines = block(&generated);
        ledger.put_block(owner,&primary,&lines.iter().map(String::as_str).collect::<Vec<_>>(),Placement::End).map_err(|error|format!("{error:#}"))?;
        reload().map_err(|error|format!("{error:#}"))
    }).map_err(anyhow::Error::msg)?;
    Ok(vec![generated,primary])
}

pub(super) fn undo(_owner: &str,locations: &Locations) -> Result<()> {
    let primary = primary(locations);
    if !primary.try_exists()? { return Ok(()); }
    let original = super::super::apply::read_text(&primary)?;
    let generated = Extra::Scroll.generated(locations);
    let next = compositor_theme::compose(Target::Sway,Kind::AppColors,&original,&generated,false)?;
    let include = compositor_theme::include(Target::Sway,Kind::AppColors,&generated);
    let next = next.lines().filter(|line|line.trim() != include).fold(String::with_capacity(next.len()),|mut body,line| { body.push_str(line);body.push('\n');body });
    crate::outputs::reload_preserving(&[],true,crate::outputs::current_revision(),|| {
        if next != original {
            doctor::theme::guarded_write(&primary,&original,&next,&Native { primary:&primary,home:&locations.home }).map_err(|error|format!("{error:#}"))?;
        }
        reload().map_err(|error|format!("{error:#}"))
    }).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn include_paths_are_escaped_and_undo_markers_match_the_shared_composer() {
        let path = Path::new("/tmp/a b/colors.conf");
        let lines = block(path);
        let body = compositor_theme::compose(Target::Sway,Kind::AppColors,"set $mod Mod4\n",path,true).unwrap();
        assert!(body.contains(&lines.join("\n")));
        assert!(lines[1].contains(r"/tmp/a\ b/colors.conf"));
        assert_eq!(compositor_theme::compose(Target::Sway,Kind::AppColors,&body,path,false).unwrap(),"set $mod Mod4\n");
    }
}
