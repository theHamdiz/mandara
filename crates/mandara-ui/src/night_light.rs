//! The night-light controller: decides the screen colour temperature from
//! `[night_light]` (schedule, temperatures, exceptions) plus a temporary
//! manual override, and drives [`mandara_services::gamma`].
//!
//! The gamma thread starts only once a warm ramp is first needed. The
//! schedule is re-evaluated every [`TICK`] (fades between day and night
//! are many small eased steps), on every config change, and when the
//! focused window changes (fullscreen / excluded-app pauses).

use std::time::{Duration, Instant};

use chrono::Local;
use gpui::{App, Global};
use mandara_config::ShellConfig;
use mandara_config::outputs::{NightSchedule, defaults};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::gamma::{Gamma, GammaSupport, probe};
use mandara_services::night_light::{GammaTarget, NightWindow, next_change, phase};
use serde_json::json;

use crate::state::{config, update_config};

/// Schedule re-evaluation period.
const TICK: Duration = Duration::from_secs(30);
/// Fade for scheduled steps (each step is small; this hides the stairs).
const STEP_FADE: Duration = Duration::from_secs(3);
/// Fade for manual toggles and setting changes.
const TOGGLE_FADE: Duration = Duration::from_millis(1200);
/// Fade for previews (follows a dragged slider closely).
const PREVIEW_FADE: Duration = Duration::from_millis(250);
/// Default and longest preview, seconds.
const PREVIEW_SECONDS: f64 = 3.0;
const MAX_PREVIEW_SECONDS: f64 = 10.0;
/// The schedule counts as "night" from this factor on (override bookkeeping).
const NIGHT_SIDE: f32 = 0.5;

/// A manual override that lasts until the schedule next crosses day/night.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Forced {
    on: bool,
    /// The schedule's side (night?) when it was set.
    night: bool,
}

#[derive(Default)]
struct NightLight {
    gamma: Option<Gamma>,
    forced: Option<Forced>,
    preview: Option<(Instant, GammaTarget)>,
    /// Last target sent.
    sent: Option<GammaTarget>,
    /// Why it's paused, when it is.
    paused: Option<&'static str>,
    /// Cached support probe (before the thread runs).
    support: Option<GammaSupport>,
}
impl Global for NightLight {}

fn state(cx: &mut App) -> &mut NightLight {
    cx.default_global::<NightLight>()
}

/// Sun-schedule location: the night light's own, else the shared
/// location, else the weather / prayer-times location.
fn location(cfg: &ShellConfig) -> (f64, f64) {
    if let (Some(lat), Some(lon)) = (cfg.night_light.latitude, cfg.night_light.longitude) {
        return (lat, lon);
    }
    if cfg.location.shared {
        return (cfg.location.latitude, cfg.location.longitude);
    }
    let q = crate::plugins::weather::query(cfg);
    (q.latitude, q.longitude)
}

/// The full night target for `cfg`.
fn night_target(cfg: &ShellConfig) -> GammaTarget {
    let n = &cfg.night_light;
    GammaTarget { temperature: n.night_temperature, gamma: n.gamma, brightness: n.brightness }
}

/// What the screens should show now, and why it's paused (if it is).
fn desired(cfg: &ShellConfig, forced: &mut Option<Forced>, preview: Option<GammaTarget>, focused: Option<(&str, bool)>) -> (GammaTarget, Option<&'static str>) {
    if let Some(p) = preview {
        return (p, None);
    }
    let n = &cfg.night_light;
    let (lat, lon) = location(cfg);
    let sched = phase(n, Local::now(), lat, lon);
    let night_side = sched.factor >= NIGHT_SIDE;
    if forced.is_some_and(|f| f.night != night_side) {
        *forced = None;
    }
    let target = match forced {
        Some(Forced { on: true, .. }) => night_target(cfg),
        Some(Forced { on: false, .. }) => GammaTarget::NEUTRAL,
        None if n.enabled => sched.target,
        None => GammaTarget::NEUTRAL,
    };
    if target.is_neutral() {
        return (target, None);
    }
    let paused = match focused {
        Some((_, true)) if n.pause_fullscreen => Some(tk("a fullscreen window is focused")),
        Some((app, _)) if n.excludes(app) => Some(tk("an excluded app is focused")),
        _ => None,
    };
    if paused.is_some() { (GammaTarget::NEUTRAL, paused) } else { (target, None) }
}

