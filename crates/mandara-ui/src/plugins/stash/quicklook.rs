//! Quick look: a big preview of one item on its own overlay surface,
//! centred on the shelf's output. A picture is decoded off the UI thread at
//! the preview's physical size (never full size); text is read up to a
//! bound. Everything it holds goes when it closes (Space, Escape, a click).

use std::sync::Arc;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    App, AppContext as _, Bounds, Context, FocusHandle, FontWeight, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, ObjectFit, ParentElement, Render, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _,
    Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, img, point, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{SurfaceRole, ToGpui as _};
use mandara_i18n::t;

use super::model::{Item, ItemKind};
use super::{entity, entity_ref, store, thumbs};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};

/// Most text shown, bytes.
const TEXT_MAX: usize = 256 * 1024;
const MAX_W: f32 = 980.0;
const MAX_H: f32 = 760.0;
const FOOT_H: f32 = 56.0;
/// The text preview's corners.
const TEXT_RADIUS: f32 = 12.0;

pub struct Handle(WindowHandle<QuickLook>);

enum Content {
    Loading,
    Picture(Arc<RenderImage>),
    Text(SharedString),
    Info,
    Failed(String),
}

pub struct QuickLook {
    shelf: u64,
    item: Item,
    content: Content,
    started: bool,
    focus: FocusHandle,
}

pub fn is_open(cx: &App) -> bool {
    entity_ref(cx).is_some_and(|e| e.read(cx).quicklook.is_some())
}

/// Closes the preview and releases its picture.
pub fn close(cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let Some(Handle(h)) = e.update(cx, |s, cx| {
        cx.notify();
        s.quicklook.take()
    }) else {
        return;
    };
    // Deferred: a key or click in the preview itself closes it.
    cx.defer(move |cx| {
        let image = h.update(cx, |v, window, _| {
            window.remove_window();
            match std::mem::replace(&mut v.content, Content::Info) {
                Content::Picture(i) => Some(i),
                _ => None,
            }
        });
        if let Ok(Some(image)) = image {
            cx.drop_image(image, None);
            mandara_services::memory::request_trim();
        }
    });
}

/// Opens the preview of `item` (replacing one that is open).
pub fn open(shelf: u64, item: u64, cx: &mut App) {
    close(cx);
    let Some(e) = entity_ref(cx) else { return };
    let (it, output) = {
        let s = e.read(cx);
        let Some(sh) = s.state.shelf(shelf) else { return };
        let Some(it) = sh.item(item).cloned() else { return };
        (it, sh.output.clone())
    };
    let out = output.as_deref().map_or_else(|| crate::overlays::primary_output_size(cx), |o| super::output_size(o, cx));
    let (w, h) = ((out.0 * 0.7).min(MAX_W), (out.1 * 0.75).min(MAX_H));
    let display = output.as_deref().and_then(|o| crate::outputs::display_for(o, cx));
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(w), px(h)) })),
        focus: false,
        show: true,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: SurfaceRole::Stash.namespace().to_string(),
            layer: Layer::Overlay,
            // Unanchored: the compositor centres it.
            anchor: Anchor::empty(),
            exclusive_zone: Some(px(super::shelf::IGNORE_ZONES)),
            exclusive_edge: None,
            margin: None,
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
        }),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(mandara_core::SHELL_APP_ID.to_string()),
        inactive_frame_interval: None,
        ..Default::default()
    };
    match cx.open_window(options, move |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            QuickLook { shelf, item: it, content: Content::Loading, started: false, focus }
        })
    }) {
        Ok(h) => e.update(cx, |s, _| s.quicklook = Some(Handle(h))),
        Err(err) => tracing::warn!("stash: quick look: {err:#}"),
    }
}

