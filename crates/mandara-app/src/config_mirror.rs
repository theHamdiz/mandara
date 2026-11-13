//! Studio forwards preference edits; the shell keeps unrelated live settings.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::{Arc, Mutex};

use anyhow::Result;
use mandara_config::{ShellConfig, patch::ConfigPatch};

/// One coalesced request waits while the previous delivery is in flight.
const PENDING_CONFIGS: usize = 1;

type Pending = Arc<Mutex<Option<Request>>>;

struct Request {
    before: Arc<ShellConfig>,
    after: Arc<ShellConfig>,
    reply: async_channel::Sender<bool>,
}

pub struct Mirror {
    requests: async_channel::Sender<()>,
    pending: Pending,
    alive: Arc<AtomicBool>,
}

impl Mirror {
    pub fn start(socket: PathBuf, alive: Arc<AtomicBool>) -> Result<Self> {
        let (requests, receiver) = async_channel::bounded(PENDING_CONFIGS);
        let pending = Pending::default();
        let (worker_alive, worker_pending) = (alive.clone(), pending.clone());
        std::thread::Builder::new().name("mandara-studio-mirror".into()).spawn(move || {
            forward(receiver, &worker_pending, &worker_alive, |patch| {
                    let Ok(json) = serde_json::to_string(&patch) else {
                        return false;
                    };
                    crate::ipc::send(&socket, &["config".into(), "patch".into(), json]).is_ok_and(|reply| reply.ok)
            });
        })?;
        Ok(Self { requests,
            pending,
            alive })
    }

    /// Known offline: save locally while still trying to reconnect. Otherwise
    /// wait for the shell's reply before trusting it to save these edits.
    pub fn queue(&self,
        mut before: Arc<ShellConfig>,
        after: Arc<ShellConfig>,
    ) -> Option<async_channel::Receiver<bool>> {
        let wait = self.alive.load(Ordering::Relaxed);
        let (reply, received) = async_channel::bounded(1);
        let mut slot = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = slot.take() {
            before = old.before;
        }
        *slot = Some(Request {
            before,
            after,
            reply,
        });
        if self.requests.is_closed() {
            slot.take();
            return None;
        }
        // A full wake-up channel already points at this pending request.
        let _ = self.requests.try_send(());
        wait.then_some(received)
    }
}

fn forward(receiver: async_channel::Receiver<()>,
    pending: &Pending, alive: &AtomicBool, mut deliver: impl FnMut(ConfigPatch) -> bool) {
    while receiver.recv_blocking().is_ok() {
        let Some(request) = pending.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            continue;
        };
        let ok = ConfigPatch::between(&request.before, &request.after).is_ok_and(&mut deliver);
        alive.store(ok, Ordering::Relaxed);
        let _ = request.reply.try_send(ok);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(
        mirror: &Mirror,
        before: &ShellConfig,
        after: &ShellConfig,
    ) -> Option<async_channel::Receiver<bool>> {
        mirror.queue(Arc::new(before.clone()), Arc::new(after.clone()))
    }

    fn fixture(alive: bool) -> (Mirror, async_channel::Receiver<()>) {
        let (requests, receiver) = async_channel::bounded(PENDING_CONFIGS);
        (Mirror { requests,
                pending: Pending::default(), alive: Arc::new(AtomicBool::new(alive)) }, receiver)
    }

    #[test]
    fn failed_delivery_is_reported_even_after_a_successful_ping() {
        let (mirror, requests) = fixture(true);
        let reply = queue(&mirror, &ShellConfig::default(), &ShellConfig::default()).unwrap();
        assert!(reply.try_recv().is_err(), "queueing is not an acknowledgement");
        mirror.requests.close();
        forward(requests, &mirror.pending, &mirror.alive, |_| false);
        assert_eq!(reply.recv_blocking(), Ok(false), "Studio must persist this edit itself");
        assert!(!mirror.alive.load(Ordering::Relaxed));
    }

    #[test]
    fn pending_edits_coalesce_without_losing_a_different_setting() {
        let (mirror, requests) = fixture(true);
        let a = ShellConfig::default();
        let mut b = a.clone();
        b.bar.height = 37.0;
        let old = queue(&mirror, &a, &b).unwrap();
        let mut c = b.clone();
        c.theme.mode = mandara_config::ThemeMode::Light;
        let latest = queue(&mirror, &b, &c).unwrap();
        assert!(old.recv_blocking().is_err(),
            "only the latest edit confirms delivery"
        );
        assert_eq!(requests.len(), PENDING_CONFIGS);
        mirror.requests.close();
        let mut sent = Vec::new();
        forward(requests, &mirror.pending, &mirror.alive, |patch| {
            sent.push(patch.apply(&a).unwrap());
            true
        });
        assert_eq!(sent, [c]);
        assert_eq!(latest.recv_blocking(), Ok(true));
    }

    #[test]
    fn an_edit_queued_during_delivery_is_not_lost_or_reordered() {
        let (mirror, requests) = fixture(true);
        let a = ShellConfig::default();
        let mut b = a.clone();
        b.bar.height = 37.0;
        let first = queue(&mirror, &a, &b).unwrap();
        let mut c = b.clone();
        c.bar.height = 43.0;
        let mut delivered = a.clone();
        let mut latest = None;
        forward(requests, &mirror.pending, &mirror.alive, |patch| {
            delivered = patch.apply(&delivered).unwrap();
            if latest.is_none() {
                latest = queue(&mirror, &b, &c);
                mirror.requests.close();
            }
            true
        });
        assert_eq!(delivered, c);
        assert_eq!(first.recv_blocking(), Ok(true));
        assert_eq!(latest.unwrap().recv_blocking(), Ok(true));
    }

    #[test]
    fn offline_studio_saves_immediately_and_can_reconnect() {
        let (mirror, requests) = fixture(false);
        assert!(queue(&mirror, &ShellConfig::default(), &ShellConfig::default()).is_none());
        mirror.requests.close();
        forward(requests, &mirror.pending, &mirror.alive, |_| true);
        assert!(mirror.alive.load(Ordering::Relaxed));
    }

    #[test]
    fn stopped_worker_falls_back_to_local_persistence() {
        let (mirror, requests) = fixture(true);
        drop(requests);
        assert!(queue(&mirror, &ShellConfig::default(), &ShellConfig::default()).is_none());
        assert!(mirror.pending.lock().unwrap().is_none());
    }
}
