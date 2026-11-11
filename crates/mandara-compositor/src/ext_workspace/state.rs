//! The `ext-workspace-v1` state of one connection — outputs, workspace
//! groups, workspaces and windows — independent of the event loop, so the
//! Wayland handlers and the tests feed it the same [`Event`]s.
//!
//! Workspace and group events are double-buffered: they change the pending
//! set, and the manager's `done` installs it whole (the protocol's atomic
//! batches). Window events apply at once but are published only on their
//! own `done`. Objects are keyed by their protocol id on this connection.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};

use crate::{ClientInfo, CompositorEvent, MonitorInfo, NavigationModel, WorkspaceStatus};

/// `ext_workspace_handle_v1.state` bits.
pub const STATE_ACTIVE: u32 = 1;
pub const STATE_URGENT: u32 = 2;
pub const STATE_HIDDEN: u32 = 4;
/// `ext_workspace_handle_v1.capabilities`: `activate` is available.
pub const CAN_ACTIVATE: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub name: String,
    pub x: i32,
    pub y: i32,
    /// Logical size (xdg-output).
    pub width: i32,
    pub height: i32,
    /// The current mode, in physical pixels.
    pub mode_width: i32,
    pub mode_height: i32,
    pub scale: i32,
}

/// A workspace group: the outputs it is shown on and its workspaces, in
/// the order they entered it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub outputs: Vec<u32>,
    pub workspaces: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub coordinates: Vec<u32>,
    pub state: u32,
    pub capabilities: u32,
    /// Creation order, the tie-breaker after coordinates.
    pub serial: u64,
}

impl Workspace {
    fn has(&self, bit: u32) -> bool {
        self.state & bit != 0
    }
}

/// Groups and workspaces as of one `done`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspaces {
    pub groups: BTreeMap<u32, Group>,
    pub workspaces: BTreeMap<u32, Workspace>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub title: String,
    pub app_id: String,
    pub activated: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub outputs: Vec<u32>,
    /// Workspaces the compositor says the window is on (COSMIC's toplevel
    /// info; labwc's window protocols don't say).
    pub workspaces: Vec<u32>,
    pub geometry: Option<(i32, i32, i32, i32)>,
}

/// One protocol event, by object id.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    OutputName { output: u32, name: String },
    OutputMode { output: u32, width: i32, height: i32 },
    OutputScale { output: u32, factor: i32 },
    OutputPosition { output: u32, x: i32, y: i32 },
    OutputSize { output: u32, width: i32, height: i32 },
    OutputDone,
    OutputGone { output: u32 },
    GroupCreated { group: u32 },
    GroupOutput { group: u32, output: u32, entered: bool },
    GroupWorkspace { group: u32, workspace: u32, entered: bool },
    GroupRemoved { group: u32 },
    WorkspaceCreated { workspace: u32 },
    WorkspaceName { workspace: u32, name: String },
    WorkspaceCoordinates { workspace: u32, coordinates: Vec<u32> },
    WorkspaceState { workspace: u32, state: u32 },
    WorkspaceCapabilities { workspace: u32, capabilities: u32 },
    WorkspaceRemoved { workspace: u32 },
    /// `ext_workspace_manager_v1.done`: the pending set becomes current.
    Done,
    WindowTitle { window: u32, title: String },
    WindowAppId { window: u32, app_id: String },
    WindowState { window: u32, activated: bool, minimized: bool, maximized: bool, fullscreen: bool },
    WindowOutput { window: u32, output: u32, entered: bool },
    WindowWorkspace { window: u32, workspace: u32, entered: bool },
    WindowGeometry { window: u32, rect: (i32, i32, i32, i32) },
    WindowDone,
    WindowClosed { window: u32 },
    Idle(bool),
}

