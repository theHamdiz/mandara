//! The socket2 event loop. Hyprland reports one change as several lines (a
//! `workspace` and its `workspacev2`, a window opening and the focus moving
//! to it), so lines are gathered into a burst, folded into what must be
//! refreshed ([`Dirty`]), and answered with one set of requests.

use std::os::unix::net::UnixStream;
use std::time::Instant;

use anyhow::{Context as _, Result};

use super::{HyprMonitor, HyprWorkspace, HyprlandDriver, keyboard, navigation, parse_event};
use crate::model::{ClientInfo, CompositorEvent, MonitorInfo};
use crate::stream::{Backoff, Burst, Dedup, Inbox, Next};

/// What a burst of socket2 lines asks to refresh.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Dirty {
    /// Workspaces, per monitor.
    navigation: bool,
    clients: bool,
    /// The monitor list itself.
    monitors: bool,
    /// The focused window changed: ask which.
    focus: bool,
    /// Nothing is focused any more (the event says so; nothing to ask).
    unfocused: bool,
    /// A `windowtitle` without the title's owner or text.
    title_v1: bool,
    /// `windowtitlev2` payloads: window address and new title.
    titles: Vec<(u64, String)>,
    keyboard: bool,
}

impl Dirty {
    /// The whole desktop, for a fresh connection.
    pub fn everything() -> Self {
        Self { navigation: true, clients: true, monitors: true, focus: true, keyboard: true, ..Self::default() }
    }

    /// Folds in one socket2 line. The versioned spelling of an event
    /// (`workspacev2`) sets what the plain one does, so a pair costs
    /// nothing extra.
    pub fn note(&mut self, line: &str) {
        let Some((event, data)) = parse_event(line) else { return };
        match event {
            "workspace" | "workspacev2" | "createworkspace" | "createworkspacev2" | "destroyworkspace" | "destroyworkspacev2" | "focusedmon" | "focusedmonv2" => self.navigation = true,
            "openwindow" | "closewindow" | "movewindow" | "movewindowv2" => {
                self.navigation = true;
                self.clients = true;
            }
            // Workspaces are listed per monitor.
            "monitoradded" | "monitoraddedv2" | "monitorremoved" | "monitorremovedv2" => {
                self.navigation = true;
                self.monitors = true;
            }
            // `class,title`; both empty when nothing is focused.
            "activewindow" => self.focused(data != ","),
            // The window's address; empty when nothing is focused.
            "activewindowv2" => self.focused(!data.is_empty()),
            "windowtitle" => self.title_v1 = true,
            "windowtitlev2" => {
                if let Some((address, title)) = data.split_once(',')
                    && let Ok(address) = u64::from_str_radix(address.trim_start_matches("0x"), 16)
                {
                    self.titles.push((address, title.to_string()));
                }
            }
            "activelayout" if keyboard::switched(data).is_some() => self.keyboard = true,
            _ => {}
        }
    }

    fn focused(&mut self, some: bool) {
        self.focus = some;
        self.unfocused = !some;
    }
}

/// The socket2 reader and what it remembers between bursts.
pub(super) struct Watcher {
    driver: HyprlandDriver,
    /// The monitors as of the last refresh that needed them: they resolve
    /// connector names for the windows, and focus changes are too frequent
    /// to ask again each time.
    monitors: Vec<HyprMonitor>,
    focused: Option<ClientInfo>,
    dedup: Dedup,
}

impl Watcher {
    pub fn new(driver: HyprlandDriver) -> Self {
        Self { driver, monitors: Vec::new(), focused: None, dedup: Dedup::default() }
    }

    /// Streams events until the receiver is dropped, reconnecting with
    /// backoff whenever the connection ends.
    pub fn watch(mut self, tx: &async_channel::Sender<CompositorEvent>) {
        let mut backoff = Backoff::new(crate::RECONNECT_DELAY);
        loop {
            let began = Instant::now();
            match self.connected(tx) {
                Ok(true) => return,
                Ok(false) => {}
                Err(err) => tracing::debug!("hyprland event stream ended: {err:#}"),
            }
            if tx.is_closed() {
                return;
            }
            std::thread::sleep(backoff.after(began.elapsed()));
        }
    }

    /// One connection's worth of events; `true` when the receiver is gone.
    fn connected(&mut self, tx: &async_channel::Sender<CompositorEvent>) -> Result<bool> {
        let path = self.driver.dir.join(".socket2.sock");
        let mut inbox = Inbox::new(UnixStream::connect(&path).with_context(|| format!("connecting {}", path.display()))?);
        // Query only after socket2 is connected, so intervening changes are queued.
        let events = self.refresh(&Dirty::everything());
        if !self.dedup.send(tx, events) {
            return Ok(true);
        }
        loop {
            let first = match inbox.line(None)? {
                Next::Item(line) => line,
                Next::Idle => continue,
                Next::Closed => return Ok(false),
            };
            let mut dirty = Dirty::default();
            dirty.note(&first);
            if dirty == Dirty::default() {
                continue;
            }
            let open = inbox.burst_lines(Burst::EVENTS, |line| dirty.note(&line))?;
            let events = self.refresh(&dirty);
            if !self.dedup.send(tx, events) {
                return Ok(true);
            }
            if !open {
                return Ok(false);
            }
        }
    }

