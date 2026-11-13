//! Compositor imports use the same checked drop-in as native settings.

mod check;
mod keyed;
mod scroll;
use keyed::Extra;
use super::paths::Locations;
use serde::{Deserialize,Serialize};
use anyhow::Result;
use mandara_config::{compositor::Target, compositor_theme};
use mandara_theme::templates::{Ledger, custom::Template};
use std::path::PathBuf;

#[derive(Clone,Copy,Debug,Serialize,Deserialize)]
#[serde(untagged)]
pub(super) enum Driver { Other(Extra),Native(Target) }

pub(super) fn target(owner: &str) -> Result<Driver> {
    let id = owner.rsplit('/').next().unwrap_or(owner);
    let target = Target::parse(id).filter(|target| compositor_theme::supported(*target));
    target.map(Driver::Native).or_else(||Extra::parse(id).map(Driver::Other)).ok_or_else(||anyhow::anyhow!("this compositor has no native app-theme adapter"))
}

pub(super) fn prepare(owner: &str, spec: &Template,locations: &Locations) -> Result<Template> {
    let target = target(owner)?;
    let mut spec = spec.clone();
    spec.pre_hook.clear();
    spec.post_hook.clear();
    spec.undo_hook.clear();
    spec.input_path_dynamic.clear();
    spec.output_path_dynamic.clear();
    spec.output_path = mandara_theme::templates::custom::Paths::One(match target {
        Driver::Native(target) => compositor_theme::file(target,compositor_theme::Kind::AppColors),
        Driver::Other(extra) => extra.generated(locations),
    });
    if matches!(target,Driver::Native(Target::Hyprland)) {
        spec.input_path = if Target::hyprland_lua() {
            "hyprland/hyprland.lua"
        } else {
            "hyprland/hyprland.conf"
        }
        .into();
    }
    Ok(spec)
}

pub(super) fn apply(owner: &str, body: &str, locations: &Locations,ledger: &mut Ledger) -> Result<Vec<PathBuf>> {
    let target = match target(owner)? {
        Driver::Native(target) => target,
        Driver::Other(Extra::Scroll) => return scroll::apply(owner,body,locations,ledger),
        Driver::Other(extra) => return keyed::apply(extra,owner,body,locations,ledger),
    };
    let original = ledger.clone();
    let result =
        crate::compositor_config::set(target, compositor_theme::Kind::AppColors, body, |path| {
            ledger.put_file(owner, path, body)?;
            Ok(())
        });
    if result.is_err() {
        *ledger = original;
    }
    result
}

pub(super) fn undo(owner: &str,driver: Driver,locations: &Locations) -> Result<()> {
    match driver {
        Driver::Native(target) => crate::compositor_config::detach(target,compositor_theme::Kind::AppColors),
        Driver::Other(Extra::Scroll) => scroll::undo(owner,locations),
        Driver::Other(extra) => keyed::undo(extra,owner,locations),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_compositor_scripts_are_replaced_by_checked_generated_files() {
        let source = Template {
            post_hook: "replace-primary-config".into(),
            undo_hook: "remove-primary-config".into(),
            ..Default::default()
        };
        let locations = Locations::current().unwrap();
        let adapted = prepare("official:mango/mango", &source,&locations).unwrap();
        assert!(adapted.post_hook.is_empty() && adapted.undo_hook.is_empty());
        assert_eq!(
            adapted.output_path.iter().next(),
            Some(&compositor_theme::file(
                Target::Mango,
                compositor_theme::Kind::AppColors
            ))
        );
        let adapted = prepare("community:jay/jay", &source,&locations).unwrap();
        assert!(adapted.post_hook.is_empty() && adapted.undo_hook.is_empty());
        assert_eq!(adapted.output_path.iter().next(),Some(&Extra::Jay.generated(&locations)));
    }
}
