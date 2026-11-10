//! Power actions on FreeBSD when ConsoleKit2 isn't on the system bus: the
//! base system's `shutdown(8)` and `acpiconf(8)`. In a user's session they
//! run as root through pkexec, so Mandara's own password prompt asks for
//! the administrator's approval; the login screen runs them directly
//! ([`Via`]). Every command is a fixed argv; nothing from the caller
//! reaches it.
//!
//! The login screen's own path is ConsoleKit2: Mandara's polkit rule
//! (`mandara greeter power-rule`) lets greetd's user, and nobody else,
//! shut down, restart and suspend through it. [`login_step`] decides what
//! happens after ConsoleKit2 answered.

/// pkexec from the polkit package.
pub const PKEXEC: &str = "/usr/local/bin/pkexec";

/// The argv for a login-manager method (`PowerOff`, `Reboot`, `Suspend`),
/// or `None` for the ones FreeBSD's tools don't offer.
pub fn command(method: &str) -> Option<&'static [&'static str]> {
    match method {
        "PowerOff" => Some(&["/sbin/shutdown", "-p", "now"]),
        "Reboot" => Some(&["/sbin/shutdown", "-r", "now"]),
        // S3, suspend to RAM.
        "Suspend" => Some(&["/usr/sbin/acpiconf", "-s", "3"]),
        _ => None,
    }
}

/// How a fallback command gets the rights it needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Via {
    /// pkexec, answered by Mandara's password prompt (a user's session).
    Pkexec,
    /// As the caller: only root and the `operator` group may run
    /// `shutdown` (setuid root for that group) or suspend through
    /// `/dev/acpi`. Mandara never adds anyone to `operator`, which can
    /// also read raw disks; the login screen tries this only when
    /// ConsoleKit2 isn't there, for a user an administrator gave those
    /// rights to.
    Direct,
}

/// The program and arguments that run `method` [`Via`] `via`.
pub fn program(method: &str, via: Via) -> Option<(&'static str, &'static [&'static str])> {
    let argv = command(method)?;
    match via {
        Via::Pkexec => Some((PKEXEC, argv)),
        Via::Direct => argv.split_first().map(|(prog, args)| (*prog, args)),
    }
}

/// Whether a D-Bus failure means no login manager is there to ask (as
/// opposed to one that answered with a refusal).
pub fn bus_missing(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner" | "org.freedesktop.DBus.Error.UnknownMethod"),
        // No system bus to reach (zbus reports a missing socket as
        // `Connection`).
        zbus::Error::InputOutput(_) | zbus::Error::Address(_) | zbus::Error::Connection(..) => true,
        _ => false,
    }
}

/// Where the login screen's power buttons on FreeBSD are explained.
pub const LOGIN_SCREEN_DOC: &str = "docs/login-manager.md#freebsd";

/// How long the login screen waits for ConsoleKit2's answer. A
/// ConsoleKit2 that D-Bus starts for this very call can leave it
/// unanswered while polkit starts up (FreeBSD 15.1, ConsoleKit2 2.0): the
/// login screen then asks once more, and the second call is answered.
pub const MANAGER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// Whether a call ran into [`MANAGER_TIMEOUT`] (the connection's method
/// timeout) instead of getting an answer.
pub fn timed_out(e: &zbus::Error) -> bool {
    matches!(e, zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut)
}

/// What the login screen does once ConsoleKit2 has answered.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LoginStep {
    /// ConsoleKit2 did it.
    Done,
    /// No ConsoleKit2 to ask: the tools [`Via::Direct`], which work only
    /// where an administrator gave the login screen's user those rights.
    Direct,
    /// ConsoleKit2 refused (no polkit rule for this user, or another user
    /// is logged in). Never retried another way.
    Refused,
}

/// The login screen's next step after asking ConsoleKit2.
pub fn login_step(result: &zbus::Result<()>) -> LoginStep {
    match result {
        Ok(()) => LoginStep::Done,
        Err(e) if bus_missing(e) => LoginStep::Direct,
        Err(_) => LoginStep::Refused,
    }
}

/// The login screen's message when no path may do it: what went wrong,
/// and where the setup is explained.
pub fn login_screen_error(error: &dyn std::fmt::Display) -> String {
    mandara_i18n::tf("The login screen isn't allowed to do this ({error}). See {doc}", &[("error", error), ("doc", &LOGIN_SCREEN_DOC)])
}

/// A D-Bus error as the login screen shows it: a refusal's own
/// description ("Not Authorized") rather than its long error name.
pub fn short_error(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, description, _) => description.clone().unwrap_or_else(|| name.to_string()),
        e => e.to_string(),
    }
}

