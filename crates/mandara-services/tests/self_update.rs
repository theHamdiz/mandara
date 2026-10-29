//! End to end, against a local mock of GitHub (pointed at with
//! `MANDARA_UPDATE_API` / `MANDARA_MARKETPLACE_API`): Mandara's self-update
//! and marketplace plugin updates — check, delta install, fallback to the
//! full download (corrupt patch, locally modified binary, release without
//! update.json), rollback, the check cache and offline mode. Everything is
//! installed into a temporary folder; nothing real is touched.
//!
//! One test function: it sets process-wide environment variables.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mandara_delta::{MANIFEST_FILE, Manifest, PLUGIN_TARGET, SCHEMA, SHELL_BINARY};
use mandara_services::marketplace::{self, Class, Finding, Vetting};
use mandara_services::self_update::{self as su, ApplyOptions, Method, install, plugins};

const OLD: &str = "0.1.0";
const NEW: &str = "0.2.0";
const PLUGIN_REPO: &str = "mock-author/mandara-demo";
const PLUGIN_ID: &str = "demo";
/// Size of the synthetic shell binary (a few MB, so the delta matters).
const BINARY_LEN: usize = 4 << 20;
const PLUGIN_WASM_LEN: usize = 300 << 10;

/// path → body; the test swaps bodies to simulate broken releases.
type Routes = Arc<Mutex<HashMap<String, Vec<u8>>>>;
type Log = Arc<Mutex<Vec<String>>>;

fn pass(_: &Path, _: &str) -> Result<(), Finding> {
    Ok(())
}
/// Plugin updates with a compliance check that always passes.
const PASS: Vetting = Vetting { check: &pass, force: false };

fn serve(stream: TcpStream, routes: &Routes, log: &Log) {
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

fn start_mock(routes: Routes) -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let l = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            serve(stream, &routes, &l);
        }
    });
    (base, log)
}

/// Deterministic pseudo-random bytes (xorshift).
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

/// Two "builds": the new one has insertions, a deletion, shifted code and
/// scattered byte edits, like a real rebuild.
fn binaries() -> (Vec<u8>, Vec<u8>) {
    let mut old = b"\x7fELF".to_vec();
    old.extend(noise(BINARY_LEN, 1));
    let mut new = old.clone();
    new.splice(10_000..10_000, noise(20_000, 2));
    new.drain(1_000_000..1_050_000);
    new.splice(2_000_000..2_000_000, noise(5_000, 3));
    for i in (3_000_000..3_100_000).step_by(997) {
        new[i] = new[i].wrapping_add(7);
    }
    (old, new)
}

