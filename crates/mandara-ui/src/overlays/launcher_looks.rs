//! The launcher's looks beyond the plain list (`[launcher.look]`, Studio →
//! Launcher → Launcher looks):
//!
//! * **Grid** (`layout = "grid"`): app icons with their names under them,
//!   as many columns as fit the launcher's width (`grid_columns` caps it,
//!   `grid_icon_size` sizes the icons), a row of category chips (All,
//!   Pinned, Internet, Office… from the apps' `Categories=`) above it.
//!   Arrows move in two dimensions, Tab / Shift+Tab switch the category,
//!   typing filters with the same fuzzy ranking and the tiles reflow with
//!   a spring (each tile slides from where it was; new ones fade in). Rows
//!   are virtualized (`uniform_list`): only visible tiles lay out and
//!   decode their icons.
//! * **Grouped results** (`group_results`): a Top hit, then headings by
//!   kind (Applications, Actions & settings, Files, Windows…) in the order
//!   their best result ranks; only the visible rows are built.
//! * **Beacon** (`style = "beacon"`): a centred glass search bar in the
//!   upper part of the screen with round mode buttons at its reading end
//!   (Apps, Files, Actions, Clipboard: Ctrl+1–4), and the results in their
//!   own glass card under it once you type. The Apps mode shows the grid.
//!   It pops in with the panels' spring (a slight scale and fade), the mode
//!   buttons slide out from behind the bar one after another, and the card
//!   grows to its rows with a stiffer spring.
//!
//! Every icon drawn here is recorded in the launcher's
//! [`crate::launcher_icons::IconLedger`] (bounded while open, all freed on
//! close).

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, ScrollEvent, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
    uniform_list,
};
use mandara_config::workflow::{LauncherProviders, LauncherStyle};
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tk};
use mandara_theme::{Color, MandaraPalette};

use super::{Item, LauncherView, Run, format_number};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::text;
use crate::launcher_place::{BEACON_BAR_H, BEACON_GAP, Rect};
use crate::plugins::panel_fx::readable;

// ── Geometry (logical px) ───────────────────────────────────────────────

/// Grid tiles: room beside the icon, above it, between it and its name,
/// the name's two lines and the room under them.
const TILE_PAD_X: f32 = 16.0;
const TILE_PAD_TOP: f32 = 10.0;
const NAME_GAP: f32 = 8.0;
const NAME_LINE_H: f32 = 15.0;
const NAME_LINES: usize = 2;
const TILE_PAD_BOTTOM: f32 = 8.0;
const TILE_RADIUS: f32 = 16.0;
/// Column bounds of the grid.
pub(super) const GRID_COLUMNS_MIN: usize = 3;
pub(super) const GRID_COLUMNS_MAX: usize = 10;
/// A reflowing tile slides at most this many rows (farther ones arrive
/// from just above or below instead of flying across the grid).
const REFLOW_MAX_ROWS: f32 = 1.5;
/// The reflow: quick, a hint of overshoot.
pub(super) const REFLOW_SPRING: SpringConfig = SpringConfig::new(420.0, 32.0, 1.0);
/// Category chips row.
pub(super) const CHIPS_H: f32 = 30.0;
const CHIP_PAD: f32 = 12.0;
const CHIP_GAP: f32 = 6.0;
/// Section headings of grouped results.
pub(super) const HEADER_H: f32 = 28.0;
const HEADER_PAD_X: f32 = 12.0;
const HEADER_PAD_BOTTOM: f32 = 4.0;
/// Keyboard hints strip.
pub(super) const HINTS_H: f32 = 30.0;
const KEYCAP_H: f32 = 20.0;
const KEYCAP_PAD: f32 = 6.0;
/// Gap between the caps of one chord.
const KEYCAP_GAP: f32 = 3.0;
const HINT_GAP: f32 = 14.0;
/// Beacon: bar padding, glyph and type sizes, card padding and radius.
const BAR_PAD_X: f32 = 18.0;
const BAR_ICON: f32 = 21.0;
pub(super) const BAR_TEXT: f32 = 20.0;
const MODE_ICON: f32 = 20.0;
const TOKEN_H: f32 = 30.0;
const TOKEN_PAD_X: f32 = 10.0;
const TOKEN_GAP: f32 = 6.0;
/// Gap between the bar glyph, mode token and text.
const BAR_GAP: f32 = 12.0;
/// Base type size before the typography scale (as the Classic launcher).
const BASE_TEXT: f32 = 13.0;
const CARD_PAD: f32 = 8.0;
const CARD_RADIUS: f32 = 24.0;
const ROW_RADIUS: f32 = 12.0;
/// Beacon open: the bar starts at this scale and this far above its rest.
const POP_SCALE: f32 = 0.94;
const POP_DROP_PX: f32 = 8.0;
/// Each mode button starts sliding out this much later in the open
/// progress than the one before it.
const MODE_STAGGER: f32 = 0.08;
/// The results card grows with a stiffer spring than the bar pops.
pub(super) const CARD_SPRING: SpringConfig = SpringConfig::new(560.0, 44.0, 1.0);
/// Selected-row and hover fills.
const SELECTED_ALPHA: f32 = 0.16;
const HOVER_ALPHA: f32 = 0.07;
const ROW_SELECTED_ALPHA: f32 = 0.92;
pub(super) const SUB_ON_ACCENT_ALPHA: f32 = 0.78;
pub(super) const BADGE_ON_ACCENT_ALPHA: f32 = 0.18;

// ── Grid ────────────────────────────────────────────────────────────────

