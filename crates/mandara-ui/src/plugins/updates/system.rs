//! The Updates panel's system update: the running job's card (phases,
//! progress bar, the latest log lines, cancel at safe points, the summary
//! with "restart required", and the terminal for an AUR failure that needs
//! a person) and, on NixOS, the generations view (update and
//! rebuild modes, roll back, switch to a generation, compare two, clean up).
//!
//! The job itself is [`mandara_services::sysupdate::job`]: one owned worker
//! thread. It is held here, not by a panel, so it keeps running and
//! reporting when the panel closes and reopens; the panel only reads its
//! state. Redraws are coalesced to [`job::REDRAW_EVERY`].

use std::cell::RefCell;
use std::sync::Arc;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf};
use mandara_services::sysupdate::job::{self, Finished, Job, JobOutcome, JobState, Request};
use mandara_services::sysupdate::nixos::{self, Cleanup, Diff, Generation, HomeGeneration, NixAction, RebuildMode, Setup};
use mandara_services::sysupdate::{self, FailureKind, Phase, System};
use mandara_theme::{Color, MandaraPalette};

use super::super::panel_fx::{ANGLE_RIGHT_DEG, PILL_H_PX, SECTION_H_PX, SMALL_ICON_PX, TINT_FILL, TINT_HOVER, alpha, glass_card, meter, partner, pill_button, readable, section, wash, WASH_SOFT};
use super::{Shared, alive, notify};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::text;

// ── Geometry (px) ────────────────────────────────────────────────────────

const CARD_PAD_PX: f32 = 12.0;
const CARD_GAP_PX: f32 = 8.0;
const TITLE_ROW_H: f32 = 32.0;
const BAR_H: f32 = 8.0;
const META_ROW_H: f32 = 16.0;
const BUTTON_ROW_H: f32 = PILL_H_PX;
const ICON_DISC_PX: f32 = 32.0;
const ICON_GLYPH_PX: f32 = 15.0;
const LOG_LINE_H: f32 = 15.0;
const LOG_PAD_PX: f32 = 8.0;
const LOG_TEXT_PX: f32 = 10.5;
/// Log lines shown when the log is open (the whole log is in the file).
const LOG_VISIBLE: usize = 12;
const LOG_H: f32 = LOG_VISIBLE as f32 * LOG_LINE_H + LOG_PAD_PX * 2.0;
const BANNER_H: f32 = 40.0;
/// A failure's advice: two lines of body text.
const ADVICE_LINES: usize = 2;
const ADVICE_H: f32 = 40.0;
const BANNER_PAD_X_PX: f32 = 10.0;
const INPUT_ROW_H: f32 = 20.0;
/// Flake inputs listed in the summary at most.
const INPUTS_SHOWN: usize = 4;
const INLINE_GAP_PX: f32 = 6.0;
const ROW_RADIUS_PX: f32 = 10.0;
/// NixOS view.
const SETUP_H: f32 = 58.0;
const MODES_H: f32 = 32.0;
const MODE_CHIP_PAD_X_PX: f32 = 10.0;
const MODE_HINT_H: f32 = 16.0;
const GEN_ROW_H: f32 = 40.0;
const GEN_ROW_GAP: f32 = 2.0;
const GEN_ACTIONS_H: f32 = 34.0;
/// Generation rows visible before the list scrolls.
const GEN_VISIBLE: f32 = 5.5;
const GEN_VISIBLE_COMPACT: f32 = 3.5;
const GEN_BADGE_PX: f32 = 30.0;
const CONFIRM_H: f32 = 48.0;
const DIFF_ROW_H: f32 = 20.0;
/// Package changes listed in a comparison at most.
const DIFF_SHOWN: usize = 8;
const CLEAN_H: f32 = 32.0;
const STEPPER_PX: f32 = 24.0;
const HOME_ROW_H: f32 = 24.0;
/// Home-manager generations listed at most.
const HOME_SHOWN: usize = 3;
/// Generations whose configuration revision is looked up.
const REVISIONS_LOOKED_UP: usize = 12;
/// Short revisions and hashes show this many characters.
const SHORT_REV: usize = 7;
const PILL_GAP_PX: f32 = 6.0;
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_TINT: (f32, f32) = (0.16, 0.11);
const SELECTED_ROW: (f32, f32) = (0.08, 0.06);
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const BUTTON_GLOW_ALPHA: f32 = 0.3;
const BUTTON_GLOW_BLUR_PX: f32 = 8.0;
const DISABLED_OPACITY: f32 = 0.45;

thread_local! {
    /// The running or last finished job (survives panels and chips).
    static ACTIVE: RefCell<Option<Job>> = const { RefCell::new(None) };
}

/// The current job, if any.
pub(super) fn active() -> Option<Job> {
    ACTIVE.with(|a| a.borrow().clone())
}

/// Whether a job is running now.
pub(super) fn running() -> bool {
    active().is_some_and(|j| !j.finished())
}

/// Starts `request` unless a job is running.
pub(super) fn start(request: Request, cx: &mut App) -> Result<(), String> {
    if running() {
        return Err(t("An update is already running").to_string());
    }
    let (tx, rx) = async_channel::bounded::<()>(1);
    let ping: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = tx.try_send(());
    });
    let job = job::start(request, sysupdate::launcher(), ping);
    ACTIVE.with(|a| *a.borrow_mut() = Some(job.clone()));
    if let Some(m) = alive() {
        let mut mb = m.borrow_mut();
        mb.log_open = false;
        mb.log_cache = (u64::MAX, Vec::new());
    }
    cx.spawn(async move |cx| {
        // Pings coalesce in the one-slot channel; redraws are paced.
        while rx.recv().await.is_ok() {
            cx.update(|cx| {
                if let Some(m) = alive() {
                    notify(&m, cx);
                }
            });
            cx.background_executor().timer(job::REDRAW_EVERY).await;
        }
        // The worker ended (its ping went with it).
        cx.update(|cx| finished(&job, cx));
    })
    .detach();
    if let Some(m) = alive() {
        notify(&m, cx);
    }
    Ok(())
}

