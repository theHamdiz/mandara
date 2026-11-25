//! Language packs on demand, end to end against a local mock of GitHub
//! (`MANDARA_UPDATE_API`): a language used without its pack is downloaded
//! from the running version's own signed release, verified, installed and
//! swapped in; a tampered download, a pack built for another version, an
//! unsigned release and offline mode never install anything; and an update
//! fetches the new version's packs into the new version's folder.
//!
//! One test function: it sets process-wide environment variables.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mandara_delta::{LocalePack, MANIFEST_FILE, Manifest, SCHEMA};
use mandara_i18n::{Lang, packs, t_in};
use mandara_services::self_update::{self as su, langpacks};

type Routes = Arc<Mutex<HashMap<String, Vec<u8>>>>;

fn serve(stream: TcpStream, routes: &Routes) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_or(true, |n| n == 0) || line == "\r\n" {
            break;
        }
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = routes.lock().unwrap().get(target.split('?').next().unwrap_or_default()).cloned();
    let (status, body) = match body {
        Some(b) => ("200 OK", b),
        None => ("404 Not Found", br#"{"message":"Not Found"}"#.to_vec()),
    };
    let mut out = stream;
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = out.write_all(head.as_bytes()).and_then(|_| out.write_all(&body)).and_then(|_| out.flush());
    let _ = reader.read(&mut [0u8; 1]);
}

fn start_mock(routes: Routes) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            serve(stream, &routes);
        }
    });
    base
}

fn catalog(lang: Lang) -> String {
    std::fs::read_to_string(packs::source_catalog(lang)).unwrap()
}

