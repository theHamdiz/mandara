//! Shell diagnostics for Studio → Performance: per-frame timings of the
//! shell views (a small ring buffer filled by [`frame`] guards in the main
//! view's render) and a memory breakdown (`/proc/self/smaps_rollup`, GPU
//! memory from the DRM fdinfo, cache / plugin counts).
//!
//! * `diagnostics frames` — recent frames + percentile stats
//! * `diagnostics memory` — memory by category
//! * `diagnostics views` — render count and cost per plugin view (find
//!   views that redraw while idle or are slow to build)
//!
//! Recording costs two `Instant::now()` calls and a mutex push per frame.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mandara_plugins::{CommandSpec, PluginOrigin, PluginRegistry};
use serde::Serialize;
use serde_json::{Value, json};

/// Frames kept (≈ 1.7 s at 144 Hz, longer when idle).
pub const CAPACITY: usize = 240;
/// A gap longer than this between two renders of one surface means it was
/// idle, not a slow frame.
const CONTINUOUS: Duration = Duration::from_millis(100);

#[derive(Copy, Clone, Debug, Serialize, PartialEq)]
pub struct Frame {
    /// Milliseconds before the query.
    pub age_ms: f32,
    /// Time spent building the view tree (CPU) in ms.
    pub build_ms: f32,
    /// Time since this surface's previous frame while animating, in ms.
    pub interval_ms: Option<f32>,
}

struct Recorded {
    at: Instant,
    build: Duration,
    interval: Option<Duration>,
}

#[derive(Default)]
struct Ring {
    frames: VecDeque<Recorded>,
    last_start: HashMap<u64, Instant>,
    total: u64,
}

static RING: Mutex<Option<Ring>> = Mutex::new(None);

fn with_ring<R>(f: impl FnOnce(&mut Ring) -> R) -> R {
    let mut guard = RING.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Ring::default))
}

/// Times one render of surface `key`; records on drop.
pub struct FrameGuard {
    key: u64,
    start: Instant,
}

/// Starts timing a frame of surface `key` (e.g. the view's entity id).
pub fn frame(key: u64) -> FrameGuard {
    FrameGuard { key, start: Instant::now() }
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        let build = self.start.elapsed();
        record(self.key, self.start, build);
    }
}

fn record(key: u64, start: Instant, build: Duration) {
    with_ring(|r| {
        let interval = r.last_start.insert(key, start).map(|prev| start.saturating_duration_since(prev)).filter(|d| *d <= CONTINUOUS);
        r.frames.push_back(Recorded { at: start, build, interval });
        r.total += 1;
        while r.frames.len() > CAPACITY {
            r.frames.pop_front();
        }
    });
}

/// Render samples kept per plugin view for its percentiles.
const VIEW_SAMPLES: usize = 64;
/// Milliseconds per second, for duration → ms conversions.
const MS_PER_S: f32 = 1000.0;

#[derive(Default)]
struct ViewStats {
    renders: u64,
    total: Duration,
    max: Duration,
    /// Recent build times in ms (bounded ring).
    recent: VecDeque<f32>,
}

type ViewLabel = &'static std::panic::Location<'static>;

static VIEWS: Mutex<Option<HashMap<ViewLabel, ViewStats>>> = Mutex::new(None);

/// Records one render of the plugin view built at `label` that took `build`.
pub fn record_view(label: ViewLabel, build: Duration) {
    let mut guard = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let s = guard.get_or_insert_with(HashMap::new).entry(label).or_default();
    s.renders += 1;
    s.total += build;
    s.max = s.max.max(build);
    if s.recent.len() == VIEW_SAMPLES {
        s.recent.pop_front();
    }
    s.recent.push_back(build.as_secs_f32() * MS_PER_S);
}

/// `crates/mandara-ui/src/plugins/media.rs:629:…` → `media.rs:629`.
fn short_view_label(label: ViewLabel) -> String {
    let file = label.file().rsplit('/').next().unwrap_or(label.file());
    format!("{file}:{}", label.line())
}