    /// The events that bring the shell up to date with `dirty`. A request
    /// that fails leaves its part of the shell as it was.
    pub fn refresh(&mut self, dirty: &Dirty) -> Vec<CompositorEvent> {
        let mut out = Vec::new();
        let driver = &self.driver;
        if dirty.navigation || dirty.clients || dirty.monitors {
            match driver.json::<Vec<HyprMonitor>>("monitors") {
                Ok(monitors) => self.monitors = monitors,
                Err(err) => tracing::debug!("hyprland monitors: {err:#}"),
            }
        }
        if dirty.navigation {
            match driver.json::<Vec<HyprWorkspace>>("workspaces") {
                Ok(workspaces) => out.extend(navigation(&workspaces, &self.monitors).into_iter().map(|(monitor, model)| CompositorEvent::Navigation { monitor, model })),
                Err(err) => tracing::debug!("hyprland workspaces: {err:#}"),
            }
        }
        if dirty.clients {
            match driver.all_clients(Some(&self.monitors)) {
                Ok(clients) => out.push(CompositorEvent::Clients(clients)),
                Err(err) => tracing::debug!("hyprland clients: {err:#}"),
            }
        }
        if dirty.unfocused {
            self.focused = None;
            out.push(CompositorEvent::FocusedClient(None));
        } else if dirty.focus || (dirty.title_v1 && dirty.titles.is_empty()) {
            let known = (!self.monitors.is_empty()).then_some(self.monitors.as_slice());
            match driver.active_client(known) {
                Ok(focused) => {
                    self.focused = focused.clone();
                    out.push(CompositorEvent::FocusedClient(focused));
                }
                Err(err) => tracing::debug!("hyprland active window: {err:#}"),
            }
        } else if let Some(client) = self.focused.as_mut()
            && let Some((_, title)) = dirty.titles.iter().rev().find(|(address, _)| *address == client.id)
            && client.title != *title
        {
            // `windowtitlev2` carries the new title: no request needed.
            client.title.clone_from(title);
            out.push(CompositorEvent::FocusedClient(Some(client.clone())));
        }
        if dirty.monitors {
            out.push(CompositorEvent::Monitors(monitor_infos(&self.monitors)));
        }
        if dirty.keyboard {
            out.extend(driver.keyboard_event());
        }
        out
    }
}