/// Re-evaluates and sends a new target if it changed.
fn evaluate(fade: Duration, cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let focused = crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| (c.app_id.clone(), c.fullscreen));
    let st = state(cx);
    if st.preview.is_some_and(|(until, _)| Instant::now() >= until) {
        st.preview = None;
    }
    let preview = st.preview.map(|(_, p)| p);
    let mut forced = st.forced;
    let (target, paused) = desired(&cfg, &mut forced, preview, focused.as_ref().map(|(a, f)| (a.as_str(), *f)));
    st.forced = forced;
    st.paused = paused;
    if st.sent == Some(target) || (st.sent.is_none() && target.is_neutral()) {
        return;
    }
    if st.gamma.is_none() && !target.is_neutral() {
        st.gamma = Some(Gamma::spawn());
    }
    if let Some(g) = &st.gamma {
        g.set(target, if preview.is_some() { PREVIEW_FADE } else { fade });
        st.sent = Some(target);
    }
}

/// Starts the controller (the gamma thread itself waits until needed).
pub fn init(cx: &mut App) {
    cx.set_global(NightLight::default());
    let store = config(cx);
    cx.observe(&store, |_, cx| evaluate(TOGGLE_FADE, cx)).detach();
    let hub = crate::hub::hub(cx);
    cx.observe(&hub, |_, cx| evaluate(TOGGLE_FADE, cx)).detach();
    cx.spawn(async move |cx| loop {
        let _ = cx.update(|cx| evaluate(STEP_FADE, cx));
        cx.background_executor().timer(TICK).await;
    })
    .detach();
}

/// Whether the screens are warmed right now.
pub fn active(cx: &App) -> bool {
    cx.try_global::<NightLight>().and_then(|n| n.sent).is_some_and(|t| !t.is_neutral())
}

/// Flips what the screens show until the schedule next changes.
pub fn toggle(cx: &mut App) -> bool {
    let cfg = config(cx).read(cx).snapshot();
    let (lat, lon) = location(&cfg);
    let night = phase(&cfg.night_light, Local::now(), lat, lon).factor >= NIGHT_SIDE;
    let on = !active(cx);
    state(cx).forced = Some(Forced { on, night });
    evaluate(TOGGLE_FADE, cx);
    announce(on, cx);
    on
}

/// The night-light OSD (Studio → On-screen displays → Night light) for a
/// manual switch: on / off and the night temperature.
fn announce(on: bool, cx: &mut App) {
    let kelvin = on.then(|| config(cx).read(cx).get().night_light.night_temperature);
    crate::osd::show(crate::osd::OsdEvent::night_light(on, kelvin), cx);
}

fn support(cx: &mut App) -> GammaSupport {
    let st = state(cx);
    if let Some(g) = &st.gamma {
        return g.status().support;
    }
    *st.support.get_or_insert_with(probe)
}

