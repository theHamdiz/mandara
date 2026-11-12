//! The dock's pure logic: which slots it shows and in what order, the
//! geometry helpers, magnification and bounce curves, badges, attention,
//! recent apps and opening files with an app. No GPUI state here, so all
//! of it is unit-tested.

use mandara_config::dock::{AttentionFeedback, DockReveal, IndicatorMotion, ItemMotion, LAUNCHER_PIN, LaunchFeedback, MinimizeFeedback};
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
    /// The launcher button: kept in `pinned` as [`LAUNCHER_PIN`], so it
    /// sits, moves and goes like a pinned app.
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

    /// Pinned slots (apps and the launcher button) are dragged to
    /// reorder, and away to remove; stacks reorder among themselves.
    pub fn movable(&self) -> bool {
        self.is_app() || matches!(self.kind, Kind::Launcher | Kind::Stack(_))
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

/// The dock's slots, macOS order: pinned apps (with their windows) and the
/// launcher button where it is pinned, other running apps, installs in
/// progress | recent apps | minimized windows, Applications, stacks,
/// trash. Separators sit between non-empty sections.
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
    for id in &cfg.pinned {
        if id == LAUNCHER_PIN {
            out.push(Item { pinned: true, ..Item::fixed(Kind::Launcher, LAUNCHER_PIN) });
            continue;
        }
        let entry = input.apps.iter().find(|a| a.id.eq_ignore_ascii_case(id)).cloned();
        let mut mine: Vec<Toplevel> = Vec::new();
        for (i, w) in windows.iter().enumerate() {
            if !used[i] && belongs(&w.app_id, id, entry.as_ref()) {
                used[i] = true;
                mine.push((*w).clone());
            }
        }
        if entry.is_none() && mine.is_empty() { continue; }
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

// ── Dock motion ──────────────────────────────────────────────────────────
//
// Every curve is a pure function of elapsed seconds (already scaled by the
// dock's playback speed) or of a spring's value, bounded, and exactly at
// rest outside its window, so an icon always settles on its classic
// geometry.

/// Pulse: one swell lasts this long, seconds, and grows the icon by up to
/// this share.
pub const PULSE_S: f32 = 0.7;
pub const PULSE_AMP: f32 = 0.14;
/// Ripple: one ring lasts this long and spreads to this many icon sizes,
/// starting at this opacity.
pub const RIPPLE_S: f32 = 1.0;
pub const RIPPLE_REACH: f32 = 1.6;
pub const RIPPLE_ALPHA: f32 = 0.6;
/// Wiggle: side-to-side swings per second and their reach (icon sizes).
pub const WIGGLE_HZ: f32 = 6.0;
pub const WIGGLE_FRAC: f32 = 0.09;
/// Minimize feedback: how long the longest style plays, and how deep a
/// Dip squashes.
pub const MINIMIZE_S: f32 = PULSE_S;
pub const DIP_AMP: f32 = 0.3;
/// A Dip's first squash lasts this long and its rebound decays at this
/// rate; it ends after three half-swings, exactly at rest.
const DIP_HALF_S: f32 = 0.22;
const DIP_DECAY: f32 = 5.0;
/// A Scale reveal starts this much smaller than the resting dock, and
/// may overshoot its rest by this share.
pub const REVEAL_SCALE_DEPTH: f32 = 0.35;
const REVEAL_SCALE_OVER: f32 = 0.04;
/// Arriving icons may pop this far past their resting size.
pub const ARRIVE_OVERSHOOT: f32 = 0.25;

/// How an icon is nudged off its resting place, in icon sizes: `lift`
/// (share of a full hop, toward the screen's middle), `grow` (extra scale:
/// growth is centred, shrinking sinks into the dock) and `shift` (along
/// the dock).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Nudge {
    pub lift: f32,
    pub grow: f32,
    pub shift: f32,
}

impl Nudge {
    pub const REST: Self = Self { lift: 0.0, grow: 0.0, shift: 0.0 };

    pub fn at_rest(&self) -> bool {
        *self == Self::REST
    }
}

/// Extra scale (0..=[`PULSE_AMP`]) of an icon pulsing `t` seconds in.
pub fn pulse(t: f32) -> f32 {
    let x = (t.max(0.0) / PULSE_S).fract();
    let s = (std::f32::consts::PI * x).sin();
    PULSE_AMP * s * s
}

/// A ring spreading from under a launching icon `t` seconds in: its size
/// (icon sizes) and opacity. A new ring starts every [`RIPPLE_S`].
pub fn ripple(t: f32) -> (f32, f32) {
    let x = (t.max(0.0) / RIPPLE_S).fract();
    let ease = 1.0 - (1.0 - x).powi(3);
    // Fades in over the first tenth so a new ring never pops.
    (1.0 + (RIPPLE_REACH - 1.0) * ease, RIPPLE_ALPHA * (1.0 - x) * (x * 10.0).min(1.0))
}

/// A launching icon's nudge `t` seconds in (Ripple draws a ring instead,
/// [`launch_ring`]).
pub fn launch_nudge(style: LaunchFeedback, t: f32) -> Nudge {
    match style {
        LaunchFeedback::Bounce => Nudge { lift: launch_hop(t), ..Nudge::REST },
        LaunchFeedback::Pulse => Nudge { grow: pulse(t), ..Nudge::REST },
        LaunchFeedback::Ripple | LaunchFeedback::None => Nudge::REST,
    }
}

/// The ring under a launching icon, for [`LaunchFeedback::Ripple`].
pub fn launch_ring(style: LaunchFeedback, t: f32) -> Option<(f32, f32)> {
    (style == LaunchFeedback::Ripple).then(|| ripple(t))
}

/// Seconds until the next attention burst, while `t` is in a rest (no
/// frames are needed until then), else `None`.
pub fn attention_rest_left(t: f32) -> Option<f32> {
    let burst = ATTENTION_HOPS * LAUNCH_HOP_S;
    let x = t.max(0.0) % (burst + ATTENTION_REST_S);
    (x >= burst).then_some(burst + ATTENTION_REST_S - x)
}

/// An icon asking for attention `t` seconds in: bursts of
/// [`ATTENTION_HOPS`] bounces, pulses or a wiggle, then a rest.
pub fn attention_nudge(style: AttentionFeedback, t: f32) -> Nudge {
    let burst = ATTENTION_HOPS * LAUNCH_HOP_S;
    let x = t.max(0.0) % (burst + ATTENTION_REST_S);
    if x >= burst {
        return Nudge::REST;
    }
    match style {
        AttentionFeedback::Bounce => Nudge { lift: launch_hop(x), ..Nudge::REST },
        AttentionFeedback::Pulse => {
            let s = (std::f32::consts::PI * x / LAUNCH_HOP_S).sin();
            Nudge { grow: PULSE_AMP * s * s, ..Nudge::REST }
        }
        AttentionFeedback::Wiggle => {
            // Swings inside a soft envelope, still at both ends of the burst.
            let envelope = (std::f32::consts::PI * x / burst).sin();
            Nudge { shift: WIGGLE_FRAC * envelope * (std::f32::consts::TAU * WIGGLE_HZ * x).sin(), ..Nudge::REST }
        }
        AttentionFeedback::None => Nudge::REST,
    }
}

/// The app icon's answer `t` seconds after one of its windows minimized:
/// a Dip squashes into the dock and springs back past its size once, a
/// Pulse swells once.
pub fn minimize_nudge(style: MinimizeFeedback, t: f32) -> Nudge {
    if !(0.0..MINIMIZE_S).contains(&t) {
        return Nudge::REST;
    }
    match style {
        MinimizeFeedback::Dip if t < 3.0 * DIP_HALF_S => {
            let wave = (std::f32::consts::PI * t / DIP_HALF_S).sin();
            Nudge { grow: -DIP_AMP * (-DIP_DECAY * t).exp() * wave, ..Nudge::REST }
        }
        MinimizeFeedback::Pulse => Nudge { grow: pulse(t), ..Nudge::REST },
        MinimizeFeedback::Dip => Nudge::REST,
        MinimizeFeedback::None => Nudge::REST,
    }
}

/// An icon arriving or leaving at `presence` (its spring: 0 gone, 1 here):
/// the share of its slot it takes, its size and its opacity.
pub fn presence(style: ItemMotion, presence: f32) -> (f32, f32, f32) {
    let p = presence.clamp(0.0, 1.0 + ARRIVE_OVERSHOOT);
    match style {
        ItemMotion::Spring => (p, p, p.min(1.0)),
        // Glides on a critically damped spring: no overshoot to clamp.
        ItemMotion::Fade => (p.min(1.0), 1.0, p.min(1.0)),
        ItemMotion::None => (1.0, 1.0, 1.0),
    }
}

/// Indicator mark `k` (0-based) when the indicator spring is at `count`
/// marks: its size share and opacity (`None` while it is gone).
pub fn indicator_mark(style: IndicatorMotion, count: f32, k: usize) -> Option<(f32, f32)> {
    let f = (count - k as f32).clamp(0.0, 1.0);
    if f <= f32::EPSILON {
        return None;
    }
    Some(match style {
        // Eases out: quick to appear, gentle to land.
        IndicatorMotion::Spring => (f * (2.0 - f), 1.0),
        IndicatorMotion::Fade => (1.0, f),
        IndicatorMotion::None => (1.0, 1.0),
    })
}

/// The whole dock while it shows or hides, from the hide spring (0 shown,
/// 1 hidden, overshooting either way): how far it slides (share of its
/// travel), its opacity and its scale about its screen edge.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RevealFrame {
    pub slide: f32,
    pub alpha: f32,
    pub scale: f32,
}

