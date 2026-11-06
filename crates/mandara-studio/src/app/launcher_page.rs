//! Studio → Launcher. The page reads top to bottom as the launcher is set
//! up: the ready-made looks (Classic, Grid, Beacon; a card each with a
//! miniature, all the same size), where it opens (a small screen diagram of
//! the seven placements), the bar's launcher button, search & results,
//! behaviour, your apps (your own names and icons, `[launcher.overrides]`),
//! and slash commands and skills. A glance at what the ranking has learnt
//! (frecency from `launcher-usage.toml`) closes Search & results.

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;
use std::rc::Rc;

use gpui::{AnyElement, AppContext as _, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_config::workflow::{LAUNCHER_LOOKS, LauncherLookPreset};
use mandara_i18n::{t, tf, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::launcher_data::USAGE_ROWS;
use super::compositor_page::{badge, button, card, heading, tip};
use super::themes_page::section_head;
use crate::registry::{self, Section};
use crate::registry::workflow_settings::{APPS, BEACON, GENERAL, GRID, LAUNCHING, LOOK, PROVIDERS, RANKING, RESULTS};
use crate::text_field::TextField;
use crate::tokens::*;

/// Usage bar height.
const USAGE_BAR_PX: f32 = 6.0;

/// The registry groups of each page section, in page order. Every group of
/// the Launcher section is in exactly one of them (a unit test checks), so
/// no setting is ever left off the page.
const PLACEMENT_GROUPS: &[&str] = &[LOOK];
const BUTTON_GROUPS: &[&str] = &[BUTTON];
const RESULTS_GROUPS: &[&str] = &[RESULTS, GRID, BEACON, PROVIDERS, RANKING];
const BEHAVIOUR_GROUPS: &[&str] = &[GENERAL, LAUNCHING];
const APPS_GROUPS: &[&str] = &[APPS];
/// The launcher button's group (the icon picker).
const BUTTON: &str = mandara_i18n::tk("Button");
/// Config path of the placement (drawn as the screen diagram instead of a
/// choice row).
pub(super) const LAUNCHER_POSITION_PATH: &str = "/launcher/look/position";

/// Launcher looks: the card grid. `max_w ≥ 1.5 × min_w + gap`, so a row of
/// two or more cards always fills the column exactly.
const LOOK_GRID: CardGrid = CardGrid { min_w: 200.0, max_w: 620.0, gap: SPACE_14_PX };
/// A look card's padding, inside its 2 px ring.
const LOOK_PAD_PX: f32 = SPACE_8_PX;
/// Lines of a look's description. It is clamped, and every slot of the card
/// has a fixed height, so all cards are exactly as tall whatever the
/// language or the length of the text.
const LOOK_BLURB_LINES: usize = 3;
/// The text block under the miniature: padding, title, blurb and tags.
const LOOK_TEXT_PAD_TOP_PX: f32 = SPACE_12_PX;
const LOOK_TEXT_PAD_X_PX: f32 = SPACE_6_PX;
const LOOK_TEXT_PAD_BOTTOM_PX: f32 = SPACE_6_PX;
const LOOK_TITLE_H_PX: f32 = DIM_22_PX;
const LOOK_BLURB_H_PX: f32 = LEADING_16_PX * LOOK_BLURB_LINES as f32;
const LOOK_TAG_H_PX: f32 = DIM_22_PX;
const LOOK_GAP_PX: f32 = SPACE_6_PX;
/// The miniature drawn on each card (a little screen with the launcher in it).
const MINI_H_PX: f32 = 120.0;
/// Every look card's height: the sum of its fixed slots.
const LOOK_CARD_H_PX: f32 =
    2.0 * (BORDER_2_PX + LOOK_PAD_PX) + MINI_H_PX + LOOK_TEXT_PAD_TOP_PX + LOOK_TITLE_H_PX + LOOK_GAP_PX + LOOK_BLURB_H_PX + LOOK_GAP_PX + LOOK_TAG_H_PX + LOOK_TEXT_PAD_BOTTOM_PX;
const MINI_RADIUS_PX: f32 = RADIUS_15_PX;
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
    let screen = div().relative().w(px(w)).h(px(MINI_H_PX)).flex_none().rounded(px(MINI_RADIUS_PX)).overflow_hidden().bg(mandara_core::oklab_gradient(mandara_ui::plugins::panel_fx::ANGLE_RIGHT_DEG, p.surface_dim, p.surface_dim.mix(p.primary, ALPHA_8)));
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
                .top(px((MINI_H_PX - (2.0 * SPACE_6_PX + MINI_SEARCH_H_PX + MINI_CHIP_H_PX + 2.0 * SPACE_5_PX + rows as f32 * MINI_ICON_PX + SPACE_4_PX)) / 2.0))
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
pub(super) fn shell_ipc(cx: &gpui::App, args: &[&str]) {
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || {
            if let Err(err) = (ipc.0)(args) {
                tracing::warn!("launcher: {err}");
            }
        });
    }
}

