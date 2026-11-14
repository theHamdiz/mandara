//! Runs the external wallpaper renderer only on outputs with a Workshop selection.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use gpui::{App, Global, Task};
use mandara_config::WorkshopOptions;
use mandara_config::outputs::{SurfaceKind, assignment};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{
    ArgKind, ArgSpec, CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry,
};
use mandara_services::wallpaper_engine::{Library, discover};
use serde_json::json;

use crate::overlays::Outputs;
use crate::state::{config, update_config};

/// A slow scene can take several seconds to produce its first frame.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the screenshot while the original wallpaper is still present.
const START_POLL: Duration = Duration::from_millis(100);
/// Check liveness only while a renderer is running.
const EXIT_POLL: Duration = Duration::from_secs(1);

type Reply = async_channel::Sender<Result<String, String>>;

struct Selection {
    id: u64,
    options: WorkshopOptions,
    process: Option<Running>,
    _task: Task<()>,
    active: bool,
    preview: Option<PathBuf>,
    still: Option<PathBuf>,
}

#[derive(Default)]
struct Engine(BTreeMap<String, Selection>);
impl Global for Engine {}

pub use mandara_config::WALLPAPER_ENGINE_ID as ID;

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(
            ID,
            tk("Wallpaper Engine"),
            tk("Apply a Steam Workshop wallpaper to a display"),
            None,
        )
        .opt_in(),
        |_, _, _| PluginInstance::empty(),
    );
    r.command(
        ID,
        CommandSpec::new_output(
            "wallpaper workshop set",
            mandara_i18n::tk("Apply a Steam Workshop wallpaper to a display"),
            |a, cx| {
                let id = a
                    .str(1)?
                    .parse()
                    .map_err(|_| mandara_i18n::t("Invalid Workshop item ID").to_string())?;
                apply(a.str(0)?, id, a.get(2), cx)
            },
        )
        .arg(ArgSpec::required(
            "output",
            ArgKind::String,
            mandara_i18n::tk("Display"),
        ))
        .arg(ArgSpec::required(
            "id",
            ArgKind::String,
            mandara_i18n::tk("Workshop item ID"),
        ))
        .arg(ArgSpec::optional(
            "workspace",
            ArgKind::String,
            tk("tag / workspace number"),
        ))
        .example("wallpaper workshop set DP-1 2344341327"),
    );
    r.command(
        ID,
        CommandSpec::new(
            "wallpaper workshop clear",
            mandara_i18n::tk("Restore the regular wallpaper on a display"),
            |a, cx| {
                let output = a.str(0)?;
                clear(output, a.get(1), cx);
                Ok(json!({ "output": output }))
            },
        )
        .arg(ArgSpec::required(
            "output",
            ArgKind::String,
            mandara_i18n::tk("Display"),
        ))
        .arg(ArgSpec::optional(
            "workspace",
            ArgKind::String,
            tk("tag / workspace number"),
        ))
        .example("wallpaper workshop clear DP-1"),
    );
    r.command(
        ID,
        CommandSpec::new(
            "wallpaper workshop save-still",
            tk("Still image"),
            |a, cx| {
                let output = a.str(0)?;
                let selection = cx
                    .global::<Engine>()
                    .0
                    .get(output)
                    .filter(|selection| selection.active || selection.process.is_some())
                    .ok_or_else(|| t("Wallpaper Engine produced no frame").to_string())?;
                let path = if let Some(process) = &selection.process {
                    process.save_still(selection.id)?
                } else {
                    selection
                        .preview
                        .clone()
                        .ok_or_else(|| t("Wallpaper Engine produced no frame").to_string())?
                };
                crate::wallpaper::apply(path.clone(), cx);
                Ok(json!({ "path": path }))
            },
        )
        .arg(ArgSpec::required("output", ArgKind::String, tk("Display"))),
    );
}

pub fn init(cx: &mut App) {
    cx.set_global(Engine::default());
    cx.observe(&config(cx), |_, cx| sync(cx)).detach();
    cx.observe_global::<Outputs>(sync).detach();
    let hub = crate::hub::hub(cx);
    let mut rev = hub.read(cx).revision(crate::hub::Domains::NAVIGATION);
    cx.observe(&hub, move |hub, cx| {
        let next = hub.read(cx).revision(crate::hub::Domains::NAVIGATION);
        if next != rev {
            rev = next;
            sync(cx);
        }
    })
    .detach();
    cx.on_app_quit(|cx| {
        cx.global_mut::<Engine>().0.clear();
        async {}
    })
    .detach();
    sync(cx);
}