/// A signed mock release `v<version>` publishing `packs` (code → the
/// `.mlang` bytes, built for whatever version the test wants).
fn publish(routes: &Routes, base: &str, private: &str, version: &str, packs: &[(Lang, Vec<u8>)]) {
    let mut manifest = Manifest { schema: SCHEMA, name: "mandara".into(), version: version.into(), ..Default::default() };
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (lang, bytes) in packs {
        let zst = mandara_delta::create::compress(bytes).unwrap();
        let file = format!("mandara-{version}-{}{}", lang.code(), mandara_delta::LOCALE_ZST_SUFFIX);
        manifest.locales.insert(lang.code().into(), LocalePack { file: file.clone(), sha256: mandara_delta::sha256_hex(&zst), size: zst.len() as u64, pack_sha256: mandara_delta::sha256_hex(bytes), pack_size: bytes.len() as u64 });
        files.push((file, zst));
    }
    let json = manifest.to_json();
    files.push((MANIFEST_FILE.into(), json.into_bytes()));
    let sums: String = files.iter().map(|(n, d)| format!("{}  {n}\n", mandara_delta::sha256_hex(d))).collect();
    let sig = mandara_delta::signing::sign::sign(private, sums.as_bytes()).unwrap();
    files.push(("SHA256SUMS".into(), sums.into_bytes()));
    files.push(("SHA256SUMS.sig".into(), sig.into_bytes()));
    let assets: Vec<serde_json::Value> = files.iter().map(|(n, d)| serde_json::json!({ "name": n, "browser_download_url": format!("{base}/v{version}/{n}"), "size": d.len() })).collect();
    let mut r = routes.lock().unwrap();
    r.insert(format!("/repos/{}/releases/tags/v{version}", su::REPO), serde_json::json!({ "tag_name": format!("v{version}"), "assets": assets }).to_string().into_bytes());
    for (n, d) in files {
        r.insert(format!("/v{version}/{n}"), d);
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn language_packs_download_verify_and_swap_in() {
    let root = tempfile::tempdir().unwrap();
    let routes: Routes = Arc::default();
    let base = start_mock(routes.clone());
    // SAFETY: set before any other thread reads them (the mock server never
    // reads the environment).
    unsafe {
        std::env::set_var("XDG_DATA_HOME", root.path().join("data"));
        std::env::set_var("XDG_DATA_DIRS", root.path().join("system"));
        std::env::set_var("XDG_CACHE_HOME", root.path().join("cache"));
        std::env::set_var("XDG_CONFIG_HOME", root.path().join("config"));
        std::env::set_var(su::API_ENV, &base);
    }
    mandara_services::offline::set(false);
    // What a release build sees: no source catalogs to fall back on.
    packs::disable_source_fallback();
    let (private, _) = mandara_delta::signing::sign::generate().unwrap();
    su::trust_keys_for_tests(vec![mandara_delta::signing::sign::public_key(&private).unwrap()]);

    // ── Downloading and verifying, for an explicit version ─────────────────
    const V: &str = "0.3.0";
    let ar = mandara_i18n::compile(Lang::Ar, V, &catalog(Lang::Ar)).unwrap();
    // Persian's pack claims another version in its header, but its hashes
    // are published correctly: only the header check can catch it.
    let fa_wrong_version = mandara_i18n::compile(Lang::Fa, "0.2.9", &catalog(Lang::Fa)).unwrap();
    publish(&routes, &base, &private, V, &[(Lang::Ar, ar.clone()), (Lang::Fa, fa_wrong_version)]);
    let published = langpacks::published(&base, V).unwrap();
    assert_eq!(published.locales.len(), 2);
    let dir = root.path().join("explicit");
    let bytes_seen = Mutex::new(0u64);
    let path = langpacks::download(&published, Lang::Ar, &dir, &|done, _| *bytes_seen.lock().unwrap() = done).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), ar, "the installed pack is the published one, byte for byte");
    assert_eq!(*bytes_seen.lock().unwrap(), published.locales["ar"].size, "progress reached the download's size");
    assert_eq!(packs::open(&path, V, Lang::Ar).unwrap().get("Settings"), Some("الإعدادات"));

    let fa = langpacks::download(&published, Lang::Fa, &dir, &|_, _| {}).unwrap_err();
    assert!(fa.contains("0.2.9"), "a pack for another version is refused: {fa}");
    assert!(!dir.join("fa.mlang").exists());
    assert!(langpacks::download(&published, Lang::De, &dir, &|_, _| {}).unwrap_err().contains(V), "a language the release doesn't publish");

    // A tampered download fails its hash and installs nothing.
    let zst_route = format!("/v{V}/{}", published.locales["ar"].file);
    let genuine = routes.lock().unwrap().get(&zst_route).cloned().unwrap();
    let mut tampered = genuine.clone();
    let mid = tampered.len() / 2;
    tampered[mid] ^= 0x40;
    routes.lock().unwrap().insert(zst_route.clone(), tampered);
    let clean = root.path().join("tampered");
    assert!(langpacks::download(&published, Lang::Ar, &clean, &|_, _| {}).unwrap_err().contains("verification"));
    assert!(!clean.join("ar.mlang").exists());
    routes.lock().unwrap().insert(zst_route, genuine);

    // An unsigned release is never used.
    let sig_route = format!("/v{V}/SHA256SUMS.sig");
    let sig = routes.lock().unwrap().remove(&sig_route).unwrap();
    assert!(langpacks::published(&base, V).is_err(), "a signature that can't be downloaded");
    let (stranger, _) = mandara_delta::signing::sign::generate().unwrap();
    let sums = routes.lock().unwrap().get(&format!("/v{V}/SHA256SUMS")).cloned().unwrap();
    routes.lock().unwrap().insert(sig_route.clone(), mandara_delta::signing::sign::sign(&stranger, &sums).unwrap().into_bytes());
    assert!(langpacks::published(&base, V).unwrap_err().contains("isn't signed"));
    routes.lock().unwrap().insert(sig_route, sig);
    // A version without a release says so.
    assert!(langpacks::published(&base, "9.9.9").unwrap_err().contains("9.9.9"));
    // Offline mode: no request at all.
    mandara_services::offline::set(true);
    assert_eq!(langpacks::published(&base, V).unwrap_err(), mandara_services::offline::blocked());
    mandara_services::offline::set(false);

    // ── A language switch in the running shell ─────────────────────────────
    // The running version's release publishes Arabic; using Arabic without
    // its pack falls back to English, downloads it in the background, and
    // swaps it in.
    let running = packs::running_version();
    publish(&routes, &base, &private, running, &[(Lang::Ar, mandara_i18n::compile(Lang::Ar, running, &catalog(Lang::Ar)).unwrap())]);
    langpacks::install_hooks();
    let nudges = langpacks::subscribe();
    assert_eq!(t_in(Lang::Ar, "Settings"), "Settings", "English until the pack is in");
    wait_for("the Arabic pack", || langpacks::state(Lang::Ar) == Some(langpacks::State::Ready));
    assert!(nudges.try_recv().is_ok(), "views were told to re-render");
    assert_eq!(t_in(Lang::Ar, "Settings"), "الإعدادات");
    assert_eq!(packs::loaded_from(Lang::Ar), Some(packs::user_dir(running).join("ar.mlang")));
    // A language the release doesn't publish fails visibly (Studio shows it
    // with Retry) and stays English.
    assert_eq!(t_in(Lang::Ja, "Settings"), "Settings");
    wait_for("the Japanese failure", || matches!(langpacks::state(Lang::Ja), Some(langpacks::State::Failed(_))));
    let Some(langpacks::State::Failed(why)) = langpacks::state(Lang::Ja) else { unreachable!() };
    assert!(why.contains(running), "{why}");
    assert!(langpacks::describe(Lang::Ja, &langpacks::State::Failed(why)).is_some());

    // ── An update to V ─────────────────────────────────────────────────────
    // Its packs land in V's folder before the restart; a pack that fails
    // (Persian, built for another version) is reported, not fatal. Old
    // folders go, the running version's stays for a rollback.
    let stale = packs::user_dir("0.0.1");
    std::fs::create_dir_all(&stale).unwrap();
    let report = langpacks::prepare_update_with(&base, V, &[Lang::Ar, Lang::Fa, Lang::En], &|_| {});
    assert_eq!(report.installed, vec![Lang::Ar]);
    assert_eq!(report.failed.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec![Lang::Fa]);
    assert_eq!(std::fs::read(packs::user_dir(V).join("ar.mlang")).unwrap(), ar);
    assert!(!stale.exists(), "older folders are pruned");
    assert!(packs::user_dir(running).join("ar.mlang").exists(), "the running version keeps its packs");
}
