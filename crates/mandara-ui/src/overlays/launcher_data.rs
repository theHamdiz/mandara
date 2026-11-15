//! Inputs and snapshots for the launcher's blocking catalog reads.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use mandara_config::{IconStyle, LauncherConfig, ShellConfig, workflow::AppOverride};
use mandara_services::toplevels::Toplevel;

use crate::{
    apps,
    launcher_ext::{self, BackgroundSnapshot},
};

#[derive(Clone, PartialEq)]
pub(super) struct CatalogRequest {
    language: mandara_i18n::Lang,
    theme: Option<String>,
    monochrome: bool,
    hidden: Vec<String>,
    overrides: BTreeMap<String, AppOverride>,
    windows: Vec<String>,
    aliases: Vec<mandara_config::apps::AppIdAlias>,
}

impl CatalogRequest {
    pub(super) fn new(config: &ShellConfig, windows: &[Toplevel]) -> Self {
        let mut windows: Vec<String> = windows
            .iter()
            .filter(|w| !crate::windows::is_shell(&w.app_id))
            .map(|w| w.app_id.clone())
            .collect();
        windows.sort_unstable();
        windows.dedup();
        Self {
            language: mandara_i18n::language(),
            theme: config.appearance.icon_theme.clone(),
            monochrome: config.appearance.launcher_icons == IconStyle::Monochrome,
            hidden: config.launcher.hidden.clone(),
            overrides: config.launcher.overrides.clone(),
            windows,
            aliases: config.apps.id_map.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct Catalog {
    pub(super) apps: Vec<apps::App>,
    pub(super) symbolic: HashMap<String, gpui::SharedString>,
    pub(super) window_names: HashMap<String, String>,
    pub(super) window_icons: HashMap<String, PathBuf>,
}

pub(super) type AppCatalog = BackgroundSnapshot<CatalogRequest, Catalog>;
pub(super) type Wallpapers = BackgroundSnapshot<String, Vec<PathBuf>>;

pub(super) fn read_wallpapers(configured: String) -> Vec<PathBuf> {
    crate::wallpaper::list(&mandara_config::paths::wallpaper_dir(&configured))
}

pub(super) fn read_catalog(request: CatalogRequest) -> Catalog {
    read_catalog_from(request, apps::discover_for_theme)
}

fn read_catalog_from(request: CatalogRequest, discover: impl FnOnce(mandara_i18n::Lang, Option<&str>) -> Vec<apps::App>) -> Catalog {
    let theme = request.theme.as_deref().filter(|theme| !theme.is_empty()).map(str::to_owned)
        .or_else(crate::icon_theme::desktop_icon_theme).unwrap_or_else(|| "hicolor".into());
    let apps = launcher_ext::customize_for_theme(
        discover(request.language, Some(&theme)),
        &LauncherConfig {
            hidden: request.hidden,
            overrides: request.overrides,
            ..Default::default()
        },
        Some(&theme),
    );
    let mut catalog = Catalog {
        apps,
        ..Default::default()
    };
    if !request.monochrome && request.windows.is_empty() {
        return catalog;
    }
    // The shared entry index can be initialized by bar renders. Read our
    // own snapshot so the launcher worker never holds that cache's lock.
    let index = apps::build_entry_index();
    let theme = Some(theme.as_str());
    if request.monochrome {
        for app in &catalog.apps {
            let id = mandara_config::apps::alias_for(&request.aliases, &app.id).unwrap_or(&app.id);
            if let Some(icon) = apps::icon_from_index(&index, id, theme, true)
                && icon.symbolic
                && crate::icon_raster::svg_draws(&icon.path)
            {
                catalog.symbolic.insert(
                    app.id.clone(),
                    icon.path.to_string_lossy().into_owned().into(),
                );
            }
        }
    }
    for app_id in request.windows {
        let id = mandara_config::apps::alias_for(&request.aliases, &app_id).unwrap_or(&app_id);
        if let Some(name) = apps::name_from_index(&index, id) {
            catalog.window_names.insert(app_id.clone(), name);
        }
        if let Some(icon) = apps::icon_from_index(&index, id, theme, false) {
            catalog.window_icons.insert(app_id, icon.path);
        }
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_catalogs_resolve_base_and_override_icons_from_their_own_theme() {
        let root = tempfile::tempdir().unwrap();
        let applications = root.path().join("applications");
        std::fs::create_dir(&applications).unwrap();
        std::fs::write(applications.join("demo.desktop"), "[Desktop Entry]\nType=Application\nName=Demo\nExec=true\nIcon=base\n").unwrap();
        for theme in ["a", "b"] {
            let directory = root.path().join(theme);
            std::fs::create_dir_all(directory.join("48x48/apps")).unwrap();
            std::fs::write(directory.join("index.theme"), "[Icon Theme]\nName=Fixture\nDirectories=48x48/apps\n").unwrap();
            for icon in ["base", "override"] { std::fs::write(directory.join("48x48/apps").join(format!("{icon}.png")), b"icon").unwrap(); }
        }
        for icon in ["base", "override"] {
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                let jobs: Vec<_> = ["a", "b"].into_iter().map(|name| {
                    let barrier = &barrier;
                    let root = root.path();
                    scope.spawn(move || {
                        let theme = root.join(name);
                        let mut config = ShellConfig::default();
                        config.appearance.icon_theme = Some(theme.to_string_lossy().into_owned());
                        if icon == "override" { config.launcher.overrides.insert("demo".into(), mandara_config::workflow::AppOverride { name: String::new(), icon: icon.into() }); }
                        let request = CatalogRequest::new(&config, &[]);
                        barrier.wait();
                        let catalog = read_catalog_from(request, |language, theme| apps::discover_in_for(&[root.to_path_buf()], language, theme));
                        assert_eq!(catalog.apps[0].icon, Some(theme.join("48x48/apps").join(format!("{icon}.png"))));
                    })
                }).collect();
                for job in jobs { job.join().unwrap(); }
            });
        }
    }

    #[test]
    fn alias_edits_invalidate_catalogs_and_keep_each_requests_snapshot() {
        let mut config = ShellConfig::default();
        config.apps.id_map.push(mandara_config::apps::AppIdAlias { pattern: "steam_app_*".into(), desktop: "steam.desktop".into() });
        let before = CatalogRequest::new(&config, &[]);
        config.apps.id_map[0].desktop = "other.desktop".into();
        let after = CatalogRequest::new(&config, &[]);
        assert!(before != after);
        assert_eq!(mandara_config::apps::alias_for(&before.aliases, "steam_app_12"), Some("steam"));
        assert_eq!(mandara_config::apps::alias_for(&after.aliases, "steam_app_12"), Some("other"));
    }
}
