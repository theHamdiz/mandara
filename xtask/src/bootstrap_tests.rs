//! The download command talks only to a private fixture server in these tests.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::Duration;

use flate2::{Compression, write::GzEncoder};
use sha2::{Digest as _, Sha256};

const SCRIPT: &str = include_str!("../../install.sh");
const FIXTURE: &str = r#"
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::fs::write(std::env::var_os("BOOTSTRAP_RECORD").unwrap(), args.join("\0")).unwrap();
    std::process::exit(std::env::var("BOOTSTRAP_EXIT").unwrap().parse().unwrap());
}
"#;
/// A stalled local client must not keep a fixture thread alive indefinitely.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Yield while a fixture has no connection, so shutdown needs no wake socket.
const ACCEPT_PAUSE: Duration = Duration::from_millis(5);

fn read_request(client: &mut TcpStream) -> String {
    client.set_read_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = client.read(&mut buffer).unwrap();
        assert_ne!(n, 0, "fixture client closed before sending its request");
        request.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8(request).unwrap()
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new(files: BTreeMap<String, Vec<u8>>, mut transient: Option<(&'static str, usize)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let stop = stop.clone();
            let requests = requests.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let (mut client, _) = match listener.accept() {
                        Ok(client) => client,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { std::thread::sleep(ACCEPT_PAUSE); continue; }
                        Err(e) => panic!("fixture server: {e}"),
                    };
                    let request = read_request(&mut client);
                    let path = request.split_whitespace().nth(1).unwrap().to_string();
                    requests.lock().unwrap().push(path.clone());
                    let unavailable = transient.as_mut().is_some_and(|(failed_path, remaining)| {
                        if path != *failed_path || *remaining == 0 { return false; }
                        *remaining -= 1;
                        true
                    });
                    let (status, body) = match (unavailable, files.get(&path)) {
                        (true, _) => ("504 Gateway Timeout", b"temporary fixture failure".as_slice()),
                        (false, Some(body)) => ("200 OK", body.as_slice()),
                        (false, None) => ("404 Not Found", b"missing".as_slice()),
                    };
                    write!(client, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                    client.write_all(body).unwrap();
                }
            })
        };
        Self { url, stop, requests, thread: Some(thread) }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[derive(Clone, Copy)]
enum Fault { None, Signature, Archive, MissingMetadata, MissingChecksum, MissingInstaller, ExtraInstaller, NotExecutable }

