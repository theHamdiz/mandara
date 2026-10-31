//! Runs Mandara as root for Studio's administrator actions (login screen,
//! keyring unlock), always through a root-owned binary
//! ([`mandara_config::admin`]): never your own copy, which your account can
//! change. With none in place yet, the admin helper is installed first,
//! with one more approval.

use std::ffi::OsString;

use mandara_config::admin;
use mandara_i18n::t;

/// Runs argv; Ok(stdout) on success, Err(stderr or stdout) otherwise.
pub(super) fn run(prog: OsString, args: Vec<OsString>) -> Result<String, String> {
    match std::process::Command::new(&prog).args(&args).stdin(std::process::Stdio::null()).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Err([out, err].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"))
        }
        Err(e) => Err(format!("{}: {e}", prog.to_string_lossy())),
    }
}

/// The message while waiting: a first run also installs the helper.
pub(super) fn waiting_message() -> String {
    if admin::admin_exe().is_some() { t("Waiting for authorization…") } else { t("Setting up Mandara's admin helper first (one more approval, only this once)…") }.to_string()
}

/// `mandara <args>` as root (blocking: call off the UI thread).
pub(super) fn as_root(args: Vec<OsString>) -> Result<String, String> {
    let admin = match admin::admin_exe() {
        Some(exe) => exe,
        None => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let (prog, setup) = admin::setup_command(&exe);
            run(prog, setup)?;
            admin::admin_exe().ok_or_else(|| t("The admin helper couldn't be installed, so nothing was changed.").to_string())?
        }
    };
    let (prog, args) = admin::pkexec(&admin, args);
    run(prog, args)
}
