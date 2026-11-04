//! The shared window actor backed by Plasma's native window protocol.

use std::io::PipeReader;
use std::sync::mpsc;

use mandara_compositor::kwin::{Session, WindowAction, state::{State, window_id}};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::State as Flag;

use super::{Toplevel, ToplevelCommand, ToplevelsState, touch};
use crate::actor::{Envelope, reply};

fn snapshot(state: &State, order: &mut Vec<u64>) -> ToplevelsState {
    let mut windows: Vec<_> = state.windows.iter().filter(|(_, w)| !w.has(Flag::Skiptaskbar)).map(|(uuid, w)| Toplevel {
        id: window_id(uuid), app_id: w.app_id.clone(), title: w.title.clone(), activated: w.has(Flag::Active),
        minimized: w.has(Flag::Minimized), maximized: w.has(Flag::Maximized), fullscreen: w.has(Flag::Fullscreen),
        outputs: state.window_outputs(w), parent: w.parent.as_deref().map(window_id),
    }).collect();
    order.retain(|id| windows.iter().any(|w| w.id == *id));
    if let Some(active) = windows.iter().find(|w| w.activated) {
        touch(order, active.id);
    }
    windows.sort_by_key(|w| order.iter().position(|id| *id == w.id).unwrap_or(usize::MAX));
    ToplevelsState { available: true, toplevels: windows, idle: state.idle }
}

pub(super) fn run(updates: async_channel::Sender<ToplevelsState>, commands: mpsc::Receiver<Envelope<ToplevelCommand>>, wake: PipeReader) -> Result<(), String> {
    let mut session = Session::open().map_err(|e| e.to_string())?;
    let mut order = Vec::new();
    let mut previous = None;
    loop {
        while let Ok(Envelope { command, reply: response }) = commands.try_recv() {
            let action = match command {
                ToplevelCommand::Activate(id) => Some((id, WindowAction::Activate)),
                ToplevelCommand::Close(id) => Some((id, WindowAction::Close)),
                ToplevelCommand::SetMinimized(id, on) => Some((id, WindowAction::Minimized(on))),
                ToplevelCommand::SetMaximized(id, on) => Some((id, WindowAction::Maximized(on))),
                ToplevelCommand::SetFullscreen(id, on) => Some((id, WindowAction::Fullscreen(on))),
                ToplevelCommand::IdleTimeout(ms) => {
                    reply(&response, session.idle_timeout(ms).map(|_| String::new()).map_err(|e| e.to_string()));
                    None
                }
            };
            if let Some((id, action)) = action {
                reply(&response, session.window_action(id, &action).map(|_| String::new()).map_err(|e| e.to_string()));
            }
        }
        let next = snapshot(session.state(), &mut order);
        if previous.as_ref() != Some(&next) {
            if updates.send_blocking(next.clone()).is_err() { return Ok(()); }
            previous = Some(next);
        }
        if !session.wait(Some(&wake)).map_err(|e| e.to_string())? { return Ok(()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_compositor::kwin::state::{Output, Window};

    #[test]
    fn plasma_windows_keep_state_parent_output_and_recent_focus() {
        let mut state = State::default();
        state.outputs.insert(1, Output { name: "DP-1".into(), width: 1920, height: 1080, ..Default::default() });
        state.windows.insert("editor".into(), Window { app_id: "editor".into(), title: "Notes".into(), geometry: Some((0, 0, 800, 600)), flags: Flag::Active as u32, ..Default::default() });
        state.windows.insert("dialog".into(), Window { parent: Some("editor".into()), flags: Flag::Minimized as u32, ..Default::default() });
        let mut order = Vec::new();
        let first = snapshot(&state, &mut order);
        assert_eq!(first.toplevels[0].id, window_id("editor"));
        assert_eq!(first.toplevels[0].outputs, ["DP-1"]);
        assert!(first.toplevels[1].minimized);
        assert_eq!(first.toplevels[1].parent, Some(window_id("editor")));
        state.windows.get_mut("editor").unwrap().flags = 0;
        state.windows.get_mut("dialog").unwrap().flags = Flag::Active as u32 | Flag::Fullscreen as u32;
        let next = snapshot(&state, &mut order);
        assert_eq!(next.toplevels[0].id, window_id("dialog"));
        assert!(next.toplevels[0].fullscreen);
        state.windows.remove("dialog");
        assert_eq!(snapshot(&state, &mut order).toplevels.len(), 1);
    }
}
