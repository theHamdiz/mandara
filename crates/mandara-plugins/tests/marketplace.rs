//! End to end, against a local mock of GitHub's REST API (pointed at with
//! `MANDARA_MARKETPLACE_API`): marketplace search → install of a real
//! example plugin's release archive → the plugin host discovers and runs
//! it → uninstall. Also covers pagination, the search cache, rate limits,
//! repositories without a release, pre-release-only repositories and
//! offline mode.
//!
//! One test function: it sets process-wide environment variables (XDG
//! folders, the API base) that every step shares.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mandara_plugins::wasm::{self, Source, WasmPlugin};
use mandara_services::marketplace::{self, Class, InstallError, SEARCH_PER_PAGE, Vetting};

const PLUGIN_REPO: &str = "mock-author/mandara-moon-phase";
const NO_RELEASE_REPO: &str = "mock-author/no-release-yet";
const PRERELEASE_REPO: &str = "mock-author/prerelease-only";
const PLUGIN_ID: &str = "moon-phase";
const TAG: &str = "v0.1.0";
const PRERELEASE_TAG: &str = "v0.2.0-beta.1";
/// Repositories the "many" query reports (more than one search page).
const MANY_TOTAL: usize = 150;
/// Seconds until the mock's rate limit resets.
const RATE_LIMIT_RESET_SECS: u64 = 42;
const ASSET_PATH: &str = "/download/moon-phase.mandara.tar.gz";

type Log = Arc<Mutex<Vec<String>>>;

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/moon-phase")
}

fn example_wasm() -> PathBuf {
    example_dir().join("target/wasm32-wasip2/release/moon_phase.wasm")
}

/// The release asset, packed the way PLUGINS.md tells authors to
/// (`tar -czf <id>.mandara.tar.gz -C dist <id>`).
fn release_archive() -> Vec<u8> {
    let dir = example_dir();
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    b.append_path_with_name(dir.join("plugin.toml"), format!("{PLUGIN_ID}/plugin.toml")).unwrap();
    b.append_path_with_name(example_wasm(), format!("{PLUGIN_ID}/plugin.wasm")).unwrap();
    b.append_path_with_name(dir.join("icon.png"), format!("{PLUGIN_ID}/icon.png")).unwrap();
    b.append_dir_all(format!("{PLUGIN_ID}/locales"), dir.join("locales")).unwrap();
    b.into_inner().unwrap().finish().unwrap()
}

fn repo_json(full_name: &str, archived: bool, fork: bool) -> serde_json::Value {
    let (owner, name) = full_name.split_once('/').unwrap();
    serde_json::json!({
        "full_name": full_name, "name": name, "description": format!("{name} for Mandara"),
        "html_url": format!("https://github.com/{full_name}"), "stargazers_count": 3,
        "topics": [marketplace::TOPIC], "owner": { "login": owner, "avatar_url": "" },
        "updated_at": "2026-10-01T00:00:00Z", "archived": archived, "fork": fork,
    })
}

fn release_json(base: &str, tag: &str, size: usize, prerelease: bool) -> serde_json::Value {
    serde_json::json!({
        "tag_name": tag, "draft": false, "prerelease": prerelease,
        "assets": [
            { "name": "checksums.txt", "browser_download_url": format!("{base}/download/checksums.txt"), "size": 10 },
            { "name": format!("{PLUGIN_ID}{}", marketplace::ASSET_SUFFIX), "browser_download_url": format!("{base}{ASSET_PATH}"), "size": size },
        ],
    })
}

fn query_param<'a>(target: &'a str, key: &str) -> Option<&'a str> {
    target.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// Answers one request: (status line, extra headers, body).
