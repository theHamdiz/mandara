//! Presentation mode (built-in `presentation` plugin, opt-in): one switch
//! for screen sharing and talks. While on, notifications are held (do not
//! disturb), the screen stays awake (the keep-awake inhibitor) and the
//! desktop widgets are hidden; turning it off puts back exactly what was
//! there before (do not disturb you had on stays on).
//!
//! The bookkeeping (what was saved, what to restore, when screen sharing
//! decides) is [`mandara_services::presentation`]. While the mode is on its
//! session is also kept in `~/.local/state/mandara/presentation.json`, so a
//! shell that restarts mid-talk resumes it and still knows what to put
//! back. Nothing ticks while it is off; while it is on, the bar chip redraws
//! once a minute for its elapsed time.
//!
//! Settings (`[plugins.settings.presentation]`): `auto_screen_share` (turn
//! on by itself while the privacy monitor reports screen sharing, and off
//! again when sharing stops, if it turned itself on; default off).
//!
//! Commands: `presentation on`, `presentation off`, `presentation toggle`,
//! `presentation status`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyElement, App, AppContext as _, Entity, FontWeight, Global, InteractiveElement as _, IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_config::ShellConfig;
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::NotificationCommand;
use mandara_services::presentation::{self as pm, Plan, Session, Switches};
use mandara_theme::MandaraPalette;
use serde_json::json;

use super::panel_fx::{ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, SECTION_H_PX, glass_card, glow, header, partner, readable, redraw_in, section, switch};
use super::{fn_view, follow};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};
use crate::state::{config, update_config};

pub const ID: &str = "presentation";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 340.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
/// The big toggle card.
const HERO_H: f32 = 92.0;
const HERO_PAD_PX: f32 = 16.0;
const HERO_GLYPH_PX: f32 = 46.0;
const HERO_ICON_PX: f32 = 22.0;
const HERO_FILL: (f32, f32) = (0.24, 0.10);
const HERO_GLOW: (f32, f32) = (0.35, 18.0);
const HERO_TITLE_PX: f32 = 16.0;
/// Glyph disc alpha while on / off.
const HERO_GLYPH_ALPHA: (f32, f32) = (0.22, 0.12);
/// "What it changes" rows.
const ROW_H: f32 = 40.0;
const ROW_GAP: f32 = 4.0;
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_ICON_PX: f32 = 15.0;
const STATE_DOT_PX: f32 = 7.0;
const STATE_DOT_OFF_ALPHA: f32 = 0.14;
const ROW_INNER_GAP_PX: f32 = 10.0;
const LIST_PAD_PX: f32 = 6.0;
/// Bar chip: height, padding, dot and its glow, the fill.
const BAR_H_PX: f32 = 20.0;
const BAR_PAD_X_PX: f32 = 8.0;
const BAR_GAP_PX: f32 = 5.0;
const BAR_DOT_PX: f32 = 6.0;
const BAR_DOT_GLOW: (f32, f32) = (0.6, 6.0);
const BAR_FILL_ALPHA: f32 = 0.16;
/// Rows in "what it changes".
const ROWS: usize = 4;

const MS_PER_S: i64 = 1000;
const SECS_PER_MIN: i64 = 60;
const MINUTE_MS: i64 = 60 * MS_PER_S;

// ── Model ────────────────────────────────────────────────────────────────

pub struct Presentation {
    pub session: Option<Session>,
    /// Turned off by hand while the screen is shared (stays off until the
    /// share ends).
    declined: bool,
}

struct GlobalPresentation(Entity<Presentation>);
impl Global for GlobalPresentation {}

pub fn model(cx: &mut App) -> Entity<Presentation> {
    if let Some(g) = cx.try_global::<GlobalPresentation>() {
        return g.0.clone();
    }
    let entity = cx.new(|_| Presentation { session: None, declined: false });
    cx.set_global(GlobalPresentation(entity.clone()));
    entity
}

