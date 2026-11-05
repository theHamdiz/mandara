//! Studio → Compositor → **Config doctor**: the compositor's own checker
//! run on the user's config (`mandara_config::doctor`), its findings grouped
//! by file with the offending line, a plain-language explanation and the
//! fixes that apply — open in your editor, comment out, remove (your files:
//! backed up, re-checked, rolled back when worse) or regenerate (Mandara's
//! drop-in). "Check again" re-runs the checker; "Remind me later" quiets
//! the start-up notification until the config or the compositor changes.
//!
//! Every check and fix runs on a background thread; the page only reads
//! finished reports.

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::compositor::{Target, is_generated};
use mandara_config::doctor::{self, CheckerStatus, Diagnostic, Owner, Report, Severity, fix::Fix, state as dstate};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;

use super::StudioView;
use super::compositor_page::{badge, button, card};
use crate::tokens::*;

/// Seconds per minute / hour / day, for "checked … ago".
const SECS_PER_MIN: u64 = 60;
const SECS_PER_HOUR: u64 = 60 * SECS_PER_MIN;
const SECS_PER_DAY: u64 = 24 * SECS_PER_HOUR;

#[derive(Default)]
struct DoctorUi {
    /// Last report per compositor (by `Target::index`).
    reports: [Option<Report>; Target::ALL.len()],
    /// A check is running.
    running: [bool; Target::ALL.len()],
    /// The stored report was asked for (once per compositor).
    started: [bool; Target::ALL.len()],
    /// The diagnostic a fix is running for.
    busy: Option<String>,
    /// The last fix's outcome: (worked, text).
    message: Option<(bool, String)>,
    snoozed: [bool; Target::ALL.len()],
}
impl Global for DoctorUi {}

fn doctor_ui<'a>(cx: &'a mut Context<StudioView>) -> &'a mut DoctorUi {
    if !cx.has_global::<DoctorUi>() {
        cx.set_global(DoctorUi::default());
    }
    cx.global_mut::<DoctorUi>()
}

/// "just now", "5 min ago", "3 h ago", "2 d ago".
fn ago(unix: u64) -> String {
    let s = doctor_now().saturating_sub(unix);
    if s < SECS_PER_MIN {
        t("just now").into()
    } else if s < SECS_PER_HOUR {
        tf("{minutes} min ago", &[("minutes", &(s / SECS_PER_MIN))])
    } else if s < SECS_PER_DAY {
        tf("{hours} h ago", &[("hours", &(s / SECS_PER_HOUR))])
    } else {
        tf("{days} d ago", &[("days", &(s / SECS_PER_DAY))])
    }
}

fn doctor_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// `~/…` for paths in the home folder.
fn short_path(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(|p| p.display().to_string())) {
        Some(rest) => format!("~/{rest}"),
        None => path.display().to_string(),
    }
}

fn severity_color(s: Severity, p: &MandaraPalette) -> Color {
    match s {
        Severity::Error => p.urgent_coral,
        Severity::Warning => p.warning_amber,
        Severity::Info => p.on_surface_variant,
    }
}

fn diagnostic_details(report: &Report) -> bool {
    report.status == CheckerStatus::Ran && report.diagnostics.iter().any(|diagnostic| show_diagnostic(report.target, diagnostic))
}

fn show_diagnostic(target: Target, diagnostic: &Diagnostic) -> bool {
    target != Target::KWin || diagnostic.severity != Severity::Info
}

fn kwin_availability_note(report: &Report) -> bool {
    report.target == Target::KWin && report.status == CheckerStatus::Ran && report.problems() == 0
}

