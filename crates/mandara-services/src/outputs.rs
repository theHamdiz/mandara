//! Display configuration over `wlr-output-management-unstable-v1`
//! (MangoWM, Hyprland, Niri and Sway all implement it): list heads with
//! their modes, test or apply a configuration, and follow hotplug.
//!
//! Every call uses its own short-lived Wayland connection, so nothing runs
//! while no one asks; [`watch`] keeps one thread for profile auto-apply.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mandara_config::outputs::{OutputConfig, Transform, VrrMode, identity};
use serde::Serialize;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1 as cfg_head, zwlr_output_configuration_v1 as configuration, zwlr_output_head_v1 as head, zwlr_output_manager_v1 as manager,
    zwlr_output_mode_v1 as mode,
};

/// Highest protocol version used (v4: adaptive sync).
const MANAGER_VERSION: u32 = 4;
/// First version with `set_adaptive_sync`.
const ADAPTIVE_SYNC_SINCE: u32 = 4;
/// Two refresh rates closer than this are the same mode, millihertz.
const REFRESH_TOLERANCE_MHZ: i64 = 500;
/// Roundtrips to wait for the compositor's verdict before giving up.
const MAX_VERDICT_ROUNDTRIPS: usize = 50;

/// One display mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    /// Millihertz (0 = unknown).
    pub refresh_mhz: i32,
    pub preferred: bool,
}

/// One connected (or disconnected-but-listed) monitor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Head {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    /// Physical size in millimetres (0 for projectors / unknown).
    pub physical_mm: (i32, i32),
    pub enabled: bool,
    pub modes: Vec<Mode>,
    pub current_mode: Option<usize>,
    pub x: i32,
    pub y: i32,
    pub transform: Transform,
    pub scale: f64,
    /// `None` when the compositor doesn't report adaptive sync.
    pub adaptive_sync: Option<bool>,
}

impl Head {
    /// The stable identity used by display profiles.
    pub fn identity(&self) -> String {
        identity(&self.name, &self.make, &self.model, &self.serial)
    }

    pub fn current(&self) -> Option<&Mode> {
        self.current_mode.and_then(|i| self.modes.get(i))
    }

    /// Size in the global logical space (mode ÷ scale, rotated).
    pub fn logical_size(&self) -> (f64, f64) {
        let (w, h) = self.current().map(|m| (m.width as f64, m.height as f64)).unwrap_or_default();
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = (w / s, h / s);
        if self.transform.swaps_axes() { (h, w) } else { (w, h) }
    }

    /// The head's present state as a full [`OutputConfig`].
    pub fn to_config(&self) -> OutputConfig {
        let m = self.current();
        OutputConfig {
            name: self.name.clone(),
            make: self.make.clone(),
            model: self.model.clone(),
            serial: self.serial.clone(),
            enabled: self.enabled,
            width: m.map(|m| m.width.max(0) as u32),
            height: m.map(|m| m.height.max(0) as u32),
            refresh_mhz: m.map(|m| m.refresh_mhz.max(0) as u32).filter(|r| *r > 0),
            scale: Some(self.scale as f32),
            transform: Some(self.transform),
            x: Some(self.x),
            y: Some(self.y),
            vrr: self.adaptive_sync.map(|on| if on { VrrMode::On } else { VrrMode::Off }),
        }
    }

    /// Distinct resolutions, largest first.
    pub fn resolutions(&self) -> Vec<(i32, i32)> {
        let mut out: Vec<(i32, i32)> = Vec::new();
        for m in &self.modes {
            if !out.contains(&(m.width, m.height)) {
                out.push((m.width, m.height));
            }
        }
        out.sort_by_key(|(w, h)| std::cmp::Reverse((*w as i64) * (*h as i64)));
        out
    }

    /// Refresh rates offered at one resolution, fastest first (mHz).
    pub fn refresh_rates(&self, width: i32, height: i32) -> Vec<i32> {
        let mut out: Vec<i32> = self.modes.iter().filter(|m| m.width == width && m.height == height).map(|m| m.refresh_mhz).collect();
        out.sort_unstable_by(|a, b| b.cmp(a));
        out.dedup();
        out
    }
}

/// The identities of the connected heads (for profile matching).
pub fn identities(heads: &[Head]) -> BTreeSet<String> {
    heads.iter().map(Head::identity).collect()
}

