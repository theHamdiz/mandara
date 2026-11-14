//! Flatpak supplies the effective metadata and validates each override's native syntax.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;
use mandara_theme::templates::FileStamp;

use super::{App, Backend, Capability, Session, Undo, model};

/// Installed apps and permissions fit below the shared native-command output limit.
const MAX_APPS: usize = 512;
pub(super) const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Flatpak { pub(super) data: PathBuf }
impl Default for Flatpak {
    fn default() -> Self {
        Self { data: user_directory(std::env::var_os("FLATPAK_USER_DIR").as_deref()) }
    }
}

fn user_directory(custom: Option<&OsStr>) -> PathBuf {
    custom.filter(|path| !path.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| mandara_config::paths::data_dir().with_file_name("flatpak"))
}

fn permissions(metadata: &str) -> crate::app_install::flatpak::Permissions {
    // A successful native read can omit Context when the app has no grants.
    crate::app_install::flatpak::parse_context(metadata).unwrap_or_default()
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() <= 255 && id.split('.').count() >= 3
        && id.split('.').all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
}

fn select_installation(command: &mut Command, installation: &str) -> Result<()> {
    ensure!(!installation.is_empty() && !installation.starts_with('-') && !installation.contains(char::is_control), "invalid Flatpak installation");
    match installation {
        "user" => { command.arg("--user"); }
        "system" | "default" => { command.arg("--system"); }
        name => { command.arg(format!("--installation={name}")); }
    }
    Ok(())
}

pub(super) fn run(command: Command, cancel: &AtomicBool) -> Result<String> {
    crate::app_themes::process::native(command, cancel, crate::app_themes::process::TIMEOUT)
        .map(|output| output.0).context(t("Could not read Flatpak. Install Flatpak to use App permissions."))
}

pub(super) fn apps(text: &str) -> Result<Vec<App>> {
    let mut rows = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        ensure!(rows.len() < MAX_APPS, "too many installed Flatpak apps");
        let mut fields = line.split('\t');
        let id = fields.next().unwrap_or_default();
        let name = fields.next().unwrap_or(id);
        let installation = fields.next().unwrap_or("user");
        ensure!(valid_id(id) && !installation.is_empty() && !installation.starts_with('-') && !installation.contains(char::is_control), "invalid Flatpak app record");
        rows.push(App { id: id.into(), name: name.into(), installation: installation.into() });
    }
    rows.sort_unstable_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)).then_with(|| a.installation.cmp(&b.installation)));
    Ok(rows)
}

impl Backend for Flatpak {
    fn apps(&self, cancel: &AtomicBool) -> Result<Vec<App>> {
        ensure!(cfg!(target_os = "linux"), "{}", t("App permissions are available for Flatpak apps on Linux."));
        let mut command = Command::new("flatpak");
        command.args(["list", "--app", "--columns=application,name,installation"]).env("LC_ALL", "C");
        apps(&run(command, cancel)?)
    }

    fn inspect(&self, app: &App, cancel: &AtomicBool) -> Result<Session> {
        ensure!(cfg!(target_os = "linux"), "{}", t("App permissions are available for Flatpak apps on Linux."));
        ensure!(valid_id(&app.id), "invalid Flatpak app id");
        let mut command = Command::new("flatpak");
        command.args(["info", "--show-permissions"]);
        select_installation(&mut command, &app.installation)?;
        command.arg(&app.id);
        let metadata = run(command, cancel)?;
        let permissions = permissions(&metadata);
        let path = self.data.join("overrides").join(&app.id);
        let target = mandara_theme::templates::destination::identity(&path)?;
        let stamp = FileStamp::at(&target)?;
        let before = super::transaction::read(&target)?;
        ensure!(FileStamp::at(&target)? == stamp && mandara_theme::templates::destination::identity(&path)? == target, "{}", t("The app permissions changed. Refresh before saving."));
        Ok(Session { app: app.clone(), capabilities: model::capabilities(&permissions), permissions, metadata, path, target, before, stamp })
    }

    fn set(&self, session: &Session, capability: &Capability, allowed: bool, cancel: &AtomicBool) -> Result<Undo> {
        ensure!(capability.editable && session.capabilities.contains(capability), "{}", t("Refresh the app permissions before changing this grant."));
        super::transaction::apply(session, capability, allowed, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_user_installations_and_empty_permission_groups_are_supported() {
        assert_eq!(user_directory(Some(OsStr::new("/custom/apps"))), PathBuf::from("/custom/apps"));
        assert_eq!(user_directory(Some(OsStr::new(""))), user_directory(None));
        assert!(permissions("").is_empty());
        assert!(permissions("[Environment]\nLANG=en_US.UTF-8\n").is_empty());
        assert_eq!(permissions("[Context]\nshared=network;\n").shared, ["network"]);
    }

    #[test]
    fn default_user_and_named_installations_use_their_native_flags() {
        for (installation, flag) in [("system", "--system"), ("default", "--system"), ("user", "--user"), ("extra apps", "--installation=extra apps")] {
            let mut command = Command::new("flatpak");
            select_installation(&mut command, installation).unwrap();
            assert_eq!(command.get_args().collect::<Vec<_>>(), [flag]);
        }
        for invalid in ["", "--system", "system\n"] {
            assert!(select_installation(&mut Command::new("flatpak"), invalid).is_err());
        }
    }
}
