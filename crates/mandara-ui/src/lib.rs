//! Mandara layer-shell views: the per-output shell surface (bar, plugin
//! panels, sidebar), keyboard overlays (launcher, session dock), shipped
//! plugins and their shared UI kit.

pub mod alerts;
pub mod apps;
pub mod backdrop;
pub mod bar;
pub mod bidi;
pub mod bt_pairing;
pub mod calendar;
pub mod cards;
pub mod corners;
/// Depth layer 3, now the `wallpaper-depth` plugin (kept under its old
/// path for the renderers).
pub use plugins::wallpaper_depth as depth;
pub mod desktop;
pub mod desktop_actions;
pub mod desktop_media;
pub mod desktop_edit;
pub mod desktop_feeds;
pub mod desktop_info;
pub mod desktop_menu;
pub mod desktop_plugins;
pub mod dismiss;
pub mod floating;
pub mod fonts;
pub mod hstrip;
pub mod hub;
pub mod icon_theme;
pub mod icons;
pub mod icon_raster;
pub mod image_cache;
pub mod installer_overlay;
pub mod keepassxc_prompt;
pub mod keyring_prompt;
pub mod kit;
pub mod launcher_ext;
pub mod launcher_place;
pub mod location;
pub mod timezone;
pub mod outputs;
pub mod lock;
pub mod night_light;
pub mod osd;
pub mod overlays;
pub mod polkit;
pub mod powerctl;
pub mod plugin_art;
pub mod plugins;
pub mod scrollbar;
pub mod shell;
pub mod snip;
pub mod state;
pub mod style;
pub mod switcher;
pub mod text_field;
pub mod theme_auto;
pub mod update_watch;
pub mod toasts;
pub mod slideshow;
pub mod wallpaper;
pub mod wallpaper_fill;
pub mod markdown;
pub mod notes_store;
pub mod productivity;
pub mod sidebar_tabs;
pub mod slider_model;
pub mod todo_lists;
pub mod recorder;
pub mod widgets;
pub mod windows;
pub mod workflow;

pub use fonts::Fonts;
pub use icons::MandaraAssets;
pub use shell::{ExclusionView, ShellView, SidebarExclusionView};
pub use state::{GlobalConfig, GlobalShell, ShellState, init_state, reload_config_from_disk, update_config};
