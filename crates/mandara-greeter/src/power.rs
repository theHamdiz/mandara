//! Power actions over logind's D-Bus API (zbus, no external tools).
//!
//! In demo mode these are never called: [`PowerAction::run`] refuses
//! outright, so a test run on a live session can't suspend or power off
//! the machine.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Suspend,
    Reboot,
    PowerOff,
}

impl PowerAction {
    pub const ALL: [PowerAction; 3] = [PowerAction::Suspend, PowerAction::Reboot, PowerAction::PowerOff];

    pub fn method(self) -> &'static str {
        match self {
            PowerAction::Suspend => "Suspend",
            PowerAction::Reboot => "Reboot",
            PowerAction::PowerOff => "PowerOff",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            PowerAction::Suspend => "moon",
            PowerAction::Reboot => "restart",
            PowerAction::PowerOff => "power",
        }
    }

    /// The localized label.
    pub fn label(self) -> String {
        match self {
            PowerAction::Suspend => crate::i18n::tl("Sleep"),
            PowerAction::Reboot => crate::i18n::tl("Restart"),
            PowerAction::PowerOff => crate::i18n::tl("Shut Down"),
        }
    }

    /// Calls `org.freedesktop.login1.Manager.<method>(interactive = true)`
    /// (ConsoleKit2's manager on FreeBSD, which Mandara's polkit rule lets
    /// the login screen's user use) on a helper thread. FreeBSD without
    /// ConsoleKit2 runs the base system's `shutdown` / `acpiconf` instead.
    /// Always an error in demo mode.
    pub fn run(self, demo: bool) -> Result<(), String> {
        if demo {
            return Err(crate::i18n::tl("Power actions are disabled in the demo"));
        }
        std::thread::Builder::new()
            .name("mandara-greeter-power".into())
            .spawn(move || zbus::block_on(self.call()))
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "power thread crashed".to_string())?
    }

    async fn call(self) -> Result<(), String> {
        use mandara_services::logind::bus;
        let result = async {
            // FreeBSD: a bounded wait, and one more try when ConsoleKit2,
            // started by this call, left it unanswered.
            #[cfg(target_os = "freebsd")]
            let conn = zbus::connection::Builder::system()?.method_timeout(mandara_services::freebsd::power::MANAGER_TIMEOUT).build().await?;
            #[cfg(not(target_os = "freebsd"))]
            let conn = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(&conn, bus::DEST, bus::MANAGER_PATH, bus::MANAGER).await?;
            let result = proxy.call::<_, _, ()>(self.method(), &(true,)).await;
            #[cfg(target_os = "freebsd")]
            if result.as_ref().is_err_and(mandara_services::freebsd::power::timed_out) {
                tracing::info!("ConsoleKit2 left {} unanswered; asking again", self.method());
                return proxy.call::<_, _, ()>(self.method(), &(true,)).await;
            }
            result
        }
        .await;
        // FreeBSD: ConsoleKit2 answers when Mandara's polkit rule allows
        // the login screen's user. Without ConsoleKit2 (or a system bus)
        // the base system's tools run directly, which works only where an
        // administrator gave this user those rights. A refusal is never
        // retried another way; either failure points to the docs.
        #[cfg(target_os = "freebsd")]
        {
            use mandara_services::freebsd::power::{self as fbsd, LoginStep};
            match fbsd::login_step(&result) {
                LoginStep::Done => Ok(()),
                LoginStep::Direct => fbsd::run(self.method(), fbsd::Via::Direct).map_err(|e| fbsd::login_screen_error(&e)),
                LoginStep::Refused => Err(result.err().map(|e| fbsd::login_screen_error(&fbsd::short_error(&e))).unwrap_or_default()),
            }
        }
        #[cfg(not(target_os = "freebsd"))]
        result.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_never_reaches_logind() {
        for a in PowerAction::ALL {
            assert!(a.run(true).is_err());
        }
    }
}
