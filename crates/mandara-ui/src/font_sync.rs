//! Watches typography settings and pushes Mandara's families to the
//! desktop when System font sync is on (`mandara_services::font_sync`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Mutex;
use std::time::Duration;

use gpui::{App, Task};
use mandara_config::{FontSyncConfig, TypographyConfig};
use mandara_services::font_sync::{self, ApplyEnv, FontGsettingsCommand, Report};

/// Debounce while the user flips several toggles or picks fonts.
const SETTLE_MS: u64 = 400;
const SETTLE: Duration = Duration::from_millis(SETTLE_MS);

struct Job {
    cfg: mandara_config::ShellConfig,
    /// When set, the worker sends the reconcile result to this caller.
    reply: Option<SyncSender<Result<Report, String>>>,
}

/// Ordered work for the single `mandara-font-sync` worker.
static QUEUE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
/// True while the worker owns the loop.
static WORKER: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    sync: FontSyncConfig,
    ui: String,
    display: String,
    mono: String,
}

impl Key {
    fn from(t: &TypographyConfig) -> Self {
        Self { sync: t.sync.clone(), ui: t.ui_font.clone(), display: t.display_font.clone(), mono: t.mono_font.clone() }
    }
}

fn enqueue(job: Job) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push_back(job);
    }
}

fn ensure_worker() -> Result<(), String> {
    if WORKER.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    match std::thread::Builder::new().name("mandara-font-sync".into()).spawn(worker_loop) {
        Ok(_) => Ok(()),
        Err(err) => {
            WORKER.store(false, Ordering::SeqCst);
            tracing::warn!(error = %err, "system font sync worker failed to start");
            Err(format!("system font sync worker failed to start: {err}"))
        }
    }
}

fn worker_loop() {
    loop {
        let jobs = match QUEUE.lock() {
            Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
            _ => {
                WORKER.store(false, Ordering::SeqCst);
                // A submit may have raced after we saw an empty queue.
                if QUEUE.lock().ok().is_some_and(|q| !q.is_empty()) && !WORKER.swap(true, Ordering::SeqCst) {
                    continue;
                }
                break;
            }
        };
        for job in coalesce(jobs) {
            let result = font_sync::reconcile(&job.cfg, &ApplyEnv::current(), &FontGsettingsCommand);
            match job.reply {
                Some(tx) => {
                    let _ = tx.send(result);
                }
                None => {
                    if let Err(err) = result {
                        tracing::warn!(error = %err, "system font sync");
                    }
                }
            }
        }
    }
}

/// Fire-and-forget jobs without a reply collapse to the latest config;
/// jobs that need a reply keep their order so callers see their own result.
fn coalesce(jobs: VecDeque<Job>) -> Vec<Job> {
    let mut out = Vec::with_capacity(jobs.len());
    let mut pending_silent: Option<Job> = None;
    for job in jobs {
        if job.reply.is_some() {
            if let Some(silent) = pending_silent.take() {
                out.push(silent);
            }
            out.push(job);
        } else {
            pending_silent = Some(job);
        }
    }
    if let Some(silent) = pending_silent {
        out.push(silent);
    }
    out
}

/// Applies or restores system fonts from the current config (single worker).
pub fn reconcile_now(cfg: &mandara_config::ShellConfig) {
    enqueue(Job { cfg: cfg.clone(), reply: None });
    let _ = ensure_worker();
}

/// Same worker queue as [`reconcile_now`], but waits for this job's result.
pub fn reconcile_wait(cfg: mandara_config::ShellConfig) -> Result<Report, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    enqueue(Job { cfg, reply: Some(tx) });
    ensure_worker()?;
    rx.recv().unwrap_or_else(|_| Err("system font sync worker stopped".into()))
}

/// Observe the config store and reconcile after typography / sync changes settle.
pub fn init_font_sync(cx: &mut App) {
    let last: Rc<RefCell<Option<Key>>> = Rc::default();
    let pending: Rc<RefCell<Option<Task<()>>>> = Rc::default();
    let on_change = Rc::new(RefCell::new({
        let last = Rc::clone(&last);
        let pending = Rc::clone(&pending);
        move |cx: &mut App| {
            let cfg = crate::state::config(cx).read(cx).get().clone();
            let key = Key::from(&cfg.typography);
            if last.borrow().as_ref() == Some(&key) {
                return;
            }
            *last.borrow_mut() = Some(key);
            let pending_slot = Rc::clone(&pending);
            *pending.borrow_mut() = Some(cx.spawn(async move |cx| {
                cx.background_executor().timer(SETTLE).await;
                cx.update(|cx| {
                    let cfg = crate::state::config(cx).read(cx).get().clone();
                    reconcile_now(&cfg);
                });
                *pending_slot.borrow_mut() = None;
            }));
        }
    }));
    (on_change.borrow_mut())(cx);
    cx.observe(&crate::state::config(cx), move |_, cx| {
        (on_change.borrow_mut())(cx);
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_tracks_sync_and_families() {
        let mut t = TypographyConfig::default();
        let a = Key::from(&t);
        t.sync.enabled = true;
        assert_ne!(a, Key::from(&t));
        t.ui_font = "Cairo".into();
        assert_ne!(a, Key::from(&t));
    }

    #[test]
    fn coalesce_keeps_latest_silent_and_ordered_replies() {
        let cfg = mandara_config::ShellConfig::default();
        let (tx, _rx) = mpsc::sync_channel(1);
        let jobs = VecDeque::from([
            Job { cfg: cfg.clone(), reply: None },
            Job { cfg: cfg.clone(), reply: None },
            Job { cfg: cfg.clone(), reply: Some(tx) },
            Job { cfg: cfg.clone(), reply: None },
        ]);
        let out = coalesce(jobs);
        assert_eq!(out.len(), 3);
        assert!(out[0].reply.is_none());
        assert!(out[1].reply.is_some());
        assert!(out[2].reply.is_none());
    }
}