pub fn reveal_frame(style: DockReveal, hide: f32) -> RevealFrame {
    let alpha = 1.0 - hide.clamp(0.0, 1.0);
    match style {
        DockReveal::Slide => RevealFrame { slide: hide, alpha, scale: 1.0 },
        DockReveal::Fade => RevealFrame { slide: 0.0, alpha, scale: 1.0 },
        DockReveal::Scale => RevealFrame { slide: 0.0, alpha, scale: (1.0 - REVEAL_SCALE_DEPTH * hide).clamp(1.0 - REVEAL_SCALE_DEPTH, 1.0 + REVEAL_SCALE_OVER) },
    }
}

/// `r` scaled by `k` about `origin` (a Scale reveal's edge point).
pub fn zoom(r: (f32, f32, f32, f32), origin: (f32, f32), k: f32) -> (f32, f32, f32, f32) {
    if k == 1.0 {
        return r;
    }
    (origin.0 + (r.0 - origin.0) * k, origin.1 + (r.1 - origin.1) * k, r.2 * k, r.3 * k)
}

/// The point on the screen edge a Scale reveal grows out of: the middle of
/// the dock's edge side.
pub fn zoom_origin(pos: DockPosition, dock: (f32, f32, f32, f32)) -> (f32, f32) {
    match pos {
        DockPosition::Bottom => (dock.0 + dock.2 / 2.0, dock.1 + dock.3),
        DockPosition::Left => (dock.0, dock.1 + dock.3 / 2.0),
        DockPosition::Right => (dock.0 + dock.2, dock.1 + dock.3 / 2.0),
    }
}

