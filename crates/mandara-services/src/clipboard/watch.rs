//! Shared ext/wlr clipboard watching, with bounded reads and explicit shutdown.

use super::*;
use wl_clipboard_rs::watch::{CancelHandle, ClipboardEvent, ClipboardType, Watcher};

pub(super) enum Event {
    Ready(bool),
    Entry(ClipEntry),
    Cleared,
}

#[derive(Default)]
struct StopState {
    stopped: bool,
    handle: Option<CancelHandle>,
}

/// Dropping the service wakes its watcher even when the clipboard is idle.
pub(super) struct Stop(Arc<Mutex<StopState>>);

impl Drop for Stop {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.stopped = true;
        if let Some(handle) = state.handle.take() { handle.cancel(); }
    }
}

pub(super) fn start(events: async_channel::Sender<Event>, shared: Arc<Mutex<Shared>>) -> Stop {
    let state = Arc::new(Mutex::new(StopState::default()));
    let control = state.clone();
    let failed = events.clone();
    if let Err(e) = std::thread::Builder::new().name("mandara-clipboard-watch".into()).spawn(move || {
        if let Err(e) = run(&events, shared, control) {
            let _ = events.send_blocking(Event::Ready(false));
            tracing::warn!("clipboard watcher: {e}");
        }
    }) {
        let _ = failed.try_send(Event::Ready(false));
        tracing::warn!("starting clipboard watcher: {e}");
    }
    Stop(state)
}

fn run(events: &async_channel::Sender<Event>, shared: Arc<Mutex<Shared>>, control: Arc<Mutex<StopState>>) -> Result<(), String> {
    let mut watcher = Watcher::new(ClipboardType::Regular, wl_clipboard_rs::paste::Seat::Unspecified).map_err(|e| e.to_string())?;
    {
        let mut state = control.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped { return Ok(()); }
        state.handle = Some(watcher.cancel_handle());
    }
    if events.send_blocking(Event::Ready(true)).is_err() { return Ok(()); }
    while !events.is_closed() {
        let Some(event) = watcher.next_event().map_err(|e| e.to_string())? else { break };
        match event {
            ClipboardEvent::Changed { mime_types, mut offer, .. } => {
                let types: Vec<&str> = mime_types.iter().map(String::as_str).collect();
                let (policy, app) = shared.lock().map(|s| (s.policy.clone(), s.focused_app.clone())).unwrap_or_default();
                if let Some(entry) = capture(&types, &policy, &app, |mime| read(offer.receive(mime).ok()?, READ_TIMEOUT, READ_CEILING))
                    && events.send_blocking(Event::Entry(entry)).is_err() { break; }
            }
            ClipboardEvent::Cleared { .. } => if events.send_blocking(Event::Cleared).is_err() { break; },
        }
    }
    Ok(())
}

/// A clipboard owner may never close its pipe. Bound both time and memory;
/// one byte beyond the limit lets the existing capture policy reject it.
fn read(mut source: impl Read + AsFd, timeout: Duration, limit: usize) -> Option<Vec<u8>> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let deadline = std::time::Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK.min(limit.saturating_add(1))];
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() { return None; }
        let timeout = Timespec { tv_sec: remaining.as_secs() as i64, tv_nsec: i64::from(remaining.subsec_nanos()) };
        let mut fds = [PollFd::new(&source, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => return None,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return None,
        }
        let wanted = chunk.len().min(limit.saturating_add(1).saturating_sub(bytes.len()));
        match source.read(&mut chunk[..wanted]) {
            Ok(0) => return Some(bytes),
            Ok(n) => {
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.len() > limit { return Some(bytes); }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keep stalled-source tests quick without changing the production bound.
    const TEST_WAIT: Duration = Duration::from_millis(20);
    /// Time for the isolated compositor to transfer a fixture selection.
    const FIXTURE_WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn reads_finish_at_eof_and_keep_only_one_byte_past_the_limit() {
        let (rd, mut wr) = std::io::pipe().unwrap();
        wr.write_all(b"hello").unwrap();
        drop(wr);
        assert_eq!(read(rd, TEST_WAIT, 8), Some(b"hello".to_vec()));
        let (rd, mut wr) = std::io::pipe().unwrap();
        wr.write_all(&[0; 128]).unwrap();
        assert_eq!(read(rd, TEST_WAIT, 16).unwrap().len(), 17);
    }

    #[test]
    fn a_source_that_never_closes_does_not_stall_the_watcher() {
        let (rd, mut wr) = std::io::pipe().unwrap();
        wr.write_all(b"partial").unwrap();
        assert_eq!(read(rd, TEST_WAIT, 128), None);
    }

    #[test]
    #[ignore = "requires the isolated KWin container and its marker"]
    fn native_kwin_clipboard_capture_and_idle_shutdown() {
        let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        assert!(runtime.starts_with("/tmp") || runtime.starts_with("/var/tmp"));
        assert_eq!(std::fs::read_to_string(runtime.join(".mandara-kwin-test")).unwrap().trim(), "mandara isolated kwin test");
        assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap(), "unix:path=/nonexistent");
        let (tx, rx) = async_channel::bounded(EVENT_QUEUE);
        let stop = start(tx, Arc::default());
        let receive = || crate::runtime::block_on(async {
            use futures::FutureExt as _;
            futures::select! {
                event = rx.recv().fuse() => event.ok(),
                _ = futures::FutureExt::fuse(async_io::Timer::after(FIXTURE_WAIT)) => panic!("clipboard fixture timed out"),
            }
        });
        assert!(matches!(receive(), Some(Event::Ready(true))));
        serve(b"Mandara isolated clipboard fixture".to_vec(), None).unwrap();
        loop {
            if let Some(Event::Entry(entry)) = receive() {
                assert_eq!(entry.preview(128), "Mandara isolated clipboard fixture");
                break;
            }
        }
        drop(stop);
        while receive().is_some() {}
    }
}
