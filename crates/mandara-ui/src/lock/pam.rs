//! Password checks through the system's PAM stack.
//!
//! libpam is a system library (like libc): it is loaded at runtime with
//! `dlopen("libpam.so.0")` and driven through a tiny hand-written binding of
//! the calls a screen locker needs (`pam_start`, `pam_authenticate`,
//! `pam_setcred`, `pam_end`) — no build-time headers, no extra crate, and
//! the shell still starts on a machine without PAM.
//!
//! The account stack (`pam_acct_mgmt`) is deliberately not consulted: you
//! are already logged in, and the password proves who you are, which is all
//! a lock screen needs (swaylock and hyprlock do the same). A service file
//! without `account` lines falls back to PAM's `other` service, `pam_deny`
//! on most distributions, so checking it refused every correct password.
//!
//! The service is `/etc/pam.d/mandara-lock` when installed (see
//! `packaging/pam/mandara-lock`), else `login`, which every distribution
//! ships and which runs the normal `pam_unix` (+ `pam_faillock`) auth stack.
//!
//! Authentication runs on a worker thread ([`AuthWorker`]) so a slow PAM
//! module (fingerprint, LDAP, the deliberate `pam_unix` failure delay) never
//! stalls the lock screen's frames.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use mandara_i18n::{t, tf};

pub const PAM_SUCCESS: c_int = 0;
pub const PAM_PROMPT_ECHO_OFF: c_int = 1;
pub const PAM_PROMPT_ECHO_ON: c_int = 2;
pub const PAM_ERROR_MSG: c_int = 3;
pub const PAM_TEXT_INFO: c_int = 4;
pub const PAM_BUF_ERR: c_int = 5;
pub const PAM_PERM_DENIED: c_int = 6;
pub const PAM_AUTH_ERR: c_int = 7;
pub const PAM_MAXTRIES: c_int = 11;
pub const PAM_NEW_AUTHTOK_REQD: c_int = 12;
pub const PAM_ACCT_EXPIRED: c_int = 13;
pub const PAM_CONV_ERR: c_int = 19;
pub const PAM_REFRESH_CRED: c_int = 0x0010;

/// `struct pam_message`.
#[repr(C)]
pub struct PamMessage {
    pub msg_style: c_int,
    pub msg: *const c_char,
}

