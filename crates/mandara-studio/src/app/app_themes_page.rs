//! Studio → Themes → Other apps: the apps Mandara can theme, as a compact
//! grid of tiles (glyph, name, switch) grouped by kind, Noctalia style.
//!
//! Every app is off until switched on: nothing is written to another app's
//! files before that, and switching one off takes back what Mandara changed
//! (`mandara_theme::templates::Ledger`). Each tile's chevron discloses where
//! the theme goes and how to select it in the app (the registry entry's
//! help text); Kvantum's also holds "Use the Kvantum theme". The
//! `/integrations/*` registry entries stay the source of labels and help,
//! and keep the switches searchable.

use std::cell::RefCell;
use std::collections::HashSet;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::style::ClipRounded as _;
use serde_json::Value;

use super::StudioView;
use super::compositor_page::group_title;
use super::themes_page::section_head;
use crate::registry;
use crate::tokens::*;

/// One themeable app: its `[integrations]` switch, its product name (a
/// brand, the same in every language; the registry label with its details
/// shows in the disclosure) and glyph.
struct AppTile {
    path: &'static str,
    name: &'static str,
    icon: &'static str,
}

/// `kvantum_apply`: a sub-switch inside Kvantum's disclosure.
const KVANTUM_PATH: &str = "/integrations/kvantum";
const KVANTUM_APPLY_PATH: &str = "/integrations/kvantum_apply";

/// The apps by kind, in display order (`tk` titles).
const GROUPS: &[(&str, &[AppTile])] = &[
    (
        tk("Desktop toolkits"),
        &[
            AppTile { path: "/integrations/gtk4", name: "GTK", icon: "window" },
            AppTile { path: "/integrations/qt6", name: "Qt", icon: "tiles" },
            AppTile { path: KVANTUM_PATH, name: "Kvantum", icon: "sliders" },
            AppTile { path: "/integrations/kde", name: "KDE", icon: "monitor" },
        ],
    ),
    (
        tk("Terminals"),
        &[
            AppTile { path: "/integrations/ghostty", name: "Ghostty", icon: "terminal" },
            AppTile { path: "/integrations/kitty", name: "kitty", icon: "terminal" },
            AppTile { path: "/integrations/foot", name: "foot", icon: "terminal" },
            AppTile { path: "/integrations/alacritty", name: "Alacritty", icon: "terminal" },
            AppTile { path: "/integrations/wezterm", name: "WezTerm", icon: "terminal" },
        ],
    ),
    (tk("Editors"), &[AppTile { path: "/integrations/zed", name: "Zed", icon: "pencil" }, AppTile { path: "/integrations/helix", name: "Helix", icon: "pencil" }, AppTile { path: "/integrations/neovim", name: "Neovim", icon: "pencil" }]),
    (
        tk("Apps"),
        &[
            AppTile { path: "/integrations/fuzzel", name: "Fuzzel", icon: "search" },
            AppTile { path: "/integrations/btop", name: "btop", icon: "gauge" },
            AppTile { path: "/integrations/firefox", name: "Firefox", icon: "globe" },
            AppTile { path: "/integrations/vesktop", name: "Vesktop", icon: "user" },
        ],
    ),
];

/// Tiles: narrow enough for four across a normal page, one name per line.
const TILE_GRID: CardGrid = CardGrid { min_w: 200.0, max_w: 320.0, gap: SPACE_8_PX };
/// The glyph's rounded square.
const GLYPH_BOX_PX: f32 = DIM_32_PX;

thread_local! {
    /// Tiles whose details are disclosed (config paths), for this session.
    static OPEN: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
}

fn is_open(path: &'static str) -> bool {
    OPEN.with(|o| o.borrow().contains(path))
}

fn toggle_open(path: &'static str) {
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        if !o.remove(path) {
            o.insert(path);
        }
    });
}

