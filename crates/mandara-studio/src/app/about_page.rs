//! Studio → About: the app's product page. The brand over a soft accent
//! wash, the update card, an info strip (plugins, commands, languages,
//! compositor), What's new, and grouped details, links and support.

use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_plugins::{PluginOrigin, PluginRegistry};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::components::surfaces::{CardSurface as _, Fill};
use mandara_ui::icons::icon;

use super::{HIDDEN_PLUGINS, ISSUES_URL, LICENSE, PLUGIN_GUIDE_URL, PROJECT_URL, RELEASES_URL, SUPPORT_URL, StudioView, grid, grouped, open_link};
use crate::tokens::*;

/// Direction of the hero's accent wash, degrees.
const WASH_ANGLE: f32 = 135.0;
/// The brand mark on the hero (smaller on a narrow page).
const BRAND_PX: f32 = 88.0;
const BRAND_SMALL_PX: f32 = 64.0;
/// The info strip's cells: as many as fit at this width, widened to fill.
const INFO_GRID: CardGrid = CardGrid { min_w: 150.0, max_w: 1000.0, gap: 0.0 };
/// An info cell's value.
const INFO_VALUE_PX: f32 = 22.0;

impl StudioView {
    /// Show the installed build, update state and project links in one page.
    pub(super) fn about(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let updates = self.updates_card(cfg, p, window, cx);
        let header_links = self.about_header_links(cfg, p, window, cx);
        let build = mandara_core::build_info(cx);
        let fonts = mandara_ui::fonts::fonts(cx);
        let column = self.page.column_w;
        let stack = self.stack_rows;
        let registry = cx.global::<PluginRegistry>();
        let commands = registry.commands.all().len();
        let first_party = registry.manifests().filter(|m| m.origin == PluginOrigin::Builtin && !HIDDEN_PLUGINS.contains(&m.id.as_str())).count();
        let compositor = mandara_compositor::detected().map(|k| format!("{k:?}")).unwrap_or_else(|| t("none").into());

        // Hero: the mark and the name over the accent wash; one quiet line
        // of facts instead of a row of coloured chips.
        let brand = if stack { BRAND_SMALL_PX } else { BRAND_PX };
        let facts = format!("{} {} · {LICENSE} · {}", t("Version"), mandara_ui::bidi::ltr_isolate(&build.version), t("100% Rust"));
        let hero = div()
            .relative()
            .clipped_card_surface(grouped::radius(), Fill(p.surface_container, None), Vec::new(), p)
            .child(div().absolute().inset_0().bg(mandara_core::oklab_gradient(WASH_ANGLE, p.accent_gradient_start.with_alpha(ALPHA_25), p.accent_gradient_end.with_alpha(ALPHA_5))))
            .child(
                div()
                    .relative()
                    .p(px(if stack { SPACE_20_PX } else { SPACE_28_PX }))
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_24_PX))
                    .child(gpui::img(gpui::ImageSource::from(mandara_ui::plugin_art::brand_icon())).size(px(brand)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_220_PX))
                            .row()
                            .flex_col()
                            .cross_start()
                            .gap(px(SPACE_6_PX))
                            .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_26_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Mandara Shell")))
                            .child(div().max_w(px(TEXT_MEASURE_PX)).text_size(px(TEXT_14_PX)).line_height(px(LEADING_21_PX)).text_color(p.on_surface.with_alpha(ALPHA_85).hsla()).child(mandara_ui::bidi::paragraph(t("A design-first Wayland desktop shell, written entirely in Rust."))))
                            .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(facts)),
                    )
                    .child(header_links),
            );

        // The info strip: one surface, cells divided by hairlines (four
        // across, two by two, or a column on a narrow page).
        let cells = [
            (t("Plugins"), first_party.to_string(), t("built in")),
            (t("Commands"), commands.to_string(), t("documented")),
            (t("Languages"), mandara_i18n::Lang::ALL.len().to_string(), t("translated")),
            (t("Compositor"), compositor, t("running")),
        ];
        let fit = grid::fit(column, INFO_GRID).balanced(cells.len());
        let line = grouped::hairline(p);
        let rtl = mandara_i18n::language().is_rtl();
        let cells = cells.into_iter().enumerate().map(|(i, (label, value, caption))| {
            let (col, row) = (i % fit.cols, i / fit.cols);
            div()
                .min_w(px(0.0))
                .px(px(SPACE_16_PX))
                .py(px(SPACE_16_PX))
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_4_PX))
                .when(col > 0 && !rtl, |el| el.border_l_1().border_color(line))
                .when(col > 0 && rtl, |el| el.border_r_1().border_color(line))
                .when(row > 0, |el| el.border_t_1().border_color(line))
                .child(div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(label))
                .child(div().max_w_full().truncate().font_family(fonts.display.clone()).text_size(px(INFO_VALUE_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(value))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(caption))
        });
        let info = div().clipped_card_surface(grouped::radius(), Fill(p.surface_container, None), Vec::new(), p).child(fit.render(cells));

        let whats_new = self.whats_new_card(p, cx);

        // Details and links: grouped rows, side by side on a wide page.
        let mono = fonts.mono.clone();
        let theme = SharedString::from(&mandara_core::theme_entity(cx).read(cx).target().name);
        let details = grouped::group(
            vec![
                grouped::row(Some("package"), t("Build")).value(build.long.clone()).render(stack, mono.clone(), p),
                grouped::row(Some("droplet"), t("Theme")).trailing(div().text_color(p.on_surface_variant.hsla()).child(theme)).render(stack, mono.clone(), p),
                grouped::row(Some(if cfg.theme.mode.is_dark() { "moon" } else { "sun" }), t("Appearance")).trailing(div().text_color(p.on_surface_variant.hsla()).child(if cfg.theme.mode.is_dark() { t("Dark") } else { t("Light") })).render(stack, mono.clone(), p),
                grouped::row(Some("note"), t("Config")).value(mandara_config::paths::config_file().display().to_string()).render(stack, mono.clone(), p),
            ],
            true,
            p,
        );
        let link = |id: &'static str, glyph: &'static str, label: &'static str, caption: &'static str, url: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Link)
                .cursor_pointer()
                .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_3).hsla()))
                .child(grouped::row(Some(glyph), label).caption(caption).trailing(icon("external", ICON_13_PX, p.on_surface_variant)).render(false, mono.clone(), p))
                .on_click(move |_, _, _| open_link(url))
                .into_any_element()
        };
        let links = grouped::group(
            vec![
                link("about-source", "github", t("Source code"), t("Read it, build it, fork it"), PROJECT_URL),
                link("about-issues", "alert", t("Report a problem"), t("Bugs and ideas on GitHub"), ISSUES_URL),
                link("about-plugins", "book", t("Plugin guide"), t("Write a plugin in Rust"), PLUGIN_GUIDE_URL),
                link("about-releases", "download", t("Releases"), t("Every version and its notes"), RELEASES_URL),
            ],
            true,
            p,
        );
        let details = grouped::section(grouped::header(t("This installation"), None, Vec::new(), p), details);
        let links = grouped::section(grouped::header(t("Project"), None, Vec::new(), p), links);
        let pair = if column >= TWO_COLUMNS_FROM_PX {
            div().row().cross_start().gap(px(BENTO_GAP_PX)).child(details.flex_1()).child(links.flex_1()).into_any_element()
        } else {
            div().row().flex_col().gap(px(GROUP_SECTION_GAP_PX)).child(details).child(links).into_any_element()
        };

        // Support: optional, friendly, never in the way — one quiet row.
        let coffee = super::compositor_page::button("about-support", "coffee", t("Buy me a coffee on Ko-fi"), false, p).on_click(|_, _, _| open_link(SUPPORT_URL));
        let support = grouped::group(
            vec![grouped::row(Some("heart"), t("Enjoying Mandara?"))
                .caption(t("Mandara is free and always will be. If it makes your desktop a little nicer and you'd like to say thanks, you can chip in toward the AI credits and developer time behind it. Completely optional: using it, sharing it and reporting bugs help just as much."))
                .wrap()
                .trailing(coffee)
                .render(stack || column < TWO_COLUMNS_FROM_PX, mono, p)],
            true,
            p,
        );

        div()
            .row()
            .flex_col()
            .gap(px(GROUP_SECTION_GAP_PX))
            .child(div().row().flex_col().gap(px(BENTO_GAP_PX)).child(hero).child(info).child(updates))
            .child(whats_new)
            .child(pair)
            .child(support)
            .into_any_element()
    }
}