/// `struct pam_response`.
#[repr(C)]
pub struct PamResponse {
    pub resp: *mut c_char,
    pub resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int;

/// `struct pam_conv`.
#[repr(C)]
pub struct PamConv {
    pub conv: Option<ConvFn>,
    pub appdata_ptr: *mut c_void,
}

/// One message from a PAM module.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Prompt<'a> {
    /// Ask for a secret (password) — input not echoed.
    Secret(&'a str),
    /// Ask for visible input (user name, OTP on some setups).
    Visible(&'a str),
    Error(&'a str),
    Info(&'a str),
}

/// Answers PAM's questions. `Ok(None)` acknowledges a message, `Err(())`
/// aborts the conversation (PAM then fails with `PAM_CONV_ERR`).
pub trait Conversation {
    fn answer(&mut self, prompt: Prompt<'_>) -> Result<Option<String>, ()>;
}

/// Overwrites a secret's bytes before it is dropped.
pub fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

/// Answers the first prompt with the password; collects module messages
/// (e.g. pam_faillock's "account locked" notice) for the UI. A second
/// prompt (OTP, new password) is not supported and fails the attempt.
pub struct PasswordConversation {
    password: Option<String>,
    /// `(text, is_error)`.
    pub messages: Vec<(String, bool)>,
}

impl PasswordConversation {
    pub fn new(password: &str) -> Self {
        Self { password: Some(password.to_string()), messages: Vec::new() }
    }
}

impl Drop for PasswordConversation {
    fn drop(&mut self) {
        if let Some(p) = self.password.as_mut() {
            wipe(p);
        }
    }
}

impl Conversation for PasswordConversation {
    fn answer(&mut self, prompt: Prompt<'_>) -> Result<Option<String>, ()> {
        match prompt {
            Prompt::Secret(_) | Prompt::Visible(_) => self.password.take().map(Some).ok_or(()),
            Prompt::Error(m) => {
                self.messages.push((m.trim().to_string(), true));
                Ok(None)
            }
            Prompt::Info(m) => {
                self.messages.push((m.trim().to_string(), false));
                Ok(None)
            }
        }
    }
}

/// The C conversation callback. `appdata` points at a `&mut dyn
/// Conversation`. Responses are allocated with `calloc` / `strdup` because
/// PAM frees them with `free`.
///
/// # Safety
/// Called by libpam with a valid message array of `num_msg` entries, a
/// writable `resp` and the `appdata` installed by [`PamAuthenticator`].
pub unsafe extern "C" fn conversation_trampoline(num_msg: c_int, msg: *mut *const PamMessage, resp: *mut *mut PamResponse, appdata: *mut c_void) -> c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata.is_null() {
        return PAM_CONV_ERR;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: see the function contract.
        let conv = unsafe { &mut **(appdata as *mut &mut dyn Conversation) };
        let n = num_msg as usize;
        let replies = unsafe { libc::calloc(n, std::mem::size_of::<PamResponse>()) } as *mut PamResponse;
        if replies.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..n {
            // Linux-PAM passes an array of pointers to messages.
            let m = unsafe { &**msg.add(i) };
            let text = if m.msg.is_null() { String::new() } else { unsafe { CStr::from_ptr(m.msg) }.to_string_lossy().into_owned() };
            let prompt = match m.msg_style {
                PAM_PROMPT_ECHO_OFF => Prompt::Secret(&text),
                PAM_PROMPT_ECHO_ON => Prompt::Visible(&text),
                PAM_ERROR_MSG => Prompt::Error(&text),
                _ => Prompt::Info(&text),
            };
            match conv.answer(prompt) {
                Ok(Some(mut answer)) => {
                    let c = CString::new(answer.as_bytes()).unwrap_or_default();
                    wipe(&mut answer);
                    let dup = unsafe { libc::strdup(c.as_ptr()) };
                    // Wipe our temporary copy too.
                    let mut bytes = c.into_bytes();
                    bytes.fill(0);
                    if dup.is_null() {
                        unsafe { free_responses(replies, i) };
                        return PAM_BUF_ERR;
                    }
                    unsafe { (*replies.add(i)).resp = dup };
                }
                Ok(None) => {}
                Err(()) => {
                    unsafe { free_responses(replies, i) };
                    return PAM_CONV_ERR;
                }
            }
        }
        unsafe { *resp = replies };
        PAM_SUCCESS
    }));
    result.unwrap_or(PAM_CONV_ERR)
}

/// Frees the first `filled` responses (wiping them) and the array.
unsafe fn free_responses(replies: *mut PamResponse, filled: usize) {
    for i in 0..filled {
        let r = unsafe { (*replies.add(i)).resp };
        if !r.is_null() {
            unsafe {
                let len = libc::strlen(r);
                std::ptr::write_bytes(r, 0, len);
                libc::free(r as *mut c_void);
            }
        }
    }
    unsafe { libc::free(replies as *mut c_void) };
}

// ─────────────────────────── libpam loading ───────────────────────────

type StartFn = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type StartConfdirFn = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *const c_char, *mut *mut c_void) -> c_int;
type HandleFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type StrErrorFn = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;

struct LibPam {
    start: StartFn,
    /// Linux-PAM 1.4+.
    start_confdir: Option<StartConfdirFn>,
    authenticate: HandleFn,
    setcred: HandleFn,
    end: HandleFn,
    strerror: StrErrorFn,
}

// SAFETY: plain function pointers into a library that is never unloaded.
unsafe impl Send for LibPam {}
unsafe impl Sync for LibPam {}

fn libpam() -> Result<&'static LibPam, String> {
    static LIB: OnceLock<Result<LibPam, String>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: dlopen/dlsym with NUL-terminated names; the handle is kept
        // for the life of the process.
        unsafe {
            let handle = libc::dlopen(c"libpam.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
            if handle.is_null() {
                return Err(t("PAM is not available on this system (libpam.so.0 not found)").to_string());
            }
            let sym = |name: &CStr| -> Result<*mut c_void, String> {
                let p = libc::dlsym(handle, name.as_ptr());
                if p.is_null() { Err(format!("libpam lacks {}", name.to_string_lossy())) } else { Ok(p) }
            };
            Ok(LibPam {
                start: std::mem::transmute::<*mut c_void, StartFn>(sym(c"pam_start")?),
                start_confdir: sym(c"pam_start_confdir").ok().map(|p| std::mem::transmute::<*mut c_void, StartConfdirFn>(p)),
                authenticate: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_authenticate")?),
                setcred: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_setcred")?),
                end: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_end")?),
                strerror: std::mem::transmute::<*mut c_void, StrErrorFn>(sym(c"pam_strerror")?),
            })
        }
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// The PAM service to authenticate against.
pub fn pam_service(exists: impl Fn(&Path) -> bool) -> &'static str {
    if exists(Path::new("/etc/pam.d/mandara-lock")) || exists(Path::new("/usr/lib/pam.d/mandara-lock")) { "mandara-lock" } else { "login" }
}

// ─────────────────────────── authenticators ───────────────────────────

/// Result of one attempt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthResult {
    pub ok: bool,
    /// Short, translated reason on failure.
    pub error: Option<String>,
    /// Messages from PAM modules, `(text, is_error)`.
    pub messages: Vec<(String, bool)>,
}

