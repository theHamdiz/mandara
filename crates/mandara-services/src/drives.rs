//! Storage & removable media through UDisks2 (`org.freedesktop.UDisks2`,
//! system bus).
//!
//! The actor reads the whole object tree with `ObjectManager.GetManagedObjects`
//! and re-reads it (debounced) whenever UDisks2 emits a signal — interfaces
//! added / removed when a drive is plugged or unplugged, property changes
//! when something is mounted, unlocked or ejected. Usage figures come from
//! `statvfs` on each mount point and are refreshed on demand.
//!
//! Mutations (mount, unmount, unlock, lock, eject, power off) are built as
//! pure [`CallPlan`]s and executed through the [`Caller`] trait, so the
//! sequencing ("safely remove" = unmount every filesystem → lock unlocked
//! LUKS containers → eject or power off) is unit-tested with a recording
//! mock. Real calls carry `auth.no_user_interaction = false` and the D-Bus
//! `ALLOW_INTERACTIVE_AUTHORIZATION` flag, so polkit can prompt through the
//! shell's own agent when the action needs administrator rights.
//!
//! The actor runs on its own thread and exits once every command handle has
//! been dropped (the drives chip and panel are gone).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use futures::{FutureExt as _, StreamExt as _, select};
use mandara_i18n::{t, tf};
use serde::Serialize;
use zbus::proxy::MethodFlags;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use crate::actor::{Envelope, ServiceHandle, proxy, reply};

pub const UDISKS: &str = "org.freedesktop.UDisks2";
pub const UDISKS_ROOT: &str = "/org/freedesktop/UDisks2";
pub const IFACE_BLOCK: &str = "org.freedesktop.UDisks2.Block";
pub const IFACE_FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
pub const IFACE_ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
pub const IFACE_PARTITION: &str = "org.freedesktop.UDisks2.Partition";
pub const IFACE_PARTITION_TABLE: &str = "org.freedesktop.UDisks2.PartitionTable";
pub const IFACE_DRIVE: &str = "org.freedesktop.UDisks2.Drive";

/// Signals arriving within this window are folded into one re-read (a plug
/// event emits a burst of InterfacesAdded / PropertiesChanged).
const DEBOUNCE: Duration = Duration::from_millis(300);
/// After an unlock, UDisks needs a moment to publish the cleartext
/// device's filesystem before it can be mounted.
const CLEARTEXT_SETTLE: Duration = Duration::from_millis(600);
/// Signals buffered while a long call (an auth prompt) is in flight.
const SIGNAL_QUEUE: usize = 128;
/// Updates buffered for the UI.
const UPDATE_QUEUE: usize = 8;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 32;

/// GPT / MBR partition types of boot plumbing (EFI system, BIOS boot,
/// Microsoft reserved), hidden unless "show system partitions" is on.
const SYSTEM_PARTITION_TYPES: [&str; 4] =
    ["c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "21686148-6449-6e6f-744e-656564454649", "e3c9e316-0b5c-4db8-817d-f92df00215ae", "0xef"];
/// Mount points that belong to the boot chain.
const SYSTEM_MOUNT_PREFIXES: [&str; 2] = ["/boot", "/efi"];

// ── Model ────────────────────────────────────────────────────────────────

/// A filesystem-bearing block device (a partition, a whole unpartitioned
/// medium, or the cleartext side of an unlocked LUKS container).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Volume {
    pub object_path: String,
    /// `/dev/sdb1`.
    pub device: String,
    pub label: String,
    /// `ext4`, `btrfs`, `vfat`, `crypto_LUKS`…
    pub fs_type: String,
    pub uuid: String,
    pub size: u64,
    pub read_only: bool,
    /// Carries a filesystem UDisks can mount.
    pub has_filesystem: bool,
    pub mount_points: Vec<String>,
    /// `(used, total)` bytes from statvfs of the first mount point.
    pub usage: Option<(u64, u64)>,
    /// A LUKS container.
    pub encrypted: bool,
    /// The unlocked cleartext device of a LUKS container.
    pub cleartext: Option<Box<Volume>>,
    /// Boot plumbing (EFI, BIOS boot, /boot…).
    pub system: bool,
    pub partition_number: Option<u32>,
}

impl Volume {
    /// The volume that actually holds the files: the cleartext side of an
    /// unlocked container, else itself.
    pub fn effective(&self) -> &Volume {
        self.cleartext.as_deref().unwrap_or(self)
    }

    pub fn locked(&self) -> bool {
        self.encrypted && self.cleartext.is_none()
    }

    pub fn mounted(&self) -> bool {
        !self.effective().mount_points.is_empty()
    }

    /// The most meaningful mount point (`/` first, then the shortest).
    pub fn primary_mount(&self) -> Option<&str> {
        let mounts = &self.effective().mount_points;
        mounts.iter().min_by_key(|m| (m.as_str() != "/", m.len())).map(String::as_str)
    }

    /// `sdb1` from `/dev/sdb1`.
    pub fn device_name(&self) -> &str {
        self.device.rsplit('/').next().unwrap_or(&self.device)
    }

    fn matches(&self, query: &str) -> bool {
        let q = query.trim();
        !q.is_empty()
            && (self.object_path == q
                || self.device == q
                || self.device_name() == q.trim_start_matches("/dev/")
                || (!self.uuid.is_empty() && self.uuid.eq_ignore_ascii_case(q))
                || (!self.label.is_empty() && self.label.eq_ignore_ascii_case(q)))
    }
}