/// What a command becomes on the wire, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `ext_workspace_handle_v1.activate`.
    Activate { workspace: u32 },
    /// `ext_workspace_manager_v1.commit`: the requests before it apply
    /// together.
    Commit,
    /// `zcosmic_toplevel_manager_v1.move_to_ext_workspace`.
    MoveWindow { window: u32, workspace: u32, output: u32 },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub outputs: BTreeMap<u32, Output>,
    /// Installed by the last `done`.
    pub current: Workspaces,
    pending: Workspaces,
    pub windows: BTreeMap<u32, Window>,
    pub idle: bool,
    serial: u64,
}

impl State {
    /// Applies `event`; `true` when it completes a batch worth publishing.
    pub fn apply(&mut self, event: Event) -> bool {
        use Event::*;
        match event {
            OutputName { output, name } => self.outputs.entry(output).or_default().name = name,
            OutputMode { output, width, height } => {
                let o = self.outputs.entry(output).or_default();
                (o.mode_width, o.mode_height) = (width, height);
            }
            OutputScale { output, factor } => self.outputs.entry(output).or_default().scale = factor,
            OutputPosition { output, x, y } => {
                let o = self.outputs.entry(output).or_default();
                (o.x, o.y) = (x, y);
            }
            OutputSize { output, width, height } => {
                let o = self.outputs.entry(output).or_default();
                (o.width, o.height) = (width, height);
            }
            OutputDone => return true,
            OutputGone { output } => {
                self.outputs.remove(&output);
                for set in [&mut self.pending, &mut self.current] {
                    for group in set.groups.values_mut() {
                        group.outputs.retain(|o| *o != output);
                    }
                }
                for window in self.windows.values_mut() {
                    window.outputs.retain(|o| *o != output);
                }
                return true;
            }
            GroupCreated { group } => {
                self.pending.groups.entry(group).or_default();
            }
            GroupOutput { group, output, entered } => {
                let outputs = &mut self.pending.groups.entry(group).or_default().outputs;
                outputs.retain(|o| *o != output);
                if entered {
                    outputs.push(output);
                }
            }
            GroupWorkspace { group, workspace, entered } => {
                // A workspace belongs to one group at a time.
                for g in self.pending.groups.values_mut() {
                    g.workspaces.retain(|w| *w != workspace);
                }
                if entered {
                    self.pending.groups.entry(group).or_default().workspaces.push(workspace);
                }
            }
            GroupRemoved { group } => {
                self.pending.groups.remove(&group);
            }
            WorkspaceCreated { workspace } => {
                self.serial += 1;
                self.pending.workspaces.insert(workspace, Workspace { serial: self.serial, ..Default::default() });
            }
            WorkspaceName { workspace, name } => self.pending_workspace(workspace).name = name,
            WorkspaceCoordinates { workspace, coordinates } => self.pending_workspace(workspace).coordinates = coordinates,
            WorkspaceState { workspace, state } => self.pending_workspace(workspace).state = state,
            WorkspaceCapabilities { workspace, capabilities } => self.pending_workspace(workspace).capabilities = capabilities,
            WorkspaceRemoved { workspace } => {
                self.pending.workspaces.remove(&workspace);
                for g in self.pending.groups.values_mut() {
                    g.workspaces.retain(|w| *w != workspace);
                }
            }
            Done => {
                self.current.clone_from(&self.pending);
                // A removed workspace's id may be reused: windows forget it.
                let live = &self.current.workspaces;
                for window in self.windows.values_mut() {
                    window.workspaces.retain(|w| live.contains_key(w));
                }
                return true;
            }
            WindowTitle { window, title } => self.windows.entry(window).or_default().title = title,
            WindowAppId { window, app_id } => self.windows.entry(window).or_default().app_id = app_id,
            WindowState { window, activated, minimized, maximized, fullscreen } => {
                let w = self.windows.entry(window).or_default();
                (w.activated, w.minimized, w.maximized, w.fullscreen) = (activated, minimized, maximized, fullscreen);
            }
            WindowOutput { window, output, entered } => {
                let outputs = &mut self.windows.entry(window).or_default().outputs;
                outputs.retain(|o| *o != output);
                if entered {
                    outputs.push(output);
                }
            }
            WindowWorkspace { window, workspace, entered } => {
                let workspaces = &mut self.windows.entry(window).or_default().workspaces;
                workspaces.retain(|w| *w != workspace);
                if entered {
                    workspaces.push(workspace);
                }
            }
            WindowGeometry { window, rect } => self.windows.entry(window).or_default().geometry = Some(rect),
            WindowDone => return true,
            WindowClosed { window } => return self.windows.remove(&window).is_some(),
            Idle(idle) => {
                let changed = self.idle != idle;
                self.idle = idle;
                return changed;
            }
        }
        false
    }