pub trait Authenticator: Send {
    fn authenticate(&mut self, user: &str, password: &str) -> AuthResult;
}

/// Real authentication through libpam.
pub struct PamAuthenticator {
    pub service: String,
    /// Read the service from this directory instead of `/etc/pam.d`
    /// (`pam_start_confdir`; tests only).
    pub confdir: Option<std::path::PathBuf>,
}

impl PamAuthenticator {
    pub fn system() -> Self {
        Self { service: pam_service(|p| p.exists()).to_string(), confdir: None }
    }
}

fn describe(code: c_int) -> String {
    match code {
        PAM_AUTH_ERR => t("Wrong password").to_string(),
        PAM_MAXTRIES => t("Too many attempts — try again later").to_string(),
        PAM_ACCT_EXPIRED => t("Your account has expired").to_string(),
        PAM_PERM_DENIED => t("Access denied").to_string(),
        PAM_CONV_ERR => t("This sign-in method needs more than a password").to_string(),
        _ => tf("Authentication failed (PAM error {code})", &[("code", &code)]),
    }
}

impl Authenticator for PamAuthenticator {
    fn authenticate(&mut self, user: &str, password: &str) -> AuthResult {
        let lib = match libpam() {
            Ok(lib) => lib,
            Err(e) => return AuthResult { ok: false, error: Some(e), messages: Vec::new() },
        };
        let (Ok(service), Ok(user_c)) = (CString::new(self.service.as_str()), CString::new(user)) else {
            return AuthResult { ok: false, error: Some(t("Invalid user name").into()), messages: Vec::new() };
        };
        let mut conv = PasswordConversation::new(password);
        let mut dyn_conv: &mut dyn Conversation = &mut conv;
        let pam_conv = PamConv { conv: Some(conversation_trampoline), appdata_ptr: &mut dyn_conv as *mut &mut dyn Conversation as *mut c_void };
        let mut handle: *mut c_void = std::ptr::null_mut();
        let confdir = match self.confdir.as_ref().map(|d| CString::new(d.as_os_str().as_encoded_bytes())) {
            Some(Ok(dir)) => Some(dir),
            Some(Err(_)) => return AuthResult { ok: false, error: Some("invalid PAM configuration directory".into()), messages: Vec::new() },
            None => None,
        };
        // SAFETY: valid C strings; `pam_conv` and `dyn_conv` outlive the handle
        // (pam_end is called below before they drop).
        let mut rc = match (&confdir, lib.start_confdir) {
            (Some(dir), Some(start_confdir)) => unsafe { start_confdir(service.as_ptr(), user_c.as_ptr(), &pam_conv, dir.as_ptr(), &mut handle) },
            (Some(_), None) => return AuthResult { ok: false, error: Some("libpam lacks pam_start_confdir".into()), messages: Vec::new() },
            (None, _) => unsafe { (lib.start)(service.as_ptr(), user_c.as_ptr(), &pam_conv, &mut handle) },
        };
        if rc != PAM_SUCCESS || handle.is_null() {
            return AuthResult { ok: false, error: Some(tf("Could not start PAM ({code})", &[("code", &rc)])), messages: Vec::new() };
        }
        rc = unsafe { (lib.authenticate)(handle, 0) };
        if rc == PAM_SUCCESS {
            // Refresh Kerberos tickets & co; failures are harmless.
            unsafe { (lib.setcred)(handle, PAM_REFRESH_CRED) };
        }
        let detail = if rc == PAM_SUCCESS {
            None
        } else {
            let s = unsafe { (lib.strerror)(handle, rc) };
            let raw = if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() };
            tracing::info!("lock: PAM ({}) refused: {raw} ({rc})", self.service);
            Some(describe(rc))
        };
        unsafe { (lib.end)(handle, rc) };
        let messages = std::mem::take(&mut conv.messages);
        AuthResult { ok: rc == PAM_SUCCESS, error: detail, messages }
    }
}

/// `mandara lock --demo`: accepts any non-empty password after a short,
/// realistic delay. Never used for a real lock.
pub struct DemoAuthenticator {
    pub delay: Duration,
}

