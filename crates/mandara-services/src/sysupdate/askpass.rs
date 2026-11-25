//! Mandara's `sudo` askpass, for the AUR step of a system update.
//!
//! An AUR helper runs as you and starts `sudo` to install what it built.
//! Without a terminal, sudo runs the program in `SUDO_ASKPASS` (with `-A`,
//! or on its own when no terminal is attached) and reads the password from
//! its standard output. Mandara points it at itself:
//!
//! 1. While the AUR step runs, the job opens a [`Session`]: a private
//!    folder (mode 0700) in your runtime directory holding a Unix socket
//!    and two links to the `mandara` binary, [`PROGRAM`] and `sudo`, plus
//!    a random one-time token. Only the helper's environment carries the
//!    socket, the token, `SUDO_ASKPASS` and a `PATH` starting with the
//!    folder.
//! 2. The helper runs `sudo …`, which is that link: [`sudo_main`] runs the
//!    real sudo (from the root-owned system folders) as `sudo -A …`.
//!    sudo 1.9 uses an askpass without a terminal only with `-A`.
//! 3. sudo runs `mandara-askpass "[sudo] password for you: "`. That
//!    [`client_main`] sends the token and the prompt to the socket.
//! 4. The shell answers only when the caller runs as you (`SO_PEERCRED`),
//!    descends from the helper the job started, shows the right token, and
//!    the step is still running. It then shows its own password dialog
//!    through the registered [`Prompter`] and writes the password back.
//! 5. The client prints it to sudo and exits.
//!
//! Cancelling the dialog, or [`MAX_ASKS`] requests in one step (sudo gives
//! three tries per run; yay's keep-alive would ask forever), refuses every
//! later request and stops the helper: a dismissed dialog never comes back
//! by itself.
//!
//! The password is held in [`Secret`]s that are wiped when dropped; it
//! never goes to a file, an argument, the environment or the log. A request
//! waits at most [`ANSWER_TIMEOUT`] for the dialog, the client at most
//! [`CLIENT_TIMEOUT`]; with no session (no update waiting) there is no
//! socket and the client refuses at once. A request that arrives before the
//! job has recorded the helper it just started (its first `sudo` can be that
//! quick) waits up to [`REGISTER_TIMEOUT`] for it instead of being refused.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

/// The file name sudo runs; `mandara` started under it acts as the askpass.
pub const PROGRAM: &str = "mandara-askpass";
/// The socket and token, in the helper's environment only.
pub const ENV_SOCKET: &str = "MANDARA_ASKPASS_SOCKET";
pub const ENV_TOKEN: &str = "MANDARA_ASKPASS_TOKEN";
/// How long one request waits for an answer in the dialog.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(180);
/// How long the client waits for the shell (a little longer, so the shell's
/// own timeout answers first).
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(195);
/// How long the shell waits for a client to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a request with the right token waits for the job to record the
/// helper it started (the job stores it right after spawning, and the
/// helper may ask first).
pub const REGISTER_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the server looks at its stop flag and a waiting client.
const POLL: Duration = Duration::from_millis(100);
/// Random bytes of the token (hex in the environment).
const TOKEN_BYTES: usize = 32;
/// A request is the token and a prompt line.
const MAX_REQUEST: usize = 512;
const MAX_PROMPT: usize = 200;
/// Longest password accepted (the dialog takes 255 bytes).
pub const MAX_SECRET: usize = 1024;
/// Parents walked up from the caller to find the helper.
const MAX_DEPTH: usize = 64;
/// Requests answered in one step at most (one per sudo approval; a few
/// more for mistyped passwords).
pub const MAX_ASKS: u32 = 5;
/// The wrapper's file name in the folder.
const SUDO: &str = "sudo";
const OK: &[u8] = b"ok ";
const NO: &[u8] = b"no ";

/// A password, wiped when dropped (its whole buffer, not just the length).
pub struct Secret(Zeroizing<Vec<u8>>);

impl Secret {
    /// Takes `s`'s buffer without copying it.
    pub fn from_string(s: String) -> Secret {
        Secret(Zeroizing::new(s.into_bytes()))
    }

