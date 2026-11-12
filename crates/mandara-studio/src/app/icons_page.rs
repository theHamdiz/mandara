//! Studio → Themes → Icons: the icon theme gallery (the opt-in *Icon
//! themes* plugin; while it is off, the tab shows its gate,
//! `plugin_gate.rs`).
//!
//! * **Your icons** — the icon theme in use, drawn from its own files
//!   (folder, home, files, terminal, browser, settings, text editor,
//!   music), and *Follow your desktop*, which puts back what Mandara
//!   changed. Applying sets Mandara's own icons (`appearance.icon_theme`:
//!   launcher, dock, tray) and the desktop's (GTK, `gsettings`, Qt / KDE;
//!   `mandara_services::icons::apply`).
//! * **Gallery** — the fifty catalog themes (`mandara_services::icons`),
//!   searchable and filtered by style and tone: glass cards that lift on
//!   hover, a strip of the theme's own icons once installed, author,
//!   licence, its variants, and one action that follows the theme's state:
//!   Get → Installing (progress, cancel) → Apply → Active, Remove for what
//!   Mandara installed, errors in place with Retry.
//! * **Your own icon sets** — an archive, a folder or a link
//!   (`theme_galleries::import_panel`).
//! * **Also installed** — themes from packages, your own folders or
//!   imports, with their own previews.
//!
//! The lists are read off the UI thread once per visit (and after every
//! install, import or removal); preview icons are found off the UI thread
//! and decoded at their drawn size (`icon_raster`), and leaving the tab
//! drops them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, img, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_services::icons::catalog::{CATALOG, IconTheme, IconVariant, Style};
use mandara_services::icons::{self, apply, install};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::compositor_page::{badge, button, chip, tip};
use super::plugin_gate::Gate;
use super::theme_galleries::{self as g, ImportKind, Job, PERCENT_PER_PERMILLE};
use super::themes_page::section_head;
use super::{StudioView, grid};
use crate::tokens::*;

/// Theme cards.
const ICON_GRID: CardGrid = CardGrid { min_w: 270.0, max_w: 400.0, gap: SPACE_16_PX };
/// A card's preview strip: its height and icon size.
const STRIP_H_PX: f32 = DIM_76_PX;
const STRIP_ICON_PX: f32 = DIM_30_PX;
/// Icons a card's strip shows (of [`icons::PREVIEW`]).
const STRIP_ICONS: usize = 6;
/// "Your icons": icon size.
const HERO_ICON_PX: f32 = DIM_44_PX;
/// Preview icons are looked up for this device size (sharp at 2×).
const LOOKUP_PX: u32 = 64;
const DIAGONAL_DEG: f32 = 135.0;
/// How much a theme's folder colour tints its strip.
const STRIP_TINT: f32 = 0.18;
/// Tone filter chips.
const TONES: [&str; 3] = [tk("All"), tk("Light"), tk("Dark")];

const GATE: Gate = Gate {
    will: &[
        tk("Lists fifty popular open-source icon themes and downloads the ones you pick from their projects' GitHub releases, checked against a pinned checksum"),
        tk("Installs them, or an archive, folder or link of your own, to ~/.local/share/icons"),
        tk("Sets the icon theme for Mandara's launcher, dock and tray and for GTK, Qt and KDE apps, one setting at a time, and puts back what was there when you follow your desktop again"),
    ],
    wont: &[
        tk("Change your icons until you pick a theme"),
        tk("Run any script that comes with a theme"),
        tk("Remove themes it didn't install, or touch your settings while it is off"),
    ],
};

/// Mandara's own record of one installed theme.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ours {
    theme: String,
    verified: bool,
}

/// What the tab reads from disk, off the UI thread.
#[derive(Clone, Debug, Default)]
struct Lists {
    installed: Rc<[icons::Found]>,
    ours: Rc<[Ours]>,
    /// The theme Mandara made the desktop's, if any.
    applied: Option<String>,
    /// The desktop's own icon theme (GTK / KDE settings).
    desktop: Option<String>,
}

