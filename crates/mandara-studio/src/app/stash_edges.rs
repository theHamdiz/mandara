//! Studio → Plugins → Stash → **Edge catchers**: one switch per screen
//! edge. An edge another feature uses (the bar, the sidebar's open-on-hover
//! edge, the launcher and power-menu edges, side bars, the dock) can't have
//! a catcher: turning one on there shows exactly what uses the edge, with
//! one-click fixes ("Turn that off", "Move it…") and a link to its setting,
//! and the catcher switches on only once the edge is free. Corners used by
//! hot corners (or the compositor's own) are listed too: catchers leave a
//! gap there.
//!
//! The reverse check: a Studio change that would take an edge with an
//! active catcher is held until you choose which one keeps the edge
//! ([`EdgesUi::hold_if_colliding`], the dialog in
//! [`StudioView::stash_collision_layer`]). Nothing is switched off without
//! asking, on either side.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::desktop::CornerAction;
use mandara_config::stash::{self as st, EdgeContext, EdgeFix, EdgeUse, FixValue, ScreenEdge};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use serde_json::Value;

use super::StudioView;
use super::compositor_page::button;
use crate::registry;
use crate::tokens::*;

/// Status chip padding, the dimmed switch of a busy edge, the box widths
/// and the warm wash of an edge being resolved.
const BADGE_PAD_Y_PX: f32 = 1.0;
const LOCKED_OPACITY: f32 = 0.45;
const CONTROL_W_PX: f32 = 560.0;
const DIALOG_W_PX: f32 = 460.0;
const WASH_ALPHA: f32 = 0.08;

/// A change waiting for you to choose which feature keeps an edge.
pub(super) struct Pending {
    next: Box<ShellConfig>,
    /// What the change was (`tk` text).
    what: &'static str,
    hits: Vec<(ScreenEdge, Vec<EdgeUse>)>,
}

#[derive(Default)]
pub(super) struct EdgesUi {
    /// The busy edge whose switch was just clicked: its users are listed.
    open: Option<ScreenEdge>,
    pending: Option<Pending>,
}

impl EdgesUi {
    /// Holds `next` back when it would take an edge with an active catcher
    /// (returns `true`; the dialog asks). Otherwise returns `false`.
    pub(super) fn hold_if_colliding(&mut self, before: &ShellConfig, next: ShellConfig, what: &'static str) -> bool {
        let hits = st::new_collisions(before, &next, context());
        if hits.is_empty() {
            return false;
        }
        self.pending = Some(Pending { next: Box::new(next), what, hits });
        true
    }
}

/// The compositor Studio runs under, for the edge rules.
pub(super) fn context() -> EdgeContext {
    use mandara_compositor::CompositorKind as K;
    let kind = mandara_compositor::detected();
    EdgeContext { niri: kind == Some(K::Niri), kwin: kind == Some(K::KWin) }
}

/// "Top-left corner: hot corner → Overview".
pub(super) fn corner_note(u: &st::CornerUse) -> String {
    let corner = match u.corner {
        mandara_config::desktop::ScreenCorner::TopLeft => t("Top-left corner"),
        mandara_config::desktop::ScreenCorner::TopRight => t("Top-right corner"),
        mandara_config::desktop::ScreenCorner::BottomLeft => t("Bottom-left corner"),
        mandara_config::desktop::ScreenCorner::BottomRight => t("Bottom-right corner"),
    };
    let what = match u.action {
        None => t("the compositor's own hot corner").to_string(),
        Some(a) => tf("hot corner → {action}", &[("action", &tr(action_label(a)))]),
    };
    tf("{corner}: {what}. Catchers leave a gap there.", &[("corner", &corner), ("what", &what)])
}

fn action_label(a: CornerAction) -> &'static str {
    use mandara_i18n::tk;
    match a {
        CornerAction::None => tk("Nothing"),
        CornerAction::Launcher => tk("Launcher"),
        CornerAction::Overview => tk("Overview"),
        CornerAction::Sidebar => tk("Sidebar"),
        CornerAction::Session => tk("Power menu"),
        CornerAction::Notifications => tk("Notifications"),
        CornerAction::Wallpapers => tk("Wallpapers"),
        CornerAction::Lock => tk("Lock"),
        CornerAction::Command => tk("Command"),
    }
}

fn fix_value(v: FixValue) -> Value {
    match v {
        FixValue::Bool(b) => Value::from(b),
        FixValue::Str(s) => Value::from(s),
    }
}

