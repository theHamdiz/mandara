//! One theme writer, with one coalesced request waiting behind it.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use mandara_config::IntegrationsConfig;
use mandara_theme::templates::{AppTargets, Ledger, Targets};
use mandara_theme::{MandaraPalette, PaletteAdjustments, ThemeSource};
use parking_lot::Mutex;

use super::{
    apply::{self, Report, Theme},
    paths::Locations,
};

/// Everything an export reads, compared before cloning on the UI thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    pub palette: MandaraPalette,
    pub source: ThemeSource,
    pub adjustments: PaletteAdjustments,
    pub integrations: IntegrationsConfig,
    pub font_family: String,
    pub image: PathBuf,
}

pub fn targets(i: &IntegrationsConfig) -> (Targets, AppTargets) {
    (
        Targets {
            ghostty: i.ghostty,
            gtk: i.gtk4,
            qt6: i.qt6,
            zed: i.zed,
            fuzzel: i.fuzzel,
            btop: i.btop,
            kvantum: i.kvantum,
            kvantum_apply: i.kvantum_apply,
        },
        AppTargets {
            cava: i.cava,
            tmux: i.tmux,
            zellij: i.zellij,
            spicetify: i.spicetify,
            kitty: i.kitty,
            foot: i.foot,
            alacritty: i.alacritty,
            wezterm: i.wezterm,
            helix: i.helix,
            neovim: i.neovim,
            firefox: i.firefox,
            vesktop: i.vesktop,
            kde: i.kde,
        },
    )
}

/// Callers waiting for the same export share its result; repeated commands
/// cannot retain an unbounded number of reply channels.
const MAX_WAITERS: usize = 8;

struct Request {
    inputs: Arc<Inputs>,
    generation: u64,
    force: bool,
    replies: Vec<async_channel::Sender<Arc<Report>>>,
}

#[derive(Default)]
struct Requests {
    pending: Option<Request>,
    active: Option<Request>,
    completed: Option<Arc<Inputs>>,
    status: Arc<Report>,
}

trait Exporter: Send + Sync + 'static {
    fn export(
        &self,
        inputs: &Inputs,
        cancel: &AtomicBool,
        superseded: &dyn Fn() -> bool,
    ) -> anyhow::Result<Report>;
}
struct Native;
impl Exporter for Native {
    fn export(
        &self,
        inputs: &Inputs,
        cancel: &AtomicBool,
        superseded: &dyn Fn() -> bool,
    ) -> anyhow::Result<Report> {
        export(inputs, cancel, superseded)
    }
}

pub struct Worker {
    wake: async_channel::Sender<()>,
    requests: Arc<Mutex<Requests>>,
    generation: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

fn failed(error: impl Into<String>) -> Arc<Report> {
    Arc::new(Report {
        errors: [("export".into(), error.into())].into(),
        ..Default::default()
    })
}

impl Default for Worker {
    fn default() -> Self {
        Self::new(Native)
    }
}

impl Worker {
    fn new(exporter: impl Exporter) -> Self {
        let (wake, receiver) = async_channel::bounded(1);
        let requests = Arc::new(Mutex::new(Requests::default()));
        let generation = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (queue, latest, shutdown) = (requests.clone(), generation.clone(), cancel.clone());
        let exporter = Arc::new(exporter);
        crate::runtime::spawn("app-themes", async move {
            while receiver.recv().await.is_ok() {
                loop {
                    let next = {
                        let mut requests = queue.lock();
                        requests.pending.take().map(|request| {
                            let next = (request.inputs.clone(), request.generation, request.force);
                            requests.active = Some(request);
                            next
                        })
                    };
                    let Some((inputs, generation, force)) = next else {
                        break;
                    };
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let previous = {
                        let requests = queue.lock();
                        (!force && requests.completed.as_deref() == Some(inputs.as_ref()))
                            .then(|| requests.status.clone())
                    };
                    let (report, finished) = if let Some(previous) = previous {
                        (previous, true)
                    } else {
                        let (job, latest, shutdown, exporter) = (
                            inputs.clone(),
                            latest.clone(),
                            shutdown.clone(),
                            exporter.clone(),
                        );
                        crate::runtime::unblock(move || {
                            let superseded = || latest.load(Ordering::Acquire) != generation;
                            if superseded() || shutdown.load(Ordering::Acquire) {
                                return (
                                    failed("a newer app-theme request replaced this one"),
                                    false,
                                );
                            }
                            let result = exporter.export(&job, &shutdown, &superseded);
                            if superseded() || shutdown.load(Ordering::Acquire) {
                                return (
                                    failed("a newer app-theme request replaced this one"),
                                    false,
                                );
                            }
                            let report = result
                                .map(Arc::new)
                                .unwrap_or_else(|error| failed(format!("{error:#}")));
                            let finished = report.errors.is_empty();
                            (report, finished)
                        })
                        .await
                    };
                    let active = {
                        let mut requests = queue.lock();
                        requests.completed = finished.then_some(inputs);
                        requests.status = report.clone();
                        requests.active.take()
                    };
                    if let Some(request) = active {
                        for reply in request.replies {
                            let _ = reply.try_send(report.clone());
                        }
                    }
                }
            }
        });
        Self {
            wake,
            requests,
            generation,
            cancel,
        }
    }