/// The grid's columns and tile size for a width.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct GridGeom {
    pub cols: usize,
    pub cell_w: f32,
    pub tile_h: f32,
    pub icon: f32,
}

/// As many columns as `avail_w` fits at `icon` size (between
/// [`GRID_COLUMNS_MIN`] and `max_cols`), the tiles widened to fill the row.
pub(super) fn grid_geom(avail_w: f32, icon: f32, max_cols: u32) -> GridGeom {
    let min_w = icon + TILE_PAD_X * 2.0;
    let cap = (max_cols as usize).clamp(GRID_COLUMNS_MIN, GRID_COLUMNS_MAX);
    let cols = ((avail_w.max(0.0) / min_w).floor() as usize).clamp(GRID_COLUMNS_MIN, cap);
    GridGeom { cols, cell_w: avail_w.max(0.0) / cols as f32, tile_h: TILE_PAD_TOP + icon + NAME_GAP + NAME_LINE_H * NAME_LINES as f32 + TILE_PAD_BOTTOM, icon }
}

/// A plain search (no provider prefix, `/` or `=`): the grid stays.
pub(super) fn is_plain(query: &str, providers: &LauncherProviders) -> bool {
    let q = query.trim();
    !q.starts_with('/') && !q.starts_with('=') && crate::launcher_ext::route(q, providers).is_none()
}

/// The selection step an arrow key makes in a grid of `cols` (left and
/// right swap in right-to-left languages, where the rows run the other
/// way).
pub(super) fn grid_step(key: &str, cols: usize, rtl: bool) -> Option<isize> {
    let forward = if rtl { "left" } else { "right" };
    match key {
        "up" => Some(-(cols as isize)),
        "down" => Some(cols as isize),
        k if k == forward => Some(1),
        "left" | "right" => Some(-1),
        _ => None,
    }
}

/// Where tile `slot` sits in the grid, physically (`(x, y)`).
fn slot_xy(slot: usize, g: &GridGeom, rtl: bool) -> (f32, f32) {
    let col = slot % g.cols;
    let col = if rtl { g.cols - 1 - col } else { col };
    (col as f32 * g.cell_w, (slot / g.cols) as f32 * g.tile_h)
}

/// A reflowing tile's offset from its new slot and its opacity at reflow
/// progress `t` (0 → 1): it slides from its old slot (at most
/// [`REFLOW_MAX_ROWS`] away), a new tile fades in.
pub(super) fn reflow(prev: Option<usize>, now: usize, g: &GridGeom, rtl: bool, t: f32) -> (f32, f32, f32) {
    let k = 1.0 - t;
    match prev {
        Some(p) if p != now => {
            let (a, b) = (slot_xy(p, g, rtl), slot_xy(now, g, rtl));
            let reach = g.tile_h * REFLOW_MAX_ROWS;
            ((a.0 - b.0) * k, (a.1 - b.1).clamp(-reach, reach) * k, 1.0)
        }
        Some(_) => (0.0, 0.0, 1.0),
        None => (0.0, 0.0, t.clamp(0.0, 1.0)),
    }
}

/// A category chip above the grid.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum GridFilter {
    Pinned,
    /// A display group of `crate::plugins::dock::applications::group_of`.
    Group(&'static str),
}

impl GridFilter {
    fn label(self) -> SharedString {
        match self {
            GridFilter::Pinned => t("Pinned").into(),
            GridFilter::Group(g) => t(g).into(),
        }
    }

    pub(super) fn keeps(self, item: &Item, pinned: &[String]) -> bool {
        match (self, item) {
            (GridFilter::Pinned, Item::App(a)) => pinned.iter().any(|p| p.eq_ignore_ascii_case(&a.id)),
            (GridFilter::Group(g), Item::App(a)) => crate::plugins::dock::applications::group_of(&a.categories) == g,
            _ => false,
        }
    }
}

/// The chips for these apps: Pinned (when any is), then each category
/// present, in the Applications folder's order.
pub(super) fn categories(apps: &[crate::apps::App], pinned: &[String]) -> Vec<GridFilter> {
    let mut groups: Vec<&'static str> = apps.iter().map(|a| crate::plugins::dock::applications::group_of(&a.categories)).collect();
    let order = |g: &&'static str| crate::plugins::dock::applications::group_rank(g);
    groups.sort_by_key(order);
    groups.dedup();
    let any_pinned = apps.iter().any(|a| pinned.iter().any(|p| p.eq_ignore_ascii_case(&a.id)));
    any_pinned.then_some(GridFilter::Pinned).into_iter().chain(groups.into_iter().map(GridFilter::Group)).collect()
}

/// The next category (`None` = All) cycling forward or back.
pub(super) fn cycle(current: Option<GridFilter>, all: &[GridFilter], back: bool) -> Option<GridFilter> {
    let n = all.len() + 1;
    let at = current.and_then(|c| all.iter().position(|x| *x == c)).map_or(0, |i| i + 1);
    let next = if back { (at + n - 1) % n } else { (at + 1) % n };
    next.checked_sub(1).map(|i| all[i])
}

// ── Grouped results ─────────────────────────────────────────────────────

/// The heading a result is listed under, and its group's id.
fn group_of(item: &Item) -> &'static str {
    match item {
        Item::App(_) => tk("Applications"),
        Item::Calc { .. } => tk("Calculator"),
        Item::Hit(h) => match h.provider {
            "windows" => tk("Windows"),
            "files" => tk("Files"),
            "clipboard" => tk("Clipboard"),
            "emoji" => tk("Emoji"),
            "wallpapers" => tk("Wallpapers"),
            _ => tk("App actions"),
        },
        Item::Entry(e) => match e.run {
            Run::Slash { .. } => tk("Web & shortcuts"),
            Run::Skill(_) => tk("Skills"),
            Run::Command { .. } => tk("Actions & settings"),
        },
    }
}

