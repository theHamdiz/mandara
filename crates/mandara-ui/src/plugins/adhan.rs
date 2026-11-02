//! The adhan at prayer times: the Prayer times plugin's `adhan` setting
//! (off by default). Turning it on downloads the recording once
//! (`mandara_services::adhan`: progress, a failure with Try again, nothing
//! while offline); turning it off stops it and keeps the file until you
//! remove it (Studio, `prayer adhan remove`).
//!
//! While it is on, one timer looks at the clock at most a minute apart
//! (the bar's prayer chip already redraws each minute) and at each of the
//! five prayers (never sunrise) plays the adhan once and posts a
//! notification with a Stop action. It plays only within a couple of
//! minutes of the prayer's time, so a prayer missed while the computer
//! slept is never played late, and the last prayer it played for is kept
//! in `~/.local/state/mandara/prayer.json`, so a restart never plays one
//! twice. Do not disturb and presentation mode keep it silent (the
//! notification still goes to the notification center). It plays on its
//! own stream at the system volume; the panel, the notification and
//! `prayer adhan stop` stop it.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime, TimeDelta};
use gpui::{AnyElement, App, AppContext as _, Global, InteractiveElement as _, IntoElement, ParentElement, StatefulInteractiveElement as _, Styled, Task, div, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{Lang, t, tf, tk};
use mandara_plugins::{CommandSpec, Output, PluginRegistry};
use mandara_services::adhan;
use mandara_services::prayer::Prayer;
use mandara_services::quran::player::{Event, Player, Status};
use mandara_theme::MandaraPalette;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::extras::caption;
use super::panel_fx::{ViewSet, pill_button, readable};
use super::prayer::ID;
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};
use crate::state::config;

/// The notification action that stops the adhan.
pub const STOP_ACTION: &str = "mandara-adhan-stop";
/// The adhan plays only this long after a prayer's time (later, the
/// prayer was missed: asleep, or the shell wasn't running).
const GRACE: TimeDelta = TimeDelta::minutes(2);
/// The clock is looked at least this often while the adhan is on, so a
/// resume from suspend never leaves the timer waiting past a prayer.
const LOOK_EVERY: Duration = Duration::from_secs(60);
/// Wake-ups land this far past what they wait for.
const TICK_MARGIN: Duration = Duration::from_millis(250);

/// Where the recording stands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fetch {
    Unknown,
    Missing,
    Downloading { done: u64, total: u64 },
    Ready,
    Failed(String),
}

/// What is remembered between sessions: the last prayer the adhan was
/// played (or announced) for, `YYYY-MM-DD Fajr`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    adhan_last: Option<String>,
}

fn state_path() -> std::path::PathBuf {
    mandara_config::paths::state_dir().join("prayer.json")
}

struct Model {
    fetch: Fetch,
    player: Option<Player>,
    status: Status,
    timer: Option<Task<()>>,
    saved: Saved,
    views: ViewSet,
}

type Shared = Rc<RefCell<Model>>;

struct GlobalAdhan(Shared);

impl Global for GlobalAdhan {}

fn model(cx: &mut App) -> Shared {
    if let Some(g) = cx.try_global::<GlobalAdhan>() {
        return g.0.clone();
    }
    let saved: Saved = std::fs::read_to_string(state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let shared = Rc::new(RefCell::new(Model { fetch: Fetch::Unknown, player: None, status: Status::Stopped, timer: None, saved, views: ViewSet::default() }));
    cx.set_global(GlobalAdhan(shared.clone()));
    shared
}

/// The setting: the adhan at each prayer.
pub fn wanted(cx: &App) -> bool {
    let cfg = config(cx).read(cx).snapshot();
    cfg.plugins.setting(super::prayer::ID, "adhan", false) && crate::desktop_plugins::enabled(ID, &cfg, cx)
}

fn changed(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

/// Keeps everything as the setting asks: the recording downloaded, the
/// timer running, or (off) the adhan stopped and the timer gone. Cheap;
/// the prayer views call it as they render.
pub fn ensure(window: Option<&gpui::Window>, cx: &mut App) {
    let shared = model(cx);
    if let Some(w) = window {
        shared.borrow().views.track(w);
    }
    if !wanted(cx) {
        let mut m = shared.borrow_mut();
        m.timer = None;
        if m.status != Status::Stopped
            && let Some(p) = &m.player
        {
            p.stop();
        }
        return;
    }
    if shared.borrow().fetch == Fetch::Unknown {
        shared.borrow_mut().fetch = if adhan::downloaded().is_some() { Fetch::Ready } else { Fetch::Missing };
    }
    let fetch = shared.borrow().fetch.clone();
    match fetch {
        Fetch::Missing => download(&shared, cx),
        Fetch::Ready if shared.borrow().timer.is_none() => look(&shared, cx),
        _ => {}
    }
}

/// Downloads the recording off the UI thread, progress as it comes.
fn download(shared: &Shared, cx: &mut App) {
    if mandara_services::offline::is_offline() {
        shared.borrow_mut().fetch = Fetch::Failed(mandara_services::offline::blocked());
        changed(shared, cx);
        return;
    }
    shared.borrow_mut().fetch = Fetch::Downloading { done: 0, total: adhan::EXPECTED_BYTES };
    let (tx, rx) = async_channel::unbounded::<(u64, u64)>();
    let task = cx.background_spawn(async move {
        adhan::download(&|done, total| {
            let _ = tx.try_send((done, total));
        })
    });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        while let Ok((done, total)) = rx.recv().await {
            cx.update(|cx| {
                shared.borrow_mut().fetch = Fetch::Downloading { done, total };
                changed(&shared, cx);
            });
        }
        let result = task.await;
        cx.update(|cx| {
            shared.borrow_mut().fetch = match result {
                Ok(_) => Fetch::Ready,
                Err(e) => {
                    tracing::warn!("prayer: {e}");
                    Fetch::Failed(e)
                }
            };
            changed(&shared, cx);
            ensure(None, cx);
        });
    })
    .detach();
}

