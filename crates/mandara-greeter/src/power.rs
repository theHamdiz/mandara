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
    /// on a helper thread. Always an error in demo mode.
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
        let conn = zbus::Connection::system().await.map_err(|e| e.to_string())?;
        let proxy = zbus::Proxy::new(&conn, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")
            .await
            .map_err(|e| e.to_string())?;
        proxy.call::<_, _, ()>(self.method(), &(true,)).await.map_err(|e| e.to_string())
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