fn archive(dir: &Path, fault: Fault) -> Vec<u8> {
    let source = dir.join("fixture.rs");
    let binary = dir.join("fixture");
    std::fs::write(&source, FIXTURE).unwrap();
    let compiled = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg(&source).arg("-o").arg(&binary).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let bytes = std::fs::read(binary).unwrap();
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(if matches!(fault, Fault::NotExecutable) { 0o644 } else { 0o755 });
    header.set_cksum();
    let name = if matches!(fault, Fault::MissingInstaller) { "mandara-0.99.0/bin/another-program" } else { "mandara-0.99.0/bin/mandara" };
    tar.append_data(&mut header, name, bytes.as_slice()).unwrap();
    if matches!(fault, Fault::ExtraInstaller) {
        tar.append_data(&mut header, "mandara-extra/bin/mandara", bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn run(fault: Fault, exit: i32) -> (std::process::Output, Option<String>, Vec<String>) {
    run_in(fault, exit, "C")
}

fn run_in(fault: Fault, exit: i32, language: &str) -> (std::process::Output, Option<String>, Vec<String>) {
    run_with_mask(fault, exit, language, "0022")
}

fn run_with_mask(fault: Fault, exit: i32, language: &str, mask: &str) -> (std::process::Output, Option<String>, Vec<String>) {
    run_with_network(fault, exit, language, mask, None)
}

fn run_with_network(fault: Fault, exit: i32, language: &str, mask: &str, transient: Option<(&'static str, usize)>) -> (std::process::Output, Option<String>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let archive = archive(dir.path(), fault);
    let hash = mandara_delta::hex(&Sha256::digest(&archive));
    let mut files = BTreeMap::new();
    let mut sums = String::new();
    for arch in ["x86_64", "aarch64"] {
        for libc in ["gnu", "musl"] {
            let name = format!("mandara-{arch}-linux-{libc}.tar.gz");
            sums.push_str(&format!("{hash}  {name}\n"));
            files.insert(format!("/{name}"), if matches!(fault, Fault::Archive) { b"damaged archive".to_vec() } else { archive.clone() });
        }
    }
    if matches!(fault, Fault::MissingChecksum) { sums = format!("{hash}  something-else.tar.gz\n"); }
    // Test keys are generated in memory and never replace the shipped trust root.
    let (private, public) = mandara_delta::signing::sign::generate().unwrap();
    let signature = mandara_delta::signing::sign::sign(&private, sums.as_bytes()).unwrap();
    if !matches!(fault, Fault::MissingMetadata) { files.insert("/SHA256SUMS".into(), sums.into_bytes()); }
    files.insert("/SHA256SUMS.sig".into(), if matches!(fault, Fault::Signature) { format!("{}\n", "00".repeat(64)).into_bytes() } else { signature.into_bytes() });
    let server = Server::new(files, transient);
    let record = dir.path().join("record");
    let downloads = dir.path().join("downloads");
    std::fs::create_dir(&downloads).unwrap();
    let trusted = mandara_delta::signing::trusted_keys().iter().map(|key| mandara_delta::hex(key)).collect::<Vec<_>>().join(" ");
    let script = SCRIPT.replace(&format!("release_keys='{trusted}'"), &format!("release_keys='{public}'"));
    assert_ne!(script, SCRIPT);
    let script_path = dir.path().join("install.sh");
    std::fs::write(&script_path, script).unwrap();
    let output = Command::new("sh").args(["-c", "umask \"$1\"; shift; exec sh \"$@\"", "fixture", mask]).arg(&script_path)
        .args(["--prefix", "/tmp/a path with 'quotes' and $signs", "--no-deps", "--no-greeter", "--compositor", "kwin"])
        .env("MANDARA_BOOTSTRAP_BASE_URL", &server.url).env("NO_PROXY", "127.0.0.1")
        .env("LC_ALL", language)
        .env("BOOTSTRAP_RECORD", &record).env("BOOTSTRAP_EXIT", exit.to_string()).env("TMPDIR", &downloads)
        .stdin(std::process::Stdio::null()).output().unwrap();
    assert_eq!(std::fs::read_dir(downloads).unwrap().count(), 0, "temporary downloads remain");
    if record.exists() {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&record).unwrap().permissions().mode() & 0o777, 0o666 & !u32::from_str_radix(mask, 8).unwrap(), "the bootstrap changed the installer's umask");
    }
    let record = std::fs::read_to_string(record).ok();
    let requests = server.requests.lock().unwrap().clone();
    (output, record, requests)
}

#[test]
fn a_signed_download_forwards_arguments_and_installer_failures() {
    for code in [0, 23] {
        let (output, record, requests) = run(Fault::None, code);
        assert_eq!(output.status.code(), Some(code), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(record.unwrap().split('\0').collect::<Vec<_>>(), ["install", "--prefix", "/tmp/a path with 'quotes' and $signs", "--no-deps", "--no-greeter", "--compositor", "kwin"]);
        assert_eq!(requests.len(), 3);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Verifying the download"));
        let completed: Vec<_> = stderr.lines().filter_map(|line| line.strip_prefix("  ✓ Downloaded: ")).collect();
        assert_eq!(completed.len(), 3, "{stderr}");
        for line in completed {
            let (_, bytes) = line.rsplit_once(" (").unwrap();
            assert!(bytes.strip_suffix(" B)").unwrap().parse::<u64>().unwrap() > 0, "{line}");
        }
    }
}

#[test]
fn untrusted_incomplete_or_damaged_downloads_never_execute() {
    for fault in [Fault::Signature, Fault::Archive, Fault::MissingMetadata, Fault::MissingChecksum, Fault::MissingInstaller, Fault::ExtraInstaller, Fault::NotExecutable] {
        let (output, record, requests) = run(fault, 0);
        assert!(!output.status.success());
        assert!(record.is_none());
        if matches!(fault, Fault::Signature | Fault::MissingMetadata | Fault::MissingChecksum) { assert!(!requests.iter().any(|p| p.ends_with(".tar.gz"))); }
    }
}

#[test]
fn recovered_signature_downloads_report_verified_before_fetching_the_archive() {
    for path in ["/SHA256SUMS", "/SHA256SUMS.sig"] {
        let (output, record, requests) = run_with_network(Fault::None, 0, "C", "0022", Some((path, 3)));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(record.is_some());
        assert_eq!(requests.iter().filter(|request| *request == path).count(), 4);
        let signature = stderr.find("✓ Release signature verified.").expect("successful verification must be visible");
        let archive = stderr.find("[2/5] Downloading Mandara").unwrap();
        assert!(signature < archive, "{stderr}");
        assert!(stderr.contains("Retrying"), "captured logs must retain retry notices: {stderr}");
        assert!(stderr.contains("✓ Archive checksum verified."), "{stderr}");
        assert!(stderr.contains("[5/5] Ready."), "{stderr}");
    }
}

#[test]
fn exhausted_signature_downloads_stop_before_requesting_an_archive() {
    for path in ["/SHA256SUMS", "/SHA256SUMS.sig"] {
        let (output, record, requests) = run_with_network(Fault::None, 0, "C", "0022", Some((path, 4)));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(record.is_none());
        assert_eq!(requests.iter().filter(|request| *request == path).count(), 4);
        assert!(!requests.iter().any(|path| path.ends_with(".tar.gz")));
        assert!(stderr.contains("Nothing was installed. Check your connection and try again."), "{stderr}");
        assert!(!stderr.contains("✓ Release signature verified."), "{stderr}");
    }
}

#[test]
fn retry_recovery_still_rejects_an_invalid_signature() {
    let (output, record, requests) = run_with_network(Fault::Signature, 0, "C", "0022", Some(("/SHA256SUMS.sig", 1)));
    assert!(!output.status.success());
    assert!(record.is_none());
    assert_eq!(requests.iter().filter(|path| *path == "/SHA256SUMS.sig").count(), 2);
    assert!(!requests.iter().any(|path| path.ends_with(".tar.gz")));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not be verified. Nothing was installed."));
}

#[test]
fn terminal_progress_shows_transfer_totals_and_the_completed_byte_count() {
    use std::os::fd::FromRawFd as _;
    use std::process::Stdio;

    let dir = tempfile::tempdir().unwrap();
    let body = b"fixture";
    let server = Server::new(BTreeMap::from([("/archive".to_string(), body.to_vec())]), None);
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty writes two distinct descriptors; default terminal
    // attributes are enough for curl's isatty-based progress meter.
    assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) }, 0);
    // SAFETY: ownership transfers once; each File closes its descriptor.
    let (mut master, slave) = unsafe { (std::fs::File::from_raw_fd(master), std::fs::File::from_raw_fd(slave)) };
    let functions = SCRIPT.strip_suffix("main \"$@\"\n").unwrap();
    let script = format!("{functions}\nrelease_base=$1\nprotocols=http\nfetch archive \"$2\" 1024 60\n");
    let destination = dir.path().join("download");
    let output = Command::new("sh").args(["-c", &script, "progress-fixture"]).arg(&server.url).arg(&destination)
        .env("LC_ALL", "C").env("NO_PROXY", "127.0.0.1")
        .stdin(Stdio::null()).stderr(slave).output().unwrap();
    let mut terminal = Vec::new();
    if let Err(error) = master.read_to_end(&mut terminal) {
        assert_eq!(error.raw_os_error(), Some(libc::EIO), "terminal read failed: {error}");
    }
    let terminal = String::from_utf8_lossy(&terminal);
    assert!(output.status.success(), "{terminal}");
    assert!(terminal.contains("% Total"), "the detailed meter must retain totals: {terminal}");
    assert!(terminal.contains("Received"), "{terminal}");
    assert!(terminal.contains("Downloaded: archive (7 B)"), "{terminal}");
    assert_eq!(std::fs::read(destination).unwrap(), body);
}