/// Tries a failed download again.
pub fn retry(cx: &mut App) {
    let shared = model(cx);
    if matches!(shared.borrow().fetch, Fetch::Failed(_)) {
        shared.borrow_mut().fetch = Fetch::Missing;
    }
    ensure(None, cx);
    changed(&shared, cx);
}

/// The prayer whose time is now (within [`GRACE`]), with its key.
fn due(cx: &App, now: NaiveDateTime) -> Option<(Prayer, String)> {
    due_in(&super::prayer::today(cx), now)
}

/// [`due`] for today's `times`.
fn due_in(times: &mandara_services::prayer::PrayerTimes, now: NaiveDateTime) -> Option<(Prayer, String)> {
    times.times.iter().copied().filter(|(p, _)| p.is_prayer()).map(|(p, at)| (p, now.date().and_time(at))).find(|(_, at)| *at <= now && now - *at <= GRACE).map(|(p, _)| (p, format!("{} {}", now.date().format("%Y-%m-%d"), p.name())))
}

/// How long until the clock should be looked at again.
fn next_look(cx: &App, now: NaiveDateTime) -> Duration {
    let today = super::prayer::today(cx);
    let next = match today.next_after(now.time()) {
        Some((_, at)) => now.date().and_time(at),
        None => (now.date() + TimeDelta::days(1)).and_time(super::prayer::tomorrow(cx).get(Prayer::Fajr)),
    };
    (next - now).to_std().unwrap_or(Duration::ZERO).min(LOOK_EVERY) + TICK_MARGIN
}

/// Looks at the clock: plays the adhan when a prayer's time has just come,
/// then waits for the next look.
fn look(shared: &Shared, cx: &mut App) {
    let now = Local::now().naive_local();
    if let Some((prayer, key)) = due(cx, now)
        && shared.borrow().saved.adhan_last.as_deref() != Some(key.as_str())
    {
        shared.borrow_mut().saved.adhan_last = Some(key);
        save(shared, cx);
        announce(shared, prayer, cx);
    }
    let wait = next_look(cx, now);
    let weak = Rc::downgrade(shared);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            let Some(shared) = weak.upgrade() else { return };
            shared.borrow_mut().timer = None;
            if wanted(cx) {
                look(&shared, cx);
            }
        });
    });
    shared.borrow_mut().timer = Some(task);
}

fn save(shared: &Shared, cx: &mut App) {
    let saved = shared.borrow().saved.clone();
    cx.background_spawn(async move {
        let Ok(json) = serde_json::to_vec_pretty(&saved) else { return };
        if let Some(dir) = state_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = mandara_config::store::write_atomic(&state_path(), &json) {
            tracing::warn!("prayer: saving state: {e:#}");
        }
    })
    .detach();
}

