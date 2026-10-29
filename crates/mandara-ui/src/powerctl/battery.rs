//! Battery and power-source policy: low / critical alerts (notification
//! and OSD), the critical action, peripheral warnings, power profiles per
//! power source and at low battery, "fully charged" notices, power hooks —
//! plus the locking policy around sleep, lid and session start.
//!
//! The decisions are pure ([`evaluate`]) so they are unit-tested without
//! UPower; the entity only feeds them states and carries out the result.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::power::{AlertStyle, CriticalAction, HookEvent, PowerConfig};
use mandara_config::ShellConfig;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::hooks;
use mandara_services::power::{self, ChargeState, PowerService, PowerState};
use serde::Serialize;
use serde_json::json;

use crate::state::config;

/// Owner id of the power-policy commands.
const OWNER: &str = "power";
/// A warning re-arms once the charge climbs this far above its threshold.
const REARM_MARGIN_PERCENT: u8 = 3;
/// Percent → fraction.
const PERCENT: f32 = 100.0;
/// Poll for the lock screen's "locked" marker before letting sleep proceed.
const LOCK_POLL: Duration = Duration::from_millis(50);
/// Leave this much of logind's delay budget unused (the suspend must not be
/// forced through by the timeout).
const LOCK_WAIT_MARGIN: Duration = Duration::from_millis(500);
/// Notification icon names.
const ICON_BATTERY_LOW: &str = "alert";
const ICON_BATTERY: &str = "battery";
const ICON_PLUG: &str = "zap";
/// File in the runtime dir marking that this login session already locked
/// at startup (the runtime dir is emptied at logout).
const STARTUP_LOCK_MARKER: &str = "startup-locked";

/// What the battery looks like, reduced to what the policy needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    pub present: bool,
    pub percent: u8,
    pub on_battery: bool,
    pub charging: bool,
    pub full: bool,
    /// The charge limit (end threshold), when one is on.
    pub limit: Option<u8>,
}

impl Summary {
    pub fn of(state: &PowerState) -> Self {
        let batteries: Vec<_> = state.batteries().collect();
        if batteries.is_empty() {
            return Self { on_battery: state.on_battery, ..Default::default() };
        }
        // Capacity-weighted charge across batteries (dual-battery laptops).
        let weight: f64 = batteries.iter().map(|b| b.energy_full.max(1.0)).sum();
        let percent = batteries.iter().map(|b| b.percentage * b.energy_full.max(1.0)).sum::<f64>() / weight;
        let limit = batteries.iter().find_map(|b| b.threshold.as_ref().filter(|t| t.enabled).and_then(|t| t.end)).and_then(|e| u8::try_from(e).ok());
        Self {
            present: true,
            percent: percent.round().clamp(0.0, 100.0) as u8,
            on_battery: state.on_battery,
            charging: batteries.iter().any(|b| b.state.is_charging()),
            full: batteries.iter().all(|b| b.state == ChargeState::FullyCharged),
            limit,
        }
    }
}

/// Which warnings already fired (so each fires once per discharge).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Memory {
    pub low: bool,
    pub critical: bool,
    pub action: bool,
    pub full: bool,
    pub saver_applied: bool,
    /// Peripheral device paths already warned about.
    pub peripherals: HashSet<String>,
}

/// One thing the policy wants done.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Low(u8),
    Critical(u8),
    /// Schedule the critical action (after `action_delay_s`).
    ArmAction(CriticalAction),
    /// Charging again: cancel a scheduled action.
    CancelAction,
    Full(u8),
    Peripheral { name: String, percent: u8 },
    /// Switch to this profile (power source change or low battery).
    Profile(String),
    /// Leave power-saver again (charging after `saver_at_low`).
    RestoreProfile,
}