#[test]
fn the_bootstrap_trusts_the_same_public_keys_as_the_binary() {
    let trusted = mandara_delta::signing::trusted_keys().iter().map(|key| mandara_delta::hex(key)).collect::<Vec<_>>().join(" ");
    assert!(!trusted.is_empty());
    assert!(SCRIPT.contains(&format!("release_keys='{trusted}'")));
    assert!(!SCRIPT.contains("eval "));
    assert!(Command::new("sh").arg("-n").arg(crate::root().join("install.sh")).status().unwrap().success());
}

#[test]
fn bootstrap_messages_follow_the_terminal_language() {
    let (output, record, _) = run_in(Fault::None, 0, "ar_EG.UTF-8");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(record.is_some());
    assert!(String::from_utf8_lossy(&output.stderr).contains("جارٍ التحقق من التنزيل"));
}

#[test]
fn nixos_gets_flake_instructions_before_any_download() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = SCRIPT.strip_suffix("main \"$@\"\n").unwrap();
    let script = format!("{prefix}check_nixos \"$1\"\n");
    let run = || Command::new("sh").args(["-c", &script, "mandara-platform-test"])
        .arg(dir.path()).env("LC_ALL", "C").output().unwrap();
    assert!(run().status.success());
    std::fs::create_dir(dir.path().join("etc")).unwrap();
    std::fs::write(dir.path().join("etc/NIXOS"), "").unwrap();
    let output = run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("install Mandara through your flake"));
}