    fn pending_workspace(&mut self, workspace: u32) -> &mut Workspace {
        self.pending.workspaces.entry(workspace).or_default()
    }

    /// The groups shown on `output`: those that entered it, or, when no
    /// group names any output at all, every group.
    fn groups_on(&self, output: u32) -> impl Iterator<Item = &Group> {
        let unplaced = self.current.groups.values().all(|g| g.outputs.is_empty());
        self.current.groups.values().filter(move |g| unplaced || g.outputs.contains(&output))
    }

    /// `output`'s visible workspaces in bar order (coordinates, then
    /// creation): their position is the number the bar shows and clicks.
    pub fn workspaces_on(&self, output: u32) -> Vec<(u32, &Workspace)> {
        let mut list: Vec<(u32, &Workspace)> = self
            .groups_on(output)
            .flat_map(|g| g.workspaces.iter())
            .filter_map(|id| self.current.workspaces.get(id).map(|w| (*id, w)))
            .filter(|(_, w)| !w.has(STATE_HIDDEN))
            .collect();
        list.sort_by(|a, b| a.1.coordinates.cmp(&b.1.coordinates).then(a.1.serial.cmp(&b.1.serial)));
        list.dedup_by_key(|(id, _)| *id);
        list
    }

    fn output_named(&self, name: &str) -> Option<u32> {
        self.outputs.iter().find(|(_, o)| o.name == name).map(|(id, _)| *id)
    }

    fn output_name(&self, id: u32) -> &str {
        self.outputs.get(&id).map_or("", |o| o.name.as_str())
    }

    /// Named outputs, by name.
    fn named_outputs(&self) -> Vec<(u32, &Output)> {
        let mut named: Vec<(u32, &Output)> = self.outputs.iter().filter(|(_, o)| !o.name.is_empty()).map(|(id, o)| (*id, o)).collect();
        named.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        named
    }

    /// The focused window's output, else the first named output.
    pub fn focused_output(&self) -> Option<u32> {
        let focused = self.windows.values().find(|w| w.activated).and_then(|w| w.outputs.first().copied()).filter(|o| self.outputs.contains_key(o));
        focused.or_else(|| self.named_outputs().first().map(|(id, _)| *id))
    }

    /// Workspace `workspace`'s number on `output` (1-based), if shown there.
    fn number_on(&self, output: u32, workspace: u32) -> Option<u32> {
        self.workspaces_on(output).iter().position(|(id, _)| *id == workspace).and_then(|i| u32::try_from(i + 1).ok())
    }

    pub fn navigation(&self) -> Vec<(String, NavigationModel)> {
        self.named_outputs()
            .into_iter()
            .map(|(output, o)| {
                let workspaces = self
                    .workspaces_on(output)
                    .into_iter()
                    .zip(1u32..)
                    .map(|((key, w), id)| WorkspaceStatus {
                        id,
                        name: if w.name.is_empty() { id.to_string() } else { w.name.clone() },
                        active: w.has(STATE_ACTIVE),
                        occupied: self.windows.values().any(|win| win.workspaces.contains(&key)),
                        urgent: w.has(STATE_URGENT),
                        layout: String::new(),
                    })
                    .collect();
                (o.name.clone(), NavigationModel::Workspaces { workspaces })
            })
            .collect()
    }