/// Decides what to do for a new reading. Pure: mutates only `memory`.
pub fn evaluate(cfg: &PowerConfig, memory: &mut Memory, prev: Option<Summary>, now: Summary, state: &PowerState) -> Vec<Decision> {
    let mut out = Vec::new();
    // Power source switches apply the per-source profile (which then also
    // wins over restoring the profile from before power-saver).
    let mut source_profile = false;
    if prev.is_none_or(|p| p.on_battery != now.on_battery) {
        let profile = if now.on_battery { &cfg.profile_battery } else { &cfg.profile_ac };
        if let Some(p) = profile.as_ref().filter(|p| !p.is_empty()) {
            out.push(Decision::Profile(p.clone()));
            source_profile = true;
        }
    }
    if now.present {
        let discharging = now.on_battery && !now.charging;
        if discharging {
            if now.percent <= cfg.critical_percent && !memory.critical {
                memory.critical = true;
                memory.low = true;
                out.push(Decision::Critical(now.percent));
            } else if now.percent <= cfg.low_percent && !memory.low {
                memory.low = true;
                out.push(Decision::Low(now.percent));
            }
            if now.percent <= cfg.low_percent && cfg.saver_at_low && !memory.saver_applied {
                memory.saver_applied = true;
                out.push(Decision::Profile("power-saver".into()));
            }
            if now.percent <= cfg.action_percent && cfg.critical_action != CriticalAction::Nothing && !memory.action {
                memory.action = true;
                out.push(Decision::ArmAction(cfg.critical_action));
            }
        } else {
            if memory.action {
                memory.action = false;
                out.push(Decision::CancelAction);
            }
            if memory.saver_applied {
                memory.saver_applied = false;
                if !source_profile {
                    out.push(Decision::RestoreProfile);
                }
            }
        }
        // Re-arm once the charge recovered (or the charger is in).
        let rearm = |threshold: u8| !discharging || now.percent > threshold.saturating_add(REARM_MARGIN_PERCENT);
        if rearm(cfg.low_percent) {
            memory.low = false;
        }
        if rearm(cfg.critical_percent) {
            memory.critical = false;
        }
        let at_limit = now.limit.is_some_and(|l| now.percent >= l);
        if cfg.notify_full && !now.on_battery && (now.full || at_limit) {
            if !memory.full {
                memory.full = true;
                out.push(Decision::Full(now.percent));
            }
        } else if now.on_battery {
            memory.full = false;
        }
    }
    if cfg.peripheral_alerts {
        for d in state.peripherals() {
            let threshold = cfg.peripheral_threshold(&d.model, &d.serial);
            let percent = d.percentage.round().clamp(0.0, 100.0) as u8;
            if d.state.is_charging() || percent > threshold.saturating_add(REARM_MARGIN_PERCENT) {
                memory.peripherals.remove(&d.path);
            } else if percent <= threshold && percent > 0 && memory.peripherals.insert(d.path.clone()) {
                out.push(Decision::Peripheral { name: d.display_name(), percent });
            }
        }
    }
    out
}

/// The policy entity (one per shell).
pub struct PowerPolicy {
    _service: PowerService,
    pub state: PowerState,
    pub summary: Option<Summary>,
    memory: Memory,
    /// The profile active before `saver_at_low` switched to power-saver.
    saver_restore: Option<String>,
    /// The scheduled critical action and when it fires.
    action: Option<(Task<()>, Instant, CriticalAction)>,
    last_profile: Option<String>,
}

#[derive(Clone)]
struct GlobalPolicy(Entity<PowerPolicy>);
impl Global for GlobalPolicy {}

/// The power policy entity (shell only).
pub fn policy(cx: &App) -> Option<Entity<PowerPolicy>> {
    cx.try_global::<GlobalPolicy>().map(|g| g.0.clone())
}

/// Whether the computer runs on battery (false when unknown).
pub fn on_battery(cx: &App) -> bool {
    policy(cx).is_some_and(|p| p.read(cx).state.on_battery)
}

fn notify(summary: String, body: String, icon: &str, cx: &mut App) {
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Power").into(), icon: icon.into(), summary, body });
}

/// Shows a battery alert the way `style` says; its sound follows the
/// `[sounds]` settings (Studio → Audio).
fn alert(style: AlertStyle, critical: bool, summary: String, body: String, percent: u8, cx: &mut App) {
    if style != AlertStyle::Off {
        use mandara_services::sounds::SoundEvent;
        crate::alerts::play_sound(if critical { SoundEvent::BatteryCritical } else { SoundEvent::BatteryLow }, cx);
    }
    if style.notifies() {
        notify(summary, body, ICON_BATTERY_LOW, cx);
    }
    if style.shows_osd() {
        crate::osd::show(crate::osd::OsdEvent::battery(f32::from(percent) / PERCENT, false, f32::from(crate::state::config(cx).read(cx).snapshot().power.low_percent) / PERCENT), cx);
    }
}