fn status(cx: &mut App) -> serde_json::Value {
    let cfg = config(cx).read(cx).snapshot();
    let n = &cfg.night_light;
    let (lat, lon) = location(&cfg);
    let now = Local::now();
    let sched = phase(n, now, lat, lon);
    let support = support(cx);
    let st = state(cx);
    let gamma = st.gamma.as_ref().map(|g| g.status()).unwrap_or_default();
    let next = if n.enabled { next_change(sched.window, now) } else { None };
    json!({
        "support": support,
        "enabled": n.enabled,
        "active": st.sent.is_some_and(|t| !t.is_neutral()),
        "temperature": st.sent.map(|t| t.temperature).unwrap_or(defaults::NEUTRAL_TEMPERATURE_K),
        "schedule": n.schedule,
        "night_factor": sched.factor,
        "window": match sched.window {
            NightWindow::Between { start, end } => json!({ "start": hm(start), "end": hm(end) }),
            NightWindow::Never => json!("never"),
            NightWindow::Always => json!("always"),
        },
        "override": st.forced.map(|f| if f.on { "on" } else { "off" }),
        "paused": st.paused,
        "next_change": next.map(|(at, to_night)| json!({ "at": at.format("%H:%M").to_string(), "to_night": to_night })),
        "outputs": gamma.outputs,
        "held_by_another_app": gamma.busy,
        "location": { "latitude": lat, "longitude": lon },
    })
}

fn hm(minutes: f64) -> String {
    let m = minutes.round() as i64;
    format!("{:02}:{:02}", m.div_euclid(60).rem_euclid(24), m.rem_euclid(60))
}

fn unsupported_error(cx: &mut App) -> Option<String> {
    match support(cx) {
        GammaSupport::Unsupported => Some(t("This compositor has no gamma control (wlr-gamma-control); Niri, GNOME and KDE can't do Mandara's night light").into()),
        GammaSupport::NoDisplay => Some(t("No Wayland display").into()),
        _ => None,
    }
}

