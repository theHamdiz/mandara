//! Studio → Plugins → Community: browse GitHub repositories tagged
//! `mandara-plugin` as store cards, install / update / remove their
//! releases (logic in [`mandara_services::marketplace`]). The store search
//! (`store_page.rs`) drives the query; Discover's "New & updated" and
//! "Community picks" shelves reuse the same listing and cards.

use mandara_ui::bidi::Bidi as _;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::marketplace::{self, Class, Finding, Flag, InstallError, Listing, Results, Vetting};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::plugins_local::{LocalSection, PluginsTab};

use super::StudioView;
use super::store_page::{self as store, ButtonSize, Tone};
use crate::tokens::*;

/// Repaint interval while an install / removal runs (its step text).
const JOB_REPAINT: Duration = Duration::from_millis(250);
/// Angle of the hero card's gradient.
const HERO_GRADIENT_DEG: f32 = 135.0;
/// Typing pause before the marketplace is searched.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(450);
/// Community cards: corner radius, the initial-letter tile on the cover's
/// edge (and its OKLCH lightness), and the pitch's line clamp.
const LISTING_RADIUS_PX: f32 = 20.0;
const LISTING_TILE_PX: f32 = 44.0;
const LISTING_TILE_LIGHTNESS: f32 = 0.55;
const LISTING_PITCH_LINES: usize = 2;
/// A repository name hashed to a cover hue (FNV-style multiplier).
const HASH_MULTIPLIER: u32 = 31;
const FULL_TURN_DEG: u32 = 360;

#[derive(Default)]
pub struct Market {
    /// The whole marketplace was asked for once.
    primed: bool,
    results: Option<Result<Results, String>>,
    /// Repositories whose install found nothing to install (no release, or
    /// no plugin archive in it): repository → why. Shown on the card.
    unavailable: HashMap<String, String>,
    /// Installed marketplace plugins (repository → folder, tag), read with
    /// every search and after every install / removal — not per row per frame.
    installed: Rc<HashMap<String, (std::path::PathBuf, String)>>,
    loading: bool,
    /// The query of the last search (empty = the whole marketplace).
    searched: String,
    /// Releases that failed the compliance check (repository → flag), read
    /// with `installed`.
    pub(super) flags: Rc<BTreeMap<String, Flag>>,
    /// Installed plugins and flags were read once for the library.
    local_primed: bool,
    /// The repository (or plugin id) whose malicious release waits for a
    /// second click on "Install anyway".
    pub(super) arm: Option<String>,
    /// Per repository: the running step's text.
    jobs: HashMap<String, Arc<Mutex<String>>>,
    message: Option<(bool, String)>,
    /// Debounced search after typing.
    search_task: Option<gpui::Task<()>>,
}

/// The compliance check as Studio runs it: the built-in ids it may not
/// pose as, and whether to install a failing release anyway.
pub(super) fn vetting_check(cx: &gpui::App) -> impl Fn(&Path, &str) -> Result<(), Finding> + Send + 'static {
    let reserved = mandara_plugins::vet::builtin_ids(cx.global::<mandara_plugins::PluginRegistry>());
    move |dir, repo| mandara_plugins::vet::vet(dir, &reserved.iter().map(String::as_str).collect::<Vec<_>>(), Some(repo))
}

/// The "This version isn't recommended" line under a flagged release.
pub(super) fn flag_note(flag: &Flag, p: &MandaraPalette) -> AnyElement {
    super::form::note(&flag.warning(), true, p)
}


