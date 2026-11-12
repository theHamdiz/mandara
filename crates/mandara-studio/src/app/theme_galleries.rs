//! What the Cursors and Icons tabs of Studio → Themes share: the card
//! material and its hover lift (a small spring that rests when the
//! pointer leaves, so an open tab costs no animation work), the search
//! field and filter chips, the progress bar, and the **Your own** panel
//! that installs a theme from an archive, a folder or a pasted link
//! (`cursor import` / `icons import` underneath; a link has no checksum,
//! so it is marked unverified and asks before installing).
//!
//! Leaving a tab or the page calls [`release`]: previews, springs, the
//! cover art of the plugin gates and the search fields all go; installs
//! and imports that are running keep going.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::spring::MotionProfile;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf};
use mandara_services::icons::install::ImportSource;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;

use super::StudioView;
use super::compositor_page::{button, chip, tip};
use crate::text_field::TextField;
use crate::tokens::*;

/// A card's lift on hover: quick, one soft overshoot.
const LIFT_SPRING: SpringConfig = SpringConfig::new(420.0, 24.0, 1.0);
/// How far a hovered card rises.
const LIFT_PX: f32 = 3.0;
/// The card material: how much its accent tints the surface.
const CARD_TINT: f32 = 0.07;
const DIAGONAL_DEG: f32 = 135.0;
const HORIZONTAL_DEG: f32 = 90.0;
/// The progress bar.
const PROGRESS_H_PX: f32 = DIM_5_PX;
/// The bar never draws narrower than this share (it reads as "started").
const PROGRESS_MIN: f32 = 0.03;
pub(super) const PERMILLE: f32 = 1000.0;
pub(super) const PERCENT_PER_PERMILLE: u32 = 10;
/// Repaint interval while an install runs (progress moves; stops with it).
pub(super) const JOB_REPAINT: Duration = Duration::from_millis(120);
/// The search field's width when there is room.
const SEARCH_W_PX: f32 = DIM_280_PX;
/// Stars above this read as "4.2k".
const THOUSAND: f32 = 1000.0;

/// Which gallery an import panel serves.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum ImportKind {
    Cursors,
    Icons,
}

