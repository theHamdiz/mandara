//! Sleep, lid and restart targets over logind / UPower (pure D-Bus).
//!
//! * **Lock before sleep** — [`watch_sleep`] holds a logind *delay*
//!   inhibitor (`Inhibit("sleep", …, "delay")`). When `PrepareForSleep(true)`
//!   arrives the `before` callback runs (lock the screen, before-sleep
//!   hooks) and only then is the inhibitor released, so the machine never
//!   suspends showing an unlocked desktop. After resume a fresh inhibitor
//!   is taken and `after` runs.
//! * **Lid** — [`watch_lid`] follows UPower's `LidIsClosed`.
//! * **Sleep modes** — suspend, hibernate, suspend-then-hibernate, hybrid.
//! * **Restart targets** — firmware setup (`SetRebootToFirmwareSetup`),
//!   boot-loader entries (`BootLoaderEntries`, `SetRebootToBootLoaderEntry`),
//!   and soft reboot (`systemctl soft-reboot`).
//!
//! `MANDARA_POWER_DRY_RUN=1` turns every action into a log line (tests and
//! demos must never suspend or restart the machine).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;

use futures::StreamExt as _;
use mandara_config::power::SuspendMode;
use mandara_i18n::{t, tf};
use serde::Serialize;
use zbus::zvariant::OwnedValue;

use crate::actor::proxy;

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// Environment switch for dry runs.
const DRY_RUN_ENV: &str = "MANDARA_POWER_DRY_RUN";
/// How often [`watch_sleep`] re-checks whether the inhibitor is wanted.
const ENABLED_POLL: std::time::Duration = std::time::Duration::from_secs(5);
/// Who holds the inhibitor, as `systemd-inhibit --list` shows it.
const INHIBITOR_WHO: &str = "Mandara Shell";

/// Whether power actions only log (`MANDARA_POWER_DRY_RUN=1`).
pub fn dry_run() -> bool {
    static DRY: OnceLock<bool> = OnceLock::new();
    *DRY.get_or_init(|| std::env::var(DRY_RUN_ENV).is_ok_and(|v| !v.is_empty() && v != "0"))
}

fn bus_error(e: zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, msg, _) => crate::power::friendly_error(name.as_str(), msg.as_deref().unwrap_or_default()),
        other => other.to_string(),
    }
}

async fn manager(conn: &zbus::Connection) -> zbus::Result<zbus::Proxy<'static>> {
    proxy(conn, LOGIND, LOGIND_PATH, MANAGER_IFACE).await
}

/// Calls a logind manager method taking only `interactive: bool`.
async fn call_interactive(method: &'static str) -> Result<(), String> {
    if dry_run() {
        tracing::info!("dry run: login1.Manager.{method}");
        return Ok(());
    }
    let conn = crate::runtime::system().await.map_err(bus_error)?;
    manager(&conn).await.map_err(bus_error)?.call::<_, _, ()>(method, &(true,)).await.map_err(bus_error)
}

/// Sleeps the computer the way `mode` says (polkit may prompt).
pub fn sleep(mode: SuspendMode) -> Result<(), String> {
    crate::runtime::block_on(call_interactive(mode.logind_method())).map_err(|e| tf("Could not sleep ({mode}): {error}", &[("mode", &mode.id()), ("error", &e)]))
}

/// Whether logind allows `mode` (`CanSuspend`, …): `yes`, `no`, `challenge`, `na`.
pub fn can_sleep(mode: SuspendMode) -> String {
    let method = match mode {
        SuspendMode::Suspend => "CanSuspend",
        SuspendMode::Hibernate => "CanHibernate",
        SuspendMode::SuspendThenHibernate => "CanSuspendThenHibernate",
        SuspendMode::HybridSleep => "CanHybridSleep",
    };
    crate::runtime::block_on(async move {
        let conn = crate::runtime::system().await.ok()?;
        manager(&conn).await.ok()?.call::<_, _, String>(method, &()).await.ok()
    })
    .unwrap_or_else(|| "na".into())
}

