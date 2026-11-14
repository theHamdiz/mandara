//! Effective grants and one revision of a user's override file.

use std::path::PathBuf;
use mandara_theme::templates::FileStamp;
use serde::Serialize;

use crate::app_install::flatpak::Permissions;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub installation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Key { Shared, Sockets, Devices, Filesystems }
impl Key {
    pub fn name(self) -> &'static str {
        match self { Self::Shared => "shared", Self::Sockets => "sockets", Self::Devices => "devices", Self::Filesystems => "filesystems" }
    }
    pub(super) fn flag(self, allowed: bool) -> &'static str {
        match (self, allowed) {
            (Self::Shared, true) => "--share", (Self::Shared, false) => "--unshare",
            (Self::Sockets, true) => "--socket", (Self::Sockets, false) => "--nosocket",
            (Self::Devices, true) => "--device", (Self::Devices, false) => "--nodevice",
            (Self::Filesystems, true) => "--filesystem", (Self::Filesystems, false) => "--nofilesystem",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Capability {
    pub key: Key,
    pub value: String,
    pub allowed: bool,
    pub editable: bool,
    pub constraint: Option<Constraint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Constraint { Scoped, Conditional, Broader }

#[derive(Clone, Debug)]
pub struct Session {
    pub app: App,
    pub permissions: Permissions,
    pub capabilities: Vec<Capability>,
    /// Includes non-Context groups so advanced grants stay visible.
    pub metadata: String,
    pub(super) path: PathBuf,
    pub(super) target: PathBuf,
    pub(super) before: String,
    pub(super) stamp: Option<FileStamp>,
}

pub(super) fn capabilities(permissions: &Permissions, domain: &Permissions) -> Vec<Capability> {
    let mut rows = Vec::new();
    for (key, effective, known, common) in [
        (Key::Shared, &permissions.shared, &domain.shared, &["network", "ipc"][..]),
        (Key::Sockets, &permissions.sockets, &domain.sockets, &["wayland", "x11", "pulseaudio", "ssh-auth"][..]),
        (Key::Devices, &permissions.devices, &domain.devices, &["dri", "all"][..]),
        (Key::Filesystems, &permissions.filesystems, &domain.filesystems, &["home", "host"][..]),
    ] {
        for value in known.iter().map(String::as_str).chain(common.iter().copied()) {
            if rows.iter().any(|row: &Capability| row.key == key && row.value == value) { continue; }
            let editable = !value.contains('=') && !value.contains(':')
                && match key {
                    Key::Shared => matches!(value, "network" | "ipc"),
                    Key::Sockets => matches!(value, "wayland" | "x11" | "fallback-x11" | "pulseaudio" | "session-bus" | "system-bus" | "ssh-auth" | "pcsc" | "cups" | "gpg-agent" | "inherit-wayland-socket"),
                    Key::Devices => matches!(value, "dri" | "all" | "kvm" | "shm" | "input" | "usb"),
                    Key::Filesystems => matches!(value, "home" | "host" | "host-os" | "host-etc" | "xdg-desktop" | "xdg-documents" | "xdg-download" | "xdg-music" | "xdg-pictures" | "xdg-videos"),
                };
            let conditional = effective.iter().any(|grant| grant.strip_prefix("if:").and_then(|grant| grant.split_once(':')).is_some_and(|(permission, _)| permission == value));
            let scoped = value.contains(':') || value.contains('=') || effective.iter().any(|grant| grant.strip_prefix(value).is_some_and(|suffix| matches!(suffix, ":ro" | ":rw" | ":create")));
            let broader = match key {
                Key::Devices => !matches!(value, "all" | "shm") && effective.iter().any(|grant| grant == "all" || grant.starts_with("if:all:")),
                Key::Filesystems => editable && (effective.iter().any(|grant| matches!(grant.as_str(), "host-root" | "host-root:ro" | "host-root:rw" | "host-root:create"))
                    || matches!(value, "home" | "host-os" | "host-etc" | "xdg-desktop" | "xdg-documents" | "xdg-download" | "xdg-music" | "xdg-pictures" | "xdg-videos") && effective.iter().any(|grant| matches!(grant.as_str(), "host" | "host:ro" | "host:rw" | "host:create"))),
                _ => false,
            };
            let fallback = key == Key::Sockets && matches!(value, "x11" | "fallback-x11" | "wayland")
                && effective.iter().any(|grant| grant == "fallback-x11" || grant.strip_prefix("if:")
                    .and_then(|grant| grant.split_once(':')).is_some_and(|(permission, _)| matches!(permission, "x11" | "fallback-x11" | "wayland")));
            let constraint = if conditional || fallback || value.starts_with("if:") { Some(Constraint::Conditional) }
                else if broader { Some(Constraint::Broader) }
                else if scoped || !editable { Some(Constraint::Scoped) } else { None };
            rows.push(Capability { key, value: value.into(), allowed: !conditional && effective.iter().any(|grant| grant == value), editable: editable && constraint.is_none(), constraint });
        }
    }
    rows
}
