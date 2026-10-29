//! What happens when you step away (`[idle]`): fade → lock → screens off →
//! sleep, with separate timeouts on mains power and battery, your own
//! commands after N idle seconds, and a shorter screens-off timeout while
//! locked.
//!
//! Idleness comes from the compositor (`ext-idle-notify-v1`, see
//! [`mandara_services::idle`]), so apps' idle inhibitors (a fullscreen
//! video) are honoured for free. On top of those, Mandara keeps the
//! computer awake while **Keep awake** (caffeine) is on or — with
//! `idle.respect_media` — while music or video plays over MPRIS.
//!
//! The timer plan is a pure function ([`plan`]) recomputed whenever the
//! config, power source, lock state or inhibitors change; re-arming
//! restarts every timer from "now", which is exactly the right semantics
//! after an inhibitor ends.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::power::{HookEvent, IdleConfig};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::hooks;
use mandara_services::idle::{IdleCapabilities, IdleEvent, IdleHandle, IdleTimer};
use serde::Serialize;
use serde_json::json;

use super::dim::{self, DimStage};
use crate::state::config;

/// Owner id of the idle commands.
const OWNER: &str = "core";
/// "Idle since" resolution: a probe timer this short tells when the user
/// stopped interacting (and drives the idle / resumed hooks).
const STATUS_PROBE: Duration = Duration::from_secs(5);
/// Seconds per minute (keep-awake durations are given in minutes).
const SECS_PER_MINUTE: u64 = 60;
/// Longest keep-awake duration accepted, minutes (one day).
const MAX_KEEP_AWAKE_MIN: i64 = 24 * 60;
/// Preview fade length when `idle.fade_s` is 0.
const PREVIEW_FADE: Duration = Duration::from_secs(4);
/// Ids of custom behaviours start here (`Stage::id`).
const CUSTOM_BASE: u32 = 100;

/// One idle stage.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Short probe for the status readout and idle / resumed hooks.
    Status,
    /// Start fading (`fade_s` before the first of lock / screens off).
    Fade,
    Lock,
    ScreensOff,
    Suspend,
    /// Screens off while locked (ignores app inhibitors).
    LockedScreensOff,
    /// `idle.behaviors[i]`.
    Custom(usize),
}

impl Stage {
    pub fn id(self) -> u32 {
        match self {
            Stage::Status => 1,
            Stage::Fade => 2,
            Stage::Lock => 3,
            Stage::ScreensOff => 4,
            Stage::Suspend => 5,
            Stage::LockedScreensOff => 6,
            Stage::Custom(i) => CUSTOM_BASE + u32::try_from(i).unwrap_or(u32::MAX - CUSTOM_BASE),
        }
    }

    pub fn from_id(id: u32) -> Option<Stage> {
        Some(match id {
            1 => Stage::Status,
            2 => Stage::Fade,
            3 => Stage::Lock,
            4 => Stage::ScreensOff,
            5 => Stage::Suspend,
            6 => Stage::LockedScreensOff,
            n if n >= CUSTOM_BASE => Stage::Custom((n - CUSTOM_BASE) as usize),
            _ => return None,
        })
    }
}

/// Why idle actions are held back right now.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Inhibitors {
    pub keep_awake: bool,
    pub media: bool,
}

impl Inhibitors {
    pub fn any(self) -> bool {
        self.keep_awake || self.media
    }
}

fn secs(s: u32) -> Option<Duration> {
    (s > 0).then(|| Duration::from_secs(u64::from(s)))
}

