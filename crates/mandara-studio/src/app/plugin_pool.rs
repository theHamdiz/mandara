//! Studio → Bar → Layout designer: the plugin pool under the bar's zones.
//!
//! Every plugin that can sit on the bar, in equal cells on an aligned grid,
//! grouped by category (a category rail on wide windows, chips on narrow
//! ones) and filtered live by a search over the translated name, the
//! English name and the id ([`rank`]: what holds the typed words first,
//! a fuzzy match when nothing does, then category and description hits).
//! Plugins already on the bar carry the
//! zone they sit in. Click a plugin for a small menu (add to or move
//! between zones, take it off, details), or drag it onto a zone; dropping a
//! bar item back on the pool takes it off the bar.
//!
//! The searchable strings are built once per plugin set and language
//! ([`PoolIndex`]); a keystroke only runs the matcher over them, and the
//! ranked order is kept until the query or the plugin set changes.

use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use gpui::{AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::side_bars::SideZone;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{PluginManifest, PluginOrigin, PluginRegistry};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher};

use super::designer::{DragChip, Lane, bar_capable, place};
use super::plugin_modal::ModalView;
use super::popover::{self, Anchor, Appear};
use super::{PLUGIN_CATEGORIES, StudioView, grid, plugin_icon, plugin_tile};
use crate::text_field::TextField;
use crate::tokens::*;

// ── Categories ───────────────────────────────────────────────────────────

/// Faith plugins: listed only under their own category, at the end of the
/// built-in ones (never among the essentials).
const FAITH: &[&str] = &["prayer-times", "jannah", "quran", "hadith"];
const FAITH_CATEGORY: &str = tk("Faith");
/// The launcher button sits with the bar's essentials (the store lists it
/// on its own).
const LAUNCHER: &str = "launcher";
const BAR_ESSENTIALS: &str = tk("Bar essentials");
/// Built-ins outside every store category.
const MORE_CATEGORY: &str = tk("More");
const THIRD_PARTY_CATEGORY: &str = tk("Third-party");

/// The pool's category of a plugin: the store's, with faith plugins in
/// their own group.
pub(crate) fn category_of(id: &str, third_party: bool) -> &'static str {
    if third_party {
        THIRD_PARTY_CATEGORY
    } else if id == LAUNCHER {
        BAR_ESSENTIALS
    } else if FAITH.contains(&id) {
        FAITH_CATEGORY
    } else {
        PLUGIN_CATEGORIES.iter().find(|(_, ids)| ids.contains(&id)).map(|(t, _)| *t).unwrap_or(MORE_CATEGORY)
    }
}

/// Categories in the order the pool lists them: the store's built-in
/// groups, then Faith, More and Third-party.
pub(crate) fn category_order() -> impl Iterator<Item = &'static str> {
    PLUGIN_CATEGORIES.iter().map(|(t, _)| *t).filter(|t| *t != THIRD_PARTY_CATEGORY).chain([FAITH_CATEGORY, MORE_CATEGORY, THIRD_PARTY_CATEGORY])
}

// ── The searchable index ─────────────────────────────────────────────────

/// One plugin, with its searchable strings built once.
pub(crate) struct PoolEntry {
    pub manifest: PluginManifest,
    /// The name in the current language.
    pub name: String,
    pub category: &'static str,
    /// Fuzzy-matched: the translated name, the English name and the id.
    hay: String,
    /// Lower-case translated name (sorting, "starts with" ranking).
    name_lower: String,
    /// Lower-case English name and id.
    alt_lower: String,
    /// Lower-case category and description: plain substring hits.
    extra_lower: String,
}

impl PoolEntry {
    pub(crate) fn new(manifest: PluginManifest) -> Self {
        let name = manifest.display_name();
        let third_party = matches!(manifest.origin, PluginOrigin::ThirdParty { .. });
        let category = category_of(&manifest.id, third_party);
        let hay = format!("{name} {} {}", manifest.name, manifest.id);
        let extra_lower = format!("{} {}", tr(category), manifest.display_description()).to_lowercase();
        let alt_lower = format!("{} {}", manifest.name, manifest.id).to_lowercase();
        Self { name_lower: name.to_lowercase(), alt_lower, name, category, hay, extra_lower, manifest }
    }
}

/// Every plugin of the registry, sorted by name, with a stamp of what it
/// was built from (the plugins and the language).
#[derive(Default)]
pub(crate) struct PoolIndex {
    stamp: u64,
    pub entries: Vec<PoolEntry>,
}

/// A cheap fingerprint of the registry's plugins and the language (no
/// allocation: it runs every frame the designer is shown).
fn stamp(registry: &PluginRegistry) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    mandara_i18n::language().code().hash(&mut h);
    for m in registry.manifests() {
        m.id.hash(&mut h);
        m.name.hash(&mut h);
        m.version.hash(&mut h);
    }
    h.finish()
}

