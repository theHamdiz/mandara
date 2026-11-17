//! The plugin platform on real components: features are negotiated, every
//! broker request is checked against the live grants, sensitive requests
//! need a user gesture, and views, queues, launcher results and launcher
//! time are bounded. The extension fixture is one component whose `mode`
//! setting picks its behaviour (tests/data/extension/README.md).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mandara_plugins::extension::{self, Access, Capability};
use mandara_plugins::wasm::{EventValue, ImageEntry, ImageOrigin, Kind, Palette, PluginEvent, RenderContext, SurfaceKind, WasmPlugin};

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn palette() -> Palette {
    Palette {
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

fn context() -> RenderContext {
    RenderContext { surface: SurfaceKind::Panel, outlet: String::new(), instance: String::new(), width: 360.0, height: 380.0, numerals: String::new(), calendar: String::new(), language: String::new() }
}

/// A scratch plugin folder holding `wasm` (and an `assets/moon.png`).
fn folder(name: &str, wasm: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mandara-ext-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    std::fs::write(dir.join("assets/moon.png"), b"not really a png").unwrap();
    std::fs::write(dir.join("plugin.toml"), format!("id = \"{name}\"\nname = \"{name}\"\n")).unwrap();
    dir
}

fn settings(mode: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter([("mode".to_string(), serde_json::Value::String(mode.into()))])
}

/// Loads the extension fixture in `mode` with `access`, off the test's
/// thread like the shell's lazy load would on the UI thread.
fn extension(mode: &str, access: Access) -> anyhow::Result<WasmPlugin> {
    // Tests run in parallel: each load gets its own folder.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = folder(&format!("fixture-{mode}-{n}"), &data().join("extension/fixture.wasm"));
    WasmPlugin::load(&format!("fixture-{mode}"), &dir, &dir.join("plugin.wasm"), settings(mode), access)
}

/// The fixture's log: the first text node of its panel view.
fn log(p: &mut WasmPlugin) -> String {
    let view = p.render(&palette(), &context()).unwrap().expect("a panel view");
    view.nodes.iter().find(|n| n.kind == Kind::Text).map(|n| n.text.clone()).unwrap_or_default()
}

fn click(p: &mut WasmPlugin, action: &str) {
    p.on_event(&PluginEvent { context: context(), node: action.into(), action: action.into(), value: EventValue::Activate });
}

fn use_scratch_cache() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let cache = std::env::temp_dir().join(format!("mandara-ext-cache-{}", std::process::id()));
        // SAFETY: set once, before any test of this binary compiles a
        // component; nothing else here reads the environment.
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", &cache);
            std::env::set_var("XDG_DATA_HOME", cache.join("data"));
        }
    });
}

#[test]
fn permission_generations_cancel_old_work_and_handles_after_coalesced_regrant() {
    use_scratch_cache();
    let capabilities = ["media.read".into(), "net:example.com".into()];
    let mut access = Access::new(&capabilities, &capabilities);
    access.generation = 1;
    let mut plugin = extension("ok", access.clone()).unwrap();
    click(&mut plugin, "media");
    let old = plugin.take_requests().pop().unwrap().id;
    let asset = plugin.issue_image(ImageEntry { path: "/asset.png".into(), origin: ImageOrigin::Asset }).unwrap();
    let cover = plugin.issue_image(ImageEntry { path: "/cover.png".into(), origin: ImageOrigin::MediaArt }).unwrap();
    let thumbnail = plugin.issue_image(ImageEntry { path: "/thumbnail.png".into(), origin: ImageOrigin::Network("example.com".into()) }).unwrap();
    assert!(plugin.image(thumbnail).is_some());
    access.generation = 3;
    assert!(plugin.set_access(access));
    assert_eq!(plugin.take_cancelled(), [old]);
    assert!(!plugin.deliver(old, Ok("old response".into())));
    assert!(plugin.image(cover).is_none());
    assert!(plugin.image(thumbnail).is_none());
    assert!(plugin.image(asset).is_some());
    click(&mut plugin, "media");
    let new = plugin.take_requests().pop().unwrap().id;
    assert_ne!(new, old);
    assert!(plugin.deliver(new, Ok("current response".into())));
}

