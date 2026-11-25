//! labwc and COSMIC: the standard `ext-workspace-v1` Wayland protocol.
//!
//! Neither compositor has an IPC socket. Both publish their workspaces
//! with `ext_workspace_manager_v1` (labwc since 0.8.3: one group on every
//! output; cosmic-comp: a group per output, or one for all when workspaces
//! span displays), so one driver serves both:
//!
//! * one owned thread (`labwc-workspaces` / `cosmic-workspaces`) holds one
//!   Wayland connection and the [`state::State`] of every group, workspace,
//!   output and window, installed atomically on the manager's `done`; a
//!   copy is published to queries after each completed batch, and only
//!   what changed reaches the bar ([`state::Diff`]).
//! * clicks never wait on the compositor: they go to the thread through a
//!   bounded queue and a wake pipe, are resolved there against the current
//!   state (bar number → the workspace at that position on the output) and
//!   sent as `activate` + `commit`.
//! * windows come from `zwlr_foreign_toplevel_manager_v1` on labwc
//!   (title, app id, state, outputs; it doesn't say which workspace a
//!   window is on), and on COSMIC from `ext_foreign_toplevel_list_v1` with
//!   COSMIC's toplevel info (state, outputs, workspaces) and manager (moving
//!   a window to a workspace).
//!
//! Both keep their settings in their own files and apps (labwc's rc.xml,
//! COSMIC Settings): Mandara writes no drop-in for either
//! ([`mandara_config::compositor::Target::self_configured`]).

pub mod cosmic;
pub mod session;
pub mod state;

use std::io::{PipeWriter, Write as _};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel};
pub use session::{Options, Session};
use state::{Diff, State};

/// Clicks queued for the thread before new ones are refused as busy.
const COMMAND_QUEUE: usize = 32;
/// The longest pause between reconnection attempts.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// What the bar asks for, resolved on the thread.
#[derive(Debug, PartialEq)]
enum Job {
    /// Workspace `number` of output `monitor` (the focused one when empty).
    Activate { monitor: String, number: u32 },
    /// A window (the focused one for `None`) to workspace `number` of its
    /// output.
    Move { client: Option<u64>, number: u32 },
    /// A new subscriber: send it everything once.
    Resend,
}

#[derive(Default)]
struct Live {
    state: State,
    /// The thread has a connection and its first state.
    ready: bool,
    /// Windows can be moved between workspaces (COSMIC's manager).
    can_move: bool,
    /// Connection attempts that failed, so a first query stops waiting.
    failures: u32,
}

struct Shared {
    live: Mutex<Live>,
    changed: Condvar,
    events: Mutex<Option<async_channel::Sender<CompositorEvent>>>,
}

impl Shared {
    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn events(&self) -> MutexGuard<'_, Option<async_channel::Sender<CompositorEvent>>> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The running thread's queue and wake pipe; dropping it stops the thread.
struct Control {
    jobs: SyncSender<Job>,
    wake: PipeWriter,
}

pub struct ExtWorkspaceDriver {
    kind: CompositorKind,
    shared: Arc<Shared>,
    control: Mutex<Option<Control>>,
}

impl ExtWorkspaceDriver {
    /// A driver for labwc or COSMIC. Construction runs on the UI thread and
    /// never connects: the thread starts on first use.
    pub fn from_env(kind: CompositorKind) -> Option<Self> {
        Self::for_display(kind, std::env::var_os("WAYLAND_DISPLAY").as_deref())
    }

    fn for_display(kind: CompositorKind, display: Option<&std::ffi::OsStr>) -> Option<Self> {
        if !matches!(kind, CompositorKind::Labwc | CompositorKind::Cosmic) {
            return None;
        }
        display.filter(|d| !d.is_empty())?;
        let shared = Arc::new(Shared { live: Mutex::new(Live::default()), changed: Condvar::new(), events: Mutex::new(None) });
        Some(Self { kind, shared, control: Mutex::new(None) })
    }

    fn thread_name(&self) -> &'static str {
        if self.kind == CompositorKind::Cosmic { "cosmic-workspaces" } else { "labwc-workspaces" }
    }

    /// Queues `job`, starting the thread when it isn't running. `true`
    /// when this call started it.
    fn submit(&self, job: Job) -> Result<bool> {
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        let (job, started) = match control.as_mut() {
            Some(c) => match c.jobs.try_send(job) {
                Ok(()) => {
                    let _ = c.wake.write(&[1]);
                    return Ok(false);
                }
                Err(TrySendError::Full(_)) => bail!("ext-workspace: {COMMAND_QUEUE} requests are already waiting"),
                // The thread is gone: start another.
                Err(TrySendError::Disconnected(job)) => (job, true),
            },
            None => (job, true),
        };
        let (jobs, rx) = std::sync::mpsc::sync_channel(COMMAND_QUEUE);
        let (wake_rx, wake) = std::io::pipe().context("ext-workspace: wake pipe")?;
        let shared = self.shared.clone();
        std::thread::Builder::new().name(self.thread_name().into()).spawn(move || run(&shared, &rx, &wake_rx)).context("ext-workspace: starting its thread")?;
        jobs.try_send(job).map_err(|_| anyhow!("ext-workspace: the thread stopped"))?;
        *control = Some(Control { jobs, wake });
        Ok(started)
    }

    /// `f` over the published state. The call that starts the thread (the
    /// shell's first load, on a background thread) waits for the first
    /// state; later ones never wait on the compositor.
    fn query<T>(&self, f: impl FnOnce(&State) -> T) -> Result<T> {
        let started = self.control.lock().unwrap_or_else(PoisonError::into_inner).is_none() && self.submit(Job::Resend)?;
        let live = self.shared.live();
        // Opening takes two bounded round trips.
        let wait = if started { crate::REQUEST_TIMEOUT * 3 } else { Duration::ZERO };
        let failures = live.failures;
        let (live, _) = self.shared.changed.wait_timeout_while(live, wait, |l| !l.ready && l.failures == failures).unwrap_or_else(PoisonError::into_inner);
        if !live.ready {
            bail!("{}", mandara_i18n::t("The compositor did not respond in time"));
        }
        Ok(f(&live.state))
    }
}

