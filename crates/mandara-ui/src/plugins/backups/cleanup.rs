//! Reviewed cleanup keeps its manifest only while the user inspects that selection.

mod commands;
pub(super) use commands::register;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tk};
use mandara_services::backups::cleanup::{self as api, Candidate, Kind, Preview, Report};
use mandara_theme::MandaraPalette;

use super::super::{integration, panel_fx::{glass_card, pill_button, readable}};
use super::{Page, Shared, jobs};
use crate::bidi::Bidi as _;
use crate::kit::{text, tokens};

type Data<K, V> = Rc<RefCell<integration::Data<K, V>>>;
/// Limit card and path creation even for very large caches.
const ITEMS_PER_PAGE: usize = 8;
const PATHS_PER_PAGE: usize = 12;
const CACHE_SECONDS: u64 = 24 * 60 * 60;
const CACHE_AGE: Duration = Duration::from_secs(CACHE_SECONDS);

pub(super) struct State {
    candidates: Data<(), Vec<Candidate>>,
    preview: Data<(Candidate, u64), Preview>,
    command_preview: Option<Arc<Preview>>,
    selected: Option<Candidate>,
    cancel: Arc<AtomicBool>,
    opened: bool,
    page: usize,
    paths_page: usize,
    report: Option<Report>,
    review_generation: u64,
}
impl Default for State {
    fn default() -> Self {
        Self { candidates: integration::evicting(), preview: integration::evicting(), command_preview: None, selected: None, cancel: Arc::default(), opened: false, page: 0, paths_page: 0, report: None, review_generation: 0 }
    }
}
impl State {
    pub fn invalidate_review(&mut self) { self.review_generation = self.review_generation.wrapping_add(1); }
    pub fn cancel_reads(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.candidates.borrow_mut().invalidate();
        self.preview.borrow_mut().invalidate();
        self.opened = false;
        let missing = self.command_preview.is_none() && self.preview.borrow().value.is_none();
        if missing {
            self.selected = None;
            self.paths_page = 0;
            self.preview.borrow_mut().clear();
        }
    }
    pub fn clear(&mut self) {
        self.invalidate_review();
        self.cancel.store(true, Ordering::Release);
        self.candidates.borrow_mut().clear();
        self.preview.borrow_mut().clear();
        if self.command_preview.take().is_some() { mandara_services::memory::request_trim(); }
        self.selected = None;
        self.report = None;
        self.opened = false;
        self.page = 0;
        self.paths_page = 0;
    }
}

fn load(model: &Shared, cx: &mut App) {
    let state = model.borrow();
    integration::load(&state.cleanup.candidates, (), true, CACHE_AGE, |_| api::candidates().map_err(|error| format!("{error:#}")), cx);
}

fn select(model: &Shared, candidate: Candidate, cx: &mut App) {
    let age = super::preferences(cx).1.cleanup_days;
    let (data, token) = {
        let mut state = model.borrow_mut();
        if state.active.is_some() { return; }
        state.cleanup.cancel.store(true, Ordering::Release);
        state.cleanup.preview.borrow_mut().invalidate();
        state.cleanup.command_preview = None;
        state.cleanup.cancel = Arc::new(AtomicBool::new(false));
        state.cleanup.selected = Some(candidate.clone());
        state.cleanup.paths_page = 0;
        state.error = None;
        (state.cleanup.preview.clone(), state.cleanup.cancel.clone())
    };
    integration::load(&data, (candidate.clone(), age), true, CACHE_AGE,
        move |_| api::preview(&candidate, age, &token).map_err(|error| format!("{error:#}")), cx);
    model.borrow().views.notify(cx);
}

fn action(id: impl Into<SharedString>, label: &'static str, glyph: &'static str, p: &MandaraPalette, callback: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let action: super::super::panel_fx::controls::Action = std::rc::Rc::new(callback);
    let click = action.clone();
    super::super::panel_fx::controls::keyboard(pill_button(id.into(), Some(glyph), label, readable(p.primary, p), p, move |window, cx| click(window, cx)), p, action).into_any_element()
}

