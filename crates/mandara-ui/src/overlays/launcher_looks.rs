//! The launcher's looks beyond the plain list (`[launcher.look]`, Studio →
//! Launcher → Launcher looks):
//!
//! * **Grid** (`layout = "grid"`): app icons with labels and a row of
//!   categories above them. The chips' placement and display are configurable.
//!   Arrows move in two dimensions and Tab switches category. Rows are virtualized.
//!   Plain searches return to tiles when grouping is off.
//! * **Grouped results** (`group_results`): a Top hit, then headings by
//!   kind (Applications, Actions & settings, Files, Windows…) in the order
//!   their best result ranks; only the visible rows are built.
//! * **Beacon** (`style = "beacon"`): a centred glass search bar in the
//!   upper part of the screen with round mode buttons at its reading end
//!   (Apps, Files, Actions, Clipboard: Ctrl+1–4), and the results in their
//!   own glass card under it once you type. The Apps mode shows the grid.
//!   Tab / Shift+Tab focuses its search, modes and visible app categories;
//!   Enter or Space activates a focused button, while typing returns to search.
//!   It pops in with the panels' spring (a slight scale and fade), the mode
//!   buttons slide out from behind the bar one after another, and the card
//!   grows to its rows with a stiffer spring.
//! * **Fullscreen** fills the focused display and uses a paginated app grid
//!   below the same search and mode controls.
//!
//! Every icon drawn here is recorded in the launcher's
//! [`crate::launcher_icons::IconLedger`] (bounded while open, all freed on
//! close).

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, ScrollEvent, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
    uniform_list,
};
use mandara_config::workflow::{LauncherCategoryDisplay, LauncherLayout, LauncherLook, LauncherProviders, LauncherStyle};
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_theme::{Color, MandaraPalette};

use super::{Item, LauncherView, Run, format_number};
use super::launcher_folders::DragApp;
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
/// Separate app tiles and leave room for the page controls at the foot.
const PAGE_TILE_GAP: f32 = 24.0;
const PAGE_NAV_H: f32 = 36.0;
const PAGE_ARROW_H: f32 = 52.0;
const PAGE_ARROW_ICON: f32 = 28.0;
const PAGE_GUTTER: f32 = PAGE_ARROW_H + 8.0;
const PAGE_SLIDE_PX: f32 = 56.0;
const FULLSCREEN_SEARCH_WIDTH: f32 = 800.0;
const FULLSCREEN_SEARCH_HEIGHT: f32 = 640.0;
const FULLSCREEN_SEARCH_HEIGHT_FRACTION: f32 = 0.76;
const FULLSCREEN_RESULT_ROW_SPACE: f32 = 8.0;
const PAGE_DOT: f32 = 7.0;
const PAGE_MAX_DOTS: usize = 9;
/// Match the session overlay's dim while keeping the compositor blur visible.
const FULLSCREEN_SCRIM_ALPHA: f32 = 0.45;
const FULLSCREEN_LIGHT_SCRIM_ALPHA: f32 = 0.78;
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
/// Width of the compact Grid's category column, including localized labels.
const CATEGORY_SIDEBAR_W: f32 = 138.0;
const CATEGORY_ICON: f32 = 15.0;
const CATEGORY_PAD: f32 = 8.0;
/// Two rows of mini icons identify a folder's contents.
const FOLDER_ICON_FRACTION: f32 = 0.42;
const FOLDER_ICON_GAP: f32 = 3.0;
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
/// Choreography travel of a grid tile cascading in, in px.
const TILE_CASCADE_PX: f32 = 14.0;
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

/// Fit a page within the output while respecting the requested density.
fn fullscreen_grid(width: f32, height: f32, look: &LauncherLook) -> (GridGeom, usize) {
    let icon = look.grid_icon();
    let cap = (look.grid_columns as usize).clamp(GRID_COLUMNS_MIN, GRID_COLUMNS_MAX);
    let stride = icon + TILE_PAD_X * 2.0 + PAGE_TILE_GAP;
    let cols = (((width + PAGE_TILE_GAP) / stride).floor() as usize).clamp(1, cap);
    let cell_w = ((width - PAGE_TILE_GAP * (cols - 1) as f32) / cols as f32).max(0.0);
    let mut g = grid_geom(width, icon.min(cell_w), look.grid_columns);
    g.cols = cols;
    g.cell_w = cell_w;
    let rows = (((height + PAGE_TILE_GAP) / (g.tile_h + PAGE_TILE_GAP)).floor() as usize).clamp(1, look.fullscreen_rows());
    (g, rows * cols)
}

