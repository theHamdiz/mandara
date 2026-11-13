//! Serialize disk saves while retaining only the newest pending snapshot.

use mandara_config::store::{SaveRequest, Saved};

/// The in-flight save owns its snapshot; one later edit may wait behind it.
const PENDING_SAVES: usize = 1;

struct Request {
    save: SaveRequest,
    reply: async_channel::Sender<Result<Saved, String>>,
}

pub struct Writer {
    sender: async_channel::Sender<Request>,
}

impl Default for Writer {
    fn default() -> Self {
        let (sender, receiver) = async_channel::bounded::<Request>(PENDING_SAVES);
        crate::runtime::spawn("config-writer", async move {
            while let Ok(request) = receiver.recv().await {
                let result = crate::runtime::unblock(move || {
                    request.save.write().map_err(|error| format!("{error:#}"))
                })
                .await;
                let _ = request.reply.try_send(result);
            }
        });
        Self { sender }
    }
}

impl Writer {
    pub fn submit(&self, save: SaveRequest) -> async_channel::Receiver<Result<Saved, String>> {
        let (reply, receiver) = async_channel::bounded(1);
        let _ = self.sender.force_send(Request { save, reply });
        receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::ConfigStore;

    #[test]
    fn the_latest_save_finishes_after_earlier_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        let writer = Writer::default();
        let initial = store.get().bar.height;
        for offset in 1..16 {
            store.update(|config| config.bar.height = initial + offset as f32);
            writer.submit(store.save_request().unwrap());
        }
        store.update(|config| config.bar.height = initial + 16.0);
        let done = writer
            .submit(store.save_request().unwrap())
            .recv_blocking()
            .unwrap()
            .unwrap();
        store.finish_save(done).unwrap();
        assert!(!store.is_dirty());
        assert_eq!(
            ConfigStore::open(path).unwrap().get().bar.height,
            initial + 16.0
        );
    }
}