fn route(target: &str, base: &str, archive: &[u8]) -> (&'static str, Vec<String>, Vec<u8>) {
    let json = |v: serde_json::Value| ("200 OK", vec!["Content-Type: application/json".to_string()], v.to_string().into_bytes());
    let path = target.split('?').next().unwrap_or_default();
    if path == "/search/repositories" {
        let q = query_param(target, "q").unwrap_or_default();
        assert!(q.contains(&format!("topic:{}", marketplace::TOPIC)), "search without the topic: {q}");
        let page: usize = query_param(target, "page").and_then(|p| p.parse().ok()).unwrap_or(1);
        if q.contains("ratelimited") {
            let reset = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + RATE_LIMIT_RESET_SECS;
            return ("403 Forbidden", vec!["X-RateLimit-Remaining: 0".into(), format!("X-RateLimit-Reset: {reset}")], br#"{"message":"API rate limit exceeded"}"#.to_vec());
        }
        if q.contains("many") {
            let per_page = SEARCH_PER_PAGE as usize;
            let items: Vec<_> = ((page - 1) * per_page..(page * per_page).min(MANY_TOTAL)).map(|i| repo_json(&format!("mass/plugin-{i}"), false, false)).collect();
            return json(serde_json::json!({ "total_count": MANY_TOTAL, "incomplete_results": false, "items": items }));
        }
        let items = vec![repo_json(PLUGIN_REPO, false, false), repo_json(NO_RELEASE_REPO, true, true), repo_json(PRERELEASE_REPO, false, false)];
        return json(serde_json::json!({ "total_count": items.len(), "incomplete_results": false, "items": items }));
    }
    let not_found = ("404 Not Found", vec!["Content-Type: application/json".to_string()], br#"{"message":"Not Found"}"#.to_vec());
    match path {
        p if p == format!("/repos/{PLUGIN_REPO}/releases/latest") => json(release_json(base, TAG, archive.len(), false)),
        p if p == format!("/repos/{NO_RELEASE_REPO}/releases") => json(serde_json::json!([])),
        p if p == format!("/repos/{PRERELEASE_REPO}/releases") => {
            let mut draft = release_json(base, "v9.9.9", archive.len(), false);
            draft["draft"] = true.into();
            json(serde_json::json!([draft, release_json(base, PRERELEASE_TAG, archive.len(), true)]))
        }
        ASSET_PATH => ("200 OK", vec!["Content-Type: application/gzip".into()], archive.to_vec()),
        _ => not_found,
    }
}

fn serve(stream: TcpStream, base: &str, archive: &[u8], log: &Log) {
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
    log.lock().unwrap().push(target.clone());
    let (status, headers, body) = route(&target, base, archive);
    let mut out = stream;
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for h in headers {
        head.push_str(&h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let _ = out.write_all(head.as_bytes()).and_then(|_| out.write_all(&body)).and_then(|_| out.flush());
    // Drain anything left so the client sees a clean close.
    let _ = reader.read(&mut [0u8; 1]);
}

/// Starts the mock on 127.0.0.1 and returns its base URL and request log.
fn start_mock(archive: Vec<u8>) -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let (b, l) = (base.clone(), log.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            serve(stream, &b, &archive, &l);
        }
    });
    (base, log)
}

fn requests(log: &Log) -> usize {
    log.lock().unwrap().len()
}

fn palette() -> wasm::mandara::plugin::types::Palette {
    wasm::mandara::plugin::types::Palette {
        dark: true,
        primary: "#80cbc4".into(),
        on_primary: "#00201d".into(),
        surface: "#101418".into(),
        on_surface: "#e0e3e8".into(),
        on_surface_variant: "#c0c7cd".into(),
        container: "#1c2024".into(),
        error: "#ffb4ab".into(),
        success: "#7dd3a0".into(),
        warning: "#f5c26b".into(),
    }
}

#[test]
fn marketplace_search_install_load_uninstall() {
    if !example_wasm().is_file() {
        eprintln!("skipped: {} not built (cargo build --release --target wasm32-wasip2 in plugins/examples/moon-phase)", example_wasm().display());
        return;
    }
    let root = std::env::temp_dir().join(format!("mandara-marketplace-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (base, log) = start_mock(release_archive());
    // SAFETY: set before any other thread of this test binary reads them
    // (the mock server never reads the environment).
    unsafe {
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_DATA_DIRS", root.join("system"));
        std::env::set_var("LC_ALL", "C");
        std::env::set_var(marketplace::API_ENV, &base);
    }
    assert_eq!(marketplace::api_base(), base);
    mandara_services::offline::set(false);

    // Search: only the topic decides; forks, archived repositories and a
    // repository without a release are all listed.
    let found = marketplace::search("").unwrap();
    let names: Vec<&str> = found.items.iter().map(|l| l.full_name.as_str()).collect();
    assert_eq!(names, [PLUGIN_REPO, NO_RELEASE_REPO, PRERELEASE_REPO]);
    assert!(found.items[1].archived && found.items[1].fork && !found.truncated());
    assert!(log.lock().unwrap()[0].contains("fork:true"));

    // Pagination: 150 results over two pages, then the cache answers.
    let before = requests(&log);
    let many = marketplace::search("many").unwrap();
    assert_eq!((many.items.len(), many.total), (MANY_TOTAL, MANY_TOTAL as u64));
    assert_eq!(requests(&log) - before, 2, "two search pages");
    let before = requests(&log);
    assert_eq!(marketplace::search("many").unwrap(), many);
    assert_eq!(requests(&log), before, "a repeated search is served from the cache");

    // Rate limit: a clear message with the wait, not a raw HTTP error.
    let err = marketplace::search("ratelimited").unwrap_err();
    assert!(err.contains("anonymous requests") && err.contains("Try again in"), "{err}");

    // The compliance check, as Studio runs it (no built-ins reserved here).
    let check = |dir: &Path, repo: &str| mandara_plugins::vet::vet(dir, &[], Some(repo));
    let (strict, forced) = (Vetting { check: &check, force: false }, Vetting { check: &check, force: true });

    // A listed repository without a release: a clear state, no crash.
    assert_eq!(marketplace::install(NO_RELEASE_REPO, &strict, &|_| {}), Err(InstallError::NoRelease));

    // The example republished as it is fails the compliance check: it isn't
    // installed, and that release is flagged.
    let dir = marketplace::install_dir().join(PLUGIN_ID);
    match marketplace::install(PLUGIN_REPO, &strict, &|_| {}) {
        Err(InstallError::Rejected { tag, class: Class::Placeholder, .. }) => assert_eq!(tag, TAG),
        other => panic!("the unchanged example was not rejected: {other:?}"),
    }
    assert!(!dir.exists() && marketplace::installed(PLUGIN_REPO).is_none());
    assert_eq!(marketplace::flag_for(PLUGIN_REPO, TAG).map(|f| f.class), Some(Class::Placeholder));

    // Install anyway: it installs and the warning stays on that version.
    let steps = Mutex::new(Vec::new());
    let (id, tag) = marketplace::install(PLUGIN_REPO, &forced, &|s| steps.lock().unwrap().push(s.to_string())).unwrap();
    assert_eq!((id.as_str(), tag.as_str()), (PLUGIN_ID, TAG));
    assert_eq!(steps.lock().unwrap().len(), 4);
    assert!(marketplace::flag_for(PLUGIN_REPO, TAG).is_some());
    assert_eq!(dir, root.join("data/mandara/plugins").join(PLUGIN_ID));
    assert!(dir.join("plugin.wasm").is_file() && dir.join("icon.png").is_file() && dir.join("locales").is_dir());
    assert_eq!(marketplace::installed(PLUGIN_REPO).map(|(_, t)| t), Some(TAG.to_string()));
    assert!(log.lock().unwrap().iter().any(|r| r == ASSET_PATH));

    // The plugin host finds it in the installed folder and runs it.
    let discovered = wasm::discover();
    let plugin = discovered.iter().find(|d| d.id == PLUGIN_ID).expect("installed plugin discovered");
    assert_eq!((plugin.source, plugin.dir.as_path()), (Source::Installed, dir.as_path()));
    let (pid, pdir, entry) = (plugin.id.clone(), plugin.dir.clone(), plugin.entry.clone());
    let settings = plugin.settings.iter().map(|s| (s.key.clone(), s.default_json())).collect();
    let ran = std::thread::spawn(move || {
        let mut p = WasmPlugin::load(&pid, &pdir, &entry, settings, false).map_err(|e| format!("{e:#}"))?;
        let theme = palette();
        let surfaces = (p.bar(&theme).is_some(), p.panel(&theme).is_some());
        Ok::<_, String>((surfaces, p.disabled.clone()))
    })
    .join()
    .unwrap();
    assert_eq!(ran, Ok(((true, true), None)), "the installed plugin's bar and panel render");

    // Uninstall: gone from disk and from discovery.
    assert_eq!(marketplace::uninstall(PLUGIN_REPO).as_deref(), Ok(PLUGIN_ID));
    assert!(!dir.exists() && marketplace::installed(PLUGIN_REPO).is_none());
    assert!(!wasm::discover().iter().any(|d| d.id == PLUGIN_ID));

    // A repository whose only plugin release is a pre-release (and whose
    // newest release is a draft) still installs.
    assert_eq!(marketplace::install(PRERELEASE_REPO, &forced, &|_| {}).map(|(_, t)| t), Ok(PRERELEASE_TAG.to_string()));
    assert_eq!(marketplace::uninstall(PRERELEASE_REPO).as_deref(), Ok(PLUGIN_ID));

    // Offline mode: nothing reaches the network, cached or not.
    mandara_services::offline::set(true);
    let before = requests(&log);
    assert_eq!(marketplace::search(""), Err(mandara_services::offline::blocked()));
    assert_eq!(marketplace::install(PLUGIN_REPO, &strict, &|_| {}), Err(InstallError::Failed(mandara_services::offline::blocked())));
    assert_eq!(requests(&log), before, "offline mode sent a request");
    mandara_services::offline::set(false);

    assert!(log.lock().unwrap().iter().all(|r| r.starts_with('/')), "every request went to the mock");
    std::fs::remove_dir_all(&root).unwrap();
}
