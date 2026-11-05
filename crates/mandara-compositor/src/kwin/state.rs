//! Plasma desktop, output and window state, independent of the event loop.

use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash as _, Hasher as _};

use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::State as WindowFlag;

use crate::{ClientInfo, CompositorEvent, MonitorInfo, NavigationModel, WorkspaceStatus};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub rotated: bool,
    pub scale: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desktop {
    pub name: String,
    pub position: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub title: String,
    pub app_id: String,
    pub flags: u32,
    pub desktops: Vec<String>,
    pub geometry: Option<(i32, i32, i32, i32)>,
    pub parent: Option<String>,
}

impl Window {
    pub fn has(&self, flag: WindowFlag) -> bool {
        self.flags & flag as u32 != 0
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub idle: bool,
    pub outputs: BTreeMap<u32, Output>,
    pub desktops: BTreeMap<String, Desktop>,
    pub windows: BTreeMap<String, Window>,
    /// Version 4 reports a current desktop for each output.
    pub active_outputs: BTreeMap<String, String>,
    /// Older servers report one current desktop for the whole session.
    pub active_global: Option<String>,
}

/// Window UUIDs survive separate protocol connections; object IDs do not.
pub fn window_id(uuid: &str) -> u64 {
    let mut hash = DefaultHasher::new();
    uuid.hash(&mut hash);
    hash.finish()
}

impl State {
    pub fn desktop_id(&self, uuid: &str) -> Option<u32> {
        self.desktops.get(uuid)?.position.checked_add(1)
    }

    pub fn desktop_uuid(&self, id: u32) -> Option<&str> {
        self.desktops.iter().find(|(_, d)| d.position.checked_add(1) == Some(id)).map(|(uuid, _)| uuid.as_str())
    }

    pub fn window_uuid(&self, id: Option<u64>) -> Option<&str> {
        self.windows.iter().find(|(uuid, w)| id.map_or_else(|| w.has(WindowFlag::Active), |id| window_id(uuid) == id)).map(|(uuid, _)| uuid.as_str())
    }

    pub fn window_outputs(&self, window: &Window) -> Vec<String> {
        let Some((x, y, w, h)) = window.geometry else { return Vec::new() };
        self.outputs.values().filter(|o| overlap((x, y, w, h), o) > 0).map(|o| o.name.clone()).collect()
    }

    fn monitor_for(&self, window: &Window) -> String {
        window.geometry.and_then(|rect| self.outputs.values().max_by_key(|o| overlap(rect, o)).filter(|o| overlap(rect, o) > 0))
            .map(|o| o.name.clone()).unwrap_or_default()
    }

    pub fn clients(&self) -> Vec<ClientInfo> {
        self.windows.iter().filter(|(_, w)| !w.has(WindowFlag::Skiptaskbar)).map(|(uuid, w)| {
            let tags = if w.has(WindowFlag::OnAllDesktops) || w.desktops.is_empty() {
                self.desktops.values().filter_map(|d| d.position.checked_add(1)).collect()
            } else {
                w.desktops.iter().filter_map(|uuid| self.desktop_id(uuid)).collect()
            };
            ClientInfo { id: window_id(uuid), title: w.title.clone(), app_id: w.app_id.clone(), monitor: self.monitor_for(w), tags,
                focused: w.has(WindowFlag::Active), floating: !w.has(WindowFlag::Maximized), fullscreen: w.has(WindowFlag::Fullscreen), minimized: w.has(WindowFlag::Minimized), geometry: w.geometry }
        }).collect()
    }

    pub fn focused(&self) -> Option<ClientInfo> {
        self.clients().into_iter().find(|w| w.focused)
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        let focused = self.focused().map(|c| c.monitor);
        self.outputs.values().filter(|o| !o.name.is_empty() && o.width > 0 && o.height > 0).map(|o| {
            let physical_width = if o.rotated { o.physical_height } else { o.physical_width };
            let scale = if physical_width > 0 { physical_width as f32 / o.width as f32 } else { o.scale.max(1) as f32 };
            MonitorInfo { name: o.name.clone(), x: o.x, y: o.y, width: o.width, height: o.height, scale, active: focused.as_ref() == Some(&o.name) }
        }).collect()
    }