/// An install or import in flight.
pub(super) struct Job {
    /// Permille done.
    pub progress: Arc<AtomicU32>,
    pub cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn new() -> Self {
        Self { progress: Arc::new(AtomicU32::new(0)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn permille(&self) -> u32 {
        self.progress.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct ImportPanel {
    url_field: Option<Entity<TextField>>,
    url: String,
    /// A link waiting for "Install anyway".
    confirm: Option<String>,
    job: Option<Job>,
    /// The last result: text and whether it is an error.
    note: Option<(String, bool)>,
}

#[derive(Default)]
struct Galleries {
    lifts: HashMap<SharedString, (bool, SpringValue<f32>)>,
    clock: Option<FrameClock>,
    imports: HashMap<ImportKind, ImportPanel>,
    /// The tabs' search fields (made on first use) and their text.
    searches: HashMap<ImportKind, (Entity<TextField>, Rc<str>)>,
}
impl Global for Galleries {}

fn st(cx: &mut App) -> &mut Galleries {
    cx.default_global::<Galleries>()
}

/// Drops what the Cursors and Icons tabs hold (leaving the tab or Themes).
pub(super) fn release(cx: &mut App) {
    super::cursor_page::release(cx);
    super::icons_page::release(cx);
    super::plugin_gate::release(cx);
    let s = st(cx);
    s.lifts = HashMap::new();
    s.clock = None;
    s.searches.clear();
    for panel in s.imports.values_mut() {
        panel.url_field = None;
        panel.confirm = None;
        if panel.job.is_none() {
            panel.note = None;
        }
    }
}

/// Advances the card springs; asks for the next frame only while one
/// moves (reduced motion snaps them).
pub(super) fn step_lifts(cfg: &mandara_config::ShellConfig, window: &mut Window, cx: &mut App) {
    let m = &cfg.motion;
    let profile = MotionProfile::new(m.stiffness, m.damping, m.mass, m.reduce_motion);
    let s = st(cx);
    let clock = s.clock.get_or_insert_with(FrameClock::new);
    let dt = clock.tick(Instant::now());
    if step_springs(&mut s.lifts, dt, &profile) {
        window.request_animation_frame();
    } else if let Some(c) = s.clock.as_mut() {
        c.idle();
    }
}

/// Moves every card spring toward its hover state by `dt`; whether one
/// still moves. Settled cards at rest need no entry; one still swinging
/// back (its overshoot dips below rest) keeps its spring until it settles,
/// so the card never jumps.
fn step_springs(lifts: &mut HashMap<SharedString, (bool, SpringValue<f32>)>, dt: f32, profile: &MotionProfile) -> bool {
    let mut moving = false;
    for (hovered, value) in lifts.values_mut() {
        value.set_target(if *hovered { 1.0 } else { 0.0 });
        moving |= value.step_in(dt, profile);
    }
    lifts.retain(|_, (hovered, value)| *hovered || value.is_animating());
    moving
}

/// How far card `id` is lifted (0 at rest, 1 hovered).
pub(super) fn lift(id: &str, cx: &mut App) -> f32 {
    st(cx).lifts.get(id).map_or(0.0, |(_, v)| v.value().clamp(0.0, 1.2))
}

/// Records whether card `id` is hovered (its spring then follows).
pub(super) fn set_hover(id: SharedString, hovered: bool, cx: &mut App) {
    let entry = st(cx).lifts.entry(id).or_insert_with(|| (false, SpringValue::new(0.0, LIFT_SPRING)));
    entry.0 = hovered;
}

/// The group a lifting card's hover styles follow: its [`lift_frame`].
pub(super) const CARD_GROUP: &str = "gallery-card";

/// The still frame a lifting card sits in. The frame, not the card, takes
/// the pointer: a card that rises from under the pointer at its lower edge
/// would otherwise lose its own hover, drop back, be hovered again and
/// flicker. Give the card `.flex_1()` (it fills the frame) and style its
/// hover with `group_hover(CARD_GROUP, …)`.
pub(super) fn lift_frame(id: &SharedString) -> gpui::Stateful<gpui::Div> {
    div().id(gpui::ElementId::NamedInteger(id.clone(), 0)).group(CARD_GROUP).flex_1().min_w(px(0.0)).row().flex_col()
}

/// The glass card material: a surface tinted toward `tint`, the specular
/// rim, raised by `amount` of the hover lift (inside a [`lift_frame`]).
pub(super) fn glass_card(id: impl Into<SharedString>, tint: Color, amount: f32, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .relative()
        .top(px(-LIFT_PX * amount))
        .rounded(px(RADIUS_22_PX))
        .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(tint, CARD_TINT + CARD_TINT * amount)))
        .shadow(mandara_core::glass_rim(p.specular_rim_light.with_alpha(ALPHA_60 + (1.0 - ALPHA_60) * amount.min(1.0)), p.specular_rim_shadow))
}

/// A thin progress bar at `permille`.
pub(super) fn progress_bar(permille: u32, p: &MandaraPalette) -> gpui::Div {
    div().h(px(PROGRESS_H_PX)).w_full().rounded_full().bg(p.surface_container_high.hsla()).child(
        div()
            .h_full()
            .w(gpui::relative((permille.min(PERMILLE as u32) as f32 / PERMILLE).max(PROGRESS_MIN)))
            .rounded_full()
            .bg(mandara_core::oklab_gradient(HORIZONTAL_DEG, p.accent_gradient_start, p.accent_gradient_end)),
    )
}

/// An error line with its icon.
pub(super) fn error_row(text: String, p: &MandaraPalette) -> gpui::Div {
    div().row().items_start().gap(px(SPACE_6_PX)).child(icon("alert", ICON_13_PX, p.urgent_coral)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.urgent_coral.hsla()).child(mandara_ui::bidi::paragraph(text)))
}

/// A GitHub star count in words ("4.2k stars"), no star glyph.
pub(super) fn stars(n: u32) -> String {
    let count = if n as f32 >= THOUSAND { format!("{:.1}k", n as f32 / THOUSAND) } else { n.to_string() };
    if n == 1 { t("1 star").to_string() } else { tf("{count} stars", &[("count", &count)]) }
}

/// Whether `query` (lower case) matches any of `fields`.
pub(super) fn matches(query: &str, fields: &[&str]) -> bool {
    query.is_empty() || query.split_whitespace().all(|word| fields.iter().any(|f| f.to_lowercase().contains(word)))
}

/// The round link to a theme's homepage.
pub(super) fn homepage_button(id: SharedString, url: &'static str, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(DIM_34_PX))
        .rounded_full()
        .flex_none()
        .row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
        .tooltip(tip(url, p))
        .child(icon("external", ICON_14_PX, p.on_surface_variant))
        .on_click(move |_: &ClickEvent, _, _| super::open_link(url))
}

/// The remove button: a trash glyph, then "Remove?" while armed.
pub(super) fn remove_button(id: SharedString, arming: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(DIM_34_PX))
        .px(px(if arming { SPACE_12_PX } else { SPACE_9_PX }))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_6_PX))
        .flex_none()
        .cursor_pointer()
        .tab_index(0)
        .bg(if arming { p.urgent_coral.hsla() } else { p.urgent_coral.with_alpha(ALPHA_14).hsla() })
        .text_color(if arming { p.on_primary.hsla() } else { p.urgent_coral.hsla() })
        .text_size(px(TEXT_12_5_PX))
        .font_weight(FontWeight::MEDIUM)
        .tooltip(tip(t("Remove (only what Mandara installed)"), p))
        .child(icon("trash", ICON_14_PX, if arming { p.on_primary } else { p.urgent_coral }))
        .when(arming, |el| el.child(t("Remove?")))
}