/// After a job: refresh the package lists (or generations), and tell the
/// user if no panel shows the summary.
fn finished(job: &Job, cx: &mut App) {
    let Some(f) = job.with(|s| s.finished.clone()) else { return };
    let nix = job.with(|s| matches!(s.request, Request::Nix { .. }));
    if let Some(m) = alive() {
        {
            let mut mb = m.borrow_mut();
            if nix {
                mb.nix.loaded = false;
            }
        }
        if matches!(f.outcome, JobOutcome::Done { .. }) && !nix {
            super::start_check(&m, None, cx);
        }
        let open = m.borrow().open_panels > 0;
        notify(&m, cx);
        if open {
            return;
        }
    }
    let request = job.with(|s| s.request.clone());
    let (summary, body) = summary_text(&request, &f);
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Updates").into(), icon: "package".into(), summary, body });
}

/// Dismisses a finished job's summary (frees its log).
pub(super) fn dismiss() {
    ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        if a.as_ref().is_some_and(Job::finished) {
            *a = None;
        }
    });
}

/// A finished job's headline and detail.
fn summary_text(request: &Request, f: &Finished) -> (String, String) {
    let freed = || f.freed.as_ref().map(|s| tf("Freed {size}", &[("size", s)])).unwrap_or_default();
    if let (Request::Nix { action }, JobOutcome::Done { .. }) = (request, &f.outcome) {
        let headline = match *action {
            NixAction::Update(mode) | NixAction::Rebuild(mode) => match mode {
                RebuildMode::Switch => t("Switched to the new generation").to_string(),
                RebuildMode::Boot => t("The new generation starts at the next boot").to_string(),
                RebuildMode::Test => t("Testing the new generation until you restart").to_string(),
                RebuildMode::Build => t("Built the new generation").to_string(),
            },
            NixAction::Rollback => t("Rolled back to the previous generation").to_string(),
            NixAction::SwitchTo(n) => tf("Switched to generation {n}", &[("n", &n)]),
            NixAction::Clean(_) => t("Old generations cleaned up").to_string(),
        };
        return (headline, freed());
    }
    let updated = || if f.changed == 1 { t("Updated 1 package").to_string() } else { tf("Updated {count} packages", &[("count", &f.changed)]) };
    match &f.outcome {
        JobOutcome::Done { nothing_to_do: true } => (t("Everything was already up to date").to_string(), String::new()),
        JobOutcome::Done { .. } if f.changed > 0 => (updated(), f.restart.as_ref().map(|r| tf("Restart to finish: {reason}", &[("reason", r)])).unwrap_or_default()),
        JobOutcome::Done { .. } => (t("Done").to_string(), freed()),
        // Stopped later (during Flatpak, after the system's packages).
        JobOutcome::Cancelled if f.changed > 0 => (t("Update cancelled").to_string(), updated()),
        JobOutcome::Cancelled => (t("Update cancelled").to_string(), t("Nothing was installed").to_string()),
        JobOutcome::Failed(failure) => (failure_title(failure.kind).to_string(), failure.kind.advice().to_string()),
    }
}

fn failure_title(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Locked => t("The package manager is busy"),
        FailureKind::Network => t("Couldn't reach the package servers"),
        FailureKind::Conflict => t("The update needs your decision"),
        FailureKind::DiskFull => t("Not enough disk space"),
        FailureKind::Keyring => t("A signature couldn't be verified"),
        FailureKind::NotAuthorized => t("Not authorized"),
        FailureKind::NoPrompt => t("No password prompt available"),
        FailureKind::Helper => t("Admin helper unavailable"),
        FailureKind::BuildFailed => t("An AUR package didn't build"),
        FailureKind::Other => t("The update stopped"),
    }
}

fn phase_icon(phase: Phase) -> &'static str {
    match phase {
        Phase::Authorizing => "key",
        Phase::Refreshing => "refresh",
        Phase::Downloading => "download",
        Phase::Verifying => "shield",
        Phase::Installing | Phase::Preparing => "package",
        Phase::Hooks => "workflow",
        Phase::Building => "box",
        Phase::Activating => "zap",
        Phase::Cleaning => "trash",
        Phase::Flatpak => "box",
        Phase::AurBuild => "box",
        Phase::Finished => "check",
    }
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn disc(icon_name: &'static str, tint: Color) -> AnyElement {
    div()
        .size(px(ICON_DISC_PX))
        .flex_none()
        .rounded_full()
        .bg(tint.with_alpha(TINT_FILL.0).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(icon(icon_name, ICON_GLYPH_PX, tint))
        .into_any_element()
}

fn badge(label: impl Into<SharedString>, tint: Color, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .bg(tint.with_alpha(alpha(BADGE_TINT, palette)).hsla())
        .font_features(tabular_nums())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label.into())
        .into_any_element()
}

/// The accent gradient button (the main action of a row).
fn primary_button(id: &'static str, icon_name: &'static str, label: impl Into<SharedString>, enabled: bool, palette: &MandaraPalette, on_click: impl Fn(&mut gpui::Window, &mut App) + 'static) -> AnyElement {
    let from = palette.primary;
    let to = partner(palette.primary).with_lightness(palette.primary.lightness());
    let ink = palette.on_primary;
    div()
        .id(id)
        .h(px(PILL_H_PX))
        .px(px(MODE_CHIP_PAD_X_PX + 2.0))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(INLINE_GAP_PX))
        .bg(oklab_gradient(ANGLE_RIGHT_DEG, from, to))
        .shadow(vec![super::super::panel_fx::glow(readable(from, palette).with_alpha(BUTTON_GLOW_ALPHA), BUTTON_GLOW_BLUR_PX, 0.0, 1.0)])
        .text_size(px(text::LABEL + 0.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink.hsla())
        .child(icon(icon_name, SMALL_ICON_PX, ink))
        .child(label.into())
        .when(!enabled, |el| el.opacity(DISABLED_OPACITY))
        .when(enabled, |el| {
            el.cursor_pointer().on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_click(window, cx);
            })
        })
        .into_any_element()
}