/// The timers to arm for this situation.
pub fn plan(cfg: &IdleConfig, on_battery: bool, inhibitors: Inhibitors, locked: bool) -> Vec<(Stage, IdleTimer)> {
    let mut out = Vec::new();
    if !cfg.enabled {
        return out;
    }
    let mut push = |stage: Stage, timeout: Duration, ignore_inhibitors: bool| out.push((stage, IdleTimer { id: stage.id(), timeout, ignore_inhibitors }));
    push(Stage::Status, STATUS_PROBE, false);
    let locked_off = secs(cfg.locked_screen_off_s);
    if locked && let Some(d) = locked_off {
        // While locked the screens go dark even if a video holds idle off.
        push(Stage::LockedScreensOff, d, true);
    }
    if inhibitors.any() {
        return out;
    }
    let t = cfg.timeouts(on_battery);
    let lock = secs(t.lock_s).filter(|_| !locked);
    let screens = secs(t.screen_off_s).filter(|_| !(locked && locked_off.is_some()));
    if let Some(fade) = secs(cfg.fade_s)
        && let Some(first) = [lock, screens].into_iter().flatten().min()
        && first > fade
    {
        push(Stage::Fade, first - fade, false);
    }
    if let Some(d) = lock {
        push(Stage::Lock, d, false);
    }
    if let Some(d) = screens {
        push(Stage::ScreensOff, d, false);
    }
    if let Some(d) = secs(t.suspend_s) {
        push(Stage::Suspend, d, false);
    }
    for (i, b) in cfg.behaviors.iter().enumerate() {
        if b.enabled && !b.command.trim().is_empty() && b.power.applies(on_battery)
            && let Some(d) = secs(b.timeout_s)
        {
            push(Stage::Custom(i), d, false);
        }
    }
    out
}

/// The idle manager (one per shell).
pub struct IdleManager {
    handle: Option<IdleHandle>,
    caps: Option<IdleCapabilities>,
    error: Option<String>,
    armed: Vec<(Stage, IdleTimer)>,
    /// When each armed timer was (re)started, for "next stage in".
    armed_at: Instant,
    idle_since: Option<Instant>,
    /// The `idle` hook ran (the first real stage was reached); `resumed`
    /// runs on the next input.
    idle_announced: bool,
    screens_off: bool,
    locked: bool,
    inhibitors: Inhibitors,
    /// Keep-awake ends at this moment (`None`: until switched off).
    keep_awake_until: Option<Instant>,
    keep_awake_timer: Option<Task<()>>,
}

#[derive(Clone)]
struct GlobalIdle(Entity<IdleManager>);
impl Global for GlobalIdle {}

pub fn manager(cx: &App) -> Option<Entity<IdleManager>> {
    cx.try_global::<GlobalIdle>().map(|g| g.0.clone())
}

fn media_playing(cx: &App) -> bool {
    crate::hub::hub(cx).read(cx).media.as_ref().is_some_and(|m| m.status == mandara_services::PlaybackStatus::Playing)
}

impl IdleManager {
    fn current_inhibitors(cx: &App) -> Inhibitors {
        let cfg = config(cx).read(cx).get();
        Inhibitors { keep_awake: crate::plugins::quick_settings::keep_awake_active(cx), media: cfg.idle.respect_media && media_playing(cx) }
    }

