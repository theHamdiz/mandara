//! Bounded asynchronous post hooks, with explicit barriers between dependent steps.

use super::{apply::Report, process};
use anyhow::Result;
use mandara_theme::templates::custom::Command;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::Duration;

/// App reloads can overlap, but never fan out across the full catalog.
const MAX_RUNNING: usize = 2;
/// Only an active export waits here; cancellation still reaches its child processes.
const CANCEL_CHECK_MILLIS: u64 = 20;
const CANCEL_CHECK: Duration = Duration::from_millis(CANCEL_CHECK_MILLIS);

struct Running {
    owner: String,
    result: mpsc::Receiver<Result<()>>,
}

pub(super) struct Hooks<'a> {
    cancel: &'a AtomicBool,
    stop: Arc<AtomicBool>,
    pending: VecDeque<Running>,
}
impl<'a> Hooks<'a> {
    pub fn new(cancel: &'a AtomicBool) -> Self {
        Self { cancel,stop:Arc::new(AtomicBool::new(false)),pending:VecDeque::with_capacity(MAX_RUNNING) }
    }
    fn cancelled(&self) {
        if self.cancel.load(Ordering::Acquire) { self.stop.store(true,Ordering::Release); }
    }
    fn wait_one(&mut self,report: &mut Report) {
        let Some(job) = self.pending.pop_front() else { return };
        let result = loop {
            self.cancelled();
            match job.result.recv_timeout(CANCEL_CHECK) {
                Ok(result) => break result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break Err(anyhow::anyhow!("template post hook stopped")),
            }
        };
        if let Err(error) = result { report.errors.insert(job.owner,format!("post hook: {error:#}")); }
    }
    pub fn barrier(&mut self,report: &mut Report) {
        while !self.pending.is_empty() { self.wait_one(report); }
    }
    fn start(&mut self,owner: &str,report: &mut Report,work: impl FnOnce(Arc<AtomicBool>) -> Result<()> + Send + 'static) {
        if self.pending.len() == MAX_RUNNING { self.wait_one(report); }
        self.cancelled();
        let stop = self.stop.clone();
        let (sender,result) = mpsc::sync_channel(1);
        crate::runtime::spawn("app-theme-post-hook",async move {
            let result = crate::runtime::unblock(move ||work(stop)).await;
            let _ = sender.send(result);
        });
        self.pending.push_back(Running { owner:owner.into(),result });
    }
    pub fn post(&mut self,owner: &str,command: Command,base: &Path,environment: &[(String,String)],lease: Option<Arc<std::fs::File>>,report: &mut Report) {
        let base = base.to_path_buf();
        let environment = environment.to_vec();
        self.start(owner,report,move |stop| {
            let _lease = lease;
            process::run(&command,&base,&environment,&stop,process::TIMEOUT).map(|_|())
        });
    }
}
impl Drop for Hooks<'_> {
    fn drop(&mut self) {
        self.stop.store(true,Ordering::Release);
        while let Some(job) = self.pending.pop_front() { let _ = job.result.recv(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn async_hooks_overlap_with_a_fixed_bound_and_barriers_report_failures() {
        let cancel = AtomicBool::new(false);
        let mut hooks = Hooks::new(&cancel);
        let mut report = Report::default();
        let barrier = Arc::new(std::sync::Barrier::new(MAX_RUNNING+1));
        let (sender,receiver) = mpsc::channel();
        for index in 0..MAX_RUNNING {
            let barrier = barrier.clone();
            let sender = sender.clone();
            hooks.start(&format!("app/{index}"),&mut report,move |_| { sender.send(index).unwrap();barrier.wait();Ok(()) });
        }
        for _ in 0..MAX_RUNNING { receiver.recv().unwrap(); }
        assert_eq!(hooks.pending.len(),MAX_RUNNING);
        barrier.wait();
        for index in 0..10 { hooks.start(&format!("later/{index}"),&mut report,|_|Ok(()));assert!(hooks.pending.len() <= MAX_RUNNING); }
        hooks.start("broken",&mut report,|_|anyhow::bail!("reload failed"));
        hooks.barrier(&mut report);
        assert!(hooks.pending.is_empty());
        assert!(report.errors["broken"].contains("reload failed"));
    }
    #[test]
    fn dropping_an_export_cancels_and_joins_its_active_hooks() {
        let cancel = AtomicBool::new(false);
        let stopped = Arc::new(AtomicBool::new(false));
        let mut hooks = Hooks::new(&cancel);
        let ended = stopped.clone();
        hooks.start("app",&mut Report::default(),move |stop| { while !stop.load(Ordering::Acquire) { std::thread::yield_now(); } ended.store(true,Ordering::Release);Ok(()) });
        drop(hooks);
        assert!(stopped.load(Ordering::Acquire));
    }
}
