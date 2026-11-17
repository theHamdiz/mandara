//! `wallpaper.live`: live wallpaper scenes that third-party plugins ask
//! for, run by a renderer the host knows.
//!
//! A plugin lists the projects in a granted folder (`files.folders`
//! `projects`, opaque handles) and asks for one to `render`. The host
//! decides everything else:
//!
//! * **Which program.** The first installed renderer from [`RENDERERS`],
//!   looked up on `PATH` when asked (off the UI thread). Never a name, path
//!   or argument a plugin sent.
//! * **Its command line.** The project folder comes from the handle and is
//!   checked again inside the granted folder; the display comes from the
//!   shell's own outputs; the frame rate is the video wallpaper's limit
//!   (Studio → Wallpaper → Fit & effects), and sound (off), mouse (off) and
//!   the fullscreen pause are host defaults.
//! * **Its process.** No shell, a minimal environment, `/` as its working
//!   folder, stdin closed,
//!   stdout discarded and the last [`MAX_STDERR`] bytes of stderr kept for
//!   the error message. A dedicated guardian leads its process group and
//!   kills its helpers when the shell’s control pipe closes, including after
//!   a crash. The controller reserves the group id until cleanup completes.
//!
//! At most one scene runs per display. The shell's wallpaper stays the
//! single source of truth: a scene is a layer over it that ends when any
//! wallpaper is applied (Studio, `mandara msg`, a plugin, a slideshow), a
//! grant is revoked, the plugin is switched off, removed or rebuilt, the
//! display goes away or the shell quits. While the desktop can't be seen
//! (the video wallpaper rules: locked, screens off, covered, on battery)
//! the renderer is stopped with `SIGSTOP` and continued after. Nothing runs
//! and nothing is observed while no scene does.

mod pending;
mod preparation;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::App;
use mandara_plugins::extension::{Access, Capability};
use serde_json::{Value, json};

use super::{deliver, project_of};
use crate::plugins::external::discovered;
use crate::renderer_process::{Group, Report, launch, MAX_STDERR};
#[cfg(test)]
use crate::renderer_process::CHILD_PATH;

/// A renderer the host may start, and the command line it builds.
pub(super) struct Renderer {
    /// The executable's file name, looked up on `PATH`.
    pub name: &'static str,
    pub argv: fn(&Job) -> Vec<OsString>,
}

/// Renderers the host knows, in order of preference.
pub(super) const RENDERERS: &[Renderer] = &[Renderer { name: "linux-wallpaperengine", argv: wallpaperengine_argv }];

/// What one scene shows where: everything here comes from the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Job {
    /// The project folder (canonical, inside the granted folder).
    pub project: PathBuf,
    /// The display's connector name, from the shell's outputs.
    pub output: String,
    /// Frames per second: the video wallpaper's limit.
    pub fps: u32,
    /// The renderer's shared assets, when the project sits in a Steam
    /// library that has them (else the renderer looks for them itself).
    pub assets: Option<PathBuf>,
}

/// How long a renderer has to exit after `SIGTERM` before `SIGKILL`.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// Largest `project.json` read for a title, type and preview.
const MAX_PROJECT_JSON: u64 = 64 * 1024;
/// Longest project title handed to a plugin (bytes).
const MAX_TITLE: usize = 256;
/// Entries looked at for a project's first image when it names no preview.
const MAX_PREVIEW_SCAN: usize = 256;
/// The Wallpaper Engine assets folder, relative to a Steam library's
/// `steamapps` folder.
const STEAM_ASSETS: &str = "common/wallpaper_engine/assets";
/// Renderer discovery stays bounded even with an unusually large PATH.
const MAX_PATH_ENTRIES: usize = 256;
/// Plugins share one renderer lookup and a bounded set of pending status replies.
const MAX_STATUS_WAITERS: usize = 32;
type StatusReply = Box<dyn FnOnce(Result<Value, String>, &mut App)>;

thread_local! {
    static STATUS: RefCell<super::queue::Batch<StatusReply, MAX_STATUS_WAITERS>> = const { RefCell::new(super::queue::Batch::new()) };
}

/// `linux-wallpaperengine`'s command line: on the background layer (below
/// Mandara's desktop widgets), silent, without the mouse, pausing for
/// fullscreen windows (its default).
fn wallpaperengine_argv(job: &Job) -> Vec<OsString> {
    let mut argv: Vec<OsString> = ["--layer", "background", "--fps"].map(OsString::from).into();
    argv.push(job.fps.to_string().into());
    argv.extend(["--silent", "--noautomute", "--no-audio-processing", "--disable-mouse"].map(OsString::from));
    if let Some(assets) = &job.assets {
        argv.push("--assets-dir".into());
        argv.push(assets.clone().into());
    }
    argv.push("--screen-root".into());
    argv.push(job.output.clone().into());
    argv.push("--bg".into());
    argv.push(job.project.clone().into());
    argv
}

/// The first installed renderer of `renderers` in `path` (a `PATH` value;
/// relative entries are skipped), with its executable.
pub(super) fn find_renderer<'a>(renderers: &'a [Renderer], path: Option<&std::ffi::OsStr>) -> Option<(&'a Renderer, PathBuf)> {
    use std::os::unix::fs::PermissionsExt as _;
    renderers.iter().find_map(|r| {
        path.into_iter().flat_map(std::env::split_paths).take(MAX_PATH_ENTRIES).filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(r.name)).find(|exe| std::fs::metadata(exe).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)).map(|exe| (r, exe))
    })
}

// ── Projects ─────────────────────────────────────────────────────────────

