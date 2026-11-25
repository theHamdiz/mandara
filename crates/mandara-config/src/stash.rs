//! Stash (floating drag-and-drop shelves): its settings
//! (`[plugins.settings.stash]`), and the screen-edge rules of its edge
//! catchers.
//!
//! An edge catcher is a hairline strip along one screen edge that takes a
//! drag arriving there and springs a shelf open under it. Mandara has other
//! things that react to the pointer at an edge (the bar, the sidebar's
//! open-on-hover edge, the launcher and session-dock edges, side bars and
//! rails, the dock) and at corners (hot corners). Edges are never shared:
//!
//! * [`edge_users`] lists what else uses an edge. A catcher on a busy edge
//!   stays inactive ([`catcher_states`]); Studio blocks turning it on and
//!   offers to turn the other feature off or move it ([`EdgeUse::fixes`]).
//!   Nothing is ever switched off silently, on either side.
//! * [`new_collisions`] is the reverse check: a change that makes an edge
//!   with an active catcher busy, so Studio can ask which one keeps it.
//! * Corners are not edges: a catcher leaves a gap at each end, wider where
//!   a hot corner (or the compositor's own corner) sits ([`corner_gap`]).
//! * [`catcher_rects`] is the geometry: the outer stretches of an output's
//!   enabled, free edges, minus the corner gaps and any stretch shared with
//!   a neighbouring output (the pointer only passes through those).
//!
//! Catchers are off by default.

use serde::{Deserialize, Serialize};

use crate::desktop::{CornerAction, ScreenCorner};
use crate::dock::DockPosition;
use crate::{BarPosition, ShellConfig};

/// The plugin's id (opt-in).
pub const STASH_PLUGIN_ID: &str = "stash";

/// Thickness of an edge catcher, logical px (never 0: see `mandara-core`'s
/// layer notes). Two: at fractional scales the output's logical size is
/// fractional (2560 px at 1.5× is 1706.67) and compositors keep the pointer
/// inside a whole-pixel box, so a single logical pixel at the very edge
/// can be out of the pointer's reach.
pub const CATCHER_THICKNESS_PX: f32 = 2.0;
/// Gap a catcher always leaves at each end of its edge, logical px.
pub const CATCHER_CORNER_GAP_PX: f32 = 32.0;
/// Extra room beyond a hot corner's square, logical px.
pub const CATCHER_HOT_CORNER_MARGIN_PX: f32 = 24.0;
/// Shortest catcher stretch worth keeping, logical px.
pub const CATCHER_MIN_LEN_PX: f32 = 48.0;

/// Shelf size limits and defaults, logical px.
pub const SHELF_MIN_W: f32 = 220.0;
pub const SHELF_MIN_H: f32 = 180.0;
pub const SHELF_MAX_W: f32 = 1200.0;
pub const SHELF_MAX_H: f32 = 1200.0;
pub const SHELF_DEFAULT_W: f32 = 320.0;
pub const SHELF_DEFAULT_H: f32 = 380.0;
/// Cache limit range and default, MiB.
pub const CACHE_MIN_MIB: u32 = 64;
pub const CACHE_MAX_MIB: u32 = 8192;
pub const CACHE_DEFAULT_MIB: u32 = 1024;

/// A screen edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenEdge {
    Top,
    Right,
    Bottom,
    Left,
}

impl ScreenEdge {
    pub const ALL: [ScreenEdge; 4] = [ScreenEdge::Top, ScreenEdge::Right, ScreenEdge::Bottom, ScreenEdge::Left];