impl PoolIndex {
    pub(crate) fn build(manifests: impl IntoIterator<Item = PluginManifest>, stamp: u64) -> Self {
        let mut entries: Vec<PoolEntry> = manifests.into_iter().filter(|m| bar_capable(&m.id)).map(PoolEntry::new).collect();
        entries.sort_by(|a, b| a.name_lower.cmp(&b.name_lower).then_with(|| a.manifest.id.cmp(&b.manifest.id)));
        Self { stamp, entries }
    }
}

struct Hay<'a>(usize, &'a str);
impl AsRef<str> for Hay<'_> {
    fn as_ref(&self) -> &str {
        self.1
    }
}

/// How well every typed word appears, contiguously, in a plugin's names
/// or id: 0 the name starts with it, 1 a word of the name does, 2 the name
/// holds it, 3 the English name or the id does (`None`: a word is missing).
fn tier(e: &PoolEntry, words: &[&str]) -> Option<u8> {
    let mut worst = 0;
    for w in words {
        let t = if e.name_lower.starts_with(w) {
            0
        } else if e.name_lower.split(|c: char| !c.is_alphanumeric()).any(|part| part.starts_with(w)) {
            1
        } else if e.name_lower.contains(w) {
            2
        } else if e.alt_lower.contains(w) {
            3
        } else {
            return None;
        };
        worst = worst.max(t);
    }
    Some(worst)
}

/// Indices of `entries` matching `query`, best first. Plugins whose names
/// or id hold what was typed come first (name starts, word starts, the
/// rest; in name order within each); only when none does, a fuzzy match
/// over the names and id (typos, skipped letters) takes their place. Plain
/// substring hits in the category or description follow. All of them, in
/// order, for an empty query.
pub(crate) fn rank(entries: &[PoolEntry], query: &str, matcher: &mut Matcher) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let mut tiered: Vec<(u8, usize)> = entries.iter().enumerate().filter_map(|(i, e)| tier(e, &words).map(|t| (t, i))).collect();
    let mut out: Vec<usize> = if tiered.is_empty() {
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(usize, u32)> = pattern.match_list(entries.iter().enumerate().map(|(i, e)| Hay(i, e.hay.as_str())), matcher).into_iter().map(|(h, s)| (h.0, s)).collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.into_iter().map(|(i, _)| i).collect()
    } else {
        tiered.sort_unstable();
        tiered.into_iter().map(|(_, i)| i).collect()
    };
    let mut hit = vec![false; entries.len()];
    for &i in &out {
        hit[i] = true;
    }
    out.extend((0..entries.len()).filter(|&i| !hit[i] && words.iter().all(|w| entries[i].extra_lower.contains(w))));
    out
}

/// `shown` (indices into `entries`, in order) split into categories, in
/// [`category_order`]; empty categories are left out.
pub(crate) fn group(entries: &[PoolEntry], shown: &[usize]) -> Vec<(&'static str, Vec<usize>)> {
    category_order()
        .map(|cat| (cat, shown.iter().copied().filter(|&i| entries[i].category == cat).collect::<Vec<_>>()))
        .filter(|(_, list)| !list.is_empty())
        .collect()
}

// ── State ────────────────────────────────────────────────────────────────

/// What a click on a plugin opened a menu for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuFor {
    /// A pool cell.
    Pool,
    /// A zone item at `index` of a zone holding `len` items.
    Zone { index: usize, len: usize },
}

#[derive(Clone, Debug)]
pub(crate) struct Menu {
    pub id: String,
    pub name: String,
    /// Where the plugin sits now (`Lane::Tray`: nowhere).
    pub from: Lane,
    pub kind: MenuFor,
    /// The click, in window px.
    pub at: (f32, f32),
}

/// The layout designer's state, kept on [`StudioView`].
#[derive(Default)]
pub struct Designer {
    /// Presets keep the user's existing items unless replacement is explicit.
    pub(super) replace_items: bool,
    pub(super) preset_selection: super::designer::PresetSelection,
    search: Option<Entity<TextField>>,
    query: String,
    /// The selected category (`None`: every category).
    category: Option<&'static str>,
    index: PoolIndex,
    /// The ranked order for (query, index stamp).
    ranked: Option<(String, u64, Rc<[usize]>)>,
    matcher: Option<Matcher>,
    pub(crate) menu: Option<Menu>,
    appear: Appear,
}

// ── Layout ───────────────────────────────────────────────────────────────