/// A plain search (no provider prefix, `/` or `=`): the grid stays.
pub(super) fn is_plain(query: &str, providers: &LauncherProviders) -> bool {
    let q = query.trim();
    !q.starts_with('/') && !q.starts_with('=') && !q.starts_with('!') && crate::launcher_ext::route(q, providers).is_none()
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

    fn glyph(self) -> &'static str {
        match self {
            Self::Pinned => "pin",
            Self::Group("Internet") => "globe",
            Self::Group("Office") => "clipboard",
            Self::Group("Development") => "terminal",
            Self::Group("Graphics") => "palette",
            Self::Group("Media") => "music",
            Self::Group("Games") => "dice",
            Self::Group("Education") => "book",
            Self::Group("Utilities") => "sliders",
            Self::Group("System") => "monitor",
            _ => "folder",
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
    pub(super) fn placeholder(self) -> &'static str {
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
    pub(super) fn prefix(self, providers: &LauncherProviders) -> Option<&str> {
        self.provider(providers).map(|p| p.prefix.as_str())
    }

    /// The provider it searches (`None` for Apps).
    fn provider(self, providers: &LauncherProviders) -> Option<&mandara_config::workflow::ProviderConfig> {
        match self {
            BeaconMode::Apps => None,
            BeaconMode::Files => Some(&providers.files),
            BeaconMode::Actions => Some(&providers.actions),
            BeaconMode::Clipboard => Some(&providers.clipboard),
        }
    }

    /// Offered: Apps always, the others while their provider is on and has
    /// a prefix to search with.
    pub(super) fn available(self, providers: &LauncherProviders) -> bool {
        self.provider(providers).is_none_or(|p| p.enabled && !p.prefix.is_empty())
    }

    /// The mode Ctrl+`digit` picks.
    pub(super) fn from_digit(digit: &str) -> Option<Self> {
        digit.parse::<usize>().ok().and_then(|d| d.checked_sub(1)).and_then(|i| Self::ALL.get(i).copied())
    }
}

/// The mode Beacon keeps after a provider setting changed while it is open:
/// the same one while it is still offered, otherwise plain search (its
/// provider was switched off, or its prefix cleared).
pub(super) fn settle_mode(mode: Option<BeaconMode>, providers: &LauncherProviders) -> Option<BeaconMode> {
    mode.filter(|m| m.available(providers))
}

/// Classic Grid keeps its category filters in ordinary text searches.
pub(super) fn classic_grid_query(look: &LauncherLook, query: &str, providers: &LauncherProviders) -> bool {
    look.style == LauncherStyle::Classic && look.layout == LauncherLayout::Grid && is_plain(query, providers)
}

/// Grouped queries switch Grid to a list. Beacon keeps its Apps-mode grid.
pub(super) fn shows_app_grid(
    look: &LauncherLook,
    mode: Option<BeaconMode>,
    query: &str,
    providers: &LauncherProviders,
) -> bool {
    match look.style {
        LauncherStyle::Classic => classic_grid_query(look, query, providers)
            && (!look.group_results || query.trim().is_empty()),
        LauncherStyle::Beacon => mode == Some(BeaconMode::Apps),
        LauncherStyle::Fullscreen => (mode == Some(BeaconMode::Apps)
            || (mode.is_none() && query.trim().is_empty()))
            && is_plain(query, providers),
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

/// Tab moves between focusable controls or cycles categories in a scoped grid.
fn tab_hint_label(chips: bool, beacon: bool) -> Option<&'static str> {
    if beacon { Some(tk("Focus")) } else if chips { Some(tk("Category")) } else { None }
}

/// The keyboard hints strip for this state.
fn hints(grid: bool, chips: bool, beacon: bool, palette: &MandaraPalette) -> AnyElement {
    let mut row = div().h(px(HINTS_H)).w_full().row().items_center().justify_center().gap(px(HINT_GAP)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla());
    row = row.child(if grid { hint(&["←", "↑", "↓", "→"], tk("Move"), palette) } else { hint(&["↑", "↓"], tk("Move"), palette) });
    row = row.child(hint(&["Enter"], tk("Open"), palette));
    if let Some(label) = tab_hint_label(chips, beacon) {
        row = row.child(hint(&["Tab"], label, palette));
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
        .border(px(crate::plugins::panel_fx::edge_px()))
        .border_color(crate::plugins::panel_fx::surface_edge(palette, cfg.theme.adjustments.rim_intensity).hsla())
        .shadow(shadow)
        .when_some(crate::plugins::panel_fx::ink_texture(palette), |el, t| el.overflow_hidden().child(t))
}

#[derive(Clone)]
struct CategoryTip {
    label: SharedString,
    palette: MandaraPalette,
}

impl gpui::Render for CategoryTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px(px(CHIP_PAD)).h(px(CHIPS_H)).row().items_center()
            .rounded_full().bg(self.palette.on_surface.hsla()).text_color(self.palette.surface_base.hsla())
            .text_size(px(text::BODY_S)).child(self.label.clone())
    }
}

impl LauncherView {
    // ── Grid ────────────────────────────────────────────────────────────

    pub(super) fn category_sidebar_width(&self) -> f32 {
        match self.look.grid_category_display {
            LauncherCategoryDisplay::LabelOnly => CATEGORY_SIDEBAR_W,
            _ => CHIPS_H,
        }
    }

    pub(super) fn grid_categories(&self) -> &[GridFilter] {
        if self.look.style == LauncherStyle::Fullscreen { &[] } else { &self.categories }
    }

    /// The grid's rows `range` (the `uniform_list` asks for visible ones).
    fn grid_rows(&mut self, range: Range<usize>, g: GridGeom, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = mandara_core::frame_palette(window, cx);
        let rtl = crate::bidi::rtl();
        let t = self.reflow.value();
        let accent = readable(palette.primary, &palette);
        let ignore_mouse = self.look.ignore_mouse;
        let mut out = Vec::with_capacity(range.len());
        let choreo = mandara_core::choreo::choreography();
        let (opened, origin) = self.cascade;
        for r in range {
            let paged = self.fullscreen_page_size.is_some();
            let mut row = div().row().w_full().h(px(g.tile_h)).when(paged, |row| row.gap(px(PAGE_TILE_GAP)));
            for i in r * g.cols..((r + 1) * g.cols).min(self.results.len()) {
                let item = self.results[i].clone();
                let prev = self.reflow_from.get(&super::usage_key(&item)).copied();
                let (dx, dy, opacity) = if paged { (0.0, 0.0, 1.0) } else { reflow(prev, i, &g, rtl, t) };
                // Tiles cascade in with the choreography as it opens.
                let (dx, dy, opacity) = match choreo.item(opened, i, origin, rtl, TILE_CASCADE_PX) {
                    Some((cx_, cy, alpha)) => (dx + cx_, dy + cy, opacity * alpha),
                    None => (dx, dy, opacity),
                };
                let selected = i == self.selected;
                let group = self.fullscreen_groups.get(&i).cloned();
                let title = if let Some(apps) = &group {
                    format!("{} +{}", title_of(&item), apps.len() - 1)
                } else {
                    title_of(&item)
                };
                let lead = if let Some(apps) = group {
                    let rows = apps[..apps.len().min(4)].chunks(2).map(|apps| {
                        div().row().gap(px(FOLDER_ICON_GAP)).children(apps.iter().map(|app| {
                            self.lead(&Item::App(app.clone()), g.icon * FOLDER_ICON_FRACTION, &palette, cx)
                        }))
                    }).collect::<Vec<_>>();
                    Some(div().size(px(g.icon)).rounded(px(crate::style::radius(TILE_RADIUS)))
                        .bg(palette.surface_container_high.hsla()).flex().flex_col().items_center().justify_center()
                        .gap(px(FOLDER_ICON_GAP)).children(rows).into_any_element())
                } else {
                    Some(self.lead(&item, g.icon, &palette, cx))
                };
                let hover = cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && !ignore_mouse && (!paged || !this.wheel_hold) && this.selected != i {
                        this.selected = i;
                        cx.notify();
                    }
                });
                let tile = div()
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
                        .role(gpui::Role::Button)
                        .aria_label(title.clone())
                        .children(lead)
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
                                .child(title.clone()),
                        )
                        .on_hover(hover)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.activate(i, cx)));
                let tile = if paged && self.organizes_fullscreen() && let Item::App(app) = item {
                    self.draggable_fullscreen_tile(tile, app.id, title, g.cell_w, &palette, cx)
                } else {
                    tile
                };
                row = row.child(tile);
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

    pub(super) fn select_grid_page(&mut self, page: usize, size: usize, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let last = (self.results.len() - 1) / size;
        let page = page.min(last);
        if page != self.selected / size {
            self.selected = page * size;
            self.wheel_hold = true;
            cx.notify();
        }
    }

    fn paged_grid(&mut self, g: GridGeom, size: usize, height: f32, tab_start: usize, palette: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let pages = self.results.len().div_ceil(size);
        let page = self.selected / size;
        let first_row = page * size / g.cols;
        let end_row = (first_row + size / g.cols).min(self.results.len().div_ceil(g.cols));
        let grid = if pages == 0 {
            div().text_color(palette.on_surface_variant.hsla()).child(t("No results")).into_any_element()
        } else {
            div().w_full().flex().flex_col().gap(px(PAGE_TILE_GAP))
                .children(self.grid_rows(first_row..end_row, g, window, cx)).into_any_element()
        };
        let label = |index: usize| tf("Page {page} of {pages}", &[("page", &(index + 1)), ("pages", &pages)]);
        let accent = readable(palette.primary, palette);
        let organize = self.organizes_fullscreen();
        let button = |index: usize, name: String, order: usize, side: bool, cx: &mut Context<Self>| {
            let edge = if side { PAGE_ARROW_H } else { PAGE_NAV_H };
            let button = div().id(("fullscreen-page", order)).size(px(edge)).rounded_full()
                .when(side, |s| s.bg(palette.surface_container_high.hsla()).hover(|s| s.bg(palette.surface_container.hsla())))
                .flex().items_center().justify_center()
                .role(gpui::Role::Button).aria_label(name)
                .focusable().tab_index((tab_start + order) as isize)
                .focus_visible(move |s| s.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_grid_page(index, size, cx)));
            if organize {
                button.drag_over::<DragApp>(move |s, _, _, _| s.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
                    .on_drop(cx.listener(move |this, drag: &DragApp, _, cx| this.drop_on_fullscreen_page(drag, index, size, cx)))
            } else {
                button
            }
        };
        let rtl = crate::bidi::rtl();
        let mut nav = div().row().h(px(PAGE_NAV_H)).flex_none().justify_center().items_center();
        let side = |index: Option<usize>, name: &str, glyph: &'static str, order: usize, cx: &mut Context<Self>| {
            div().w(px(PAGE_GUTTER)).flex_none().flex().justify_center()
                .children(index.map(|index| button(index, name.into(), order, true, cx)
                    .child(icon(glyph, PAGE_ARROW_ICON, palette.on_surface))))
        };
        let previous = side(page.checked_sub(1), t("Previous"), if rtl { "chevron-right" } else { "chevron-left" }, 0, cx);
        let next = side((page + 1 < pages).then_some(page + 1), t("Next"), if rtl { "chevron-left" } else { "chevron-right" }, pages + 1, cx);
        if pages > 1 {
            let dots_fit = (g.cols as f32 * g.cell_w / PAGE_NAV_H) as usize;
            if pages <= PAGE_MAX_DOTS.min(dots_fit) {
                for index in 0..pages {
                    let active = index == page;
                    nav = nav.child(button(index, label(index), index + 1, false, cx)
                        .aria_toggled(if active { gpui::Toggled::True } else { gpui::Toggled::False })
                        .child(div().size(px(PAGE_DOT)).rounded_full().bg(if active { accent } else { palette.on_surface_variant }.hsla())));
                }
            } else {
                nav = nav.child(div().text_size(px(text::BODY_S)).child(label(page)));
            }
        }
        let page_h = (size / g.cols) as f32 * (g.tile_h + PAGE_TILE_GAP) - PAGE_TILE_GAP;
        let slide = self.page_slide.value();
        let tiles = div().flex_1().min_w(px(0.0)).h(px(page_h)).overflow_hidden().child(
            div().relative().left(px(slide)).opacity(1.0 - 0.28 * (slide / PAGE_SLIDE_PX).abs().min(1.0)).w_full().child(grid),
        );
        div().h(px(height)).flex().flex_col()
            .children(self.fullscreen_folder_back(tab_start + pages + 2, palette, cx))
            .child(div().flex_1().row().items_center().child(previous).child(tiles).child(next))
            .child(nav)
            .into_any_element()
    }

    /// The category chips above the grid.
    pub(super) fn chips_element(&self, palette: &MandaraPalette, modes: Option<usize>, vertical: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let accent = readable(palette.primary, palette);
        let on_accent = accent.best_on(Color::hex("#ffffff"), Color::hex("#101010"));
        let categories = self.grid_categories();
        let focus = modes.map(|modes| super::beacon_keyboard::categories(categories.len() + 1, modes, vertical, window, cx));
        // Shaped like the chips of the design picked under Themes →
        // Components: classic filled pills, Material's outlined chips with a
        // tinted pick, Connected's outlined pills.
        let design = crate::style::chip_design();
        let classic = design == crate::components::ChipDesign::Classic;
        let material = design == crate::components::ChipDesign::Material;
        let outline = crate::components::chips::outline(palette);
        let chosen = crate::components::chips::chosen(palette);
        let display = if self.look.style == LauncherStyle::Classic {
            self.look.grid_category_display
        } else {
            LauncherCategoryDisplay::LabelOnly
        };
        let icons = display != LauncherCategoryDisplay::LabelOnly;
        let chip = |id: SharedString, filter: Option<GridFilter>, index: usize| {
            let label = filter.map_or_else(|| t("All").into(), GridFilter::label);
            let on = self.category == filter;
            let tint = if on { if material { accent } else { on_accent } } else { palette.on_surface_variant };
            div()
                .id(id)
                .h(px(CHIPS_H))
                .px(px(if vertical && icons { 0.0 } else if vertical { CATEGORY_PAD } else { CHIP_PAD }))
                .rounded(px(crate::components::chips::chip_radius(design, CHIPS_H)))
                .flex()
                .items_center()
                .flex_none()
                .cursor_pointer()
                .text_size(px(text::BODY_S))
                .role(gpui::Role::RadioButton)
                .aria_label(label.clone())
                .aria_toggled(if on { gpui::Toggled::True } else { gpui::Toggled::False })
                .when_some(focus.as_ref(), |el, (_, handles)| el
                    .track_focus(&handles[index])
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .focus_visible(move |style| style.border_color(if on { on_accent } else { accent }.hsla())))
                .when(on && !material, |el| el.bg(accent.hsla()).text_color(on_accent.hsla()).font_weight(FontWeight::SEMIBOLD))
                .when(on && material, |el| el.bg(chosen.hsla()).text_color(accent.hsla()).font_weight(FontWeight::SEMIBOLD))
                .when(!on && classic, |el| el.bg(crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_SOFT, palette).hsla()))
                .when(!on && !classic, |el| el.border_1().border_color(outline.hsla()))
                .when(!on, |el| el.hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla())))
                .when(icons, |el| el.justify_center().child(icon(filter.map_or("grid", GridFilter::glyph), CATEGORY_ICON, tint)))
                .when(!icons && !vertical, |el| el.child(label.clone()))
                .when(!icons && vertical, |el| el.child(div().min_w(px(0.0)).truncate().child(label.clone())))
                .when(display == LauncherCategoryDisplay::IconWithHoverLabel, |el| {
                    let tip = CategoryTip { label, palette: palette.clone() };
                    el.tooltip(move |_, cx| cx.new(|_| tip.clone()).into())
                })
        };
        let mut row = div().id("launcher-chips").flex_none().row().gap(px(CHIP_GAP))
            .when(vertical, |el| el.flex_col().w(px(self.category_sidebar_width())).h_full().overflow_y_scroll())
            .when(!vertical, |el| el.h(px(CHIPS_H)).overflow_x_scroll())
            .when(!vertical && modes.is_some(), |el| el.flex_row())
            .when_some(focus.as_ref(), |el, (scroll, _)| el.track_scroll(scroll));
        let count = categories.len() + 1;
        let rtl = !vertical && modes.is_some() && crate::bidi::rtl();
        for child in 0..count {
            let index = super::beacon_keyboard::category_child(child, count, rtl);
            if index == 0 {
                row = row.child(chip("launcher-chip-all".into(), None, 0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_category(None, cx))));
                continue;
            }
            let n = index - 1;
            let f = categories[n];
            row = row.child(chip(SharedString::from(format!("launcher-chip-{n}")), Some(f), n + 1).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let next = if this.category == Some(f) { None } else { Some(f) };
                this.set_category(next, cx);
            })));
        }
        row.into_any_element()
    }

    pub(super) fn set_category(&mut self, category: Option<GridFilter>, cx: &mut Context<Self>) {
        self.category = category;
        self.refilter(cx);
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
        self.refilter(cx);
        cx.notify();
    }

    /// Whether the Beacon results card shows (something typed, or a mode).
    pub(super) fn beacon_open_results(&self) -> bool {
        self.look.style == LauncherStyle::Beacon && (!self.query.trim().is_empty() || self.mode.is_some())
    }

    pub(super) fn render_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use mandara_config::surfaces::PanelAnimation;
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let look = self.look.clone();
        let fullscreen = look.style == LauncherStyle::Fullscreen;
        let viewport = window.viewport_size();
        let (sw, sh) = (f32::from(viewport.width), f32::from(viewport.height));
        let rtl = crate::bidi::rtl();
        let modes = self.beacon_modes();
        let bar_here = self.output.as_deref().is_none_or(|o| cfg.bar_on_output(o));
        let place = crate::launcher_place::beacon(&cfg, (sw, sh), bar_here, modes.len(), rtl);
        let accent = readable(palette.primary, &palette);

        // What the card holds and how tall it is.
        let show = fullscreen || self.beacon_open_results();
        let grid = self.grid_active();
        let mut card = place.card;
        if fullscreen && !grid {
            let width = card.w.min(FULLSCREEN_SEARCH_WIDTH);
            card.x += (card.w - width) / 2.0;
            card.w = width;
            card.h = (card.h * FULLSCREEN_SEARCH_HEIGHT_FRACTION).min(FULLSCREEN_SEARCH_HEIGHT);
        }
        let row_h = (if look.compact { super::ROW_H_COMPACT } else { super::ROW_H })
            + if fullscreen && !grid { FULLSCREEN_RESULT_ROW_SPACE } else { 0.0 };
        let lead_px = if look.compact { super::ROW_LEAD_COMPACT_PX } else { super::ROW_LEAD_PX };
        let hints_h = if look.hints { HINTS_H } else { 0.0 };
        let chips = !fullscreen && grid && look.grid_categories && !self.grid_categories().is_empty();
        let room = (card.h - CARD_PAD * 2.0 - hints_h).max(row_h);
        let inner_w = card.w - CARD_PAD * 2.0;
        let chips_h = if chips { CHIPS_H + CARD_PAD } else { 0.0 };
        let paged = fullscreen && grid;
        let g = if paged {
            let back = if self.fullscreen_folder.is_some() { super::launcher_folders::FOLDER_BACK_H } else { 0.0 };
            let (g, size) = fullscreen_grid(inner_w - PAGE_GUTTER * 2.0, room - back - PAGE_NAV_H, &look);
            self.fullscreen_page_size = Some(size);
            g
        } else {
            self.fullscreen_page_size = None;
            grid_geom(inner_w, look.grid_icon(), look.grid_columns)
        };
        self.grid_cols = g.cols;
        let lay = list_layout(self.results.len(), &self.sections, row_h, HEADER_H);
        let content_h = if paged {
            room
        } else if grid {
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
        self.page_slide.set_instant(instant);
        let visible_page = self.fullscreen_page_size.map(|size| (self.selected / size, size));
        if let (Some((page, size)), Some((previous, previous_size))) = (visible_page, self.rendered_fullscreen_page)
            && size == previous_size && page != previous
        {
            let direction = if (page > previous) != rtl { 1.0 } else { -1.0 };
            self.page_slide.snap_to(direction * PAGE_SLIDE_PX);
        }
        self.rendered_fullscreen_page = visible_page;
        self.page_slide.set_target(0.0);
        let page = self.page_slide.step(dt);
        if self.closing && !opening {
            self.release(window, cx);
            window.remove_window();
        } else if opening || a || b || c || d || page {
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
        self.rect = if fullscreen { (0.0, 0.0, sw, sh) } else {
            (place.card.x, place.bar.y, place.card.w, place.bar.h + BEACON_GAP + self.card_h.value())
        };

        // ── The search bar ──
        self.field_px = BAR_TEXT;
        // A mode names what it searches; otherwise the prefix help every
        // look shares.
        let placeholder = match self.mode {
            Some(mode) => SharedString::new_static(t(mode.placeholder())),
            None if fullscreen => SharedString::new_static(t("Search apps, files and more")),
            None => self.prefix_help(BAR_TEXT),
        };
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
        let field = self.query_field(BAR_TEXT, placeholder, &palette, self.focus.is_focused(window));
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
            let button = glass(&cfg, &palette, progress, r.h / 2.0)
                .absolute()
                .left(px(r.x))
                .top(px(r.y))
                .size(px(r.w))
                .opacity(local.clamp(0.0, 1.0));
            buttons.push(self.mode_button(button, n, m, &palette, cx));
        }

        // ── The results card ──
        let card_h = self.card_h.value().max(0.0);
        let card_el = (card_h > 0.5).then(|| {
            let reveal = if card_target > 0.0 { (card_h / card_target).clamp(0.0, 1.0) } else { (card_h / (CARD_PAD * 2.0 + row_h)).clamp(0.0, 1.0) };
            let body = if grid {
                let grid_el = if let Some(size) = self.fullscreen_page_size {
                    let tabs = 1 + modes.len() + if chips { self.grid_categories().len() + 1 } else { 0 };
                    self.paged_grid(g, size, list_h, tabs, &palette, window, cx)
                } else {
                    self.grid_element(g, list_h, &palette, cx)
                };
                div().flex().flex_col().gap(px(CARD_PAD)).when(chips, |el| el.child(self.chips_element(&palette, Some(modes.len()), false, window, cx))).child(grid_el).into_any_element()
            } else {
                self.grouped_element(&lay, list_h, row_h, lead_px, true, &palette, cx)
            };
            let wheel_row = row_h;
            let panel = if fullscreen { div() } else {
                glass(&cfg, &palette, progress, crate::style::radius(CARD_RADIUS))
            };
            panel.id("beacon-card")
                .absolute()
                .left(px(card.x))
                .top(px(card.y - lift))
                .w(px(card.w))
                .h(px(card_h))
                .opacity(progress)
                .overflow_hidden()
                .occlude()
                .on_scroll(cx.listener(move |this, e: &ScrollEvent, _, cx| {
                    if let Some(size) = this.fullscreen_page_size {
                        this.wheel_select(e, g.tile_h, size, cx);
                        return;
                    }
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
        let fullscreen_scrim = if palette.is_dark {
            gpui::black().opacity(FULLSCREEN_SCRIM_ALPHA * progress)
        } else {
            palette.surface_base.with_alpha(FULLSCREEN_LIGHT_SCRIM_ALPHA * progress).hsla()
        };
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
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.on_key(e, window, cx)))
            .child(
                div()
                    .id("launcher-outside")
                    .absolute()
                    .inset_0()
                    .when(fullscreen, |el| el.bg(fullscreen_scrim))
                    .when(!fullscreen && dim > 0.0, |el| el.bg(gpui::black().opacity(dim)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close(cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close(cx))),
            )
            .children(card_el)
            .child(bar_el.occlude())
            .children(buttons)
            .into_any_element()
    }

    fn mode_button(
        &self,
        surface: gpui::Div,
        index: usize,
        mode: BeaconMode,
        palette: &MandaraPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = self.mode == Some(mode);
        let accent = readable(palette.primary, palette);
        let fg = if on { accent.best_on(Color::hex("#ffffff"), Color::hex("#101010")) } else { palette.on_surface };
        let label = mode.label();
        let hover = palette.surface_container_high;
        surface.id(SharedString::from(format!("beacon-mode-{}", label.to_lowercase())))
            .role(gpui::Role::Button)
            .aria_label(t(label))
            .aria_toggled(if on { gpui::Toggled::True } else { gpui::Toggled::False })
            .focusable()
            .tab_index(index as isize + 1)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .focus_visible(move |style| style.border_color(accent.hsla()))
            .when(on, |el| el.bg(accent.hsla()))
            .when(!on, |el| el.hover(move |s| s.bg(hover.hsla())))
            .child(icon(mode.glyph(), MODE_ICON, fg))
            .occlude()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_mode(mode, cx)))
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

    #[test]
    fn fullscreen_page_respects_configured_density_and_available_space() {
        let mut look = LauncherLook::default();
        mandara_config::workflow::LauncherLookPreset::find("fullscreen").unwrap().apply(&mut look);
        let (grid, page_size) = fullscreen_grid(1048.0, 480.0, &look);
        assert_eq!(grid.cols, 6);
        assert_eq!(page_size, 18);
        assert!(grid.cell_w >= grid.icon + TILE_PAD_X * 2.0);

        look.fullscreen_rows = 2;
        look.grid_columns = 4;
        look.grid_icon_size = 80.0;
        let (grid, page_size) = fullscreen_grid(1048.0, 480.0, &look);
        assert_eq!(page_size, 8);
        assert_eq!(grid.icon, 80.0);

        let (grid, page_size) = fullscreen_grid(300.0, 240.0, &look);
        assert_eq!(grid.cols, 2);
        assert_eq!(page_size, 2);
        assert!(grid.tile_h <= 240.0);
    }

    #[test]
    fn grid_search_keeps_categories_and_has_a_legacy_tiles_option() {
        let providers = LauncherProviders::default();
        let mut look = LauncherLook::default();
        mandara_config::workflow::LauncherLookPreset::find("grid").unwrap().apply(&mut look);
        assert!(shows_app_grid(&look, None, "  ", &providers));
        assert!(!shows_app_grid(&look, None, "zen", &providers));
        assert!(classic_grid_query(&look, "zen", &providers));
        look.group_results = false;
        assert!(shows_app_grid(&look, None, "zen", &providers));
        assert!(!shows_app_grid(&look, None, ":smile", &providers));
    }

    #[test]
    fn beacon_and_fullscreen_keep_their_own_browsing_and_modes() {
        let providers = LauncherProviders::default();
        for id in ["beacon", "fullscreen"] {
            let mut look = LauncherLook::default();
            mandara_config::workflow::LauncherLookPreset::find(id).unwrap().apply(&mut look);
            assert_eq!(shows_app_grid(&look, None, "", &providers), id == "fullscreen");
            assert!(!shows_app_grid(&look, None, "zen", &providers));
            assert!(shows_app_grid(&look, Some(BeaconMode::Apps), "zen", &providers));
            assert!(!shows_app_grid(&look, Some(BeaconMode::Files), "", &providers));
            assert!(!classic_grid_query(&look, "zen", &providers));
        }
    }

    #[test]
    fn tab_hint_follows_search_bar_focus_and_visible_categories() {
        assert_eq!(tab_hint_label(false, true), Some("Focus"));
        assert_eq!(tab_hint_label(true, true), Some("Focus"));
        assert_eq!(tab_hint_label(true, false), Some("Category"));
        assert_eq!(tab_hint_label(false, false), None);
    }

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
    fn plain_queries_exclude_provider_prefixes() {
        let p = LauncherProviders::default();
        assert!(is_plain("", &p) && is_plain("fire", &p));
        assert!(!is_plain("!set gaps 8", &p) && !is_plain("! set gaps 8", &p));
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
        assert_eq!(BeaconMode::Files.prefix(&p), Some("~"));
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