/// What to send for one head.
#[derive(Clone, Debug, PartialEq)]
pub enum Planned {
    Disable,
    Enable {
        /// An advertised mode (index), or a custom mode.
        mode: PlannedMode,
        x: i32,
        y: i32,
        transform: Transform,
        scale: f64,
        adaptive_sync: Option<bool>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlannedMode {
    Listed(usize),
    Custom { width: i32, height: i32, refresh_mhz: i32 },
    /// Disabled head with no known mode: let the compositor pick.
    Preferred(Option<usize>),
}

/// The config entry for `head`: by connector name, else by identity.
fn config_for<'a>(head: &Head, configs: &'a [OutputConfig]) -> Option<&'a OutputConfig> {
    configs.iter().find(|c| c.name == head.name).or_else(|| configs.iter().find(|c| !c.serial.is_empty() && c.identity() == head.identity()))
}

/// The best mode index for a requested size / refresh.
fn pick_mode(head: &Head, width: i32, height: i32, refresh: Option<i32>) -> Option<usize> {
    let same: Vec<(usize, &Mode)> = head.modes.iter().enumerate().filter(|(_, m)| m.width == width && m.height == height).collect();
    match refresh {
        Some(r) => same.iter().min_by_key(|(_, m)| (m.refresh_mhz as i64 - r as i64).abs()).filter(|(_, m)| (m.refresh_mhz as i64 - r as i64).abs() <= REFRESH_TOLERANCE_MHZ).map(|(i, _)| *i),
        None => head.current_mode.filter(|i| same.iter().any(|(j, _)| j == i)).or_else(|| same.iter().max_by_key(|(_, m)| m.refresh_mhz).map(|(i, _)| *i)),
    }
}

/// Plans a configuration: heads without an entry keep their state; unset
/// fields keep the head's value. Refuses to disable every output.
pub fn plan(heads: &[Head], configs: &[OutputConfig]) -> Result<Vec<Planned>, String> {
    let mut out = Vec::with_capacity(heads.len());
    for h in heads {
        let c = config_for(h, configs);
        let enabled = c.map(|c| c.enabled).unwrap_or(h.enabled);
        if !enabled {
            out.push(Planned::Disable);
            continue;
        }
        let mode = match c.and_then(|c| Some((c.width?, c.height?, c.refresh_mhz))) {
            Some((w, hgt, r)) => match pick_mode(h, w as i32, hgt as i32, r.map(|r| r as i32)) {
                Some(i) => PlannedMode::Listed(i),
                None => PlannedMode::Custom { width: w as i32, height: hgt as i32, refresh_mhz: r.unwrap_or(0) as i32 },
            },
            None => match h.current_mode {
                Some(i) => PlannedMode::Listed(i),
                None => PlannedMode::Preferred(h.modes.iter().position(|m| m.preferred)),
            },
        };
        out.push(Planned::Enable {
            mode,
            x: c.and_then(|c| c.x).unwrap_or(h.x),
            y: c.and_then(|c| c.y).unwrap_or(h.y),
            transform: c.and_then(|c| c.transform).unwrap_or(h.transform),
            scale: c.and_then(|c| c.scale).filter(|s| *s > 0.0).map(f64::from).unwrap_or(if h.scale > 0.0 { h.scale } else { 1.0 }),
            adaptive_sync: c.and_then(|c| c.vrr).map(|v| v != VrrMode::Off).filter(|_| h.adaptive_sync.is_some()),
        });
    }
    if !out.iter().any(|p| matches!(p, Planned::Enable { .. })) {
        return Err(mandara_i18n::t("At least one display must stay on").into());
    }
    Ok(out)
}

// ── Wayland plumbing ────────────────────────────────────────────────────────

struct HeadState {
    proxy: head::ZwlrOutputHeadV1,
    info: Head,
    modes: Vec<mode::ZwlrOutputModeV1>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Default)]
struct State {
    heads: Vec<HeadState>,
    serial: Option<u32>,
    /// `done` events received.
    dones: u64,
    verdict: Option<Verdict>,
    finished: bool,
}

impl State {
    fn head_mut(&mut self, proxy: &head::ZwlrOutputHeadV1) -> Option<&mut HeadState> {
        self.heads.iter_mut().find(|h| &h.proxy == proxy)
    }