#[derive(Default)]
struct IconStudio {
    lists: Option<Lists>,
    loading_lists: bool,
    /// Bumped by refreshes, so a late read never replaces a newer one.
    generation: u64,
    /// The variant each card shows (catalog id → index).
    picked: HashMap<&'static str, usize>,
    jobs: HashMap<&'static str, Job>,
    /// The last failure per theme name, shown on its card with Retry.
    errors: HashMap<String, String>,
    /// Preview files per theme ([`icons::PREVIEW`] order).
    previews: HashMap<String, Rc<[Option<PathBuf>]>>,
    loading: HashSet<String>,
    /// Icons drawn (file → device edges): their bitmaps go on release.
    drawn: HashMap<PathBuf, Vec<u32>>,
    style: usize,
    tone: usize,
    confirm_remove: Option<String>,
    /// The last apply's outcome (an error, or files left as you changed them).
    note: Option<(String, bool)>,
}
impl Global for IconStudio {}

fn st(cx: &mut App) -> &mut IconStudio {
    cx.default_global::<IconStudio>()
}

/// Drops what the tab read and drew (on leaving it or Themes). Running
/// installs keep going.
pub(super) fn release(cx: &mut App) {
    let s = st(cx);
    s.lists = None;
    s.previews = HashMap::new();
    s.loading.clear();
    s.confirm_remove = None;
    s.generation += 1;
    let drawn = std::mem::take(&mut s.drawn);
    for (path, edges) in drawn {
        for edge in edges {
            mandara_ui::icon_raster::forget_cached_edge(&path, edge, cx);
        }
    }
}

fn rgb(swatch: u32) -> Color {
    let [_, r, g, b] = swatch.to_be_bytes();
    Color::from_rgba8(r, g, b, u8::MAX)
}

fn set_style(i: usize, cx: &mut App) {
    st(cx).style = i;
}

fn set_tone(i: usize, cx: &mut App) {
    st(cx).tone = i;
}

/// The styles the catalog has (the chips after "All").
fn styles_present() -> Vec<Style> {
    Style::ALL.into_iter().filter(|s| CATALOG.iter().any(|t| t.style == *s)).collect()
}

/// Whether `theme` passes the style (`0` = all, else `styles[i - 1]`) and
/// tone filters.
fn passes(theme: &IconTheme, style: usize, tone: usize, styles: &[Style]) -> bool {
    let style_ok = style == 0 || styles.get(style - 1).is_some_and(|s| *s == theme.style);
    let tone_ok = match tone {
        1 => theme.tone.has_light(),
        2 => theme.tone.has_dark(),
        _ => true,
    };
    style_ok && tone_ok
}

/// The strip backdrop for a theme with folder colour `tint`.
fn strip_bg(p: &MandaraPalette, tint: Option<Color>, dark: bool) -> gpui::Background {
    // Dark variants are shown on a dark strip, light ones on a light one,
    // as they are meant to be seen.
    let base = if dark { p.surface_dim.mix(Color::from_rgba8(0, 0, 0, u8::MAX), 0.35) } else { p.surface_container_high.mix(Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX), 0.35) };
    match tint {
        Some(tint) => mandara_core::oklab_gradient(DIAGONAL_DEG, base, base.mix(tint, STRIP_TINT)),
        None => base.hsla().into(),
    }
}

/// What a card's action does for the picked variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CardState {
    Get,
    Installing(u32),
    Failed,
    Apply,
    Active,
}

impl StudioView {
    /// The lists, read off the UI thread on first use (`None` until ready).
    fn icon_lists(cx: &mut Context<Self>) -> Option<Lists> {
        if let Some(l) = &st(cx).lists {
            return Some(l.clone());
        }
        if std::mem::replace(&mut st(cx).loading_lists, true) {
            return None;
        }
        let generation = st(cx).generation;
        super::form::job(
            cx,
            || {
                let roots = mandara_config::input::icon_dirs();
                let installed = icons::installed_themes(&roots);
                let ours = install::installed(&install::Env::current()).into_iter().map(|i| Ours { theme: i.theme, verified: i.verified }).collect::<Vec<_>>();
                Ok::<_, String>((installed, ours, apply::applied(&apply::ApplyEnv::current()), mandara_ui::icon_theme::desktop_icon_theme()))
            },
            move |_, result, cx| {
                let s = st(cx);
                s.loading_lists = false;
                if s.generation != generation {
                    return;
                }
                if let Ok((installed, ours, applied, desktop)) = result {
                    s.lists = Some(Lists { installed: installed.into(), ours: ours.into(), applied, desktop });
                }
            },
        );
        None
    }