/// Top hit heading.
pub(super) const TOP_HIT: &str = tk("Top hit");

/// Ranked results regrouped under headings: the best one alone as the Top
/// hit (with `top_hit` and at least two results), then each kind in the
/// order its best result ranks, ranked within. Returns the results in
/// display order and `(first index, heading)` per section.
pub(super) fn group_results(found: Vec<Item>, top_hit: bool) -> (Vec<Item>, Vec<(usize, &'static str)>) {
    let mut out = Vec::with_capacity(found.len());
    let mut sections = Vec::new();
    let mut rest = found.into_iter();
    if top_hit && rest.len() >= 2
        && let Some(first) = rest.next()
    {
        sections.push((0, TOP_HIT));
        out.push(first);
    }
    let mut order: Vec<&'static str> = Vec::new();
    let mut groups: HashMap<&'static str, Vec<Item>> = HashMap::new();
    for item in rest {
        let g = group_of(&item);
        if !order.contains(&g) {
            order.push(g);
        }
        groups.entry(g).or_default().push(item);
    }
    for g in order {
        sections.push((out.len(), g));
        out.extend(groups.remove(g).unwrap_or_default());
    }
    (out, sections)
}

/// Where each row of a grouped list sits: item tops, heading tops and the
/// total height.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ListLayout {
    pub item_y: Vec<f32>,
    pub headers: Vec<(f32, &'static str)>,
    pub total: f32,
}

pub(super) fn list_layout(n: usize, sections: &[(usize, &'static str)], row_h: f32, header_h: f32) -> ListLayout {
    let mut out = ListLayout { item_y: Vec::with_capacity(n), ..Default::default() };
    let mut y = 0.0;
    let mut next = sections.iter().peekable();
    for i in 0..n {
        while let Some((first, label)) = next.peek().copied() {
            if *first > i {
                break;
            }
            out.headers.push((y, *label));
            y += header_h;
            next.next();
        }
        out.item_y.push(y);
        y += row_h;
    }
    out.total = y;
    out
}

/// The scroll offset keeping the row at `[top, top + row_h)` in a view of
/// `view_h` (with its heading, `above` px, when scrolling up to it).
pub(super) fn follow(scroll: f32, top: f32, row_h: f32, above: f32, view_h: f32, total: f32) -> f32 {
    let max = (total - view_h).max(0.0);
    let s = if top - above < scroll {
        top - above
    } else if top + row_h > scroll + view_h {
        top + row_h - view_h
    } else {
        scroll
    };
    s.clamp(0.0, max)
}

// ── Beacon modes ────────────────────────────────────────────────────────

/// A Beacon browse mode (Ctrl+1–4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum BeaconMode {
    Apps,
    Files,
    Actions,
    Clipboard,
}

impl BeaconMode {
    pub(super) const ALL: [BeaconMode; 4] = [BeaconMode::Apps, BeaconMode::Files, BeaconMode::Actions, BeaconMode::Clipboard];

    fn label(self) -> &'static str {
        match self {
            BeaconMode::Apps => tk("Apps"),
            BeaconMode::Files => tk("Files"),
            BeaconMode::Actions => tk("Actions"),
            BeaconMode::Clipboard => tk("Clipboard"),
        }
    }

    /// The search field's placeholder in this mode.
    fn placeholder(self) -> &'static str {
        match self {
            BeaconMode::Apps => tk("Search apps"),
            BeaconMode::Files => tk("Search files"),
            BeaconMode::Actions => tk("Search actions and settings"),
            BeaconMode::Clipboard => tk("Search what you copied"),
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            BeaconMode::Apps => "grid",
            BeaconMode::Files => "folder",
            BeaconMode::Actions => "zap",
            BeaconMode::Clipboard => "clipboard",
        }
    }

    /// The provider prefix it searches with (`None` for Apps, which
    /// filters plain results).
    pub(super) fn prefix(self, providers: &LauncherProviders) -> Option<String> {
        let p = match self {
            BeaconMode::Apps => return None,
            BeaconMode::Files => &providers.files,
            BeaconMode::Actions => &providers.actions,
            BeaconMode::Clipboard => &providers.clipboard,
        };
        Some(p.prefix.clone())
    }

    /// Offered: Apps always, the others while their provider is on.
    pub(super) fn available(self, providers: &LauncherProviders) -> bool {
        self.prefix(providers).is_none_or(|p| !p.is_empty())
            && match self {
                BeaconMode::Apps => true,
                BeaconMode::Files => providers.files.enabled,
                BeaconMode::Actions => providers.actions.enabled,
                BeaconMode::Clipboard => providers.clipboard.enabled,
            }
    }

    /// The mode Ctrl+`digit` picks.
    pub(super) fn from_digit(digit: &str) -> Option<Self> {
        digit.parse::<usize>().ok().and_then(|d| d.checked_sub(1)).and_then(|i| Self::ALL.get(i).copied())
    }
}

// ── Shared pieces ───────────────────────────────────────────────────────

/// A result's title (grid tiles).
pub(super) fn title_of(item: &Item) -> String {
    match item {
        Item::App(a) => a.name.clone(),
        Item::Entry(e) => e.label.clone(),
        Item::Hit(h) => h.label.clone(),
        Item::Calc { value, .. } => format_number(*value),
    }
}

