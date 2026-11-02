//! Runs what a click on a painted desktop widget asks for
//! ([`crate::backdrop::widgets::actions::Action`]): the wallpaper canvas
//! posts the widget and the action, and `mandara msg desktop widgets press`
//! does the same from the command line.
//!
//! Every action goes through the service the rest of the shell uses for it
//! (the to-do book, the focus timer, the network and Bluetooth actors, the
//! audio server, logind's backlight), so a click on the desktop and a click
//! in a panel are the same thing.

use std::collections::HashMap;

use gpui::{App, Global};
use mandara_i18n::{t, tf};
use mandara_services::{BluetoothCommand, NetworkCommand, NotificationCommand};

use crate::backdrop::widgets::actions::Action;
use crate::state::{config, shell, update_config};

/// Volume and brightness change this much per scroll step.
const LEVEL_STEP: f32 = 0.05;

/// Per-widget turns of its "next" (another photo, another quote, a poke)
/// and when it was last asked for (Unix ms): runtime only, never written
/// to the config.
#[derive(Default)]
pub struct Turns(pub HashMap<String, (u32, i64)>);
impl Global for Turns {}

/// Runs `action` for the widget `widget` (posted by the canvas).
pub fn run(widget: &str, action: Action, cx: &mut App) {
    if let Err(err) = perform(widget, &action, cx) {
        tracing::warn!("desktop widget {widget}: {action}: {err}");
    }
}

/// Runs `action`; the error says why it couldn't.
pub fn perform(widget: &str, action: &Action, cx: &mut App) -> Result<(), String> {
    match action {
        Action::Todo(id) => {
            let p = crate::productivity::productivity(cx);
            let done = p.read(cx).book.todos.iter().find(|t| t.id == *id).map(|t| t.done).ok_or_else(|| tf("No task {id}", &[("id", id)]))?;
            p.update(cx, |p, cx| p.set_done(*id, !done, cx));
        }
        Action::TodoAdd => open_panel(crate::plugins::todo::ID, cx),
        Action::Note(_) => open_panel(crate::plugins::notes::ID, cx),
        Action::Pomodoro => crate::productivity::productivity(cx).update(cx, |p, cx| p.toggle(cx)),
        Action::PomodoroSkip => crate::productivity::productivity(cx).update(cx, |p, cx| p.skip(cx)),
        Action::PomodoroReset => crate::productivity::productivity(cx).update(cx, |p, cx| p.reset(cx)),
        Action::Launch(id) => crate::plugins::app_installer::open_app(id, cx)?,
        Action::Open(target) => open(target),
        Action::Toggle(what) => toggle(what, cx)?,
        Action::Copy(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone())),
        Action::Media(verb) => {
            use mandara_services::MediaCommand;
            let command = match verb.as_str() {
                "play-pause" => MediaCommand::PlayPause,
                "next" => MediaCommand::Next,
                "previous" => MediaCommand::Previous,
                other => return Err(format!("unknown media action {other:?}")),
            };
            crate::plugins::media::send(cx, command);
        }
        Action::Search => shell(cx).update(cx, |s, cx| s.set_launcher(true, cx)),
        Action::Level(what, v) => set_level(what, *v, cx)?,
        Action::Step(what, d) => {
            let now = level(what, cx).ok_or_else(|| tf("No {what} to change here", &[("what", what)]))?;
            set_level(what, (now + LEVEL_STEP * *d as f32).clamp(0.0, 1.0), cx)?;
        }
        Action::Next => {
            if !cx.has_global::<Turns>() {
                cx.set_global(Turns::default());
            }
            let t = cx.global_mut::<Turns>().0.entry(widget.to_string()).or_default();
            *t = (t.0 + 1, chrono::Local::now().timestamp_millis());
            crate::backdrop::refresh(cx);
        }
        Action::Panel(id) => open_panel(id, cx),
        Action::Refresh => crate::desktop_feeds::refresh(cx),
        Action::JannahStep => {
            crate::plugins::jannah::mark_step_done(cx);
        }
        Action::QuranContinue => crate::plugins::quran::continue_reading(cx),
    }
    Ok(())
}