impl StudioView {
    /// The "Other apps" section: a header and the tile groups.
    pub(super) fn app_themes_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let on_count = GROUPS.iter().flat_map(|(_, tiles)| tiles.iter()).filter(|tile| registry::by_path(tile.path).is_some_and(|s| registry::get(cfg, s).as_bool() == Some(true))).count();
        let badge = (on_count > 0).then(|| super::compositor_page::badge(mandara_i18n::tf("{count} on", &[("count", &on_count)]), p.primary).into_any_element());
        let head = section_head(
            t("Other apps"),
            t("Mandara writes nothing to other apps until you switch them on. Switching one off takes back exactly what Mandara changed, and any file it replaced comes back from its backup."),
            badge.into_iter().collect(),
            fonts.display.clone(),
            p,
        );
        let fit = super::grid::fit(self.page.column_w, TILE_GRID);
        let mut col = div().row().flex_col().gap(px(SPACE_14_PX)).child(head);
        for (title, tiles) in GROUPS {
            let (items, details): (Vec<AnyElement>, Vec<Option<AnyElement>>) = tiles.iter().filter_map(|tile| self.app_tile(tile, cfg, p, cx)).unzip();
            // Disclosed details span the whole column under the group's
            // tiles, so the grid itself stays one even line of tiles.
            col = col.child(div().row().flex_col().gap(px(SPACE_8_PX)).child(group_title(tr(title), p)).child(fit.render(items)).children(details.into_iter().flatten()));
        }
        col.into_any_element()
    }

    /// One app: glyph, name, disclosure chevron and switch; and, when
    /// disclosed, its details card (the full name and help text, plus
    /// Kvantum's activation switch).
    fn app_tile(&mut self, tile: &AppTile, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<(AnyElement, Option<AnyElement>)> {
        let setting = registry::by_path(tile.path)?;
        let on = registry::get(cfg, setting).as_bool().unwrap_or(false);
        let open = is_open(tile.path);
        let path = tile.path;
        let toggle = {
            let setting = setting.clone();
            div()
                .id(SharedString::from(format!("app-theme-{path}")))
                .cursor_pointer()
                .flex_none()
                .child(mandara_ui::plugins::network::switch(on, p))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting, Value::from(!on), cx)))
        };
        let chevron = div()
            .id(SharedString::from(format!("app-theme-more-{path}")))
            .size(px(DIM_24_PX))
            .flex_none()
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
            .tab_index(0)
            .child(gpui::svg().path("icons/chevron-right.svg").size(px(ICON_13_PX)).flex_none().text_color(p.on_surface_variant.hsla()).with_transformation(gpui::Transformation::rotate(gpui::radians(if open { std::f32::consts::FRAC_PI_2 } else { 0.0 }))))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                toggle_open(path);
                cx.notify();
            }));
        let glyph = div()
            .size(px(GLYPH_BOX_PX))
            .flex_none()
            .rounded(px(RADIUS_10_PX))
            .row()
            .items_center()
            .justify_center()
            .bg(if on { p.primary.with_alpha(ALPHA_16).hsla() } else { p.on_surface.with_alpha(ALPHA_6).hsla() })
            .child(icon(tile.icon, ICON_16_PX, if on { p.primary } else { p.on_surface_variant }));
        let header = div()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(glyph)
            .child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::MEDIUM).child(tile.name))
            .child(chevron)
            .child(toggle);
        let detail = open.then(|| {
            let mut d = div()
                .rounded(px(RADIUS_16_PX))
                .p(px(SPACE_14_PX))
                .bg(p.surface_container.hsla())
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(setting.label))))
                .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(setting.help))));
            if path == KVANTUM_PATH
                && let Some(apply) = registry::by_path(KVANTUM_APPLY_PATH)
            {
                let applied = registry::get(cfg, apply).as_bool().unwrap_or(false);
                let apply = apply.clone();
                d = d.child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(SPACE_3_PX)).child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(tr(apply.label))).child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(apply.help)))))
                        .child(
                            div()
                                .id("app-theme-kvantum-apply")
                                .cursor_pointer()
                                .flex_none()
                                .when(!on, |el| el.opacity(ALPHA_45))
                                .child(mandara_ui::plugins::network::switch(applied, p))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&apply, Value::from(!applied), cx))),
                        ),
                );
            }
            d.into_any_element()
        });
        let tile = div()
            .clip_rounded()
            .rounded(px(RADIUS_16_PX))
            .p(px(SPACE_10_PX))
            .bg(if on { p.primary.with_alpha(ALPHA_8).hsla() } else { p.surface_container.hsla() })
            .border_1()
            .border_color(if on || open { p.primary.with_alpha(ALPHA_35).hsla() } else { gpui::transparent_black() })
            .child(header)
            .into_any_element();
        Some((tile, detail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_switch_has_exactly_one_tile() {
        let tiled: Vec<&str> = GROUPS.iter().flat_map(|(_, tiles)| tiles.iter().map(|t| t.path)).collect();
        for path in &tiled {
            assert!(registry::by_path(path).is_some(), "{path} is a registry setting");
        }
        let unique: HashSet<&&str> = tiled.iter().collect();
        assert_eq!(unique.len(), tiled.len(), "no app twice");
        // Every app switch on the Themes page is a tile (Kvantum's activation
        // sits inside Kvantum's tile).
        for s in registry::in_section(registry::Section::Themes).filter(|s| s.path.starts_with("/integrations/")) {
            assert!(s.path == KVANTUM_APPLY_PATH || tiled.contains(&s.path), "{} has no tile", s.path);
        }
    }
}
