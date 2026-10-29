//! Glue between the alert settings and the services: pushes
//! `[notifications]` / `[audio]` / `[media]` into their actors, runs the
//! privacy monitor (`[privacy]`) and automatic do not disturb (screen
//! sharing, fullscreen), and plays shell event sounds (`[sounds]`) for
//! notifications, volume changes, the charger and the session's login.
//! Also raises the matching on-screen displays (do not disturb, power
//! profile, now playing, privacy).

use gpui::{App, AppContext as _, Entity, Global};
use mandara_config::ShellConfig;
use mandara_i18n::{t, tf};
use mandara_services::notifications::AutoDnd;
use mandara_services::privacy::{PrivacyMonitor, PrivacyState};
use mandara_services::sounds::{self, SoundEvent};
use mandara_services::system_events::{self, SystemEvent};
use mandara_services::{NotificationCommand, PlaybackStatus, Urgency};

use crate::hub::hub;
use crate::osd::{self, OsdEvent};
use crate::state::config;

/// Marker in `$XDG_RUNTIME_DIR`: the login sound already played this session.
const LOGIN_MARKER: &str = "mandara-login-sound";
/// Names shown in privacy summaries before "+N".
const SUMMARY_NAMES: usize = 2;

/// Live privacy indicators (a global entity the bar chip observes).
pub struct Privacy {
    pub state: PrivacyState,
    monitor: Option<PrivacyMonitor>,
}

#[derive(Clone)]
struct GlobalPrivacy(Entity<Privacy>);
impl Global for GlobalPrivacy {}

pub fn privacy_entity(cx: &App) -> Option<Entity<Privacy>> {
    cx.try_global::<GlobalPrivacy>().map(|g| g.0.clone())
}

/// The current indicators (empty when the monitor is off).
pub fn privacy_state(cx: &App) -> PrivacyState {
    privacy_entity(cx).map(|p| p.read(cx).state.clone()).unwrap_or_default()
}

/// "Firefox, OBS +1" for a list of app names.
fn names(apps: &[mandara_services::privacy::AppUse]) -> String {
    let mut s: Vec<&str> = apps.iter().take(SUMMARY_NAMES).map(|a| a.name.as_str()).collect::<Vec<_>>();
    s.dedup();
    let more = apps.len().saturating_sub(SUMMARY_NAMES);
    if more > 0 { format!("{} +{more}", s.join(", ")) } else { s.join(", ") }
}

/// One line naming who uses what.
pub fn privacy_summary(st: &PrivacyState) -> String {
    let mut parts = Vec::new();
    if !st.microphone.is_empty() {
        parts.push(tf("Microphone: {apps}", &[("apps", &names(&st.microphone))]));
    }
    if !st.camera.is_empty() {
        parts.push(tf("Camera: {apps}", &[("apps", &names(&st.camera))]));
    }
    if !st.screen_share.is_empty() {
        parts.push(tf("Screen: {apps}", &[("apps", &names(&st.screen_share))]));
    }
    if parts.is_empty() { t("Nothing is using your microphone, camera or screen").to_string() } else { parts.join(" · ") }
}

/// Plays `event` when sounds and that event are on, staying quiet while
/// media plays if asked to.
pub fn play_sound(event: SoundEvent, cx: &mut App) {
    let cfg = config(cx).read(cx).get().sounds.clone();
    if !event.enabled_in(&cfg) {
        return;
    }
    play_unchecked(event, cfg, cx);
}

/// The sound for a notification: an app's own sound (`sound-name` hint)
/// follows the toggle of the notification's urgency, as its default sound
/// would (a critical alert with its own sound used to need the plain
/// "Notifications" toggle). `None` when that toggle is off.
fn notification_sound(urgency: Urgency, hint: Option<String>, cfg: &mandara_config::alerts::SoundsConfig) -> Option<SoundEvent> {
    let base = match urgency {
        Urgency::Low => SoundEvent::NotificationLow,
        Urgency::Normal => SoundEvent::Notification,
        Urgency::Critical => SoundEvent::NotificationCritical,
    };
    base.enabled_in(cfg).then(|| hint.map(SoundEvent::Named).unwrap_or(base))
}

