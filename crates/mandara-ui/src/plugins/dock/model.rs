//! The dock's pure logic: which slots it shows and in what order, the
//! geometry helpers, magnification and bounce curves, badges, attention,
//! recent apps and opening files with an app. No GPUI state here, so all
//! of it is unit-tested.

use mandara_config::workflow::{DockConfig, DockPosition};
use mandara_services::notifications::{Notification, Urgency};
use mandara_services::toplevels::Toplevel;

use crate::apps::App as DesktopApp;
use crate::windows::same_app;

/// Gaussian magnification: the scale profile's width in icon sizes.
const MAGNIFY_SIGMA: f32 = 1.0;
/// Launch bounce: one hop lasts this long, seconds.
pub const LAUNCH_HOP_S: f32 = 0.62;
/// Attention bounce: hops per burst and the rest between bursts, seconds.
pub const ATTENTION_HOPS: f32 = 2.0;
pub const ATTENTION_REST_S: f32 = 1.4;
/// Hop height as a fraction of the icon size.
pub const HOP_FRAC: f32 = 0.42;

// ── Slots ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    App,
    Launcher,
    Separator,
    /// A minimized window on the right side (when windows don't minimize
    /// into their app's icon).
    Minimized(u64),
    /// Stack `n` of `DockConfig::stacks`.
    Stack(usize),
    /// The Applications folder (a Launchpad-style grid; drop apps on it to
    /// install them).
    Applications,
    Trash,
    /// An app being installed (install job id).
    Installing(u64),
    /// The now-playing chip (`dock.media_chip`, while a player is active).
    Media,
}

/// One slot of the dock.
#[derive(Clone, Debug)]
pub struct Item {
    /// Desktop id (pinned, recent) or app id (running), or a fixed key.
    pub key: String,
    /// App id used for the icon.
    pub icon_id: String,
    pub name: String,
    pub desktop: Option<DesktopApp>,
    /// Windows of this app, most recently used first.
    pub windows: Vec<Toplevel>,
    pub pinned: bool,
    /// Shown in the recent-apps section (not pinned, not running).
    pub recent: bool,
    pub kind: Kind,
}

impl Item {
    pub fn focused(&self) -> bool {
        self.windows.iter().any(|w| w.activated)
    }

    pub fn running(&self) -> bool {
        !self.windows.is_empty()
    }

    /// The id "Keep in Dock" pins: the desktop id when the app is
    /// installed, else the window's app id.
    pub fn pin_key(&self) -> String {
        self.desktop.as_ref().map(|d| d.id.clone()).unwrap_or_else(|| self.key.clone())
    }

    fn fixed(kind: Kind, key: &str) -> Self {
        Item { key: key.into(), icon_id: String::new(), name: String::new(), desktop: None, windows: Vec::new(), pinned: false, recent: false, kind }
    }

    pub fn is_app(&self) -> bool {
        self.kind == Kind::App
    }
}

/// An install in progress, as the dock draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct InstallSlot {
    pub id: u64,
    pub name: String,
    pub icon: String,
    /// 0–1.
    pub progress: f32,
    pub failed: Option<String>,
}

/// What the dock's slots are built from.
pub struct Inputs<'a> {
    pub cfg: &'a DockConfig,
    pub windows: &'a [Toplevel],
    pub apps: &'a [DesktopApp],
    /// Output the dock is on ("" = all).
    pub monitor: &'a str,
    /// Apps unpinned while running: they keep their icon until they quit,
    /// even when running apps aren't shown.
    pub linger: &'a [String],
    /// Recently used desktop ids, most recent first.
    pub recent: &'a [String],
    pub installing: &'a [InstallSlot],
    /// A media player is active (playing or paused).
    pub media: bool,
}

/// Whether window app id `app_id` belongs to desktop entry `entry` (or the
/// pinned id when the entry isn't installed).
pub fn belongs(app_id: &str, pinned: &str, entry: Option<&DesktopApp>) -> bool {
    same_app(pinned, app_id) || entry.is_some_and(|e| e.wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(app_id)) || same_app(&e.id, app_id))
}

fn app_name(id: &str) -> String {
    crate::plugins::window::app_name(id)
}

