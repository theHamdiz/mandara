//! Paired devices through KDE Connect's session-bus interfaces. The daemon
//! handles discovery, encryption and transfers; Mandara never opens a phone
//! connection of its own.

mod dbus;

use mandara_i18n::t;
use serde::Serialize;

pub use dbus::KdeConnect;

/// Clipboard sharing is for a short message or link, not a document dump.
pub const MAX_SHARED_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub reachable: bool,
    pub paired: bool,
    pub incoming_pair: bool,
    pub battery: Option<Battery>,
    pub plugins: Vec<String>,
}

impl Device {
    pub fn connected(&self) -> bool { self.paired && self.reachable }

    pub fn supports(&self, plugin: &str) -> bool {
        self.connected() && self.plugins.iter().any(|id| id == plugin)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Ring,
    Ping,
    ShareText(String),
    Pair,
    AcceptPair,
    RejectPair,
    Unpair,
}

impl Action {
    /// Gate actions against a fresh device state, including IPC callers.
    pub fn validate(&self, device: &Device) -> Result<(), String> {
        if let Self::ShareText(text) = self && text.len() > MAX_SHARED_TEXT_BYTES {
            return Err(t("This text is too large to send to a phone").into());
        }
        match self {
            Self::Ring if device.supports("kdeconnect_findmyphone") => Ok(()),
            Self::Ping if device.supports("kdeconnect_ping") => Ok(()),
            Self::ShareText(text) if device.supports("kdeconnect_share") && !text.trim().is_empty() => Ok(()),
            Self::Pair if device.reachable && !device.paired => Ok(()),
            Self::AcceptPair | Self::RejectPair if device.incoming_pair && !device.paired => Ok(()),
            Self::Unpair if device.paired => Ok(()),
            Self::ShareText(text) if text.trim().is_empty() => Err(t("There is no text to send").into()),
            Self::Ring | Self::Ping | Self::ShareText(_) if !device.connected() => Err(t("This phone is not connected").into()),
            _ => Err(t("This phone does not support that action right now").into()),
        }
    }
}

/// Implementations perform their I/O off the UI thread.
pub trait Backend {
    fn devices(&self) -> Result<Vec<Device>, String>;
    fn act(&self, id: &str, action: Action) -> Result<(), String>;
}

#[cfg(test)]
mod tests;