/// Plays `event` once its own toggle was checked: the master switch,
/// "Quiet while media plays" and the volume still apply.
fn play_unchecked(event: SoundEvent, mut cfg: mandara_config::alerts::SoundsConfig, cx: &mut App) {
    if !cfg.enabled {
        return;
    }
    // `sounds::play` re-checks the event's toggle; it was checked already.
    cfg.notification = true;
    let playing = hub(cx).read(cx).media.as_ref().is_some_and(|m| m.status == PlaybackStatus::Playing);
    if cfg.mute_while_media && playing {
        return;
    }
    // Decoding and the first upload touch the disk: off the UI thread.
    cx.background_executor()
        .spawn(async move {
            sounds::play(&event, &cfg);
        })
        .detach();
}

/// Decides now (on the UI thread) whether `event` should sound, and
/// returns a closure that plays it later from any thread — for work that
/// finishes off the UI thread, like saving a screenshot.
pub fn deferred_sound(event: SoundEvent, cx: &App) -> impl FnOnce() + Send + 'static {
    let cfg = config(cx).read(cx).get().sounds.clone();
    let playing = hub(cx).read(cx).media.as_ref().is_some_and(|m| m.status == PlaybackStatus::Playing);
    let allowed = cfg.enabled && event.enabled_in(&cfg) && !(cfg.mute_while_media && playing);
    move || {
        if allowed {
            sounds::play(&event, &cfg);
        }
    }
}

fn notify_service(command: NotificationCommand, cx: &mut App) {
    crate::plugins::notifications::send(cx, command);
}

/// Applies the parts of the config the services own.
fn apply(cfg: &ShellConfig, prev: Option<&ShellConfig>, cx: &mut App) {
    if prev.is_none_or(|p| p.notifications != cfg.notifications) {
        notify_service(NotificationCommand::Configure(Box::new(cfg.notifications.clone())), cx);
    }
    if prev.is_none_or(|p| p.audio != cfg.audio) {
        mandara_services::audio::set_volume_limit(cfg.audio.volume_limit());
        let routes = mandara_services::audio::route_streams(cfg.audio.routes.clone());
        cx.set_global(Routes(routes.map(std::rc::Rc::new)));
    }
    if prev.is_none_or(|p| p.media != cfg.media) {
        mandara_services::mpris::configure(&cfg.media);
        if let Some(media) = hub(cx).read(cx).media_service.clone() {
            media.send(mandara_services::MediaCommand::Refresh);
        }
    }
    if prev.is_none_or(|p| p.privacy != cfg.privacy || p.plugins.disabled != cfg.plugins.disabled || p.plugins.enabled != cfg.plugins.enabled || p.plugins.removed != cfg.plugins.removed) {
        configure_privacy(cfg, cx);
    }
    // Automatic do not disturb follows its switches too.
    if prev.is_none_or(|p| p.notifications.dnd_fullscreen != cfg.notifications.dnd_fullscreen || p.notifications.dnd_screen_share != cfg.notifications.dnd_screen_share) {
        sync_auto_dnd(cx);
    }
}