#[test]
fn asynchronous_launcher_results_appear_after_the_broker_reply() {
    use_scratch_cache();
    let mut plugin = extension("async", Access::new(&["files.folders".into()], &["files.folders".into()])).unwrap();
    assert!(plugin.launcher_query("gallery", "test", 10).is_empty());
    let request = plugin.take_requests().pop().unwrap();
    assert_eq!(request.operation, "folders");
    assert!(plugin.deliver(request.id, Ok("[]".into())));
    plugin.flush_replies(&context());
    let hits = plugin.launcher_query("gallery", "test", 10);
    assert_eq!(hits.len(), 3);
    assert!(hits[0].title.contains("test result 0"));
    assert!(plugin.take_requests().is_empty());
}

#[test]
fn file_reads_need_the_new_permission_and_binary_requests_use_the_existing_host_scope() {
    use_scratch_cache();
    let declared = ["files.folders".into(), "files.read".into(), "net:api.example.com".into()];
    let mut plugin = extension("ok", Access::new(&declared, &["files.folders".into(), "net:api.example.com".into()])).unwrap();
    let request = |plugin: &mut WasmPlugin, capability: &str, operation: &str| {
        plugin.on_event(&PluginEvent { context: context(), node: String::new(), action: "request".into(), value: EventValue::Command(vec!["request".into(), capability.into(), operation.into(), "{}".into()]) });
    };
    request(&mut plugin, "files.read", "entries");
    assert!(plugin.take_requests().is_empty());
    assert!(log(&mut plugin).contains("hasn't been granted"));
    plugin.set_access(Access::new(&declared, &declared));
    request(&mut plugin, "files.read", "entries");
    assert_eq!(plugin.take_requests()[0].capability, Capability::FilesRead);
    request(&mut plugin, "net:api.example.com", "fetch-binary");
    assert_eq!(plugin.take_requests()[0].operation, "fetch-binary");
    request(&mut plugin, "net:other.example.com", "fetch-image");
    assert!(plugin.take_requests().is_empty());
}

#[test]
fn downloaded_image_handles_reach_the_guest_view_and_expire_with_the_grant() {
    use_scratch_cache();
    let permissions = ["net:example.com".into()];
    let mut plugin = extension("live", Access::new(&permissions, &permissions)).unwrap();
    plugin.on_event(&PluginEvent { context: context(), node: String::new(), action: "request".into(), value: EventValue::Command(vec!["request".into(), "net:example.com".into(), "fetch-image".into(), "{}".into()]) });
    let request = plugin.take_requests().pop().unwrap();
    let image = plugin.issue_image(ImageEntry { path: "/thumbnail.png".into(), origin: ImageOrigin::Network("example.com".into()) }).unwrap();
    assert!(plugin.deliver(request.id, Ok(serde_json::json!({"status":200,"image":image}).to_string())));
    plugin.flush_replies(&context());
    let view = plugin.render(&palette(), &context()).unwrap().unwrap();
    assert!(view.nodes.iter().any(|node| node.kind == Kind::Image && node.image == image));
    plugin.set_access(Access::default());
    assert!(plugin.image(image).is_none());
}

#[test]
fn a_plugin_negotiates_then_renders_its_surfaces() {
    use_scratch_cache();
    let mut p = extension("ok", Access::default()).expect("the fixture loads");
    assert!(p.features.iter().any(|f| f == "launcher"));
    let bar = p.render(&palette(), &RenderContext { surface: SurfaceKind::Bar, ..context() }).unwrap();
    assert!(bar.is_some_and(|v| v.nodes.iter().any(|n| n.text == "Fixture")));
    assert!(log(&mut p).contains(&format!("api {}", extension::API_LEVEL)), "negotiate got the host's API level");
    // The compliance fixture (one more component) loads the same way.
    let dir = folder("vet", &data().join("vet/fixture.wasm"));
    let mut vet = WasmPlugin::load("vet", &dir, &dir.join("plugin.wasm"), settings("ok"), Access::default()).expect("loads");
    assert!(matches!(vet.render(&palette(), &RenderContext { surface: SurfaceKind::Bar, ..context() }), Ok(Some(_))));
    assert!(matches!(vet.render(&palette(), &context()), Ok(None)), "none where it has nothing");
}

#[test]
fn a_plugin_needing_a_missing_feature_stops_with_a_clear_message() {
    use_scratch_cache();
    let err = extension("future", Access::default()).err().expect("loading must stop").to_string();
    assert!(err.contains("ui.webview") && err.contains("update Mandara"), "{err}");
}

