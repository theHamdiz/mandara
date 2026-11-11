//! Studio → Bars → Sidebars: what each side edge holds, then the settings
//! of what is there. A side bar is a slim bar of items, like the top bar
//! standing on its side; the sidebar is the slide-out panel of tabs and
//! cards. The picker at the top gives each edge one of them or nothing
//! ([`mandara_config::side_edges`] keeps the rules), and the sections below
//! show only what applies: "Side bar on the left/right" (its settings and
//! layout designer) and "Sidebar panel" (its tabs, cards and look).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Div, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder as _, px, relative,
};
use mandara_config::{BarPosition, ShellConfig};
use mandara_config::side_edges::{EdgeBlock, EdgeUse};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::super::StudioView;
use super::super::compositor_page::{button, card, heading};
use crate::registry::bars::{BarsTab, SidebarsPart};
use crate::registry::{self, Section, Setting};
use crate::tokens::*;

/// Switches the picker stands for; the sections leave them out.
const EDGE_PATHS: [&str; 5] = ["/side_bars/enabled", "/side_bars/left/enabled", "/side_bars/right/enabled", "/sidebar/enabled", "/sidebar/side"];

/// Height of an option's little screen.
const PREVIEW_H_PX: f32 = DIM_56_PX;
/// The drawn main bar's thickness, the side bar's width and the panel's
/// share of the screen in the previews.
const PREVIEW_BAR_PX: f32 = DIM_5_PX;
const PREVIEW_SIDE_BAR_PX: f32 = DIM_6_PX;
const PREVIEW_PANEL_SHARE: f32 = 0.42;
/// Space round the drawn pieces inside a preview.
const PREVIEW_INSET_PX: f32 = SPACE_4_PX;
const PREVIEW_GAP_PX: f32 = SPACE_3_PX;
/// An option that can't be chosen right now.
const BLOCKED_ALPHA: f32 = ALPHA_45;

/// Deep links: frames to wait for a section's anchor before scrolling,
/// the re-check delay meanwhile, when to give up (the section isn't
/// drawn), and the space kept above it.
const ANCHOR_FRAMES: u8 = 2;
const ANCHOR_SETTLE: Duration = Duration::from_millis(32);
const ANCHOR_GIVE_UP: u8 = 60;
const SCROLL_MARGIN_PX: f32 = SPACE_12_PX;

/// A deep-linked section to scroll to, and where it was painted. Anchors
/// are recorded only while a scroll waits.
#[derive(Default)]
struct Nav {
    pending: Option<(SidebarsPart, u8)>,
    anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
}
impl Global for Nav {}

fn nav(cx: &mut gpui::App) -> &mut Nav {
    cx.default_global::<Nav>()
}

/// Drops a deep link's scroll that hasn't happened yet: picking a tab by
/// hand starts it at the top, and a later visit never jumps.
pub(super) fn forget_scroll(cx: &mut gpui::App) {
    if cx.has_global::<Nav>() {
        let nav = cx.global_mut::<Nav>();
        nav.pending = None;
        nav.anchor.set(None);
    }
}

/// The sidebar's look the power dock (attachment, margin, corners,
/// opacity) and the cards in panels (radius, padding) share, so it stays
/// while the sidebar is off.
const SHARED_LOOK: [&str; 6] = ["/sidebar/look/attach", "/sidebar/look/margin", "/sidebar/look/radius", "/sidebar/look/opacity", "/sidebar/look/card_radius", "/sidebar/look/card_padding"];

/// The settings shown in `part` under `cfg`: the edge switches stay with
/// the picker, a side bar's own settings show while it does, and with the
/// sidebar off only the look others share remains.
fn shows(setting: &Setting, part: SidebarsPart, cfg: &ShellConfig) -> bool {
    let path = setting.path;
    if BarsTab::for_path(path) != Some(BarsTab::Sidebars) || SidebarsPart::for_path(path) != Some(part) || EDGE_PATHS.contains(&path) {
        return false;
    }
    match part {
        SidebarsPart::SideBar => {
            let left = path.starts_with("/side_bars/left/");
            let right = path.starts_with("/side_bars/right/");
            (!left || cfg.side_bar_on(false)) && (!right || cfg.side_bar_on(true)) && (cfg.side_bar_on(false) || cfg.side_bar_on(true))
        }
        SidebarsPart::Panel => cfg.sidebar_active() || SHARED_LOOK.contains(&path),
    }
}