/// One keyboard hint: key caps and what they do.
fn hint(keys: &[&str], what: &'static str, palette: &MandaraPalette) -> AnyElement {
    let cap = |k: &str| {
        div()
            .h(px(KEYCAP_H))
            .min_w(px(KEYCAP_H))
            .px(px(KEYCAP_PAD))
            .rounded(px(crate::style::radius(KEYCAP_H * 0.3)))
            .flex()
            .items_center()
            .justify_center()
            .bg(crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_STRONG, palette).hsla())
            .text_color(palette.on_surface.hsla())
            .font_weight(FontWeight::MEDIUM)
            .child(k.to_string())
    };
    div()
        .row()
        .items_center()
        .gap(px(KEYCAP_PAD))
        .child(div().ltr_row().gap(px(KEYCAP_GAP)).children(keys.iter().map(|k| cap(k))))
        .child(t(what))
        .into_any_element()
}

/// The keyboard hints strip for this state.
fn hints(grid: bool, chips: bool, beacon: bool, palette: &MandaraPalette) -> AnyElement {
    let mut row = div().h(px(HINTS_H)).w_full().row().items_center().justify_center().gap(px(HINT_GAP)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla());
    row = row.child(if grid { hint(&["←", "↑", "↓", "→"], tk("Move"), palette) } else { hint(&["↑", "↓"], tk("Move"), palette) });
    row = row.child(hint(&["Enter"], tk("Open"), palette));
    if grid && chips {
        row = row.child(hint(&["Tab"], tk("Category"), palette));
    }
    if beacon {
        row = row.child(hint(&["Ctrl", "1–4"], tk("Modes"), palette));
    }
    row.child(hint(&["Esc"], tk("Close"), palette)).into_any_element()
}

/// A glass surface: the panels' fill (its opacity setting), an even
/// hairline edge, the specular rim and the panels' elevation.
fn glass(cfg: &mandara_config::ShellConfig, palette: &MandaraPalette, progress: f32, radius: f32) -> gpui::Div {
    let frame = crate::shell::frame_color(palette);
    let alpha = mandara_config::surfaces::ranges::OPACITY.clamp(cfg.panels.opacity, 1.0);
    let mut shadow = crate::plugins::panel_fx::elevation_scaled(palette, progress, cfg.panels.shadow_strength);
    shadow.extend(crate::plugins::panel_fx::rim(palette));
    div()
        .rounded(px(radius))
        .bg(frame.with_alpha(frame.alpha() * alpha).hsla())
        .border_1()
        .border_color(crate::plugins::panel_fx::surface_edge(palette, cfg.theme.adjustments.rim_intensity).hsla())
        .shadow(shadow)
}

impl LauncherView {
    // ── Grid ────────────────────────────────────────────────────────────

