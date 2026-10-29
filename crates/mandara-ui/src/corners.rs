//! Screen corners on the shell surface (no extra layer surfaces, so no
//! extra swapchains):
//!
//! * **Hot corners** (`[hot_corners]`): a tiny square in each corner of the
//!   output layout joins the shell surface's input region; resting the
//!   pointer there for `delay_ms` runs the corner's action (launcher,
//!   overview, sidebar, session dock, notifications, wallpapers, lock or a
//!   command). The rules live in [`CornerGate`], a pure state machine:
//!   - leaving before the delay, or pressing a button (a drag or a click
//!     aimed at something in the corner) cancels;
//!   - a fired corner re-arms only after the pointer really leaves it. The
//!     launcher, power menu, wallpaper picker or lock screen covering the
//!     corner is not "leaving": closing them with the pointer still in the
//!     corner never fires it again;
//!   - nothing fires while a fullscreen window is shown on that display,
//!     unless `over_fullscreen` is on;
//!   - only corners at the edge of the whole output layout are hot (the
//!     pointer slides through a corner shared with a neighbouring display).
//!
//!   While any corner is active, the compositor's own hot corner is
//!   switched off in the drop-in (`ShellConfig::effective_compositor`), so
//!   the two never both toggle the overview.
//! * **Rounded screen corners** (`[screen_corners]`): black concave
//!   quarter-circles drawn in each corner, outside the input region so
//!   they never take clicks.
//!
//! `mandara msg hotcorners status` reports each corner's state and the
//! recent fires; `hotcorners trigger <corner>` runs a corner's action.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, App, Context, Global, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Task, div, px};
use mandara_config::ShellConfig;
use mandara_config::desktop::{CornerAction, ScreenCorner};
use mandara_core::{FlareCenter, InputMaskRegistry, concave_flare};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use serde_json::json;

/// Hot corner hit square bounds, logical px.
const MIN_HOT_SIZE_PX: f32 = 1.0;
const MAX_HOT_SIZE_PX: f32 = 24.0;
/// Element-id base of the hot corner triggers.
const HOT_ID: &str = "hot-corner";
/// After the overlay a corner opened closes, an entry this soon means the
/// pointer never left (compositors re-enter the surface under a still
/// pointer), so the corner stays spent.
const REENTER_GRACE: Duration = Duration::from_millis(250);
/// Timers may wake a hair early; a dwell this close to the delay counts.
const DUE_SLACK: Duration = Duration::from_millis(4);
/// Fires kept for `hotcorners status`.
const FIRE_LOG_CAP: usize = 32;
/// Set to `1` and a hot corner's Lock action is only recorded, never run
/// (headless tests; every other action runs as usual).
pub const DRY_RUN_ENV: &str = "MANDARA_HOT_CORNERS_DRY_RUN";

// ── The gate: pure dwell / re-arm rules ──────────────────────────────────

/// Where one corner stands.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the pointer.
    #[default]
    Idle,
    /// The pointer rests in the corner; fires when the delay is up.
    Dwelling,
    /// Fired; spent until the pointer leaves.
    Fired,
    /// Fired, then an overlay (launcher, power menu, picker, lock screen)
    /// took the pointer while it stayed in the corner.
    Covered,
    /// A button is held (a drag or a click): never fires until it leaves.
    Blocked,
}

