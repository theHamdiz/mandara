//! Studio → Power & idle: what happens when you step away, how the screen
//! locks, battery alerts and power profiles, the power / session menu and
//! event hooks.
//!
//! * **Status** — live from the shell (`mandara msg idle status`): idle
//!   time, what keeps the computer awake, the power source and a timeline
//!   of the stages to come.
//! * **Timeouts** — mains power and battery side by side, one chip row per
//!   stage (lock, screens off, sleep).
//! * **Your idle commands**, the **session menu** (order, visibility,
//!   labels, custom commands, boot entries, a live miniature) and **hooks**
//!   (event, command, test run, recent results) are list editors; every
//!   scalar option is a registry row (`Section::Power`).

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Bounds, ClickEvent, Pixels, canvas, point, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::power::{HookEvent, HookRule, IdleBehavior, IdleTimeouts, PowerSource, SessionEntry, SessionEntryKind, SessionMenuStyle};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::{any_icon, icon};
use mandara_ui::powerctl::session_menu::{entry_icon, entry_label, kind_label};
use mandara_ui::state::update_config;
use serde_json::Value;

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title};
use crate::registry::{self, Section};
use crate::tokens::*;

// ── Page constants ──────────────────────────────────────────────────────────

/// Live status refresh while the page shows.
const STATUS_REFRESH: Duration = Duration::from_secs(1);
/// Hook history refresh.
const HOOKS_REFRESH: Duration = Duration::from_secs(4);
/// Duration chips of the timeout rows, seconds (0 = never).
const TIMEOUT_CHOICES_S: &[u32] = &[0, 60, 120, 300, 600, 900, 1800, 3600];
/// Keep-awake button duration, minutes.
const KEEP_AWAKE_MIN: &str = "60";
/// Two timeout cards side by side above this width each.
const TIMEOUT_CARD_MIN_W_PX: f32 = 320.0;
/// The status card's state disc and its glyph.
const DISC_PX: f32 = 56.0;
const DISC_ICON_PX: f32 = 26.0;
/// Timeline track.
const TRACK_H_PX: f32 = 6.0;
const MARK_PX: f32 = 12.0;
const TIMELINE_LABEL_W_PX: f32 = 96.0;
const TIMELINE_H_PX: f32 = 54.0;
/// Session-menu miniature tiles.
const MINI_TILE_PX: f32 = 34.0;
const MINI_ICON_PX: f32 = 15.0;
/// List row app glyph.
const ROW_ICON_PX: f32 = 16.0;
/// Hook runs listed.
const RECENT_RUNS: usize = 5;
/// Seconds per minute / hour (duration labels).
const SECS_PER_MIN: u32 = 60;
const SECS_PER_HOUR: u32 = 3600;
/// Hook events by theme, for the event picker.
const EVENT_GROUPS: &[(&str, &[HookEvent])] = &[
    (tk("Session"), &[HookEvent::Started, HookEvent::Locked, HookEvent::Unlocked, HookEvent::BeforeLogout, HookEvent::BeforeReboot, HookEvent::BeforeShutdown]),
    (tk("Idle & sleep"), &[HookEvent::Idle, HookEvent::Resumed, HookEvent::ScreensOff, HookEvent::ScreensOn, HookEvent::BeforeSleep, HookEvent::AfterSleep]),
    (
        tk("Power"),
        &[HookEvent::PluggedIn, HookEvent::Unplugged, HookEvent::BatteryLevel, HookEvent::BatteryLow, HookEvent::BatteryCritical, HookEvent::BatteryFull, HookEvent::PowerProfileChanged],
    ),
    (tk("Look"), &[HookEvent::WallpaperChanged, HookEvent::ColorsChanged, HookEvent::ThemeModeChanged, HookEvent::TemplatesRendered]),
    (tk("Connections"), &[HookEvent::WifiOn, HookEvent::WifiOff, HookEvent::BluetoothOn, HookEvent::BluetoothOff]),
];

/// Registry groups shown under each bespoke part of the page.
const IDLE_GROUPS: &[&str] = &[tk("When idle"), tk("Locking")];
const POWER_GROUPS: &[&str] = &[tk("Battery alerts"), tk("Power profiles")];
const MENU_GROUPS: &[&str] = &[tk("Session menu")];
const HOOK_GROUPS: &[&str] = &[tk("Hooks")];