/// The dock's slots, macOS order: launcher, pinned apps (with their
/// windows), other running apps, installs in progress | recent apps |
/// minimized windows, Applications, stacks, trash. Separators sit between
/// non-empty sections.
pub fn items(input: &Inputs) -> Vec<Item> {
    let cfg = input.cfg;
    let windows: Vec<&Toplevel> = input
        .windows
        .iter()
        .filter(|w| !crate::windows::is_shell(&w.app_id) && w.parent.is_none())
        .filter(|w| !cfg.isolate_displays || input.monitor.is_empty() || w.outputs.is_empty() || w.outputs.iter().any(|o| o == input.monitor))
        .collect();
    let mut used = vec![false; windows.len()];
    let mut out = Vec::new();
    if cfg.launcher_button && !cfg.launcher_at_end {
        out.push(Item::fixed(Kind::Launcher, "@launcher"));
    }
    for id in &cfg.pinned {
        let entry = input.apps.iter().find(|a| a.id.eq_ignore_ascii_case(id)).cloned();
        let mut mine: Vec<Toplevel> = Vec::new();
        for (i, w) in windows.iter().enumerate() {
            if !used[i] && belongs(&w.app_id, id, entry.as_ref()) {
                used[i] = true;
                mine.push((*w).clone());
            }
        }
        let name = entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| app_name(id));
        let icon_id = entry.as_ref().and_then(|e| e.wm_class.clone()).filter(|_| mine.is_empty()).or_else(|| mine.first().map(|w| w.app_id.clone())).unwrap_or_else(|| id.clone());
        out.push(Item { key: id.clone(), icon_id, name, desktop: entry, windows: mine, pinned: true, recent: false, kind: Kind::App });
    }
    // Other running apps, grouped by app id in window-creation order
    // (stable while you switch between them).
    let mut rest: Vec<(&str, Vec<Toplevel>)> = Vec::new();
    let mut by_age: Vec<&Toplevel> = windows.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(_, w)| *w).collect();
    by_age.sort_by_key(|w| w.id);
    for w in by_age {
        match rest.iter_mut().find(|(a, _)| *a == w.app_id) {
            Some((_, list)) => list.push(w.clone()),
            None => rest.push((&w.app_id, vec![w.clone()])),
        }
    }
    for (app_id, mut list) in rest {
        let entry = crate::windows::desktop_entry_for(app_id, input.apps).cloned();
        let lingering = input.linger.iter().any(|l| same_app(l, app_id) || entry.as_ref().is_some_and(|e| e.id.eq_ignore_ascii_case(l)));
        if !cfg.show_running && !lingering {
            continue;
        }
        // Most recently used window first within the app.
        list.sort_by_key(|w| windows.iter().position(|x| x.id == w.id));
        let name = entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| app_name(app_id));
        out.push(Item { key: app_id.to_string(), icon_id: app_id.to_string(), name, desktop: entry, windows: list, pinned: false, recent: false, kind: Kind::App });
    }
    for job in input.installing {
        out.push(Item { key: format!("@install-{}", job.id), icon_id: job.icon.clone(), name: job.name.clone(), desktop: None, windows: Vec::new(), pinned: false, recent: false, kind: Kind::Installing(job.id) });
    }
    if cfg.max_apps > 0 {
        let mut seen = 0;
        out.retain(|i| {
            if i.kind != Kind::App {
                return true;
            }
            seen += 1;
            seen <= cfg.max_apps as usize
        });
    }
    if cfg.launcher_button && cfg.launcher_at_end {
        out.push(Item::fixed(Kind::Launcher, "@launcher"));
    }

    // Recent apps.
    let mut recent = Vec::new();
    if cfg.show_recent && cfg.recent_count > 0 {
        for id in input.recent {
            if recent.len() >= cfg.recent_count as usize {
                break;
            }
            let Some(entry) = input.apps.iter().find(|a| a.id.eq_ignore_ascii_case(id)) else { continue };
            let shown = out.iter().chain(recent.iter()).any(|i: &Item| i.is_app() && (i.key.eq_ignore_ascii_case(&entry.id) || i.desktop.as_ref().is_some_and(|d| d.id == entry.id)));
            if shown {
                continue;
            }
            recent.push(Item { key: entry.id.clone(), icon_id: entry.wm_class.clone().unwrap_or_else(|| entry.id.clone()), name: entry.name.clone(), desktop: Some(entry.clone()), windows: Vec::new(), pinned: false, recent: true, kind: Kind::App });
        }
    }
    push_section(&mut out, recent, cfg.separator, "@separator-recent");

    // The right side.
    let mut right = Vec::new();
    if !cfg.minimize_to_app {
        for it in out.iter().filter(|i| i.is_app()) {
            for w in it.windows.iter().filter(|w| w.minimized) {
                right.push(Item { key: format!("@min-{}", w.id), icon_id: it.icon_id.clone(), name: if w.title.is_empty() { it.name.clone() } else { w.title.clone() }, desktop: it.desktop.clone(), windows: vec![w.clone()], pinned: false, recent: false, kind: Kind::Minimized(w.id) });
            }
        }
    }
    // The Applications folder leads the folders, like macOS's.
    if cfg.applications.enabled {
        right.push(Item::fixed(Kind::Applications, "@applications"));
    }
    for (i, _) in cfg.stacks.iter().enumerate() {
        right.push(Item::fixed(Kind::Stack(i), &format!("@stack-{i}")));
    }
    if cfg.trash {
        right.push(Item::fixed(Kind::Trash, "@trash"));
    }
    push_section(&mut out, right, cfg.separator, "@separator-right");
    if cfg.media_chip && input.media {
        push_section(&mut out, vec![Item::fixed(Kind::Media, MEDIA_KEY)], cfg.separator, "@separator-media");
    }
    out
}

/// The media chip's slot key.
pub const MEDIA_KEY: &str = "@media";

fn push_section(out: &mut Vec<Item>, section: Vec<Item>, separator: bool, key: &str) {
    if section.is_empty() {
        return;
    }
    if separator && !out.is_empty() && out.last().is_some_and(|i| i.kind != Kind::Separator) {
        out.push(Item::fixed(Kind::Separator, key));
    }
    out.extend(section);
}

/// Where a pinned slot lands when a dragged icon is dropped at `pointer`
/// along the dock: before the first pinned centre past the pointer.
pub fn pin_drop_index(pinned_centres: &[f32], pointer: f32) -> usize {
    pinned_centres.iter().position(|c| pointer < *c).unwrap_or(pinned_centres.len())
}