/// The player, made on first use; its events reach the views.
fn player(shared: &Shared, cx: &mut App) -> Player {
    if let Some(p) = shared.borrow().player.clone() {
        return p;
    }
    let (tx, rx) = async_channel::unbounded::<Event>();
    let p = adhan::player(move |ev| {
        let _ = tx.try_send(ev);
    });
    let weak = Rc::downgrade(shared);
    cx.spawn(async move |cx| {
        while let Ok(ev) = rx.recv().await {
            let Some(shared) = weak.upgrade() else { break };
            cx.update(|cx| {
                if let Event::Error(e) = &ev {
                    tracing::warn!("prayer: adhan: {e}");
                }
                let status = shared.borrow().player.as_ref().map(|p| p.snapshot().status).unwrap_or_default();
                shared.borrow_mut().status = status;
                changed(&shared, cx);
            });
        }
    })
    .detach();
    shared.borrow_mut().player = Some(p.clone());
    p
}

/// Whether the desktop asks for quiet: do not disturb or presentation mode.
fn quiet(cx: &App) -> bool {
    crate::hub::hub(cx).read(cx).notifications.quiet() || super::presentation::active(cx)
}

/// The prayer's time has come: the notification, and the adhan unless the
/// desktop asks for quiet.
fn announce(shared: &Shared, prayer: Prayer, cx: &mut App) {
    let _lang = mandara_i18n::scoped(super::prayer::own_lang(cx));
    let silent = quiet(cx);
    let playing = !silent && adhan::downloaded().is_some();
    if playing {
        let p = player(shared, cx);
        adhan::play(&p);
        shared.borrow_mut().status = Status::Loading;
    }
    let name = t(prayer.name());
    let body = if playing { tf("The adhan for {prayer} is playing", &[("prayer", &name)]) } else { tf("It's time for {prayer}", &[("prayer", &name)]) };
    let actions = if playing { vec![(STOP_ACTION.to_string(), t("Stop the adhan").to_string())] } else { Vec::new() };
    super::notifications::send(
        cx,
        mandara_services::NotificationCommand::PostActions { app: t("Prayer times").into(), icon: "crescent".into(), summary: name.into(), body, actions },
    );
    changed(shared, cx);
}

/// Stops the adhan (a no-op when it isn't playing); whether it was.
pub fn stop(cx: &mut App) -> bool {
    let shared = model(cx);
    let was = shared.borrow().status != Status::Stopped;
    if let Some(p) = shared.borrow().player.clone() {
        p.stop();
    }
    shared.borrow_mut().status = Status::Stopped;
    changed(&shared, cx);
    was
}

/// Whether the adhan is playing now.
pub fn playing(cx: &mut App) -> bool {
    model(cx).borrow().status != Status::Stopped
}

/// The panel's adhan row: the download's progress, its failure with Try
/// again, or Stop while it plays (`None`: nothing to say).
pub fn row(palette: &MandaraPalette, cx: &mut App) -> Option<AnyElement> {
    let shared = model(cx);
    let (fetch, status) = {
        let m = shared.borrow();
        (m.fetch.clone(), m.status)
    };
    let accent = readable(palette.primary, palette);
    let line = |glyph: &'static str, words: String| div().flex_1().min_w(px(0.0)).row().items_center().gap(px(tokens::SM)).child(icon(glyph, text::BODY, accent)).child(caption(words, palette).truncate());
    if status != Status::Stopped {
        return Some(
            div()
                .w_full()
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(line("volume-high", t("The adhan is playing").into()))
                .child(pill_button("prayer-adhan-stop", Some("stop"), t("Stop the adhan"), accent, palette, |_, cx| {
                    stop(cx);
                }))
                .into_any_element(),
        );
    }
    if !wanted(cx) {
        return None;
    }
    match fetch {
        Fetch::Downloading { done, total } => {
            let pct = (done as f64 / total.max(1) as f64 * 100.0).round() as u32;
            Some(line("download", tf("Downloading the adhan… {percent}%", &[("percent", &pct)])).into_any_element())
        }
        Fetch::Failed(why) => Some(
            div()
                .w_full()
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(line("alert", why))
                .child(
                    div()
                        .id("prayer-adhan-retry")
                        .cursor_pointer()
                        .text_size(px(text::LABEL))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(accent.hsla())
                        .child(t("Try again"))
                        .on_click(|_, _, cx| retry(cx)),
                )
                .into_any_element(),
        ),
        _ => None,
    }
}

fn status_json(cx: &mut App) -> Value {
    let shared = model(cx);
    let m = shared.borrow();
    json!({
        "on": wanted(cx),
        "downloaded_bytes": adhan::downloaded(),
        "file": adhan::path(),
        "download": match &m.fetch {
            Fetch::Downloading { done, total } => json!({ "bytes": done, "total_bytes": total }),
            Fetch::Failed(why) => json!({ "failed": why }),
            Fetch::Ready => json!("ready"),
            Fetch::Missing | Fetch::Unknown => json!(if adhan::downloaded().is_some() { "ready" } else { "missing" }),
        },
        "playing": m.status != Status::Stopped,
        "last_announced": m.saved.adhan_last,
        "source": { "url": adhan::URL, "page": adhan::PAGE, "licence": adhan::LICENCE, "author": adhan::AUTHOR },
    })
}