/// A physical drive with its volumes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Drive {
    pub object_path: String,
    /// Whole-disk block device (`/dev/sdb`), when UDisks exposes one.
    pub device: String,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    /// `usb`, `sdio`, `ieee1394` or empty (internal).
    pub bus: String,
    pub size: u64,
    pub removable: bool,
    pub media_removable: bool,
    pub ejectable: bool,
    pub can_power_off: bool,
    /// `0` = solid state, `> 0` = spinning (rpm), `-1` = unknown.
    pub rotation_rate: i32,
    pub optical: bool,
    pub volumes: Vec<Volume>,
}

impl Drive {
    /// Plugged in rather than built in.
    pub fn is_removable(&self) -> bool {
        self.removable || self.media_removable || self.optical || matches!(self.bus.as_str(), "usb" | "sdio" | "ieee1394")
    }

    /// `Vendor Model`, without a vendor repeated in the model.
    pub fn display_name(&self) -> String {
        let model = self.model.replace('_', " ").trim().to_string();
        let vendor = self.vendor.trim();
        match (vendor.is_empty(), model.is_empty()) {
            (_, true) if !vendor.is_empty() => vendor.to_string(),
            (true, _) => model,
            _ if model.to_lowercase().starts_with(&vendor.to_lowercase()) => model,
            _ => format!("{vendor} {model}"),
        }
    }

    pub fn device_name(&self) -> &str {
        self.device.rsplit('/').next().unwrap_or(&self.device)
    }

    fn matches(&self, query: &str) -> bool {
        let q = query.trim();
        !q.is_empty()
            && (self.object_path == q
                || (!self.device.is_empty() && (self.device == q || self.device_name() == q.trim_start_matches("/dev/")))
                || (!self.serial.is_empty() && self.serial == q)
                || self.display_name().eq_ignore_ascii_case(q))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DrivesState {
    /// The UDisks2 tree was read at least once.
    pub available: bool,
    pub error: Option<String>,
    pub drives: Vec<Drive>,
    /// Bumped each time a new drive appears after the initial read.
    pub plug_seq: u64,
    /// Name of the most recently plugged drive.
    pub last_plugged: Option<String>,
}

impl DrivesState {
    pub fn removable(&self) -> impl Iterator<Item = &Drive> {
        self.drives.iter().filter(|d| d.is_removable())
    }

    pub fn find_volume(&self, query: &str) -> Option<(&Drive, &Volume)> {
        self.drives.iter().find_map(|d| d.volumes.iter().find(|v| v.matches(query) || v.cleartext.as_deref().is_some_and(|c| c.matches(query))).map(|v| (d, v)))
    }

    /// A drive by its own identifiers, or the drive holding a volume.
    pub fn find_drive(&self, query: &str) -> Option<&Drive> {
        self.drives.iter().find(|d| d.matches(query)).or_else(|| self.find_volume(query).map(|(d, _)| d))
    }
}

/// Commands for the drives actor. Devices are queries: an object path, a
/// device node (`/dev/sdb1` or `sdb1`), a label or a UUID (drives also
/// match their model name).
#[derive(Clone, PartialEq)]
pub enum DrivesCommand {
    /// Re-read the tree and usage.
    Refresh,
    /// Replies with the current state as JSON.
    List,
    Mount(String),
    Unmount(String),
    Unlock { device: String, passphrase: String },
    Lock(String),
    /// Unmount everything on the drive, lock it, then eject / power off.
    SafelyRemove(String),
    /// Opens the mount point in the file manager (mounting first if needed).
    Open(String),
}

impl fmt::Debug for DrivesCommand {
    // Never print a passphrase.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DrivesCommand::Unlock { device, .. } => f.debug_struct("Unlock").field("device", device).field("passphrase", &"<redacted>").finish(),
            DrivesCommand::Refresh => f.write_str("Refresh"),
            DrivesCommand::List => f.write_str("List"),
            DrivesCommand::Mount(d) => f.debug_tuple("Mount").field(d).finish(),
            DrivesCommand::Unmount(d) => f.debug_tuple("Unmount").field(d).finish(),
            DrivesCommand::Lock(d) => f.debug_tuple("Lock").field(d).finish(),
            DrivesCommand::SafelyRemove(d) => f.debug_tuple("SafelyRemove").field(d).finish(),
            DrivesCommand::Open(d) => f.debug_tuple("Open").field(d).finish(),
        }
    }
}

pub type DrivesService = ServiceHandle<DrivesState, DrivesCommand>;

/// Overwrites a secret's bytes before it is dropped.
pub fn wipe(secret: &mut String) {
    // SAFETY: zero bytes are valid UTF-8, so the string stays well-formed.
    unsafe { secret.as_bytes_mut().fill(0) };
    secret.clear();
}

// ── Raw D-Bus records and assembly (pure, tested) ────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawBlock {
    pub path: String,
    pub device: String,
    pub size: u64,
    pub label: String,
    pub hint_name: String,
    pub id_type: String,
    pub id_usage: String,
    pub uuid: String,
    pub hint_ignore: bool,
    pub read_only: bool,
    pub drive: Option<String>,
    pub crypto_backing: Option<String>,
    pub has_filesystem: bool,
    pub mount_points: Vec<String>,
    pub encrypted: bool,
    pub cleartext: Option<String>,
    pub partition_number: Option<u32>,
    pub partition_type: Option<String>,
    pub is_partition_table: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawDrive {
    pub path: String,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub bus: String,
    pub size: u64,
    pub removable: bool,
    pub media_removable: bool,
    pub ejectable: bool,
    pub can_power_off: bool,
    pub rotation_rate: i32,
    pub optical: bool,
}