    pub fn from_bytes(b: &[u8]) -> Secret {
        let mut v = Vec::with_capacity(b.len());
        v.extend_from_slice(b);
        Secret(Zeroizing::new(v))
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Zeroes the whole buffer now (dropping does the same).
    pub fn wipe(&mut self) {
        use zeroize::Zeroize as _;
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskRequest {
    /// sudo's own prompt (`[sudo] password for you: `), cleaned and bounded.
    pub prompt: String,
    /// The helper asking (`paru`).
    pub helper: &'static str,
    /// 1 for the first request of the step; more after a wrong password.
    pub attempt: u32,
}

/// A dialog being shown: the answer arrives on `answer` (`None`:
/// cancelled); `close` takes it away early (timed out, step ended).
pub struct Pending {
    pub answer: std::sync::mpsc::Receiver<Option<Secret>>,
    pub close: Box<dyn Fn() + Send>,
}

/// Shows Mandara's password dialog (the shell registers one).
pub trait Prompter: Send + Sync {
    fn ask(&self, request: AskRequest) -> Pending;
}

static PROMPTER: Mutex<Option<Arc<dyn Prompter>>> = Mutex::new(None);

/// Registers the shell's dialog (once, at start-up).
pub fn set_prompter(p: Arc<dyn Prompter>) {
    *PROMPTER.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

/// The registered dialog, if the shell has one.
pub fn prompter() -> Option<Arc<dyn Prompter>> {
    PROMPTER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Why a request was refused (said to the client; never the password).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not from the helper this step started, or the wrong token.
    NotOurs,
    /// No update step is waiting for a password.
    NotWaiting,
    /// The dialog was cancelled.
    Cancelled,
    TimedOut,
    Malformed,
}

impl Refusal {
    fn word(self) -> &'static str {
        match self {
            Refusal::NotOurs => "not-ours",
            Refusal::NotWaiting => "not-waiting",
            Refusal::Cancelled => "cancelled",
            Refusal::TimedOut => "timed-out",
            Refusal::Malformed => "malformed",
        }
    }
}

/// What the server did with each request (tests and the step's failure
/// message read it).
#[derive(Default)]
struct Counters {
    asked: AtomicU32,
    answered: AtomicU32,
    cancelled: AtomicBool,
    refused: AtomicU32,
    /// Stopped answering for good (cancelled, or too many requests).
    gave_up: AtomicBool,
}

/// The helper's process id once the job has started it (0 before), with a
/// signal for a request that arrived first.
#[derive(Default)]
struct Root {
    pid: Mutex<i32>,
    set: Condvar,
}

impl Root {
    /// Wakes a request waiting for the helper (it was recorded, or the step
    /// is over). Taking the lock first means the wake-up can't slip in
    /// between the waiter's check and its wait.
    fn wake(&self) {
        drop(self.pid.lock().unwrap_or_else(|e| e.into_inner()));
        self.set.notify_all();
    }
}

/// The askpass of one AUR step. Dropping it stops the server, closes a
/// dialog still open and removes the folder.
pub struct Session {
    dir: tempfile::TempDir,
    socket: PathBuf,
    program: PathBuf,
    token: Zeroizing<String>,
    root: Arc<Root>,
    armed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Hooks the job gives a session: "a password is being asked for" and
/// "answered or refused", for the phase the card shows.
pub type OnAsk = Box<dyn Fn(bool) + Send>;
/// Called once when the session stops answering for good (it stops the
/// helper).
pub type OnGiveUp = Box<dyn Fn() + Send>;

impl Session {
    /// Opens a session in `parent` (your runtime folder), with sudo running
    /// `exe` (the `mandara` binary) as the askpass.
    pub fn open(parent: &Path, exe: &Path, prompter: Arc<dyn Prompter>, helper: &'static str, on_ask: OnAsk, on_give_up: OnGiveUp) -> std::io::Result<Session> {
        Session::open_with(parent, exe, prompter, helper, on_ask, on_give_up, ANSWER_TIMEOUT)
    }

    /// [`Session::open`] with a dialog timeout of `timeout`.
    pub(crate) fn open_with(parent: &Path, exe: &Path, prompter: Arc<dyn Prompter>, helper: &'static str, on_ask: OnAsk, on_give_up: OnGiveUp, timeout: Duration) -> std::io::Result<Session> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::Builder::new().prefix("mandara-askpass-").permissions(std::fs::Permissions::from_mode(0o700)).tempdir_in(parent)?;
        let socket = dir.path().join("socket");
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let program = dir.path().join(PROGRAM);
        std::os::unix::fs::symlink(exe, &program)?;
        std::os::unix::fs::symlink(exe, dir.path().join(SUDO))?;
        let token = new_token()?;
        let root = Arc::new(Root::default());
        let armed = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let server = Server { listener, token: token.clone(), root: root.clone(), armed: armed.clone(), stop: stop.clone(), counters: counters.clone(), prompter, helper, on_ask, on_give_up, timeout };
        let thread = std::thread::Builder::new().name("mandara-askpass".into()).spawn(move || server.run())?;
        Ok(Session { dir, socket, program, token, root, armed, stop, counters, thread: Some(thread) })
    }

    /// The environment the helper runs with.
    pub fn env(&self) -> [(&'static str, &std::ffi::OsStr); 3] {
        [("SUDO_ASKPASS", self.program.as_os_str()), (ENV_SOCKET, self.socket.as_os_str()), (ENV_TOKEN, std::ffi::OsStr::new(self.token.as_str()))]
    }

    /// The helper's `PATH`: this folder (its `sudo` wrapper) first.
    pub fn path(&self) -> std::ffi::OsString {
        let mut path = self.dir.path().as_os_str().to_owned();
        if let Some(rest) = std::env::var_os("PATH").filter(|p| !p.is_empty()) {
            path.push(":");
            path.push(rest);
        }
        path
    }

    /// The helper's process: only it and its descendants are answered.
    /// A request already waiting for it is answered now.
    pub fn set_root(&self, pid: u32) {
        *self.root.pid.lock().unwrap_or_else(|e| e.into_inner()) = i32::try_from(pid).unwrap_or(0);
        self.root.set.notify_all();
    }

    /// Stops answering (the step is over) before the session is dropped.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.root.wake();
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Passwords given, and whether you cancelled the dialog.
    pub fn answered(&self) -> u32 {
        self.counters.answered.load(Ordering::SeqCst)
    }

    pub fn cancelled(&self) -> bool {
        self.counters.cancelled.load(Ordering::SeqCst)
    }

    pub fn refused(&self) -> u32 {
        self.counters.refused.load(Ordering::SeqCst)
    }

    /// Whether it stopped answering for good (the helper was stopped).
    pub fn gave_up(&self) -> bool {
        self.counters.gave_up.load(Ordering::SeqCst)
    }

    pub fn folder(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.armed.store(false, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.root.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn new_token() -> std::io::Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
    getrandom::fill(bytes.as_mut()).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut hex = Zeroizing::new(String::with_capacity(TOKEN_BYTES * 2));
    for b in bytes.iter() {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// Constant-time comparison (the token must not leak through timing).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Server {
    listener: UnixListener,
    token: Zeroizing<String>,
    root: Arc<Root>,
    armed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
    prompter: Arc<dyn Prompter>,
    helper: &'static str,
    on_ask: OnAsk,
    on_give_up: OnGiveUp,
    timeout: Duration,
}

impl Server {
    fn run(self) {
        use std::os::fd::AsRawFd as _;
        let fd = self.listener.as_raw_fd();
        while !self.stop.load(Ordering::SeqCst) {
            // Sleeps in the kernel until a client connects (or the poll
            // interval passes and the stop flag is looked at again).
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for a listener this thread owns.
            let ready = unsafe { libc::poll(&mut pfd, 1, POLL.as_millis() as libc::c_int) };
            if ready <= 0 {
                continue;
            }
            let Ok((stream, _)) = self.listener.accept() else { continue };
            self.serve(stream);
        }
    }

    fn serve(&self, mut stream: UnixStream) {
        let outcome = self.answer(&mut stream);
        match outcome {
            Ok(secret) => {
                let mut reply = Zeroizing::new(Vec::with_capacity(OK.len() + secret.expose().len()));
                reply.extend_from_slice(OK);
                reply.extend_from_slice(secret.expose());
                let _ = stream.write_all(&reply);
                self.counters.answered.fetch_add(1, Ordering::SeqCst);
            }
            Err(r) => {
                self.counters.refused.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(NO);
                let _ = stream.write_all(r.word().as_bytes());
            }
        }
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    fn answer(&self, stream: &mut UnixStream) -> Result<Secret, Refusal> {
        // The request is read first (bounded, with a deadline), so a refusal
        // reaches the client instead of breaking its pipe.
        let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
        let mut buf = Zeroizing::new([0u8; MAX_REQUEST]);
        let n = read_bounded(stream, buf.as_mut()).map_err(|_| Refusal::Malformed)?;
        let (pid, uid) = peer(stream).ok_or(Refusal::NotOurs)?;
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        if uid != me {
            return Err(Refusal::NotOurs);
        }
        let text = std::str::from_utf8(&buf[..n]).map_err(|_| Refusal::Malformed)?;
        let (token, prompt) = text.split_once('\n').ok_or(Refusal::Malformed)?;
        if !same(token.as_bytes(), self.token.as_bytes()) {
            return Err(Refusal::NotOurs);
        }
        // Only a caller with the token waits for the helper to be recorded.
        let root = self.registered_root();
        if root <= 0 || !descends_from(pid, root, &proc_parent) {
            return Err(Refusal::NotOurs);
        }
        if !self.armed.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) || self.counters.gave_up.load(Ordering::SeqCst) {
            return Err(Refusal::NotWaiting);
        }
        let prompt = clean_prompt(prompt);
        let attempt = self.counters.asked.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt > MAX_ASKS {
            self.give_up();
            return Err(Refusal::NotWaiting);
        }
        (self.on_ask)(true);
        let pending = self.prompter.ask(AskRequest { prompt, helper: self.helper, attempt });
        let result = self.wait(&pending, stream);
        (self.on_ask)(false);
        if result.is_err() {
            (pending.close)();
        }
        if matches!(result, Err(Refusal::Cancelled | Refusal::TimedOut)) {
            self.give_up();
        }
        result
    }

    /// The helper's process id, waiting up to [`REGISTER_TIMEOUT`] while the
    /// job hasn't recorded it yet (0 if it never does or the step ends).
    fn registered_root(&self) -> i32 {
        let pid = self.root.pid.lock().unwrap_or_else(|e| e.into_inner());
        let waiting = |p: &mut i32| *p <= 0 && self.armed.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst);
        let (pid, _) = self.root.set.wait_timeout_while(pid, REGISTER_TIMEOUT, waiting).unwrap_or_else(|e| e.into_inner());
        *pid
    }

    fn give_up(&self) {
        if !self.counters.gave_up.swap(true, Ordering::SeqCst) {
            (self.on_give_up)();
        }
    }

    /// Waits for the dialog; gives up when it times out, the step ends or
    /// the client goes away (sudo was stopped).
    fn wait(&self, pending: &Pending, stream: &UnixStream) -> Result<Secret, Refusal> {
        let started = Instant::now();
        loop {
            match pending.answer.recv_timeout(POLL) {
                Ok(Some(secret)) if !secret.is_empty() && secret.expose().len() <= MAX_SECRET && !secret.expose().contains(&b'\n') => return Ok(secret),
                Ok(Some(_)) => return Err(Refusal::Malformed),
                Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.counters.cancelled.store(true, Ordering::SeqCst);
                    return Err(Refusal::Cancelled);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            if self.stop.load(Ordering::SeqCst) || !self.armed.load(Ordering::SeqCst) {
                return Err(Refusal::NotWaiting);
            }
            if started.elapsed() >= self.timeout {
                return Err(Refusal::TimedOut);
            }
            if closed(stream) {
                return Err(Refusal::NotWaiting);
            }
        }
    }
}

/// Reads until EOF or `buf` is full.
fn read_bounded(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Whether the client went away (sudo was stopped).
fn closed(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd as _;
    // The client shut its writing side after the request (that alone is
    // POLLRDHUP); POLLHUP means it is gone altogether.
    let mut pfd = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd; zero timeout.
    let ready = unsafe { libc::poll(&mut pfd, 1, 0) };
    ready > 0 && pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0
}

/// The connecting process and its user (`SO_PEERCRED`).
#[cfg(target_os = "linux")]
fn peer(stream: &UnixStream) -> Option<(i32, u32)> {
    use std::os::fd::AsRawFd as _;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: a valid socket, and a ucred-sized buffer with its length.
    let rc = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (rc == 0 && cred.pid > 0).then_some((cred.pid, cred.uid))
}

/// The AUR step exists only on Arch Linux; elsewhere no caller can be
/// checked, so every request is refused.
#[cfg(not(target_os = "linux"))]
fn peer(_: &UnixStream) -> Option<(i32, u32)> {
    None
}

/// `pid`'s parent, from `/proc/<pid>/stat` (the field after the command,
/// whose name may itself hold spaces and parentheses).
fn proc_parent(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parent_from_stat(&stat)
}

fn parent_from_stat(stat: &str) -> Option<i32> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

/// Whether `pid` is `root` or one of its descendants.
fn descends_from(mut pid: i32, root: i32, parent: &dyn Fn(i32) -> Option<i32>) -> bool {
    for _ in 0..MAX_DEPTH {
        if pid == root {
            return true;
        }
        if pid <= 1 {
            return false;
        }
        match parent(pid) {
            Some(p) => pid = p,
            None => return false,
        }
    }
    false
}

/// sudo's prompt as one bounded line without control characters.
fn clean_prompt(p: &str) -> String {
    let p = p.trim_matches(|c: char| c.is_whitespace() || c.is_control());
    let p = &p[..p.floor_char_boundary(MAX_PROMPT)];
    p.chars().filter(|c| !c.is_control()).collect()
}

// ── The client: what sudo runs ───────────────────────────────────────────

/// Asks the shell at `socket` for the password (the client side).
pub fn request(socket: &Path, token: &str, prompt: &str, timeout: Duration) -> Result<Secret, String> {
    let mut stream = UnixStream::connect(socket).map_err(|_| "no Mandara update is waiting for a password".to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT)).map_err(|e| e.to_string())?;
    let prompt = clean_prompt(prompt);
    let mut req = Zeroizing::new(Vec::with_capacity(token.len() + prompt.len() + 2));
    req.extend_from_slice(token.as_bytes());
    req.push(b'\n');
    req.extend_from_slice(prompt.as_bytes());
    req.push(b'\n');
    stream.write_all(&req).map_err(|e| e.to_string())?;
    stream.shutdown(std::net::Shutdown::Write).map_err(|e| e.to_string())?;
    let mut buf = Zeroizing::new([0u8; OK.len() + MAX_SECRET]);
    let n = read_bounded(&mut stream, buf.as_mut()).map_err(|e| if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut { "timed out".to_string() } else { e.to_string() })?;
    let reply = &buf[..n];
    match reply.strip_prefix(OK) {
        Some(secret) if !secret.is_empty() => Ok(Secret::from_bytes(secret)),
        _ => Err(format!("refused: {}", String::from_utf8_lossy(reply.strip_prefix(NO).unwrap_or(b"no answer")))),
    }
}

/// `mandara-askpass <prompt>` (sudo's askpass): prints the password and
/// exits 0, or explains on standard error and exits 1. The process exit
/// code.
pub fn client_main(prompt: &str) -> i32 {
    let (Some(socket), Ok(token)) = (std::env::var_os(ENV_SOCKET), std::env::var(ENV_TOKEN).map(Zeroizing::new)) else {
        eprintln!("mandara-askpass: no Mandara update is waiting for a password");
        return 1;
    };
    match request(Path::new(&socket), &token, prompt, CLIENT_TIMEOUT) {
        Ok(secret) => {
            let mut out = std::io::stdout().lock();
            let ok = out.write_all(secret.expose()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush()).is_ok();
            drop(secret);
            if ok { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("mandara-askpass: {e}");
            1
        }
    }
}

/// Whether this process was started as the askpass (sudo runs the link
/// named [`PROGRAM`]).
pub fn invoked_as_askpass(argv0: &std::ffi::OsStr) -> bool {
    Path::new(argv0).file_name().is_some_and(|n| n == PROGRAM)
}

/// Whether this process is a session's `sudo` wrapper: started as `sudo`
/// from the folder that holds the socket in its environment. A bare `sudo`
/// (yay starts it by name) is the one `PATH` finds first.
pub fn invoked_as_sudo(argv0: &std::ffi::OsStr) -> bool {
    is_wrapper(argv0, std::env::var_os(ENV_SOCKET).as_deref(), std::env::var_os("PATH").as_deref())
}

fn is_wrapper(argv0: &std::ffi::OsStr, socket: Option<&std::ffi::OsStr>, path: Option<&std::ffi::OsStr>) -> bool {
    let me = Path::new(argv0);
    let Some(socket) = socket else { return false };
    if me.file_name().is_none_or(|n| n != SUDO) {
        return false;
    }
    let folder = Path::new(socket).parent();
    match me.parent() {
        Some(d) if d.as_os_str() != "" => Some(d) == folder,
        // Started by name: the folder exec found it in, as a link path
        // (never canonicalized: the link's target is the `mandara` binary).
        _ => path.and_then(|p| std::env::split_paths(p).find(|d| d.as_os_str() != "" && is_executable(&d.join(SUDO)))).is_some_and(|d| Some(d.as_path()) == folder),
    }
}

/// Whether `path` is an executable file (what `execvp` takes from `PATH`).
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The real sudo's arguments: `-A` first, then the caller's, unchanged.
pub fn sudo_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    std::iter::once(std::ffi::OsString::from("-A")).chain(args).collect()
}

/// The `sudo` wrapper: replaces this process with the root-owned sudo from
/// the system folders (never one found on `PATH`) as `sudo -A args…`.
/// Returns an exit code only when that fails.
pub fn sudo_main(args: impl IntoIterator<Item = std::ffi::OsString>) -> i32 {
    use std::os::unix::process::CommandExt as _;
    let sudo = match super::helper::trusted_program(SUDO) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("sudo: {e}");
            return 1;
        }
    };
    let e = std::process::Command::new(sudo).args(sudo_args(args)).exec();
    eprintln!("sudo: {e}");
    1
}

#[cfg(test)]
mod tests;