/// What the shell reported last.
#[derive(Default)]
struct PowerStudio {
    status: Option<Result<Value, String>>,
    status_at: Option<Instant>,
    hooks: Option<Value>,
    hooks_at: Option<Instant>,
    boot: Option<mandara_services::sleep::RebootTargets>,
    /// Hook rule whose event picker is open.
    picking_event: Option<usize>,
    /// Result of the last button (test run, preview…): (text, ok).
    message: Option<(String, bool)>,
    alive: bool,
    /// Where each part of the page was painted (deep links scroll there).
    anchors: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    /// A deep-linked part to scroll to, and frames waited for its anchor.
    pending_scroll: Option<(&'static str, u8)>,
}
impl Global for PowerStudio {}

/// Parts `mandara studio power/<part>` can scroll to.
pub(crate) const PARTS: &[&str] = &["status", "timeouts", "idle", "commands", "battery", "menu", "hooks"];
/// Frames to let pass before trusting recorded anchors, and the re-check
/// delay meanwhile.
const ANCHOR_FRAMES: u8 = 2;
const ANCHOR_SETTLE: Duration = Duration::from_millis(32);
/// Space kept above a part scrolled to.
const SCROLL_MARGIN_PX: f32 = 12.0;

fn st(cx: &mut gpui::App) -> &mut PowerStudio {
    cx.default_global::<PowerStudio>()
}

/// `90` → "1 min 30 s", `600` → "10 min", `0` → "Never".
pub(crate) fn duration_label(s: u32) -> String {
    match s {
        0 => t("Never").to_string(),
        s if s >= SECS_PER_HOUR && s % SECS_PER_HOUR == 0 => tf("{count} h", &[("count", &(s / SECS_PER_HOUR))]),
        s if s >= SECS_PER_MIN && s % SECS_PER_MIN == 0 => tf("{count} min", &[("count", &(s / SECS_PER_MIN))]),
        s if s > SECS_PER_MIN => tf("{min} min {sec} s", &[("min", &(s / SECS_PER_MIN)), ("sec", &(s % SECS_PER_MIN))]),
        s => tf("{count} s", &[("count", &s)]),
    }
}

/// `125` → "2:05".
fn clock_label(s: u64) -> String {
    format!("{}:{:02}", s / u64::from(SECS_PER_MIN), s % u64::from(SECS_PER_MIN))
}

fn stage_label(stage: &Value) -> &'static str {
    match stage.as_str().unwrap_or_default() {
        "fade" => t("Fade"),
        "lock" => t("Lock"),
        "screens-off" => t("Screens off"),
        "suspend" => t("Sleep"),
        "locked-screens-off" => t("Screens off (locked)"),
        _ => t("Your command"),
    }
}