fn open_panel(id: &str, cx: &mut App) {
    let id = id.to_string();
    shell(cx).update(cx, |s, cx| s.set_panel(Some(id), cx));
}

/// Opens a folder, file or web address with its default app.
fn open(target: &str) {
    let target = target.trim();
    if target.contains("://") {
        let uri = target.to_string();
        std::thread::spawn(move || {
            if let Err(err) = mandara_services::session::open_uri(&uri) {
                tracing::warn!("desktop widget: opening {uri}: {err}");
            }
        });
    } else {
        crate::plugins::dock::stacks::open_path(&mandara_config::paths::expand_tilde(target));
    }
}

/// The quick toggles a widget can flip, and their state.
pub const TOGGLES: [&str; 7] = ["wifi", "bluetooth", "dnd", "night-light", "keep-awake", "dark", "mute"];

/// Whether toggle `what` is on.
pub fn toggle_state(what: &str, cx: &App) -> Option<bool> {
    let hub = crate::hub::try_hub(cx)?;
    let hub = hub.read(cx);
    Some(match what {
        "wifi" => hub.network.wifi_enabled,
        "bluetooth" => hub.bluetooth.powered,
        "dnd" => hub.notifications.dnd,
        "night-light" => crate::night_light::active(cx),
        "keep-awake" => crate::plugins::quick_settings::keep_awake_active(cx),
        "dark" => config(cx).read(cx).get().theme.mode.is_dark(),
        "mute" => hub.audio.all_muted,
        _ => return None,
    })
}

fn toggle(what: &str, cx: &mut App) -> Result<(), String> {
    let on = toggle_state(what, cx).ok_or_else(|| tf("Unknown toggle {name}", &[("name", &what)]))?;
    match what {
        "wifi" => crate::plugins::hub_network(cx).ok_or_else(|| t("Networking isn't available").to_string())?.send(NetworkCommand::SetWifi(!on)),
        "bluetooth" => crate::plugins::hub_bluetooth(cx).ok_or_else(|| t("Bluetooth isn't available").to_string())?.send(BluetoothCommand::SetPower(!on)),
        "dnd" => crate::plugins::notifications::send(cx, NotificationCommand::SetDnd(!on)),
        "night-light" => {
            crate::night_light::toggle(cx);
        }
        "keep-awake" => {
            crate::plugins::quick_settings::set_keep_awake(!on, cx)?;
        }
        "dark" => {
            let mode = config(cx).read(cx).get().theme.mode.toggled();
            update_config(cx, |c| c.theme.mode = mode);
        }
        "mute" => {
            if let Some(s) = crate::hub::hub(cx).read(cx).audio_service.clone() {
                s.send(mandara_services::AudioCommand::ToggleMuteAll);
            }
        }
        _ => {}
    }
    cx.refresh_windows();
    crate::backdrop::refresh(cx);
    Ok(())
}

/// The level of `volume` or `brightness` (0–1), when there is one.
pub fn level(what: &str, cx: &App) -> Option<f32> {
    match what {
        "volume" => crate::hub::try_hub(cx).map(|h| h.read(cx).audio.output.volume.clamp(0.0, 1.0)),
        "brightness" => crate::osd::brightness_level(cx),
        _ => None,
    }
}

fn set_level(what: &str, v: f32, cx: &mut App) -> Result<(), String> {
    match what {
        "volume" => {
            let s = crate::hub::hub(cx).read(cx).audio_service.clone().ok_or_else(|| t("Sound isn't available").to_string())?;
            s.send(mandara_services::AudioCommand::SetVolume(v));
        }
        "brightness" => {
            let _ = crate::osd::set_brightness(v, cx);
        }
        other => return Err(tf("Unknown level {name}", &[("name", &other)]).to_string()),
    }
    crate::backdrop::refresh(cx);
    Ok(())
}