impl StudioView {
    /// Loads (`fresh = false`: the stored report, re-checked only when the
    /// config or compositor changed) or re-runs (`fresh`) the check.
    fn doctor_check(&mut self, target: Target, fresh: bool, cx: &mut Context<Self>) {
        let i = target.index();
        let ui = doctor_ui(cx);
        if ui.running[i] {
            return;
        }
        ui.running[i] = true;
        ui.started[i] = true;
        let task = cx.background_executor().spawn(async move {
            let report = if fresh { dstate::record(doctor::check(target)) } else { dstate::check_cached(target).0 };
            let snoozed = dstate::is_snoozed(&report);
            (report, snoozed)
        });
        cx.spawn(async move |this, cx| {
            let (report, snoozed) = task.await;
            let _ = this.update(cx, |_, cx| {
                let ui = doctor_ui(cx);
                ui.reports[i] = Some(report);
                ui.snoozed[i] = snoozed;
                ui.running[i] = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn doctor_fix(&mut self, target: Target, id: String, fix: Fix, cfg: &ShellConfig, cx: &mut Context<Self>) {
        let ui = doctor_ui(cx);
        let Some(report) = ui.reports[target.index()].clone() else { return };
        if ui.busy.is_some() {
            return;
        }
        ui.busy = Some(id.clone());
        ui.message = None;
        let shell = cx.try_global::<crate::perf::ShellQuery>().cloned();
        let compositor = cfg.effective_compositor().into_owned();
        let task = cx.background_executor().spawn(async move {
            // Mandara's drop-in is the shell's to write (border colours,
            // reload); without a shell Studio writes it itself.
            let regenerate = || -> anyhow::Result<()> {
                if let Some(q) = &shell
                    && (q.0)(vec!["compositor".into(), "sync".into()]).is_ok()
                {
                    return Ok(());
                }
                mandara_compositor::backend::write_dropin(target, &compositor, None, mandara_config::compositor::installed_version(target)).map(|_| ())
            };
            let env = doctor::CheckEnv { version: report.version.clone(), ..doctor::CheckEnv::default() };
            let d = report.find(&id).cloned();
            let outcome = doctor::fix::apply(&report, &id, fix, &env, &regenerate);
            match outcome {
                Ok(o) => {
                    let after = dstate::record(o.after.clone());
                    let line = d.as_ref().and_then(|d| d.line).unwrap_or_default();
                    let file = d.as_ref().and_then(|d| d.file.as_deref()).map(short_path).unwrap_or_default();
                    let backup = o.backup.as_deref().map(short_path).unwrap_or_default();
                    let compositor = target.name();
                    let text = if o.rolled_back {
                        (false, tf("That made {compositor} report more problems, so it was undone.", &[("compositor", &compositor)]))
                    } else if !o.resolved {
                        (false, tf("Done, but {compositor} still reports the problem.", &[("compositor", &compositor)]))
                    } else {
                        match fix {
                            Fix::Comment => (true, tf("Commented out line {line} of {file}. Backup: {backup}", &[("line", &line), ("file", &file), ("backup", &backup)])),
                            Fix::Remove => (true, tf("Removed line {line} of {file}. Backup: {backup}", &[("line", &line), ("file", &file), ("backup", &backup)])),
                            Fix::Regenerate => (true, t("Regenerated Mandara's drop-in.").to_string()),
                        }
                    };
                    (Some(after), text)
                }
                Err(err) => (None, (false, format!("{err:#}"))),
            }
        });
        cx.spawn(async move |this, cx| {
            let (after, message) = task.await;
            let _ = this.update(cx, |_, cx| {
                let ui = doctor_ui(cx);
                if let Some(after) = after {
                    ui.reports[target.index()] = Some(after);
                }
                ui.busy = None;
                ui.message = Some(message);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn doctor_snooze(&mut self, target: Target, cx: &mut Context<Self>) {
        let Some(report) = doctor_ui(cx).reports[target.index()].clone() else { return };
        doctor_ui(cx).snoozed[target.index()] = true;
        cx.background_executor().spawn(async move { dstate::snooze(&report) }).detach();
        cx.notify();
    }

    /// The Config doctor card for `target`'s config.
    pub(super) fn config_doctor(&mut self, cfg: &ShellConfig, target: Target, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let i = target.index();
        if !doctor_ui(cx).started[i] {
            self.doctor_check(target, false, cx);
        }
        let ui = doctor_ui(cx);
        let (report, running, snoozed, busy, message) = (ui.reports[i].clone(), ui.running[i], ui.snoozed[i], ui.busy.clone(), ui.message.clone());
        let compositor = target.name();
        let version = report.as_ref().and_then(|r| r.version.clone()).map(|v| format!("{compositor} {v}")).unwrap_or_else(|| compositor.to_string());
        let problems = report.as_ref().map_or(0, Report::problems);
        let (status, tone, glyph) = match &report {
            None => (tf("Checking your {compositor} config…", &[("compositor", &compositor)]), p.on_surface_variant, "refresh"),
            Some(r) => match &r.status {
                CheckerStatus::NoConfig => (tf("{compositor} has no config file here yet.", &[("compositor", &compositor)]), p.on_surface_variant, "note"),
                CheckerStatus::NotInstalled => (tf("{compositor} isn't installed here, so its config can't be checked.", &[("compositor", &compositor)]), p.on_surface_variant, "note"),
                CheckerStatus::TimedOut => (tf("{compositor}'s checker didn't finish in time.", &[("compositor", &compositor)]), p.warning_amber, "alert"),
                CheckerStatus::Failed(e) => (tf("{compositor}'s checker couldn't run: {error}", &[("compositor", &compositor), ("error", &e)]), p.warning_amber, "alert"),
                CheckerStatus::Ran if kwin_availability_note(r) => (t("KWin has no standalone configuration checker. Its settings are managed in Plasma System Settings.").to_string(), p.on_surface_variant, "note"),
                CheckerStatus::Ran if problems == 0 => (tf("All good — checked {when} with {compositor}", &[("when", &ago(r.checked_at)), ("compositor", &version)]), p.success_emerald, "check"),
                CheckerStatus::Ran if problems == 1 => (tf("1 problem — checked {when} with {compositor}", &[("when", &ago(r.checked_at)), ("compositor", &version)]), if r.errors() > 0 { p.urgent_coral } else { p.warning_amber }, "alert"),
                CheckerStatus::Ran => (tf("{count} problems — checked {when} with {compositor}", &[("count", &problems), ("when", &ago(r.checked_at)), ("compositor", &version)]), if r.errors() > 0 { p.urgent_coral } else { p.warning_amber }, "alert"),
            },
        };
        let header = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_14_PX))
            .child(div().size(px(DIM_44_PX)).rounded(px(RADIUS_14_PX)).row().items_center().justify_center().flex_none().bg(tone.with_alpha(ALPHA_15).hsla()).child(icon(glyph, ICON_22_PX, tone)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_200_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Config doctor"))).when(snoozed && problems > 0, |el| el.child(badge(t("Reminders paused"), p.on_surface_variant))))
                    .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(status))),
            )
            .child(button("doctor-check", "refresh", if running { t("Checking…") } else { t("Check again") }, false, p).when(!running, |el| el.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.doctor_check(target, true, cx)))))
            .when(problems > 0 && !snoozed, |el| el.child(button("doctor-snooze", "clock", t("Remind me later"), false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.doctor_snooze(target, cx)))));
        let mut col = card(p).child(header);
        if let Some(r) = &report
            && !r.verified_parser
            && r.status == CheckerStatus::Ran
        {
            col = col.child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tf("Mandara reads {compositor}'s checker from its documented output; tell us on GitHub if a problem shows up wrong.", &[("compositor", &compositor)]))));
        }
        if let Some((ok, text)) = message {
            let color = if ok { p.success_emerald } else { p.urgent_coral };
            col = col.child(div().px(px(SPACE_14_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).text_size(px(TEXT_12_5_PX)).bg(color.with_alpha(ALPHA_12).hsla()).text_color(color.hsla()).child(mandara_ui::bidi::paragraph(text)));
        }
        if let Some(mut r) = report.filter(diagnostic_details) {
            let target = r.target;
            r.diagnostics.retain(|diagnostic| show_diagnostic(target, diagnostic));
            let mono = mandara_ui::fonts::fonts(cx).mono.clone();
            let terminal = cfg.apps.terminal_command();
            for (file, list) in r.by_file() {
                let ours = file.as_deref().is_some_and(is_generated);
                let head = div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .pt(px(SPACE_6_PX))
                    .child(icon(if ours { "sparkles" } else { "note" }, ICON_14_PX, p.on_surface_variant))
                    .child(div().min_w(px(0.0)).overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(mono.clone()).text_size(px(TEXT_12_PX)).text_color(p.on_surface.hsla()).child(file.as_deref().map(short_path).unwrap_or_else(|| t("Somewhere in your config").to_string())))
                    .child(badge(if ours { t("Mandara's drop-in") } else { t("Your config") }, if ours { p.primary } else { p.on_surface_variant }));
                col = col.child(head);
                for d in list {
                    col = col.child(self.doctor_row(cfg, &r, d, busy.as_deref(), terminal.clone(), mono.clone(), p, cx));
                }
            }
        }
        col.into_any_element()
    }

    /// One problem: what, where, why, and the buttons that fix it.
    fn doctor_row(&mut self, cfg: &ShellConfig, r: &Report, d: &Diagnostic, busy: Option<&str>, terminal: Option<String>, mono: SharedString, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let target = r.target;
        let tone = severity_color(d.severity, p);
        let working = busy == Some(d.id.as_str());
        let any_busy = busy.is_some();
        let line_label = d.line.map(|l| if d.guessed { tf("Line {line} (found by name)", &[("line", &l)]) } else { tf("Line {line}", &[("line", &l)]) });
        let top = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_8_PX))
            .child(div().size(px(DIM_8_PX)).rounded_full().flex_none().bg(tone.hsla()))
            .child(badge(tr(d.kind.label()), tone))
            .when_some(line_label, |el, l| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(l)));
        let explanation = div().text_size(px(TEXT_13_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface.hsla()).child(mandara_ui::bidi::paragraph(doctor::explain(d, target, r.version.as_deref())));
        let snippet = d.text.clone().map(|text| {
            let number = d.line.map(|l| format!("{l}  ")).unwrap_or_default();
            div()
                .ltr_row()
                .rounded(px(RADIUS_10_PX))
                .bg(p.surface_dim.hsla())
                .px(px(SPACE_12_PX))
                .py(px(SPACE_8_PX))
                .font_family(mono.clone())
                .text_size(px(TEXT_12_PX))
                .child(div().flex_none().text_color(p.on_surface_variant.hsla()).child(number))
                .child(div().min_w(px(0.0)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(p.on_surface.hsla()).child(text))
        });
        let says = div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(tf("{compositor} says: {message}", &[("compositor", &target.name()), ("message", &d.message)]));
        let related = (!d.related.is_empty()).then(|| {
            let places = d.related.iter().map(|l| format!("{}:{}", short_path(&l.file), l.line)).collect::<Vec<_>>().join(", ");
            div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(tf("Also here: {places}", &[("places", &places)]))
        });
        let mut actions = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX));
        if let (Some(file), Some(line)) = (d.file.clone(), d.line) {
            let terminal = terminal.clone();
            actions = actions.child(button(SharedString::from(format!("doctor-open-{}", d.id)), "pencil", t("Open in editor"), false, p).on_click(move |_: &ClickEvent, _, _| {
                let (file, terminal) = (file.clone(), terminal.clone());
                std::thread::spawn(move || {
                    if let Err(err) = mandara_services::editor::open_at(&file, line, terminal.as_deref()) {
                        tracing::warn!("config doctor: opening {}: {err}", file.display());
                    }
                });
            }));
        }
        for fix in doctor::fixes_for(d) {
            let (glyph, label, filled) = match fix {
                Fix::Comment => ("note", t("Comment out"), true),
                Fix::Remove => ("trash", t("Remove line"), false),
                Fix::Regenerate => ("refresh", t("Regenerate"), true),
            };
            let label = if working { t("Working…") } else { label };
            let id = d.id.clone();
            let cfg = cfg.clone();
            actions = actions.child(
                button(SharedString::from(format!("doctor-{}-{}", fix.id(), d.id)), glyph, label, filled, p)
                    .when(any_busy, |el| el.opacity(ALPHA_60))
                    .when(!any_busy, |el| el.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.doctor_fix(target, id.clone(), fix, &cfg, cx)))),
            );
        }
        let ours = d.owner == Owner::Mandara;
        div()
            .id(SharedString::from(format!("doctor-row-{}", d.id)))
            .clip_rounded()
            .rounded(px(RADIUS_14_PX))
            .bg(p.surface_container_high.hsla())
            .border_1()
            .border_color(tone.with_alpha(if ours { ALPHA_20 } else { ALPHA_30 }).hsla())
            .p(px(SPACE_14_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(top)
            .child(explanation)
            .when_some(snippet, |el, s| el.child(s))
            .child(says)
            .when_some(related, |el, r| el.child(r))
            .child(actions)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kwin_availability_stays_a_single_status_note_without_reminders() {
        let dir = tempfile::tempdir().unwrap();
        let report = doctor::check_with(Target::KWin, &dir.path().join("kwinrc"), &doctor::CheckEnv {
            argv: Some(vec!["printf".into(), "kwin 6.8.2".into()]),
            version: Some("6.8.2".into()),
            ..Default::default()
        });
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.problems(), 0);
        assert!(!diagnostic_details(&report));
        assert!(kwin_availability_note(&report));
        let mut other = report;
        other.target = Target::Sway;
        assert!(diagnostic_details(&other), "other compositor notes must remain visible");
        other.diagnostics.push(Diagnostic::new(Severity::Warning, "fixture warning"));
        assert_eq!(other.problems(), 1);
        assert!(diagnostic_details(&other));
    }

    #[test]
    fn kwin_probe_errors_are_visible_alongside_a_problem_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = doctor::check_with(Target::KWin, &dir.path().join("kwinrc"), &doctor::CheckEnv {
            argv: Some(vec!["false".into()]), version: Some("6.8.2".into()), ..Default::default()
        });
        assert_eq!(report.status, CheckerStatus::Ran);
        assert!(report.errors() > 0);
        assert!(diagnostic_details(&report));
        assert!(!kwin_availability_note(&report));
        report.diagnostics.push(Diagnostic::new(Severity::Info, "availability note"));
        assert_eq!(report.diagnostics.iter().filter(|diagnostic| show_diagnostic(Target::KWin, diagnostic)).count(), report.problems());
    }
}
