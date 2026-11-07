//! Copy complete themed reading cards as PNGs, with one lazy bounded export job.

mod document;
pub(crate) mod jobs;
mod paint;
mod png;
mod render;

pub(super) use document::{Block, Document, Family, Ornament};

use std::rc::Rc;

use gpui::{
    AnyElement, App, Global, InteractiveElement as _, IntoElement, StatefulInteractiveElement as _,
    Window,
};
use mandara_i18n::t;
use mandara_plugins::PanelCx;
use mandara_theme::MandaraPalette;

use super::{extras::Flash, panel_fx::pill_button};

#[derive(Default)]
struct Exports(jobs::Gate);
impl Global for Exports {}

/// Panel screenshots and complete reading cards share one image-producing slot.
pub(crate) fn acquire(cx: &mut App) -> Option<jobs::Lease> {
    cx.default_global::<Exports>().0.acquire()
}

/// Only the panel's visibility token is retained; images and layout live in a job.
#[derive(Clone)]
pub(super) struct View {
    panel: PanelCx,
    life: Rc<()>,
}
impl View {
    pub fn new(panel: PanelCx) -> Self {
        Self {
            panel,
            life: Rc::new(()),
        }
    }
}

/// The callback resolves the already displayed entry only when the button is pressed.
pub(super) fn button(
    id: &'static str,
    view: &View,
    flash: Flash,
    palette: &MandaraPalette,
    notified: gpui::EntityId,
    make: impl Fn(&mut App, &Window) -> Result<Document, String> + 'static,
) -> AnyElement {
    let view = view.clone();
    let language = mandara_i18n::language();
    let label = t("Copy as image");
    pill_button(
        id,
        Some("image"),
        t("Image"),
        palette.on_surface_variant,
        palette,
        move |window, cx| {
            let _language = mandara_i18n::scoped(Some(language));
            let Some(lease) = acquire(cx) else {
                flash.show(false, t("An image is already being prepared"));
                cx.notify(notified);
                return;
            };
            let doc = match make(cx, window) {
                Ok(doc) => doc,
                Err(error) => {
                    flash.show(false, error);
                    cx.notify(notified);
                    return;
                }
            };
            flash.show(true, t("Preparing the complete image…"));
            let token = lease.token();
            let (progress, events) = async_channel::bounded::<()>(1);
            let task = cx.background_executor().spawn(async move {
                let _language = mandara_i18n::scoped(Some(language));
                let result = render::png(&doc, || {
                    if !token.current() || progress.is_closed() {
                        return false;
                    }
                    let _ = progress.try_send(());
                    true
                });
                drop(doc);
                mandara_services::memory::request_trim();
                result
            });
            let (panel, alive, flash, notified) = (
                view.panel.clone(),
                Rc::downgrade(&view.life),
                flash.clone(),
                notified,
            );
            cx.notify(notified);
            cx.spawn(async move |cx| {
                let token = lease.token();
                while events.recv().await.is_ok() {
                    if panel.progress() <= 0.0 || alive.upgrade().is_none() {
                        token.cancel();
                        break;
                    }
                }
                drop(events);
                let result = task.await;
                if !token.current() || panel.progress() <= 0.0 || alive.upgrade().is_none() {
                    drop(result);
                    mandara_services::memory::request_trim();
                    return;
                }
                let result = match result {
                    Ok(png) => {
                        let copying = token.clone();
                        cx.background_executor()
                            .spawn(async move {
                                let _language = mandara_i18n::scoped(Some(language));
                                if !copying.current() {
                                    return Err(t("Image copying cancelled").into());
                                }
                                mandara_services::screenshot::copy_png(png)
                            })
                            .await
                    }
                    Err(error) => Err(error),
                };
                if panel.progress() > 0.0 && token.current() && alive.upgrade().is_some() {
                    cx.update(|cx| {
                        let _language = mandara_i18n::scoped(Some(language));
                        match result {
                            Ok(()) => flash.show(true, t("Complete image copied")),
                            Err(error) => flash.show(false, error),
                        }
                        cx.notify(notified);
                    });
                }
                drop(lease);
                mandara_services::memory::request_trim();
            })
            .detach();
        },
    )
    .aria_label(label)
    .role(gpui::Role::Button)
    .focusable()
    .tab_index(0)
    .into_any_element()
}

/// CLI exports use the same worker gate and clipboard acknowledgement as buttons.
pub(super) fn command(doc: Document, cx: &mut App) -> Result<mandara_plugins::Output, String> {
    command_job(move || Ok(doc), cx)
}

pub(super) fn command_job(
    make: impl FnOnce() -> Result<Document, String> + Send + 'static,
    cx: &mut App,
) -> Result<mandara_plugins::Output, String> {
    let lease = cx
        .default_global::<Exports>()
        .0
        .acquire()
        .ok_or_else(|| t("An image is already being prepared").to_string())?;
    let language = mandara_i18n::language();
    let (reply, result) = async_channel::bounded(1);
    cx.background_executor()
        .spawn(async move {
            let _language = mandara_i18n::scoped(Some(language));
            let token = lease.token();
            let image =
                make().and_then(|doc| render::png(&doc, || token.current() && !reply.is_closed()));
            let copied = image.and_then(|png| {
                if reply.is_closed() || !token.current() {
                    return Err(t("Image copying cancelled").into());
                }
                mandara_services::screenshot::copy_png(png)
            });
            drop(lease);
            mandara_services::memory::request_trim();
            let _ = reply.try_send(
                copied.map(|()| serde_json::json!({"copied":true,"mime":"image/png"}).to_string()),
            );
        })
        .detach();
    Ok(mandara_plugins::Output::Pending(result))
}
