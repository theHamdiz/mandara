//! Power actions on FreeBSD when ConsoleKit2 isn't on the system bus: the
//! base system's `shutdown(8)` and `acpiconf(8)`. In a user's session they
//! run as root through pkexec, so Mandara's own password prompt asks for
//! the administrator's approval; the login screen runs them directly
//! ([`Via`]). Every command is a fixed argv; nothing from the caller
//! reaches it.

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
    /// As the caller: FreeBSD lets the `operator` group run `shutdown`
    /// (setuid root for that group) and open `/dev/acpi` for
    /// `acpiconf -s`. The login screen has no password prompt for pkexec
    /// to ask through, so its greeter user needs to be in `operator`.
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

    #[test]
    fn only_a_missing_manager_falls_back() {
        let missing = zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from("org.freedesktop.DBus.Error.ServiceUnknown").unwrap(), None, zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap());
        assert!(bus_missing(&missing));
        let denied = zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from("org.freedesktop.ConsoleKit.Manager.NotPrivileged").unwrap(), None, zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap());
        assert!(!bus_missing(&denied), "a refusal is never retried as root");
    }
}