    fn client(&self, id: u32, w: &Window) -> ClientInfo {
        let output = w.outputs.iter().copied().find(|o| self.outputs.contains_key(o));
        let tags = w
            .workspaces
            .iter()
            .filter_map(|ws| output.and_then(|o| self.number_on(o, *ws)).or_else(|| self.outputs.keys().find_map(|o| self.number_on(*o, *ws))))
            .collect();
        ClientInfo {
            id: u64::from(id),
            title: w.title.clone(),
            app_id: w.app_id.clone(),
            monitor: output.map(|o| self.output_name(o).to_string()).unwrap_or_default(),
            tags,
            focused: w.activated,
            floating: false,
            fullscreen: w.fullscreen,
            minimized: w.minimized,
            geometry: w.geometry,
        }
    }

    pub fn clients(&self) -> Vec<ClientInfo> {
        self.windows.iter().map(|(id, w)| self.client(*id, w)).collect()
    }

    pub fn focused(&self) -> Option<ClientInfo> {
        self.windows.iter().find(|(_, w)| w.activated).map(|(id, w)| self.client(*id, w))
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        let focused = self.windows.values().find(|w| w.activated).and_then(|w| w.outputs.first().copied());
        self.named_outputs()
            .into_iter()
            .filter(|(_, o)| o.width > 0 && o.height > 0)
            .map(|(id, o)| MonitorInfo {
                name: o.name.clone(),
                x: o.x,
                y: o.y,
                width: o.width,
                height: o.height,
                // The longer sides match whatever the rotation.
                scale: match o.mode_width.max(o.mode_height) {
                    mode if mode > 0 => mode as f32 / o.width.max(o.height) as f32,
                    _ => o.scale.max(1) as f32,
                },
                active: focused == Some(id),
            })
            .collect()
    }

    /// The window `client` names (the focused one for `None`).
    pub fn window(&self, client: Option<u64>) -> Option<u32> {
        match client {
            Some(id) => u32::try_from(id).ok().filter(|id| self.windows.contains_key(id)),
            None => self.windows.iter().find(|(_, w)| w.activated).map(|(id, _)| *id),
        }
    }

    /// Requests that show workspace `number` of `monitor` (the focused
    /// output when empty): activate, then commit.
    pub fn plan_activate(&self, monitor: &str, number: u32) -> Result<[Request; 2]> {
        let output = if monitor.is_empty() { self.focused_output() } else { self.output_named(monitor) }.ok_or_else(|| anyhow!("ext-workspace: no output {monitor:?}"))?;
        let (workspace, ws) = self.workspace_number(output, number)?;
        if ws.capabilities & CAN_ACTIVATE == 0 {
            bail!("{}", mandara_i18n::t("Not supported by this compositor"));
        }
        Ok([Request::Activate { workspace }, Request::Commit])
    }

    /// The request that moves a window (the focused one for `None`) to
    /// workspace `number` of its own output.
    pub fn plan_move(&self, client: Option<u64>, number: u32) -> Result<Request> {
        let window = self.window(client).ok_or_else(|| anyhow!("{}", mandara_i18n::t("The window is no longer available")))?;
        let output = self.windows[&window].outputs.iter().copied().find(|o| self.outputs.contains_key(o)).or_else(|| self.focused_output()).ok_or_else(|| anyhow!("ext-workspace: the window has no output"))?;
        let (workspace, _) = self.workspace_number(output, number)?;
        Ok(Request::MoveWindow { window, workspace, output })
    }

    fn workspace_number(&self, output: u32, number: u32) -> Result<(u32, &Workspace)> {
        let index = usize::try_from(number).ok().and_then(|n| n.checked_sub(1));
        index.and_then(|i| self.workspaces_on(output).get(i).copied()).ok_or_else(|| anyhow!("{}", mandara_i18n::t("Workspace not found")))
    }
}

/// What changed since the last state sent.
#[derive(Default)]
pub struct Diff {
    navigation: Vec<(String, NavigationModel)>,
    clients: Option<Vec<ClientInfo>>,
    focused: Option<Option<ClientInfo>>,
    monitors: Option<Vec<MonitorInfo>>,
}

