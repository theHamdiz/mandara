//! A focused permission inspector: explicit grants, clear scope and persistent Undo.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{AnyElement, App, AppContext as _, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf};
use mandara_plugins::PanelCx;
use mandara_services::app_permissions::{Capability, Constraint, Key};
use mandara_theme::MandaraPalette;
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};
use super::super::panel_fx::{glass_card, header, pill_button, readable, switch};
use super::{Shared, refresh, select};

/// Controls share the shell's accessible utility-panel sizing.
const HERO_PX: f32 = 24.0;
const GLYPH_PX: f32 = 22.0;
const RADIUS_PX: f32 = 16.0;
/// Search reaches all installed apps while each frame builds a small shelf.
const APPS_PER_PAGE: usize = 6;
const MONOGRAM_PX: f32 = 38.0;

fn label(capability: &Capability) -> (&'static str, &'static str, &'static str) {
    match (capability.key, capability.value.as_str()) {
        (Key::Shared, "network") => ("globe", t("Network access"), t("Connect to the network")),
        (Key::Shared, "ipc") => ("server", t("Shared IPC"), t("Share the host's IPC namespace")),
        (Key::Sockets, "pulseaudio") => ("headphones", t("Audio and microphone"), t("This grant permits both playback and recording")),
        (Key::Sockets, "wayland") => ("window", "Wayland", t("Connect to the Wayland display")),
        (Key::Sockets, "x11" | "fallback-x11") => ("window", "X11", t("Legacy display access can expose other applications")),
        (Key::Sockets, "ssh-auth") => ("key", t("SSH agent"), t("Use keys through the authentication agent")),
        (Key::Devices, "all") => ("usb", t("All devices"), t("Broad device access includes cameras and graphics devices")),
        (Key::Devices, "dri") => ("monitor", t("Graphics devices"), t("Direct rendering and GPU access")),
        (Key::Filesystems, "home") => ("home", t("Home folder"), t("Direct access to files in your home folder")),
        (Key::Filesystems, "host") => ("disk", t("Host files"), t("Broad access to host files outside protected system paths")),
        (Key::Filesystems, _) => ("folder", t("File access"), t("An explicit filesystem grant")),
        (Key::Sockets, _) => ("plug", t("System connection"), t("An explicit socket grant")),
        (Key::Devices, _) => ("usb", t("Device access"), t("An explicit device grant")),
        (Key::Shared, _) => ("server", t("Shared resources"), t("An explicit shared-resource grant")),
    }
}

fn button(id: &'static str, glyph: &'static str, label: &'static str, p: &MandaraPalette, action: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let action: super::super::panel_fx::controls::Action = std::rc::Rc::new(action);
    let click = action.clone();
    super::super::panel_fx::controls::keyboard(pill_button(id, Some(glyph), label, readable(p.primary, p), p, move |window, cx| click(window, cx)), p, action).into_any_element()
}

fn grant(row: &Capability, busy: bool, model: &Shared, p: &MandaraPalette) -> AnyElement {
    let (glyph, title, hint) = label(row);
    let mut card = glass_card(p).rounded(px(RADIUS_PX)).p(px(tokens::MD)).row().items_center().gap(px(tokens::MD))
        .child(icon(glyph, GLYPH_PX, if row.allowed { readable(p.primary, p) } else { p.on_surface_variant }))
        .child(div().flex_1().min_w_0().row().flex_col().gap(px(tokens::XS))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(hint)))
            .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).truncate().child(row.value.clone())));
    if row.editable && !busy {
        let capability = row.clone();
        let allowed = !row.allowed;
        let shared = model.clone();
        card = card.child(super::super::panel_fx::controls::action(div().id(SharedString::from(format!("grant-{}-{}", row.key.name(), row.value))).cursor_pointer()
            .aria_label(title).child(switch(row.allowed, p)), p,
            std::rc::Rc::new(move |_, cx| super::change(&shared, capability.clone(), allowed, cx))));
    } else {
        card = card.child(icon("lock", GLYPH_PX, p.on_surface_variant));
    }
    if !row.editable {
        let reason = match row.constraint {
            Some(Constraint::Conditional) => t("Conditional access; the native rule decides at app startup."),
            Some(Constraint::Broader) => t("A broader grant overlaps this access. Review that grant and the native details first."),
            _ => t("Advanced or scoped grant; inspect its native details below."),
        };
        return div().row().flex_col().gap(px(tokens::XS)).child(card)
            .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(reason))).into_any_element();
    }
    card.into_any_element()
}