fn tar_gz(files: &[(String, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(&mut h, path, *data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

fn asset(base: &str, name: &str, size: usize) -> serde_json::Value {
    serde_json::json!({ "name": name, "browser_download_url": format!("{base}/download/{name}"), "size": size })
}

fn plugin_archive(version: &str, wasm: &[u8]) -> Vec<u8> {
    let toml = format!("id = \"{PLUGIN_ID}\"\nname = \"Demo\"\nversion = \"{version}\"\n");
    tar_gz(&[(format!("{PLUGIN_ID}/plugin.toml"), toml.as_bytes()), (format!("{PLUGIN_ID}/plugin.wasm"), wasm), (format!("{PLUGIN_ID}/locales/ar.toml"), format!("v = \"{version}\"\n").as_bytes())])
}

#[test]
fn self_update_and_plugin_updates_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    let routes: Routes = Arc::default();
    let (base, log) = start_mock(routes.clone());
    // SAFETY: set before any other thread reads them (the mock server never
    // reads the environment).
    unsafe {
        std::env::set_var("XDG_DATA_HOME", root.path().join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.path().join("cache"));
        std::env::set_var("XDG_CONFIG_HOME", root.path().join("config"));
        std::env::set_var("LC_ALL", "C");
        std::env::set_var(su::API_ENV, &base);
        std::env::set_var(mandara_services::marketplace::API_ENV, &base);
    }
    mandara_services::offline::set(false);
    assert_eq!(su::api_base(), base);

    // ── The shell release ────────────────────────────────────────────────
    let target = su::running_target();
    let (old_bin, new_bin) = binaries();
    let top = format!("mandara-{NEW}-{target}");
    let tarball_name = format!("{top}.tar.gz");
    let tarball = tar_gz(&[(format!("{top}/{SHELL_BINARY}"), &new_bin), (format!("{top}/README.md"), b"readme")]);
    let (entry, patch) = mandara_delta::create::target_entry("mandara", NEW, Some(&target), &(tarball_name.clone(), tarball.clone()), SHELL_BINARY, &new_bin, Some((OLD, &old_bin))).unwrap();
    let (patch_name, patch_bytes) = patch.unwrap();
    assert!(patch_bytes.len() * 20 < tarball.len(), "patch {} vs tarball {}", patch_bytes.len(), tarball.len());
    let mut manifest = Manifest { schema: SCHEMA, name: "mandara".into(), version: NEW.into(), ..Default::default() };
    manifest.targets.insert(target.clone(), entry);
    let sums = format!("{}  {tarball_name}\n", mandara_delta::sha256_hex(&tarball));
    let release = |with_manifest: bool| {
        let mut assets = vec![asset(&base, &tarball_name, tarball.len()), asset(&base, &patch_name, patch_bytes.len()), asset(&base, "SHA256SUMS", sums.len())];
        if with_manifest {
            assets.push(asset(&base, MANIFEST_FILE, 1));
        }
        serde_json::json!({ "tag_name": format!("v{NEW}"), "html_url": "https://example.invalid/releases/v0.2.0", "assets": assets }).to_string().into_bytes()
    };
    {
        let mut r = routes.lock().unwrap();
        r.insert(format!("/repos/{}/releases/latest", su::REPO), release(true));
        r.insert(format!("/download/{MANIFEST_FILE}"), manifest.to_json().into_bytes());
        r.insert(format!("/download/{tarball_name}"), tarball.clone());
        r.insert(format!("/download/{patch_name}"), patch_bytes.clone());
        r.insert("/download/SHA256SUMS".into(), sums.clone().into_bytes());
    }

    // A temporary install, as `mandara install` lays it out.
    let exe = root.path().join("prefix/bin/mandara");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let reset = || std::fs::write(&exe, &old_bin).unwrap();
    reset();
    assert_eq!(install::classify(&exe, &|_| None), install::Install::SelfManaged(exe.clone()));

    // Check: newer, with a patch from our version.
    let info = su::check_with(&base, OLD, &target).unwrap();
    assert!(info.available && info.latest == NEW);
    assert_eq!(info.patch.as_ref().map(|p| p.download.size), Some(patch_bytes.len() as u64));
    assert_eq!(info.download_size(false), Some(patch_bytes.len() as u64));
    assert_eq!(info.download_size(true), Some(tarball.len() as u64));
    assert!(!su::check_with(&base, NEW, &target).unwrap().available);

    // Dry run: plans the patch, touches nothing.
    let plan = su::apply(&info, &exe, ApplyOptions { dry_run: true, ..Default::default() }, &|_| {}).unwrap();
    assert_eq!((plan.method, plan.dry_run), (Method::Delta, true));
    assert_eq!(std::fs::read(&exe).unwrap(), old_bin);

    // Delta: only the patch is downloaded; byte-exact result; rollback copy.
    let steps = Mutex::new(Vec::new());
    let report = su::apply(&info, &exe, ApplyOptions::default(), &|p| steps.lock().unwrap().push(p)).unwrap();
    assert_eq!(report.method, Method::Delta);
    assert_eq!(report.downloaded, patch_bytes.len() as u64);
    assert_eq!(report.full_size, tarball.len() as u64);
    assert!(report.fallback.is_none());
    assert_eq!(std::fs::read(&exe).unwrap(), new_bin);
    assert_eq!(std::fs::read(install::backup_path(&exe)).unwrap(), old_bin);
    assert!(steps.lock().unwrap().iter().any(|p| matches!(p, su::Progress::Bytes { .. })));
    install::rollback(&exe).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), old_bin);

    // A corrupt patch fails its checksum → the full download, verified.
    let mut corrupt = patch_bytes.clone();
    let mid = corrupt.len() / 2;
    corrupt[mid] ^= 0xff;
    routes.lock().unwrap().insert(format!("/download/{patch_name}"), corrupt);
    let report = su::apply(&info, &exe, ApplyOptions::default(), &|_| {}).unwrap();
    assert_eq!(report.method, Method::Full);
    assert!(report.fallback.is_some());
    assert_eq!(report.downloaded, (patch_bytes.len() + tarball.len()) as u64);
    assert_eq!(std::fs::read(&exe).unwrap(), new_bin);
    routes.lock().unwrap().insert(format!("/download/{patch_name}"), patch_bytes.clone());

    // A locally modified binary skips the patch without downloading it.
    let mut modified = old_bin.clone();
    modified[100] ^= 1;
    std::fs::write(&exe, &modified).unwrap();
    let report = su::apply(&info, &exe, ApplyOptions::default(), &|_| {}).unwrap();
    assert_eq!((report.method, report.downloaded), (Method::Full, tarball.len() as u64));
    assert!(report.fallback.as_deref().unwrap_or_default().contains(OLD));
    assert_eq!(std::fs::read(&exe).unwrap(), new_bin);

    // --full skips the patch.
    reset();
    let report = su::apply(&info, &exe, ApplyOptions { full_only: true, ..Default::default() }, &|_| {}).unwrap();
    assert_eq!((report.method, report.fallback.clone()), (Method::Full, None));
    assert_eq!(std::fs::read(&exe).unwrap(), new_bin);

    // A tampered tarball fails verification and nothing is replaced.
    reset();
    let mut bad = tarball.clone();
    let last = bad.len() - 10;
    bad[last] ^= 0xff;
    routes.lock().unwrap().insert(format!("/download/{tarball_name}"), bad);
    assert!(su::apply(&info, &exe, ApplyOptions { full_only: true, ..Default::default() }, &|_| {}).is_err());
    assert_eq!(std::fs::read(&exe).unwrap(), old_bin);
    routes.lock().unwrap().insert(format!("/download/{tarball_name}"), tarball.clone());

    // A release without update.json: tarball naming + SHA256SUMS.
    routes.lock().unwrap().insert(format!("/repos/{}/releases/latest", su::REPO), release(false));
    let bare = su::check_with(&base, OLD, &target).unwrap();
    assert!(bare.available && bare.patch.is_none() && bare.full.as_ref().is_some_and(|f| f.sha256.is_none()));
    let report = su::apply(&bare, &exe, ApplyOptions::default(), &|_| {}).unwrap();
    assert_eq!(report.method, Method::Full);
    assert_eq!(std::fs::read(&exe).unwrap(), new_bin);
    routes.lock().unwrap().insert(format!("/repos/{}/releases/latest", su::REPO), release(true));

    // Automatic checks reuse the cached answer within the interval.
    let before = log.lock().unwrap().len();
    su::check(true, Duration::from_secs(6 * 3600)).unwrap();
    let after_forced = log.lock().unwrap().len();
    assert!(after_forced > before);
    su::check(false, Duration::from_secs(6 * 3600)).unwrap();
    assert_eq!(log.lock().unwrap().len(), after_forced, "a fresh cached answer needs no request");
    assert!(su::cached().is_some());
    assert!(su::first_shell_announcement(NEW) && !su::first_shell_announcement(NEW), "each version is announced once");

    // Offline mode blocks every request.
    mandara_services::offline::set(true);
    let requests = log.lock().unwrap().len();
    assert_eq!(su::check_with(&base, OLD, &target).unwrap_err(), mandara_services::offline::blocked());
    assert!(su::check(true, Duration::from_secs(1)).is_err());
    assert!(plugins::check_all(true, Duration::from_secs(1)).is_err());
    assert_eq!(log.lock().unwrap().len(), requests);
    mandara_services::offline::set(false);

    // ── A marketplace plugin ─────────────────────────────────────────────
    let old_wasm = noise(PLUGIN_WASM_LEN, 11);
    let mut new_wasm = old_wasm.clone();
    new_wasm.splice(1000..1000, noise(4000, 12));
    let old_archive = plugin_archive(OLD, &old_wasm);
    let new_archive_name = format!("{PLUGIN_ID}.mandara.tar.gz");
    let new_archive = plugin_archive(NEW, &new_wasm);
    let (pm, extra) = mandara_delta::create::plugin_release(&(new_archive_name.clone(), new_archive.clone()), Some(&old_archive)).unwrap();
    let dir = mandara_services::marketplace::install_dir().join(PLUGIN_ID);
    let install_plugin = || {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("locales")).unwrap();
        std::fs::write(dir.join("plugin.toml"), format!("id = \"{PLUGIN_ID}\"\nname = \"Demo\"\nversion = \"{OLD}\"\n")).unwrap();
        std::fs::write(dir.join("plugin.wasm"), &old_wasm).unwrap();
        std::fs::write(dir.join(".source"), format!("{PLUGIN_REPO}\nv{OLD}\n")).unwrap();
    };
    install_plugin();
    let plugin_release_json = |with_manifest: bool| {
        let mut assets = vec![asset(&base, &new_archive_name, new_archive.len())];
        assets.extend(extra.iter().map(|(n, d)| asset(&base, n, d.len())));
        if with_manifest {
            // Its own path on the mock (the shell's update.json has the same name).
            assets.push(serde_json::json!({ "name": MANIFEST_FILE, "browser_download_url": format!("{base}/plugin/{MANIFEST_FILE}"), "size": 1 }));
        }
        serde_json::json!({ "tag_name": format!("v{NEW}"), "html_url": "https://example.invalid/demo/v0.2.0", "assets": assets }).to_string().into_bytes()
    };
    {
        let mut r = routes.lock().unwrap();
        r.insert(format!("/repos/{PLUGIN_REPO}/releases/latest"), plugin_release_json(true));
        r.insert(format!("/plugin/{MANIFEST_FILE}"), pm.to_json().into_bytes());
        r.insert(format!("/download/{new_archive_name}"), new_archive.clone());
        for (n, d) in &extra {
            r.insert(format!("/download/{n}"), d.clone());
        }
    }
    let updates = plugins::check_all(true, Duration::from_secs(3600)).unwrap();
    assert_eq!(updates.len(), 1);
    let u = &updates[0];
    assert!(u.available && u.id == PLUGIN_ID && u.latest == format!("v{NEW}"));
    let (patch_size, meta_size) = (u.patch.as_ref().unwrap().download.size, u.meta.as_ref().unwrap().size);
    assert_eq!(u.download_size(false), Some(patch_size + meta_size));
    assert!(patch_size + meta_size < new_archive.len() as u64);
    assert_eq!(plugins::cached().len(), 1);

    let check_new = |dir: &Path| {
        assert_eq!(std::fs::read(dir.join("plugin.wasm")).unwrap(), new_wasm);
        assert!(std::fs::read_to_string(dir.join("plugin.toml")).unwrap().contains(NEW));
        assert_eq!(std::fs::read_to_string(dir.join(".source")).unwrap(), format!("{PLUGIN_REPO}\nv{NEW}\n"));
    };
    // Delta: patch + meta only.
    let report = plugins::apply(u, ApplyOptions::default(), &PASS, &|_| {}).unwrap();
    assert_eq!((report.method, report.downloaded), (Method::Delta, patch_size + meta_size));
    check_new(&dir);
    assert!(plugins::can_roll_back(PLUGIN_ID));
    assert!(plugins::cached().is_empty(), "an updated plugin drops out of the cached list");
    plugins::rollback(PLUGIN_ID).unwrap();
    assert_eq!(std::fs::read(dir.join("plugin.wasm")).unwrap(), old_wasm);

    // Corrupt patch → the full archive (checked against update.json).
    let patch_file = u.patch.as_ref().unwrap().download.file.clone();
    let good = routes.lock().unwrap().get(&format!("/download/{patch_file}")).cloned().unwrap();
    let mut corrupt = good.clone();
    corrupt[good.len() / 2] ^= 0xff;
    routes.lock().unwrap().insert(format!("/download/{patch_file}"), corrupt);
    install_plugin();
    let report = plugins::apply(u, ApplyOptions::default(), &PASS, &|_| {}).unwrap();
    assert_eq!(report.method, Method::Full);
    assert!(report.fallback.is_some());
    check_new(&dir);

    // A release without update.json or SHA256SUMS: the plain archive, as
    // the marketplace installs it.
    install_plugin();
    routes.lock().unwrap().insert(format!("/repos/{PLUGIN_REPO}/releases/latest"), plugin_release_json(false));
    let bare = plugins::check_all(true, Duration::from_secs(3600)).unwrap();
    assert!(bare[0].available && bare[0].patch.is_none());
    let report = plugins::apply(&bare[0], ApplyOptions::default(), &PASS, &|_| {}).unwrap();
    assert_eq!((report.method, report.downloaded), (Method::Full, new_archive.len() as u64));
    check_new(&dir);

    // A release that fails the compliance check isn't applied: the old
    // version stays, the new tag is flagged, and forcing installs it with
    // the flag kept. A passing check clears the flag.
    install_plugin();
    let reject = |_: &Path, _: &str| Err(Finding::new(Class::Placeholder, "it shows nothing"));
    let err = plugins::apply(&bare[0], ApplyOptions::default(), &Vetting { check: &reject, force: false }, &|_| {}).unwrap_err();
    assert!(err.contains(&format!("v{NEW}")) && err.contains("it shows nothing"), "{err}");
    assert_eq!(std::fs::read(dir.join("plugin.wasm")).unwrap(), old_wasm, "a rejected update leaves the installed version");
    assert_eq!(marketplace::flag_for(PLUGIN_REPO, &format!("v{NEW}")).map(|f| f.class), Some(Class::Placeholder));
    plugins::apply(&bare[0], ApplyOptions::default(), &Vetting { check: &reject, force: true }, &|_| {}).unwrap();
    check_new(&dir);
    assert!(marketplace::flag_for(PLUGIN_REPO, &format!("v{NEW}")).is_some(), "a forced install keeps its warning");
    install_plugin();
    plugins::apply(&bare[0], ApplyOptions::default(), &PASS, &|_| {}).unwrap();
    assert!(marketplace::flags().is_empty());
    assert!(pm.targets.contains_key(PLUGIN_TARGET));
}