fn ago(ts: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d %b %Y").to_string()).unwrap_or_default()
}

fn short(rev: &str) -> &str {
    &rev[..rev.floor_char_boundary(SHORT_REV)]
}

/// `+12.3 MiB` / `-300 KiB`.
fn signed_size(bytes: i64) -> String {
    let sign = if bytes < 0 { "-" } else { "+" };
    format!("{sign}{}", mandara_services::app_install::human_bytes(bytes.unsigned_abs()))
}

// ── The job card ─────────────────────────────────────────────────────────

/// The card of the current job and its height; `None` without a job.
pub(super) fn job_card(m: &Shared, card_r: f32, palette: &MandaraPalette, cx: &App) -> Option<(AnyElement, f32)> {
    let job = active()?;
    let log_open = m.borrow().log_open;
    // The visible log lines, re-copied only when the job changed.
    let revision = job.with(|s| s.revision);
    if log_open && m.borrow().log_cache.0 != revision {
        let lines = job.with(|s| s.log.iter().skip(s.log.len().saturating_sub(LOG_VISIBLE)).map(|l| SharedString::from(l.clone())).collect());
        m.borrow_mut().log_cache = (revision, lines);
    }
    let el = job.with(|s| match &s.finished {
        None => running_card(&job, s, m, log_open, card_r, palette, cx),
        Some(f) => finished_card(s, f, m, log_open, card_r, palette, cx),
    });
    Some(el)
}

fn log_box(m: &Shared, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let mono = crate::fonts::fonts(cx).mono.clone();
    let lines = m.borrow().log_cache.1.clone();
    div()
        .h(px(LOG_H))
        .w_full()
        .p(px(LOG_PAD_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(wash(WASH_SOFT, palette).hsla())
        .overflow_hidden()
        .flex()
        .flex_col()
        .justify_end()
        .font_family(mono)
        .text_size(px(LOG_TEXT_PX))
        .text_color(palette.on_surface_variant.hsla())
        .children(lines.into_iter().map(|l| div().h(px(LOG_LINE_H)).flex_none().whitespace_nowrap().truncate().child(l)))
        .into_any_element()
}

fn log_toggle(m: &Shared, open: bool, palette: &MandaraPalette) -> AnyElement {
    let m = m.clone();
    pill_button("upd-log", Some(if open { "chevron-up" } else { "chevron-down" }), if open { t("Hide log") } else { t("Show log") }, palette.on_surface_variant, palette, move |_, cx| {
        let open = {
            let mut mb = m.borrow_mut();
            mb.log_open = !mb.log_open;
            mb.log_cache.0 = u64::MAX;
            mb.log_open
        };
        let _ = open;
        notify(&m, cx);
    })
    .into_any_element()
}

fn full_log_button(palette: &MandaraPalette) -> AnyElement {
    pill_button("upd-full-log", Some("external"), t("Full log"), palette.on_surface_variant, palette, |_, _| {
        let uri = format!("file://{}", job::log_path().display());
        std::thread::spawn(move || {
            if let Err(e) = mandara_services::session::open_uri(&uri) {
                tracing::warn!("updates: {e}");
            }
        });
    })
    .into_any_element()
}

fn running_card(job: &Job, s: &JobState, m: &Shared, log_open: bool, card_r: f32, palette: &MandaraPalette, cx: &App) -> (AnyElement, f32) {
    let accent = readable(palette.primary, palette);
    let phase = s.phase();
    // The program at work: the package manager, or the AUR helper.
    let what = match s.manager() {
        Some(manager) if phase != Phase::Flatpak => tf("{phase} · {manager}", &[("phase", &phase.label()), ("manager", &manager)]),
        _ => phase.label().to_string(),
    };
    // The item being worked on; the title already names the phase.
    let current = s.tracker.current.clone();
    let step = if s.steps > 1 { tf("Step {n} of {total}", &[("n", &s.step.max(1)), ("total", &s.steps)]) } else { String::new() };
    let counts = s.tracker.counts().map(|(d, n)| format!("{d}/{n}"));
    let meta = [Some(step).filter(|x| !x.is_empty()), counts].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let elapsed = crate::kit::format_duration(s.started.elapsed().as_secs());
    let cancel: AnyElement = if s.cancellable() {
        let job = job.clone();
        pill_button("upd-cancel", Some("close"), t("Cancel"), readable(palette.urgent_coral, palette), palette, move |_, _| {
            job.cancel();
        })
        .into_any_element()
    } else if s.cancel_requested {
        div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Stopping…")).into_any_element()
    } else if s.cancel_refused || matches!(phase, Phase::Installing | Phase::Hooks | Phase::Activating | Phase::Verifying | Phase::Cleaning) {
        div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Can't stop now")).into_any_element()
    } else {
        div().into_any_element()
    };
    let mut h = CARD_PAD_PX * 2.0 + TITLE_ROW_H + CARD_GAP_PX + BAR_H + CARD_GAP_PX + META_ROW_H + CARD_GAP_PX + BUTTON_ROW_H;
    if log_open {
        h += CARD_GAP_PX + LOG_H;
    }
    let el = glass_card(palette)
        .rounded(px(card_r))
        .p(px(CARD_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(CARD_GAP_PX))
        .child(
            div()
                .h(px(TITLE_ROW_H))
                .flex()
                .items_center()
                .gap(px(CARD_GAP_PX))
                .child(disc(phase_icon(phase), accent))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(what))
                        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(current)),
                )
                .child(cancel),
        )
        .child(meter(s.overall(), BAR_H, palette.primary, partner(palette.primary), palette))
        .child(
            div()
                .h(px(META_ROW_H))
                .flex()
                .justify_between()
                .font_features(tabular_nums())
                .text_size(px(text::LABEL))
                .text_color(palette.on_surface_variant.hsla())
                .child(meta)
                .child(elapsed),
        )
        .child(div().h(px(BUTTON_ROW_H)).flex().items_center().gap(px(PILL_GAP_PX)).child(log_toggle(m, log_open, palette)).when(log_open, |el| el.child(full_log_button(palette))))
        .when(log_open, |el| el.child(log_box(m, palette, cx)))
        .into_any_element();
    (el, h)
}

