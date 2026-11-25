//! Waits for a sandbox shell to answer IPC after it starts.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Startup includes service discovery and the first shell surfaces.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// A not-yet-ready shell must not hold one probe for the whole startup.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);
/// A ping response is small; a corrupt reply cannot grow without a bound.
const MAX_REPLY_BYTES: u64 = 4096;

pub(super) fn wait(runtime: &Path) -> crate::Result<()> {
    let socket = runtime.join("mandara.sock");
    if crate::proc::wait_until(START_TIMEOUT, || ready(&socket)) {
        Ok(())
    } else {
        Err(format!("the sandbox shell did not answer IPC at {}", socket.display()))
    }
}

fn ready(socket: &Path) -> bool {
    let probe = || -> std::io::Result<bool> {
        let mut stream = UnixStream::connect(socket)?;
        stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
        stream.set_write_timeout(Some(PROBE_TIMEOUT))?;
        stream.write_all(b"ping\n")?;
        let mut reply = String::new();
        BufReader::new(stream.take(MAX_REPLY_BYTES)).read_line(&mut reply)?;
        Ok(serde_json::from_str::<serde_json::Value>(&reply).ok().is_some_and(|v| v.get("ok").and_then(|v| v.as_bool()) == Some(true)))
    };
    probe().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_bound_socket_is_ready_only_after_a_successful_ping() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mandara.sock");
        assert!(!ready(&socket));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            for reply in ["{\"ok\":false}\n", "invalid\n", "{\"ok\":true,\"data\":\"pong\"}\n"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut command).unwrap();
                assert_eq!(command, "ping\n");
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        assert!(!ready(&socket));
        assert!(!ready(&socket));
        assert!(ready(&socket));
        server.join().unwrap();
    }
}