impl StudioView {
    fn market_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.market.loading = true;
        self.market.searched = query.trim().to_string();
        let task = cx.background_executor().spawn(async move { (marketplace::search(&query), marketplace::installed_all(), marketplace::flags()) });
        cx.spawn(async move |this, cx| {
            let (result, installed, flags) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.market.loading = false;
                this.market.results = Some(result);
                this.market.installed = Rc::new(installed);
                this.market.flags = Rc::new(flags);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reads the installed marketplace plugins and the flags once, for the
    /// library's warnings (the marketplace search reads them too).
    pub(super) fn market_local_prime(&mut self, cx: &mut Context<Self>) {
        if self.market.local_primed {
            return;
        }
        self.market.local_primed = true;
        let task = cx.background_executor().spawn(async move { (marketplace::installed_all(), marketplace::flags()) });
        cx.spawn(async move |this, cx| {
            let (installed, flags) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.market.installed = Rc::new(installed);
                this.market.flags = Rc::new(flags);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads them again (after an update or install outside the marketplace).
    pub(super) fn market_local_refresh(&mut self, cx: &mut Context<Self>) {
        self.market.local_primed = false;
        self.market_local_prime(cx);
    }

    /// The flag of installed plugin `id`, when the installed release is
    /// the flagged one (installed anyway).
    pub(super) fn installed_flag(&self, id: &str) -> Option<Flag> {
        let (repo, (_, tag)) = self.market.installed.iter().find(|(_, (dir, _))| dir.file_name().is_some_and(|n| n == id))?;
        self.market.flags.get(repo).filter(|f| f.tag == *tag).cloned()
    }

    /// "Install anyway" for a flagged release; a malicious one asks for a
    /// second click first. `go` runs the forced install.
    pub(super) fn install_anyway(&mut self, key: String, flag: &Flag, p: &MandaraPalette, cx: &mut Context<Self>, go: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> gpui::Stateful<gpui::Div> {
        let arming = flag.class == Class::Malicious;
        let armed = self.market.arm.as_deref() == Some(key.as_str());
        let label = if armed { t("Confirm: install anyway") } else { t("Install anyway") };
        store::pill(format!("mk-force-{key}"), Some("download"), label, Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            if arming && this.market.arm.as_deref() != Some(key.as_str()) {
                this.market.arm = Some(key.clone());
                cx.notify();
                return;
            }
            this.market.arm = None;
            go(this, cx);
        }))
    }

    fn market_job(&mut self, repo: String, install: bool, force: bool, cx: &mut Context<Self>) {
        if self.market.jobs.contains_key(&repo) {
            return;
        }
        let step = Arc::new(Mutex::new(t("Starting…").to_string()));
        self.market.jobs.insert(repo.clone(), step.clone());
        let r = repo.clone();
        let check = vetting_check(cx);
        let task = cx.background_executor().spawn(async move {
            let result = if install {
                let s = step.clone();
                marketplace::install(&r, &Vetting { check: &check, force }, &move |msg| *s.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string()).map(|(id, tag)| (id, Some(tag)))
            } else {
                marketplace::uninstall(&r).map(|id| (id, None)).map_err(InstallError::Failed)
            };
            (result, marketplace::installed_all(), marketplace::flags())
        });
        // Repaint while it runs so the step text updates.
        cx.spawn({
            let repo = repo.clone();
            async move |this, cx| loop {
                cx.background_executor().timer(JOB_REPAINT).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.market.jobs.contains_key(&repo)
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let (result, installed, flags) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.market.jobs.remove(&repo);
                this.market.installed = Rc::new(installed);
                this.market.flags = Rc::new(flags);
                this.market.unavailable.remove(&repo);
                match result {
                    Ok((id, Some(tag))) => {
                        // Enable it, then restart the shell so it loads.
                        update_config(cx, |c| {
                            c.plugins.disabled.retain(|x| x != &id);
                            if !c.plugins.enabled.contains(&id) {
                                c.plugins.enabled.push(id.clone());
                            }
                        });
                        reload_plugins(cx);
                        this.market.message = Some((true, tf("Installed {plugin} {version} — it's live now", &[("plugin", &id), ("version", &tag)])));
                    }
                    Ok((id, None)) => {
                        update_config(cx, |c| c.plugins.enabled.retain(|x| x != &id));
                        reload_plugins(cx);
                        this.market.message = Some((true, tf("Removed {plugin}", &[("plugin", &id)])));
                    }
                    Err(e @ (InstallError::NoRelease | InstallError::NoAsset)) => {
                        this.market.unavailable.insert(repo.clone(), e.short());
                        this.market.message = Some((false, e.to_string()));
                    }
                    Err(e) => this.market.message = Some((false, e.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Loads the whole marketplace once (Discover's community shelves and
    /// the Community tab share it).
    pub(super) fn market_prime(&mut self, cx: &mut Context<Self>) {
        if !self.market.primed {
            self.market.primed = true;
            self.market_search(String::new(), cx);
        }
    }

    /// Searches the marketplace for the store query: after a typing pause
    /// (`debounce`), or now when the shown results are for another query.
    pub(super) fn market_follow_query(&mut self, query: String, debounce: bool, cx: &mut Context<Self>) {
        if !debounce {
            if !self.market.primed || self.market.searched != query.trim() {
                self.market.primed = true;
                self.market_search(query, cx);
            }
            return;
        }
        self.market.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |studio, cx| studio.market_search(query, cx));
        }));
    }

    /// The whole marketplace's listings (not a search's), if loaded.
    fn market_listings(&self) -> &[Listing] {
        match &self.market.results {
            Some(Ok(found)) if self.market.searched.is_empty() => &found.items,
            _ => &[],
        }
    }

    /// Up to `n` community plugins, most recently updated first.
    pub(super) fn community_recent(&self, n: usize) -> Vec<Listing> {
        let mut recent: Vec<&Listing> = self.market_listings().iter().filter(|l| !l.archived).collect();
        // ISO 8601 timestamps sort as text.
        recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        recent.into_iter().take(n).cloned().collect()
    }

    /// Up to `n` of the most-starred community plugins, except `skip`.
    pub(super) fn community_top(&self, n: usize, skip: &[String]) -> Vec<Listing> {
        self.market_listings().iter().filter(|l| !l.archived && !skip.contains(&l.full_name)).take(n).cloned().collect()
    }

    /// A community plugin as a store card: a generated cover (no art is
    /// downloaded), name, author and stars, pitch, and Get / Update / Remove.
    pub(super) fn listing_card(&mut self, scope: &str, l: &Listing, card_w: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let installed = self.market.installed.get(&l.full_name).cloned();
        let unavailable = self.market.unavailable.get(&l.full_name).cloned();
        let step = self.market.jobs.get(&l.full_name).map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone());
        // The warning shows for the flagged release only: not once another
        // version is installed.
        let flag = self.market.flags.get(&l.full_name).filter(|f| installed.as_ref().is_none_or(|(_, tag)| *tag == f.tag)).cloned();
        let (repo_i, repo_u) = (l.full_name.clone(), l.full_name.clone());
        let key = format!("{scope}-{}", l.full_name);
        let buttons: AnyElement = match (&step, &installed, &flag) {
            (Some(s), _, _) => div().min_w(px(0.0)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(s.clone()).into_any_element(),
            (None, None, _) if unavailable.is_some() => div()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .child(store::badge(unavailable.clone().unwrap_or_default(), p.warning_amber))
                .child(store::pill(format!("mk-re-{key}"), Some("refresh"), t("Check again"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.market_job(repo_i.clone(), true, false, cx))))
                .into_any_element(),
            (None, Some(_), _) => div()
                .row()
                .gap(px(SPACE_6_PX))
                .child(store::pill(format!("mk-up-{key}"), Some("download"), t("Update"), Tone::Tonal, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.market_job(repo_i.clone(), true, false, cx))))
                .child(store::pill(format!("mk-rm-{key}"), Some("trash"), t("Remove"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.market_job(repo_u.clone(), false, false, cx))))
                .into_any_element(),
            (None, None, Some(flag)) => self.install_anyway(key.clone(), flag, p, cx, move |this, cx| this.market_job(repo_i.clone(), true, true, cx)).into_any_element(),
            (None, None, None) => store::pill(format!("mk-in-{key}"), Some("plus"), t("Get"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.market_job(repo_i.clone(), true, false, cx))).into_any_element(),
        };
        let cover_h = (card_w * STORE_COVER_ASPECT).floor();
        // A stable hue per repository, so each community card has its own color.
        let hue = l.full_name.bytes().fold(0u32, |h, b| h.wrapping_mul(HASH_MULTIPLIER).wrapping_add(u32::from(b))) % FULL_TURN_DEG;
        let tone = p.primary.rotate_hue(hue as f32);
        let url = l.html_url.clone();
        let github = div()
            .id(SharedString::from(format!("mk-open-{key}")))
            .absolute()
            .top(px(SPACE_10_PX))
            .inset_end(px(SPACE_10_PX))
            .h(px(DIM_24_PX))
            .px(px(SPACE_10_PX))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(SPACE_4_PX))
            .cursor_pointer()
            .bg(p.surface_base.with_alpha(ALPHA_72).hsla())
            .text_size(px(TEXT_11_PX))
            .font_weight(FontWeight::MEDIUM)
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .child(icon("external", ICON_11_PX, p.on_surface))
            .child(t("GitHub"))
            .on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&url);
            });
        let initial = l.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        div()
            .relative()
            .flex_1()
            .h_full()
            .rounded(px(LISTING_RADIUS_PX))
            .overflow_hidden()
            .bg(p.surface_container.hsla())
            .row()
            .flex_col()
            .child(store::cover_placeholder("grid", tone, card_w, cover_h, p))
            .child(
                div()
                    .absolute()
                    .top(px(cover_h - LISTING_TILE_PX / 2.0))
                    .inset_start(px(SPACE_14_PX))
                    .size(px(LISTING_TILE_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .justify_center()
                    .bg(tone.with_lightness(LISTING_TILE_LIGHTNESS).hsla())
                    .text_color(p.on_primary.hsla())
                    .text_size(px(TEXT_18_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(initial),
            )
            .child(github)
            .child(
                div()
                    .flex_1()
                    .px(px(SPACE_14_PX))
                    .pt(px(LISTING_TILE_PX / 2.0 + SPACE_8_PX))
                    .pb(px(SPACE_14_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_14_5_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(l.name.clone()))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).truncate().child(tf("by {author} · ★ {stars}", &[("author", &l.owner.login), ("stars", &l.stargazers_count)])))
                    // A flagged release's warning takes the pitch's place (same height).
                    .child(
                        div()
                            .h(px(LEADING_17_PX * LISTING_PITCH_LINES as f32))
                            .text_size(px(TEXT_12_PX))
                            .line_height(px(LEADING_17_PX))
                            .text_color(if flag.is_some() { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() })
                            .line_clamp(LISTING_PITCH_LINES)
                            .child(flag.as_ref().map_or_else(|| l.description.clone().unwrap_or_default(), Flag::short_warning)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .pt(px(SPACE_8_PX))
                            .row()
                            .items_center()
                            .gap(px(SPACE_6_PX))
                            .child(store::badge(t("Third-party"), p.secondary))
                            .when_some(installed.as_ref(), |el, (_, tag)| el.child(store::badge(tf("Installed {version}", &[("version", tag)]), p.success_emerald)))
                            .when(l.archived, |d| d.child(store::badge(t("Archived"), p.on_surface_variant)))
                            .when_some(flag.as_ref(), |d, f| d.child(store::badge(f.class.label(), p.urgent_coral)))
                            .child(div().flex_1())
                            .child(buttons),
                    ),
            )
            .child(store::rim_overlay(LISTING_RADIUS_PX, p))
            .into_any_element()
    }

    /// Community: the GitHub marketplace, searched with the store search.
    pub(super) fn marketplace(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        self.market_prime(cx);
        let hero = div()
            .relative()
            .rounded(px(RADIUS_20_PX))
            .p(px(SPACE_20_PX))
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_16_PX))
            .bg(mandara_core::oklab_gradient(HERO_GRADIENT_DEG, p.primary.with_alpha(ALPHA_20), p.tertiary.with_alpha(ALPHA_12)))
            .child(div().flex_none().size(px(DIM_52_PX)).rounded(px(RADIUS_16_PX)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_18).hsla()).child(icon("globe", ICON_22_PX, p.primary)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_240_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Community plugins")))
                    .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Free and open: plugins are GitHub repositories tagged mandara-plugin. They run sandboxed — no files, network or system access unless they ask for commands.")))),
            )
            .child(store::pill("mk-publish", Some("terminal"), t("How to publish a plugin"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_plugins_tab(PluginsTab::Local, Some(LocalSection::QuickStart), cx))))
            .child(store::rim_overlay(RADIUS_20_PX, p));
        let fit = super::grid::fit(self.page.column_w, STORE_CARD_GRID);
        let body: AnyElement = match &self.market.results {
            None => div().py(px(SPACE_30_PX)).row().justify_center().text_color(p.on_surface_variant.hsla()).child(t("Searching GitHub…")).into_any_element(),
            Some(Err(e)) => div().px(px(SPACE_14_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).bg(p.urgent_coral.with_alpha(ALPHA_12).hsla()).text_color(p.urgent_coral.hsla()).child(e.clone()).into_any_element(),
            Some(Ok(found)) if found.items.is_empty() => {
                let (title, hint) = if self.market.searched.is_empty() {
                    (t("No community plugins yet. Be the first to publish one."), t("A public GitHub repository tagged mandara-plugin with a release is all it takes."))
                } else {
                    (t("No plugins found"), t("Be the first: publish yours by tagging the repository mandara-plugin (see PLUGINS.md)."))
                };
                store::empty_state("globe", title, hint, p)
            }
            Some(Ok(found)) => {
                let (items, truncated, total) = (found.items.clone(), found.truncated(), found.total);
                let cards: Vec<AnyElement> = items.iter().map(|l| self.listing_card("market", l, fit.card_w, p, cx)).collect();
                let more = truncated.then(|| {
                    div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tf("Showing the {shown} most-starred of {total} plugins. Search to narrow it down.", &[("shown", &items.len()), ("total", &total)])))
                });
                div().row().flex_col().gap(px(SPACE_12_PX)).child(fit.render(cards)).children(more).into_any_element()
            }
        };
        let loading = (self.market.loading && self.market.results.is_some()).then(|| div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Searching GitHub…")));
        let message = self.market.message.clone().map(|(ok, m)| super::form::note(&m, !ok, p));
        let nothing = match &self.market.results {
            Some(Ok(found)) => found.items.is_empty(),
            Some(Err(_)) => true,
            None => false,
        };
        let suggestions = nothing.then(|| self.mandara_suggestions(p, cx));
        div().row().flex_col().gap(px(SPACE_18_PX)).child(hero).children(message).children(loading).child(body).children(suggestions).into_any_element()
    }
}

/// Hot-loads the change in Studio's own list and in the running shell (the
/// shell's folder watcher would catch it too; this makes it immediate).
pub(super) fn reload_plugins(cx: &mut gpui::App) {
    mandara_ui::plugins::external::sync(cx);
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        std::thread::spawn(move || {
            let _ = (ipc.0)(vec!["plugins".into(), "reload".into()]);
        });
    }
}