fn volume_from(b: &RawBlock) -> Volume {
    let system = b.hint_ignore
        || b.partition_type.as_deref().is_some_and(|t| SYSTEM_PARTITION_TYPES.contains(&t.to_lowercase().as_str()))
        || b.mount_points.iter().any(|m| SYSTEM_MOUNT_PREFIXES.iter().any(|p| m == p || m.starts_with(&format!("{p}/"))));
    Volume {
        object_path: b.path.clone(),
        device: b.device.clone(),
        label: if b.label.is_empty() { b.hint_name.clone() } else { b.label.clone() },
        fs_type: b.id_type.clone(),
        uuid: b.uuid.clone(),
        size: b.size,
        read_only: b.read_only,
        has_filesystem: b.has_filesystem,
        mount_points: b.mount_points.clone(),
        usage: None,
        encrypted: b.encrypted,
        cleartext: None,
        system,
        partition_number: b.partition_number,
    }
}

/// Builds drives with their volumes: filesystem and LUKS blocks attached
/// to their drive, unlocked containers carrying their cleartext volume.
/// Loop devices, zram and swap are left out (blocks UDisks marks "ignore",
/// like EFI partitions, count as system volumes); internal
/// drives without a visible volume are dropped, removable ones kept (an
/// empty card reader, an unformatted stick).
pub fn assemble(blocks: &[RawBlock], drives: &[RawDrive]) -> Vec<Drive> {
    let by_path: HashMap<&str, &RawBlock> = blocks.iter().map(|b| (b.path.as_str(), b)).collect();
    let mut out: Vec<Drive> = drives
        .iter()
        .map(|d| Drive {
            object_path: d.path.clone(),
            device: blocks.iter().find(|b| b.drive.as_deref() == Some(d.path.as_str()) && b.partition_number.is_none()).map(|b| b.device.clone()).unwrap_or_default(),
            vendor: d.vendor.clone(),
            model: d.model.clone(),
            serial: d.serial.clone(),
            bus: d.bus.clone(),
            size: d.size,
            removable: d.removable,
            media_removable: d.media_removable,
            ejectable: d.ejectable,
            can_power_off: d.can_power_off,
            rotation_rate: d.rotation_rate,
            optical: d.optical,
            volumes: Vec::new(),
        })
        .collect();
    for b in blocks {
        let Some(drive) = b.drive.as_deref() else { continue };
        if b.id_type == "swap" || !(b.has_filesystem || b.encrypted) {
            continue;
        }
        let Some(d) = out.iter_mut().find(|d| d.object_path == drive) else { continue };
        let mut v = volume_from(b);
        if let Some(clear) = b.cleartext.as_deref().and_then(|p| by_path.get(p)) {
            let mut inner = volume_from(clear);
            inner.system |= v.system;
            v.system = inner.system;
            v.cleartext = Some(Box::new(inner));
        }
        d.volumes.push(v);
    }
    for d in &mut out {
        d.volumes.sort_by_key(|v| (v.partition_number.unwrap_or(0), v.device.clone()));
    }
    out.retain(|d| d.is_removable() || !d.volumes.is_empty());
    // Internal drives first (by device), then removable ones.
    out.sort_by(|a, b| a.is_removable().cmp(&b.is_removable()).then(a.device.cmp(&b.device)).then(a.object_path.cmp(&b.object_path)));
    out
}

/// Drive object paths that are new compared with `before`.
pub fn newly_plugged<'a>(before: &HashSet<String>, after: &'a [Drive]) -> Vec<&'a Drive> {
    after.iter().filter(|d| !before.contains(&d.object_path)).collect()
}

// ── Call plans ───────────────────────────────────────────────────────────

/// One UDisks2 method call. Every call passes
/// `{"auth.no_user_interaction": false}` as its options.
#[derive(Clone, PartialEq, Eq)]
pub struct CallPlan {
    pub path: String,
    pub interface: &'static str,
    pub method: &'static str,
    /// `Encrypted.Unlock` only.
    pub passphrase: Option<String>,
    /// Sent with the D-Bus ALLOW_INTERACTIVE_AUTHORIZATION flag.
    pub interactive: bool,
}

impl fmt::Debug for CallPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallPlan")
            .field("path", &self.path)
            .field("method", &format_args!("{}.{}", self.interface, self.method))
            .field("passphrase", &self.passphrase.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl CallPlan {
    fn new(path: &str, interface: &'static str, method: &'static str) -> Self {
        Self { path: path.to_string(), interface, method, passphrase: None, interactive: true }
    }

    pub fn mount(block: &str) -> Self {
        Self::new(block, IFACE_FILESYSTEM, "Mount")
    }

    pub fn unmount(block: &str) -> Self {
        Self::new(block, IFACE_FILESYSTEM, "Unmount")
    }

    pub fn unlock(block: &str, passphrase: String) -> Self {
        let mut plan = Self::new(block, IFACE_ENCRYPTED, "Unlock");
        plan.passphrase = Some(passphrase);
        plan
    }

    pub fn lock(block: &str) -> Self {
        Self::new(block, IFACE_ENCRYPTED, "Lock")
    }

    pub fn eject(drive: &str) -> Self {
        Self::new(drive, IFACE_DRIVE, "Eject")
    }

    pub fn power_off(drive: &str) -> Self {
        Self::new(drive, IFACE_DRIVE, "PowerOff")
    }

    /// The options dictionary sent with the call.
    pub fn options(&self) -> HashMap<&'static str, bool> {
        HashMap::from([("auth.no_user_interaction", !self.interactive)])
    }
}

impl Drop for CallPlan {
    fn drop(&mut self) {
        if let Some(p) = self.passphrase.as_mut() {
            wipe(p);
        }
    }
}

/// Unmounts a volume (or its cleartext side).
pub fn plan_unmount(v: &Volume) -> Result<Vec<CallPlan>, String> {
    let target = v.effective();
    if target.mount_points.is_empty() {
        return Err(tf("{device} is not mounted", &[("device", &v.device)]));
    }
    Ok(vec![CallPlan::unmount(&target.object_path)])
}

