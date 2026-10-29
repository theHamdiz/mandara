//! Studio → Applications, in four tabs (deep link: `mandara studio
//! applications --tab <installed|defaults|startup|launching>`; the last
//! tab is remembered in Studio's UI state):
//!
//! * **Installed** — every app with a desktop entry: icon, name, where it
//!   came from (package manager, Flatpak, Snap, AppImage, Nix, local), and
//!   open / hide from the launcher / rename / uninstall. Uninstalling goes
//!   through `mandara_services::app_uninstall`: the app's own package
//!   manager (system packages via pkexec), a confirmation showing the exact
//!   command and what else goes, and an optional, unchecked-by-default
//!   cleanup of leftovers that only ever moves files to the Trash.
//! * **Defaults** — default apps per kind of file or link (only Mandara's
//!   keys in `~/.config/mimeapps.list` change; the first edit keeps a
//!   `mimeapps.list.mandara-backup`), and the terminal / file manager /
//!   browser Mandara itself opens.
//! * **Startup** — autostart entries (`Hidden=` in `~/.config/autostart`;
//!   a system entry gets a user override, the system file is never
//!   touched; only entries added here can be removed) and Mandara's runner.
//! * **Launching** — launch prefix, systemd scopes, the terminal for
//!   terminal apps, and window → app matches for icons.

use mandara_ui::bidi::Bidi as _;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred, div, point, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::apps::{self, APP_CATEGORIES, AppCategory, AutostartEntry, AutostartNew, AutostartOrigin, DesktopEntry};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_services::app_uninstall::{self as un, Choices, InstalledApp, Plan, Source, SystemRunner};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title, heading, tip};
use super::form::{self, note, switch};
use crate::registry::system_settings::{AUTOSTART, PREFERRED};
use crate::registry::workflow_settings::LAUNCHING;
use crate::registry::{self, Section, Setting};
use crate::tokens::*;

/// App icon size in rows and cards.
const APP_ICON_PX: f32 = 28.0;
/// App icon size on Installed cards.
const CARD_ICON_PX: f32 = 40.0;
/// Category cards.
const CATEGORY_GRID: CardGrid = CardGrid { min_w: 240.0, max_w: 420.0, gap: SPACE_12_PX };
/// Installed app cards.
const INSTALLED_GRID: CardGrid = CardGrid { min_w: 280.0, max_w: 460.0, gap: SPACE_12_PX };
/// Installed apps shown before "Show all".
const INSTALLED_PAGE: usize = 48;
/// Candidate apps listed per category / search results.
const CANDIDATES_MAX: usize = 24;
const SEARCH_RESULTS: usize = 8;
/// Re-read mimeapps.list / autostart, and the installed apps, this often.
const RELOAD: std::time::Duration = std::time::Duration::from_secs(5);
const APPS_RELOAD: std::time::Duration = std::time::Duration::from_secs(60);
/// Launch options that belong to the launcher, not to starting apps.
const LAUNCHER_ONLY: &[&str] = &["/launcher/launch/auto_paste"];
/// Segmented tab control (matches the Plugins page).
const SEGMENT_H_PX: f32 = 34.0;
const SEGMENT_PAD_PX: f32 = 4.0;
const SEGMENT_PAD_X_PX: f32 = 16.0;
const COUNT_PILL_H_PX: f32 = 18.0;
/// How long Studio follows the shell's install jobs.
const INSTALL_WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
const INSTALL_WATCH_POLLS: usize = 900;
/// Uninstall dialog.
const MODAL_W_PX: f32 = 640.0;
const MODAL_MARGIN_PX: f32 = 28.0;
const SCRIM_DARK: f32 = 0.55;
const SCRIM_LIGHT: f32 = 0.32;
const DIALOG_SHADOW_Y_PX: f32 = 18.0;
const DIALOG_SHADOW_BLUR_PX: f32 = 48.0;
const CHECKBOX_PX: f32 = 18.0;
const CHECK_ICON_PX: f32 = 12.0;
/// The drop card's wash (CSS degrees).
const GRADIENT_DEG: f32 = 120.0;

/// The Applications page's tabs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AppsTab {
    #[default]
    Installed,
    Defaults,
    Startup,
    Launching,
}

impl AppsTab {
    pub const ALL: [AppsTab; 4] = [AppsTab::Installed, AppsTab::Defaults, AppsTab::Startup, AppsTab::Launching];