impl StudioView {
    fn power_loop(cx: &mut Context<Self>) {
        if st(cx).alive {
            return;
        }
        st(cx).alive = true;
        cx.spawn(async move |this, cx| {
            loop {
                let on_page = this.update(cx, |v, _| v.section == Section::Power && v.query.is_empty()).unwrap_or(false);
                if !on_page {
                    let _ = this.update(cx, |_, cx| {
                        // Keep a deep link that is about to open the page.
                        let pending = st(cx).pending_scroll;
                        *st(cx) = PowerStudio { pending_scroll: pending, ..Default::default() };
                    });
                    return;
                }
                let status = this.update(cx, |_, cx| crate::perf::query(&["idle", "status"], cx)).ok().flatten();
                if let Some(task) = status {
                    let result = task.await;
                    let _ = this.update(cx, |_, cx| {
                        let s = st(cx);
                        s.status = Some(result);
                        s.status_at = Some(Instant::now());
                        cx.notify();
                    });
                }
                let hooks_due = this.update(cx, |_, cx| st(cx).hooks_at.is_none_or(|t| t.elapsed() >= HOOKS_REFRESH)).unwrap_or(false);
                if hooks_due && let Some(task) = this.update(cx, |_, cx| crate::perf::query(&["hooks", "status"], cx)).ok().flatten() {
                    let result = task.await.ok();
                    let _ = this.update(cx, |_, cx| {
                        let s = st(cx);
                        s.hooks = result;
                        s.hooks_at = Some(Instant::now());
                        cx.notify();
                    });
                }
                cx.background_executor().timer(STATUS_REFRESH).await;
            }
        })
        .detach();
        // Restart targets come straight from logind (no shell needed).
        cx.spawn(async move |this, cx| {
            let targets = cx.background_executor().spawn(async { mandara_services::sleep::reboot_targets() }).await;
            let _ = this.update(cx, |_, cx| {
                st(cx).boot = Some(targets);
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a shell command from a button and shows its outcome.
    fn power_command(args: &[&str], done: &'static str, cx: &mut Context<Self>) {
        let Some(task) = crate::perf::query(args, cx) else {
            st(cx).message = Some((t("Mandara Shell isn't running").into(), false));
            cx.notify();
            return;
        };
        cx.spawn(async move |_, cx| {
            let result = task.await;
            cx.update(|cx| {
                st(cx).message = Some(match result {
                    Ok(_) => (tr(done), true),
                    Err(e) => (e, false),
                });
                cx.refresh_windows();
            });
        })
        .detach();
    }

    /// Opens the page scrolled to `part` (one of [`PARTS`]).
    pub fn show_power_part(&mut self, part: &str, cx: &mut Context<Self>) {
        if let Some(part) = PARTS.iter().copied().find(|p| *p == part) {
            self.section = Section::Power;
            st(cx).pending_scroll = Some((part, 0));
            cx.notify();
        }
    }

    fn power_scroll(&mut self, cx: &mut Context<Self>) {
        let Some((part, frames)) = st(cx).pending_scroll else { return };
        let anchor = st(cx).anchors.borrow().get(part).copied();
        let viewport = self.content_scroll.bounds();
        match anchor {
            Some(b) if frames >= ANCHOR_FRAMES && viewport.size.height > px(0.0) => {
                let offset = self.content_scroll.offset();
                let max = self.content_scroll.max_offset();
                let y = (offset.y - (b.top() - viewport.top()) + px(SCROLL_MARGIN_PX)).min(px(0.0)).max(-max.y);
                self.content_scroll.set_offset(point(offset.x, y));
                st(cx).pending_scroll = None;
                cx.notify();
            }
            _ => {
                st(cx).pending_scroll = Some((part, frames.saturating_add(1)));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(ANCHOR_SETTLE).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
    }

    pub(super) fn power_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        Self::power_loop(cx);
        self.power_scroll(cx);
        let anchors = st(cx).anchors.clone();
        let anchored = move |part: &'static str, el: AnyElement| {
            let anchors = anchors.clone();
            div()
                .relative()
                .child(el)
                .child(canvas(move |bounds, _, _| {
                    anchors.borrow_mut().insert(part, bounds);
                }, |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full())
        };
        let in_groups = |groups: &'static [&'static str]| registry::in_section(Section::Power).filter(move |s| groups.contains(&s.group));
        let status = self.idle_status(cfg, p, cx);
        let timeouts = self.idle_timeouts(cfg, p, cx);
        let idle_rows = self.rows(in_groups(IDLE_GROUPS), cfg, p, cx);
        let behaviors = self.idle_behaviors(cfg, p, cx);
        let power_rows = self.rows(in_groups(POWER_GROUPS), cfg, p, cx);
        let menu = self.session_menu_editor(cfg, p, cx);
        let menu_rows = self.rows(in_groups(MENU_GROUPS), cfg, p, cx);
        let hooks = self.hooks_editor(cfg, p, cx);
        let hook_rows = self.rows(in_groups(HOOK_GROUPS), cfg, p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(anchored("status", status))
            .child(anchored("timeouts", timeouts))
            .child(anchored("idle", idle_rows))
            .child(anchored("commands", behaviors))
            .child(anchored("battery", power_rows))
            .child(anchored("menu", menu_rows))
            .child(menu)
            .child(anchored("hooks", hook_rows))
            .child(hooks)
            .into_any_element()
    }

    // ── Status ──────────────────────────────────────────────────────────────

    fn idle_status(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let s = st(cx);
        let status = s.status.clone();
        let message = s.message.clone();
        let enabled = cfg.idle.enabled;
        let get = |k: &str| status.as_ref().and_then(|r| r.as_ref().ok()).and_then(|v| v.get(k)).cloned().unwrap_or(Value::Null);
        let idle_for = get("idle_for_s").as_u64();
        let inhibited: Vec<String> = get("inhibited_by").as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let on_battery = get("power_source").as_str() == Some("battery");
        let keep_awake = inhibited.iter().any(|i| i == "keep-awake");
        let (glyph, title, accent) = match (&status, enabled) {
            (Some(Err(_)), _) => ("alert", t("Mandara Shell isn't running").to_string(), p.warning_amber),
            (_, false) => ("moon", t("Idle handling is off").to_string(), p.on_surface_variant),
            _ if keep_awake => ("coffee", t("Kept awake").to_string(), p.warning_amber),
            _ if inhibited.iter().any(|i| i == "media") => ("music", t("Awake while media plays").to_string(), p.tertiary),
            _ => match idle_for {
                Some(secs) => ("moon", tf("Idle for {time}", &[("time", &clock_label(secs))]), p.secondary),
                None => ("sun", t("Active").to_string(), p.primary),
            },
        };
        let detail = if !enabled {
            t("Turn it on to fade, lock, turn the screens off and sleep after the times below. Leave it off if hypridle or swayidle handles this.").to_string()
        } else if on_battery {
            t("On battery — the battery timeouts apply").to_string()
        } else {
            t("On mains power — the mains timeouts apply").to_string()
        };
        let caps = get("capabilities");
        let cap = |k: &str| caps.get(k).and_then(Value::as_bool).unwrap_or(false);

        let disc = div()
            .size(px(DISC_PX))
            .flex_none()
            .rounded_full()
            .bg(accent.with_alpha(ALPHA_16).hsla())
            .row()
            .items_center()
            .justify_center()
            .child(icon(glyph, DISC_ICON_PX, accent));
        let toggle = div()
            .id("idle-enabled")
            .cursor_pointer()
            .child(mandara_ui::plugins::network::switch(enabled, p))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, move |c| c.idle.enabled = !enabled)));
        let mut badges = div().row().flex_wrap().gap(px(SPACE_6_PX));
        if status.as_ref().is_some_and(Result::is_ok) {
            badges = badges.child(badge(if on_battery { t("Battery") } else { t("Mains power") }, if on_battery { p.warning_amber } else { p.success_emerald }));
            // Capabilities are known once the shell's idle client connected.
            if enabled && caps.is_object() {
                badges = badges
                    .child(badge(if cap("idle_notify") { t("Idle detection ✓") } else { t("No idle detection") }, if cap("idle_notify") { p.success_emerald } else { p.urgent_coral }))
                    .child(badge(if cap("output_power") { t("Screen power ✓") } else { t("Screens via compositor") }, if cap("output_power") { p.success_emerald } else { p.warning_amber }));
            }
            if get("dry_run").as_bool() == Some(true) {
                badges = badges.child(badge(t("Dry run"), p.tertiary));
            }
        }
        let actions = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_8_PX))
            .child(button("idle-preview", "sun", t("Preview the fade"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::power_command(&["idle", "fade-preview"], tk("Fading — move the mouse to end it"), cx))))
            .child(if keep_awake {
                button("idle-awake", "coffee", t("Stop keeping awake"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::power_command(&["idle", "keep-awake", "0"], tk("Idle stages are back on"), cx)))
            } else {
                button("idle-awake", "coffee", t("Keep awake for an hour"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::power_command(&["idle", "keep-awake", KEEP_AWAKE_MIN], tk("Kept awake for an hour"), cx)))
            })
            .when_some(message, |el, (text, ok)| el.child(div().row().items_center().text_size(px(TEXT_12_PX)).text_color(if ok { p.on_surface_variant.hsla() } else { p.urgent_coral.hsla() }).child(text)));

        let timeline = if enabled { self.timeline(&get("stages"), idle_for, p) } else { None };
        card(p)
            .gap(px(SPACE_16_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_16_PX))
                    .child(disc)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_6_PX))
                            .child(div().text_size(px(TEXT_18_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(title))
                            .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(detail)))
                            .child(badges),
                    )
                    .child(div().row().flex_col().cross_end().gap(px(SPACE_4_PX)).child(toggle).child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(t("Mandara handles idle")))),
            )
            .when_some(timeline, |el, tl| el.child(tl))
            .child(actions)
            .into_any_element()
    }

    /// The stages ahead on one track: markers at each stage's time, the
    /// idle time so far as the fill.
    fn timeline(&self, stages: &Value, idle_for: Option<u64>, p: &MandaraPalette) -> Option<AnyElement> {
        let stages: Vec<(&'static str, u64, u64)> = stages
            .as_array()?
            .iter()
            .filter(|s| s.get("stage").and_then(Value::as_str) != Some("locked-screens-off"))
            .filter_map(|s| Some((stage_label(s.get("stage")?), s.get("after_s")?.as_u64()?, s.get("in_s")?.as_u64()?)))
            .collect();
        let end = stages.iter().map(|s| s.1).max().filter(|e| *e > 0)?;
        let width = (self.page.column_w - 2.0 * SPACE_18_PX).max(DIM_200_PX);
        let usable = width - TIMELINE_LABEL_W_PX;
        let x_of = |secs: u64| TIMELINE_LABEL_W_PX / 2.0 + usable * (secs as f32 / end as f32).min(1.0);
        let fill = x_of(idle_for.unwrap_or(0)) - TIMELINE_LABEL_W_PX / 2.0;
        let mut track = div()
            .relative()
            .w(px(width))
            .h(px(TIMELINE_H_PX))
            .child(div().absolute().left(px(TIMELINE_LABEL_W_PX / 2.0)).top(px(MARK_PX / 2.0 - TRACK_H_PX / 2.0)).w(px(usable)).h(px(TRACK_H_PX)).rounded_full().bg(p.surface_container_high.hsla()))
            .child(div().absolute().left(px(TIMELINE_LABEL_W_PX / 2.0)).top(px(MARK_PX / 2.0 - TRACK_H_PX / 2.0)).w(px(fill.max(0.0))).h(px(TRACK_H_PX)).rounded_full().bg(p.primary.hsla()));
        for (label, after, left) in stages {
            let x = x_of(after);
            let passed = left == 0 && idle_for.is_some();
            track = track
                .child(
                    div()
                        .absolute()
                        .left(px(x - MARK_PX / 2.0))
                        .top_0()
                        .size(px(MARK_PX))
                        .rounded_full()
                        .bg(if passed { p.primary.hsla() } else { p.surface_container.hsla() })
                        .border_2()
                        .border_color(p.primary.hsla()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x - TIMELINE_LABEL_W_PX / 2.0))
                        .top(px(MARK_PX + SPACE_6_PX))
                        .w(px(TIMELINE_LABEL_W_PX))
                        .row()
                        .flex_col()
                        .items_center()
                        .child(div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::MEDIUM).child(label))
                        .child(div().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(duration_label(u32::try_from(after).unwrap_or(u32::MAX)))),
                );
        }
        Some(track.into_any_element())
    }

    // ── Timeouts ────────────────────────────────────────────────────────────

    fn idle_timeouts(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let side = |battery: bool, t_: IdleTimeouts, cx: &mut Context<Self>| {
            let row = |stage: &'static str, label: &'static str, current: u32, set: fn(&mut IdleTimeouts, u32), cx: &mut Context<Self>| {
                let mut chips = div().row().flex_wrap().gap(px(SPACE_4_PX));
                let mut choices = TIMEOUT_CHOICES_S.to_vec();
                if !choices.contains(&current) {
                    choices.push(current);
                    choices.sort_unstable();
                }
                for secs in choices {
                    let id = SharedString::from(format!("idle-{}-{stage}-{secs}", if battery { "bat" } else { "ac" }));
                    chips = chips.child(chip(id, duration_label(secs), secs == current, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        update_config(cx, move |c| set(if battery { &mut c.idle.battery } else { &mut c.idle.ac }, secs));
                    })));
                }
                div().row().flex_col().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(label)))).child(chips)
            };
            let lock = row("lock", tk("Lock after"), t_.lock_s, |t, v| t.lock_s = v, cx);
            let screens = row("screens", tk("Turn screens off after"), t_.screen_off_s, |t, v| t.screen_off_s = v, cx);
            let sleep = row("sleep", tk("Sleep after"), t_.suspend_s, |t, v| t.suspend_s = v, cx);
            card(p)
                .flex_1()
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(icon(if battery { "battery" } else { "plug" }, ICON_16_PX, p.primary))
                        .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(if battery { t("On battery") } else { t("On mains power") })),
                )
                .child(lock)
                .child(screens)
                .child(sleep)
        };
        let ac = side(false, cfg.idle.ac, cx);
        let bat = side(true, cfg.idle.battery, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Timeouts"), p))
            .child(super::grid::pair(self.page.column_w, TIMEOUT_CARD_MIN_W_PX, SPACE_14_PX, ac, bat))
            .into_any_element()
    }

    // ── Your idle commands ──────────────────────────────────────────────────

    fn idle_behaviors(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let stack = self.stack_rows;
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, b) in cfg.idle.behaviors.iter().enumerate() {
            let name = self.list_field(format!("idle-b-{i}-name"), &b.name, t("Name"), Rc::new(move |c, v| if let Some(b) = c.idle.behaviors.get_mut(i) { b.name = v }), cx);
            let after = self.list_field(
                format!("idle-b-{i}-after"),
                &b.timeout_s.to_string(),
                t("seconds"),
                Rc::new(move |c, v| {
                    if let (Some(b), Ok(n)) = (c.idle.behaviors.get_mut(i), v.trim().parse::<u32>()) {
                        b.timeout_s = n;
                    }
                }),
                cx,
            );
            let command = self.list_field(format!("idle-b-{i}-cmd"), &b.command, t("command, e.g. brightnessctl set 10%"), Rc::new(move |c, v| if let Some(b) = c.idle.behaviors.get_mut(i) { b.command = v }), cx);
            let resume = self.list_field(format!("idle-b-{i}-resume"), &b.resume_command, t("on return (optional), e.g. brightnessctl -r"), Rc::new(move |c, v| if let Some(b) = c.idle.behaviors.get_mut(i) { b.resume_command = v }), cx);
            let mut sources = div().row().gap(px(SPACE_4_PX));
            for (src, label) in [(PowerSource::Any, tk("Always")), (PowerSource::Ac, tk("Mains")), (PowerSource::Battery, tk("Battery"))] {
                sources = sources.child(chip(SharedString::from(format!("idle-b-{i}-src-{label}")), tr(label), b.power == src, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_config(cx, move |c| {
                        if let Some(b) = c.idle.behaviors.get_mut(i) {
                            b.power = src;
                        }
                    })
                })));
            }
            let on = b.enabled;
            list = list.child(
                row_card(p)
                    .child(
                        div()
                            .row()
                            .when(stack, |el| el.flex_col())
                            .gap(px(SPACE_8_PX))
                            .items_center()
                            .child(div().id(SharedString::from(format!("idle-b-{i}-on"))).cursor_pointer().child(mandara_ui::plugins::network::switch(on, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                update_config(cx, move |c| {
                                    if let Some(b) = c.idle.behaviors.get_mut(i) {
                                        b.enabled = !on;
                                    }
                                })
                            })))
                            .child(div().w(px(DIM_180_PX)).child(name))
                            .child(div().row().items_center().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("after"))).child(div().w(px(DIM_76_PX)).child(after)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("s idle"))))
                            .child(sources)
                            .child(div().flex_1())
                            .child(delete_button(SharedString::from(format!("idle-b-{i}-del")), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.list_fields.clear();
                                update_config(cx, move |c| {
                                    if i < c.idle.behaviors.len() {
                                        c.idle.behaviors.remove(i);
                                    }
                                });
                            }))),
                    )
                    .child(command)
                    .child(resume),
            );
        }
        list = list.child(div().row().child(add_button("idle-b-add", t("Add an idle command"), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.list_fields.clear();
            update_config(cx, |c| c.idle.behaviors.push(IdleBehavior { name: t("Dim the keyboard").into(), timeout_s: 120, command: "brightnessctl --device='*::kbd_backlight' set 0".into(), resume_command: "brightnessctl --device='*::kbd_backlight' -r".into(), ..Default::default() }));
        }))));
        section(
            t("Your idle commands"),
            t("Run anything after some idle time, and something else when you're back. Commands run directly, without a shell — write sh -c '…' for pipes."),
            list,
            p,
        )
    }

    // ── Session menu ───────────────────────────────────────────────────────

    fn session_menu_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let entries = cfg.session_menu.entries.clone();
        let n = entries.len();
        let boot = st(cx).boot.clone();
        let mut list = div().row().flex_col().gap(px(SPACE_6_PX));
        for (i, e) in entries.iter().enumerate() {
            let label = self.list_field(format!("menu-{i}-label"), &e.label, kind_label(e.kind), Rc::new(move |c, v| if let Some(e) = c.session_menu.entries.get_mut(i) { e.label = v }), cx);
            let on = e.enabled;
            let arrow = |id: String, glyph: &'static str, enabled: bool, to: usize, cx: &mut Context<Self>| {
                div()
                    .id(SharedString::from(id))
                    .size(px(DIM_28_PX))
                    .flex_none()
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .when(enabled, |el| el.cursor_pointer().hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
                    .when(!enabled, |el| el.opacity(ALPHA_30))
                    .child(icon(glyph, ICON_14_PX, p.on_surface_variant))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !enabled {
                            return;
                        }
                        this.list_fields.clear();
                        update_config(cx, move |c| {
                            if i < c.session_menu.entries.len() && to < c.session_menu.entries.len() {
                                c.session_menu.entries.swap(i, to);
                            }
                        });
                    }))
            };
            let up = arrow(format!("menu-{i}-up"), "arrow-up", i > 0, i.saturating_sub(1), cx);
            let down = arrow(format!("menu-{i}-down"), "arrow-down", i + 1 < n, i + 1, cx);
            let removable = matches!(e.kind, SessionEntryKind::Custom | SessionEntryKind::BootEntry);
            let mut row = row_card(p).child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(up)
                    .child(down)
                    .child(div().size(px(DIM_30_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(if e.kind.is_destructive() { p.urgent_coral.with_alpha(ALPHA_14).hsla() } else { p.primary.with_alpha(ALPHA_14).hsla() }).row().items_center().justify_center().child(any_icon(&entry_icon(e), ROW_ICON_PX, if e.kind.is_destructive() { p.urgent_coral } else { p.primary })))
                    .child(div().flex_1().min_w(px(DIM_120_PX)).child(label))
                    // Renamed entries keep showing what they do.
                    .when(!e.label.trim().is_empty(), |el| el.child(badge(tr(kind_label(e.kind)), p.on_surface_variant)))
                    .child(div().id(SharedString::from(format!("menu-{i}-on"))).cursor_pointer().child(mandara_ui::plugins::network::switch(on, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        update_config(cx, move |c| {
                            if let Some(e) = c.session_menu.entries.get_mut(i) {
                                e.enabled = !on;
                            }
                        })
                    })))
                    .when(removable, |el| {
                        el.child(delete_button(SharedString::from(format!("menu-{i}-del")), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.list_fields.clear();
                            update_config(cx, move |c| {
                                if i < c.session_menu.entries.len() {
                                    c.session_menu.entries.remove(i);
                                }
                            });
                        })))
                    }),
            );
            if e.kind == SessionEntryKind::Custom {
                let command = self.list_field(format!("menu-{i}-cmd"), &e.command, t("command, e.g. systemctl --user restart pipewire"), Rc::new(move |c, v| if let Some(e) = c.session_menu.entries.get_mut(i) { e.command = v }), cx);
                row = row.child(command);
            }
            if e.kind == SessionEntryKind::BootEntry {
                let field = self.list_field(format!("menu-{i}-boot"), &e.boot_entry, t("boot entry id, e.g. windows.conf"), Rc::new(move |c, v| if let Some(e) = c.session_menu.entries.get_mut(i) { e.boot_entry = v.trim().to_string() }), cx);
                let mut options = div().row().flex_wrap().gap(px(SPACE_4_PX));
                for id in boot.as_ref().map(|b| b.boot_entries.clone()).unwrap_or_default() {
                    let active = id == e.boot_entry;
                    let pick = id.clone();
                    options = options.child(chip(SharedString::from(format!("menu-{i}-boot-{id}")), id.clone(), active, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.list_fields.clear();
                        let pick = pick.clone();
                        update_config(cx, move |c| {
                            if let Some(e) = c.session_menu.entries.get_mut(i) {
                                e.boot_entry = pick;
                            }
                        });
                    })));
                }
                row = row.child(field).child(options);
            }
            list = list.child(row);
        }
        let firmware = boot.as_ref().map(|b| b.firmware.clone()).unwrap_or_default();
        let entries_supported = boot.as_ref().is_some_and(|b| !b.boot_entries.is_empty());
        let adds = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_8_PX))
            .child(add_button("menu-add-cmd", t("Add a command"), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.list_fields.clear();
                update_config(cx, |c| c.session_menu.entries.push(SessionEntry { kind: SessionEntryKind::Custom, enabled: true, label: t("My command").into(), ..Default::default() }));
            })))
            .when(entries_supported, |el| {
                el.child(add_button("menu-add-boot", t("Add a boot entry"), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.list_fields.clear();
                    update_config(cx, |c| c.session_menu.entries.push(SessionEntry::new(SessionEntryKind::BootEntry, true)));
                })))
            })
            .child(div().row().items_center().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(match firmware.as_str() {
                "yes" | "challenge" => t("Firmware setup restarts are supported here").to_string(),
                "" => String::new(),
                _ => t("This machine can't restart into the firmware setup").to_string(),
            }));
        let preview = self.menu_preview(cfg, p);
        section(
            t("Menu entries"),
            t("Choose what the power menu offers and in which order. Built-in entries can be hidden or renamed; your own commands and boot entries can be removed."),
            div().row().flex_col().gap(px(SPACE_12_PX)).child(preview).child(list).child(adds),
            p,
        )
    }

    /// A miniature of the menu as it will open.
    fn menu_preview(&self, cfg: &ShellConfig, p: &MandaraPalette) -> AnyElement {
        let menu = &cfg.session_menu;
        let visible: Vec<&SessionEntry> = menu.visible().collect();
        let tiles = visible.iter().enumerate().map(|(i, e)| {
            let selected = e.kind.id() == menu.default_action;
            let (bg, fg) = if selected { if e.kind.is_destructive() { (p.urgent_coral, p.on_primary) } else { (p.primary, p.on_primary) } } else { (p.surface_container_high, p.on_surface) };
            div()
                .relative()
                .size(px(MINI_TILE_PX))
                .flex_none()
                .rounded(px(RADIUS_10_PX))
                .bg(bg.hsla())
                .row()
                .items_center()
                .justify_center()
                .child(any_icon(&entry_icon(e), MINI_ICON_PX, fg))
                .when(menu.show_shortcuts && i < 9, |el| el.child(div().absolute().top(px(1.0)).inset_end(px(SPACE_3_PX)).text_size(px(TEXT_9_5_PX)).text_color(fg.with_alpha(ALPHA_70).hsla()).child((i + 1).to_string())))
        });
        let grid = menu.style == SessionMenuStyle::Grid;
        let cols = usize::from(menu.grid_columns).clamp(1, visible.len().max(1));
        let body = if grid {
            div().w(px(cols as f32 * (MINI_TILE_PX + SPACE_6_PX))).row().flex_wrap().gap(px(SPACE_6_PX)).children(tiles)
        } else {
            div().row().flex_col().gap(px(SPACE_6_PX)).children(tiles)
        };
        let hint = match visible.iter().find(|e| e.kind.id() == menu.default_action) {
            Some(e) => tf("Opens on “{entry}”", &[("entry", &entry_label(e))]),
            None => t("Opens on the first entry").to_string(),
        };
        div()
            .row()
            .items_center()
            .gap(px(SPACE_18_PX))
            .child(div().p(px(SPACE_10_PX)).rounded(px(RADIUS_16_PX)).bg(p.surface_dim.hsla()).child(body))
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(if grid { t("Grid") } else { t("Edge dock") }))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(hint))
                    .when(menu.hold_to_confirm, |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tf("Hold {ms} ms to log out, restart or shut down", &[("ms", &menu.hold_ms)])))),
            )
            .into_any_element()
    }

    // ── Hooks ──────────────────────────────────────────────────────────────

    fn hooks_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let picking = st(cx).picking_event;
        let history = st(cx).hooks.clone();
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, rule) in cfg.hooks.rules.iter().enumerate() {
            let command = self.list_field(format!("hook-{i}-cmd"), &rule.command, t("command — e.g. notify-send Locked, mandara:theme mode dark or skill:Focus"), Rc::new(move |c, v| if let Some(r) = c.hooks.rules.get_mut(i) { r.command = v }), cx);
            let on = rule.enabled;
            let event = rule.event;
            let mut row = row_card(p).child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().id(SharedString::from(format!("hook-{i}-on"))).cursor_pointer().child(mandara_ui::plugins::network::switch(on, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        update_config(cx, move |c| {
                            if let Some(r) = c.hooks.rules.get_mut(i) {
                                r.enabled = !on;
                            }
                        })
                    })))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("When")))
                    .child(chip(SharedString::from(format!("hook-{i}-event")), event.id(), picking == Some(i), p).child(icon(mandara_ui::bidi::dir_icon("chevron-right"), ICON_11_PX, p.on_surface_variant)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let s = st(cx);
                        s.picking_event = if s.picking_event == Some(i) { None } else { Some(i) };
                        cx.notify();
                    })))
                    .when(event.blocks(), |el| el.child(badge(t("waits for it"), p.tertiary)))
                    .child(div().flex_1())
                    .child(button(SharedString::from(format!("hook-{i}-test")), "play", t("Test"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        // This row's rule only, even while it's switched off.
                        let index = i.to_string();
                        Self::power_command(&["hooks", "test", event.id(), &index], tk("Hook ran — see recent runs"), cx);
                        st(cx).hooks_at = None;
                    })))
                    .child(delete_button(SharedString::from(format!("hook-{i}-del")), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.list_fields.clear();
                        st(cx).picking_event = None;
                        update_config(cx, move |c| {
                            if i < c.hooks.rules.len() {
                                c.hooks.rules.remove(i);
                            }
                        });
                    }))),
            );
            if picking == Some(i) {
                row = row.child(self.event_picker(i, event, p, cx));
            }
            list = list.child(row.child(command));
        }
        list = list.child(div().row().child(add_button("hook-add", t("Add a hook"), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.list_fields.clear();
            update_config(cx, |c| c.hooks.rules.push(HookRule { event: HookEvent::Locked, command: "notify-send Mandara Locked".into(), ..Default::default() }));
            // Open the new rule's event picker right away.
            let len = mandara_ui::state::config(cx).read(cx).get().hooks.rules.len();
            st(cx).picking_event = len.checked_sub(1);
        }))));

        // After app themes are written: one command per template.
        let mut templates = div().row().flex_col().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("After app themes are written")));
        for (key, command) in cfg.hooks.template_post.clone() {
            let k = key.clone();
            let field = self.list_field(format!("tpl-{key}"), &command, t("command, e.g. pkill -USR1 kitty"), Rc::new(move |c, v| {
                c.hooks.template_post.insert(k.clone(), v);
            }), cx);
            let k = key.clone();
            templates = templates.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().w(px(DIM_90_PX)).flex_none().child(badge(key.clone(), p.primary)))
                    .child(div().flex_1().min_w(px(0.0)).child(field))
                    .child(delete_button(SharedString::from(format!("tpl-{key}-del")), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.list_fields.clear();
                        let k = k.clone();
                        update_config(cx, move |c| {
                            c.hooks.template_post.remove(&k);
                        });
                    }))),
            );
        }
        let mut unused = div().row().flex_wrap().gap(px(SPACE_4_PX));
        for key in mandara_ui::powerctl::hooks::TEMPLATE_KEYS.iter().copied().filter(|k| !cfg.hooks.template_post.contains_key(*k)) {
            unused = unused.child(chip(SharedString::from(format!("tpl-add-{key}")), key, false, p).child(icon("plus", ICON_11_PX, p.primary)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.list_fields.clear();
                update_config(cx, move |c| {
                    c.hooks.template_post.insert(key.to_string(), String::new());
                });
            })));
        }
        templates = templates.child(unused);

        // Recent runs.
        let runs: Vec<Value> = history.as_ref().and_then(Value::as_array).map(|a| a.iter().take(RECENT_RUNS).cloned().collect()).unwrap_or_default();
        let recent = (!runs.is_empty()).then(|| {
            let mut col = div().row().flex_col().gap(px(SPACE_4_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Recent runs")));
            for r in runs {
                let ok = r.get("code").and_then(Value::as_i64) == Some(0);
                let timed_out = r.get("timed_out").and_then(Value::as_bool).unwrap_or(false);
                let (glyph, color, note) = if ok {
                    ("check", p.success_emerald, String::new())
                } else if timed_out {
                    ("timer", p.warning_amber, t("timed out").to_string())
                } else {
                    ("alert", p.urgent_coral, r.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| tf("exit {code}", &[("code", &r.get("code").cloned().unwrap_or(Value::Null))])))
                };
                col = col.child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_8_PX))
                        .text_size(px(TEXT_12_PX))
                        .child(icon(glyph, ICON_13_PX, color))
                        .child(div().w(px(DIM_150_PX)).flex_none().text_color(p.on_surface_variant.hsla()).child(r.get("event").and_then(Value::as_str).unwrap_or_default().to_string()))
                        .child(div().flex_1().min_w(px(0.0)).truncate().font_family("monospace").child(r.get("command").and_then(Value::as_str).unwrap_or_default().to_string()))
                        .child(div().flex_none().font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(format!("{} ms", r.get("duration_ms").and_then(Value::as_u64).unwrap_or(0))))
                        .when(!note.is_empty(), |el| el.child(div().flex_none().text_color(color.hsla()).child(note))),
                );
            }
            col
        });
        section(
            t("Your hooks"),
            t("Run a command when something happens. Event details arrive as environment variables (MANDARA_EVENT, MANDARA_WALLPAPER, MANDARA_BATTERY_PERCENT…), never inside the command line. Before-logout, restart, shutdown and sleep hooks finish before the action."),
            div().row().flex_col().gap(px(SPACE_16_PX)).child(list).child(templates).when_some(recent, |el, r| el.child(r)),
            p,
        )
    }

    fn event_picker(&mut self, i: usize, current: HookEvent, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().row().flex_col().gap(px(SPACE_8_PX)).p(px(SPACE_10_PX)).rounded(px(RADIUS_14_PX)).bg(p.surface_dim.hsla());
        for (group, events) in EVENT_GROUPS {
            let mut chips = div().row().flex_wrap().gap(px(SPACE_4_PX));
            for e in events.iter().copied() {
                chips = chips.child(chip(SharedString::from(format!("hook-{i}-pick-{}", e.id())), e.id(), e == current, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    st(cx).picking_event = None;
                    update_config(cx, move |c| {
                        if let Some(r) = c.hooks.rules.get_mut(i) {
                            r.event = e;
                        }
                    });
                })));
            }
            col = col.child(div().row().flex_col().gap(px(SPACE_4_PX)).child(div().text_size(px(TEXT_11_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(group)))).child(chips));
        }
        col.into_any_element()
    }
}

