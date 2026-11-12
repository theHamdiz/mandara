//! The shared window actor on compositors without the wlr foreign-toplevel
//! manager but with `ext_foreign_toplevel_list_v1` (COSMIC): titles and app
//! ids from the ext list, state and outputs from COSMIC's toplevel info, and
//! actions through COSMIC's toplevel manager. With the ext list alone, the
//! dock still lists windows but can't tell which one is focused or act on
//! them.

use std::io::PipeReader;
use std::sync::mpsc;

use mandara_compositor::ext_workspace::state::State;
use mandara_compositor::ext_workspace::{Options, Session};
use mandara_compositor::kwin::WindowAction;

use super::{Toplevel, ToplevelCommand, ToplevelsState, touch};
use crate::actor::{Envelope, reply};

fn snapshot(state: &State, order: &mut Vec<u64>) -> ToplevelsState {
    let output_name = |id: &u32| state.outputs.get(id).map(|o| o.name.clone()).filter(|n| !n.is_empty());
    let mut windows: Vec<Toplevel> = state
        .windows
        .iter()
        .map(|(id, w)| Toplevel {
            id: u64::from(*id),
            app_id: w.app_id.clone(),
            title: w.title.clone(),
            activated: w.activated,
            minimized: w.minimized,
            maximized: w.maximized,
            fullscreen: w.fullscreen,
            outputs: w.outputs.iter().filter_map(output_name).collect(),
            parent: None,
        })
        .collect();
    order.retain(|id| windows.iter().any(|w| w.id == *id));
    for w in &windows {
        if !order.contains(&w.id) {
            // New windows go to the back until they are activated.
            order.push(w.id);
        }
    }
    if let Some(active) = windows.iter().find(|w| w.activated) {
        touch(order, active.id);
    }
    windows.sort_by_key(|w| order.iter().position(|id| *id == w.id).unwrap_or(usize::MAX));
    ToplevelsState { available: true, toplevels: windows, idle: state.idle }
}