/// A small pill label (a tone, a style, "Unverified").
pub(super) fn tag(text: impl Into<SharedString>, color: Color) -> gpui::Div {
    div().px(px(SPACE_7_PX)).h(px(DIM_20_PX)).rounded_full().row().items_center().bg(color.with_alpha(ALPHA_14).hsla()).text_color(color.hsla()).text_size(px(TEXT_10_5_PX)).font_weight(FontWeight::MEDIUM).child(text.into())
}

impl StudioView {
    /// The search text of `kind`'s tab, lower case.
    pub(super) fn gallery_query(kind: ImportKind, cx: &mut App) -> Rc<str> {
        st(cx).searches.get(&kind).map(|(_, q)| q.clone()).unwrap_or_else(|| Rc::from(""))
    }

    /// The search field of `kind`'s tab (made on first use).
    pub(super) fn gallery_search(&mut self, kind: ImportKind, placeholder: String, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let field = match st(cx).searches.get(&kind) {
            Some((f, _)) => f.clone(),
            None => {
                let this = cx.entity().downgrade();
                let f = cx.new(|cx| {
                    TextField::new(
                        String::new(),
                        placeholder,
                        true,
                        Rc::new(move |value: String, _, cx| {
                            let _ = this.update(cx, |_, cx| {
                                if let Some(entry) = st(cx).searches.get_mut(&kind) {
                                    entry.1 = value.to_lowercase().into();
                                }
                                cx.notify();
                            });
                        }),
                        cx,
                    )
                });
                st(cx).searches.insert(kind, (f.clone(), Rc::from("")));
                f
            }
        };
        div()
            .w(px(SEARCH_W_PX))
            .max_w_full()
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .child(icon("search", ICON_15_PX, p.on_surface_variant))
            .child(div().flex_1().min_w(px(0.0)).child(field))
            .into_any_element()
    }