/// The shell's view of Hyprland's monitors.
pub(super) fn monitor_infos(monitors: &[HyprMonitor]) -> Vec<MonitorInfo> {
    monitors.iter().map(|m| MonitorInfo { name: m.name.clone(), x: m.x, y: m.y, width: m.width, height: m.height, scale: m.scale, active: m.focused }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn dirty(lines: &[&str]) -> Dirty {
        let mut dirty = Dirty::default();
        for line in lines {
            dirty.note(line);
        }
        dirty
    }

    #[test]
    fn the_two_spellings_of_an_event_ask_for_one_refresh() {
        let plain = dirty(&["workspace>>2"]);
        assert_eq!(plain, dirty(&["workspace>>2", "workspacev2>>2,two", "focusedmon>>DP-1,two", "createworkspace>>2", "createworkspacev2>>2,two"]));
        assert!(plain.navigation && !plain.clients && !plain.focus);
        let opened = dirty(&["openwindow>>7,2,foot,foot", "activewindow>>foot,foot", "activewindowv2>>7", "windowtitle>>7", "movewindowv2>>7,2,two"]);
        assert!(opened.navigation && opened.clients && opened.focus && opened.title_v1 && !opened.unfocused);
        assert_eq!(dirty(&["monitoradded>>DP-2", "monitoraddedv2>>1,DP-2,Dell"]), Dirty { navigation: true, monitors: true, ..Dirty::default() });
    }

    #[test]
    fn unrelated_and_malformed_lines_ask_for_nothing() {
        let nothing = Dirty::default();
        assert_eq!(dirty(&["submap>>resize", "configreloaded>>", "fullscreen>>1", "not an event", "windowtitlev2>>nothex,title", "activelayout>>keyboard,", "urgent>>7"]), nothing);
    }

    #[test]
    fn focus_follows_the_last_word_and_an_empty_focus_needs_no_request() {
        let lost = dirty(&["activewindow>>,", "activewindowv2>>"]);
        assert!(lost.unfocused && !lost.focus);
        let regained = dirty(&["activewindow>>,", "activewindowv2>>", "activewindow>>foot,zsh", "activewindowv2>>7"]);
        assert!(regained.focus && !regained.unfocused);
        let lost_again = dirty(&["activewindowv2>>7", "activewindowv2>>"]);
        assert!(lost_again.unfocused && !lost_again.focus);
        // A title with commas keeps them.
        assert_eq!(dirty(&["windowtitlev2>>0x7,a, b"]).titles, [(7, "a, b".to_string())]);
    }

    /// A Hyprland stand-in: `.socket.sock` answers from `replies` and records
    /// each request; `.socket2.sock` is the event stream.
    struct Hypr {
        dir: PathBuf,
        requests: Arc<Mutex<Vec<String>>>,
        events: Arc<Mutex<Option<UnixStream>>>,
    }

    impl Drop for Hypr {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const MONITORS: &str = r#"[{"id":0,"name":"DP-1","width":1920,"height":1080,"activeWorkspace":{"id":2}}]"#;
    const WORKSPACES: &str = r#"[{"id":2,"name":"2","monitor":"DP-1","windows":1}]"#;
    const FOCUSED: &str = r#"{"address":"0x7","class":"foot","title":"zsh","monitor":0,"workspace":{"id":2}}"#;

    impl Hypr {
        fn start(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mdr-hypr-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let events = Arc::new(Mutex::new(None));
            let socket = UnixListener::bind(dir.join(".socket.sock")).unwrap();
            let log = requests.clone();
            std::thread::spawn(move || {
                for stream in socket.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let mut buf = [0u8; 256];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let command = String::from_utf8_lossy(&buf[..n]).to_string();
                    let reply = match command.as_str() {
                        "j/monitors" => MONITORS,
                        "j/workspaces" => WORKSPACES,
                        "j/clients" => "[]",
                        "j/activewindow" => FOCUSED,
                        "j/devices" => r#"{"keyboards":[]}"#,
                        _ => "",
                    };
                    log.lock().unwrap().push(command);
                    let _ = stream.write_all(reply.as_bytes());
                }
            });
            let stream = UnixListener::bind(dir.join(".socket2.sock")).unwrap();
            let slot = events.clone();
            std::thread::spawn(move || {
                if let Ok((stream, _)) = stream.accept() {
                    *slot.lock().unwrap() = Some(stream);
                }
            });
            Self { dir, requests, events }
        }

        fn driver(&self) -> HyprlandDriver {
            HyprlandDriver { dir: self.dir.clone(), version: Some((0, 57)), lua: true }
        }

        fn send(&self, lines: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(stream) = self.events.lock().unwrap().as_mut() {
                    stream.write_all(lines.as_bytes()).unwrap();
                    return;
                }
                assert!(Instant::now() < deadline, "the watcher never connected");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn next(rx: &async_channel::Receiver<CompositorEvent>) -> CompositorEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match rx.try_recv() {
                Ok(event) => return event,
                Err(async_channel::TryRecvError::Closed) => panic!("event stream closed"),
                Err(async_channel::TryRecvError::Empty) => {}
            }
            assert!(Instant::now() < deadline, "no event arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn quiet(rx: &async_channel::Receiver<CompositorEvent>) {
        std::thread::sleep(Duration::from_millis(250));
        assert!(rx.try_recv().is_err(), "no further event expected");
    }

    #[test]
    fn a_burst_of_workspace_lines_is_one_refresh_and_a_repeat_is_not_resent() {
        use crate::backend::CompositorBackend;
        let hypr = Hypr::start("burst");
        let rx = hypr.driver().subscribe_events();
        // Initial snapshot: navigation, clients, focus, monitors, keyboard (none: no keyboards... an empty layout).
        for _ in 0..4 {
            next(&rx);
        }
        assert!(matches!(next(&rx), CompositorEvent::KeyboardLayout(_)));
        assert_eq!(hypr.requests(), ["j/monitors", "j/workspaces", "j/clients", "j/activewindow", "j/devices"], "five requests for the whole desktop, the monitors asked once");

        // The same change reported the way Hyprland does: several lines.
        hypr.send("workspace>>2\nworkspacev2>>2,2\nfocusedmon>>DP-1,2\n");
        quiet(&rx);
        let after = hypr.requests();
        assert_eq!(&after[5..], ["j/monitors", "j/workspaces"], "one refresh for the burst");
    }

    #[test]
    fn focus_changes_cost_one_request_and_title_changes_none() {
        use crate::backend::CompositorBackend;
        let hypr = Hypr::start("focus");
        let rx = hypr.driver().subscribe_events();
        for _ in 0..5 {
            next(&rx);
        }
        let before = hypr.requests().len();
        // Focus moves: v1 and v2 spellings, one request.
        hypr.send("activewindow>>foot,zsh\nactivewindowv2>>7\n");
        quiet(&rx); // the snapshot was identical: nothing is resent
        assert_eq!(&hypr.requests()[before..], ["j/activewindow"]);
        // A new title for the focused window arrives in the event.
        hypr.send("windowtitle>>7\nwindowtitlev2>>7,vim\n");
        assert!(matches!(next(&rx), CompositorEvent::FocusedClient(Some(c)) if c.title == "vim"));
        // Another window's title is none of its business.
        hypr.send("windowtitlev2>>9,other\n");
        // Losing focus is in the event too.
        hypr.send("activewindow>>,\nactivewindowv2>>\n");
        assert!(matches!(next(&rx), CompositorEvent::FocusedClient(None)));
        quiet(&rx);
        assert_eq!(hypr.requests().len(), before + 1, "no request for either");
    }
}