/// What `project.json` says about a project (all optional).
#[derive(Default, Debug, PartialEq)]
pub(super) struct ProjectInfo {
    pub title: String,
    /// `scene`, `video`, `web`, …, lowercase.
    pub kind: String,
    pub preview: Option<PathBuf>,
}

/// Whether `path` resolves (links followed) to somewhere inside `root`,
/// which must already be canonical; the resolved path when it does.
pub(super) fn contained(path: &Path, root: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().filter(|real| real.starts_with(root))
}

/// The project folders directly inside granted folder `root`: no hidden
/// ones, links pointing out skipped, canonical, sorted by name, at most
/// `max` entries looked at.
pub(super) fn projects_in(root: &Path, max: usize) -> Vec<PathBuf> {
    let Ok(real_root) = root.canonicalize() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&real_root) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .take(max)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| contained(&e.path(), &real_root))
        .filter(|real| *real != real_root && real.is_dir())
        .collect();
    sort_unique(&mut out);
    out
}

/// Sorts resolved paths by name (case-insensitive) and drops repeats: two
/// links to one file or folder list it once.
pub(super) fn sort_unique(paths: &mut Vec<PathBuf>) {
    paths.sort_by_cached_key(|p| (p.file_name().map(|n| n.to_string_lossy().to_lowercase()), p.clone()));
    paths.dedup();
}

/// Reads `project.json` in `project` (canonical): bounded, and a preview
/// only when it is an image inside the project.
pub(super) fn project_info(project: &Path) -> ProjectInfo {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        title: String,
        #[serde(default, rename = "type")]
        kind: String,
        #[serde(default)]
        preview: Option<PathBuf>,
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(project.join("project.json")).and_then(|f| f.take(MAX_PROJECT_JSON + 1).read_to_end(&mut bytes));
    let raw = match read {
        Ok(n) if n as u64 <= MAX_PROJECT_JSON => serde_json::from_slice::<Raw>(&bytes).ok(),
        _ => None,
    };
    let Some(raw) = raw else { return ProjectInfo::default() };
    let preview = raw.preview.filter(|p| p.is_relative()).and_then(|p| contained(&project.join(p), project)).filter(|p| p.is_file() && mandara_plugins::wasm::is_image(p));
    ProjectInfo { title: super::clip(raw.title.trim(), MAX_TITLE), kind: raw.kind.trim().to_ascii_lowercase(), preview }
}

/// A picture that shows project `project` (canonical): its declared
/// preview, else its first image by name.
pub(super) fn project_preview(project: &Path, info: &ProjectInfo) -> Option<PathBuf> {
    if let Some(p) = &info.preview {
        return Some(p.clone());
    }
    let entries = std::fs::read_dir(project).ok()?;
    let mut images: Vec<PathBuf> = entries.flatten().take(MAX_PREVIEW_SCAN).map(|e| e.path()).filter(|p| mandara_plugins::wasm::is_image(p) && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).filter_map(|p| contained(&p, project)).filter(|p| p.is_file()).collect();
    sort_unique(&mut images);
    images.into_iter().next()
}

/// Only scenes and videos run: web projects run scripts with network
/// access, outside what a wallpaper grant should allow.
pub(super) fn runnable(kind: &str) -> bool {
    matches!(kind, "scene" | "video")
}

/// The shared assets of the Steam library `project` sits in, if any.
fn steam_assets(project: &Path) -> Option<PathBuf> {
    let steamapps = project.ancestors().find(|a| a.file_name().is_some_and(|n| n == "steamapps"))?;
    Some(steamapps.join(STEAM_ASSETS)).filter(|p| p.is_dir())
}

/// Ends a renderer: continued (if stopped) and terminated now, killed
/// after [`STOP_GRACE`] if it is still there.
fn end(group: Group, cx: &mut App) {
    if !group.alive() {
        return;
    }
    group.signal(libc::SIGCONT);
    group.signal(libc::SIGTERM);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(STOP_GRACE).await;
        group.signal(libc::SIGKILL);
    })
    .detach();
}

// ── Scenes ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum State {
    Running,
    Paused,
    Failed(String),
}

struct Scene {
    plugin: String,
    permission_generation: u64,
    /// The granted folder the project came from (as in the config).
    root: PathBuf,
    /// A picture of it, for the desktop's text colours.
    ink: Option<PathBuf>,
    state: State,
    group: Option<Group>,
    /// Reports from its thread; dropped with the scene.
    _reports: Option<gpui::Task<()>>,
    generation: u64,
    wallpaper: WallpaperKey,
}

/// What ends scenes, observed only while one runs.
struct Watch {
    _config: gpui::Subscription,
    _outputs: gpui::Subscription,
    _quit: gpui::Subscription,
    /// The wallpaper as the scenes started over it.
    wallpaper: WallpaperKey,
}

/// The part of the config whose change means "another wallpaper".
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WallpaperKey {
    enabled: bool,
    path: Option<PathBuf>,
    variants: mandara_config::desktop::WallpaperVariants,
}

impl WallpaperKey {
    fn of(c: &mandara_config::ShellConfig) -> Self {
        Self { enabled: c.wallpaper.enabled, path: c.wallpaper.path.clone(), variants: c.wallpaper.variants.clone() }
    }

}

