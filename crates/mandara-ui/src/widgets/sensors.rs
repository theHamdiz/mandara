//! Hardware sensors from sysfs: component temperatures (`hwmon`) grouped
//! into friendly components, and per-GPU load / VRAM / temperature / clock
//! (`drm`), with an optional `nvidia-smi` fallback for the proprietary
//! NVIDIA driver. Every reader takes the filesystem root so tests run
//! against fake sysfs trees. Temperatures are always Celsius here; convert
//! for display with [`mandara_config::Units`].

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mandara_i18n::{tf, tk, tr};

/// Readings at or beyond these bounds are disconnected / bogus sensors.
const MIN_C: f32 = -40.0;
const MAX_C: f32 = 150.0;

// ── Components ───────────────────────────────────────────────────────────

/// What a temperature belongs to, in display order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Component {
    Cpu,
    Gpu,
    Storage,
    Memory,
    Board,
    Network,
    Battery,
    Other,
}

impl Component {
    /// Stable machine key (JSON output).
    pub fn key(self) -> &'static str {
        match self {
            Component::Cpu => "cpu",
            Component::Gpu => "gpu",
            Component::Storage => "storage",
            Component::Memory => "memory",
            Component::Board => "motherboard",
            Component::Network => "network",
            Component::Battery => "battery",
            Component::Other => "other",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Component::Cpu => "cpu",
            Component::Gpu => "monitor",
            Component::Storage => "disk",
            Component::Memory => "memory",
            Component::Board => "grid",
            Component::Network => "ethernet",
            Component::Battery => "bolt",
            Component::Other => "thermometer",
        }
    }

    /// Generic `tk()`-marked name of the component class.
    pub fn kind_label(self) -> &'static str {
        match self {
            Component::Cpu => tk("Processor"),
            Component::Gpu => tk("Graphics"),
            Component::Storage => tk("Storage"),
            Component::Memory => tk("Memory"),
            Component::Board => tk("Motherboard"),
            Component::Network => tk("Network"),
            Component::Battery => tk("Battery"),
            Component::Other => tk("Sensor"),
        }
    }

    /// `(warm, hot)` thresholds in °C for consumer hardware.
    pub fn thresholds(self) -> (f32, f32) {
        match self {
            Component::Cpu => (75.0, 90.0),
            Component::Gpu => (80.0, 95.0),
            Component::Storage => (60.0, 75.0),
            Component::Memory => (70.0, 85.0),
            // The hottest board sensor is usually the chipset, which runs warm.
            Component::Board => (70.0, 90.0),
            Component::Network => (70.0, 90.0),
            Component::Battery => (45.0, 55.0),
            Component::Other => (70.0, 90.0),
        }
    }
}

/// A display label, translated at render time (the language can change
/// while samples are cached).
#[derive(Clone, Debug, PartialEq)]
pub enum Label {
    /// A `tk()`-marked key.
    Key(&'static str),
    /// A `tk()`-marked template with an `{n}` placeholder.
    Numbered(&'static str, u32),
    /// Hardware-provided text (model names, chip names).
    Raw(String),
}

impl Label {
    /// The label in the current language.
    pub fn text(&self) -> String {
        match self {
            Label::Key(k) => tr(k),
            Label::Numbered(k, n) => mandara_i18n::fill(&tr(k), &[("n", n)]),
            Label::Raw(s) => s.clone(),
        }
    }

    /// The untranslated English label (JSON output).
    pub fn english(&self) -> String {
        match self {
            Label::Key(k) => k.to_string(),
            Label::Numbered(k, n) => mandara_i18n::fill(k, &[("n", n)]),
            Label::Raw(s) => s.clone(),
        }
    }
}

/// A secondary reading of a component (`CCD 1`, `Hotspot`, `Sensor 2`).
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub label: Label,
    pub celsius: f32,
}

/// How hot a component runs relative to its thresholds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Heat {
    Cool,
    Warm,
    Hot,
}

/// One component's temperature: a primary reading plus secondaries.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentTemp {
    pub kind: Component,
    pub name: Label,
    /// Model / chip identity, shown when there are no secondary readings.
    pub device: Option<String>,
    pub celsius: f32,
    pub readings: Vec<Reading>,
    pub warm: f32,
    pub hot: f32,
    /// The hwmon chip name(s) it came from.
    pub chip: String,
}

impl ComponentTemp {
    pub fn heat(&self) -> Heat {
        if self.celsius >= self.hot {
            Heat::Hot
        } else if self.celsius >= self.warm {
            Heat::Warm
        } else {
            Heat::Cool
        }
    }

    /// Fill of a heat bar: 20 °C empty, a little past `hot` full.
    pub fn fraction(&self) -> f32 {
        ((self.celsius - 20.0) / (self.hot + 5.0 - 20.0).max(1.0)).clamp(0.0, 1.0)
    }
}

// ── hwmon ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Temp {
    index: u32,
    label: Option<String>,
    celsius: f32,
    crit: Option<f32>,
}

#[derive(Clone, Debug)]
struct Chip {
    name: String,
    dir: PathBuf,
    temps: Vec<Temp>,
}

fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn read_num<T: std::str::FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read_trim(path)?.parse().ok()
}

/// Millidegrees → °C, rejecting impossible values and the `0` that
/// unpopulated sensor inputs report.
fn millideg(path: impl AsRef<Path>) -> Option<f32> {
    let c = read_num::<i64>(path)? as f32 / 1000.0;
    (c > MIN_C && c < MAX_C && c != 0.0).then_some(c)
}

