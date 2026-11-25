//! Wayland waits that a stalled compositor cannot hold forever.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::protocol::wl_callback;
use wayland_client::{Connection, Dispatch, EventQueue};

/// A client state that counts the `wl_display.sync` callbacks it has seen.
pub(crate) trait Synced {
    fn synced(&self) -> u64;
}

/// Dispatches `queue` until `done` holds, giving up after `timeout`.
pub(crate) fn wait_until<S: 'static>(conn: &Connection, queue: &mut EventQueue<S>, state: &mut S, timeout: Duration, mut done: impl FnMut(&S) -> bool) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        queue.dispatch_pending(state)?;
        if done(state) {
            return Ok(());
        }
        conn.flush()?;
        let Some(guard) = queue.prepare_read() else { continue };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("{}", mandara_i18n::t("The compositor did not respond in time"));
        }
        let timeout = Timespec { tv_sec: remaining.as_secs() as i64, tv_nsec: i64::from(remaining.subsec_nanos()) };
        let fd = guard.connection_fd();
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => bail!("{}", mandara_i18n::t("The compositor did not respond in time")),
            Ok(_) if fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) => bail!("{}", mandara_i18n::t("The compositor connection closed")),
            Ok(_) => {
                guard.read()?;
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

/// A round trip bounded by [`crate::REQUEST_TIMEOUT`].
pub(crate) fn roundtrip<S: Synced + Dispatch<wl_callback::WlCallback, ()> + 'static>(conn: &Connection, queue: &mut EventQueue<S>, state: &mut S) -> Result<()> {
    let synced = state.synced();
    conn.display().sync(&queue.handle(), ());
    wait_until(conn, queue, state, crate::REQUEST_TIMEOUT, |s| s.synced() != synced)
}
