//! A quiet archive timeline with live operation progress and restore-to-copy actions.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::rc::Rc;
use gpui::{AnyElement, App, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf};
use mandara_plugins::PanelCx;
use mandara_services::backups::Operation;
use mandara_theme::MandaraPalette;
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};
use super::super::panel_fx::{glass_card, header, meter, pill_button, readable, redraw_in};
use super::{Shared, jobs};
use super::Page;

/// Timeline medallion and hero typography follow the utility panel scale.
const HERO_PX: f32 = 26.0;
const MEDALLION_PX: f32 = 34.0;
const RADIUS_PX: f32 = 18.0;
const PROGRESS_H: f32 = 5.0;

fn button(id: impl Into<SharedString>, glyph: &'static str, label: impl Into<SharedString>, p: &MandaraPalette, action: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let action: super::super::panel_fx::controls::Action = std::rc::Rc::new(action);
    let click = action.clone();
    super::super::panel_fx::controls::keyboard(pill_button(id.into(), Some(glyph), label, readable(p.primary, p), p, move |window, cx| click(window, cx)), p, action).into_any_element()
}

fn snapshot(row: &super::timeline::Row, busy: bool, model: &Shared, p: &MandaraPalette) -> AnyElement {
    let identity = row.identity.clone();
    let model = model.clone();
    let mut card = glass_card(p).rounded(px(RADIUS_PX)).p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM))
        .child(div().row().items_center().gap(px(tokens::MD))
            .child(icon("box", MEDALLION_PX, readable(p.primary, p)))
            .child(div().flex_1().min_w_0().row().flex_col().gap(px(tokens::XS))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(row.time.clone()))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(row.detail.clone())))
            .child(div().font_features(crate::fonts::tabular_nums()).child(row.size.clone())))
        .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(row.files.clone()))
        .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).children(row.paths.iter().map(|path| div().truncate().child(path.clone()))));
    if let Some(more) = &row.more { card = card.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(more.clone())); }
    if !busy {
        card = card.child(button(row.restore_id.clone(), "clock", t("Restore a copy…"), p, move |_, cx| {
            let identity = identity.clone();
            let model = model.clone();
            let settings = super::preferences(cx).0.clone();
            cx.spawn(async move |cx| {
                let selected = mandara_services::runtime::unblock(|| mandara_services::file_chooser::pick_folder(t("Choose where to restore a copy"), None)).await;
                cx.update(|cx| {
                    match selected {
                        Ok(Some(parent)) => { if let Err(error) = jobs::start_scoped(&model, &settings, Operation::Restore { snapshot: identity, parent }, cx) { model.borrow_mut().error = Some(error); } }
                        Ok(None) => {}
                        Err(error) => model.borrow_mut().error = Some(error),
                    }
                    model.borrow().views.notify(cx);
                });
            }).detach();
        }));
    }
    card.into_any_element()
}