/// `hwmonN` / `cardN` entries of `dir`, in numeric order.
fn numbered(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out: Vec<(u32, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix(prefix)?.parse::<u32>().ok()?;
            Some((n, e.path()))
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out.into_iter().map(|(_, p)| p).collect()
}

fn read_chip(dir: &Path) -> Option<Chip> {
    let name = read_trim(dir.join("name"))?;
    let mut temps: Vec<Temp> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let index = file.strip_prefix("temp")?.strip_suffix("_input")?.parse::<u32>().ok()?;
            let celsius = millideg(e.path())?;
            let label = read_trim(dir.join(format!("temp{index}_label")));
            let crit = millideg(dir.join(format!("temp{index}_crit"))).filter(|c| *c >= 40.0);
            Some(Temp { index, label, celsius, crit })
        })
        .collect();
    temps.sort_by_key(|t| t.index);
    (!temps.is_empty()).then_some(Chip { name, dir: dir.to_path_buf(), temps })
}

fn read_chips(root: &Path) -> Vec<Chip> {
    numbered(&root.join("sys/class/hwmon"), "hwmon").iter().filter_map(|d| read_chip(d)).collect()
}

fn is_wifi(name: &str) -> bool {
    ["iwlwifi", "mt76", "mt79", "ath9k", "ath10k", "ath11k", "ath12k", "rtw88", "rtw89", "brcmfmac", "mwifiex"].iter().any(|p| name.starts_with(p))
}

fn is_ethernet(name: &str) -> bool {
    ["r8169", "r8125", "r8126", "igc", "igb", "e1000e", "atlantic", "aqc", "ixgbe", "bnxt", "tg3", "alx"].iter().any(|p| name.starts_with(p))
}

fn is_board(name: &str) -> bool {
    ["nct", "it87", "it86", "w83", "f71", "asus", "gigabyte", "dell_smm", "thinkpad", "hp_wmi", "pch_", "acpitz", "msi"].iter().any(|p| name.starts_with(p))
}

fn classify(name: &str) -> Component {
    match name {
        "k10temp" | "coretemp" | "zenpower" | "cpu_thermal" | "cpu" | "via_cputemp" | "fam15h_power" => Component::Cpu,
        "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" | "nvidia" => Component::Gpu,
        "nvme" | "drivetemp" => Component::Storage,
        "spd5118" | "jc42" => Component::Memory,
        n if n.starts_with("BAT") || n.contains("battery") => Component::Battery,
        n if is_board(n) => Component::Board,
        n if is_wifi(n) || is_ethernet(n) => Component::Network,
        _ => Component::Other,
    }
}

/// `"Tccd2"` → `2`, `"Core 7"` → `7`.
fn trailing_number(s: &str) -> Option<u32> {
    let digits: String = s.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse().ok()
}

fn labelled<'a>(chip: &'a Chip, pred: impl Fn(&str) -> bool) -> Option<&'a Temp> {
    chip.temps.iter().find(|t| t.label.as_deref().is_some_and(&pred))
}

fn thresholds(kind: Component, crit: Option<f32>) -> (f32, f32) {
    let (warm, hot) = kind.thresholds();
    match crit {
        Some(c) => {
            let hot = hot.min(c - 5.0);
            (warm.min(hot - 10.0), hot)
        }
        None => (warm, hot),
    }
}

fn component(kind: Component, name: Label, device: Option<String>, primary: &Temp, readings: Vec<Reading>, chip: &str) -> ComponentTemp {
    let (warm, hot) = thresholds(kind, primary.crit);
    ComponentTemp { kind, name, device, celsius: primary.celsius, readings, warm, hot, chip: chip.to_string() }
}

/// `"AMD Ryzen 9 9950X 16-Core Processor"` → `"AMD Ryzen 9 9950X"`.
fn cpu_model(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("proc/cpuinfo")).ok()?;
    let raw = text.lines().find(|l| l.starts_with("model name"))?.split_once(':')?.1.trim().to_string();
    let mut s = raw.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    if let Some(at) = s.find(" @ ") {
        s.truncate(at);
    }
    let words: Vec<&str> = s.split_whitespace().filter(|w| !w.ends_with("-Core") && *w != "Processor" && *w != "CPU").collect();
    let s = words.join(" ");
    (!s.is_empty()).then_some(s)
}

fn cpu_component(chip: &Chip, model: Option<String>) -> ComponentTemp {
    let name = Label::Key(tk("CPU"));
    match chip.name.as_str() {
        "k10temp" | "zenpower" => {
            let primary = labelled(chip, |l| l == "Tdie").or_else(|| labelled(chip, |l| l == "Tctl")).unwrap_or(&chip.temps[0]);
            let readings = chip
                .temps
                .iter()
                .filter(|t| t.label.as_deref().is_some_and(|l| l.starts_with("Tccd")))
                .map(|t| Reading { label: Label::Numbered(tk("CCD {n}"), t.label.as_deref().and_then(trailing_number).unwrap_or(t.index)), celsius: t.celsius })
                .collect();
            component(Component::Cpu, name, model, primary, readings, &chip.name)
        }
        "coretemp" => {
            let primary = labelled(chip, |l| l.starts_with("Package")).unwrap_or(&chip.temps[0]);
            let hottest = chip.temps.iter().filter(|t| t.label.as_deref().is_some_and(|l| l.starts_with("Core"))).map(|t| t.celsius).fold(None, |m: Option<f32>, c| Some(m.map_or(c, |m| m.max(c))));
            let readings = hottest.map(|c| vec![Reading { label: Label::Key(tk("Hottest core")), celsius: c }]).unwrap_or_default();
            component(Component::Cpu, name, model, primary, readings, &chip.name)
        }
        _ => component(Component::Cpu, name, model, &chip.temps[0], Vec::new(), &chip.name),
    }
}

