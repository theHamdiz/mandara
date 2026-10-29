//! Launched apps as their own systemd user scopes: after spawning, the
//! process is moved into `app-mandara-<app>-<pid>.scope` with
//! `org.freedesktop.systemd1.Manager.StartTransientUnit` (what GNOME and
//! KDE do), so its resources are tracked per app and a shell restart never
//! takes it down.

use zbus::zvariant::Value;

const DEST: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
/// Longest app part of a unit name (unit names are capped at 255 bytes).
const MAX_APP_PART: usize = 64;

/// `app-mandara-<escaped app>-<pid>.scope`: only `[A-Za-z0-9:_.-]` survive,
/// everything else becomes `_` (systemd's unit-name alphabet; `\x` escapes
/// are legal but unreadable in `systemctl` listings).
pub fn scope_name(app: &str, pid: u32) -> String {
    let mut part: String = app.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.') { c } else { '_' }).collect();
    part.truncate(MAX_APP_PART);
    if part.is_empty() {
        part.push_str("app");
    }
    format!("app-mandara-{part}-{pid}.scope")
}

async fn start(app: &str, pid: u32, description: &str) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let m = crate::actor::proxy(&conn, DEST, PATH, MANAGER).await?;
    let props: Vec<(&str, Value<'_>)> = vec![("PIDs", Value::from(vec![pid])), ("Description", Value::from(description)), ("CollectMode", Value::from("inactive-or-failed"))];
    let aux: Vec<(&str, Vec<(&str, Value<'_>)>)> = Vec::new();
    let _job: zbus::zvariant::OwnedObjectPath = m.call("StartTransientUnit", &(scope_name(app, pid), "fail", props, aux)).await?;
    Ok(())
}

/// Moves `pid` into a fresh scope named after `app`.
pub fn move_into_scope(app: &str, pid: u32, description: &str) -> Result<String, String> {
    crate::runtime::block_on(start(app, pid, description)).map(|_| scope_name(app, pid)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_names_are_valid_units() {
        assert_eq!(scope_name("org.gnome.Nautilus", 42), "app-mandara-org.gnome.Nautilus-42.scope");
        assert_eq!(scope_name("my app/ü", 7), "app-mandara-my_app__-7.scope");
        assert_eq!(scope_name("", 1), "app-mandara-app-1.scope");
        assert!(scope_name(&"x".repeat(500), 1).len() < 100);
    }
}
