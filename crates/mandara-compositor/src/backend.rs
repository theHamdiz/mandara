//! The compositor abstraction every Mandara surface talks to.

use std::path::PathBuf;

use anyhow::Result;
use mandara_config::compositor::{BorderColors, CompositorConfig, Emitted, ManagedConfig, Target, emit, existing_chords, write_generated};

use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel};

pub trait CompositorBackend: Send + Sync {
    fn kind(&self) -> CompositorKind;

    /// `(major, minor)` of the running compositor, where it reports one
    /// (gates version-specific config such as Niri 26.04's blur).
    fn version(&self) -> Option<(u32, u32)> {
        None
    }

    /// Starts persistent event streams.
    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent>;

    /// Navigation state per monitor name.
    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>>;
    fn get_active_client(&self) -> Result<Option<ClientInfo>>;
    fn get_all_clients(&self) -> Result<Vec<ClientInfo>>;
    fn get_monitors(&self) -> Result<Vec<MonitorInfo>>;

    /// Focus a workspace, or view a single MangoWM tag.
    fn select_primary(&self, monitor: &str, id: u32) -> Result<()>;
    /// MangoWM `toggleview` (composite views); focus elsewhere.
    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()>;
    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()>;
    /// MangoWM `toggletag`; plain move elsewhere.
    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()>;
    fn cycle_layout(&self, monitor: &str) -> Result<()>;
    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()>;
    fn reload_config(&self) -> Result<()>;

    /// Switches to the next keyboard layout. MangoWM's dispatcher by
    /// default; Niri and Hyprland override it with their own requests.
    fn switch_keyboard_layout(&self) -> Result<()> {
        self.dispatch_raw("switch_keyboard_layout", &[])
    }

    /// Applies a cursor theme / size to the cursor the compositor is drawing
    /// right now. Nothing by default: MangoWM and Niri re-read the drop-in's
    /// cursor lines when they reload; Hyprland only reads its `env` lines
    /// for apps started later, so it overrides this with `setcursor`.
    fn apply_cursor(&self, _theme: Option<&str>, _size: Option<u32>) -> Result<()> {
        Ok(())
    }

    /// Config dialect this compositor reads.
    fn target(&self) -> Target {
        match self.kind() {
            CompositorKind::MangoWM => Target::Mango,
            CompositorKind::Niri => Target::Niri,
            CompositorKind::Hyprland => Target::Hyprland,
        }
    }

    /// Emits and writes the generated drop-in, skipping binds the user
    /// already owns. Returns the file and the emitter report.
    fn sync_managed_config(&self, config: &CompositorConfig, borders: Option<BorderColors>) -> Result<(PathBuf, Emitted)> {
        let target = self.target();
        let mandara_bin = mandara_binary();
        let managed = ManagedConfig { config, borders, existing_chords: existing_chords(target), mandara_bin, version: self.version(), user_autostarts: mandara_config::compositor::user_autostarts(target) };
        let emitted = emit(target, &managed);
        let path = write_generated(target, &emitted)?;
        Ok((path, emitted))
    }
}

/// The command keybinds use to reach Mandara. An installed `mandara` on
/// `PATH` is preferred (it survives upgrades); otherwise the running
/// binary's path — minus the ` (deleted)` suffix Linux appends once the file
/// has been replaced by a rebuild — shell-quoted when needed.
pub fn mandara_binary() -> String {
    // Always an absolute path: the compositor starts the shell (and runs
    // its binds) with the login session's PATH, which often lacks
    // ~/.local/bin even when this shell's PATH has it.
    let on_path = std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join("mandara")).find(|p| p.is_file() && p.is_absolute()));
    let exe = on_path.unwrap_or_else(mandara_config::paths::self_exe).display().to_string();
    if exe == "mandara" {
        return exe;
    }
    if exe.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        exe
    } else {
        format!("'{}'", exe.replace('\'', "'\\''"))
    }
}
