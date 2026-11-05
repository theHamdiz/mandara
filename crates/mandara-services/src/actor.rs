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
pub fn drain_signals(stream: &mut zbus::MessageStream) {
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
pub async fn next_wake<C>(signals: &mut zbus::MessageStream, commands: &async_channel::Receiver<Envelope<C>>, debounce: Duration, fallback: Duration) -> Wake<C> {
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