/// Mounts a volume (the cleartext side of an unlocked container).
pub fn plan_mount(v: &Volume) -> Result<Vec<CallPlan>, String> {
    if v.locked() {
        return Err(tf("{device} is encrypted — unlock it first", &[("device", &v.device)]));
    }
    let target = v.effective();
    if !target.has_filesystem {
        return Err(tf("{device} has no filesystem to mount", &[("device", &v.device)]));
    }
    if !target.mount_points.is_empty() {
        return Err(tf("{device} is already mounted", &[("device", &v.device)]));
    }
    Ok(vec![CallPlan::mount(&target.object_path)])
}

/// Locks an unlocked container, unmounting its cleartext side first.
pub fn plan_lock(v: &Volume) -> Result<Vec<CallPlan>, String> {
    let Some(clear) = v.cleartext.as_deref() else {
        return Err(if v.encrypted { tf("{device} is already locked", &[("device", &v.device)]) } else { tf("{device} is not encrypted", &[("device", &v.device)]) });
    };
    let mut plans = Vec::new();
    if !clear.mount_points.is_empty() {
        plans.push(CallPlan::unmount(&clear.object_path));
    }
    plans.push(CallPlan::lock(&v.object_path));
    Ok(plans)
}

/// "Safely remove": unmount every mounted filesystem, lock unlocked LUKS
/// containers, then eject removable media (card readers, optical drives)
/// or power the drive off (USB sticks and disks).
pub fn plan_safely_remove(d: &Drive) -> Vec<CallPlan> {
    let mut plans = Vec::new();
    for v in &d.volumes {
        match v.cleartext.as_deref() {
            Some(clear) => {
                if !clear.mount_points.is_empty() {
                    plans.push(CallPlan::unmount(&clear.object_path));
                }
                plans.push(CallPlan::lock(&v.object_path));
            }
            None if !v.mount_points.is_empty() => plans.push(CallPlan::unmount(&v.object_path)),
            None => {}
        }
    }
    if d.media_removable && d.ejectable {
        plans.push(CallPlan::eject(&d.object_path));
    } else if d.can_power_off {
        plans.push(CallPlan::power_off(&d.object_path));
    } else if d.ejectable {
        plans.push(CallPlan::eject(&d.object_path));
    }
    plans
}

/// Executes UDisks2 calls. Returns the method's string result (a mount
/// point or a cleartext object path), empty for calls without one.
#[allow(async_fn_in_trait)]
pub trait Caller {
    async fn call(&mut self, plan: &CallPlan) -> Result<String, String>;
}

/// Runs `plans` in order, stopping at the first failure. Returns each
/// call's result.
pub async fn run_plans<C: Caller>(caller: &mut C, plans: &[CallPlan]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(plans.len());
    for plan in plans {
        out.push(caller.call(plan).await?);
    }
    Ok(out)
}

/// A human message for a UDisks2 / polkit D-Bus error.
pub fn friendly_error(name: &str, message: &str) -> String {
    let short = name.rsplit('.').next().unwrap_or(name);
    let lower = message.to_lowercase();
    match short {
        "NotAuthorizedDismissed" => t("Authentication was cancelled").to_string(),
        "NotAuthorized" | "NotAuthorizedCanObtain" | "AccessDenied" => t("Not authorized to do that").to_string(),
        "DeviceBusy" => t("The drive is busy — close files and apps that use it").to_string(),
        "Cancelled" => t("Cancelled").to_string(),
        _ if lower.contains("target is busy") || lower.contains("device is busy") => t("The drive is busy — close files and apps that use it").to_string(),
        _ if lower.contains("no key available") || lower.contains("operation not permitted") && lower.contains("unlock") => t("Wrong passphrase").to_string(),
        _ if lower.contains("not authorized") => t("Not authorized to do that").to_string(),
        _ if !message.is_empty() => message.to_string(),
        _ => short.to_string(),
    }
}

/// `file://` URI for a local path (percent-encoding everything but
/// unreserved characters and `/`).
pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ── D-Bus ────────────────────────────────────────────────────────────────

type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn prop<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    props.get(key).and_then(|v| T::try_from(v.try_clone().ok()?).ok())
}

/// A NUL-terminated byte-string property (`ay`).
fn bytes_prop(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    let raw: Vec<u8> = prop(props, key).unwrap_or_default();
    String::from_utf8_lossy(raw.split(|b| *b == 0).next().unwrap_or_default()).into_owned()
}

fn path_prop(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    prop::<OwnedObjectPath>(props, key).map(|p| p.to_string()).filter(|p| p != "/")
}

