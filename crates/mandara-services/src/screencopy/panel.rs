//! One bounded panel capture in a disposable worker, without a renderer cache.

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::{Read, Write}, process::{Command, Stdio}, time::{Duration, Instant}};
use std::os::unix::process::CommandExt as _;

/// A native 4K RGBA panel fits; larger requests fail before allocating pixels.
pub(super) const MAX_RAW_BYTES: usize = 32 << 20;
/// Match the shared compressed clipboard image limit.
const MAX_PNG_BYTES: usize = crate::screenshot::png::MAX_BYTES;
/// Reserve the retained output before the worker allocates temporary pixels.
const INITIAL_OUTPUT_BYTES: usize = 64 << 10;
/// Drain a pipe in small stack chunks without another heap buffer.
const READ_CHUNK_BYTES: usize = 8 << 10;
/// Output names and logical rectangle metadata need only a small message.
const MAX_REQUEST_BYTES: u64 = 4096;
/// A stalled compositor must not retain a capture job indefinitely.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Cancellation is checked only while an explicitly requested capture is active.
const CANCEL_INTERVAL: Duration = Duration::from_millis(10);
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The small wire request sent to the disposable capture worker.
#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub output: String,
    /// Output-local logical coordinates, excluding the panel's external controls.
    pub rect: (i32, i32, i32, i32),
}

#[derive(Serialize, Deserialize)]
struct Failure { error: String }

/// Hidden CLI entry point. Native pixels and encoder scratch die with this process.
pub fn worker_main() -> Result<()> {
    let result = (|| -> Result<Vec<u8>> {
        let mut wire = Vec::new();
        std::io::stdin().take(MAX_REQUEST_BYTES + 1).read_to_end(&mut wire)?;
        ensure!(wire.len() as u64 <= MAX_REQUEST_BYTES, "capture request too large");
        let request: Request = serde_json::from_slice(&wire)?;
        let mut output = crate::screenshot::png::Output::new();
        let image = super::capture_region(&request.output, request.rect)?;
        use image::ImageEncoder as _;
        image::codecs::png::PngEncoder::new(&mut output).write_image(&image.rgba, image.width, image.height, image::ExtendedColorType::Rgba8)?;
        Ok(output.finish())
    })();
    let mut stdout = std::io::stdout().lock();
    match result {
        Ok(bytes) => stdout.write_all(&bytes)?,
        Err(error) => serde_json::to_writer(&mut stdout, &Failure { error: format!("{error:#}") })?,
    }
    Ok(())
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

/// Run off the UI thread. Cancellation, failure and timeout kill and reap the worker.
pub fn run(request: Request, current: impl Fn() -> bool) -> Result<Vec<u8>> {
    let wire = serde_json::to_vec(&request)?;
    ensure!(wire.len() as u64 <= MAX_REQUEST_BYTES, "capture request too large");
    let mut bytes = Vec::with_capacity(INITIAL_OUTPUT_BYTES);
    ensure!(current(), "capture cancelled");
    let executable = std::path::Path::new("/proc/self/exe");
    let mut command = Command::new(executable);
    command.arg("panel-capture-worker").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: only async-signal-safe syscalls run between fork and exec. There
    // is no persistent state to flush if the shell exits during a capture.
    unsafe { command.pre_exec(move || {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) != 0 { return Err(std::io::Error::last_os_error()); }
        if libc::getppid() != parent { return Err(std::io::Error::from_raw_os_error(libc::ESRCH)); }
        Ok(())
    }); }
    let mut child = Child(command.spawn()?);
    child.0.stdin.take().context("missing capture worker input")?.write_all(&wire)?;
    let mut output = child.0.stdout.take().context("missing capture worker output")?;
    let flags = rustix::fs::fcntl_getfl(&output)?;
    rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK)?;
    let deadline = Instant::now() + TIMEOUT;
    let mut chunk = [0; READ_CHUNK_BYTES];
    let mut eof = false;
    loop {
        ensure!(current(), "capture cancelled");
        ensure!(Instant::now() < deadline, "panel capture timed out");
        match output.read(&mut chunk) {
            Ok(0) => eof = true,
            Ok(length) => {
                append(&mut bytes, &chunk[..length])?;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {},
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
        if eof && let Some(status) = child.0.try_wait()? {
            ensure!(status.success(), "panel capture worker failed");
            return response(bytes);
        }
        wait_ready(&output, eof)?;
    }
}

/// Wake as soon as bytes arrive, while bounding cancellation latency.
fn wait_ready(output: &impl std::os::fd::AsFd, eof: bool) -> Result<()> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let timeout = Timespec::try_from(CANCEL_INTERVAL)?;
    let mut fds = [PollFd::new(output, PollFlags::IN)];
    let watched = if eof { &mut fds[..0] } else { &mut fds[..] };
    match poll(watched, Some(&timeout)) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Reject oversized output before extending the retained PNG buffer.
fn append(bytes: &mut Vec<u8>, part: &[u8]) -> Result<()> {
    ensure!(bytes.len().checked_add(part.len()).is_some_and(|length| length <= MAX_PNG_BYTES), "panel image exceeds the clipboard limit");
    bytes.try_reserve(part.len())?;
    bytes.extend_from_slice(part);
    Ok(())
}

/// Move PNG output to the caller or report the worker's structured failure.
fn response(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.starts_with(PNG_SIGNATURE) { return Ok(bytes); }
    let failure: Failure = serde_json::from_slice(&bytes).context("invalid capture worker response")?;
    anyhow::bail!(failure.error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_handles_idle_data_and_closed_pipes() {
        let (mut reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        wait_ready(&reader, false).unwrap();
        writer.write_all(&[7, 8, 9]).unwrap();
        wait_ready(&reader, false).unwrap();
        let mut bytes = [0; 3]; reader.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [7, 8, 9]);
        drop(writer);
        wait_ready(&reader, false).unwrap();
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
        wait_ready(&reader, true).unwrap();
    }
    #[test]
    fn encoded_pixels_move_without_a_second_buffer_and_failures_are_reported() {
        let bytes = PNG_SIGNATURE.to_vec(); let pointer = bytes.as_ptr();
        let returned = response(bytes).unwrap();
        assert_eq!(returned.as_ptr(), pointer);
        assert!(response(br#"{"error":"output disconnected"}"#.to_vec()).unwrap_err().to_string().contains("output disconnected"));
        assert!(response(vec![]).is_err());
        let mut full = vec![0; MAX_PNG_BYTES];
        assert!(append(&mut full, &[1]).is_err());
        assert_eq!(full.len(), MAX_PNG_BYTES);
    }
    #[test]
    fn cancellation_does_not_start_a_capture_process() {
        let request = Request { output: "fixture".into(), rect: (0, 0, 10, 10) };
        assert!(run(request, || false).unwrap_err().to_string().contains("cancelled"));
    }
}