/// Where a dragged pinned icon lands: the pinned slot whose centre is
/// nearest to the pointer.
pub fn drop_index(centres: &[f32], pointer: f32) -> usize {
    centres.iter().enumerate().min_by(|a, b| (a.1 - pointer).abs().total_cmp(&(b.1 - pointer).abs())).map(|(i, _)| i).unwrap_or(0)
}

/// Moves `from` to index `to`.
pub fn reorder<T>(list: &mut Vec<T>, from: usize, to: usize) {
    if from < list.len() && to < list.len() && from != to {
        let v = list.remove(from);
        list.insert(to, v);
    }
}

// ── Curves ───────────────────────────────────────────────────────────────

/// Magnification of an icon whose centre is `dist` icon-sizes from the
/// pointer: `1 + (peak - 1) · e^(-(d/spread)²/2σ²)`.
pub fn magnification(dist: f32, peak: f32, spread: f32) -> f32 {
    let d = dist / spread.max(0.1);
    1.0 + (peak - 1.0).max(0.0) * (-(d * d) / (2.0 * MAGNIFY_SIGMA * MAGNIFY_SIGMA)).exp()
}

/// Height (0–1 of the hop) of a launch bounce `t` seconds in: parabolic
/// hops, like an app starting on macOS.
pub fn launch_hop(t: f32) -> f32 {
    let x = (t / LAUNCH_HOP_S).fract();
    4.0 * x * (1.0 - x)
}

/// Height (0–1) of an attention bounce `t` seconds in: a burst of
/// [`ATTENTION_HOPS`] hops, a rest, repeat.
pub fn attention_hop(t: f32) -> f32 {
    let burst = ATTENTION_HOPS * LAUNCH_HOP_S;
    let x = t % (burst + ATTENTION_REST_S);
    if x < burst { launch_hop(x) } else { 0.0 }
}

// ── Geometry ─────────────────────────────────────────────────────────────

/// Whether rect `a` overlaps rect `b` (`x, y, w, h`).
pub fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Maps dock space (`along` the edge, `cross` = distance from the outer
/// edge inward, after `margin`) to surface coordinates for a box of
/// `len × depth`.
pub fn place(pos: DockPosition, surface: (f32, f32), margin: f32, along: f32, cross: f32, len: f32, depth: f32) -> (f32, f32, f32, f32) {
    match pos {
        DockPosition::Bottom => (along, surface.1 - margin - cross - depth, len, depth),
        DockPosition::Left => (margin + cross, along, depth, len),
        DockPosition::Right => (surface.0 - margin - cross - depth, along, depth, len),
    }
}

/// The input region of a hidden dock: a strip `thickness` px deep flush
/// with the screen edge, along the dock's resting span. (It used to be
/// pushed past the surface's edge by the margin, so a hidden dock could
/// never be revealed.)
pub fn reveal_strip(pos: DockPosition, surface: (f32, f32), along: f32, len: f32, thickness: f32) -> (f32, f32, f32, f32) {
    place(pos, surface, 0.0, along, 0.0, len, thickness)
}

/// Distance of `point` (surface coords) from the dock's screen edge.
pub fn edge_distance(pos: DockPosition, surface: (f32, f32), point: (f32, f32)) -> f32 {
    match pos {
        DockPosition::Bottom => surface.1 - point.1,
        DockPosition::Left => point.0,
        DockPosition::Right => surface.0 - point.0,
    }
}

/// Along-the-edge coordinate of `point`.
pub fn along(pos: DockPosition, point: (f32, f32)) -> f32 {
    if pos == DockPosition::Bottom { point.0 } else { point.1 }
}

// ── Visibility ───────────────────────────────────────────────────────────

/// Why the dock is out (shown) right now.
#[derive(Copy, Clone, Debug, Default)]
pub struct Visibility {
    /// `dock show`, just switched on, a drag, the launcher above it…
    pub forced: bool,
    /// The pointer revealed it at the edge.
    pub revealed: bool,
    /// A window overlaps it or one is fullscreen (smart hide).
    pub obstructed: bool,
}

/// Whether the dock is shown under `auto_hide`.
pub fn visible(auto_hide: mandara_config::workflow::DockAutoHide, v: Visibility) -> bool {
    use mandara_config::workflow::DockAutoHide;
    v.forced
        || v.revealed
        || match auto_hide {
            DockAutoHide::Never => true,
            DockAutoHide::Always => false,
            DockAutoHide::Smart => !v.obstructed,
        }
}

/// A window that can hide the dock (smart hide): never one of the shell's
/// own surfaces (its popovers, menus and grids are layer surfaces, but a
/// compositor listing them as clients must not make the dock hide under
/// its own stack).
pub fn obstructs(app_id: &str, geometry: Option<(i32, i32, i32, i32)>, dock: (f32, f32, f32, f32)) -> bool {
    !crate::windows::is_shell(app_id) && geometry.is_some_and(|(x, y, w, h)| overlaps(dock, (x as f32, y as f32, w as f32, h as f32)))
}

/// The pointer side of hiding: whether the pointer revealed the dock,
/// with the show / hide delays as pending timers, and whether a dock
/// popover (stack, menu, Applications grid, app picker) pins it out.
///
/// While a popover is open the pointer sits on the popover's surface, not
/// the dock's, so the dock must neither hide under it nor keep a stale
/// "pointer inside" state afterwards: [`Reveal::pin`] holds it out and
/// [`Reveal::unpin`] (the popover closed) restarts the hide rules as if
/// the pointer had just left — the next touch of the reveal strip always
/// shows it again.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Reveal {
    /// The pointer is over the dock (or its reveal strip).
    pub inside: bool,
    /// The pointer revealed it (it stays out until the hide delay passes).
    pub revealed: bool,
    /// A popover of this dock is open.
    pub pinned: bool,
    /// The show delay is running (the pointer entered a hidden dock).
    pub show_pending: bool,
    /// The hide delay is running (the pointer left).
    pub hide_pending: bool,
}