fn finished_card(s: &JobState, f: &Finished, m: &Shared, log_open: bool, card_r: f32, palette: &MandaraPalette, cx: &App) -> (AnyElement, f32) {
    let (headline, detail) = summary_text(&s.request, f);
    let (glyph, tint) = match &f.outcome {
        JobOutcome::Done { .. } => ("check", readable(palette.success_emerald, palette)),
        JobOutcome::Cancelled => ("close", palette.on_surface_variant),
        JobOutcome::Failed(_) => ("alert", readable(palette.urgent_coral, palette)),
    };
    let failure_detail = match &f.outcome {
        JobOutcome::Failed(failure) if !failure.detail.is_empty() => Some(failure.detail.clone()),
        _ => None,
    };
    let failed = matches!(f.outcome, JobOutcome::Failed(_));
    // A failure's advice gets two lines of its own; other details fit the
    // subtitle.
    let advice = failed.then(|| detail.clone());
    let sub = [Some(crate::kit::format_duration(f.elapsed_secs)), Some(detail).filter(|d| !d.is_empty() && f.restart.is_none() && !failed)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let request = s.request.clone();
    let mut h = CARD_PAD_PX * 2.0 + TITLE_ROW_H + CARD_GAP_PX + BUTTON_ROW_H;
    let restart = f.restart.clone().filter(|_| !matches!(f.outcome, JobOutcome::Failed(_)));
    if restart.is_some() {
        h += CARD_GAP_PX + BANNER_H;
    }
    if failure_detail.is_some() {
        h += CARD_GAP_PX + META_ROW_H;
    }
    if advice.is_some() {
        h += CARD_GAP_PX + ADVICE_H;
    }
    let inputs: Vec<_> = f.inputs.iter().take(INPUTS_SHOWN).cloned().collect();
    if !inputs.is_empty() {
        h += CARD_GAP_PX + inputs.len() as f32 * INPUT_ROW_H;
    }
    if log_open {
        h += CARD_GAP_PX + LOG_H;
    }
    let warn = readable(palette.warning_amber, palette);
    let m2 = m.clone();
    let retry_m = m.clone();
    // An AUR failure that needs a person: the helper's own command in your
    // terminal, where it can ask.
    let terminal = match (&f.outcome, s.failed_step, s.helper) {
        (JobOutcome::Failed(failure), Some(mandara_services::sysupdate::plan::StepKind::Aur), Some(helper)) if sysupdate::aur::needs_terminal(failure.kind) => Some(helper.terminal_command()),
        _ => None,
    };
    let terminal_m = m.clone();
    let el = glass_card(palette)
        .rounded(px(card_r))
        .p(px(CARD_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(CARD_GAP_PX))
        .child(
            div()
                .h(px(TITLE_ROW_H))
                .flex()
                .items_center()
                .gap(px(CARD_GAP_PX))
                .child(disc(glyph, tint))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(headline))
                        .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(sub)),
                ),
        )
        .when_some(advice, |el, a| el.child(div().h(px(ADVICE_H)).line_clamp(ADVICE_LINES).text_size(px(text::BODY_S)).child(a)))
        .when_some(failure_detail, |el, d| {
            el.child(div().h(px(META_ROW_H)).truncate().font_family(crate::fonts::fonts(cx).mono.clone()).text_size(px(LOG_TEXT_PX)).text_color(palette.on_surface_variant.hsla()).child(d))
        })
        .when(!inputs.is_empty(), |el| {
            el.child(div().flex().flex_col().children(inputs.into_iter().map(|i| {
                let old = i.old.as_ref().map(|o| short(&o.rev).to_string()).unwrap_or_else(|| "∅".into());
                let new = i.new.as_ref().map(|n| short(&n.rev).to_string()).unwrap_or_else(|| "∅".into());
                let when = i.new.as_ref().map(|n| ago(n.last_modified)).unwrap_or_default();
                div()
                    .h(px(INPUT_ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(INLINE_GAP_PX))
                    .text_size(px(text::BODY_S))
                    .child(div().font_weight(FontWeight::MEDIUM).child(i.name))
                    .child(div().flex_1())
                    .child(div().font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(format!("{old} → {new}")))
                    .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(when))
            })))
        })
        .when_some(restart, |el, reason| {
            el.child(
                div()
                    .h(px(BANNER_H))
                    .px(px(BANNER_PAD_X_PX))
                    .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
                    .bg(warn.with_alpha(alpha(TINT_FILL, palette)).hsla())
                    .flex()
                    .items_center()
                    .gap(px(CARD_GAP_PX))
                    .child(icon("refresh", SMALL_ICON_PX, warn))
                    .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(tf("Restart to finish: {reason}", &[("reason", &reason)])))
                    .child(pill_button("upd-restart", Some("refresh"), t("Restart…"), warn, palette, |_, cx| {
                        let registry = cx.global::<mandara_plugins::PluginRegistry>().commands.clone();
                        let words: Vec<String> = ["session", "menu", "open"].map(String::from).to_vec();
                        if let Err(e) = registry.execute(&words, cx) {
                            tracing::warn!("updates: {e}");
                        }
                    })),
            )
        })
        .child(
            div()
                .h(px(BUTTON_ROW_H))
                .flex()
                .items_center()
                .gap(px(PILL_GAP_PX))
                .child(log_toggle(m, log_open, palette))
                .when(log_open, |el| el.child(full_log_button(palette)))
                .child(div().flex_1())
                .when_some(terminal, |el, command| {
                    el.child(pill_button("upd-aur-terminal", Some("terminal"), t("Open in terminal"), readable(palette.primary, palette), palette, move |_, cx| {
                        let _ = super::run_in_terminal(Some(&terminal_m), command, cx);
                        notify(&terminal_m, cx);
                    }))
                })
                .when(failed, |el| {
                    el.child(pill_button("upd-retry", Some("refresh"), t("Try again"), readable(palette.primary, palette), palette, move |_, cx| {
                        dismiss();
                        if let Err(e) = start(request.clone(), cx) {
                            tracing::warn!("updates: {e}");
                        }
                        notify(&retry_m, cx);
                    }))
                })
                .child(pill_button("upd-done", Some("check"), t("Done"), readable(palette.primary, palette), palette, move |_, cx| {
                    dismiss();
                    notify(&m2, cx);
                })),
        )
        .when(log_open, |el| el.child(log_box(m, palette, cx)))
        .into_any_element();
    (el, h)
}