    pub(super) fn icons_refresh(cx: &mut App) {
        let s = st(cx);
        s.lists = None;
        s.loading_lists = false;
        s.generation += 1;
        // Icons can't be cached across installs: the lookup may change.
        mandara_ui::icon_theme::set_theme_override(mandara_ui::state::config(cx).read(cx).get().appearance.icon_theme.clone());
    }

    /// The preview files of installed theme `name`, found off the UI
    /// thread on first use (`None` until ready).
    fn icon_previews(name: &str, cx: &mut Context<Self>) -> Option<Rc<[Option<PathBuf>]>> {
        if let Some(p) = st(cx).previews.get(name) {
            return Some(p.clone());
        }
        if !st(cx).loading.insert(name.to_string()) {
            return None;
        }
        let theme = name.to_string();
        let task = cx.background_executor().spawn(async move { icons::preview_paths(&theme, LOOKUP_PX, &mandara_config::input::icon_dirs()) });
        let key = name.to_string();
        cx.spawn(async move |this, cx| {
            let paths = task.await;
            let _ = this.update(cx, |_, cx| {
                // Left since: the late result stays out.
                if st(cx).loading.remove(&key) {
                    st(cx).previews.insert(key, paths.into());
                    cx.notify();
                }
            });
        })
        .detach();
        None
    }

    /// `count` preview icons of `name` at `size`, drawn at their size.
    fn icon_row(&mut self, name: &str, count: usize, size: f32, labels: bool, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let Some(paths) = Self::icon_previews(name, cx) else {
            return div().row().items_center().justify_center().child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Loading preview…")));
        };
        let edge = mandara_ui::icon_raster::edge_for(size, window.scale_factor());
        let mut cells = Vec::with_capacity(count);
        for (i, (label, _)) in icons::PREVIEW.iter().enumerate().take(count) {
            let path = paths.get(i).and_then(|p| p.as_deref());
            if let Some(path) = path {
                // Recorded once per file and size (no allocation on later frames).
                let drawn = &mut st(cx).drawn;
                match drawn.get_mut(path) {
                    Some(edges) if edges.contains(&edge) => {}
                    Some(edges) => edges.push(edge),
                    None => {
                        drawn.insert(path.to_path_buf(), vec![edge]);
                    }
                }
            }
            let art = match path {
                Some(path) => img(mandara_ui::icon_raster::source(path, size)).size(px(size)).flex_none().into_any_element(),
                None => div().size(px(size * 0.5)).rounded_full().bg(p.on_surface_variant.with_alpha(ALPHA_20).hsla()).into_any_element(),
            };
            let cell = div()
                .id(SharedString::from(format!("icon-{name}-{i}")))
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_4_PX))
                .child(div().size(px(size)).row().items_center().justify_center().child(art))
                .when(labels, |el| el.child(div().text_size(px(TEXT_10_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(tr(label))))
                .when(!labels, |el| el.tooltip(tip(tr(label), p)));
            cells.push(cell);
        }
        div().row().items_center().justify_around().px(px(SPACE_8_PX)).children(cells)
    }

    /// Makes `theme` the icon theme of Mandara and the desktop (`None`:
    /// back to the desktop's own).
    fn icons_apply(&mut self, theme: Option<String>, cx: &mut Context<Self>) {
        let next = theme.clone();
        update_config(cx, move |c| c.appearance.icon_theme = next);
        st(cx).note = None;
        super::form::job(
            cx,
            move || {
                let env = apply::ApplyEnv::current();
                match &theme {
                    Some(name) => apply::apply(name, &env, &apply::GsettingsCommand),
                    None => apply::restore(&env, &apply::GsettingsCommand),
                }
            },
            |_, result, cx| {
                st(cx).note = match result {
                    Ok(r) if !r.kept.is_empty() => Some((tf("You changed {files} since Mandara did, so they stay as you set them.", &[("files", &r.kept.join(", "))]), false)),
                    Ok(_) => None,
                    Err(e) => Some((e, true)),
                };
                Self::icons_refresh(cx);
            },
        );
        cx.notify();
    }