impl Phase {
    pub fn id(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Dwelling => "dwelling",
            Phase::Fired => "fired",
            Phase::Covered => "covered",
            Phase::Blocked => "blocked",
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct Slot {
    phase: Phase,
    /// Dwell generation: a timer only fires the dwell it was started for.
    token: u64,
    since: Option<Instant>,
    /// Covered: until when a re-entry still means "never left"
    /// (`None` while the overlay is open).
    reenter_until: Option<Instant>,
}

/// Dwell, cancel and re-arm rules of the four hot corners of one display.
#[derive(Clone, Debug, Default)]
pub struct CornerGate {
    slots: [Slot; 4],
    next_token: u64,
}

impl CornerGate {
    pub fn phase(&self, corner: ScreenCorner) -> Phase {
        self.slots[index(corner)].phase
    }

    fn slot(&mut self, corner: ScreenCorner) -> &mut Slot {
        &mut self.slots[index(corner)]
    }

    /// A covered corner whose re-entry window passed has been left.
    fn settle(&mut self, corner: ScreenCorner, now: Instant) {
        let s = self.slot(corner);
        if s.phase == Phase::Covered && s.reenter_until.is_some_and(|t| now > t) {
            *s = Slot { token: s.token, ..Slot::default() };
        }
    }

    /// The pointer entered `corner`. `pressed`: a button is held. `allowed`:
    /// the corner has an action and may fire now (on, not suppressed by a
    /// fullscreen window). Returns the dwell token to pass to [`Self::due`]
    /// once the delay is up.
    pub fn enter(&mut self, corner: ScreenCorner, pressed: bool, allowed: bool, now: Instant) -> Option<u64> {
        self.settle(corner, now);
        let next = self.next_token + 1;
        let s = self.slot(corner);
        match s.phase {
            Phase::Idle if pressed => {
                s.phase = Phase::Blocked;
                None
            }
            Phase::Idle if allowed => {
                *s = Slot { phase: Phase::Dwelling, token: next, since: Some(now), reenter_until: None };
                self.next_token = next;
                Some(next)
            }
            // Back under a still pointer after the overlay closed.
            Phase::Covered => {
                s.phase = Phase::Fired;
                s.reenter_until = None;
                None
            }
            _ => None,
        }
    }

    /// The pointer left `corner`. `covered`: one of Mandara's full-screen
    /// overlays is open (it took the pointer; the pointer may not have
    /// moved).
    pub fn leave(&mut self, corner: ScreenCorner, covered: bool, now: Instant) {
        self.settle(corner, now);
        let s = self.slot(corner);
        match s.phase {
            Phase::Dwelling | Phase::Blocked => *s = Slot { token: s.token, ..Slot::default() },
            Phase::Fired if covered => {
                s.phase = Phase::Covered;
                s.reenter_until = None;
            }
            Phase::Fired => *s = Slot { token: s.token, ..Slot::default() },
            Phase::Idle | Phase::Covered => {}
        }
    }

    /// A button went down (or is held) in `corner`: a drag or a click, not
    /// a request for the corner's action.
    pub fn press(&mut self, corner: ScreenCorner) {
        let s = self.slot(corner);
        if matches!(s.phase, Phase::Idle | Phase::Dwelling) {
            s.phase = Phase::Blocked;
            s.since = None;
        }
    }

    /// The dwell started with `token` is up. Returns whether the corner
    /// fires now (it then counts as [`Phase::Fired`]).
    pub fn due(&mut self, corner: ScreenCorner, token: u64, delay: Duration, now: Instant) -> bool {
        let s = self.slot(corner);
        let ripe = s.since.is_some_and(|since| now + DUE_SLACK >= since + delay);
        if s.phase == Phase::Dwelling && s.token == token && ripe {
            s.phase = Phase::Fired;
            s.since = None;
            return true;
        }
        false
    }

    /// The dwell can't complete (the corner was switched off meanwhile).
    pub fn cancel(&mut self, corner: ScreenCorner) {
        let s = self.slot(corner);
        if s.phase == Phase::Dwelling {
            *s = Slot { token: s.token, ..Slot::default() };
        }
    }

    /// Every overlay is closed again: covered corners get
    /// [`REENTER_GRACE`] to see the pointer come back.
    pub fn uncover(&mut self, now: Instant) {
        for s in &mut self.slots {
            if s.phase == Phase::Covered && s.reenter_until.is_none() {
                s.reenter_until = Some(now + REENTER_GRACE);
            }
        }
    }
}

// ── Geometry ─────────────────────────────────────────────────────────────

/// Top-left origin of a `size`² square in `corner` of a `w × h` surface.
pub fn corner_origin(corner: ScreenCorner, size: f32, (w, h): (f32, f32)) -> (f32, f32) {
    match corner {
        ScreenCorner::TopLeft => (0.0, 0.0),
        ScreenCorner::TopRight => (w - size, 0.0),
        ScreenCorner::BottomLeft => (0.0, h - size),
        ScreenCorner::BottomRight => (w - size, h - size),
    }
}

/// Output rectangle in the layout `(x, y, w, h)`, logical px.
pub type LayoutRect = (f32, f32, f32, f32);

fn contains(r: &LayoutRect, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// Whether `corner` of output `this` is a corner of the whole layout: the
/// pointer can go neither further out horizontally nor vertically there.
/// A corner shared with a neighbouring display is a pass-through, not a
/// target.
pub fn corner_is_bound(corner: ScreenCorner, this: LayoutRect, others: &[LayoutRect]) -> bool {
    let (x, y, w, h) = this;
    // A point just outside the corner along each axis (half a pixel out).
    const OUT: f32 = 0.5;
    let px_in = if corner.is_right() { x + w - OUT } else { x + OUT };
    let py_in = if corner.is_bottom() { y + h - OUT } else { y + OUT };
    let px_out = if corner.is_right() { x + w + OUT } else { x - OUT };
    let py_out = if corner.is_bottom() { y + h + OUT } else { y - OUT };
    !others.iter().any(|o| contains(o, px_out, py_in) || contains(o, px_in, py_out))
}

/// The flare whose filled part sits in `corner` (hollow toward the screen).
fn flare_center(corner: ScreenCorner) -> FlareCenter {
    match corner {
        ScreenCorner::TopLeft => FlareCenter::BottomRight,
        ScreenCorner::TopRight => FlareCenter::BottomLeft,
        ScreenCorner::BottomLeft => FlareCenter::TopRight,
        ScreenCorner::BottomRight => FlareCenter::TopLeft,
    }
}

fn index(corner: ScreenCorner) -> usize {
    ScreenCorner::ALL.iter().position(|c| *c == corner).unwrap_or(0)
}

/// Whether `corner` may fire under `cfg` while a fullscreen window is (or
/// isn't) shown on its display.
pub fn corner_allowed(cfg: &ShellConfig, corner: ScreenCorner, fullscreen: bool) -> bool {
    cfg.hot_corners.enabled && cfg.hot_corners.corner(corner).is_active() && (cfg.hot_corners.over_fullscreen || !fullscreen)
}

/// Hit square size for `cfg`, clamped to sane bounds.
pub fn hot_size(cfg: &ShellConfig) -> f32 {
    cfg.hot_corners.size.clamp(MIN_HOT_SIZE_PX, MAX_HOT_SIZE_PX)
}

// ── Actions ──────────────────────────────────────────────────────────────

/// One fire, for `hotcorners status`.
#[derive(Clone, Debug)]
struct FireRecord {
    corner: ScreenCorner,
    action: CornerAction,
    monitor: String,
    at_ms: u128,
    dry_run: bool,
}

#[derive(Default)]
struct FireLog {
    recent: VecDeque<FireRecord>,
    total: u64,
}
impl Global for FireLog {}

fn dry_run() -> bool {
    std::env::var(DRY_RUN_ENV).is_ok_and(|v| v == "1")
}

/// Whether one of Mandara's full-screen overlays covers the corners.
fn covering(cx: &App) -> bool {
    let s = crate::state::shell(cx).read(cx);
    s.launcher_open || s.session_open || s.wallpaper_open || crate::lock::is_locked()
}

/// Runs a corner's action (`monitor` is recorded in the fire log).
pub fn fire(corner: ScreenCorner, action: CornerAction, command: &str, monitor: &str, cx: &mut App) {
    let dry = dry_run() && action == CornerAction::Lock;
    tracing::info!(target: "mandara::hot_corners", "{} on {monitor}: {action:?}{}", corner.id(), if dry { " (dry run)" } else { "" });
    let log = cx.default_global::<FireLog>();
    log.total += 1;
    if log.recent.len() >= FIRE_LOG_CAP {
        log.recent.pop_front();
    }
    let at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    log.recent.push_back(FireRecord { corner, action, monitor: monitor.to_string(), at_ms, dry_run: dry });
    let shell = crate::state::shell(cx);
    match action {
        CornerAction::None => {}
        CornerAction::Launcher => shell.update(cx, |s, cx| s.set_launcher(true, cx)),
        CornerAction::Session => shell.update(cx, |s, cx| s.set_session(true, cx)),
        CornerAction::Wallpapers => shell.update(cx, |s, cx| s.set_wallpaper_picker(true, cx)),
        CornerAction::Sidebar => {
            let open = !shell.read(cx).sidebar_visible();
            shell.update(cx, |s, cx| s.set_sidebar(open, cx));
        }
        CornerAction::Notifications => shell.update(cx, |s, cx| s.toggle_panel(crate::plugins::notifications::ID, cx)),
        CornerAction::Lock if dry => {}
        CornerAction::Lock => {
            if let Err(err) = crate::lock::lock_now() {
                tracing::warn!("hot corner lock: {err}");
            }
        }
        CornerAction::Overview => {
            let Some(backend) = crate::hub::hub(cx).read(cx).compositor.clone() else { return };
            let (cmd, args): (&str, &[&str]) = match backend.kind() {
                mandara_compositor::CompositorKind::Niri => ("ToggleOverview", &[]),
                mandara_compositor::CompositorKind::MangoWM => ("toggleoverview", &[]),
                // Hyprland ships no overview; hyprexpo is the common plugin.
                mandara_compositor::CompositorKind::Hyprland => ("hyprexpo:expo", &["toggle"]),
            };
            if let Err(err) = backend.dispatch_raw(cmd, args) {
                tracing::warn!("hot corner overview: {err:#}");
            }
        }
        CornerAction::Command => {
            let command = command.trim();
            if command.is_empty() {
                return;
            }
            match std::process::Command::new("sh").arg("-c").arg(command).stdin(std::process::Stdio::null()).spawn() {
                // Reap in the background so no zombie is left behind.
                Ok(child) => mandara_services::reaper::adopt(child),
                Err(err) => tracing::warn!("hot corner command: {err}"),
            }
        }
    }
}

// ── The shell surface's corners ──────────────────────────────────────────

/// The hot corners of one shell surface (one display).
pub struct HotCorners {
    gate: CornerGate,
    monitor: String,
    dwell: [Option<Task<()>>; 4],
}

/// Gates of every display, for `hotcorners status`.
#[derive(Default)]
struct Gates(Vec<(String, CornerGate)>);
impl Global for Gates {}

impl HotCorners {
    /// Hot corners of the shell surface on `monitor`; `get` reaches this
    /// state inside the host view.
    pub fn new<V: 'static>(monitor: String, get: fn(&mut V) -> &mut HotCorners, cx: &mut Context<V>) -> Self {
        let shell = crate::state::shell(cx);
        cx.observe(&shell, move |this, _, cx| {
            if !covering(cx) {
                get(this).gate.uncover(Instant::now());
            }
        })
        .detach();
        Self { gate: CornerGate::default(), monitor, dwell: Default::default() }
    }

    fn publish(&self, cx: &mut App) {
        let gates = &mut cx.default_global::<Gates>().0;
        match gates.iter_mut().find(|(m, _)| *m == self.monitor) {
            Some(slot) => slot.1 = self.gate.clone(),
            None => gates.push((self.monitor.clone(), self.gate.clone())),
        }
    }

    fn fullscreen(&self, cx: &App) -> bool {
        crate::bar::autohide::coverage(crate::hub::hub(cx).read(cx), &self.monitor).1
    }

    fn on_hover<V: 'static>(&mut self, corner: ScreenCorner, hovered: bool, get: fn(&mut V) -> &mut HotCorners, cx: &mut Context<V>) {
        let now = Instant::now();
        if !hovered {
            let covered = covering(cx);
            self.gate.leave(corner, covered, now);
            self.dwell[index(corner)] = None;
            self.publish(cx);
            return;
        }
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let allowed = corner_allowed(&cfg, corner, self.fullscreen(cx));
        if let Some(token) = self.gate.enter(corner, false, allowed, now) {
            let delay = Duration::from_millis(cfg.hot_corners.delay_ms);
            self.dwell[index(corner)] = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, cx| get(this).on_due(corner, token, cx));
            }));
        }
        self.publish(cx);
    }

    fn on_press(&mut self, corner: ScreenCorner, cx: &mut App) {
        self.gate.press(corner);
        self.dwell[index(corner)] = None;
        self.publish(cx);
    }

    fn on_due<V: 'static>(&mut self, corner: ScreenCorner, token: u64, cx: &mut Context<V>) {
        let cfg = crate::state::config(cx).read(cx).snapshot();
        // Settings may have changed (or a window gone fullscreen) during
        // the dwell.
        if !corner_allowed(&cfg, corner, self.fullscreen(cx)) {
            self.gate.cancel(corner);
            self.publish(cx);
            return;
        }
        let delay = Duration::from_millis(cfg.hot_corners.delay_ms);
        if self.gate.due(corner, token, delay, Instant::now()) {
            let c = cfg.hot_corners.corner(corner).clone();
            self.publish(cx);
            fire(corner, c.action, &c.command, &self.monitor, cx);
        }
    }

    /// Hot corner triggers and screen-corner overlays for a shell surface of
    /// `surface` logical px. `get` reaches this state inside the view.
    pub fn elements<V: 'static>(
        &self,
        cfg: &ShellConfig,
        surface: (f32, f32),
        mask: &InputMaskRegistry,
        get: fn(&mut V) -> &mut HotCorners,
        cx: &mut Context<V>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if cfg.screen_corners.enabled && cfg.screen_corners.radius > 0.0 {
            let r = cfg.screen_corners.radius.min(surface.0.min(surface.1) / 2.0);
            let ink = gpui::black();
            for corner in ScreenCorner::ALL {
                let (x, y) = corner_origin(corner, r, surface);
                out.push(concave_flare(x, y, r, flare_center(corner), ink));
            }
        }
        if cfg.hot_corners.enabled {
            let size = hot_size(cfg);
            let layout: Vec<(String, LayoutRect)> = cx
                .try_global::<crate::overlays::Outputs>()
                .map(|o| o.0.iter().map(|g| (g.name.clone(), (g.x as f32, g.y as f32, g.width as f32, g.height as f32))).collect())
                .unwrap_or_default();
            let this = layout.iter().find(|(n, _)| *n == self.monitor).map(|(_, r)| *r);
            let others: Vec<LayoutRect> = layout.iter().filter(|(n, _)| *n != self.monitor).map(|(_, r)| *r).collect();
            for corner in ScreenCorner::ALL.into_iter().filter(|c| cfg.hot_corners.corner(*c).is_active()) {
                if this.is_some_and(|r| !corner_is_bound(corner, r, &others)) {
                    continue;
                }
                let (x, y) = corner_origin(corner, size, surface);
                out.push(
                    div()
                        .id((HOT_ID, index(corner)))
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .size(px(size))
                        // GPUI re-tests hover on every paint against the last
                        // known pointer position, which is (0, 0) before the
                        // pointer ever entered and goes stale once it left
                        // the surface: only a pointer that is on this surface
                        // counts (else the top-left corner fired at startup,
                        // and any corner fired again on the next repaint
                        // after the pointer left it).
                        .on_hover(cx.listener(move |this: &mut V, hovered: &bool, window, cx| {
                            if *hovered && !window.is_window_hovered() {
                                return;
                            }
                            get(this).on_hover(corner, *hovered, get, cx);
                        }))
                        // Real motion in the corner: the arrival GPUI's hover
                        // state can miss after a stale repaint. A drag through
                        // the corner (button held) or a click there is never
                        // a request for its action.
                        .on_mouse_move(cx.listener(move |this: &mut V, e: &gpui::MouseMoveEvent, window, cx| {
                            if e.pressed_button.is_some() {
                                get(this).on_press(corner, cx);
                            } else if window.is_window_hovered() {
                                get(this).on_hover(corner, true, get, cx);
                            }
                        }))
                        .on_any_mouse_down(cx.listener(move |this: &mut V, _: &gpui::MouseDownEvent, _, cx| get(this).on_press(corner, cx)))
                        .child(mask.region())
                        .into_any_element(),
                );
            }
        }
        out
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