// ── NixOS ────────────────────────────────────────────────────────────────

/// What the NixOS view shows (loaded off the UI thread while a panel is
/// open; freed when the last one closes, except the user's choices).
#[derive(Default)]
pub(super) struct NixState {
    pub loaded: bool,
    loading: bool,
    gens: Vec<Generation>,
    home: Vec<HomeGeneration>,
    setup: Option<Setup>,
    selected: Option<u32>,
    confirm: Option<NixAction>,
    mode: Option<RebuildMode>,
    pub keep: Option<u32>,
    diff: Option<(u32, u32, Result<Diff, String>)>,
    diff_loading: bool,
}

impl NixState {
    /// Frees the lists; keeps the chosen mode and count.
    pub fn free(&mut self) {
        let (mode, keep) = (self.mode, self.keep);
        *self = NixState { mode, keep, ..NixState::default() };
    }
}

/// Default number of generations a clean-up keeps.
pub(super) const DEFAULT_KEEP: u32 = 5;

/// Loads the generations for an open panel.
pub(super) fn load_nix(m: &Shared, cx: &mut App) {
    {
        let mut mb = m.borrow_mut();
        if mb.nix.loaded || mb.nix.loading {
            return;
        }
        mb.nix.loading = true;
    }
    let task = cx.background_executor().spawn(async {
        let paths = sysupdate::nix_paths();
        let mut gens = nixos::generations(&paths);
        for g in gens.iter_mut().take(REVISIONS_LOOKED_UP) {
            g.revision = nixos::revision(&g.store);
        }
        (gens, nixos::home_generations(&sysupdate::home_profile_dirs()), nixos::setup(&paths))
    });
    let weak = std::rc::Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let (gens, home, setup) = task.await;
        let Some(m) = weak.upgrade() else { return };
        {
            let mut mb = m.borrow_mut();
            mb.nix.loading = false;
            if mb.open_panels > 0 {
                mb.nix.loaded = true;
                mb.nix.gens = gens;
                mb.nix.home = home;
                mb.nix.setup = Some(setup);
            }
        }
        cx.update(|cx| notify(&m, cx));
    })
    .detach();
}

fn compare(m: &Shared, a: u32, b: u32, cx: &mut App) {
    let paths = sysupdate::nix_paths();
    {
        let mut mb = m.borrow_mut();
        if mb.nix.diff_loading {
            return;
        }
        mb.nix.diff_loading = true;
        mb.nix.diff = None;
    }
    let task = cx.background_executor().spawn(async move { nixos::closure_diff(&paths.generation_link(a), &paths.generation_link(b)) });
    let weak = std::rc::Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let diff = task.await;
        let Some(m) = weak.upgrade() else { return };
        {
            let mut mb = m.borrow_mut();
            mb.nix.diff_loading = false;
            if mb.open_panels > 0 {
                mb.nix.diff = Some((a, b, diff));
            }
        }
        cx.update(|cx| notify(&m, cx));
    })
    .detach();
}

fn mode_label(mode: RebuildMode) -> &'static str {
    match mode {
        RebuildMode::Switch => t("Switch"),
        RebuildMode::Boot => t("Next boot"),
        RebuildMode::Test => t("Test"),
        RebuildMode::Build => t("Build only"),
    }
}

fn mode_hint(mode: RebuildMode) -> &'static str {
    match mode {
        RebuildMode::Switch => t("Build, switch now and boot it next time"),
        RebuildMode::Boot => t("Build and boot it next time; the running system stays"),
        RebuildMode::Test => t("Build and switch now, without making it the boot default"),
        RebuildMode::Build => t("Only build it, to see that it works"),
    }
}

fn confirm_text(action: NixAction, deletes: usize) -> String {
    match action {
        NixAction::Rollback => t("Roll back to the previous generation now?").to_string(),
        NixAction::SwitchTo(n) => tf("Switch to generation {n} now?", &[("n", &n)]),
        NixAction::Clean(_) => tf("Delete {count} old generations and collect garbage?", &[("count", &deletes)]),
        NixAction::Update(_) | NixAction::Rebuild(_) => String::new(),
    }
}

/// How many generations the confirmed `action` deletes: what Delete will
/// run, not what the stepper (still live under the bar) shows now.
/// `deletes` is the stepper's count for `keep`, reused when they agree.
fn confirm_deletes(action: NixAction, keep: u32, deletes: usize, gens: &[Generation], now: i64) -> usize {
    match action {
        NixAction::Clean(cleanup) if cleanup != Cleanup::KeepLatest(keep) => nixos::to_delete(gens, cleanup, now).len(),
        _ => deletes,
    }
}

fn run_nix(m: &Shared, action: NixAction, cx: &mut App) {
    m.borrow_mut().nix.confirm = None;
    if let Err(e) = start(Request::Nix { action }, cx) {
        m.borrow_mut().status = Some((false, e, std::time::Instant::now()));
    }
    notify(m, cx);
}