    fn icons_install(&mut self, theme: &'static IconTheme, variant: &'static IconVariant, cx: &mut Context<Self>) {
        let name = variant.theme;
        if st(cx).jobs.contains_key(name) {
            return;
        }
        let job = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        let s = st(cx);
        s.errors.remove(name);
        s.jobs.insert(name, job);
        let task = cx.background_executor().spawn(async move { install::install(theme, variant, &install::Env::current(), &|p| progress.store((p.fraction() * g::PERMILLE) as u32, Ordering::Relaxed), &cancel) });
        Self::repaint_while(cx, move |cx| st(cx).jobs.contains_key(name));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                s.jobs.remove(name);
                match result {
                    Ok(_) => {
                        s.previews.remove(name);
                        Self::icons_refresh(cx);
                    }
                    Err(e) => {
                        s.errors.insert(name.to_string(), e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn icons_remove(&mut self, name: String, cx: &mut Context<Self>) {
        if st(cx).confirm_remove.as_deref() != Some(name.as_str()) {
            st(cx).confirm_remove = Some(name);
            cx.notify();
            return;
        }
        st(cx).confirm_remove = None;
        let theme = name.clone();
        super::form::job(
            cx,
            move || install::remove(&theme, &install::Env::current()),
            move |this, result, cx| match result {
                Ok(_) => {
                    // A removed theme can't stay the icon theme.
                    if mandara_ui::state::config(cx).read(cx).get().appearance.icon_theme.as_deref() == Some(name.as_str()) {
                        this.icons_apply(None, cx);
                    }
                    st(cx).previews.remove(&name);
                    Self::icons_refresh(cx);
                }
                Err(e) => {
                    st(cx).errors.insert(name, e);
                }
            },
        );
    }

    fn icons_act(&mut self, theme: &'static IconTheme, variant: &'static IconVariant, state: CardState, cx: &mut Context<Self>) {
        match state {
            CardState::Get | CardState::Failed => self.icons_install(theme, variant, cx),
            CardState::Installing(_) => {
                if let Some(job) = st(cx).jobs.get(variant.theme) {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            CardState::Apply => self.icons_apply(Some(variant.theme.to_string()), cx),
            CardState::Active => {}
        }
        cx.notify();
    }

    /// The Icons tab of the Themes page.
    pub(super) fn icons_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Icon themes are an opt-in plugin: off, the tab offers it and
        // nothing here reads or writes icon settings.
        if !cfg.icons_enabled() {
            return self.plugin_gate(mandara_ui::plugins::icon_themes::ID, &GATE, p, window, cx);
        }
        g::step_lifts(cfg, window, cx);
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let lists = Self::icon_lists(cx).unwrap_or_default();
        let current = cfg.appearance.icon_theme.clone();
        let follow = chip("icons-follow-desktop", t("Follow your desktop"), current.is_none(), p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.icons_apply(None, cx))).into_any_element();
        let head = section_head(
            t("Icons"),
            tf("{count} popular icon themes, downloaded on demand from their projects' GitHub releases, checked against a pinned checksum and installed to ~/.local/share/icons, plus your own from an archive, a folder or a link", &[("count", &CATALOG.len())]),
            vec![follow],
            display,
            p,
        );
        let yours = self.icons_yours(cfg, &lists, p, window, cx);
        let styles = styles_present();
        let (style, tone) = (st(cx).style, st(cx).tone);
        let style_labels: Vec<String> = std::iter::once(t("All").to_string()).chain(styles.iter().map(|s| tr(s.label()))).collect();
        let style_chips = self.filter_chips("icons-style", style_labels, style, set_style, p, cx);
        let tone_chips = self.filter_chips("icons-tone", TONES.iter().map(|l| tr(l)).collect(), tone, set_tone, p, cx);
        let search = self.gallery_search(ImportKind::Icons, t("Search icon themes…").to_string(), p, cx);
        let toolbar = div().row().flex_wrap().items_center().gap(px(SPACE_12_PX)).child(style_chips).child(div().w(px(BORDER_1_PX)).h(px(DIM_20_PX)).bg(p.outline.with_alpha(ALPHA_30).hsla())).child(tone_chips).child(div().flex_1()).child(search);
        let gallery = self.icons_gallery(cfg, &lists, &styles, p, window, cx);
        let import = self.import_panel(ImportKind::Icons, p, cx);
        let others = self.icons_others(cfg, &lists, p, window, cx);
        div().row().flex_col().gap(px(SPACE_16_PX)).child(head).child(yours).child(toolbar).child(gallery).child(import).children(others).into_any_element()
    }

    /// "Your icons": the theme in use, drawn large, and where it applies.
    fn icons_yours(&mut self, cfg: &ShellConfig, lists: &Lists, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let own = cfg.appearance.icon_theme.clone();
        let shown = own.clone().or_else(|| lists.desktop.clone());
        let title = match (&own, &shown) {
            (Some(name), _) => catalog_title(name).unwrap_or_else(|| name.clone()),
            (None, Some(name)) => tf("Your desktop's: {theme}", &[("theme", &catalog_title(name).unwrap_or_else(|| name.clone()))]),
            (None, None) => t("Your desktop's icons").to_string(),
        };
        let row = match &shown {
            Some(name) => self.icon_row(name, icons::PREVIEW.len(), HERO_ICON_PX, true, p, window, cx).into_any_element(),
            None => div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No icon theme is set; apps use their fallback icons.")).into_any_element(),
        };
        let status = match (&own, &lists.applied) {
            (Some(_), Some(_)) => t("Mandara's launcher, dock and tray use it, and GTK, Qt and KDE apps follow (apps already open may need a restart)."),
            (Some(_), None) => t("Mandara's launcher, dock and tray use it."),
            (None, _) => t("Mandara and your apps follow your desktop's icon theme."),
        };
        let note = st(cx).note.clone();
        let tint = shown.as_deref().and_then(icons::catalog::by_theme_name).map(|(t, _)| rgb(t.swatch)).unwrap_or(p.primary);
        div()
            .rounded(px(RADIUS_22_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(tint, ALPHA_25)))
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Your icons")))
                            .child(div().truncate().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(title)),
                    )
                    .when(lists.applied.is_some(), |el| el.child(badge(t("Desktop follows"), p.success_emerald))),
            )
            .child(div().py(px(SPACE_8_PX)).child(row))
            .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(status)))
            .children(note.map(|(n, error)| div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(if error { p.urgent_coral.hsla() } else { p.warning_amber.hsla() }).child(mandara_ui::bidi::paragraph(n))))
            .into_any_element()
    }

