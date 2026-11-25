//! What the information widgets show, gathered on the shell's side: the
//! machine (read once, off the UI thread), screen time, today's prayers
//! and the one thing worth knowing now.

use std::path::Path;

use chrono::{Local, Timelike as _};
use gpui::{App, Global};
use mandara_i18n::tf;

use crate::backdrop::widgets::actions::Action;
use crate::backdrop::widgets::{FetchInfo, GlanceInfo, PrayerInfo, ScreenInfo};

/// What doesn't change while the shell runs.
#[derive(Clone, Default)]
struct Machine {
    os: String,
    kernel: String,
    cpu: String,
    gpu: String,
    shell: String,
    logo_path: Option<std::path::PathBuf>,
}

#[derive(Default)]
struct MachineCache {
    machine: Option<Machine>,
    loading: bool,
}
impl Global for MachineCache {}

/// A soon-starting event is worth knowing this long ahead, minutes; a
/// battery below this share; the glance's next prayer this close.
const EVENT_SOON_MIN: i64 = 120;
const BATTERY_LOW: f32 = 0.2;
const PRAYER_SOON_MIN: i64 = 60;
const PERCENT: f32 = 100.0;

/// The first `key=` value of an os-release style file.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string())
}

/// The processor's model name.
#[cfg(target_os = "freebsd")]
fn cpu_name() -> String {
    mandara_services::freebsd::host::cpu_model()
}

#[cfg(not(target_os = "freebsd"))]
fn cpu_name() -> String {
    std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_string()))).unwrap_or_default()
}

/// The graphics card's name from the PCI ids database (its vendor's name
/// when the device isn't listed).
fn gpu_name() -> String {
    let Ok(cards) = std::fs::read_dir("/sys/class/drm") else { return String::new() };
    let mut ids = None;
    for c in cards.flatten() {
        let dev = c.path().join("device");
        let read = |f: &str| std::fs::read_to_string(dev.join(f)).ok().map(|s| s.trim().trim_start_matches("0x").to_lowercase());
        if c.file_name().to_string_lossy().starts_with("card")
            && let (Some(v), Some(d)) = (read("vendor"), read("device"))
        {
            ids = Some((v, d));
            break;
        }
    }
    let Some((vendor, device)) = ids else { return String::new() };
    for db in ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"] {
        if let Ok(text) = std::fs::read_to_string(db) {
            return lookup_pci(&text, &vendor, &device);
        }
    }
    String::new()
}

/// `Vendor Device` from a pci.ids text.
fn lookup_pci(text: &str, vendor: &str, device: &str) -> String {
    let mut vendor_name: Option<String> = None;
    for line in text.lines() {
        if let Some(name) = vendor_name.as_ref() {
            if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
                return name.clone();
            }
            if let Some(rest) = line.strip_prefix('\t')
                && !rest.starts_with('\t')
                && let Some((id, dev)) = rest.split_once("  ")
                && id == device
            {
                // `Navi 31 [Radeon RX 7900 XT]`: the bracketed name reads best.
                let dev = dev.rsplit_once('[').and_then(|(_, b)| b.strip_suffix(']')).unwrap_or(dev);
                return format!("{} {}", short_vendor(name), dev.trim());
            }
        } else if let Some((id, name)) = line.split_once("  ")
            && id == vendor
        {
            vendor_name = Some(name.to_string());
        }
    }
    vendor_name.unwrap_or_default()
}

/// `Advanced Micro Devices, Inc. [AMD/ATI]` → `AMD`.
fn short_vendor(name: &str) -> &str {
    name.rsplit_once('[').and_then(|(_, b)| b.strip_suffix(']')).map(|b| b.split('/').next().unwrap_or(b)).unwrap_or(name.split([' ', ',']).next().unwrap_or(name))
}

