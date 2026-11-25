//! Bounded display transactions and an interruptible hotplug wait.

use super::State;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::time::{Duration, Instant};
use wayland_client::{Connection, EventQueue};

/// A missing protocol reply must not strand Keep, revert or a compositor reload.
const QUERY_TIMEOUT_SECONDS: u64 = 5;
pub(super) const QUERY_TIMEOUT: Duration = Duration::from_secs(QUERY_TIMEOUT_SECONDS);

pub(super) fn dispatch(
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Option<Instant>,
    stop: Option<&std::io::PipeReader>,
) -> Result<(), String> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err("display server did not reply before the deadline".into());
    }
    queue.flush().map_err(|error| error.to_string())?;
    if queue
        .dispatch_pending(state)
        .map_err(|error| error.to_string())?
        > 0
    {
        return Ok(());
    }
    let Some(guard) = queue.prepare_read() else {
        return Ok(());
    };
    let timeout = deadline.map(|deadline| {
        let left = deadline.saturating_duration_since(Instant::now());
        Timespec {
            tv_sec: left.as_secs() as i64,
            tv_nsec: i64::from(left.subsec_nanos()),
        }
    });
    let descriptor = guard.connection_fd();
    let mut descriptors = [
        PollFd::new(&descriptor, PollFlags::IN),
        PollFd::new(&descriptor, PollFlags::empty()),
    ];
    let count = if let Some(stop) = stop {
        descriptors[1] = PollFd::new(stop, PollFlags::IN);
        2
    } else {
        1
    };
    match poll(&mut descriptors[..count], timeout.as_ref()) {
        Ok(0) => return Err("display server did not reply before the deadline".into()),
        Ok(_) => {}
        Err(rustix::io::Errno::INTR) => return Ok(()),
        Err(error) => return Err(error.to_string()),
    }
    if count == 2 && !descriptors[1].revents().is_empty() {
        return Err("display watcher stopped".into());
    }
    guard.read().map_err(|error| error.to_string())?;
    queue
        .dispatch_pending(state)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn roundtrip(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    stop: Option<&std::io::PipeReader>,
) -> Result<(), String> {
    state.callback_done = false;
    let _callback = connection.display().sync(&queue.handle(), ());
    let deadline = Instant::now() + QUERY_TIMEOUT;
    while !state.callback_done {
        dispatch(queue, state, Some(deadline), stop)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_server_times_out_and_a_stopped_watcher_wakes_without_polling() {
        let (client, _server) = std::os::unix::net::UnixStream::pair().unwrap();
        let connection = Connection::from_socket(client).unwrap();
        let mut queue = connection.new_event_queue();
        let mut state = State::default();
        let started = Instant::now();
        assert!(
            dispatch(
                &mut queue,
                &mut state,
                Some(started + Duration::from_millis(20)),
                None
            )
            .unwrap_err()
            .contains("deadline")
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        let (reader, writer) = std::io::pipe().unwrap();
        drop(writer);
        assert!(
            dispatch(&mut queue, &mut state, None, Some(&reader))
                .unwrap_err()
                .contains("stopped")
        );
    }
}
