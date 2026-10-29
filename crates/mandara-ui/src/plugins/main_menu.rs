//! The main menu (right-click any empty part of the bar): quick entry points
//! to the launcher, Studio, wallpaper, screenshot, appearance, session and
//! shell actions.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px, size};
use mandara_core::ToGpui;
use mandara_i18n::{tk, tr};
use mandara_plugins::{CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_theme::{MandaraPalette, ThemeSource, catalog};
use serde_json::json;

use super::fn_view;
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};
use crate::state::{config, shell, update_config};

pub const ID: &str = "main-menu";

type Run = fn(&mut App);

struct Entry {
    icon: &'static str,
    label: &'static str,
    hint: &'static str,
    run: Run,
}

fn close(cx: &mut App) {
    shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
}

const ENTRIES: &[&[Entry]] = &[
    &[
        Entry { icon: "grid", label: tk("Launcher"), hint: "Super+Alt+Space", run: |cx| {
            close(cx);
            shell(cx).update(cx, |s, cx| s.set_launcher(true, cx));
        } },
        Entry { icon: "image", label: tk("Wallpapers"), hint: tk("picker"), run: |cx| {
            close(cx);
            shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(true, cx));
        } },
        Entry { icon: "sidebar", label: tk("Sidebar"), hint: tk("toggle"), run: |cx| {
            close(cx);
            let s = shell(cx);
            let open = !s.read(cx).sidebar_visible();
            s.update(cx, |s, cx| s.set_sidebar(open, cx));
        } },
        Entry { icon: "monitor", label: tk("Mandara Studio"), hint: tk("settings"), run: |cx| {
            close(cx);
            crate::state::open_studio(cx);
        } },
    ],
    &[
        Entry { icon: "crop", label: tk("Screenshot"), hint: "Super+Shift+S", run: |cx| {
            close(cx);
            crate::snip::open(crate::snip::SnipMode::Region, cx);
        } },
        Entry { icon: "clipboard", label: tk("Clipboard history"), hint: "Super+Shift+V", run: |cx| {
            shell(cx).update(cx, |s, cx| s.set_panel(Some("clipboard".into()), cx));
        } },
    ],
    &[
        Entry { icon: "moon", label: tk("Toggle light / dark"), hint: "Super+Shift+T", run: |cx| {
            let mode = config(cx).read(cx).get().theme.mode.toggled();
            update_config(cx, |c| c.theme.mode = mode);
        } },
        Entry { icon: "sun", label: tk("Next theme"), hint: "theme next", run: |cx| {
            let next = match &config(cx).read(cx).get().theme.source {
                ThemeSource::Preset { preset } => catalog::next_id(preset),
                _ => catalog::DEFAULT_THEME.to_string(),
            };
            update_config(cx, |c| {
                c.theme.source = ThemeSource::Preset { preset: next };
                c.wallpaper.match_colors = false;
            });
        } },
    ],
    &[
        Entry { icon: "lock", label: tk("Lock"), hint: "", run: |cx| {
            close(cx);
            let _ = mandara_services::SessionAction::Lock.run();
        } },
        Entry { icon: "power", label: tk("Power…"), hint: "Ctrl+Alt+Del", run: |cx| {
            close(cx);
            shell(cx).update(cx, |s, cx| s.set_session(true, cx));
        } },
        Entry { icon: "refresh", label: tk("Reload config"), hint: "", run: |cx| {
            close(cx);
            let _ = crate::state::reload_config_from_disk(cx);
        } },
    ],
];

const ROW_H: f32 = 38.0;
const WIDTH: f32 = 280.0;
/// Separator row between groups: height, inset and hairline alpha.
const SEPARATOR_H_PX: f32 = 9.0;
const SEPARATOR_INSET_PX: f32 = 10.0;
const SEPARATOR_ALPHA: f32 = 0.25;
/// Menu rows.
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_GAP_PX: f32 = 12.0;
const ROW_HOVER_ALPHA: f32 = 0.08;
const ROW_ICON_PX: f32 = 16.0;
const HINT_ALPHA: f32 = 0.7;

fn height() -> f32 {
    let rows: usize = ENTRIES.iter().map(|g| g.len()).sum();
    let separators = ENTRIES.len().saturating_sub(1) as f32;
    tokens::SM * 2.0 + rows as f32 * ROW_H + separators * SEPARATOR_H_PX
}

fn render(palette: &MandaraPalette, p: &mandara_plugins::PanelCx) -> gpui::AnyElement {
    let mut col = div().size_full().p(px(tokens::SM)).flex().flex_col();
    let mut index = 0usize;
    for (gi, group) in ENTRIES.iter().enumerate() {
        if gi > 0 {
            col = col.child(div().h(px(SEPARATOR_H_PX)).flex().items_center().px(px(SEPARATOR_INSET_PX)).child(div().w_full().h(px(1.0)).bg(palette.outline.with_alpha(SEPARATOR_ALPHA).hsla())));
        }
        for entry in group.iter() {
            let run = entry.run;
            let row = div()
                .id(("menu", index))
                .h(px(ROW_H))
                .px(px(ROW_PAD_X_PX))
                .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
                .flex()
                .items_center()
                .gap(px(ROW_GAP_PX))
                .cursor_pointer()
                .hover(|s| s.bg(palette.on_surface.with_alpha(ROW_HOVER_ALPHA).hsla()))
                .child(icon(entry.icon, ROW_ICON_PX, palette.on_surface_variant))
                .child(div().flex_1().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(crate::bidi::paragraph(tr(entry.label))))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.with_alpha(HINT_ALPHA).hsla()).child(crate::bidi::paragraph(tr(entry.hint))))
                .on_click(move |_, _, cx| run(cx));
            col = col.child(reveal(p, index, row));
            index += 1;
        }
    }
    col.into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Main menu"), tk("Right-click empty bar space"), None), |window, cx, panel_cx| {
        let p = panel_cx.clone();
        let panel = fn_view(None, move |palette, _, _| render(palette, &p), window, cx);
        PluginInstance { panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(WIDTH), px(height())))) }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new("menu", mandara_i18n::tk("Open or close the main menu"), |_, cx| {
        let s = shell(cx);
        s.update(cx, |s, cx| s.toggle_panel(ID, cx));
        Ok(json!({ "open": s.read(cx).panel.as_deref() == Some(ID) }))
    }));
}