    /// Deep-link name (`mandara studio applications --tab startup`).
    pub fn slug(self) -> &'static str {
        match self {
            AppsTab::Installed => "installed",
            AppsTab::Defaults => "defaults",
            AppsTab::Startup => "startup",
            AppsTab::Launching => "launching",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.slug() == s).or(match s.as_str() {
            "apps" | "uninstall" | "all" => Some(AppsTab::Installed),
            "default" | "default-apps" | "mime" => Some(AppsTab::Defaults),
            "autostart" | "login" | "start" => Some(AppsTab::Startup),
            "launch" | "launch-options" | "wrapper" => Some(AppsTab::Launching),
            _ => None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            AppsTab::Installed => t("Installed"),
            AppsTab::Defaults => t("Defaults"),
            AppsTab::Startup => t("Startup"),
            AppsTab::Launching => t("Launching"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            AppsTab::Installed => "grid",
            AppsTab::Defaults => "check",
            AppsTab::Startup => "zap",
            AppsTab::Launching => "sliders",
        }
    }
}

/// The uninstall confirmation.
struct Uninstall {
    app: InstalledApp,
    /// `None` while the owner is being looked up.
    plan: Option<Result<Plan, String>>,
    /// Leftovers chosen for the Trash (none by default).
    purge: BTreeSet<PathBuf>,
    /// Yes to the plan's cautions.
    confirmed: bool,
    running: bool,
    error: Option<String>,
}

/// `(category id, default desktop id)` per category.
type CategoryDefaults = Rc<[(String, Option<String>)]>;

#[derive(Default)]
struct AppsStudio {
    tab: Option<AppsTab>,
    entries: Option<Rc<[DesktopEntry]>>,
    /// Current default per category id.
    defaults: Option<CategoryDefaults>,
    autostart: Option<Rc<[AutostartEntry]>>,
    /// Installed tab: apps with their source (loaded off the UI thread).
    installed: Option<Rc<[InstalledApp]>>,
    installed_loading: bool,
    installed_at: Option<std::time::Instant>,
    /// Installed tab filters.
    source_filter: Option<Source>,
    show_helpers: bool,
    show_all: bool,
    /// App (stem) whose launcher name is being edited.
    renaming: Option<String>,
    uninstall: Option<Uninstall>,
    /// Category whose app picker is open.
    picking: Option<&'static str>,
    /// Show entries limited to other desktops (`OnlyShowIn=KDE;`).
    other_desktops: bool,
    note: Option<(String, bool)>,
    /// systemd's xdg-autostart runs the entries this session (probed once).
    systemd_autostart: Option<bool>,
    systemd_probed: bool,
    loaded_at: Option<std::time::Instant>,
    entries_at: Option<std::time::Instant>,
    /// "Installed by Mandara": the installer's history instead of the grid.
    by_mandara: bool,
    /// The installer's registry with each app's size (read when shown).
    history: Option<History>,
}
impl Global for AppsStudio {}

/// One app the installer put in place.
type InstallRecord = mandara_services::app_install::registry::Entry;
/// The installer's records with each app's size on disk.
type History = Rc<[(InstallRecord, Option<u64>)]>;

/// Formats the installer's supported formats line.
const SUPPORTED_FORMATS: &str = "AppImage · Flatpak · .tar.gz / .zip · .deb · .rpm · pacman · apk · xbps";

/// `YYYY-MM-DD` of a Unix time (UTC; civil-from-days).
pub fn ymd(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Opens a folder (or a file's folder) off the UI thread.
fn reveal(path: &std::path::Path) {
    let dir = if path.is_dir() { path.to_path_buf() } else { path.parent().map(std::path::Path::to_path_buf).unwrap_or_default() };
    let uri = format!("file://{}", un::percent_encode(&dir));
    std::thread::spawn(move || {
        if let Err(e) = mandara_services::session::open_uri(&uri) {
            tracing::warn!("apps: opening {uri}: {e}");
        }
    });
}

fn st(cx: &mut gpui::App) -> &mut AppsStudio {
    cx.default_global::<AppsStudio>()
}

/// An app's icon from the icon theme, or a monogram glyph.
fn app_icon(icon_name: &str, p: &MandaraPalette) -> AnyElement {
    app_icon_sized(icon_name, APP_ICON_PX, p)
}

fn app_icon_sized(icon_name: &str, size: f32, p: &MandaraPalette) -> AnyElement {
    match mandara_ui::icon_theme::resolve(icon_name, None, false).filter(|_| !icon_name.is_empty()) {
        Some(found) if found.symbolic => gpui::svg().external_path(found.path.to_string_lossy().to_string()).size(px(size)).flex_none().text_color(p.on_surface.hsla()).into_any_element(),
        Some(found) => div().size(px(size)).flex_none().child(gpui::img(found.path).size_full()).into_any_element(),
        None => div().size(px(size)).flex_none().rounded_full().bg(p.surface_container_high.hsla()).row().items_center().justify_center().child(icon("package", ICON_14_PX, p.on_surface_variant)).into_any_element(),
    }
}

/// Badge tint per source family.
fn source_color(source: Source, p: &MandaraPalette) -> Color {
    match source {
        Source::Flatpak | Source::Nix => p.secondary,
        Source::Snap => p.tertiary,
        Source::AppImage => p.warning_amber,
        Source::Local => p.success_emerald,
        Source::Mandara => p.primary,
        Source::Unknown => p.on_surface_variant,
        _ => p.primary,
    }
}

/// argv as one copyable line (words with spaces quoted).
fn shell_line(argv: &[String]) -> String {
    argv.iter().map(|w| if w.is_empty() || w.contains([' ', '\'', '"', '$']) { format!("'{}'", w.replace('\'', "'\\''")) } else { w.clone() }).collect::<Vec<_>>().join(" ")
}

/// `/home/me/.config/x` → `~/.config/x`.
pub(super) fn tilde(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// A round icon button with a tooltip (coral on hover when `danger`).
fn icon_button(id: SharedString, glyph: &'static str, tooltip: &'static str, danger: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let hover = if danger { p.urgent_coral.with_alpha(ALPHA_14) } else { p.on_surface.with_alpha(ALPHA_8) };
    div()
        .id(id)
        .size(px(DIM_30_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(icon(glyph, ICON_14_PX, if danger { p.urgent_coral } else { p.on_surface_variant }))
        .tooltip(tip(tooltip, p))
}

/// A checkbox square.
fn checkbox(on: bool, p: &MandaraPalette) -> AnyElement {
    div()
        .size(px(CHECKBOX_PX))
        .flex_none()
        .rounded(px(RADIUS_5_PX))
        .row()
        .items_center()
        .justify_center()
        .when(on, |el| el.bg(p.primary.hsla()).child(icon("check", CHECK_ICON_PX, p.on_primary)))
        .when(!on, |el| el.border_2().border_color(p.outline.with_alpha(ALPHA_60).hsla()))
        .into_any_element()
}

impl StudioView {
    fn apps_load(cx: &mut Context<Self>) {
        let s = st(cx);
        // Files can change behind Studio's back: re-read now and then.
        if s.loaded_at.is_none_or(|t| t.elapsed() > RELOAD) {
            s.defaults = None;
            s.autostart = None;
            s.loaded_at = Some(std::time::Instant::now());
        }
        if s.entries_at.is_none_or(|t| t.elapsed() > APPS_RELOAD) {
            s.entries = None;
            s.entries_at = Some(std::time::Instant::now());
        }
        if s.entries.is_none() {
            s.entries = Some(apps::desktop_entries().into());
        }
        if s.defaults.is_none() {
            let file = apps::mimeapps_file();
            s.defaults = Some(APP_CATEGORIES.iter().map(|c| (c.id.to_string(), apps::default_for(&file, c.mime_types[0]))).collect::<Vec<_>>().into());
        }
        if s.autostart.is_none() {
            s.autostart = Some(apps::autostart_entries().into());
        }
        if !s.systemd_probed {
            s.systemd_probed = true;
            cx.spawn(async move |this, cx| {
                let active = cx.background_executor().spawn(async { mandara_ui::plugins::system_admin::systemd_runs_autostart() }).await;
                let _ = this.update(cx, |_, cx| {
                    st(cx).systemd_autostart = Some(active);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The installed apps (and their sources), loaded in the background.
    fn apps_load_installed(cx: &mut Context<Self>) {
        let s = st(cx);
        if s.installed_at.is_some_and(|t| t.elapsed() > APPS_RELOAD) && !s.installed_loading {
            s.installed_at = None;
        }
        if s.installed_loading || s.installed_at.is_some() {
            return;
        }
        s.installed_loading = true;
        form::job(
            cx,
            || Ok(un::installed(&un::Env::current(), &SystemRunner)),
            |_, result: Result<Vec<InstalledApp>, String>, cx| {
                let s = st(cx);
                s.installed_loading = false;
                s.installed_at = Some(std::time::Instant::now());
                s.installed = Some(result.unwrap_or_default().into());
            },
        );
    }

    /// The current tab: picked this session, else remembered.
    fn apps_tab(&self, cx: &mut Context<Self>) -> AppsTab {
        let remembered = AppsTab::parse(&self.ui.apps_tab).unwrap_or_default();
        *st(cx).tab.get_or_insert(remembered)
    }

    fn set_apps_tab(&mut self, tab: AppsTab, cx: &mut Context<Self>) {
        st(cx).tab = Some(tab);
        if self.ui.apps_tab != tab.slug() {
            self.ui.apps_tab = tab.slug().to_string();
            crate::ui_state::save(&self.ui);
        }
        cx.notify();
    }

    /// Opens Applications on `tab` (deep links).
    pub fn show_apps_tab(&mut self, tab: AppsTab, cx: &mut Context<Self>) {
        self.show(Section::Applications, cx);
        self.set_apps_tab(tab, cx);
    }

    pub(super) fn apps_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        Self::apps_load(cx);
        let tab = self.apps_tab(cx);
        if tab == AppsTab::Installed {
            Self::apps_load_installed(cx);
        }
        let note_el = st(cx).note.clone().map(|(text, error)| note(&text, error, p));
        let bar = self.apps_tab_bar(tab, p, cx);
        let body = match tab {
            AppsTab::Installed => self.apps_installed(cfg, p, cx),
            AppsTab::Defaults => self.apps_defaults_tab(cfg, p, cx),
            AppsTab::Startup => self.apps_startup_tab(cfg, p, cx),
            AppsTab::Launching => self.apps_launching_tab(cfg, p, cx),
        };
        let modal = self.apps_uninstall_modal(p, window, cx);
        div().row().flex_col().gap(px(SPACE_22_PX)).child(bar).children(note_el).child(body).children(modal).into_any_element()
    }

    /// The segmented control at the top of the page.
    fn apps_tab_bar(&mut self, current: AppsTab, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (installed, startup) = {
            let s = st(cx);
            (s.installed.as_ref().map(|l| l.iter().filter(|a| !a.no_display).count()), s.autostart.as_ref().map(|l| l.iter().filter(|e| e.enabled).count()))
        };
        let segments = AppsTab::ALL.into_iter().map(|tab| {
            let active = tab == current;
            let count = match tab {
                AppsTab::Installed => installed,
                AppsTab::Startup => startup,
                _ => None,
            };
            let tint = if active { p.on_primary } else { p.on_surface_variant };
            div()
                .id(SharedString::from(format!("apps-tab-{}", tab.slug())))
                .h(px(SEGMENT_H_PX))
                .px(px(SEGMENT_PAD_X_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .cursor_pointer()
                .text_size(px(TEXT_13_PX))
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow)))
                .when(!active, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
                .child(icon(tab.icon(), ICON_14_PX, tint))
                .child(tab.label())
                .when_some(count, |el, n| {
                    el.child(
                        div()
                            .min_w(px(COUNT_PILL_H_PX))
                            .h(px(COUNT_PILL_H_PX))
                            .px(px(SPACE_6_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .bg(tint.with_alpha(ALPHA_18).hsla())
                            .text_size(px(TEXT_11_PX))
                            .font_features(mandara_ui::fonts::tabular_nums())
                            .child(n.to_string()),
                    )
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_apps_tab(tab, cx)))
        });
        div()
            .row()
            .child(div().p(px(SEGMENT_PAD_PX)).rounded_full().row().flex_wrap().gap(px(SEGMENT_PAD_PX)).bg(p.surface_container.hsla()).shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow)).children(segments))
            .into_any_element()
    }

    /// A tab's one-line introduction.
    fn apps_intro(text: &str, p: &MandaraPalette) -> AnyElement {
        div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(text.to_string())).into_any_element()
    }

    // ── Installed ────────────────────────────────────────────────────────

    fn apps_installed(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (list, loading, filter, helpers, show_all) = {
            let s = st(cx);
            (s.installed.clone(), s.installed_loading, s.source_filter, s.show_helpers, s.show_all)
        };
        let search = form::input("apps-installed-search", t("Search apps by name or id"), false, cx);
        let q = form::value("apps-installed-search", cx).to_lowercase();
        let hidden = &cfg.launcher.hidden;
        let is_hidden = |a: &InstalledApp| hidden.iter().any(|h| h.eq_ignore_ascii_case(a.app.stem()));
        let all = list.clone().unwrap_or_default();
        let visible: Vec<&InstalledApp> = all.iter().filter(|a| helpers || (!a.no_display && !is_hidden(a))).collect();
        // Source chips with counts, in a stable order.
        let mut sources: Vec<(Source, usize)> = Vec::new();
        for a in &visible {
            match sources.iter_mut().find(|(s, _)| *s == a.source) {
                Some((_, n)) => *n += 1,
                None => sources.push((a.source, 1)),
            }
        }
        sources.sort_by_key(|(s, _)| s.label());
        let display_name = |a: &InstalledApp| cfg.launcher.overrides.get(a.app.stem()).map(|o| o.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| a.app.name.clone());
        let matches: Vec<&InstalledApp> = visible
            .iter()
            .copied()
            .filter(|a| filter.is_none_or(|f| a.source == f))
            .filter(|a| q.is_empty() || display_name(a).to_lowercase().contains(&q) || a.app.id.to_lowercase().contains(&q) || a.comment.to_lowercase().contains(&q))
            .collect();
        let total = matches.len();
        let shown_n = if show_all || !q.is_empty() { total } else { total.min(INSTALLED_PAGE) };

        let filter_chip = |id: String, label: String, n: usize, active: bool, pick: Option<Source>, cx: &mut Context<Self>| {
            chip(SharedString::from(id), label, active, p)
                .child(div().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).opacity(ALPHA_75).child(n.to_string()))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let s = st(cx);
                    s.source_filter = pick;
                    s.show_all = false;
                    s.by_mandara = false;
                    cx.notify();
                }))
        };
        let by_mandara = st(cx).by_mandara;
        let mut chips = vec![filter_chip("apps-src-all".into(), t("All").into(), visible.len(), filter.is_none() && !by_mandara, None, cx).into_any_element()];
        chips.push(
            chip("apps-by-mandara", t("Installed by Mandara"), by_mandara, p)
                .child(icon("download", ICON_12_PX, if by_mandara { p.on_primary } else { p.primary }))
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    let s = st(cx);
                    s.by_mandara = !s.by_mandara;
                    if s.by_mandara {
                        s.history = None;
                        Self::apps_load_history(cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        for (source, n) in &sources {
            chips.push(filter_chip(format!("apps-src-{source:?}"), source.label().to_string(), *n, filter == Some(*source), Some(*source), cx).into_any_element());
        }
        chips.push(
            chip("apps-helpers", t("Show hidden & helper entries"), helpers, p)
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    let s = st(cx);
                    s.show_helpers = !s.show_helpers;
                    cx.notify();
                }))
                .into_any_element(),
        );
        let toolbar = card(p)
            .gap(px(SPACE_12_PX))
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(icon("search", ICON_16_PX, p.on_surface_variant))
                    .child(div().flex_1().min_w(px(0.0)).child(search))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(TEXT_12_PX))
                            .text_color(p.on_surface_variant.hsla())
                            .font_features(mandara_ui::fonts::tabular_nums())
                            .child(if total == 1 { t("1 app").to_string() } else { tf("{count} apps", &[("count", &total)]) }),
                    )
                    .child(button("apps-install", "download", t("Install app…"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::apps_pick_and_install(cx)))),
            )
            .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(chips));

        let body = if by_mandara {
            self.apps_history(&all, p, cx)
        } else if list.is_none() || (loading && all.is_empty()) {
            self.apps_empty("refresh", t("Looking for installed apps…"), t("Reading desktop entries and where each one came from."), p)
        } else if matches.is_empty() {
            if q.is_empty() {
                self.apps_empty("package", t("No apps here"), t("Try another source, or show hidden and helper entries."), p)
            } else {
                self.apps_empty("search", tf("No app matches “{query}”", &[("query", &q)]), t("Search looks at names, ids and descriptions."), p)
            }
        } else {
            let cards: Vec<AnyElement> = matches.iter().take(shown_n).map(|a| self.app_card(a, cfg, p, cx)).collect();
            let more = (shown_n < total).then(|| {
                div().row().justify_center().child(button("apps-show-all", "grid", tf("Show all {count} apps", &[("count", &total)]), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    st(cx).show_all = true;
                    cx.notify();
                })))
            });
            div().row().flex_col().gap(px(SPACE_14_PX)).child(super::grid::fit(self.page.column_w, INSTALLED_GRID).render(cards)).children(more).into_any_element()
        };
        let content = div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(Self::apps_intro(t("Every app with a launcher entry, and where it came from. Uninstalling uses that app's own package manager and always shows what will happen first. Drop an AppImage, Flatpak, archive or package here to install it."), p))
            .child(self.apps_drop_hero(p, cx))
            .child(toolbar)
            .child(body)
            .into_any_element();
        mandara_ui::installer_overlay::drop_zone("apps-install-drop", content, RADIUS_12_PX, p, Rc::new(|paths, _, cx| Self::apps_install_paths(paths, cx))).into_any_element()
    }