    pub fn id(self) -> &'static str {
        match self {
            ScreenEdge::Top => "top",
            ScreenEdge::Right => "right",
            ScreenEdge::Bottom => "bottom",
            ScreenEdge::Left => "left",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.id().eq_ignore_ascii_case(s.trim()))
    }

    /// English label (`tk`-marked where shown).
    pub fn label(self) -> &'static str {
        match self {
            ScreenEdge::Top => mandara_i18n::tk("Top edge"),
            ScreenEdge::Right => mandara_i18n::tk("Right edge"),
            ScreenEdge::Bottom => mandara_i18n::tk("Bottom edge"),
            ScreenEdge::Left => mandara_i18n::tk("Left edge"),
        }
    }

    /// The setting that turns this edge's catcher on.
    pub fn setting_key(self) -> &'static str {
        match self {
            ScreenEdge::Top => "edge_top",
            ScreenEdge::Right => "edge_right",
            ScreenEdge::Bottom => "edge_bottom",
            ScreenEdge::Left => "edge_left",
        }
    }

    /// Its JSON pointer in the config.
    pub fn setting_path(self) -> &'static str {
        match self {
            ScreenEdge::Top => "/plugins/settings/stash/edge_top",
            ScreenEdge::Right => "/plugins/settings/stash/edge_right",
            ScreenEdge::Bottom => "/plugins/settings/stash/edge_bottom",
            ScreenEdge::Left => "/plugins/settings/stash/edge_left",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, ScreenEdge::Top | ScreenEdge::Bottom)
    }

    /// The two corners at its ends (start first: left or top).
    pub fn corners(self) -> [ScreenCorner; 2] {
        match self {
            ScreenEdge::Top => [ScreenCorner::TopLeft, ScreenCorner::TopRight],
            ScreenEdge::Bottom => [ScreenCorner::BottomLeft, ScreenCorner::BottomRight],
            ScreenEdge::Left => [ScreenCorner::TopLeft, ScreenCorner::BottomLeft],
            ScreenEdge::Right => [ScreenCorner::TopRight, ScreenCorner::BottomRight],
        }
    }
}

/// How a shelf lays out its items.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShelfView {
    #[default]
    Grid,
    List,
    /// A fanned pile, the newest on top.
    Stack,
}

impl ShelfView {
    pub const ALL: [ShelfView; 3] = [ShelfView::Grid, ShelfView::List, ShelfView::Stack];

    pub fn id(self) -> &'static str {
        match self {
            ShelfView::Grid => "grid",
            ShelfView::List => "list",
            ShelfView::Stack => "stack",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.id().eq_ignore_ascii_case(s.trim()))
    }
}

/// Where minimized shelves go.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MinimizeTo {
    /// The Stash bar item.
    #[default]
    Bar,
    /// The tray.
    Tray,
    Both,
}

impl MinimizeTo {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bar" => Some(MinimizeTo::Bar),
            "tray" => Some(MinimizeTo::Tray),
            "both" => Some(MinimizeTo::Both),
            _ => None,
        }
    }

    pub fn bar(self) -> bool {
        matches!(self, MinimizeTo::Bar | MinimizeTo::Both)
    }

    pub fn tray(self) -> bool {
        matches!(self, MinimizeTo::Tray | MinimizeTo::Both)
    }
}

/// Shake sensitivities, as stored (`mandara_core::shake::ShakeSensitivity`
/// ids, in its order).
pub const SHAKE_LEVELS: [&str; 4] = ["off", "gentle", "normal", "firm"];

/// `[plugins.settings.stash]`, resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct StashSettings {
    /// Index into [`SHAKE_LEVELS`] (0 = off).
    pub shake: u8,
    /// Catchers switched on, by [`ScreenEdge::index`].
    pub edges: [bool; 4],
    pub default_width: f32,
    pub default_height: f32,
    pub default_view: ShelfView,
    pub minimize_to: MinimizeTo,
    /// Pictures dropped as files are copied into the stash (they survive the
    /// file being moved or deleted) instead of referenced.
    pub copy_images: bool,
    /// Limit for the stash's own files, MiB.
    pub max_cache_mib: u32,
}

impl Default for StashSettings {
    fn default() -> Self {
        Self { shake: 2, edges: [false; 4], default_width: SHELF_DEFAULT_W, default_height: SHELF_DEFAULT_H, default_view: ShelfView::Grid, minimize_to: MinimizeTo::Bar, copy_images: false, max_cache_mib: CACHE_DEFAULT_MIB }
    }
}

impl StashSettings {
    pub fn from_config(cfg: &ShellConfig) -> Self {
        let p = &cfg.plugins;
        let id = STASH_PLUGIN_ID;
        let d = Self::default();
        let shake = p.setting_str(id, "shake").and_then(|s| SHAKE_LEVELS.iter().position(|l| l.eq_ignore_ascii_case(s.trim()))).map_or(d.shake, |i| i as u8);
        let mut edges = [false; 4];
        for e in ScreenEdge::ALL {
            edges[e.index()] = p.setting(id, e.setting_key(), false);
        }
        Self {
            shake,
            edges,
            default_width: p.setting(id, "width", f64::from(d.default_width)).clamp(f64::from(SHELF_MIN_W), f64::from(SHELF_MAX_W)) as f32,
            default_height: p.setting(id, "height", f64::from(d.default_height)).clamp(f64::from(SHELF_MIN_H), f64::from(SHELF_MAX_H)) as f32,
            default_view: p.setting_str(id, "view").and_then(ShelfView::parse).unwrap_or(d.default_view),
            minimize_to: p.setting_str(id, "minimize_to").and_then(MinimizeTo::parse).unwrap_or(d.minimize_to),
            copy_images: p.setting(id, "copy_images", d.copy_images),
            max_cache_mib: p.setting(id, "max_cache_mib", d.max_cache_mib).clamp(CACHE_MIN_MIB, CACHE_MAX_MIB),
        }
    }