/// The NixOS view and its height.
pub(super) fn nix_view(m: &Shared, card_r: f32, row_r: f32, palette: &MandaraPalette) -> (Vec<AnyElement>, f32) {
    let accent = readable(palette.primary, palette);
    let busy = running();
    let (gens, home, setup, selected, confirm, mode, keep, diff, diff_loading, loaded) = {
        let mb = m.borrow();
        let n = &mb.nix;
        (n.gens.clone(), n.home.clone(), n.setup.clone(), n.selected, n.confirm, n.mode.unwrap_or(RebuildMode::Switch), n.keep.unwrap_or(DEFAULT_KEEP), n.diff.clone(), n.diff_loading, n.loaded)
    };
    let gap = super::GAP;
    let mut out: Vec<AnyElement> = Vec::new();
    let mut h = 0.0;
    let push = |el: AnyElement, height: f32, out: &mut Vec<AnyElement>, h: &mut f32| {
        if !out.is_empty() {
            *h += gap;
        }
        *h += height;
        out.push(el);
    };

    // ── Setup ────────────────────────────────────────────────────────────
    let current = gens.iter().find(|g| g.current);
    let booted = gens.iter().find(|g| g.booted);
    let (setup_title, setup_icon) = match &setup {
        Some(Setup::Flake { dir, .. }) => (tf("Flake · {path}", &[("path", &dir.display())]), "git-branch"),
        Some(Setup::Channels { channel: Some(c) }) => (tf("Channels · {channel}", &[("channel", c)]), "rss"),
        Some(Setup::Channels { channel: None }) => (t("Channels").to_string(), "rss"),
        None => (t("NixOS").to_string(), "box"),
    };
    let setup_sub = match current {
        Some(g) => tf("Generation {n} · NixOS {version} · kernel {kernel}", &[("n", &g.number), ("version", &g.nixos_version), ("kernel", &g.kernel)]),
        None if loaded => t("No system generations found").to_string(),
        None => t("Reading generations…").to_string(),
    };
    let pending_boot = booted.zip(current).filter(|(b, c)| b.number != c.number).map(|(b, _)| b.number);
    let setup_card = glass_card(palette)
        .rounded(px(card_r))
        .h(px(SETUP_H))
        .px(px(CARD_PAD_PX))
        .flex()
        .items_center()
        .gap(px(CARD_GAP_PX))
        .child(disc(setup_icon, accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(setup_title))
                .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(setup_sub)),
        )
        .when_some(pending_boot, |el, b| el.child(badge(tf("Booted: {n}", &[("n", &b)]), readable(palette.tertiary, palette), palette)))
        .into_any_element();
    push(setup_card, SETUP_H, &mut out, &mut h);

    // ── Modes and update ─────────────────────────────────────────────────
    let modes = div().flex().items_center().gap(px(PILL_GAP_PX)).children(RebuildMode::ALL.into_iter().map(|md| {
        let on = md == mode;
        let m = m.clone();
        let tint = if on { accent } else { palette.on_surface_variant };
        let hover = tint.with_alpha(alpha(TINT_HOVER, palette));
        div()
            .id(gpui::ElementId::Name(SharedString::new_static(md.id())))
            .h(px(MODES_H - 4.0))
            .px(px(MODE_CHIP_PAD_X_PX))
            .rounded_full()
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(px(text::LABEL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tint.hsla())
            .when(on, |el| el.bg(tint.with_alpha(alpha(TINT_FILL, palette)).hsla()))
            .hover(move |s| s.bg(hover.hsla()))
            .child(mode_label(md))
            .on_click(move |_, _, cx| {
                m.borrow_mut().nix.mode = Some(md);
                notify(&m, cx);
            })
    }));
    let (mu, mr) = (m.clone(), m.clone());
    let actions = div()
        .h(px(MODES_H))
        .flex()
        .items_center()
        .gap(px(PILL_GAP_PX))
        .child(modes)
        .child(div().flex_1())
        .child(pill_button("nix-rebuild", Some("refresh"), t("Rebuild"), accent, palette, move |_, cx| {
            if !running() {
                run_nix(&mr, NixAction::Rebuild(mode), cx);
            }
        })
        .when(busy, |el| el.opacity(DISABLED_OPACITY)))
        .child(primary_button("nix-update", "download", t("Update"), !busy, palette, move |_, cx| run_nix(&mu, NixAction::Update(mode), cx)))
        .into_any_element();
    push(actions, MODES_H, &mut out, &mut h);
    let hint = div().h(px(MODE_HINT_H)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(mode_hint(mode)).into_any_element();
    out.push(hint);
    h += MODE_HINT_H;

    // ── Confirmation ─────────────────────────────────────────────────────
    let now = chrono::Utc::now().timestamp();
    let deletes = nixos::to_delete(&gens, Cleanup::KeepLatest(keep), now).len();
    if let Some(action) = confirm {
        let confirm_deletes = confirm_deletes(action, keep, deletes, &gens, now);
        let (mc, mk) = (m.clone(), m.clone());
        let danger = readable(palette.urgent_coral, palette);
        let label = match action {
            NixAction::Clean(_) => t("Delete"),
            NixAction::Rollback => t("Roll back"),
            _ => t("Switch"),
        };
        let bar = div()
            .h(px(CONFIRM_H))
            .px(px(CARD_PAD_PX))
            .rounded(px(row_r))
            .bg(danger.with_alpha(alpha(TINT_FILL, palette)).hsla())
            .flex()
            .items_center()
            .gap(px(CARD_GAP_PX))
            .child(icon("alert", SMALL_ICON_PX, danger))
            .child(div().flex_1().min_w(px(0.0)).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).truncate().child(confirm_text(action, confirm_deletes)))
            .child(pill_button("nix-confirm-no", None, t("Cancel"), palette.on_surface_variant, palette, move |_, cx| {
                mc.borrow_mut().nix.confirm = None;
                notify(&mc, cx);
            }))
            .child(pill_button("nix-confirm-yes", Some("check"), label, danger, palette, move |_, cx| run_nix(&mk, action, cx)))
            .into_any_element();
        push(bar, CONFIRM_H, &mut out, &mut h);
    }

    // ── Generations ──────────────────────────────────────────────────────
    let mrb = m.clone();
    let rollback = pill_button("nix-rollback", Some("undo"), t("Roll back"), accent, palette, move |_, cx| {
        if !running() {
            mrb.borrow_mut().nix.confirm = Some(NixAction::Rollback);
            notify(&mrb, cx);
        }
    })
    .when(busy, |el| el.opacity(DISABLED_OPACITY))
    .into_any_element();
    push(section(t("Generations"), palette, Some(rollback)).into_any_element(), SECTION_H_PX.max(PILL_H_PX), &mut out, &mut h);
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut rows_h = 0.0;
    let current_n = current.map(|g| g.number);
    for g in &gens {
        let is_sel = selected == Some(g.number);
        rows.push(gen_row(m, g, is_sel, row_r, palette));
        rows_h += GEN_ROW_H + GEN_ROW_GAP;
        if is_sel {
            rows.push(gen_actions(m, g, current_n, busy, palette));
            rows_h += GEN_ACTIONS_H + GEN_ROW_GAP;
        }
    }
    // The list gives up rows while a job, a comparison or a confirmation shows,
    // so the panel stays within the screen.
    let visible = if diff.is_some() || diff_loading || confirm.is_some() || active().is_some() { GEN_VISIBLE_COMPACT } else { GEN_VISIBLE };
    let list_h = rows_h.min(visible * (GEN_ROW_H + GEN_ROW_GAP)).max(if gens.is_empty() { 0.0 } else { GEN_ROW_H });
    if !gens.is_empty() {
        let list = div().h(px(list_h)).child(div().id("nix-gens").size_full().overflow_y_scroll().flex().flex_col().gap(px(GEN_ROW_GAP)).children(rows)).into_any_element();
        push(list, list_h, &mut out, &mut h);
    }

    // ── Comparison ───────────────────────────────────────────────────────
    if diff_loading || diff.is_some() {
        let (el, dh) = diff_card(diff.as_ref(), diff_loading, card_r, palette);
        push(el, dh, &mut out, &mut h);
    }

    // ── Clean up ─────────────────────────────────────────────────────────
    let (minus, plus, clean) = (m.clone(), m.clone(), m.clone());
    let stepper = |id: &'static str, glyph: &'static str, mm: Shared, delta: i32| {
        crate::kit::icon_button(id, glyph, STEPPER_PX, palette.on_surface_variant, Some(wash(WASH_SOFT, palette)), move |_, cx| {
            let mut mb = mm.borrow_mut();
            let k = mb.nix.keep.unwrap_or(DEFAULT_KEEP) as i32 + delta;
            mb.nix.keep = Some(k.clamp(*nixos::KEEP_RANGE.start() as i32, *nixos::KEEP_RANGE.end() as i32) as u32);
            drop(mb);
            notify(&mm, cx);
        })
        .into_any_element()
    };
    let clean_row = div()
        .h(px(CLEAN_H))
        .flex()
        .items_center()
        .gap(px(INLINE_GAP_PX))
        .text_size(px(text::BODY_S))
        .child(icon("trash", SMALL_ICON_PX, palette.on_surface_variant))
        .child(div().child(t("Keep the newest")))
        .child(stepper("nix-keep-minus", "minus", minus, -1))
        .child(div().font_features(tabular_nums()).font_weight(FontWeight::SEMIBOLD).child(keep.to_string()))
        .child(stepper("nix-keep-plus", "plus", plus, 1))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_color(palette.on_surface_variant.hsla()).child(if deletes == 0 { t("nothing to delete").to_string() } else { tf("{count} to delete", &[("count", &deletes)]) }))
        .child(pill_button("nix-clean", Some("trash"), t("Clean up"), readable(palette.urgent_coral, palette), palette, move |_, cx| {
            if !running() {
                let keep = clean.borrow().nix.keep.unwrap_or(DEFAULT_KEEP);
                clean.borrow_mut().nix.confirm = Some(NixAction::Clean(Cleanup::KeepLatest(keep)));
                notify(&clean, cx);
            }
        })
        .when(busy, |el| el.opacity(DISABLED_OPACITY)))
        .into_any_element();
    push(clean_row, CLEAN_H, &mut out, &mut h);

    // ── Home Manager ─────────────────────────────────────────────────────
    if !home.is_empty() {
        push(section(t("Home Manager"), palette, None).into_any_element(), SECTION_H_PX, &mut out, &mut h);
        let shown: Vec<&HomeGeneration> = home.iter().take(HOME_SHOWN).collect();
        let hh = shown.len() as f32 * HOME_ROW_H;
        let list = div()
            .flex()
            .flex_col()
            .children(shown.into_iter().map(|g| {
                div()
                    .h(px(HOME_ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(INLINE_GAP_PX))
                    .text_size(px(text::BODY_S))
                    .font_features(tabular_nums())
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("#{}", g.number)))
                    .child(div().text_color(palette.on_surface_variant.hsla()).child(ago(g.date)))
                    .when(g.current, |el| el.child(badge(t("Current"), accent, palette)))
            }))
            .into_any_element();
        push(list, hh, &mut out, &mut h);
    }
    (out, h)
}

