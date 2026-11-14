//! One explicit operation, with a replaceable progress value and no idle poller.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use gpui::App;
use mandara_i18n::{t, tk};
use mandara_services::backups::{Backend as _, Operation, Progress, Restic};
use super::{Shared, preferences};

/// Visible progress redraws at most four times per second.
const PROGRESS_MILLIS: u64 = 250;
pub(super) const PROGRESS_TICK: Duration = Duration::from_millis(PROGRESS_MILLIS);
/// Opening the panel is explicit; this age only coalesces repeated renders.
const HISTORY_SECONDS: u64 = 24 * 60 * 60;
const HISTORY_AGE: Duration = Duration::from_secs(HISTORY_SECONDS);

pub(super) fn cancel(model: &Shared, cx: &mut App) {
    let mut state = model.borrow_mut();
    if let Some(cancel) = &state.active {
        cancel.store(true, Ordering::Release);
        state.status = tk("Cancelling…");
    }
    state.views.notify(cx);
}

pub(super) fn refresh(model: &Shared, force: bool, cx: &mut App) {
    if model.borrow().active.is_some() || model.borrow().page != super::Page::Archive { return; }
    let preferences = preferences(cx);
    let settings = preferences.0.clone();
    let language = mandara_i18n::language();
    let format = preferences.1.format;
    let history = model.borrow().history.clone();
    if !preferences.1.archive_ready { history.borrow_mut().select((settings, language, format)); return; }
    let cancel = model.borrow().read_cancel.clone();
    super::super::integration::load(&history, (settings.clone(), language, format), force, HISTORY_AGE,
        move |_| Restic.snapshots(&settings, &cancel).map(|rows| rows.into_iter().map(|row| super::timeline::Row::new(row, language, format)).collect()).map_err(|error| format!("{error:#}")), cx);
}

fn check_scope(expected: &mandara_services::backups::Settings, current: &mandara_services::backups::Settings) -> Result<(), String> {
    if expected == current { Ok(()) }
    else { Err(t("Backup settings changed. Refresh the history and choose the snapshot again.").into()) }
}

pub(super) fn start_scoped(model: &Shared, expected: &mandara_services::backups::Settings, operation: Operation, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    check_scope(expected, &preferences(cx).0)?;
    start(model, operation, cx)
}

pub(super) fn start(model: &Shared, operation: Operation, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    if model.borrow().active.is_some() { return Err(t("An action is still running").into()); }
    let settings = preferences(cx).0.clone();
    settings.validate().map_err(|error| error.to_string())?;
    let status = match &operation {
        Operation::Initialize => tk("Initializing repository…"),
        Operation::Backup => tk("Backing up…"),
        Operation::Check => tk("Checking repository…"),
        Operation::Restore { .. } => tk("Restoring copies…"),
    };
    let progress = model.borrow().progress.clone();
    let scope = (settings.repository.clone(), settings.password_file.clone());
    perform(model, status, move |token| Restic.execute(&settings, operation, &token, &mut |value| {
        *progress.lock().unwrap_or_else(|error| error.into_inner()) = value;
    }).map_err(|error| format!("{error:#}")), move |summary, model, _| {
        let mut state = model.borrow_mut();
        state.summary = Some(summary);
        state.summary_scope = Some(scope);
    }, cx)
}

/// The archive, native filesystem and cleanup tabs share one operation slot.
pub(super) fn perform<T: serde::Serialize + Send + 'static>(
    model: &Shared, status: &'static str,
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T, String> + Send + 'static,
    done: impl FnOnce(T, &Shared, &mut App) + 'static,
    cx: &mut App,
) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    if model.borrow().active.is_some() { return Err(t("An action is still running").into()); }
    let cancel = Arc::new(AtomicBool::new(false));
    let token = cancel.clone();
    {
        let mut state = model.borrow_mut();
        state.read_cancel.store(true, Ordering::Release);
        state.status = status;
        state.active = Some(cancel);
        state.error = None;
        state.summary = None;
        state.summary_scope = None;
        *state.progress.lock().unwrap_or_else(|error| error.into_inner()) = Progress::default();
        state.history.borrow_mut().invalidate();
        state.storage.cancel_reads();
        state.cleanup.cancel_reads();
        state.views.notify(cx);
    }
    let model = model.clone();
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(move || work(token)).await;
        let reply = result.as_ref().map_err(Clone::clone).and_then(|value| serde_json::to_string(value).map_err(|error| error.to_string()));
        model.borrow_mut().active = None;
        cx.update(|cx| {
            match result { Ok(value) => done(value, &model, cx), Err(error) => model.borrow_mut().error = Some(error) }
            if model.borrow().opened && super::super::lifecycle::is_on(super::ID, cx) {
                model.borrow_mut().read_cancel = Arc::new(AtomicBool::new(false));
                refresh(&model, true, cx);
            }
            model.borrow().views.notify(cx);
        });
        let _ = tx.try_send(reply);
    }).detach();
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::backups::Settings;

    #[test]
    fn a_restore_picker_cannot_switch_repositories_while_open() {
        let selected = Settings { repository: "/first".into(), password_file: "/secret".into(), sources: Vec::new() };
        assert!(check_scope(&selected, &selected).is_ok());
        let mut changed = selected.clone();
        changed.repository = "/second".into();
        assert!(check_scope(&selected, &changed).is_err());
        changed.repository = selected.repository.clone();
        changed.password_file = "/other-secret".into();
        assert!(check_scope(&selected, &changed).is_err());
    }
}