    pub fn edge_on(&self, edge: ScreenEdge) -> bool {
        self.edges[edge.index()]
    }

    pub fn max_cache_bytes(&self) -> u64 {
        u64::from(self.max_cache_mib) * 1024 * 1024
    }
}

/// Where minimized shelves go, read without copying the settings table
/// (the bar item asks on every render).
pub fn minimize_to(cfg: &ShellConfig) -> MinimizeTo {
    cfg.plugins.setting_str(STASH_PLUGIN_ID, "minimize_to").and_then(MinimizeTo::parse).unwrap_or_default()
}

/// Which catchers are switched on, read without copying the settings table
/// (the shell surface asks on every render).
pub fn edge_flags(cfg: &ShellConfig) -> [bool; 4] {
    let table = cfg.plugins.settings.get(STASH_PLUGIN_ID);
    ScreenEdge::ALL.map(|e| table.and_then(|t| t.get(e.setting_key())).and_then(toml::Value::as_bool).unwrap_or(false))
}

/// Whether the Stash plugin is switched on (it is opt-in).
pub fn stash_enabled(cfg: &ShellConfig) -> bool {
    cfg.plugins.opted_in(STASH_PLUGIN_ID)
}

// ── Who uses an edge ─────────────────────────────────────────────────────

/// The compositor, where an edge rule depends on it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeContext {
    /// Niri: no session-dock edge (it scrolls with screen edges); its own
    /// top-left hot corner is on unless Mandara's hot corners are.
    pub niri: bool,
    /// KWin: its own screen edges (top-left: Overview by default).
    pub kwin: bool,
}

impl EdgeContext {
    /// From a compositor name (`niri`, `kwin`, …); unknown names have no rules.
    pub fn for_compositor(name: &str) -> Self {
        let n = name.trim().to_ascii_lowercase();
        Self { niri: n == "niri", kwin: n == "kwin" || n == "plasma" }
    }
}

/// A feature that reacts to the pointer along a whole edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeFeature {
    /// The bar runs along this edge (and its auto-hide reveal strip).
    Bar,
    /// The sidebar opens when the pointer touches this edge.
    SidebarHover,
    /// The launcher opens when the pointer rests on this edge.
    LauncherHover,
    /// The power / session dock opens from this edge.
    SessionDockHover,
    /// A side bar (or rail) is on this edge.
    SideBar,
    /// The app dock (and its reveal strip) is on this edge.
    Dock,
}

/// What a fix sets.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum FixValue {
    Bool(bool),
    Str(&'static str),
}

/// One click that frees an edge (or moves the feature to a free one).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EdgeFix {
    /// `tk`-marked button label ("Turn that off", "Move it to the right").
    pub label: &'static str,
    /// The setting's JSON pointer.
    pub path: &'static str,
    pub value: FixValue,
}

/// A feature on an edge, with where to change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EdgeUse {
    pub edge: ScreenEdge,
    pub feature: EdgeFeature,
    /// `tk`-marked sentence ("The sidebar opens on hover here").
    pub reason: &'static str,
    /// The Studio setting that controls it (a link).
    pub setting: &'static str,
    /// One-click resolutions, if any.
    pub fixes: Vec<EdgeFix>,
}

/// The edge a bar position sits on.
pub fn bar_edge(position: BarPosition) -> ScreenEdge {
    match position {
        BarPosition::Top => ScreenEdge::Top,
        BarPosition::Bottom => ScreenEdge::Bottom,
        BarPosition::Left => ScreenEdge::Left,
        BarPosition::Right => ScreenEdge::Right,
    }
}

fn dock_edge(position: DockPosition) -> ScreenEdge {
    match position {
        DockPosition::Bottom => ScreenEdge::Bottom,
        DockPosition::Left => ScreenEdge::Left,
        DockPosition::Right => ScreenEdge::Right,
    }
}

