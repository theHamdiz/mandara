//! Read the initial compositor snapshot before subscribing to its events.

use mandara_compositor::{ClientInfo, CompositorBackend, CompositorEvent, NavigationModel};

pub(super) struct Snapshot {
    navigation: Vec<(String, NavigationModel)>,
    focused: Option<ClientInfo>,
    clients: Vec<ClientInfo>,
}

/// Blocking compositor I/O; the caller runs this on the background executor.
pub(super) fn load(backend: &dyn CompositorBackend) -> (Snapshot, async_channel::Receiver<CompositorEvent>) {
    let snapshot = Snapshot {
        navigation: backend.get_navigation_state().unwrap_or_default(),
        focused: backend.get_active_client().ok().flatten(),
        clients: backend.get_all_clients().unwrap_or_default(),
    };
    (snapshot, backend.subscribe_events())
}

impl Snapshot {
    pub(super) fn install(self, hub: &mut super::Hub) {
        hub.navigation = self.navigation.into_iter().collect();
        hub.focused = self.focused;
        hub.clients = self.clients;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use anyhow::Result;
    use mandara_compositor::{CompositorKind, MonitorInfo};

    struct Backend { calls: Mutex<Vec<&'static str>>, clients_fail: bool }
    impl Backend {
        fn record(&self, call: &'static str) { self.calls.lock().unwrap().push(call); }
    }
    impl CompositorBackend for Backend {
        fn kind(&self) -> CompositorKind { CompositorKind::KWin }
        fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
            self.record("navigation");
            Ok(vec![("DP-1".into(), NavigationModel::Workspaces { workspaces: Vec::new() })])
        }
        fn get_active_client(&self) -> Result<Option<ClientInfo>> {
            self.record("focused");
            Ok(Some(ClientInfo { id: 7, title: "Editor".into(), ..Default::default() }))
        }
        fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
            self.record("clients");
            if self.clients_fail { anyhow::bail!("fixture query failed"); }
            Ok(vec![ClientInfo { id: 7, ..Default::default() }])
        }
        fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
            self.record("subscribe");
            async_channel::unbounded().1
        }
        fn get_monitors(&self) -> Result<Vec<MonitorInfo>> { unreachable!() }
        fn select_primary(&self, _: &str, _: u32) -> Result<()> { unreachable!() }
        fn toggle_secondary(&self, _: &str, _: u32) -> Result<()> { unreachable!() }
        fn move_client_to(&self, _: Option<u64>, _: u32) -> Result<()> { unreachable!() }
        fn toggle_client_tag(&self, _: Option<u64>, _: u32) -> Result<()> { unreachable!() }
        fn cycle_layout(&self, _: &str) -> Result<()> { unreachable!() }
        fn dispatch_raw(&self, _: &str, _: &[&str]) -> Result<()> { unreachable!() }
        fn reload_config(&self) -> Result<()> { unreachable!() }
    }

    #[test]
    fn initial_queries_finish_before_the_event_stream_starts() {
        let backend = Backend { calls: Mutex::default(), clients_fail: false };
        let (snapshot, _) = load(&backend);
        assert_eq!(*backend.calls.lock().unwrap(), ["navigation", "focused", "clients", "subscribe"]);
        let mut hub = super::super::Hub::default();
        snapshot.install(&mut hub);
        assert!(hub.navigation.contains_key("DP-1"));
        assert_eq!(hub.focused.unwrap().title, "Editor");
        assert_eq!(hub.clients[0].id, 7);
    }

    #[test]
    fn one_failed_query_keeps_the_other_initial_state() {
        let backend = Backend { calls: Mutex::default(), clients_fail: true };
        let (snapshot, _) = load(&backend);
        let mut hub = super::super::Hub::default();
        snapshot.install(&mut hub);
        assert!(hub.clients.is_empty());
        assert!(hub.focused.is_some());
        assert!(hub.navigation.contains_key("DP-1"));
    }
}