impl Diff {
    /// Appends the events since the last call to `out`.
    pub fn events(&mut self, state: &State, out: &mut Vec<CompositorEvent>) {
        for (monitor, model) in state.navigation() {
            match self.navigation.iter_mut().find(|(m, _)| *m == monitor) {
                Some((_, last)) if *last == model => {}
                Some((_, last)) => {
                    last.clone_from(&model);
                    out.push(CompositorEvent::Navigation { monitor, model });
                }
                None => {
                    self.navigation.push((monitor.clone(), model.clone()));
                    out.push(CompositorEvent::Navigation { monitor, model });
                }
            }
        }
        let clients = state.clients();
        if self.clients.as_ref() != Some(&clients) {
            self.clients = Some(clients.clone());
            out.push(CompositorEvent::Clients(clients));
        }
        let focused = state.focused();
        if self.focused.as_ref() != Some(&focused) {
            self.focused = Some(focused.clone());
            out.push(CompositorEvent::FocusedClient(focused));
        }
        let monitors = state.monitors();
        if self.monitors.as_ref() != Some(&monitors) {
            self.monitors = Some(monitors.clone());
            out.push(CompositorEvent::Monitors(monitors));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Event::*;

    fn feed(state: &mut State, events: impl IntoIterator<Item = Event>) -> bool {
        events.into_iter().fold(false, |publish, e| state.apply(e) | publish)
    }

    fn output(state: &mut State, id: u32, name: &str, x: i32) {
        feed(state, [
            OutputName { output: id, name: name.into() },
            OutputMode { output: id, width: 2560, height: 1440 },
            OutputPosition { output: id, x, y: 0 },
            OutputSize { output: id, width: 1707, height: 960 },
            OutputDone,
        ]);
    }

    fn workspace(id: u32, name: &str, state: u32) -> [Event; 4] {
        [WorkspaceCreated { workspace: id }, WorkspaceName { workspace: id, name: name.into() }, WorkspaceState { workspace: id, state }, WorkspaceCapabilities { workspace: id, capabilities: CAN_ACTIVATE }]
    }

    /// labwc's shape: one group on every output, four workspaces.
    fn labwc() -> State {
        let mut s = State::default();
        output(&mut s, 10, "DP-1", 0);
        output(&mut s, 11, "HDMI-A-1", 1707);
        s.apply(GroupCreated { group: 20 });
        for (id, name) in [(30, "1"), (31, "2"), (32, "3"), (33, "4")] {
            feed(&mut s, workspace(id, name, if id == 30 { STATE_ACTIVE } else { 0 }));
            s.apply(GroupWorkspace { group: 20, workspace: id, entered: true });
        }
        feed(&mut s, [GroupOutput { group: 20, output: 10, entered: true }, GroupOutput { group: 20, output: 11, entered: true }]);
        assert!(s.apply(Done));
        s
    }

    fn names(model: &NavigationModel) -> Vec<(u32, String, bool)> {
        let NavigationModel::Workspaces { workspaces } = model else { panic!("workspaces expected") };
        workspaces.iter().map(|w| (w.id, w.name.clone(), w.active)).collect()
    }

    #[test]
    fn nothing_shows_before_the_managers_done() {
        let mut s = State::default();
        output(&mut s, 10, "DP-1", 0);
        s.apply(GroupCreated { group: 20 });
        assert!(!feed(&mut s, workspace(30, "1", STATE_ACTIVE)));
        assert!(!s.apply(GroupWorkspace { group: 20, workspace: 30, entered: true }));
        assert!(!s.apply(GroupOutput { group: 20, output: 10, entered: true }));
        assert_eq!(names(&s.navigation()[0].1), []);
        assert!(s.apply(Done));
        assert_eq!(names(&s.navigation()[0].1), [(1, "1".into(), true)]);
        // A later batch stays invisible until its own done.
        feed(&mut s, [WorkspaceState { workspace: 30, state: 0 }]);
        assert!(names(&s.navigation()[0].1)[0].2);
        s.apply(Done);
        assert!(!names(&s.navigation()[0].1)[0].2);
    }

    #[test]
    fn one_group_on_every_output_numbers_workspaces_by_position() {
        let s = labwc();
        let nav = s.navigation();
        assert_eq!(nav.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["DP-1", "HDMI-A-1"]);
        for (_, model) in &nav {
            assert_eq!(names(model), [(1, "1".into(), true), (2, "2".into(), false), (3, "3".into(), false), (4, "4".into(), false)]);
        }
        assert_eq!(s.monitors()[0].scale, 2560.0 / 1707.0);
    }

    #[test]
    fn groups_follow_their_outputs() {
        // COSMIC's per-output workspaces: a group for each output.
        let mut s = State::default();
        output(&mut s, 10, "DP-1", 0);
        output(&mut s, 11, "DP-2", 1707);
        for (group, out, first) in [(20, 10, 30), (21, 11, 40)] {
            s.apply(GroupCreated { group });
            s.apply(GroupOutput { group, output: out, entered: true });
            for ws in first..first + 2 {
                feed(&mut s, workspace(ws, "", if ws == first + 1 { STATE_ACTIVE } else { 0 }));
                s.apply(GroupWorkspace { group, workspace: ws, entered: true });
            }
        }
        s.apply(Done);
        let nav = s.navigation();
        assert_eq!(names(&nav[0].1), [(1, "1".into(), false), (2, "2".into(), true)]);
        assert_eq!(names(&nav[1].1), [(1, "1".into(), false), (2, "2".into(), true)]);
        assert_eq!(s.plan_activate("DP-2", 1).unwrap(), [Request::Activate { workspace: 40 }, Request::Commit]);
        assert_eq!(s.plan_activate("DP-1", 1).unwrap(), [Request::Activate { workspace: 30 }, Request::Commit]);
        // The output moves to the other group.
        feed(&mut s, [GroupOutput { group: 21, output: 11, entered: false }, GroupOutput { group: 20, output: 11, entered: true }, Done]);
        assert_eq!(s.plan_activate("DP-2", 1).unwrap()[0], Request::Activate { workspace: 30 });
        assert!(s.plan_activate("HDMI-A-9", 1).is_err());
    }

    #[test]
    fn coordinates_order_workspaces_and_hidden_ones_are_left_out() {
        let mut s = State::default();
        output(&mut s, 10, "DP-1", 0);
        s.apply(GroupCreated { group: 20 });
        s.apply(GroupOutput { group: 20, output: 10, entered: true });
        for (ws, name, coord) in [(30, "c", 3), (31, "a", 1), (32, "b", 2)] {
            feed(&mut s, workspace(ws, name, 0));
            feed(&mut s, [WorkspaceCoordinates { workspace: ws, coordinates: vec![coord] }, GroupWorkspace { group: 20, workspace: ws, entered: true }]);
        }
        feed(&mut s, [WorkspaceState { workspace: 32, state: STATE_HIDDEN }, Done]);
        assert_eq!(names(&s.navigation()[0].1), [(1, "a".into(), false), (2, "c".into(), false)]);
        assert_eq!(s.plan_activate("DP-1", 2).unwrap()[0], Request::Activate { workspace: 30 });
        assert!(s.plan_activate("DP-1", 3).is_err());
        assert!(s.plan_activate("DP-1", 0).is_err());
    }

    #[test]
    fn removal_takes_workspaces_groups_and_outputs_away() {
        let mut s = labwc();
        feed(&mut s, [GroupWorkspace { group: 20, workspace: 31, entered: false }, WorkspaceRemoved { workspace: 31 }, Done]);
        assert_eq!(names(&s.navigation()[0].1).iter().map(|w| w.1.as_str()).collect::<Vec<_>>(), ["1", "3", "4"]);
        assert_eq!(s.plan_activate("DP-1", 2).unwrap()[0], Request::Activate { workspace: 32 });
        assert!(s.apply(OutputGone { output: 11 }));
        assert_eq!(s.navigation().len(), 1);
        assert!(s.current.groups[&20].outputs == [10]);
        feed(&mut s, [GroupRemoved { group: 20 }, Done]);
        assert_eq!(names(&s.navigation()[0].1), []);
    }

    #[test]
    fn activation_needs_the_capability_and_targets_the_focused_output() {
        let mut s = labwc();
        feed(&mut s, [WindowTitle { window: 50, title: "foot".into() }, WindowState { window: 50, activated: true, minimized: false, maximized: false, fullscreen: false }, WindowOutput { window: 50, output: 11, entered: true }, WindowDone]);
        assert_eq!(s.focused_output(), Some(11));
        assert_eq!(s.plan_activate("", 3).unwrap(), [Request::Activate { workspace: 32 }, Request::Commit]);
        feed(&mut s, [WorkspaceCapabilities { workspace: 32, capabilities: 0 }, Done]);
        assert!(s.plan_activate("", 3).is_err());
        assert!(s.monitors().iter().any(|m| m.name == "HDMI-A-1" && m.active));
    }

    #[test]
    fn windows_carry_workspace_numbers_occupancy_and_moves() {
        let mut s = labwc();
        assert!(feed(&mut s, [
            WindowTitle { window: 50, title: "Editor".into() },
            WindowAppId { window: 50, app_id: "dev.zed.Zed".into() },
            WindowState { window: 50, activated: true, minimized: false, maximized: true, fullscreen: false },
            WindowOutput { window: 50, output: 10, entered: true },
            WindowWorkspace { window: 50, workspace: 32, entered: true },
            WindowGeometry { window: 50, rect: (0, 0, 800, 600) },
            WindowDone,
        ]));
        let client = s.focused().unwrap();
        assert_eq!((client.id, client.monitor.as_str(), client.tags.as_slice(), client.geometry), (50, "DP-1", &[3][..], Some((0, 0, 800, 600))));
        let NavigationModel::Workspaces { workspaces } = &s.navigation()[0].1 else { unreachable!() };
        assert!(workspaces[2].occupied && !workspaces[1].occupied);
        assert_eq!(s.plan_move(None, 2).unwrap(), Request::MoveWindow { window: 50, workspace: 31, output: 10 });
        assert_eq!(s.plan_move(Some(50), 4).unwrap(), Request::MoveWindow { window: 50, workspace: 33, output: 10 });
        assert!(s.plan_move(Some(99), 1).is_err());
        // A workspace that goes away leaves the window's list at done.
        feed(&mut s, [GroupWorkspace { group: 20, workspace: 32, entered: false }, WorkspaceRemoved { workspace: 32 }, Done]);
        assert!(s.focused().unwrap().tags.is_empty());
        assert!(s.apply(WindowClosed { window: 50 }));
        assert!(s.focused().is_none() && s.clients().is_empty());
        assert!(!s.apply(WindowClosed { window: 50 }), "a second close changes nothing");
    }

    #[test]
    fn diff_sends_only_what_changed() {
        let mut s = labwc();
        let mut diff = Diff::default();
        let mut out = Vec::new();
        diff.events(&s, &mut out);
        assert_eq!(out.iter().filter(|e| matches!(e, CompositorEvent::Navigation { .. })).count(), 2);
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::Monitors(m) if m.len() == 2)));
        out.clear();
        diff.events(&s, &mut out);
        assert!(out.is_empty());
        feed(&mut s, [WorkspaceState { workspace: 30, state: 0 }, WorkspaceState { workspace: 31, state: STATE_ACTIVE }, Done]);
        diff.events(&s, &mut out);
        assert_eq!(out.len(), 2, "both outputs show the shared group: {out:?}");
        assert!(out.iter().all(|e| matches!(e, CompositorEvent::Navigation { model, .. } if model.contains_active(&[2]))));
        out.clear();
        feed(&mut s, [WindowTitle { window: 7, title: "t".into() }, WindowDone]);
        diff.events(&s, &mut out);
        assert!(matches!(out.as_slice(), [CompositorEvent::Clients(c)] if c.len() == 1));
    }

    /// What cosmic-comp sent (recorded from a nested session) when a window
    /// left the only occupied workspace: that workspace went away, the next
    /// one took its name and place, and a fresh empty one was appended.
    #[test]
    fn cosmic_renumbers_dynamic_workspaces_after_a_move() {
        let mut s = State::default();
        output(&mut s, 11, "WINIT-0", 0);
        s.apply(GroupCreated { group: 20 });
        for (ws, name, coord, state) in [(81, "1", 1, STATE_ACTIVE), (82, "2", 2, 0)] {
            feed(&mut s, workspace(ws, name, state));
            feed(&mut s, [WorkspaceCoordinates { workspace: ws, coordinates: vec![coord] }, GroupWorkspace { group: 20, workspace: ws, entered: true }]);
        }
        // The group's output arrives in a later batch.
        s.apply(Done);
        assert_eq!(s.navigation()[0].1.active_ids().collect::<Vec<_>>(), [1], "a group without outputs shows everywhere meanwhile");
        feed(&mut s, [GroupOutput { group: 20, output: 11, entered: true }, Done]);
        feed(&mut s, [WindowState { window: 83, activated: true, minimized: false, maximized: false, fullscreen: false }, WindowOutput { window: 83, output: 11, entered: true }, WindowWorkspace { window: 83, workspace: 81, entered: true }, WindowDone]);
        assert_eq!(s.focused().unwrap().tags, [1]);
        assert_eq!(s.plan_move(Some(83), 2).unwrap(), Request::MoveWindow { window: 83, workspace: 82, output: 11 });
        feed(&mut s, [
            GroupWorkspace { group: 20, workspace: 81, entered: false },
            WorkspaceRemoved { workspace: 81 },
            WorkspaceName { workspace: 82, name: "1".into() },
            WorkspaceCoordinates { workspace: 82, coordinates: vec![1] },
            WorkspaceCreated { workspace: 84 },
            GroupWorkspace { group: 20, workspace: 84, entered: true },
            WorkspaceName { workspace: 84, name: "2".into() },
            WorkspaceCoordinates { workspace: 84, coordinates: vec![2] },
            WorkspaceCapabilities { workspace: 84, capabilities: CAN_ACTIVATE },
            WorkspaceState { workspace: 84, state: 0 },
            Done,
            // No leave for the removed workspace: done already dropped it.
            WindowWorkspace { window: 83, workspace: 82, entered: true },
            WindowDone,
        ]);
        assert_eq!(s.windows[&83].workspaces, [82]);
        assert_eq!(s.focused().unwrap().tags, [1], "the window's workspace is the first one now");
        assert_eq!(names(&s.navigation()[0].1).iter().map(|w| w.1.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        let NavigationModel::Workspaces { workspaces } = &s.navigation()[0].1 else { unreachable!() };
        assert!(workspaces[0].occupied && !workspaces[1].occupied);
        assert_eq!(s.plan_activate("WINIT-0", 2).unwrap()[0], Request::Activate { workspace: 84 });
    }

    #[test]
    fn groups_without_outputs_show_everywhere_and_idle_reports_changes() {
        let mut s = State::default();
        output(&mut s, 10, "DP-1", 0);
        s.apply(GroupCreated { group: 20 });
        feed(&mut s, workspace(30, "main", STATE_ACTIVE | STATE_URGENT));
        feed(&mut s, [GroupWorkspace { group: 20, workspace: 30, entered: true }, Done]);
        let NavigationModel::Workspaces { workspaces } = &s.navigation()[0].1 else { unreachable!() };
        assert!(workspaces[0].urgent && workspaces[0].active);
        assert!(s.apply(Idle(true)));
        assert!(!s.apply(Idle(true)));
        assert!(s.idle);
    }
}