fn set_profile(name: String) {
    std::thread::spawn(move || {
        let state = power::snapshot();
        let result = state.profiles.as_ref().ok_or_else(|| t("power-profiles-daemon is not running").to_string()).and_then(|p| {
            if p.active == name {
                return Ok(());
            }
            power::execute(power::plan_set_profile(p, &name)?)
        });
        if let Err(e) = result {
            tracing::warn!("power profile {name}: {e}");
        }
    });
}

/// The critical action to run when its countdown ends: the configured one
/// at that moment, `None` once it was set to "Nothing".
fn action_when_due(cfg: &PowerConfig) -> Option<CriticalAction> {
    (cfg.critical_action != CriticalAction::Nothing).then_some(cfg.critical_action)
}

/// The per-source profile to switch to after `profile_ac` /
/// `profile_battery` changed: the one for the current source, when it
/// changed and names a profile.
fn source_profile_after_edit(before: &PowerConfig, after: &PowerConfig, on_battery: bool) -> Option<String> {
    let pick = |c: &PowerConfig| if on_battery { c.profile_battery.clone() } else { c.profile_ac.clone() };
    let now = pick(after).filter(|p| !p.is_empty());
    (now != pick(before).filter(|p| !p.is_empty())).then_some(now).flatten()
}

fn action_label(action: CriticalAction) -> &'static str {
    match action {
        CriticalAction::Nothing => t("Nothing"),
        CriticalAction::Suspend => t("Sleep"),
        CriticalAction::Hibernate => t("Hibernate"),
        CriticalAction::Shutdown => t("Shut down"),
    }
}

/// Carries out the critical action (dry runs only log).
fn run_action(action: CriticalAction, cfg: &ShellConfig) {
    let mode = cfg.idle.suspend_mode;
    std::thread::spawn(move || {
        let result = match action {
            CriticalAction::Nothing => Ok(()),
            CriticalAction::Suspend => mandara_services::sleep::sleep(mode),
            CriticalAction::Hibernate => mandara_services::sleep::sleep(mandara_config::power::SuspendMode::Hibernate),
            CriticalAction::Shutdown => {
                hooks::emit_and_wait(HookEvent::BeforeShutdown, &[("reason", "critical-battery".into())]);
                if mandara_services::sleep::dry_run() {
                    tracing::info!("dry run: shut down (critical battery)");
                    Ok(())
                } else {
                    mandara_services::SessionAction::Shutdown.run()
                }
            }
        };
        if let Err(e) = result {
            tracing::error!("critical battery action: {e}");
        }
    });
}