pub(super) fn render(model: &Shared, context: &PanelCx, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    if context.progress() <= 0.0 { return div().into_any_element(); }
    model.borrow().visibility.track(context);
    let preferences = super::preferences(cx);
    let language = mandara_i18n::language();
    if model.borrow().page == Page::Archive && model.borrow().active.is_none() && model.borrow().history_settings.as_ref().is_none_or(|(settings, lang, format)| settings != &preferences.0 || *lang != language || *format != preferences.1.format) {
        let mut state = model.borrow_mut();
        state.read_cancel.store(true, Ordering::Release);
        state.history_settings = Some((preferences.0.clone(), language, preferences.1.format));
        state.history_page = 0;
        state.history.borrow_mut().clear();
        state.opened = false;
        state.retain_summary_for(&preferences.0);
    }
    model.borrow().views.track(window);
    model.borrow().history.borrow().views.track(window);
    if context.progress() > 0.0 && !model.borrow().opened {
        model.borrow_mut().opened = true;
        model.borrow_mut().read_cancel = Arc::new(AtomicBool::new(false));
        jobs::refresh(model, true, cx);
        let shared = model.clone();
        let watch = model.borrow().close_watch.clone();
        model.borrow().visibility.on_last_close(&watch, cx, move |_| {
            let mut model = shared.borrow_mut();
            model.opened = false;
            model.read_cancel.store(true, Ordering::Release);
            model.history.borrow_mut().clear();
            model.storage.clear();
            model.cleanup.clear();
            model.visibility.clear();
        });
    }
    let state = model.borrow();
    let history = state.history.borrow();
    let busy = state.active.is_some();
    let accent = readable(p.primary, p);
    let mut body = div().size_full().p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
        .child(header("box", accent, t("Backups"), t("Keep a way back"), p, Some(button("backups-settings", "sliders", t("Settings"), p,
            |_, cx| crate::state::open_studio_at(Some("plugins/backups/settings".into()), cx)))));
    let selected_page = state.page;
    let navigation = super::super::extras::segmented("backup-tabs", [
        (Page::Archive, t("Backups"), "lock"),
        (Page::Snapshots, t("Snapshots"), "clock"),
        (Page::Cleanup, t("Cleanup"), "disk"),
    ].into_iter().map(|(page, label, glyph)| {
        let shared = model.clone();
        super::super::extras::Segment { label: label.into(), icon: Some(glyph), active: selected_page == page || page == Page::Cleanup && selected_page == Page::Maintenance,
            on_click: Rc::new(move |_, cx| { shared.borrow_mut().select_page(page); if page == Page::Archive { jobs::refresh(&shared, true, cx); } shared.borrow().views.notify(cx); }) }
    }).collect(), p);
    body = body.child(navigation);
    if selected_page != Page::Archive {
        if let Some(error) = &state.error { body = body.child(div().text_size(px(text::BODY_S)).text_color(p.warning_amber.hsla()).child(crate::bidi::paragraph(error.clone()))); }
        if busy { body = body.child(t(state.status)); }
        if busy {
            let shared = model.clone();
            body = body.child(button("storage-cancel-active", "close", t("Cancel"), p, move |_, cx| {
                jobs::cancel(&shared, cx);
            }));
        }
        drop(history); drop(state);
        return body.child(if selected_page == Page::Cleanup { super::cleanup::render(model, p, window, cx) }
            else { super::storage::render(model, selected_page == Page::Maintenance, p, window, cx) }).into_any_element();
    }
    let mut hero = glass_card(p).rounded(px(RADIUS_PX)).p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
        .child(div().text_size(px(HERO_PX)).font_weight(FontWeight::SEMIBOLD).child(if busy { t(state.status) } else { t("Your backup archive") }))
        .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Encrypted backups can live on another disk. Local snapshots stay on this filesystem."))));
    if busy {
        redraw_in(jobs::PROGRESS_TICK, window, cx);
        let progress = state.progress.lock().unwrap_or_else(|error| error.into_inner());
        if progress.total_files > 0 {
            hero = hero.child(meter(progress.percent_done as f32, PROGRESS_H, p.primary, p.secondary, p))
                .child(div().font_features(crate::fonts::tabular_nums()).text_size(px(text::LABEL)).child(tf("{done} of {total} files", &[("done", &progress.files_done), ("total", &progress.total_files)])));
        }
        let shared = model.clone();
        hero = hero.child(button("backups-cancel", "close", t("Cancel"), p, move |_, cx| {
            jobs::cancel(&shared, cx);
        }));
    } else if preferences.1.archive_ready {
        let shared = model.clone();
        let check = model.clone();
        hero = hero.child(div().row().gap(px(tokens::SM))
            .children(preferences.1.backup_ready.then(|| button("backups-start", "plus", t("Back up now"), p, move |_, cx| {
                if let Err(error) = jobs::start(&shared, Operation::Backup, cx) { shared.borrow_mut().error = Some(error); shared.borrow().views.notify(cx); }
            })))
            .child(button("backups-check", "check", t("Check repository"), p, move |_, cx| {
                if let Err(error) = jobs::start(&check, Operation::Check, cx) { check.borrow_mut().error = Some(error); check.borrow().views.notify(cx); }
            })));
    }
    if !preferences.1.archive_ready {
        hero = hero.child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Choose a repository, password file and source folders in Settings. Initialize only a new repository."))));
    } else if !preferences.1.backup_ready {
        hero = hero.child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Add source folders in Settings to enable Back up now."))));
    }
    body = body.child(hero);
    if let Some(error) = state.error.as_ref().or(history.error.as_ref()) {
        body = body.child(glass_card(p).p(px(tokens::MD)).text_size(px(text::BODY_S)).text_color(p.warning_amber.hsla()).child(crate::bidi::paragraph(error.clone())));
    } else if let Some(summary) = &state.summary {
        body = body.child(div().text_size(px(text::BODY_S)).text_color(p.success_emerald.hsla()).child(summary.restored_to.as_ref().map_or_else(|| t("Completed successfully").to_string(), |path| tf("Copies restored to {path}", &[("path", &path.display())]))));
    }
    let rows = history.value.as_deref().map_or(&[][..], Vec::as_slice);
    let mut list = div().id("backup-history").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM));
    if rows.is_empty() {
        let shared = model.clone();
        list = list.child(div().p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
            .child(if history.busy() { t("Loading backup history…") } else { t("Your backups will appear here") })
            .children((preferences.1.archive_ready && !busy && !history.busy() && history.value.is_none()).then(|| button("backups-init", "lock", t("Initialize repository"), p, move |_, cx| {
                if let Err(error) = jobs::start(&shared, Operation::Initialize, cx) { shared.borrow_mut().error = Some(error); shared.borrow().views.notify(cx); }
            }))));
    } else {
        let page = state.history_page.min(rows.len().saturating_sub(1) / super::timeline::ROWS_PER_PAGE);
        list = list.children(rows.iter().skip(page * super::timeline::ROWS_PER_PAGE).take(super::timeline::ROWS_PER_PAGE).map(|row| snapshot(row, busy, model, p)));
        if rows.len() > super::timeline::ROWS_PER_PAGE {
            let previous = model.clone(); let next = model.clone();
            let mut pages = div().row().gap(px(tokens::SM));
            if page > 0 { pages = pages.child(button("backup-previous", "chevron-left", t("Previous"), p, move |_, cx| { previous.borrow_mut().history_page = page - 1; previous.borrow().views.notify(cx); })); }
            if (page + 1) * super::timeline::ROWS_PER_PAGE < rows.len() { pages = pages.child(button("backup-next", "chevron-right", t("Next"), p, move |_, cx| { next.borrow_mut().history_page = page + 1; next.borrow().views.notify(cx); })); }
            list = list.child(pages);
        }
    }
    let shared = model.clone();
    body.child(list).child(button("backups-refresh", "refresh", t("Refresh"), p, move |_, cx| jobs::refresh(&shared, true, cx))).into_any_element()
}