/// `cfg` with Stash's catcher on `edge` switched `on`.
pub(super) fn with_edge(cfg: &ShellConfig, edge: ScreenEdge, on: bool) -> ShellConfig {
    let mut next = cfg.clone();
    next.plugins.settings.entry(st::STASH_PLUGIN_ID.into()).or_default().insert(edge.setting_key().into(), toml::Value::Boolean(on));
    next
}

impl StudioView {
    /// Switches `edge`'s catcher: off always; on only when the edge is free
    /// (else its users are shown).
    fn set_stash_edge(&mut self, edge: ScreenEdge, on: bool, cx: &mut Context<Self>) {
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        if on && !st::edge_users(&cfg, edge, context()).is_empty() {
            self.stash_edges.open = Some(edge);
            cx.notify();
            return;
        }
        self.stash_edges.open = None;
        let next = with_edge(&cfg, edge, on);
        mandara_ui::state::update_config(cx, |c| *c = next);
        cx.notify();
    }

    /// Runs a one-click fix (through the registry, so its own checks apply).
    fn apply_edge_fix(&mut self, fix: EdgeFix, cx: &mut Context<Self>) {
        let Some(setting) = registry::by_path(fix.path) else {
            tracing::warn!("stash: no Studio setting {}", fix.path);
            return;
        };
        self.apply(setting, fix_value(fix.value), cx);
    }

    /// Shows the page with the feature's own setting.
    fn open_edge_setting(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(setting) = registry::by_path(path) else { return };
        self.close_plugin(cx);
        self.show_search(&tr(setting.label), cx);
    }