impl Authenticator for DemoAuthenticator {
    fn authenticate(&mut self, _user: &str, password: &str) -> AuthResult {
        std::thread::sleep(self.delay);
        if password.is_empty() {
            AuthResult { ok: false, error: Some(t("Wrong password").into()), messages: Vec::new() }
        } else {
            AuthResult { ok: true, error: None, messages: Vec::new() }
        }
    }
}

// ───────────────────────────── the worker ─────────────────────────────

/// Runs attempts on a dedicated thread; results are polled from the UI loop.
pub struct AuthWorker {
    tx: Sender<(String, String)>,
    rx: Receiver<AuthResult>,
}

impl AuthWorker {
    pub fn spawn(mut auth: Box<dyn Authenticator>) -> std::io::Result<Self> {
        let (tx, jobs) = channel::<(String, String)>();
        let (results, rx) = channel();
        std::thread::Builder::new().name("mandara-lock-auth".into()).spawn(move || {
            while let Ok((user, mut password)) = jobs.recv() {
                let result = auth.authenticate(&user, &password);
                wipe(&mut password);
                if results.send(result).is_err() {
                    break;
                }
            }
        })?;
        Ok(Self { tx, rx })
    }

    /// Starts an attempt (the password is moved to the worker and wiped there).
    pub fn submit(&self, user: &str, password: String) {
        let _ = self.tx.send((user.to_string(), password));
    }

    pub fn poll(&self) -> Option<AuthResult> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Records prompts and answers from a script.
    struct Mock {
        answers: Vec<Result<Option<String>, ()>>,
        seen: Vec<String>,
    }

    impl Conversation for Mock {
        fn answer(&mut self, prompt: Prompt<'_>) -> Result<Option<String>, ()> {
            self.seen.push(format!("{prompt:?}"));
            self.answers.remove(0)
        }
    }

    fn call(conv: &mut dyn Conversation, messages: &[(c_int, &str)]) -> (c_int, Vec<Option<String>>) {
        let texts: Vec<CString> = messages.iter().map(|(_, m)| CString::new(*m).unwrap()).collect();
        let structs: Vec<PamMessage> = messages.iter().zip(&texts).map(|((style, _), text)| PamMessage { msg_style: *style, msg: text.as_ptr() }).collect();
        let mut ptrs: Vec<*const PamMessage> = structs.iter().map(|m| m as *const PamMessage).collect();
        let mut resp: *mut PamResponse = std::ptr::null_mut();
        let mut dyn_conv: &mut dyn Conversation = conv;
        let rc = unsafe {
            conversation_trampoline(messages.len() as c_int, ptrs.as_mut_ptr(), &mut resp, &mut dyn_conv as *mut &mut dyn Conversation as *mut c_void)
        };
        let mut out = Vec::new();
        if rc == PAM_SUCCESS {
            for i in 0..messages.len() {
                let r = unsafe { (*resp.add(i)).resp };
                out.push((!r.is_null()).then(|| unsafe { CStr::from_ptr(r) }.to_string_lossy().into_owned()));
            }
            unsafe { free_responses(resp, messages.len()) };
        } else {
            assert!(resp.is_null(), "no responses may leak on failure");
        }
        (rc, out)
    }

