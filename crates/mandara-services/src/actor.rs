//! Shared plumbing for service actors: a state stream plus a command queue
//! whose commands can carry a reply channel (so CLI callers get results).

use std::time::Duration;

use zbus::Connection;
use zbus::proxy::{Builder, CacheProperties};

/// Result sent back to a command's caller.
pub type Reply = async_channel::Sender<Result<String, String>>;

/// A command plus an optional reply channel.
pub struct Envelope<C> {
    pub command: C,
    pub reply: Option<Reply>,
}

/// Handle to a running actor.
pub struct ServiceHandle<S, C> {
    pub updates: async_channel::Receiver<S>,
    commands: async_channel::Sender<Envelope<C>>,
}

impl<S, C> Clone for ServiceHandle<S, C> {
    fn clone(&self) -> Self {
        Self { updates: self.updates.clone(), commands: self.commands.clone() }
    }
}

impl<S, C> ServiceHandle<S, C> {
    pub fn new(updates: async_channel::Receiver<S>, commands: async_channel::Sender<Envelope<C>>) -> Self {
        Self { updates, commands }
    }

    /// Fire-and-forget.
    pub fn send(&self, command: C) {
        let _ = self.try_send(command);
    }

    /// Enqueues a command without waiting, returning it when the queue is full or closed.
    pub fn try_send(&self, command: C) -> Result<(), async_channel::TrySendError<Envelope<C>>> {
        self.commands.try_send(Envelope { command, reply: None })
    }

    /// Sends a command and returns a receiver for its result.
    pub fn request(&self, command: C) -> async_channel::Receiver<Result<String, String>> {
        let (tx, rx) = async_channel::bounded(1);
        if self.commands.try_send(Envelope { command, reply: Some(tx.clone()) }).is_err() {
            let _ = tx.try_send(Err(mandara_i18n::t("The service is not running").into()));
        }
        rx
    }
}

/// Replies (if anyone is listening).
pub fn reply(reply: &Option<Reply>, result: Result<String, String>) {
    if let Some(r) = reply {
        let _ = r.try_send(result);
    }
}

/// An uncached dynamic proxy (we poll / re-read deliberately).
pub async fn proxy<'a>(conn: &Connection, dest: &'a str, path: &'a str, iface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    Builder::<zbus::Proxy<'a>>::new(conn)
        .destination(dest)?
        .path(path)?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// Signals buffered per subscription before the stream applies
/// back-pressure; actors drain their stream before every re-read.
pub const SIGNAL_QUEUE: usize = 512;

/// A stream of every signal `sender` (a well-known name) emits.
pub async fn signals_from(conn: &Connection, sender: &str) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(sender)?.build();
    zbus::MessageStream::for_match_rule(rule, conn, Some(SIGNAL_QUEUE)).await
}

/// Drops every signal already queued (one re-read covers them all).
pub fn drain_signals<S: futures::Stream + Unpin>(stream: &mut S) {
    use futures::{FutureExt as _, StreamExt as _};
    while let Some(Some(_)) = stream.next().now_or_never() {}
}

/// Why an event-driven actor woke up.
pub enum Wake<C> {
    Command(Envelope<C>),
    /// Signals arrived (already debounced) or the fallback poll expired.
    Refresh,
    /// Every handle was dropped: the actor should exit.
    Closed,
}

/// Waits for a command, a burst of signals (then `debounce` more so the
/// whole burst is one re-read), or the `fallback` poll. A finished signal
/// stream degrades to the fallback poll instead of spinning.
pub async fn next_wake<C, S: futures::Stream + Unpin>(signals: &mut S, commands: &async_channel::Receiver<Envelope<C>>, debounce: Duration, fallback: Duration) -> Wake<C> {
    use futures::{FutureExt as _, StreamExt as _};
    futures::select! {
        cmd = commands.recv().fuse() => match cmd {
            Ok(c) => Wake::Command(c),
            Err(_) => Wake::Closed,
        },
        sig = signals.next().fuse() => {
            async_io::Timer::after(if sig.is_some() { debounce } else { fallback }).await;
            Wake::Refresh
        },
        _ = futures::FutureExt::fuse(async_io::Timer::after(fallback)) => Wake::Refresh,
    }
}

