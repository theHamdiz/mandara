//! The services hub: one GPUI entity holding the latest state of every
//! service actor plus their command handles. Plugins observe it; the app
//! pumps actor updates into it on the foreground executor.

use std::collections::HashMap;

use gpui::{App, AppContext as _, AsyncApp, Entity, Global};
use mandara_compositor::{ClientInfo, CompositorBackend, CompositorEvent, NavigationModel};
use crate::widgets::system::SystemSampler;
use mandara_services::{
    AudioService, AudioState, BluetoothService, BluetoothState, ClipboardService, ClipboardState, MediaService, MediaState,
    NetworkService, NetworkState, NotificationService, NotificationsState,
};

#[derive(Default)]
pub struct Hub {
    pub media: Option<MediaState>,
    pub network: NetworkState,
    pub bluetooth: BluetoothState,
    pub audio: AudioState,
    pub clipboard: ClipboardState,
    pub notifications: NotificationsState,
    pub weather: mandara_services::WeatherState,
    /// Smoothed CPU / memory load. Updated *without* notifying the hub;
    /// observe [`resources_entity`] to follow it.
    pub resources: crate::widgets::system::SystemSampler,
    /// Navigation model per monitor name.
    pub navigation: HashMap<String, NavigationModel>,
    pub focused: Option<ClientInfo>,
    /// Every window (for per-workspace app icons).
    pub clients: Vec<ClientInfo>,
    pub keymode: String,
    /// Active keyboard layout name ("English (US)"), when reported.
    pub keyboard_layout: String,

    pub media_service: Option<MediaService>,
    pub network_service: Option<NetworkService>,
    pub bluetooth_service: Option<BluetoothService>,
    pub audio_service: Option<AudioService>,
    /// Clipboard history: runs only while the clipboard plugin is on.
    pub clipboard_service: Option<ClipboardService>,
    pub notification_service: Option<NotificationService>,
    /// The CPU / memory sampler loop: runs only while a plugin that shows
    /// it (resources, processes) is on. Dropping it stops the loop.
    sampler_task: Option<gpui::Task<()>>,
    /// Started only once the weather plugin is placed somewhere (it makes
    /// network requests).
    pub weather_service: Option<mandara_services::WeatherService>,
    /// The running compositor's native driver (MangoWM, Niri or Hyprland).
    pub compositor: Option<std::sync::Arc<dyn CompositorBackend>>,
}

#[derive(Clone)]
pub struct GlobalHub(pub Entity<Hub>);
impl Global for GlobalHub {}

pub fn hub(cx: &App) -> Entity<Hub> {
    cx.global::<GlobalHub>().0.clone()
}

/// The hub, when this process runs the shell (Studio on its own has none).
pub fn try_hub(cx: &App) -> Option<Entity<Hub>> {
    cx.try_global::<GlobalHub>().map(|h| h.0.clone())
}

#[derive(Clone)]
struct GlobalResources(Entity<SystemSampler>);
impl Global for GlobalResources {}

/// The CPU / memory sampler as its own entity. Samples arrive every
/// the System resources plugin's `refresh_ms`; keeping them out of the hub means only the
/// views that show load re-render on each tick, not every plugin view that
/// observes the hub.
pub fn resources_entity(cx: &App) -> Entity<SystemSampler> {
    cx.global::<GlobalResources>().0.clone()
}

/// Forwards updates from `rx` into the hub, notifying observers only when
/// `apply` reports a change. Every plugin view observes the hub, so an
/// unconditional notify re-renders the whole bar for nothing (services and
/// compositors re-send identical state often).
///
/// Updates that queued up while the foreground was busy are coalesced: only
/// the newest value of a burst is applied, and observers hear about it once.
fn pump<T: 'static>(
    hub: Entity<Hub>,
    rx: async_channel::Receiver<T>,
    apply: impl Fn(&mut Hub, T) -> bool + 'static,
    cx: &mut App,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(mut value) = rx.recv().await {
            while let Ok(newer) = rx.try_recv() {
                value = newer;
            }
            hub.update(cx, |h, cx| {
                if apply(h, value) {
                    cx.notify();
                }
            });
        }
    })
    .detach();
}