/// Presentation mode is on.
pub fn active(cx: &App) -> bool {
    cx.try_global::<GlobalPresentation>().is_some_and(|g| g.0.read(cx).session.is_some())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn enabled(cfg: &ShellConfig, cx: &App) -> bool {
    cx.try_global::<PluginRegistry>().is_some_and(|r| r.is_enabled(ID, &cfg.plugins))
}

fn auto_setting(cfg: &ShellConfig) -> bool {
    cfg.plugins.setting(ID, "auto_screen_share", false)
}

/// Only enablement and automatic sharing affect the config observer's decision.
fn watch_key(cfg: &ShellConfig, registry: Option<&PluginRegistry>) -> (bool, bool) {
    (registry.is_some_and(|registry| registry.is_enabled(ID, &cfg.plugins)), auto_setting(cfg))
}

/// Screen sharing should be watched for presentation mode (the privacy
/// monitor runs for it even when the privacy plugin is off).
pub fn watches_screen_share(cfg: &ShellConfig, cx: &App) -> bool {
    enabled(cfg, cx) && auto_setting(cfg)
}

/// An app shares the screen (the privacy monitor's view).
fn sharing(cx: &App) -> bool {
    !crate::alerts::privacy_state(cx).screen_share.is_empty()
}

/// The switches as they are now.
fn current(cx: &App) -> Switches {
    Switches {
        dnd: crate::hub::hub(cx).read(cx).notifications.dnd,
        keep_awake: super::quick_settings::keep_awake_active(cx),
        widgets_hidden: config(cx).read(cx).get().desktop.widgets.hidden,
    }
}

fn apply(plan: Plan, cx: &mut App) {
    if let Some(on) = plan.dnd {
        super::notifications::send(cx, NotificationCommand::SetDnd(on));
    }
    if let Some(on) = plan.keep_awake
        && let Err(e) = super::quick_settings::set_keep_awake(on, cx)
    {
        tracing::warn!("presentation: keep awake: {e}");
    }
    if let Some(hidden) = plan.widgets_hidden {
        update_config(cx, |c| c.desktop.widgets.hidden = hidden);
    }
}

/// Writes the session (or its absence) off the UI thread.
fn persist(session: Option<Session>, cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = pm::store(&pm::state_path(), session.as_ref()) {
                tracing::warn!("presentation: saving the session: {e}");
            }
        })
        .detach();
}

/// Turns the mode on or off (`auto`: screen sharing did it).
pub fn set(on: bool, auto: bool, cx: &mut App) {
    let m = model(cx);
    let before = m.read(cx).session;
    set_inner(on, auto, cx);
    let after = m.read(cx).session;
    if after != before {
        persist(after, cx);
    }
}

fn set_inner(on: bool, auto: bool, cx: &mut App) {
    let m = model(cx);
    let session = m.read(cx).session;
    match (on, session) {
        (true, None) => {
            let (session, plan) = pm::turn_on(current(cx), now_ms(), auto);
            apply(plan, cx);
            m.update(cx, |p, cx| {
                p.session = Some(session);
                cx.notify();
            });
        }
        (false, Some(session)) => {
            let plan = pm::turn_off(&session, current(cx));
            apply(plan, cx);
            // Turned off by hand mid-share: screen sharing doesn't turn it
            // straight back on.
            let declined = !auto && sharing(cx);
            m.update(cx, |p, cx| {
                p.session = None;
                p.declined = declined;
                cx.notify();
            });
        }
        // Switched by hand while on by itself: it is yours now.
        (true, Some(s)) if s.auto && !auto => m.update(cx, |p, cx| {
            if let Some(s) = p.session.as_mut() {
                s.auto = false;
            }
            cx.notify();
        }),
        _ => {}
    }
}

fn toggle(cx: &mut App) {
    let on = active(cx);
    set(!on, false, cx);
}

