//! The open-window list shared by the dock, the Alt-Tab switcher, the
//! launcher's windows provider and screen time: a GPUI entity mirroring
//! [`mandara_services::toplevels`] (most-recently-used order), plus
//! activate / close helpers.

use gpui::{App, AppContext as _, AsyncApp, Entity, Global};
use mandara_services::toplevels::{Toplevel, ToplevelCommand, ToplevelService, ToplevelsState, spawn_toplevel_service};

/// Live window list.
#[derive(Default)]
pub struct Windows {
    /// Windows in most-recently-used order, `activated` reflecting focus
    /// from the window protocol and the compositor's own focus events.
    pub state: ToplevelsState,
    /// The list as the window protocol last reported it.
    raw: ToplevelsState,
    /// Window ids, most recently focused first (merged from both sources:
    /// some compositors don't report `activated` reliably).
    mru: Vec<u64>,
    /// The focused window by the compositor's IPC, when it matches one.
    hub_focus: Option<u64>,
    service: Option<ToplevelService>,
}

/// The window a compositor client (`app_id`, `title`) is, if listed.
pub fn match_client(list: &[Toplevel], app_id: &str, title: &str) -> Option<u64> {
    list.iter().find(|t| t.app_id == app_id && t.title == title).or_else(|| {
        let mut same = list.iter().filter(|t| t.app_id == app_id);
        // Titles change fast: an app with one window still matches.
        match (same.next(), same.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    }).map(|t| t.id)
}

impl Windows {
    /// Rebuilds [`Self::state`] from the protocol list, the MRU order and
    /// the compositor's focus.
    fn merge(&mut self) -> bool {
        if let Some(active) = self.raw.active().map(|t| t.id) {
            mandara_services::toplevels::touch(&mut self.mru, active);
        }
        let live: Vec<u64> = self.raw.toplevels.iter().map(|t| t.id).collect();
        self.mru.retain(|id| live.contains(id));
        let rank = |id: u64, mru: &[u64], fallback: usize| mru.iter().position(|x| *x == id).unwrap_or(mru.len() + fallback);
        let mut list: Vec<Toplevel> = self.raw.toplevels.clone();
        let order: Vec<(u64, usize)> = list.iter().enumerate().map(|(i, t)| (t.id, rank(t.id, &self.mru, i))).collect();
        list.sort_by_key(|t| order.iter().find(|(id, _)| *id == t.id).map(|(_, r)| *r).unwrap_or(usize::MAX));
        let protocol_focus = self.raw.active().map(|t| t.id);
        let focused = protocol_focus.or(self.hub_focus);
        for t in &mut list {
            t.activated = Some(t.id) == focused;
        }
        let next = ToplevelsState { available: self.raw.available, toplevels: list, idle: self.raw.idle };
        let changed = next != self.state;
        self.state = next;
        changed
    }

    /// Follows the compositor's focus (`None`: nothing / a shell surface).
    fn set_hub_focus(&mut self, client: Option<(&str, &str)>) -> bool {
        let id = client.and_then(|(app, title)| match_client(&self.raw.toplevels, app, title));
        if id == self.hub_focus {
            return false;
        }
        self.hub_focus = id;
        if let Some(id) = id {
            mandara_services::toplevels::touch(&mut self.mru, id);
        }
        self.merge()
    }

    /// Windows in most-recently-used order.
    pub fn list(&self) -> &[Toplevel] {
        &self.state.toplevels
    }

    pub fn available(&self) -> bool {
        self.state.available
    }

    /// Sends a request to the compositor (fire and forget).
    pub fn send(&self, command: ToplevelCommand) {
        if let Some(s) = &self.service {
            s.send(command);
        }
    }

    /// Sends a request and returns its result channel.
    pub fn request(&self, command: ToplevelCommand) -> Result<async_channel::Receiver<Result<String, String>>, String> {
        self.service.as_ref().map(|s| s.request(command)).ok_or_else(|| mandara_i18n::t("The window list is not available").to_string())
    }
}

struct GlobalWindows(Entity<Windows>);
impl Global for GlobalWindows {}

/// Starts the window list once (idempotent) and returns it.
pub fn init(cx: &mut App) -> Entity<Windows> {
    if let Some(g) = cx.try_global::<GlobalWindows>() {
        return g.0.clone();
    }
    let service = spawn_toplevel_service();
    let updates = service.updates.clone();
    let entity = cx.new(|_| Windows { service: Some(service), ..Default::default() });
    cx.set_global(GlobalWindows(entity.clone()));
    let pump = entity.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(mut state) = updates.recv().await {
            while let Ok(newer) = updates.try_recv() {
                state = newer;
            }
            pump.update(cx, |w, cx| {
                if w.raw != state {
                    w.raw = state;
                    if w.merge() {
                        cx.notify();
                    }
                }
            });
        }
    })
    .detach();
    // The compositor's own focus events fill in where the protocol's
    // `activated` state is missing.
    let follow = entity.clone();
    cx.observe(&crate::hub::hub(cx), move |hub, cx| {
        let focused = hub.read(cx).focused.as_ref().map(|c| (c.app_id.clone(), c.title.clone()));
        follow.update(cx, |w, cx| {
            if w.set_hub_focus(focused.as_ref().map(|(a, t)| (a.as_str(), t.as_str()))) {
                cx.notify();
            }
        });
    })
    .detach();
    entity
}

/// The window list, when started.
pub fn windows(cx: &App) -> Option<Entity<Windows>> {
    cx.try_global::<GlobalWindows>().map(|g| g.0.clone())
}

/// Focuses window `id` (un-minimizing it).
pub fn activate(id: u64, cx: &App) {
    if let Some(w) = windows(cx) {
        w.read(cx).send(ToplevelCommand::Activate(id));
    }
}