// ── Small building blocks ──────────────────────────────────────────────────

fn row_card(p: &MandaraPalette) -> gpui::Div {
    div().p(px(SPACE_10_PX)).clip_rounded().rounded(px(RADIUS_14_PX)).bg(p.surface_container_high.with_alpha(ALPHA_50).hsla()).row().flex_col().gap(px(SPACE_8_PX))
}

fn delete_button(id: SharedString, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(DIM_32_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
        .child(icon("trash", ICON_15_PX, p.on_surface_variant))
}

fn add_button(id: &'static str, label: &str, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(DIM_34_PX))
        .px(px(SPACE_14_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_6_PX))
        .cursor_pointer()
        .bg(p.primary.with_alpha(ALPHA_14).hsla())
        .text_color(p.primary.hsla())
        .font_weight(FontWeight::MEDIUM)
        .child(icon("plus", ICON_14_PX, p.primary))
        .child(label.to_string())
}

/// A titled group: title, then a card with a help line and the body.
fn section(title: &str, help: &str, body: gpui::Div, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .flex_col()
        .gap(px(SPACE_10_PX))
        .child(group_title(title.to_string(), p))
        .child(card(p).child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(help.to_string()))).child(body))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_label(0), "Never");
        assert_eq!(duration_label(45), "45 s");
        assert_eq!(duration_label(600), "10 min");
        assert_eq!(duration_label(90), "1 min 30 s");
        assert_eq!(duration_label(7200), "2 h");
        assert_eq!(clock_label(125), "2:05");
    }

    #[test]
    fn every_hook_event_is_pickable_once() {
        let mut seen: Vec<HookEvent> = EVENT_GROUPS.iter().flat_map(|(_, e)| e.iter().copied()).collect();
        let total = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), total, "an event is listed twice");
        assert_eq!(total, HookEvent::ALL.len(), "an event is missing from the picker");
    }

    #[test]
    fn timeout_chips_are_sorted_and_start_with_never() {
        assert_eq!(TIMEOUT_CHOICES_S[0], 0);
        assert!(TIMEOUT_CHOICES_S.windows(2).all(|w| w[0] < w[1]));
    }
}