/// Equal cells of the pool grid.
const POOL_GRID: CardGrid = CardGrid { min_w: 196.0, max_w: 300.0, gap: SPACE_8_PX };
/// A pool cell's height and artwork.
const POOL_CELL_H_PX: f32 = DIM_40_PX;
const POOL_TILE_PX: f32 = DIM_24_PX;
/// The pool card's padding.
const POOL_PAD_PX: f32 = SPACE_14_PX;
/// The category rail beside the grid, from this page width.
const POOL_RAIL_FROM_PX: f32 = 860.0;
const POOL_RAIL_W_PX: f32 = DIM_200_PX;
const POOL_RAIL_ITEM_H_PX: f32 = DIM_32_PX;
/// The search field beside the pool's title (wide pages).
const POOL_SEARCH_W_PX: f32 = DIM_280_PX;
/// Category chips (narrow pages).
const CATEGORY_CHIP_H_PX: f32 = DIM_32_PX;
/// The plugin menu.
const MENU_W_PX: f32 = DIM_240_PX;
const MENU_ITEM_H_PX: f32 = DIM_34_PX;
const MENU_HEAD_H_PX: f32 = DIM_40_PX;
const MENU_SEP_H_PX: f32 = SPACE_9_PX;

impl StudioView {
    fn pool_search(&mut self, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = &self.designer.search {
            return f.clone();
        }
        let view = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search plugins…"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = view.update(cx, |v, cx| {
                        v.designer.query = value;
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        self.designer.search = Some(field.clone());
        field
    }

    /// The pool's ranked order for the current query (cached until the
    /// query or the plugins change).
    fn pool_ranked(&mut self) -> Rc<[usize]> {
        let d = &mut self.designer;
        let query = d.query.trim();
        if let Some((q, s, r)) = &d.ranked
            && q == query
            && *s == d.index.stamp
        {
            return r.clone();
        }
        let matcher = d.matcher.get_or_insert_with(|| Matcher::new(MatcherConfig::DEFAULT));
        let ranked: Rc<[usize]> = rank(&d.index.entries, query, matcher).into();
        d.ranked = Some((query.to_string(), d.index.stamp, ranked.clone()));
        ranked
    }