    fn snapshot(&self) -> Vec<Head> {
        self.heads.iter().map(|h| h.info.clone()).collect()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<manager::ZwlrOutputManagerV1, ()> for State {
    fn event(state: &mut Self, _: &manager::ZwlrOutputManagerV1, event: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            manager::Event::Head { head } => state.heads.push(HeadState {
                proxy: head,
                info: Head {
                    name: String::new(),
                    description: String::new(),
                    make: String::new(),
                    model: String::new(),
                    serial: String::new(),
                    physical_mm: (0, 0),
                    enabled: false,
                    modes: Vec::new(),
                    current_mode: None,
                    x: 0,
                    y: 0,
                    transform: Transform::Normal,
                    scale: 1.0,
                    adaptive_sync: None,
                },
                modes: Vec::new(),
            }),
            manager::Event::Done { serial } => {
                state.serial = Some(serial);
                state.dones += 1;
            }
            manager::Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, manager::ZwlrOutputManagerV1, [
        manager::EVT_HEAD_OPCODE => (head::ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<head::ZwlrOutputHeadV1, ()> for State {
    fn event(state: &mut Self, proxy: &head::ZwlrOutputHeadV1, event: head::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let head::Event::Finished = event {
            state.heads.retain(|h| &h.proxy != proxy);
            if proxy.version() >= 3 {
                proxy.release();
            }
            return;
        }
        let Some(h) = state.head_mut(proxy) else { return };
        let i = &mut h.info;
        match event {
            head::Event::Name { name } => i.name = name,
            head::Event::Description { description } => i.description = description,
            head::Event::PhysicalSize { width, height } => i.physical_mm = (width, height),
            head::Event::Mode { mode } => {
                h.modes.push(mode);
                i.modes.push(Mode { width: 0, height: 0, refresh_mhz: 0, preferred: false });
            }
            head::Event::Enabled { enabled } => i.enabled = enabled != 0,
            head::Event::CurrentMode { mode } => i.current_mode = h.modes.iter().position(|m| m == &mode),
            head::Event::Position { x, y } => (i.x, i.y) = (x, y),
            head::Event::Transform { transform } => {
                if let WEnum::Value(t) = transform {
                    i.transform = Transform::from_wl(t as u32);
                }
            }
            head::Event::Scale { scale } => i.scale = scale,
            head::Event::Make { make } => i.make = make,
            head::Event::Model { model } => i.model = model,
            head::Event::SerialNumber { serial_number } => i.serial = serial_number,
            head::Event::AdaptiveSync { state: s } => i.adaptive_sync = Some(matches!(s, WEnum::Value(head::AdaptiveSyncState::Enabled))),
            _ => {}
        }
    }

    event_created_child!(State, head::ZwlrOutputHeadV1, [
        head::EVT_MODE_OPCODE => (mode::ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<mode::ZwlrOutputModeV1, ()> for State {
    fn event(state: &mut Self, proxy: &mode::ZwlrOutputModeV1, event: mode::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        for h in &mut state.heads {
            let Some(idx) = h.modes.iter().position(|m| m == proxy) else { continue };
            let m = &mut h.info.modes[idx];
            match event {
                mode::Event::Size { width, height } => (m.width, m.height) = (width, height),
                mode::Event::Refresh { refresh } => m.refresh_mhz = refresh,
                mode::Event::Preferred => m.preferred = true,
                mode::Event::Finished => {
                    // Keep indices stable: a finished mode just stops being offered.
                    m.width = 0;
                    m.height = 0;
                    if proxy.version() >= 3 {
                        proxy.release();
                    }
                }
                _ => {}
            }
            return;
        }
    }
}

impl Dispatch<configuration::ZwlrOutputConfigurationV1, ()> for State {
    fn event(state: &mut Self, _: &configuration::ZwlrOutputConfigurationV1, event: configuration::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.verdict = Some(match event {
            configuration::Event::Succeeded => Verdict::Succeeded,
            configuration::Event::Failed => Verdict::Failed,
            _ => Verdict::Cancelled,
        });
    }
}

impl Dispatch<cfg_head::ZwlrOutputConfigurationHeadV1, ()> for State {
    fn event(_: &mut Self, _: &cfg_head::ZwlrOutputConfigurationHeadV1, _: cfg_head::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

struct Session {
    queue: wayland_client::EventQueue<State>,
    state: State,
    manager: manager::ZwlrOutputManagerV1,
}

fn unsupported() -> String {
    mandara_i18n::t("This compositor doesn't support display configuration (wlr-output-management)").into()
}

fn connect() -> Result<Session, String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: manager::ZwlrOutputManagerV1 = globals.bind(&qh, 1..=MANAGER_VERSION, ()).map_err(|_| unsupported())?;
    let mut state = State::default();
    while state.serial.is_none() && !state.finished {
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    }
    // Child objects' first events may land one roundtrip later.
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    Ok(Session { queue, state, manager })
}

/// Whether the compositor offers display configuration.
pub fn supported() -> bool {
    connect().is_ok()
}

/// Every head the compositor reports, with modes.
pub fn heads() -> Result<Vec<Head>, String> {
    let mut s = connect()?;
    let heads = s.state.snapshot();
    s.manager.stop();
    let _ = s.queue.roundtrip(&mut s.state);
    Ok(heads)
}

/// Tests (`test_only`) or applies `configs` on top of the current state.
/// Returns the state *before* the change, for reverting.
pub fn apply(configs: &[OutputConfig], test_only: bool) -> Result<Vec<Head>, String> {
    let mut s = connect()?;
    let before = s.state.snapshot();
    let planned = plan(&before, configs)?;
    let qh = s.queue.handle();
    let serial = s.state.serial.unwrap_or_default();
    let config = s.manager.create_configuration(serial, &qh, ());
    let version = s.manager.version();
    for (h, p) in s.state.heads.iter().zip(&planned) {
        match p {
            Planned::Disable => config.disable_head(&h.proxy),
            Planned::Enable { mode, x, y, transform, scale, adaptive_sync } => {
                let ch = config.enable_head(&h.proxy, &qh, ());
                match mode {
                    PlannedMode::Listed(i) | PlannedMode::Preferred(Some(i)) => {
                        if let Some(m) = h.modes.get(*i) {
                            ch.set_mode(m);
                        }
                    }
                    PlannedMode::Custom { width, height, refresh_mhz } => ch.set_custom_mode(*width, *height, *refresh_mhz),
                    PlannedMode::Preferred(None) => {}
                }
                ch.set_position(*x, *y);
                if let Ok(t) = wayland_client::protocol::wl_output::Transform::try_from(transform.wl()) {
                    ch.set_transform(t);
                }
                ch.set_scale(*scale);
                if let Some(on) = adaptive_sync
                    && version >= ADAPTIVE_SYNC_SINCE
                {
                    ch.set_adaptive_sync(if *on { head::AdaptiveSyncState::Enabled } else { head::AdaptiveSyncState::Disabled });
                }
            }
        }
    }
    if test_only {
        config.test();
    } else {
        config.apply();
    }
    for _ in 0..MAX_VERDICT_ROUNDTRIPS {
        if s.state.verdict.is_some() {
            break;
        }
        s.queue.blocking_dispatch(&mut s.state).map_err(|e| e.to_string())?;
    }
    config.destroy();
    s.manager.stop();
    let _ = s.queue.roundtrip(&mut s.state);
    match s.state.verdict {
        Some(Verdict::Succeeded) => Ok(before),
        Some(Verdict::Failed) => Err(mandara_i18n::t("The compositor rejected this display configuration").into()),
        Some(Verdict::Cancelled) => Err(mandara_i18n::t("The displays changed meanwhile; try again").into()),
        None => Err(mandara_i18n::t("The compositor did not answer").into()),
    }
}

// ── Test with automatic revert ──────────────────────────────────────────────

struct PendingRevert {
    /// What to restore.
    before: Vec<OutputConfig>,
    deadline: std::time::Instant,
    /// Bumped by every new test / confirm / revert, so a stale timer thread
    /// never restores over a newer change.
    generation: u64,
}

fn pending() -> &'static parking_lot::Mutex<Option<PendingRevert>> {
    static PENDING: std::sync::OnceLock<parking_lot::Mutex<Option<PendingRevert>>> = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Applies `configs` and restores the previous arrangement after `secs`
/// seconds unless [`confirm`] is called — so a mode the monitor can't show
/// never leaves the user stranded. A pending test's original state is kept
/// when tests are chained. Blocks for the apply; the countdown runs on its
/// own thread.
pub fn apply_with_revert(configs: &[OutputConfig], secs: u32) -> Result<u32, String> {
    let before = apply(configs, false)?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let wait = std::time::Duration::from_secs(secs as u64);
    {
        let mut p = pending().lock();
        let before = match p.take() {
            Some(old) => old.before,
            None => before.iter().map(Head::to_config).collect(),
        };
        *p = Some(PendingRevert { before, deadline: std::time::Instant::now() + wait, generation });
    }
    let _ = std::thread::Builder::new().name("mandara-display-revert".into()).spawn(move || {
        std::thread::sleep(wait);
        let restore = {
            let mut p = pending().lock();
            if p.as_ref().is_some_and(|r| r.generation == generation) { p.take().map(|r| r.before) } else { None }
        };
        if let Some(before) = restore {
            tracing::info!("displays: change not kept, reverting");
            if let Err(err) = apply(&before, false) {
                tracing::warn!("displays: revert failed: {err}");
            }
        }
    });
    Ok(secs)
}

/// Keeps a tested change. Returns whether one was pending.
pub fn confirm() -> bool {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    pending().lock().take().is_some()
}

/// Restores the arrangement from before a pending test right away.
pub fn revert_now() -> Result<bool, String> {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let Some(p) = pending().lock().take() else { return Ok(false) };
    apply(&p.before, false).map(|_| true)
}

/// Seconds left before a pending test reverts.
pub fn revert_remaining() -> Option<std::time::Duration> {
    pending().lock().as_ref().map(|p| p.deadline.saturating_duration_since(std::time::Instant::now()))
}

/// Follows hotplug: calls `on_change` with the heads after every change
/// (not for the initial state) until `stop` is set or the compositor goes
/// away. Runs on its own thread.
pub fn watch(stop: Arc<AtomicBool>, mut on_change: impl FnMut(Vec<Head>) + Send + 'static) {
    let _ = std::thread::Builder::new().name("mandara-outputs".into()).spawn(move || {
        let Ok(mut s) = connect() else { return };
        let mut seen = s.state.dones;
        let mut last = identities(&s.state.snapshot());
        while !stop.load(Ordering::Relaxed) && !s.state.finished {
            if s.queue.blocking_dispatch(&mut s.state).is_err() {
                return;
            }
            if s.state.dones != seen {
                seen = s.state.dones;
                let heads = s.state.snapshot();
                let ids = identities(&heads);
                // Only connection changes matter (mode tweaks also send `done`).
                if ids != last {
                    last = ids;
                    on_change(heads);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(name: &str) -> Head {
        Head {
            name: name.into(),
            description: String::new(),
            make: "Dell Inc.".into(),
            model: "U2720Q".into(),
            serial: format!("SN-{name}"),
            physical_mm: (600, 340),
            enabled: true,
            modes: vec![
                Mode { width: 3840, height: 2160, refresh_mhz: 60_000, preferred: true },
                Mode { width: 3840, height: 2160, refresh_mhz: 30_000, preferred: false },
                Mode { width: 1920, height: 1080, refresh_mhz: 60_000, preferred: false },
            ],
            current_mode: Some(0),
            x: 0,
            y: 0,
            transform: Transform::Normal,
            scale: 1.5,
            adaptive_sync: Some(false),
        }
    }

    #[test]
    fn logical_size_and_mode_lists() {
        let mut h = head("DP-1");
        assert_eq!(h.logical_size(), (2560.0, 1440.0));
        h.transform = Transform::Rotate90;
        assert_eq!(h.logical_size(), (1440.0, 2560.0));
        assert_eq!(h.resolutions(), vec![(3840, 2160), (1920, 1080)]);
        assert_eq!(h.refresh_rates(3840, 2160), vec![60_000, 30_000]);
        let c = h.to_config();
        assert_eq!((c.width, c.refresh_mhz, c.vrr), (Some(3840), Some(60_000), Some(VrrMode::Off)));
    }

    #[test]
    fn plan_keeps_unlisted_heads_and_picks_modes() {
        let heads = vec![head("DP-1"), head("eDP-1")];
        let change = OutputConfig { name: "DP-1".into(), width: Some(3840), height: Some(2160), refresh_mhz: Some(29_970), x: Some(1920), y: Some(0), vrr: Some(VrrMode::On), ..Default::default() };
        let p = plan(&heads, &[change]).unwrap();
        assert_eq!(p[0], Planned::Enable { mode: PlannedMode::Listed(1), x: 1920, y: 0, transform: Transform::Normal, scale: 1.5, adaptive_sync: Some(true) });
        assert_eq!(p[1], Planned::Enable { mode: PlannedMode::Listed(0), x: 0, y: 0, transform: Transform::Normal, scale: 1.5, adaptive_sync: None });
        // An unlisted mode becomes a custom one.
        let odd = OutputConfig { name: "DP-1".into(), width: Some(2560), height: Some(1080), refresh_mhz: Some(75_000), ..Default::default() };
        assert!(matches!(plan(&heads, &[odd]).unwrap()[0], Planned::Enable { mode: PlannedMode::Custom { width: 2560, .. }, .. }));
        // A monitor moved to another port is found by its serial.
        let moved = OutputConfig { name: "HDMI-A-1".into(), make: "Dell Inc.".into(), model: "U2720Q".into(), serial: "SN-DP-1".into(), enabled: false, ..Default::default() };
        assert_eq!(plan(&heads, &[moved]).unwrap()[0], Planned::Disable);
    }

    #[test]
    fn refuses_to_disable_every_output() {
        let heads = vec![head("DP-1")];
        let off = OutputConfig { name: "DP-1".into(), enabled: false, ..Default::default() };
        assert!(plan(&heads, &[off]).is_err());
    }
}