/// Like [`pump`] for event streams where every message matters (compositor
/// events are deltas of different kinds, so none may be dropped).
fn pump_events<T: 'static>(
    hub: Entity<Hub>,
    rx: async_channel::Receiver<T>,
    apply: impl Fn(&mut Hub, T) -> bool + 'static,
    cx: &mut App,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(value) = rx.recv().await {
            hub.update(cx, |h, cx| {
                let mut changed = apply(h, value);
                // Drain whatever else is already queued before notifying.
                while let Ok(more) = rx.try_recv() {
                    changed |= apply(h, more);
                }
                if changed {
                    cx.notify();
                }
            });
        }
    })
    .detach();
}

/// Playback drift below which a re-sent MPRIS position is just the
/// extrapolation of the previous one (players report position ~1×/s).
const MEDIA_POSITION_TOLERANCE: std::time::Duration = std::time::Duration::from_millis(250);

/// Stores a media update, reporting whether observers must re-render.
/// Position-only updates that agree with [`MediaState::position_now`] of the
/// previous state are stored silently: views extrapolate the position
/// themselves, so they need no notification for a clock that ticks as
/// predicted.
fn set_media(slot: &mut Option<MediaState>, value: Option<MediaState>) -> bool {
    let predicted = match (slot.as_ref(), value.as_ref()) {
        (Some(old), Some(new)) => {
            let expected = old.position_now(new.position_at);
            let drift = expected.abs_diff(new.position);
            drift <= MEDIA_POSITION_TOLERANCE && {
                let mut aligned = new.clone();
                aligned.position = old.position;
                aligned.position_at = old.position_at;
                aligned == *old
            }
        }
        _ => false,
    };
    if predicted {
        *slot = value;
        return false;
    }
    set(slot, value)
}

/// Replaces `slot` with `value`, reporting whether it differed.
fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// Starts every service actor and installs the hub as a global.
pub fn init_hub(cx: &mut App) -> Entity<Hub> {
    crate::productivity::init(cx);
    crate::recorder::init(cx);
    crate::wallpaper::init_theme_sync(cx);
    crate::state::init_app_theme_sync(cx);
    crate::theme_auto::init(cx);
    crate::state::init_language(cx);
    crate::plugins::external::watch(cx);
    crate::plugins::lifecycle::init(cx);
    let media = mandara_services::spawn_media_service();
    let network = mandara_services::spawn_network_service();
    let bluetooth = mandara_services::spawn_bluetooth_service();
    let audio = mandara_services::spawn_audio_service();
    let notifications = mandara_services::spawn_notification_service();
    let compositor = mandara_compositor::connect();

    let hub = cx.new(|_| Hub {
        media_service: Some(media.clone()),
        network_service: Some(network.clone()),
        bluetooth_service: Some(bluetooth.clone()),
        audio_service: Some(audio.clone()),
        notification_service: Some(notifications.clone()),
        compositor: compositor.clone(),
        ..Default::default()
    });
    cx.set_global(GlobalHub(hub.clone()));

    pump(hub.clone(), media.updates.clone(), |h, s| set_media(&mut h.media, s), cx);
    pump(hub.clone(), network.updates.clone(), |h, s| set(&mut h.network, s), cx);
    pump(hub.clone(), bluetooth.updates.clone(), |h, s| set(&mut h.bluetooth, s), cx);
    pump(hub.clone(), audio.updates.clone(), |h, s| set(&mut h.audio, s), cx);
    pump(hub.clone(), notifications.updates.clone(), |h, s| set(&mut h.notifications, s), cx);
    let resources = cx.new(|_| SystemSampler::default());
    cx.set_global(GlobalResources(resources));

    // Plugin-owned services start and stop with their plugin.
    sync_plugin_services(&hub, cx);
    let services_hub = hub.clone();
    cx.observe(&crate::state::config(cx), move |_, cx| sync_plugin_services(&services_hub, cx)).detach();

    // Weather: start when first placed; follow location / unit changes.
    ensure_weather(&hub, cx);
    let weather_hub = hub.clone();
    cx.observe(&crate::state::config(cx), move |_, cx| ensure_weather(&weather_hub, cx)).detach();

    if let Some(backend) = compositor {
        tracing::info!("compositor: {:?}", backend.kind());
        // Seed with one-shot queries, then follow the event streams.
        let initial = backend.get_navigation_state().unwrap_or_default();
        let focused = backend.get_active_client().ok().flatten();
        let clients = backend.get_all_clients().unwrap_or_default();
        hub.update(cx, |h, _| {
            h.navigation = initial.into_iter().collect();
            h.focused = focused;
            h.clients = clients;
        });
        pump_events(
            hub.clone(),
            backend.subscribe_events(),
            |h, event| match event {
                CompositorEvent::Navigation { monitor, model } => h.navigation.insert(monitor, model.clone()).as_ref() != Some(&model),
                CompositorEvent::FocusedClient(c) => set(&mut h.focused, c),
                CompositorEvent::Clients(list) => set(&mut h.clients, list),
                CompositorEvent::Keymode(k) => set(&mut h.keymode, k),
                CompositorEvent::KeyboardLayout(k) => set(&mut h.keyboard_layout, k),
                CompositorEvent::Monitors(_) => false,
            },
            cx,
        );
    }
    crate::osd::init(&hub, cx);
    // Notification popups, privacy monitor, event sounds, auto-DND.
    crate::toasts::init(cx);
    crate::alerts::init(cx);
    crate::update_watch::init(cx);
    crate::night_light::init(cx);
    crate::plugins::displays::init(cx);
    hub
}

