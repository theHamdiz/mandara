//! Shipped plugins. Each module registers a manifest, a builder for its bar
//! item / panel / sidebar card, and its documented commands.

pub mod app_installer;
pub mod audio;
pub mod bluetooth;
pub mod clipboard;
pub mod clock;
pub mod keyboard;
pub mod language;
pub mod lifecycle;
mod settings_cache;
pub mod compositor;
pub mod containers;
pub mod cursors;
pub mod core_commands;
pub mod displays;
pub mod drives;
pub mod power;
pub mod services;
pub mod updates;
pub mod external;
pub mod main_menu;
pub mod media;
pub mod mini_stats;
pub mod network;
pub mod notes;
pub mod notifications;
pub mod panel_fx;
pub mod pomodoro;
pub mod privacy;
pub mod prayer;
pub mod processes;
pub mod quick_settings;
pub mod resources;
pub mod screenshot;
pub mod screen_time;
pub mod todo;
pub mod tray;
pub mod visualizer;
pub mod diagnostics;
pub mod dock;
pub mod wallpaper;
pub mod wallpaper_generator;
pub mod wallpaper_depth;
pub mod wallhaven;
pub mod weather;
pub mod session;
pub mod window;
pub mod workspaces;
pub mod extras;
pub mod world_clock;
pub mod net_speed;
pub mod screen_pick;
pub mod color_picker;
pub mod authenticator;
pub mod soundscapes;
pub mod hadith;
pub mod jannah;
pub mod quran;
pub mod system_admin;
pub mod ports;
pub mod git_repos;
pub mod ci;
pub mod crashes;
pub mod tailscale;
pub mod local_ai;
pub mod uptime;
pub mod share;
pub mod keybinds;
pub mod progress;
pub mod breaks;
pub mod presentation;
pub mod plugin_lang;
pub mod feeds;
pub mod home_assistant;
pub mod phone_connect;
pub mod translator;
mod integration;
pub mod adhan;

mod view;
pub use view::{FnView, domain_view, domain_view_observing, fn_view, fn_view_observing, model_view};

use gpui::{AnyView, App};
use mandara_plugins::PluginRegistry;

/// Re-renders `view` whenever `source` notifies — for live data kept outside
/// the hub (e.g. [`crate::hub::resources_entity`]) so only the views that
/// show it redraw on each sample.
pub fn follow<T: 'static>(view: &AnyView, source: &gpui::Entity<T>, cx: &mut App) {
    let id = view.entity_id();
    cx.observe(source, move |_, cx| cx.notify(id)).detach();
}

/// Registers every shipped plugin and the core shell commands.
pub fn register_builtin(registry: &mut PluginRegistry) {
    core_commands::register(registry);
    crate::sidebar_tabs::register(registry);
    diagnostics::register(registry);
    compositor::register(registry);
    session::register(registry);
    crate::lock::register(registry);
    crate::powerctl::register(registry);
    crate::polkit::register(registry);
    crate::keyring_prompt::register(registry);
    crate::keepassxc_prompt::register(registry);
    workspaces::register(registry);
    feeds::register(registry);
    home_assistant::register(registry);
    phone_connect::register(registry);
    translator::register(registry);
    window::register(registry);
    clock::register(registry);
    crate::calendar::register(registry);
    crate::location::register(registry);
    crate::timezone::register(registry);
    keyboard::register(registry);
    language::register(registry);
    media::register(registry);
    prayer::register(registry);
    resources::register(registry);
    mini_stats::register(registry);
    processes::register(registry);
    drives::register(registry);
    containers::register(registry);
    services::register(registry);
    power::register(registry);
    updates::register(registry);
    network::register(registry);
    bluetooth::register(registry);
    system_admin::register(registry);
    cursors::register(registry);
    crate::bt_pairing::register(registry);
    audio::register(registry);
    quick_settings::register(registry);
    screenshot::register(registry);
    clipboard::register(registry);
    main_menu::register(registry);
    notifications::register(registry);
    privacy::register(registry);
    crate::alerts::register(registry);
    todo::register(registry);
    notes::register(registry);
    pomodoro::register(registry);
    weather::register(registry);
    wallpaper::register(registry);
    wallpaper_generator::register(registry);
    wallpaper_depth::register(registry);
    wallhaven::register(registry);
    crate::slideshow::register(registry);
    crate::desktop_edit::register(registry);
    crate::desktop_plugins::register(registry);
    tray::register(registry);
    world_clock::register(registry);
    net_speed::register(registry);
    color_picker::register(registry);
    authenticator::register(registry);
    soundscapes::register(registry);
    tailscale::register(registry);
    local_ai::register(registry);
    uptime::register(registry);
    share::register(registry);
    jannah::register(registry);
    hadith::register(registry);
    quran::register(registry);
    crate::osd::register(registry);
    crate::corners::register(registry);
    crate::night_light::register(registry);
    displays::register(registry);
    // Launcher personalization and window commands (Mandara workflow).
    crate::launcher_ext::register(registry);
    crate::windows::register(registry);
    crate::switcher::register(registry);
    dock::register(registry);
    app_installer::register(registry);
    screen_time::register(registry);
    ports::register(registry);
    git_repos::register(registry);
    ci::register(registry);
    crashes::register(registry);
    keybinds::register(registry);
    progress::register(registry);
    breaks::register(registry);
    presentation::register(registry);
    // Third-party / development plugins from the plugin directories.
    external::register(registry);
}

pub fn hub_network(cx: &App) -> Option<mandara_services::NetworkService> {
    crate::hub::hub(cx).read(cx).network_service.clone()
}

pub fn hub_bluetooth(cx: &App) -> Option<mandara_services::BluetoothService> {
    crate::hub::hub(cx).read(cx).bluetooth_service.clone()
}

/// JSON helper for command results.
pub fn json(value: impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