    /// Opens the plugin menu at the click.
    pub(super) fn open_plugin_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.designer.menu = Some(menu);
        self.designer.appear.open();
        cx.notify();
    }

    fn close_plugin_menu(&mut self, cx: &mut Context<Self>) {
        self.designer.appear.close();
        cx.notify();
    }

    /// The plugin pool (Studio → Bar → Layout designer).
    pub(super) fn plugin_pool(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let registry = cx.global::<PluginRegistry>();
        let current = stamp(registry);
        if self.designer.index.stamp != current || self.designer.index.entries.is_empty() {
            let manifests: Vec<PluginManifest> = registry.manifests().cloned().collect();
            self.designer.index = PoolIndex::build(manifests, current);
        }
        let ranked = self.pool_ranked();
        let search = self.pool_search(cx);
        let vertical = cfg.bar.position.is_vertical();
        let scale = window.scale_factor();
        let registry = cx.global::<PluginRegistry>();
        let entries = &self.designer.index.entries;

        // Removed built-ins stay out (they live under Plugins → Removed).
        let listed: Vec<usize> = ranked.iter().copied().filter(|&i| !cfg.plugins.is_removed(&entries[i].manifest.id)).collect();
        // Category counts follow the search.
        let groups_all = group(entries, &listed);
        let category = self.designer.category.filter(|c| groups_all.iter().any(|(g, _)| g == c));
        let searching = !self.designer.query.trim().is_empty();
        let shown: Vec<usize> = match category {
            Some(c) => listed.iter().copied().filter(|&i| entries[i].category == c).collect(),
            None => listed.clone(),
        };

        // One cell per plugin.
        struct Cell {
            index: usize,
            manifest: PluginManifest,
            name: String,
            enabled: bool,
            lane: Option<Lane>,
        }
        let cell_of = |i: usize| {
            let e = &entries[i];
            Cell { index: i, manifest: e.manifest.clone(), name: e.name.clone(), enabled: registry.is_enabled(&e.manifest.id, &cfg.plugins), lane: zone_of(cfg, &e.manifest.id) }
        };
        let column_w = self.page.column_w;
        let rail = column_w >= POOL_RAIL_FROM_PX;
        let grid_w = column_w - 2.0 * (POOL_PAD_PX + BORDER_1_PX) - if rail { POOL_RAIL_W_PX + POOL_PAD_PX } else { 0.0 };
        let fit = grid::fit(grid_w, POOL_GRID);
        // Grouped by category with headers while browsing everything; a
        // flat, ranked list while searching or inside one category.
        let sections: Vec<(Option<&'static str>, Vec<Cell>)> = if searching || category.is_some() {
            vec![(None, shown.iter().map(|&i| cell_of(i)).collect())]
        } else {
            group(entries, &shown).into_iter().map(|(cat, list)| (Some(cat), list.into_iter().map(cell_of).collect())).collect()
        };
        let total: usize = sections.iter().map(|(_, l)| l.len()).sum();

        let mut body = div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(SPACE_14_PX));
        if total == 0 {
            body = body.child(super::store_page::empty_state("search", t("No plugins match"), t("Try another word or another filter."), p));
        }
        for (cat, cells) in sections {
            if cells.is_empty() {
                continue;
            }
            let count = cells.len();
            let items: Vec<AnyElement> = cells.into_iter().map(|c| self.pool_cell(c.index, &c.manifest, c.name, c.enabled, c.lane, vertical, scale, p, cx)).collect();
            body = body.child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .when_some(cat, |el, cat| {
                        el.child(
                            div()
                                .px(px(SPACE_4_PX))
                                .row()
                                .items_center()
                                .gap(px(SPACE_6_PX))
                                .text_size(px(TEXT_11_5_PX))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.on_surface_variant.hsla())
                                .child(tr(cat))
                                .child(div().text_color(p.on_surface_variant.with_alpha(ALPHA_60).hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(count.to_string())),
                        )
                    })
                    .child(fit.render(items)),
            );
        }

        // Categories: a rail on wide pages, chips on narrow ones.
        let mut cats: Vec<(Option<&'static str>, String, usize)> = vec![(None, t("All").to_string(), listed.len())];
        cats.extend(groups_all.iter().map(|(c, l)| (Some(*c), tr(c), l.len())));
        let picker = if rail {
            let rows = cats.into_iter().enumerate().map(|(n, (cat, label, count))| {
                let active = cat == category;
                div()
                    .id(("designer-cat", n))
                    .h(px(POOL_RAIL_ITEM_H_PX))
                    .px(px(SPACE_10_PX))
                    .rounded(px(RADIUS_10_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .text_size(px(TEXT_12_5_PX))
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                    .text_color(if active { p.primary.hsla() } else { p.on_surface.hsla() })
                    .when(active, |el| el.bg(p.primary.with_alpha(ALPHA_12).hsla()))
                    .when(!active, |el| el.hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla())))
                    .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
                    .child(div().flex_none().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.with_alpha(ALPHA_80).hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(count.to_string()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.designer.category = cat;
                        cx.notify();
                    }))
                    .into_any_element()
            });
            div().w(px(POOL_RAIL_W_PX)).flex_none().row().flex_col().gap(px(SPACE_3_PX)).children(rows).into_any_element()
        } else {
            let chips: Vec<AnyElement> = cats
                .into_iter()
                .enumerate()
                .map(|(n, (cat, label, count))| {
                    let active = cat == category;
                    let fg = if active { p.on_primary } else { p.on_surface };
                    div()
                        .id(("designer-cat-chip", n))
                        .h(px(CATEGORY_CHIP_H_PX))
                        .px(px(SPACE_12_PX))
                        .rounded_full()
                        .row()
                        .items_center()
                        .justify_center()
                        .gap(px(SPACE_6_PX))
                        .cursor_pointer()
                        .text_size(px(TEXT_12_PX))
                        .font_weight(FontWeight::MEDIUM)
                        .bg(if active { p.primary.hsla() } else { p.surface_base.hsla() })
                        .text_color(fg.hsla())
                        .when(!active, |el| el.hover(|s| s.bg(p.surface_container_high.hsla())))
                        .child(div().min_w(px(0.0)).truncate().child(label))
                        .child(div().flex_none().text_color(fg.with_alpha(ALPHA_70).hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(count.to_string()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if !grid::strip("designer-cats").dragged() {
                                this.designer.category = cat;
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                })
                .collect();
            grid::chips(column_w - 2.0 * (POOL_PAD_PX + BORDER_1_PX), "designer-cats", CATEGORY_CHIP_H_PX, chips, p)
        };

        // Title, hint and search.
        let clear = searching.then(|| {
            let field = search.clone();
            div()
                .id("designer-search-clear")
                .size(px(DIM_28_PX))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                .child(icon("close", ICON_12_PX, p.on_surface_variant))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    field.update(cx, |f, cx| {
                        f.set("");
                        f.focus(window, cx);
                    });
                    this.designer.query.clear();
                    cx.notify();
                }))
        });
        let search_box = div()
            .when(!self.stack_rows, |el| el.w(px(POOL_SEARCH_W_PX)).flex_none())
            .row()
            .items_center()
            .gap(px(SPACE_6_PX))
            .child(div().flex_1().min_w(px(0.0)).child(search))
            .children(clear);
        let title = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .gap(px(SPACE_3_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::SEMIBOLD).child(t("All plugins")))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(listed.len().to_string())),
            )
            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Click a plugin to place it, or drag it onto the bar. Drop one here to take it off the bar."))));
        let head = div().row().when(self.stack_rows, |el| el.flex_col()).when(!self.stack_rows, |el| el.items_center()).gap(px(SPACE_12_PX)).child(title).child(search_box);

        let content = if rail { div().row().items_start().gap(px(POOL_PAD_PX)).child(picker).child(body) } else { div().row().flex_col().gap(px(SPACE_12_PX)).child(picker).child(body) };
        div()
            .id("designer-pool")
            .p(px(POOL_PAD_PX))
            .rounded(px(RADIUS_20_PX))
            .bg(p.surface_container.hsla())
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_6).hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            // Lit only for bar items (dropping a pool plugin back does nothing).
            .drag_over::<DragChip>(|s, chip, _, _| if chip.from != Lane::Tray { s.bg(gpui::rgba(0x6aa0ff14)).border_color(gpui::rgba(0x6aa0ffaa)) } else { s })
            // A bar item dropped here comes off the bar.
            .on_drop(|chip: &DragChip, _, cx| {
                if chip.from != Lane::Tray {
                    place(&chip.id, chip.from, Lane::Tray, None, cx);
                }
            })
            .child(head)
            .child(content)
            .into_any_element()
    }

    /// One plugin of the pool: artwork, name and where it sits (or "Off").
    fn pool_cell(&self, index: usize, m: &PluginManifest, name: String, enabled: bool, lane: Option<Lane>, vertical: bool, scale: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = m.id.clone();
        let from = lane.unwrap_or(Lane::Tray);
        let drag = DragChip::new(id.clone(), from, name.clone().into(), plugin_icon(&id), p.primary, p.on_primary);
        let placed = lane.is_some();
        let badge: Option<AnyElement> = match lane {
            Some(l) => Some(
                div()
                    .flex_none()
                    .max_w(px(DIM_110_PX))
                    .h(px(DIM_20_PX))
                    .px(px(SPACE_8_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .bg(p.primary.with_alpha(ALPHA_16).hsla())
                    .text_color(p.primary.hsla())
                    .text_size(px(TEXT_10_5_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().min_w(px(0.0)).truncate().child(tr(zone_title(l, vertical))))
                    .into_any_element(),
            ),
            None if !enabled => Some(div().flex_none().px(px(SPACE_6_PX)).h(px(DIM_20_PX)).row().items_center().rounded(px(RADIUS_6_PX)).bg(p.on_surface.with_alpha(ALPHA_8).hsla()).text_size(px(TEXT_10_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Off")).into_any_element()),
            None => None,
        };
        let menu_name = name.clone();
        div()
            .id(("designer-pool-cell", index))
            .group("pool-cell")
            .h(px(POOL_CELL_H_PX))
            .px(px(SPACE_8_PX))
            .rounded(px(RADIUS_12_PX))
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .cursor_pointer()
            .bg(if placed { p.primary.with_alpha(ALPHA_8).hsla() } else { p.surface_base.hsla() })
            .border_1()
            .border_color(if placed { p.primary.with_alpha(ALPHA_25).hsla() } else { p.on_surface.with_alpha(ALPHA_6).hsla() })
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_60).hsla()))
            .child(plugin_tile(m, POOL_TILE_PX, enabled, scale, p))
            .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).when(!enabled, |el| el.text_color(p.on_surface_variant.hsla())).child(name))
            .children(badge)
            .when(!placed, |el| el.child(div().flex_none().invisible().group_hover("pool-cell", |s| s.visible()).child(icon("plus", ICON_13_PX, p.primary))))
            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                let at = e.position();
                this.open_plugin_menu(Menu { id: id.clone(), name: menu_name.clone(), from, kind: MenuFor::Pool, at: (f32::from(at.x), f32::from(at.y)) }, cx);
            }))
            .on_drag(drag, |chip, _, _, cx| cx.new(|_| chip.clone()))
            .into_any_element()
    }

    /// The open plugin menu, if any (drawn over everything).
    pub(super) fn plugin_menu(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.designer.menu.clone()?;
        let reduce = cfg.motion.reduce_motion;
        let Some(appear) = self.designer.appear.frame(reduce, window) else {
            self.designer.menu = None;
            return None;
        };
        let vertical = cfg.bar.position.is_vertical();
        // Targets: the bar's zones, then the side bars' first group.
        let mut targets = vec![Lane::Left, Lane::Center, Lane::Right];
        if cfg.side_bars.enabled {
            targets.extend([Lane::Side(false, SideZone::Top), Lane::Side(true, SideZone::Top)]);
        }
        let same_place = |a: Lane, b: Lane| match (a, b) {
            (Lane::Side(x, _), Lane::Side(y, _)) => x == y,
            _ => a == b,
        };

        #[derive(Clone, Copy)]
        enum Act {
            Nudge(isize),
            To(Lane),
            Remove,
            Details,
        }
        let mut items: Vec<Option<(&'static str, String, Act, bool)>> = Vec::new();
        if let MenuFor::Zone { index, len } = menu.kind {
            if index > 0 {
                items.push(Some(("arrow-up", t("Move up").to_string(), Act::Nudge(-1), false)));
            }
            if index + 1 < len {
                items.push(Some(("arrow-down", t("Move down").to_string(), Act::Nudge(1), false)));
            }
            if len > 1 {
                items.push(None);
            }
        }
        let placed = menu.from != Lane::Tray;
        for to in targets.into_iter().filter(|to| !same_place(menu.from, *to)) {
            let zone = tr(zone_title(to, vertical));
            let label = if placed { tf("Move to {zone}", &[("zone", &zone)]) } else { tf("Add to {zone}", &[("zone", &zone)]) };
            items.push(Some((if placed { mandara_ui::bidi::dir_icon("chevron-right") } else { "plus" }, label, Act::To(to), false)));
        }
        items.push(None);
        if placed {
            items.push(Some(("close", t("Remove from bar").to_string(), Act::Remove, true)));
        }
        items.push(Some(("info", t("Details").to_string(), Act::Details, false)));

        let rows: Vec<AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(n, item)| match item {
                None => div().h(px(MENU_SEP_H_PX)).row().items_center().px(px(SPACE_8_PX)).child(div().h(px(BORDER_1_PX)).flex_1().bg(p.on_surface.with_alpha(ALPHA_8).hsla())).into_any_element(),
                Some((glyph, label, act, danger)) => {
                    let (id, from) = (menu.id.clone(), menu.from);
                    let fg = if danger { p.urgent_coral } else { p.on_surface };
                    div()
                        .id(("designer-menu-item", n))
                        .h(px(MENU_ITEM_H_PX))
                        .px(px(SPACE_10_PX))
                        .rounded(px(RADIUS_10_PX))
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .cursor_pointer()
                        .text_size(px(TEXT_12_5_PX))
                        .text_color(fg.hsla())
                        .hover(|s| s.bg(if danger { p.urgent_coral.with_alpha(ALPHA_12).hsla() } else { p.primary.with_alpha(ALPHA_12).hsla() }))
                        .child(icon(glyph, ICON_14_PX, if danger { p.urgent_coral } else { p.on_surface_variant }))
                        .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            match act {
                                Act::Nudge(delta) => {
                                    let id = id.clone();
                                    mandara_ui::state::update_config(cx, move |c| {
                                        nudge_in(c, from, &id, delta);
                                    });
                                }
                                Act::To(to) => place(&id, from, to, None, cx),
                                Act::Remove => place(&id, from, Lane::Tray, None, cx),
                                Act::Details => {
                                    this.close_plugin_menu(cx);
                                    this.open_plugin(&id, ModalView::Detail, window, cx);
                                    return;
                                }
                            }
                            this.close_plugin_menu(cx);
                        }))
                        .into_any_element()
                }
            })
            .collect();
        let rows_h: f32 = rows.len() as f32 * MENU_ITEM_H_PX;
        let height = MENU_HEAD_H_PX + rows_h + 2.0 * SPACE_6_PX;
        let rtl = mandara_i18n::language().is_rtl();
        let anchor = Anchor::place((menu.at.0, menu.at.1, 0.0, 0.0), height, f32::from(window.viewport_size().height), rtl);
        let head = div()
            .h(px(MENU_HEAD_H_PX))
            .px(px(SPACE_10_PX))
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .child(icon(plugin_icon(&menu.id), ICON_14_PX, p.primary))
            .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(TEXT_13_PX)).font_weight(FontWeight::SEMIBOLD).child(menu.name.clone()))
            .when(placed, |el| el.child(div().flex_none().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(tr(zone_title(menu.from, vertical)))));
        // (Drawn outside the page, so it takes the reading direction's text
        // alignment itself.)
        let panel = popover::surface(MENU_W_PX, p).p(px(SPACE_6_PX)).text_start().row().flex_col().child(head).children(rows);
        let me = cx.entity().downgrade();
        Some(popover::layer("designer-plugin-menu", anchor, appear, panel, move |_, cx| {
            let _ = me.update(cx, |this, cx| this.close_plugin_menu(cx));
        }))
    }
}

