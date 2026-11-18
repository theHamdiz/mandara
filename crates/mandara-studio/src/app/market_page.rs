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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::marketplace::{self, Class, Finding, Flag, InstallError, Listing, Vetting};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::plugins_local::{LocalSection, PluginsTab};

use super::StudioView;
use super::store_page::{self as store, ButtonSize, Tone};
use super::market_catalog::Catalog;
use crate::latest::Latest;
use crate::tokens::*;

/// Community keeps its visible search on reentry; Discover loads its broad shelves.
fn priming_query(tab: PluginsTab, query: &str) -> &str {
    if tab == PluginsTab::Marketplace { query } else { "" }
}

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
/// Narrow cards use labelled tooltips so both source links fit in every language.
const LINK_LABEL_MIN_W_PX: f32 = 320.0;
/// A repository name hashed to a cover hue (FNV-style multiplier).
const HASH_MULTIPLIER: u32 = 31;
const FULL_TURN_DEG: u32 = 360;

#[derive(Default)]
pub struct Market {
    /// The whole marketplace was asked for once.
    primed: bool,
    results: Option<Result<Arc<Catalog>, String>>,
    cached: HashMap<String, Arc<Catalog>>,
    requests: Latest<String>,
    cancelled: Option<Arc<AtomicBool>>,
    local_revision: u64,
    hidden: bool,
    /// Installed ids waiting for their registry entry and permission dialog.
    permissions: std::collections::VecDeque<String>,
    /// Repositories whose install found nothing to install (no release, or
    /// no plugin archive in it): repository → why. Shown on the card.
    unavailable: HashMap<String, String>,
    /// Installed marketplace plugins (repository → folder, tag), read with
    /// every search and after every install / removal — not per row per frame.
    installed: Rc<HashMap<String, (std::path::PathBuf, String)>>,
    /// The query of the last search (empty = the whole marketplace).
    searched: String,
    /// Releases that failed the compliance check (repository → flag), read
    /// with `installed`.
    pub(super) flags: Rc<BTreeMap<String, Flag>>,
    /// Installed plugins and flags were read once for the library.
    local_primed: bool,
    local_requests: Latest<()>,
    /// The repository (or plugin id) whose malicious release waits for a
    /// second click on "Install anyway".
    pub(super) arm: Option<String>,
    /// Per repository: the running step's text.
    jobs: HashMap<String, Arc<Mutex<String>>>,
    pub(super) message: Option<(bool, String)>,
    /// Debounced search after typing.
    search_task: Option<gpui::Task<()>>,
}

impl Market {
    pub(super) fn update_available(&self, update: &mandara_services::self_update::plugins::PluginUpdate) -> bool {
        self.installed.get(&update.repo).is_some_and(|(dir, tag)| update.available_for(dir, tag))
    }
    fn invalidate_local(&mut self) {
        self.local_revision = self.local_revision.wrapping_add(1);
        self.local_primed = false;
    }

    fn apply_local(&mut self, revision: u64, installed: HashMap<String, (std::path::PathBuf, String)>, flags: BTreeMap<String, Flag>) -> bool {
        if revision != self.local_revision { return false; }
        self.installed = Rc::new(installed);
        self.flags = Rc::new(flags);
        self.local_primed = true;
        true
    }
    fn cancel(&mut self) {
        if let Some(cancelled) = self.cancelled.take() { cancelled.store(true, Ordering::Relaxed); }
        // Free the active slot: `set(None)` alone left `active` set, so the
        // next search waited on the cancelled worker before it could begin.
        self.requests.abort();
        self.search_task = None;
    }

    fn reopen(&mut self) {
        self.primed = false;
        self.cancel();
    }
}