/// Asks window `id` to close.
pub fn close(id: u64, cx: &App) {
    if let Some(w) = windows(cx) {
        w.read(cx).send(ToplevelCommand::Close(id));
    }
}

/// Whether a window's app id belongs to Mandara itself (never listed).
pub fn is_shell(app_id: &str) -> bool {
    app_id == mandara_core::SHELL_APP_ID
}

/// App ids that name the same app: desktop id vs Wayland app id differ in
/// case and in reverse-DNS prefixes (`org.gnome.Nautilus` vs `nautilus`).
pub fn same_app(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    let tail = |s: &str| s.rsplit('.').next().unwrap_or(s).to_ascii_lowercase();
    // A plain id matches a reverse-DNS one by its last segment; two
    // reverse-DNS ids only match exactly (`org.mandara.studio` is not
    // `com.obsproject.Studio`).
    match (a.contains('.'), b.contains('.')) {
        (true, false) => tail(a) == b.to_ascii_lowercase(),
        (false, true) => tail(b) == a.to_ascii_lowercase(),
        _ => false,
    }
}

/// Resolves an app id to its desktop entry (for launching / actions).
pub fn desktop_entry_for<'a>(app_id: &str, apps: &'a [crate::apps::App]) -> Option<&'a crate::apps::App> {
    apps.iter().find(|a| a.id.eq_ignore_ascii_case(app_id) || a.wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(app_id))).or_else(|| apps.iter().find(|a| same_app(&a.id, app_id)))
}

/// Owner of the window commands (the switcher plugin).
pub const OWNER: &str = "switcher";

fn window_id(a: &mandara_plugins::Args) -> Result<u64, String> {
    a.str(0)?.parse().map_err(|_| mandara_i18n::t("id must be a number (see `window list`)").to_string())
}

/// Sends `command` and hands its result to the caller.
fn request(command: ToplevelCommand, cx: &mut App) -> Result<mandara_plugins::Output, String> {
    let w = windows(cx).ok_or_else(|| mandara_i18n::t("The window list is not available").to_string())?;
    Ok(mandara_plugins::Output::Pending(w.read(cx).request(command)?))
}

/// `window list|focus|close|minimize|maximize|fullscreen`.
pub fn register(r: &mut mandara_plugins::PluginRegistry) {
    use mandara_i18n::tk;
    use mandara_plugins::{ArgKind, ArgSpec, CommandSpec};
    r.command(
        OWNER,
        CommandSpec::new("window list", tk("Open windows, most recently used first"), |_, cx| {
            let list = windows(cx).map(|w| w.read(cx).state.toplevels.iter().filter(|t| !is_shell(&t.app_id)).cloned().collect::<Vec<_>>()).unwrap_or_default();
            Ok(serde_json::to_value(list).unwrap_or_default())
        }),
    );
    r.command(OWNER, CommandSpec::new_output("window focus", tk("Focus a window (by id from `window list`)"), |a, cx| request(ToplevelCommand::Activate(window_id(a)?), cx)).arg(ArgSpec::required("id", ArgKind::Integer, tk("window id"))));
    r.command(OWNER, CommandSpec::new_output("window close", tk("Ask a window to close"), |a, cx| request(ToplevelCommand::Close(window_id(a)?), cx)).arg(ArgSpec::required("id", ArgKind::Integer, tk("window id"))));
    for (path, summary, which) in [
        ("window minimize", tk("Minimize or restore a window"), 0u8),
        ("window maximize", tk("Maximize or restore a window"), 1),
        ("window fullscreen", tk("Make a window fullscreen or restore it"), 2),
    ] {
        r.command(
            OWNER,
            CommandSpec::new_output(path, summary, move |a, cx| {
                let id = window_id(a)?;
                let on = a.get(1).is_none_or(|v| matches!(v, "on" | "true" | "1"));
                request(
                    match which {
                        0 => ToplevelCommand::SetMinimized(id, on),
                        1 => ToplevelCommand::SetMaximized(id, on),
                        _ => ToplevelCommand::SetFullscreen(id, on),
                    },
                    cx,
                )
            })
            .arg(ArgSpec::required("id", ArgKind::Integer, tk("window id")))
            .arg(ArgSpec::optional("state", ArgKind::Bool, tk("on (default) or off"))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_focus_from_both_sources() {
        let t = |id: u64, app: &str, title: &str| Toplevel { id, app_id: app.into(), title: title.into(), ..Default::default() };
        let mut w = Windows { raw: ToplevelsState { available: true, toplevels: vec![t(1, "zed", "a"), t(2, "fox", "b"), t(3, "term", "c")], idle: false }, ..Default::default() };
        w.merge();
        assert_eq!(w.state.toplevels.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 2, 3]);
        // The compositor says "term" has focus (the protocol said nothing).
        assert!(w.set_hub_focus(Some(("term", "renamed title"))));
        assert_eq!(w.state.active().map(|t| t.id), Some(3));
        assert_eq!(w.state.toplevels[0].id, 3);
        w.set_hub_focus(Some(("fox", "b")));
        assert_eq!(w.state.toplevels.iter().map(|t| t.id).collect::<Vec<_>>(), [2, 3, 1]);
        assert_eq!(match_client(&w.raw.toplevels, "nope", ""), None);
    }

    #[test]
    fn app_ids_match_across_naming_styles() {
        assert!(same_app("firefox", "Firefox"));
        assert!(same_app("org.gnome.Nautilus", "nautilus"));
        assert!(same_app("com.mitchellh.ghostty", "com.mitchellh.ghostty"));
        assert!(!same_app("org.mandara.studio", "com.obsproject.Studio"));
        assert!(!same_app("zed", "firefox"));
    }
}
