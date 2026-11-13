//! Complete display writes before reporting persistence to callers.

use mandara_config::{compositor::Target, outputs::OutputConfig};
use parking_lot::Mutex;
use std::sync::Arc;

type Reply = async_channel::Sender<Result<(), String>>;
/// Background saves coalesce; explicit callers have a bounded reply budget.
const MAX_WAITERS: usize = 8;
struct Request {
    outputs: Arc<[OutputConfig]>,
    replies: Vec<Reply>,
}
#[derive(Default)]
struct State {
    committed: Option<Arc<[OutputConfig]>>,
    active: Option<Request>,
    pending: Option<Request>,
}
pub struct Writer {
    state: Arc<Mutex<State>>,
    wake: async_channel::Sender<()>,
}

fn reply(request: Request, result: Result<(), String>) {
    for reply in request.replies {
        let _ = reply.try_send(result.clone());
    }
}

impl Writer {
    pub fn new(target: Target) -> Self {
        Self::start(move |outputs| {
            super::persist_outputs(target, outputs).map_err(|error| format!("{error:#}"))
        })
    }

    fn start(
        write: impl Fn(&[OutputConfig]) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let (wake, awakened) = async_channel::bounded(1);
        let shared = state.clone();
        let write = Arc::new(write);
        crate::runtime::spawn("display-preferences", async move {
            while awakened.recv().await.is_ok() {
                loop {
                    let outputs = {
                        let mut state = shared.lock();
                        let Some(request) = state.pending.take() else {
                            break;
                        };
                        if state.committed.as_deref() == Some(request.outputs.as_ref()) {
                            reply(request, Ok(()));
                            continue;
                        }
                        let outputs = request.outputs.clone();
                        state.active = Some(request);
                        outputs
                    };
                    let job = write.clone();
                    let snapshot = outputs.clone();
                    let result = crate::runtime::unblock(move || job(&snapshot)).await;
                    let mut state = shared.lock();
                    state.committed = result.is_ok().then_some(outputs);
                    if let Err(error) = &result {
                        tracing::warn!("saving display preferences: {error}");
                    }
                    if let Some(request) = state.active.take() {
                        reply(request, result);
                    }
                }
            }
        });
        Self { state, wake }
    }

    pub fn submit(
        &self,
        outputs: &[OutputConfig],
        wait: bool,
    ) -> Option<async_channel::Receiver<Result<(), String>>> {
        let (sender, receiver) = if wait {
            let (sender, receiver) = async_channel::bounded(1);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let mut state = self.state.lock();
        let matching = if state
            .pending
            .as_ref()
            .is_some_and(|request| request.outputs.as_ref() == outputs)
        {
            state.pending.as_mut()
        } else if state.pending.is_none()
            && state
                .active
                .as_ref()
                .is_some_and(|request| request.outputs.as_ref() == outputs)
        {
            state.active.as_mut()
        } else {
            None
        };
        if let Some(request) = matching {
            if let Some(sender) = sender {
                if request.replies.len() < MAX_WAITERS {
                    request.replies.push(sender);
                } else {
                    let _ = sender.try_send(Err("display persistence is busy; try again".into()));
                }
            }
            return receiver;
        }
        if state.active.is_none()
            && state.pending.is_none()
            && state.committed.as_deref() == Some(outputs)
        {
            if let Some(sender) = sender {
                let _ = sender.try_send(Ok(()));
            }
            return receiver;
        }
        if let Some(previous) = state.pending.take() {
            reply(
                previous,
                Err("display preferences were superseded by a newer arrangement".into()),
            );
        }
        state.pending = Some(Request {
            outputs: outputs.into(),
            replies: sender.into_iter().collect(),
        });
        let _ = self.wake.try_send(());
        receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn writes_finish_before_replying_and_failed_snapshots_can_be_retried() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted = attempts.clone();
        let writer = Writer::start(move |_| {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("read-only filesystem".into())
            } else {
                Ok(())
            }
        });
        let outputs = [OutputConfig {
            name: "DP-1".into(),
            scale: Some(1.25),
            ..Default::default()
        }];
        assert!(
            writer
                .submit(&outputs, true)
                .unwrap()
                .recv_blocking()
                .unwrap()
                .is_err()
        );
        assert_eq!(
            writer
                .submit(&outputs, true)
                .unwrap()
                .recv_blocking()
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            writer
                .submit(&outputs, true)
                .unwrap()
                .recv_blocking()
                .unwrap(),
            Ok(())
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn automatic_duplicates_share_completion_and_newer_pending_arrangements_win() {
        let (entered, started) = std::sync::mpsc::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let waiting = Mutex::new(waiting);
        let writer = Writer::start(move |outputs| {
            if outputs[0].scale == Some(1.0) {
                entered.send(()).unwrap();
                waiting.lock().recv().unwrap();
            }
            Ok(())
        });
        let mut outputs = [OutputConfig {
            name: "DP-1".into(),
            scale: Some(1.0),
            ..Default::default()
        }];
        let active = writer.submit(&outputs, true).unwrap();
        started.recv().unwrap();
        assert!(writer.submit(&outputs, false).is_none());
        let duplicate = writer.submit(&outputs, true).unwrap();
        assert!(duplicate.try_recv().is_err());
        outputs[0].scale = Some(1.25);
        let old = writer.submit(&outputs, true).unwrap();
        outputs[0].scale = Some(1.5);
        let latest = writer.submit(&outputs, true).unwrap();
        assert!(old.recv_blocking().unwrap().is_err());
        release.send(()).unwrap();
        assert_eq!(active.recv_blocking().unwrap(), Ok(()));
        assert_eq!(duplicate.recv_blocking().unwrap(), Ok(()));
        assert_eq!(latest.recv_blocking().unwrap(), Ok(()));
        assert_eq!(
            writer.state.lock().committed.as_deref(),
            Some(outputs.as_slice())
        );
    }
}