thread_local! {
    /// Display → its scene (running, paused or failed).
    static PENDING: RefCell<pending::Pending> = RefCell::new(pending::Pending::default());
    static SCENES: RefCell<HashMap<String, Scene>> = RefCell::new(HashMap::new());
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Displays that show a live scene now, with a picture of it for the
/// desktop's text colours (the backdrop leaves their wallpaper out).
pub(crate) fn shown() -> Vec<(String, Option<PathBuf>)> {
    SCENES.with(|s| s.borrow().iter().filter(|(_, sc)| sc.group.is_some()).map(|(o, sc)| (o.clone(), sc.ink.clone())).collect())
}

/// Whether any display shows a live scene.
pub(crate) fn any() -> bool {
    SCENES.with(|s| s.borrow().values().any(|sc| sc.group.is_some()))
}

/// Whether display `output` shows a live scene.
pub(crate) fn covers(output: &str) -> bool {
    SCENES.with(|s| s.borrow().get(output).is_some_and(|sc| sc.group.is_some()))
}

/// The backdrop worked out where the desktop can be seen: scenes elsewhere
/// are stopped (`SIGSTOP`), the others continue.
pub(crate) fn set_playing(playing: &[String]) {
    SCENES.with(|s| {
        for (output, sc) in s.borrow_mut().iter_mut() {
            let (Some(group), play) = (&sc.group, playing.contains(output)) else { continue };
            match (&sc.state, play) {
                (State::Running, false) => {
                    group.signal(libc::SIGSTOP);
                    sc.state = State::Paused;
                }
                (State::Paused, true) => {
                    group.signal(libc::SIGCONT);
                    sc.state = State::Running;
                }
                _ => {}
            }
        }
    });
}

/// The displays a scene may use: the shell's outputs that show the
/// wallpaper.
fn wallpaper_outputs(cx: &App) -> Vec<String> {
    let store = crate::state::config(cx);
    let cfg = store.read(cx).get();
    cx.try_global::<crate::overlays::Outputs>()
        .into_iter()
        .flat_map(|o| &o.0)
        .filter(|o| !o.name.is_empty() && mandara_config::outputs::assignment(&cfg.displays, mandara_config::outputs::SurfaceKind::Wallpaper, &o.name))
        .map(|o| o.name.clone())
        .collect()
}

/// `status` (and the `live-wallpaper` topic): every display, its scene and
/// the plugin that runs it.
pub(super) fn snapshot(cx: &App) -> Value {
    let outputs = wallpaper_outputs(cx);
    SCENES.with(|s| {
        let s = s.borrow();
        let rows: Vec<Value> = outputs
            .iter()
            .map(|o| match s.get(o) {
                None => json!({ "output": o, "state": "off" }),
                Some(sc) => match &sc.state {
                    State::Running => json!({ "output": o, "state": "running", "plugin": sc.plugin }),
                    State::Paused => json!({ "output": o, "state": "paused", "plugin": sc.plugin }),
                    State::Failed(why) => json!({ "output": o, "state": "failed", "plugin": sc.plugin, "error": why }),
                },
            })
            .collect();
        json!({ "outputs": rows })
    })
}

/// `status`: the snapshot and the renderer that `render` would start
/// (looked up off the UI thread).
pub(super) fn status(plugin: &str, id: u64, cx: &mut App) {
    let plugin = plugin.to_string();
    status_then(cx, move |value, cx| deliver(&plugin, id, value, cx));
}

fn status_then(cx: &mut App, finish: impl FnOnce(Result<Value, String>, &mut App) + 'static) {
    match STATUS.with(|status| status.borrow_mut().join(Box::new(finish) as StatusReply)) {
        Ok(true) => {}
        Ok(false) => return,
        Err(finish) => { finish(Err("the live wallpaper status queue is full".into()), cx); return; }
    }
    cx.spawn(async move |cx| {
        let renderer = mandara_services::runtime::unblock(|| find_renderer(RENDERERS, std::env::var_os("PATH").as_deref()).map(|(r, _)| r.name)).await;
        cx.update(|cx| {
            let mut v = snapshot(cx);
            v["renderer"] = renderer.map_or(Value::Null, |n| json!(n));
            if renderer.is_none() {
                v["problem"] = json!(not_installed());
            }
            let waiting = STATUS.with(|status| status.borrow_mut().finish());
            for finish in waiting { finish(Ok(v.clone()), cx); }
        });
    })
    .detach();
}

fn not_installed() -> String {
    let names: Vec<&str> = RENDERERS.iter().map(|r| r.name).collect();
    format!("no live wallpaper renderer is installed (install {})", names.join(" or "))
}

/// What `plugin` may do now, its granted folders included.
fn access(plugin: &str, cx: &App) -> Option<Access> {
    discovered(plugin).map(|d| super::super::access_for(&d, cx))
}

/// A display selector is data; only the shell's known outputs may match it.
pub(super) fn output(payload: &serde_json::Map<String, Value>) -> Result<Option<String>, String> {
    match payload.get("output") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(output)) => Ok(Some(output.clone())),
        Some(_) => Err("`output` must be a display name from `status`".into()),
    }
}

/// Local broker requests resolve their opaque handle before sharing the renderer path.
pub(super) fn render(plugin: &str, id: u64, payload: &serde_json::Map<String, Value>, cx: &mut App) -> Option<Result<Value, String>> {
    let request = (|| {
        let handle = payload.get("project").and_then(Value::as_u64).ok_or("`project` must be a project handle")?;
        let (project, root) = project_of(plugin, handle, cx)?;
        let request = mandara_plugins::live_wallpaper::Request {
            plugin: plugin.to_string(),
            generation: access(plugin, cx).ok_or("the plugin is not registered")?.generation,
            action: mandara_plugins::live_wallpaper::Action::Render { root, project, output: output(payload)? },
        };
        render_request(request, Some((handle, id)), cx)
    })();
    super::command_output(plugin, id, request, cx)
}