/// Follows screen sharing when `auto_screen_share` is on, and turns the
/// mode off if the plugin is switched off while it is on.
fn sync(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let session = model(cx).read(cx).session;
    if session.is_some() && !enabled(&cfg, cx) {
        set(false, false, cx);
        return;
    }
    let sharing = sharing(cx);
    let m = model(cx);
    let declined = m.read(cx).declined;
    if declined && !sharing {
        m.update(cx, |p, _| p.declined = false);
    }
    match pm::follow_sharing(watches_screen_share(&cfg, cx), sharing, session.as_ref(), declined && sharing) {
        Some(true) => set(true, true, cx),
        Some(false) => set(false, true, cx),
        None => {}
    }
}

/// Watches screen sharing and the plugin switch (call after the alerts).
pub fn init(cx: &mut App) {
    let m = model(cx);
    // A previous run was presenting when it stopped: carry on, and keep
    // what to restore.
    let cfg = config(cx).read(cx).snapshot();
    if let Some(session) = pm::load(&pm::state_path()) {
        if enabled(&cfg, cx) {
            apply(pm::resume(current(cx)), cx);
            m.update(cx, |p, cx| {
                p.session = Some(session);
                cx.notify();
            });
        } else {
            // Switched off since: just put things back.
            apply(pm::turn_off(&session, current(cx)), cx);
            persist(None, cx);
        }
    }
    if let Some(privacy) = crate::alerts::privacy_entity(cx) {
        cx.observe(&privacy, |_, cx| sync(cx)).detach();
    }
    let mut last = watch_key(&config(cx).read(cx).snapshot(), cx.try_global::<PluginRegistry>());
    cx.observe(&config(cx), move |c, cx| {
        let key = watch_key(c.read(cx).get(), cx.try_global::<PluginRegistry>());
        if key != last {
            last = key;
            sync(cx);
        }
    })
    .detach();
}

fn status_json(cx: &App) -> serde_json::Value {
    let session = cx.try_global::<GlobalPresentation>().and_then(|g| g.0.read(cx).session);
    json!({
        "on": session.is_some(),
        "since_ms": session.map(|s| s.since_ms),
        "elapsed_s": session.map(|s| (now_ms() - s.since_ms).max(0) / MS_PER_S),
        "auto": session.is_some_and(|s| s.auto),
        "restores": session.map(|s| s.saved),
        "now": current(cx),
        "auto_screen_share": auto_setting(&config(cx).read(cx).snapshot()),
    })
}

// ── Look ─────────────────────────────────────────────────────────────────

/// `0:05` / `1:12` (hours:minutes on).
fn elapsed_label(ms: i64) -> String {
    let m = ms.max(0) / MINUTE_MS;
    format!("{}:{:02}", m / SECS_PER_MIN, m % SECS_PER_MIN)
}

/// Delay until the elapsed label next changes.
fn until_next_minute(since_ms: i64, now: i64) -> Duration {
    let into = (now - since_ms).max(0) % MINUTE_MS;
    Duration::from_millis(u64::try_from(MINUTE_MS - into).unwrap_or(0)) + EXPIRY_MARGIN
}

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(session) = model(cx).read(cx).session else { return div().into_any_element() };
    let now = now_ms();
    redraw_in(until_next_minute(session.since_ms, now), window, cx);
    let coral = readable(palette.urgent_coral, palette);
    div()
        .h(px(BAR_H_PX))
        .px(px(BAR_PAD_X_PX))
        .rounded_full()
        .bg(coral.with_alpha(BAR_FILL_ALPHA).hsla())
        .flex()
        .items_center()
        .gap(px(BAR_GAP_PX))
        .child(div().size(px(BAR_DOT_PX)).rounded_full().bg(coral.hsla()).shadow(vec![glow(coral.with_alpha(BAR_DOT_GLOW.0), BAR_DOT_GLOW.1, 0.0, 0.0)]))
        .child(div().font_features(tabular_nums()).font_weight(FontWeight::SEMIBOLD).text_color(coral.hsla()).child(elapsed_label(now - session.since_ms)))
        .into_any_element()
}