pub(super) fn run(updates: async_channel::Sender<ToplevelsState>, commands: mpsc::Receiver<Envelope<ToplevelCommand>>, wake: PipeReader) -> Result<(), String> {
    let mut session = Session::open(Options { workspaces: false }).map_err(|e| e.to_string())?;
    if !session.has_windows() {
        let _ = updates.send_blocking(ToplevelsState::default());
        return Err("the compositor offers no window list".into());
    }
    let mut order = Vec::new();
    let mut previous = None;
    loop {
        while let Ok(Envelope { command, reply: response }) = commands.try_recv() {
            let action = match command {
                ToplevelCommand::Activate(id) => (id, WindowAction::Activate),
                ToplevelCommand::Close(id) => (id, WindowAction::Close),
                ToplevelCommand::SetMinimized(id, on) => (id, WindowAction::Minimized(on)),
                ToplevelCommand::SetMaximized(id, on) => (id, WindowAction::Maximized(on)),
                ToplevelCommand::SetFullscreen(id, on) => (id, WindowAction::Fullscreen(on)),
                ToplevelCommand::IdleTimeout(ms) => {
                    reply(&response, session.idle_timeout(ms).map(|_| String::new()).map_err(|e| e.to_string()));
                    continue;
                }
            };
            reply(&response, session.window_action(action.0, &action.1).map(|_| String::new()).map_err(|e| e.to_string()));
        }
        if session.take_published() {
            let next = snapshot(session.state(), &mut order);
            if previous.as_ref() != Some(&next) {
                if updates.send_blocking(next.clone()).is_err() {
                    return Ok(());
                }
                previous = Some(next);
            }
        }
        if !session.wait(Some(&wake)).map_err(|e| e.to_string())? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_compositor::ext_workspace::state::Event;

    #[test]
    fn cosmic_windows_keep_state_outputs_and_recent_focus() {
        let mut state = State::default();
        state.apply(Event::OutputName { output: 1, name: "DP-1".into() });
        for (window, title) in [(10, "Notes"), (11, "Files")] {
            state.apply(Event::WindowTitle { window, title: title.into() });
            state.apply(Event::WindowOutput { window, output: 1, entered: true });
        }
        state.apply(Event::WindowState { window: 11, activated: true, minimized: false, maximized: true, fullscreen: false });
        let mut order = Vec::new();
        let first = snapshot(&state, &mut order);
        assert_eq!(first.toplevels.iter().map(|t| t.id).collect::<Vec<_>>(), [11, 10]);
        assert_eq!(first.toplevels[0].outputs, ["DP-1"]);
        assert!(first.toplevels[0].maximized && first.available);
        state.apply(Event::WindowState { window: 11, activated: false, minimized: true, maximized: false, fullscreen: false });
        state.apply(Event::WindowState { window: 10, activated: true, minimized: false, maximized: false, fullscreen: true });
        let next = snapshot(&state, &mut order);
        assert_eq!(next.toplevels[0].id, 10);
        assert!(next.toplevels[0].fullscreen && next.toplevels[1].minimized);
        state.apply(Event::WindowClosed { window: 10 });
        assert_eq!(snapshot(&state, &mut order).toplevels.len(), 1);
        assert_eq!(order, [11]);
    }

    /// Through the shared service against a real cosmic-comp (this path) or
    /// labwc (the wlr one) in an isolated, headless session (its own
    /// `XDG_RUNTIME_DIR` with the marker file) and a fixture window whose
    /// app id is `MANDARA_EXT_WORKSPACE_WINDOW`; never the desktop's own.
    #[test]
    #[ignore = "requires an isolated cosmic-comp or labwc, its marker and a fixture window"]
    fn real_compositor_lists_and_changes_windows() {
        use std::io::Write as _;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap();
        assert!(runtime.starts_with("/tmp") || runtime.starts_with("/var/tmp"), "not the desktop's runtime directory");
        assert_eq!(std::fs::read_to_string(runtime.join(".mandara-ext-workspace-test")).unwrap().trim(), "mandara isolated ext-workspace test");
        let app = std::env::var("MANDARA_EXT_WORKSPACE_WINDOW").unwrap();
        let (tx, updates) = async_channel::unbounded();
        let (commands, rx) = mpsc::channel();
        let (wake_rx, mut wake) = std::io::pipe().unwrap();
        // The shared service picks this path itself: no wlr manager there.
        let worker = std::thread::spawn(move || super::super::run(tx, rx, wake_rx));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let next = |done: &dyn Fn(&ToplevelsState) -> bool| {
            while std::time::Instant::now() < deadline {
                if let Ok(state) = updates.try_recv() {
                    if done(&state) {
                        return Some(state);
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            None
        };
        let listed = next(&|s| s.toplevels.iter().any(|t| t.app_id == app && !t.outputs.is_empty())).expect("the fixture window with its output");
        let id = listed.toplevels.iter().find(|t| t.app_id == app).unwrap().id;
        // Minimized, then activated again (which restores it).
        for (command, done) in [(ToplevelCommand::SetMinimized(id, true), &(|t: &Toplevel| t.minimized) as &dyn Fn(&Toplevel) -> bool), (ToplevelCommand::Activate(id), &|t: &Toplevel| t.activated && !t.minimized)] {
            let (reply, answer) = async_channel::bounded(1);
            commands.send(Envelope { command: command.clone(), reply: Some(reply) }).unwrap();
            wake.write_all(&[1]).unwrap();
            assert!(answer.recv_blocking().unwrap().is_ok(), "{command:?}");
            assert!(next(&|s| s.get(id).is_some_and(done)).is_some(), "{command:?} took effect");
        }
        drop(wake);
        assert!(worker.join().unwrap().is_ok(), "the worker stops when its wake pipe closes");
    }
}