fn gen_row(m: &Shared, g: &Generation, selected: bool, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = accent.with_alpha(alpha(SELECTED_ROW, palette));
    let n = g.number;
    let m = m.clone();
    let detail = [Some(tf("NixOS {version}", &[("version", &g.nixos_version)])), (!g.kernel.is_empty()).then(|| tf("kernel {kernel}", &[("kernel", &g.kernel)])), g.revision.as_deref().map(|r| short(r).to_string())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    div()
        .id(("nix-gen", n as usize))
        .h(px(GEN_ROW_H))
        .flex_none()
        .px(px(CARD_GAP_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(CARD_GAP_PX))
        .cursor_pointer()
        .when(selected, |el| el.bg(sel.hsla()))
        .when(!selected, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(
            div()
                .w(px(GEN_BADGE_PX + 8.0))
                .flex_none()
                .font_features(tabular_nums())
                .text_size(px(text::BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if g.current { accent.hsla() } else { palette.on_surface.hsla() })
                .child(format!("#{n}")),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).truncate().child(detail))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(ago(g.date))),
        )
        .when(g.current, |el| el.child(badge(t("Current"), accent, palette)))
        .when(g.booted, |el| el.child(badge(t("Booted"), readable(palette.tertiary, palette), palette)))
        .on_click(move |_, _, cx| {
            {
                let mut mb = m.borrow_mut();
                mb.nix.selected = if mb.nix.selected == Some(n) { None } else { Some(n) };
            }
            notify(&m, cx);
        })
        .into_any_element()
}

fn gen_actions(m: &Shared, g: &Generation, current: Option<u32>, busy: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let n = g.number;
    let (ms, mc) = (m.clone(), m.clone());
    div()
        .h(px(GEN_ACTIONS_H))
        .flex_none()
        .pl(px(GEN_BADGE_PX + CARD_GAP_PX * 2.0))
        .flex()
        .items_center()
        .gap(px(PILL_GAP_PX))
        .when(!g.current && !busy, |el| {
            el.child(pill_button("nix-switch-to", Some("zap"), t("Switch to it"), accent, palette, move |_, cx| {
                ms.borrow_mut().nix.confirm = Some(NixAction::SwitchTo(n));
                notify(&ms, cx);
            }))
        })
        .when_some(current.filter(|c| *c != n), |el, c| {
            el.child(pill_button("nix-compare", Some("sliders"), tf("Compare with {n}", &[("n", &c)]), accent, palette, move |_, cx| {
                let (a, b) = if n < c { (n, c) } else { (c, n) };
                compare(&mc, a, b, cx);
                notify(&mc, cx);
            }))
        })
        .when(g.current, |el| el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("This is the running configuration"))))
        .into_any_element()
}