/// The slots to draw: `new`, plus the slots of `old` that left and that
/// `keep` lets linger, each after the slot it followed. A lingering slot
/// keeps its icon but no windows or pin, so it is never mistaken for a
/// live one.
pub fn with_leaving(old: &[Item], new: Vec<Item>, mut keep: impl FnMut(&Item) -> bool) -> Vec<Item> {
    let mut gone: Vec<(Option<usize>, Item)> = Vec::new();
    let mut anchor = None;
    for o in old {
        match new.iter().position(|n| n.key == o.key) {
            Some(i) => anchor = Some(i),
            None if keep(o) => gone.push((anchor, Item { windows: Vec::new(), pinned: false, recent: false, ..o.clone() })),
            None => {}
        }
    }
    if gone.is_empty() {
        return new;
    }
    // Stable: slots that followed the same survivor keep their order.
    gone.sort_by_key(|(a, _)| a.map_or(0, |i| i + 1));
    let mut out = Vec::with_capacity(new.len() + gone.len());
    let mut gone = gone.into_iter().peekable();
    while let Some((_, g)) = gone.next_if(|(a, _)| a.is_none()) {
        out.push(g);
    }
    for (i, n) in new.into_iter().enumerate() {
        out.push(n);
        while let Some((_, g)) = gone.next_if(|(a, _)| *a == Some(i)) {
            out.push(g);
        }
    }
    out
}

/// Whether the slots both lists show swapped places (a reorder, not just
/// slots arriving or leaving).
pub fn reordered(old: &[Item], new: &[Item]) -> bool {
    let kept_old = old.iter().filter(|o| new.iter().any(|n| n.key == o.key)).map(|i| &i.key);
    let kept_new = new.iter().filter(|n| old.iter().any(|o| o.key == n.key)).map(|i| &i.key);
    !kept_old.eq(kept_new)
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
    quote_str(&path.to_string_lossy())
}