/// One line of "what it changes": its icon, what it is and whether it is on.
fn change_row(glyph: &'static str, label: &'static str, state: SharedString, on: bool, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(if on { palette.urgent_coral } else { palette.on_surface_variant }, palette);
    div()
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .child(icon(glyph, ROW_ICON_PX, tint))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(label))
        .child(div().size(px(STATE_DOT_PX)).rounded_full().bg(if on { tint.hsla() } else { palette.on_surface.with_alpha(STATE_DOT_OFF_ALPHA).hsla() }))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(tint.hsla()).child(state))
        .into_any_element()
}

fn panel_height() -> f32 {
    PAD * 2.0 + HEADER_H + GAP + HERO_H + GAP + SECTION_H_PX + tokens::XS + ROWS as f32 * ROW_H + (ROWS as f32 - 1.0) * ROW_GAP + LIST_PAD_PX * 2.0
}

fn panel_view(p: &PanelCx, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let session = model(cx).read(cx).session;
    let now = now_ms();
    if let Some(s) = session
        && p.progress() > 0.0
    {
        redraw_in(until_next_minute(s.since_ms, now), window, cx);
    }
    let sw = current(cx);
    let auto = auto_setting(&config(cx).read(cx).snapshot());
    let on = session.is_some();
    let coral = readable(palette.urgent_coral, palette);
    let accent = if on { coral } else { readable(palette.primary, palette) };
    let subtitle = match session {
        Some(s) if s.auto => tf("On for {time} · started by screen sharing", &[("time", &super::panel_fx::duration_short((now - s.since_ms) / MS_PER_S))]),
        Some(s) => tf("On for {time}", &[("time", &super::panel_fx::duration_short((now - s.since_ms) / MS_PER_S))]),
        None => t("Off").to_string(),
    };
    let head = header("monitor", accent, t("Presentation mode"), subtitle, palette, None);
    let hero = glass_card(palette)
        .id("presentation-toggle")
        .h(px(HERO_H))
        .p(px(HERO_PAD_PX))
        .flex()
        .items_center()
        .gap(px(tokens::MD))
        .cursor_pointer()
        .when(on, |el| el.bg(oklab_gradient(ANGLE_DIAGONAL_DEG, coral.with_alpha(HERO_FILL.0), partner(coral).with_alpha(HERO_FILL.1))))
        .child(
            div()
                .size(px(HERO_GLYPH_PX))
                .flex_none()
                .rounded_full()
                .bg(accent.with_alpha(if on { HERO_GLYPH_ALPHA.0 } else { HERO_GLYPH_ALPHA.1 }).hsla())
                .when(on, |el| el.shadow(vec![glow(coral.with_alpha(HERO_GLOW.0), HERO_GLOW.1, 0.0, 0.0)]))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("monitor", HERO_ICON_PX, accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(HERO_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).child(if on { t("You're presenting") } else { t("Ready to present") }))
                .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(if on { t("Click to end and put everything back") } else { t("Quiet, awake and tidy in one click") })),
        )
        .child(switch(on, palette))
        .on_click(|_, _, cx| toggle(cx));
    let state = |b: bool| -> SharedString { if b { t("On").into() } else { t("Off").into() } };
    let rows = [
        change_row("bell", t("Do not disturb"), state(sw.dnd), sw.dnd, palette),
        change_row("coffee", t("Keep the screen awake"), state(sw.keep_awake), sw.keep_awake, palette),
        change_row("eye-off", t("Hide desktop widgets"), if sw.widgets_hidden { t("Hidden").into() } else { t("Shown").into() }, sw.widgets_hidden, palette),
        change_row("monitor", t("Start when sharing the screen"), if auto { t("Auto").into() } else { t("Off").into() }, auto, palette),
    ];
    let list = glass_card(palette).p(px(LIST_PAD_PX)).flex().flex_col().gap(px(ROW_GAP)).children(rows);
    div()
        .size_full()
        .p(px(PAD))
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(reveal(p, 0, head))
        .child(reveal(p, 1, hero))
        .child(reveal(p, 2, div().flex().flex_col().gap(px(tokens::XS)).child(section(t("WHAT IT CHANGES"), palette, None)).child(list)))
        .into_any_element()
}

