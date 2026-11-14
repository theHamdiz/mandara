//! One edit or Undo at a time, with the same completion for the panel and IPC.

use std::sync::{Arc, atomic::AtomicBool};
use gpui::App;
use mandara_i18n::t;
use mandara_services::app_permissions::Backend as _;
use super::{Capability, Flatpak, InstalledApp, Shared, Undo, select};

type Completion = async_channel::Receiver<Result<String, String>>;

fn report(model: &Shared, result: Result<Completion, String>, cx: &mut App) {
    if let Err(error) = result { model.borrow_mut().error = Some(error); model.borrow().views.notify(cx); }
}

pub(super) fn read(model: &Shared, work: impl FnOnce(&AtomicBool) -> Result<serde_json::Value, String> + Send + 'static, cx: &mut App) -> Result<Completion, String> {
    if model.borrow().active.is_some() { return Err(t("An action is still running").into()); }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut state = model.borrow_mut();
        state.cancel_reads();
        state.active = Some(cancel.clone());
        state.views.notify(cx);
    }
    let shared = model.clone();
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let reply = mandara_services::runtime::unblock(move || work(&cancel).and_then(|value| serde_json::to_string(&value).map_err(|error| error.to_string()))).await;
        shared.borrow_mut().active = None;
        cx.update(|cx| {
            let selected = shared.borrow().selected.clone();
            if shared.borrow().opened {
                if let Some(app) = selected { select(&shared, app, cx); }
                else { super::refresh(&shared, true, cx); }
            }
            shared.borrow().views.notify(cx);
        });
        let _ = tx.try_send(reply);
    }).detach();
    Ok(rx)
}

pub(super) fn change(model: &Shared, capability: Capability, allowed: bool, cx: &mut App) {
    let session = model.borrow().session.borrow().value.clone();
    let Some(session) = session else { return };
    let app = session.app.clone();
    report(model, perform(model, move |cancel| Flatpak::default().set(&session, &capability, allowed, cancel).map(|undo| (undo, app)).map_err(|error| format!("{error:#}")), cx), cx);
}

pub(super) fn perform(model: &Shared, work: impl FnOnce(&AtomicBool) -> Result<(Undo, InstalledApp), String> + Send + 'static, cx: &mut App) -> Result<Completion, String> {
    if model.borrow().active.is_some() { return Err(t("An action is still running").into()); }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut state = model.borrow_mut();
        state.cancel_reads();
        state.active = Some(cancel.clone());
        state.error = None;
        state.views.notify(cx);
    }
    let shared = model.clone();
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(move || work(&cancel)).await;
        let reply = result.as_ref().map(|(undo, app)| serde_json::json!({"changed":true,"app":app,"restart_required":true,"backup":undo.backup,"persistence_error":undo.persistence_error}).to_string()).map_err(Clone::clone);
        let selected = {
            let mut state = shared.borrow_mut();
            state.active = None;
            match result {
                Ok((undo, app)) => { state.error = undo.persistence_error.clone(); state.undo = Some(undo); state.changed = Some(app); }
                Err(error) => state.error = Some(error),
            }
            state.opened.then(|| state.selected.clone()).flatten()
        };
        cx.update(|cx| {
            if let Some(app) = selected { select(&shared, app, cx); }
            shared.borrow().views.notify(cx);
        });
        let _ = tx.try_send(reply);
    }).detach();
    Ok(rx)
}

pub(super) fn undo(model: &Shared, cx: &mut App) { report(model, undo_output(model, cx), cx); }

pub(super) fn undo_output(model: &Shared, cx: &mut App) -> Result<Completion, String> {
    let saved = {
        let mut state = model.borrow_mut();
        if state.active.is_some() { return Err(t("An action is still running").into()); }
        let saved = state.undo.take().ok_or_else(|| t("There is no app permission change to undo.").to_owned())?;
        state.cancel_reads();
        state.active = Some(Arc::new(AtomicBool::new(false)));
        saved
    };
    model.borrow().views.notify(cx);
    let shared = model.clone();
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let (saved, result) = mandara_services::runtime::unblock(move || { let result = saved.restore().map_err(|error| format!("{error:#}")); (saved, result) }).await;
        let reply = result.as_ref().map(|()| serde_json::json!({"restored":true,"restart_required":true}).to_string()).map_err(Clone::clone);
        let selected = {
            let mut state = shared.borrow_mut();
            state.active = None;
            match result {
                Ok(()) => { state.error = None; state.changed = None; }
                Err(error) => { state.error = Some(error); state.undo = Some(saved); }
            }
            state.opened.then(|| state.selected.clone()).flatten()
        };
        cx.update(|cx| {
            if let Some(app) = selected { select(&shared, app, cx); }
            shared.borrow().views.notify(cx);
        });
        let _ = tx.try_send(reply);
    }).detach();
    Ok(rx)
}
