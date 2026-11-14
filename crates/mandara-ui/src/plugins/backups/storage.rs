//! Native snapshot selection and a concrete review before filesystem changes.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tk};
use mandara_services::backups::filesystem::{Backend as _, Maintenance, Mount, Native, Reply, Request, mounted};
use mandara_theme::MandaraPalette;

use super::super::{integration, panel_fx::{glass_card, pill_button, readable}};
use super::{Shared, jobs};
use crate::bidi::Bidi as _;
use crate::kit::{text, tokens};

type Data<K, V> = Rc<RefCell<integration::Data<K, V>>>;
/// Explicit opens and Refresh invalidate reads; renders never run a scan repeatedly.
const CACHE_SECONDS: u64 = 24 * 60 * 60;
const AGE: Duration = Duration::from_secs(CACHE_SECONDS);
/// Bounded card construction even on systems with thousands of snapshots.
const ROWS_PER_PAGE: usize = 6;

pub(super) struct State {
    mounts: Data<(), Vec<Mount>>,
    native: Data<Mount, Reply>,
    selected: Option<Mount>,
    elevated: Option<(Mount, Reply)>,
    cancel: Arc<AtomicBool>,
    preview: Option<Request>,
    page: usize,
    mount_page: usize,
    opened: bool,
    pub notice: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self { mounts: integration::evicting(), native: integration::evicting(), selected: None, elevated: None, cancel: Arc::default(), preview: None, page: 0, mount_page: 0, opened: false, notice: None }
    }
}
impl State {
    fn empty_message(&self, data: &integration::Data<(), Vec<Mount>>) -> Option<&'static str> {
        (self.opened && data.error.is_none()).then(|| if data.busy() { t("Reading filesystems…") } else { t("No supported filesystems found") })
    }
    fn complete(&mut self, selected: &Mount, reply: Reply, inspect: bool) -> bool {
        self.preview = None;
        if self.selected.as_ref() != Some(selected) { return false; }
        if inspect {
            self.opened = true;
            self.native.borrow_mut().error = None;
            self.elevated = Some((selected.clone(), reply));
            false
        } else {
            self.notice = Some(if reply.output.trim().is_empty() { t("Completed successfully").into() } else { reply.output });
            self.elevated = None;
            self.native.borrow_mut().clear();
            true
        }
    }
    pub fn clear(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.mounts.borrow_mut().clear();
        self.native.borrow_mut().clear();
        self.selected = None;
        self.elevated = None;
        self.preview = None;
        self.notice = None;
        self.opened = false;
        self.page = 0;
        self.mount_page = 0;
    }
    pub fn cancel_reads(&mut self) {
        self.opened = false;
        self.cancel.store(true, Ordering::Release);
        self.mounts.borrow_mut().invalidate();
        self.native.borrow_mut().invalidate();
    }
}

fn refresh(model: &Shared, cx: &mut App) {
    let mut state = model.borrow_mut();
    if state.active.is_some() { return; }
    state.storage.cancel_reads();
    state.storage.opened = true;
    state.storage.cancel = Arc::new(AtomicBool::new(false));
    state.storage.elevated = None;
    state.storage.preview = None;
    let cancel = state.storage.cancel.clone();
    if let Some(mount) = &state.storage.selected {
        if !mount.supports_snapshots() { state.storage.native.borrow_mut().clear(); return; }
        let mount = mount.clone();
        integration::load(&state.storage.native, mount.clone(), true, AGE,
            move |_| Native.inspect(&mount, &cancel).map_err(|error| format!("{error:#}")), cx);
    } else {
        integration::load(&state.storage.mounts, (), true, AGE, move |_| mounted().map(|mounts| mounts.into_iter()
            .filter(|mount| mount.supports_snapshots() || mount.supports_trim()).collect()).map_err(|error| format!("{error:#}")), cx);
    }
}

fn action(id: impl Into<SharedString>, title: &'static str, glyph: &'static str, p: &MandaraPalette, callback: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let action: super::super::panel_fx::controls::Action = std::rc::Rc::new(callback);
    let click = action.clone();
    super::super::panel_fx::controls::keyboard(pill_button(id.into(), Some(glyph), title, readable(p.primary, p), p, move |window, cx| click(window, cx)), p, action).into_any_element()
}

pub(super) fn review(model: &Shared, request: Request, cx: &mut App) {
    let mut state = model.borrow_mut();
    if state.active.is_some() { return; }
    state.storage.preview = Some(request);
    state.views.notify(cx);
}

fn apply(model: &Shared, request: Request, cx: &mut App) {
    if let Err(error) = apply_output(model, request, cx) { model.borrow_mut().error = Some(error); model.borrow().views.notify(cx); }
}