/// The label of a results layout (List, Grid).
fn layout_label(layout: mandara_config::workflow::LauncherLayout) -> String {
    let id = serde_json::to_value(layout).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    crate::registry::workflow_settings::LAYOUTS.iter().find(|(k, _)| *k == id).map(|(_, l)| tr(l).to_string()).unwrap_or(id)
}

/// A page section: its heading, then its cards and folds.
pub(super) fn page_section(head: AnyElement, body: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div().row().flex_col().gap(px(SPACE_14_PX)).child(head).children(body).into_any_element()
}

impl StudioView {
    /// Studio → Launcher: every section in order.
    pub(super) fn launcher_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let in_groups = |groups: &'static [&'static str]| registry::in_section(Section::Launcher).filter(move |s| groups.contains(&s.group) && s.path != LAUNCHER_POSITION_PATH);

        let looks = self.launcher_looks(cfg, p, cx);
        let placement_card = self.launcher_placement(cfg, p, cx);
        let placement_rows = self.rows(in_groups(PLACEMENT_GROUPS), cfg, p, cx);
        let placement = page_section(
            section_head(t("Placement"), t("Choose where the launcher opens. Attached positions follow the bar on that edge."), vec![], display.clone(), p),
            [placement_card, placement_rows],
        );

        let button_list: Vec<&'static registry::Setting> = in_groups(BUTTON_GROUPS).collect();
        let button_rows = self.group_card(&button_list, self.page.column_w - 2.0 * SPACE_6_PX, cfg, p, cx);
        let launcher_button = page_section(
            section_head(t("Launcher button"), t("The icon on the bar's launcher button"), vec![], display.clone(), p),
            [div().rounded(px(RADIUS_20_PX)).bg(p.surface_container.hsla()).p(px(SPACE_6_PX)).child(button_rows).into_any_element()],
        );

        let results_rows = self.rows(in_groups(RESULTS_GROUPS), cfg, p, cx);
        let learnt = self.launcher_learnt(cfg, p);
        let results = page_section(section_head(t("Search & results"), t("What the launcher searches, how results look and how they are ranked"), vec![], display.clone(), p), [results_rows, learnt]);

        let behaviour_rows = self.rows(in_groups(BEHAVIOUR_GROUPS), cfg, p, cx);
        let behaviour = page_section(section_head(t("Behaviour"), t("Turning the launcher on, dimming the screen, the keyboard and how apps start"), vec![], display.clone(), p), [behaviour_rows]);

        let apps_rows = self.rows(in_groups(APPS_GROUPS), cfg, p, cx);
        let names = self.launcher_names(cfg, p, cx);
        let apps = page_section(section_head(t("Apps"), t("Pin, hide, rename or re-icon apps in the launcher"), vec![], display.clone(), p), [apps_rows, names]);

        let editor = self.launcher_editor(cfg, p, cx);
        div().row().flex_col().gap(px(PAGE_SECTION_GAP_PX)).child(looks).child(placement).child(launcher_button).child(results).child(behaviour).child(apps).child(editor).into_any_element()
    }

    /// Studio → Launcher → Launcher looks: a card per ready-made look with
    /// a miniature of it; a click applies it (live, while the launcher is
    /// open too). Each setting it changes stays editable below.
    ///
    /// Every card is exactly as tall and wide as the others: the grid gives
    /// them equal columns, and each card is the sum of fixed slots (the
    /// miniature, a one-line title, a description clamped to
    /// [`LOOK_BLURB_LINES`] and a one-line row of tags).
    fn launcher_looks(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let active = LauncherLookPreset::active(&cfg.launcher.look).map(|l| l.id);
        let fit = super::grid::fit(self.page.column_w, LOOK_GRID).balanced(LAUNCHER_LOOKS.len());
        let mini_w = (fit.card_w - 2.0 * (LOOK_PAD_PX + BORDER_2_PX)).max(0.0);
        let tag = |text: String| {
            div()
                .min_w(px(0.0))
                .h(px(LOOK_TAG_H_PX))
                .px(px(SPACE_8_PX))
                .rounded_full()
                .bg(p.on_surface.with_alpha(ALPHA_6).hsla())
                .row()
                .items_center()
                .overflow_hidden()
                .text_size(px(TEXT_11_PX))
                .text_color(p.on_surface_variant.hsla())
                .child(div().min_w(px(0.0)).truncate().child(text))
        };
        let cards: Vec<AnyElement> = LAUNCHER_LOOKS
            .iter()
            .map(|preset| {
                let on = active == Some(preset.id);
                let preset = *preset;
                let preview = div().relative().flex_none().child(look_mini(&preset, mini_w, p)).when(on, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(SPACE_8_PX))
                            .inset_end(px(SPACE_8_PX))
                            .px(px(SPACE_8_PX))
                            .h(px(DIM_20_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .gap(px(SPACE_4_PX))
                            .bg(p.primary.hsla())
                            .text_color(p.on_primary.hsla())
                            .text_size(px(TEXT_10_5_PX))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon("check", ICON_11_PX, p.on_primary))
                            .child(t("Active")),
                    )
                });
                let text = div()
                    .flex_none()
                    .px(px(LOOK_TEXT_PAD_X_PX))
                    .pt(px(LOOK_TEXT_PAD_TOP_PX))
                    .pb(px(LOOK_TEXT_PAD_BOTTOM_PX))
                    .row()
                    .flex_col()
                    .gap(px(LOOK_GAP_PX))
                    .child(
                        div()
                            .h(px(LOOK_TITLE_H_PX))
                            .row()
                            .items_center()
                            .child(div().min_w(px(0.0)).truncate().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).text_color(if on { p.primary.hsla() } else { p.on_surface.hsla() }).child(tr(preset.name))),
                    )
                    .child(div().h(px(LOOK_BLURB_H_PX)).overflow_hidden().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(LOOK_BLURB_LINES).child(mandara_ui::bidi::paragraph(tr(preset.blurb))))
                    .child(div().h(px(LOOK_TAG_H_PX)).row().items_center().gap(px(SPACE_6_PX)).overflow_hidden().child(tag(layout_label(preset.layout))).child(tag(super::launcher_placement::label(preset.position).to_string())));
                div()
                    .id(SharedString::from(format!("launcher-look-{}", preset.id)))
                    .flex_none()
                    .w_full()
                    .h(px(LOOK_CARD_H_PX))
                    .p(px(LOOK_PAD_PX))
                    .clip_rounded()
                    .rounded(px(RADIUS_22_PX))
                    .row()
                    .flex_col()
                    .cursor_pointer()
                    .bg(p.surface_container.hsla())
                    .border_2()
                    .border_color(if on { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_12).hsla() })
                    .when(!on, |el| el.hover(|s| s.border_color(p.primary.with_alpha(ALPHA_55).hsla())))
                    .child(preview)
                    .child(text)
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, move |c| preset.apply(&mut c.launcher.look))))
                    .into_any_element()
            })
            .collect();
        let custom = active.is_none().then(|| {
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .rounded(px(RADIUS_16_PX))
                .bg(p.surface_container.hsla())
                .child(icon("sliders", ICON_14_PX, p.primary))
                .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Customized: your own mix of the settings below. Pick a look to start over from it."))))
        });
        let preview = button("launcher-look-preview", "eye", t("Preview"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| shell_ipc(cx, &["launcher", "open"])));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Launcher looks"), t("Each sets the launcher's style, layout and placement together; change any setting below afterwards."), vec![preview.into_any_element()], fonts.display.clone(), p))
            .child(fit.render(cards))
            .children(custom)
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

    /// Your names and icons for apps (`[launcher.overrides]`).
    fn launcher_names(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.launcher_data.snapshot().cloned() else {
            return card(p).child(heading(t("Your names & icons"), t("Applies in the launcher (and the dock). Empty fields keep the app's own name or icon."), p)).child(t("Loading…")).into_any_element();
        };
        let mut rows = div().row().flex_col().gap(px(SPACE_8_PX));
        for (id, o) in &cfg.launcher.overrides {
            let (id_name, id_icon, id_del) = (id.clone(), id.clone(), id.clone());
            let name = self.list_field(format!("ovr-{id}-name"), &o.name, t("Name"), Rc::new(move |c, v| if let Some(o) = c.launcher.overrides.get_mut(&id_name) { o.name = v.trim().to_string() }), cx);
            let icon_field = self.list_field(format!("ovr-{id}-icon"), &o.icon, t("Icon name or image path"), Rc::new(move |c, v| if let Some(o) = c.launcher.overrides.get_mut(&id_icon) { o.icon = v.trim().to_string() }), cx);
            let app = data.resolve(id);
            let shown_icon = if o.icon.trim().is_empty() { app.and_then(|app| app.icon.as_deref()).unwrap_or_default() } else { o.icon.trim() };
            let original = app.map(|app| app.name.clone()).unwrap_or_else(|| t("not installed").into());
            rows = rows.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .when(self.stack_rows, |el| el.flex_col())
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(
                        div()
                            .w(px(DIM_220_PX))
                            .flex_none()
                            .row()
                            .items_center()
                            .gap(px(SPACE_10_PX))
                            .child(div().flex_none().size(px(DIM_36_PX)).rounded(px(RADIUS_12_PX)).bg(p.surface_container_high.hsla()).row().items_center().justify_center().child(mandara_ui::icons::any_icon(if shown_icon.is_empty() { "grid" } else { shown_icon }, ICON_22_PX, p.on_surface_variant)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .row()
                                    .flex_col()
                                    .gap(px(SPACE_3_PX))
                                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(original))
                                    .child(div().truncate().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(id.clone())),
                            ),
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
                            .tooltip(tip(t("Delete"), p))
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
        let weak = cx.weak_entity();
        let add = self.add_field(
            "ovr-add",
            t("Add an app: desktop id or name, then Enter"),
            Rc::new(move |value, cx| {
                let app = weak.update(cx, |this, _| this.launcher_data.snapshot().and_then(|data| data.resolve(&value)).map(|app| (app.id.clone(), app.name.to_string()))).ok().flatten();
                let Some((id, name)) = app else {
                    tracing::warn!("launcher override: no installed app {value:?}");
                    return;
                };
                update_config(cx, move |c| {
                    c.launcher.overrides.entry(id).or_insert_with(|| mandara_config::workflow::AppOverride { name, icon: String::new() });
                });
            }),
            cx,
        );
        rows = rows.child(div().row().items_center().gap(px(SPACE_10_PX)).child(icon("plus", ICON_14_PX, p.primary)).child(div().flex_1().min_w(px(0.0)).child(add)));
        card(p).child(heading(t("Your names & icons"), t("Applies in the launcher (and the dock). Empty fields keep the app's own name or icon."), p)).child(rows).into_any_element()
    }

    /// What the ranking has learnt: the most-used results by frecency.
    fn launcher_learnt(&mut self, cfg: &ShellConfig, p: &MandaraPalette) -> AnyElement {
        let data = self.launcher_data.snapshot();
        let usage = data.map(|data| data.rows.as_ref()).unwrap_or_default();
        let max = usage.first().map(|row| row.score).unwrap_or(1.0).max(f64::EPSILON);
        let accent = p.primary;
        let usage_rows = usage.iter().enumerate().map(|(n, row)| {
            let frac = (row.score / max) as f32;
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().w(px(DIM_20_PX)).flex_none().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.with_alpha(ALPHA_70).hsla()).child(format!("{}", n + 1)))
                .child(div().w(px(DIM_200_PX)).flex_none().truncate().text_size(px(TEXT_12_5_PX)).child(row.label.clone()))
                .child(
                    div()
                        .flex_1()
                        .h(px(USAGE_BAR_PX))
                        .rounded_full()
                        .bg(p.on_surface.with_alpha(ALPHA_8).hsla())
                        .child(div().h_full().w(gpui::relative(frac)).rounded_full().bg(mandara_core::oklab_gradient(mandara_ui::plugins::panel_fx::ANGLE_RIGHT_DEG, accent, mandara_ui::plugins::panel_fx::partner(accent)))),
                )
                .child(div().w(px(DIM_44_PX)).flex_none().text_end().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(row.display.clone()))
        });
        let learnt = if data.is_none() {
            div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Loading…")).into_any_element()
        } else if usage.is_empty() {
            div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if cfg.launcher.usage.track { t("Nothing learnt yet: open a few apps from the launcher") } else { t("Usage tracking is off") }).into_any_element()
        } else {
            div().row().flex_col().gap(px(SPACE_8_PX)).children(usage_rows).into_any_element()
        };
        card(p)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(div().flex_none().size(px(DIM_36_PX)).rounded(px(RADIUS_12_PX)).bg(p.primary.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon("clock", ICON_16_PX, p.primary)))
                    .child(div().flex_1().min_w(px(0.0)).child(heading(t("What the launcher has learnt"), t("Launches count more when recent; the weight halves every few days (Ranking above)."), p)))
                    .child(badge(tf("Top {n} by frecency", &[("n", &USAGE_ROWS)]), p.primary)),
            )
            .child(learnt)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every group of the Launcher section is drawn in exactly one page
    /// section, so no setting is ever left off the page.
    #[test]
    fn every_launcher_group_has_one_section() {
        let sections = [PLACEMENT_GROUPS, BUTTON_GROUPS, RESULTS_GROUPS, BEHAVIOUR_GROUPS, APPS_GROUPS];
        for s in registry::in_section(Section::Launcher).filter(|s| s.path != LAUNCHER_POSITION_PATH) {
            let n = sections.iter().filter(|g| g.contains(&s.group)).count();
            assert_eq!(n, 1, "{} ({}) is in {n} page sections", s.path, s.group);
        }
    }

    /// The card height is the sum of its fixed slots (no slot grows with
    /// its text).
    #[test]
    fn look_cards_have_one_height() {
        let slots = MINI_H_PX + LOOK_TEXT_PAD_TOP_PX + LOOK_TITLE_H_PX + LOOK_BLURB_H_PX + LOOK_TAG_H_PX + LOOK_TEXT_PAD_BOTTOM_PX + 2.0 * LOOK_GAP_PX;
        assert!((LOOK_CARD_H_PX - 2.0 * (BORDER_2_PX + LOOK_PAD_PX) - slots).abs() < 0.01);
        const { assert!(LOOK_GRID.max_w >= 1.5 * LOOK_GRID.min_w + LOOK_GRID.gap) };
    }
}