fn authorize_request(request: &mandara_plugins::live_wallpaper::Request, cx: &App) -> Result<(), String> {
    let access = access(&request.plugin, cx).ok_or("the plugin is not registered")?;
    let enabled = cx.global::<mandara_plugins::PluginRegistry>().is_enabled(&request.plugin, &crate::state::config(cx).read(cx).get().plugins);
    request.authorize(&access, enabled)
}

/// Studio sends only host-resolved projects. The shell owns authorization and processes.
pub(super) fn remote(request: mandara_plugins::live_wallpaper::Request, cx: &mut App) -> Result<mandara_plugins::Output, String> {
    use mandara_plugins::live_wallpaper::Action;
    if crate::hub::try_hub(cx).is_none() { return Err("live wallpapers need the running shell".into()); }
    authorize_request(&request, cx)?;
    match &request.action {
        Action::Render { .. } => render_request(request, None, cx),
        Action::Stop { output } => {
            let payload = serde_json::from_value(json!({"output":output})).map_err(|e| e.to_string())?;
            stop(&request.plugin, &payload, cx).map(Into::into)
        }
        Action::Status { .. } => {
            let (reply, result) = async_channel::bounded(1);
            status_then(cx, move |value, cx| {
                let answer = authorize_request(&request, cx).and(value).map(|value| value.to_string());
                let _ = reply.try_send(answer);
            });
            Ok(result.into())
        }
    }
}

fn render_request(request: mandara_plugins::live_wallpaper::Request, local: Option<(u64, u64)>, cx: &mut App) -> Result<mandara_plugins::Output, String> {
    use mandara_plugins::live_wallpaper::Action;
    authorize_request(&request, cx)?;
    let Action::Render { output, .. } = &request.action else { return Err("a render request is required".into()) };
    if !crate::state::config(cx).read(cx).get().wallpaper.enabled { return Err("the wallpaper is switched off in Studio".into()); }
    let all = wallpaper_outputs(cx);
    let outputs = match output {
        None => all,
        Some(want) => vec![all.into_iter().find(|output| output == want).ok_or("no such display shows the wallpaper (see `status`)")?],
    };
    if outputs.is_empty() { return Err("no display shows the wallpaper".into()); }
    preparation::enqueue(request, local, outputs, cx)
}

/// A project checked and ready to run.
struct Prepared {
    root: PathBuf,
    project: PathBuf,
    ink: Option<PathBuf>,
    assets: Option<PathBuf>,
    renderer: &'static Renderer,
    exe: PathBuf,
}

/// Off the UI thread: the project is still a folder inside the granted
/// one (links resolved now, so a folder swapped for a link since the
/// listing is caught), a scene or video, and a renderer is installed.
fn prepare(project: &Path, root: &Path) -> Result<Prepared, String> {
    let real_root = root.canonicalize().map_err(|_| "the granted folder is gone".to_string())?;
    let project = contained(project, &real_root).filter(|p| p.is_dir() && *p != real_root).ok_or("that project is no longer inside the granted folder")?;
    let info = project_info(&project);
    if !runnable(&info.kind) {
        return Err("only scene and video projects can run live".into());
    }
    let (renderer, exe) = find_renderer(RENDERERS, std::env::var_os("PATH").as_deref()).ok_or_else(not_installed)?;
    let ink = project_preview(&project, &info);
    Ok(Prepared { root: root.to_path_buf(), assets: steam_assets(&project), project, ink, renderer, exe })
}

/// Starts `p` on `outputs` for `plugin` (after the background checks:
/// the handle and its grant are checked again, they may have gone).
fn start(plugin: &str, permission_generation: u64, handle: Option<u64>, p: Prepared, outputs: &[String], cx: &mut App) -> Result<Value, String> {
    if let Some(handle) = handle {
        let (_, root) = project_of(plugin, handle, cx)?;
        if root != p.root { return Err("the grant changed; try again".into()); }
    }
    if !access(plugin, cx).is_some_and(|a| a.generation == permission_generation && a.allows(&Capability::WallpaperLive) && a.allows_folder(&p.root)) {
        return Err("the grant changed; try again".into());
    }
    let live = wallpaper_outputs(cx);
    if !outputs.iter().any(|output| live.contains(output)) { return Err("no display shows the wallpaper".into()); }
    let fps = crate::state::config(cx).read(cx).get().wallpaper.video.fps_cap();
    let mut started = Vec::new();
    for output in outputs.iter().filter(|o| live.contains(o)) {
        stop_output(output, cx);
        let job = Job { project: p.project.clone(), output: output.clone(), fps, assets: p.assets.clone() };
        let (tx, rx) = async_channel::bounded(2);
        let group = launch(p.exe.clone(), (p.renderer.argv)(&job), tx);
        let generation = GENERATION.with(|g| {
            g.set(g.get() + 1);
            g.get()
        });
        let out = output.clone();
        let reports = cx.spawn(async move |cx| {
            while let Ok(report) = rx.recv().await {
                cx.update(|cx| on_report(&out, generation, report, cx));
            }
        });
        let scene = Scene { plugin: plugin.to_string(), permission_generation, root: p.root.clone(), ink: p.ink.clone(), state: State::Running, group: Some(group), _reports: Some(reports), generation, wallpaper: WallpaperKey::of(crate::state::config(cx).read(cx).get()) };
        SCENES.with(|s| s.borrow_mut().insert(output.clone(), scene));
        started.push(output.clone());
    }
    watch(cx);
    changed(cx);
    Ok(json!({ "rendering": started, "renderer": p.renderer.name }))
}

