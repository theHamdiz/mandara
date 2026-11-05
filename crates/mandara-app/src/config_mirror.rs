//! Studio sends the newest preferences and waits for the shell to accept them.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use mandara_config::ShellConfig;

/// One pending snapshot suffices while the previous request is in flight.
const PENDING_CONFIGS: usize = 1;

struct Request {
    json: String,
    reply: async_channel::Sender<bool>,
}

pub struct Mirror {
    requests: async_channel::Sender<Request>,
    alive: Arc<AtomicBool>,
}

impl Mirror {
    pub fn start(socket: PathBuf, alive: Arc<AtomicBool>) -> Result<Self> {
        let (requests, receiver) = async_channel::bounded(PENDING_CONFIGS);
        let worker_alive = alive.clone();
        std::thread::Builder::new().name("mandara-studio-mirror".into()).spawn(move || {
            forward(receiver, &worker_alive, |json| {
                crate::ipc::send(&socket, &["config".into(), "apply".into(), json]).is_ok_and(|reply| reply.ok)
            });
        })?;
        Ok(Self { requests, alive })
    }

    /// Known offline: save locally while still trying to reconnect. Otherwise
    /// the caller waits for the actual reply before trusting the shell to save.
    pub fn queue(&self, config: &ShellConfig) -> Option<async_channel::Receiver<bool>> {
        let wait = self.alive.load(Ordering::Relaxed);
        let json = serde_json::to_string(config).ok()?;
        let (reply, received) = async_channel::bounded(1);
        self.requests.force_send(Request { json, reply }).ok()?;
        wait.then_some(received)
    }
}

fn forward(receiver: async_channel::Receiver<Request>, alive: &AtomicBool, mut deliver: impl FnMut(String) -> bool) {
    while let Ok(mut request) = receiver.recv_blocking() {
        while let Ok(newer) = receiver.try_recv() {
            request = newer;
        }
        let ok = deliver(request.json);
        alive.store(ok, Ordering::Relaxed);
        let _ = request.reply.try_send(ok);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(alive: bool) -> (Mirror, async_channel::Receiver<Request>) {
        let (requests, receiver) = async_channel::bounded(PENDING_CONFIGS);
        (Mirror { requests, alive: Arc::new(AtomicBool::new(alive)) }, receiver)
    }

    #[test]
    fn failed_delivery_is_reported_even_after_a_successful_ping() {
        let (mirror, requests) = fixture(true);
        let reply = mirror.queue(&ShellConfig::default()).unwrap();
        assert!(reply.try_recv().is_err(), "queueing is not an acknowledgement");
        mirror.requests.close();
        forward(requests, &mirror.alive, |_| false);
        assert_eq!(reply.recv_blocking(), Ok(false), "Studio must persist this edit itself");
        assert!(!mirror.alive.load(Ordering::Relaxed));
    }

    #[test]
    fn newer_edit_replaces_an_older_pending_snapshot_and_acknowledgement() {
        let (mirror, requests) = fixture(true);
        let mut config = ShellConfig::default();
        let old = mirror.queue(&config).unwrap();
        config.bar.height = 37.0;
        config.theme.mode = mandara_config::ThemeMode::Light;
        let latest = mirror.queue(&config).unwrap();
        assert!(old.recv_blocking().is_err(), "the replaced request cannot confirm delivery");
        assert_eq!(requests.len(), PENDING_CONFIGS);
        mirror.requests.close();
        let mut sent = Vec::new();
        forward(requests, &mirror.alive, |json| {
            sent.push(serde_json::from_str::<ShellConfig>(&json).unwrap());
            true
        });
        assert_eq!(sent, [config]);
        assert_eq!(latest.recv_blocking(), Ok(true));
    }

    #[test]
    fn offline_studio_saves_immediately_and_can_reconnect() {
        let (mirror, requests) = fixture(false);
        assert!(mirror.queue(&ShellConfig::default()).is_none());
        mirror.requests.close();
        forward(requests, &mirror.alive, |_| true);
        assert!(mirror.alive.load(Ordering::Relaxed));
    }

    #[test]
    fn stopped_worker_falls_back_to_local_persistence() {
        let (mirror, requests) = fixture(true);
        drop(requests);
        assert!(mirror.queue(&ShellConfig::default()).is_none());
    }
}
