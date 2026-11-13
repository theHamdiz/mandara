//! Studio → Profiles & backup.
//!
//! * **Recent changes** — every edit made in Studio, newest first, with the
//!   settings it touched; Undo restores the configuration from just before
//!   it (slider drags coalesce into one entry).
//! * **Profiles** — save the look (or everything) under a name and switch
//!   between them (`mandara msg profile apply <name>`).
//! * **Export / import** — the whole config or only what differs from the
//!   defaults, optionally with personal values (locations, keys, accounts)
//!   redacted; importing backs the current settings up first.
//! * **Reset a page** — put one Studio page's settings back to defaults.
//! * **Support report** — versions, compositor and a redacted config in one
//!   text file for bug reports.

use mandara_ui::bidi::Bidi as _;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::profiles::{self, ExportOptions, Profile, ProfileScope};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::{config, update_config};

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title, heading, tip};
use super::form::{self, note};
use crate::registry::{self, Section};
use crate::tokens::*;

/// Changes kept for undo.
const HISTORY_MAX: usize = 30;
/// Edits to the same settings this close together are one change (drags).
const COALESCE: Duration = Duration::from_millis(1500);
/// Changes listed before "show all".
const HISTORY_SHOWN: usize = 8;
/// Setting labels named per change before "+N more".
const LABELS_SHOWN: usize = 3;
/// A destructive click arms for this long.
const ARM_WINDOW: Duration = Duration::from_secs(4);
/// Seconds per minute / hour (relative times).
const SECS_PER_MIN: u64 = 60;
const SECS_PER_HOUR: u64 = 3600;
/// Page-reset chips.
const RESET_GRID: CardGrid = CardGrid { min_w: 170.0, max_w: 300.0, gap: SPACE_8_PX };
/// Lists edited by page-specific designers, outside the scalar registry.
/// Counts and reset use the same paths so a changed arrangement is resettable.
const EXTRA_RESET_PATHS: &[(Section, &[&str])] = &[
    (Section::Themes, &["/studio"]),
    (Section::Bars, &["/bar/left", "/bar/center", "/bar/right", "/side_bars/left/top", "/side_bars/left/middle", "/side_bars/left/bottom", "/side_bars/right/top", "/side_bars/right/middle", "/side_bars/right/bottom", "/sidebar/tabs"]),
    (Section::Dock, &["/dock/pinned", "/dock/stacks"]),
];

/// One recorded change: when, what (setting labels), and the config before.
struct Change {
    at: Instant,
    labels: Vec<String>,
    before: Arc<ShellConfig>,
}

#[derive(Default)]
struct History {
    changes: VecDeque<Change>,
    last: Option<Arc<ShellConfig>>,
    /// The next change is an undo (not recorded).
    undoing: bool,
}
impl Global for History {}

#[derive(Default)]
struct ProfilesStudio {
    profiles: Option<Vec<Profile>>,
    scope: ProfileScope,
    changes_only: bool,
    redact: bool,
    all_history: bool,
    armed: Option<(String, Instant)>,
    note: Option<(String, bool)>,
    /// Sensitive values in the last export.
    sensitive: Vec<String>,
}
impl Global for ProfilesStudio {}

fn st(cx: &mut App) -> &mut ProfilesStudio {
    cx.default_global::<ProfilesStudio>()
}

/// The labels of registry settings that differ between two configs (runs
/// on every config change, so each config is serialized once, not twice
/// per setting).
fn changed_labels(before: &ShellConfig, after: &ShellConfig) -> Vec<String> {
    let mut out: Vec<String> = registry::differing(before, after).into_iter().map(|s| tr(s.label)).collect();
    out.dedup();
    if out.is_empty() && before != after {
        out.push(t("Other settings").into());
    }
    out
}

/// Starts recording config changes (called once when Studio opens).
pub(super) fn track_history(cx: &mut Context<StudioView>) {
    let store = config(cx);
    cx.default_global::<History>().last = Some(store.read(cx).snapshot());
    cx.observe(&store, |_, store, cx| {
        let now = store.read(cx).snapshot();
        let h = cx.default_global::<History>();
        let Some(prev) = h.last.replace(now.clone()) else { return };
        if Arc::ptr_eq(&prev, &now) || *prev == *now {
            return;
        }
        if std::mem::take(&mut h.undoing) {
            return;
        }
        let labels = changed_labels(&prev, &now);
        match h.changes.front_mut() {
            // A drag: same settings, moments apart — keep the first "before".
            Some(c) if c.at.elapsed() < COALESCE && c.labels == labels => c.at = Instant::now(),
            _ => {
                h.changes.push_front(Change { at: Instant::now(), labels, before: prev });
                h.changes.truncate(HISTORY_MAX);
            }
        }
    })
    .detach();
}

