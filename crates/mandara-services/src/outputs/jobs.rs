//! One bounded queue for blocking display and monitor operations.

use async_channel::{Receiver, Sender};
use std::sync::OnceLock;

/// Requests queued behind the active operation. Repeated callers receive a busy reply.
const QUEUED_OPERATIONS: usize = 8;
type Job = Box<dyn FnOnce() + Send>;

fn queue() -> &'static Sender<Job> {
    static QUEUE: OnceLock<Sender<Job>> = OnceLock::new();
    QUEUE.get_or_init(|| start(QUEUED_OPERATIONS))
}

fn start(capacity: usize) -> Sender<Job> {
    let (sender, receiver) = async_channel::bounded::<Job>(capacity);
    crate::runtime::spawn("display-operations", async move {
        while let Ok(job) = receiver.recv().await {
            crate::runtime::unblock(job).await;
        }
    });
    sender
}

fn submit<T: Send + 'static>(
    queue: &Sender<Job>,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<Receiver<Result<T, String>>, String> {
    let (sender, receiver) = async_channel::bounded(1);
    queue
        .try_send(Box::new(move || {
            let _ = sender.try_send(work());
        }))
        .map_err(|_| {
            mandara_i18n::t("Display controls are busy; try again in a moment").to_string()
        })?;
    Ok(receiver)
}

pub async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    submit(queue(), work)?
        .recv()
        .await
        .map_err(|_| "display operation stopped".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_are_serialized_and_excess_requests_are_rejected() {
        let queue = start(1);
        let (entered, started) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let first = submit(&queue, move || {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Ok(1)
        })
        .unwrap();
        started.recv().unwrap();
        let second = submit(&queue, || Ok(2)).unwrap();
        assert!(submit(&queue, || Ok(3)).is_err());
        release.send(()).unwrap();
        assert_eq!(first.recv_blocking().unwrap(), Ok(1));
        assert_eq!(second.recv_blocking().unwrap(), Ok(2));
    }
}
