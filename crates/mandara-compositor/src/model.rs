//! Compositor-neutral navigation model.
//!
//! MangoWM's and River's bitmask tags and Niri/Hyprland/Umbriel/Sway/KWin/Aqueous/labwc/COSMIC's workspaces are
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
    KWin,
    Aqueous,
    Labwc,
    Cosmic,
}

impl CompositorKind {
    /// The integration target for this compositor.
    pub const fn target(self) -> mandara_config::compositor::Target {
        use mandara_config::compositor::Target;
        match self {
            CompositorKind::MangoWM => Target::Mango,
            CompositorKind::Niri => Target::Niri,
            CompositorKind::Hyprland => Target::Hyprland,
            CompositorKind::Umbriel => Target::Umbriel,
            CompositorKind::Sway => Target::Sway,
            CompositorKind::River => Target::River,
            CompositorKind::KWin => Target::KWin,
            CompositorKind::Aqueous => Target::Aqueous,
            CompositorKind::Labwc => Target::Labwc,
            CompositorKind::Cosmic => Target::Cosmic,
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
    /// IDs of the active tags or workspaces, including named workspaces.
    pub fn active_ids(&self) -> impl Iterator<Item = u32> + '_ {
        let (tags, workspaces): (&[TagStatus], &[WorkspaceStatus]) = match self {
            Self::BitmaskTags { tags, .. } => (tags, &[]),
            Self::Workspaces { workspaces } => (&[], workspaces),
        };
        tags.iter().filter(|t| t.active).map(|t| t.id).chain(workspaces.iter().filter(|w| w.active).map(|w| w.id))
    }

    /// At least one of these client tags is currently active.
    pub fn contains_active(&self, ids: &[u32]) -> bool {
        self.active_ids().any(|id| ids.contains(&id))
    }

    /// Bit `n-1` for active numeric tags in the range 1..=32.
    /// Named-workspace membership uses [`Self::contains_active`].
    pub fn active_mask(&self) -> u32 {
        self.active_ids().filter_map(|id| id.checked_sub(1).and_then(|bit| 1u32.checked_shl(bit))).fold(0, |mask, bit| mask | bit)
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn active_membership_preserves_named_and_large_numbered_workspaces() {
        let model = NavigationModel::Workspaces {
            workspaces: [7, 64, UNNUMBERED_ID_BASE].into_iter().map(|id| WorkspaceStatus {
                id, name: id.to_string(), active: id != 7, occupied: true, urgent: false, layout: String::new(),
            }).collect(),
        };
        assert!(model.contains_active(&[64]));
        assert!(model.contains_active(&[UNNUMBERED_ID_BASE]));
        assert!(!model.contains_active(&[7]));
        assert_eq!(model.active_ids().collect::<Vec<_>>(), [64, UNNUMBERED_ID_BASE]);
        assert_eq!(model.active_mask(), 0);
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
    /// The compositor reports this window as minimized, not covering its output.
    #[serde(default)]
    pub minimized: bool,
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