fn remove_output(model: &Shared, preview: Arc<Preview>, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    jobs::perform(model, tk("Removing reviewed files…"), move |cancel| api::remove(&preview, &cancel).map_err(|error| format!("{error:#}")), |report, model, cx| {
        let mut state = model.borrow_mut();
        state.cleanup.preview.borrow_mut().clear();
        if state.cleanup.command_preview.take().is_some() { mandara_services::memory::request_trim(); }
        state.cleanup.selected = None;
        let opened = state.opened && state.page == Page::Cleanup;
        state.cleanup.report = opened.then_some(report);
        drop(state);
        if opened { load(model, cx); }
    }, cx)
}

fn remove(model: &Shared, preview: Arc<Preview>, cx: &mut App) {
    let result = remove_output(model, preview, cx);
    if let Err(error) = result { model.borrow_mut().error = Some(error); model.borrow().views.notify(cx); }
}

pub(super) fn cancel_review(model: &Shared) { model.borrow_mut().cleanup.clear(); }

pub(super) fn render(model: &Shared, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    model.borrow().cleanup.candidates.borrow().views.track(window);
    model.borrow().cleanup.preview.borrow().views.track(window);
    if !model.borrow().cleanup.opened && model.borrow().active.is_none() { model.borrow_mut().cleanup.opened = true; load(model, cx); }
    let state = model.borrow();
    let cleanup = &state.cleanup;
    let candidates = cleanup.candidates.borrow();
    let preview = cleanup.preview.borrow();
    let busy = state.active.is_some();
    let mut body = div().flex_1().min_h_0().row().flex_col().gap(px(tokens::MD));
    if let Some(error) = candidates.error.as_ref().or(preview.error.as_ref()) {
        body = body.child(div().text_size(px(text::BODY_S)).text_color(p.warning_amber.hsla()).child(crate::bidi::paragraph(error.clone())));
    }
    if let Some(candidate) = &cleanup.selected {
        let shared = model.clone();
        body = body.child(action("cleanup-back", t("All cleanup items"), "chevron-left", p, move |_, cx| {
            if shared.borrow().active.is_some() { return; }
            shared.borrow_mut().cleanup.clear(); load(&shared, cx); shared.borrow().views.notify(cx);
        }))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(candidate.name.clone()))
            .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(candidate.path.display().to_string()));
        if let Some(preview) = cleanup.command_preview.as_ref().or(preview.value.as_ref()) {
            body = body.child(glass_card(p).p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
                .child(tf("{count} files selected", &[("count", &preview.file_count())]))
                .child(crate::widgets::system::format_bytes(preview.logical_bytes))
                .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("These are logical file sizes. Snapshots, hard links and shared blocks can keep some space in use."))))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(tf("{count} items stay untouched", &[("count", &preview.left_untouched)]))));
            let page = cleanup.paths_page.min(preview.file_count().saturating_sub(1) / PATHS_PER_PAGE);
            let mut list = div().id("cleanup-reviewed-files").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM));
            for (path, bytes) in preview.files().skip(page * PATHS_PER_PAGE).take(PATHS_PER_PAGE) {
                list = list.child(div().row().gap(px(tokens::MD)).child(div().flex_1().min_w_0().truncate().child(path.display().to_string()))
                    .child(div().font_features(crate::fonts::tabular_nums()).child(crate::widgets::system::format_bytes(bytes))));
            }
            if preview.file_count() > PATHS_PER_PAGE {
                let previous = model.clone(); let next = model.clone();
                list = list.child(div().row().gap(px(tokens::SM))
                    .child(action("cleanup-paths-previous", t("Previous"), "chevron-left", p, move |_, cx| { let mut state = previous.borrow_mut(); state.cleanup.paths_page = state.cleanup.paths_page.saturating_sub(1); state.views.notify(cx); }))
                    .child(action("cleanup-paths-next", t("Next"), "chevron-right", p, move |_, cx| { next.borrow_mut().cleanup.paths_page = page + 1; next.borrow().views.notify(cx); })));
            }
            body = body.child(list);
            if preview.file_count() > 0 && !busy {
                let selected = preview.clone(); let shared = model.clone();
                body = body.child(div().text_size(px(text::BODY_S)).text_color(p.warning_amber.hsla()).child(crate::bidi::paragraph(t("Removal is permanent. Close affected apps first; caches may need to download again. Files added after this preview stay untouched."))))
                    .child(action("cleanup-remove-reviewed", t("Permanently remove these files"), "trash", p, move |_, cx| remove(&shared, selected.clone(), cx)));
            } else if preview.file_count() == 0 {
                body = body.child(t("No files qualify for removal. Recent files, links and protected locations stay untouched."));
            }
        } else if preview.busy() { body = body.child(t("Preparing the cleanup preview…")); }
    } else {
        body = body.child(glass_card(p).p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(t("Make room, with a clear view")))
            .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Review old cache files or one Trash item at a time. Personal documents and application settings are not cleanup locations.")))));
        if let Some(report) = &cleanup.report {
            body = body.child(div().text_color(p.success_emerald.hsla()).child(tf("Removed {count} reviewed files", &[("count", &report.files_removed)])));
        }
        let rows = candidates.value.as_deref().map_or(&[][..], Vec::as_slice);
        let page = cleanup.page.min(rows.len().saturating_sub(1) / ITEMS_PER_PAGE);
        let mut list = div().id("cleanup-items").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM));
        for (index, candidate) in rows.iter().enumerate().skip(page * ITEMS_PER_PAGE).take(ITEMS_PER_PAGE) {
            let selected = candidate.clone(); let shared = model.clone();
            list = list.child(glass_card(p).p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM))
                .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(candidate.name.clone()))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(match candidate.kind { Kind::Cache => t("Old cache files"), Kind::Trash => t("Trash item") }))
                .child(action(format!("cleanup-preview-{index}"), t("Preview files…"), "eye", p, move |_, cx| select(&shared, selected.clone(), cx))));
        }
        if rows.is_empty() { list = list.child(if candidates.busy() { t("Reading cleanup locations…") } else { t("No cleanup items found") }); }
        if rows.len() > ITEMS_PER_PAGE {
            let previous = model.clone(); let next = model.clone();
            list = list.child(div().row().gap(px(tokens::SM))
                .child(action("cleanup-previous", t("Previous"), "chevron-left", p, move |_, cx| { let mut state = previous.borrow_mut(); state.cleanup.page = state.cleanup.page.saturating_sub(1); state.views.notify(cx); }))
                .child(action("cleanup-next", t("Next"), "chevron-right", p, move |_, cx| { next.borrow_mut().cleanup.page = page + 1; next.borrow().views.notify(cx); })));
        }
        body = body.child(list);
    }
    let maintenance = model.clone(); let refresh = model.clone();
    body.child(div().row().gap(px(tokens::SM))
        .child(action("cleanup-refresh", t("Refresh"), "refresh", p, move |_, cx| { if refresh.borrow().active.is_none() { refresh.borrow_mut().cleanup.clear(); load(&refresh, cx); refresh.borrow().views.notify(cx); } }))
        .child(action("cleanup-maintenance", t("Filesystem maintenance"), "disk", p, move |_, cx| { maintenance.borrow_mut().select_page(Page::Maintenance); maintenance.borrow().views.notify(cx); }))).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_initial_cleanup_reads_allows_a_visible_page_to_retry() {
        let mut state = State { opened: true, paths_page: 4, ..State::default() };
        state.preview.borrow_mut().error = Some("interrupted preview".into());
        state.cancel_reads();
        assert!(!state.opened);
        assert!(state.selected.is_none());
        assert_eq!(state.paths_page, 0);
        assert!(state.preview.borrow().error.is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "opt-in cleanup lifecycle test; requires a marked private native home"]
    fn native_cleanup_cancellation_releases_an_unfinished_selection() {
        let home = std::path::PathBuf::from(std::env::var_os("MANDARA_BACKUP_TEST_HOME").expect("private native home required"));
        assert_eq!(std::env::var_os("HOME").unwrap(), home.as_os_str());
        assert_eq!(std::fs::read_to_string(home.join(".mandara-backup-test")).unwrap(), "mandara restic fixture\n");
        let cache = std::path::PathBuf::from(std::env::var_os("XDG_CACHE_HOME").unwrap());
        assert!(cache.starts_with(&home));
        let directory = cache.join("mandara-test-cache");
        std::fs::create_dir_all(&directory).unwrap();
        let candidate = api::candidates().unwrap().into_iter().find(|candidate| candidate.path == directory).unwrap();
        let mut state = State { opened: true, selected: Some(candidate), ..State::default() };
        state.cancel_reads();
        assert!(state.selected.is_none());
        assert!(!state.opened);
        assert!(state.command_preview.is_none());
        assert!(!state.preview.borrow().busy());
    }
}