/// A report from scene `generation` on `output` (an older scene's is
/// ignored).
fn on_report(output: &str, generation: u64, report: Report, cx: &mut App) {
    let current = SCENES.with(|s| s.borrow().get(output).is_some_and(|sc| sc.generation == generation));
    if !current {
        return;
    }
    let failed = match report {
        Report::Spawned(Ok(_)) => return,
        Report::Spawned(Err(why)) => format!("couldn't start the renderer: {why}"),
        Report::Exited(why) => format!("the renderer stopped ({why})"),
    };
    tracing::warn!(target: "mandara::plugin", "live wallpaper on {output}: {failed}");
    SCENES.with(|s| {
        if let Some(sc) = s.borrow_mut().get_mut(output) {
            sc.state = State::Failed(super::clip(&failed, MAX_STDERR));
            sc.group = None;
            sc._reports = None;
        }
    });
    settle(cx);
}

/// `stop {output?}`: ends `plugin`'s scenes (on one display or all).
pub(super) fn stop(plugin: &str, payload: &serde_json::Map<String, Value>, cx: &mut App) -> Result<Value, String> {
    let only = match payload.get("output") {
        None | Some(Value::Null) => None,
        Some(Value::String(o)) => Some(o.clone()),
        Some(_) => return Err("`output` must be a display name from `status`".into()),
    };
    PENDING.with(|p| p.borrow_mut().stop_plugin(plugin, only.as_deref()));
    preparation::stop(plugin, only.as_deref());
    let mine: Vec<String> = SCENES.with(|s| s.borrow().iter().filter(|(o, sc)| sc.plugin == plugin && only.as_ref().is_none_or(|w| w == *o)).map(|(o, _)| o.clone()).collect());
    for o in &mine {
        stop_output(o, cx);
    }
    settle(cx);
    Ok(json!({ "stopped": mine }))
}

/// Ends the scene on `output`, if any (no settling: callers do).
fn stop_output(output: &str, cx: &mut App) {
    if let Some(sc) = SCENES.with(|s| s.borrow_mut().remove(output))
        && let Some(group) = sc.group
    {
        end(group, cx);
    }
}

/// Ends every scene of `plugin` (it was switched off, removed or rebuilt).
pub(crate) fn stop_plugin(plugin: &str, cx: &mut App) {
    PENDING.with(|p| p.borrow_mut().stop_plugin(plugin, None));
    preparation::stop(plugin, None);
    let theirs: Vec<String> = SCENES.with(|s| s.borrow().iter().filter(|(_, sc)| sc.plugin == plugin).map(|(o, _)| o.clone()).collect());
    if theirs.is_empty() {
        settle(cx);
        return;
    }
    for o in &theirs {
        stop_output(o, cx);
    }
    settle(cx);
}

/// Ends every scene: a wallpaper was applied.
pub(crate) fn stop_all(cx: &mut App) {
    PENDING.with(|p| p.borrow_mut().clear());
    preparation::clear();
    let all: Vec<String> = SCENES.with(|s| s.borrow().keys().cloned().collect());
    if all.is_empty() {
        settle(cx);
        return;
    }
    for o in &all {
        stop_output(o, cx);
    }
    settle(cx);
}

/// Observes what ends scenes, from the first one on.
fn watch(cx: &mut App) {
    if WATCH.with(|w| w.borrow().is_some()) {
        return;
    }
    let wallpaper = WallpaperKey::of(crate::state::config(cx).read(cx).get());
    let config = cx.observe(&crate::state::config(cx), |_, cx| reconcile(cx));
    let outputs = cx.observe_global::<crate::overlays::Outputs>(reconcile);
    let quit = cx.on_app_quit(|_| {
        // The renderers end with the shell (their death signal too).
        SCENES.with(|s| {
            for sc in s.borrow_mut().drain().map(|(_, sc)| sc) {
                if let Some(g) = sc.group {
                    g.signal(libc::SIGCONT);
                    g.signal(libc::SIGTERM);
                }
            }
        });
        async {}
    });
    WATCH.with(|w| *w.borrow_mut() = Some(Watch { _config: config, _outputs: outputs, _quit: quit, wallpaper }));
}

/// Why a scene must end, if it must.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum End {
    Replaced,
    Revoked,
    SwitchedOff,
    DisplayGone,
}

/// What ends a scene of `plugin` from folder `root` on `output`: pure, so
/// every rule is tested.
pub(super) fn end_reason(started: &WallpaperKey, now: &WallpaperKey, access: Option<&Access>, enabled: bool, scope: (&Path, u64), output: &str, outputs: &[String]) -> Option<End> {
    let (root, generation) = scope;
    if started != now {
        return Some(End::Replaced);
    }
    if !enabled {
        return Some(End::SwitchedOff);
    }
    if !access.is_some_and(|a| a.generation == generation && a.allows(&Capability::WallpaperLive) && a.allows_folder(root)) {
        return Some(End::Revoked);
    }
    (!outputs.iter().any(|o| o == output)).then_some(End::DisplayGone)
}

/// Observe local edits immediately, including A → B → A in one UI update.
pub(crate) fn config_changed(before: &mandara_config::ShellConfig, after: &mandara_config::ShellConfig, cx: &mut App) {
    if WATCH.with(|watch| watch.borrow().is_none()) { return; }
    if WallpaperKey::of(before) != WallpaperKey::of(after)
        || before.plugins.grants != after.plugins.grants
        || before.plugins.enabled != after.plugins.enabled
        || before.plugins.disabled != after.plugins.disabled
    { reconcile(cx); }
}