/// Runs `method`'s command [`Via`] `via` and waits for it.
#[cfg(target_os = "freebsd")]
pub fn run(method: &str, via: Via) -> Result<(), String> {
    let (Some(argv), Some((prog, args))) = (command(method), program(method, via)) else {
        return Err(mandara_i18n::t("FreeBSD can't do this without ConsoleKit2").to_string());
    };
    if crate::sleep::dry_run() {
        tracing::info!("dry run: {prog} {}", args.join(" "));
        return Ok(());
    }
    let status = std::process::Command::new(prog).args(args).stdin(std::process::Stdio::null()).status().map_err(|e| format!("{prog}: {e}"))?;
    if status.success() { Ok(()) } else { Err(mandara_i18n::tf("{command} failed ({status})", &[("command", &argv.join(" ")), ("status", &status)])) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_commands_for_the_power_actions() {
        assert_eq!(command("PowerOff"), Some(&["/sbin/shutdown", "-p", "now"][..]));
        assert_eq!(command("Reboot"), Some(&["/sbin/shutdown", "-r", "now"][..]));
        assert_eq!(command("Suspend"), Some(&["/usr/sbin/acpiconf", "-s", "3"][..]));
        assert_eq!(command("Hibernate"), None, "S4 is rarely supported on FreeBSD");
        assert_eq!(command("SuspendThenHibernate"), None);
    }

    #[test]
    fn the_login_screen_runs_the_tools_without_pkexec() {
        assert_eq!(program("Reboot", Via::Pkexec), Some((PKEXEC, &["/sbin/shutdown", "-r", "now"][..])));
        assert_eq!(program("PowerOff", Via::Direct), Some(("/sbin/shutdown", &["-p", "now"][..])));
        assert_eq!(program("Suspend", Via::Direct), Some(("/usr/sbin/acpiconf", &["-s", "3"][..])));
        assert_eq!(program("Hibernate", Via::Direct), None);
    }

    /// No system bus at all (its socket is missing) is a missing manager
    /// too, so the connection error reaches the fallback.
    #[test]
    fn an_unreachable_system_bus_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let address = format!("unix:path={}", dir.path().join("no-bus").display());
        let err = crate::runtime::block_on(async { zbus::connection::Builder::address(address.as_str())?.build().await }).unwrap_err();
        assert!(bus_missing(&err), "{err:?}");
    }

    fn method_error(name: &str, description: Option<&str>) -> zbus::Error {
        zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from(name).unwrap(), description.map(str::to_string), zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap())
    }

    /// The login screen: ConsoleKit2 first; only a missing ConsoleKit2
    /// leads to the tools, and a refusal is never retried another way.
    #[test]
    fn the_login_screen_asks_consolekit_first() {
        assert_eq!(login_step(&Ok(())), LoginStep::Done);
        assert_eq!(login_step(&Err(method_error("org.freedesktop.DBus.Error.ServiceUnknown", None))), LoginStep::Direct);
        assert_eq!(login_step(&Err(method_error("org.freedesktop.DBus.Error.NameHasNoOwner", None))), LoginStep::Direct);
        assert_eq!(login_step(&Err(zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::from(std::io::ErrorKind::NotFound))))), LoginStep::Direct);
        let refused = || method_error("org.freedesktop.ConsoleKit.Manager.Error.InsufficientPermission", Some("Not Authorized"));
        assert_eq!(login_step(&Err(refused())), LoginStep::Refused);
        assert_eq!(short_error(&refused()), "Not Authorized");
        assert_eq!(short_error(&method_error("org.freedesktop.ConsoleKit.Manager.Error.Busy", None)), "org.freedesktop.ConsoleKit.Manager.Error.Busy");
    }

    /// An unanswered call is asked again (zbus's method timeout is an
    /// `InputOutput` error of kind `TimedOut`); a missing socket isn't one.
    #[test]
    fn only_an_unanswered_call_counts_as_timed_out() {
        let io = |kind| zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::from(kind)));
        assert!(timed_out(&io(std::io::ErrorKind::TimedOut)));
        assert!(!timed_out(&io(std::io::ErrorKind::NotFound)));
        assert!(!timed_out(&method_error("org.freedesktop.DBus.Error.NoReply", None)));
    }

    #[test]
    fn the_login_screen_error_points_to_the_docs() {
        let text = mandara_i18n::with_language(Some(mandara_i18n::Lang::En), || login_screen_error(&"Not Authorized"));
        assert_eq!(text, "The login screen isn't allowed to do this (Not Authorized). See docs/login-manager.md#freebsd");
    }

    #[test]
    fn only_a_missing_manager_falls_back() {
        let missing = zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from("org.freedesktop.DBus.Error.ServiceUnknown").unwrap(), None, zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap());
        assert!(bus_missing(&missing));
        let denied = zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from("org.freedesktop.ConsoleKit.Manager.NotPrivileged").unwrap(), None, zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap());
        assert!(!bus_missing(&denied), "a refusal is never retried as root");
    }
}
