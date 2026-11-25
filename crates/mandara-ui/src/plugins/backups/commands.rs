//! Backup commands and the same two-step filesystem review used by the panel.

use std::path::PathBuf;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::backups::{Backend as _, Operation, Restic};
use mandara_services::backups::filesystem::{Request, mounted};
use super::{ID, Page, jobs, model, preferences, storage};

fn request(document: &str) -> Result<Request, String> {
    if document.len() > mandara_services::backups::filesystem::admin::MAX_REQUEST_BYTES { return Err(t("The filesystem request is too large.").into()); }
    serde_json::from_str(document).map_err(|error| error.to_string())
}

pub(super) fn register(registry: &mut PluginRegistry) {
    registry.command(ID, CommandSpec::new_output("backups list", tk("Recent encrypted backup snapshots"), |_, cx| {
        let settings = preferences(cx).0.clone();
        jobs::perform(&model(), tk("Loading backup history…"), move |cancel| Restic.snapshots(&settings, &cancel).map_err(|error| format!("{error:#}")), |_, _, _| {}, cx).map(Output::Pending)
    }));
    for (command, help, operation) in [
        ("backups start", tk("Start an encrypted backup of the configured folders"), Operation::Backup),
        ("backups check", tk("Check the encrypted backup repository's integrity"), Operation::Check),
        ("backups init", tk("Initialize a new encrypted backup repository"), Operation::Initialize),
    ] {
        registry.command(ID, CommandSpec::new_output(command, help, move |_, cx| jobs::start(&model(), operation.clone(), cx).map(Output::Pending)));
    }
    registry.command(ID, CommandSpec::new_output("backups restore", tk("Restore a backup into a fresh copy folder"), |args, cx| {
        let snapshot = args.str(0)?.to_owned(); let parent = PathBuf::from(args.str(1)?);
        jobs::start(&model(), Operation::Restore { snapshot, parent }, cx).map(Output::Pending)
    }).arg(ArgSpec::required("snapshot", ArgKind::String, tk("Full snapshot id from backups list")))
      .arg(ArgSpec::required("parent", ArgKind::String, tk("Existing absolute folder for restored copies"))));
    registry.command(ID, CommandSpec::new("backups cancel", tk("Cancel Mandara's active storage operation or review"), |_, cx| {
        let shared = model();
        jobs::cancel(&shared, cx);
        storage::cancel_preview(&shared);
        super::cleanup::cancel_review(&shared);
        shared.borrow().views.notify(cx);
        Ok(serde_json::json!({"cancel_requested":true,"message":t("Partial restored copies are kept.")}))
    }));
    registry.command(ID, CommandSpec::new_output("backups storage mounts", tk("Mounted filesystems and their current identities"), |_, cx| {
        jobs::perform(&model(), tk("Reading filesystems…"), |_| mounted().map_err(|error| format!("{error:#}")), |_, _, _| {}, cx).map(Output::Pending)
    }));
    registry.command(ID, CommandSpec::new_output("backups storage inspect", tk("Read a selected Btrfs mount with administrator approval"), |args, cx| {
        let selected = request(args.str(0)?)?;
        if !matches!(selected, Request::Inspect { .. }) { return Err(t("Use an inspect request to read snapshots.").into()); }
        storage::apply_output(&model(), selected, cx).map(Output::Pending)
    }).arg(ArgSpec::required("request", ArgKind::String, tk("JSON inspect request containing a current mount"))));
    registry.command(ID, CommandSpec::new_output("backups storage preview", tk("Review a native filesystem request without changing files"), |args, cx| {
        let selected = request(args.str(0)?)?;
        jobs::perform(&model(), tk("Reviewing filesystem action…"), move |_| {
            let mounts = mounted().map_err(|error| format!("{error:#}"))?;
            if !mounts.iter().any(|mount| mount == selected.mount()) { return Err(t("The filesystem mount changed. Refresh before continuing.").into()); }
            Ok(selected)
        }, |request, model, cx| {
            model.borrow_mut().select_page(Page::Snapshots);
            storage::review(model, request, cx);
        }, cx).map(Output::Pending)
    }).arg(ArgSpec::required("request", ArgKind::String, tk("JSON create, delete or maintenance request to review"))));
    registry.command(ID, CommandSpec::new_output("backups storage confirm", tk("Run the last reviewed filesystem action with administrator approval"), |_, cx| storage::confirm(&model(), cx).map(Output::Pending)));
    super::cleanup::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reviewed_requests_reject_unknown_actions_and_bound_input_before_decoding() {
        assert!(request(r#"{"action":"format","device":"/dev/sda"}"#).is_err());
        assert!(request(&" ".repeat(mandara_services::backups::filesystem::admin::MAX_REQUEST_BYTES + 1)).is_err());
    }
}
