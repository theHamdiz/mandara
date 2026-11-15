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
    let apps = launcher_ext::customize(
        apps::discover_for(request.language),
        &LauncherConfig {
            hidden: request.hidden,
            overrides: request.overrides,
            ..Default::default()
        },
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
    let theme = request.theme.as_deref().filter(|theme| !theme.is_empty());
    if request.monochrome {
        for app in &catalog.apps {
            let id = apps::aliased(&app.id);
            if let Some(icon) = apps::icon_from_index(&index, &id, theme, true)
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
        let id = apps::aliased(&app_id);
        if let Some(name) = apps::name_from_index(&index, &id) {
            catalog.window_names.insert(app_id.clone(), name);
        }
        if let Some(icon) = apps::icon_from_index(&index, &id, theme, false) {
            catalog.window_icons.insert(app_id, icon.path);
        }
    }
    catalog
}
