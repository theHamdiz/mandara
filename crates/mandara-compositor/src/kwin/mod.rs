//! KWin's native Plasma window and virtual-desktop protocols.

mod protocol;
mod power;
mod session;
pub mod state;
mod wayland;

use anyhow::{Result, bail};

use crate::{ClientInfo, CompositorBackend, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel};
pub use session::{Session, WindowAction};

pub struct KWinDriver;

impl KWinDriver {
    pub fn from_env() -> Option<Self> {
        Session::open().is_ok().then_some(Self)
    }
}

impl CompositorBackend for KWinDriver {
    fn kind(&self) -> CompositorKind { CompositorKind::KWin }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let _ = std::thread::Builder::new().name("kwin-status".into()).spawn(move || {
            loop {
                let result = (|| -> Result<()> {
                    let mut session = Session::open()?;
                    let mut diff = state::Diff::default();
                    loop {
                        for event in diff.events(session.state()) {
                            if tx.send_blocking(event).is_err() { return Ok(()); }
                        }
                        if !session.wait(None)? { return Ok(()); }
                    }
                })();
                if result.is_ok() || tx.is_closed() { return; }
                std::thread::sleep(crate::RECONNECT_DELAY);
            }
        });
        rx
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> { Ok(Session::open()?.state().navigation()) }
    fn get_active_client(&self) -> Result<Option<ClientInfo>> { Ok(Session::open()?.state().focused()) }
    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> { Ok(Session::open()?.state().clients()) }
    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> { Ok(Session::open()?.state().monitors()) }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        let mut session = Session::open()?;
        session.activate_desktop(monitor, id)?;
        session.roundtrip()
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> { self.select_primary(monitor, id) }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        let mut session = Session::open()?;
        session.move_window(client_id, id, false)?;
        session.roundtrip()
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        let mut session = Session::open()?;
        session.move_window(client_id, tag, true)?;
        session.roundtrip()
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> { bail!("{}", mandara_i18n::t("KWin settings are managed in Plasma System Settings")) }
    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        match (cmd, args) {
            ("dpms", ["on" | "off"]) => {
                let mut session = Session::open()?;
                session.set_power(args[0] == "on")?;
                session.roundtrip()
            }
            _ => bail!("{}", mandara_i18n::t("Not supported by this compositor")),
        }
    }

    fn reload_config(&self) -> Result<()> {
        // KWin consumes no Mandara drop-in; native requests apply directly.
        Ok(())
    }

    fn sync_managed_config(&self, config: &mandara_config::compositor::CompositorConfig, borders: Option<mandara_config::compositor::BorderColors>) -> Result<(std::path::PathBuf, mandara_config::compositor::Emitted)> {
        mandara_config::kwin::sync_autostart(config.autostart, &mandara_config::paths::self_exe())?;
        crate::backend::write_dropin(mandara_config::compositor::Target::KWin, config, borders, self.version())
    }
}