// ── Zones (pure) ─────────────────────────────────────────────────────────

/// Where `id` sits: a bar zone, or a side bar group while side bars are on.
pub(crate) fn zone_of(cfg: &ShellConfig, id: &str) -> Option<Lane> {
    let has = |v: &[String]| v.iter().any(|x| x == id);
    if has(&cfg.bar.left) {
        return Some(Lane::Left);
    }
    if has(&cfg.bar.center) {
        return Some(Lane::Center);
    }
    if has(&cfg.bar.right) {
        return Some(Lane::Right);
    }
    if cfg.side_bars.enabled {
        for right in [false, true] {
            let side = cfg.side_bars.side(right);
            for (zone, list) in [(SideZone::Top, &side.top), (SideZone::Middle, &side.middle), (SideZone::Bottom, &side.bottom)] {
                if has(list) {
                    return Some(Lane::Side(right, zone));
                }
            }
        }
    }
    None
}

/// A zone's name: the bar's ends are its top and bottom when it stands on
/// a side of the screen.
pub(crate) fn zone_title(lane: Lane, vertical: bool) -> &'static str {
    match (lane, vertical) {
        (Lane::Left, false) => tk("Left"),
        (Lane::Center, false) => tk("Center"),
        (Lane::Right, false) => tk("Right"),
        (Lane::Left, true) => tk("Top"),
        (Lane::Center, true) => tk("Middle"),
        (Lane::Right, true) => tk("Bottom"),
        (Lane::Side(false, _), _) => tk("Left side bar"),
        (Lane::Side(true, _), _) => tk("Right side bar"),
        (Lane::Sidebar | Lane::Tab(_), _) => tk("Sidebar"),
        (Lane::Tray, _) => tk("Not placed"),
    }
}