impl Drop for Market {
    fn drop(&mut self) { self.cancel(); }
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


fn next_permission(queue: &mut std::collections::VecDeque<String>, mut needs: impl FnMut(&str) -> Option<bool>) -> Option<String> {
    while let Some(id) = queue.pop_front() {
        if needs(&id) == Some(true) { return Some(id); }
    }
    None
}

impl StudioView {
    pub(super) fn market_permissions_pending(&self) -> bool {
        !self.market.hidden && !self.market.permissions.is_empty()
    }
    /// Match the installer-owned origin record, never a publisher-supplied URL.
    pub(super) fn marketplace_listing(&self, manifest: &mandara_plugins::PluginManifest) -> Option<Arc<Listing>> {
        let mandara_plugins::PluginOrigin::ThirdParty { path } = &manifest.origin else { return None; };
        self.market.cached.get("")?.installed(path, &self.market.installed)
    }
    /// Installation waits for background discovery before opening the existing
    /// focus-trapped modal. Grants stay off until the person chooses them.
    pub(super) fn market_permissions(&mut self, cfg: &mandara_config::ShellConfig, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if cx.try_global::<mandara_ui::plugins::external::ReloadState>().is_some_and(|reload| reload.is_running()) { return; }
        if self.section != crate::registry::Section::Plugins || self.modal.showing() { return; }
        let registry = cx.global::<mandara_plugins::PluginRegistry>();
        if let Some(id) = next_permission(&mut self.market.permissions, |id| registry.manifest(id).map(|manifest| super::plugin_grants::needs_permissions(manifest, cfg))) {
            self.open_plugin(&id, super::plugin_modal::ModalView::Permissions, window, cx);
        }
    }

    pub(super) fn prompt_plugin_permissions(&mut self, id: &str) {
        if !self.market.permissions.iter().any(|queued| queued == id) { self.market.permissions.push_back(id.to_owned()); }
    }
    /// Reentering the store revalidates policy before showing community cards.
    pub(super) fn market_reopen(&mut self) {
        self.market.reopen();
    }

    pub(crate) fn market_pause(&mut self) {
        self.market.primed = false;
        self.market.cancel();
    }

    pub(crate) fn market_visibility(&mut self, visible: bool) {
        self.local.visibility(visible);
        self.market.hidden = !visible;
        if !visible { self.market_pause(); }
    }

    /// Refreshes in place. One blocking read runs at a time; typing replaces
    /// its pending query, and only the current request can publish results.
    fn market_search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_owned();
        if let Some(cancelled) = self.market.cancelled.take() { cancelled.store(true, Ordering::Relaxed); }
        if self.market.searched != query {
            self.market.results = self.market.cached.get(&query).cloned().map(Ok);
        }
        self.market.searched.clone_from(&query);
        self.market.requests.set(Some(query));
        self.market.requests.refresh();
        self.market_search_next(cx);
    }