impl PowerPolicy {
    fn apply(&mut self, state: PowerState, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        let now = Summary::of(&state);
        let prev = self.summary;
        // Power hooks.
        if let Some(p) = prev {
            if p.on_battery != now.on_battery {
                let event = if now.on_battery { HookEvent::Unplugged } else { HookEvent::PluggedIn };
                hooks::emit(event, &[("power_source", if now.on_battery { "battery" } else { "ac" }.into()), ("battery_percent", now.percent.to_string())]);
            }
            if now.present && p.percent != now.percent {
                hooks::emit(HookEvent::BatteryLevel, &[("battery_percent", now.percent.to_string()), ("battery_charging", now.charging.to_string())]);
            }
        }
        let profile = state.profiles.as_ref().map(|p| p.active.clone());
        if self.last_profile.is_some() && profile.is_some() && profile != self.last_profile {
            hooks::emit(HookEvent::PowerProfileChanged, &[("power_profile", profile.clone().unwrap_or_default())]);
        }
        self.last_profile = profile;

        for decision in evaluate(&cfg.power, &mut self.memory, prev, now, &state) {
            match decision {
                Decision::Low(percent) => {
                    hooks::emit(HookEvent::BatteryLow, &[("battery_percent", percent.to_string())]);
                    alert(cfg.power.low_alert, false, t("Battery low").into(), tf("{percent}% left — plug in soon", &[("percent", &percent)]), percent, cx);
                }
                Decision::Critical(percent) => {
                    hooks::emit(HookEvent::BatteryCritical, &[("battery_percent", percent.to_string())]);
                    alert(cfg.power.critical_alert, true, t("Battery critically low").into(), tf("{percent}% left — plug in now", &[("percent", &percent)]), percent, cx);
                }
                Decision::ArmAction(action) => {
                    let delay = Duration::from_secs(u64::from(cfg.power.action_delay_s));
                    notify(
                        tf("{action} in {seconds} s", &[("action", &action_label(action)), ("seconds", &cfg.power.action_delay_s)]),
                        t("The battery is nearly empty. Plug in the charger to cancel.").into(),
                        ICON_BATTERY_LOW,
                        cx,
                    );
                    let task = cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.action.take().is_some() {
                                let cfg = config(cx).read(cx).snapshot();
                                // The setting as it is now: switching to
                                // "Nothing" during the countdown cancels.
                                if let Some(now) = action_when_due(&cfg.power) {
                                    run_action(now, &cfg);
                                }
                            }
                        });
                    });
                    self.action = Some((task, Instant::now() + delay, action));
                }
                Decision::CancelAction => {
                    if self.action.take().is_some() {
                        notify(t("Charging — critical action cancelled").into(), String::new(), ICON_PLUG, cx);
                    }
                }
                Decision::Full(percent) => {
                    hooks::emit(HookEvent::BatteryFull, &[("battery_percent", percent.to_string())]);
                    notify(t("Battery charged").into(), tf("{percent}% — you can unplug the charger", &[("percent", &percent)]), ICON_BATTERY, cx);
                }
                Decision::Peripheral { name, percent } => {
                    notify(tf("{device} battery low", &[("device", &name)]), tf("{percent}% left", &[("percent", &percent)]), ICON_BATTERY_LOW, cx);
                }
                Decision::Profile(name) => {
                    if name == "power-saver" {
                        self.saver_restore = state.profiles.as_ref().map(|p| p.active.clone()).filter(|a| a != "power-saver");
                    }
                    set_profile(name);
                }
                Decision::RestoreProfile => {
                    if let Some(name) = self.saver_restore.take() {
                        set_profile(name);
                    }
                }
            }
        }
        self.summary = Some(now);
        self.state = state;
        cx.notify();
    }

    /// Seconds until the scheduled critical action, if one is armed.
    pub fn action_in(&self) -> Option<(u64, CriticalAction)> {
        self.action.as_ref().map(|(_, at, a)| (at.saturating_duration_since(Instant::now()).as_secs(), *a))
    }
}

/// `true` while the battery is at or below the critical threshold and
/// discharging (the bar chip pulses then).
pub fn critical(cx: &App) -> bool {
    let cfg = config(cx).read(cx).get().power.clone();
    policy(cx).and_then(|p| p.read(cx).summary).is_some_and(|s| cfg.critical_pulse && s.present && s.on_battery && !s.charging && s.percent <= cfg.critical_percent)
}

/// Before suspending, within logind's delay `budget`: start the lock
/// first (it returns once the locker is spawned), run the before-sleep
/// hooks in what's left (each capped at it), then wait for the lock to be
/// up. Hooks used to run first, so one slow hook could use up the delay
/// and the machine slept unlocked. `Some(confirmed)` when locking.
fn before_sleep(budget: Duration, lock: bool, lock_now: impl FnOnce() -> Result<(), String>, run_hooks: impl FnOnce(Duration), wait_locked: impl FnOnce(Duration) -> bool) -> Option<bool> {
    let started = Instant::now();
    let locking = lock
        && match lock_now() {
            Ok(()) => true,
            Err(e) => {
                tracing::error!("lock before sleep: {e}");
                false
            }
        };
    run_hooks(budget.saturating_sub(started.elapsed()));
    locking.then(|| wait_locked(budget.saturating_sub(started.elapsed())))
}

/// Waits until the lock screen reports the session locked, at most `max`.
fn wait_until_locked(max: Duration) -> bool {
    let deadline = Instant::now() + max;
    let marker = crate::lock::locked_marker();
    while Instant::now() < deadline {
        if marker.exists() {
            return true;
        }
        std::thread::sleep(LOCK_POLL);
    }
    false
}

fn startup_lock(cfg: &ShellConfig) {
    if !cfg.idle.lock_at_startup || !cfg.general.lock_screen {
        return;
    }
    let marker = dirs::runtime_dir().unwrap_or_else(mandara_config::paths::cache_dir).join("mandara").join(STARTUP_LOCK_MARKER);
    if marker.exists() {
        return; // a shell restart, not a new login
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&marker, std::env::var("XDG_SESSION_ID").unwrap_or_default());
    if let Err(e) = crate::lock::lock_now() {
        tracing::error!("lock at startup: {e}");
    }
}