/// Text of a GPU-ish chip's sensor label.
fn gpu_reading_label(label: &str) -> Label {
    match label {
        "junction" | "hotspot" => Label::Key(tk("Junction")),
        "mem" | "vram" => Label::Key(tk("Memory")),
        "edge" => Label::Key(tk("Edge")),
        other => Label::Raw(other.to_string()),
    }
}

fn gpu_component(root: &Path, chip: &Chip) -> ComponentTemp {
    let primary = labelled(chip, |l| l == "edge").or_else(|| labelled(chip, |l| l == "pkg")).unwrap_or(&chip.temps[0]);
    let readings = chip
        .temps
        .iter()
        .filter(|t| t.index != primary.index)
        .map(|t| Reading { label: t.label.as_deref().map(gpu_reading_label).unwrap_or_else(|| Label::Numbered(tk("Sensor {n}"), t.index)), celsius: t.celsius })
        .collect();
    let device = chip.dir.join("device");
    let name = pci_name(root, &device).map(|n| Label::Raw(short_gpu_name(&n))).unwrap_or(Label::Key(tk("GPU")));
    component(Component::Gpu, name, None, primary, readings, &chip.name)
}

fn storage_component(chip: &Chip, ordinal: u32) -> ComponentTemp {
    let primary = labelled(chip, |l| l == "Composite").unwrap_or(&chip.temps[0]);
    let readings = chip
        .temps
        .iter()
        .filter(|t| t.index != primary.index)
        .map(|t| Reading { label: Label::Numbered(tk("Sensor {n}"), t.label.as_deref().and_then(trailing_number).unwrap_or(t.index)), celsius: t.celsius })
        .collect();
    let model = read_trim(chip.dir.join("device/model"));
    let name = match &model {
        Some(m) => Label::Raw(m.clone()),
        None if chip.name == "nvme" => Label::Numbered(tk("NVMe {n}"), ordinal),
        None => Label::Numbered(tk("Drive {n}"), ordinal),
    };
    component(Component::Storage, name, model, primary, readings, &chip.name)
}

/// Board / memory chips collapse into one component: the hottest reading
/// leads, the coolest is listed, and memory lists each DIMM.
fn merged(kind: Component, name: Label, chips: &[&Chip]) -> Option<ComponentTemp> {
    let all: Vec<&Temp> = chips.iter().flat_map(|c| c.temps.iter()).collect();
    let primary = *all.iter().max_by(|a, b| a.celsius.total_cmp(&b.celsius))?;
    let readings = if kind == Component::Memory && chips.len() > 1 {
        chips.iter().enumerate().filter_map(|(i, c)| c.temps.first().map(|t| Reading { label: Label::Numbered(tk("DIMM {n}"), i as u32 + 1), celsius: t.celsius })).collect()
    } else if all.len() > 1 {
        let low = all.iter().map(|t| t.celsius).fold(f32::INFINITY, f32::min);
        vec![Reading { label: Label::Key(tk("Lowest")), celsius: low }]
    } else {
        Vec::new()
    };
    let chip_names = chips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
    Some(component(kind, name, None, primary, readings, &chip_names))
}

/// Battery temperature from `power_supply` (deci-°C) when no hwmon has it.
fn power_supply_batteries(root: &Path) -> Vec<ComponentTemp> {
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(root.join("sys/class/power_supply")).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for dir in entries {
        if read_trim(dir.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let Some(c) = read_num::<f32>(dir.join("temp")).map(|d| d / 10.0).filter(|c| *c > MIN_C && *c < MAX_C && *c != 0.0) else { continue };
        let t = Temp { index: 1, label: None, celsius: c, crit: None };
        out.push(component(Component::Battery, Label::Key(tk("Battery")), None, &t, Vec::new(), "power_supply"));
    }
    out
}

/// Every component temperature under `root` (`/` in production), grouped
/// and ordered CPU → GPU → storage → memory → board → network → battery →
/// other.
pub fn read_temperatures(root: &Path) -> Vec<ComponentTemp> {
    let chips = read_chips(root);
    let model = cpu_model(root);
    let mut out: Vec<ComponentTemp> = Vec::new();
    let mut board: Vec<&Chip> = Vec::new();
    let mut memory: Vec<&Chip> = Vec::new();
    let mut drives = 0;
    for chip in &chips {
        match classify(&chip.name) {
            Component::Cpu => out.push(cpu_component(chip, model.clone())),
            Component::Gpu => out.push(gpu_component(root, chip)),
            Component::Storage => {
                drives += 1;
                out.push(storage_component(chip, drives));
            }
            Component::Memory => memory.push(chip),
            Component::Board => board.push(chip),
            Component::Network => {
                let name = if is_wifi(&chip.name) { tk("Wi-Fi") } else { tk("Ethernet") };
                let primary = &chip.temps[0];
                out.push(component(Component::Network, Label::Key(name), None, primary, Vec::new(), &chip.name));
            }
            Component::Battery => out.push(component(Component::Battery, Label::Key(tk("Battery")), None, &chip.temps[0], Vec::new(), &chip.name)),
            Component::Other => {
                let name = if chip.name.contains("xhci") { Label::Key(tk("USB controller")) } else { Label::Raw(chip.name.clone()) };
                out.push(component(Component::Other, name, Some(chip.name.clone()), &chip.temps[0], Vec::new(), &chip.name));
            }
        }
    }
    // A desktop's ACPI zone is usually a static placeholder; keep it only
    // when no real board sensor exists (laptops).
    if board.iter().any(|c| c.name != "acpitz") {
        board.retain(|c| c.name != "acpitz");
    }
    out.extend(merged(Component::Memory, Label::Key(tk("Memory")), &memory));
    out.extend(merged(Component::Board, Label::Key(tk("Motherboard")), &board));
    if !out.iter().any(|c| c.kind == Component::Battery) {
        out.extend(power_supply_batteries(root));
    }
    out.sort_by_key(|c| c.kind);
    out
}

/// The CPU package temperature, if any sensor reports one.
pub fn cpu_temperature(temps: &[ComponentTemp]) -> Option<f32> {
    temps.iter().find(|c| c.kind == Component::Cpu).map(|c| c.celsius)
}

// ── GPUs ─────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GpuVendor {
    Amd,
    Intel,
    Nvidia,
    Other,
}