    fn market_search_next(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.market.requests.begin() else { return; };
        let query = request.key.clone();
        let mut previous = self.market.cached.get(&query).cloned().unwrap_or_default();
        let revision = self.market.local_revision;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.market.cancelled = Some(cancelled.clone());
        // Backpressure keeps one applicable snapshot; HTTP and merging stay on the worker.
        let (tx, rx) = async_channel::bounded(1);
        let worker = cx.background_executor().spawn(async move {
            mandara_services::runtime::unblock(move || {
                let installed = marketplace::installed_all();
                let flags = marketplace::flags();
                let mut local = Some((installed, flags));
                match marketplace::search_while(&query, || !cancelled.load(Ordering::Relaxed)) {
                    Ok(mut found) => {
                        let closed = std::cell::Cell::new(false);
                        marketplace::catalog::enrich(&mut found, || !cancelled.load(Ordering::Relaxed) && !closed.get(), |found| {
                            previous = Arc::new(previous.merge(found.clone()));
                            if tx.send_blocking((Ok(previous.clone()), local.take())).is_err() { closed.set(true); }
                        });
                    }
                    // Superseded: do not flash a cancel error on the Community tab.
                    Err(_) if cancelled.load(Ordering::Relaxed) => {}
                    Err(error) => { let _ = tx.send_blocking((Err(error), local.take())); }
                }
            }).await
        });
        cx.spawn(async move |this, cx| {
            while let Ok((result, local)) = rx.recv().await {
                if this.update(cx, |this, cx| {
                    if !this.market.requests.is_current(&request) { return; }
                    if let Some((installed, flags)) = local { this.market.apply_local(revision, installed, flags); }
                    this.market.results = Some(result.inspect(|catalog| {
                        const CACHED_QUERIES: usize = 4;
                        if this.market.cached.len() >= CACHED_QUERIES && !this.market.cached.contains_key(&this.market.searched)
                            && let Some(old) = this.market.cached.keys().find(|key| !key.is_empty() && **key != this.market.searched).cloned() { this.market.cached.remove(&old); }
                        this.market.cached.insert(this.market.searched.clone(), catalog.clone());
                    }));
                    cx.notify();
                }).is_err() { break; }
            }
            worker.await;
            let _ = this.update(cx, |this, cx| {
                this.market.requests.finish(request);
                this.market_search_next(cx);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Reads the installed marketplace plugins and the flags once, for the
    /// library's warnings (the marketplace search reads them too).
    pub(super) fn market_local_prime(&mut self, cx: &mut Context<Self>) {
        if self.market.hidden || self.market.local_primed {
            return;
        }
        self.market.local_primed = true;
        self.market.local_requests.set(Some(()));
        self.market.local_requests.refresh();
        self.market_local_next(cx);
    }

    fn market_local_next(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.market.local_requests.begin() else { return; };
        let revision = self.market.local_revision;
        cx.spawn(async move |this, cx| {
            let (installed, flags) = mandara_services::runtime::unblock(|| (marketplace::installed_all(), marketplace::flags())).await;
            let _ = this.update(cx, |this, cx| {
                if this.market.local_requests.finish(request) { this.market.apply_local(revision, installed, flags); }
                this.market_local_next(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads them again (after an update or install outside the marketplace).
    pub(super) fn market_local_refresh(&mut self, cx: &mut Context<Self>) {
        self.market.invalidate_local();
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

    /// The marketplace repository (`owner/repo`) a plugin folder was
    /// installed from, if it came from the marketplace.
    pub(super) fn market_repo_of(&self, dir: &std::path::Path) -> Option<String> {
        self.market.installed.iter().find(|(_, (folder, _))| folder == dir).map(|(repo, _)| repo.clone())
    }

    fn market_job(&mut self, repo: String, install: bool, force: bool, cx: &mut Context<Self>) {
        if !self.market.jobs.is_empty() {
            return;
        }
        let step = Arc::new(Mutex::new(t("Starting…").to_string()));
        self.market.invalidate_local();
        self.market.jobs.insert(repo.clone(), step.clone());
        let r = repo.clone();
        let check = vetting_check(cx);
        let (progress_tx, progress_rx) = async_channel::bounded(1);
        let task = cx.background_executor().spawn(async move {
          mandara_services::runtime::unblock(move || {
            let result = if install {
                let s = step.clone();
                marketplace::install(&r, &Vetting { check: &check, force }, &move |msg| {
                    *s.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string();
                    let _ = progress_tx.try_send(());
                }).map(|(id, tag)| (id, Some(tag)))
            } else {
                marketplace::uninstall(&r).map(|id| (id, None)).map_err(InstallError::Failed)
            };
            (result, marketplace::installed_all(), marketplace::flags())
          }).await
        });
        cx.spawn(async move |this, cx| {
            while progress_rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| {
                    if this.section == crate::registry::Section::Plugins { cx.notify(); }
                }).is_err() { break; }
            }
        }).detach();
        cx.spawn(async move |this, cx| {
            let (result, installed, flags) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.market.jobs.remove(&repo);
                this.market.invalidate_local();
                this.market.apply_local(this.market.local_revision, installed, flags);
                this.market.unavailable.remove(&repo);
                match result {
                    Ok((id, Some(tag))) => {
                        // Enable it and request background registration in both processes.
                        update_config(cx, |c| {
                            c.plugins.disabled.retain(|x| x != &id);
                            if !c.plugins.enabled.contains(&id) {
                                c.plugins.enabled.push(id.clone());
                            }
                        });
                        reload_plugins(cx);
                        this.updates.plugins.retain(|u| u.repo != repo);
                        this.market.permissions.push_back(id.clone());
                        this.market.message = Some((true, tf("Installed {plugin} {version}", &[("plugin", &id), ("version", &tag)])));
                    }
                    Ok((id, None)) => {
                        update_config(cx, |c| c.plugins.enabled.retain(|x| x != &id));
                        reload_plugins(cx);
                        this.updates.plugins.retain(|u| u.repo != repo);
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
        if self.market.hidden { return; }
        if !self.market.primed {
            self.market.primed = true;
            self.market_search(priming_query(self.plugins_tab, &self.store.query).to_owned(), cx);
        }
    }

    /// Searches the marketplace for the store query: after a typing pause
    /// (`debounce`), or now when the shown results are for another query.
    pub(super) fn market_follow_query(&mut self, query: String, debounce: bool, cx: &mut Context<Self>) {
        if !debounce {
            self.market.primed = true;
            self.market_search(query, cx);
            return;
        }
        self.market.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |studio, cx| studio.market_search(query, cx));
        }));
    }

    /// The whole marketplace's listings (not a search's), if loaded.
    fn market_listings(&self) -> Option<&Catalog> {
        match &self.market.results {
            Some(Ok(found)) if self.market.searched.is_empty() => Some(found),
            _ => None,
        }
    }

    /// Up to `n` community plugins, most recently updated first.
    pub(super) fn community_recent(&self) -> Arc<[Arc<Listing>]> {
        self.market_listings().map(|c| c.recent.clone()).unwrap_or_default()
    }

    /// Up to `n` of the most-starred community plugins, except `skip`.
    pub(super) fn community_top(&self) -> Arc<[Arc<Listing>]> {
        self.market_listings().map(|c| c.items.clone()).unwrap_or_default()
    }

    /// A community plugin as a store card: a generated cover (no art is
    /// downloaded), name, author and stars, pitch, and Get / Update / Remove.
    pub(super) fn listing_card(&mut self, scope: &str, l: &Listing, card_w: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let installed = self.market.installed.get(&l.full_name).cloned();
        let update = self.updates.plugins.iter().find(|u| u.repo == l.full_name && installed.as_ref().is_some_and(|(dir, tag)| u.available_for(dir, tag))).map(|u| (u.id.clone(), u.latest.clone()));
        let unavailable = self.market.unavailable.get(&l.full_name).cloned();
        let step = self.market.jobs.get(&l.full_name).map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone()).or_else(|| {
            let id = installed.as_ref()?.0.file_name()?.to_str()?;
            let job = self.updates.plugin_jobs.get(id)?;
            let step = super::updates_card::lock(job).step.clone();
            Some(if step.is_empty() { t("Updating…").to_owned() } else { step })
        });
        // The warning shows for the flagged release only: not once another
        // version is installed.
        let flag = self.market.flags.get(&l.full_name).filter(|f| installed.as_ref().is_none_or(|(_, tag)| *tag == f.tag) || update.as_ref().is_some_and(|(_, tag)| *tag == f.tag)).cloned();
        let (repo_i, repo_u) = (l.full_name.clone(), l.full_name.clone());
        let key = format!("{scope}-{}", l.full_name);
        let buttons: AnyElement = match (&step, &installed, &flag) {
            (Some(s), _, _) => div().min_w(px(0.0)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(s.clone()).into_any_element(),
            (None, None, _) if unavailable.is_some() => div()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .child(store::badge(unavailable.clone().unwrap_or_default(), p.warning_amber))
                .child(store::pill(format!("mk-re-{key}"), Some("refresh"), t("Check again"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| { cx.stop_propagation(); this.market_job(repo_i.clone(), true, false, cx); })))
                .into_any_element(),
            (None, Some(_), _) => {
              let update = update.map(|(id, tag)| {
                if let Some(flag) = flag.as_ref().filter(|f| f.tag == tag) {
                    self.install_anyway(format!("mk-up-{key}"), flag, p, cx, move |this, cx| this.plugin_update_start(id.clone(), true, cx)).into_any_element()
                } else {
                    store::pill(format!("mk-up-{key}"), Some("download"), t("Update"), Tone::Tonal, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| { cx.stop_propagation(); this.plugin_update_start(id.clone(), false, cx); })).into_any_element()
                }
              });
              div()
                .row()
                .flex_wrap()
                .gap(px(SPACE_6_PX))
                .children(update)
                .child(store::pill(format!("mk-rm-{key}"), Some("trash"), t("Remove"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| { cx.stop_propagation(); this.market_job(repo_u.clone(), false, false, cx); })))
                .into_any_element()
            },
            (None, None, Some(flag)) => self.install_anyway(key.clone(), flag, p, cx, move |this, cx| this.market_job(repo_i.clone(), true, true, cx)).into_any_element(),
            (None, None, None) => store::pill(format!("mk-in-{key}"), Some("plus"), t("Get"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| { cx.stop_propagation(); this.market_job(repo_i.clone(), true, false, cx); })).into_any_element(),
        };
        let cover_h = (card_w * STORE_COVER_ASPECT).floor();
        // A stable hue per repository, so each community card has its own color.
        let hue = l.full_name.bytes().fold(0u32, |h, b| h.wrapping_mul(HASH_MULTIPLIER).wrapping_add(u32::from(b))) % FULL_TURN_DEG;
        let tone = p.primary.rotate_hue(hue as f32);
        // Where its code lives and where to report a problem, always in
        // view on the card.
        let link = |what: &str, glyph: &'static str, label: &'static str, url: String| {
            div()
                .id(SharedString::from(format!("mk-{what}-{key}")))
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
                .child(icon(glyph, ICON_11_PX, p.on_surface))
                .when(card_w >= LINK_LABEL_MIN_W_PX, |d| d.child(label))
                .tooltip(super::compositor_page::tip(label, p))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = mandara_services::session::open_uri(&url);
                })
        };
        let repo_url = l.html_url.trim_end_matches('/').to_string();
        let github = div()
            .absolute()
            .top(px(SPACE_10_PX))
            .inset_start(px(SPACE_10_PX))
            .inset_end(px(SPACE_10_PX))
            .row()
            .flex_wrap()
            .justify_end()
            .gap(px(SPACE_6_PX))
            .child(link("source", "git-branch", t("Source code"), repo_url.clone()))
            .child(link("issues", "bug", t("Report an issue"), format!("{repo_url}/issues")));
        let (name, description) = l.plugin.as_ref().map(|m| m.text(mandara_i18n::language().code())).unwrap_or((&l.name, l.description.as_deref().unwrap_or_default()));
        let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        div()
            .id(SharedString::from(format!("market-card-{key}")))
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
                    .child(div().id(SharedString::from(format!("mk-name-{key}"))).text_size(px(TEXT_14_5_PX)).font_weight(FontWeight::SEMIBOLD).child(name.to_owned()))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).truncate().child(tf("by {author} · {stars} stars", &[("author", &l.owner.login), ("stars", &l.stargazers_count)])))
                    // A flagged release's warning takes the pitch's place (same height).
                    .child(
                        div()
                            .min_h(px(LEADING_17_PX * LISTING_PITCH_LINES as f32))
                            .text_size(px(TEXT_12_PX))
                            .line_height(px(LEADING_17_PX))
                            .text_color(if flag.is_some() { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() })
                            .child(flag.as_ref().map_or_else(|| description.to_owned(), Flag::short_warning)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .pt(px(SPACE_8_PX))
                            .row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(SPACE_6_PX))
                            .child(store::badge(t("Third-party"), p.secondary))
                            .when_some(installed.as_ref(), |el, (_, tag)| el.child(store::badge(tf("Installed {version}", &[("version", tag)]), p.success_emerald)))
                            .when(l.archived, |d| d.child(store::badge(t("Archived"), p.on_surface_variant)))
                            .when_some(flag.as_ref(), |d, f| d.child(store::badge(f.class.label(), p.urgent_coral)))
                    )
                    .child(div().pt(px(SPACE_8_PX)).w_full().row().flex_wrap().justify_end().gap(px(SPACE_6_PX)).child(buttons)),
            )
            .child(store::rim_overlay(LISTING_RADIUS_PX, p))
            .when_some(installed.as_ref().and_then(|(dir, _)| dir.file_name()).map(|id| id.to_string_lossy().into_owned()), |d, id| d.cursor_pointer().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&id, super::plugin_modal::ModalView::Detail, window, cx))))
            .into_any_element()
    }

    /// Community: installed third-party plugins on this system, then the
    /// GitHub marketplace searched with the store query. Installed plugins
    /// stay visible even when their repository is gone from GitHub search
    /// (Discover's Third-party shelf already lists them; Community must too).
    pub(super) fn marketplace(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        self.market_prime(cx);
        self.market_local_prime(cx);
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
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        let query_terms = store::terms(&self.store.query);
        let local: Vec<(bool, mandara_plugins::PluginManifest)> = store::community_installed(&cfg, cx).into_iter().filter(|(_, m)| store::matches(m, &query_terms)).collect();
        // Only repos whose local card is actually drawn — a search that drops
        // the local card must still show the matching GitHub listing.
        let shown_repos: std::collections::HashSet<String> = local
            .iter()
            .filter_map(|(_, m)| {
                let mandara_plugins::PluginOrigin::ThirdParty { path } = &m.origin else { return None };
                self.market.installed.iter().find(|(_, (dir, _))| dir == path).map(|(repo, _)| repo.clone())
            })
            .collect();
        let local_cards: Vec<AnyElement> = local.iter().map(|(on, m)| self.store_card("market-local", *on, m, fit.card_w, &cfg, p, cx)).collect();
        let has_local = !local_cards.is_empty();
        let on_system = has_local.then(|| {
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_14_PX))
                .child(store::shelf_head(t("On this system").into(), t("Third-party plugins already installed from the marketplace or your plugin folders").into(), None, p))
                .child(fit.render(local_cards))
                .into_any_element()
        });
        let github_empty = match &self.market.results {
            Some(Ok(found)) => found.items.iter().all(|l| shown_repos.contains(&l.full_name)),
            Some(Err(_)) => true,
            None => false,
        };
        let body: AnyElement = match &self.market.results {
            None => div().py(px(SPACE_30_PX)).row().justify_center().text_color(p.on_surface_variant.hsla()).child(t("Searching GitHub…")).into_any_element(),
            Some(Err(e)) => div().px(px(SPACE_14_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).bg(p.urgent_coral.with_alpha(ALPHA_12).hsla()).text_color(p.urgent_coral.hsla()).child(e.clone()).into_any_element(),
            Some(Ok(found)) => {
                let items: Vec<_> = found.items.iter().filter(|l| !shown_repos.contains(&l.full_name)).cloned().collect();
                if items.is_empty() {
                    if found.truncated() {
                        let hint = t("The top GitHub results are already installed. Narrow the search to see others.");
                        if has_local {
                            div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(hint)).into_any_element()
                        } else {
                            store::empty_state("globe", t("Installed results only"), hint, p)
                        }
                    } else {
                        let (title, hint) = if self.market.searched.is_empty() {
                            (t("No more community plugins on GitHub yet. Be the first to publish one."), t("A public GitHub repository tagged mandara-plugin with a release is all it takes."))
                        } else {
                            (t("No plugins found"), t("Be the first: publish yours by tagging the repository mandara-plugin (see PLUGINS.md)."))
                        };
                        if has_local && self.market.searched.is_empty() {
                            div()
                                .text_size(px(TEXT_12_5_PX))
                                .text_color(p.on_surface_variant.hsla())
                                .child(mandara_ui::bidi::paragraph(t("Nothing else on GitHub right now. Publish yours with the mandara-plugin topic to list it here.")))
                                .into_any_element()
                        } else if has_local {
                            div().into_any_element()
                        } else {
                            store::empty_state("globe", title, hint, p)
                        }
                    }
                } else {
                    let (truncated, total) = (found.truncated(), found.total);
                    let cards: Vec<AnyElement> = items.iter().map(|l| self.listing_card("market", l, fit.card_w, p, cx)).collect();
                    let heading = has_local.then(|| div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(t("From GitHub")));
                    let more = truncated.then(|| {
                        div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tf("Showing the {shown} most-starred of {total} plugins. Search to narrow it down.", &[("shown", &items.len()), ("total", &total)])))
                    });
                    div().row().flex_col().gap(px(SPACE_12_PX)).children(heading).child(fit.render(cards)).children(more).into_any_element()
                }
            }
        };
        let message = self.market.message.clone().map(|(ok, m)| super::form::note(&m, !ok, p));
        let nothing = github_empty && !has_local;
        let suggestions = nothing.then(|| self.mandara_suggestions(p, cx));
        div().row().flex_col().gap(px(SPACE_18_PX)).child(hero).children(message).children(on_system).child(body).children(suggestions).into_any_element()
    }
}