    /// Recomputes the plan; re-arms only when it changed.
    fn rearm(&mut self, force: bool, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        self.inhibitors = Self::current_inhibitors(cx);
        if !self.inhibitors.keep_awake {
            // Switched off elsewhere (the quick-settings tile).
            self.keep_awake_until = None;
            self.keep_awake_timer = None;
        }
        self.locked = crate::lock::is_locked();
        if cfg.idle.enabled && self.handle.is_none() {
            self.start(cx);
        }
        let next = plan(&cfg.idle, super::battery::on_battery(cx), self.inhibitors, self.locked);
        if !force && next == self.armed {
            return;
        }
        if let Some(h) = &self.handle {
            h.set_timers(next.iter().map(|(_, t)| *t).collect());
        }
        self.armed = next;
        self.armed_at = Instant::now();
        if !cfg.idle.enabled {
            // Off: drop the Wayland client entirely.
            self.handle = None;
            self.caps = None;
            self.idle_since = None;
            dim::stop(cx);
        }
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let (handle, events) = mandara_services::idle::spawn();
        self.handle = Some(handle);
        self.error = None;
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |m, cx| m.on_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_event(&mut self, event: IdleEvent, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        match event {
            IdleEvent::Ready(caps) => {
                self.caps = Some(caps);
                self.rearm(true, cx);
            }
            IdleEvent::Failed(e) => {
                tracing::warn!("idle: {e}");
                self.error = Some(e);
                self.handle = None;
            }
            IdleEvent::Screens { on, ok } => {
                if !ok {
                    screens_via_compositor(on, cx);
                }
            }
            IdleEvent::Idled(id) if Stage::from_id(id).is_some_and(|s| s != Stage::Status) && !self.idle_announced => {
                // `idle` hooks: once the first real stage is reached.
                self.idle_announced = true;
                hooks::emit(HookEvent::Idle, &[("stage", serde_json::to_value(Stage::from_id(id)).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default())]);
                self.on_event(IdleEvent::Idled(id), cx);
                return;
            }
            IdleEvent::Idled(id) => match Stage::from_id(id) {
                Some(Stage::Status) => self.idle_since = Some(Instant::now() - STATUS_PROBE),
                Some(Stage::Fade) => {
                    let t = cfg.idle.timeouts(super::battery::on_battery(cx));
                    let stage = if t.lock_s > 0 && !self.locked && (t.screen_off_s == 0 || t.lock_s <= t.screen_off_s) { DimStage::Lock } else { DimStage::ScreensOff };
                    dim::start(Duration::from_secs(u64::from(cfg.idle.fade_s)), stage, cx);
                }
                Some(Stage::Lock) => {
                    dim::stop(cx);
                    let grace = secs(cfg.idle.lock_grace_s);
                    if cfg.general.lock_screen {
                        if let Err(e) = crate::lock::lock_with_grace(grace) {
                            tracing::error!("idle lock: {e}");
                        }
                    } else {
                        // Another locker answers logind's Lock signal.
                        std::thread::spawn(|| mandara_services::SessionAction::Lock.run());
                    }
                }
                Some(Stage::ScreensOff) | Some(Stage::LockedScreensOff) => {
                    if Stage::from_id(id) == Some(Stage::LockedScreensOff) && !crate::lock::is_locked() {
                        return;
                    }
                    self.set_screens(false, cx);
                }
                Some(Stage::Suspend) => {
                    let mode = cfg.idle.suspend_mode;
                    std::thread::spawn(move || {
                        if let Err(e) = mandara_services::sleep::sleep(mode) {
                            tracing::error!("idle sleep: {e}");
                        }
                    });
                }
                Some(Stage::Custom(i)) => {
                    if let Some(b) = cfg.idle.behaviors.get(i) {
                        run_behavior(&b.command, &b.name, "idle", cfg.hooks.timeout_s);
                    }
                }
                None => {}
            },
            IdleEvent::Resumed(id) => {
                dim::stop(cx);
                if self.idle_announced {
                    self.idle_announced = false;
                    hooks::emit(HookEvent::Resumed, &[]);
                }
                match Stage::from_id(id) {
                    Some(Stage::Status) => self.idle_since = None,
                    Some(Stage::ScreensOff) | Some(Stage::LockedScreensOff) if self.screens_off => self.set_screens(true, cx),
                    Some(Stage::Custom(i)) => {
                        if let Some(b) = cfg.idle.behaviors.get(i).filter(|b| !b.resume_command.trim().is_empty()) {
                            run_behavior(&b.resume_command, &b.name, "resume", cfg.hooks.timeout_s);
                        }
                    }
                    _ => {}
                }
            }
        }
        cx.notify();
    }

    fn set_screens(&mut self, on: bool, cx: &mut Context<Self>) {
        self.screens_off = !on;
        hooks::emit(if on { HookEvent::ScreensOn } else { HookEvent::ScreensOff }, &[]);
        if mandara_services::sleep::dry_run() {
            tracing::info!("dry run: screens {}", if on { "on" } else { "off" });
            return;
        }
        match &self.handle {
            Some(h) => h.set_screens(on),
            None => screens_via_compositor(on, cx),
        }
    }

    fn status(&self, cx: &App) -> serde_json::Value {
        let cfg = config(cx).read(cx).snapshot();
        let on_battery = super::battery::on_battery(cx);
        // Timers count from the later of re-arming and the last input; while
        // active, the last input was just now.
        let counted = self.idle_since.map(|i| i.elapsed().min(self.armed_at.elapsed())).unwrap_or_default();
        let next: Vec<_> = self
            .armed
            .iter()
            .filter(|(s, _)| *s != Stage::Status)
            .map(|(stage, t)| json!({ "stage": stage, "after_s": t.timeout.as_secs(), "in_s": t.timeout.saturating_sub(counted).as_secs() }))
            .collect();
        json!({
            "enabled": cfg.idle.enabled,
            "available": self.caps.is_some_and(|c| c.idle_notify),
            "capabilities": self.caps,
            "error": self.error,
            "idle": self.idle_since.is_some(),
            "idle_for_s": self.idle_since.map(|i| i.elapsed().as_secs()),
            "power_source": if on_battery { "battery" } else { "ac" },
            "timeouts": cfg.idle.timeouts(on_battery),
            "inhibited_by": self.inhibited_by(),
            "keep_awake_for_s": self.keep_awake_until.map(|u| u.saturating_duration_since(Instant::now()).as_secs()),
            "locked": self.locked,
            "screens_off": self.screens_off,
            "fading": dim::active(cx),
            "stages": next,
            "dry_run": mandara_services::sleep::dry_run(),
        })
    }

    fn inhibited_by(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.inhibitors.keep_awake {
            out.push("keep-awake");
        }
        if self.inhibitors.media {
            out.push("media");
        }
        out
    }

    /// Seconds idle so far (None while active or when unknown).
    /// The screens are off (idle stage or `mandara msg idle screens off`).
    pub fn screens_off(&self) -> bool {
        self.screens_off
    }

    pub fn idle_for(&self) -> Option<Duration> {
        self.idle_since.map(|i| i.elapsed())
    }

    pub fn inhibitors(&self) -> Inhibitors {
        self.inhibitors
    }

    pub fn available(&self) -> Option<bool> {
        self.caps.map(|c| c.idle_notify)
    }
}

/// Runs an idle behaviour's command (no shell; dry runs only log) within
/// the hooks' time limit.
fn run_behavior(command: &str, name: &str, phase: &'static str, timeout_s: u32) {
    if mandara_services::sleep::dry_run() {
        tracing::info!("dry run: idle behaviour {name} ({phase}): {command}");
        return;
    }
    let timeout = Duration::from_secs(u64::from(timeout_s.max(1)));
    hooks::run_command(if phase == "idle" { "idle-behavior" } else { "idle-behavior-resume" }, command, &[("behavior", name.to_string())], timeout);
}

/// Compositor fallback when wlr-output-power-management is missing.
fn screens_via_compositor(on: bool, cx: &App) {
    use mandara_compositor::CompositorKind;
    let Some(backend) = crate::hub::hub(cx).read(cx).compositor.clone() else {
        tracing::warn!("idle: cannot switch screens (no output power protocol, no compositor IPC)");
        return;
    };
    let result = match backend.kind() {
        CompositorKind::Niri => backend.dispatch_raw(if on { "PowerOnMonitors" } else { "PowerOffMonitors" }, &[]),
        CompositorKind::Hyprland => backend.dispatch_raw("dpms", &[if on { "on" } else { "off" }]),
        CompositorKind::MangoWM => {
            tracing::warn!("idle: MangoWM without wlr-output-power-management; screens stay on");
            Ok(())
        }
    };
    if let Err(e) = result {
        tracing::warn!("idle: screens {}: {e:#}", if on { "on" } else { "off" });
    }
}

/// Keep-awake for `minutes` (`0` switches it off, `None` = until off).
fn keep_awake(minutes: Option<u64>, cx: &mut App) -> Result<serde_json::Value, String> {
    let Some(m) = manager(cx) else { return Err(t("The idle manager is not running").into()) };
    let on = minutes != Some(0);
    crate::plugins::quick_settings::set_keep_awake(on, cx)?;
    m.update(cx, |m, cx| {
        m.keep_awake_timer = None;
        m.keep_awake_until = None;
        if let Some(mins) = minutes.filter(|m| *m > 0) {
            let d = Duration::from_secs(mins * SECS_PER_MINUTE);
            m.keep_awake_until = Some(Instant::now() + d);
            m.keep_awake_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(d).await;
                let _ = this.update(cx, |m, cx| {
                    m.keep_awake_until = None;
                    m.keep_awake_timer = None;
                    let _ = crate::plugins::quick_settings::set_keep_awake(false, cx);
                    m.rearm(false, cx);
                });
            }));
        }
        m.rearm(false, cx);
    });
    Ok(json!({ "keep_awake": on, "minutes": minutes.filter(|m| *m > 0) }))
}