fn raw_records(objects: &Managed) -> (Vec<RawBlock>, Vec<RawDrive>) {
    let mut blocks = Vec::new();
    let mut drives = Vec::new();
    for (path, ifaces) in objects {
        if let Some(b) = ifaces.get(IFACE_BLOCK) {
            let fs = ifaces.get(IFACE_FILESYSTEM);
            let mount_points = fs
                .and_then(|fs| prop::<Vec<Vec<u8>>>(fs, "MountPoints"))
                .unwrap_or_default()
                .into_iter()
                .map(|m| String::from_utf8_lossy(m.split(|b| *b == 0).next().unwrap_or_default()).into_owned())
                .filter(|m| !m.is_empty())
                .collect();
            let enc = ifaces.get(IFACE_ENCRYPTED);
            let part = ifaces.get(IFACE_PARTITION);
            let crypto_backing = path_prop(b, "CryptoBackingDevice");
            // Cleartext devices have no drive: they hang off their container.
            let drive = path_prop(b, "Drive");
            blocks.push(RawBlock {
                path: path.to_string(),
                device: bytes_prop(b, "Device"),
                size: prop(b, "Size").unwrap_or(0),
                label: prop(b, "IdLabel").unwrap_or_default(),
                hint_name: prop(b, "HintName").unwrap_or_default(),
                id_type: prop(b, "IdType").unwrap_or_default(),
                id_usage: prop(b, "IdUsage").unwrap_or_default(),
                uuid: prop(b, "IdUUID").unwrap_or_default(),
                hint_ignore: prop(b, "HintIgnore").unwrap_or(false),
                read_only: prop(b, "ReadOnly").unwrap_or(false),
                drive,
                crypto_backing,
                has_filesystem: fs.is_some(),
                mount_points,
                encrypted: enc.is_some(),
                cleartext: enc.and_then(|e| path_prop(e, "CleartextDevice")),
                partition_number: part.and_then(|p| prop(p, "Number")),
                partition_type: part.and_then(|p| prop(p, "Type")),
                is_partition_table: ifaces.contains_key(IFACE_PARTITION_TABLE),
            });
        }
        if let Some(d) = ifaces.get(IFACE_DRIVE) {
            let media: Vec<String> = prop(d, "MediaCompatibility").unwrap_or_default();
            drives.push(RawDrive {
                path: path.to_string(),
                vendor: prop(d, "Vendor").unwrap_or_default(),
                model: prop(d, "Model").unwrap_or_default(),
                serial: prop(d, "Serial").unwrap_or_default(),
                bus: prop(d, "ConnectionBus").unwrap_or_default(),
                size: prop(d, "Size").unwrap_or(0),
                removable: prop(d, "Removable").unwrap_or(false),
                media_removable: prop(d, "MediaRemovable").unwrap_or(false),
                ejectable: prop(d, "Ejectable").unwrap_or(false),
                can_power_off: prop(d, "CanPowerOff").unwrap_or(false),
                rotation_rate: prop(d, "RotationRate").unwrap_or(-1),
                optical: media.iter().any(|m| m.starts_with("optical")),
            });
        }
    }
    (blocks, drives)
}

/// Fills `(used, total)` from statvfs on each mounted volume.
pub fn fill_usage(drives: &mut [Drive]) {
    fn usage(mount: &str) -> Option<(u64, u64)> {
        let s = rustix::fs::statvfs(mount).ok()?;
        let total = s.f_blocks.saturating_mul(s.f_frsize);
        let free = s.f_bavail.saturating_mul(s.f_frsize);
        (total > 0).then(|| (total.saturating_sub(free), total))
    }
    fn fill(v: &mut Volume) {
        v.usage = v.primary_mount().map(str::to_string).and_then(|m| usage(&m));
        if let Some(c) = v.cleartext.as_deref_mut() {
            c.usage = c.primary_mount().map(str::to_string).and_then(|m| usage(&m));
        }
    }
    for d in drives {
        d.volumes.iter_mut().for_each(fill);
    }
}

/// Reads the drives (with usage) once.
pub async fn read_drives(conn: &Connection) -> zbus::Result<Vec<Drive>> {
    let om = proxy(conn, UDISKS, UDISKS_ROOT, "org.freedesktop.DBus.ObjectManager").await?;
    let objects: Managed = om.call("GetManagedObjects", &()).await?;
    let (blocks, raw) = raw_records(&objects);
    let mut drives = assemble(&blocks, &raw);
    fill_usage(&mut drives);
    Ok(drives)
}

/// The real UDisks2 caller.
pub struct ZbusCaller<'a> {
    pub conn: &'a Connection,
}

impl Caller for ZbusCaller<'_> {
    async fn call(&mut self, plan: &CallPlan) -> Result<String, String> {
        let p = proxy(self.conn, UDISKS, &plan.path, plan.interface).await.map_err(|e| e.to_string())?;
        let flags = if plan.interactive { MethodFlags::AllowInteractiveAuth.into() } else { Default::default() };
        let options: HashMap<&str, Value> = plan.options().into_iter().map(|(k, v)| (k, Value::from(v))).collect();
        let result = match (plan.method, plan.passphrase.as_deref()) {
            ("Mount", _) => p.call_with_flags::<_, _, String>(plan.method, flags, &(options,)).await.map(Option::unwrap_or_default),
            ("Unlock", Some(pass)) => {
                p.call_with_flags::<_, _, OwnedObjectPath>(plan.method, flags, &(pass, options)).await.map(|o| o.map(|o| o.to_string()).unwrap_or_default())
            }
            _ => p.call_with_flags::<_, _, ()>(plan.method, flags, &(options,)).await.map(|_| String::new()),
        };
        result.map_err(|e| match e {
            zbus::Error::MethodError(name, msg, _) => friendly_error(name.as_str(), msg.as_deref().unwrap_or_default()),
            other => other.to_string(),
        })
    }
}

// ── Actor ────────────────────────────────────────────────────────────────

pub fn spawn_drives_service() -> DrivesService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded(COMMAND_QUEUE);
    crate::runtime::spawn("drives", async move {
        if let Err(e) = run(&tx, &rx).await {
            tracing::warn!("drives: {e}");
            let state = DrivesState { error: Some(e.to_string()), ..Default::default() };
            let _ = tx.try_send(state);
            // Still answer commands so callers get an error, not a hang.
            while let Ok(Envelope { reply: r, .. }) = rx.recv().await {
                reply(&r, Err(t("UDisks2 is not available").into()));
            }
        }
    });
    ServiceHandle::new(updates, commands)
}