impl Reveal {
    /// The pointer entered (`true`) or left the dock. Returns whether
    /// anything changed.
    pub fn pointer(&mut self, inside: bool) -> bool {
        if inside == self.inside {
            return false;
        }
        self.inside = inside;
        if inside {
            self.hide_pending = false;
            self.show_pending = !self.revealed;
        } else {
            self.show_pending = false;
            self.hide_pending = true;
        }
        true
    }

    /// The show delay passed.
    pub fn show_elapsed(&mut self) {
        if std::mem::take(&mut self.show_pending) {
            self.revealed = self.inside;
        }
    }

    /// The hide delay passed.
    pub fn hide_elapsed(&mut self) {
        if std::mem::take(&mut self.hide_pending) && !self.inside && !self.pinned {
            self.revealed = false;
        }
    }

    /// A popover opened: out until it closes.
    pub fn pin(&mut self) {
        self.pinned = true;
        self.revealed = true;
        self.show_pending = false;
        self.hide_pending = false;
    }

    /// The popover closed. The pointer was on it, not on the dock, so the
    /// dock counts as just left: out for the hide delay, then the usual
    /// rules (a pointer that is really on the dock re-enters and keeps it).
    pub fn unpin(&mut self) {
        self.pinned = false;
        self.inside = false;
        self.revealed = true;
        self.show_pending = false;
        self.hide_pending = true;
    }

    /// Whether the dock is out under `auto_hide`.
    pub fn visible(&self, auto_hide: mandara_config::workflow::DockAutoHide, forced: bool, obstructed: bool) -> bool {
        visible(auto_hide, Visibility { forced: forced || self.pinned, revealed: self.revealed, obstructed })
    }
}

/// The docks to open and close: `outputs` (in order) wanted while the dock
/// is `enabled`, against the ones `open` now.
pub fn plan_surfaces(cfg: &mandara_config::ShellConfig, enabled: bool, outputs: &[String], open: &[String]) -> (Vec<String>, Vec<String>) {
    let wanted: Vec<String> = if enabled { outputs.iter().enumerate().filter(|(i, o)| cfg.dock_on_output(o, *i)).map(|(_, o)| o.clone()).collect() } else { Vec::new() };
    let to_open = wanted.iter().filter(|w| !open.contains(w)).cloned().collect();
    let to_close = open.iter().filter(|o| !wanted.contains(o)).cloned().collect();
    (to_open, to_close)
}

// ── Badges & attention ───────────────────────────────────────────────────

/// Whether notification `n` came from the app with desktop id `desktop`
/// (empty when unknown), display name `name` and window app ids `app_ids`.
pub fn notification_matches(n: &Notification, desktop: &str, name: &str, app_ids: &[&str]) -> bool {
    let entry = n.desktop_entry.trim().trim_end_matches(".desktop");
    let app = n.app_name.trim();
    if !entry.is_empty() && (entry.eq_ignore_ascii_case(desktop) || app_ids.iter().any(|a| same_app(a, entry)) || (!desktop.is_empty() && same_app(desktop, entry))) {
        return true;
    }
    !app.is_empty() && (app.eq_ignore_ascii_case(name) || app_ids.iter().any(|a| a.eq_ignore_ascii_case(app)) || (!desktop.is_empty() && same_app(desktop, app)))
}

/// An app's badge: unread notifications and the latest progress (0–100).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Badge {
    pub unread: usize,
    pub progress: Option<u8>,
    /// An unread critical notification (attention bounce).
    pub critical: bool,
}

pub fn badge(list: &[Notification], desktop: &str, name: &str, app_ids: &[&str]) -> Badge {
    let mut b = Badge::default();
    let mut latest = i64::MIN;
    for n in list.iter().filter(|n| notification_matches(n, desktop, name, app_ids)) {
        if !n.read && !n.transient {
            b.unread += 1;
            b.critical |= n.urgency == Urgency::Critical;
        }
        if let Some(p) = n.progress
            && n.timestamp >= latest
            && p < 100
        {
            latest = n.timestamp;
            b.progress = Some(p);
        }
    }
    b
}

/// The badge text for `n` unread (`99+` past 99).
pub fn badge_text(n: usize) -> String {
    if n > 99 { "99+".into() } else { n.to_string() }
}