// ── Registration ─────────────────────────────────────────────────────────

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Presentation mode"), tk("One switch for talks and screen sharing: quiet, awake and tidy"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model(cx);
        let bar = fn_view(None, bar_view, window, cx);
        follow(&bar, &m, cx);
        let p = panel_cx.clone();
        let panel = fn_view(None, move |palette, window, cx| panel_view(&p, palette, window, cx), window, cx);
        follow(&panel, &m, cx);
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W), px(panel_height())))) }),
            sidebar: None,
            // The chip shows only while presenting: a click ends it.
            on_click: Some(Rc::new(|_, cx| {
                if active(cx) {
                    set(false, false, cx);
                } else {
                    crate::state::shell(cx).update(cx, |s, cx| s.toggle_panel(ID, cx));
                }
            })),
            on_secondary_click: Some(Rc::new(|_, cx| crate::state::shell(cx).update(cx, |s, cx| s.toggle_panel(ID, cx)))),
        }
    });
    r.command(
        ID,
        CommandSpec::new("presentation on", tk("Turn presentation mode on: do not disturb, keep awake, hide desktop widgets"), |_, cx| {
            set(true, false, cx);
            Ok(status_json(cx))
        })
        .example("presentation on"),
    );
    r.command(
        ID,
        CommandSpec::new("presentation off", tk("Turn presentation mode off and put everything back as it was"), |_, cx| {
            set(false, false, cx);
            Ok(status_json(cx))
        })
        .example("presentation off"),
    );
    r.command(
        ID,
        CommandSpec::new("presentation toggle", tk("Turn presentation mode on or off"), |_, cx| {
            toggle(cx);
            Ok(status_json(cx))
        })
        .example("presentation toggle"),
    );
    r.command(
        ID,
        CommandSpec::new("presentation status", tk("Whether presentation mode is on, for how long, and what it will restore (JSON)"), |_, cx| Ok(status_json(cx))).example("presentation status"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unrelated settings do not refollow sharing; enablement and auto edits do.
    #[test]
    fn config_watch_ignores_settings_outside_its_presentation_flags() {
        let mut registry = PluginRegistry::default();
        register(&mut registry);
        let mut config = ShellConfig::default();
        let initial = watch_key(&config, Some(&registry));
        assert_eq!(initial, (false, false));
        config.plugins.settings.entry("feeds".into()).or_default().insert("subscriptions".into(), toml::Value::String("https://example.org/feed".into()));
        assert_eq!(watch_key(&config, Some(&registry)), initial);
        config.plugins.enabled.push(ID.into());
        assert_eq!(watch_key(&config, Some(&registry)), (true, false));
        config.plugins.settings.entry(ID.into()).or_default().insert("auto_screen_share".into(), toml::Value::Boolean(true));
        assert_eq!(watch_key(&config, Some(&registry)), (true, true));
        config.plugins.disabled.push(ID.into());
        assert_eq!(watch_key(&config, Some(&registry)), (false, true));
        assert_eq!(watch_key(&config, None), (false, true));
    }

    #[test]
    fn elapsed_reads_hours_and_minutes() {
        assert_eq!(elapsed_label(0), "0:00");
        assert_eq!(elapsed_label(5 * MINUTE_MS + 59 * MS_PER_S), "0:05");
        assert_eq!(elapsed_label(72 * MINUTE_MS), "1:12");
        let d = until_next_minute(0, 90 * MS_PER_S);
        assert_eq!(d, Duration::from_secs(30) + EXPIRY_MARGIN);
    }
}
