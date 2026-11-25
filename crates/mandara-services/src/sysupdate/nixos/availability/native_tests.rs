//! The disposable NixOS VM checks actual evaluator traffic, including the daemon-backed store.

use super::*;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Keep the loopback fixture's request headers bounded independently of the evaluator.
const MAX_HEADER_BYTES: usize = 8 * 1024;

struct Server {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let count = requests.clone();
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                count.fetch_add(1, Ordering::AcqRel);
                stream.set_read_timeout(Some(CHECK_TIMEOUT)).unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read_exact(&mut byte).is_err() {
                        break;
                    }
                    header.push(byte[0]);
                    assert!(header.len() <= MAX_HEADER_BYTES);
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        Self {
            address,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        let result = self.thread.take().unwrap().join();
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
#[ignore = "requires the disposable NixOS VM marker, root and the real Nix evaluator"]
fn native_nix_inspection_never_fetches_missing_inputs_and_still_accepts_cached_configs() {
    assert_eq!(
        std::fs::read_to_string("/mandara-native-nix-fixture").unwrap(),
        "mandara-disposable-nixos\n"
    );
    assert!(rustix::process::geteuid().is_root());
    let root = tempfile::tempdir().unwrap();
    let server = Server::new();
    let nix = super::super::super::helper::trusted_program("nix").unwrap();
    let write = |suffix: &str| {
        let url = format!("http://{}/{suffix}.tar.gz", server.address);
        std::fs::write(root.path().join("flake.nix"), format!(
            "{{ inputs.remote = {{ url = {url:?}; flake = false; }}; outputs = {{ self, remote }}: {{ nixosConfigurations = builtins.seq remote.outPath {{ fixture = {{}}; }}; }}; }}"
        )).unwrap();
        std::fs::write(root.path().join("flake.lock"), serde_json::to_vec(&serde_json::json!({
            "version":7,"root":"root","nodes":{"root":{"inputs":{"remote":"remote"}},"remote":{"flake":false,
            "original":{"type":"tarball","url":url},"locked":{"type":"tarball","url":url,"lastModified":0,"narHash":"sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}}
        })).unwrap()).unwrap();
    };
    write("control");
    let mut command = std::process::Command::new(&nix);
    command
        .args(super::super::NIX_FEATURES)
        .args([
            "eval",
            "--json",
            "--read-only",
            "--offline",
            "--no-write-lock-file",
            "--apply",
            "builtins.attrNames",
        ])
        .arg(format!("{}#nixosConfigurations", root.path().display()));
    assert!(
        crate::app_themes::process::native(command, &AtomicBool::new(false), CHECK_TIMEOUT)
            .is_err()
    );
    assert!(
        server.requests.load(Ordering::Acquire) > 0,
        "the control must actually reach the fixture server"
    );
    server.requests.store(0, Ordering::Release);
    write("isolated");
    assert!(
        Native
            .flake_has_host(root.path(), "fixture", &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(server.requests.load(Ordering::Acquire), 0);
    std::fs::remove_file(root.path().join("flake.lock")).unwrap();
    std::fs::write(
        root.path().join("flake.nix"),
        "{ outputs = { self }: { nixosConfigurations.fixture = {}; }; }",
    )
    .unwrap();
    assert_eq!(
        Native.flake_has_host(root.path(), "fixture", &AtomicBool::new(false)),
        Ok(true)
    );
}