#[test]
fn installer_input_uses_the_terminal_or_preserves_an_unattended_pipe() {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    for terminal in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("answer");
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty initializes two distinct descriptors; null attributes
        // retain canonical input, so the fixture reads complete terminal lines.
        assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) }, 0);
        // SAFETY: each descriptor is transferred once and closed by its File.
        let (mut master, slave) = unsafe { (std::fs::File::from_raw_fd(master), std::fs::File::from_raw_fd(slave)) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            // SAFETY: fcntl changes flags on a live descriptor without taking
            // ownership. Neither PTY end should leak into the fixture command.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) }, 0);
        }
        let script = format!("{}run_installer sh -c 'IFS= read -r answer; printf %s \"$answer\" > \"$1\"' fixture \"$1\"\n", SCRIPT.strip_suffix("main \"$@\"\n").unwrap());
        let mut command = Command::new("sh");
        command.args(["-c", &script, "input-test"]).arg(&record)
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        let slave_fd = slave.as_raw_fd();
        // SAFETY: this closure calls only async-signal-safe session/ioctl
        // operations after fork. The slave remains open until spawn returns.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 { return Err(std::io::Error::last_os_error()); }
                if terminal && libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(b"pipe answer\n").unwrap();
        if terminal { master.write_all(b"terminal answer\n").unwrap(); }
        let deadline = std::time::Instant::now() + CLIENT_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() { break status; }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("installer input fixture did not finish");
            }
            std::thread::sleep(ACCEPT_PAUSE);
        };
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(record).unwrap(), if terminal { "terminal answer" } else { "pipe answer" });
    }
}