    pub fn navigation(&self) -> Vec<(String, NavigationModel)> {
        let clients = self.clients();
        let mut desktops: Vec<_> = self.desktops.iter().collect();
        desktops.sort_by_key(|(_, d)| d.position);
        self.outputs.values().filter(|o| !o.name.is_empty()).map(|output| {
            let active = self.active_outputs.get(&output.name).or(self.active_global.as_ref());
            let workspaces = desktops.iter().filter_map(|(uuid, desktop)| {
                let id = desktop.position.checked_add(1)?;
                let occupied = clients.iter().any(|c| c.monitor == output.name && c.tags.contains(&id));
                let urgent = self.windows.values().any(|w| w.has(WindowFlag::DemandsAttention) && self.monitor_for(w) == output.name && (w.has(WindowFlag::OnAllDesktops) || w.desktops.contains(uuid)));
                Some(WorkspaceStatus { id, name: desktop.name.clone(), active: active == Some(*uuid), occupied, urgent, layout: String::new() })
            }).collect();
            (output.name.clone(), NavigationModel::Workspaces { workspaces })
        }).collect()
    }
}

fn overlap((x, y, w, h): (i32, i32, i32, i32), output: &Output) -> i64 {
    let left = i64::from(x).max(i64::from(output.x));
    let top = i64::from(y).max(i64::from(output.y));
    let right = (i64::from(x) + i64::from(w)).min(i64::from(output.x) + i64::from(output.width));
    let bottom = (i64::from(y) + i64::from(h)).min(i64::from(output.y) + i64::from(output.height));
    (right - left).max(0) * (bottom - top).max(0)
}

#[derive(Default)]
pub(super) struct Diff(Option<State>);

impl Diff {
    pub fn events(&mut self, state: &State) -> Vec<CompositorEvent> {
        if self.0.as_ref() == Some(state) {
            return Vec::new();
        }
        let mut events = Vec::new();
        let previous = self.0.as_ref();
        let old_navigation = previous.map(State::navigation).unwrap_or_default();
        for (monitor, model) in state.navigation() {
            if !old_navigation.iter().any(|(m, n)| *m == monitor && *n == model) {
                events.push(CompositorEvent::Navigation { monitor, model });
            }
        }
        if previous.is_none_or(|s| s.clients() != state.clients()) {
            events.push(CompositorEvent::Clients(state.clients()));
        }
        if previous.is_none_or(|s| s.focused() != state.focused()) {
            events.push(CompositorEvent::FocusedClient(state.focused()));
        }
        if previous.is_none_or(|s| s.monitors() != state.monitors()) {
            events.push(CompositorEvent::Monitors(state.monitors()));
        }
        self.0 = Some(state.clone());
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> State {
        let mut state = State::default();
        for (index, name) in ["DP-1", "DP-2"].into_iter().enumerate() {
            state.outputs.insert(index as u32, Output { name: name.into(), x: index as i32 * 1280, width: 1280, height: 720, physical_width: 1920, physical_height: 1080, ..Default::default() });
        }
        state.desktops.insert("one".into(), Desktop { name: "Web".into(), position: 0 });
        state.desktops.insert("two".into(), Desktop { name: "Code".into(), position: 1 });
        state.active_outputs.insert("DP-1".into(), "one".into());
        state.active_outputs.insert("DP-2".into(), "two".into());
        state.windows.insert("window-a".into(), Window { title: "Editor".into(), app_id: "org.editor".into(), flags: WindowFlag::Active as u32 | WindowFlag::Maximized as u32, desktops: vec!["two".into()], geometry: Some((1280, 0, 1280, 720)), ..Default::default() });
        state
    }

    #[test]
    fn workspaces_follow_each_output_and_window_desktop_membership() {
        let state = fixture();
        let navigation = state.navigation();
        assert!(navigation[0].1.contains_active(&[1]));
        assert!(!navigation[0].1.contains_active(&[2]));
        assert!(navigation[1].1.contains_active(&[2]));
        let client = state.focused().unwrap();
        assert_eq!(client.id, window_id("window-a"));
        assert_eq!(client.monitor, "DP-2");
        assert_eq!(client.tags, [2]);
        assert!(!client.floating);
        assert_eq!(state.monitors()[1].scale, 1.5);
        assert_eq!(state.desktop_uuid(2), Some("two"));
        assert_eq!(state.window_uuid(Some(client.id)), Some("window-a"));
    }

    #[test]
    fn all_desktop_windows_and_output_removal_update_the_snapshot() {
        let mut state = fixture();
        let mut diff = Diff::default();
        assert!(!diff.events(&state).is_empty());
        assert!(diff.events(&state).is_empty());
        state.windows.get_mut("window-a").unwrap().flags |= WindowFlag::OnAllDesktops as u32;
        assert_eq!(state.clients()[0].tags, [1, 2]);
        state.outputs.remove(&1);
        assert_eq!(state.clients()[0].monitor, "");
        assert!(diff.events(&state).iter().any(|e| matches!(e, CompositorEvent::Monitors(m) if m.len() == 1)));
    }

    #[test]
    fn older_servers_share_one_active_desktop_and_window_ids_ignore_connection_order() {
        let mut state = fixture();
        state.active_outputs.clear();
        state.active_global = Some("two".into());
        assert!(state.navigation().iter().all(|(_, n)| n.contains_active(&[2])));
        let id = state.focused().unwrap().id;
        state.windows.insert("another-window".into(), Window::default());
        assert_eq!(state.focused().unwrap().id, id);
        assert_eq!(state.window_outputs(state.windows.get("window-a").unwrap()), ["DP-2"]);
    }

    #[test]
    fn minimized_windows_keep_their_workspace_and_state_for_restore() {
        let mut state = fixture();
        state.windows.get_mut("window-a").unwrap().flags |= WindowFlag::Minimized as u32;
        let client = &state.clients()[0];
        assert!(client.minimized);
        assert_eq!(client.tags, [2]);
        assert_eq!(client.monitor, "DP-2");
        assert!(!client.floating);
    }
}