/// Moves `id` one place up (`delta` < 0) or down inside its lane. Whether
/// it moved.
pub(crate) fn nudge_in(c: &mut ShellConfig, lane: Lane, id: &str, delta: isize) -> bool {
    let Some(list) = lane.list(c) else { return false };
    let Some(at) = list.iter().position(|x| x == id) else { return false };
    let Some(to) = at.checked_add_signed(delta).filter(|to| *to < list.len()) else { return false };
    list.swap(at, to);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> PoolIndex {
        PoolIndex::build(
            [
                PluginManifest::builtin("weather", "Weather", "Forecasts in the bar", None),
                PluginManifest::builtin("network", "Network", "Wi-Fi and wired connections", None),
                PluginManifest::builtin("net-speed", "Network speed", "Upload and download rates", None),
                PluginManifest::builtin("prayer-times", "Prayer times", "The day's prayers", None),
                PluginManifest::builtin("audio", "Audio", "Volume and devices", None),
                PluginManifest::builtin("clock", "Clock", "Time and calendar", None),
            ],
            1,
        )
    }

    fn ids(ix: &PoolIndex, order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| ix.entries[i].manifest.id.clone()).collect()
    }

    #[test]
    fn index_is_sorted_by_name() {
        let ix = index();
        assert_eq!(ids(&ix, &(0..ix.entries.len()).collect::<Vec<_>>()), ["audio", "clock", "network", "net-speed", "prayer-times", "weather"]);
    }

    #[test]
    fn bar_index_excludes_desktop_families_and_background_tools() {
        let manifests = mandara_config::desktop_widgets::plugin_ids::ALL.into_iter()
            .chain(["wallhaven", "dock", "cursors", "quick-settings", "power", "weather"])
            .map(|id| PluginManifest::builtin(id, id, "", None));
        let ix = PoolIndex::build(manifests, 1);
        assert_eq!(ids(&ix, &(0..ix.entries.len()).collect::<Vec<_>>()), ["power", "weather"]);
    }

    #[test]
    fn empty_query_lists_everything_in_order() {
        let ix = index();
        let mut m = Matcher::new(MatcherConfig::DEFAULT);
        assert_eq!(rank(&ix.entries, "  ", &mut m), (0..ix.entries.len()).collect::<Vec<_>>());
    }

    #[test]
    fn fuzzy_search_ranks_name_prefixes_first() {
        let ix = index();
        let mut m = Matcher::new(MatcherConfig::DEFAULT);
        assert_eq!(ids(&ix, &rank(&ix.entries, "net", &mut m)), ["network", "net-speed"]);
        assert_eq!(ids(&ix, &rank(&ix.entries, "au", &mut m)).first().map(String::as_str), Some("audio"));
        // A word of the name, then letters inside it.
        assert_eq!(ids(&ix, &rank(&ix.entries, "speed", &mut m)), ["net-speed"]);
        assert_eq!(ids(&ix, &rank(&ix.entries, "ather", &mut m)), ["weather"]);
        // Every word must hit, in any order.
        assert_eq!(ids(&ix, &rank(&ix.entries, "time pray", &mut m)), ["prayer-times"]);
        // Two letters don't light up half the list.
        assert_eq!(ids(&ix, &rank(&ix.entries, "et", &mut m)), ["network", "net-speed"]);
        // Fuzzy, when nothing holds the letters as typed: in order, not contiguous.
        assert_eq!(ids(&ix, &rank(&ix.entries, "wthr", &mut m)).first().map(String::as_str), Some("weather"));
        // The id counts too.
        assert!(ids(&ix, &rank(&ix.entries, "prayer-times", &mut m)).contains(&"prayer-times".to_string()));
        // Nothing for nonsense.
        assert!(rank(&ix.entries, "zzqx", &mut m).is_empty());
    }

    #[test]
    fn descriptions_match_after_names() {
        let ix = index();
        let mut m = Matcher::new(MatcherConfig::DEFAULT);
        let hits = ids(&ix, &rank(&ix.entries, "calendar", &mut m));
        assert_eq!(hits, ["clock"]);
        let hits = ids(&ix, &rank(&ix.entries, "volume", &mut m));
        assert_eq!(hits, ["audio"]);
    }

    #[test]
    fn faith_plugins_have_their_own_category_late_in_the_list() {
        for id in FAITH {
            assert_eq!(category_of(id, false), FAITH_CATEGORY);
        }
        assert_eq!(category_of("weather", false), "Everyday info");
        assert_eq!(category_of("launcher", false), BAR_ESSENTIALS);
        assert_eq!(category_of("anything", true), THIRD_PARTY_CATEGORY);
        assert_eq!(category_of("unknown-builtin", false), MORE_CATEGORY);
        let order: Vec<&str> = category_order().collect();
        let faith = order.iter().position(|c| *c == FAITH_CATEGORY).unwrap();
        // Every store category of built-ins comes first.
        assert_eq!(faith, PLUGIN_CATEGORIES.len() - 1);
        assert_eq!(order.iter().filter(|c| **c == THIRD_PARTY_CATEGORY).count(), 1);
    }

    #[test]
    fn grouping_keeps_order_and_drops_empty_categories() {
        let ix = index();
        let all: Vec<usize> = (0..ix.entries.len()).collect();
        let groups = group(&ix.entries, &all);
        let names: Vec<&str> = groups.iter().map(|(c, _)| *c).collect();
        assert_eq!(names, ["Bar essentials", "System", "Everyday info", "Faith"]);
        let everyday = &groups[2].1;
        assert_eq!(ids(&ix, everyday), ["net-speed", "weather"]);
        // A filtered list groups only what is shown.
        let one = group(&ix.entries, &[0]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].0, "System");
    }

    fn config() -> ShellConfig {
        let mut c = ShellConfig::default();
        c.bar.left = vec!["launcher".into(), "workspaces".into()];
        c.bar.center = vec!["clock".into()];
        c.bar.right = vec!["audio".into(), "network".into(), "session".into()];
        c
    }

    #[test]
    fn nudging_moves_one_place_and_stops_at_the_ends() {
        let mut c = config();
        assert!(nudge_in(&mut c, Lane::Right, "network", -1));
        assert_eq!(c.bar.right, ["network", "audio", "session"]);
        assert!(!nudge_in(&mut c, Lane::Right, "network", -1));
        assert!(nudge_in(&mut c, Lane::Right, "audio", 1));
        assert_eq!(c.bar.right, ["network", "session", "audio"]);
        assert!(!nudge_in(&mut c, Lane::Right, "audio", 1));
        assert!(!nudge_in(&mut c, Lane::Right, "clock", 1));
        assert!(!nudge_in(&mut c, Lane::Tray, "clock", 1));
    }

    #[test]
    fn zones_are_found_and_named() {
        let mut c = config();
        assert_eq!(zone_of(&c, "workspaces"), Some(Lane::Left));
        assert_eq!(zone_of(&c, "clock"), Some(Lane::Center));
        assert_eq!(zone_of(&c, "network"), Some(Lane::Right));
        assert_eq!(zone_of(&c, "weather"), None);
        c.side_bars.left.middle = vec!["weather".into()];
        // Side bar groups count only while side bars are on.
        c.side_bars.enabled = false;
        assert_eq!(zone_of(&c, "weather"), None);
        c.side_bars.enabled = true;
        assert_eq!(zone_of(&c, "weather"), Some(Lane::Side(false, SideZone::Middle)));
        assert_eq!(zone_title(Lane::Left, false), "Left");
        assert_eq!(zone_title(Lane::Left, true), "Top");
        assert_eq!(zone_title(Lane::Right, true), "Bottom");
    }
}