/// The config or the outputs changed: ends what must end.
fn reconcile(cx: &mut App) {
    let Some(started) = WATCH.with(|w| w.borrow().as_ref().map(|w| w.wallpaper.clone())) else { return };
    let now = WallpaperKey::of(crate::state::config(cx).read(cx).get());
    PENDING.with(|pending| pending.borrow_mut().wallpaper_changed(&now));
    let outputs = wallpaper_outputs(cx);
    let pending_revoked = PENDING.with(|pending| pending.borrow_mut().retain_allowed(|plugin, root, generation, output| {
        outputs.iter().any(|known| known == output)
            && cx.global::<mandara_plugins::PluginRegistry>().is_enabled(plugin, &crate::state::config(cx).read(cx).get().plugins)
            && access(plugin, cx).is_some_and(|access| access.generation == generation && access.allows(&Capability::WallpaperLive) && access.allows_folder(root))
    }));
    let queued_cancelled = preparation::prune(cx);
    let scenes: Vec<_> = SCENES.with(|s| s.borrow().iter().map(|(o, sc)| (o.clone(), sc.plugin.clone(), sc.root.clone(), sc.permission_generation, sc.wallpaper.clone())).collect());
    let mut ended = false;
    for (output, plugin, root, generation, wallpaper) in scenes {
        let enabled = discovered(&plugin).is_some() && cx.global::<mandara_plugins::PluginRegistry>().is_enabled(&plugin, &crate::state::config(cx).read(cx).get().plugins);
        let access = access(&plugin, cx);
        if let Some(why) = end_reason(&wallpaper, &now, access.as_ref(), enabled, (&root, generation), &output, &outputs) {
            tracing::info!(target: "mandara::plugin", "live wallpaper on {output} ended: {why:?}");
            stop_output(&output, cx);
            ended = true;
        }
    }
    WATCH.with(|watch| { if let Some(watch) = watch.borrow_mut().as_mut() { watch.wallpaper = now.clone(); } });
    if ended || pending_revoked || queued_cancelled || started != now { settle(cx); }
}

/// After scenes ended: the watchers go once none runs (a failed scene
/// stays only to tell `status` why, until it is replaced or stopped), and
/// the desktop and `live-wallpaper` subscribers hear about it.
fn settle(cx: &mut App) {
    let running = SCENES.with(|s| s.borrow().values().any(|sc| sc.group.is_some()));
    if !running && PENDING.with(|pending| pending.borrow().is_empty()) {
        WATCH.with(|w| w.borrow_mut().take());
        SCENES.with(|s| s.borrow_mut().shrink_to_fit());
    }
    changed(cx);
}

