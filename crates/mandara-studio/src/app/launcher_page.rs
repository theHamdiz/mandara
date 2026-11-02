//! Studio → Launcher, the bespoke parts: the ready-made launcher looks
//! (Classic, Grid, Beacon; a card each with a miniature), your own names and icons for apps
//! (`[launcher.overrides]`) and a glance at what the ranking has learnt
//! (frecency from `launcher-usage.toml`). The providers, ranking, look and
//! launching options are plain registry rows above, under the placement
//! picker (a small screen diagram of the seven placements).

use mandara_ui::bidi::Bidi as _;
use std::rc::Rc;

use gpui::{AnyElement, AppContext as _, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_config::workflow::{LAUNCHER_LOOKS, LauncherEdge, LauncherLookPreset, LauncherPosition};
use mandara_i18n::{t, tf, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{button, card, group_title, heading, tip};
use crate::text_field::TextField;
use crate::tokens::*;

/// Most-used results shown.
const USAGE_ROWS: usize = 8;
/// Usage bar height.
const USAGE_BAR_PX: f32 = 6.0;

/// The placement diagram: a little screen with the bar along its edge and
/// a tile per placement (physical sides, also in right-to-left languages).
const SCREEN_W_PX: f32 = 300.0;
const SCREEN_H_PX: f32 = 180.0;
const SCREEN_RADIUS_PX: f32 = 14.0;
const BAR_STRIP_PX: f32 = 12.0;
/// Inset of the tiles from the diagram's sides and edges.
const TILE_INSET_PX: f32 = 10.0;
/// Gap between the bar strip and the tiles hanging from it.
const TILE_BAR_GAP_PX: f32 = 4.0;
const EDGE_TILE_W_PX: f32 = 78.0;
const EDGE_TILE_H_PX: f32 = 40.0;
const CENTER_TILE_W_PX: f32 = 112.0;
const CENTER_TILE_H_PX: f32 = 58.0;
const TILE_RADIUS_PX: f32 = 9.0;
/// The launcher glyph inside each tile.
const TILE_ICON_PX: f32 = 14.0;

/// Launcher looks: card grid and the miniature drawn on each card (a
/// little screen with the launcher in it).
const LOOK_GRID: CardGrid = CardGrid { min_w: 190.0, max_w: 300.0, gap: SPACE_10_PX };
const MINI_H_PX: f32 = 104.0;
const MINI_RADIUS_PX: f32 = 10.0;
/// Classic: a drawer at the bottom (width share, rows, row and search height).
const MINI_DRAWER_W: f32 = 0.56;
const MINI_ROWS: usize = 3;
const MINI_ROW_H_PX: f32 = 9.0;
const MINI_SEARCH_H_PX: f32 = 10.0;
/// Grid: the card's width share, columns × rows of icons and their size.
const MINI_CARD_W: f32 = 0.66;
const MINI_GRID: (usize, usize) = (5, 2);
const MINI_ICON_PX: f32 = 11.0;
const MINI_CHIP_W_PX: f32 = 16.0;
const MINI_CHIP_H_PX: f32 = 5.0;
/// Beacon: the bar's width share and height, its round mode buttons, the
/// results card's rows, and how far down the bar sits.
const MINI_BAR_W: f32 = 0.5;
const MINI_BAR_H_PX: f32 = 13.0;
const MINI_MODES: usize = 4;
const MINI_BEACON_TOP: f32 = 0.16;
const MINI_LINE_H_PX: f32 = 3.0;

/// A miniature of a launcher look in this theme's colours.
fn look_mini(preset: &LauncherLookPreset, w: f32, p: &MandaraPalette) -> gpui::Div {
    use mandara_config::workflow::{LauncherLayout, LauncherStyle};
    let surface = p.surface_container_high;
    let line = |w: f32, c: mandara_theme::Color| div().h(px(MINI_LINE_H_PX)).w(px(w)).rounded_full().bg(c.hsla());
    let screen = div().relative().w(px(w)).h(px(MINI_H_PX)).flex_none().rounded(px(MINI_RADIUS_PX)).overflow_hidden().bg(p.surface_dim.hsla());
    match (preset.style, preset.layout) {
        (LauncherStyle::Beacon, _) => {
            let bar_w = w * MINI_BAR_W;
            let step = MINI_BAR_H_PX + SPACE_3_PX;
            let total = bar_w + MINI_MODES as f32 * step;
            let x0 = (w - total) / 2.0;
            let y0 = MINI_H_PX * MINI_BEACON_TOP;
            // The mode buttons sit at the bar's reading end (its left in
            // right-to-left languages).
            let rtl = mandara_ui::bidi::rtl();
            let bar_x = if rtl { x0 + total - bar_w } else { x0 };
            let mut s = screen.child(
                div().absolute().left(px(bar_x)).top(px(y0)).w(px(bar_w)).h(px(MINI_BAR_H_PX)).rounded_full().bg(surface.hsla()).row().items_center().px(px(SPACE_4_PX)).gap(px(SPACE_3_PX)).child(icon("search", MINI_LINE_H_PX * 2.0, p.on_surface_variant)).child(line(bar_w * 0.4, p.on_surface_variant.with_alpha(ALPHA_40))),
            );
            for i in 0..MINI_MODES {
                let x = if rtl { bar_x - (i as f32 + 1.0) * step } else { x0 + bar_w + SPACE_3_PX + i as f32 * step };
                s = s.child(div().absolute().left(px(x)).top(px(y0)).size(px(MINI_BAR_H_PX)).rounded_full().bg(surface.hsla()));
            }
            // The results card: a heading, the highlighted top hit, rows.
            let mut card = div().absolute().left(px(x0)).top(px(y0 + MINI_BAR_H_PX + SPACE_3_PX)).w(px(total)).p(px(SPACE_4_PX)).rounded(px(RADIUS_6_PX)).bg(surface.hsla()).row().flex_col().gap(px(SPACE_3_PX));
            card = card.child(line(total * 0.18, p.on_surface_variant.with_alpha(ALPHA_60)));
            card = card.child(div().h(px(MINI_ROW_H_PX)).w_full().rounded(px(RADIUS_3_PX)).bg(p.primary.hsla()).row().items_center().px(px(SPACE_3_PX)).child(line(total * 0.3, p.on_primary)));
            card = card.child(line(total * 0.18, p.on_surface_variant.with_alpha(ALPHA_60)));
            for k in 0..2 {
                card = card.child(div().h(px(MINI_ROW_H_PX)).row().items_center().px(px(SPACE_3_PX)).child(line(total * (0.42 - 0.1 * k as f32), p.on_surface.with_alpha(ALPHA_40))));
            }
            s.child(card)
        }
        (_, LauncherLayout::Grid) => {
            let cw = w * MINI_CARD_W;
            let (cols, rows) = MINI_GRID;
            let mut grid = div().row().flex_col().gap(px(SPACE_4_PX));
            for _ in 0..rows {
                grid = grid.child(div().row().justify_between().children((0..cols).map(|c| div().size(px(MINI_ICON_PX)).rounded(px(RADIUS_3_PX)).bg(if c == 0 { p.primary.hsla() } else { p.primary.with_alpha(ALPHA_30).hsla() }))));
            }
            let chips = div().row().gap(px(SPACE_3_PX)).children((0..4).map(|c| div().w(px(MINI_CHIP_W_PX)).h(px(MINI_CHIP_H_PX)).rounded_full().bg(if c == 0 { p.primary.hsla() } else { p.on_surface.with_alpha(ALPHA_15).hsla() })));
            let card = div()
                .absolute()
                .left(px((w - cw) / 2.0))
                .top(px(SPACE_8_PX))
                .w(px(cw))
                .p(px(SPACE_6_PX))
                .rounded(px(RADIUS_8_PX))
                .bg(surface.hsla())
                .row()
                .flex_col()
                .gap(px(SPACE_5_PX))
                .child(div().h(px(MINI_SEARCH_H_PX)).w_full().rounded_full().bg(p.surface_dim.hsla()))
                .child(chips)
                .child(grid);
            screen.child(card)
        }
        _ => {
            let dw = w * MINI_DRAWER_W;
            let mut drawer = div().absolute().bottom_0().left(px((w - dw) / 2.0)).w(px(dw)).p(px(SPACE_5_PX)).rounded_t(px(RADIUS_8_PX)).bg(surface.hsla()).row().flex_col().gap(px(SPACE_3_PX));
            for r in 0..MINI_ROWS {
                drawer = drawer.child(
                    div()
                        .h(px(MINI_ROW_H_PX))
                        .row()
                        .items_center()
                        .gap(px(SPACE_3_PX))
                        .px(px(SPACE_3_PX))
                        .rounded(px(RADIUS_3_PX))
                        .when(r == MINI_ROWS - 1, |el| el.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                        .child(div().size(px(MINI_ROW_H_PX - SPACE_3_PX)).rounded(px(RADIUS_1_5_PX)).bg(p.primary.with_alpha(ALPHA_60).hsla()))
                        .child(line(dw * (0.5 - 0.08 * r as f32), p.on_surface.with_alpha(ALPHA_40))),
                );
            }
            drawer = drawer.child(div().h(px(MINI_SEARCH_H_PX)).w_full().rounded_full().border_1().border_color(p.primary.with_alpha(ALPHA_60).hsla()).bg(p.surface_dim.hsla()));
            screen.child(drawer)
        }
    }
}

/// Asks the running shell to do something (Studio is its own process).
fn shell_ipc(cx: &gpui::App, args: &[&str]) {
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || {
            if let Err(err) = (ipc.0)(args) {
                tracing::warn!("launcher: {err}");
            }
        });
    }
}

/// A placement's tile in the diagram `(x, y, w, h)`, for a bar on the top
/// (`Some(true)`) or bottom edge, or a side bar (`None`: shares no edge).
fn tile_rect(pos: LauncherPosition, bar_top: Option<bool>) -> (f32, f32, f32, f32) {
    use mandara_config::workflow::LauncherAlign;
    let Some(edge) = pos.edge() else {
        return ((SCREEN_W_PX - CENTER_TILE_W_PX) / 2.0, (SCREEN_H_PX - CENTER_TILE_H_PX) / 2.0, CENTER_TILE_W_PX, CENTER_TILE_H_PX);
    };
    let x = match pos.align() {
        LauncherAlign::Left => TILE_INSET_PX,
        LauncherAlign::Center => (SCREEN_W_PX - EDGE_TILE_W_PX) / 2.0,
        LauncherAlign::Right => SCREEN_W_PX - TILE_INSET_PX - EDGE_TILE_W_PX,
    };
    // Tiles on the bar's edge hang from the bar; the others from the edge.
    let at_bar = bar_top.is_some_and(|top| (edge == LauncherEdge::Top) == top);
    let line = if at_bar { BAR_STRIP_PX + TILE_BAR_GAP_PX } else { 0.0 };
    let y = match edge {
        LauncherEdge::Top => line,
        LauncherEdge::Bottom => SCREEN_H_PX - line - EDGE_TILE_H_PX,
    };
    (x, y, EDGE_TILE_W_PX, EDGE_TILE_H_PX)
}

/// The label of a placement (Studio's choice list).
fn placement_label(pos: LauncherPosition) -> String {
    crate::registry::workflow_settings::POSITIONS.iter().find(|(id, _)| *id == pos.id()).map(|(_, l)| tr(l).to_string()).unwrap_or_else(|| pos.id().to_string())
}

/// The installed app a typed id or name refers to.
fn resolve_app(name: &str) -> Option<mandara_ui::apps::App> {
    let n = name.trim();
    let apps = mandara_ui::apps::discover();
    apps.iter().find(|a| a.id.eq_ignore_ascii_case(n)).or_else(|| apps.iter().find(|a| a.name.eq_ignore_ascii_case(n))).cloned()
}

/// `app:org.gnome.Nautilus` → its name; other keys as they are.
fn usage_label(key: &str) -> String {
    match key.split_once(':') {
        Some(("app", id)) => resolve_app(id).map(|a| a.name).unwrap_or_else(|| id.to_string()),
        Some(("cmd", line)) => format!("› {line}"),
        Some((_, rest)) => rest.to_string(),
        None => key.to_string(),
    }
}

impl StudioView {
    /// Studio → Launcher → Launcher looks: a card per ready-made look with
    /// a miniature of it; a click applies it (live, while the launcher is
    /// open too). Each setting it changes stays editable below.
    pub(super) fn launcher_looks(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let active = LauncherLookPreset::active(&cfg.launcher.look).map(|l| l.id);
        let fit = super::grid::fit(self.page.column_w - 2.0 * SPACE_18_PX, LOOK_GRID);
        let mini_w = (fit.card_w - 2.0 * SPACE_10_PX).max(0.0);
        let cards: Vec<AnyElement> = LAUNCHER_LOOKS
            .iter()
            .map(|preset| {
                let on = active == Some(preset.id);
                let preset = *preset;
                div()
                    .id(SharedString::from(format!("launcher-look-{}", preset.id)))
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .bg(if on { p.primary_container.hsla() } else { p.surface_container_high.hsla() })
                    .hover(|s| s.opacity(ALPHA_88))
                    .child(look_mini(&preset, mini_w, p))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_6_PX))
                            .child(div().text_size(px(TEXT_14_PX)).font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).child(tr(preset.name)))
                            .when(on, |el| el.child(icon("check", ICON_11_PX, p.primary))),
                    )
                    .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(preset.blurb))))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, move |c| preset.apply(&mut c.launcher.look))))
                    .into_any_element()
            })
            .collect();
        let custom = active.is_none().then(|| div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Customized: your own mix of the settings below. Pick a look to start over from it.")));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Launcher looks"), p))
            .child(
                card(p)
                    .child(heading(t("Ready-made looks"), t("Each sets the launcher's style, layout and placement together; change any setting below afterwards."), p))
                    .child(fit.balanced(LAUNCHER_LOOKS.len()).render(cards))
                    .children(custom)
                    .child(div().row().child(button("launcher-look-preview", "eye", t("Preview"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| shell_ipc(cx, &["launcher", "open"]))))),
            )
            .into_any_element()
    }

    /// Studio → Launcher → Placement: the screen diagram, what the chosen
    /// spot looks like with this bar, and a live preview.
    pub(super) fn launcher_placement(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let current = cfg.launcher.look.position;
        let bar_top = cfg.bar.position == mandara_config::BarPosition::Top;
        let side = cfg.bar.position.is_vertical();
        let right_bar = cfg.bar.position == mandara_config::BarPosition::Right;
        let mut screen = div()
            .relative()
            .flex_none()
            .w(px(SCREEN_W_PX))
            .h(px(SCREEN_H_PX))
            .rounded(px(SCREEN_RADIUS_PX))
            .overflow_hidden()
            .bg(p.surface_container_high.hsla())
            .when(!side, |el| el.child(div().absolute().left_0().w(px(SCREEN_W_PX)).h(px(BAR_STRIP_PX)).when(bar_top, |el| el.top_0()).when(!bar_top, |el| el.bottom_0()).bg(p.primary.with_alpha(ALPHA_30).hsla())))
            .when(side, |el| el.child(div().absolute().top_0().h(px(SCREEN_H_PX)).w(px(BAR_STRIP_PX)).when(!right_bar, |el| el.left_0()).when(right_bar, |el| el.right_0()).bg(p.primary.with_alpha(ALPHA_30).hsla())));
        for pos in LauncherPosition::ALL {
            let (x, y, w, h) = tile_rect(pos, (!side).then_some(bar_top));
            let on = pos == current;
            let (bg, fg) = if on { (p.primary, p.on_primary) } else { (p.surface_container, p.on_surface_variant) };
            screen = screen.child(
                div()
                    .id(SharedString::from(format!("launcher-place-{}", pos.id())))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(TILE_RADIUS_PX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(bg.hsla())
                    .when(!on, |el| el.hover(|s| s.bg(p.primary.with_alpha(ALPHA_25).hsla())))
                    .child(icon("search", TILE_ICON_PX, fg))
                    .tooltip(tip(placement_label(pos), p))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, move |c| c.launcher.look.position = pos))),
            );
        }
        // What the chosen spot does with this bar and dock.
        let edge = current.edge();
        let at_bar = !side && edge.is_some_and(|e| (e == LauncherEdge::Top) == bar_top);
        let how = match edge {
            None => t("Floats in the middle of the screen and scales in."),
            Some(_) if side => t("A side bar shares no edge with the launcher: it opens from the bare screen edge, beside the bar."),
            Some(_) if !at_bar => t("The bar is on the other edge: the launcher opens from the bare screen edge instead of growing out of the bar."),
            Some(_) if cfg.bar.layout.is_attached() => t("Pours out of the bar with curved flares, like its panels."),
            Some(_) => t("A floating card under the bar, the panels' gap away from it."),
        };
        let dock_note = (cfg.dock_bottom_clearance().is_some() && edge != Some(LauncherEdge::Top)).then(|| t("The dock is on: the launcher floats above it and opens on click only."));
        let button_note = (at_bar && cfg.launcher.look.align_to_button).then(|| t("In the zone holding the bar's launcher button, it grows out of the button."));
        let note = |text: SharedString| div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(text));
        let side = div()
            .flex_1()
            .min_w(px(DIM_180_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(placement_label(current)))
            .child(note(how.into()))
            .children(dock_note.map(|n| note(n.into())))
            .children(button_note.map(|n| note(n.into())))
            .child(div().row().child(button("launcher-place-preview", "eye", t("Preview"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| shell_ipc(cx, &["launcher", "open"])))));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Placement"), p))
            .child(
                card(p)
                    .child(heading(t("Where the launcher opens"), t("Click a spot. Spots on the bar's edge grow out of the bar like its panels; the middle one floats over everything. Left and right are the screen's own sides."), p))
                    .child(div().row().when(self.stack_rows, |el| el.flex_col()).gap(px(SPACE_18_PX)).child(screen).child(side)),
            )
            .into_any_element()
    }

    /// A text field that runs `on_enter` with its text and then clears.
    pub(super) fn add_field(&mut self, key: &str, placeholder: &'static str, on_enter: Rc<dyn Fn(String, &mut gpui::App)>, cx: &mut Context<Self>) -> gpui::Entity<TextField> {
        if let Some(f) = self.list_fields.get(key) {
            return f.clone();
        }
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                placeholder,
                false,
                Rc::new(move |value, _, cx| {
                    if !value.trim().is_empty() {
                        on_enter(value, cx);
                    }
                }),
                cx,
            )
            .clear_on_commit()
        });
        self.list_fields.insert(key.to_string(), field.clone());
        field
    }

    pub(super) fn launcher_extras(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        // Your names and icons.
        let mut rows = div().row().flex_col().gap(px(SPACE_8_PX));
        let overrides: Vec<(String, mandara_config::workflow::AppOverride)> = cfg.launcher.overrides.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (id, o) in overrides {
            let (id_name, id_icon, id_del) = (id.clone(), id.clone(), id.clone());
            let name = self.list_field(format!("ovr-{id}-name"), &o.name, t("Name"), Rc::new(move |c, v| if let Some(o) = c.launcher.overrides.get_mut(&id_name) { o.name = v.trim().to_string() }), cx);
            let icon_field = self.list_field(format!("ovr-{id}-icon"), &o.icon, t("Icon name or image path"), Rc::new(move |c, v| if let Some(o) = c.launcher.overrides.get_mut(&id_icon) { o.icon = v.trim().to_string() }), cx);
            let original = resolve_app(&id).map(|a| a.name).unwrap_or_else(|| t("not installed").to_string());
            rows = rows.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .when(self.stack_rows, |el| el.flex_col())
                    .gap(px(SPACE_8_PX))
                    .child(
                        div()
                            .w(px(DIM_180_PX))
                            .flex_none()
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(original))
                            .child(div().truncate().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(id.clone())),
                    )
                    .child(div().flex_1().min_w(px(DIM_120_PX)).child(name))
                    .child(div().flex_1().min_w(px(DIM_120_PX)).child(icon_field))
                    .child(
                        div()
                            .id(SharedString::from(format!("ovr-del-{id}")))
                            .size(px(DIM_32_PX))
                            .flex_none()
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .child(icon("trash", ICON_15_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.list_fields.retain(|k, _| !k.starts_with("ovr-"));
                                let id = id_del.clone();
                                update_config(cx, move |c| {
                                    c.launcher.overrides.remove(&id);
                                });
                            })),
                    ),
            );
        }
        let add = self.add_field(
            "ovr-add",
            t("Add an app: desktop id or name, then Enter"),
            Rc::new(|value, cx| {
                let Some(app) = resolve_app(&value) else {
                    tracing::warn!("launcher override: no installed app {value:?}");
                    return;
                };
                update_config(cx, move |c| {
                    c.launcher.overrides.entry(app.id.clone()).or_insert_with(|| mandara_config::workflow::AppOverride { name: app.name.clone(), icon: String::new() });
                });
            }),
            cx,
        );
        rows = rows.child(div().child(add));

        // What the ranking has learnt.
        let usage = mandara_ui::launcher_ext::usage_list_from_disk(cfg, USAGE_ROWS);
        let max = usage.first().map(|(_, s)| *s).unwrap_or(1.0).max(f64::EPSILON);
        let accent = p.primary;
        let usage_rows = usage.iter().map(|(key, score)| {
            let frac = (*score / max) as f32;
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().w(px(DIM_220_PX)).flex_none().truncate().text_size(px(TEXT_12_5_PX)).child(usage_label(key)))
                .child(
                    div()
                        .flex_1()
                        .h(px(USAGE_BAR_PX))
                        .rounded_full()
                        .bg(p.on_surface.with_alpha(ALPHA_8).hsla())
                        .child(div().h_full().w(gpui::relative(frac)).rounded_full().bg(mandara_core::oklab_gradient(mandara_ui::plugins::panel_fx::ANGLE_RIGHT_DEG, accent, mandara_ui::plugins::panel_fx::partner(accent)))),
                )
                .child(div().w(px(DIM_44_PX)).flex_none().text_end().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(format!("{score:.1}")))
        });
        let learnt = if usage.is_empty() {
            div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if cfg.launcher.usage.track { t("Nothing learnt yet: open a few apps from the launcher") } else { t("Usage tracking is off") }).into_any_element()
        } else {
            div().row().flex_col().gap(px(SPACE_8_PX)).children(usage_rows).into_any_element()
        };

        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(group_title(t("Your names & icons"), p))
                    .child(card(p).child(heading(t("Rename apps or give them your own icon"), t("Applies in the launcher (and the dock). Empty fields keep the app's own name or icon."), p)).child(rows)),
            )
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(group_title(t("What the launcher has learnt"), p))
                    .child(card(p).child(heading(tf("Top {n} by frecency", &[("n", &USAGE_ROWS)]), t("Launches count more when recent; the weight halves every few days (Ranking above)."), p)).child(learnt)),
            )
            .into_any_element()
    }
}