#[test]
fn documented_download_command_checks_completion_and_preserves_the_callers_directory() {
    for complete in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let caller = dir.path().join("caller");
        std::fs::create_dir(&caller).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/install.sh", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + CLIENT_TIMEOUT;
            let mut client = loop {
                match listener.accept() {
                    Ok((client, _)) => break client,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => std::thread::sleep(ACCEPT_PAUSE),
                    Err(err) => panic!("download command did not reach the fixture server: {err}"),
                }
            };
            read_request(&mut client);
            // Even a valid program prefix must not run on a short download.
            let body = b"printf '%s\\0' \"$PWD\" \"$@\" > \"$BOOTSTRAP_RECORD\"\n";
            let length = body.len() + if complete { 0 } else { 100 };
            write!(client, "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").unwrap();
            client.write_all(body).unwrap();
        });
        let guide = std::fs::read_to_string(crate::root().join("docs/install.md")).unwrap();
        let command = guide.lines().find(|line| line.starts_with("sh -c 'f=")).unwrap();
        assert!(command.contains("--proto =https"), "the initial request and redirects must stay on HTTPS");
        let command = command.replace("https://thehamdiz.github.io/mandara/install.sh", &url).replace("--proto =https", "--proto =http");
        assert!(command.contains(&url));
        let command = format!("{command} --compositor kwin --prefix 'path with spaces'");
        let record = dir.path().join("executed");
        let downloads = dir.path().join("downloads $(touch trap-ran)");
        std::fs::create_dir(&downloads).unwrap();
        let output = Command::new("sh").args(["-c", &command]).current_dir(&caller).env("TMPDIR", &downloads)
            .env("BOOTSTRAP_RECORD", &record).env("NO_PROXY", "127.0.0.1")
            .stdin(std::process::Stdio::null()).output().unwrap();
        server.join().unwrap();
        assert_eq!(output.status.success(), complete, "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::fs::read_dir(downloads).unwrap().count(), 0, "the first-stage script was not removed");
        assert!(!caller.join("trap-ran").exists(), "cleanup interpreted the file name as code");
        if complete {
            assert_eq!(std::fs::read_to_string(record).unwrap().split('\0').collect::<Vec<_>>(), [caller.to_str().unwrap(), "--compositor", "kwin", "--prefix", "path with spaces", ""]);
        } else {
            assert!(!record.exists());
        }
    }
}

#[test]
fn download_directories_prefer_the_home_cache_and_respect_explicit_tmpdir() {
    for explicit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let temporary = dir.path().join("explicit temporary");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&temporary).unwrap();
        let functions = SCRIPT.strip_suffix("main \"$@\"\n").unwrap();
        let code = format!("{functions}\ndownload_directory\nprintf '%s' \"$work\"\n");
        let mut command = Command::new("sh");
        command.args(["-c", &code]).env("HOME", &home).env_remove("XDG_CACHE_HOME").env_remove("TMPDIR");
        if explicit { command.env("TMPDIR", &temporary); }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let work = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap());
        let expected = if explicit { temporary } else { home.join(".cache") };
        assert_eq!(work.parent(), Some(expected.as_path()));
        assert!(!work.exists(), "the release work directory was not removed");
    }
}

#[test]
fn the_public_command_refuses_plain_http_and_removes_its_script() {
    let dir = tempfile::tempdir().unwrap();
    let guide = std::fs::read_to_string(crate::root().join("docs/install.md")).unwrap();
    let published = guide.lines().find(|line| line.starts_with("sh -c 'f=")).unwrap();
    let workflow = std::fs::read_to_string(crate::root().join(".github/workflows/release.yml")).unwrap();
    assert!(workflow.lines().any(|line| line.trim() == published), "release notes must use the documented verified installer");
    let command = published.replace("https://thehamdiz.github.io/mandara/install.sh", "http://127.0.0.1:9/install.sh");
    let output = Command::new("sh").args(["-c", &command]).env("TMPDIR", dir.path()).output().unwrap();
    // curl reports unsupported protocol before it tries the closed local port.
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn an_unusable_cache_falls_back_but_an_explicit_tmpdir_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, "keep").unwrap();
    let functions = SCRIPT.strip_suffix("main \"$@\"\n").unwrap();
    let code = format!("{functions}\ndownload_directory\nprintf '%s' \"$work\"\n");
    for explicit in [false, true] {
        let mut command = Command::new("sh");
        command.args(["-c", &code]).env("HOME", dir.path()).env("XDG_CACHE_HOME", &file).env_remove("TMPDIR");
        if explicit { command.env("TMPDIR", &file); }
        let output = command.output().unwrap();
        assert_eq!(output.status.success(), !explicit);
        if !explicit {
            let work = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap());
            assert_eq!(work.parent(), Some(Path::new("/tmp")));
            assert!(!work.exists());
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
    }
}

#[test]
fn verified_installers_receive_the_callers_permissions_mask() {
    for mask in ["0022", "0027", "0077"] {
        let (output, record, _) = run_with_mask(Fault::None, 0, "C", mask);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(record.is_some());
    }
}