/// Keeps the audio route watcher alive while routes exist.
struct Routes(#[allow(dead_code)] Option<std::rc::Rc<mandara_services::audio::RouteWatcher>>);
impl Global for Routes {}

fn configure_privacy(cfg: &ShellConfig, cx: &mut App) {
    let Some(entity) = privacy_entity(cx) else { return };
    // The monitor belongs to the privacy plugin: off plugin, no watching.
    let plugin_on = cx.try_global::<mandara_plugins::PluginRegistry>().is_none_or(|r| r.is_enabled(crate::plugins::privacy::ID, &cfg.plugins));
    let want = plugin_on && cfg.privacy.indicators && (cfg.privacy.microphone || cfg.privacy.camera || cfg.privacy.screen_share);
    let start = entity.update(cx, |p, _| match (&mut p.monitor, want) {
        (Some(m), true) => {
            m.configure(&cfg.privacy);
            None
        }
        (None, true) => {
            let m = PrivacyMonitor::new(&cfg.privacy);
            let rx = m.updates.clone();
            p.monitor = Some(m);
            Some(rx)
        }
        (_, false) => {
            p.monitor = None;
            p.state = PrivacyState::default();
            None
        }
    });
    if !want {
        entity.update(cx, |_, cx| cx.notify());
        sync_auto_dnd(cx);
    }
    if let Some(rx) = start {
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(mut state) = rx.recv().await {
                while let Ok(newer) = rx.try_recv() {
                    state = newer;
                }
                let alive = cx.update(|cx| {
                    let Some(entity) = weak.upgrade() else { return false };
                    let prev = entity.read(cx).state.clone();
                    entity.update(cx, |p, cx| {
                        p.state = state.clone();
                        cx.notify();
                    });
                    privacy_changed(&prev, &state, cx);
                    // Plugin views (the bar chip) re-render on hub changes.
                    hub(cx).update(cx, |_, cx| cx.notify());
                    true
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }
}

/// OSDs for each device that started / stopped being used, and auto-DND.
fn privacy_changed(prev: &PrivacyState, next: &PrivacyState, cx: &mut App) {
    let pairs = [
        ("mic", t("Microphone in use").to_string(), t("Microphone released").to_string(), &prev.microphone, &next.microphone),
        ("camera", t("Camera in use").to_string(), t("Camera released").to_string(), &prev.camera, &next.camera),
        ("monitor", t("Screen sharing").to_string(), t("Screen sharing stopped").to_string(), &prev.screen_share, &next.screen_share),
    ];
    for (icon, on_title, off_title, before, after) in pairs {
        match (before.is_empty(), after.is_empty()) {
            (true, false) => {
                osd::show(OsdEvent::privacy(icon, on_title, true, &names(after)), cx);
            }
            (false, true) => {
                osd::show(OsdEvent::privacy(icon, off_title, false, &names(before)), cx);
            }
            _ => {}
        }
    }
    if prev.screen_share.is_empty() != next.screen_share.is_empty() {
        sync_auto_dnd(cx);
    }
}

/// Screen sharing (the privacy monitor or Mandara's own recorder) and a
/// focused fullscreen window switch automatic do not disturb.
fn sync_auto_dnd(cx: &mut App) {
    let cfg = config(cx).read(cx).get().notifications.clone();
    let sharing = !privacy_state(cx).screen_share.is_empty() || crate::recorder::is_recording(cx);
    let fullscreen = hub(cx).read(cx).focused.as_ref().is_some_and(|c| c.fullscreen);
    let current = hub(cx).read(cx).notifications.auto_dnd.clone();
    for (reason, on) in [(AutoDnd::ScreenShare, cfg.dnd_screen_share && sharing), (AutoDnd::Fullscreen, cfg.dnd_fullscreen && fullscreen)] {
        if current.contains(&reason) != on {
            notify_service(NotificationCommand::Auto(reason, on), cx);
        }
    }
}

/// Plays the login sound once per session (not on shell restarts).
fn login_sound(cx: &mut App) {
    let Some(dir) = dirs::runtime_dir() else { return };
    let marker = dir.join(LOGIN_MARKER);
    if marker.exists() {
        return;
    }
    let _ = std::fs::write(&marker, b"");
    play_sound(SoundEvent::Login, cx);
}

/// Installs the glue. Call after the hub and the OSD exist.
pub fn init(cx: &mut App) {
    let privacy = cx.new(|_| Privacy { state: PrivacyState::default(), monitor: None });
    cx.set_global(GlobalPrivacy(privacy));
    cx.set_global(Routes(None));
    let cfg = config(cx).read(cx).snapshot();
    apply(&cfg, None, cx);
    let mut last: ShellConfig = (*cfg).clone();
    cx.observe(&config(cx), move |c, cx| {
        let next = c.read(cx).snapshot();
        if *next != last {
            apply(&next, Some(&last), cx);
            last = (*next).clone();
        }
    })
    .detach();

    // Hub-driven: chimes, volume clicks, do-not-disturb and track OSDs,
    // fullscreen auto-DND.
    let h = hub(cx);
    let mut seen_chime: Option<u64> = h.read(cx).notifications.chime.as_ref().map(|c| c.seq);
    let mut quiet = h.read(cx).notifications.quiet();
    let mut audio = h.read(cx).audio.clone();
    let mut track: Option<(String, String)> = h.read(cx).media.as_ref().map(|m| (m.title.clone(), m.artist.clone()));
    let mut fullscreen = h.read(cx).focused.as_ref().is_some_and(|c| c.fullscreen);
    let mut recording = crate::recorder::is_recording(cx);
    cx.observe(&h, move |h, cx| {
        let (chime, now_quiet, now_audio, now_track, playing, now_fullscreen) = {
            let s = h.read(cx);
            (
                s.notifications.chime.clone(),
                s.notifications.quiet(),
                s.audio.clone(),
                s.media.as_ref().map(|m| (m.title.clone(), m.artist.clone())),
                s.media.as_ref().is_some_and(|m| m.status == PlaybackStatus::Playing),
                s.focused.as_ref().is_some_and(|c| c.fullscreen),
            )
        };
        if let Some(c) = chime
            && seen_chime != Some(c.seq)
        {
            seen_chime = Some(c.seq);
            let cfg = config(cx).read(cx).get().sounds.clone();
            if let Some(event) = notification_sound(c.urgency, c.sound, &cfg) {
                play_unchecked(event, cfg, cx);
            }
        }
        if now_quiet != quiet {
            quiet = now_quiet;
            osd::show(OsdEvent::dnd(now_quiet), cx);
        }
        if now_audio != audio {
            let same_device = now_audio.output.device == audio.output.device;
            let changed = (now_audio.output.volume - audio.output.volume).abs() > f32::EPSILON || now_audio.output.muted != audio.output.muted;
            if audio.available && now_audio.available && same_device && changed && !now_audio.output.muted {
                play_sound(SoundEvent::VolumeChange, cx);
            }
            audio = now_audio;
        }
        if now_track != track {
            if playing
                && let Some((title, artist)) = &now_track
                && !title.is_empty()
            {
                osd::show(OsdEvent::media(title, artist), cx);
            }
            track = now_track;
        }
        let now_recording = crate::recorder::is_recording(cx);
        if now_fullscreen != fullscreen || now_recording != recording {
            fullscreen = now_fullscreen;
            recording = now_recording;
            sync_auto_dnd(cx);
        }
    })
    .detach();

    // Charger and power-profile changes.
    let events = system_events::watch();
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv().await {
            cx.update(|cx| match event {
                SystemEvent::OnBattery(on_battery) => play_sound(if on_battery { SoundEvent::PowerUnplug } else { SoundEvent::PowerPlug }, cx),
                SystemEvent::PowerProfile(profile) => {
                    osd::show(OsdEvent::power_profile(&profile), cx);
                }
            });
        }
    })
    .detach();

    login_sound(cx);
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Event keys `sounds preview` offers.
const SOUND_EVENTS: &[&str] = &["volume-change", "notification-low", "notification", "notification-critical", "power-plug", "power-unplug", "screenshot", "login", "battery-low", "battery-critical"];

fn sound_arg(a: &mandara_plugins::Args) -> Result<SoundEvent, String> {
    Ok(SoundEvent::parse(a.str(0)?))
}

pub fn register(r: &mut mandara_plugins::PluginRegistry) {
    use mandara_plugins::{ArgKind, ArgSpec, CommandSpec};
    use mandara_i18n::tk;
    use serde_json::json;
    let owner = "sounds";
    r.command(
        owner,
        CommandSpec::new("sounds play", tk("Play a shell sound if sounds and that event are on (or any theme sound name)"), |a, cx| {
            let event = sound_arg(a)?;
            let cfg = config(cx).read(cx).get().sounds.clone();
            let file = (cfg.enabled && event.enabled_in(&cfg)).then(|| sounds::resolve(&event, &cfg)).flatten();
            play_sound(event, cx);
            Ok(json!({ "file": file, "enabled": cfg.enabled }))
        })
        .arg(ArgSpec::required("event", ArgKind::String, tk("volume-change, notification, notification-critical, power-plug, screenshot, login, battery-low… or a sound name")))
        .example("sounds play screenshot"),
    );
    r.command(
        owner,
        CommandSpec::new("sounds preview", tk("Play a shell sound even while sounds are off"), |a, cx| {
            let event = sound_arg(a)?;
            let cfg = config(cx).read(cx).get().sounds.clone();
            let file = sounds::resolve(&event, &cfg).ok_or_else(|| mandara_i18n::tf("No sound for \"{event}\" in this theme", &[("event", &event.key())]))?;
            cx.background_executor()
                .spawn(async move {
                    sounds::preview(&event, &cfg);
                })
                .detach();
            Ok(json!({ "file": file }))
        })
        .arg(ArgSpec::required("event", ArgKind::Enum, tk("which sound")).choices(SOUND_EVENTS)),
    );
    r.command(
        owner,
        CommandSpec::new("sounds status", tk("Shell sound settings and the file each event plays"), |_, cx| {
            let cfg = config(cx).read(cx).get().sounds.clone();
            let theme = if cfg.theme.trim().is_empty() { sounds::system_theme() } else { cfg.theme.clone() };
            let files: serde_json::Map<String, serde_json::Value> = SoundEvent::CONFIGURABLE
                .iter()
                .map(|e| (e.key().to_string(), json!({ "on": e.enabled_in(&cfg), "file": sounds::resolve(e, &cfg) })))
                .collect();
            Ok(json!({ "enabled": cfg.enabled, "theme": theme, "volume": cfg.volume, "events": files }))
        }),
    );
    r.command(owner, CommandSpec::new("sounds themes", tk("Installed sound themes"), |_, _| Ok(json!({ "themes": sounds::themes(), "system": sounds::system_theme() }))));
    r.command(
        owner,
        CommandSpec::new("sounds enable", tk("Turn shell sounds on or off"), |a, cx| {
            let on = a.str(0)? == "on";
            crate::state::update_config(cx, |c| c.sounds.enabled = on);
            Ok(json!({ "enabled": on }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on / off")).choices(&["on", "off"])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::privacy::AppUse;

    #[test]
    fn notification_sounds_follow_the_urgency_toggle() {
        let mut cfg = mandara_config::alerts::SoundsConfig { notification: false, notification_critical: true, notification_low: false, ..Default::default() };
        assert_eq!(notification_sound(Urgency::Critical, Some("alarm-clock-elapsed".into()), &cfg), Some(SoundEvent::Named("alarm-clock-elapsed".into())), "critical with its own sound plays with Notifications off");
        assert_eq!(notification_sound(Urgency::Critical, None, &cfg), Some(SoundEvent::NotificationCritical));
        assert_eq!(notification_sound(Urgency::Normal, Some("bell".into()), &cfg), None);
        assert_eq!(notification_sound(Urgency::Low, None, &cfg), None);
        cfg.notification_critical = false;
        cfg.notification = true;
        assert_eq!(notification_sound(Urgency::Critical, Some("bell".into()), &cfg), None, "critical off silences its own sound too");
        assert_eq!(notification_sound(Urgency::Normal, None, &cfg), Some(SoundEvent::Notification));
    }

    fn app(n: &str) -> AppUse {
        AppUse { name: n.into(), pid: None, icon: None }
    }

    #[test]
    fn summaries_name_apps_and_count_the_rest() {
        assert_eq!(names(&[app("Firefox")]), "Firefox");
        assert_eq!(names(&[app("A"), app("B"), app("C"), app("D")]), "A, B +2");
        let st = PrivacyState { microphone: vec![app("Zoom")], screen_share: vec![app("OBS")], ..Default::default() };
        let s = privacy_summary(&st);
        assert!(s.contains("Zoom") && s.contains("OBS"));
        assert_eq!(privacy_summary(&PrivacyState::default()), t("Nothing is using your microphone, camera or screen"));
    }
}