fn fix(label: &'static str, path: &'static str, value: FixValue) -> EdgeFix {
    EdgeFix { label, path, value }
}

/// Everything else that uses `edge` (empty = the edge is free).
pub fn edge_users(cfg: &ShellConfig, edge: ScreenEdge, cx: EdgeContext) -> Vec<EdgeUse> {
    use mandara_i18n::tk;
    let mut out = Vec::new();
    let mut push = |feature, reason, setting, fixes: &[EdgeFix]| out.push(EdgeUse { edge, feature, reason, setting, fixes: fixes.to_vec() });
    // The bar (with its auto-hide reveal strip on the same edge).
    if bar_edge(cfg.bar.position) == edge {
        let away: &[EdgeFix] = match edge {
            ScreenEdge::Top => &[fix(tk("Move the bar to the bottom"), "/bar/position", FixValue::Str("bottom"))],
            _ => &[fix(tk("Move the bar to the top"), "/bar/position", FixValue::Str("top"))],
        };
        push(EdgeFeature::Bar, tk("The bar runs along this edge"), "/bar/position", away);
    }
    // The sidebar's open-on-hover edge (not while a vertical bar holds it).
    let sidebar_edge = if cfg.sidebar_on_right() { ScreenEdge::Right } else { ScreenEdge::Left };
    if edge == sidebar_edge && cfg.sidebar_active() && cfg.sidebar.hover_reveal && bar_edge(cfg.bar.position) != edge {
        let other: &'static str = if sidebar_edge == ScreenEdge::Right { "left" } else { "right" };
        let move_label = if other == "left" { tk("Move the sidebar to the left") } else { tk("Move the sidebar to the right") };
        push(EdgeFeature::SidebarHover, tk("The sidebar opens on hover here"), "/sidebar/hover_reveal", &[fix(tk("Turn that off"), "/sidebar/hover_reveal", FixValue::Bool(false)), fix(move_label, "/sidebar/side", FixValue::Str(other))]);
    }
    // The launcher's edge: the bottom, or the top while the bar is at the bottom.
    let launcher_edge = if cfg.bar.position == BarPosition::Bottom { ScreenEdge::Top } else { ScreenEdge::Bottom };
    if edge == launcher_edge && cfg.launcher_hover_enabled() {
        push(EdgeFeature::LauncherHover, tk("The launcher opens when the pointer rests here"), "/edges/launcher_bottom", &[fix(tk("Turn that off"), "/edges/launcher_bottom", FixValue::Bool(false))]);
    }
    // The session dock's edge: opposite the sidebar (never on Niri).
    let session_edge = if cfg.sidebar_on_right() { ScreenEdge::Left } else { ScreenEdge::Right };
    let session_side_taken = bar_edge(cfg.bar.position) == session_edge || cfg.side_bar_on(session_edge == ScreenEdge::Right);
    if edge == session_edge && cfg.edges.session_right && !cx.niri && !session_side_taken {
        push(EdgeFeature::SessionDockHover, tk("The power menu opens from this edge"), "/edges/session_right", &[fix(tk("Turn that off"), "/edges/session_right", FixValue::Bool(false))]);
    }
    // Side bars and rails (pinned columns or their reveal strips).
    for right in [false, true] {
        let side = if right { ScreenEdge::Right } else { ScreenEdge::Left };
        if edge == side && cfg.side_bar_on(right) {
            let path = if right { "/side_bars/right/enabled" } else { "/side_bars/left/enabled" };
            push(EdgeFeature::SideBar, tk("A side bar runs along this edge"), path, &[fix(tk("Turn that off"), path, FixValue::Bool(false))]);
        }
    }
    // The app dock (its reveal strip while hidden).
    if cfg.dock_enabled() {
        let dock = cfg.dock_sanitized();
        if dock_edge(dock.position) == edge {
            let moves: &[EdgeFix] = match edge {
                ScreenEdge::Bottom => &[fix(tk("Move the dock to the left"), "/dock/position", FixValue::Str("left")), fix(tk("Move the dock to the right"), "/dock/position", FixValue::Str("right"))],
                _ => &[fix(tk("Move the dock to the bottom"), "/dock/position", FixValue::Str("bottom"))],
            };
            push(EdgeFeature::Dock, tk("The dock sits on this edge"), "/dock/position", moves);
        }
    }
    out
}