/// Restart targets the machine offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RebootTargets {
    /// `CanRebootToFirmwareSetup`: `yes`, `no`, `challenge` or `na`.
    pub firmware: String,
    /// `CanRebootToBootLoaderEntry`.
    pub boot_entries_supported: String,
    /// Boot-loader entry ids (systemd-boot `LoaderEntries`).
    pub boot_entries: Vec<String>,
}

/// Reads the firmware / boot-entry capabilities (blocking).
pub fn reboot_targets() -> RebootTargets {
    crate::runtime::block_on(async {
        let Ok(conn) = crate::runtime::system().await else { return RebootTargets::default() };
        let Ok(m) = manager(&conn).await else { return RebootTargets::default() };
        RebootTargets {
            firmware: m.call::<_, _, String>("CanRebootToFirmwareSetup", &()).await.unwrap_or_else(|_| "na".into()),
            boot_entries_supported: m.call::<_, _, String>("CanRebootToBootLoaderEntry", &()).await.unwrap_or_else(|_| "na".into()),
            boot_entries: m.get_property::<Vec<String>>("BootLoaderEntries").await.unwrap_or_default(),
        }
    })
}

/// Restarts into the firmware (UEFI) setup.
pub fn reboot_to_firmware() -> Result<(), String> {
    crate::runtime::block_on(async {
        if dry_run() {
            tracing::info!("dry run: SetRebootToFirmwareSetup(true) + Reboot");
            return Ok(());
        }
        let conn = crate::runtime::system().await.map_err(bus_error)?;
        let m = manager(&conn).await.map_err(bus_error)?;
        m.call::<_, _, ()>("SetRebootToFirmwareSetup", &(true,)).await.map_err(bus_error)?;
        m.call::<_, _, ()>("Reboot", &(true,)).await.map_err(bus_error)
    })
    .map_err(|e| tf("Could not restart into the firmware setup: {error}", &[("error", &e)]))
}

/// Restarts into boot-loader entry `id` (once).
pub fn reboot_to_entry(id: &str) -> Result<(), String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(t("No boot entry chosen").into());
    }
    crate::runtime::block_on(async move {
        if dry_run() {
            tracing::info!("dry run: SetRebootToBootLoaderEntry({id}) + Reboot");
            return Ok(());
        }
        let conn = crate::runtime::system().await.map_err(bus_error)?;
        let m = manager(&conn).await.map_err(bus_error)?;
        let entries: Vec<String> = m.get_property("BootLoaderEntries").await.unwrap_or_default();
        if !entries.is_empty() && !entries.contains(&id) {
            return Err(tf("Unknown boot entry “{id}”", &[("id", &id)]));
        }
        m.call::<_, _, ()>("SetRebootToBootLoaderEntry", &(id.as_str(),)).await.map_err(bus_error)?;
        m.call::<_, _, ()>("Reboot", &(true,)).await.map_err(bus_error)
    })
}

/// Restarts userspace only (`systemctl soft-reboot`, systemd ≥ 254).
pub fn soft_reboot() -> Result<(), String> {
    if dry_run() {
        tracing::info!("dry run: systemctl soft-reboot");
        return Ok(());
    }
    let status = std::process::Command::new("systemctl").arg("soft-reboot").status().map_err(|e| format!("systemctl: {e}"))?;
    if status.success() { Ok(()) } else { Err(tf("systemctl soft-reboot failed ({status})", &[("status", &status)])) }
}

/// Takes a logind delay inhibitor for `what` (`sleep`, `shutdown`).
async fn delay_inhibitor(m: &zbus::Proxy<'_>, what: &str, why: &str) -> Option<std::os::fd::OwnedFd> {
    match m.call::<_, _, zbus::zvariant::OwnedFd>("Inhibit", &(what, INHIBITOR_WHO, why, "delay")).await {
        Ok(fd) => Some(fd.into()),
        Err(e) => {
            tracing::warn!("logind: no {what} delay inhibitor: {e}");
            None
        }
    }
}

