//! A persistent Plasma connection shared by the compositor and window actors.

use std::io::{PipeReader, Read as _};

use anyhow::{Context as _, Result};
use wayland_client::{Connection, EventQueue, Proxy as _};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::State as Flag;

use super::state::State;
use super::wayland::{self, Wl};

#[derive(Clone, Debug)]
pub enum WindowAction {
    Activate,
    Close,
    Minimized(bool),
    Maximized(bool),
    Fullscreen(bool),
}

pub struct Session {
    connection: Connection,
    queue: EventQueue<Wl>,
    wl: Wl,
}

impl Session {
    pub fn open() -> Result<Self> {
        let (connection, queue, wl) = wayland::open()?;
        Ok(Self { connection, queue, wl })
    }

    pub fn state(&self) -> &State {
        &self.wl.state
    }

    pub fn roundtrip(&mut self) -> Result<()> {
        wayland::roundtrip(&self.connection, &mut self.queue, &mut self.wl)
    }

    pub fn activate_desktop(&mut self, monitor: &str, id: u32) -> Result<()> {
        let uuid = self.wl.state.desktop_uuid(id).context(mandara_i18n::t("Workspace not found").to_string())?;
        let desktop = self.wl.desktops.get(uuid).context(mandara_i18n::t("Workspace not found").to_string())?;
        if desktop.version() >= 4 && !monitor.is_empty() {
            desktop.request_enter_output(monitor.to_string());
        } else {
            desktop.request_activate();
        }
        self.connection.flush()?;
        Ok(())
    }

    pub fn move_window(&mut self, id: Option<u64>, desktop: u32, toggle: bool) -> Result<()> {
        let uuid = self.wl.state.window_uuid(id).context(mandara_i18n::t("The window is no longer available").to_string())?;
        let target = self.wl.state.desktop_uuid(desktop).context(mandara_i18n::t("Workspace not found").to_string())?;
        let window = self.wl.windows.get(uuid).context(mandara_i18n::t("The window is no longer available").to_string())?;
        let current = &self.wl.state.windows.get(uuid).context(mandara_i18n::t("The window is no longer available").to_string())?.desktops;
        if toggle && current.iter().any(|d| d == target) {
            window.request_leave_virtual_desktop(target.to_string());
        } else {
            // Enter first: briefly leaving all desktops would make the
            // window visible on every desktop.
            window.request_enter_virtual_desktop(target.to_string());
            if !toggle {
                for old in current.iter().filter(|d| d.as_str() != target) {
                    window.request_leave_virtual_desktop(old.clone());
                }
                window.set_state(Flag::OnAllDesktops as u32, 0);
            }
        }
        self.connection.flush()?;
        Ok(())
    }

    pub fn window_action(&mut self, id: u64, action: &WindowAction) -> Result<()> {
        let uuid = self.wl.state.window_uuid(Some(id)).context(mandara_i18n::t("The window is no longer available").to_string())?;
        let window = self.wl.windows.get(uuid).context(mandara_i18n::t("The window is no longer available").to_string())?;
        let flag = match *action {
            WindowAction::Activate => Some((Flag::Active, true)),
            WindowAction::Close => { window.close(); None }
            WindowAction::Minimized(on) => Some((Flag::Minimized, on)),
            WindowAction::Maximized(on) => Some((Flag::Maximized, on)),
            WindowAction::Fullscreen(on) => Some((Flag::Fullscreen, on)),
        };
        if let Some((flag, on)) = flag {
            window.set_state(flag as u32, if on { flag as u32 } else { 0 });
        }
        self.connection.flush()?;
        Ok(())
    }