struct Actor<'a> {
    conn: &'a Connection,
    state: DrivesState,
    known: HashSet<String>,
    tx: &'a async_channel::Sender<DrivesState>,
}

impl Actor<'_> {
    async fn reread(&mut self) {
        match read_drives(self.conn).await {
            Ok(drives) => {
                if self.state.available {
                    let fresh = newly_plugged(&self.known, &drives);
                    if let Some(d) = fresh.last() {
                        self.state.plug_seq += 1;
                        self.state.last_plugged = Some(d.display_name());
                    }
                }
                self.known = drives.iter().map(|d| d.object_path.clone()).collect();
                self.state.drives = drives;
                self.state.available = true;
                self.state.error = None;
            }
            Err(e) => self.state.error = Some(e.to_string()),
        }
        // The UI may be gone (CLI-only handle); never block on it.
        let _ = self.tx.try_send(self.state.clone());
    }

    fn volume(&self, q: &str) -> Result<Volume, String> {
        self.state.find_volume(q).map(|(_, v)| v.clone()).ok_or_else(|| tf("No volume matching \"{query}\"", &[("query", &q)]))
    }

    async fn execute(&mut self, command: DrivesCommand) -> Result<String, String> {
        let mut caller = ZbusCaller { conn: self.conn };
        match command {
            DrivesCommand::Refresh => Ok("ok".into()),
            DrivesCommand::List => serde_json::to_string_pretty(&self.state.drives).map_err(|e| e.to_string()),
            DrivesCommand::Mount(q) => {
                let v = self.volume(&q)?;
                let out = run_plans(&mut caller, &plan_mount(&v)?).await?;
                Ok(tf("Mounted {device} at {path}", &[("device", &v.device), ("path", &out.concat())]))
            }
            DrivesCommand::Unmount(q) => {
                let v = self.volume(&q)?;
                run_plans(&mut caller, &plan_unmount(&v)?).await?;
                Ok(tf("Unmounted {device}", &[("device", &v.device)]))
            }
            DrivesCommand::Unlock { device, mut passphrase } => {
                let v = match self.volume(&device) {
                    Ok(v) => v,
                    Err(e) => {
                        wipe(&mut passphrase);
                        return Err(e);
                    }
                };
                if !v.locked() {
                    wipe(&mut passphrase);
                    return Err(if v.encrypted { tf("{device} is already unlocked", &[("device", &v.device)]) } else { tf("{device} is not encrypted", &[("device", &v.device)]) });
                }
                let plan = CallPlan::unlock(&v.object_path, std::mem::take(&mut passphrase));
                let clear = run_plans(&mut caller, std::slice::from_ref(&plan)).await?.concat();
                drop(plan);
                // Mount the unlocked filesystem, as file managers do.
                async_io::Timer::after(CLEARTEXT_SETTLE).await;
                match caller.call(&CallPlan::mount(&clear)).await {
                    Ok(at) if !at.is_empty() => Ok(tf("Unlocked {device} and mounted it at {path}", &[("device", &v.device), ("path", &at)])),
                    _ => Ok(tf("Unlocked {device}", &[("device", &v.device)])),
                }
            }
            DrivesCommand::Lock(q) => {
                let v = self.volume(&q)?;
                run_plans(&mut caller, &plan_lock(&v)?).await?;
                Ok(tf("Locked {device}", &[("device", &v.device)]))
            }
            DrivesCommand::SafelyRemove(q) => {
                let d = self.state.find_drive(&q).cloned().ok_or_else(|| tf("No drive matching \"{query}\"", &[("query", &q)]))?;
                let plans = plan_safely_remove(&d);
                if plans.is_empty() {
                    return Err(tf("{drive} can't be ejected or powered off", &[("drive", &d.display_name())]));
                }
                run_plans(&mut caller, &plans).await?;
                Ok(tf("{drive} can be removed safely", &[("drive", &d.display_name())]))
            }
            DrivesCommand::Open(q) => {
                let v = self.volume(&q)?;
                let path = match v.primary_mount() {
                    Some(p) => p.to_string(),
                    None => run_plans(&mut caller, &plan_mount(&v)?).await?.concat(),
                };
                if path.is_empty() {
                    return Err(tf("{device} is not mounted", &[("device", &v.device)]));
                }
                crate::session::open_uri(&file_uri(&path))?;
                Ok(tf("Opened {path}", &[("path", &path)]))
            }
        }
    }
}