/// Restores the config from before change `i` (dropping it and newer ones).
fn undo_to(i: usize, cx: &mut App) {
    let h = cx.default_global::<History>();
    let Some(before) = h.changes.get(i).map(|c| c.before.clone()) else { return };
    h.changes.drain(..=i);
    h.undoing = true;
    update_config(cx, move |c| *c = (*before).clone());
}

/// Returns a page reset without writing configuration. Registry values and
/// bespoke layout lists reset together, including every Bars tab. Layout
/// defaults respect removed plugins; unrelated pages keep their current values.
pub(super) fn reset_section(cfg: &ShellConfig, section: Section) -> ShellConfig {
    let mut next = cfg.clone();
    for s in registry::in_section(section).filter(|s| !s.path.starts_with("/studio/")) {
        let v = registry::default_value(s);
        if let Ok(c) = registry::set(&next, s, v) {
            next = c;
        }
    }
    if section == Section::Themes {
        next.studio = None;
    } else if let Some((_, paths)) = EXTRA_RESET_PATHS.iter().find(|(s, _)| *s == section) {
        let dotted: Vec<String> = paths.iter().map(|path| path.trim_start_matches('/').replace('/', ".")).collect();
        let paths: Vec<&str> = dotted.iter().map(String::as_str).collect();
        match profiles::snapshot_paths(&registry::layout_defaults(cfg), "", "", &paths, "").and_then(|defaults| profiles::apply(&next, &defaults)) {
            Ok(reset) => next = reset,
            Err(error) => tracing::error!("resetting custom page settings: {error:#}"),
        }
    }
    next
}

fn ago(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    if s < SECS_PER_MIN {
        t("just now").into()
    } else if s < SECS_PER_HOUR {
        tf("{minutes} min ago", &[("minutes", &(s / SECS_PER_MIN))])
    } else {
        tf("{hours} h ago", &[("hours", &(s / SECS_PER_HOUR))])
    }
}