pub(super) fn render(model: &Shared, context: &PanelCx, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    if context.progress() <= 0.0 { return div().into_any_element(); }
    model.borrow().visibility.track(context);
    if model.borrow().search.is_none() {
        let weak = std::rc::Rc::downgrade(model);
        let search = cx.new(|cx| crate::text_field::TextField::new(String::new(), t("Search apps…"), true, std::rc::Rc::new(move |value, _, cx| {
            if let Some(model) = weak.upgrade() {
                let mut state = model.borrow_mut(); state.query = value.to_lowercase(); state.page = 0; state.views.notify(cx);
            }
        }), cx));
        model.borrow_mut().search = Some(search);
    }
    model.borrow().views.track(window);
    model.borrow().apps.borrow().views.track(window);
    model.borrow().session.borrow().views.track(window);
    if context.progress() > 0.0 && !model.borrow().opened {
        let mut state = model.borrow_mut();
        state.opened = true;
        state.read_cancel = Arc::new(AtomicBool::new(false));
        let watch = state.close_watch.clone();
        drop(state);
        refresh(model, true, cx);
        let shared = model.clone();
        model.borrow().visibility.on_last_close(&watch, cx, move |_| {
            let mut state = shared.borrow_mut();
            state.opened = false;
            state.read_cancel.store(true, Ordering::Release);
            state.apps.borrow_mut().clear();
            state.session.borrow_mut().clear();
            state.selected = None;
            state.search = None;
            state.query = String::new();
            state.page = 0;
            state.visibility.clear();
        });
    }
    let state = model.borrow();
    let busy = state.active.is_some();
    let apps = state.apps.borrow();
    let session = state.session.borrow();
    let accent = readable(p.primary, p);
    let mut body = div().size_full().p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
        .child(header("shield", accent, t("App permissions"), t("Decide what each app can reach"), p, None));
    if let Some(error) = state.error.as_ref().or(session.error.as_ref()).or(apps.error.as_ref()) {
        body = body.child(glass_card(p).p(px(tokens::MD)).text_color(p.warning_amber.hsla()).text_size(px(text::BODY_S)).child(crate::bidi::paragraph(error.clone())));
    }
    if state.changed.as_ref().is_some_and(|changed| state.selected.as_ref().is_some_and(|selected| selected.id == changed.id)) {
        body = body.child(div().text_size(px(text::BODY_S)).text_color(p.success_emerald.hsla()).child(t("Saved. Restart this app for the new permissions to take effect.")));
    }
    let mut list = div().id("permissions-content").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM));
    if let Some(selected) = &state.selected {
        let shared = model.clone();
        list = list.child(button("permissions-back", "chevron-left", t("All apps"), p, move |_, cx| {
            let mut state = shared.borrow_mut();
            if state.active.is_some() { return; }
            state.read_cancel.store(true, Ordering::Release);
            state.apps.borrow_mut().invalidate();
            state.selected = None;
            state.session.borrow_mut().clear();
            state.views.notify(cx);
        }))
            .child(glass_card(p).p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
                .child(div().text_size(px(HERO_PX)).font_weight(FontWeight::SEMIBOLD).child(selected.name.clone()))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(selected.id.clone()))
                .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Personal overrides apply to this app in every installation. Portals can still grant files you explicitly choose.")))));
        if let Some(session) = &session.value {
            list = list.children(session.capabilities.iter().map(|row| grant(row, busy, model, p)));
            let shared = model.clone();
            list = list.child(button("permissions-details", "info", t("Native permission details"), p, move |_, cx| {
                let mut state = shared.borrow_mut(); state.details = !state.details; state.views.notify(cx);
            }));
            if state.details {
                list = list.child(glass_card(p).p(px(tokens::MD)).text_size(px(text::LABEL)).child(crate::bidi::paragraph(session.metadata.clone())));
            }
        } else if session.busy() { list = list.child(t("Reading app permissions…")); }
    } else {
        let entries = apps.value.as_deref().map_or(&[][..], Vec::as_slice);
        list = list.child(glass_card(p).p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
            .child(div().text_size(px(HERO_PX)).font_weight(FontWeight::SEMIBOLD).child(tf("{count} apps, clearer boundaries", &[("count", &entries.len())])))
            .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Review network, audio, files and device grants. Each change is backed up and can be undone.")))));
        if entries.is_empty() { list = list.child(if apps.busy() { t("Finding installed apps…") } else { t("No Flatpak apps found") }); }
        if let Some(field) = &state.search {
            list = list.child(div().row().flex_shrink_0().child(super::super::extras::search_box("permissions-search-clear", field, &state.query, p, cx)));
        }
        let count = entries.iter().filter(|row| row.matches(&state.query)).count();
        let page = state.page.min(count.saturating_sub(1) / APPS_PER_PAGE);
        list = list.children(entries.iter().filter(|row| row.matches(&state.query)).skip(page * APPS_PER_PAGE).take(APPS_PER_PAGE).map(|row| {
            let app = row.app.clone();
            let shared = model.clone();
            super::super::panel_fx::controls::action(glass_card(p).id(row.id.clone()).cursor_pointer().p(px(tokens::MD))
                .row().items_center().gap(px(tokens::MD))
                .child(super::super::extras::monogram(&row.initials, accent, MONOGRAM_PX))
                .child(div().flex_1().min_w_0().row().flex_col().gap(px(tokens::XS)).child(row.name.clone())
                    .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).truncate().child(row.subtitle.clone())))
                .child(icon("chevron-right", GLYPH_PX, p.on_surface_variant)), p,
                std::rc::Rc::new(move |_, cx| select(&shared, app.clone(), cx)))
        }));
        if count == 0 && !entries.is_empty() { list = list.child(t("No matching apps")); }
        if count > APPS_PER_PAGE {
            let previous = model.clone(); let next = model.clone();
            let mut pages = div().row().items_center().gap(px(tokens::SM));
            if page > 0 { pages = pages.child(button("permissions-previous", "chevron-left", t("Previous"), p, move |_, cx| { previous.borrow_mut().page = page - 1; previous.borrow().views.notify(cx); })); }
            if (page + 1) * APPS_PER_PAGE < count { pages = pages.child(button("permissions-next", "chevron-right", t("Next"), p, move |_, cx| { next.borrow_mut().page = page + 1; next.borrow().views.notify(cx); })); }
            list = list.child(pages);
        }
    }
    let shared = model.clone();
    let undo = model.clone();
    let mut footer = div().row().gap(px(tokens::SM))
        .child(button("permissions-refresh", "refresh", t("Refresh"), p, move |_, cx| {
            let selected = shared.borrow().selected.clone();
            if let Some(app) = selected { select(&shared, app, cx); } else { refresh(&shared, true, cx); }
        }));
    if state.undo.is_some() && !busy {
        footer = footer.child(button("permissions-undo", "undo", t("Undo"), p, move |_, cx| super::undo(&undo, cx)));
        if let Some(app) = &state.changed { footer = footer.child(div().min_w_0().truncate().text_size(px(text::LABEL)).child(app.name.clone())); }
    }
    body.child(list).child(footer).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_labels_describe_the_actual_combined_grant() {
        let row = Capability { key: Key::Sockets, value: "pulseaudio".into(), allowed: true, editable: true, constraint: None };
        assert_eq!(label(&row).1, t("Audio and microphone"));
        let row = Capability { key: Key::Devices, value: "all".into(), allowed: true, editable: true, constraint: None };
        assert_eq!(label(&row).1, t("All devices"));
    }
}