/// The side bar section's title for the edges holding one.
fn side_bar_title(left: bool, right: bool) -> &'static str {
    match (left, right) {
        (true, true) => tk("Side bars on the left and right"),
        (true, false) => tk("Side bar on the left"),
        (false, true) => tk("Side bar on the right"),
        (false, false) => tk("Side bar"),
    }
}

impl StudioView {
    /// Opens Bars → Sidebars scrolled to `part` (`mandara studio
    /// bars/sidebars/<side-bar|sidebar>`, and the former tabs' routes).
    pub fn show_sidebars_part(&mut self, part: SidebarsPart, cx: &mut Context<Self>) {
        self.show_bars_tab(BarsTab::Sidebars, cx);
        let nav = nav(cx);
        nav.pending = Some((part, 0));
        nav.anchor.set(None);
        cx.notify();
    }

    /// Scrolls to the waiting section once it has been laid out; gives up
    /// when it never draws.
    fn sidebars_scroll(&mut self, cx: &mut Context<Self>) {
        let Some((part, frames)) = nav(cx).pending else { return };
        let anchor = nav(cx).anchor.get();
        let viewport = self.content_scroll.bounds();
        match anchor {
            Some(b) if frames >= ANCHOR_FRAMES && viewport.size.height > px(0.0) => {
                let offset = self.content_scroll.offset();
                let max = self.content_scroll.max_offset();
                let y = (offset.y - (b.top() - viewport.top()) + px(SCROLL_MARGIN_PX)).min(px(0.0)).max(-max.y);
                self.content_scroll.set_offset(gpui::point(offset.x, y));
                nav(cx).pending = None;
                cx.notify();
            }
            _ if frames >= ANCHOR_GIVE_UP || self.bars_tab != BarsTab::Sidebars => nav(cx).pending = None,
            _ => {
                nav(cx).pending = Some((part, frames.saturating_add(1)));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(ANCHOR_SETTLE).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
    }

    /// The whole tab: the picker, then each section.
    pub(super) fn sidebars_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.sidebars_scroll(cx);
        let waiting = nav(cx).pending.map(|(part, _)| part);
        let anchor = nav(cx).anchor.clone();
        let anchored = move |part: SidebarsPart, el: AnyElement| -> AnyElement {
            if waiting != Some(part) {
                return el;
            }
            let anchor = anchor.clone();
            div().relative().child(el).child(canvas(move |bounds, _, _| anchor.set(Some(bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()).into_any_element()
        };
        let picker = self.edge_picker(cfg, p, cx);
        let side_bar = self.side_bar_section(cfg, p, window, cx);
        let panel = self.panel_section(cfg, p, window, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(picker)
            .child(anchored(SidebarsPart::SideBar, side_bar))
            .child(anchored(SidebarsPart::Panel, panel))
            .into_any_element()
    }

    /// What each edge holds: three illustrated choices per edge, with the
    /// reason beside any that can't be taken.
    fn edge_picker(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let edges = div()
            .ltr_row()
            .when(self.stack_rows, |el| el.flex_col())
            .gap(px(SPACE_14_PX))
            .child(self.edge_column(false, cfg, p, cx))
            .child(self.edge_column(true, cfg, p, cx));
        let dock = cfg.sidebar_active().then(|| note("info", t("The power dock opens from the edge opposite the sidebar."), p));
        card(p)
            .child(heading(t("What each edge holds"), t("A side bar is a slim bar of items, like the top bar standing on its side. The sidebar is the panel that slides out with tabs and cards. Each edge holds one of them, or nothing."), p))
            .child(edges)
            .children(dock)
            .into_any_element()
    }

    /// One edge: its title, the three choices and any note.
    fn edge_column(&mut self, right: bool, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let current = cfg.edge_use(right);
        let options: Vec<AnyElement> = EdgeUse::ALL.into_iter().map(|choice| option(right, choice, current, cfg, p)).collect();
        let mut notes: Vec<AnyElement> = Vec::new();
        if let Some(style) = cfg.edge_rail(right) {
            let name = super::super::designer::preset_name(style).map_or_else(|| style.id().to_string(), tr);
            let text = tf("This side bar is the rail of the {preset} preset. It changes with the preset.", &[("preset", &name)]);
            notes.push(self.note_with_link(if right { "edge-rail-right" } else { "edge-rail-left" }, text, t("Bar presets").into(), p, cx));
        } else if cfg.edge_allows(right, EdgeUse::SideBar) == Err(EdgeBlock::BarHere) {
            let text = t("The bar itself stands on this edge, so a side bar can't. The sidebar slides out beside it.");
            notes.push(self.note_with_link(if right { "edge-bar-right" } else { "edge-bar-left" }, text.into(), t("Bar position").into(), p, cx));
        }
        if current == EdgeUse::Sidebar && cfg.sidebar.side == mandara_config::SidebarSide::Auto {
            notes.push(note("globe", t("Placed by the interface language: left, or right for right-to-left languages."), p));
        }
        div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(if right { t("Right edge") } else { t("Left edge") }))
            .child(div().ltr_row().gap(px(SPACE_8_PX)).children(options))
            .children(notes)
            .into_any_element()
    }

    /// A note with a button that opens the Bar tab (presets and position).
    fn note_with_link(&mut self, id: &'static str, text: String, action: SharedString, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_8_PX))
            .child(div().flex_1().min_w(px(DIM_150_PX)).child(note("sidebar", text, p)))
            .child(button(id, "window", action, false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_bars_tab(BarsTab::Bar, cx))))
            .into_any_element()
    }

    /// "Side bar on the left/right": its settings and layout designer, or
    /// how to get one.
    fn side_bar_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (left, right) = (cfg.side_bar_on(false), cfg.side_bar_on(true));
        let title = tr(side_bar_title(left, right));
        if !left && !right {
            return section(section_head("sidebar", title, t("No edge holds a side bar. Choose Side bar for an edge above to set one up."), p), Vec::new());
        }
        let rows = self.rows(registry::in_section(Section::Bars).filter(|s| shows(s, SidebarsPart::SideBar, cfg)), cfg, p, cx);
        let designer = self.side_bars_designer(cfg, p, window, cx);
        section(section_head("sidebar", title, t("Its items, shape and look. Drag plugins into its groups below."), p), vec![rows, designer])
    }

    /// "Sidebar panel": its tabs and cards, how it behaves and looks; with
    /// it off, the look it shares with the power dock.
    fn panel_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows(registry::in_section(Section::Bars).filter(|s| shows(s, SidebarsPart::Panel, cfg)), cfg, p, cx);
        if !cfg.sidebar_active() {
            return section(section_head("grid", t("Sidebar panel"), t("No edge holds the sidebar. The settings below also shape the power dock and the cards in panels."), p), vec![rows]);
        }
        let line = if cfg.sidebar_on_right() { t("Slides out from the right edge: its tabs and cards, how it behaves and how it looks.") } else { t("Slides out from the left edge: its tabs and cards, how it behaves and how it looks.") };
        let tabs = self.sidebar_tabs_editor(cfg, p, window, cx);
        section(section_head("grid", t("Sidebar panel"), line, p), vec![tabs, rows])
    }
}

/// One choice for an edge: a little screen showing it, its name and a
/// line. The current choice is outlined; a blocked one is dimmed and
/// does nothing (the note under the edge says why).
fn option(right: bool, choice: EdgeUse, current: EdgeUse, cfg: &ShellConfig, p: &MandaraPalette) -> AnyElement {
    let on = choice == current;
    let blocked = !on && cfg.edge_allows(right, choice).is_err();
    let blurb = match choice {
        EdgeUse::Nothing => tk("Leave the edge free"),
        EdgeUse::SideBar => tk("A slim bar of items"),
        EdgeUse::Sidebar if !on && cfg.sidebar_active() => tk("Moves here from the other edge"),
        EdgeUse::Sidebar => tk("Slides out with tabs and cards"),
    };
    let name = match choice {
        EdgeUse::Nothing => tk("Nothing"),
        EdgeUse::SideBar => tk("Side bar"),
        EdgeUse::Sidebar => tk("Sidebar"),
    };
    div()
        .id(("edge-option", usize::from(right) * EdgeUse::ALL.len() + choice as usize))
        .flex_1()
        .min_w(px(0.0))
        .rounded(px(RADIUS_14_PX))
        .p(px(SPACE_8_PX))
        .row()
        .flex_col()
        .gap(px(SPACE_6_PX))
        .bg(if on { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container_high.hsla() })
        .border_2()
        .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
        .when(blocked, |el| el.opacity(BLOCKED_ALPHA))
        .when(!on && !blocked, |el| {
            el.cursor_pointer().hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla())).on_click(move |_: &ClickEvent, _, cx| {
                update_config(cx, |c| {
                    // Refused only if the config changed meanwhile; then
                    // nothing changes.
                    let _ = c.set_edge_use(right, choice);
                });
            })
        })
        .child(preview(right, choice, cfg.bar.position, p))
        .child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::SEMIBOLD).child(mandara_ui::bidi::paragraph(tr(name))))
        .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(blurb))))
        .into_any_element()
}

