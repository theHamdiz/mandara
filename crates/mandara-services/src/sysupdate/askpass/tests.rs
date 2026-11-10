//! The askpass handshake over a real socket: who is answered, who isn't,
//! timeouts, giving up, and wiping.

use super::*;
use std::sync::mpsc;

#[derive(Clone, Copy)]
enum Behaviour {
    Answer(&'static str),
    Cancel,
    Never,
}

/// A dialog that answers as told and records what it was asked.
struct TestDialog {
    behaviour: Behaviour,
    asked: Mutex<Vec<AskRequest>>,
    closed: Arc<AtomicU32>,
    /// Keeps "never" answers' senders alive (the dialog stays open).
    open: Mutex<Vec<mpsc::Sender<Option<Secret>>>>,
}

impl TestDialog {
    fn new(behaviour: Behaviour) -> Arc<TestDialog> {
        Arc::new(TestDialog { behaviour, asked: Mutex::new(Vec::new()), closed: Arc::default(), open: Mutex::new(Vec::new()) })
    }

    fn asked(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

impl Prompter for TestDialog {
    fn ask(&self, request: AskRequest) -> Pending {
        self.asked.lock().unwrap().push(request);
        let (tx, rx) = mpsc::channel();
        match self.behaviour {
            Behaviour::Answer(p) => {
                let _ = tx.send(Some(Secret::from_string(p.to_string())));
            }
            Behaviour::Cancel => {
                let _ = tx.send(None);
            }
            Behaviour::Never => self.open.lock().unwrap().push(tx),
        }
        let closed = self.closed.clone();
        Pending { answer: rx, close: Box::new(move || {
            closed.fetch_add(1, Ordering::SeqCst);
        }) }
    }
}

struct Fixture {
    _parent: tempfile::TempDir,
    session: Session,
    gave_up: Arc<AtomicU32>,
    asking: Arc<Mutex<Vec<bool>>>,
}

fn session(dialog: Arc<TestDialog>, timeout: Duration) -> Fixture {
    let parent = tempfile::tempdir().unwrap();
    let gave_up = Arc::new(AtomicU32::new(0));
    let asking = Arc::new(Mutex::new(Vec::new()));
    let (g, a) = (gave_up.clone(), asking.clone());
    let session = Session::open_with(
        parent.path(),
        Path::new("/usr/bin/true"),
        dialog,
        "paru",
        Box::new(move |on| a.lock().unwrap().push(on)),
        Box::new(move || {
            g.fetch_add(1, Ordering::SeqCst);
        }),
        timeout,
    )
    .unwrap();
    Fixture { _parent: parent, session, gave_up, asking }
}

fn ask(s: &Session) -> Result<Secret, String> {
    request(s.socket(), s.token(), "[sudo] password for you: ", Duration::from_secs(10))
}

#[test]
fn the_running_steps_helper_gets_the_password() {
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    // This test process stands in for the helper.
    f.session.set_root(std::process::id());
    let secret = ask(&f.session).unwrap();
    assert_eq!(secret.expose(), b"hunter2");
    assert_eq!(f.session.answered(), 1);
    let asked = dialog.asked.lock().unwrap().clone();
    assert_eq!(asked, [AskRequest { prompt: "[sudo] password for you:".into(), helper: "paru", attempt: 1 }]);
    assert_eq!(*f.asking.lock().unwrap(), [true, false], "the card says it waits for your password, then stops saying so");
    // A second sudo (a mistyped password) is attempt 2.
    ask(&f.session).unwrap();
    assert_eq!(dialog.asked.lock().unwrap()[1].attempt, 2);
}

#[test]
fn a_wrong_token_is_refused_without_a_dialog() {
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    f.session.set_root(std::process::id());
    let e = request(f.session.socket(), "0000", "x", Duration::from_secs(10)).err().unwrap();
    assert!(e.contains("not-ours"), "{e}");
    let wrong_len = "a".repeat(f.session.token().len());
    assert!(request(f.session.socket(), &wrong_len, "x", Duration::from_secs(10)).is_err());
    assert_eq!(dialog.asked(), 0);
    assert_eq!(f.session.refused(), 2);
}

#[test]
fn only_the_helpers_own_processes_are_answered() {
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    // A helper that is never recorded: refused once the wait runs out.
    let started = Instant::now();
    assert!(ask(&f.session).err().unwrap().contains("not-ours"));
    assert!(started.elapsed() >= REGISTER_TIMEOUT, "it waited for the helper first");
    // A helper that isn't this process or one of its parents.
    let mut other = std::process::Command::new("sleep").arg("10").spawn().unwrap();
    f.session.set_root(other.id());
    assert!(ask(&f.session).err().unwrap().contains("not-ours"), "a process outside the helper's tree, even with the token");
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(dialog.asked(), 0);
}

#[test]
fn a_request_before_the_helper_is_recorded_waits_for_it() {
    // The helper's first sudo can ask before the job stores its process id
    // (spawning returns first): that request is answered, not refused.
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    std::thread::scope(|s| {
        let asking = s.spawn(|| ask(&f.session));
        // The request is in and waiting before the helper is recorded.
        std::thread::sleep(Duration::from_millis(300));
        assert!(!asking.is_finished(), "the early request waits");
        f.session.set_root(std::process::id());
        let secret = asking.join().unwrap().unwrap();
        assert_eq!(secret.expose(), b"hunter2");
    });
    assert_eq!(f.session.answered(), 1);
    assert_eq!(f.session.refused(), 0);
    assert_eq!(dialog.asked(), 1);
}

#[test]
fn ending_the_step_stops_waiting_for_the_helper() {
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    let started = Instant::now();
    std::thread::scope(|s| {
        let asking = s.spawn(|| ask(&f.session));
        std::thread::sleep(Duration::from_millis(200));
        f.session.disarm();
        assert!(asking.join().unwrap().is_err());
    });
    assert!(started.elapsed() < REGISTER_TIMEOUT, "disarming wakes the waiting request");
    assert_eq!(dialog.asked(), 0);
}

#[test]
fn a_wrong_token_never_waits_for_the_helper() {
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    let started = Instant::now();
    assert!(request(f.session.socket(), "0000", "x", Duration::from_secs(10)).err().unwrap().contains("not-ours"));
    assert!(started.elapsed() < REGISTER_TIMEOUT);
    assert_eq!(dialog.asked(), 0);
}

#[test]
fn nothing_is_answered_once_the_step_is_over() {
    let dialog = TestDialog::new(Behaviour::Answer("hunter2"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    f.session.set_root(std::process::id());
    f.session.disarm();
    assert!(ask(&f.session).err().unwrap().contains("not-waiting"));
    assert_eq!(dialog.asked(), 0);
    let (socket, folder) = (f.session.socket().to_path_buf(), f.session.folder().to_path_buf());
    drop(f.session);
    assert!(!folder.exists(), "the folder goes with the session");
    assert!(request(&socket, "x", "x", Duration::from_secs(1)).err().unwrap().contains("no Mandara update is waiting"));
}

#[test]
fn without_a_waiting_update_the_client_refuses() {
    assert!(request(Path::new("/nonexistent/mandara-askpass/socket"), "t", "p", Duration::from_secs(1)).is_err());
    // `mandara-askpass` itself, with no session in its environment (this
    // test process has none).
    if std::env::var_os(ENV_SOCKET).is_none() {
        assert_eq!(client_main("[sudo] password: "), 1);
    }
}

#[test]
fn an_unanswered_dialog_times_out_and_stops_the_helper() {
    let dialog = TestDialog::new(Behaviour::Never);
    let f = session(dialog.clone(), Duration::from_millis(300));
    f.session.set_root(std::process::id());
    let started = Instant::now();
    let e = ask(&f.session).err().unwrap();
    assert!(e.contains("timed-out"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(dialog.closed.load(Ordering::SeqCst), 1, "the dialog was taken away");
    assert!(f.session.gave_up());
    assert_eq!(f.gave_up.load(Ordering::SeqCst), 1, "the job was told once");
    assert!(ask(&f.session).err().unwrap().contains("not-waiting"), "and no dialog comes back");
    assert_eq!(dialog.asked(), 1);
}

#[test]
fn the_client_times_out_when_the_shell_never_answers() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let listener = UnixListener::bind(&socket).unwrap();
    let held = std::thread::spawn(move || listener.accept().map(|(s, _)| {
        std::thread::sleep(Duration::from_secs(2));
        drop(s);
    }));
    let started = Instant::now();
    let e = request(&socket, "t", "p", Duration::from_millis(300)).err().unwrap();
    assert!(started.elapsed() < Duration::from_millis(1500), "{e}");
    let _ = held.join();
}

#[test]
fn cancelling_the_dialog_refuses_and_stops_asking() {
    let dialog = TestDialog::new(Behaviour::Cancel);
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    f.session.set_root(std::process::id());
    assert!(ask(&f.session).err().unwrap().contains("cancelled"));
    assert!(f.session.cancelled() && f.session.gave_up());
    assert!(ask(&f.session).is_err());
    assert_eq!(dialog.asked(), 1, "a dismissed dialog doesn't come back (yay's keep-alive would retry forever)");
}

#[test]
fn asking_is_bounded_per_step() {
    let dialog = TestDialog::new(Behaviour::Answer("wrong"));
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    f.session.set_root(std::process::id());
    for _ in 0..MAX_ASKS {
        ask(&f.session).unwrap();
    }
    assert!(ask(&f.session).is_err());
    assert!(f.session.gave_up());
    assert_eq!(dialog.asked() as u32, MAX_ASKS);
}

#[test]
fn a_client_that_goes_away_closes_its_dialog() {
    let dialog = TestDialog::new(Behaviour::Never);
    let f = session(dialog.clone(), ANSWER_TIMEOUT);
    f.session.set_root(std::process::id());
    // sudo was stopped (a cancel during the build): its askpass is gone.
    let _ = request(f.session.socket(), f.session.token(), "p", Duration::from_millis(200));
    let start = Instant::now();
    while dialog.closed.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(dialog.closed.load(Ordering::SeqCst), 1);
}

#[test]
fn the_folder_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let f = session(TestDialog::new(Behaviour::Never), ANSWER_TIMEOUT);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(f.session.folder()), 0o700);
    assert_eq!(mode(f.session.socket()), 0o600);
    assert_eq!(std::fs::read_link(f.session.folder().join(PROGRAM)).unwrap(), Path::new("/usr/bin/true"));
    assert_eq!(std::fs::read_link(f.session.folder().join("sudo")).unwrap(), Path::new("/usr/bin/true"));
    let env = f.session.env();
    assert_eq!(env[0].0, "SUDO_ASKPASS");
    assert_eq!(Path::new(env[0].1), f.session.folder().join(PROGRAM));
    assert_eq!(f.session.token().len(), TOKEN_BYTES * 2);
    assert!(f.session.path().to_string_lossy().starts_with(&*f.session.folder().to_string_lossy()), "its sudo wrapper comes first");
    // Two sessions never share a token.
    let g = session(TestDialog::new(Behaviour::Never), ANSWER_TIMEOUT);
    assert_ne!(f.session.token(), g.session.token());
}

#[test]
fn secrets_are_wiped() {
    let mut s = Secret::from_string(String::from("hunter2"));
    let cap = s.0.capacity();
    s.wipe();
    assert!(s.is_empty());
    assert_eq!(s.0.capacity(), cap, "the buffer stays, zeroed");
    // SAFETY: zeroize wrote zeros over the whole capacity, so every byte
    // of the spare capacity is initialised.
    let bytes = unsafe { std::slice::from_raw_parts(s.0.as_ptr(), cap) };
    assert!(bytes.iter().all(|&b| b == 0));
    assert_eq!(format!("{s:?}"), "Secret(…)", "never printed");
}

#[test]
fn parents_and_descendants() {
    // 30 → 20 → 10 → 1.
    let parent = |p: i32| match p {
        30 => Some(20),
        20 => Some(10),
        10 => Some(1),
        _ => None,
    };
    assert!(descends_from(30, 10, &parent));
    assert!(descends_from(10, 10, &parent));
    assert!(!descends_from(10, 30, &parent));
    assert!(!descends_from(30, 99, &parent));
    // A loop can't keep it walking.
    assert!(!descends_from(5, 99, &|p| Some(p)));
    assert_eq!(parent_from_stat("1234 (we(ird) n)ame) S 42 1234 1234 0"), Some(42));
    assert_eq!(proc_parent(std::process::id() as i32), Some(unsafe { libc::getppid() }));
}

#[test]
fn the_sudo_wrapper_adds_askpass_and_only_answers_in_its_folder() {
    let args = sudo_args(["pacman", "-U", "/tmp/x.pkg.tar.zst"].map(std::ffi::OsString::from));
    assert_eq!(args, ["-A", "pacman", "-U", "/tmp/x.pkg.tar.zst"]);
    let socket = std::ffi::OsStr::new("/run/user/1000/mandara-askpass-x/socket");
    assert!(is_wrapper(std::ffi::OsStr::new("/run/user/1000/mandara-askpass-x/sudo"), Some(socket), None));
    assert!(!is_wrapper(std::ffi::OsStr::new("/usr/bin/sudo"), Some(socket), None));
    assert!(!is_wrapper(std::ffi::OsStr::new("sudo"), Some(socket), None));
    assert!(!is_wrapper(std::ffi::OsStr::new("/run/user/1000/mandara-askpass-x/sudo"), None, None));
    assert!(invoked_as_askpass(std::ffi::OsStr::new("/run/user/1000/mandara-askpass-x/mandara-askpass")));
    assert!(!invoked_as_askpass(std::ffi::OsStr::new("/usr/bin/mandara")));
}

#[test]
fn the_sudo_wrapper_started_by_name_is_found_through_path() {
    use std::os::unix::fs::PermissionsExt as _;
    let session = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let exe = other.path().join("mandara");
    std::fs::write(&exe, b"").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The session's `sudo` is a link to the binary, as in `Session::open`.
    std::os::unix::fs::symlink(&exe, session.path().join("sudo")).unwrap();
    let socket = session.path().join("socket");
    let socket = Some(socket.as_os_str());
    let sudo = std::ffi::OsStr::new("sudo");
    let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
    // The helper's PATH puts the session folder first (an empty entry and
    // a folder without sudo are passed over).
    assert!(is_wrapper(sudo, socket, Some(&path(&[Path::new(""), other.path().join("none").as_path(), session.path(), other.path()]))));
    // Another sudo found first, or none at all, is not the wrapper.
    std::fs::copy(&exe, other.path().join("sudo")).unwrap();
    assert!(!is_wrapper(sudo, socket, Some(&path(&[other.path(), session.path()]))));
    assert!(!is_wrapper(sudo, socket, Some(&path(&[other.path().join("none").as_path()]))));
    assert!(!is_wrapper(sudo, socket, None));
    // A sudo that can't be run is skipped, as exec skips it.
    std::fs::set_permissions(other.path().join("sudo"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(is_wrapper(sudo, socket, Some(&path(&[other.path(), session.path()]))));
    // Any other name never is.
    assert!(!is_wrapper(std::ffi::OsStr::new("mandara"), socket, Some(&path(&[session.path()]))));
}

#[test]
fn prompts_are_one_bounded_line() {
    assert_eq!(clean_prompt("  [sudo] password for you: \n"), "[sudo] password for you:");
    assert_eq!(clean_prompt("a\u{1b}[31mb\u{7}"), "a[31mb");
    assert!(clean_prompt(&"é".repeat(500)).len() <= MAX_PROMPT);
    assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"abc", b"ab"));
}