pub(crate) fn active(output: &str, cx: &App) -> bool {
    cx.try_global::<Engine>()
        .and_then(|engine| engine.0.get(output))
        .is_some_and(|s| s.active && s.options.live)
}

pub(crate) fn still_for_output(output: &str, cx: &App) -> Option<PathBuf> {
    cx.try_global::<Engine>()?
        .0
        .get(output)
        .filter(|s| s.active && !s.options.live)
        .and_then(|s| s.still.clone())
}

pub(crate) fn theme_preview(cx: &App) -> Option<PathBuf> {
    let engine = cx.try_global::<Engine>()?;
    let store = config(cx);
    let primary = store.read(cx).get().displays.primary.as_ref();
    primary
        .and_then(|p| engine.0.get(p))
        .and_then(|s| s.preview.clone())
        .or_else(|| engine.0.values().find_map(|s| s.preview.clone()))
}

pub(crate) fn capture_for_output(output: &str, cx: &App) -> Option<PathBuf> {
    let s = cx.try_global::<Engine>()?.0.get(output)?;
    (s.active && s.options.live)
        .then(|| s.still.clone().or_else(|| s.process.as_ref().map(|p| p.capture.clone())))
        .flatten()
}

fn desired(cx: &App) -> BTreeMap<String, u64> {
    let store = config(cx);
    let c = store.read(cx).get();
    if !c.plugins.opted_in(ID)
        || !c.wallpaper.enabled
        || (c.wallpaper.workshop.is_empty() && c.wallpaper.workshop_workspaces.is_empty())
    {
        return BTreeMap::new();
    }
    let hub = crate::hub::hub(cx);
    let navigation = &hub.read(cx).navigation;
    cx.try_global::<Outputs>()
        .into_iter()
        .flat_map(|outputs| &outputs.0)
        .filter(|o| assignment(&c.displays, SurfaceKind::Wallpaper, &o.name))
        .filter_map(|o| {
            let selected = c
                .wallpaper
                .workshop_workspaces
                .get(&o.name)
                .and_then(|scenes| {
                    navigation
                        .get(&o.name)?
                        .active_ids()
                        .find_map(|id| scenes.get(&id.to_string()))
                });
            selected
                .or_else(|| c.wallpaper.workshop.get(&o.name))
                .map(|id| (o.name.clone(), *id))
        })
        .collect()
}

fn sync(cx: &mut App) {
    let want = desired(cx);
    let options = config(cx).read(cx).get().wallpaper.workshop_options.clone();
    let current =
        |output: &String, s: &Selection| want.get(output) == Some(&s.id) && options == s.options;
    let engine = cx.global::<Engine>();
    if engine.0.len() == want.len() && engine.0.iter().all(|(o, s)| current(o, s)) {
        return;
    }
    cx.global_mut::<Engine>()
        .0
        .retain(|output, selection| current(output, selection));
    for (output, id) in want {
        if !cx.global::<Engine>().0.contains_key(&output) {
            start(output, id, options.clone(), None, cx);
        }
    }
    redraw(cx);
}

fn redraw(cx: &mut App) {
    crate::backdrop::refresh(cx);
    crate::wallpaper::sync_theme(cx);
    cx.refresh_windows();
}