    pub fn submit(&self, inputs: Inputs, force: bool) -> async_channel::Receiver<Arc<Report>> {
        let (reply, receiver) = async_channel::bounded(1);
        let mut requests = self.requests.lock();
        if !force {
            let matching_pending = requests
                .pending
                .as_ref()
                .is_some_and(|request| *request.inputs == inputs);
            let matching_active = requests.pending.is_none()
                && requests
                    .active
                    .as_ref()
                    .is_some_and(|request| *request.inputs == inputs);
            let matching = if matching_pending {
                requests.pending.as_mut()
            } else if matching_active {
                requests.active.as_mut()
            } else {
                None
            };
            if let Some(request) = matching {
                if request.replies.len() < MAX_WAITERS {
                    request.replies.push(reply);
                } else {
                    let _ = reply.try_send(failed("app themes are busy; try again in a moment"));
                }
                return receiver;
            }
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(old) = requests.pending.take() {
            let report = failed("a newer app-theme request replaced this one");
            for reply in old.replies {
                let _ = reply.try_send(report.clone());
            }
        }
        requests.pending = Some(Request {
            inputs: Arc::new(inputs),
            generation,
            force,
            replies: vec![reply],
        });
        let _ = self.wake.force_send(());
        receiver
    }

    pub fn status(&self) -> Arc<Report> {
        self.requests.lock().status.clone()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let mut requests = self.requests.lock();
        requests.pending = None;
        requests.completed = None;
    }
}

fn export(
    inputs: &Inputs,
    cancel: &AtomicBool,
    superseded: &dyn Fn() -> bool,
) -> anyhow::Result<Report> {
    export_at(inputs, &Locations::current()?, cancel, superseded)
}

fn export_at(
    inputs: &Inputs,
    locations: &Locations,
    cancel: &AtomicBool,
    superseded: &dyn Fn() -> bool,
) -> anyhow::Result<Report> {
    let generated = locations.config.join("mandara/generated");
    let ledger_path = locations.ledger();
    let (targets, apps) = targets(&inputs.integrations);
    if !inputs.integrations.any_app()
        && !ledger_path.try_exists()?
        && !locations.applied().try_exists()?
    {
        return Ok(Report::default());
    }
    std::fs::create_dir_all(&generated)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(generated.join("app-themes.lock"))?;
    lock.lock()?;
    if superseded() || cancel.load(Ordering::Acquire) {
        return Ok(Report::default());
    }
    let cache = locations.cache.join("mandara/themes");
    let mut ledger = Ledger::read(&ledger_path)?;
    let mut native = mandara_theme::templates::sync_checked(
        &inputs.palette,
        targets,
        &cache,
        &locations.config,
        &mut ledger,
    );
    native.append(mandara_theme::templates::sync_apps_checked(
        &inputs.palette,
        apps,
        &cache,
        &locations.config,
        &locations.data,
        &mut ledger,
    ));
    ledger.store(&ledger_path)?;
    if superseded() {
        return Ok(Report {
            files: native.files,
            errors: native.errors,
            ..Default::default()
        });
    }
    let custom = &inputs.integrations.app_themes;
    // Resolving wallpaper colors is unnecessary when no custom entry is on.
    let other = if custom.any_enabled() {
        inputs
            .source
            .resolve(!inputs.palette.is_dark)?
            .palette
            .adjusted(&inputs.adjustments)
    } else {
        inputs.palette.clone()
    };
    let theme = Theme {
        palette: inputs.palette.clone(),
        other,
        font_family: inputs.font_family.clone(),
        image: inputs.image.clone(),
    };
    let mut report = apply::sync(custom, &theme, locations, cancel, superseded)?;
    report.files.extend(native.files);
    report.errors.extend(native.errors);
    for (entry, error) in &report.errors {
        tracing::warn!("app theme {entry}: {error}");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl Exporter for Mock {
        fn export(
            &self,
            _: &Inputs,
            _: &AtomicBool,
            _: &dyn Fn() -> bool,
        ) -> anyhow::Result<Report> {
            let number = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if number == 1 {
                self.started.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            }
            Ok(Report {
                files: vec![number.to_string().into()],
                ..Default::default()
            })
        }
    }
    fn inputs() -> Inputs {
        Inputs {
            palette: mandara_theme::engine::default_palette(true),
            source: ThemeSource::default(),
            adjustments: Default::default(),
            integrations: Default::default(),
            font_family: "Inter".into(),
            image: "wallpaper.webp".into(),
        }
    }

    fn locations(root: &std::path::Path) -> Locations {
        Locations {
            home: root.into(),
            config: root.join("config"),
            cache: root.join("cache"),
            data: root.join("data"),
            state: root.join("state"),
        }
    }

    #[test]
    fn built_in_imports_and_custom_whole_files_share_one_ownership_record() {
        use mandara_theme::templates::custom::{Paths, Template};
        for custom_first in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let locations = locations(root.path());
            let css = locations.config.join("gtk-4.0/gtk.css");
            std::fs::create_dir_all(css.parent().unwrap()).unwrap();
            std::fs::write(&css, "/* personal */\n").unwrap();
            let mut inputs = inputs();
            let custom = Template { enabled: true, content: Some("/* custom */\n".into()), output_path: Paths::One(css.clone()), ..Default::default() };
            if custom_first { inputs.integrations.app_themes.user.insert("gtk-file".into(), custom.clone()); }
            else { inputs.integrations.gtk4 = true; }
            let cancel = AtomicBool::new(false);
            assert!(export_at(&inputs, &locations, &cancel, &||false).unwrap().errors.is_empty());
            if custom_first { inputs.integrations.gtk4 = true; }
            else { inputs.integrations.app_themes.user.insert("gtk-file".into(), custom); }
            assert!(!export_at(&inputs, &locations, &cancel, &||false).unwrap().errors.is_empty());
            inputs.integrations.gtk4 = false;
            inputs.integrations.app_themes.user.clear();
            assert!(export_at(&inputs, &locations, &cancel, &||false).unwrap().errors.is_empty());
            assert_eq!(std::fs::read_to_string(css).unwrap(), "/* personal */\n");
        }
    }

    #[test]
    fn disabled_exports_do_not_create_folders_or_ownership_records() {
        let root = tempfile::tempdir().unwrap();
        let report = export_at(&inputs(), &locations(root.path()), &AtomicBool::new(false), &||false).unwrap();
        assert!(report.files.is_empty() && report.errors.is_empty());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn native_write_and_restore_failures_are_returned_to_the_caller() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        std::fs::create_dir_all(&locations.config).unwrap();
        std::fs::write(locations.config.join("ghostty"), "a file blocks the theme directory").unwrap();
        let mut inputs = inputs();
        inputs.integrations.ghostty = true;
        let report = export_at(&inputs, &locations, &AtomicBool::new(false), &||false).unwrap();
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors.keys().next().unwrap().ends_with("ghostty/themes/mandara"));

        std::fs::remove_file(locations.config.join("ghostty")).unwrap();
        let target = locations.config.join("ghostty/themes/mandara");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "original").unwrap();
        assert!(export_at(&inputs, &locations, &AtomicBool::new(false), &||false).unwrap().errors.is_empty());
        let ledger_path = locations.config.join("mandara/generated/app-themes.toml");
        let ledger = Ledger::read(&ledger_path).unwrap();
        let entry = ledger.files.values().next().unwrap();
        std::fs::remove_file(entry.backup.as_ref().unwrap()).unwrap();
        inputs.integrations.ghostty = false;
        let report = export_at(&inputs, &locations, &AtomicBool::new(false), &||false).unwrap();
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors.values().next().unwrap().contains("backup is missing"));
        assert!(target.exists());
        assert_eq!(Ledger::read(&ledger_path).unwrap().files.len(), 1);
    }

    #[test]
    fn automatic_exports_join_a_forced_update_and_do_not_cancel_it_or_repeat_hooks() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (started, waiting) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let worker = Worker::new(Mock {
            calls: calls.clone(),
            started,
            release: std::sync::Mutex::new(blocked),
        });
        let first = worker.submit(inputs(), true);
        waiting.recv().unwrap();
        let update = worker.submit(inputs(), true);
        let automatic = worker.submit(inputs(), false);
        release.send(()).unwrap();
        assert!(!first.recv_blocking().unwrap().errors.is_empty());
        let updated = update.recv_blocking().unwrap();
        let joined = automatic.recv_blocking().unwrap();
        assert!(Arc::ptr_eq(&updated, &joined));
        assert_eq!(updated.files, vec![PathBuf::from("2")]);
        let cached = worker.submit(inputs(), false).recv_blocking().unwrap();
        assert!(Arc::ptr_eq(&updated, &cached));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn every_palette_role_and_template_context_participates_in_invalidation() {
        let original = Inputs {
            palette: mandara_theme::engine::default_palette(true),
            source: ThemeSource::default(),
            adjustments: Default::default(),
            integrations: Default::default(),
            font_family: "Inter".into(),
            image: "wallpaper.webp".into(),
        };
        let mut changed = original.clone();
        changed.palette.on_surface = mandara_theme::Color::hex("#ffcc00");
        assert_ne!(original, changed);
        changed = original.clone();
        changed.font_family.push_str(" New");
        assert_ne!(original, changed);
        changed = original.clone();
        changed.image = "another.webp".into();
        assert_ne!(original, changed);
        changed = original.clone();
        changed.integrations.app_themes.run_hooks = false;
        assert_ne!(original, changed);
    }
}