/// Plugin views by total render cost (most expensive first).
pub fn views_report(reset: bool) -> Value {
    let mut guard = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let views = guard.get_or_insert_with(HashMap::new);
    let mut rows: Vec<_> = views
        .iter()
        .map(|(label, s)| {
            let recent: Vec<f32> = s.recent.iter().copied().collect();
            let p99 = percentiles(&recent).map(|(_, _, p99, _)| p99);
            (s.total, json!({
                "view": short_view_label(label),
                "renders": s.renders,
                "total_ms": s.total.as_secs_f32() * MS_PER_S,
                "max_ms": s.max.as_secs_f32() * MS_PER_S,
                "recent_p99_ms": p99,
            }))
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    if reset {
        views.clear();
    }
    Value::Array(rows.into_iter().map(|(_, v)| v).collect())
}

/// Quantiles reported by [`percentiles`].
const P50: f32 = 0.5;
const P95: f32 = 0.95;
const P99: f32 = 0.99;
/// Floor for the median interval when deriving fps (avoids division by ~0).
const MIN_INTERVAL_MS: f32 = 0.1;
/// An animation frame later than this multiple of the median is janky.
const JANK_FACTOR: f32 = 1.5;
/// A surface that rendered within this many ms counts as animating.
const ANIMATING_WITHIN_MS: f32 = 250.0;
/// KiB per MiB (and MiB per GiB), and bytes per KiB.
const KIB: u64 = 1024;

/// Percentiles of `values` (sorted copy): `(p50, p95, p99, max)`.
pub fn percentiles(values: &[f32]) -> Option<(f32, f32, f32, f32)> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
    Some((at(P50), at(P95), at(P99), v[v.len() - 1]))
}

fn stats(values: &[f32]) -> Value {
    match percentiles(values) {
        Some((p50, p95, p99, max)) => json!({ "p50": p50, "p95": p95, "p99": p99, "max": max, "mean": values.iter().sum::<f32>() / values.len() as f32 }),
        None => Value::Null,
    }
}

/// Recent frames (oldest first) and their statistics.
pub fn frames_report() -> Value {
    let now = Instant::now();
    let (frames, total) = with_ring(|r| {
        let frames: Vec<Frame> = r
            .frames
            .iter()
            .map(|f| Frame {
                age_ms: now.saturating_duration_since(f.at).as_secs_f32() * MS_PER_S,
                build_ms: f.build.as_secs_f32() * MS_PER_S,
                interval_ms: f.interval.map(|d| d.as_secs_f32() * MS_PER_S),
            })
            .collect();
        (frames, r.total)
    });
    let build: Vec<f32> = frames.iter().map(|f| f.build_ms).collect();
    let intervals: Vec<f32> = frames.iter().filter_map(|f| f.interval_ms).collect();
    let fps = percentiles(&intervals).map(|(p50, ..)| MS_PER_S / p50.max(MIN_INTERVAL_MS));
    // Over budget: an animation frame later than 1.5× the median interval.
    let janky = percentiles(&intervals).map(|(p50, ..)| intervals.iter().filter(|i| **i > p50 * JANK_FACTOR).count()).unwrap_or(0);
    json!({
        "frames": frames,
        "total_frames": total,
        "animating": frames.last().is_some_and(|f| f.age_ms < ANIMATING_WITHIN_MS),
        "build_ms": stats(&build),
        "interval_ms": stats(&intervals),
        "fps": fps,
        "janky": janky,
    })
}

/// `/proc/self/smaps_rollup` fields in KiB.
pub fn smaps_rollup(text: &str) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let kib = v.split_whitespace().next()?.parse().ok()?;
            Some((k.trim().to_string(), kib))
        })
        .collect()
}

/// Sums `drm-memory-*` / `drm-engine-*` from DRM fdinfo blocks (one per
/// client id) into `(memory KiB by region, engine busy ns by engine)`.
pub fn drm_usage(fdinfos: &[String]) -> (HashMap<String, u64>, HashMap<String, u64>) {
    let mut memory = HashMap::new();
    let mut engines = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for info in fdinfos {
        let field = |name: &str| info.lines().find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_string()));
        let Some(client) = field("drm-client-id:") else { continue };
        if !seen.insert(client) {
            continue;
        }
        for line in info.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let mut parts = v.split_whitespace();
            let (Some(n), unit) = (parts.next().and_then(|n| n.parse::<u64>().ok()), parts.next()) else { continue };
            if let Some(region) = k.strip_prefix("drm-memory-") {
                let kib = match unit {
                    Some("MiB") => n * KIB,
                    Some("GiB") => n * KIB * KIB,
                    Some("KiB") => n,
                    _ => n / KIB,
                };
                *memory.entry(region.trim().to_string()).or_default() += kib;
            } else if let Some(engine) = k.strip_prefix("drm-engine-")
                && unit == Some("ns")
            {
                *engines.entry(engine.trim().to_string()).or_default() += n;
            }
        }
    }
    (memory, engines)
}

fn own_drm_fdinfo() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir("/proc/self/fd") else { return Vec::new() };
    dir.flatten()
        .filter(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.starts_with("/dev/dri/")))
        .filter_map(|e| std::fs::read_to_string(format!("/proc/self/fdinfo/{}", e.file_name().to_string_lossy())).ok())
        .collect()
}