fn corner_arg() -> ArgSpec {
    ArgSpec::required("corner", ArgKind::Enum, mandara_i18n::tk("which corner")).choices(&["top-left", "top-right", "bottom-left", "bottom-right"])
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        "core",
        CommandSpec::new("hotcorners status", mandara_i18n::tk("Hot corners: settings, each corner's state per display and the recent fires"), |_, cx| {
            let cfg = crate::state::config(cx).read(cx).snapshot();
            let hc = &cfg.hot_corners;
            let corners: serde_json::Map<String, serde_json::Value> = ScreenCorner::ALL
                .into_iter()
                .map(|c| {
                    let b = hc.corner(c);
                    (c.id().to_string(), json!({ "action": b.action, "command": b.command, "active": hc.enabled && b.is_active() }))
                })
                .collect();
            let displays: serde_json::Map<String, serde_json::Value> = cx
                .try_global::<Gates>()
                .map(|g| g.0.iter().map(|(m, gate)| (m.clone(), json!(ScreenCorner::ALL.iter().map(|c| (c.id(), gate.phase(*c).id())).collect::<std::collections::BTreeMap<_, _>>()))).collect())
                .unwrap_or_default();
            let log = cx.try_global::<FireLog>();
            let fired: Vec<_> = log
                .map(|l| l.recent.iter().map(|f| json!({ "corner": f.corner.id(), "action": f.action, "display": f.monitor, "at_ms": f.at_ms as u64, "dry_run": f.dry_run })).collect())
                .unwrap_or_default();
            Ok(json!({
                "enabled": hc.enabled,
                "delay_ms": hc.delay_ms,
                "size": hot_size(&cfg),
                "over_fullscreen": hc.over_fullscreen,
                "compositor_corner_off": hc.any_active(),
                "corners": corners,
                "displays": displays,
                "fire_count": log.map_or(0, |l| l.total),
                "fired": fired,
            }))
        })
        .example("hotcorners status"),
    );
    r.command(
        "core",
        CommandSpec::new("hotcorners trigger", mandara_i18n::tk("Run a hot corner's action now, as if the pointer rested there"), |a, cx| {
            let corner = ScreenCorner::parse(a.str(0)?).ok_or_else(|| mandara_i18n::t("Corners: top-left, top-right, bottom-left, bottom-right").to_string())?;
            let cfg = crate::state::config(cx).read(cx).snapshot();
            let c = cfg.hot_corners.corner(corner).clone();
            if !c.is_active() {
                return Err(mandara_i18n::tf("The {corner} corner has no action", &[("corner", &corner.id())]).to_string());
            }
            fire(corner, c.action, &c.command, "ipc", cx);
            Ok(json!({ "corner": corner.id(), "action": c.action }))
        })
        .arg(corner_arg())
        .example("hotcorners trigger top-left"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop::Corner;

    const DELAY: Duration = Duration::from_millis(120);
    const TL: ScreenCorner = ScreenCorner::TopLeft;
    const BR: ScreenCorner = ScreenCorner::BottomRight;

    fn ms(base: Instant, n: u64) -> Instant {
        base + Duration::from_millis(n)
    }

    #[test]
    fn corners_sit_in_the_screen_corners() {
        let s = (2560.0, 1440.0);
        assert_eq!(corner_origin(ScreenCorner::TopLeft, 3.0, s), (0.0, 0.0));
        assert_eq!(corner_origin(ScreenCorner::TopRight, 3.0, s), (2557.0, 0.0));
        assert_eq!(corner_origin(ScreenCorner::BottomLeft, 3.0, s), (0.0, 1437.0));
        assert_eq!(corner_origin(ScreenCorner::BottomRight, 3.0, s), (2557.0, 1437.0));
        assert_eq!(index(ScreenCorner::BottomRight), 3);
    }

    #[test]
    fn resting_for_the_delay_fires_once() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let token = g.enter(TL, false, true, t0).expect("dwell starts");
        assert_eq!(g.phase(TL), Phase::Dwelling);
        assert!(!g.due(TL, token, DELAY, ms(t0, 50)), "too early");
        assert!(g.due(TL, token, DELAY, ms(t0, 120)));
        assert_eq!(g.phase(TL), Phase::Fired);
        assert!(!g.due(TL, token, DELAY, ms(t0, 400)), "a timer never fires twice");
        // Still there: entering again (e.g. a synthetic enter) does nothing.
        assert_eq!(g.enter(TL, false, true, ms(t0, 500)), None);
    }

    #[test]
    fn leaving_early_cancels_and_a_stale_timer_is_ignored() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let first = g.enter(TL, false, true, t0).unwrap();
        g.leave(TL, false, ms(t0, 60));
        assert_eq!(g.phase(TL), Phase::Idle);
        let second = g.enter(TL, false, true, ms(t0, 100)).unwrap();
        assert_ne!(first, second);
        assert!(!g.due(TL, first, DELAY, ms(t0, 130)), "the first dwell's timer");
        assert!(!g.due(TL, second, DELAY, ms(t0, 130)), "second dwell not ripe yet");
        assert!(g.due(TL, second, DELAY, ms(t0, 220)));
    }

    #[test]
    fn a_fired_corner_rearms_after_the_pointer_leaves() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let t = g.enter(BR, false, true, t0).unwrap();
        assert!(g.due(BR, t, DELAY, ms(t0, 120)));
        g.leave(BR, false, ms(t0, 900));
        assert_eq!(g.phase(BR), Phase::Idle);
        let t = g.enter(BR, false, true, ms(t0, 2000)).expect("re-armed");
        assert!(g.due(BR, t, DELAY, ms(t0, 2120)));
    }

    #[test]
    fn closing_the_overlay_under_a_still_pointer_does_not_fire_again() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let t = g.enter(TL, false, true, t0).unwrap();
        assert!(g.due(TL, t, DELAY, ms(t0, 120)));
        // The launcher opens over the corner and takes the pointer.
        g.leave(TL, true, ms(t0, 130));
        assert_eq!(g.phase(TL), Phase::Covered);
        // Esc closes it seconds later; the compositor re-enters the shell
        // surface right away under the still pointer.
        g.uncover(ms(t0, 5000));
        assert_eq!(g.enter(TL, false, true, ms(t0, 5010)), None);
        assert_eq!(g.phase(TL), Phase::Fired);
        // Only a real leave re-arms it.
        g.leave(TL, false, ms(t0, 6000));
        assert!(g.enter(TL, false, true, ms(t0, 7000)).is_some());
    }

    #[test]
    fn a_pointer_that_moved_away_under_the_overlay_fires_on_return() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let t = g.enter(TL, false, true, t0).unwrap();
        assert!(g.due(TL, t, DELAY, ms(t0, 120)));
        g.leave(TL, true, ms(t0, 130));
        g.uncover(ms(t0, 3000));
        // No re-entry within the grace: the pointer is elsewhere (MangoWM
        // doesn't re-enter a still pointer at all).
        let t = g.enter(TL, false, true, ms(t0, 9000)).expect("fires on a real return");
        assert!(g.due(TL, t, DELAY, ms(t0, 9120)));
    }

    #[test]
    fn covered_stays_covered_while_the_overlay_is_open() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let t = g.enter(TL, false, true, t0).unwrap();
        assert!(g.due(TL, t, DELAY, ms(t0, 120)));
        g.leave(TL, true, ms(t0, 130));
        // Without `uncover`, no amount of time re-arms it.
        g.leave(TL, true, ms(t0, 60_000));
        assert_eq!(g.phase(TL), Phase::Covered);
    }

    #[test]
    fn drags_and_clicks_never_fire() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        // Arriving with a button held.
        assert_eq!(g.enter(TL, true, true, t0), None);
        assert_eq!(g.phase(TL), Phase::Blocked);
        g.leave(TL, false, ms(t0, 50));
        // Pressing during the dwell (a click aimed at the corner).
        let t = g.enter(TL, false, true, ms(t0, 100)).unwrap();
        g.press(TL);
        assert!(!g.due(TL, t, DELAY, ms(t0, 400)));
        assert_eq!(g.phase(TL), Phase::Blocked);
        // Releasing in place doesn't re-arm; leaving does.
        assert_eq!(g.enter(TL, false, true, ms(t0, 500)), None);
        g.leave(TL, false, ms(t0, 600));
        assert!(g.enter(TL, false, true, ms(t0, 700)).is_some());
    }

    #[test]
    fn a_disallowed_corner_never_dwells() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        assert_eq!(g.enter(TL, false, false, t0), None);
        assert_eq!(g.phase(TL), Phase::Idle);
        // Switched off during a dwell.
        let t = g.enter(TL, false, true, t0).unwrap();
        g.cancel(TL);
        assert!(!g.due(TL, t, DELAY, ms(t0, 500)));
        assert_eq!(g.phase(TL), Phase::Idle);
    }

    #[test]
    fn corners_are_independent() {
        let t0 = Instant::now();
        let mut g = CornerGate::default();
        let a = g.enter(TL, false, true, t0).unwrap();
        let b = g.enter(BR, false, true, t0).unwrap();
        g.leave(TL, false, ms(t0, 10));
        assert!(!g.due(TL, a, DELAY, ms(t0, 200)));
        assert!(g.due(BR, b, DELAY, ms(t0, 200)));
    }

    #[test]
    fn fullscreen_and_settings_gate_the_corner() {
        let mut cfg = ShellConfig::default();
        assert!(!corner_allowed(&cfg, TL, false), "off by default");
        cfg.hot_corners.enabled = true;
        assert!(corner_allowed(&cfg, TL, false), "default top-left overview");
        assert!(!corner_allowed(&cfg, TL, true), "not over fullscreen windows");
        cfg.hot_corners.over_fullscreen = true;
        assert!(corner_allowed(&cfg, TL, true));
        assert!(!corner_allowed(&cfg, BR, false), "no action");
        cfg.hot_corners.bottom_right = Corner { action: CornerAction::Command, command: String::new() };
        assert!(!corner_allowed(&cfg, BR, false), "a command corner needs a command");
        cfg.hot_corners.bottom_right.command = "true".into();
        assert!(corner_allowed(&cfg, BR, false));
        cfg.hot_corners.size = 0.0;
        assert_eq!(hot_size(&cfg), MIN_HOT_SIZE_PX);
        cfg.hot_corners.size = 500.0;
        assert_eq!(hot_size(&cfg), MAX_HOT_SIZE_PX);
    }

    #[test]
    fn only_outer_corners_of_the_layout_are_hot() {
        let left: LayoutRect = (0.0, 0.0, 2560.0, 1440.0);
        let right: LayoutRect = (2560.0, 0.0, 1920.0, 1080.0);
        assert!(ScreenCorner::ALL.iter().all(|c| corner_is_bound(*c, left, &[])), "a single display");
        // Left display: its right corners touch the right display…
        assert!(!corner_is_bound(ScreenCorner::TopRight, left, &[right]));
        // …but its bottom-right is below the shorter right display.
        assert!(corner_is_bound(ScreenCorner::BottomRight, left, &[right]));
        assert!(corner_is_bound(ScreenCorner::TopLeft, left, &[right]));
        // Right display: its left corners touch the left one.
        assert!(!corner_is_bound(ScreenCorner::TopLeft, right, &[left]));
        assert!(!corner_is_bound(ScreenCorner::BottomLeft, right, &[left]), "1080 < 1440: still beside the left display");
        assert!(corner_is_bound(ScreenCorner::TopRight, right, &[left]));
        // Stacked displays.
        let below: LayoutRect = (0.0, 1440.0, 2560.0, 1440.0);
        assert!(!corner_is_bound(ScreenCorner::BottomLeft, left, &[below]));
        assert!(!corner_is_bound(ScreenCorner::TopRight, below, &[left]));
    }
}