/// A little screen with the main bar where it stands and the choice drawn
/// on its edge (physical sides: never mirrored).
fn preview(right: bool, choice: EdgeUse, bar: BarPosition, p: &MandaraPalette) -> AnyElement {
    let inset = px(PREVIEW_INSET_PX);
    let piece = p.surface_container_high.hsla();
    let accent = p.primary.with_alpha(ALPHA_75).hsla();
    let dot = || div().flex_none().size(px(DIM_5_PX)).rounded_full().bg(accent);
    let line = |w: f32| div().h(px(DIM_5_PX)).w(relative(w)).rounded_full().bg(p.on_surface.with_alpha(ALPHA_12).hsla());
    // The free area: inside the inset, clear of the main bar.
    let beside_bar = |edge: BarPosition| PREVIEW_INSET_PX + if bar == edge { PREVIEW_BAR_PX + PREVIEW_GAP_PX } else { 0.0 };
    let (top, bottom) = (px(beside_bar(BarPosition::Top)), px(beside_bar(BarPosition::Bottom)));
    let (left, far) = (beside_bar(BarPosition::Left), beside_bar(BarPosition::Right));
    let (near, other) = if right { (far, left) } else { (left, far) };
    let at_edge = |el: Div| if right { el.right(px(near)) } else { el.left(px(near)) };
    let main_bar = {
        let el = div().absolute().rounded_full().bg(piece);
        match bar {
            BarPosition::Top => el.top(inset).left(inset).right(inset).h(px(PREVIEW_BAR_PX)),
            BarPosition::Bottom => el.bottom(inset).left(inset).right(inset).h(px(PREVIEW_BAR_PX)),
            BarPosition::Left => el.left(inset).top(inset).bottom(inset).w(px(PREVIEW_BAR_PX)),
            BarPosition::Right => el.right(inset).top(inset).bottom(inset).w(px(PREVIEW_BAR_PX)),
        }
    };
    // A window filling the free space beside a side bar.
    let window_near = near + if choice == EdgeUse::SideBar { PREVIEW_SIDE_BAR_PX + PREVIEW_GAP_PX } else { 0.0 };
    let window = {
        let el = div().absolute().top(top).bottom(bottom).rounded(px(RADIUS_4_PX)).bg(p.on_surface.with_alpha(ALPHA_6).hsla());
        if right { el.left(px(other)).right(px(window_near)) } else { el.right(px(other)).left(px(window_near)) }
    };
    let held = match choice {
        EdgeUse::Nothing => None,
        EdgeUse::SideBar => Some(
            at_edge(div().absolute().top(top).bottom(bottom).w(px(PREVIEW_SIDE_BAR_PX)))
                .rounded_full()
                .bg(piece)
                .py(px(PREVIEW_GAP_PX))
                .flex()
                .flex_col()
                .items_center()
                .justify_between()
                .child(dot())
                .child(dot())
                .child(dot()),
        ),
        EdgeUse::Sidebar => Some(
            at_edge(div().absolute().top(top).bottom(bottom).w(relative(PREVIEW_PANEL_SHARE)))
                .rounded(px(RADIUS_5_PX))
                .bg(piece)
                .shadow_sm()
                .p(px(PREVIEW_GAP_PX))
                .flex()
                .flex_col()
                .gap(px(PREVIEW_GAP_PX))
                .child(div().flex().gap(px(PREVIEW_GAP_PX)).child(dot()).child(dot()).child(dot()))
                .child(line(1.0))
                .child(line(0.7)),
        ),
    };
    div()
        .relative()
        .h(px(PREVIEW_H_PX))
        .w_full()
        .rounded(px(RADIUS_8_PX))
        .overflow_hidden()
        .bg(p.surface_base.hsla())
        .child(window)
        .child(main_bar)
        .children(held)
        .into_any_element()
}

