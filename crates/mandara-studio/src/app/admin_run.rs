//! Runs Mandara as root for Studio's administrator actions (login screen,
//! keyring unlock), always through a root-owned binary
//! ([`mandara_config::admin`]): never your own copy, which your account can
//! change. With none in place yet, or only one from an older version, the
//! same approval installs the admin helper first.

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

/// The message while waiting for the password.
pub(super) fn waiting_message() -> String {
    t("Waiting for authorization…").to_string()
}

/// `mandara <args>` as root (blocking: call off the UI thread).
pub(super) fn as_root(args: Vec<OsString>) -> Result<String, String> {
    // An older root-owned copy may not read what this build wrote (a login
    // screen draft with a newer element): refresh the helper first, in the
    // same approval.
    let (prog, args) = match admin::current_admin_exe() {
        Some(exe) => admin::pkexec(&exe, args),
        None => admin::setup_and_run(&std::env::current_exe().map_err(|e| e.to_string())?, args),
    };
    run(prog, args)
}