/// Apps (window app ids) with a window on an urgent tag / workspace that
/// isn't focused.
pub fn urgent_apps(clients: &[mandara_compositor::ClientInfo], navigation: &std::collections::HashMap<String, mandara_compositor::NavigationModel>) -> Vec<String> {
    let urgent_mask = |monitor: &str| match navigation.get(monitor) {
        Some(mandara_compositor::NavigationModel::BitmaskTags { tags, .. }) => tags.iter().filter(|t| t.urgent).fold(0u32, |m, t| m | 1u32.checked_shl(t.id.saturating_sub(1)).unwrap_or(0)),
        _ => 0,
    };
    clients
        .iter()
        .filter(|c| !c.focused && c.tags.iter().any(|t| urgent_mask(&c.monitor) & 1u32.checked_shl(t.saturating_sub(1)).unwrap_or(0) != 0))
        .map(|c| c.app_id.clone())
        .collect()
}

// ── Recent apps ──────────────────────────────────────────────────────────

/// Most-recent-first desktop ids: apps that quit this session (newest
/// first), then the launcher's most used, without duplicates.
pub fn recent_ids(closed: &[String], usage: &[(String, f64)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let from_usage = usage.iter().filter_map(|(k, _)| k.strip_prefix("app:").map(str::to_string));
    for id in closed.iter().rev().cloned().chain(from_usage) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&id)) {
            out.push(id);
        }
    }
    out
}

/// Records that the apps in `before` but not in `now` quit (keeping the
/// newest `cap`).
pub fn note_closed(closed: &mut Vec<String>, before: &[String], now: &[String], cap: usize) {
    for id in before.iter().filter(|b| !now.contains(b)) {
        closed.retain(|c| c != id);
        closed.push(id.clone());
    }
    if closed.len() > cap {
        let drop = closed.len() - cap;
        closed.drain(..drop);
    }
}

// ── Opening files ────────────────────────────────────────────────────────