/// Whether the shell's hot corner `corner` fires (on, with an action).
fn hot_corner_active(cfg: &ShellConfig, corner: ScreenCorner) -> bool {
    cfg.hot_corners.enabled && cfg.hot_corners.corner(corner).is_active()
}

/// What sits in a corner, for Studio's notes ("Top-left corner: hot corner
/// → Overview").
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CornerUse {
    #[serde(serialize_with = "corner_id")]
    pub corner: ScreenCorner,
    /// `None`: the compositor's own corner (Niri, KWin).
    pub action: Option<CornerAction>,
}

fn corner_id<S: serde::Serializer>(corner: &ScreenCorner, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(corner.id())
}

/// The corners in use (shell hot corners and the compositor's own).
pub fn corner_users(cfg: &ShellConfig, cx: EdgeContext) -> Vec<CornerUse> {
    let mut out: Vec<CornerUse> = ScreenCorner::ALL.into_iter().filter(|c| hot_corner_active(cfg, *c)).map(|c| CornerUse { corner: c, action: Some(cfg.hot_corners.corner(c).action) }).collect();
    // The compositor's own top-left corner stays on unless Mandara's are.
    if (cx.niri || cx.kwin) && !cfg.hot_corners.any_active() {
        out.push(CornerUse { corner: ScreenCorner::TopLeft, action: None });
    }
    out
}

/// The gap a catcher leaves at `corner`, logical px.
pub fn corner_gap(cfg: &ShellConfig, corner: ScreenCorner, cx: EdgeContext) -> f32 {
    let hot = corner_users(cfg, cx).iter().any(|u| u.corner == corner);
    if !hot {
        return CATCHER_CORNER_GAP_PX;
    }
    let square = cfg.hot_corners.size.clamp(1.0, 24.0);
    CATCHER_CORNER_GAP_PX.max(square + CATCHER_HOT_CORNER_MARGIN_PX)
}

/// Where one edge's catcher stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CatcherState {
    /// Not switched on (or Stash is off).
    Off,
    /// On and catching.
    Active,
    /// On in the settings, but the edge is in use: inactive until resolved.
    Blocked { by: Vec<EdgeUse> },
}

impl CatcherState {
    pub fn is_active(&self) -> bool {
        matches!(self, CatcherState::Active)
    }
}

/// Every edge's catcher state, by [`ScreenEdge::index`].
pub fn catcher_states(cfg: &ShellConfig, cx: EdgeContext) -> [CatcherState; 4] {
    let settings = StashSettings::from_config(cfg);
    let on = stash_enabled(cfg);
    ScreenEdge::ALL.map(|edge| {
        if !on || !settings.edge_on(edge) {
            return CatcherState::Off;
        }
        let by = edge_users(cfg, edge, cx);
        if by.is_empty() { CatcherState::Active } else { CatcherState::Blocked { by } }
    })
}

/// The reverse check: edges whose catcher was active in `before` and that
/// `after` makes busy, with what now claims them. Studio asks which one
/// keeps the edge instead of applying such a change silently.
pub fn new_collisions(before: &ShellConfig, after: &ShellConfig, cx: EdgeContext) -> Vec<(ScreenEdge, Vec<EdgeUse>)> {
    let was = catcher_states(before, cx);
    let settings = StashSettings::from_config(after);
    if !stash_enabled(after) {
        return Vec::new();
    }
    ScreenEdge::ALL
        .into_iter()
        .filter(|e| was[e.index()].is_active() && settings.edge_on(*e))
        .filter_map(|e| {
            let by = edge_users(after, e, cx);
            (!by.is_empty()).then_some((e, by))
        })
        .collect()
}

// ── Geometry ─────────────────────────────────────────────────────────────