/// The thread: one connection at a time, reconnecting with bounded backoff
/// until the driver is dropped (its wake pipe closes).
fn run(shared: &Shared, jobs: &Receiver<Job>, wake: &std::io::PipeReader) {
    let mut delay = crate::RECONNECT_DELAY;
    loop {
        match serve(shared, jobs, wake, &mut delay) {
            Ok(()) => return,
            Err(err) => tracing::debug!("ext-workspace: {err:#}"),
        }
        {
            let mut live = shared.live();
            let failures = live.failures.wrapping_add(1);
            *live = Live { failures, ..Live::default() };
        }
        shared.changed.notify_all();
        // Clicks meant for the lost state are not replayed.
        while jobs.try_recv().is_ok() {}
        if !pause(wake, delay) {
            return;
        }
        delay = (delay * 2).min(MAX_RECONNECT_DELAY);
    }
}

/// Waits `delay` before reconnecting; `false` once the driver is gone
/// (its end of the wake pipe closed).
fn pause(wake: &std::io::PipeReader, delay: Duration) -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let deadline = std::time::Instant::now() + delay;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        let timeout = Timespec { tv_sec: remaining.as_secs() as i64, tv_nsec: i64::from(remaining.subsec_nanos()) };
        let mut fds = [PollFd::new(wake, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => return true,
            Ok(_) if fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) => return false,
            Ok(_) => {
                // A request while disconnected: dropped above or on reconnect.
                let mut bytes = [0u8; 64];
                if matches!(std::io::Read::read(&mut &*wake, &mut bytes), Ok(0) | Err(_)) {
                    return false;
                }
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => return true,
        }
    }
}

/// One connection, until it breaks (`Err`) or the driver is gone (`Ok`).
fn serve(shared: &Shared, jobs: &Receiver<Job>, wake: &std::io::PipeReader, delay: &mut Duration) -> Result<()> {
    let mut session = Session::open(Options { workspaces: true })?;
    if !session.has_workspaces() {
        tracing::warn!("ext-workspace: the compositor offers no ext_workspace_manager_v1; the bar shows no workspaces");
    }
    *delay = crate::RECONNECT_DELAY;
    let mut diff = Diff::default();
    let mut out = Vec::new();
    let mut publish = true;
    loop {
        while let Ok(job) = jobs.try_recv() {
            let result = match job {
                Job::Activate { monitor, number } => session.state().plan_activate(&monitor, number).and_then(|r| session.execute(&r)),
                Job::Move { client, number } => session.state().plan_move(client, number).and_then(|r| session.execute(&[r])),
                Job::Resend => {
                    diff = Diff::default();
                    publish = true;
                    Ok(())
                }
            };
            if let Err(err) = result {
                tracing::warn!("ext-workspace: {err:#}");
            }
        }
        if session.take_published() || publish {
            publish = false;
            {
                let mut live = shared.live();
                live.state.clone_from(session.state());
                live.can_move = session.can_move_windows();
                live.ready = true;
            }
            shared.changed.notify_all();
            let sender = shared.events().clone();
            if let Some(tx) = sender {
                diff.events(session.state(), &mut out);
                for event in out.drain(..) {
                    if tx.send_blocking(event).is_err() {
                        // Nobody listens any more (the rest is dropped
                        // with the drain).
                        *shared.events() = None;
                        break;
                    }
                }
            }
        }
        if !session.wait(Some(wake))? {
            return Ok(());
        }
    }
}

impl CompositorBackend for ExtWorkspaceDriver {
    fn kind(&self) -> CompositorKind {
        self.kind
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        *self.shared.events() = Some(tx);
        if let Err(err) = self.submit(Job::Resend) {
            tracing::warn!("ext-workspace: {err:#}");
        }
        rx
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
        self.query(State::navigation)
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        self.query(State::focused)
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        self.query(State::clients)
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        self.query(State::monitors)
    }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        self.submit(Job::Activate { monitor: monitor.to_string(), number: id }).map(drop)
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        // No composite views: a workspace switch.
        self.select_primary(monitor, id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        // labwc's window protocols can't move a window to a workspace;
        // COSMIC's manager can (version 4).
        let live = self.shared.live();
        if live.ready && !live.can_move {
            bail!("{}", mandara_i18n::t("Not supported by this compositor"));
        }
        drop(live);
        self.submit(Job::Move { client: client_id, number: id }).map(drop)
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        self.move_client_to(client_id, tag)
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        bail!("{}", mandara_i18n::t("Not supported by this compositor"))
    }

    fn dispatch_raw(&self, _cmd: &str, _args: &[&str]) -> Result<()> {
        bail!("{}", mandara_i18n::t("Not supported by this compositor"))
    }

    fn reload_config(&self) -> Result<()> {
        // Mandara writes nothing either compositor reads.
        Ok(())
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        bail!("{}", mandara_i18n::t("Not supported by this compositor"))
    }

    fn sync_managed_config(&self, _config: &mandara_config::compositor::CompositorConfig, _borders: Option<mandara_config::compositor::BorderColors>) -> Result<(std::path::PathBuf, mandara_config::compositor::Emitted)> {
        let note = self.kind.target().settings_elsewhere().unwrap_or("Not supported by this compositor");
        bail!("{}", mandara_i18n::t(note))
    }
}

#[cfg(test)]
mod tests;