fn diff_card(diff: Option<&(u32, u32, Result<Diff, String>)>, loading: bool, card_r: f32, palette: &MandaraPalette) -> (AnyElement, f32) {
    let accent = readable(palette.primary, palette);
    let mut h = CARD_PAD_PX * 2.0 + TITLE_ROW_H;
    let (title, sub, rows): (String, String, Vec<AnyElement>) = match diff {
        None if loading => (t("Comparing generations…").to_string(), String::new(), Vec::new()),
        None => (String::new(), String::new(), Vec::new()),
        Some((a, b, Err(e))) => (tf("Generation {a} → {b}", &[("a", a), ("b", b)]), e.clone(), Vec::new()),
        Some((a, b, Ok(d))) => {
            let sub = match d.closure_delta {
                Some(delta) => tf("{count} packages changed · closure {size}", &[("count", &d.changes.len()), ("size", &signed_size(delta))]),
                None => tf("{count} packages changed", &[("count", &d.changes.len())]),
            };
            let rows = d
                .changes
                .iter()
                .take(DIFF_SHOWN)
                .map(|c| {
                    let versions = format!("{} → {}", if c.old.is_empty() { "∅" } else { &c.old }, if c.new.is_empty() { "∅" } else { &c.new });
                    div()
                        .h(px(DIFF_ROW_H))
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .text_size(px(text::BODY_S))
                        .font_features(tabular_nums())
                        .child(div().min_w(px(0.0)).truncate().font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                        .child(div().flex_1())
                        .child(div().text_color(palette.on_surface_variant.hsla()).child(versions))
                        .when(c.delta != 0, |el| el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(signed_size(c.delta))))
                        .into_any_element()
                })
                .collect();
            (tf("Generation {a} → {b}", &[("a", a), ("b", b)]), sub, rows)
        }
    };
    if !rows.is_empty() {
        h += CARD_GAP_PX + rows.len() as f32 * DIFF_ROW_H;
    }
    let el = glass_card(palette)
        .rounded(px(card_r))
        .p(px(CARD_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(CARD_GAP_PX))
        .child(
            div()
                .h(px(TITLE_ROW_H))
                .flex()
                .items_center()
                .gap(px(CARD_GAP_PX))
                .child(disc("sliders", accent))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                        .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(sub)),
                ),
        )
        .when(!rows.is_empty(), |el| el.child(div().flex().flex_col().children(rows)))
        .into_any_element();
    (el, h)
}

/// The update request for the current system ("Update all").
pub(super) fn system_request(system: System, flatpak_pending: bool) -> Option<Request> {
    match system {
        System::Managed(_) => Some(Request::System { flatpak: flatpak_pending }),
        System::NixOs if flatpak_pending => Some(Request::Flatpak),
        _ if flatpak_pending => Some(Request::Flatpak),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(outcome: JobOutcome, changed: u32) -> Finished {
        Finished { outcome, elapsed_secs: 1, changed, restart: None, freed: None, inputs: Vec::new(), warnings: 0 }
    }

    #[test]
    fn a_cancel_after_packages_changed_says_how_many() {
        let request = Request::System { flatpak: true };
        assert_eq!(summary_text(&request, &finished(JobOutcome::Cancelled, 0)), (t("Update cancelled").to_string(), t("Nothing was installed").to_string()));
        assert_eq!(summary_text(&request, &finished(JobOutcome::Cancelled, 1)).1, t("Updated 1 package"));
        assert_eq!(summary_text(&request, &finished(JobOutcome::Cancelled, 12)).1, tf("Updated {count} packages", &[("count", &12)]));
    }

    #[test]
    fn the_clean_up_confirmation_counts_what_delete_runs() {
        let now = 100 * 86_400;
        let gens: Vec<Generation> = (1..=8).map(|n| Generation { number: n, date: now - i64::from(9 - n) * 86_400, current: n == 8, ..Generation::default() }).collect();
        let shown = nixos::to_delete(&gens, Cleanup::KeepLatest(10), now).len();
        assert_eq!(shown, 0, "the stepper, raised to 10 after Clean up");
        // Clean up was pressed with 3: Delete still deletes all but the newest 3.
        let confirmed = confirm_deletes(NixAction::Clean(Cleanup::KeepLatest(3)), 10, shown, &gens, now);
        assert_eq!(confirmed, 5);
        assert_eq!(confirm_text(NixAction::Clean(Cleanup::KeepLatest(3)), confirmed), tf("Delete {count} old generations and collect garbage?", &[("count", &5)]));
        assert_eq!(confirm_deletes(NixAction::Clean(Cleanup::KeepLatest(10)), 10, shown, &gens, now), 0);
        assert_eq!(confirm_deletes(NixAction::Clean(Cleanup::OlderThanDays(4)), 10, shown, &gens, now), 4);
        assert_eq!(confirm_deletes(NixAction::Rollback, 10, shown, &gens, now), shown);
    }
}