/// Starts the policy (shell process).
pub fn init(cx: &mut App) {
    let service = power::spawn_power_service();
    let updates = service.updates.clone();
    let entity = cx.new(|_| PowerPolicy { _service: service, state: PowerState::default(), summary: None, memory: Memory::default(), saver_restore: None, action: None, last_profile: None });
    cx.set_global(GlobalPolicy(entity.clone()));
    let weak = entity.downgrade();
    cx.spawn(async move |cx| {
        while let Ok(mut state) = updates.recv().await {
            while let Ok(newer) = updates.try_recv() {
                state = newer;
            }
            if !state.ready {
                continue;
            }
            if weak.update(cx, |p, cx| p.apply(state, cx)).is_err() {
                break;
            }
        }
    })
    .detach();

    // Lock before sleep / on lid close follow the config through atomics
    // (the logind watchers run on the service runtime).
    let lock_sleep = Arc::new(AtomicBool::new(false));
    let lock_lid = Arc::new(AtomicBool::new(false));
    let sync = {
        let (lock_sleep, lock_lid) = (lock_sleep.clone(), lock_lid.clone());
        move |cfg: &ShellConfig| {
            lock_sleep.store(cfg.idle.lock_before_sleep && cfg.general.lock_screen, Ordering::Relaxed);
            lock_lid.store(cfg.idle.lock_on_lid_close && cfg.general.lock_screen, Ordering::Relaxed);
        }
    };
    let store = config(cx);
    let cfg = store.read(cx).snapshot();
    sync(&cfg);
    cx.observe(&store, move |store, cx| sync(store.read(cx).get())).detach();
    // Editing "On mains power" / "On battery" applies the profile for the
    // current source right away (not only at the next plug / unplug), and
    // changed thresholds are evaluated without waiting for UPower.
    let mut last_power = cfg.power.clone();
    let policy_entity = entity.clone();
    cx.observe(&store, move |store, cx| {
        let now = store.read(cx).get().power.clone();
        if now == last_power {
            return;
        }
        let before = std::mem::replace(&mut last_power, now.clone());
        policy_entity.update(cx, |p, cx| {
            if let Some(summary) = p.summary
                && let Some(profile) = source_profile_after_edit(&before, &now, summary.on_battery)
            {
                set_profile(profile);
            }
            if p.state.ready {
                p.apply(p.state.clone(), cx);
            }
        });
    })
    .detach();

    let before = lock_sleep.clone();
    let (resumed_tx, resumed_rx) = async_channel::unbounded::<()>();
    mandara_services::sleep::watch_sleep(
        move || before.load(Ordering::Relaxed) || hooks::has_rules(HookEvent::BeforeSleep),
        {
            let lock_sleep = lock_sleep.clone();
            move || {
                let budget = mandara_services::sleep::inhibit_delay_max().saturating_sub(LOCK_WAIT_MARGIN);
                let locked = before_sleep(
                    budget,
                    lock_sleep.load(Ordering::Relaxed),
                    || crate::lock::lock_now().map(|_| ()),
                    |left| {
                        hooks::emit_and_wait_within(HookEvent::BeforeSleep, &[], left);
                    },
                    wait_until_locked,
                );
                if locked == Some(false) {
                    tracing::warn!("lock before sleep: the lock was not confirmed in time");
                }
            }
        },
        {
            let resumed = resumed_tx;
            move || {
                hooks::emit(HookEvent::AfterSleep, &[]);
                let _ = resumed.try_send(());
            }
        },
    );
    // Back from sleep: the shell's own resume work (Bluetooth reconnects).
    cx.spawn(async move |cx| {
        while resumed_rx.recv().await.is_ok() {
            cx.update(crate::plugins::system_admin::after_resume);
        }
    })
    .detach();
    mandara_services::sleep::watch_lid(move |closed| {
        if closed
            && lock_lid.load(Ordering::Relaxed)
            && let Err(e) = crate::lock::lock_now()
        {
            tracing::error!("lock on lid close: {e}");
        }
    });
    startup_lock(&cfg);
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new("power alerts", tk("Battery alert thresholds, the battery summary and a pending critical action (JSON)"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let p = policy(cx).ok_or_else(|| t("Power policy is not running").to_string())?;
            let p = p.read(cx);
            Ok(json!({
                "summary": p.summary,
                "low_percent": cfg.power.low_percent,
                "critical_percent": cfg.power.critical_percent,
                "critical_action": cfg.power.critical_action,
                "action_percent": cfg.power.action_percent,
                "pending_action": p.action_in().map(|(s, a)| json!({ "action": a, "in_s": s })),
                "profile_ac": cfg.power.profile_ac,
                "profile_battery": cfg.power.profile_battery,
                "saver_at_low": cfg.power.saver_at_low,
            }))
        })
        .example("power alerts"),
    );
    r.command(
        OWNER,
        CommandSpec::new("power alert-test", tk("Show a sample low or critical battery alert (nothing else happens)"), |a, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let level = a.str(0)?;
            let critical = level == "critical";
            let (style, summary, percent) = if critical {
                (cfg.power.critical_alert, t("Battery critically low"), cfg.power.critical_percent)
            } else {
                (cfg.power.low_alert, t("Battery low"), cfg.power.low_percent)
            };
            alert(style, critical, summary.into(), tf("{percent}% left — this is a test", &[("percent", &percent)]), percent, cx);
            Ok(json!({ "shown": level, "style": style }))
        })
        .arg(ArgSpec::required("level", ArgKind::Enum, tk("which alert")).choices(&["low", "critical"]))
        .example("power alert-test low"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::power::{DeviceKind, PowerDevice};
    use std::cell::RefCell;

    /// The lock starts before the hooks, the hooks get only what's left
    /// of logind's delay, and the lock gets the rest.
    #[test]
    fn before_sleep_locks_first_and_shares_the_budget() {
        let log = RefCell::new(Vec::<String>::new());
        let budget = Duration::from_secs(4);
        let confirmed = before_sleep(
            budget,
            true,
            || {
                log.borrow_mut().push("lock".into());
                Ok(())
            },
            |left| {
                assert!(left <= budget);
                log.borrow_mut().push("hooks".into());
            },
            |left| {
                assert!(left <= budget);
                log.borrow_mut().push("wait".into());
                true
            },
        );
        assert_eq!(confirmed, Some(true));
        assert_eq!(*log.borrow(), ["lock", "hooks", "wait"]);
        // Locking off: hooks only; a failed lock still runs the hooks.
        log.borrow_mut().clear();
        let r = before_sleep(budget, false, || panic!("no lock"), |_| log.borrow_mut().push("hooks".into()), |_| panic!("no wait"));
        assert_eq!((r, log.borrow().clone()), (None, vec!["hooks".to_string()]));
        log.borrow_mut().clear();
        let r = before_sleep(budget, true, || Err("no locker".into()), |_| log.borrow_mut().push("hooks".into()), |_| panic!("nothing to wait for"));
        assert_eq!((r, log.borrow().clone()), (None, vec!["hooks".to_string()]));
    }

    #[test]
    fn critical_action_is_read_when_due() {
        let mut cfg = PowerConfig { critical_action: CriticalAction::Hibernate, ..Default::default() };
        assert_eq!(action_when_due(&cfg), Some(CriticalAction::Hibernate));
        cfg.critical_action = CriticalAction::Nothing;
        assert_eq!(action_when_due(&cfg), None, "switching to Nothing mid-countdown cancels");
    }

    #[test]
    fn editing_the_source_profile_applies_it_now() {
        let before = PowerConfig { profile_ac: Some("balanced".into()), profile_battery: Some("power-saver".into()), ..Default::default() };
        let mut after = before.clone();
        after.profile_ac = Some("performance".into());
        assert_eq!(source_profile_after_edit(&before, &after, false), Some("performance".into()), "on mains: the new mains profile");
        assert_eq!(source_profile_after_edit(&before, &after, true), None, "on battery the battery profile didn't change");
        after.profile_ac = None;
        assert_eq!(source_profile_after_edit(&before, &after, false), None, "\"no change\" switches nothing");
        assert_eq!(source_profile_after_edit(&before, &before, false), None);
    }

    fn battery(percent: f64, state: ChargeState) -> PowerState {
        let b = PowerDevice { path: "/bat0".into(), kind: DeviceKind::Battery, power_supply: true, present: true, percentage: percent, state, energy_full: 50.0, ..Default::default() };
        PowerState { ready: true, on_battery: state == ChargeState::Discharging, devices: vec![b], ..Default::default() }
    }

    fn step(cfg: &PowerConfig, mem: &mut Memory, prev: &mut Option<Summary>, state: &PowerState) -> Vec<Decision> {
        let now = Summary::of(state);
        let out = evaluate(cfg, mem, *prev, now, state);
        *prev = Some(now);
        out
    }

    #[test]
    fn alerts_fire_once_per_discharge_and_rearm() {
        let cfg = PowerConfig { critical_action: CriticalAction::Suspend, ..Default::default() };
        let (mut mem, mut prev) = (Memory::default(), None);
        assert!(step(&cfg, &mut mem, &mut prev, &battery(60.0, ChargeState::Discharging)).is_empty());
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(20.0, ChargeState::Discharging)), [Decision::Low(20)]);
        assert!(step(&cfg, &mut mem, &mut prev, &battery(19.0, ChargeState::Discharging)).is_empty(), "no repeat");
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(8.0, ChargeState::Discharging)), [Decision::Critical(8)]);
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(4.0, ChargeState::Discharging)), [Decision::ArmAction(CriticalAction::Suspend)]);
        // Plugging in cancels the action and re-arms the warnings.
        let plugged = step(&cfg, &mut mem, &mut prev, &battery(4.0, ChargeState::Charging));
        assert!(plugged.contains(&Decision::CancelAction), "{plugged:?}");
        assert!(!mem.low && !mem.critical);
        // Skipping straight to critical reports only the critical alert.
        let (mut mem, mut prev) = (Memory::default(), None);
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(6.0, ChargeState::Discharging)), [Decision::Critical(6)]);
    }

    #[test]
    fn profiles_follow_the_power_source_and_low_battery() {
        let cfg = PowerConfig { profile_ac: Some("performance".into()), profile_battery: Some("balanced".into()), saver_at_low: true, critical_action: CriticalAction::Nothing, ..Default::default() };
        let (mut mem, mut prev) = (Memory::default(), None);
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(80.0, ChargeState::Charging)), [Decision::Profile("performance".into())]);
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(80.0, ChargeState::Discharging)), [Decision::Profile("balanced".into())]);
        let low = step(&cfg, &mut mem, &mut prev, &battery(15.0, ChargeState::Discharging));
        assert!(low.contains(&Decision::Low(15)) && low.contains(&Decision::Profile("power-saver".into())), "{low:?}");
        let back = step(&cfg, &mut mem, &mut prev, &battery(15.0, ChargeState::Charging));
        assert_eq!(back, [Decision::Profile("performance".into())], "the AC profile wins over restoring");
        // Without per-source profiles, charging restores the earlier one.
        let cfg = PowerConfig { saver_at_low: true, critical_action: CriticalAction::Nothing, ..Default::default() };
        let (mut mem, mut prev) = (Memory::default(), None);
        step(&cfg, &mut mem, &mut prev, &battery(15.0, ChargeState::Discharging));
        assert_eq!(step(&cfg, &mut mem, &mut prev, &battery(15.0, ChargeState::Charging)), [Decision::RestoreProfile]);
    }

    #[test]
    fn peripherals_and_full_notices() {
        let mut cfg = PowerConfig { notify_full: true, ..Default::default() };
        cfg.device_thresholds.insert("MX Master".into(), 30);
        let mut state = battery(100.0, ChargeState::FullyCharged);
        state.devices.push(PowerDevice { path: "/mouse".into(), kind: DeviceKind::Mouse, model: "MX Master".into(), percentage: 25.0, state: ChargeState::Discharging, ..Default::default() });
        let (mut mem, mut prev) = (Memory::default(), None);
        let first = step(&cfg, &mut mem, &mut prev, &state);
        assert!(first.contains(&Decision::Full(100)), "{first:?}");
        assert!(first.contains(&Decision::Peripheral { name: "MX Master".into(), percent: 25 }), "{first:?}");
        assert!(step(&cfg, &mut mem, &mut prev, &state).is_empty(), "each notice once");
        // No battery at all (desktop): nothing but peripherals.
        let desktop = PowerState { ready: true, ..Default::default() };
        let (mut mem, mut prev) = (Memory::default(), None);
        assert!(step(&PowerConfig::default(), &mut mem, &mut prev, &desktop).is_empty());
    }
}