    pub fn idle_timeout(&mut self, milliseconds: u32) -> Result<()> {
        if let Some(previous) = self.wl.idle_note.take() {
            previous.destroy();
        }
        self.wl.state.idle = false;
        if milliseconds > 0 {
            let notifier = self.wl.idle_notifier.as_ref().context(mandara_i18n::t("Idle notifications are not available").to_string())?;
            let seat = self.wl.seat.as_ref().context(mandara_i18n::t("No input seat is available").to_string())?;
            self.wl.idle_note = Some(notifier.get_idle_notification(milliseconds, seat, &self.queue.handle(), ()));
        }
        self.connection.flush()?;
        Ok(())
    }

    pub fn set_power(&mut self, on: bool) -> Result<()> {
        self.wl.power.set(on)?;
        self.connection.flush()?;
        Ok(())
    }

    /// Sleeps until an event or command arrives; false means the actor closed.
    pub fn wait(&mut self, wake: Option<&PipeReader>) -> Result<bool> {
        use rustix::event::{PollFd, PollFlags, poll};
        if self.queue.dispatch_pending(&mut self.wl)? > 0 {
            return Ok(true);
        }
        self.connection.flush()?;
        let Some(guard) = self.queue.prepare_read() else { return Ok(true) };
        let fd = guard.connection_fd();
        let mut fds = vec![PollFd::new(&fd, PollFlags::IN)];
        if let Some(wake) = wake {
            fds.push(PollFd::new(wake, PollFlags::IN));
        }
        match poll(&mut fds, None) {
            Ok(_) if fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) => anyhow::bail!("{}", mandara_i18n::t("The compositor connection closed")),
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => return Ok(true),
            Err(err) => return Err(err.into()),
        }
        let wl_ready = fds[0].revents().contains(PollFlags::IN);
        let wake_ready = fds.get(1).is_some_and(|fd| fd.revents().contains(PollFlags::IN));
        if fds.get(1).is_some_and(|fd| fd.revents().intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL)) {
            return Ok(false);
        }
        drop(fds);
        if wl_ready {
            guard.read()?;
            self.queue.dispatch_pending(&mut self.wl)?;
        } else {
            drop(guard);
        }
        if let Some(wake) = wake && wake_ready {
            let mut bytes = [0u8; 64];
            let _ = (&*wake).read(&mut bytes);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the isolated KWin container and its marker"]
    fn native_kwin_reports_outputs_desktops_and_controls_only_the_fixture_window() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap();
        assert!(runtime.starts_with("/tmp") || runtime.starts_with("/var/tmp"));
        assert_eq!(std::fs::read_to_string(runtime.join(".mandara-kwin-test")).unwrap().trim(), "mandara isolated kwin test");
        assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").as_deref(), Ok("unix:path=/nonexistent"));
        let mut session = Session::open().unwrap();
        assert_eq!(session.state().monitors().len(), 2);
        assert!(!session.state().desktops.is_empty());
        let window = session.state().clients().into_iter().find(|c| c.app_id == "mandara-kwin-test-window").expect("start the fixture foot window in the private KWin session");
        assert_eq!(window.title, "Mandara-test-window");
        assert!(!window.monitor.is_empty());
        let tag = window.tags[0];
        session.activate_desktop(&window.monitor, tag).unwrap();
        session.move_window(Some(window.id), tag, false).unwrap();
        session.roundtrip().unwrap();
        assert!(session.state().navigation().iter().any(|(name, model)| name == &window.monitor && model.contains_active(&[tag])));
        session.window_action(window.id, &WindowAction::Minimized(true)).unwrap();
        session.roundtrip().unwrap();
        let uuid = session.state().window_uuid(Some(window.id)).unwrap();
        assert!(session.state().windows[uuid].has(Flag::Minimized));
        session.window_action(window.id, &WindowAction::Minimized(false)).unwrap();
        session.window_action(window.id, &WindowAction::Activate).unwrap();
        session.idle_timeout(0).unwrap();
        session.roundtrip().unwrap();
        let fresh = Session::open().unwrap();
        assert!(fresh.state().clients().iter().any(|c| c.id == window.id && c.app_id == window.app_id));
    }
}