/// `path` quoted for an `Exec` line.
fn quote(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        if c == '%' {
            out.push('%');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// `exec` with dropped `files` filled in: `%f` / `%u` take the first,
/// `%F` / `%U` all of them (as `file://` URIs for the `%u`/`%U` codes);
/// an `Exec` line without a file code gets them appended.
pub fn exec_with_files(exec: &str, files: &[std::path::PathBuf]) -> String {
    let uri = |p: &std::path::PathBuf| quote(std::path::Path::new(&format!("file://{}", mandara_services::app_uninstall::percent_encode(p))));
    let mut used = false;
    let mut out = Vec::new();
    for word in exec.split_whitespace() {
        let filled = match word {
            "%f" => files.first().map(|f| quote(f)),
            "%u" => files.first().map(uri),
            "%F" => Some(files.iter().map(|f| quote(f)).collect::<Vec<_>>().join(" ")),
            "%U" => Some(files.iter().map(uri).collect::<Vec<_>>().join(" ")),
            _ => None,
        };
        match filled {
            Some(f) => {
                used = true;
                if !f.is_empty() {
                    out.push(f);
                }
            }
            None => out.push(word.to_string()),
        }
    }
    if !used {
        out.extend(files.iter().map(|f| quote(f)));
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::workflow::DockAutoHide;

    fn win(id: u64, app: &str) -> Toplevel {
        Toplevel { id, app_id: app.into(), title: format!("{app} {id}"), ..Default::default() }
    }

    fn app(id: &str, class: Option<&str>) -> DesktopApp {
        DesktopApp { id: id.into(), name: id.into(), wm_class: class.map(str::to_string), ..Default::default() }
    }

    fn inputs<'a>(cfg: &'a DockConfig, windows: &'a [Toplevel], apps: &'a [DesktopApp]) -> Inputs<'a> {
        Inputs { cfg, windows, apps, monitor: "", linger: &[], recent: &[], installing: &[], media: false }
    }

    fn keys(list: &[Item]) -> Vec<&str> {
        list.iter().map(|i| i.key.as_str()).collect()
    }

    fn bare() -> DockConfig {
        DockConfig { pinned: Vec::new(), stacks: Vec::new(), trash: false, show_recent: false, applications: mandara_config::dock::DockApplications { enabled: false, ..Default::default() }, ..DockConfig::default() }
    }

    #[test]
    fn items_follow_the_macos_order() {
        let cfg = DockConfig { pinned: vec!["org.gnome.Nautilus".into(), "firefox".into()], launcher_button: true, trash: true, ..bare() };
        let apps = vec![app("org.gnome.Nautilus", None), app("firefox", None), app("org.demo.alpha", None)];
        let windows = vec![win(5, "nautilus"), win(3, "zed"), win(7, "zed"), win(1, mandara_core::SHELL_APP_ID)];
        let list = items(&inputs(&cfg, &windows, &apps));
        assert_eq!(keys(&list), ["@launcher", "org.gnome.Nautilus", "firefox", "zed", "@separator-right", "@trash"]);
        assert_eq!(list[1].windows.len(), 1, "nautilus window grouped under its pinned entry");
        assert_eq!(list[3].windows.iter().map(|w| w.id).collect::<Vec<_>>(), [3, 7]);
        let capped = items(&inputs(&DockConfig { max_apps: 2, ..cfg.clone() }, &windows, &apps));
        assert_eq!(capped.iter().filter(|i| i.is_app()).count(), 2);

        // Recent apps sit between the running apps and the right side.
        let recent = vec!["firefox".to_string(), "org.demo.alpha".into(), "missing".into()];
        let cfg = DockConfig { show_recent: true, recent_count: 3, stacks: vec![Default::default()], ..cfg };
        let list = items(&Inputs { recent: &recent, ..inputs(&cfg, &windows, &apps) });
        assert_eq!(keys(&list), ["@launcher", "org.gnome.Nautilus", "firefox", "zed", "@separator-recent", "org.demo.alpha", "@separator-right", "@stack-0", "@trash"]);
        assert!(list[5].recent && !list[5].pinned, "pinned apps and uninstalled ids never show as recent");
    }

    #[test]
    fn unpinned_running_apps_linger_until_they_quit() {
        let cfg = DockConfig { show_running: false, launcher_button: false, ..bare() };
        let apps = vec![app("org.demo.beta", None)];
        let windows = vec![win(2, "org.demo.beta")];
        assert!(items(&inputs(&cfg, &windows, &apps)).is_empty(), "running apps hidden");
        let linger = vec!["org.demo.beta".to_string()];
        let list = items(&Inputs { linger: &linger, ..inputs(&cfg, &windows, &apps) });
        assert_eq!(keys(&list), ["org.demo.beta"], "just unpinned: still there while it runs");
        assert!(items(&Inputs { linger: &linger, ..inputs(&cfg, &[], &apps) }).is_empty(), "gone once it quits");
    }

    #[test]
    fn minimized_windows_move_right_unless_they_minimize_into_the_icon() {
        let mut minimized = win(4, "org.demo.gamma");
        minimized.minimized = true;
        let windows = vec![minimized, win(6, "org.demo.gamma")];
        let apps = vec![app("org.demo.gamma", None)];
        let cfg = DockConfig { launcher_button: false, minimize_to_app: false, trash: true, ..bare() };
        let list = items(&inputs(&cfg, &windows, &apps));
        assert_eq!(keys(&list), ["org.demo.gamma", "@separator-right", "@min-4", "@trash"]);
        assert_eq!(list[0].windows.len(), 2, "the app keeps counting it");
        let into_icon = items(&inputs(&DockConfig { minimize_to_app: true, ..cfg }, &windows, &apps));
        assert!(!into_icon.iter().any(|i| matches!(i.kind, Kind::Minimized(_))));
    }

    #[test]
    fn installs_show_after_the_apps() {
        let cfg = DockConfig { launcher_button: false, pinned: vec!["a".into()], ..bare() };
        let jobs = vec![InstallSlot { id: 9, name: "Thing".into(), icon: String::new(), progress: 0.4, failed: None }];
        let list = items(&Inputs { installing: &jobs, ..inputs(&cfg, &[], &[]) });
        assert_eq!(keys(&list), ["a", "@install-9"]);
        assert_eq!(list[1].kind, Kind::Installing(9));
    }

    #[test]
    fn magnification_peaks_under_the_pointer() {
        assert!((magnification(0.0, 1.6, 2.0) - 1.6).abs() < 1e-6);
        assert!(magnification(1.0, 1.6, 2.0) < 1.6 && magnification(1.0, 1.6, 2.0) > magnification(3.0, 1.6, 2.0));
        assert!((magnification(50.0, 1.6, 2.0) - 1.0).abs() < 1e-4);
        assert_eq!(magnification(0.0, 0.5, 2.0), 1.0, "never shrinks");
    }

    #[test]
    fn bounces_hop_and_rest() {
        assert_eq!(launch_hop(0.0), 0.0);
        assert!((launch_hop(LAUNCH_HOP_S / 2.0) - 1.0).abs() < 1e-5, "peak mid-hop");
        assert!(launch_hop(LAUNCH_HOP_S * 1.5) > 0.99, "keeps hopping");
        assert!(attention_hop(LAUNCH_HOP_S / 2.0) > 0.99);
        assert_eq!(attention_hop(ATTENTION_HOPS * LAUNCH_HOP_S + ATTENTION_REST_S / 2.0), 0.0, "rests between bursts");
    }

    #[test]
    fn drag_reorders_pinned() {
        assert_eq!(drop_index(&[10.0, 60.0, 110.0], 95.0), 2);
        assert_eq!(pin_drop_index(&[10.0, 60.0, 110.0], 70.0), 2);
        assert_eq!(pin_drop_index(&[10.0, 60.0], 500.0), 2);
        let mut v = vec!["a", "b", "c"];
        reorder(&mut v, 0, 2);
        assert_eq!(v, ["b", "c", "a"]);
        reorder(&mut v, 5, 0);
        assert_eq!(v, ["b", "c", "a"]);
    }

    #[test]
    fn placement_per_edge() {
        let s = (1000.0, 100.0);
        assert_eq!(place(DockPosition::Bottom, s, 8.0, 10.0, 0.0, 50.0, 40.0), (10.0, 52.0, 50.0, 40.0));
        assert_eq!(place(DockPosition::Left, (100.0, 800.0), 8.0, 10.0, 4.0, 50.0, 40.0), (12.0, 10.0, 40.0, 50.0));
        assert_eq!(place(DockPosition::Right, (100.0, 800.0), 8.0, 10.0, 0.0, 50.0, 40.0), (52.0, 10.0, 40.0, 50.0));
        assert!(overlaps((0.0, 0.0, 10.0, 10.0), (5.0, 5.0, 10.0, 10.0)) && !overlaps((0.0, 0.0, 10.0, 10.0), (10.0, 0.0, 5.0, 5.0)));
        assert_eq!(edge_distance(DockPosition::Bottom, s, (5.0, 70.0)), 30.0);
        assert_eq!(edge_distance(DockPosition::Right, (100.0, 800.0), (90.0, 5.0)), 10.0);
    }

    /// Regression: the hidden dock's reveal strip must lie inside its
    /// surface, flush with the screen edge, or the dock never comes back.
    #[test]
    fn the_reveal_strip_is_inside_the_surface_on_every_edge() {
        for pos in [DockPosition::Bottom, DockPosition::Left, DockPosition::Right] {
            let surface = if pos == DockPosition::Bottom { (2560.0, 126.0) } else { (126.0, 1440.0) };
            let (x, y, w, h) = reveal_strip(pos, surface, 400.0, 600.0, 3.0);
            assert!(x >= 0.0 && y >= 0.0 && x + w <= surface.0 && y + h <= surface.1, "{pos:?}: {:?}", (x, y, w, h));
            let edge = edge_distance(pos, surface, (x + w / 2.0, y + h / 2.0));
            assert!(edge <= 3.0, "{pos:?} strip is {edge} px from the edge");
        }
    }

    /// Regression for "Show the dock doesn't work at all": switching the
    /// dock on opens a surface on every assigned output, and a freshly
    /// opened dock is out even under auto-hide or with windows over it.
    #[test]
    fn enabling_the_dock_opens_its_surface_and_shows_it() {
        let mut cfg = mandara_config::ShellConfig::default();
        let outputs = vec!["HEADLESS-1".to_string(), "DP-2".to_string()];
        let registry = {
            let mut r = mandara_plugins::PluginRegistry::default();
            super::super::register(&mut r);
            r
        };
        assert!(!registry.is_enabled(super::super::ID, &cfg.plugins), "opt-in");
        assert_eq!(plan_surfaces(&cfg, false, &outputs, &[]), (vec![], vec![]));
        // Studio's switch, the Plugins page and `plugins enable dock` all
        // go through the lifecycle rules.
        mandara_plugins::lifecycle::set_enabled(&mut cfg, super::super::ID, "Dock", true, None).unwrap();
        let enabled = registry.is_enabled(super::super::ID, &cfg.plugins);
        assert!(enabled && cfg.dock_enabled());
        let (open, close) = plan_surfaces(&cfg, enabled, &outputs, &[]);
        assert_eq!(open, outputs, "a dock on every display");
        assert!(close.is_empty());
        // Even the user's old `disabled = ["dock"]` config comes back on.
        let mut off = mandara_config::ShellConfig::default();
        off.plugins.disabled.push("dock".into());
        mandara_plugins::lifecycle::set_enabled(&mut off, "dock", "Dock", true, None).unwrap();
        assert!(registry.is_enabled("dock", &off.plugins));
        // Just opened: forced out whatever the hiding mode.
        for mode in [DockAutoHide::Never, DockAutoHide::Always, DockAutoHide::Smart] {
            assert!(visible(mode, Visibility { forced: true, revealed: false, obstructed: true }), "{mode:?}");
        }
        assert!(!visible(DockAutoHide::Smart, Visibility { obstructed: true, ..Default::default() }));
        assert!(visible(DockAutoHide::Never, Visibility { obstructed: true, ..Default::default() }), "the default never hides");
        // Turning it off closes them; per-display assignment is honoured.
        mandara_plugins::lifecycle::set_enabled(&mut cfg, "dock", "Dock", false, None).unwrap();
        assert_eq!(plan_surfaces(&cfg, false, &outputs, &outputs).1, outputs);
        cfg.dock.displays = mandara_config::workflow::DockDisplays::Primary;
        assert_eq!(plan_surfaces(&cfg, true, &outputs, &[]).0, vec!["HEADLESS-1".to_string()]);
    }

    fn note(app: &str, entry: &str) -> Notification {
        Notification { app_name: app.into(), desktop_entry: entry.into(), ..Default::default() }
    }

    #[test]
    fn badges_match_by_desktop_entry_app_id_or_name() {
        let mut read = note("Beta Browser", "org.demo.beta");
        read.read = true;
        let mut critical = note("", "org.demo.beta.desktop");
        critical.urgency = Urgency::Critical;
        let mut progress = note("beta", "");
        progress.progress = Some(40);
        progress.timestamp = 5;
        let list = vec![note("Beta Browser", ""), read, critical, progress, note("Other", "org.other")];
        let b = badge(&list, "org.demo.beta", "Beta Browser", &["org.demo.beta"]);
        assert_eq!(b.unread, 3, "the read one and the other app's don't count");
        assert!(b.critical);
        assert_eq!(b.progress, Some(40));
        // Unknown desktop id: matched by the window's app id.
        assert_eq!(badge(&list, "", "x", &["beta"]).unread, 2, "the critical one (entry tail) and the progress one (app name)");
        assert_eq!(badge_text(7), "7");
        assert_eq!(badge_text(130), "99+");
    }

    #[test]
    fn urgent_tags_flag_their_apps() {
        use mandara_compositor::{ClientInfo, NavigationModel, TagStatus};
        let mut nav = std::collections::HashMap::new();
        nav.insert("M".to_string(), NavigationModel::BitmaskTags { tags: vec![TagStatus { id: 3, active: false, occupied: true, urgent: true, client_count: 1, layout: String::new() }], active_layout: String::new(), keymode: String::new() });
        let c = |app: &str, tag: u32, focused: bool| ClientInfo { app_id: app.into(), monitor: "M".into(), tags: vec![tag], focused, ..Default::default() };
        assert_eq!(urgent_apps(&[c("a", 3, false), c("b", 2, false), c("d", 3, true)], &nav), ["a"]);
    }

    #[test]
    fn recent_apps_newest_first() {
        let mut closed = vec![];
        note_closed(&mut closed, &["x".into(), "y".into()], &["y".into()], 4);
        note_closed(&mut closed, &["y".into()], &[], 4);
        assert_eq!(closed, ["x", "y"]);
        let usage = vec![("app:z".to_string(), 3.0), ("cmd:ls".into(), 2.0), ("app:x".into(), 1.0)];
        assert_eq!(recent_ids(&closed, &usage), ["y", "x", "z"]);
        let mut many: Vec<String> = Vec::new();
        for i in 0..6 {
            note_closed(&mut many, &[i.to_string()], &[], 3);
        }
        assert_eq!(many, ["3", "4", "5"]);
    }

    #[test]
    fn dropped_files_fill_the_exec_field_codes() {
        let files = vec![std::path::PathBuf::from("/tmp/a b.txt"), std::path::PathBuf::from("/tmp/c$.md")];
        assert_eq!(exec_with_files("gedit %U", &files), "gedit \"file:///tmp/a%%20b.txt\" \"file:///tmp/c%%24.md\"");
        assert_eq!(exec_with_files("mpv --player %f", &files), "mpv --player \"/tmp/a b.txt\"");
        assert_eq!(exec_with_files("app %F", &files), "app \"/tmp/a b.txt\" \"/tmp/c\\$.md\"");
        assert_eq!(exec_with_files("kitty", &files[..1]), "kitty \"/tmp/a b.txt\"");
        assert_eq!(crate::apps::parse_exec(&exec_with_files("app %F", &files)), ["app", "/tmp/a b.txt", "/tmp/c$.md"]);
        assert_eq!(crate::apps::parse_exec(&exec_with_files("gedit %U", &files)), ["gedit", "file:///tmp/a%20b.txt", "file:///tmp/c%24.md"], "a literal % survives the Exec unescaping");
    }

    #[test]
    fn a_stack_pins_the_dock_out_and_closing_it_resumes_hiding() {
        use mandara_config::workflow::DockAutoHide::{Always, Never, Smart};
        let mut r = Reveal::default();
        // Hidden (smart, a window over it): the pointer touches the edge.
        assert!(!r.visible(Smart, false, true));
        assert!(r.pointer(true) && r.show_pending);
        r.show_elapsed();
        assert!(r.visible(Smart, false, true), "revealed");
        for cycle in 0..3 {
            // Click the stack: its popover opens and the pointer moves onto it.
            r.pin();
            r.pointer(false);
            r.hide_elapsed();
            for mode in [Never, Always, Smart] {
                assert!(r.visible(mode, false, true), "cycle {cycle}: pinned out under {mode:?} while the stack is open");
            }
            // Click outside / Esc / open a file / click the icon again.
            r.unpin();
            assert!(!r.inside, "the pointer was on the popover");
            assert!(r.visible(Smart, false, true), "out for the hide delay");
            r.hide_elapsed();
            assert!(!r.visible(Smart, false, true) && !r.visible(Always, false, false), "cycle {cycle}: hides again");
            assert!(r.visible(Never, false, true) && r.visible(Smart, false, false), "never / unobstructed stay out");
            // The reveal strip always works afterwards.
            assert!(r.pointer(true) && r.show_pending, "cycle {cycle}: the strip schedules a reveal");
            r.show_elapsed();
            assert!(r.visible(Always, false, true), "cycle {cycle}: shown again");
        }
    }

    #[test]
    fn a_pointer_resting_on_the_dock_when_a_popover_closes_keeps_it() {
        let mut r = Reveal::default();
        r.pointer(true);
        r.show_elapsed();
        r.pin();
        r.unpin();
        // Back on the dock before the hide delay ends (its surface gets an
        // enter, then motion).
        assert!(r.pointer(true) && !r.hide_pending && !r.show_pending, "no hide, no second reveal delay");
        r.hide_elapsed();
        assert!(r.visible(mandara_config::workflow::DockAutoHide::Always, false, false));
        // A stale hide timer can't hide a pinned dock either.
        r.pointer(false);
        r.pin();
        r.hide_pending = true;
        r.hide_elapsed();
        assert!(r.revealed && r.visible(mandara_config::workflow::DockAutoHide::Always, false, false));
    }

    #[test]
    fn the_shells_own_surfaces_never_obstruct_the_dock() {
        let dock = (100.0, 1300.0, 600.0, 64.0);
        assert!(obstructs("kitty", Some((0, 0, 2560, 1440)), dock));
        assert!(!obstructs(mandara_core::SHELL_APP_ID, Some((0, 0, 2560, 1440)), dock), "a full-screen popover of the shell");
        assert!(!obstructs("kitty", Some((0, 0, 2560, 1200)), dock), "above the dock");
        assert!(!obstructs("kitty", None, dock));
    }
}