    /// A row of filter chips; `on_pick(index)` selects one.
    pub(super) fn filter_chips(&mut self, id: &'static str, labels: Vec<String>, current: usize, on_pick: fn(usize, &mut App), p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_6_PX))
            .children(labels.into_iter().enumerate().map(|(i, label)| {
                chip(SharedString::from(format!("{id}-{i}")), label, i == current, p)
                    .tab_index(0)
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        on_pick(i, cx);
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn import_url_field(&mut self, kind: ImportKind, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let existing = st(cx).imports.get(&kind).and_then(|i| i.url_field.clone());
        let field = match existing {
            Some(f) => f,
            None => {
                let this = cx.entity().downgrade();
                let f = cx.new(|cx| {
                    TextField::new(
                        String::new(),
                        t("https://… a link to a .tar.gz, .tar.xz or .zip"),
                        true,
                        Rc::new(move |value: String, _, cx| {
                            let _ = this.update(cx, |_, cx| {
                                let panel = st(cx).imports.entry(kind).or_default();
                                panel.url = value;
                                panel.confirm = None;
                                cx.notify();
                            });
                        }),
                        cx,
                    )
                });
                st(cx).imports.entry(kind).or_default().url_field = Some(f.clone());
                f
            }
        };
        div().flex_1().min_w(px(DIM_200_PX)).row().items_center().gap(px(SPACE_8_PX)).child(icon("globe", ICON_15_PX, p.on_surface_variant)).child(div().flex_1().min_w(px(0.0)).child(field)).into_any_element()
    }

    /// Opens the portal's picker for an archive or a folder, then imports
    /// what was picked.
    fn import_pick(kind: ImportKind, folder: bool, cx: &mut Context<Self>) {
        super::form::job(
            cx,
            move || {
                let start = dirs::download_dir().filter(|d| d.is_dir());
                if folder {
                    mandara_services::file_chooser::open_folder(t("Pick a theme folder"), start)
                } else {
                    let mut picked = mandara_services::file_chooser::open_files(t("Pick a theme archive"), t("Archives"), mandara_services::file_chooser::ARCHIVE_MIME_TYPES, mandara_services::file_chooser::ARCHIVE_GLOBS, start)?;
                    Ok(picked.pop())
                }
            },
            move |_, result, cx| match result {
                Ok(Some(path)) => Self::import_run(kind, ImportSource::Path(path), cx),
                Ok(None) => {}
                Err(e) => st(cx).imports.entry(kind).or_default().note = Some((tf("Couldn't open the file picker: {error}", &[("error", &e)]), true)),
            },
        );
    }

    /// Imports `source` on a worker, with progress and cancel.
    fn import_run(kind: ImportKind, source: ImportSource, cx: &mut Context<Self>) {
        let panel = st(cx).imports.entry(kind).or_default();
        if panel.job.is_some() {
            return;
        }
        let job = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        panel.job = Some(job);
        panel.note = None;
        panel.confirm = None;
        let task = cx.background_executor().spawn(async move {
            let report = |permille: f32| progress.store((permille * PERMILLE) as u32, Ordering::Relaxed);
            match kind {
                ImportKind::Cursors => {
                    let env = mandara_services::cursors::install::Env::current();
                    mandara_services::cursors::install::import(&source, &env, &|p| report(p.fraction()), &cancel).map(|r| (r.installed.into_iter().map(|i| i.theme).collect::<Vec<_>>(), r.skipped))
                }
                ImportKind::Icons => {
                    let env = mandara_services::icons::install::Env::current();
                    mandara_services::icons::install::import(&source, &env, &|p| report(p.fraction()), &cancel).map(|r| (r.installed.into_iter().map(|i| i.theme).collect::<Vec<_>>(), r.skipped))
                }
            }
        });
        Self::repaint_while(cx, move |cx| st(cx).imports.get(&kind).is_some_and(|p| p.job.is_some()));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                let panel = st(cx).imports.entry(kind).or_default();
                panel.job = None;
                panel.note = Some(match result {
                    Ok((installed, skipped)) if skipped.is_empty() => (tf("Installed {themes}", &[("themes", &installed.join(", "))]), false),
                    Ok((installed, skipped)) => {
                        let why: Vec<String> = skipped.iter().map(|(n, w)| format!("{n} ({w})")).collect();
                        (tf("Installed {themes}; skipped {skipped}", &[("themes", &installed.join(", ")), ("skipped", &why.join(", "))]), false)
                    }
                    Err(e) => (e, true),
                });
                match kind {
                    ImportKind::Cursors => Self::cursor_refresh(cx),
                    ImportKind::Icons => Self::icons_refresh(cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Repaints every [`JOB_REPAINT`] while `running` holds.
    pub(super) fn repaint_while(cx: &mut Context<Self>, running: impl Fn(&mut App) -> bool + 'static) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(JOB_REPAINT).await;
            let go = this.update(cx, |_, cx| {
                cx.notify();
                running(cx)
            });
            if !matches!(go, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn import_get(kind: ImportKind, cx: &mut Context<Self>) {
        let panel = st(cx).imports.entry(kind).or_default();
        let url = panel.url.trim().to_string();
        if url.is_empty() {
            return;
        }
        match mandara_services::theme_pkg::require_https(&url) {
            // No checksum exists for a link: ask first.
            Ok(()) => panel.confirm = Some(url),
            Err(e) => panel.note = Some((e, true)),
        }
        cx.notify();
    }

    /// The **Your own** panel: archive, folder or link.
    pub(super) fn import_panel(&mut self, kind: ImportKind, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let url_field = self.import_url_field(kind, p, cx);
        let panel = st(cx).imports.entry(kind).or_default();
        let job = panel.job.as_ref().map(Job::permille);
        let confirm = panel.confirm.clone();
        let note = panel.note.clone();
        let busy = job.is_some();
        let (what, detail) = match kind {
            ImportKind::Cursors => (t("Your own cursors"), t("Install a cursor theme from an archive or a folder on this computer, or from a link. Every theme found in it is checked (an index.theme and readable cursor files); nothing in it is run.")),
            ImportKind::Icons => (t("Your own icon sets"), t("Install an icon theme from an archive or a folder on this computer, or from a link. Every theme found in it is checked (a valid index.theme and its icons); scripts in it are never run or kept.")),
        };
        let key = match kind {
            ImportKind::Cursors => "cursor",
            ImportKind::Icons => "icons",
        };
        let pickers = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_8_PX))
            .child(button(SharedString::from(format!("{key}-import-file")), "package", t("Archive…"), false, p).tab_index(0).when(busy, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| Self::import_pick(kind, false, cx))))
            .child(button(SharedString::from(format!("{key}-import-folder")), "folder", t("Folder…"), false, p).tab_index(0).when(busy, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| Self::import_pick(kind, true, cx))))
            .child(url_field)
            .child(button(SharedString::from(format!("{key}-import-url")), "download", t("Get"), true, p).tab_index(0).when(busy, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| Self::import_get(kind, cx))));
        let confirm_row = confirm.map(|url| {
            div()
                .rounded(px(RADIUS_16_PX))
                .bg(p.warning_amber.with_alpha(ALPHA_12).hsla())
                .p(px(SPACE_12_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(
                    div()
                        .row()
                        .items_start()
                        .gap(px(SPACE_8_PX))
                        .child(icon("shield", ICON_16_PX, p.warning_amber))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .row()
                                .flex_col()
                                .gap(px(SPACE_3_PX))
                                .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Unverified download")))
                                .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tf("{url} has no known checksum, so Mandara can't tell whether it is what its author published. Install it only from a source you trust; it is checked like any theme and nothing in it is run.", &[("url", &url)])))),
                        ),
                )
                .child(
                    div()
                        .row()
                        .gap(px(SPACE_8_PX))
                        .child(button(SharedString::from(format!("{key}-import-confirm")), "check", t("Install anyway"), true, p).tab_index(0).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| Self::import_run(kind, ImportSource::Url(url.clone()), cx))))
                        .child(button(SharedString::from(format!("{key}-import-cancel")), "close", t("Cancel"), false, p).tab_index(0).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            st(cx).imports.entry(kind).or_default().confirm = None;
                            cx.notify();
                        }))),
                )
        });
        let job_row = job.map(|pm| {
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().flex_1().child(progress_bar(pm, p)))
                .child(div().text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(tf("Installing {percent}%", &[("percent", &(pm / PERCENT_PER_PERMILLE))])))
                .child(
                    div()
                        .id(SharedString::from(format!("{key}-import-stop")))
                        .cursor_pointer()
                        .text_size(px(TEXT_12_PX))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.primary.hsla())
                        .child(t("Cancel"))
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            if let Some(job) = st(cx).imports.get(&kind).and_then(|p| p.job.as_ref()) {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        })),
                )
        });
        let note_row = note.map(|(text, error)| if error { error_row(text, p) } else { div().row().items_start().gap(px(SPACE_6_PX)).child(icon("check", ICON_13_PX, p.success_emerald)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(text))) });
        glass_card(SharedString::from(format!("{key}-import")), p.tertiary, 0.0, p)
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(div().size(px(DIM_36_PX)).rounded(px(RADIUS_12_PX)).bg(p.tertiary.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon("plus", ICON_17_PX, p.tertiary)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(what))
                            .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(detail))),
                    ),
            )
            .child(pickers)
            .children(confirm_row)
            .children(job_row)
            .children(note_row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_every_word_in_any_field() {
        assert!(matches("", &["Papirus"]));
        assert!(matches("papirus dark", &["Papirus", "Dark variant"]));
        assert!(!matches("papirus neon", &["Papirus", "Dark"]));
        assert!(matches("vince", &["Tela", "Vince Liuice"]));
    }

    #[test]
    fn stars_read_in_words() {
        assert_eq!(stars(4233), "4.2k stars");
        assert_eq!(stars(186), "186 stars");
        assert_eq!(stars(1), "1 star");
    }

    #[test]
    fn hover_lifts_settle_and_leave_nothing_behind() {
        let mut lifts: HashMap<SharedString, (bool, SpringValue<f32>)> = HashMap::new();
        lifts.insert("a".into(), (true, SpringValue::new(0.0, LIFT_SPRING)));
        let profile = MotionProfile::default();
        let dt = 1.0 / 60.0;
        for _ in 0..300 {
            step_springs(&mut lifts, dt, &profile);
        }
        assert_eq!(lifts["a"].1.value(), 1.0);
        assert!(!step_springs(&mut lifts, dt, &profile), "at rest: no frames");
        lifts.get_mut("a").unwrap().0 = false;
        // Falling back it swings past rest; the spring is kept (no jump)
        // until it really settles.
        let mut last = 1.0_f32;
        let mut dipped = false;
        for _ in 0..300 {
            step_springs(&mut lifts, dt, &profile);
            let Some((_, v)) = lifts.get("a") else { break };
            assert!((v.value() - last).abs() < 0.2, "moves smoothly: {last} -> {}", v.value());
            dipped |= v.value() < 0.0;
            last = v.value();
        }
        assert!(dipped, "the lift spring overshoots rest once");
        assert!(lifts.is_empty(), "a settled, unhovered card leaves no entry");
    }

    /// The card lifts inside a frame that stays put: the pointer is tested
    /// against the frame, so the lift can't move the card from under it.
    #[test]
    fn the_hover_frame_never_moves_with_the_lift() {
        let p = mandara_theme::engine::default_palette(true);
        let id = SharedString::from("card");
        let mut frame = lift_frame(&id);
        assert_eq!(frame.style().inset.top, None, "the frame isn't offset");
        let mut card = glass_card(id, p.primary, 1.0, &p);
        assert_eq!(card.style().inset.top, Some(px(-LIFT_PX).into()), "the card lifts");
    }
}