/// Plugins that show the CPU / memory sampler.
const SAMPLER_PLUGINS: &[&str] = &[crate::plugins::resources::ID, crate::plugins::processes::ID];

/// Which plugin-owned hub services should run.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PluginServices {
    /// Clipboard history (the clipboard plugin).
    pub clipboard: bool,
    /// CPU / memory sampler ([`SAMPLER_PLUGINS`]).
    pub sampler: bool,
    /// Weather requests: the weather plugin on, weather not switched off,
    /// and the forecast shown somewhere (bar, sidebar, desktop clock).
    pub weather: bool,
}

impl PluginServices {
    pub fn wanted(registry: &mandara_plugins::PluginRegistry, cfg: &mandara_config::ShellConfig) -> Self {
        let on = |id: &str| registry.is_enabled(id, &cfg.plugins);
        let weather_id = crate::plugins::weather::ID;
        let weather_shown = !cfg.plugin_places(weather_id).is_empty()
            || (cfg.wallpaper.desktop_clock && cfg.wallpaper.show_date && cfg.wallpaper.show_weather)
            || cfg.desktop.widgets.painted().any(|w| matches!(&w.kind, mandara_config::desktop_widgets::WidgetKind::Clock(c) if c.weather && c.face == mandara_config::desktop_widgets::ClockFace::DayBlock));
        Self {
            clipboard: on(crate::plugins::clipboard::ID),
            sampler: SAMPLER_PLUGINS.iter().any(|id| on(id)),
            weather: cfg.weather.enabled && on(weather_id) && weather_shown,
        }
    }
}

/// Starts or stops the services owned by one plugin as it is switched on or
/// off (or removed), so an off plugin keeps nothing running.
fn sync_plugin_services(hub: &Entity<Hub>, cx: &mut App) {
    let want = {
        let cfg = crate::state::config(cx).read(cx).snapshot();
        match cx.try_global::<mandara_plugins::PluginRegistry>() {
            Some(registry) => PluginServices::wanted(registry, &cfg),
            None => PluginServices { clipboard: true, sampler: true, weather: true },
        }
    };
    let (want_clipboard, want_sampler) = (want.clipboard, want.sampler);
    match (hub.read(cx).clipboard_service.is_some(), want_clipboard) {
        (false, true) => start_clipboard(hub, cx),
        (true, false) => hub.update(cx, |h, cx| {
            // Dropping the last handle ends the actor; its watcher thread
            // follows at its next wake-up.
            h.clipboard_service = None;
            h.clipboard = Default::default();
            cx.notify();
        }),
        _ => {}
    }
    match (hub.read(cx).sampler_task.is_some(), want_sampler) {
        (false, true) => {
            let task = sampler_loop(hub.clone(), cx);
            hub.update(cx, |h, _| h.sampler_task = Some(task));
        }
        (true, false) => hub.update(cx, |h, _| h.sampler_task = None),
        _ => {}
    }
}