/// How long [`send_event`] waits for a full event queue to drain.
const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Queues an event for the UI without silently losing it: a full queue is
/// waited on for a few seconds, then the loss is logged. A closed queue
/// (the UI side is gone) is not an error.
pub async fn send_event<T>(tx: &async_channel::Sender<T>, event: T, what: &str) {
    match tx.try_send(event) {
        Ok(()) | Err(async_channel::TrySendError::Closed(_)) => {}
        Err(async_channel::TrySendError::Full(event)) => {
            if crate::runtime::timeout(EVENT_SEND_TIMEOUT, tx.send(event)).await.is_none() {
                tracing::warn!("{what}: the UI did not take the event in time; dropped");
            }
        }
    }
}

/// Whether the caller of a D-Bus method is `service`'s current owner, or
/// root. For agent interfaces the system bus policy already limits who can
/// call them; this keeps another local process from driving the prompts.
pub async fn caller_is(conn: &Connection, header: &zbus::message::Header<'_>, service: &str) -> bool {
    let Some(sender) = header.sender() else { return false };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else { return false };
    if let Ok(name) = zbus::names::BusName::try_from(service)
        && dbus.get_name_owner(name).await.is_ok_and(|owner| owner.as_str() == sender.as_str())
    {
        return true;
    }
    dbus.get_connection_unix_user(zbus::names::BusName::from(sender.clone())).await.is_ok_and(|uid| uid == 0)
}

/// Follows `name` on the bus: `true` each time it gains an owner, `false`
/// when it loses it. For services that must re-register after the daemon
/// they talk to restarts.
pub async fn owner_changes(conn: &Connection, name: &'static str) -> zbus::Result<std::pin::Pin<Box<dyn futures::Stream<Item = bool> + Send>>> {
    use futures::StreamExt as _;
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let stream = dbus.receive_name_owner_changed_with_args(&[(0, name)]).await?;
    Ok(stream.filter_map(|signal| async move { signal.args().ok().map(|a| a.new_owner().is_some()) }).boxed())
}

/// Waits until `owners` reports the name gained an owner, or `retry` passes.
/// `false` when the stream ended (no more changes will come).
pub async fn wait_for_owner<S: futures::Stream<Item = bool> + Unpin>(owners: &mut S, retry: Option<Duration>) -> bool {
    use futures::{FutureExt as _, StreamExt as _};
    let appeared = async {
        loop {
            match owners.next().await {
                Some(true) => return true,
                Some(false) => {}
                None => return false,
            }
        }
    };
    match retry {
        Some(retry) => futures::select! {
            seen = appeared.fuse() => seen,
            _ = futures::FutureExt::fuse(async_io::Timer::after(retry)) => true,
        },
        None => appeared.await,
    }
}

/// Waits for the next command or a timeout, whichever comes first.
pub async fn next_command<C>(commands: &async_channel::Receiver<Envelope<C>>, timeout: Duration) -> Option<Option<Envelope<C>>> {
    use futures::FutureExt as _;
    futures::select! {
        cmd = commands.recv().fuse() => match cmd {
            Ok(c) => Some(Some(c)),
            Err(_) => None, // all handles dropped
        },
        _ = futures::FutureExt::fuse(async_io::Timer::after(timeout)) => Some(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonblocking_send_reports_full_and_closed_queues_without_losing_the_command() {
        let (_, updates) = async_channel::bounded::<()>(1);
        let (commands, incoming) = async_channel::bounded(1);
        let service = ServiceHandle::new(updates, commands);
        assert!(service.try_send(7).is_ok());
        let Err(async_channel::TrySendError::Full(full)) = service.try_send(8) else { panic!("a full queue must return its command") };
        assert_eq!(full.command, 8);
        assert!(full.reply.is_none());
        assert_eq!(incoming.try_recv().unwrap().command, 7);
        assert!(service.try_send(full.command).is_ok());
        assert_eq!(incoming.try_recv().unwrap().command, 8);
        incoming.close();
        let Err(async_channel::TrySendError::Closed(closed)) = service.try_send(9) else { panic!("a closed queue must return its command") };
        assert_eq!(closed.command, 9);
        assert!(closed.reply.is_none());
    }
}