#[test]
fn broker_requests_follow_the_live_grants() {
    use_scratch_cache();
    let declared = ["media.read".to_string(), "media.control".into()];
    let mut p = extension("ok", Access::new(&declared, &[])).unwrap();

    click(&mut p, "media");
    assert!(log(&mut p).contains("hasn't been granted"), "declared but not granted is refused at submit");
    click(&mut p, "undeclared");
    assert!(log(&mut p).contains("isn't in this plugin's permissions"));
    assert!(p.take_requests().is_empty(), "nothing refused reaches a broker");

    // The person grants media.read in Studio: it applies at once.
    assert!(p.set_access(Access::new(&declared, &["media.read".into()])));
    click(&mut p, "media");
    click(&mut p, "unknown");
    assert!(log(&mut p).contains("has no operation `erase`"));
    click(&mut p, "subscribe");
    let requests = p.take_requests();
    assert_eq!(requests.iter().map(|r| r.operation.as_str()).collect::<Vec<_>>(), ["status", "subscribe"]);
    assert!(requests.iter().all(|r| r.capability == Capability::MediaRead));

    // Answers arrive as one `replies` event; nothing waits or polls.
    assert!(p.deliver(requests[0].id, Ok("{\"title\":\"Song\"}".into())));
    assert!(p.deliver(requests[1].id, Ok("{\"playing\":true}".into())));
    assert!(p.flush_replies(&context()));
    let seen = log(&mut p);
    assert!(seen.contains(&format!("reply {} ok {{\"title\":\"Song\"}}", requests[0].id)), "{seen}");
    assert!(!p.flush_replies(&context()), "no replies left");

    // Revoking closes the open subscription and its late updates are dropped.
    assert!(p.set_access(Access::new(&declared, &[])));
    assert_eq!(p.take_cancelled(), [requests[1].id]);
    assert!(!p.deliver(requests[1].id, Ok("{}".into())));
    click(&mut p, "granted");
    assert!(log(&mut p).contains("granted []"));
}

#[test]
fn image_handles_are_plugin_scoped_and_follow_grants() {
    use_scratch_cache();
    let declared = ["files.folders".to_string()];
    let grant = |folders: Vec<PathBuf>| mandara_config::PluginGrant { generation: 0, capabilities: declared.to_vec(), folders };
    let mut p = extension("ok", Access::from_grant(&declared, Some(&grant(vec!["/pictures".into()])))).unwrap();
    click(&mut p, "asset");
    let seen = log(&mut p);
    assert!(seen.contains("asset Ok(") && seen.contains("isn't a PNG, JPEG or WebP file name"), "its own asset only: {seen}");
    let folder = p.issue_image(ImageEntry { path: "/pictures/a.png".into(), origin: ImageOrigin::Folder("/pictures".into()) }).unwrap();
    assert!(p.image(folder).is_some());
    assert!(p.image(folder + 1000).is_none(), "an unknown handle resolves to nothing");
    p.set_access(Access::from_grant(&declared, Some(&grant(vec!["/elsewhere".into()]))));
    assert!(p.image(folder).is_none(), "a folder taken out of the grant stops drawing");
    p.set_access(Access::new(&declared, &[]));
    assert!(p.image(folder).is_none(), "a revoked capability's images can't be drawn");
}

#[test]
fn views_queues_and_launcher_results_are_bounded() {
    use_scratch_cache();
    let mut huge = extension("huge", Access::default()).unwrap();
    let refused = huge.render(&palette(), &context()).unwrap_err();
    assert!(refused.contains("5000 nodes"), "{refused}");
    assert!(huge.disabled.is_none(), "a refused view doesn't disable the plugin");

    let mut p = extension("ok", Access::new(&["media.control".into()], &["media.control".into()])).unwrap();
    click(&mut p, "flood");
    assert!(log(&mut p).contains(&format!("flood {}", extension::MAX_PENDING_REQUESTS)), "only the queue limit is accepted");

    let hits = p.launcher_query("moon", "full", 1000);
    assert_eq!(hits.len(), extension::MAX_HITS);
    assert!(hits.iter().all(|h| h.title.len() <= 256));

    let answer = p.on_call("caller-plugin", "ping", "{\"n\":1}").unwrap();
    assert!(answer.contains("caller-plugin"));
    assert!(p.on_call("caller-plugin", "nope", "{}").is_err());
}

#[test]
fn a_slow_launcher_query_is_stopped_within_its_budget() {
    use_scratch_cache();
    let mut p = extension("spin", Access::default()).unwrap();
    let started = Instant::now();
    assert!(p.launcher_query("moon", "x", 5).is_empty());
    let took = started.elapsed();
    assert!(took < extension::LAUNCHER_BUDGET + Duration::from_millis(100), "a spinning query held the launcher for {took:?}");
    assert!(p.disabled.as_deref().is_some_and(|r| r.contains("launcher-query")));
}