/// A change's setting labels, the first few named and the rest counted.
fn summary(labels: &[String]) -> String {
    let mut what = labels.iter().take(LABELS_SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if labels.len() > LABELS_SHOWN {
        what.push_str(&tf(" +{count} more", &[("count", &(labels.len() - LABELS_SHOWN))]));
    }
    what
}

/// Home → the newest `n` changes (what, how long ago) and how many are kept.
pub(super) fn recent_changes(n: usize, cx: &mut App) -> (Vec<(String, String)>, usize) {
    let h = cx.default_global::<History>();
    (h.changes.iter().take(n).map(|c| (summary(&c.labels), ago(c.at))).collect(), h.changes.len())
}

/// Home → undoes the newest change.
pub(super) fn undo_last(cx: &mut App) {
    undo_to(0, cx);
}

/// Home → the saved profiles' names (read from disk once, like the page).
pub(super) fn profile_names(cx: &mut App) -> Vec<String> {
    let s = st(cx);
    s.profiles.get_or_insert_with(profiles::list).iter().map(|p| p.name.clone()).collect()
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// How many registry settings of each section differ from the defaults:
/// one pass over the registry against the cached default tree (the page
/// draws this every frame, so it must not serialize a config per setting).
fn changed_per_section(cfg: &ShellConfig) -> HashMap<Section, usize> {
    let mut out = HashMap::new();
    for s in registry::non_default(cfg) {
        *out.entry(s.section).or_default() += 1;
    }
    for (section, paths) in EXTRA_RESET_PATHS {
        let changed = registry::changed_paths(cfg, paths);
        if changed > 0 { *out.entry(*section).or_default() += changed; }
    }
    out
}

/// Versions, session and a redacted changes-only config, for bug reports.
fn support_report(cfg: &ShellConfig, version: &str) -> String {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let os = read("/etc/os-release").lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string()).unwrap_or_default();
    let kernel = mandara_ui::widgets::system::kernel_release();
    let compositor = mandara_compositor::detected().map(|k| format!("{k:?}")).unwrap_or_else(|| "none".into());
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let (config_text, _) = profiles::export_text(cfg, ExportOptions { changes_only: true, redact: true }).unwrap_or_default();
    format!(
        "Mandara Shell support report\n\nVersion: {version}\nOS: {os}\nKernel: {kernel}\nCompositor: {compositor}\nSession: {} / {}\nLocale: {}\n\n── Settings (changed from the defaults, personal values redacted) ──\n{config_text}",
        env("XDG_CURRENT_DESKTOP"),
        env("XDG_SESSION_TYPE"),
        env("LANG")
    )
}

impl StudioView {
    fn prof_arm(key: &str, cx: &mut App) -> bool {
        let s = st(cx);
        if s.armed.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < ARM_WINDOW) {
            s.armed = None;
            true
        } else {
            s.armed = Some((key.to_string(), Instant::now()));
            false
        }
    }

    fn prof_armed(key: &str, cx: &mut App) -> bool {
        st(cx).armed.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < ARM_WINDOW)
    }

    pub(super) fn profiles_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if st(cx).profiles.is_none() {
            st(cx).profiles = Some(profiles::list());
        }
        let note_el = st(cx).note.clone().map(|(text, error)| note(&text, error, p));
        let history = self.prof_history(p, cx);
        let saved = self.prof_profiles(p, cx);
        let transfer = self.prof_transfer(p, cx);
        let reset = self.prof_reset(cfg, p, cx);
        let section = |title: String, body: AnyElement| div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(title, p)).child(body);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .children(note_el)
            .child(section(t("Recent changes").into(), history))
            .child(section(t("Profiles").into(), saved))
            .child(section(t("Export, import & support").into(), transfer))
            .child(section(t("Reset a page").into(), reset))
            .into_any_element()
    }

    fn prof_history(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let all = st(cx).all_history;
        let rows: Vec<(usize, Instant, Vec<String>)> = cx.default_global::<History>().changes.iter().enumerate().map(|(i, c)| (i, c.at, c.labels.clone())).collect();
        if rows.is_empty() {
            return card(p).child(heading(t("No changes yet"), t("Edits you make in Studio appear here, newest first, with an Undo."), p)).into_any_element();
        }
        let total = rows.len();
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        for (i, at, labels) in rows.into_iter().take(if all { usize::MAX } else { HISTORY_SHOWN }) {
            let what = summary(&labels);
            col = col.child(
                div()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_8_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_3_5).hsla()))
                    .child(icon("refresh", ICON_14_PX, if i == 0 { p.primary } else { p.on_surface_variant }))
                    .child(div().flex_1().min_w(px(0.0)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(what))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(ago(at)))
                    .child(
                        chip(SharedString::from(format!("undo-{i}")), if i == 0 { t("Undo") } else { t("Back to before this") }, false, p)
                            .child(icon("undo", ICON_11_PX, p.primary))
                            .when(i > 0, |el| el.tooltip(tip(t("Also undoes the newer changes above"), p)))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                undo_to(i, cx);
                                st(cx).note = Some((t("Undone").into(), false));
                                cx.notify();
                            })),
                    ),
            );
        }
        if total > HISTORY_SHOWN {
            col = col.child(div().p(px(SPACE_6_PX)).child(chip("history-all", if all { SharedString::from(t("Fewer")) } else { SharedString::from(tf("All {count}", &[("count", &total)])) }, false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                let s = st(cx);
                s.all_history = !s.all_history;
                cx.notify();
            }))));
        }
        col.into_any_element()
    }

    fn prof_profiles(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (list, scope) = {
            let s = st(cx);
            (s.profiles.clone().unwrap_or_default(), s.scope)
        };
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        if list.is_empty() {
            col = col.child(div().p(px(SPACE_10_PX)).child(heading(t("No profiles yet"), t("Save how things look now — say “Work” and “Night” — and switch with one click or `mandara msg profile apply Night`."), p)));
        }
        for prof in list {
            let del_key = format!("profile-del-{}", prof.name);
            let armed = Self::prof_armed(&del_key, cx);
            let (n1, n2) = (prof.name.clone(), prof.name.clone());
            col = col.child(
                div()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(icon(match prof.scope { ProfileScope::Bar => "monitor", ProfileScope::Look => "sun", ProfileScope::Full => "package" }, ICON_16_PX, p.primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_140_PX))
                            .row()
                            .flex_col()
                            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().font_weight(FontWeight::MEDIUM).child(prof.name.clone())).child(badge(match prof.scope { ProfileScope::Bar => t("Bar"), ProfileScope::Look => t("Look"), ProfileScope::Full => t("Everything") }, p.tertiary)))
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(if prof.description.is_empty() { tf("Saved {when}", &[("when", &prof.saved)]) } else { prof.description.clone() })),
                    )
                    .child(button(SharedString::from(format!("profile-apply-{}", prof.name)), "check", t("Apply"), true, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let Some(profile) = profiles::find(&n1) else { return };
                        let cfg = config(cx).read(cx).snapshot();
                        match profiles::apply(&cfg, &profile) {
                            Ok(next) => {
                                update_config(cx, move |c| *c = next);
                                st(cx).note = Some((tf("Switched to {name} — Undo is under Recent changes", &[("name", &profile.name)]), false));
                            }
                            Err(e) => st(cx).note = Some((format!("{e:#}"), true)),
                        }
                        cx.notify();
                    })))
                    .child(chip(SharedString::from(del_key.clone()), if armed { t("Delete?") } else { t("Delete") }, false, p).when(armed, |el| el.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()).text_color(p.urgent_coral.hsla())).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        if Self::prof_arm(&del_key, cx) {
                            let r = profiles::delete(&n2);
                            let s = st(cx);
                            s.profiles = None;
                            if let Err(e) = r {
                                s.note = Some((format!("{e:#}"), true));
                            }
                        }
                        cx.notify();
                    }))),
            );
        }
        let name = form::input("profile-name", t("Profile name, e.g. Night"), false, cx);
        let scope_chip = |id: &'static str, label: &'static str, value: ProfileScope, cx: &mut Context<Self>| {
            chip(id, label, scope == value, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                st(cx).scope = value;
                cx.notify();
            }))
        };
        let look = scope_chip("profile-scope-look", t("Look only"), ProfileScope::Look, cx).tooltip(tip(t("Theme, bar, motion, typography, wallpaper, sidebar and window look"), p));
        let full = scope_chip("profile-scope-full", t("Everything"), ProfileScope::Full, cx);
        let bar = scope_chip("profile-scope-bar", t("Bar"), ProfileScope::Bar, cx);
        let save = button("profile-save", "plus", t("Save profile"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            let name = form::value("profile-name", cx);
            if name.is_empty() {
                st(cx).note = Some((t("Name the profile first").into(), true));
                return cx.notify();
            }
            let cfg = config(cx).read(cx).snapshot();
            let scope = st(cx).scope;
            let saved = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            let r = profiles::snapshot(&cfg, &name, "", scope, &saved).and_then(|prof| profiles::save(&prof));
            form::clear("profile-name", cx);
            let s = st(cx);
            s.profiles = None;
            s.note = Some(match r {
                Ok(_) => (tf("Saved {name}", &[("name", &name)]), false),
                Err(e) => (format!("{e:#}"), true),
            });
            cx.notify();
        }));
        col.child(div().p(px(SPACE_10_PX)).row().flex_wrap().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(DIM_200_PX)).child(name)).child(bar).child(look).child(full).child(save)).into_any_element()
    }

    fn prof_transfer(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (changes_only, redact, sensitive) = {
            let s = st(cx);
            (s.changes_only, s.redact, s.sensitive.clone())
        };
        let toggle = |id: &'static str, label: &'static str, on: bool, flip: fn(&mut ProfilesStudio), cx: &mut Context<Self>| {
            chip(id, label, on, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                flip(st(cx));
                cx.notify();
            }))
        };
        let path = form::input("export-path", t("File, e.g. ~/mandara-settings.toml"), false, cx);
        let export = button("export", "download", t("Export"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            let raw = form::value("export-path", cx);
            let path = mandara_config::paths::expand_tilde(if raw.is_empty() { "~/mandara-settings.toml" } else { &raw });
            let (changes_only, redact) = (st(cx).changes_only, st(cx).redact);
            let cfg = config(cx).read(cx).snapshot();
            let result = if path.exists() {
                Err(tf("{file} already exists", &[("file", &path.display())]))
            } else {
                profiles::export_text(&cfg, ExportOptions { changes_only, redact }).map_err(|e| format!("{e:#}")).and_then(|(text, sensitive)| {
                    mandara_config::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{e:#}"))?;
                    Ok(sensitive)
                })
            };
            let s = st(cx);
            match result {
                Ok(sensitive) => {
                    s.note = Some((tf("Exported to {file}", &[("file", &path.display())]), false));
                    s.sensitive = sensitive;
                }
                Err(e) => s.note = Some((e, true)),
            }
            cx.notify();
        }));
        let import_path = form::input("import-path", t("Exported file to import"), false, cx);
        let import_key = "import-confirm".to_string();
        let armed = Self::prof_armed(&import_key, cx);
        let import = button("import", "arrow-up", if armed { t("Replace my settings?") } else { t("Import") }, false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            let raw = form::value("import-path", cx);
            if raw.is_empty() {
                st(cx).note = Some((t("Choose a file to import").into(), true));
                return cx.notify();
            }
            if !Self::prof_arm(&import_key, cx) {
                return cx.notify();
            }
            let path = mandara_config::paths::expand_tilde(&raw);
            let result = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display())).and_then(|text| profiles::import_text(&text).map_err(|e| format!("{e:#}"))).and_then(|next| {
                let backup = crate::maintenance::backup_now("before-import")?;
                Ok((next, backup))
            });
            match result {
                Ok((next, backup)) => {
                    update_config(cx, move |c| *c = next);
                    form::clear("import-path", cx);
                    st(cx).note = Some((tf("Imported — the previous settings are in {folder}", &[("folder", &backup.display())]), false));
                }
                Err(e) => st(cx).note = Some((e, true)),
            }
            cx.notify();
        }));
        let version = mandara_core::build_info(cx).long.clone();
        let report = button("support-report", "note", t("Create support report"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            let path = mandara_config::paths::expand_tilde(&format!("~/mandara-support-{}.txt", stamp()));
            let cfg = config(cx).read(cx).snapshot();
            let text = support_report(&cfg, &version);
            let r = mandara_config::write_atomic(&path, text.as_bytes());
            st(cx).note = Some(match r {
                Ok(()) => (tf("Report written to {file} (personal values redacted) — attach it to your bug report", &[("file", &path.display())]), false),
                Err(e) => (format!("{e:#}"), true),
            });
            cx.notify();
        }));
        let warning = (!sensitive.is_empty() && !redact).then(|| {
            div()
                .text_size(px(TEXT_12_PX))
                .text_color(p.warning_amber.hsla())
                .child(mandara_ui::bidi::paragraph(tf("That file includes personal values ({keys}); turn on “Redact personal values” before sharing it.", &[("keys", &sensitive.join(", "))])))
        });
        card(p)
            .child(heading(t("Export"), t("A single TOML file you can keep, share or import on another machine. Existing files are never overwritten."), p))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_6_PX))
                    .child(toggle("export-changes", t("Only what I changed"), changes_only, |s| s.changes_only = !s.changes_only, cx))
                    .child(toggle("export-redact", t("Redact personal values"), redact, |s| s.redact = !s.redact, cx)),
            )
            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().child(path)).child(export))
            .children(warning)
            .child(div().h(px(SPACE_6_PX)))
            .child(heading(t("Import"), t("Loads an exported file; your current settings are backed up first (Maintenance → Backups)."), p))
            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().child(import_path)).child(import))
            .child(div().h(px(SPACE_6_PX)))
            .child(div().row().items_center().gap(px(SPACE_10_PX)).child(report).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Versions, compositor and your changed settings with personal values removed.")))))
            .into_any_element()
    }

    fn prof_reset(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let sections: Vec<Section> = Section::ALL.into_iter().filter(|s| registry::in_section(*s).next().is_some()).collect();
        let changed_by = changed_per_section(cfg);
        let chips: Vec<AnyElement> = sections
            .into_iter()
            .map(|section| {
                let changed = changed_by.get(&section).copied().unwrap_or(0);
                let key = format!("reset-{}", section.title());
                let armed = Self::prof_armed(&key, cx);
                div()
                    .id(SharedString::from(key.clone()))
                    .flex_1()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .cursor_pointer()
                    .bg(if armed { p.urgent_coral.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
                    .when(changed == 0, |el| el.opacity(ALPHA_50))
                    .hover(|s| s.opacity(ALPHA_88))
                    .child(icon(section.icon(), ICON_15_PX, if armed { p.urgent_coral } else { p.primary }))
                    .child(div().flex_1().min_w(px(0.0)).row().flex_col().child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(section.label())).child(div().text_size(px(TEXT_11_5_PX)).text_color(if armed { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() }).child(if armed {
                        t("Click again to reset").to_string()
                    } else if changed == 0 {
                        t("All defaults").to_string()
                    } else {
                        tf("{count} changed", &[("count", &changed)])
                    })))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        if changed == 0 {
                            return;
                        }
                        if Self::prof_arm(&key, cx) {
                            let cfg = config(cx).read(cx).snapshot();
                            let next = reset_section(&cfg, section);
                            update_config(cx, move |c| *c = next);
                            st(cx).note = Some((tf("{page} is back to its defaults — Undo is under Recent changes", &[("page", &section.label())]), false));
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let n = chips.len();
        super::grid::fit(self.page.column_w, RESET_GRID).balanced(n).render(chips)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_reset_touches_only_that_section() {
        let mut cfg = ShellConfig::default();
        cfg.bar.height = 26.0;
        cfg.compositor.input.keyboard.repeat_rate = Some(40);
        let reset = reset_section(&cfg, Section::Input);
        assert_eq!(reset.compositor.input.keyboard.repeat_rate, None);
        assert_eq!(reset.bar.height, 26.0);
    }

    #[test]
    fn change_labels() {
        let a = ShellConfig::default();
        let mut b = a.clone();
        b.bar.height = 26.0;
        assert!(changed_labels(&a, &b).iter().any(|l| l.contains("height") || l.contains("Height")));
        let mut c = a.clone();
        c.network.last_vpn = Some("x".into());
        assert_eq!(changed_labels(&a, &c), vec!["Other settings"]);
        assert!(changed_labels(&a, &a).is_empty());
    }

    #[test]
    fn report_is_redacted() {
        let mut cfg = ShellConfig::default();
        cfg.plugins.settings.entry("prayer-times".into()).or_default().insert("latitude".into(), toml::Value::Float(31.2));
        let r = support_report(&cfg, "test");
        assert!(r.contains("<redacted>") && !r.contains("31.2"));
    }

    /// The per-section counts match comparing every setting's value with
    /// a freshly built default config (what the page used to do per frame).
    #[test]
    fn reset_counts_match_a_full_comparison() {
        let mut cfg = ShellConfig::default();
        cfg.bar.height = 26.0;
        cfg.compositor.input.keyboard.repeat_rate = Some(40);
        let cfg = Arc::new(cfg);
        registry::prime(&cfg);
        let defaults = ShellConfig::default();
        let counts = changed_per_section(&cfg);
        for section in Section::ALL {
            let full = registry::in_section(section).filter(|s| registry::get(&cfg, s) != registry::get(&defaults, s)).count()
                + EXTRA_RESET_PATHS.iter().find(|(s, _)| *s == section).map(|(_, paths)| registry::changed_paths(&cfg, paths)).unwrap_or(0);
            assert_eq!(counts.get(&section).copied().unwrap_or(0), full, "{section:?}");
        }
        assert!(counts.get(&Section::Input).copied().unwrap_or(0) >= 1);
        assert!(changed_per_section(&ShellConfig::default()).is_empty(), "defaults change nothing");
    }

    #[test]
    fn resetting_themes_restores_shell_following_without_touching_other_pages() {
        let mut cfg = ShellConfig::default();
        cfg.studio = Some(Box::new(mandara_config::studio::StudioLook::Arcade.appearance()));
        cfg.bar.height = 26.0;
        let counts = changed_per_section(&cfg);
        assert!(counts.get(&Section::Themes).copied().unwrap_or_default() > 0);
        let reset = reset_section(&cfg, Section::Themes);
        assert!(reset.studio.is_none());
        assert_eq!(reset.bar, cfg.bar);
        assert_eq!(reset.compositor, cfg.compositor);
    }

    /// Two configs differ exactly where a per-setting comparison says so.
    #[test]
    fn differing_matches_get() {
        let a = ShellConfig::default();
        let mut b = a.clone();
        b.bar.height = 26.0;
        let full: Vec<&str> = registry::settings().iter().filter(|s| registry::get(&a, s) != registry::get(&b, s)).map(|s| s.path).collect();
        let fast: Vec<&str> = registry::differing(&a, &b).into_iter().map(|s| s.path).collect();
        assert_eq!(fast, full);
    }

    #[test]
    fn restored_custom_arrangements_count_as_changed_and_reset_with_their_page() {
        let cfg: ShellConfig = toml::from_str("[bar]\nleft = ['clock']\ncenter = []\nright = ['audio']\n[dock]\npinned = ['fixture-app']").unwrap();
        let counts = changed_per_section(&cfg);
        assert!(counts.get(&Section::Bars).copied().unwrap_or(0) >= 3);
        assert!(counts.get(&Section::Dock).copied().unwrap_or(0) >= 1);
        let reset = reset_section(&cfg, Section::Bars);
        let defaults = ShellConfig::default();
        assert_eq!(reset.bar, defaults.bar);
        assert_eq!(reset.dock, cfg.dock);
        assert!(!changed_per_section(&reset).contains_key(&Section::Bars));
        assert_eq!(reset_section(&cfg, Section::Dock).dock.pinned, defaults.dock.pinned);
        let with_studio = ShellConfig { studio: Some(Box::default()), ..defaults };
        let root = serde_json::to_value(&with_studio).unwrap();
        for (_, paths) in EXTRA_RESET_PATHS {
            for path in *paths {
                assert!(registry::by_path(path).is_none(), "{path} is already counted by the registry");
                assert!(root.pointer(path).is_some(), "{path} is not a real config path");
            }
        }
    }

    #[test]
    fn bars_reset_includes_all_tabs_and_preserves_unrelated_settings() {
        let cfg: ShellConfig = toml::from_str("[bar]\nheight = 26\nleft = ['clock']\n[side_bars]\nenabled = true\n[side_bars.left]\ntop = ['audio']\n[sidebar]\nwidth = 450\n[[sidebar.tabs]]\nname = 'Fixture'\ncards = ['media']\n[panels]\nmax_height = 0.7\n[edges]\ndwell_ms = 20\n[dock]\npinned = ['fixture-app']\n[plugins.settings.fixture]\nvalue = 'preserved'").unwrap();
        let reset = reset_section(&cfg, Section::Bars);
        let defaults = ShellConfig::default();
        assert_eq!(reset.bar, defaults.bar);
        assert_eq!(reset.side_bars, defaults.side_bars);
        assert_eq!(reset.sidebar, defaults.sidebar);
        assert_eq!(reset.panels, defaults.panels);
        assert_eq!(reset.edges, defaults.edges);
        assert_eq!(reset.dock, cfg.dock);
        assert_eq!(reset.plugins.settings, cfg.plugins.settings);
        assert!(!changed_per_section(&reset).contains_key(&Section::Bars));
    }

    /// What the page does per frame and per config change, timed:
    /// `cargo test --release -p mandara-studio -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing probe, run by hand in a release build"]
    fn time_per_frame_work() {
        let cfg = Arc::new(ShellConfig::default());
        registry::prime(&cfg);
        let started = Instant::now();
        let counts = changed_per_section(&cfg);
        eprintln!("reset counts: {:?} ({} sections changed)", started.elapsed(), counts.len());
        let a = ShellConfig::default();
        let mut b = a.clone();
        b.bar.height = 26.0;
        let started = Instant::now();
        let labels = changed_labels(&a, &b);
        eprintln!("changed_labels: {:?} ({} labels)", started.elapsed(), labels.len());
    }
    #[test]
    fn resetting_a_layout_keeps_removals_and_restores_each_plugin_once() {
        let mut cfg = ShellConfig::default();
        cfg.take_plugin_places("weather");
        cfg.bar.left.push("weather".into());
        mandara_plugins::lifecycle::remove(&mut cfg, "weather", "Weather").unwrap();
        cfg.bar.left.reverse();
        let remembered = cfg.plugins.removed_places.clone();
        let mut reset = reset_section(&cfg, Section::Bars);
        assert!(reset.plugin_places("weather").is_empty());
        assert_eq!(reset.plugins.removed_places, remembered);
        assert!(!changed_per_section(&reset).contains_key(&Section::Bars));
        mandara_plugins::lifecycle::restore(&mut reset, "weather", Some(mandara_plugins::Zone::Right));
        assert_eq!(reset.plugin_places("weather").len(), 1);
        assert!(reset.bar.left.iter().any(|id| id == "weather"));
        let removed = reset_section(&cfg, Section::Bars);
        assert!(removed.plugin_places("weather").is_empty());
    }

}
