//! Compositor-neutral navigation model.
//!
//! MangoWM's bitmask tags and Niri/Hyprland's linear workspaces are modelled
//! natively rather than squeezed into a lowest-common-denominator struct.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositorKind {
    MangoWM,
    Niri,
    Hyprland,
}

/// One MangoWM tag.
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

/// One linear / scrollable workspace (Niri, Hyprland).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub id: u32,
    pub name: String,
    pub active: bool,
    pub occupied: bool,
    pub urgent: bool,
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