fn read_machine() -> Machine {
    let release = ["/etc/os-release", "/usr/lib/os-release"].iter().find_map(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
    let shell = std::env::var("SHELL").ok().and_then(|s| Path::new(&s).file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default();
    let logo_path = crate::icon_theme::distro_logo().and_then(|name| crate::icon_theme::resolve(&name, None, false)).map(|r| r.path);
    Machine {
        os: field(&release, "PRETTY_NAME").unwrap_or_default(),
        kernel: crate::widgets::system::kernel_release(),
        cpu: cpu_name(),
        gpu: gpu_name(),
        shell,
        logo_path,
    }
}

/// Memory used and total, bytes.
#[cfg(target_os = "freebsd")]
fn memory() -> (u64, u64) {
    mandara_services::freebsd::host::memory().map(|m| (m.used(), m.total)).unwrap_or_default()
}

#[cfg(not(target_os = "freebsd"))]
fn memory() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)).and_then(|r| r.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * KIB;
    let (total, free) = (kb("MemTotal:"), kb("MemAvailable:"));
    (total.saturating_sub(free), total)
}

#[cfg(not(target_os = "freebsd"))]
const KIB: u64 = 1024;
/// The machine's logo, drawn at most this big, physical px.
const LOGO_PX: u32 = 220;

/// The machine card's data (the machine is read once, in the background).
pub fn fetch(cx: &mut App) -> Option<FetchInfo> {
    if !cx.has_global::<MachineCache>() {
        cx.set_global(MachineCache::default());
    }
    let cache = cx.global_mut::<MachineCache>();
    let Some(machine) = cache.machine.clone() else {
        if !cache.loading {
            cache.loading = true;
            cx.spawn(async move |cx| {
                let m = cx.background_executor().spawn(async move { read_machine() }).await;
                cx.update(|cx| {
                    let c = cx.global_mut::<MachineCache>();
                    c.machine = Some(m);
                    c.loading = false;
                    crate::backdrop::refresh(cx);
                });
            })
            .detach();
        }
        return None;
    };
    let me = crate::plugins::main_menu::me();
    let desktop = crate::hub::try_hub(cx).and_then(|h| h.read(cx).compositor.as_ref().map(|c| format!("{:?}", c.kind()))).unwrap_or_default();
    let screen = cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.iter().map(|g| format!("{}×{}", g.width, g.height)).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let logo = machine.logo_path.as_deref().and_then(|p| crate::desktop_media::picture(p, LOGO_PX, cx));
    let mut info = FetchInfo {
        user: me.login,
        host: me.host,
        os: machine.os,
        kernel: machine.kernel,
        desktop,
        cpu: machine.cpu,
        gpu: machine.gpu,
        memory: memory(),
        uptime_s: crate::plugins::main_menu::uptime_secs().unwrap_or(0),
        shell: machine.shell,
        screen,
        logo,
    };
    // Screenshots and demos name the same calm, fictional parts as the
    // Resources panel, never the real hardware.
    if crate::plugins::resources::demo::active() {
        use crate::plugins::resources::demo;
        let (used, total, _, _) = demo::meminfo();
        info.kernel = demo::KERNEL.into();
        info.cpu = demo::CPU_NAME.into();
        info.gpu = demo::gpu_name().into();
        info.memory = (used, total);
        info.uptime_s = demo::uptime_secs();
    }
    Some(info)
}

/// Screen time of the last days (short day names, today flagged) and
/// today's apps, longest first.
pub fn screen(days: u32, cx: &App) -> Option<ScreenInfo> {
    let tracker = crate::plugins::screen_time::tracker(cx)?;
    let store = &tracker.read(cx).store;
    let today = Local::now().date_naive();
    let days = store
        .last_days(today, days as usize)
        .into_iter()
        .map(|(d, secs)| (crate::widgets::clock::localized(&d.and_hms_opt(0, 0, 0).unwrap_or_default().and_local_timezone(Local).earliest().unwrap_or_else(Local::now), "%a"), secs, d == today))
        .collect();
    let top = store.top(today).into_iter().map(|(app, secs)| (crate::apps::name_for_app_id(&app).unwrap_or(app), secs)).collect();
    Some(ScreenInfo { days, top })
}

/// Today's prayer times, tomorrow's Fajr and today's Hijri date.
pub fn prayer(cfg: &mandara_config::ShellConfig, cx: &App) -> PrayerInfo {
    let today = crate::plugins::prayer::today(cx);
    let tomorrow = crate::plugins::prayer::tomorrow(cx);
    PrayerInfo {
        times: today.times.iter().map(|(p, t)| (p.name(), p.name_ar(), *t)).collect(),
        tomorrow_fajr: tomorrow.get(mandara_services::prayer::Prayer::Fajr),
        hijri: Some(crate::desktop::hijri_today(cfg, cx)),
        lang: crate::plugins::prayer::own_lang(cx),
    }
}