impl GpuVendor {
    fn from_pci(id: &str) -> Self {
        match id.trim_start_matches("0x").to_ascii_lowercase().as_str() {
            "1002" => GpuVendor::Amd,
            "8086" => GpuVendor::Intel,
            "10de" => GpuVendor::Nvidia,
            _ => GpuVendor::Other,
        }
    }

    pub fn brand(self) -> &'static str {
        match self {
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Other => "",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            GpuVendor::Amd => "amd",
            GpuVendor::Intel => "intel",
            GpuVendor::Nvidia => "nvidia",
            GpuVendor::Other => "other",
        }
    }
}

/// One GPU's live stats; fields the driver doesn't expose are `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    /// PCI slot (`0000:03:00.0`), the stable identity.
    pub slot: String,
    pub vendor: GpuVendor,
    /// Marketing name (`AMD Radeon RX 9070 XT`), when known.
    pub name: Option<String>,
    pub integrated: bool,
    /// Busy fraction `0..=1`.
    pub load: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub temp_c: Option<f32>,
    pub freq_mhz: Option<f32>,
}

impl Gpu {
    /// The name in the current language (`AMD GPU` when unknown).
    pub fn display_name(&self) -> String {
        match (&self.name, self.vendor) {
            (Some(n), _) => n.clone(),
            (None, GpuVendor::Other) => tr(tk("GPU")),
            (None, v) => tf("{vendor} GPU", &[("vendor", &v.brand())]),
        }
    }

    pub fn vram_fraction(&self) -> Option<f32> {
        match (self.vram_used, self.vram_total) {
            (Some(u), Some(t)) if t > 0 => Some((u as f64 / t as f64) as f32),
            _ => None,
        }
    }
}

/// Cached `pci.ids` lookups keyed by (file, vendor, device, subsystem).
fn pci_cache() -> &'static Mutex<HashMap<(PathBuf, String, String, String), Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, String, String, String), Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Looks up a device (preferring its exact subsystem) in `pci.ids` text.
pub fn lookup_pci_ids(text: &str, vendor: &str, device: &str, sub: Option<(&str, &str)>) -> Option<String> {
    let (vendor, device) = (vendor.to_ascii_lowercase(), device.to_ascii_lowercase());
    let mut in_vendor = false;
    let mut found: Option<String> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if found.is_some() {
                break;
            }
            in_vendor = line.get(..4).is_some_and(|v| v.eq_ignore_ascii_case(&vendor)) && line.as_bytes().get(4) == Some(&b' ');
            continue;
        }
        if !in_vendor {
            continue;
        }
        if let Some(rest) = line.strip_prefix("\t\t") {
            // Subsystem line: `\t\tSSSS DDDD  name`.
            if let (Some(_), Some((sv, sd))) = (&found, sub) {
                let mut f = rest.split_whitespace();
                if f.next().is_some_and(|v| v.eq_ignore_ascii_case(sv)) && f.next().is_some_and(|d| d.eq_ignore_ascii_case(sd)) {
                    return rest.get(10..).map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).or(found);
                }
            }
            continue;
        }
        if found.is_some() {
            break; // past our device's subsystem block
        }
        let rest = &line[1..];
        if rest.get(..4).is_some_and(|d| d.eq_ignore_ascii_case(&device)) {
            found = rest.get(4..).map(|n| n.trim().to_string());
        }
    }
    found
}

fn hex_id(path: impl AsRef<Path>) -> Option<String> {
    read_trim(path).map(|s| s.trim_start_matches("0x").to_ascii_lowercase())
}

/// Marketing name of the PCI device at `device` (a sysfs device dir):
/// the driver's `product_name`, else `pci.ids`, prettified with the brand.
fn pci_name(root: &Path, device: &Path) -> Option<String> {
    if let Some(p) = read_trim(device.join("product_name")) {
        return Some(p);
    }
    let vendor = hex_id(device.join("vendor"))?;
    let dev = hex_id(device.join("device"))?;
    let sub = (hex_id(device.join("subsystem_vendor")).unwrap_or_default(), hex_id(device.join("subsystem_device")).unwrap_or_default());
    let file = ["usr/share/hwdata/pci.ids", "usr/share/misc/pci.ids"].iter().map(|p| root.join(p)).find(|p| p.exists())?;
    let key = (file.clone(), vendor.clone(), dev.clone(), format!("{}:{}", sub.0, sub.1));
    if let Some(hit) = pci_cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let text = fs::read_to_string(&file).ok()?;
    let raw = lookup_pci_ids(&text, &vendor, &dev, (!sub.0.is_empty()).then_some((sub.0.as_str(), sub.1.as_str())));
    let name = raw.map(|r| pretty_gpu_name(&r, GpuVendor::from_pci(&vendor)));
    if let Ok(mut c) = pci_cache().lock() {
        c.insert(key, name.clone());
    }
    name
}