pub(super) fn confirm(model: &Shared, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    let request = model.borrow().storage.preview.clone().ok_or_else(|| t("Review a filesystem action before confirming it.").to_owned())?;
    apply_output(model, request, cx)
}

pub(super) fn apply_output(model: &Shared, request: Request, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    let selected = request.mount().clone();
    let status = match &request {
        Request::Inspect { .. } => tk("Reading local snapshots…"),
        Request::Create { .. } => tk("Creating a read-only snapshot…"),
        Request::Delete { .. } => tk("Removing the selected snapshot…"),
        Request::Maintain { operation: Maintenance::Scrub, .. } => tk("Checking data and metadata…"),
        Request::Maintain { operation: Maintenance::ReclaimEmptyChunks, .. } => tk("Reclaiming empty Btrfs chunks…"),
        Request::Maintain { operation: Maintenance::Trim, .. } => tk("Trimming unused blocks…"),
    };
    let inspect = matches!(request, Request::Inspect { .. });
    jobs::perform(model, status, move |cancel| Native.apply(&request, &cancel).map_err(|error| format!("{error:#}")), move |reply, model, cx| {
        let reload = {
            let mut state = model.borrow_mut();
            state.opened && state.storage.complete(&selected, reply, inspect)
        };
        if reload { refresh(model, cx); }
    }, cx)
}

pub(super) fn cancel_preview(model: &Shared) { model.borrow_mut().storage.preview = None; }

fn preview(request: &Request, model: &Shared, p: &MandaraPalette) -> AnyElement {
    let (title, detail) = match request {
        Request::Delete { snapshot, .. } => (t("Remove this snapshot?"), format!("{}\n{} · {}", snapshot.path.display(), snapshot.id, snapshot.uuid)),
        Request::Create { source, directory, name, .. } => (t("Create a read-only snapshot?"), format!("{} → {}", source.display(), directory.join(name).display())),
        Request::Maintain { operation, .. } => (t("Review filesystem maintenance"), match operation {
            Maintenance::Scrub => t("Scrub checks data and metadata. Redundant good copies can repair damaged copies; it does not free disk space."),
            Maintenance::ReclaimEmptyChunks => t("Reclaim completely empty data and metadata block groups. Existing files, snapshot contents and redundancy profiles stay intact."),
            Maintenance::Trim => t("Tell this device which blocks are unused. This does not remove files or increase filesystem free space."),
        }.into()),
        Request::Inspect { .. } => (t("Read local snapshots"), String::new()),
    };
    let selected = request.clone();
    let confirm = model.clone();
    let cancel = model.clone();
    glass_card(p).p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_size(px(text::BODY_S)).child(crate::bidi::paragraph(detail)))
        .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(request.mount().target.display().to_string()))
        .child(div().row().gap(px(tokens::SM))
            .child(action("storage-confirm", t("Continue"), "check", p, move |_, cx| apply(&confirm, selected.clone(), cx)))
            .child(action("storage-cancel-review", t("Cancel"), "close", p, move |_, cx| { cancel.borrow_mut().storage.preview = None; cancel.borrow().views.notify(cx); })))
        .into_any_element()
}