    /// The grid's rows `range` (the `uniform_list` asks for visible ones).
    fn grid_rows(&mut self, range: Range<usize>, g: GridGeom, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = mandara_core::frame_palette(window, cx);
        let rtl = crate::bidi::rtl();
        let t = self.reflow.value();
        let accent = readable(palette.primary, &palette);
        let ignore_mouse = self.look.ignore_mouse;
        let mut out = Vec::with_capacity(range.len());
        for r in range {
            let mut row = div().row().w_full().h(px(g.tile_h));
            for i in r * g.cols..((r + 1) * g.cols).min(self.results.len()) {
                let item = self.results[i].clone();
                let prev = self.reflow_from.get(&super::usage_key(&item)).copied();
                let (dx, dy, opacity) = reflow(prev, i, &g, rtl, t);
                let selected = i == self.selected;
                let lead = self.lead(&item, g.icon, &palette, cx);
                let hover = cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && !ignore_mouse && this.selected != i {
                        this.selected = i;
                        cx.notify();
                    }
                });
                row = row.child(
                    div()
                        .id(("tile", i))
                        .relative()
                        .left(px(dx))
                        .top(px(dy))
                        .opacity(opacity)
                        .w(px(g.cell_w))
                        .h(px(g.tile_h))
                        .flex()
                        .flex_col()
                        .items_center()
                        .pt(px(TILE_PAD_TOP))
                        .gap(px(NAME_GAP))
                        .rounded(px(crate::style::radius(TILE_RADIUS)))
                        .cursor_pointer()
                        .when(selected, |el| el.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
                        .when(!selected, |el| el.hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla())))
                        .child(lead)
                        .child(
                            div()
                                .w_full()
                                .px(px(CHIP_GAP))
                                .text_center()
                                .line_clamp(NAME_LINES)
                                .text_ellipsis()
                                .text_size(px(text::BODY_S))
                                .line_height(px(NAME_LINE_H))
                                .when(selected, |el| el.text_color(accent.hsla()))
                                .child(title_of(&item)),
                        )
                        .on_hover(hover)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.activate(i, cx))),
                );
            }
            out.push(row.into_any_element());
        }
        out
    }

    /// The virtualized grid, `height` tall.
    pub(super) fn grid_element(&mut self, g: GridGeom, height: f32, palette: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if self.results.is_empty() {
            return div().h(px(height)).flex().items_center().justify_center().text_color(palette.on_surface_variant.hsla()).child(t("No results")).into_any_element();
        }
        let rows = self.results.len().div_ceil(g.cols);
        uniform_list("launcher-grid", rows, cx.processor(move |this, range: Range<usize>, window, cx| this.grid_rows(range, g, window, cx)))
            .track_scroll(&self.grid_scroll)
            .h(px(height))
            .w_full()
            .into_any_element()
    }

    /// The category chips above the grid.
    pub(super) fn chips_element(&self, palette: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let accent = readable(palette.primary, palette);
        let on_accent = accent.best_on(Color::hex("#ffffff"), Color::hex("#101010"));
        let chip = |id: SharedString, label: SharedString, on: bool| {
            div()
                .id(id)
                .h(px(CHIPS_H))
                .px(px(CHIP_PAD))
                .rounded_full()
                .flex()
                .items_center()
                .flex_none()
                .cursor_pointer()
                .text_size(px(text::BODY_S))
                .when(on, |el| el.bg(accent.hsla()).text_color(on_accent.hsla()).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |el| el.bg(crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_SOFT, palette).hsla()).hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla())))
                .child(label)
        };
        let mut row = div().id("launcher-chips").h(px(CHIPS_H)).flex_none().row().gap(px(CHIP_GAP)).overflow_x_scroll();
        row = row.child(chip("launcher-chip-all".into(), t("All").into(), self.category.is_none()).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_category(None, cx))));
        for (n, f) in self.categories.iter().copied().enumerate() {
            row = row.child(chip(SharedString::from(format!("launcher-chip-{n}")), f.label(), self.category == Some(f)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let next = if this.category == Some(f) { None } else { Some(f) };
                this.set_category(next, cx);
            })));
        }
        row.into_any_element()
    }

    pub(super) fn set_category(&mut self, category: Option<GridFilter>, cx: &mut Context<Self>) {
        self.category = category;
        self.refilter();
        cx.notify();
    }

    /// Scrolls the grid to the selected tile's row.
    pub(super) fn reveal_selected(&self) {
        if let Some(row) = self.selected.checked_div(self.grid_cols) {
            self.grid_scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    // ── Grouped list ────────────────────────────────────────────────────

    /// The grouped results in a `view_h` window scrolled by
    /// [`Self::list_scroll`]: only rows in view are built. `beacon`: the
    /// selected row is filled with the accent.
    pub(super) fn grouped_element(&mut self, lay: &ListLayout, view_h: f32, row_h: f32, lead_px: f32, beacon: bool, palette: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let scroll = self.list_scroll.value();
        let look = self.look.clone();
        let mut list = div().relative().h(px(view_h)).w_full().overflow_hidden();
        if self.results.is_empty() {
            return list.flex().items_center().justify_center().text_color(palette.on_surface_variant.hsla()).child(t("No results")).into_any_element();
        }
        // The highlight springs between rows.
        let fill = if beacon { readable(palette.primary, palette).with_alpha(ROW_SELECTED_ALPHA) } else { palette.on_surface.with_alpha(0.08) };
        list = list.child(div().absolute().left_0().right_0().top(px(self.highlight.value() - scroll)).h(px(row_h)).rounded(px(crate::style::radius(ROW_RADIUS))).bg(fill.hsla()));
        for (y, label) in lay.headers.iter().copied() {
            if y + HEADER_H < scroll || y > scroll + view_h {
                continue;
            }
            list = list.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(y - scroll))
                    .h(px(HEADER_H))
                    .px(px(HEADER_PAD_X))
                    .flex()
                    .items_end()
                    .pb(px(HEADER_PAD_BOTTOM))
                    .text_size(px(text::LABEL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.on_surface_variant.hsla())
                    .child(div().w_full().text_start().child(t(label))),
            );
        }
        let visible: Vec<usize> = (0..self.results.len()).filter(|i| lay.item_y.get(*i).is_some_and(|y| y + row_h > scroll && *y < scroll + view_h)).collect();
        for i in visible {
            let item = self.results[i].clone();
            let selected = beacon && i == self.selected;
            let row = self.list_row(i, &item, row_h, lead_px, &look, selected, palette, cx);
            list = list.child(div().absolute().left_0().right_0().top(px(lay.item_y[i] - scroll)).child(row));
        }
        list.into_any_element()
    }

    // ── Beacon ──────────────────────────────────────────────────────────

    /// The modes offered right now, in order.
    pub(super) fn beacon_modes(&self) -> Vec<BeaconMode> {
        BeaconMode::ALL.into_iter().filter(|m| m.available(&self.ctx.providers)).collect()
    }

    /// Ctrl+digit / a mode button: toggles that mode.
    pub(super) fn toggle_mode(&mut self, mode: BeaconMode, cx: &mut Context<Self>) {
        if !mode.available(&self.ctx.providers) {
            return;
        }
        self.mode = if self.mode == Some(mode) { None } else { Some(mode) };
        self.refilter();
        cx.notify();
    }

    /// Whether the Beacon results card shows (something typed, or a mode).
    pub(super) fn beacon_open_results(&self) -> bool {
        self.look.style == LauncherStyle::Beacon && (!self.query.trim().is_empty() || self.mode.is_some())
    }

    pub(super) fn render_beacon(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use mandara_config::surfaces::PanelAnimation;
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let look = self.look.clone();
        let viewport = window.viewport_size();
        let (sw, sh) = (f32::from(viewport.width), f32::from(viewport.height));
        let rtl = crate::bidi::rtl();
        let modes = self.beacon_modes();
        let bar_here = self.output.as_deref().is_none_or(|o| cfg.bar_on_output(o));
        let place = crate::launcher_place::beacon(&cfg, (sw, sh), bar_here, modes.len(), rtl);
        let accent = readable(palette.primary, &palette);

        // What the card holds and how tall it is.
        let show = self.beacon_open_results();
        let grid = self.grid_active();
        let row_h = if look.compact { super::ROW_H_COMPACT } else { super::ROW_H };
        let lead_px = if look.compact { super::ROW_LEAD_COMPACT_PX } else { super::ROW_LEAD_PX };
        let hints_h = if look.hints { HINTS_H } else { 0.0 };
        let chips = grid && look.grid_categories && !self.categories.is_empty();
        let room = (place.card.h - CARD_PAD * 2.0 - hints_h).max(row_h);
        let inner_w = place.card.w - CARD_PAD * 2.0;
        let g = grid_geom(inner_w, look.grid_icon(), look.grid_columns);
        self.grid_cols = g.cols;
        let lay = list_layout(self.results.len(), &self.sections, row_h, HEADER_H);
        let content_h = if grid {
            let chips_h = if chips { CHIPS_H + CARD_PAD } else { 0.0 };
            let rows = self.results.len().div_ceil(g.cols).max(1);
            chips_h + ((((room - chips_h) / g.tile_h).floor() as usize).max(1).min(rows)) as f32 * g.tile_h
        } else {
            lay.total.max(row_h).min(room)
        };
        let list_h = if grid { content_h - if chips { CHIPS_H + CARD_PAD } else { 0.0 } } else { content_h };
        let card_target = if show { CARD_PAD * 2.0 + content_h + hints_h } else { 0.0 };

        // Springs.
        let (spring, custom) = crate::shell::spring_for(super::DRAWER_SPRING, &cfg.panels.spring);
        let instant = cfg.panels.animation == PanelAnimation::None;
        if !self.placed {
            tracing::debug!(target: "mandara::launcher", "first frame {:.1} ms after opening (beacon on {:?})", self.created.elapsed().as_secs_f32() * 1000.0, self.output);
            // The card (with a remembered query) grows in after the bar.
            self.open.snap_to(0.0);
            self.card_h.snap_to(0.0);
            self.placed = true;
        }
        self.open.set_config(if self.closing { SpringConfig::critical(super::CLOSE_STIFFNESS) } else { spring });
        self.open.set_instant(instant);
        self.open.set_target(if self.closing { 0.0 } else { 1.0 });
        self.card_h.set_instant(instant);
        self.card_h.set_target(card_target);
        if !grid
            && let Some(&top) = lay.item_y.get(self.selected)
        {
            let above = if lay.headers.iter().any(|(y, _)| (top - HEADER_H - y).abs() < 0.5) { HEADER_H } else { 0.0 };
            let target = follow(self.list_scroll.target(), top, row_h, above, list_h, lay.total);
            self.list_scroll.set_target(target);
            self.highlight.set_target(top);
        }
        let dt = self.clock.tick(std::time::Instant::now());
        let opening = crate::shell::step_spring(&mut self.open, custom, dt);
        let a = self.card_h.step(dt);
        let b = self.highlight.step(dt);
        let c = self.list_scroll.step(dt);
        let d = self.reflow.step(dt);
        if self.closing && !opening {
            self.release(window, cx);
            window.remove_window();
        } else if opening || a || b || c || d {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let p = self.open.value();
        let progress = p.clamp(0.0, 1.0);

        // The bar pops in: a little smaller and higher, fading in.
        let k = POP_SCALE + (1.0 - POP_SCALE) * p;
        let lift = POP_DROP_PX * (1.0 - p);
        let grow = |r: Rect| Rect::new(r.x + r.w * (1.0 - k) / 2.0, r.y - lift + r.h * (1.0 - k) / 2.0, r.w * k, r.h * k);
        let bar = grow(place.bar);
        self.rect = (place.card.x, place.bar.y, place.card.w, place.bar.h + BEACON_GAP + self.card_h.value());

        // ── The search bar ──
        self.field_px = BAR_TEXT;
        let placeholder = t(self.mode.map_or(tk("Search apps, files and actions"), BeaconMode::placeholder)).to_string();
        let token = self.mode.map(|m| {
            div()
                .h(px(TOKEN_H))
                .px(px(TOKEN_PAD_X))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .gap(px(TOKEN_GAP))
                .bg(accent.with_alpha(SELECTED_ALPHA).hsla())
                .text_color(accent.hsla())
                .text_size(px(text::BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon(m.glyph(), text::BODY, accent))
                .child(t(m.label()))
        });
        let field = self.query_field(BAR_TEXT, placeholder, &palette);
        let bar_el = glass(&cfg, &palette, progress, bar.h / 2.0)
            .id("beacon-bar")
            .absolute()
            .left(px(bar.x))
            .top(px(bar.y))
            .w(px(bar.w))
            .h(px(bar.h))
            .opacity(progress)
            .px(px(BAR_PAD_X))
            .row()
            .items_center()
            .gap(px(BAR_GAP))
            .cursor_text()
            .child(icon("search", BAR_ICON, palette.on_surface_variant))
            .children(token)
            .child(field)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                if e.click_count >= 2 {
                    this.select_all = !this.query.is_empty();
                } else {
                    this.caret_at(e.position.x, window, cx);
                }
                cx.notify();
            }));

        // ── Mode buttons: slide out from behind the bar, one by one ──
        let mut buttons: Vec<AnyElement> = Vec::new();
        for (n, m) in modes.iter().copied().enumerate() {
            let rest = Rect::new(place.first_mode.x + place.mode_step * n as f32, place.first_mode.y, BEACON_BAR_H, BEACON_BAR_H);
            // Its share of the open progress: starts later, rests at 1 and
            // carries the spring's overshoot.
            let start = MODE_STAGGER * (n as f32 + 1.0);
            let local = ((p - start) / (1.0 - start)).max(0.0);
            let from = place.first_mode.x - place.mode_step;
            let x = from + (rest.x - from) * local;
            let r = grow(Rect::new(x, rest.y, rest.w, rest.h));
            let on = self.mode == Some(m);
            let (fill_fg, label) = (if on { accent.best_on(Color::hex("#ffffff"), Color::hex("#101010")) } else { palette.on_surface }, m.label());
            let el = glass(&cfg, &palette, progress, r.h / 2.0)
                .id(SharedString::from(format!("beacon-mode-{}", label.to_lowercase())))
                .absolute()
                .left(px(r.x))
                .top(px(r.y))
                .size(px(r.w))
                .opacity(local.clamp(0.0, 1.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(on, |el| el.bg(accent.hsla()))
                .when(!on, |el| el.hover(|s| s.bg(palette.surface_container_high.hsla())))
                .child(icon(m.glyph(), MODE_ICON, fill_fg))
                .occlude()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_mode(m, cx)));
            buttons.push(el.into_any_element());
        }

        // ── The results card ──
        let card_h = self.card_h.value().max(0.0);
        let card_el = (card_h > 0.5).then(|| {
            let reveal = if card_target > 0.0 { (card_h / card_target).clamp(0.0, 1.0) } else { (card_h / (CARD_PAD * 2.0 + row_h)).clamp(0.0, 1.0) };
            let body = if grid {
                let grid_el = self.grid_element(g, list_h, &palette, cx);
                div().flex().flex_col().gap(px(CARD_PAD)).when(chips, |el| el.child(self.chips_element(&palette, cx))).child(grid_el).into_any_element()
            } else {
                self.grouped_element(&lay, list_h, row_h, lead_px, true, &palette, cx)
            };
            let wheel_row = row_h;
            let card = place.card;
            glass(&cfg, &palette, progress, crate::style::radius(CARD_RADIUS))
                .id("beacon-card")
                .absolute()
                .left(px(card.x))
                .top(px(card.y - lift))
                .w(px(card.w))
                .h(px(card_h))
                .opacity(progress)
                .overflow_hidden()
                .occlude()
                .on_scroll(cx.listener(move |this, e: &ScrollEvent, _, cx| {
                    if this.grid_active() {
                        return;
                    }
                    this.wheel_select(e, wheel_row, 1, cx);
                }))
                .on_mouse_move(cx.listener(|this, _: &gpui::MouseMoveEvent, _, _| this.wheel_hold = false))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(px(card.w))
                        .h(px(card_target.max(card_h)))
                        .p(px(CARD_PAD))
                        .flex()
                        .flex_col()
                        .opacity(reveal)
                        .child(body)
                        .when(look.hints, |el| el.child(hints(grid, chips, true, &palette))),
                )
                .into_any_element()
        });

        let dim = look.dim.clamp(0.0, super::DIM_MAX) * progress;
        div()
            .id("launcher-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_start()
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            .text_size(px(BASE_TEXT * cfg.typography.scale))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .child(
                div()
                    .id("launcher-outside")
                    .absolute()
                    .inset_0()
                    .when(dim > 0.0, |el| el.bg(gpui::black().opacity(dim)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close(cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close(cx))),
            )
            .children(card_el)
            .child(bar_el.occlude())
            .children(buttons)
            .into_any_element()
    }

    /// The keyboard hints for the Classic surface.
    pub(super) fn classic_hints(&self, grid: bool, chips: bool, palette: &MandaraPalette) -> AnyElement {
        hints(grid, chips, false, palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher_ext::{Hit, HitAction, Lead};

    fn app(name: &str, categories: &str) -> Item {
        Item::App(crate::apps::App { id: name.to_lowercase(), name: name.into(), categories: categories.into(), ..Default::default() })
    }

    fn hit(provider: &'static str, label: &str) -> Item {
        Item::Hit(Hit { provider, key: label.into(), label: label.into(), detail: String::new(), lead: Lead::Icon("folder"), badge: tk("File"), keywords: String::new(), action: HitAction::Copy(String::new()) })
    }

    fn names(items: &[Item]) -> Vec<String> {
        items.iter().map(title_of).collect()
    }

    #[test]
    fn grid_columns_adapt_to_the_width_and_icon_size() {
        let g = grid_geom(600.0 - 28.0, 48.0, 6);
        assert_eq!(g.cols, 6, "572 px fits six 80 px tiles");
        assert!((g.cell_w * g.cols as f32 - 572.0).abs() < 0.01, "the tiles fill the row");
        assert_eq!(grid_geom(852.0, 56.0, 8).cols, 8, "capped by grid_columns");
        assert_eq!(grid_geom(852.0, 96.0, 10).cols, 6, "bigger icons, fewer columns");
        assert_eq!(grid_geom(100.0, 96.0, 10).cols, GRID_COLUMNS_MIN, "never fewer than three");
        assert!(grid_geom(600.0, 64.0, 6).tile_h > grid_geom(600.0, 48.0, 6).tile_h);
    }

    #[test]
    fn arrows_move_in_two_dimensions_and_mirror_in_rtl() {
        assert_eq!(grid_step("down", 6, false), Some(6));
        assert_eq!(grid_step("up", 6, false), Some(-6));
        assert_eq!(grid_step("right", 6, false), Some(1));
        assert_eq!(grid_step("left", 6, false), Some(-1));
        assert_eq!(grid_step("left", 6, true), Some(1), "rows run right to left");
        assert_eq!(grid_step("right", 6, true), Some(-1));
        assert_eq!(grid_step("enter", 6, false), None);
    }

    #[test]
    fn tiles_reflow_from_their_old_slot_and_new_ones_fade_in() {
        let g = grid_geom(480.0, 48.0, 6);
        // Moved one slot back: starts one cell to the right.
        let (dx, dy, o) = reflow(Some(3), 2, &g, false, 0.0);
        assert!((dx - g.cell_w).abs() < 0.01 && dy == 0.0 && o == 1.0);
        // Mirrored in RTL.
        assert!((reflow(Some(3), 2, &g, true, 0.0).0 + g.cell_w).abs() < 0.01);
        // From far down the grid: arrives from at most 1.5 rows away.
        let (_, dy, _) = reflow(Some(600), 0, &g, false, 0.0);
        assert!((dy - g.tile_h * REFLOW_MAX_ROWS).abs() < 0.01);
        // Settled: in place; new tiles fade in.
        assert_eq!(reflow(Some(9), 2, &g, false, 1.0), (0.0, 0.0, 1.0));
        assert_eq!(reflow(None, 2, &g, false, 0.25).2, 0.25);
    }

    #[test]
    fn plain_searches_keep_the_grid() {
        let p = LauncherProviders::default();
        assert!(is_plain("", &p) && is_plain("fire", &p));
        assert!(!is_plain("=2+2", &p) && !is_plain("/g rust", &p) && !is_plain(":smile", &p) && !is_plain("~notes", &p));
    }

    #[test]
    fn results_group_under_a_top_hit_and_their_kinds() {
        let found = vec![app("Firefox", "Network;WebBrowser"), hit("files", "firefox.md"), app("Files", "System"), hit("windows", "Firefox — Docs"), hit("files", "fire.txt")];
        let (out, sections) = group_results(found, true);
        assert_eq!(names(&out), ["Firefox", "firefox.md", "fire.txt", "Files", "Firefox — Docs"]);
        assert_eq!(sections, vec![(0, TOP_HIT), (1, tk("Files")), (3, tk("Applications")), (4, tk("Windows"))]);
        // One result: no Top hit heading of its own.
        let (_, s) = group_results(vec![app("Firefox", "")], true);
        assert_eq!(s, vec![(0, tk("Applications"))]);
        let (_, s) = group_results(Vec::new(), true);
        assert!(s.is_empty());
    }

    #[test]
    fn grouped_rows_lay_out_under_their_headings_and_scroll_into_view() {
        let sections = [(0, TOP_HIT), (1, tk("Files")), (3, tk("Applications"))];
        let lay = list_layout(5, &sections, 48.0, HEADER_H);
        assert_eq!(lay.item_y, vec![HEADER_H, 2.0 * HEADER_H + 48.0, 2.0 * HEADER_H + 96.0, 3.0 * HEADER_H + 144.0, 3.0 * HEADER_H + 192.0]);
        assert_eq!(lay.headers.len(), 3);
        assert_eq!(lay.total, 3.0 * HEADER_H + 240.0);
        // Moving down past the view scrolls just enough; back up shows the heading.
        let view = 150.0;
        let s = follow(0.0, lay.item_y[4], 48.0, 0.0, view, lay.total);
        assert!((s - (lay.item_y[4] + 48.0 - view)).abs() < 0.01);
        let back = follow(s, lay.item_y[1], 48.0, HEADER_H, view, lay.total);
        assert!((back - (lay.item_y[1] - HEADER_H)).abs() < 0.01);
        assert_eq!(follow(s, lay.item_y[3], 48.0, HEADER_H, view, lay.total), s, "already in view: no scroll");
        assert_eq!(follow(40.0, lay.item_y[0], 48.0, HEADER_H, view, lay.total), 0.0);
    }

    #[test]
    fn categories_cycle_through_all_and_back() {
        let apps = vec![
            crate::apps::App { id: "firefox".into(), name: "Firefox".into(), categories: "Network;WebBrowser".into(), ..Default::default() },
            crate::apps::App { id: "gimp".into(), name: "GIMP".into(), categories: "Graphics".into(), ..Default::default() },
            crate::apps::App { id: "foot".into(), name: "Foot".into(), categories: "System;TerminalEmulator".into(), ..Default::default() },
        ];
        let all = categories(&apps, &["gimp".to_string()]);
        assert_eq!(all[0], GridFilter::Pinned);
        assert!(all.contains(&GridFilter::Group(tk("Internet"))) && all.contains(&GridFilter::Group(tk("Graphics"))));
        assert!(categories(&apps, &[])[0] != GridFilter::Pinned, "no Pinned chip without pinned apps");
        assert_eq!(cycle(None, &all, false), Some(all[0]));
        assert_eq!(cycle(Some(*all.last().unwrap()), &all, false), None, "wraps to All");
        assert_eq!(cycle(None, &all, true), all.last().copied());
        assert!(GridFilter::Pinned.keeps(&Item::App(apps[1].clone()), &["gimp".to_string()]));
        assert!(!GridFilter::Group(tk("Graphics")).keeps(&Item::App(apps[0].clone()), &[]));
    }

    #[test]
    fn beacon_modes_follow_their_providers_and_ctrl_digits() {
        let mut p = LauncherProviders::default();
        assert!(BeaconMode::ALL.iter().all(|m| m.available(&p)));
        assert_eq!(BeaconMode::Files.prefix(&p).as_deref(), Some("~"));
        assert_eq!(BeaconMode::Apps.prefix(&p), None);
        p.clipboard.enabled = false;
        p.files.prefix.clear();
        assert!(!BeaconMode::Clipboard.available(&p) && !BeaconMode::Files.available(&p) && BeaconMode::Apps.available(&p));
        assert_eq!(BeaconMode::from_digit("1"), Some(BeaconMode::Apps));
        assert_eq!(BeaconMode::from_digit("4"), Some(BeaconMode::Clipboard));
        assert_eq!(BeaconMode::from_digit("5"), None);
        assert_eq!(BeaconMode::from_digit("0"), None);
    }
}