/// `"Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]"` → `"AMD Radeon RX
/// 9070/9070 XT/9070 GRE"`: the bracketed marketing name, branded.
pub fn pretty_gpu_name(raw: &str, vendor: GpuVendor) -> String {
    let inner = match (raw.find('['), raw.rfind(']')) {
        (Some(a), Some(b)) if b > a + 1 => raw[a + 1..b].trim(),
        _ => raw.trim(),
    };
    let brand = vendor.brand();
    if brand.is_empty() || inner.to_ascii_lowercase().starts_with(&brand.to_ascii_lowercase()) {
        inner.to_string()
    } else {
        format!("{brand} {inner}")
    }
}

/// A compact GPU name for small cells: no brand, first variant only
/// (`"AMD Radeon RX 9070/9070 XT"` → `"Radeon RX 9070"`).
pub fn short_gpu_name(name: &str) -> String {
    let s = ["AMD ", "Intel ", "NVIDIA ", "Intel(R) "].iter().fold(name, |s, p| s.strip_prefix(p).unwrap_or(s));
    s.split('/').next().unwrap_or(s).trim().to_string()
}

fn first_hwmon(device: &Path) -> Option<PathBuf> {
    numbered(&device.join("hwmon"), "hwmon").into_iter().next()
}

/// Current shader clock: hwmon `freqN` labelled `sclk` (Hz), else the
/// active `pp_dpm_sclk` level.
fn amd_freq_mhz(device: &Path, hwmon: Option<&Path>) -> Option<f32> {
    if let Some(h) = hwmon {
        for i in 1..=4 {
            if read_trim(h.join(format!("freq{i}_label"))).as_deref() == Some("sclk") {
                if let Some(hz) = read_num::<f64>(h.join(format!("freq{i}_input"))) {
                    return Some((hz / 1e6) as f32);
                }
            }
        }
    }
    let table = fs::read_to_string(device.join("pp_dpm_sclk")).ok()?;
    let line = table.lines().find(|l| l.trim_end().ends_with('*'))?;
    let mhz = line.split_whitespace().find(|w| w.to_ascii_lowercase().ends_with("mhz"))?;
    mhz[..mhz.len() - 3].parse().ok()
}

fn hwmon_gpu_temp(hwmon: Option<&Path>) -> Option<f32> {
    let chip = read_chip(hwmon?)?;
    let t = labelled(&chip, |l| l == "edge").or_else(|| labelled(&chip, |l| l == "pkg")).unwrap_or(&chip.temps[0]);
    Some(t.celsius)
}

fn read_gpu(root: &Path, card: &Path) -> Option<Gpu> {
    let device = card.join("device");
    let vendor = GpuVendor::from_pci(&read_trim(device.join("vendor"))?);
    let slot = read_trim(device.join("uevent"))
        .and_then(|u| u.lines().find_map(|l| l.strip_prefix("PCI_SLOT_NAME=").map(str::to_string)))
        .or_else(|| fs::canonicalize(&device).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())))
        .unwrap_or_else(|| card.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let hwmon = first_hwmon(&device);
    let mut gpu = Gpu {
        slot,
        vendor,
        name: pci_name(root, &device),
        integrated: false,
        load: None,
        vram_used: None,
        vram_total: None,
        temp_c: hwmon_gpu_temp(hwmon.as_deref()),
        freq_mhz: None,
    };
    match vendor {
        GpuVendor::Amd => {
            gpu.load = read_num::<f32>(device.join("gpu_busy_percent")).map(|p| (p / 100.0).clamp(0.0, 1.0));
            gpu.vram_used = read_num(device.join("mem_info_vram_used"));
            gpu.vram_total = read_num::<u64>(device.join("mem_info_vram_total")).filter(|t| *t > 0);
            gpu.freq_mhz = amd_freq_mhz(&device, hwmon.as_deref());
            // APUs carve "VRAM" out of system memory and report no VRAM vendor.
            gpu.integrated = !device.join("mem_info_vram_vendor").exists();
        }
        GpuVendor::Intel => {
            gpu.freq_mhz = [card.join("gt/gt0/rps_act_freq_mhz"), card.join("gt_act_freq_mhz"), device.join("tile0/gt0/freq0/act_freq")]
                .iter()
                .find_map(|p| read_num::<f32>(p))
                .filter(|f| *f > 0.0);
            gpu.vram_total = read_num::<u64>(device.join("lmem_total_bytes")).filter(|t| *t > 0);
            gpu.integrated = gpu.vram_total.is_none() && !gpu.name.as_deref().is_some_and(|n| n.contains("Arc"));
        }
        GpuVendor::Nvidia | GpuVendor::Other => {}
    }
    Some(gpu)
}

/// GPUs visible in sysfs under `root`, discrete before integrated.
pub fn read_gpus(root: &Path) -> Vec<Gpu> {
    let mut out: Vec<Gpu> = Vec::new();
    for card in numbered(&root.join("sys/class/drm"), "card") {
        if let Some(g) = read_gpu(root, &card) {
            if !out.iter().any(|o| o.slot == g.slot) {
                out.push(g);
            }
        }
    }
    out.sort_by_key(|g| g.integrated);
    out
}

fn smi_field<T: std::str::FromStr>(s: Option<&str>) -> Option<T> {
    s?.trim().parse().ok()
}