pub(crate) fn apply(
    output: &str,
    id: u64,
    workspace: Option<&str>,
    cx: &mut App,
) -> Result<Output, String> {
    let outputs = cx
        .try_global::<Outputs>()
        .ok_or_else(|| tf("No display named {name}", &[("name", &output)]))?;
    if !outputs
        .0
        .iter()
        .any(|o| !o.name.is_empty() && o.name == output)
        || !assignment(
            &config(cx).read(cx).get().displays,
            SurfaceKind::Wallpaper,
            output,
        )
    {
        return Err(tf(
            "Wallpaper Engine cannot use display {output}",
            &[("output", &output)],
        ));
    }
    let output = output.to_string();
    let workspace = workspace
        .map(|w| w.parse::<u32>().map(|id| id.to_string()))
        .transpose()
        .map_err(|_| t("Workspace not found").to_string())?;
    update_config(cx, |c| {
        c.wallpaper.enabled = true;
        if let Some(workspace) = workspace {
            c.wallpaper
                .workshop_workspaces
                .entry(output.clone())
                .or_default()
                .insert(workspace, id);
        } else {
            c.wallpaper.workshop.insert(output.clone(), id);
        }
    });
    if desired(cx).get(&output) != Some(&id) {
        return Ok(json!({ "output": output, "id": id }).into());
    }
    let options = config(cx).read(cx).get().wallpaper.workshop_options.clone();
    let (tx, rx) = async_channel::bounded(1);
    start(output, id, options, Some(tx), cx);
    redraw(cx);
    Ok(Output::Pending(rx))
}

fn clear(output: &str, workspace: Option<&str>, cx: &mut App) {
    update_config(cx, |c| {
        if let Some(workspace) = workspace {
            if let Some(scenes) = c.wallpaper.workshop_workspaces.get_mut(output) {
                scenes.remove(workspace);
                if scenes.is_empty() {
                    c.wallpaper.workshop_workspaces.remove(output);
                }
            }
        } else {
            c.wallpaper.workshop.remove(output);
        }
    });
    sync(cx);
}

fn start(
    output: String,
    id: u64,
    options: WorkshopOptions,
    mut reply: Option<Reply>,
    cx: &mut App,
) {
    let monitor = output.clone();
    let prefs = options.clone();
    let task = cx.spawn(async move |cx| {
        let folder = prefs.steam_folder.clone();
        let library = cx.background_executor().spawn(async move { discover(&folder) }).await;
        let result = async {
            let library = library.map_err(|path| tf("Wallpaper Engine folder not found: {path}", &[("path", &path.display())]))?;
            let process = Running::spawn(library, &monitor, id, &prefs)?;
            cx.update(|cx| {
                cx.global_mut::<Engine>()
                    .0
                    .get_mut(&monitor)
                    .unwrap()
                    .process = Some(process)
            });
            let started = Instant::now();
            loop {
                let ready = cx.update(|cx| {
                    let p = cx
                        .global_mut::<Engine>()
                        .0
                        .get_mut(&monitor)
                        .unwrap()
                        .process
                        .as_mut()
                        .unwrap();
                    p.check()?;
                    Ok::<_, String>(p.has_frame())
                })?;
                if ready {
                    break;
                }
                if started.elapsed() >= START_TIMEOUT {
                    return Err(t("Wallpaper Engine produced no frame").to_string());
                }
                cx.background_executor().timer(START_POLL).await;
            }
            cx.update(|cx| {
                let s = cx.global_mut::<Engine>().0.get_mut(&monitor).unwrap();
                let p = s.process.as_mut().unwrap();
                let still = (prefs.save_still || !prefs.live)
                    .then(|| p.save_still(id))
                    .transpose()?;
                s.preview = still
                    .clone()
                    .or_else(|| p.preview.take())
                    .or_else(|| Some(p.capture.clone()));
                s.still = still.clone();
                s.active = true;
                if !prefs.live {
                    s.process.take();
                }
                let is_default =
                    config(cx).read(cx).get().wallpaper.workshop.get(&monitor) == Some(&id);
                if let Some(path) = still.filter(|_| prefs.save_still && is_default) {
                    crate::wallpaper::apply(path, cx);
                }
                redraw(cx);
                Ok::<_, String>(())
            })?;
            if let Some(tx) = reply.take() {
                let _ = tx.try_send(Ok(json!({ "output": monitor, "id": id }).to_string()));
            }
            if !prefs.live {
                return Ok(());
            }
            loop {
                cx.background_executor().timer(EXIT_POLL).await;
                cx.update(|cx| {
                    cx.global_mut::<Engine>()
                        .0
                        .get_mut(&monitor)
                        .unwrap()
                        .process
                        .as_mut()
                        .unwrap()
                        .check()
                })?;
            }
        }
        .await;
        let Err(err) = result else { return };
        cx.update(|cx| {
            let s = cx.global_mut::<Engine>().0.get_mut(&monitor).unwrap();
            s.process.take();
            s.active = false;
            s.preview = None;
            redraw(cx);
            tracing::warn!(%monitor, id, %err, "Wallpaper Engine");
            if let Some(tx) = reply {
                let _ = tx.try_send(Err(err));
            } else {
                crate::plugins::notifications::send(
                    cx,
                    mandara_services::NotificationCommand::Post {
                        app: "Wallpaper Engine".into(),
                        icon: "image".into(),
                        summary: tf(
                            "Wallpaper Engine cannot use display {output}",
                            &[("output", &monitor)],
                        ),
                        body: err,
                    },
                );
            }
        });
    });
    cx.global_mut::<Engine>().0.insert(
        output,
        Selection {
            id,
            options,
            process: None,
            _task: task,
            active: false,
            preview: None,
            still: None,
        },
    );
}

