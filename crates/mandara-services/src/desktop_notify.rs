//! A notification with one action, posted like any app posts one
//! (`org.freedesktop.Notifications.Notify` on the session bus), so it works
//! with Mandara's own notification daemon and with any other.
//!
//! The returned channel answers `true` when the action (or the notification
//! itself) was clicked, `false` when it was closed another way or couldn't
//! be posted.

use std::collections::HashMap;

use futures::StreamExt as _;

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
/// The action a click on the notification itself invokes.
const DEFAULT_ACTION: &str = "default";
/// `expire_timeout` of -1: the server's own default.
const SERVER_DEFAULT_TIMEOUT: i32 = -1;

/// Posts a notification with one action labelled `action`.
pub fn post_with_action(app: &str, icon: &str, summary: &str, body: &str, action: &str) -> async_channel::Receiver<bool> {
    let (tx, rx) = async_channel::bounded(1);
    let (app, icon, summary, body, action) = (app.to_string(), icon.to_string(), summary.to_string(), body.to_string(), action.to_string());
    crate::runtime::spawn("desktop-notify", async move {
        let clicked = post(&app, &icon, &summary, &body, &action).await.unwrap_or_else(|e| {
            tracing::debug!("notification: {e}");
            false
        });
        let _ = tx.send(clicked).await;
    });
    rx
}

async fn post(app: &str, icon: &str, summary: &str, body: &str, action: &str) -> zbus::Result<bool> {
    let conn = crate::runtime::session().await?;
    let proxy = crate::actor::proxy(&conn, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS).await?;
    // Subscribed before posting, so a quick click is never missed.
    let mut invoked = proxy.receive_signal("ActionInvoked").await?;
    let mut closed = proxy.receive_signal("NotificationClosed").await?;
    let actions = vec![DEFAULT_ACTION, action];
    let hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
    let id: u32 = proxy.call("Notify", &(app, 0u32, icon, summary, body, actions, hints, SERVER_DEFAULT_TIMEOUT)).await?;
    loop {
        use futures::future::Either;
        match futures::future::select(invoked.next(), closed.next()).await {
            Either::Left((Some(msg), _)) => {
                if msg.body().deserialize::<(u32, String)>().is_ok_and(|(nid, _)| nid == id) {
                    return Ok(true);
                }
            }
            Either::Right((Some(msg), _)) => {
                if msg.body().deserialize::<(u32, u32)>().is_ok_and(|(nid, _)| nid == id) {
                    return Ok(false);
                }
            }
            Either::Left((None, _)) | Either::Right((None, _)) => return Ok(false),
        }
    }
}