pub fn register(r: &mut PluginRegistry) {
    let owner = "night-light";
    r.command(
        owner,
        CommandSpec::new("nightlight", tk("Night light: on (warm now and follow the schedule), off, toggle until the next change, or auto (drop the toggle)"), |a, cx| {
            if let Some(err) = unsupported_error(cx) {
                return Err(err);
            }
            match a.str(0)? {
                "on" => {
                    update_config(cx, |c| c.night_light.enabled = true);
                    let cfg = config(cx).read(cx).snapshot();
                    let (lat, lon) = location(&cfg);
                    let night = phase(&cfg.night_light, Local::now(), lat, lon).factor >= NIGHT_SIDE;
                    // Warm right away even in daytime (until the schedule says night).
                    state(cx).forced = (!night).then_some(Forced { on: true, night });
                    evaluate(TOGGLE_FADE, cx);
                    announce(true, cx);
                }
                "off" => {
                    state(cx).forced = None;
                    update_config(cx, |c| c.night_light.enabled = false);
                    evaluate(TOGGLE_FADE, cx);
                    announce(false, cx);
                }
                "toggle" => {
                    toggle(cx);
                }
                _ => {
                    state(cx).forced = None;
                    evaluate(TOGGLE_FADE, cx);
                }
            }
            Ok(status(cx))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on / off / toggle / auto")).choices(&["on", "off", "toggle", "auto"]))
        .example("nightlight toggle"),
    );
    r.command(owner, CommandSpec::new("nightlight status", tk("Night light state, schedule and next change (JSON)"), |_, cx| Ok(status(cx))));
    r.command(
        owner,
        CommandSpec::new("nightlight temperature", tk("Set the night colour temperature in kelvin (1000–10000)"), |a, cx| {
            let k = a.int(0)?.clamp(defaults::MIN_TEMPERATURE_K as i64, defaults::MAX_TEMPERATURE_K as i64) as u32;
            update_config(cx, |c| c.night_light.night_temperature = k);
            Ok(json!({ "night_temperature": k }))
        })
        .arg(ArgSpec::required("kelvin", ArgKind::Integer, "1000-10000"))
        .example("nightlight temperature 3800"),
    );
    r.command(
        owner,
        CommandSpec::new("nightlight schedule", tk("Night light schedule: sunset to sunrise, fixed times, or always"), |a, cx| {
            let schedule = match a.str(0)? {
                "fixed" => NightSchedule::Fixed,
                "always" => NightSchedule::Always,
                _ => NightSchedule::Sun,
            };
            update_config(cx, |c| c.night_light.schedule = schedule);
            Ok(status(cx))
        })
        .arg(ArgSpec::required("schedule", ArgKind::Enum, tk("sun / fixed / always")).choices(&["sun", "fixed", "always"])),
    );
    r.command(
        owner,
        CommandSpec::new("nightlight preview", tk("Show a colour temperature for a few seconds (default 3, at most 10)"), |a, cx| {
            if let Some(err) = unsupported_error(cx) {
                return Err(err);
            }
            let k = a.int(0)?.clamp(defaults::MIN_TEMPERATURE_K as i64, defaults::MAX_TEMPERATURE_K as i64) as u32;
            let secs = a.get(1).map(|s| s.parse::<f64>().map_err(|_| t("Seconds must be a number").to_string())).transpose()?.unwrap_or(PREVIEW_SECONDS).clamp(0.5, MAX_PREVIEW_SECONDS);
            let cfg = config(cx).read(cx).snapshot();
            let target = GammaTarget { temperature: k, gamma: cfg.night_light.gamma, brightness: cfg.night_light.brightness };
            state(cx).preview = Some((Instant::now() + Duration::from_secs_f64(secs), target));
            evaluate(PREVIEW_FADE, cx);
            let wait = Duration::from_secs_f64(secs);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(wait).await;
                let _ = cx.update(|cx| evaluate(TOGGLE_FADE, cx));
            })
            .detach();
            Ok(json!({ "preview": k, "seconds": secs }))
        })
        .arg(ArgSpec::required("kelvin", ArgKind::Integer, "1000-10000"))
        .arg(ArgSpec::optional("seconds", ArgKind::Number, tk("how long")))
        .example("nightlight preview 3400"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool) -> ShellConfig {
        let mut c = ShellConfig::default();
        c.night_light.enabled = enabled;
        c.night_light.schedule = NightSchedule::Always;
        c
    }

    #[test]
    fn schedule_override_and_pauses() {
        let mut forced = None;
        assert!(desired(&cfg(false), &mut forced, None, None).0.is_neutral());
        let (on, paused) = desired(&cfg(true), &mut forced, None, None);
        assert_eq!(on.temperature, defaults::NIGHT_TEMPERATURE_K);
        assert!(paused.is_none());
        // Forced off while the schedule says night.
        let mut forced = Some(Forced { on: false, night: true });
        assert!(desired(&cfg(true), &mut forced, None, None).0.is_neutral());
        // Forced on while disabled.
        let mut forced = Some(Forced { on: true, night: true });
        assert!(!desired(&cfg(false), &mut forced, None, None).0.is_neutral());
        // An override from the other side of the schedule expires.
        let mut forced = Some(Forced { on: false, night: false });
        assert!(!desired(&cfg(true), &mut forced, None, None).0.is_neutral());
        assert!(forced.is_none());
        // Pauses.
        let mut c = cfg(true);
        c.night_light.pause_fullscreen = true;
        c.night_light.excluded_apps = vec!["org.gimp.GIMP".into()];
        let mut forced = None;
        assert_eq!(desired(&c, &mut forced, None, Some(("mpv", true))), (GammaTarget::NEUTRAL, Some("a fullscreen window is focused")));
        assert!(desired(&c, &mut forced, None, Some(("org.gimp.GIMP", false))).1.is_some());
        assert!(desired(&c, &mut forced, None, Some(("foot", false))).1.is_none());
        // Previews win over everything.
        let p = GammaTarget { temperature: 3000, ..GammaTarget::NEUTRAL };
        assert_eq!(desired(&cfg(false), &mut None, Some(p), None).0, p);
    }

    #[test]
    fn minutes_format() {
        assert_eq!(hm(19.0 * 60.0 + 7.4), "19:07");
        assert_eq!(hm(-30.0), "23:30");
    }
}