/// Parses `nvidia-smi --query-gpu=pci.bus_id,name,utilization.gpu,
/// memory.used,memory.total,temperature.gpu,clocks.gr
/// --format=csv,noheader,nounits` (`[N/A]` fields become `None`).
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 6 {
                return None;
            }
            let bus = f[0].to_ascii_lowercase();
            // `00000000:01:00.0` → `0000:01:00.0`.
            let slot = match bus.split_once(':') {
                Some((domain, rest)) if domain.len() > 4 => format!("{}:{rest}", &domain[domain.len() - 4..]),
                _ => bus.clone(),
            };
            let mib = |v: Option<u64>| v.map(|m| m * 1024 * 1024);
            Some(Gpu {
                slot,
                vendor: GpuVendor::Nvidia,
                name: Some(f[1].to_string()).filter(|n| !n.is_empty() && !n.starts_with('[')),
                integrated: false,
                load: smi_field::<f32>(f.get(2).copied()).map(|p| (p / 100.0).clamp(0.0, 1.0)),
                vram_used: mib(smi_field(f.get(3).copied())),
                vram_total: mib(smi_field(f.get(4).copied())),
                temp_c: smi_field::<f32>(f.get(5).copied()).filter(|c| *c > MIN_C && *c < MAX_C),
                freq_mhz: smi_field(f.get(6).copied()),
            })
        })
        .collect()
}

/// Folds `nvidia-smi` rows into the sysfs list (matching PCI slots).
pub fn merge_nvidia(gpus: &mut Vec<Gpu>, smi: Vec<Gpu>) {
    for s in smi {
        match gpus.iter_mut().find(|g| g.slot.eq_ignore_ascii_case(&s.slot)) {
            Some(g) => {
                g.name = s.name.or(g.name.take());
                g.load = s.load.or(g.load);
                g.vram_used = s.vram_used.or(g.vram_used);
                g.vram_total = s.vram_total.or(g.vram_total);
                g.temp_c = s.temp_c.or(g.temp_c);
                g.freq_mhz = s.freq_mhz.or(g.freq_mhz);
            }
            None => gpus.push(s),
        }
    }
    gpus.sort_by_key(|g| g.integrated);
}

fn nvidia_smi_path() -> Option<&'static Path> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt as _;
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .map(|d| d.join("nvidia-smi"))
            .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
    })
    .as_deref()
}