/// `s` quoted for an `Exec` line.
fn quote_str(s: &str) -> String {
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

/// `exec` with an address (`trash:///`) in its file code, whichever it is,
/// or appended when it has none.
pub fn exec_with_uri(exec: &str, uri: &str) -> String {
    let mut used = false;
    let mut out: Vec<String> = exec
        .split_whitespace()
        .map(|word| match word {
            "%f" | "%u" | "%F" | "%U" => {
                used = true;
                quote_str(uri)
            }
            _ => word.to_string(),
        })
        .collect();
    if !used {
        out.push(quote_str(uri));
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
        let cfg = DockConfig { pinned: vec![LAUNCHER_PIN.into(), "org.gnome.Nautilus".into(), "firefox".into()], trash: true, ..bare() };
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
    fn missing_pinned_apps_stay_saved_without_showing_dead_launchers() {
        let cfg = DockConfig { pinned: vec!["not-installed".into(), "running-without-entry".into(), "installed".into()], ..bare() };
        let apps = vec![app("installed", None)];
        let windows = vec![win(1, "running-without-entry")];
        let list = items(&inputs(&cfg, &windows, &apps));
        assert_eq!(keys(&list), ["running-without-entry", "installed"]);
        assert_eq!(cfg.pinned, ["not-installed", "running-without-entry", "installed"]);
        let apps = vec![app("installed", None), app("not-installed", None)];
        let list = items(&inputs(&cfg, &windows, &apps));
        assert_eq!(keys(&list), ["not-installed", "running-without-entry", "installed"]);
    }

    #[test]
    fn unpinned_running_apps_linger_until_they_quit() {
        let cfg = DockConfig { show_running: false, ..bare() };
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
        let cfg = DockConfig { minimize_to_app: false, trash: true, ..bare() };
        let list = items(&inputs(&cfg, &windows, &apps));
        assert_eq!(keys(&list), ["org.demo.gamma", "@separator-right", "@min-4", "@trash"]);
        assert_eq!(list[0].windows.len(), 2, "the app keeps counting it");
        let into_icon = items(&inputs(&DockConfig { minimize_to_app: true, ..cfg }, &windows, &apps));
        assert!(!into_icon.iter().any(|i| matches!(i.kind, Kind::Minimized(_))));
    }

    /// The launcher button sits where it is pinned: first, between apps or
    /// last, and drags like a pinned app.
    #[test]
    fn the_launcher_button_shows_where_it_is_pinned() {
        let apps = vec![app("a", None), app("b", None)];
        let windows = vec![win(3, "zed")];
        for (pinned, want) in [
            (vec![LAUNCHER_PIN, "a", "b"], vec![LAUNCHER_PIN, "a", "b", "zed"]),
            (vec!["a", LAUNCHER_PIN, "b"], vec!["a", LAUNCHER_PIN, "b", "zed"]),
            (vec!["a", "b", LAUNCHER_PIN], vec!["a", "b", LAUNCHER_PIN, "zed"]),
            (vec!["a", "b"], vec!["a", "b", "zed"]),
        ] {
            let cfg = DockConfig { pinned: pinned.iter().map(|p| p.to_string()).collect(), ..bare() };
            let list = items(&inputs(&cfg, &windows, &apps));
            assert_eq!(keys(&list), want);
            if let Some(button) = list.iter().find(|i| i.key == LAUNCHER_PIN) {
                assert_eq!(button.kind, Kind::Launcher);
                assert!(button.pinned && button.movable() && !button.is_app(), "kept, dragged and removed like a pin");
                assert_eq!(button.pin_key(), LAUNCHER_PIN);
            }
        }
        // Not an app: the app cap never hides it.
        let cfg = DockConfig { pinned: vec!["a".into(), "b".into(), LAUNCHER_PIN.into()], max_apps: 1, ..bare() };
        assert_eq!(keys(&items(&inputs(&cfg, &[], &apps))), ["a", LAUNCHER_PIN]);
    }

    #[test]
    fn installs_show_after_the_apps() {
        let cfg = DockConfig { pinned: vec!["a".into()], ..bare() };
        let jobs = vec![InstallSlot { id: 9, name: "Thing".into(), icon: String::new(), progress: 0.4, failed: None }];
        let apps = vec![app("a", None)];
        let list = items(&Inputs { installing: &jobs, ..inputs(&cfg, &[], &apps) });
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

    /// Samples `f` over `0..until` seconds at 1 ms.
    fn sample(until: f32, mut f: impl FnMut(f32)) {
        for i in 0..=(until * 1000.0) as usize {
            f(i as f32 / 1000.0);
        }
    }

    #[test]
    fn every_feedback_is_bounded_and_starts_and_ends_at_rest() {
        for style in LaunchFeedback::ALL {
            assert!(launch_nudge(style, 0.0).at_rest(), "{style:?} starts in place");
            sample(6.0, |t| {
                let n = launch_nudge(style, t);
                assert!((0.0..=1.0).contains(&n.lift) && (0.0..=PULSE_AMP).contains(&n.grow) && n.shift == 0.0, "{style:?} {t}: {n:?}");
                if let Some((size, alpha)) = launch_ring(style, t) {
                    assert!((1.0..=RIPPLE_REACH).contains(&size) && (0.0..=RIPPLE_ALPHA).contains(&alpha), "{t}");
                }
            });
        }
        assert!(launch_nudge(LaunchFeedback::None, 0.3).at_rest() && launch_ring(LaunchFeedback::Bounce, 0.3).is_none());
        assert!(launch_nudge(LaunchFeedback::Pulse, PULSE_S / 2.0).grow > PULSE_AMP * 0.99, "swells mid-pulse");
        assert!(launch_ring(LaunchFeedback::Ripple, 0.0).is_some_and(|(_, a)| a == 0.0), "a ring fades in");

        let burst = ATTENTION_HOPS * LAUNCH_HOP_S;
        for style in AttentionFeedback::ALL {
            sample(burst + ATTENTION_REST_S, |t| {
                let n = attention_nudge(style, t);
                assert!((0.0..=1.0).contains(&n.lift) && (0.0..=PULSE_AMP).contains(&n.grow) && n.shift.abs() <= WIGGLE_FRAC, "{style:?} {t}: {n:?}");
                if t >= burst {
                    assert!(n.at_rest(), "{style:?} rests at {t}");
                }
            });
            assert!(attention_nudge(style, 0.0).at_rest(), "{style:?}");
        }
        assert!(attention_nudge(AttentionFeedback::Wiggle, burst / 2.0 + 1.0 / (4.0 * WIGGLE_HZ)).shift.abs() > 0.0);
        assert_eq!(attention_rest_left(0.1), None);
        let left = attention_rest_left(burst + 0.4).unwrap();
        assert!((left - (ATTENTION_REST_S - 0.4)).abs() < 1e-4, "{left}");

        for style in MinimizeFeedback::ALL {
            let mut least = 0.0_f32;
            sample(MINIMIZE_S + 0.5, |t| {
                let n = minimize_nudge(style, t);
                assert!((-DIP_AMP..=PULSE_AMP).contains(&n.grow) && n.lift == 0.0 && n.shift == 0.0, "{style:?} {t}: {n:?}");
                least = least.min(n.grow);
                if t >= MINIMIZE_S {
                    assert!(n.at_rest(), "{style:?} done at {t}");
                }
            });
            assert_eq!(least < -DIP_AMP * 0.5, style == MinimizeFeedback::Dip, "only a Dip squashes ({style:?})");
        }
        // A Dip lands exactly at rest: no jump when it ends.
        assert!(minimize_nudge(MinimizeFeedback::Dip, 3.0 * 0.22 - 1e-4).grow.abs() < 1e-3);
        assert!(minimize_nudge(MinimizeFeedback::Pulse, MINIMIZE_S - 1e-3).grow < 1e-3);
    }

    #[test]
    fn arriving_icons_and_indicators_settle_on_their_classic_size() {
        for style in ItemMotion::ALL {
            assert_eq!(presence(style, 1.0), (1.0, 1.0, 1.0), "{style:?} at rest");
            let (slot, size, alpha) = presence(style, 2.0);
            assert!(slot <= 1.0 + ARRIVE_OVERSHOOT && size <= 1.0 + ARRIVE_OVERSHOOT && alpha <= 1.0);
            let (slot, size, alpha) = presence(style, -0.5);
            assert!(slot >= 0.0 && size >= 0.0 && alpha >= 0.0);
        }
        assert_eq!(presence(ItemMotion::Spring, 0.0), (0.0, 0.0, 0.0), "grows from nothing");
        assert_eq!(presence(ItemMotion::Fade, 0.0), (0.0, 1.0, 0.0), "fades at full size");
        assert_eq!(presence(ItemMotion::None, 0.0), (1.0, 1.0, 1.0));
        for style in IndicatorMotion::ALL {
            assert_eq!(indicator_mark(style, 2.0, 0), Some((1.0, 1.0)), "{style:?}");
            assert_eq!(indicator_mark(style, 2.0, 1), Some((1.0, 1.0)));
            assert_eq!(indicator_mark(style, 2.0, 2), None, "a third dot isn't drawn");
            assert_eq!(indicator_mark(style, 0.0, 0), None);
        }
        assert_eq!(indicator_mark(IndicatorMotion::Spring, 1.5, 1), Some((0.75, 1.0)), "half grown");
        assert_eq!(indicator_mark(IndicatorMotion::Fade, 1.5, 1), Some((1.0, 0.5)), "half faded");
    }

    #[test]
    fn every_reveal_rests_where_the_classic_dock_does() {
        for style in DockReveal::ALL {
            assert_eq!(reveal_frame(style, 0.0), RevealFrame { slide: 0.0, alpha: 1.0, scale: 1.0 }, "{style:?} shown");
            let hidden = reveal_frame(style, 1.0);
            assert_eq!(hidden.alpha, 0.0, "{style:?} hidden");
            for h in [-0.3, 0.5, 1.2] {
                let f = reveal_frame(style, h);
                assert!((0.0..=1.0).contains(&f.alpha) && (1.0 - REVEAL_SCALE_DEPTH..=1.05).contains(&f.scale), "{style:?} {h}: {f:?}");
            }
        }
        assert_eq!(reveal_frame(DockReveal::Slide, 1.0).slide, 1.0, "the classic slide");
        assert_eq!(reveal_frame(DockReveal::Fade, 1.0).slide, 0.0);
        assert!(reveal_frame(DockReveal::Scale, 1.0).scale < 1.0 && reveal_frame(DockReveal::Scale, 1.0).slide == 0.0);
        let r = (100.0, 50.0, 40.0, 40.0);
        assert_eq!(zoom(r, (0.0, 0.0), 1.0), r);
        assert_eq!(zoom(r, (100.0, 90.0), 0.5), (100.0, 70.0, 20.0, 20.0), "shrinks toward the edge point");
        assert_eq!(zoom_origin(DockPosition::Bottom, (10.0, 20.0, 100.0, 60.0)), (60.0, 80.0));
        assert_eq!(zoom_origin(DockPosition::Left, (10.0, 20.0, 60.0, 100.0)), (10.0, 70.0));
        assert_eq!(zoom_origin(DockPosition::Right, (10.0, 20.0, 60.0, 100.0)), (70.0, 70.0));
    }

    #[test]
    fn leaving_slots_linger_where_they_were_and_reorders_are_noticed() {
        let mk = |k: &str| Item { key: k.into(), windows: vec![win(1, k)], pinned: true, ..Item::fixed(Kind::App, k) };
        let old = vec![mk("a"), mk("b"), mk("c"), mk("d")];
        let new = vec![mk("a"), mk("c")];
        let list = with_leaving(&old, new.clone(), |_| true);
        assert_eq!(keys(&list), ["a", "b", "c", "d"], "each after the slot it followed");
        assert!(!list[1].running() && !list[1].pinned, "a leaving slot is no longer live");
        assert_eq!(keys(&with_leaving(&old, new.clone(), |o| o.key == "d")), ["a", "c", "d"]);
        assert_eq!(keys(&with_leaving(&old, vec![mk("c")], |_| true)), ["a", "b", "c", "d"], "leading slots stay first");
        assert_eq!(keys(&with_leaving(&old, new.clone(), |_| false)), ["a", "c"]);
        assert!(!reordered(&old, &new), "removals aren't a reorder");
        assert!(!reordered(&new, &old), "nor are arrivals");
        assert!(reordered(&old, &[mk("b"), mk("a"), mk("c"), mk("d")]));
        assert!(!reordered(&old, &old));
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
    fn an_address_fills_any_file_code() {
        for (exec, argv) in [
            ("nautilus --new-window %U", &["nautilus", "--new-window", "trash:///"][..]),
            ("dolphin %u", &["dolphin", "trash:///"]),
            ("thunar %F", &["thunar", "trash:///"]),
            ("pcmanfm", &["pcmanfm", "trash:///"]),
            ("caja --no-desktop %U %i", &["caja", "--no-desktop", "trash:///"]),
        ] {
            assert_eq!(crate::apps::parse_exec(&exec_with_uri(exec, "trash:///")), argv, "{exec}");
        }
        assert_eq!(crate::apps::parse_exec(&exec_with_uri("x %u", "trash:/a%20b")), ["x", "trash:/a%20b"], "a literal % survives the Exec unescaping");
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
