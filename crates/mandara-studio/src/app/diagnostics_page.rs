//! Studio → Performance → Diagnostics: a live frame-time graph of the
//! shell (from `diagnostics frames`) and a memory profiler (`diagnostics
//! memory`: heap / file-mapped / shared, GPU memory and busy time from the
//! DRM fdinfo, caches and plugins). Polled only while the page is open.

use mandara_ui::bidi::Bidi as _;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Context, FontWeight, Global, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use serde_json::Value;

use super::StudioView;
use super::compositor_page::{badge, card, heading};
use crate::registry::Section;
use crate::tokens::*;

/// Frames drawn in the graph.
const BARS: usize = 120;
/// Graph ceiling in ms (two 60 Hz frames).
const CEILING: f32 = 33.3;
/// How often the frame graph asks the shell for new frames.
const POLL: Duration = Duration::from_millis(250);
/// The memory profile is read every this many frame polls (2 s).
const MEMORY_EVERY_POLLS: u32 = 8;
/// Shortest interval GPU busy time is computed over, seconds.
const MIN_GPU_SAMPLE_SECS: f64 = 0.01;
/// Nanoseconds per second.
const NS_PER_S: f64 = 1e9;
/// Percent per whole.
const PERCENT: f64 = 100.0;
/// Height of the frame graph.
const GRAPH_H_PX: f32 = 96.0;
/// A frame interval longer than this many medians is late (coral).
const LATE_FACTOR: f32 = 1.5;
/// Milliseconds per second (frame budgets).
const MS_PER_S: f32 = 1000.0;
/// Refresh rates whose frame budgets the graph marks.
const HZ_144: f32 = 144.0;
const HZ_60: f32 = 60.0;
const KIB_PER_MIB: f64 = 1024.0;
const KIB_PER_GIB: f64 = KIB_PER_MIB * 1024.0;

#[derive(Default)]
struct Diagnostics {
    polling: bool,
    /// Shared with the render (cloning the `Rc` per frame, not the JSON).
    frames: Option<Rc<Result<Value, String>>>,
    memory: Option<Rc<Value>>,
    /// Previous GPU engine total (ns) and when it was read.
    engine: Option<(Instant, u64)>,
    gpu_busy: Option<f32>,
}
impl Global for Diagnostics {}

fn state(cx: &mut App) -> &mut Diagnostics {
    cx.default_global::<Diagnostics>()
}

fn f(v: &Value, path: &str) -> Option<f64> {
    v.pointer(path).and_then(Value::as_f64)
}

fn mib(kib: f64) -> String {
    if kib >= KIB_PER_GIB { format!("{:.2} GB", kib / KIB_PER_GIB) } else { format!("{:.0} MB", kib / KIB_PER_MIB) }
}