/// Hot-loads the change in Studio's own list and in the running shell (the
/// shell's folder watcher would catch it too; this makes it immediate).
pub(super) fn reload_plugins(cx: &mut gpui::App) {
    mandara_ui::plugins::external::sync(cx);
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        mandara_services::runtime::spawn("studio-plugin-reload", async move {
            mandara_services::runtime::unblock(move || {
                let _ = (ipc.0)(vec!["plugins".into(), "reload".into()]);
            }).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_offers_match_the_current_installed_folder_and_release() {
        use mandara_services::self_update::{Download, plugins::PluginUpdate};
        let mut market = Market::default();
        let dir = std::path::PathBuf::from("/plugins/picker");
        market.apply_local(0, HashMap::from([("author/picker".into(), (dir.clone(), "v0.1.0".into()))]), BTreeMap::new());
        let update = PluginUpdate { repo: "author/picker".into(), id: "picker".into(), dir, installed: "v0.1.0".into(), latest: "v0.2.0".into(), available: true, full: Some(Download { file: "picker.mandara.tar.gz".into(), url: "https://example.com/picker".into(), size: 1, sha256: None }), ..Default::default() };
        assert!(market.update_available(&update));
        assert!(!market.update_available(&PluginUpdate { latest: "v0.1.0".into(), ..update.clone() }));
        assert!(!market.update_available(&PluginUpdate { installed: "v0.0.9".into(), ..update.clone() }));
        assert!(!market.update_available(&PluginUpdate { dir: "/old/picker".into(), ..update }));
    }

    #[test]
    fn community_reentry_preserves_the_displayed_query_without_filtering_discover() {
        assert_eq!(priming_query(PluginsTab::Marketplace, "weather"), "weather");
        assert_eq!(priming_query(PluginsTab::Marketplace, ""), "");
        assert_eq!(priming_query(PluginsTab::Discover, "weather"), "");
    }

    #[test]
    fn reopening_keeps_cards_but_requires_policy_revalidation_and_rejects_an_old_search() {
        let catalog = Arc::new(Catalog::default());
        let mut market = Market::default();
        market.primed = true;
        market.results = Some(Ok(catalog.clone()));
        market.requests.set(Some(String::new()));
        let old = market.requests.begin().unwrap();
        market.reopen();
        assert!(!market.primed);
        assert!(Arc::ptr_eq(market.results.as_ref().unwrap().as_ref().unwrap(), &catalog));
        assert!(!market.requests.finish(old));
    }

    #[test]
    fn reopening_lets_the_next_marketplace_search_begin_immediately() {
        let mut market = Market::default();
        market.requests.set(Some(String::new()));
        let _in_flight = market.requests.begin().unwrap();
        market.reopen();
        market.requests.set(Some(String::new()));
        assert!(market.requests.begin().is_some(), "abort must free the slot so Community can search without waiting on the cancelled worker");
    }

    #[test]
    fn removed_plugins_cannot_block_later_permission_prompts() {
        let mut queue = ["removed", "already-granted", "new-plugin"].map(str::to_owned).into();
        assert_eq!(next_permission(&mut queue, |id| match id {
            "removed" => None,
            "already-granted" => Some(false),
            _ => Some(true),
        }).as_deref(), Some("new-plugin"));
        assert!(queue.is_empty());
    }

    #[test]
    fn dropping_the_store_cancels_its_metadata_worker() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut market = Market::default();
        market.cancelled = Some(cancelled.clone());
        drop(market);
        assert!(cancelled.load(Ordering::Relaxed));
    }

    #[test]
    fn an_old_search_cannot_restore_an_installed_version_from_before_an_update() {
        let mut market = Market::default();
        let before_update = market.local_revision;
        let installed = |tag: &str| HashMap::from([("author/picker".into(), (std::path::PathBuf::from("/plugins/picker"), tag.to_owned()))]);
        market.invalidate_local();
        assert!(market.apply_local(market.local_revision, installed("v0.2.0"), BTreeMap::new()));
        let current = market.installed.clone();
        assert!(!market.apply_local(before_update, installed("v0.1.0"), BTreeMap::new()));
        assert!(Rc::ptr_eq(&current, &market.installed));
        assert_eq!(market.installed["author/picker"].1, "v0.2.0");
    }
}