    /// The inviting drop target at the top of the Installed tab.
    fn apps_drop_hero(&self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        card(p)
            .row()
            .items_center()
            .gap(px(SPACE_16_PX))
            .py(px(SPACE_18_PX))
            .bg(mandara_core::oklab_gradient(GRADIENT_DEG, p.primary.with_alpha(ALPHA_12), p.tertiary.with_alpha(ALPHA_5)))
            .border_1()
            .border_color(p.primary.with_alpha(ALPHA_25).hsla())
            .child(div().size(px(DIM_56_PX)).flex_none().rounded_full().row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_15).hsla()).child(icon("download", ICON_26_PX, p.primary)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Drop an app here, or choose a file")))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Mandara shows what it is and what will happen before anything is installed."))))
                    .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.with_alpha(ALPHA_75).hsla()).child(SUPPORTED_FORMATS)),
            )
            .child(button("apps-install-hero", "folder", t("Choose a file…"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::apps_pick_and_install(cx))))
            .into_any_element()
    }

    /// Reads the installer's registry (and each app's size) off the UI thread.
    fn apps_load_history(cx: &mut Context<Self>) {
        form::job(
            cx,
            || {
                let data = dirs::data_dir().unwrap_or_default();
                Ok(mandara_services::app_install::registry::installed(&data)
                    .into_iter()
                    .map(|e| {
                        let size = e.dir.as_ref().and_then(|d| std::fs::read_dir(d).ok()).map(|r| r.flatten().filter_map(|f| f.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum());
                        (e, size)
                    })
                    .collect::<Vec<_>>())
            },
            |_, result: Result<Vec<(InstallRecord, Option<u64>)>, String>, cx| {
                let mut list = result.unwrap_or_default();
                list.sort_by_key(|e| std::cmp::Reverse(e.0.installed_at));
                st(cx).history = Some(list.into());
            },
        );
    }

    /// "Installed by Mandara": newest first, with Reveal / Uninstall.
    fn apps_history(&mut self, all: &[InstalledApp], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let Some(history) = st(cx).history.clone() else {
            return self.apps_empty("refresh", t("Reading what Mandara installed…"), t("From the installer's own records."), p);
        };
        if history.is_empty() {
            return self.apps_empty("download", t("Nothing installed by Mandara yet"), t("Drop an app on the dock or here to install it."), p);
        }
        let rows: Vec<AnyElement> = history
            .iter()
            .enumerate()
            .map(|(n, (e, size))| {
                let desktop = e.desktop_id.clone().unwrap_or_default();
                let app = all.iter().find(|a| a.app.stem() == desktop || a.app.id == format!("{desktop}.desktop")).cloned();
                let kind = e.family.map(|f| f.label().to_string()).unwrap_or_default();
                let mut detail = vec![tf("From {file}", &[("file", &e.source_file)]), ymd(e.installed_at)];
                if let Some(b) = size {
                    detail.push(mandara_services::app_install::human_bytes(*b));
                }
                let place = e.dir.clone().or_else(|| e.desktop_file.clone());
                div()
                    .id(("apps-history", n))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .px(px(SPACE_14_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_12_PX))
                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_5).hsla()))
                    .child(app_icon_sized(app.as_ref().map(|a| a.app.icon.as_str()).unwrap_or("package"), CARD_ICON_PX, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(
                                div()
                                    .row()
                                    .items_center()
                                    .gap(px(SPACE_8_PX))
                                    .child(div().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).child(e.name.clone()))
                                    .when(!e.version.is_empty(), |el| el.child(badge(e.version.clone(), p.secondary)))
                                    .when(!kind.is_empty(), |el| el.child(badge(kind, p.primary))),
                            )
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).truncate().child(detail.join(" · "))),
                    )
                    .when_some(place, |el, place| el.child(icon_button(SharedString::from(format!("apps-history-reveal-{n}")), "folder", t("Show in Files"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, _| reveal(&place)))))
                    .when_some(app, |el, app| el.child(icon_button(SharedString::from(format!("apps-history-uninstall-{n}")), "trash", t("Uninstall…"), true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apps_open_uninstall(app.clone(), cx)))))
                    .into_any_element()
            })
            .collect();
        card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX)).children(rows).into_any_element()
    }

    /// "Install app…": the portal's file picker, filtered to installable
    /// formats, then the shell's installer (its card confirms).
    fn apps_pick_and_install(cx: &mut Context<Self>) {
        form::job(
            cx,
            || {
                let globs = mandara_services::app_install::picker_globs();
                let start = dirs::download_dir().filter(|d| d.is_dir());
                mandara_services::file_chooser::open_files(t("Install apps"), t("Apps and packages"), mandara_services::app_install::MIME_TYPES, &globs, start)
            },
            |_, result: Result<Vec<PathBuf>, String>, cx| match result {
                Ok(paths) if !paths.is_empty() => Self::apps_install_paths(paths, cx),
                Ok(_) => {}
                Err(e) => st(cx).note = Some((tf("Couldn't open the file picker: {error}", &[("error", &e)]), true)),
            },
        );
    }

    /// Hands files to the shell's installer (`apps install`), then watches
    /// its jobs to refresh the list when they finish.
    fn apps_install_paths(paths: Vec<PathBuf>, cx: &mut gpui::App) {
        let mut args = vec!["apps".to_string(), "install".to_string()];
        args.extend(paths.iter().map(|p| p.to_string_lossy().to_string()));
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        let Some(task) = crate::perf::query(&words, cx) else {
            st(cx).note = Some((t("Studio isn't connected to the shell").to_string(), true));
            cx.refresh_windows();
            return;
        };
        cx.spawn(async move |cx| {
            let result = task.await;
            let _ = cx.update(|cx| {
                st(cx).note = match result {
                    Ok(_) => Some((t("Confirm the install on screen").to_string(), false)),
                    Err(e) => Some((e, true)),
                };
                cx.refresh_windows();
            });
            // Refresh the list once the shell's jobs are done.
            for _ in 0..INSTALL_WATCH_POLLS {
                cx.background_executor().timer(INSTALL_WATCH_EVERY).await;
                let Some(jobs) = cx.update(|cx| crate::perf::query(&["apps", "jobs"], cx)) else { break };
                let running = jobs.await.ok().and_then(|v| v.as_array().cloned()).is_some_and(|a| a.iter().any(|j| !matches!(j["phase"].as_str(), Some("done" | "failed" | "cancelled"))));
                if !running {
                    break;
                }
            }
            let _ = cx.update(|cx| {
                let s = st(cx);
                s.installed_at = None;
                s.entries = None;
                cx.refresh_windows();
            });
        })
        .detach();
    }

    /// A centered empty / loading state.
    fn apps_empty(&self, glyph: &'static str, title: impl Into<SharedString>, detail: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
        card(p)
            .items_center()
            .py(px(SPACE_30_PX))
            .gap(px(SPACE_10_PX))
            .child(div().size(px(DIM_48_PX)).rounded_full().row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_12).hsla()).child(icon(glyph, ICON_22_PX, p.primary)))
            .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(detail.into()))
            .into_any_element()
    }

    /// One app on the Installed tab.
    fn app_card(&mut self, a: &InstalledApp, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let stem = a.app.stem().to_string();
        let hidden = cfg.launcher.hidden.iter().any(|h| h.eq_ignore_ascii_case(&stem));
        let renamed = cfg.launcher.overrides.get(&stem).map(|o| o.name.trim().to_string()).filter(|n| !n.is_empty());
        let name = renamed.clone().unwrap_or_else(|| a.app.name.clone());
        let renaming = st(cx).renaming.as_deref() == Some(stem.as_str());
        let busy = st(cx).uninstall.as_ref().is_some_and(|u| u.running && u.app.app.id == a.app.id);
        let key = |what: &str| SharedString::from(format!("app-{what}-{}", a.app.id));
        let color = source_color(a.source, p);
        let open = {
            let id = a.app.id.clone();
            let options = cfg.launcher.launch.clone();
            icon_button(key("open"), "play", t("Open"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let entry = st(cx).entries.as_ref().and_then(|l| l.iter().find(|e| e.id == id).cloned());
                let result = entry.ok_or_else(|| t("This app is gone").to_string()).and_then(|e| Self::apps_launch(&e, &options));
                if let Err(e) = result {
                    st(cx).note = Some((e, true));
                }
                cx.notify();
            }))
        };
        let hide = {
            let stem = stem.clone();
            icon_button(key("hide"), if hidden { "eye" } else { "eye-off" }, if hidden { t("Show in the launcher") } else { t("Hide from the launcher") }, false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let stem = stem.clone();
                mandara_ui::state::update_config(cx, move |c| match c.launcher.hidden.iter().position(|h| h.eq_ignore_ascii_case(&stem)) {
                    Some(i) => {
                        c.launcher.hidden.remove(i);
                    }
                    None => c.launcher.hidden.push(stem),
                });
            }))
        };
        let rename = {
            let stem = stem.clone();
            icon_button(key("rename"), "pencil", t("Rename in the launcher"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let s = st(cx);
                s.renaming = if s.renaming.as_deref() == Some(stem.as_str()) { None } else { Some(stem.clone()) };
                cx.notify();
            }))
        };
        let uninstall = {
            let app = a.clone();
            icon_button(key("uninstall"), "trash", t("Uninstall…"), true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apps_open_uninstall(app.clone(), cx)))
        };
        let rename_row = renaming.then(|| {
            let stem2 = stem.clone();
            let field = self.list_field(
                format!("app-rename-{stem}"),
                renamed.as_deref().unwrap_or_default(),
                t("Name in the launcher (empty keeps the app's own)"),
                Rc::new(move |c, v| {
                    let v = v.trim().to_string();
                    let entry = c.launcher.overrides.entry(stem2.clone()).or_default();
                    entry.name = v;
                    if entry.name.is_empty() && entry.icon.is_empty() {
                        c.launcher.overrides.remove(&stem2);
                    }
                }),
                cx,
            );
            div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).child(field)).child(chip(key("rename-done"), t("Done"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                st(cx).renaming = None;
                cx.notify();
            })))
        });
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        div()
            .id(key("card"))
            .flex_1()
            .min_w(px(0.0))
            .rounded(px(RADIUS_18_PX))
            .bg(p.surface_container.hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .justify_between()
            .gap(px(SPACE_12_PX))
            .when(hidden || a.no_display || busy, |el| el.opacity(ALPHA_78))
            .child(
                div()
                    .row()
                    .items_start()
                    .gap(px(SPACE_12_PX))
                    .child(app_icon_sized(&a.app.icon, CARD_ICON_PX, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(name))
                            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(if a.comment.is_empty() { renamed.map(|_| a.app.name.clone()).unwrap_or_default() } else { a.comment.clone() }))
                            .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.with_alpha(ALPHA_75).hsla()).font_family(mono).truncate().child(a.app.id.clone())),
                    ),
            )
            .children(rename_row)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .child(badge(a.source.label(), color).font_weight(FontWeight::SEMIBOLD))
                    .when(hidden, |el| el.child(badge(t("Hidden"), p.on_surface_variant)))
                    .when(a.no_display, |el| el.child(badge(t("Helper"), p.on_surface_variant)))
                    .when_some(a.bytes, |el, b| el.child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(mandara_ui::widgets::system::format_bytes(b))))
                    .child(div().flex_1())
                    .when(!a.no_display, |el| el.child(open))
                    .child(hide)
                    .child(rename)
                    .child(uninstall),
            )
            .into_any_element()
    }

    /// Starts an app with the launcher's options (wrapper, terminal).
    fn apps_launch(entry: &DesktopEntry, options: &mandara_config::workflow::LaunchOptions) -> Result<(), String> {
        use std::os::unix::process::CommandExt as _;
        let argv = mandara_ui::apps::launch_argv(&entry.exec, entry.terminal, options);
        let (program, args) = argv.split_first().ok_or_else(|| t("This app has no command to run").to_string())?;
        let child = std::process::Command::new(program)
            .args(args)
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| tf("Could not run {program}: {error}", &[("program", program), ("error", &e)]))?;
        mandara_services::reaper::adopt(child);
        Ok(())
    }

    // ── Uninstall ────────────────────────────────────────────────────────

    /// Opens the confirmation and looks up the owner in the background.
    fn apps_open_uninstall(&mut self, app: InstalledApp, cx: &mut Context<Self>) {
        let target = app.app.clone();
        st(cx).uninstall = Some(Uninstall { app, plan: None, purge: BTreeSet::new(), confirmed: false, running: false, error: None });
        let apps_cfg = mandara_ui::state::config(cx).read(cx).get().apps.clone();
        form::job(
            cx,
            move || Ok(un::plan(&target, &un::env_for(&apps_cfg), &SystemRunner)),
            |_, result: Result<Plan, String>, cx| {
                if let Some(u) = st(cx).uninstall.as_mut() {
                    u.plan = Some(result);
                }
            },
        );
        cx.notify();
    }

    fn apps_run_uninstall(&mut self, cx: &mut Context<Self>) {
        let Some(u) = st(cx).uninstall.as_mut() else { return };
        let Some(Ok(plan)) = u.plan.clone() else { return };
        if u.running {
            return;
        }
        u.running = true;
        u.error = None;
        let choices = Choices { purge: u.purge.iter().cloned().collect(), confirm_cautions: u.confirmed, dry_run: false };
        let name = u.app.app.name.clone();
        let apps_cfg = mandara_ui::state::config(cx).read(cx).get().apps.clone();
        form::job(
            cx,
            move || un::execute(&plan, &choices, &un::env_for(&apps_cfg), &SystemRunner),
            move |_, result, cx| {
                let s = st(cx);
                match result {
                    Ok(out) => {
                        let moved = if out.dry_run { out.would_trash.len() } else { out.trashed.len() };
                        let text = match (out.dry_run, moved) {
                            (true, n) => tf("Dry run: nothing changed. {app} would be removed and {count} items moved to the Trash.", &[("app", &name), ("count", &n)]),
                            (false, 0) => tf("{app} was uninstalled", &[("app", &name)]),
                            (false, n) => tf("{app} was uninstalled; {count} items moved to the Trash", &[("app", &name), ("count", &n)]),
                        };
                        let text = if out.errors.is_empty() { text } else { format!("{text} ({})", out.errors.join("; ")) };
                        s.note = Some((text, !out.errors.is_empty()));
                        s.uninstall = None;
                        // Re-read the apps: the entry is gone.
                        s.installed_at = None;
                        s.entries = None;
                    }
                    Err(e) => {
                        if let Some(u) = s.uninstall.as_mut() {
                            u.running = false;
                            u.error = Some(e.clone());
                        }
                        s.note = Some((tf("Couldn't uninstall {app}: {error}", &[("app", &name), ("error", &e)]), true));
                    }
                }
            },
        );
        cx.notify();
    }

    fn apps_uninstall_modal(&mut self, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (app, plan, purge, confirmed, running, error) = {
            let u = st(cx).uninstall.as_ref()?;
            (u.app.clone(), u.plan.clone(), u.purge.clone(), u.confirmed, u.running, u.error.clone())
        };
        let viewport = window.viewport_size();
        let w = (f32::from(viewport.width) - 2.0 * MODAL_MARGIN_PX).clamp(0.0, MODAL_W_PX);
        let h = (f32::from(viewport.height) - 2.0 * MODAL_MARGIN_PX).max(0.0);
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let forced = un::forced_dry_run();
        let section = |title: String| div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(title);
        let mut body = div().row().flex_col().gap(px(SPACE_16_PX));
        // Who and what.
        body = body.child(
            div()
                .row()
                .items_center()
                .gap(px(SPACE_14_PX))
                .child(app_icon_sized(&app.app.icon, CARD_ICON_PX, p))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_3_PX))
                        .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(app.app.name.clone()))
                        .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_family(mono.clone()).truncate().child(app.app.id.clone())),
                )
                .child(match plan.as_ref() {
                    Some(Ok(plan)) => badge(plan.owner.source.label(), source_color(plan.owner.source, p)).font_weight(FontWeight::SEMIBOLD),
                    _ => badge(app.source.label(), source_color(app.source, p)),
                }),
        );
        if forced {
            body = body.child(note(t("Dry run (MANDARA_UNINSTALL_DRY_RUN): nothing will be removed or moved."), false, p));
        }
        let ready = match plan.as_ref() {
            None => {
                body = body.child(div().row().items_center().gap(px(SPACE_10_PX)).text_color(p.on_surface_variant.hsla()).child(icon("refresh", ICON_14_PX, p.on_surface_variant)).child(t("Checking who installed it…")));
                false
            }
            Some(Err(e)) => {
                body = body.child(note(e, true, p));
                false
            }
            Some(Ok(plan)) => {
                for b in &plan.blockers {
                    body = body.child(note(b, true, p));
                }
                // What happens.
                let mut what = div().row().flex_col().gap(px(SPACE_8_PX)).child(section(t("What happens").into()));
                if !plan.owner.package.is_empty() {
                    what = what.child(div().text_size(px(TEXT_12_5_PX)).child(tf("Package: {package}", &[("package", &plan.owner.package)])));
                }
                if let Some(cmd) = &plan.command {
                    what = what
                        .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if cmd.first().is_some_and(|c| c == "pkexec") { t("This command runs; you'll be asked to authorize it:") } else { t("This command runs:") }))
                        .child(div().px(px(SPACE_12_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).bg(p.surface_container_high.hsla()).font_family(mono.clone()).text_size(px(TEXT_12_PX)).child(shell_line(cmd)));
                }
                if !plan.owner.files.is_empty() {
                    what = what.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("These files move to the Trash:"))).children(
                        plan.owner.files.iter().map(|f| div().font_family(mono.clone()).text_size(px(TEXT_11_5_PX)).truncate().child(tilde(f))),
                    );
                }
                if plan.also_removes.is_empty() {
                    if plan.owner.source.is_system() && plan.preview_note.is_none() {
                        what = what.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Nothing else is removed.")));
                    }
                } else {
                    what = what
                        .child(div().text_size(px(TEXT_12_PX)).text_color(p.warning_amber.hsla()).child(tf("{manager} also removes:", &[("manager", &plan.owner.source.label())])))
                        .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(plan.also_removes.iter().map(|n| badge(n.clone(), p.warning_amber))));
                }
                if let Some(n) = &plan.preview_note {
                    what = what.child(div().text_size(px(TEXT_12_PX)).text_color(p.warning_amber.hsla()).child(n.clone()));
                }
                body = body.child(card(p).p(px(SPACE_14_PX)).bg(p.surface_container.hsla()).child(what));
                // Cautions: an extra yes.
                if !plan.cautions.is_empty() {
                    body = body.child(
                        div()
                            .id("uninstall-confirm-caution")
                            .row()
                            .items_center()
                            .gap(px(SPACE_10_PX))
                            .p(px(SPACE_12_PX))
                            .rounded(px(RADIUS_14_PX))
                            .bg(p.warning_amber.with_alpha(ALPHA_12).hsla())
                            .cursor_pointer()
                            .child(checkbox(confirmed, p))
                            .child(div().flex_1().row().flex_col().gap(px(SPACE_3_PX)).children(plan.cautions.iter().map(|c| div().text_size(px(TEXT_12_5_PX)).child(c.clone()))).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Yes, remove it anyway"))))
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                if let Some(u) = st(cx).uninstall.as_mut() {
                                    u.confirmed = !u.confirmed;
                                }
                                cx.notify();
                            })),
                    );
                }
                // Optional cleanup.
                let mut cleanup = div().row().flex_col().gap(px(SPACE_6_PX)).child(section(t("Optional: clean up leftovers").into())).child(
                    div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Settings and data this app may have left in your home folder. Only what you tick moves to the Trash; nothing is deleted, and the app's removal doesn't depend on it."))),
                );
                if plan.leftovers.is_empty() {
                    cleanup = cleanup.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No leftovers found.")));
                }
                for (i, l) in plan.leftovers.iter().enumerate() {
                    let on = purge.contains(&l.path);
                    let path = l.path.clone();
                    cleanup = cleanup.child(
                        div()
                            .id(SharedString::from(format!("uninstall-leftover-{i}")))
                            .row()
                            .items_center()
                            .gap(px(SPACE_10_PX))
                            .px(px(SPACE_10_PX))
                            .py(px(SPACE_8_PX))
                            .rounded(px(RADIUS_12_PX))
                            .cursor_pointer()
                            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_5).hsla()))
                            .child(checkbox(on, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .row()
                                    .flex_col()
                                    .child(div().font_family(mono.clone()).text_size(px(TEXT_12_PX)).truncate().child(tilde(&l.path)))
                                    .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(l.kind.label())),
                            )
                            .child(div().flex_none().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(mandara_ui::widgets::system::format_bytes(l.bytes)))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                if let Some(u) = st(cx).uninstall.as_mut()
                                    && !u.purge.remove(&path)
                                {
                                    u.purge.insert(path.clone());
                                }
                                cx.notify();
                            })),
                    );
                }
                body = body.child(cleanup);
                plan.can_uninstall() && (plan.cautions.is_empty() || confirmed)
            }
        };
        if let Some(e) = &error {
            body = body.child(note(e, true, p));
        }
        let enabled = ready && !running;
        let label = match (running, forced) {
            (true, _) => t("Uninstalling…"),
            (false, true) => t("Simulate"),
            (false, false) if purge.is_empty() => t("Uninstall"),
            (false, false) => t("Uninstall and clean up"),
        };
        let white = Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX);
        let footer = div()
            .w_full()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(div().flex_1())
            .child(button("uninstall-cancel", "close", t("Cancel"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                let s = st(cx);
                if !s.uninstall.as_ref().is_some_and(|u| u.running) {
                    s.uninstall = None;
                }
                cx.notify();
            })))
            .child(
                div()
                    .id("uninstall-confirm")
                    .h(px(DIM_34_PX))
                    .px(px(SPACE_16_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_7_PX))
                    .text_size(px(TEXT_12_5_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(p.urgent_coral.hsla())
                    .text_color(gpui::white())
                    .child(icon("trash", ICON_14_PX, white))
                    .child(label)
                    .when(!enabled, |el| el.opacity(ALPHA_45))
                    .when(enabled, |el| el.cursor_pointer().hover(|s| s.opacity(ALPHA_88)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.apps_run_uninstall(cx)))),
            );
        Some(
            deferred(
                anchored().position(point(px(0.0), px(0.0))).child(
                    div()
                        .id("uninstall-scrim")
                        .occlude()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(gpui::black().opacity(if p.is_dark { SCRIM_DARK } else { SCRIM_LIGHT }))
                        .row()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id("uninstall-dialog")
                                .w(px(w))
                                .max_h(px(h))
                                .rounded(px(RADIUS_26_PX))
                                .bg(p.surface_container_low.hsla())
                                .shadow(vec![gpui::BoxShadow { color: gpui::black().opacity(ALPHA_35), offset: point(px(0.0), px(DIALOG_SHADOW_Y_PX)), blur_radius: px(DIALOG_SHADOW_BLUR_PX), spread_radius: px(0.0), inset: false }])
                                .row()
                                .flex_col()
                                .text_color(p.on_surface.hsla())
                                .child(div().px(px(SPACE_22_PX)).pt(px(SPACE_18_PX)).pb(px(SPACE_10_PX)).text_size(px(TEXT_17_PX)).font_weight(FontWeight::SEMIBOLD).child(tf("Uninstall {app}?", &[("app", &app.app.name)])))
                                .child(div().id("uninstall-body").flex_1().min_h(px(0.0)).overflow_y_scroll().px(px(SPACE_22_PX)).pb(px(SPACE_8_PX)).child(body))
                                .child(div().row().items_center().px(px(SPACE_22_PX)).py(px(SPACE_16_PX)).child(footer)),
                        ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    // ── Defaults / Startup / Launching ───────────────────────────────────

    fn apps_defaults_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let preferred: Vec<&'static Setting> = registry::in_section(Section::Applications).filter(|s| s.group == PREFERRED).collect();
        let rows = self.rows(preferred, cfg, p, cx);
        let detected = {
            let a = &cfg.apps;
            let value = |v: Option<String>| v.unwrap_or_else(|| t("none found").into());
            tf(
                "In use: terminal {terminal} · file manager {files} · browser {browser}",
                &[("terminal", &value(a.terminal_command())), ("files", &value(a.file_manager_command())), ("browser", &a.browser_command().unwrap_or_else(|| t("your default").into()))],
            )
        };
        let defaults = self.apps_defaults(p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_22_PX))
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Opens files and links"), p)).child(Self::apps_intro(t("The app each kind of file or link opens in, for every app that asks the system (mimeapps.list)."), p)).child(defaults))
            .child(div().row().flex_col().gap(px(SPACE_8_PX)).child(rows).child(div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(detected)))
            .into_any_element()
    }

    fn apps_startup_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let runner: Vec<&'static Setting> = registry::in_section(Section::Applications).filter(|s| s.group == AUTOSTART).collect();
        let runner_rows = self.rows(runner, cfg, p, cx);
        // Who starts the list below in this session.
        let runner_note = match (st(cx).systemd_autostart, cfg.apps.run_autostart) {
            (Some(true), _) => t("Your session starts these (systemd's xdg-autostart)."),
            (Some(false), true) => t("Mandara starts the enabled entries at your next login."),
            (Some(false), false) => t("Nothing in this session starts these yet: turn on “Mandara starts these at login”, or start them from your compositor config."),
            (None, _) => "",
        };
        let autostart = self.apps_autostart(p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(runner_rows)
            .child(div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(runner_note))
            .child(autostart)
            .into_any_element()
    }

    fn apps_launching_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let launch: Vec<&'static Setting> = registry::in_section(Section::Launcher).filter(|s| s.group == LAUNCHING && !LAUNCHER_ONLY.contains(&s.path)).collect();
        let rows = self.rows(launch, cfg, p, cx);
        let aliases = self.apps_aliases(cfg, p, cx);
        let example = mandara_ui::apps::launch_argv("firefox %u", false, &cfg.launcher.launch);
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_22_PX))
            .child(Self::apps_intro(t("How Mandara starts apps from the launcher, dock and bar: a prefix such as uwsm app --, a systemd scope per app, and the terminal for terminal apps."), p))
            .child(rows)
            .child(
                card(p)
                    .gap(px(SPACE_8_PX))
                    .child(heading(t("Example"), t("Firefox would start as:"), p))
                    .child(div().px(px(SPACE_12_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).bg(p.surface_container_high.hsla()).font_family(mono).text_size(px(TEXT_12_PX)).child(shell_line(&example)))
                    .when(cfg.launcher.launch.systemd_scope, |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("…in its own systemd scope (app-mandara-firefox-<pid>.scope)."))))),
            )
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Window icons"), p)).child(aliases))
            .into_any_element()
    }

    fn apps_defaults(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (entries, defaults, picking) = {
            let s = st(cx);
            (s.entries.clone().unwrap_or_default(), s.defaults.clone().unwrap_or_default(), s.picking)
        };
        let name_of = |id: &str| entries.iter().find(|e| e.id == id).map(|e| (e.name.clone(), e.icon.clone()));
        let cards: Vec<AnyElement> = APP_CATEGORIES
            .iter()
            .map(|c| {
                let current = defaults.iter().find(|(id, _)| id == c.id).and_then(|(_, d)| d.clone());
                let shown = current.as_deref().and_then(name_of);
                let open = picking == Some(c.id);
                let id = c.id;
                card(p)
                    .flex_1()
                    .gap(px(SPACE_10_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_12_PX))
                            .child(match &shown {
                                Some((_, icon_name)) => app_icon(icon_name, p),
                                None => div().size(px(APP_ICON_PX)).flex_none().rounded_full().bg(p.primary.with_alpha(ALPHA_12).hsla()).row().items_center().justify_center().child(icon(c.icon, ICON_14_PX, p.primary)).into_any_element(),
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .row()
                                    .flex_col()
                                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(c.label))))
                                    .child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(match (&shown, &current) {
                                        (Some((name, _)), _) => name.clone(),
                                        (None, Some(id)) => id.clone(),
                                        (None, None) => t("Not set").into(),
                                    })),
                            )
                            .child(chip(SharedString::from(format!("default-{id}")), if open { t("Close") } else { t("Change") }, open, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                let s = st(cx);
                                s.picking = if s.picking == Some(id) { None } else { Some(id) };
                                cx.notify();
                            }))),
                    )
                    .when(open, |el| el.child(self.apps_candidates(c, current.as_deref(), &entries, p, cx)))
                    .into_any_element()
            })
            .collect();
        super::grid::fit(self.page.column_w, CATEGORY_GRID).render(cards)
    }

    fn apps_candidates(&mut self, category: &'static AppCategory, current: Option<&str>, entries: &[DesktopEntry], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let list = apps::candidates(entries, category);
        if list.is_empty() {
            return div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No installed app opens these.")).into_any_element();
        }
        div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_6_PX))
            .children(list.into_iter().take(CANDIDATES_MAX).map(|e| {
                let active = current == Some(e.id.as_str());
                let id = e.id.clone();
                let name = e.name.clone();
                chip(SharedString::from(format!("default-{}-{}", category.id, e.id)), e.name.clone(), active, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let result = apps::set_default(category, &id).map_err(|e| format!("{e:#}"));
                    let s = st(cx);
                    s.note = Some(match result {
                        Ok(()) => (tf("{app} now opens {kind}", &[("app", &name), ("kind", &tr(category.label).to_lowercase())]), false),
                        Err(e) => (e, true),
                    });
                    s.defaults = None;
                    s.picking = None;
                    cx.notify();
                }))
            }))
            .into_any_element()
    }

    /// `apps.id_map`: windows whose app id doesn't name their desktop file.
    fn apps_aliases(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let list = cfg.apps.id_map.clone();
        let mut col = card(p).child(heading(t("Match windows to apps"), t("For windows whose app id doesn't name their app (games, web apps): * matches anything, e.g. steam_app_* → steam."), p));
        for (i, a) in list.iter().enumerate() {
            let pattern = self.list_field(format!("alias-{i}-pattern"), &a.pattern, t("Window app id, e.g. steam_app_*"), Rc::new(move |c, v| if let Some(a) = c.apps.id_map.get_mut(i) { a.pattern = v.trim().to_string() }), cx);
            let desktop = self.list_field(format!("alias-{i}-desktop"), &a.desktop, t("App (desktop id), e.g. steam"), Rc::new(move |c, v| if let Some(a) = c.apps.id_map.get_mut(i) { a.desktop = v.trim().to_string() }), cx);
            let found = apps::find_desktop(&format!("{}.desktop", a.desktop.trim().trim_end_matches(".desktop"))).is_some();
            col = col.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().flex_1().child(pattern))
                    .child(icon("arrow", ICON_13_PX, p.on_surface_variant))
                    .child(div().flex_1().child(desktop))
                    .child(badge(if found { t("Found") } else { t("No such app") }, if found { p.success_emerald } else { p.warning_amber }))
                    .child(
                        div()
                            .id(SharedString::from(format!("alias-{i}-rm")))
                            .size(px(DIM_30_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                            .child(icon("close", ICON_13_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                mandara_ui::state::update_config(cx, move |c| {
                                    if i < c.apps.id_map.len() {
                                        c.apps.id_map.remove(i);
                                    }
                                });
                                this.list_fields.retain(|k, _| !k.starts_with("alias-"));
                            })),
                    ),
            );
        }
        col.child(div().row().child(button("alias-add", "plus", t("Add a match"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            mandara_ui::state::update_config(cx, |c| c.apps.id_map.push(Default::default()));
        }))))
        .into_any_element()
    }

    fn apps_autostart(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (list, entries) = {
            let s = st(cx);
            (s.autostart.clone().unwrap_or_default(), s.entries.clone().unwrap_or_default())
        };
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        if list.is_empty() {
            col = col.child(div().p(px(SPACE_10_PX)).child(heading(t("Nothing starts at login"), t("Add apps or commands below."), p)));
        }
        // Entries for other desktops (OnlyShowIn=KDE…) never start here:
        // folded away unless asked for.
        let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').map(|d| d.to_ascii_lowercase()).collect();
        let here = |e: &AutostartEntry| e.only_show_in.is_empty() || e.only_show_in.iter().any(|d| desktops.contains(&d.to_ascii_lowercase()));
        let elsewhere = list.iter().filter(|e| !here(e)).count();
        let show_all = st(cx).other_desktops;
        for e in list.iter().filter(|e| show_all || here(e)) {
            let icon_name = if e.icon.is_empty() { entries.iter().find(|d| d.id == e.id).map(|d| d.icon.clone()).unwrap_or_default() } else { e.icon.clone() };
            let (id, id2, enabled) = (e.id.clone(), e.id.clone(), e.enabled);
            let origin = match (e.mandara, e.origin) {
                (true, _) => badge(t("Added here"), p.primary),
                (false, AutostartOrigin::System) => badge(t("System"), p.on_surface_variant),
                (false, AutostartOrigin::User) => badge(t("Yours"), p.tertiary),
            };
            let limited = (!e.only_show_in.is_empty()).then(|| badge(tf("Only in {desktops}", &[("desktops", &e.only_show_in.join(", "))]), p.warning_amber));
            let remove = e.mandara.then(|| {
                div()
                    .id(SharedString::from(format!("autostart-rm-{}", e.id)))
                    .size(px(DIM_30_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                    .child(icon("trash", ICON_14_PX, p.on_surface_variant))
                    .tooltip(tip(t("Remove this entry"), p))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let r = apps::remove_autostart(&id2).map_err(|e| format!("{e:#}"));
                        let s = st(cx);
                        s.note = Some(match r {
                            Ok(()) => (t("Removed from login").into(), false),
                            Err(e) => (e, true),
                        });
                        s.autostart = None;
                        cx.notify();
                    }))
            });
            col = col.child(
                div()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .when(!enabled, |el| el.opacity(ALPHA_60))
                    .child(app_icon(&icon_name, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().font_weight(FontWeight::MEDIUM).child(e.name.clone())).child(origin).children(limited))
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_family(mandara_ui::fonts::fonts(cx).mono.clone()).overflow_hidden().text_ellipsis().whitespace_nowrap().child(apps::exec_program(&e.exec))),
                    )
                    .children(remove)
                    .child(div().id(SharedString::from(format!("autostart-{}", e.id))).cursor_pointer().child(switch(enabled, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let r = apps::set_autostart_enabled(&id, !enabled).map_err(|e| format!("{e:#}"));
                        let s = st(cx);
                        if let Err(e) = r {
                            s.note = Some((e, true));
                        }
                        s.autostart = None;
                        cx.notify();
                    }))),
            );
        }
        if elsewhere > 0 {
            col = col.child(div().px(px(SPACE_10_PX)).py(px(SPACE_6_PX)).child(
                chip("autostart-elsewhere", if show_all { SharedString::from(t("Hide other desktops' entries")) } else { SharedString::from(tf("{count} entries for other desktops (not started here)", &[("count", &elsewhere)])) }, show_all, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    let s = st(cx);
                    s.other_desktops = !s.other_desktops;
                    cx.notify();
                })),
            ));
        }
        // Add: an installed app (search) or a command.
        let search = form::input("autostart-search", t("Add an app: search by name"), false, cx);
        let q = form::value("autostart-search", cx).to_lowercase();
        let results = (!q.is_empty()).then(|| {
            div().row().flex_wrap().gap(px(SPACE_6_PX)).children(
                entries
                    .iter()
                    .filter(|e| !e.no_display && !e.hidden && (e.name.to_lowercase().contains(&q) || e.id.to_lowercase().contains(&q)))
                    .filter(|e| !list.iter().any(|a| a.id == e.id))
                    .take(SEARCH_RESULTS)
                    .map(|e| {
                        let id = e.id.clone();
                        chip(SharedString::from(format!("autostart-add-{}", e.id)), e.name.clone(), false, p).child(icon("plus", ICON_11_PX, p.primary)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            let r = apps::add_autostart(&AutostartNew::App(id.clone())).map_err(|e| format!("{e:#}"));
                            form::clear("autostart-search", cx);
                            let s = st(cx);
                            s.note = Some(match r {
                                Ok(_) => (t("Starts at your next login").into(), false),
                                Err(e) => (e, true),
                            });
                            s.autostart = None;
                            cx.notify();
                        }))
                    }),
            )
        });
        let name = form::input("autostart-name", t("Name"), false, cx);
        let command = form::input("autostart-cmd", t("…or a command, e.g. syncthing serve --no-browser"), false, cx);
        let add = button("autostart-add-cmd", "plus", t("Add command"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            let (name, command) = (form::value("autostart-name", cx), form::value("autostart-cmd", cx));
            if command.is_empty() {
                return;
            }
            let r = apps::add_autostart(&AutostartNew::Command { name, command }).map_err(|e| format!("{e:#}"));
            form::clear("autostart-name", cx);
            form::clear("autostart-cmd", cx);
            let s = st(cx);
            s.note = Some(match r {
                Ok(_) => (t("Starts at your next login").into(), false),
                Err(e) => (e, true),
            });
            s.autostart = None;
            cx.notify();
        }));
        col.child(
            div()
                .p(px(SPACE_10_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .child(search)
                .children(results)
                .child(super::grid::pair(self.page.column_w, DIM_220_PX, SPACE_8_PX, div().flex_1().child(name), div().flex_1().child(command)))
                .child(div().row().child(add)),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_rows_exist_and_categories_are_unique() {
        assert_eq!(registry::in_section(Section::Applications).filter(|s| s.group == PREFERRED).count(), 3);
        let mut ids: Vec<&str> = APP_CATEGORIES.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), APP_CATEGORIES.len());
    }

    #[test]
    fn tabs_parse_their_slugs_and_aliases() {
        for tab in AppsTab::ALL {
            assert_eq!(AppsTab::parse(tab.slug()), Some(tab));
        }
        assert_eq!(AppsTab::parse("Autostart"), Some(AppsTab::Startup));
        assert_eq!(AppsTab::parse("uninstall"), Some(AppsTab::Installed));
        assert_eq!(AppsTab::parse("weather"), None);
        // The Launching tab shows the launcher's launch options (not auto-paste).
        let launch = registry::in_section(Section::Launcher).filter(|s| s.group == LAUNCHING && !LAUNCHER_ONLY.contains(&s.path)).count();
        assert_eq!(launch, 3);
    }

    #[test]
    fn shell_lines_quote_only_what_needs_it() {
        let argv: Vec<String> = ["pkexec", "pacman", "-Rs", "My App"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shell_line(&argv), "pkexec pacman -Rs 'My App'");
    }
}

#[cfg(test)]
mod history_tests {
    #[test]
    fn dates_are_civil() {
        assert_eq!(super::ymd(0), "1970-01-01");
        assert_eq!(super::ymd(1_798_416_000), "2026-12-28");
        assert_eq!(super::ymd(951_782_400), "2000-02-29");
    }
}