/// A short explanation with an icon.
fn note(glyph: &'static str, text: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .items_start()
        .gap(px(SPACE_8_PX))
        .text_size(px(TEXT_11_5_PX))
        .line_height(px(LEADING_16_PX))
        .text_color(p.on_surface_variant.hsla())
        .child(div().pt(px(SPACE_3_PX / 2.0)).child(icon(glyph, ICON_13_PX, p.on_surface_variant)))
        .child(div().flex_1().min_w(px(0.0)).child(mandara_ui::bidi::paragraph(text.into())))
        .into_any_element()
}

/// A section's title row: an icon tile, the title and a line under it.
fn section_head(glyph: &'static str, title: impl Into<SharedString>, line: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .child(div().flex_none().size(px(DIM_34_PX)).rounded(px(RADIUS_12_PX)).bg(p.primary.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_16_PX, p.primary)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(SPACE_3_PX))
                .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(line.into()))),
        )
        .into_any_element()
}

fn section(head: AnyElement, body: Vec<AnyElement>) -> AnyElement {
    div().row().flex_col().gap(px(SPACE_14_PX)).child(head).children(body).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::SidebarSide;

    fn shown(part: SidebarsPart, cfg: &ShellConfig) -> Vec<&'static str> {
        registry::in_section(Section::Bars).filter(|s| shows(s, part, cfg)).map(|s| s.path).collect()
    }

    #[test]
    fn each_section_shows_only_what_applies_to_the_choice() {
        let mut cfg = ShellConfig::default();
        cfg.sidebar.side = SidebarSide::Right;
        // Nothing on either edge: no side bar settings, only the look the
        // power dock shares.
        cfg.set_edge_use(true, EdgeUse::Nothing).unwrap();
        assert!(shown(SidebarsPart::SideBar, &cfg).is_empty());
        let panel = shown(SidebarsPart::Panel, &cfg);
        assert_eq!(panel.len(), SHARED_LOOK.len(), "{panel:?}");
        assert!(panel.iter().all(|p| SHARED_LOOK.contains(p)), "{panel:?}");

        // A side bar on the left: its own settings and the shared ones, not
        // the right one's.
        cfg.set_edge_use(false, EdgeUse::SideBar).unwrap();
        let side = shown(SidebarsPart::SideBar, &cfg);
        assert!(side.contains(&"/side_bars/left/pinned") && side.contains(&"/side_bars/width"));
        assert!(!side.iter().any(|p| p.starts_with("/side_bars/right/")), "{side:?}");
        assert!(side.iter().all(|p| !EDGE_PATHS.contains(p)), "the picker owns the switches");

        // The sidebar on the right: its tabs, behaviour and look.
        cfg.set_edge_use(true, EdgeUse::Sidebar).unwrap();
        let panel = shown(SidebarsPart::Panel, &cfg);
        assert!(panel.contains(&"/sidebar/mode") && panel.contains(&"/sidebar/width") && panel.contains(&"/sidebar/look/radius"));
        assert!(!panel.contains(&"/sidebar/enabled") && !panel.contains(&"/sidebar/side"));
        assert!(!panel.contains(&"/sidebar/hover_reveal"), "hover stays on Edges & corners");
        assert!(!shown(SidebarsPart::SideBar, &cfg).iter().any(|p| p.starts_with("/side_bars/right/")));
    }

    #[test]
    fn every_sidebars_setting_shows_somewhere_once_both_are_chosen() {
        // A side bar on the left and the sidebar on the right, then the
        // other way round: between them every setting has its place.
        let mut cfg = ShellConfig::default();
        cfg.set_edge_use(false, EdgeUse::SideBar).unwrap();
        cfg.set_edge_use(true, EdgeUse::Sidebar).unwrap();
        let mut mirrored = ShellConfig::default();
        mirrored.set_edge_use(false, EdgeUse::Sidebar).unwrap();
        mirrored.set_edge_use(true, EdgeUse::SideBar).unwrap();
        for s in registry::in_section(Section::Bars).filter(|s| BarsTab::for_path(s.path) == Some(BarsTab::Sidebars) && !EDGE_PATHS.contains(&s.path)) {
            let seen = SidebarsPart::ALL.into_iter().any(|part| shows(s, part, &cfg) || shows(s, part, &mirrored));
            assert!(seen, "{} is never shown", s.path);
        }
    }

    #[test]
    fn titles_name_the_edges_holding_a_side_bar() {
        assert_eq!(side_bar_title(true, false), "Side bar on the left");
        assert_eq!(side_bar_title(false, true), "Side bar on the right");
        assert_eq!(side_bar_title(true, true), "Side bars on the left and right");
    }
}