fn memory_report(cx: &gpui::App) -> Value {
    let roll = smaps_rollup(&std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default());
    let get = |k: &str| roll.get(k).copied().unwrap_or(0);
    let (gpu, engines) = drm_usage(&own_drm_fdinfo());
    let registry = cx.global::<PluginRegistry>();
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let manifests: Vec<_> = registry.manifests().collect();
    let enabled = manifests.iter().filter(|m| registry.is_enabled(&m.id, &cfg.plugins)).count();
    let third_party = manifests.iter().filter(|m| matches!(m.origin, PluginOrigin::ThirdParty { .. })).count();
    let hub = crate::hub::hub(cx).read(cx);
    let clip_images = hub.clipboard.entries.iter().filter(|e| matches!(e.content, mandara_services::clipboard::ClipContent::Image { .. })).count();
    json!({
        "rss_kib": get("Rss"),
        "pss_kib": get("Pss"),
        "categories_kib": {
            "heap_and_anon": get("Anonymous"),
            "file_mapped": get("Pss_File"),
            "shared_memory": get("Pss_Shmem"),
            "swap": get("Swap"),
        },
        "gpu_kib": gpu,
        "gpu_engine_ns": engines,
        "counts": {
            "plugins": manifests.len(),
            "plugins_enabled": enabled,
            "wasm_plugins": third_party,
            "windows": cx.windows().len(),
            "clipboard_entries": hub.clipboard.entries.len(),
            "clipboard_images": clip_images,
            "notifications": hub.notifications.items.len(),
            "tracked_clients": hub.clients.len(),
        },
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.command("core", CommandSpec::new("diagnostics frames", mandara_i18n::tk("Recent shell frame timings with p50 / p95 / p99 (build time and frame interval)"), |_, _| Ok(frames_report())).example("diagnostics frames"));
    r.command("core", CommandSpec::new("diagnostics views", mandara_i18n::tk("Render count and build cost per plugin view since the last reset"), |_, _| Ok(views_report(false))).example("diagnostics views"));
    r.command("core", CommandSpec::new("diagnostics views reset", mandara_i18n::tk("Prints and clears the per-view render statistics"), |_, _| Ok(views_report(true))).example("diagnostics views reset"));
    r.command("core", CommandSpec::new("diagnostics memory", mandara_i18n::tk("Shell memory by category: heap, file-mapped, shared, GPU, caches and plugins"), |_, cx| Ok(memory_report(cx))).example("diagnostics memory"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_of_sorted_copy() {
        let v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let (p50, p95, p99, max) = percentiles(&v).unwrap();
        assert_eq!((p50.round(), p95, p99, max), (51.0, 95.0, 99.0, 100.0));
        assert!(percentiles(&[]).is_none());
    }

    #[test]
    fn ring_records_intervals_only_while_continuous() {
        let t0 = Instant::now();
        let key = 0xfeed_u64;
        record(key, t0, Duration::from_micros(800));
        record(key, t0 + Duration::from_millis(7), Duration::from_micros(900));
        record(key, t0 + Duration::from_millis(500), Duration::from_micros(700));
        let frames: Vec<(Option<Duration>, Duration)> = with_ring(|r| r.frames.iter().rev().take(3).map(|f| (f.interval, f.build)).collect());
        assert_eq!(frames[2].0, None, "first frame has no predecessor");
        assert_eq!(frames[1].0, Some(Duration::from_millis(7)));
        assert_eq!(frames[0].0, None, "an idle gap is not a slow frame");
        for i in 0..(CAPACITY + 10) {
            record(key ^ 1, t0 + Duration::from_millis(i as u64), Duration::ZERO);
        }
        assert_eq!(with_ring(|r| r.frames.len()), CAPACITY);
        let report = frames_report();
        assert!(report["interval_ms"]["p50"].as_f64().is_some());
    }

    #[test]
    fn parses_smaps_and_drm_fdinfo() {
        let roll = smaps_rollup("55b0-7ffe ---p 00000000 00:00 0 [rollup]\nRss:  8220 kB\nAnonymous:  1212 kB\n");
        assert_eq!((roll["Rss"], roll["Anonymous"]), (8220, 1212));
        let a = "drm-driver:\tamdgpu\ndrm-client-id:\t7\ndrm-memory-vram:\t954840 KiB\ndrm-memory-gtt: \t4 MiB\ndrm-engine-gfx:\t1000 ns\n".to_string();
        let (mem, eng) = drm_usage(&[a.clone(), a]);
        assert_eq!(mem["vram"], 954840, "a client id is only counted once");
        assert_eq!(mem["gtt"], 4096);
        assert_eq!(eng["gfx"], 1000);
    }
}
