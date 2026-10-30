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
pub mod system_admin;

use std::time::Duration;

use gpui::{AnyElement, AnyView, App, AppContext as _, Context, IntoElement, Render, Window};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;

/// A view whose content is produced by a closure. It re-renders whenever the
/// hub, theme, config or shell state changes, plus on an optional timer.
pub struct FnView {
    render: Box<dyn Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement>,
    /// Set once the render closure panicked (it is never called again).
    failed: bool,
    /// Where the view was built (`media.rs:629`), for `diagnostics views`.
    label: &'static std::panic::Location<'static>,
}

impl FnView {
    #[track_caller]
    pub fn new(
        tick: Option<Duration>,
        render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hub = crate::hub::hub(cx);
        cx.observe(&hub, |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&crate::state::shell(cx), |_, _, cx| cx.notify()).detach();
        if let Some(p) = cx.try_global::<crate::productivity::GlobalProductivity>().map(|g| g.0.clone()) {
            cx.observe(&p, |_, _, cx| cx.notify()).detach();
        }
        if let Some(r) = crate::recorder::recorder(cx) {
            cx.observe(&r, |_, _, cx| cx.notify()).detach();
        }
        if let Some(every) = tick {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(every).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let label = std::panic::Location::caller();
        Self { render: Box::new(render), failed: false, label }
    }
}

impl Render for FnView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        use mandara_core::ToGpui as _;
        let palette = mandara_core::frame_palette(window, cx);
        if !self.failed {
            let started = std::time::Instant::now();
            let rendered = mandara_plugins::guard("plugin view", || (self.render)(&palette, window, cx));
            diagnostics::record_view(self.label, started.elapsed());
            match rendered {
                Some(el) => return el,
                // Stop calling a view that panicked; show a quiet marker.
                None => self.failed = true,
            }
        }
        gpui::div()
            .flex()
            .items_center()
            .gap(gpui::px(4.0))
            .text_size(gpui::px(11.0))
            .text_color(palette.urgent_coral.hsla())
            .child(crate::icons::icon("close", 12.0, palette.urgent_coral))
            .child(mandara_i18n::t("plugin error"))
            .into_any_element()
    }
}

/// Convenience: builds an [`FnView`] as an `AnyView`.
#[track_caller]
pub fn fn_view(
    tick: Option<Duration>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let label = std::panic::Location::caller();
    cx.new(|cx| {
        let mut view = FnView::new(tick, render, window, cx);
        view.label = label;
        view
    })
    .into()
}

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
    wallhaven::register(registry);
    crate::slideshow::register(registry);
    crate::desktop_edit::register(registry);
    tray::register(registry);
    world_clock::register(registry);
    net_speed::register(registry);
    color_picker::register(registry);
    authenticator::register(registry);
    soundscapes::register(registry);
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