    #[test]
    fn trampoline_answers_prompts_and_acknowledges_messages() {
        let mut mock = Mock { answers: vec![Ok(None), Ok(Some("hunter2".into()))], seen: Vec::new() };
        let (rc, out) = call(&mut mock, &[(PAM_TEXT_INFO, "Touch the key"), (PAM_PROMPT_ECHO_OFF, "Password: ")]);
        assert_eq!(rc, PAM_SUCCESS);
        assert_eq!(out, vec![None, Some("hunter2".into())]);
        assert_eq!(mock.seen, vec![r#"Info("Touch the key")"#, r#"Secret("Password: ")"#]);
    }

    #[test]
    fn trampoline_aborts_cleanly() {
        let mut mock = Mock { answers: vec![Ok(Some("a".into())), Err(())], seen: Vec::new() };
        let (rc, out) = call(&mut mock, &[(PAM_PROMPT_ECHO_ON, "login:"), (PAM_PROMPT_ECHO_OFF, "Password:")]);
        assert_eq!(rc, PAM_CONV_ERR);
        assert!(out.is_empty());
        let mut none = Mock { answers: vec![], seen: Vec::new() };
        let mut ptr: *mut PamResponse = std::ptr::null_mut();
        let mut dyn_conv: &mut dyn Conversation = &mut none;
        let rc = unsafe { conversation_trampoline(0, std::ptr::null_mut(), &mut ptr, &mut dyn_conv as *mut &mut dyn Conversation as *mut c_void) };
        assert_eq!(rc, PAM_CONV_ERR);
    }

    #[test]
    fn password_conversation_answers_once_and_keeps_messages() {
        let mut conv = PasswordConversation::new("s3cret");
        let (rc, out) = call(&mut conv, &[(PAM_ERROR_MSG, "The account is locked due to 3 failed logins.\n"), (PAM_PROMPT_ECHO_OFF, "Password: ")]);
        assert_eq!(rc, PAM_SUCCESS);
        assert_eq!(out[1].as_deref(), Some("s3cret"));
        assert_eq!(conv.messages, vec![("The account is locked due to 3 failed logins.".to_string(), true)]);
        // An OTP prompt after the password is not answerable.
        let (rc, _) = call(&mut conv, &[(PAM_PROMPT_ECHO_OFF, "Verification code: ")]);
        assert_eq!(rc, PAM_CONV_ERR);
    }

    #[test]
    fn service_prefers_the_mandara_file() {
        assert_eq!(pam_service(|p| p.ends_with("mandara-lock")), "mandara-lock");
        assert_eq!(pam_service(|_| false), "login");
    }

    #[test]
    fn wipe_zeroes_the_buffer() {
        let mut s = String::from("secret");
        let ptr = s.as_ptr();
        wipe(&mut s);
        assert!(s.is_empty());
        // The allocation is still ours; its bytes must be zero now.
        assert_eq!(unsafe { std::slice::from_raw_parts(ptr, 6) }, &[0u8; 6]);
    }

    struct Scripted(Vec<bool>);
    impl Authenticator for Scripted {
        fn authenticate(&mut self, user: &str, password: &str) -> AuthResult {
            assert_eq!(user, "amira");
            let ok = self.0.remove(0);
            AuthResult { ok, error: (!ok).then(|| format!("bad {}", password.len())), messages: Vec::new() }
        }
    }

    fn wait(worker: &AuthWorker) -> AuthResult {
        let start = Instant::now();
        loop {
            if let Some(r) = worker.poll() {
                return r;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "worker timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn worker_runs_attempts_off_thread_in_order() {
        let worker = AuthWorker::spawn(Box::new(Scripted(vec![false, true]))).unwrap();
        assert!(worker.poll().is_none());
        worker.submit("amira", "wrong".into());
        assert_eq!(wait(&worker), AuthResult { ok: false, error: Some("bad 5".into()), messages: vec![] });
        worker.submit("amira", "right!".into());
        assert!(wait(&worker).ok);
    }

    #[test]
    fn demo_accepts_any_password() {
        let worker = AuthWorker::spawn(Box::new(DemoAuthenticator { delay: Duration::ZERO })).unwrap();
        worker.submit("anyone", "x".into());
        assert!(wait(&worker).ok);
    }

    /// Runs the real libpam against service files in a scratch directory
    /// (`pam_start_confdir`), never `/etc/pam.d`. `None` when this libpam
    /// predates 1.4 or lacks the stock modules.
    fn real_pam(service: &str, files: &[(&str, &str)]) -> Option<AuthResult> {
        if libpam().ok()?.start_confdir.is_none() || !["/usr/lib/security", "/usr/lib64/security", "/lib/x86_64-linux-gnu/security", "/usr/lib/x86_64-linux-gnu/security", "/usr/lib/aarch64-linux-gnu/security"].iter().any(|d| Path::new(d).join("pam_permit.so").exists()) {
            eprintln!("skipped: no libpam with pam_start_confdir and pam_permit.so");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        Some(PamAuthenticator { service: service.into(), confdir: Some(dir.path().to_path_buf()) }.authenticate("nobody", "any password"))
    }

    /// Issue #5: a service file with only `auth` lines (like the one 0.1.x
    /// packages shipped) must not fall through to `other`'s `pam_deny` and
    /// call a correct password wrong.
    #[test]
    fn an_auth_only_service_unlocks() {
        let files = [("mandara-lock", "auth required pam_permit.so\n"), ("other", "auth required pam_deny.so\naccount required pam_deny.so\n")];
        let Some(result) = real_pam("mandara-lock", &files) else { return };
        assert!(result.ok, "{result:?}");
    }

    #[test]
    fn a_refusing_stack_reports_a_wrong_password() {
        let Some(result) = real_pam("mandara-lock", &[("mandara-lock", "auth required pam_deny.so\n")]) else { return };
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("Wrong password"));
    }
}