/// Runs `before` when the machine is about to sleep (holding the sleep
/// back until it returns, within logind's `InhibitDelayMaxUSec`) and
/// `after` once it woke up. `enabled()` is asked each time: while it is
/// false no inhibitor is held and `before` is skipped.
pub fn watch_sleep(enabled: impl Fn() -> bool + Send + Sync + 'static, before: impl Fn() + Send + Sync + 'static, after: impl Fn() + Send + Sync + 'static) {
    let (enabled, before, after) = (Arc::new(enabled), Arc::new(before), Arc::new(after));
    crate::runtime::spawn("logind-sleep", async move {
        let result: zbus::Result<()> = async {
            let conn = crate::runtime::system().await?;
            let m = manager(&conn).await?;
            let mut signals = m.receive_signal("PrepareForSleep").await?;
            let why = t("Lock the screen before sleeping");
            let mut held = if enabled() { delay_inhibitor(&m, "sleep", why).await } else { None };
            loop {
                use futures::FutureExt as _;
                let next = futures::select! {
                    msg = signals.next().fuse() => msg,
                    _ = futures::FutureExt::fuse(async_io::Timer::after(ENABLED_POLL)) => {
                        // Follow the setting: hold the inhibitor only while wanted.
                        match (enabled(), held.is_some()) {
                            (true, false) => held = delay_inhibitor(&m, "sleep", why).await,
                            (false, true) => held = None,
                            _ => {}
                        }
                        continue;
                    }
                };
                let Some(msg) = next else { break };
                let starting: bool = msg.body().deserialize().unwrap_or(false);
                if starting {
                    if enabled() {
                        let f = before.clone();
                        crate::runtime::unblock(move || f()).await;
                    }
                    // Releasing the fd lets the suspend proceed.
                    held = None;
                } else {
                    if held.is_none() && enabled() {
                        held = delay_inhibitor(&m, "sleep", why).await;
                    }
                    let f = after.clone();
                    crate::runtime::unblock(move || f()).await;
                }
            }
            drop(held);
            Ok(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!("logind: not watching for sleep: {err}");
        }
    });
}

/// Logind's `InhibitDelayMaxUSec`: how long a delay inhibitor may hold a
/// sleep back (5 s unless configured).
pub fn inhibit_delay_max() -> std::time::Duration {
    const FALLBACK_US: u64 = 5_000_000;
    let us = crate::runtime::block_on(async {
        let conn = crate::runtime::system().await.ok()?;
        manager(&conn).await.ok()?.get_property::<u64>("InhibitDelayMaxUSec").await.ok()
    });
    std::time::Duration::from_micros(us.unwrap_or(FALLBACK_US))
}

/// Calls `on_change(closed)` whenever the laptop lid opens or closes
/// (UPower `LidIsClosed`). Does nothing on machines without a lid.
pub fn watch_lid(on_change: impl Fn(bool) + Send + Sync + 'static) {
    let on_change = Arc::new(on_change);
    crate::runtime::spawn("upower-lid", async move {
        let result: zbus::Result<()> = async {
            let conn = crate::runtime::system().await?;
            let root = proxy(&conn, UPOWER, UPOWER_PATH, UPOWER).await?;
            if !root.get_property::<bool>("LidIsPresent").await.unwrap_or(false) {
                return Ok(());
            }
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(UPOWER)?
                .path(UPOWER_PATH)?
                .interface(PROPERTIES_IFACE)?
                .member("PropertiesChanged")?
                .build();
            let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, Some(crate::actor::SIGNAL_QUEUE)).await?;
            let mut last = root.get_property::<bool>("LidIsClosed").await.unwrap_or(false);
            while let Some(Ok(msg)) = stream.next().await {
                let Ok((iface, changed, _)) = msg.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else { continue };
                if iface != UPOWER {
                    continue;
                }
                if let Some(closed) = changed.get("LidIsClosed").and_then(|v| bool::try_from(v).ok())
                    && closed != last
                {
                    last = closed;
                    let f = on_change.clone();
                    crate::runtime::unblock(move || f(closed)).await;
                }
            }
            Ok(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!("upower: not watching the lid: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_modes_map_to_logind() {
        assert_eq!(SuspendMode::SuspendThenHibernate.logind_method(), "SuspendThenHibernate");
        assert_eq!(SuspendMode::HybridSleep.logind_method(), "HybridSleep");
        assert!(reboot_to_entry("  ").is_err());
    }
}