#[test]
fn sensitive_requests_need_a_user_gesture() {
    use_scratch_cache();
    let clip = ["clipboard.read".to_string()];
    let mut sneaky = extension("sneaky", Access::new(&clip, &clip)).unwrap();
    let seen = log(&mut sneaky);
    assert!(seen.contains("render-clip Err(") && seen.contains("click or key press"), "{seen}");
    click(&mut sneaky, "clip");
    assert!(log(&mut sneaky).contains("clip Ok("), "a click is a user gesture");
    assert_eq!(sneaky.take_requests().len(), 1, "only the gesture's request was queued");
}

#[test]
fn private_storage_persists_through_the_host_and_is_bounded() {
    use_scratch_cache();
    let mut p = extension("ok", Access::default()).unwrap();
    click(&mut p, "store");
    assert!(log(&mut p).contains("store Ok(()) Err("), "an empty key is refused");
    let (path, json) = p.take_storage_flush().expect("a change is flushed");
    assert!(path.ends_with(format!("plugin-storage/{}.json", p.id)) && json.contains("salam"));
    assert!(p.take_storage_flush().is_none(), "flushed once");
    mandara_plugins::wasm::write_storage(&path, &json).unwrap();
    // A fresh instance of the same plugin reads it back.
    let dir = p.dir().to_path_buf();
    let mut again = WasmPlugin::load(&p.id, &dir, &dir.join("plugin.wasm"), settings("ok"), Access::default()).unwrap();
    click(&mut again, "recall");
    assert!(log(&mut again).contains("recall Some(\"salam\") [\"greeting\"]"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn commands_reach_the_plugin_as_a_gesture() {
    use_scratch_cache();
    let mut p = extension("ok", Access::default()).unwrap();
    p.on_event(&PluginEvent { context: context(), node: String::new(), action: "remind".into(), value: EventValue::Command(vec!["remind".into(), "tonight".into()]) });
    assert!(log(&mut p).contains("command remind tonight"));
}

#[test]
fn project_handles_follow_the_folder_grant_and_are_never_pictures() {
    use_scratch_cache();
    let declared = ["files.folders".to_string(), "wallpaper.live".into()];
    let grant = |caps: &[&str], folders: Vec<PathBuf>| mandara_config::PluginGrant { generation: 0, capabilities: caps.iter().map(|c| c.to_string()).collect(), folders };
    let both = ["files.folders", "wallpaper.live"];
    let mut p = extension("ok", Access::from_grant(&declared, Some(&grant(&both, vec!["/steam/431960".into()])))).unwrap();
    let project = p.issue_image(ImageEntry { path: "/steam/431960/123".into(), origin: ImageOrigin::Project("/steam/431960".into()) }).unwrap();
    let preview = p.issue_image(ImageEntry { path: "/steam/431960/123/preview.jpg".into(), origin: ImageOrigin::Folder("/steam/431960".into()) }).unwrap();
    assert!(p.image(project).is_some_and(|e| !e.origin.is_picture()), "a project handle resolves, but never as a picture");
    assert!(p.image(preview).is_some_and(|e| e.origin.is_picture()));
    // Revoking only wallpaper.live keeps the folder's handles: discovery
    // and scenes are granted apart.
    p.set_access(Access::from_grant(&declared, Some(&grant(&["files.folders"], vec!["/steam/431960".into()]))));
    assert!(p.image(project).is_some());
    // The folder taken out of the grant: both handles die at once.
    p.set_access(Access::from_grant(&declared, Some(&grant(&both, vec!["/elsewhere".into()]))));
    assert!(p.image(project).is_none() && p.image(preview).is_none());
}

#[test]
fn live_wallpapers_are_their_own_grant_and_start_only_from_a_gesture() {
    let live = Capability::parse("wallpaper.live").unwrap();
    assert!(live.needs_grant() && live.needs_activation("render"));
    assert!(!live.needs_activation("stop") && !live.needs_activation("status"));
    assert_eq!(live.operations(), ["status", "render", "stop"], "no operation takes a program, an argument or a path");
    let folders_only = Access::new(&["files.folders".into(), "wallpaper.live".into()], &["files.folders".into()]);
    assert!(folders_only.refuse(&live, "render").unwrap().contains("granted"), "a folder grant doesn't start scenes");
    let topic = extension::EVENT_TOPICS.iter().find(|(t, _)| *t == "live-wallpaper").unwrap();
    assert_eq!(topic.1, Some(Capability::WallpaperLive), "its events need the grant too");
}