/// `prayer adhan …`, run in the plugin's language (`lang`).
pub fn register_commands(r: &mut PluginRegistry, lang: fn(&App) -> Option<Lang>) {
    let mut add = |spec: CommandSpec| r.command(ID, super::plugin_lang::speaking(spec, lang));
    add(CommandSpec::new("prayer adhan", tk("The adhan: whether it is on, downloaded or playing, and where it comes from"), |_, cx| Ok(status_json(cx))));
    add(CommandSpec::new("prayer adhan stop", tk("Stop the adhan"), |_, cx| Ok(json!({ "stopped": stop(cx) }))));
    add(
        CommandSpec::new("prayer adhan play", tk("Play the adhan now (to hear it; it must be downloaded)"), |_, cx| {
            if adhan::downloaded().is_none() {
                return Err(t("The adhan isn't downloaded: turn it on in Studio, or run `prayer adhan download`").into());
            }
            let shared = model(cx);
            let p = player(&shared, cx);
            adhan::play(&p);
            shared.borrow_mut().status = Status::Loading;
            changed(&shared, cx);
            Ok(status_json(cx))
        })
        .example("prayer adhan play"),
    );
    add(CommandSpec::new_output("prayer adhan download", tk("Download the adhan now (it downloads on its own when you turn it on)"), |_, cx| {
        let shared = model(cx);
        if adhan::downloaded().is_some() {
            shared.borrow_mut().fetch = Fetch::Ready;
            return Ok(Output::Value(status_json(cx)));
        }
        mandara_services::offline::check()?;
        let (tx, rx) = async_channel::bounded(1);
        let task = cx.background_spawn(async move { adhan::download(&|_, _| {}) });
        cx.spawn(async move |cx| {
            let result = task.await;
            let reply = cx.update(|cx| {
                let shared = model(cx);
                shared.borrow_mut().fetch = if result.is_ok() { Fetch::Ready } else { Fetch::Missing };
                changed(&shared, cx);
                result.map(|_| status_json(cx).to_string())
            });
            let _ = tx.send(reply).await;
        })
        .detach();
        Ok(Output::Pending(rx))
    }));
    add(CommandSpec::new("prayer adhan remove", tk("Delete the downloaded adhan (it downloads again only when the adhan is on)"), |_, cx| {
        stop(cx);
        let freed = adhan::remove()?;
        let shared = model(cx);
        shared.borrow_mut().fetch = Fetch::Unknown;
        // Still on: it comes back; off: it stays gone.
        if wanted(cx) {
            update_off(cx);
        }
        changed(&shared, cx);
        Ok(json!({ "removed": true, "freed_bytes": freed, "adhan_on": wanted(cx) }))
    }));
}

/// Removing the recording turns the adhan off (or it would come straight
/// back on the next look).
fn update_off(cx: &mut App) {
    crate::state::update_config(cx, |c| {
        c.plugins.settings.entry(ID.to_string()).or_default().insert("adhan".into(), toml::Value::Boolean(false));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_prayer_just_come_is_due() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let at = |h, m| chrono::NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let times = mandara_services::prayer::PrayerTimes {
            date,
            times: [(Prayer::Fajr, at(4, 50)), (Prayer::Sunrise, at(6, 10)), (Prayer::Dhuhr, at(11, 40)), (Prayer::Asr, at(14, 40)), (Prayer::Maghrib, at(17, 0)), (Prayer::Isha, at(18, 20))],
        };
        let now = |h, m, s| date.and_hms_opt(h, m, s).unwrap();
        // On time, and a moment later: Dhuhr, keyed by the day.
        assert_eq!(due_in(&times, now(11, 40, 0)), Some((Prayer::Dhuhr, "2026-11-02 Dhuhr".into())));
        assert_eq!(due_in(&times, now(11, 41, 30)).map(|(p, _)| p), Some(Prayer::Dhuhr));
        // Too late (asleep at the time), too early, and never sunrise.
        assert_eq!(due_in(&times, now(11, 45, 0)), None);
        assert_eq!(due_in(&times, now(11, 39, 59)), None);
        assert_eq!(due_in(&times, now(6, 10, 30)), None);
        // The clock is looked at often enough to land inside the window.
        assert!(LOOK_EVERY + TICK_MARGIN < GRACE.to_std().unwrap());
    }
}