impl QuickLook {
    /// Starts loading what the preview shows, at `(w, h)` physical px.
    fn load(&mut self, (w, h): (u32, u32), cx: &mut Context<Self>) {
        self.started = true;
        let shelf = self.shelf;
        enum Job {
            Picture(std::path::PathBuf),
            Text(std::path::PathBuf),
        }
        let job = match &self.item.kind {
            ItemKind::Image { blob, mime, .. } if mime != "image/svg+xml" => Some(Job::Picture(store::blob_path(shelf, blob))),
            ItemKind::File { path, mime, .. } if mime.starts_with("image/") && mime != "image/svg+xml" => Some(Job::Picture(path.clone())),
            ItemKind::File { path, mime, dir: false } if mime.starts_with("text/") => Some(Job::Text(path.clone())),
            ItemKind::Text { blob, .. } => Some(Job::Text(store::blob_path(shelf, blob))),
            _ => None,
        };
        let Some(job) = job else {
            self.content = Content::Info;
            return;
        };
        let item = self.item.id;
        let task = cx.background_executor().spawn(async move {
            match job {
                Job::Picture(path) => thumbs::decode(&path, thumbs::Key { item, w, h, cover: false }).map(Content::Picture).unwrap_or_else(|| Content::Failed(t("This picture can't be shown").to_string())),
                Job::Text(path) => match store::read_head(&path, TEXT_MAX) {
                    Ok(bytes) => Content::Text(super::model::decode_text(&bytes).into()),
                    Err(e) => Content::Failed(e.to_string()),
                },
            }
        });
        cx.spawn(async move |this, cx| {
            let content = task.await;
            let late = this.update(cx, |v, cx| {
                v.content = content;
                cx.notify();
            });
            // Closed meanwhile: the picture goes too.
            if late.is_err() {
                mandara_services::memory::request_trim();
            }
        })
        .detach();
    }
}

impl Render for QuickLook {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        let viewport = window.viewport_size();
        let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
        if !self.started && w > 1.0 {
            let s = window.scale_factor();
            self.load((((w - 2.0 * tokens::LG) * s) as u32, ((h - FOOT_H - 2.0 * tokens::LG) * s) as u32), cx);
        }
        let title: SharedString = self.item.title().into();
        let detail: SharedString = match &self.item.kind {
            ItemKind::File { path, .. } => path.display().to_string().into(),
            ItemKind::Url { url, .. } => url.clone().into(),
            ItemKind::Image { source: Some(s), .. } => s.clone().into(),
            ItemKind::Data { mime, bytes, .. } => format!("{mime} · {}", super::shelf::human_bytes(*bytes)).into(),
            _ => SharedString::default(),
        };
        let body = match &self.content {
            Content::Picture(image) => img(ImageSource::Render(image.clone())).size_full().object_fit(ObjectFit::Contain).into_any_element(),
            Content::Text(text_) => div()
                .id("stash-ql-text")
                .size_full()
                .overflow_y_scroll()
                .p(px(tokens::LG))
                .rounded(px(TEXT_RADIUS))
                .bg(p.surface_container.hsla())
                .text_size(px(text::BODY))
                .text_color(p.on_surface.hsla())
                .child(text_.clone())
                .into_any_element(),
            Content::Loading => div().size_full().flex().items_center().justify_center().text_color(p.on_surface_variant.hsla()).child(t("Loading…")).into_any_element(),
            Content::Failed(why) => div().size_full().flex().items_center().justify_center().text_color(p.on_surface_variant.hsla()).child(why.clone()).into_any_element(),
            Content::Info => {
                let glyph = match &self.item.kind {
                    ItemKind::File { dir: true, .. } => "folder",
                    ItemKind::Url { .. } => "globe",
                    ItemKind::Image { .. } => "image",
                    _ => "box",
                };
                div().size_full().flex().flex_col().items_center().justify_center().gap(px(tokens::MD)).child(icon(glyph, 96.0, p.primary)).child(div().text_size(px(text::TITLE)).text_color(p.on_surface.hsla()).child(title.clone())).into_any_element()
            }
        };
        let shelf = self.shelf;
        let item = self.item.clone();
        div()
            .id("stash-ql")
            .font_family(cx.global::<crate::fonts::Fonts>().panels.clone())
            .text_start()
            .track_focus(&self.focus)
            .size_full()
            .rounded(px(crate::style::radius(22.0)))
            .bg(p.surface_container_low.with_alpha(0.98).hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(tokens::LG))
            .gap(px(tokens::MD))
            .on_key_down(|e: &KeyDownEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "escape" | "space" | "enter") {
                    close(cx);
                }
            })
            .on_click(|_, _, cx| close(cx))
            .child(div().flex_1().min_h(px(0.0)).child(body))
            .child(
                div()
                    .h(px(FOOT_H - tokens::MD))
                    .flex_none()
                    .row()
                    .items_center()
                    .gap(px(tokens::MD))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(title))
                            .child(div().truncate().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(detail)),
                    )
                    .child(crate::kit::icon_button("stash-ql-open", "external", 34.0, p.on_surface, Some(p.surface_container_high), move |_, cx| {
                        super::open_item(&item, shelf, cx);
                        close(cx);
                    }))
                    .child(crate::kit::icon_button("stash-ql-close", "close", 34.0, p.on_surface, Some(p.surface_container_high), |_, cx| close(cx))),
            )
            .when(entity(cx).read(cx).quicklook.is_none(), |el| el.opacity(0.0))
    }
}