    /// The users of an edge, each with its fixes and a link to its setting.
    fn edge_users_list(&mut self, users: &[EdgeUse], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .children(users.iter().enumerate().map(|(i, u)| {
                let edge = u.edge;
                let link = u.setting;
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface.hsla()).child(mandara_ui::bidi::paragraph(tf("{edge}: {reason}", &[("edge", &tr(edge.label())), ("reason", &tr(u.reason))]))))
                    .child(
                        div()
                            .row()
                            .flex_wrap()
                            .gap(px(SPACE_6_PX))
                            .children(u.fixes.iter().enumerate().map(|(j, f)| {
                                let f = *f;
                                button(format!("stash-fix-{}-{i}-{j}", edge.id()), "check", tr(f.label), j == 0, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply_edge_fix(f, cx)))
                            }))
                            .child(button(format!("stash-link-{}-{i}", edge.id()), "external", t("Open that setting"), false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_edge_setting(link, cx)))),
                    )
            }))
            .into_any_element()
    }

    /// Studio's edge-catcher control.
    pub(super) fn stash_edges_control(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let ctx = context();
        let settings = st::StashSettings::from_config(cfg);
        let enabled = st::stash_enabled(cfg);
        let rows = ScreenEdge::ALL.map(|edge| {
            let on = settings.edge_on(edge);
            let users = st::edge_users(cfg, edge, ctx);
            let busy = !users.is_empty();
            let (status, color) = match (on, busy) {
                (true, false) if enabled => (t("Catching drags"), p.success_emerald),
                (true, false) => (t("On (Stash is off)"), p.on_surface_variant),
                (true, true) => (t("Inactive: this edge is in use"), p.warning_amber),
                (false, true) => (t("In use"), p.on_surface_variant),
                (false, false) => (t("Free"), p.on_surface_variant),
            };
            let expanded = (on && busy) || self.stash_edges.open == Some(edge);
            let list = expanded.then(|| self.edge_users_list(&users, p, cx));
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .p(px(SPACE_10_PX))
                .rounded(px(RADIUS_12_PX))
                .bg(if expanded { p.warning_amber.with_alpha(WASH_ALPHA).hsla() } else { p.surface_container.with_alpha(0.0).hsla() })
                .child(
                    div()
                        .id(SharedString::from(format!("stash-edge-{}", edge.id())))
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .cursor_pointer()
                        .child(div().flex_1().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(tr(edge.label())))
                        .child(div().px(px(SPACE_8_PX)).py(px(BADGE_PAD_Y_PX)).rounded_full().bg(color.with_alpha(ALPHA_14).hsla()).text_color(color.hsla()).text_size(px(TEXT_11_PX)).child(status))
                        .child(div().when(busy && !on, |el| el.opacity(LOCKED_OPACITY)).child(mandara_ui::plugins::network::switch(on, p)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_stash_edge(edge, !on, cx))),
                )
                .when(expanded && !on, |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Free this edge first; the catcher switches on once nothing else uses it.")))))
                .children(list)
        });
        let corners: Vec<String> = st::corner_users(cfg, ctx).iter().map(corner_note).collect();
        div()
            .max_w(px(CONTROL_W_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .children(rows)
            .children(corners.into_iter().map(|c| div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(c))))
            .into_any_element()
    }

    /// The reverse check's question: a change wants an edge a catcher holds.
    pub(super) fn stash_collision_layer(&mut self, _cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.stash_edges.pending.as_ref()?;
        let lines: Vec<String> = pending.hits.iter().flat_map(|(_, users)| users.iter().map(|u| tf("{edge}: {reason}", &[("edge", &tr(u.edge.label())), ("reason", &tr(u.reason))]))).collect();
        let what = tr(pending.what);
        Some(
            div()
                .id("stash-collision")
                .absolute()
                .inset_0()
                .bg(gpui::black().opacity(0.4))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w(px(DIALOG_W_PX))
                        .p(px(SPACE_20_PX))
                        .rounded(px(RADIUS_16_PX))
                        .bg(p.surface_container_high.hsla())
                        .row()
                        .flex_col()
                        .gap(px(SPACE_12_PX))
                        .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(t("This edge has a Stash catcher")))
                        .child(div().text_size(px(TEXT_13_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tf("“{change}” would use an edge where Stash catches drags. Which one keeps the edge?", &[("change", &what)]))))
                        .children(lines.into_iter().map(|l| div().text_size(px(TEXT_12_5_PX)).child(mandara_ui::bidi::paragraph(l))))
                        .child(
                            div()
                                .row()
                                .flex_wrap()
                                .gap(px(SPACE_8_PX))
                                .child(button("stash-keep-catcher", "stash", t("Keep the catcher"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.stash_edges.pending = None;
                                    cx.notify();
                                })))
                                .child(button("stash-give-edge", "check", t("Give the edge to the new setting"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(pending) = this.stash_edges.pending.take() {
                                        let mut next = *pending.next;
                                        for (edge, _) in &pending.hits {
                                            next = with_edge(&next, *edge, false);
                                        }
                                        mandara_ui::state::update_config(cx, |c| *c = next);
                                    }
                                    cx.notify();
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fix_and_link_is_a_studio_setting() {
        let mut cfg = ShellConfig::default();
        cfg.plugins.enabled.push(st::STASH_PLUGIN_ID.into());
        cfg.side_bars.enabled = true;
        cfg.side_bars.left.enabled = true;
        cfg.plugins.enabled.push(mandara_config::workflow::DOCK_PLUGIN_ID.into());
        let mut seen = 0;
        for c in [cfg.clone(), ShellConfig::default()] {
            for edge in ScreenEdge::ALL {
                for u in st::edge_users(&c, edge, EdgeContext::default()) {
                    assert!(registry::by_path(u.setting).is_some(), "{} links to a Studio setting", u.setting);
                    for f in &u.fixes {
                        let setting = registry::by_path(f.path).unwrap_or_else(|| panic!("{} is a Studio setting", f.path));
                        registry::set(&c, setting, fix_value(f.value)).unwrap_or_else(|e| panic!("{}: {e}", f.path));
                        seen += 1;
                    }
                }
            }
        }
        assert!(seen >= 6);
    }

    #[test]
    fn a_change_onto_a_catchers_edge_is_held_and_others_pass() {
        let mut before = ShellConfig::default();
        before.plugins.enabled.push(st::STASH_PLUGIN_ID.into());
        before.sidebar.hover_reveal = false;
        let before = with_edge(&before, ScreenEdge::Left, true);
        let mut ui = EdgesUi::default();
        let mut after = before.clone();
        after.sidebar.hover_reveal = true;
        assert!(ui.hold_if_colliding(&before, after, mandara_i18n::tk("Open on hover")));
        let pending = ui.pending.as_ref().unwrap();
        assert_eq!(pending.hits[0].0, ScreenEdge::Left);
        let mut harmless = before.clone();
        harmless.bar.height += 1.0;
        let mut ui = EdgesUi::default();
        assert!(!ui.hold_if_colliding(&before, harmless, "x"));
        assert!(ui.pending.is_none());
    }
}