    fn icons_gallery(&mut self, cfg: &ShellConfig, lists: &Lists, styles: &[Style], p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let active = cfg.appearance.icon_theme.clone();
        let query = Self::gallery_query(ImportKind::Icons, cx);
        let (style, tone) = (st(cx).style, st(cx).tone);
        let shown: Vec<&'static IconTheme> = CATALOG
            .iter()
            .filter(|th| passes(th, style, tone, styles))
            .filter(|th| {
                // No search: nothing to compare (and nothing to format).
                if query.is_empty() {
                    return true;
                }
                let labels: Vec<String> = th.variants.iter().map(|v| tr(v.label)).collect();
                let desc = tr(th.description);
                let style_name = tr(th.style.label());
                let mut fields: Vec<&str> = vec![th.name, th.author, th.license, &desc, &style_name];
                fields.extend(labels.iter().map(String::as_str));
                fields.extend(th.variants.iter().map(|v| v.theme));
                g::matches(&query, &fields)
            })
            .collect();
        if shown.is_empty() {
            return div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("No icon theme matches.")).into_any_element();
        }
        let fit = grid::fit(self.page.column_w, ICON_GRID);
        let cards: Vec<AnyElement> = shown.into_iter().map(|theme| self.icons_card(theme, active.as_deref(), lists, p, window, cx)).collect();
        fit.render(cards)
    }

    fn icons_card(&mut self, theme: &'static IconTheme, active: Option<&str>, lists: &Lists, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let is_installed = |v: &IconVariant| lists.installed.iter().any(|f| f.name == v.theme);
        let picked = st(cx)
            .picked
            .get(theme.id)
            .copied()
            .or_else(|| theme.variants.iter().position(|v| Some(v.theme) == active))
            .or_else(|| theme.variants.iter().position(&is_installed))
            .unwrap_or(0)
            .min(theme.variants.len() - 1);
        let variant: &'static IconVariant = &theme.variants[picked];
        let job = st(cx).jobs.get(variant.theme).map(Job::permille);
        let error = st(cx).errors.get(variant.theme).cloned();
        let installed = is_installed(variant);
        let state = match (job, error.is_some(), installed, Some(variant.theme) == active) {
            (Some(pm), ..) => CardState::Installing(pm),
            (None, true, false, _) => CardState::Failed,
            (None, _, true, true) => CardState::Active,
            (None, _, true, false) => CardState::Apply,
            (None, _, false, _) => CardState::Get,
        };
        let card_active = theme.variants.iter().any(|v| Some(v.theme) == active);
        let removable = lists.ours.iter().any(|o| o.theme == variant.theme) && installed && job.is_none();
        let arming = st(cx).confirm_remove.as_deref() == Some(variant.theme);
        let tint = rgb(theme.swatch);
        let card_id = SharedString::from(format!("icons-card-{}", theme.id));
        let amount = g::lift(&card_id, cx);

        let strip_inner = if installed {
            self.icon_row(variant.theme, STRIP_ICONS, STRIP_ICON_PX, false, p, window, cx)
        } else {
            div()
                .row()
                .items_center()
                .justify_center()
                .gap(px(SPACE_10_PX))
                .child(div().size(px(DIM_40_PX)).rounded(px(RADIUS_12_PX)).bg(tint.hsla()).row().items_center().justify_center().child(icon("folder", ICON_17_PX, tint.best_on(Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX), Color::from_rgba8(0, 0, 0, u8::MAX)))))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Preview after download")))
        };
        let strip = div().h(px(STRIP_H_PX)).rounded(px(RADIUS_16_PX)).bg(if installed { strip_bg(p, Some(tint), variant.dark) } else { mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container_high, p.surface_container_high.mix(tint, STRIP_TINT)) }).overflow_hidden().relative().child(strip_inner.size_full()).when(state == CardState::Active, |el| el.child(active_badge(p)));

        // Variants as small chips (a dark dot marks variants for dark panels).
        let rtl = mandara_i18n::language().is_rtl();
        let variants = (theme.variants.len() > 1).then(|| {
            div().row().flex_wrap().gap(px(SPACE_4_PX)).children(theme.variants.iter().enumerate().map(|(i, v)| {
                let id = theme.id;
                let on = i == picked;
                div()
                    .id(SharedString::from(format!("icons-variant-{}-{}", theme.id, v.id)))
                    .h(px(DIM_22_PX))
                    .px(px(SPACE_8_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_4_PX))
                    .cursor_pointer()
                    .text_size(px(TEXT_11_PX))
                    .bg(if on { p.primary.with_alpha(ALPHA_18).hsla() } else { p.on_surface.with_alpha(ALPHA_6).hsla() })
                    .text_color(if on { p.primary.hsla() } else { p.on_surface_variant.hsla() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
                    .child(div().size(px(DIM_8_PX)).rounded_full().bg(if v.dark { Color::from_rgba8(0x22, 0x22, 0x28, u8::MAX).hsla() } else { Color::from_rgba8(0xf4, 0xf4, 0xf6, u8::MAX).hsla() }).border_1().border_color(p.outline.with_alpha(ALPHA_40).hsla()))
                    .child(tr(v.label))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        st(cx).picked.insert(id, i);
                        st(cx).confirm_remove = None;
                        cx.notify();
                    }))
            }))
        });

        let (label, glyph, filled): (String, &'static str, bool) = match state {
            CardState::Get => (tf("Get · {size}", &[("size", &mandara_ui::widgets::system::format_bytes(theme.archive.size))]), "download", false),
            CardState::Installing(pm) => (tf("Installing {percent}% · Cancel", &[("percent", &(pm / PERCENT_PER_PERMILLE))]), "close", false),
            CardState::Failed => (t("Retry").to_string(), "refresh", false),
            CardState::Apply => (t("Apply").to_string(), "check", true),
            CardState::Active => (t("Active").to_string(), "check", false),
        };
        let primary = button(SharedString::from(format!("icons-act-{}", variant.theme)), glyph, label, filled, p)
            .when(state == CardState::Active, |el| el.opacity(ALPHA_75).cursor_default())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.icons_act(theme, variant, state, cx)));
        let remove = removable.then(|| {
            let name = variant.theme.to_string();
            g::remove_button(SharedString::from(format!("icons-rm-{name}")), arming, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.icons_remove(name.clone(), cx)))
        });
        let tones = div()
            .row()
            .gap(px(SPACE_4_PX))
            .child(g::tag(tr(theme.style.label()), p.secondary))
            .when(theme.tone.has_light(), |el| el.child(g::tag(t("Light"), p.on_surface_variant)))
            .when(theme.tone.has_dark(), |el| el.child(g::tag(t("Dark"), p.on_surface_variant)));
        let n = theme.variants.len();
        let frame = g::lift_frame(&card_id).on_hover(cx.listener({
            let id = card_id.clone();
            move |_, h: &bool, _, cx| {
                g::set_hover(id.clone(), *h, cx);
                cx.notify();
            }
        }));
        let card = g::glass_card(card_id, tint, amount, p)
            .flex_1()
            .tab_index(0)
            .p(px(SPACE_8_PX))
            .border_2()
            .border_color(if card_active { p.primary.hsla() } else { gpui::transparent_black() })
            .group_hover(g::CARD_GROUP, |s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
            .focus_visible(|s| s.border_color(p.primary.with_alpha(ALPHA_80).hsla()))
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(strip)
            .child(
                div()
                    .px(px(SPACE_6_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(div().flex_1().min_w(px(0.0)).truncate().text_start().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(theme.name))
                            .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(g::stars(theme.stars))),
                    )
                    .child(div().row().items_center().gap(px(SPACE_6_PX)).child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(theme.author)).child(badge(theme.license, p.tertiary)))
                    .child(tones)
                    .children(variants)
                    .child(div().h(px(DIM_32_PX)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(tr(theme.description))))
                    .children(job.map(|pm| g::progress_bar(pm, p)))
                    .children(error.map(|e| g::error_row(e, p))),
            )
            .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().items_center().gap(px(SPACE_6_PX)).child(primary).child(div().flex_1()).children(remove).child(g::homepage_button(SharedString::from(format!("icons-home-{}", theme.id)), theme.homepage, p)))
            .on_key_down(cx.listener(move |this, e: &gpui::KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
                    return;
                }
                let step = |forward: bool| if forward ^ rtl { 1 } else { n - 1 };
                match k.key.as_str() {
                    "enter" | "space" => this.icons_act(theme, variant, state, cx),
                    "right" => {
                        st(cx).picked.insert(theme.id, (picked + step(true)) % n);
                        cx.notify();
                    }
                    "left" => {
                        st(cx).picked.insert(theme.id, (picked + step(false)) % n);
                        cx.notify();
                    }
                    "delete" | "backspace" if removable => this.icons_remove(variant.theme.to_string(), cx),
                    _ => return,
                }
                cx.stop_propagation();
            }));
        frame.child(card).into_any_element()
    }

    /// Installed themes outside the catalog (packages, your own folders,
    /// imports).
    fn icons_others(&mut self, cfg: &ShellConfig, lists: &Lists, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let query = Self::gallery_query(ImportKind::Icons, cx);
        let others: Vec<icons::Found> = lists.installed.iter().filter(|f| icons::catalog::by_theme_name(&f.name).is_none() && g::matches(&query, &[f.name.as_str(), f.title.as_str()])).cloned().collect();
        if others.is_empty() {
            return None;
        }
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let current = cfg.appearance.icon_theme.clone();
        let fit = grid::fit(self.page.column_w, ICON_GRID);
        let mut cards = Vec::with_capacity(others.len());
        for found in others {
            let name = found.name.clone();
            let active = current.as_deref() == Some(name.as_str());
            let mine = lists.ours.iter().find(|o| o.theme == name).map(|o| o.verified);
            let card_id = SharedString::from(format!("icons-other-{name}"));
            let amount = g::lift(&card_id, cx);
            let strip_inner = self.icon_row(&name, STRIP_ICONS, STRIP_ICON_PX, false, p, window, cx);
            let dark = name.to_lowercase().contains("dark");
            let strip = div().h(px(STRIP_H_PX)).rounded(px(RADIUS_16_PX)).bg(strip_bg(p, None, dark)).overflow_hidden().relative().child(strip_inner.size_full()).when(active, |el| el.child(active_badge(p)));
            let origin = match mine {
                Some(false) => g::tag(t("Imported · unverified"), p.warning_amber),
                Some(true) => g::tag(t("Installed by Mandara"), p.primary),
                None => g::tag(t("From a package or your folders"), p.on_surface_variant),
            };
            let arming = st(cx).confirm_remove.as_deref() == Some(name.as_str());
            let apply_name = name.clone();
            let action = button(SharedString::from(format!("icons-other-act-{name}")), "check", if active { t("Active") } else { t("Apply") }, !active, p)
                .when(active, |el| el.opacity(ALPHA_75).cursor_default())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !active {
                        this.icons_apply(Some(apply_name.clone()), cx);
                    }
                }));
            let remove = mine.is_some().then(|| {
                let n = name.clone();
                g::remove_button(SharedString::from(format!("icons-other-rm-{name}")), arming, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.icons_remove(n.clone(), cx)))
            });
            let error = st(cx).errors.get(&name).cloned().map(|e| g::error_row(e, p));
            let subtitle = (found.title != name).then(|| div().truncate().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(found.title.clone()));
            let hover_id = card_id.clone();
            cards.push(
                g::lift_frame(&card_id)
                    .on_hover(cx.listener(move |_, h: &bool, _, cx| {
                        g::set_hover(hover_id.clone(), *h, cx);
                        cx.notify();
                    }))
                    .child(
                g::glass_card(card_id, p.secondary, amount, p)
                    .flex_1()
                    .p(px(SPACE_8_PX))
                    .border_2()
                    .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                    .group_hover(g::CARD_GROUP, |s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(strip)
                    .child(div().px(px(SPACE_6_PX)).row().flex_col().gap(px(SPACE_6_PX)).child(div().truncate().text_start().font_family(display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(name.clone())).children(subtitle).child(div().row().child(origin)).children(error))
                    .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().items_center().gap(px(SPACE_6_PX)).child(action).child(div().flex_1()).children(remove)),
                    )
                    .into_any_element(),
            );
        }
        Some(div().row().flex_col().gap(px(SPACE_10_PX)).child(div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Also installed"))).child(fit.render(cards)).into_any_element())
    }
}

