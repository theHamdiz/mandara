//! Compositor-neutral navigation model.
//!
//! MangoWM's and River's bitmask tags and Niri/Hyprland/Umbriel/Sway's linear workspaces are
//! modelled natively rather than squeezed into a lowest-common-denominator
//! struct.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositorKind {
    MangoWM,
    Niri,
    Hyprland,
    Umbriel,
    Sway,
    River,
}

impl CompositorKind {
    /// The config dialect this compositor reads.
    pub const fn target(self) -> mandara_config::compositor::Target {
        use mandara_config::compositor::Target;
        match self {
            CompositorKind::MangoWM => Target::Mango,
            CompositorKind::Niri => Target::Niri,
            CompositorKind::Hyprland => Target::Hyprland,
            CompositorKind::Umbriel => Target::Umbriel,
            CompositorKind::Sway => Target::Sway,
            CompositorKind::River => Target::River,
        }
    }
}

/// One MangoWM or River tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagStatus {
    /// `1..=9`.
    pub id: u32,
    pub active: bool,
    pub occupied: bool,
    pub urgent: bool,
    pub client_count: u32,
    /// Layout symbol for this tag (e.g. `T`, `S`, `M`).
    pub layout: String,
}

/// The first id of a workspace that has no number (Sway's named
/// workspaces): ids from here on are made up and stable per name, and can
/// never equal a real workspace number (Sway's numbers stop below it).
pub const UNNUMBERED_ID_BASE: u32 = 1 << 31;

/// Whether `id` belongs to a workspace without a number
/// ([`UNNUMBERED_ID_BASE`]): show its name, never the id.
pub fn is_unnumbered(id: u32) -> bool {
    id >= UNNUMBERED_ID_BASE
}

/// One linear / scrollable workspace (Niri, Hyprland, Umbriel, Sway).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    /// Its number (what "go to workspace N" takes), or a made-up id from
    /// [`UNNUMBERED_ID_BASE`] up for a workspace without one.
    pub id: u32,
    pub name: String,
    pub active: bool,
    pub occupied: bool,
    pub urgent: bool,
    /// The workspace's own layout where the compositor has one per
    /// workspace (Umbriel: `scrolling`, `dwindle`, `master`, `stacking`;
    /// Sway: its container layout, `splith`, `splitv`, `tabbed`,
    /// `stacked`); empty elsewhere.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub layout: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationModel {
    BitmaskTags { tags: Vec<TagStatus>, active_layout: String, keymode: String },
    Workspaces { workspaces: Vec<WorkspaceStatus> },
}

impl NavigationModel {
    /// Bit `n-1` set for every active tag / workspace `n`.
    pub fn active_mask(&self) -> u32 {
        match self {
            NavigationModel::BitmaskTags { tags, .. } => {
                tags.iter().filter(|t| t.active).fold(0, |m, t| m | 1 << (t.id.saturating_sub(1)))
            }
            NavigationModel::Workspaces { workspaces } => workspaces
                .iter()
                .filter(|w| w.active)
                .fold(0, |m, w| m | 1u32.checked_shl(w.id.saturating_sub(1)).unwrap_or(0)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub monitor: String,
    pub tags: Vec<u32>,
    pub focused: bool,
    pub floating: bool,
    pub fullscreen: bool,
    /// Layout-space geometry `(x, y, width, height)` when the compositor
    /// reports it (used for window screenshots).
    pub geometry: Option<(i32, i32, i32, i32)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f32,
    pub active: bool,
}

/// Events pushed by a driver's watch streams.
#[derive(Clone, Debug, PartialEq)]
pub enum CompositorEvent {
    /// Navigation state for a monitor changed.
    Navigation { monitor: String, model: NavigationModel },
    /// The focused client changed (or its title did).
    FocusedClient(Option<ClientInfo>),
    /// The full window list changed.
    Clients(Vec<ClientInfo>),
    Monitors(Vec<MonitorInfo>),
    Keymode(String),
    KeyboardLayout(String),
}
