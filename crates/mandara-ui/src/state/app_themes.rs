//! Debounce theme exports without retaining configuration on animation frames.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Global, Task};
use mandara_services::app_themes::worker::{Inputs, Worker};

struct AppThemeWorker(Worker);
impl Global for AppThemeWorker {}

/// A theme crossfade or slider drag writes once after it settles.
const SETTLE_MILLIS: u64 = 600;
const SETTLE: Duration = Duration::from_millis(SETTLE_MILLIS);

pub fn app_theme_targets(
    i: &mandara_config::IntegrationsConfig,
) -> (
    mandara_theme::templates::Targets,
    mandara_theme::templates::AppTargets,
) {
    mandara_services::app_themes::worker::targets(i)
}

fn inputs(palette: mandara_theme::MandaraPalette, cx: &App) -> Inputs {
    let store = super::config(cx);
    let cfg = store.read(cx).get();
    let engine = mandara_core::theme_entity(cx);
    let engine = engine.read(cx);
    let image = image(cfg, palette.is_dark).to_path_buf();
    Inputs {
        palette,
        source: engine.source().clone(),
        adjustments: *engine.adjustments(),
        integrations: cfg.integrations.clone(),
        font_family: cfg.typography.ui_font.clone(),
        image,
    }
}

fn image(cfg: &mandara_config::ShellConfig, dark: bool) -> &std::path::Path {
    let variant = if dark {
        &cfg.wallpaper.variants.dark
    } else {
        &cfg.wallpaper.variants.light
    };
    variant
        .as_deref()
        .filter(|path| !path.as_os_str().is_empty())
        .or(cfg.wallpaper.path.as_deref())
        .unwrap_or_else(|| std::path::Path::new(""))
}

pub fn app_theme_status(cx: &App) -> std::sync::Arc<mandara_services::app_themes::apply::Report> {
    cx.try_global::<AppThemeWorker>()
        .map(|worker| worker.0.status())
        .unwrap_or_default()
}

pub fn export_app_themes_report(
    palette: mandara_theme::MandaraPalette,
    cx: &mut App,
) -> Task<std::sync::Arc<mandara_services::app_themes::apply::Report>> {
    submit(palette, true, cx)
}

fn submit(
    palette: mandara_theme::MandaraPalette,
    force: bool,
    cx: &mut App,
) -> Task<std::sync::Arc<mandara_services::app_themes::apply::Report>> {
    if cx.try_global::<AppThemeWorker>().is_none() {
        cx.set_global(AppThemeWorker(Default::default()));
    }
    let reply = cx
        .global::<AppThemeWorker>()
        .0
        .submit(inputs(palette, cx), force);
    cx.spawn(async move |cx| {
        let report = reply.recv().await.unwrap_or_else(|_| {
            std::sync::Arc::new(mandara_services::app_themes::apply::Report {
                errors: [(
                    "export".into(),
                    "a newer app-theme request replaced this one".into(),
                )]
                .into(),
                ..Default::default()
            })
        });
        cx.update(|cx| cx.refresh_windows());
        report
    })
}

pub fn export_app_themes(
    palette: mandara_theme::MandaraPalette,
    cx: &mut App,
) -> Task<Vec<std::path::PathBuf>> {
    let report = submit(palette, false, cx);
    cx.spawn(async move |_| report.await.files.clone())
}

pub fn init_app_theme_sync(cx: &mut App) {
    let mut last: Option<Inputs> = None;
    let pending: Rc<RefCell<Option<Task<()>>>> = Rc::default();
    let on_change = Rc::new(RefCell::new(move |cx: &mut App| {
        let store = super::config(cx);
        let cfg = store.read(cx).get();
        let engine = mandara_core::theme_entity(cx);
        let engine = engine.read(cx);
        if last.as_ref().is_some_and(|last| {
            last.palette == *engine.target()
                && last.source == *engine.source()
                && last.adjustments == *engine.adjustments()
                && last.integrations == cfg.integrations
                && last.font_family == cfg.typography.ui_font
                && last.image == image(cfg, engine.is_dark())
        }) {
            return;
        }
        last = Some(inputs(engine.target().clone(), cx));
        *pending.borrow_mut() = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(SETTLE).await;
            let task = cx.update(|cx| {
                export_app_themes(mandara_core::theme_entity(cx).read(cx).target().clone(), cx)
            });
            let files = task.await;
            cx.update(|cx| crate::powerctl::hooks::templates_rendered(&files, cx));
        }));
    }));
    (on_change.borrow_mut())(cx);
    let on_theme = on_change.clone();
    cx.observe(&mandara_core::theme_entity(cx), move |_, cx| {
        (on_theme.borrow_mut())(cx)
    })
    .detach();
    cx.observe(&super::config(cx), move |_, cx| {
        (on_change.borrow_mut())(cx)
    })
    .detach();
}