/// "Papirus · Dark" for a catalog theme name.
fn catalog_title(theme: &str) -> Option<String> {
    icons::catalog::by_theme_name(theme).map(|(t, v)| if t.variants.len() > 1 { format!("{} · {}", t.name, tr(v.label)) } else { t.name.to_string() })
}

/// The "Active" pill over a strip's corner.
fn active_badge(p: &MandaraPalette) -> gpui::Div {
    div()
        .absolute()
        .top(px(SPACE_8_PX))
        .inset_end(px(SPACE_8_PX))
        .px(px(SPACE_8_PX))
        .h(px(DIM_20_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_4_PX))
        .bg(p.primary.hsla())
        .text_color(p.on_primary.hsla())
        .text_size(px(TEXT_10_5_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon("check", ICON_11_PX, p.on_primary))
        .child(t("Active"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_split_the_catalog() {
        let styles = styles_present();
        assert!(!styles.is_empty());
        assert!(CATALOG.iter().all(|th| passes(th, 0, 0, &styles)));
        for (i, s) in styles.iter().enumerate() {
            assert!(CATALOG.iter().filter(|th| passes(th, i + 1, 0, &styles)).all(|th| th.style == *s));
            assert!(CATALOG.iter().any(|th| passes(th, i + 1, 0, &styles)), "{s:?} has themes");
        }
        assert!(CATALOG.iter().filter(|th| passes(th, 0, 2, &styles)).all(|th| th.tone.has_dark()));
        assert!(CATALOG.iter().any(|th| passes(th, 0, 1, &styles)) && CATALOG.iter().any(|th| passes(th, 0, 2, &styles)));
    }

    #[test]
    fn titles_name_the_variant_when_there_are_several() {
        assert_eq!(catalog_title("Papirus-Dark").as_deref(), Some("Papirus · Dark"));
        assert_eq!(catalog_title("Tela").as_deref(), Some("Tela"));
        assert_eq!(catalog_title("Mine"), None);
        assert!(STRIP_ICONS <= icons::PREVIEW.len());
        assert!(GATE.will.len() >= 2 && !GATE.wont.is_empty());
    }

    #[test]
    fn release_forgets_lists_and_drawn_icons() {
        let mut s = IconStudio { lists: Some(Lists::default()), ..Default::default() };
        s.drawn.insert(PathBuf::from("/x.svg"), vec![48]);
        s.loading.insert("Papirus".into());
        let generation = s.generation;
        // What `release` does to the state (it also drops the bitmaps).
        s.lists = None;
        s.loading.clear();
        s.generation += 1;
        let drawn = std::mem::take(&mut s.drawn);
        assert_eq!(drawn.len(), 1);
        assert!(s.lists.is_none() && s.loading.is_empty() && s.generation > generation);
    }
}