/// An axis-aligned rectangle in layout coordinates, logical px.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Stretches of `output`'s `edge` (as `(start, end)` along the edge, in
/// layout coordinates) that no other output in `layout` touches.
pub fn outer_spans(output: Rect, layout: &[Rect], edge: ScreenEdge) -> smallvec::SmallVec<[(f32, f32); 2]> {
    const TOUCH: f32 = 0.5;
    let (start, end) = if edge.is_horizontal() { (output.x, output.right()) } else { (output.y, output.bottom()) };
    let mut spans: smallvec::SmallVec<[(f32, f32); 2]> = smallvec::smallvec![(start, end)];
    for other in layout {
        if *other == output {
            continue;
        }
        // The stretch of `other` that lies against this edge, if any.
        let covered = match edge {
            ScreenEdge::Top if (other.bottom() - output.y).abs() <= TOUCH => Some((other.x, other.right())),
            ScreenEdge::Bottom if (other.y - output.bottom()).abs() <= TOUCH => Some((other.x, other.right())),
            ScreenEdge::Left if (other.right() - output.x).abs() <= TOUCH => Some((other.y, other.bottom())),
            ScreenEdge::Right if (other.x - output.right()).abs() <= TOUCH => Some((other.y, other.bottom())),
            _ => None,
        };
        let Some((cs, ce)) = covered else { continue };
        let mut next = smallvec::SmallVec::new();
        for (s, e) in spans {
            if ce <= s || cs >= e {
                next.push((s, e));
                continue;
            }
            if cs > s {
                next.push((s, cs));
            }
            if ce < e {
                next.push((ce, e));
            }
        }
        spans = next;
    }
    spans
}

/// The catcher strips of one output, in output-local logical px: each
/// active edge's outer stretches, minus `gap(start corner)` and `gap(end
/// corner)` where a stretch reaches a corner, and dropped when shorter than
/// [`CATCHER_MIN_LEN_PX`].
pub fn catcher_rects(output: Rect, layout: &[Rect], active: [bool; 4], gap: impl Fn(ScreenCorner) -> f32) -> smallvec::SmallVec<[(ScreenEdge, Rect); 4]> {
    let t = CATCHER_THICKNESS_PX;
    let mut out = smallvec::SmallVec::new();
    for edge in ScreenEdge::ALL {
        if !active[edge.index()] {
            continue;
        }
        let [first, last] = edge.corners();
        let (lo, hi) = if edge.is_horizontal() { (output.x, output.right()) } else { (output.y, output.bottom()) };
        for (s, e) in outer_spans(output, layout, edge) {
            let s = if s <= lo + 0.5 { s + gap(first) } else { s };
            let e = if e >= hi - 0.5 { e - gap(last) } else { e };
            if e - s < CATCHER_MIN_LEN_PX {
                continue;
            }
            // Output-local.
            let r = match edge {
                ScreenEdge::Top => Rect::new(s - output.x, 0.0, e - s, t),
                ScreenEdge::Bottom => Rect::new(s - output.x, output.h - t, e - s, t),
                ScreenEdge::Left => Rect::new(0.0, s - output.y, t, e - s),
                ScreenEdge::Right => Rect::new(output.w - t, s - output.y, t, e - s),
            };
            out.push((edge, r));
        }
    }
    out
}

/// Where a shelf springs open for a drag that reached `edge` at `along`
/// (output-local, along the edge), sized `w`×`h` in an output `out_w`×`out_h`:
/// its top-left corner, flush with the edge plus `inset`, centred on the
/// pointer and kept on screen.
pub fn shelf_at_edge(edge: ScreenEdge, along: f32, out_w: f32, out_h: f32, w: f32, h: f32, inset: f32) -> (f32, f32) {
    let clamp = |v: f32, max: f32| v.clamp(inset, (max - inset).max(inset));
    match edge {
        ScreenEdge::Top => (clamp(along - w / 2.0, out_w - w), inset),
        ScreenEdge::Bottom => (clamp(along - w / 2.0, out_w - w), (out_h - h - inset).max(0.0)),
        ScreenEdge::Left => (inset, clamp(along - h / 2.0, out_h - h)),
        ScreenEdge::Right => ((out_w - w - inset).max(0.0), clamp(along - h / 2.0, out_h - h)),
    }
}

/// A moved shelf settles flush against an edge when it lands within
/// `snap` of it (its gentle edge snap), else stays where it was dropped,
/// always fully on the output.
pub fn snap_shelf(x: f32, y: f32, w: f32, h: f32, out_w: f32, out_h: f32, snap: f32, inset: f32) -> (f32, f32) {
    let max_x = (out_w - w).max(0.0);
    let max_y = (out_h - h).max(0.0);
    let mut x = x.clamp(0.0, max_x);
    let mut y = y.clamp(0.0, max_y);
    if x < snap + inset {
        x = inset.min(max_x);
    } else if max_x - x < snap + inset {
        x = (max_x - inset).max(0.0);
    }
    if y < snap + inset {
        y = inset.min(max_y);
    } else if max_y - y < snap + inset {
        y = (max_y - inset).max(0.0);
    }
    (x, y)
}

#[cfg(test)]
mod tests;