/// The desktop redraws (wallpaper back or left out) and subscribers hear.
fn changed(cx: &mut App) {
    crate::wallpaper::live_changed(cx);
    crate::wallpaper_playback::refresh(cx);
    crate::backdrop::refresh(cx);
    cx.refresh_windows();
    super::events_soon(cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    fn job() -> Job {
        Job { project: "/lib/431960/123".into(), output: "DP-1".into(), fps: 24, assets: None }
    }

    #[test]
    fn the_command_line_comes_only_from_the_host() {
        let argv: Vec<String> = wallpaperengine_argv(&job()).into_iter().map(|a| a.into_string().unwrap()).collect();
        assert_eq!(argv, ["--layer", "background", "--fps", "24", "--silent", "--noautomute", "--no-audio-processing", "--disable-mouse", "--screen-root", "DP-1", "--bg", "/lib/431960/123"]);
        let with_assets = Job { assets: Some("/lib/steamapps/common/wallpaper_engine/assets".into()), ..job() };
        let argv = wallpaperengine_argv(&with_assets);
        let at = argv.iter().position(|a| a == "--assets-dir").unwrap();
        assert_eq!(argv[at + 1], "/lib/steamapps/common/wallpaper_engine/assets");
        // The project is one argument even with spaces or a leading dash.
        let odd = Job { project: "/lib/-x y".into(), ..job() };
        assert_eq!(wallpaperengine_argv(&odd).last().unwrap(), "/lib/-x y");
    }

    #[test]
    fn a_missing_renderer_is_reported_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path(), Path::new("relative/bin")]).unwrap();
        assert!(find_renderer(RENDERERS, Some(&path)).is_none(), "nothing installed");
        assert!(find_renderer(RENDERERS, None).is_none(), "no PATH at all");
        let exe = dir.path().join("linux-wallpaperengine");
        std::fs::write(&exe, b"").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(find_renderer(RENDERERS, Some(&path)).is_none(), "not executable");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (r, found) = find_renderer(RENDERERS, Some(&path)).unwrap();
        assert_eq!((r.name, found), ("linux-wallpaperengine", exe));
        assert!(not_installed().contains("linux-wallpaperengine"));
        // A relative PATH entry is never searched, even if it has one.
        let cwd_like = tempfile::tempdir().unwrap();
        std::fs::create_dir(cwd_like.path().join("bin")).unwrap();
        assert!(find_renderer(RENDERERS, Some(std::ffi::OsStr::new("bin"))).is_none());
    }

    /// `root/` with projects `a` (a scene with a preview), `b` (a web
    /// project), a hidden one, a file, a link out and a link to `a`.
    fn library() -> (tempfile::TempDir, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let r = root.path();
        for d in ["a", "b", ".hidden"] {
            std::fs::create_dir(r.join(d)).unwrap();
        }
        std::fs::write(r.join("a/project.json"), r#"{"title":"  Sea glass  ","type":"Scene","preview":"preview.jpg"}"#).unwrap();
        std::fs::write(r.join("a/preview.jpg"), b"x").unwrap();
        std::fs::write(r.join("b/project.json"), r#"{"title":"Site","type":"web"}"#).unwrap();
        std::fs::write(r.join("b/z.png"), b"x").unwrap();
        std::fs::write(r.join("b/a.webp"), b"x").unwrap();
        std::fs::write(r.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir(outside.path().join("secret")).unwrap();
        std::fs::write(outside.path().join("secret.png"), b"x").unwrap();
        symlink(outside.path().join("secret"), r.join("escape")).unwrap();
        symlink(r.join("a"), r.join("alias")).unwrap();
        (root, outside)
    }

    #[test]
    fn projects_stay_inside_the_granted_folder() {
        let (root, _outside) = library();
        let real = root.path().canonicalize().unwrap();
        let found = projects_in(root.path(), 100);
        // The link to `a` resolves to `a` itself (listed once); the link
        // out, the hidden folder and the file aren't projects.
        assert_eq!(found, [real.join("a"), real.join("b")]);
        // Same name in another case between the folder and a link to it.
        std::fs::create_dir(root.path().join("A")).unwrap();
        symlink(root.path().join("a"), root.path().join("a-link")).unwrap();
        let again = projects_in(root.path(), 100);
        assert_eq!(again.iter().filter(|p| **p == real.join("a")).count(), 1, "a folder reached twice is listed once: {again:?}");
        let a = project_info(&real.join("a"));
        assert_eq!((a.title.as_str(), a.kind.as_str()), ("Sea glass", "scene"));
        assert_eq!(project_preview(&real.join("a"), &a), Some(real.join("a/preview.jpg")));
        let b = project_info(&real.join("b"));
        assert_eq!(project_preview(&real.join("b"), &b), Some(real.join("b/a.webp")), "no preview named: the first image");
        assert!(runnable(&a.kind) && !runnable(&b.kind), "web projects never run");
    }

    #[test]
    fn project_scans_stop_at_the_entry_limit() {
        let root = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        assert!(projects_in(root.path(), 0).is_empty());
        assert_eq!(projects_in(root.path(), 1).len(), 1);
        assert_eq!(projects_in(root.path(), 2).len(), 2);
        assert_eq!(projects_in(root.path(), 3).len(), 3);
    }

    #[test]
    fn previews_and_titles_cant_leave_the_project_or_grow() {
        let (root, outside) = library();
        let a = root.path().canonicalize().unwrap().join("a");
        let secret = outside.path().join("secret.png");
        for preview in ["../notes.txt", "../../secret.png", secret.to_str().unwrap()] {
            std::fs::write(a.join("project.json"), format!(r#"{{"type":"scene","preview":"{preview}"}}"#)).unwrap();
            assert_eq!(project_info(&a).preview, None, "{preview}");
        }
        symlink(&secret, a.join("linked.png")).unwrap();
        std::fs::write(a.join("project.json"), r#"{"type":"scene","preview":"linked.png"}"#).unwrap();
        assert_eq!(project_info(&a).preview, None, "a link out isn't a preview");
        std::fs::write(a.join("project.json"), format!(r#"{{"type":"scene","title":"{}"}}"#, "é".repeat(MAX_TITLE))).unwrap();
        assert!(project_info(&a).title.len() <= MAX_TITLE);
        std::fs::write(a.join("project.json"), vec![b' '; MAX_PROJECT_JSON as usize + 1]).unwrap();
        assert_eq!(project_info(&a), ProjectInfo::default(), "an oversized project.json is ignored");
    }

    #[test]
    fn a_project_swapped_for_a_link_out_is_refused_at_render() {
        let (root, outside) = library();
        let real = root.path().canonicalize().unwrap();
        let listed = projects_in(root.path(), 100)[0].clone();
        assert_eq!(listed, real.join("a"));
        match prepare(&listed, root.path()) {
            Ok(p) => assert_eq!(p.project, listed),
            Err(e) => assert!(e.contains("renderer"), "inside: only the renderer can be missing: {e}"),
        }
        // Between the listing and the request, `a` becomes a link out.
        std::fs::rename(real.join("a"), outside.path().join("moved")).unwrap();
        symlink(outside.path().join("moved"), real.join("a")).unwrap();
        assert_eq!(prepare(&listed, root.path()).err().as_deref(), Some("that project is no longer inside the granted folder"));
        // A web project is refused before any renderer is looked for.
        assert_eq!(prepare(&real.join("b"), root.path()).err().as_deref(), Some("only scene and video projects can run live"));
        assert!(prepare(&real.join("b"), Path::new("/nonexistent-mandara-root")).is_err());
    }

    #[test]
    fn steam_assets_come_from_the_projects_own_library() {
        let lib = tempfile::tempdir().unwrap();
        let project = lib.path().join("steamapps/workshop/content/431960/123");
        std::fs::create_dir_all(&project).unwrap();
        assert_eq!(steam_assets(&project), None);
        std::fs::create_dir_all(lib.path().join("steamapps").join(STEAM_ASSETS)).unwrap();
        assert_eq!(steam_assets(&project), Some(lib.path().join("steamapps").join(STEAM_ASSETS)));
    }

    /// A fake renderer from the test seam: an ordinary program run with a
    /// fixed command line, found on the test's own `PATH`.
    fn fake(name: &'static str, argv: fn(&Job) -> Vec<OsString>) -> PathBuf {
        let list = [Renderer { name, argv }];
        find_renderer(&list, std::env::var_os("PATH").as_deref()).map(|(_, exe)| exe).unwrap_or_else(|| panic!("{name} is needed for this test"))
    }

    fn run(exe: PathBuf, argv: Vec<OsString>) -> (Group, async_channel::Receiver<Report>, i32) {
        let (tx, rx) = async_channel::bounded(2);
        let group = launch(exe, argv, tx);
        let Ok(Report::Spawned(Ok(pid))) = rx.recv_blocking() else { panic!("not started") };
        (group, rx, pid)
    }

    /// Whether `pid` ended: no such process, or a zombie its new parent
    /// hasn't reaped yet (given a moment).
    fn gone(pid: i32) -> bool {
        let ended = || {
            // SAFETY: signal 0 only checks whether the process exists.
            let missing = unsafe { libc::kill(pid, 0) == -1 };
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z')));
            missing || zombie
        };
        let started = std::time::Instant::now();
        while !ended() {
            if started.elapsed() > STOP_GRACE {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn a_renderer_runs_in_its_own_group_and_ends_with_it() {
        let sh = fake("sh", |_| Vec::new());
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("helper");
        // A renderer that starts a helper of its own, then waits.
        let script = format!("sleep 30 & echo $! > '{}.tmp' && mv '{0}.tmp' '{0}'; wait", pidfile.display());
        let (group, rx, pid) = run(sh, vec!["-c".into(), script.into()]);
        let started = std::time::Instant::now();
        while !pidfile.exists() {
            assert!(started.elapsed() < Duration::from_secs(10), "the helper never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        let helper: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(group.alive());
        // SAFETY: reads the child's process group.
        assert_eq!(unsafe { libc::getpgid(pid) }, pid, "it leads its own process group");
        assert_eq!(unsafe { libc::getpgid(helper) }, pid, "its helper is in that group");
        group.signal(libc::SIGTERM);
        let Ok(Report::Exited(message)) = rx.recv_blocking() else { panic!("no exit report") };
        assert!(!group.alive(), "the group is cleared before reaping");
        assert!(gone(pid) && gone(helper), "the leader and its helper both ended: {message}");
        group.signal(libc::SIGKILL); // a no-op now, never a stray signal
    }

    #[test]
    fn a_renderer_gets_a_minimal_environment_and_its_errors_are_kept() {
        let sh = fake("sh", |_| Vec::new());
        let (_group, rx, _) = run(sh, ["-c", "env >&2; exit 3"].map(OsString::from).into());
        let Ok(Report::Exited(message)) = rx.recv_blocking() else { panic!("no exit report") };
        assert!(message.contains("exit status: 3") && message.contains(&format!("PATH={CHILD_PATH}")), "{message}");
        assert!(!message.contains("CARGO"), "the shell's own environment isn't passed on: {message}");
        assert!(message.lines().all(|line| !line.starts_with("PWD=") || line == "PWD=/"), "it runs in / : {message}");
        let (_group, rx, _) = run(fake("sh", |_| Vec::new()), ["-c", "head -c 100000 /dev/zero | tr '\\0' x >&2; echo END >&2; exit 1"].map(OsString::from).into());
        let Ok(Report::Exited(message)) = rx.recv_blocking() else { panic!("no exit report") };
        assert!(message.len() < MAX_STDERR + 64 && message.ends_with("END"), "only the end of stderr is kept");
    }

    #[test]
    fn a_renderer_that_cant_start_is_reported() {
        let (tx, rx) = async_channel::bounded(2);
        let _group = launch("/nonexistent/linux-wallpaperengine".into(), Vec::new(), tx);
        assert!(matches!(rx.recv_blocking(), Ok(Report::Spawned(Err(_)))));
    }


    #[test]
    fn scenes_end_on_replace_revoke_switch_off_and_unplug() {
        let wp = |path: &str| WallpaperKey { enabled: true, path: Some(path.into()), variants: Default::default() };
        let root = Path::new("/steam/431960");
        let declared = ["files.folders".to_string(), "wallpaper.live".into()];
        let grant = |caps: &[&str], folders: &[&str]| Access::from_grant(&declared, Some(&mandara_config::PluginGrant { generation: 0, capabilities: caps.iter().map(|c| c.to_string()).collect(), folders: folders.iter().map(PathBuf::from).collect() }));
        let full = grant(&["files.folders", "wallpaper.live"], &["/steam/431960"]);
        let outputs = ["DP-1".to_string()];
        let reason = |now: &WallpaperKey, access: Option<&Access>, enabled: bool, output: &str| end_reason(&wp("/a.png"), now, access, enabled, (root, 0), output, &outputs);
        assert_eq!(reason(&wp("/a.png"), Some(&full), true, "DP-1"), None, "nothing changed");
        assert_eq!(reason(&wp("/b.png"), Some(&full), true, "DP-1"), Some(End::Replaced));
        assert_eq!(reason(&WallpaperKey { enabled: false, ..wp("/a.png") }, Some(&full), true, "DP-1"), Some(End::Replaced), "the wallpaper switched off");
        assert_eq!(reason(&wp("/a.png"), Some(&full), false, "DP-1"), Some(End::SwitchedOff));
        assert_eq!(reason(&wp("/a.png"), Some(&grant(&["files.folders"], &["/steam/431960"])), true, "DP-1"), Some(End::Revoked));
        assert_eq!(reason(&wp("/a.png"), Some(&grant(&["wallpaper.live"], &["/steam/431960"])), true, "DP-1"), Some(End::Revoked), "the folders capability");
        assert_eq!(reason(&wp("/a.png"), Some(&grant(&["files.folders", "wallpaper.live"], &["/other"])), true, "DP-1"), Some(End::Revoked), "the folder taken out");
        assert_eq!(reason(&wp("/a.png"), None, true, "DP-1"), Some(End::Revoked), "the plugin is gone");
        assert_eq!(reason(&wp("/a.png"), Some(&full), true, "HDMI-A-1"), Some(End::DisplayGone));
    }
}