async fn run(tx: &async_channel::Sender<DrivesState>, rx: &async_channel::Receiver<Envelope<DrivesCommand>>) -> zbus::Result<()> {
    // Private connection: its signal stream must never stall other actors.
    let conn = crate::runtime::connect(zbus::connection::Builder::system()?).await?;
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(UDISKS)?.build();
    let mut signals = MessageStream::for_match_rule(rule, &conn, Some(SIGNAL_QUEUE)).await?.fuse();
    let mut actor = Actor { conn: &conn, state: DrivesState::default(), known: HashSet::new(), tx };
    // The first read must succeed: otherwise UDisks2 isn't there at all.
    let drives = read_drives(&conn).await?;
    actor.known = drives.iter().map(|d| d.object_path.clone()).collect();
    actor.state = DrivesState { available: true, drives, ..Default::default() };
    let _ = tx.try_send(actor.state.clone());
    loop {
        select! {
            _ = signals.next() => {
                // Fold the burst into one re-read.
                loop {
                    select! {
                        _ = signals.next() => {}
                        _ = futures::FutureExt::fuse(async_io::Timer::after(DEBOUNCE)) => break,
                    }
                }
                actor.reread().await;
            }
            cmd = rx.recv().fuse() => {
                let Ok(Envelope { command, reply: r }) = cmd else { return Ok(()) };
                let mutates = !matches!(command, DrivesCommand::List);
                if matches!(command, DrivesCommand::Refresh) {
                    actor.reread().await;
                }
                let result = actor.execute(command).await;
                if mutates && result.is_ok() {
                    actor.reread().await;
                }
                reply(&r, result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRIVE: &str = "/org/freedesktop/UDisks2/drives/Stick";

    fn stick() -> (Vec<RawBlock>, Vec<RawDrive>) {
        let drive = RawDrive { path: DRIVE.into(), vendor: "SanDisk".into(), model: "Ultra Fit".into(), bus: "usb".into(), removable: true, can_power_off: true, size: 64_000_000_000, ..Default::default() };
        let whole = RawBlock { path: "/b/sdb".into(), device: "/dev/sdb".into(), drive: Some(DRIVE.into()), is_partition_table: true, ..Default::default() };
        let data = RawBlock {
            path: "/b/sdb1".into(),
            device: "/dev/sdb1".into(),
            drive: Some(DRIVE.into()),
            has_filesystem: true,
            id_type: "exfat".into(),
            label: "PHOTOS".into(),
            mount_points: vec!["/run/media/me/PHOTOS".into()],
            partition_number: Some(1),
            ..Default::default()
        };
        let luks = RawBlock {
            path: "/b/sdb2".into(),
            device: "/dev/sdb2".into(),
            drive: Some(DRIVE.into()),
            encrypted: true,
            id_type: "crypto_LUKS".into(),
            cleartext: Some("/b/dm0".into()),
            partition_number: Some(2),
            ..Default::default()
        };
        let clear = RawBlock {
            path: "/b/dm0".into(),
            device: "/dev/dm-0".into(),
            crypto_backing: Some("/b/sdb2".into()),
            has_filesystem: true,
            id_type: "ext4".into(),
            mount_points: vec!["/run/media/me/vault".into()],
            ..Default::default()
        };
        let swap = RawBlock { path: "/b/sdb3".into(), device: "/dev/sdb3".into(), drive: Some(DRIVE.into()), id_type: "swap".into(), has_filesystem: false, partition_number: Some(3), ..Default::default() };
        let loopdev = RawBlock { path: "/b/loop0".into(), device: "/dev/loop0".into(), has_filesystem: true, id_type: "squashfs".into(), ..Default::default() };
        (vec![whole, data, luks, clear, swap, loopdev], vec![drive])
    }

    fn assembled() -> Drive {
        let (b, d) = stick();
        assemble(&b, &d).remove(0)
    }

    /// Records calls; fails on a chosen method.
    #[derive(Default)]
    struct Mock {
        calls: Vec<(String, String, bool, bool)>,
        fail_on: Option<&'static str>,
    }

    impl Caller for Mock {
        async fn call(&mut self, plan: &CallPlan) -> Result<String, String> {
            self.calls.push((plan.path.clone(), plan.method.to_string(), plan.interactive, plan.passphrase.is_some()));
            if self.fail_on == Some(plan.method) {
                return Err(friendly_error("org.freedesktop.UDisks2.Error.DeviceBusy", ""));
            }
            Ok(match plan.method {
                "Mount" => "/run/media/me/X".into(),
                "Unlock" => "/b/dm9".into(),
                _ => String::new(),
            })
        }
    }

    #[test]
    fn assembles_volumes_and_cleartext() {
        let d = assembled();
        assert_eq!(d.device, "/dev/sdb");
        assert!(d.is_removable());
        assert_eq!(d.display_name(), "SanDisk Ultra Fit");
        // Swap, the partition table and the loop device are left out.
        assert_eq!(d.volumes.iter().map(|v| v.device.as_str()).collect::<Vec<_>>(), ["/dev/sdb1", "/dev/sdb2"]);
        let luks = &d.volumes[1];
        assert!(luks.encrypted && !luks.locked() && luks.mounted());
        assert_eq!(luks.effective().fs_type, "ext4");
        assert_eq!(luks.primary_mount(), Some("/run/media/me/vault"));
    }

    #[test]
    fn finds_by_device_label_and_cleartext() {
        let state = DrivesState { available: true, drives: vec![assembled()], ..Default::default() };
        assert_eq!(state.find_volume("sdb1").unwrap().1.label, "PHOTOS");
        assert_eq!(state.find_volume("photos").unwrap().1.device, "/dev/sdb1");
        assert_eq!(state.find_volume("/dev/dm-0").unwrap().1.device, "/dev/sdb2");
        assert!(state.find_volume("sdz").is_none());
        assert_eq!(state.find_drive("sdb").unwrap().object_path, DRIVE);
        assert_eq!(state.find_drive("/dev/sdb1").unwrap().object_path, DRIVE);
        assert_eq!(state.removable().count(), 1);
    }

    #[test]
    fn system_partitions_are_flagged_and_internal_empties_dropped() {
        let drive = RawDrive { path: "/d/nvme".into(), model: "SSD".into(), rotation_rate: 0, ..Default::default() };
        let efi = RawBlock { path: "/b/p1".into(), device: "/dev/nvme0n1p1".into(), drive: Some("/d/nvme".into()), has_filesystem: true, partition_type: Some("C12A7328-F81F-11D2-BA4B-00A0C93EC93B".into()), partition_number: Some(1), ..Default::default() };
        let boot = RawBlock { path: "/b/p2".into(), device: "/dev/nvme0n1p2".into(), drive: Some("/d/nvme".into()), has_filesystem: true, mount_points: vec!["/boot".into()], partition_number: Some(2), ..Default::default() };
        let home = RawBlock { path: "/b/p3".into(), device: "/dev/nvme0n1p3".into(), drive: Some("/d/nvme".into()), has_filesystem: true, mount_points: vec!["/home".into(), "/".into()], partition_number: Some(3), ..Default::default() };
        let ignored = RawBlock { path: "/b/p4".into(), device: "/dev/nvme0n1p4".into(), drive: Some("/d/nvme".into()), has_filesystem: true, hint_ignore: true, partition_number: Some(4), ..Default::default() };
        let d = assemble(&[efi, boot, home, ignored], &[drive, RawDrive { path: "/d/empty".into(), ..Default::default() }]);
        assert_eq!(d.len(), 1, "internal drive without volumes is dropped");
        assert_eq!(d[0].volumes.iter().map(|v| v.system).collect::<Vec<_>>(), [true, true, false, true]);
        assert_eq!(d[0].volumes[2].primary_mount(), Some("/"));
    }

    #[test]
    fn plans_mount_unmount_lock() {
        let d = assembled();
        let (plain, luks) = (&d.volumes[0], &d.volumes[1]);
        assert!(plan_mount(plain).is_err(), "already mounted");
        let p = plan_unmount(plain).unwrap();
        assert_eq!((p[0].path.as_str(), p[0].interface, p[0].method), ("/b/sdb1", IFACE_FILESYSTEM, "Unmount"));
        assert_eq!(p[0].options(), HashMap::from([("auth.no_user_interaction", false)]));
        assert!(p[0].interactive);
        // Unmounting a container targets its cleartext side.
        assert_eq!(plan_unmount(luks).unwrap()[0].path, "/b/dm0");
        let lock = plan_lock(luks).unwrap();
        assert_eq!(lock.iter().map(|p| (p.path.as_str(), p.method)).collect::<Vec<_>>(), [("/b/dm0", "Unmount"), ("/b/sdb2", "Lock")]);
        assert!(plan_lock(plain).is_err());
        let mut locked = luks.clone();
        locked.cleartext = None;
        assert!(locked.locked());
        assert!(plan_mount(&locked).is_err(), "locked volumes need unlocking first");
        let unlock = CallPlan::unlock(&locked.object_path, "hunter2".into());
        assert_eq!((unlock.interface, unlock.method), (IFACE_ENCRYPTED, "Unlock"));
        assert!(!format!("{unlock:?}").contains("hunter2"), "passphrase is redacted");
        assert!(!format!("{:?}", DrivesCommand::Unlock { device: "x".into(), passphrase: "hunter2".into() }).contains("hunter2"));
    }

    #[test]
    fn safely_remove_sequence() {
        let d = assembled();
        let plans = plan_safely_remove(&d);
        assert_eq!(
            plans.iter().map(|p| (p.path.as_str(), p.method)).collect::<Vec<_>>(),
            [("/b/sdb1", "Unmount"), ("/b/dm0", "Unmount"), ("/b/sdb2", "Lock"), (DRIVE, "PowerOff")]
        );
        // A card reader ejects its medium instead.
        let reader = Drive { media_removable: true, ejectable: true, ..d.clone() };
        assert_eq!(plan_safely_remove(&reader).last().unwrap().method, "Eject");
        let mut mock = Mock::default();
        let out = zbus::block_on(run_plans(&mut mock, &plans)).unwrap();
        assert_eq!(out.len(), 4);
        assert!(mock.calls.iter().all(|c| c.2), "every call allows interactive auth");
    }

    #[test]
    fn run_plans_stops_at_first_failure() {
        let plans = plan_safely_remove(&assembled());
        let mut mock = Mock { fail_on: Some("Unmount"), ..Default::default() };
        let err = zbus::block_on(run_plans(&mut mock, &plans)).unwrap_err();
        assert_eq!(mock.calls.len(), 1, "nothing runs after the failed unmount");
        assert!(err.contains("busy"));
        let mut mock = Mock::default();
        let plan = CallPlan::unlock("/b/sdb2", "pw".into());
        assert_eq!(zbus::block_on(run_plans(&mut mock, std::slice::from_ref(&plan))).unwrap(), ["/b/dm9"]);
        assert!(mock.calls[0].3, "passphrase travels with the unlock call");
    }

    #[test]
    fn plug_detection_and_helpers() {
        let d = assembled();
        let before: HashSet<String> = HashSet::new();
        assert_eq!(newly_plugged(&before, std::slice::from_ref(&d)).len(), 1);
        let known: HashSet<String> = [DRIVE.to_string()].into();
        assert!(newly_plugged(&known, &[d]).is_empty());
        assert_eq!(file_uri("/run/media/me/My Stick"), "file:///run/media/me/My%20Stick");
        assert_eq!(friendly_error("org.freedesktop.UDisks2.Error.NotAuthorizedDismissed", "x"), t("Authentication was cancelled"));
        assert_eq!(friendly_error("org.freedesktop.UDisks2.Error.Failed", "Error unlocking /dev/sdb2: Failed to activate device: Operation not permitted"), t("Wrong passphrase"));
        let mut s = String::from("secret");
        wipe(&mut s);
        assert!(s.is_empty());
    }

    /// Read-only live listing: `cargo test -p mandara-services drives -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_listing() {
        let drives = zbus::block_on(async { read_drives(&Connection::system().await?).await }).expect("UDisks2");
        for d in &drives {
            println!("{} ({}) removable={} bus={:?}", d.display_name(), d.device, d.is_removable(), d.bus);
            for v in &d.volumes {
                println!("  {} {:?} {} enc={} locked={} system={} mounts={:?} usage={:?}", v.device, v.label, v.effective().fs_type, v.encrypted, v.locked(), v.system, v.effective().mount_points, v.effective().usage);
            }
        }
        assert!(!drives.is_empty());
    }
}