/// The one thing worth knowing now, most pressing first: an event soon,
/// the next prayer within the hour, a low battery, unread notifications,
/// what plays; else the day's progress.
pub fn glance(cfg: &mandara_config::ShellConfig, prayer_on: bool, cx: &App) -> GlanceInfo {
    let now = Local::now();
    if cfg.calendar.enabled
        && let Some(store) = crate::calendar::store(cx)
        && let Some(e) = store.read(cx).between(now, now + chrono::Duration::minutes(EVENT_SOON_MIN)).into_iter().find(|o| o.end > now && !o.all_day)
    {
        let mins = (e.start - now).num_minutes();
        let text = if mins <= 0 { tf("{event} now", &[("event", &e.summary)]) } else { tf("{event} in {minutes} min", &[("event", &e.summary), ("minutes", &mins)]) };
        return GlanceInfo { icon: "calendar", text: text.to_string(), action: Some(Action::Panel(crate::plugins::clock::ID.into())) };
    }
    if prayer_on {
        let t = now.time();
        if let Some((p, at)) = crate::plugins::prayer::today(cx).next_after(t)
            && (at - t).num_minutes() <= PRAYER_SOON_MIN
        {
            let text = tf("{prayer} in {minutes} min", &[("prayer", &mandara_i18n::tr(p.name())), ("minutes", &(at - t).num_minutes())]);
            return GlanceInfo { icon: "crescent", text: text.to_string(), action: Some(Action::Panel(crate::plugins::prayer::ID.into())) };
        }
    }
    if let Some(hub) = crate::hub::try_hub(cx) {
        let hub = hub.read(cx);
        let battery = crate::powerctl::battery::policy(cx).and_then(|p| p.read(cx).state.batteries().next().map(|b| (b.percentage as f32 / PERCENT, p.read(cx).state.on_battery)));
        if let Some((level, on_battery)) = battery
            && level < BATTERY_LOW
            && on_battery
        {
            return GlanceInfo { icon: "battery", text: tf("Battery at {percent}%", &[("percent", &((level * PERCENT).round() as i32))]).to_string(), action: None };
        }
        let unread = hub.notifications.unread();
        if unread > 0 {
            return GlanceInfo { icon: "bell", text: tf("{count} unread notifications", &[("count", &unread)]).to_string(), action: Some(Action::Panel(crate::plugins::notifications::ID.into())) };
        }
        if let Some(m) = hub.media.as_ref().filter(|m| m.status == mandara_services::PlaybackStatus::Playing && !m.title.is_empty()) {
            let text = if m.artist.is_empty() { m.title.clone() } else { format!("{} · {}", m.title, m.artist) };
            return GlanceInfo { icon: "music", text, action: Some(Action::Panel(crate::plugins::media::ID.into())) };
        }
    }
    let left = 1.0 - (now.num_seconds_from_midnight() as f32 / SECONDS_PER_DAY);
    GlanceInfo { icon: "clock", text: tf("{percent}% of today left", &[("percent", &((left * PERCENT).round() as i32))]).to_string(), action: None }
}

const SECONDS_PER_DAY: f32 = 86_400.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_names_read_well() {
        let db = "# comment\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX]\n\t164e  Raphael\n10de  NVIDIA Corporation\n\t2684  AD102 [GeForce RTX 4090]\n";
        assert_eq!(lookup_pci(db, "1002", "744c"), "AMD Radeon RX 7900 XT/7900 XTX");
        assert_eq!(lookup_pci(db, "1002", "164e"), "AMD Raphael");
        assert_eq!(lookup_pci(db, "10de", "2684"), "NVIDIA GeForce RTX 4090");
        assert_eq!(lookup_pci(db, "10de", "0000"), "NVIDIA Corporation");
        assert_eq!(field("NAME=\"Arch\"\nPRETTY_NAME=\"Arch Linux\"\n", "PRETTY_NAME").as_deref(), Some("Arch Linux"));
    }
}
