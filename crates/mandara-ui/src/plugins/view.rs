//! Closure-backed plugin views and their data subscriptions.

use std::time::Duration;

use gpui::{AnyElement, AnyView, App, AppContext as _, Context, IntoElement, Render, Subscription, Window};
use mandara_theme::MandaraPalette;
use crate::hub::Domains;

/// A view produced by a closure. Theme, configuration and shell state stay
/// reactive; model-owned views receive data changes from their own model.
pub struct FnView {
    render: Box<dyn Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement>,
    /// Set once the render closure panicked (it is never called again).
    failed: bool,
    /// Where the view was built (`media.rs:629`), for `diagnostics views`.
    label: &'static std::panic::Location<'static>,
    /// What it observes. Kept, not detached: the shell's long-lived
    /// entities (theme, config, hub) would otherwise keep one dead
    /// callback per dropped view until they next change, and sidebar
    /// cards are built every time the sidebar opens.
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
enum Updates { Hub, Domains(Domains), Model }

impl FnView {
    #[track_caller]
    pub fn new(
        tick: Option<Duration>,
        render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_updates(tick, Updates::Hub, render, cx)
    }

    /// Model views already receive their data notifications through `ViewSet`.
    #[track_caller]
    fn with_updates(
        tick: Option<Duration>, updates: Updates,
        render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        // At most six: hub, productivity, recorder, theme, config, shell
        // (plus one a caller adds), reserved once.
        let mut subscriptions = Vec::with_capacity(7);
        if matches!(updates, Updates::Hub) {
            let hub = crate::hub::hub(cx);
            subscriptions.push(cx.observe(&hub, |_, _, cx| cx.notify()));
            if let Some(p) = cx.try_global::<crate::productivity::GlobalProductivity>().map(|g| g.0.clone()) {
                subscriptions.push(cx.observe(&p, |_, _, cx| cx.notify()));
            }
            if let Some(r) = crate::recorder::recorder(cx) {
                subscriptions.push(cx.observe(&r, |_, _, cx| cx.notify()));
            }
        }
        if let Updates::Domains(domains) = updates {
            let hub = crate::hub::hub(cx);
            let mut revision = hub.read(cx).revision(domains);
            subscriptions.push(cx.observe(&hub, move |_, hub, cx| {
                let next = hub.read(cx).revision(domains);
                if next != revision {
                    revision = next;
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&crate::state::shell(cx), |_, _, cx| cx.notify()));
        // A visible render owns the next tick. Once hidden, its last pending
        // tick expires without starting another timer or retaining the view.
        let render: Box<dyn Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement> = match tick {
            Some(every) => Box::new(move |palette, window, cx| {
                super::panel_fx::redraw_in(every, window, cx);
                render(palette, window, cx)
            }),
            None => Box::new(render),
        };
        let label = std::panic::Location::caller();
        Self { render, failed: false, label, _subscriptions: subscriptions }
    }
}

impl FnView {
    /// Also re-render whenever `source` notifies, for as long as this view
    /// lives ([`super::follow`]).
    pub fn observe_too<T: 'static>(&mut self, source: &gpui::Entity<T>, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.observe(source, |_, _, cx| cx.notify()));
    }
}

impl Render for FnView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        use mandara_core::ToGpui as _;
        let palette = mandara_core::frame_palette(window, cx);
        if !self.failed {
            let started = std::time::Instant::now();
            let rendered = mandara_plugins::guard("plugin view", || (self.render)(&palette, window, cx));
            super::diagnostics::record_view(self.label, started.elapsed());
            match rendered {
                Some(el) => return el,
                // Stop calling a view that panicked; show a quiet marker.
                None => self.failed = true,
            }
        }
        gpui::div()
            .flex()
            .items_center()
            .gap(gpui::px(crate::kit::tokens::XS))
            .text_size(gpui::px(crate::kit::text::LABEL))
            .text_color(palette.urgent_coral.hsla())
            .child(crate::icons::icon("close", crate::kit::text::BODY_S, palette.urgent_coral))
            .child(mandara_i18n::t("plugin error"))
            .into_any_element()
    }
}

/// Convenience: builds an [`FnView`] as an `AnyView`.
#[track_caller]
pub fn fn_view(
    tick: Option<Duration>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    _window: &mut Window,
    cx: &mut App,
) -> AnyView {
    build(tick, Updates::Hub, |_| None, render, cx)
}

/// [`fn_view`] that also re-renders on what `observe` subscribes it to
/// (an entity only this view cares about, such as a running clock). The
/// returned subscription lives as long as the view.
#[track_caller]
pub fn fn_view_observing(
    tick: Option<Duration>,
    observe: impl FnOnce(&mut Context<FnView>) -> Option<Subscription>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    _window: &mut Window,
    cx: &mut App,
) -> AnyView {
    build(tick, Updates::Hub, observe, render, cx)
}

/// A view whose data comes from its own model, independent of hub services.
/// `ViewSet` or explicit observers must notify it when that data changes.
#[track_caller]
pub fn model_view(
    tick: Option<Duration>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    _window: &mut Window,
    cx: &mut App,
) -> AnyView {
    build(tick, Updates::Model, |_| None, render, cx)
}

/// Follow the selected hub services and the view's own model callbacks.
#[track_caller]
pub fn domain_view(
    domains: Domains,
    tick: Option<Duration>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    _window: &mut Window,
    cx: &mut App,
) -> AnyView {
    build(tick, Updates::Domains(domains), |_| None, render, cx)
}

/// A service-scoped view with an additional model-specific observer (kept
/// as long as the view).
#[track_caller]
pub fn domain_view_observing(
    domains: Domains,
    tick: Option<Duration>,
    observe: impl FnOnce(&mut Context<FnView>) -> Option<Subscription>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    _window: &mut Window,
    cx: &mut App,
) -> AnyView {
    build(tick, Updates::Domains(domains), observe, render, cx)
}

#[track_caller]
fn build(
    tick: Option<Duration>, updates: Updates,
    observe: impl FnOnce(&mut Context<FnView>) -> Option<Subscription>,
    render: impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static,
    cx: &mut App,
) -> AnyView {
    let label = std::panic::Location::caller();
    cx.new(|cx| {
        let mut view = FnView::with_updates(tick, updates, render, cx);
        view.label = label;
        view._subscriptions.extend(observe(cx));
        view
    }).into()
}