pub(super) fn render(model: &Shared, maintenance: bool, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    model.borrow().storage.mounts.borrow().views.track(window);
    model.borrow().storage.native.borrow().views.track(window);
    if !model.borrow().storage.opened && model.borrow().active.is_none() { refresh(model, cx); }
    let state = model.borrow();
    let storage = &state.storage;
    let mut body = div().flex_1().min_h_0().row().flex_col().gap(px(tokens::MD));
    if let Some(request) = &storage.preview { return preview(request, model, p); }
    let mounts = storage.mounts.borrow();
    let native = storage.native.borrow();
    if let Some(error) = mounts.error.as_ref().or(native.error.as_ref()) {
        body = body.child(div().text_size(px(text::BODY_S)).text_color(p.warning_amber.hsla()).child(crate::bidi::paragraph(error.clone())));
    }
    if let Some(mount) = &storage.selected {
        let shared = model.clone();
        body = body.child(action("storage-all-disks", t("All filesystems"), "chevron-left", p, move |_, cx| {
            if shared.borrow().active.is_some() { return; }
            shared.borrow_mut().storage.clear(); refresh(&shared, cx); shared.borrow().views.notify(cx);
        }))
            .child(glass_card(p).p(px(tokens::LG)).row().flex_col().gap(px(tokens::SM))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(mount.target.display().to_string()))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(format!("{} · {}", mount.filesystem, mount.device))));
        if maintenance {
            body = body.child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Choose an operation for this filesystem. Each action is reviewed before administrator approval."))));
            if state.active.is_none() {
                for (operation, title, hint, glyph, available) in [
                    (Maintenance::Scrub, t("Check integrity"), t("Btrfs scrub validates data and metadata"), "shield", mount.supports_snapshots() && !mount.read_only),
                    (Maintenance::ReclaimEmptyChunks, t("Reclaim empty chunks"), t("A filtered Btrfs balance releases empty block groups"), "box", mount.supports_snapshots() && !mount.read_only),
                    (Maintenance::Trim, t("Trim unused blocks"), t("For supported SSDs and thin-provisioned storage"), "disk", mount.supports_trim()),
                ] {
                    if !available {
                        body = body.child(glass_card(p).p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM)).child(title)
                            .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(if mount.read_only { t("This filesystem is mounted read-only.") } else { t("This operation is unavailable for the selected filesystem.") }))));
                        continue;
                    }
                    let shared = model.clone(); let mount = mount.clone();
                    body = body.child(glass_card(p).p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM)).child(title)
                        .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(hint))
                        .child(action(format!("storage-{operation:?}"), t("Review…"), glyph, p, move |_, cx| review(&shared, Request::Maintain { mount: mount.clone(), operation }, cx))));
                }
            }
        } else if mount.supports_snapshots() {
            let settings = super::preferences(cx);
            if state.active.is_none() && !mount.read_only && settings.1.source.is_absolute() && settings.1.directory.is_absolute() {
                let source = settings.1.source.clone(); let directory = settings.1.directory.clone();
                let shared = model.clone(); let selected = mount.clone();
                body = body.child(action("storage-create", t("Create snapshot…"), "plus", p, move |_, cx| {
                    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                        Ok(time) => review(&shared, Request::Create { mount: selected.clone(), source: source.clone(), directory: directory.clone(), name: format!("Mandara-{}", time.as_nanos()) }, cx),
                        Err(error) => { shared.borrow_mut().error = Some(error.to_string()); shared.borrow().views.notify(cx); }
                    }
                }));
            } else if !settings.1.source.is_absolute() || !settings.1.directory.is_absolute() {
                body = body.child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Choose a local snapshot source and destination folder in Settings to create snapshots. Nested subvolumes are not included."))));
            }
            let reply = storage.elevated.as_ref().filter(|(owner, _)| owner == mount).map(|(_, reply)| reply).or(native.value.as_deref());
            let rows = reply.map_or(&[][..], |reply| reply.snapshots.as_slice());
            let page = storage.page.min(rows.len().saturating_sub(1) / ROWS_PER_PAGE);
            let mut list = div().id("local-snapshots").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM));
            for snapshot in rows.iter().skip(page * ROWS_PER_PAGE).take(ROWS_PER_PAGE) {
                let mut card = glass_card(p).p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM))
                    .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(snapshot.path.display().to_string()))
                    .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(snapshot.id.to_string()));
                if snapshot.protected { card = card.child(t("Mounted or default snapshot · protected")); }
                else if mount.read_only { card = card.child(t("This filesystem is mounted read-only.")); }
                else if state.active.is_none() {
                    let shared = model.clone(); let snapshot = snapshot.clone(); let mount = mount.clone();
                    card = card.child(action(format!("delete-snapshot-{}", snapshot.id), t("Remove snapshot…"), "trash", p, move |_, cx| review(&shared, Request::Delete { mount: mount.clone(), snapshot: snapshot.clone() }, cx)));
                }
                list = list.child(card);
            }
            if rows.is_empty() { list = list.child(if native.busy() { t("Reading local snapshots…") } else { t("No snapshots loaded") }); }
            if rows.len() > ROWS_PER_PAGE {
                let previous = model.clone(); let next = model.clone();
                list = list.child(div().row().gap(px(tokens::SM))
                    .child(action("snapshots-previous", t("Previous"), "chevron-left", p, move |_, cx| { let mut state = previous.borrow_mut(); state.storage.page = state.storage.page.saturating_sub(1); state.views.notify(cx); }))
                    .child(action("snapshots-next", t("Next"), "chevron-right", p, move |_, cx| { next.borrow_mut().storage.page = page + 1; next.borrow().views.notify(cx); })));
            }
            body = body.child(list);
            if state.active.is_none() {
                let shared = model.clone(); let mount = mount.clone();
                body = body.child(action("storage-read-admin", t("Load snapshots with administrator approval"), "lock", p, move |_, cx| apply(&shared, Request::Inspect { mount: mount.clone() }, cx)));
            }
        } else { body = body.child(t("This filesystem does not support native Btrfs snapshots.")); }
        if let Some(notice) = &storage.notice { body = body.child(div().text_size(px(text::LABEL)).child(crate::bidi::paragraph(notice.clone()))); }
    } else {
        let mut list = div().id("storage-mounts").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM));
        let rows = mounts.value.as_deref().map_or(&[][..], Vec::as_slice);
        let count = rows.iter().filter(|mount| maintenance || mount.supports_snapshots()).count();
        let page = storage.mount_page.min(count.saturating_sub(1) / ROWS_PER_PAGE);
        for mount in rows.iter().filter(|mount| maintenance || mount.supports_snapshots()).skip(page * ROWS_PER_PAGE).take(ROWS_PER_PAGE) {
            let selected = mount.clone(); let shared = model.clone();
            list = list.child(action(format!("mount-{}", mount.id), if mount.supports_snapshots() { t("Btrfs filesystem") } else { t("Filesystem") }, "disk", p, move |_, cx| {
                if shared.borrow().active.is_some() { return; }
                shared.borrow_mut().storage.selected = Some(selected.clone()); refresh(&shared, cx); shared.borrow().views.notify(cx);
            })).child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(mount.target.display().to_string()));
        }
        if count == 0 && let Some(message) = storage.empty_message(&mounts) { list = list.child(message); }
        if count > ROWS_PER_PAGE {
            let previous = model.clone(); let next = model.clone();
            let mut pages = div().row().gap(px(tokens::SM));
            if page > 0 { pages = pages.child(action("mounts-previous", t("Previous"), "chevron-left", p, move |_, cx| { previous.borrow_mut().storage.mount_page = page - 1; previous.borrow().views.notify(cx); })); }
            if (page + 1) * ROWS_PER_PAGE < count { pages = pages.child(action("mounts-next", t("Next"), "chevron-right", p, move |_, cx| { next.borrow_mut().storage.mount_page = page + 1; next.borrow().views.notify(cx); })); }
            list = list.child(pages);
        }
        body = body.child(list);
    }
    let shared = model.clone();
    body.child(action("storage-refresh", t("Refresh"), "refresh", p, move |_, cx| {
        if shared.borrow().active.is_none() { shared.borrow_mut().storage.clear(); refresh(&shared, cx); shared.borrow().views.notify(cx); }
    })).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount() -> Mount {
        Mount { id: 8, device: "0:33".into(), root: "/".into(), target: "/snapshots".into(), filesystem: "btrfs".into(), read_only: false, subvolume_id: Some(5) }
    }

    #[test]
    fn mutations_reload_the_visible_selection_and_inspection_keeps_its_reply() {
        let mount = mount();
        let mut state = State { opened: true, selected: Some(mount.clone()), ..State::default() };
        assert!(!state.complete(&mount, Reply::default(), true));
        assert!(state.elevated.is_some());
        for output in ["Snapshot created", "Snapshot deleted", "Maintenance finished"] {
            assert!(state.complete(&mount, Reply { output: output.into(), ..Reply::default() }, false));
            assert_eq!(state.notice.as_deref(), Some(output));
            assert!(state.elevated.is_none());
        }
        let mut other = mount.clone();
        other.id += 1;
        assert!(!state.complete(&other, Reply::default(), false));
        state.clear();
        assert!(!state.complete(&mount, Reply::default(), false));
        assert!(state.selected.is_none());
        assert!(state.notice.is_none());
        assert!(!state.native.borrow().busy());
    }

    #[test]
    fn cancelled_reads_defer_reopening_and_successful_admin_reads_clear_old_errors() {
        let mount = mount();
        let mut state = State { opened: true, selected: Some(mount.clone()), ..State::default() };
        state.native.borrow_mut().error = Some("permission denied".into());
        state.cancel_reads();
        assert!(!state.opened, "the completion redraw must retry a cancelled read");
        assert!(state.empty_message(&state.mounts.borrow()).is_none());
        assert!(!state.complete(&mount, Reply::default(), true));
        assert!(state.opened, "a successful privileged reply needs no unprivileged reload");
        assert!(state.elevated.is_some());
        assert!(state.native.borrow().error.is_none());
        assert_eq!(state.empty_message(&state.mounts.borrow()), Some(t("No supported filesystems found")));
        state.mounts.borrow_mut().error = Some("unreadable mount table".into());
        assert!(state.empty_message(&state.mounts.borrow()).is_none());
        state.clear();
        assert!(!state.complete(&mount, Reply::default(), true));
        assert!(!state.opened);
        assert!(state.elevated.is_none());
    }
}
