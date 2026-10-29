//! The desktop's light/dark preference via the XDG Settings portal
//! (`org.freedesktop.appearance` `color-scheme`: 0 none, 1 dark, 2 light),
//! read once and followed live through `SettingChanged`.

use futures::StreamExt;
use zbus::zvariant::OwnedValue;

/// `Some(true)` = prefers dark, `Some(false)` = prefers light, `None` = no
/// preference / portal unavailable.
pub type Preference = Option<bool>;

/// `color-scheme` values.
const SCHEME_DARK: u32 = 1;
const SCHEME_LIGHT: u32 = 2;
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS_IFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
/// Preference changes buffered for the UI (only the latest matters).
const UPDATE_QUEUE: usize = 4;

fn decode(v: &OwnedValue) -> Preference {
    // ReadOne returns the u32 directly; the older Read wraps it in a variant.
    let n = u32::try_from(v).ok().or_else(|| {
        let inner: zbus::zvariant::Value = v.downcast_ref::<zbus::zvariant::Value>().ok()?;
        u32::try_from(&inner).ok()
    })?;
    match n {
        SCHEME_DARK => Some(true),
        SCHEME_LIGHT => Some(false),
        _ => None,
    }
}

/// Spawns a watcher; the receiver gets the current preference first, then
/// every change.
pub fn watch() -> async_channel::Receiver<Preference> {
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    crate::runtime::spawn("appearance", async move {
        let result: zbus::Result<()> = async move {
            let conn = crate::runtime::session().await?;
            let settings = crate::actor::proxy(&conn, PORTAL, PORTAL_PATH, SETTINGS_IFACE).await?;
            let mut changes = settings.receive_signal("SettingChanged").await?;
            let initial: zbus::Result<OwnedValue> = settings.call("ReadOne", &(NAMESPACE, KEY)).await;
            let initial = match initial {
                Ok(v) => decode(&v),
                Err(_) => settings.call::<_, _, OwnedValue>("Read", &(NAMESPACE, KEY)).await.ok().and_then(|v| decode(&v)),
            };
            if tx.send(initial).await.is_err() {
                return Ok(());
            }
            while let Some(msg) = changes.next().await {
                // Borrowed strings: no allocation for other namespaces' changes.
                let body = msg.body();
                let Ok((ns, key, value)) = body.deserialize::<(&str, &str, OwnedValue)>() else { continue };
                if ns == NAMESPACE && key == KEY && tx.send(decode(&value)).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!("appearance portal unavailable: {err}");
        }
    });
    rx
}

/// Light or dark for a daily schedule: light from `light_at` until
/// `dark_at` (minutes since midnight; handles spans across midnight).
pub fn scheduled_dark(now_min: u32, light_at: u32, dark_at: u32) -> bool {
    if light_at == dark_at {
        return false;
    }
    let light = if light_at < dark_at { now_min >= light_at && now_min < dark_at } else { now_min >= light_at || now_min < dark_at };
    !light
}

/// Parses `HH:MM` into minutes since midnight.
pub fn parse_hm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_spans() {
        let (l, d) = (parse_hm("07:00").unwrap(), parse_hm("19:30").unwrap());
        assert!(!scheduled_dark(parse_hm("12:00").unwrap(), l, d));
        assert!(scheduled_dark(parse_hm("20:00").unwrap(), l, d));
        assert!(scheduled_dark(parse_hm("06:59").unwrap(), l, d));
        // Light span across midnight (night-shift worker).
        assert!(!scheduled_dark(parse_hm("23:00").unwrap(), parse_hm("22:00").unwrap(), parse_hm("06:00").unwrap()));
        assert!(parse_hm("24:00").is_none());
        assert!(parse_hm("7:5").is_some());
    }
}