impl StudioView {
    fn ensure_diagnostics(cx: &mut Context<Self>) {
        if std::mem::replace(&mut state(cx).polling, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                let on_page = this.update(cx, |v, _| v.section == Section::Performance && v.query.is_empty()).unwrap_or(false);
                if !on_page {
                    let _ = this.update(cx, |_, cx| state(cx).polling = false);
                    return;
                }
                let frames = this.update(cx, |_, cx| crate::perf::query(&["diagnostics", "frames"], cx)).ok().flatten();
                let memory = if tick.is_multiple_of(MEMORY_EVERY_POLLS) { this.update(cx, |_, cx| crate::perf::query(&["diagnostics", "memory"], cx)).ok().flatten() } else { None };
                tick = tick.wrapping_add(1);
                let frames = match frames {
                    Some(task) => Some(task.await),
                    None => Some(Err(t("Studio isn't connected to the shell").to_string())),
                };
                let memory = match memory {
                    Some(task) => task.await.ok(),
                    None => None,
                };
                let _ = this.update(cx, |_, cx| {
                    let st = state(cx);
                    // Re-render only when something changed: an idle shell
                    // reports the same frames on every poll.
                    let mut changed = st.frames.as_deref() != frames.as_ref();
                    if changed {
                        st.frames = frames.map(Rc::new);
                    }
                    if let Some(m) = memory {
                        changed = true;
                        let total: u64 = m.get("gpu_engine_ns").and_then(Value::as_object).map(|o| o.values().filter_map(Value::as_u64).sum()).unwrap_or(0);
                        let now = Instant::now();
                        if let Some((at, prev)) = st.engine
                            && total >= prev
                        {
                            let secs = now.duration_since(at).as_secs_f64().max(MIN_GPU_SAMPLE_SECS);
                            st.gpu_busy = Some(((total - prev) as f64 / NS_PER_S / secs * PERCENT).clamp(0.0, PERCENT) as f32);
                        }
                        st.engine = (total > 0).then_some((now, total));
                        st.memory = Some(Rc::new(m));
                    }
                    if changed {
                        cx.notify();
                    }
                });
                cx.background_executor().timer(POLL).await;
            }
        })
        .detach();
    }

    pub(super) fn diagnostics(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        Self::ensure_diagnostics(cx);
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let st = state(cx);
        let frames = st.frames.clone();
        let memory = st.memory.clone();
        let gpu_busy = st.gpu_busy;

        // ── Frame graph ──
        let frame_card = match frames.as_deref() {
            None => card(p).child(heading(t("Frame timing"), t("Connecting to the shell…"), p)),
            Some(Err(e)) => card(p).child(heading(t("Frame timing"), tf("The shell didn't answer: {error}", &[("error", &e)]), p)),
            Some(Ok(v)) => {
                let list: Vec<(f32, Option<f32>)> = v["frames"]
                    .as_array()
                    .map(|a| a.iter().map(|fr| (fr["build_ms"].as_f64().unwrap_or(0.0) as f32, fr["interval_ms"].as_f64().map(|x| x as f32))).collect())
                    .unwrap_or_default();
                let animating = v["animating"].as_bool().unwrap_or(false);
                let median = f(v, "/interval_ms/p50").map(|x| x as f32);
                let shown = &list[list.len().saturating_sub(BARS)..];
                let graph_h = GRAPH_H_PX;
                let bar = |build: f32, interval: Option<f32>| {
                    let total = interval.unwrap_or(build).max(build);
                    let h = (total / CEILING).clamp(0.0, 1.0) * graph_h;
                    let janky = matches!((interval, median), (Some(i), Some(m)) if i > m * LATE_FACTOR);
                    let color: Color = if janky { p.urgent_coral } else { p.primary };
                    let build_h = (build / CEILING).clamp(0.0, 1.0) * graph_h;
                    div()
                        .flex_1()
                        .h(px(h.max(2.0)))
                        .rounded_t(px(RADIUS_1_5_PX))
                        .bg(color.with_alpha(ALPHA_35).hsla())
                        .row()
                        .flex_col()
                        .justify_end()
                        .child(div().w_full().h(px(build_h.min(h).max(1.0))).rounded_t(px(RADIUS_1_5_PX)).bg(color.hsla()))
                };
                // Budget lines at 144 Hz and 60 Hz.
                let line = |ms: f32, label: &'static str| {
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(ms / CEILING * graph_h))
                        .h(px(1.0))
                        .bg(p.on_surface.with_alpha(ALPHA_14).hsla())
                        .child(div().absolute().right_0().bottom(px(2.0)).text_size(px(TEXT_10_PX)).text_color(p.on_surface_variant.hsla()).child(label))
                };
                let graph = div()
                    .relative()
                    .h(px(graph_h))
                    .w_full()
                    .row()
                    .items_end()
                    .gap(px(1.0))
                    .children((0..BARS.saturating_sub(shown.len())).map(|_| div().flex_1().h(px(2.0)).bg(p.on_surface.with_alpha(ALPHA_5).hsla())))
                    .children(shown.iter().map(|(b, i)| bar(*b, *i)))
                    .child(line(MS_PER_S / HZ_144, "144 Hz"))
                    .child(line(MS_PER_S / HZ_60, "60 Hz"));
                let stat = |label: String, value: String, detail: String| {
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .p(px(SPACE_12_PX))
                        .rounded(px(RADIUS_14_PX))
                        .bg(p.surface_container_high.hsla())
                        .row()
                        .flex_col()
                        .gap(px(SPACE_3_PX))
                        .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(label))
                        .child(div().text_size(px(TEXT_20_PX)).font_weight(FontWeight::MEDIUM).font_family(mono.clone()).child(value))
                        .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(detail))
                };
                let ms = |path: &str| f(v, path).map(|x| format!("{x:.2} ms")).unwrap_or_else(|| "—".into());
                // 4 across, 2 × 2 when narrower, one column on the narrowest windows.
                let stats = [
                    stat(t("Frame rate").into(), f(v, "/fps").map(|x| format!("{x:.0} fps")).unwrap_or_else(|| "—".into()), tf("p95 interval {ms}", &[("ms", &ms("/interval_ms/p95"))])),
                    stat(t("Build time p50").into(), ms("/build_ms/p50"), tf("p95 {p95} · p99 {p99}", &[("p95", &ms("/build_ms/p95")), ("p99", &ms("/build_ms/p99"))])),
                    stat(t("Slowest build").into(), ms("/build_ms/max"), tf("of the last {count} frames", &[("count", &list.len())])),
                    stat(t("Late frames").into(), v["janky"].as_u64().unwrap_or(0).to_string(), tf("{total} frames drawn since start", &[("total", &v["total_frames"].as_u64().unwrap_or(0))])),
                ];
                let stats = super::grid::fit(self.page.column_w - 2.0 * SECTION_CARD_PAD_PX, STAT_TILE_GRID).balanced(stats.len()).render(stats);
                card(p)
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_10_PX))
                            .child(div().flex_1().min_w(px(0.0)).child(heading(
                                t("Frame timing"),
                                t("Each bar is one shell frame: the solid part is the time spent building it, the full height the time until the next frame. Coral bars came late."),
                                p,
                            )))
                            .child(if animating { badge(t("Animating"), p.success_emerald) } else { badge(t("Idle"), p.on_surface_variant) }),
                    )
                    .child(graph)
                    .when(!animating && shown.is_empty(), |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("No frames yet — the shell only draws while something changes. Open a panel to see it work.")))))
                    .child(stats)
            }
        };

        // ── Memory profiler ──
        let memory_card = match memory.as_deref() {
            None => card(p).child(heading(t("Memory profile"), t("Reading the shell's memory…"), p)),
            Some(m) => {
                let get = |path: &str| f(m, path).unwrap_or(0.0);
                let rss = get("/rss_kib").max(1.0);
                let parts = [
                    (t("Heap & buffers"), get("/categories_kib/heap_and_anon"), p.primary),
                    (t("Libraries & fonts (shared files)"), get("/categories_kib/file_mapped"), p.tertiary),
                    (t("Shared memory"), get("/categories_kib/shared_memory"), p.secondary),
                ];
                let total: f64 = parts.iter().map(|(_, v, _)| v).sum::<f64>().max(1.0);
                let stacked = div().h(px(DIM_14_PX)).w_full().rounded_full().overflow_hidden().row().bg(p.surface_dim.hsla()).children(parts.iter().filter(|(_, v, _)| *v > 0.0).map(|(_, v, c)| div().h_full().w(gpui::relative((*v / total) as f32)).bg(c.hsla())));
                let legend = div().row().flex_wrap().gap(px(SPACE_14_PX)).children(parts.iter().map(|(label, v, c)| {
                    div().row().items_center().gap(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).child(div().size(px(DIM_9_PX)).rounded_full().bg(c.hsla())).child(label.to_string()).child(div().font_family(mono.clone()).text_color(p.on_surface_variant.hsla()).child(mib(*v)))
                }));
                let gpu = m.get("gpu_kib").and_then(Value::as_object).cloned().unwrap_or_default();
                let mut gpu_parts: Vec<(String, f64)> = gpu.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).filter(|(_, v)| *v > 0.0).collect();
                gpu_parts.sort_by(|a, b| b.1.total_cmp(&a.1));
                let row = |label: String, value: String| div().row().justify_between().py(px(SPACE_5_PX)).child(div().text_color(p.on_surface_variant.hsla()).child(label)).child(div().font_family(mono.clone()).child(value));
                let counts = m.get("counts").cloned().unwrap_or(Value::Null);
                let count = |k: &str| counts[k].as_u64().unwrap_or(0);
                let gpu_block = div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).pb(px(SPACE_4_PX)).child(icon("monitor", ICON_14_PX, p.primary)).child(div().font_weight(FontWeight::SEMIBOLD).child(t("GPU"))))
                    .when(gpu_parts.is_empty(), |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("This driver doesn't report per-process GPU memory."))))
                    .children(gpu_parts.iter().map(|(k, v)| row(tf("{region} memory", &[("region", &k.to_uppercase())]), mib(*v))))
                    .when_some(gpu_busy, |el, b| el.child(row(t("GPU busy").into(), format!("{b:.1}%"))));
                let cache_block = div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).pb(px(SPACE_4_PX)).child(icon("memory", ICON_14_PX, p.primary)).child(div().font_weight(FontWeight::SEMIBOLD).child(t("Caches & plugins"))))
                    .child(row(t("Plugins (enabled / loaded)").into(), format!("{} / {}", count("plugins_enabled"), count("plugins"))))
                    .child(row(t("WASM plugins").into(), count("wasm_plugins").to_string()))
                    .child(row(t("Shell surfaces").into(), count("windows").to_string()))
                    .child(row(t("Clipboard history (images)").into(), format!("{} ({})", count("clipboard_entries"), count("clipboard_images"))))
                    .child(row(t("Notifications kept").into(), count("notifications").to_string()))
                    .child(row(t("Windows tracked").into(), count("tracked_clients").to_string()));
                card(p)
                    .child(heading(
                        t("Memory profile"),
                        tf("Resident {rss} · proportional share {pss} (shared libraries split between the processes using them)", &[("rss", &mib(rss)), ("pss", &mib(get("/pss_kib")))]),
                        p,
                    ))
                    .child(stacked)
                    .child(legend)
                    .child(div().pt(px(SPACE_6_PX)).child(super::grid::pair(self.page.column_w - 2.0 * SECTION_CARD_PAD_PX, PANEL_GRID.min_w, SPACE_28_PX, gpu_block, cache_block)))
            }
        };
        div().row().flex_col().gap(px(SPACE_14_PX)).child(frame_card).child(memory_card).into_any_element()
    }
}
