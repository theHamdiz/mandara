//! Power actions on FreeBSD when ConsoleKit2 isn't on the system bus: the
//! base system's `shutdown(8)` and `acpiconf(8)`, run as root through
//! pkexec, so Mandara's own password prompt asks for the administrator's
//! approval. Every command is a fixed argv; nothing from the caller reaches
//! it.

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

/// Whether a D-Bus failure means no login manager is there to ask (as
/// opposed to one that answered with a refusal).
pub fn bus_missing(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner" | "org.freedesktop.DBus.Error.UnknownMethod"),
        zbus::Error::InputOutput(_) | zbus::Error::Address(_) => true,
        _ => false,
    }
}

/// Runs `method`'s command through pkexec and waits for it.
#[cfg(target_os = "freebsd")]
pub fn run(method: &str) -> Result<(), String> {
    let argv = command(method).ok_or_else(|| mandara_i18n::t("FreeBSD can't do this without ConsoleKit2").to_string())?;
    if crate::sleep::dry_run() {
        tracing::info!("dry run: pkexec {}", argv.join(" "));
        return Ok(());
    }
    let status = std::process::Command::new(PKEXEC).args(argv).stdin(std::process::Stdio::null()).status().map_err(|e| format!("pkexec: {e}"))?;
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
    fn only_a_missing_manager_falls_back() {
        let missing = zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from("org.freedesktop.DBus.Error.ServiceUnknown").unwrap(), None, zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap());
        assert!(bus_missing(&missing));
        let denied = zbus::Error::MethodError(zbus::names::OwnedErrorName::try_from("org.freedesktop.ConsoleKit.Manager.NotPrivileged").unwrap(), None, zbus::message::Message::method_call("/", "Ping").unwrap().build(&()).unwrap());
        assert!(!bus_missing(&denied), "a refusal is never retried as root");
    }
}
