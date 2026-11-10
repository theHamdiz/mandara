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
    /// (ConsoleKit2's manager on FreeBSD) on a helper thread. FreeBSD
    /// without ConsoleKit2 runs the base system's `shutdown` / `acpiconf`
    /// instead. Always an error in demo mode.
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
            let conn = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(&conn, bus::DEST, bus::MANAGER_PATH, bus::MANAGER).await?;
            proxy.call::<_, _, ()>(self.method(), &(true,)).await
        }
        .await;
        // No login manager to ask (no ConsoleKit2, or no system bus): the
        // base system's tools, run directly, since the login screen has no
        // password prompt for pkexec; FreeBSD allows them to the `operator`
        // group. A manager that refuses keeps its error.
        #[cfg(target_os = "freebsd")]
        if let Err(e) = &result
            && mandara_services::freebsd::power::bus_missing(e)
        {
            return mandara_services::freebsd::power::run(self.method(), mandara_services::freebsd::power::Via::Direct);
        }
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