/// Starts the idle manager (shell process).
pub fn init(cx: &mut App) {
    let entity = cx.new(|_| IdleManager {
        handle: None,
        caps: None,
        error: None,
        armed: Vec::new(),
        armed_at: Instant::now(),
        idle_since: None,
        idle_announced: false,
        screens_off: false,
        locked: false,
        inhibitors: Inhibitors::default(),
        keep_awake_until: None,
        keep_awake_timer: None,
    });
    cx.set_global(GlobalIdle(entity.clone()));
    entity.update(cx, |m, cx| m.rearm(true, cx));

    // Anything that changes the plan re-arms (no-op when unchanged).
    let rearm = |e: &Entity<IdleManager>, cx: &mut App| e.update(cx, |m, cx| m.rearm(false, cx));
    let e = entity.clone();
    cx.observe(&config(cx), move |_, cx| rearm(&e, cx)).detach();
    let e = entity.clone();
    cx.observe(&crate::hub::hub(cx), move |_, cx| rearm(&e, cx)).detach();
    if let Some(p) = super::battery::policy(cx) {
        let e = entity.clone();
        cx.observe(&p, move |_, cx| rearm(&e, cx)).detach();
    }
    let e = entity.clone();
    cx.observe_global::<crate::plugins::quick_settings::KeepAwake>(move |cx| rearm(&e, cx)).detach();
    let locks = crate::lock::subscribe();
    let e = entity.downgrade();
    cx.spawn(async move |cx| {
        while locks.recv().await.is_ok() {
            if e.update(cx, |m, cx| m.rearm(false, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new("idle status", tk("Idle state: idle time, inhibitors, power source and the stages to come (JSON)"), |_, cx| {
            let m = manager(cx).ok_or_else(|| t("The idle manager is not running").to_string())?;
            Ok(m.read(cx).status(cx))
        })
        .example("idle status"),
    );
    r.command(
        OWNER,
        CommandSpec::new("idle enable", tk("Turn Mandara's idle stages (fade, lock, screens off, sleep) on or off"), |a, cx| {
            let current = config(cx).read(cx).get().idle.enabled;
            let on = match a.str(0)? {
                "on" => true,
                "off" => false,
                _ => !current,
            };
            crate::state::update_config(cx, |c| c.idle.enabled = on);
            Ok(json!({ "enabled": on }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on, off or toggle")).choices(&["on", "off", "toggle"]))
        .example("idle enable on"),
    );
    r.command(
        OWNER,
        CommandSpec::new("idle keep-awake", tk("Keep the computer awake for N minutes (0 stops; no number = until turned off)"), |a, cx| {
            let minutes = match a.get(0) {
                None => None,
                Some(_) => Some(a.int(0)?.clamp(0, MAX_KEEP_AWAKE_MIN) as u64),
            };
            keep_awake(minutes, cx)
        })
        .arg(ArgSpec::optional("minutes", ArgKind::Integer, tk("how long, in minutes")))
        .example("idle keep-awake 30")
        .example("idle keep-awake 0"),
    );
    r.command(
        OWNER,
        CommandSpec::new("idle fade-preview", tk("Preview the fade shown before locking (move the mouse or wait to end it)"), |_, cx| {
            let fade = secs(config(cx).read(cx).get().idle.fade_s).unwrap_or(PREVIEW_FADE);
            dim::start(fade, DimStage::Preview, cx);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(fade + PREVIEW_FADE).await;
                cx.update(dim::stop);
            })
            .detach();
            Ok(json!({ "seconds": fade.as_secs() }))
        })
        .example("idle fade-preview"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::power::{IdleBehavior, IdleTimeouts, PowerSource};

    fn stages(p: &[(Stage, IdleTimer)]) -> Vec<(Stage, u64)> {
        p.iter().map(|(s, t)| (*s, t.timeout.as_secs())).collect()
    }

    fn cfg() -> IdleConfig {
        IdleConfig {
            enabled: true,
            ac: IdleTimeouts { lock_s: 600, screen_off_s: 900, suspend_s: 0 },
            battery: IdleTimeouts { lock_s: 300, screen_off_s: 240, suspend_s: 900 },
            fade_s: 10,
            locked_screen_off_s: 30,
            ..Default::default()
        }
    }

    #[test]
    fn plans_follow_power_source() {
        let c = cfg();
        assert_eq!(stages(&plan(&c, false, Inhibitors::default(), false)), [(Stage::Status, 5), (Stage::Fade, 590), (Stage::Lock, 600), (Stage::ScreensOff, 900)]);
        // On battery the screens go off first: the fade leads to that.
        assert_eq!(stages(&plan(&c, true, Inhibitors::default(), false)), [(Stage::Status, 5), (Stage::Fade, 230), (Stage::Lock, 300), (Stage::ScreensOff, 240), (Stage::Suspend, 900)]);
        assert!(plan(&IdleConfig { enabled: false, ..c }, false, Inhibitors::default(), false).is_empty());
    }

    #[test]
    fn inhibitors_and_lock_shape_the_plan() {
        let c = cfg();
        let held = Inhibitors { keep_awake: false, media: true };
        assert_eq!(stages(&plan(&c, false, held, false)), [(Stage::Status, 5)]);
        // Locked: the short screens-off timer ignores app inhibitors.
        let locked = plan(&c, false, held, true);
        assert_eq!(stages(&locked), [(Stage::Status, 5), (Stage::LockedScreensOff, 30)]);
        assert!(locked[1].1.ignore_inhibitors);
        assert_eq!(stages(&plan(&c, false, Inhibitors::default(), true)), [(Stage::Status, 5), (Stage::LockedScreensOff, 30)]);
    }

    #[test]
    fn custom_behaviours_respect_source_and_ids_round_trip() {
        let mut c = cfg();
        c.fade_s = 0;
        c.behaviors.push(IdleBehavior { name: "dim kbd".into(), timeout_s: 60, command: "brightnessctl -d kbd set 0".into(), power: PowerSource::Battery, ..Default::default() });
        c.behaviors.push(IdleBehavior { name: "empty".into(), timeout_s: 60, command: " ".into(), ..Default::default() });
        assert!(!plan(&c, false, Inhibitors::default(), false).iter().any(|(s, _)| matches!(s, Stage::Custom(_))));
        assert!(plan(&c, true, Inhibitors::default(), false).iter().any(|(s, t)| *s == Stage::Custom(0) && t.timeout.as_secs() == 60));
        for s in [Stage::Status, Stage::Fade, Stage::Lock, Stage::ScreensOff, Stage::Suspend, Stage::LockedScreensOff, Stage::Custom(3)] {
            assert_eq!(Stage::from_id(s.id()), Some(s));
        }
    }
}