thread_local! {
    /// The clipboard history started once already this session (so turning
    /// the plugin back on doesn't apply "clear on start" again).
    static CLIPBOARD_STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn start_clipboard(hub: &Entity<Hub>, cx: &mut App) {
    let mut config = crate::state::config(cx).read(cx).get().clipboard.clone();
    if CLIPBOARD_STARTED.with(|s| s.replace(true)) {
        config.clear_on_start = false;
    }
    let service = mandara_services::clipboard::spawn_clipboard_service_with(&config);
    pump(hub.clone(), service.updates.clone(), |h, s| set(&mut h.clipboard, s), cx);
    // Notify: hooks that configure the service (`plugins::clipboard::init`)
    // follow the hub.
    hub.update(cx, |h, cx| {
        h.clipboard_service = Some(service);
        cx.notify();
    });
}

/// Samples CPU / memory every `refresh_ms` (System resources) until the
/// returned task is dropped. Sampling reads two small procfs files: a timer
/// on the shared background executor replaces a dedicated sleeping thread.
fn sampler_loop(hub: Entity<Hub>, cx: &mut App) -> gpui::Task<()> {
    let resources = resources_entity(cx);
    cx.spawn(async move |cx: &mut AsyncApp| {
        let bg = cx.background_executor().clone();
        let mut sampler = SystemSampler::default();
        loop {
            sampler = bg
                .spawn(async move {
                    sampler.sample();
                    sampler
                })
                .await;
            let sample = sampler.clone();
            // Mirror into `Hub::resources` for readers, but notify only the
            // dedicated entity's observers.
            hub.update(cx, |h, _| h.resources = sample.clone());
            resources.update(cx, |r, cx| {
                if set(r, sample) {
                    cx.notify();
                }
            });
            let every = cx.update(|cx| crate::plugins::resources::sample_every(cx));
            bg.timer(every).await;
        }
    })
}

fn ensure_weather(hub: &Entity<Hub>, cx: &mut App) {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let wanted = cx.try_global::<mandara_plugins::PluginRegistry>().is_none_or(|r| PluginServices::wanted(r, &cfg).weather);
    if !cfg.weather.enabled {
        // "Off: no weather requests at all" — stop the running service
        // (dropping its handle ends the actor) and forget its forecast.
        if hub.read(cx).weather_service.is_some() {
            hub.update(cx, |h, cx| {
                h.weather_service = None;
                h.weather = Default::default();
                cx.notify();
            });
            crate::plugins::weather::forget_query(cx);
        }
        return;
    }
    if !wanted {
        // Off, removed or shown nowhere: no requests in the background.
        if hub.read(cx).weather_service.is_some() {
            hub.update(cx, |h, cx| {
                h.weather_service = None;
                h.weather = Default::default();
                cx.notify();
            });
            crate::plugins::weather::forget_query(cx);
        }
        return;
    }
    let query = crate::plugins::weather::query(&cfg);
    if let Some(service) = hub.read(cx).weather_service.clone() {
        if crate::plugins::weather::last_query(cx) != Some(query) {
            service.send(mandara_services::WeatherCommand::Configure(query));
        }
    } else {
        let service = mandara_services::spawn_weather_service(query);
        pump(hub.clone(), service.updates.clone(), |h, s| set(&mut h.weather, s), cx);
        hub.update(cx, |h, _| h.weather_service = Some(service));
    }
    crate::plugins::weather::remember_query(query, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::ShellConfig;
    use mandara_plugins::PluginRegistry;

    fn registry() -> PluginRegistry {
        let mut r = PluginRegistry::default();
        crate::plugins::register_builtin(&mut r);
        r
    }

    #[test]
    fn off_plugins_run_no_hub_services() {
        let r = registry();
        let mut c = ShellConfig::default();
        assert_eq!(PluginServices::wanted(&r, &c), PluginServices { clipboard: true, sampler: true, weather: true });
        for id in [crate::plugins::clipboard::ID, crate::plugins::resources::ID, crate::plugins::processes::ID] {
            c.plugins.disabled.push(id.into());
        }
        let w = PluginServices::wanted(&r, &c);
        assert!(!w.clipboard && !w.sampler, "{w:?}");
        // Removing weather stops requests even for the desktop clock's line.
        c.wallpaper.desktop_clock = true;
        c.wallpaper.show_date = true;
        c.wallpaper.show_weather = true;
        mandara_plugins::lifecycle::remove(&mut c, crate::plugins::weather::ID, "Weather").unwrap();
        assert!(!PluginServices::wanted(&r, &c).weather);
        mandara_plugins::lifecycle::restore(&mut c, crate::plugins::weather::ID, None);
        assert!(PluginServices::wanted(&r, &c).weather, "restored live");
    }

    #[test]
    fn weather_runs_only_where_it_is_shown() {
        let r = registry();
        let mut c = ShellConfig::default();
        c.wallpaper.show_weather = false;
        c.take_plugin_places(crate::plugins::weather::ID);
        assert!(!PluginServices::wanted(&r, &c).weather, "placed nowhere");
    }
}
