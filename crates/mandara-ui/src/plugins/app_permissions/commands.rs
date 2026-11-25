//! Native permission commands share the panel's guarded writes and Undo receipt.

use std::sync::atomic::AtomicBool;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::app_permissions::{Backend, Flatpak, Key, Session};
use super::{ID, InstalledApp, actions, model};

fn lookup(backend: &impl Backend, id: &str, installation: &str, cancel: &AtomicBool) -> Result<InstalledApp, String> {
    backend.apps(cancel).map_err(|error| format!("{error:#}"))?.into_iter()
        .find(|app| app.id == id && app.installation == installation)
        .ok_or_else(|| t("Choose an installed app and its installation from permissions apps.").into())
}
fn inspect(id: &str, installation: &str, cancel: &AtomicBool) -> Result<Session, String> {
    let backend = Flatpak::default();
    let app = lookup(&backend, id, installation, cancel)?;
    backend.inspect(&app, cancel).map_err(|error| format!("{error:#}"))
}
fn group(name: &str) -> Result<Key, String> {
    match name { "shared" => Ok(Key::Shared), "sockets" => Ok(Key::Sockets), "devices" => Ok(Key::Devices), "filesystems" => Ok(Key::Filesystems), _ => Err(t("Choose shared, sockets, devices or filesystems.").into()) }
}

pub(super) fn register(registry: &mut PluginRegistry) {
    registry.command(ID, CommandSpec::new_output("permissions apps", tk("Installed Flatpak apps and their installation"), |_, cx| actions::read(&model(), |cancel| {
        Flatpak::default().apps(cancel).and_then(|apps| serde_json::to_value(apps).map_err(Into::into)).map_err(|error| format!("{error:#}"))
    }, cx).map(Output::Pending)));
    registry.command(ID, CommandSpec::new_output("permissions inspect", tk("Read an installed app's native sandbox grants"), |args, cx| {
        let id = args.str(0)?.to_owned(); let installation = args.str(1)?.to_owned();
        actions::read(&model(), move |cancel| inspect(&id, &installation, cancel)
            .map(|session| serde_json::json!({"app":session.app,"grants":session.capabilities,"native":session.metadata})), cx).map(Output::Pending)
    }).arg(ArgSpec::required("app", ArgKind::String, tk("Flatpak app id")))
      .arg(ArgSpec::required("installation", ArgKind::String, tk("Installation returned by permissions apps"))));
    registry.command(ID, CommandSpec::new_output("permissions set", tk("Change one editable personal grant and retain Undo"), |args, cx| {
        let id = args.str(0)?.to_owned(); let installation = args.str(1)?.to_owned();
        let key = group(args.str(2)?)?; let value = args.str(3)?.to_owned(); let allowed = args.bool(4)?;
        actions::perform(&model(), move |cancel| {
            let session = inspect(&id, &installation, cancel)?;
            let capability = session.capabilities.iter().find(|row| row.key == key && row.value == value && row.editable)
                .ok_or_else(|| t("Refresh the app permissions before changing this grant.").to_owned())?;
            Flatpak::default().set(&session, capability, allowed, cancel).map(|undo| (undo, session.app)).map_err(|error| format!("{error:#}"))
        }, cx).map(Output::Pending)
    }).arg(ArgSpec::required("app", ArgKind::String, tk("Flatpak app id")))
      .arg(ArgSpec::required("installation", ArgKind::String, tk("Installation returned by permissions apps")))
      .arg(ArgSpec::required("group", ArgKind::Enum, tk("Native permission group")).choices(&["shared", "sockets", "devices", "filesystems"]))
      .arg(ArgSpec::required("grant", ArgKind::String, tk("Editable native grant returned by permissions inspect")))
      .arg(ArgSpec::required("allowed", ArgKind::Bool, tk("Allow or deny this grant"))));
    registry.command(ID, CommandSpec::new_output("permissions undo", tk("Undo the last app permission change while its file is unchanged"), |_, cx| actions::undo_output(&model(), cx).map(Output::Pending)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_group_names_are_a_closed_domain() {
        for (name, expected) in [("shared", Key::Shared), ("sockets", Key::Sockets), ("devices", Key::Devices), ("filesystems", Key::Filesystems)] { assert_eq!(group(name).unwrap(), expected); }
        assert!(group("environment").is_err());
        assert!(group("--reset").is_err());
    }
}