struct Running {
    child: Child,
    capture: PathBuf,
    preview: Option<PathBuf>,
    _scratch: tempfile::TempDir,
}

impl Running {
    fn spawn(
        mut library: Library,
        output: &str,
        id: u64,
        options: &WorkshopOptions,
    ) -> Result<Self, String> {
        let project = library
            .projects
            .remove(&id)
            .ok_or_else(|| tf("Workshop item {id} is unavailable", &[("id", &id)]))?;
        let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
        let capture = scratch.path().join("frame.jpg");
        let executable = if options.executable.trim().is_empty() {
            PathBuf::from("linux-wallpaperengine")
        } else {
            mandara_config::paths::expand_tilde(options.executable.trim())
        };
        let mut cmd = Command::new(&executable);
        cmd.args(["--layer", "background", "--screen-root", output, "--bg"])
            .arg(project.directory)
            .arg("--fps")
            .arg(options.fps.clamp(1, 240).to_string())
            .arg("--screenshot")
            .arg(&capture)
            .stdin(Stdio::null());
        if matches!(options.scaling.as_str(), "fill" | "fit" | "stretch") {
            cmd.arg("--scaling").arg(&options.scaling);
        }
        if options.mute {
            cmd.args(["--silent", "--noautomute", "--no-audio-processing"]);
        } else {
            cmd.arg("--volume").arg(options.volume.min(100).to_string());
        }
        if !options.mouse {
            cmd.arg("--disable-mouse");
        }
        if !options.pause_fullscreen {
            cmd.arg("--no-fullscreen-pause");
        }
        if let Some(assets) = library.assets {
            cmd.arg("--assets-dir").arg(assets);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt as _;
            let parent = std::process::id();
            // The child should also stop when the shell is killed or crashes.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent {
                        return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                    }
                    Ok(())
                });
            }
        }
        let child = cmd.spawn().map_err(|e| {
            tf(
                "Could not start {program}: {error}",
                &[("program", &"linux-wallpaperengine"), ("error", &e)],
            )
        })?;
        let preview = project.preview;
        Ok(Self {
            child,
            capture,
            preview,
            _scratch: scratch,
        })
    }

    fn save_still(&self, id: u64) -> Result<PathBuf, String> {
        use std::io::copy;
        let directory = mandara_config::paths::data_dir()
            .join("wallpapers")
            .join("workshop");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join(format!("{id}.jpg"));
        if !path.is_file() {
            let mut source = std::fs::File::open(&self.capture).map_err(|e| e.to_string())?;
            let mut temp =
                tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
            copy(&mut source, &mut temp).map_err(|e| e.to_string())?;
            temp.persist(&path).map_err(|e| e.to_string())?;
        }
        Ok(path)
    }

    fn has_frame(&self) -> bool {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&self.capture) else {
            return false;
        };
        // The engine writes directly to the file; wait for the JPEG end marker.
        let mut end = [0; 2];
        file.seek(SeekFrom::End(-2)).is_ok()
            && file.read_exact(&mut end).is_ok()
            && end == [0xff, 0xd9]
    }

    fn check(&mut self) -> Result<(), String> {
        if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
            return Err(tf(
                "Wallpaper Engine exited: {status}",
                &[("status", &status)],
            ));
        }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