/// `nvidia-smi` stats, at most every 2 s (cached in between). Blocking:
/// call off the UI thread. Empty when the tool isn't installed.
pub fn nvidia_smi() -> Vec<Gpu> {
    const EVERY: Duration = Duration::from_secs(2);
    static LAST: Mutex<Option<(Instant, Vec<Gpu>)>> = Mutex::new(None);
    let Some(bin) = nvidia_smi_path() else { return Vec::new() };
    if let Ok(last) = LAST.lock() {
        if let Some((_, gpus)) = last.as_ref().filter(|(at, _)| at.elapsed() < EVERY) {
            return gpus.clone();
        }
    }
    let gpus = std::process::Command::new(bin)
        .args(["--query-gpu=pci.bus_id,name,utilization.gpu,memory.used,memory.total,temperature.gpu,clocks.gr", "--format=csv,noheader,nounits"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_nvidia_smi(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    if let Ok(mut last) = LAST.lock() {
        *last = Some((Instant::now(), gpus.clone()));
    }
    gpus
}

/// All GPUs: sysfs plus the `nvidia-smi` fallback. Blocking.
pub fn read_all_gpus() -> Vec<Gpu> {
    let mut gpus = read_gpus(Path::new("/"));
    merge_nvidia(&mut gpus, nvidia_smi());
    gpus
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mandara-sensors-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        fn file(&self, rel: &str, contents: &str) -> &Self {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
            self
        }

        /// `hwmonN` with `name` and `(index, label, millideg)` inputs.
        fn hwmon(&self, n: u32, name: &str, temps: &[(u32, &str, i64)]) -> &Self {
            let base = format!("sys/class/hwmon/hwmon{n}");
            self.file(&format!("{base}/name"), &format!("{name}\n"));
            for (i, label, v) in temps {
                self.file(&format!("{base}/temp{i}_input"), &format!("{v}\n"));
                if !label.is_empty() {
                    self.file(&format!("{base}/temp{i}_label"), &format!("{label}\n"));
                }
            }
            self
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const PCI_IDS: &str = "# comment\n\
1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
\t13c0  Granite Ridge [Radeon 610M]\n\
\t7550  Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]\n\
\t\t1458 2437  Navi 48 XTX [Radeon RX 9070 XT Gaming OC ICE 16G]\n\
\t\t148c 2435  Radeon RX 9070 XT 16GB\n\
\t7551  Other\n\
8086  Intel Corporation\n\
\ta780  Raptor Lake-S GT1 [UHD Graphics 770]\n";

    #[test]
    fn groups_hwmon_chips_into_components() {
        let t = Tree::new("hwmon");
        t.hwmon(0, "acpitz", &[(1, "", 16_800)])
            .hwmon(1, "nvme", &[(1, "Composite", 63_850), (2, "Sensor 1", 63_850), (3, "Sensor 2", 53_850)])
            .file("sys/class/hwmon/hwmon1/device/model", "Lexar SSD NM790 4TB          \n")
            .file("sys/class/hwmon/hwmon1/temp1_crit", "94850\n")
            .hwmon(3, "amdgpu", &[(1, "edge", 53_000), (2, "junction", 57_000), (3, "mem", 74_000)])
            .file("sys/class/hwmon/hwmon3/temp1_crit", "110000\n")
            .file("sys/class/hwmon/hwmon3/device/vendor", "0x1002\n")
            .file("sys/class/hwmon/hwmon3/device/device", "0x7550\n")
            .file("sys/class/hwmon/hwmon3/device/subsystem_vendor", "0x1458\n")
            .file("sys/class/hwmon/hwmon3/device/subsystem_device", "0x2426\n")
            .hwmon(5, "prom21_xhci", &[(1, "", 74_591)])
            .hwmon(6, "k10temp", &[(1, "Tctl", 74_125), (3, "Tccd1", 73_375), (4, "Tccd2", 57_875)])
            .hwmon(7, "gigabyte_wmi", &[(1, "", 53_000), (2, "", 63_000), (3, "", 74_000), (6, "", -55_000)])
            .hwmon(8, "r8169_0_900:00", &[(1, "", 60_000)])
            .hwmon(9, "iwlwifi_1", &[(1, "", 0)])
            .hwmon(10, "spd5118", &[(1, "", 41_000)])
            .hwmon(11, "spd5118", &[(1, "", 43_500)])
            .hwmon(12, "broken", &[(1, "", 200_000)])
            .file("usr/share/hwdata/pci.ids", PCI_IDS)
            .file("proc/cpuinfo", "processor\t: 0\nmodel name\t: AMD Ryzen 9 9950X 16-Core Processor\n");
        let temps = read_temperatures(&t.0);
        let kinds: Vec<Component> = temps.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![Component::Cpu, Component::Gpu, Component::Storage, Component::Memory, Component::Board, Component::Network, Component::Other]);

        let cpu = &temps[0];
        assert!((cpu.celsius - 74.125).abs() < 1e-3);
        assert_eq!(cpu.device.as_deref(), Some("AMD Ryzen 9 9950X"));
        assert_eq!(cpu.readings.iter().map(|r| r.label.english()).collect::<Vec<_>>(), vec!["CCD 1", "CCD 2"]);
        assert_eq!(cpu_temperature(&temps), Some(cpu.celsius));

        let gpu = &temps[1];
        assert_eq!(gpu.name, Label::Raw("Radeon RX 9070".into()));
        assert_eq!(gpu.celsius, 53.0);
        assert_eq!(gpu.readings.iter().map(|r| r.label.english()).collect::<Vec<_>>(), vec!["Junction", "Memory"]);
        assert_eq!(gpu.hot, 95.0, "crit 110 leaves the default");

        let nvme = &temps[2];
        assert_eq!(nvme.name, Label::Raw("Lexar SSD NM790 4TB".into()));
        assert_eq!(nvme.readings.len(), 2);
        assert_eq!(nvme.heat(), Heat::Warm);

        let mem = &temps[3];
        assert_eq!(mem.celsius, 43.5);
        assert_eq!(mem.readings.iter().map(|r| r.label.english()).collect::<Vec<_>>(), vec!["DIMM 1", "DIMM 2"]);

        // gigabyte_wmi wins over acpitz; -55 °C is dropped.
        let board = &temps[4];
        assert_eq!(board.chip, "gigabyte_wmi");
        assert_eq!(board.celsius, 74.0);
        assert_eq!(board.readings, vec![Reading { label: Label::Key("Lowest"), celsius: 53.0 }]);
        assert_eq!(board.heat(), Heat::Warm);

        // The Wi-Fi chip read 0 (unpopulated) and is gone; Ethernet stays.
        assert_eq!(temps[5].name, Label::Key("Ethernet"));
        assert_eq!(temps[6].name, Label::Key("USB controller"));
        assert!(!temps.iter().any(|c| c.chip == "broken"));
    }

    #[test]
    fn intel_cpu_laptop_battery_and_acpi_fallback() {
        let t = Tree::new("laptop");
        t.hwmon(0, "acpitz", &[(1, "", 45_000)])
            .hwmon(1, "coretemp", &[(1, "Package id 0", 61_000), (2, "Core 0", 58_000), (3, "Core 1", 66_000)])
            .file("sys/class/power_supply/BAT0/type", "Battery\n")
            .file("sys/class/power_supply/BAT0/temp", "312\n")
            .file("sys/class/power_supply/AC/type", "Mains\n");
        let temps = read_temperatures(&t.0);
        assert_eq!(temps.len(), 3);
        assert_eq!(temps[0].celsius, 61.0);
        assert_eq!(temps[0].readings, vec![Reading { label: Label::Key("Hottest core"), celsius: 66.0 }]);
        assert_eq!(temps[1].kind, Component::Board, "acpitz kept without other board chips");
        assert_eq!(temps[2].kind, Component::Battery);
        assert!((temps[2].celsius - 31.2).abs() < 1e-4);
        assert!(temps[0].fraction() > 0.5 && temps[0].fraction() < 1.0);
    }

    #[test]
    fn pci_ids_lookup_and_names() {
        assert_eq!(lookup_pci_ids(PCI_IDS, "1002", "7550", Some(("1458", "2437"))).as_deref(), Some("Navi 48 XTX [Radeon RX 9070 XT Gaming OC ICE 16G]"));
        assert_eq!(lookup_pci_ids(PCI_IDS, "1002", "7550", Some(("1458", "9999"))).as_deref(), Some("Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]"));
        assert_eq!(lookup_pci_ids(PCI_IDS, "1002", "7551", None).as_deref(), Some("Other"));
        assert_eq!(lookup_pci_ids(PCI_IDS, "8086", "a780", None).as_deref(), Some("Raptor Lake-S GT1 [UHD Graphics 770]"));
        assert_eq!(lookup_pci_ids(PCI_IDS, "10de", "2786", None), None);
        assert_eq!(pretty_gpu_name("Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]", GpuVendor::Amd), "AMD Radeon RX 9070/9070 XT/9070 GRE");
        assert_eq!(pretty_gpu_name("Raptor Lake-S GT1 [UHD Graphics 770]", GpuVendor::Intel), "Intel UHD Graphics 770");
        assert_eq!(pretty_gpu_name("NVIDIA GeForce RTX 4070", GpuVendor::Nvidia), "NVIDIA GeForce RTX 4070");
        assert_eq!(short_gpu_name("AMD Radeon RX 9070/9070 XT/9070 GRE"), "Radeon RX 9070");
    }

    #[test]
    fn reads_amd_and_intel_gpus_from_drm() {
        let t = Tree::new("drm");
        let d = "sys/class/drm/card1/device";
        t.file(&format!("{d}/vendor"), "0x1002\n")
            .file(&format!("{d}/device"), "0x7550\n")
            .file(&format!("{d}/uevent"), "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n")
            .file(&format!("{d}/gpu_busy_percent"), "37\n")
            .file(&format!("{d}/mem_info_vram_used"), "8267927552\n")
            .file(&format!("{d}/mem_info_vram_total"), "17095983104\n")
            .file(&format!("{d}/mem_info_vram_vendor"), "hynix\n")
            .file(&format!("{d}/pp_dpm_sclk"), "0: 500Mhz\n1: 2450Mhz *\n")
            .file(&format!("{d}/hwmon/hwmon3/name"), "amdgpu\n")
            .file(&format!("{d}/hwmon/hwmon3/temp1_input"), "53000\n")
            .file(&format!("{d}/hwmon/hwmon3/temp1_label"), "edge\n")
            .file(&format!("{d}/hwmon/hwmon3/freq1_label"), "sclk\n")
            .file(&format!("{d}/hwmon/hwmon3/freq1_input"), "2390000000\n");
        let d0 = "sys/class/drm/card0/device";
        t.file(&format!("{d0}/vendor"), "0x8086\n")
            .file(&format!("{d0}/device"), "0xa780\n")
            .file(&format!("{d0}/uevent"), "DRIVER=i915\nPCI_SLOT_NAME=0000:00:02.0\n")
            .file("sys/class/drm/card0/gt/gt0/rps_act_freq_mhz", "1450\n")
            // Connector entries and render nodes are not cards.
            .file("sys/class/drm/card0-DP-1/status", "connected\n")
            .file("sys/class/drm/version", "drm 1.1.0\n")
            .file("usr/share/hwdata/pci.ids", PCI_IDS);
        let gpus = read_gpus(&t.0);
        assert_eq!(gpus.len(), 2);
        let amd = &gpus[0];
        assert!(!amd.integrated);
        assert_eq!(amd.slot, "0000:03:00.0");
        assert_eq!(amd.name.as_deref(), Some("AMD Radeon RX 9070/9070 XT/9070 GRE"));
        assert_eq!(amd.load, Some(0.37));
        assert_eq!((amd.vram_used, amd.vram_total), (Some(8_267_927_552), Some(17_095_983_104)));
        assert!((amd.vram_fraction().unwrap() - 0.4836).abs() < 1e-3);
        assert_eq!(amd.temp_c, Some(53.0));
        assert_eq!(amd.freq_mhz, Some(2390.0));
        let intel = &gpus[1];
        assert!(intel.integrated);
        assert_eq!(intel.name.as_deref(), Some("Intel UHD Graphics 770"));
        assert_eq!((intel.load, intel.vram_total, intel.temp_c), (None, None, None));
        assert_eq!(intel.freq_mhz, Some(1450.0));
        assert_eq!(intel.display_name(), "Intel UHD Graphics 770");
        let unnamed = Gpu { name: None, ..intel.clone() };
        assert_eq!(unnamed.display_name(), "Intel GPU");
    }

    #[test]
    fn amd_dpm_fallback_clock() {
        let t = Tree::new("dpm");
        let d = "sys/class/drm/card0/device";
        t.file(&format!("{d}/vendor"), "0x1002\n").file(&format!("{d}/device"), "0x13c0\n").file(&format!("{d}/pp_dpm_sclk"), "0: 600Mhz *\n1: 700Mhz\n2: 2200Mhz\n");
        let gpus = read_gpus(&t.0);
        assert_eq!(gpus[0].freq_mhz, Some(600.0));
        assert!(gpus[0].integrated);
        assert_eq!(gpus[0].name, None, "no pci.ids in this tree");
        assert_eq!(gpus[0].display_name(), "AMD GPU");
    }

    #[test]
    fn nvidia_smi_rows_merge_by_slot() {
        let text = "00000000:01:00.0, NVIDIA GeForce RTX 4070, 12, 1024, 12282, 45, 2505\n00000000:02:00.0, NVIDIA T400, [N/A], 10, 2048, [N/A], [N/A]\n";
        let smi = parse_nvidia_smi(text);
        assert_eq!(smi.len(), 2);
        assert_eq!(smi[0].slot, "0000:01:00.0");
        assert_eq!(smi[0].load, Some(0.12));
        assert_eq!(smi[0].vram_total, Some(12282 * 1024 * 1024));
        assert_eq!(smi[1].load, None);
        assert_eq!(smi[1].temp_c, None);
        let mut gpus = vec![Gpu { slot: "0000:01:00.0".into(), vendor: GpuVendor::Nvidia, name: None, integrated: false, load: None, vram_used: None, vram_total: None, temp_c: Some(44.0), freq_mhz: None }];
        merge_nvidia(&mut gpus, smi);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name.as_deref(), Some("NVIDIA GeForce RTX 4070"));
        assert_eq!(gpus[0].temp_c, Some(45.0));
    }

    #[test]
    fn live_sysfs_reads_without_panicking() {
        let _ = read_temperatures(Path::new("/"));
        let _ = read_gpus(Path::new("/"));
    }
}
