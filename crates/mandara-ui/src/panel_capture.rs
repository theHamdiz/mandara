//! Shared camera control. A visible panel owns its cancellable capture and feedback.

use gpui::{AnyElement, App, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, StatefulInteractiveElement as _, Window, div};
use mandara_i18n::{t, tf};
use mandara_services::screencopy::panel::Request;
use mandara_theme::{Color, MandaraPalette};
use crate::plugins::{design_share::{self, jobs::Token}, extras::Flash};

/// Drop the observation with the panel so no old capture entity stays subscribed.
pub(crate) struct Owner {
    pub state: Entity<Capture>,
    _observed: gpui::Subscription,
}

impl Owner {
    pub fn new<T: 'static>(output: String, cx: &mut Context<T>) -> Self {
        let state = cx.new(|_| Capture::new(output));
        let observed = cx.observe(&state, |_, _, cx| cx.notify());
        Self { state, _observed: observed }
    }
}

#[derive(Default)]
pub(crate) struct Capture {
    output: String,
    rect: Option<(i32, i32, i32, i32)>,
    flash: Flash,
    cancel: Option<Token>,
    task: Option<gpui::Task<()>>,
    visibility: Option<gpui::Subscription>,
}

impl Drop for Capture {
    fn drop(&mut self) { if let Some(token) = self.cancel.take() { token.cancel(); } }
}

impl Capture {
    pub(crate) fn new(output: String) -> Self { Self { output, rect: None, flash: Flash::default(), cancel: None, task: None, visibility: None } }

    /// A small completion reply; pixel bytes never enter GPUI or a view cache.
    fn start(&mut self, cx: &mut Context<Self>) -> Result<async_channel::Receiver<Result<String, String>>, String> {
        if self.cancel.is_some() { return Err(t("An image is already being prepared").into()); }
        let rect = self.rect.ok_or_else(|| t("The panel is not visible").to_string())?;
        let Some(lease) = design_share::acquire(cx) else {
            self.flash.show(false, t("An image is already being prepared")); cx.notify();
            return Err(t("An image is already being prepared").into());
        };
        self.flash.clear();
        let request = Request { output: self.output.clone(), rect };
        let token = lease.token();
        self.cancel = Some(token.clone());
        let (reply, received) = async_channel::bounded(1);
        let language = mandara_i18n::language();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                let _lease = lease;
                let _language = mandara_i18n::scoped(Some(language));
                let result = mandara_services::screencopy::panel::run(request, || token.current()).map_err(|error| error.to_string());
                let result = result.and_then(|png| {
                    if !token.current() { return Err(t("Image copying cancelled").into()); }
                    mandara_services::screenshot::copy_png(png)
                });
                mandara_services::memory::request_trim();
                result
            }).await;
            let response = result.as_ref().map(|()| "{\"copied\":true,\"mime\":\"image/png\"}".to_string()).map_err(Clone::clone);
            let _ = this.update(cx, |this, cx| {
                this.cancel = None;
                match result {
                    Ok(()) => this.flash.show(true, t("Panel image copied")),
                    Err(error) => {
                        tracing::warn!("panel capture: {error}");
                        this.flash.show(false, tf("Panel capture failed: {error}", &[("error", &error)]));
                    }
                }
                cx.notify();
            });
            let _ = reply.try_send(response);
        }));
        cx.notify();
        Ok(received)
    }
}

/// Cover fractional edges and clip to this output, never a neighbouring window.
fn rectangle(bounds: Bounds<Pixels>, surface: (f32, f32)) -> Option<(i32, i32, i32, i32)> {
    let (x, y, w, h) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y), f32::from(bounds.size.width), f32::from(bounds.size.height));
    if ![x, y, w, h, surface.0, surface.1].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 { return None; }
    let x0 = x.floor().max(0.0).min(surface.0);
    let y0 = y.floor().max(0.0).min(surface.1);
    let x1 = (x + w).ceil().max(0.0).min(surface.0);
    let y1 = (y + h).ceil().max(0.0).min(surface.1);
    (x1 > x0 && y1 > y0).then_some((x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

pub(crate) fn start(state: &Entity<Capture>, bounds: Bounds<Pixels>, surface: (f32, f32), cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    state.update(cx, |state, cx| { state.rect = rectangle(bounds, surface); state.start(cx) })
}

/// One camera button for every host-owned panel, including third-party views.
pub(crate) fn button(state: &Entity<Capture>, bounds: Bounds<Pixels>, surface: (f32, f32), size: f32, ink: Color, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let weak = state.downgrade();
    state.update(cx, |state, _| {
        state.rect = rectangle(bounds, surface);
        if state.visibility.is_none() {
            state.visibility = Some(window.observe_window_visibility(move |_, window, cx| {
                if !window.is_visible() {
                    let _ = weak.update(cx, |state, _| {
                        if let Some(token) = state.cancel.take() { token.cancel(); }
                        state.task = None;
                        state.flash.clear();
                    });
                }
            }));
        }
    });
    let (flash, busy) = { let state = state.read(cx); (state.flash.clone(), state.cancel.is_some()) };
    let feedback = flash.current(window, cx);
    let (glyph, color) = match feedback.as_ref() {
        Some((true, _)) => ("check", palette.success_emerald),
        Some((false, _)) => ("alert", palette.urgent_coral),
        None => ("camera", if busy { palette.primary } else { ink }),
    };
    let label = feedback.map_or_else(|| t("Copy panel image").into(), |(_, message)| message);
    let pressed = state.downgrade();
    let keyed = pressed.clone();
    div().id("panel-camera-control").tab_index(0).aria_label(label)
        .on_key_down(move |event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.keystroke.modifiers.modified() {
                let _ = keyed.update(cx, |state, cx| state.start(cx));
                cx.stop_propagation();
            }
        })
        .child(crate::kit::icon_button("panel-camera", glyph, size, color, None, move |_, cx| { let _ = pressed.update(cx, |state, cx| state.start(cx)); }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_rectangles_are_bounded_and_offscreen_panels_are_rejected() {
        let bounds = |x, y, w, h| Bounds { origin: gpui::point(gpui::px(x), gpui::px(y)), size: gpui::size(gpui::px(w), gpui::px(h)) };
        assert_eq!(rectangle(bounds(10.4, 20.8, 100.2, 200.3), (1920.0, 1080.0)), Some((10, 20, 101, 202)));
        assert_eq!(rectangle(bounds(-5.0, -3.0, 20.0, 10.0), (1920.0, 1080.0)), Some((0, 0, 15, 7)));
        assert_eq!(rectangle(bounds(2000.0, 0.0, 20.0, 10.0), (1920.0, 1080.0)), None);
        assert_eq!(rectangle(bounds(f32::NAN, 0.0, 20.0, 10.0), (1920.0, 1080.0)), None);
    }
    #[test]
    fn closing_a_panel_cancels_its_capture_without_reopening_the_worker_slot() {
        let gate = design_share::jobs::Gate::default(); let lease = gate.acquire().unwrap(); let token = lease.token();
        let mut state = Capture::new(String::new()); state.cancel = Some(token.clone());
        drop(state); assert!(!token.current()); assert!(gate.acquire().is_none());
        drop(lease); assert!(gate.acquire().is_some());
    }
}
