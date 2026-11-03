//! Shortcuts cheatsheet (built-in `keybinds` plugin, opt-in): every
//! keyboard shortcut in one searchable sheet — Mandara's own binds
//! (`[compositor] keybinds`, described by the commands they run) and the
//! user's own binds for the running compositor (MangoWM, Niri, Hyprland or
//! Umbriel: the config and every file it sources / includes, through
//! [`mandara_config::conflicts::user_binds`]).
//!
//! Binds are grouped (Mandara, windows, workspaces / tags, apps, media &
//! system, other; see [`mandara_services::keybinds`]) and laid out in three
//! newspaper columns, each chord drawn as rounded keycaps. The sheet opens
//! like the launcher: a centred overlay over a dimmed screen on the focused
//! output, with the keyboard (so a bound key can open it and type at once).
//! The config files are read on a background thread each time it opens,
//! never while it stays open or closed. Typing filters live; Esc or a click
//! outside closes it.
//!
//! No settings. Commands: `keybinds list` (JSON: chord, keys, action,
//! description, group, source file), `keybinds show`, `keybinds hide`,
//! `keybinds toggle`. Mandara adds no compositor bind for the sheet; bind
//! `mandara msg keybinds toggle` yourself.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    AnyElement, App, AppContext as _, BoxShadow, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Window, WindowHandle, div, point, prelude::FluentBuilder as _, px,
};
use mandara_compositor::CompositorKind;
use mandara_config::ShellConfig;
use mandara_config::compositor::{BindAction, Target};
use mandara_core::{LayerSurfaceSpec, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::keybinds::{self as kb, Entry, Group};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{CONTROL_H, caption, search_box};
use super::fn_view;
use super::panel_fx::{CARD_RADIUS_PX, HEADER_H, chip, elevation, empty_state, header, readable, rim};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::text;
use crate::state::config;
use crate::text_field::TextField;

pub const ID: &str = "keybinds";

// ── Layout ───────────────────────────────────────────────────────────────

/// The sheet card: width, padding, gap, the share of the screen it may
/// take and its fade / rise on opening.
const SHEET_W: f32 = 920.0;
const SCREEN_MARGIN_PX: f32 = 32.0;
const SCREEN_SHARE_H: f32 = 0.72;
const PAD: f32 = 20.0;
const GAP: f32 = 14.0;
const FADE_IN: Duration = Duration::from_millis(180);
const RISE_PX: f32 = 12.0;
/// The dimmed screen behind the sheet.
const SCRIM_ALPHA: f32 = 0.45;
const SCRIM_BASE_HEX: &str = "#000000";
/// Tallest the columns grow; longer sheets scroll.
const MAX_SHEET_H: f32 = 560.0;
const EMPTY_H: f32 = 200.0;
/// Newspaper columns and the gap between them.
const COLUMNS: usize = 3;
const COLUMN_GAP_PX: f32 = 20.0;
/// A group header, a bind row, the space above a group.
const GROUP_H: f32 = 26.0;
const ROW_H: f32 = 28.0;
const GROUP_GAP: f32 = 10.0;
const GROUP_ICON_PX: f32 = 13.0;
const ROW_GAP_PX: f32 = 10.0;
const ROW_PAD_X_PX: f32 = 6.0;
const ROW_RADIUS_PX: f32 = 8.0;
const ROW_HOVER_ALPHA: f32 = 0.05;
/// Keycaps: height, padding, radius, gap, text size and the soft key edge
/// `(alpha, offset, blur)`, lighter on light themes.
const KEY_H_PX: f32 = 20.0;
const KEY_PAD_X_PX: f32 = 6.0;
const KEY_MIN_W_PX: f32 = 20.0;
const KEY_RADIUS_PX: f32 = 6.0;
const KEY_GAP_PX: f32 = 3.0;
const KEY_TEXT_PX: f32 = 10.5;
const KEY_EDGE: (f32, f32, f32) = (0.22, 1.5, 1.5);
const KEY_EDGE_LIGHT: f32 = 0.6;
/// Bar chip glyph.
const BAR_ICON_PX: f32 = 15.0;

// ── Building the sheet ───────────────────────────────────────────────────

/// The running compositor.
fn compositor(cx: &App) -> Option<CompositorKind> {
    crate::hub::hub(cx).read(cx).compositor.as_ref().map(|b| b.kind()).or_else(mandara_compositor::detect)
}

/// Mandara's own binds, described by the commands they run (only the ones
/// its drop-in writes; `skip` holds chords your config binds itself).
fn mandara_entries(cfg: &ShellConfig, skip: &[String], cx: &App) -> Vec<Entry> {
    if !cfg.compositor.manage {
        return Vec::new();
    }
    let registry = &cx.global::<PluginRegistry>().commands;
    cfg.compositor
        .keybinds
        .iter()
        .filter(|b| b.problem().is_none() && !skip.contains(&b.chord()))
        .map(|b| {
            let (action, description) = match &b.action {
                BindAction::Mandara(words) => {
                    let w: Vec<String> = words.split_whitespace().map(String::from).collect();
                    let summary = kb::mandara_label(words)
                        .map(str::to_string)
                        .unwrap_or_else(|| registry.resolve(&w).map(|r| t(r.spec.summary).to_string()).unwrap_or_else(|_| words.clone()));
                    (format!("mandara msg {words}"), summary)
                }
                BindAction::Spawn(cmd) => (cmd.clone(), tf("Run {program}", &[("program", &cmd)])),
            };
            let group = kb::group_of(&action, &b.chord());
            Entry::new(b.chord(), action, description, group, "mandara".into(), None)
        })
        .collect()
}

/// The user's binds for `target` (reads files: call off the UI thread).
fn user_entries(target: Target, cfg: &mandara_config::compositor::CompositorConfig) -> Vec<Entry> {
    mandara_config::conflicts::user_binds(target, cfg)
        .into_values()
        .map(|b| kb::user_entry(target, &b.chord, &b.body, b.file.display().to_string(), b.line))
        .collect()
}

/// Reads every bind on the background executor; `done` gets the entries.
fn load(cx: &mut App, done: impl FnOnce(Vec<Entry>, &mut App) + 'static) {
    let cfg = config(cx).read(cx).snapshot();
    let target = compositor(cx).map(CompositorKind::target);
    let compositor_cfg = cfg.compositor.clone();
    let scan = cx.background_executor().spawn(async move { target.map(|t| user_entries(t, &compositor_cfg)).unwrap_or_default() });
    cx.spawn(async move |cx| {
        let user = scan.await;
        cx.update(|cx| {
            let chords: Vec<String> = user.iter().map(|e| e.chord.clone()).collect();
            let mut all = mandara_entries(&cfg, &chords, cx);
            all.extend(user);
            done(all, cx);
        });
    })
    .detach();
}

// ── Layout of the columns ────────────────────────────────────────────────

/// One cell of a column.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    /// A group header (`continued` when a group runs over from the column
    /// before) with its bind count.
    Head { group: Group, count: usize, continued: bool },
    Bind(usize),
}

impl Slot {
    fn height(&self, top: bool) -> f32 {
        match self {
            Slot::Head { .. } if top => GROUP_H,
            Slot::Head { .. } => GROUP_H + GROUP_GAP,
            Slot::Bind(_) => ROW_H,
        }
    }
}

/// Lays `groups` (entry indices per group) out in `columns` columns of
/// about equal height, top to bottom then left to right; a group cut by a
/// column break repeats its header, and a header never ends a column.
fn columns(groups: &[(Group, Vec<usize>)], columns: usize) -> Vec<Vec<Slot>> {
    let mut flat: Vec<Slot> = Vec::new();
    for (g, idx) in groups {
        flat.push(Slot::Head { group: *g, count: idx.len(), continued: false });
        flat.extend(idx.iter().map(|i| Slot::Bind(*i)));
    }
    let total = column_height(&flat);
    let target = total / columns.max(1) as f32 + ROW_H / 2.0;
    let mut out: Vec<Vec<Slot>> = Vec::new();
    let mut col: Vec<Slot> = Vec::new();
    let mut h = 0.0;
    let mut current: Option<(Group, usize)> = None;
    for slot in flat {
        let head = matches!(slot, Slot::Head { .. });
        if let Slot::Head { group, count, .. } = slot {
            current = Some((group, count));
        }
        // A header needs room for its first row too (groups are never
        // empty), so it never ends a column.
        let need = slot.height(col.is_empty()) + if head { ROW_H } else { 0.0 };
        if !col.is_empty() && out.len() + 1 < columns && h + need > target {
            out.push(std::mem::take(&mut col));
            h = 0.0;
            if let (false, Some((group, count))) = (head, current) {
                col.push(Slot::Head { group, count, continued: true });
                h = GROUP_H;
            }
        }
        h += slot.height(col.is_empty());
        col.push(slot);
    }
    out.push(col);
    out
}

fn column_height(col: &[Slot]) -> f32 {
    col.iter().enumerate().map(|(i, c)| c.height(i == 0)).sum()
}

// ── Views ────────────────────────────────────────────────────────────────

/// Rounded keycaps for one chord.
fn keycaps(keys: &[String], palette: &MandaraPalette) -> AnyElement {
    let (edge_alpha, edge_dy, edge_blur) = KEY_EDGE;
    let edge = BoxShadow { color: palette.on_surface.with_alpha(edge_alpha * if palette.is_dark { 1.0 } else { KEY_EDGE_LIGHT }).hsla(), offset: point(px(0.0), px(edge_dy)), blur_radius: px(edge_blur), spread_radius: px(0.0), inset: false };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(KEY_GAP_PX))
        .children(keys.iter().map(|k| {
            div()
                .h(px(KEY_H_PX))
                .min_w(px(KEY_MIN_W_PX))
                .px(px(KEY_PAD_X_PX))
                .rounded(px(crate::style::radius(KEY_RADIUS_PX)))
                .bg(palette.surface_container_high.hsla())
                .shadow(vec![edge.clone()])
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(KEY_TEXT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .font_features(tabular_nums())
                .whitespace_nowrap()
                .child(SharedString::from(k.clone()))
        }))
        .into_any_element()
}

fn group_head(group: Group, count: usize, continued: bool, top: bool, tags: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let label = if continued { tf("{group} (continued)", &[("group", &group.label(tags))]) } else { group.label(tags).to_string() };
    div()
        .h(px(GROUP_H))
        .when(!top, |el| el.mt(px(GROUP_GAP)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX / 2.0))
        .child(icon(group.icon(), GROUP_ICON_PX, accent))
        .child(div().flex_1().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(label))
        .child(caption(count.to_string(), palette).font_features(tabular_nums()))
        .into_any_element()
}

fn bind_row(e: &Entry, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .hover(|s| s.bg(palette.on_surface.with_alpha(ROW_HOVER_ALPHA).hsla()))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(SharedString::from(e.description.clone())))
        .child(keycaps(&e.keys, palette))
        .into_any_element()
}

/// The loaded sheet.
struct Sheet {
    entries: Rc<[Entry]>,
    compositor: Option<CompositorKind>,
    /// Height of the unfiltered columns (kept while typing, so the card
    /// doesn't jump).
    height: f32,
}

/// The filtered column layout for one query over one loaded sheet.
#[derive(Clone)]
struct Laid {
    query: String,
    entries: Rc<[Entry]>,
    cols: Rc<[Vec<Slot>]>,
    shown: usize,
}

fn grouped_indices(entries: &[Entry], query: &str) -> Vec<(Group, Vec<usize>)> {
    let mut idx: Vec<usize> = (0..entries.len()).filter(|i| entries[*i].matches(query)).collect();
    idx.sort_by(|a, b| entries[*a].description.to_lowercase().cmp(&entries[*b].description.to_lowercase()).then_with(|| entries[*a].chord.cmp(&entries[*b].chord)));
    Group::ALL
        .into_iter()
        .filter_map(|g| {
            let list: Vec<usize> = idx.iter().copied().filter(|i| entries[*i].group == g).collect();
            (!list.is_empty()).then_some((g, list))
        })
        .collect()
}

fn sheet_height(cols: &[Vec<Slot>]) -> f32 {
    cols.iter().map(|c| column_height(c)).fold(0.0, f32::max)
}

/// The overlay: one per open sheet, on the focused output.
pub struct SheetView {
    field: Entity<TextField>,
    sheet: Option<Sheet>,
    laid: Option<Laid>,
    opened: Instant,
    /// The output's logical size (the card fits inside it).
    output: (f32, f32),
    focused: bool,
}

#[derive(Default)]
struct OpenSheet(Option<WindowHandle<SheetView>>);
impl Global for OpenSheet {}

impl SheetView {
    fn new(output: (f32, f32), cx: &mut Context<Self>) -> Self {
        let view = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search shortcuts…"),
                true,
                Rc::new(move |_, _, cx| {
                    let _ = view.update(cx, |_, cx| cx.notify());
                }),
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let kind = compositor(cx);
        load(cx, move |entries, cx| {
            let height = sheet_height(&columns(&grouped_indices(&entries, ""), COLUMNS));
            let _ = this.update(cx, |v, cx| {
                v.sheet = Some(Sheet { entries: entries.into(), compositor: kind, height });
                cx.notify();
            });
        });
        Self { field, sheet: None, laid: None, opened: Instant::now(), output, focused: false }
    }

    /// The columns for `query`, laid out once per query (not per frame).
    fn laid(&mut self, query: &str) -> Laid {
        let entries: Rc<[Entry]> = self.sheet.as_ref().map_or_else(|| Rc::from([]), |s| s.entries.clone());
        if let Some(l) = self.laid.as_ref().filter(|l| l.query == query && Rc::ptr_eq(&l.entries, &entries)) {
            return l.clone();
        }
        let groups = grouped_indices(&entries, query);
        let shown = groups.iter().map(|(_, l)| l.len()).sum();
        let laid = Laid { query: query.to_string(), entries, cols: columns(&groups, COLUMNS).into(), shown };
        self.laid = Some(laid.clone());
        laid
    }
}

impl Render for SheetView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        if !self.focused {
            self.focused = true;
            self.field.update(cx, |f, cx| f.focus(window, cx));
        }
        let p = (self.opened.elapsed().as_secs_f32() / FADE_IN.as_secs_f32()).clamp(0.0, 1.0);
        if p < 1.0 {
            window.request_animation_frame();
        }
        let eased = p * p * (3.0 - 2.0 * p);
        let query = self.field.read(cx).value.trim().to_string();
        let Laid { entries, cols, shown, .. } = self.laid(&query);
        let (kind, height) = self.sheet.as_ref().map_or((compositor(cx), EMPTY_H), |s| (s.compositor, s.height));
        let tags = matches!(kind, Some(CompositorKind::MangoWM | CompositorKind::River));
        let accent = readable(palette.primary, &palette);
        let subtitle = match kind {
            Some(k) => tf("{n} shortcuts · Mandara and {compositor}", &[("n", &entries.len()), ("compositor", &format!("{k:?}"))]),
            None => tf("{n} shortcuts", &[("n", &entries.len())]),
        };
        let trailing = (!query.is_empty()).then(|| chip("search", tf("{n} found", &[("n", &shown)]), accent, &palette).into_any_element());
        let room = self.output.1 * SCREEN_SHARE_H - (PAD * 2.0 + HEADER_H + GAP + CONTROL_H + GAP);
        let sheet_h = height.clamp(EMPTY_H, MAX_SHEET_H.min(room.max(EMPTY_H)));
        let body: AnyElement = if self.sheet.is_none() {
            div().h(px(sheet_h)).into_any_element()
        } else if entries.is_empty() {
            empty_state("keyboard", t("No shortcuts found"), t("Mandara's shortcuts and your compositor's appear here"), sheet_h, &palette).into_any_element()
        } else if shown == 0 {
            empty_state("search", t("Nothing matches"), tf("No shortcut matches \u{201c}{query}\u{201d}", &[("query", &query)]), sheet_h, &palette).into_any_element()
        } else {
            div()
                .id("keybinds-sheet")
                .h(px(sheet_h))
                .overflow_y_scroll()
                .flex()
                .items_start()
                .gap(px(COLUMN_GAP_PX))
                .children(cols.iter().map(|col| {
                    div().flex_1().min_w(px(0.0)).flex().flex_col().children(col.iter().enumerate().map(|(i, slot)| match slot {
                        Slot::Head { group, count, continued } => group_head(*group, *count, *continued, i == 0, tags, &palette),
                        Slot::Bind(ix) => bind_row(&entries[*ix], &palette),
                    }))
                }))
                // Fewer columns than the layout holds keep their width.
                .children((cols.len()..COLUMNS).map(|_| div().flex_1()))
                .into_any_element()
        };
        let card = div()
            .id("keybinds-card")
            .w(px(SHEET_W.min(self.output.0 - SCREEN_MARGIN_PX * 2.0)))
            .p(px(PAD))
            .mt(px(RISE_PX * (1.0 - eased)))
            .rounded(px(crate::style::radius(CARD_RADIUS_PX)))
            .bg(palette.surface_container_low.hsla())
            .shadow([rim(&palette), elevation(&palette, eased)].concat())
            .flex()
            .flex_col()
            .gap(px(GAP))
            // Presses on the card stay on it (outside closes the sheet).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header("keyboard", accent, t("Shortcuts"), subtitle, &palette, trailing))
            .child(div().flex().child(search_box("keybinds-clear", &self.field, &query, &palette)))
            .child(body);
        let scrim = palette.surface_dim.mix(Color::hex(SCRIM_BASE_HEX), SCRIM_ALPHA).with_alpha(SCRIM_ALPHA * eased);
        div()
            .size_full()
            .bg(scrim.hsla())
            .opacity(eased)
            .font_family(fonts(cx).ui.clone())
            .text_color(palette.on_surface.hsla())
            .flex()
            .items_center()
            .justify_center()
            .capture_key_down(|e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    close(cx);
                    window.remove_window();
                }
            })
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                close(cx);
                window.remove_window();
            })
            .child(card)
    }
}

fn is_open(cx: &App) -> bool {
    cx.try_global::<OpenSheet>().and_then(|o| o.0.as_ref()).is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

/// Opens the sheet on the focused output (with the keyboard, like the
/// launcher).
fn open(cx: &mut App) {
    if is_open(cx) {
        return;
    }
    let (display, (w, h)) = crate::outputs::overlay_target(cx);
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Launcher, gpui::size(px(w), px(h)), display).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    let handle = cx.open_window(spec.window_options(), move |_, cx| cx.new(|cx| SheetView::new((w, h), cx))).map_err(|e| tracing::error!("shortcuts sheet: {e:#}")).ok();
    cx.set_global(OpenSheet(handle));
}

/// Forgets the open sheet (its window removes itself) or closes it.
fn close(cx: &mut App) {
    let Some(handle) = cx.try_global::<OpenSheet>().and_then(|o| o.0) else { return };
    cx.set_global(OpenSheet(None));
    // Already being removed by its own handler, or closed from outside.
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

fn set_open(open_: Option<bool>, cx: &mut App) -> serde_json::Value {
    let want = open_.unwrap_or(!is_open(cx));
    if want {
        open(cx);
    } else {
        close(cx);
    }
    json!({ "open": want })
}

fn bar_view(palette: &MandaraPalette, _: &mut Window, _: &mut App) -> AnyElement {
    icon("keyboard", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface).into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Shortcuts cheatsheet"), tk("Every keyboard shortcut, Mandara's and your compositor's, in one searchable sheet"), Some(Zone::Right)).opt_in(), |window, cx, _| {
        let bar = fn_view(None, bar_view, window, cx);
        PluginInstance {
            bar: Some(bar),
            on_click: Some(Rc::new(|_, cx| {
                set_open(None, cx);
            })),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new_output("keybinds list", tk("Every shortcut: chord, keys, action, description, group and source file (JSON)"), |_, cx| {
            let (tx, rx) = async_channel::bounded(1);
            load(cx, move |entries, _| {
                let _ = tx.try_send(serde_json::to_string(&entries).map_err(|e| e.to_string()));
            });
            Ok(Output::Pending(rx))
        })
        .example("keybinds list"),
    );
    r.command(ID, CommandSpec::new("keybinds show", tk("Open the shortcuts cheatsheet"), |_, cx| Ok(set_open(Some(true), cx))).example("keybinds show"));
    r.command(ID, CommandSpec::new("keybinds hide", tk("Close the shortcuts cheatsheet"), |_, cx| Ok(set_open(Some(false), cx))).example("keybinds hide"));
    r.command(ID, CommandSpec::new("keybinds toggle", tk("Open or close the shortcuts cheatsheet (bind this to a key yourself)"), |_, cx| Ok(set_open(None, cx))).example("keybinds toggle"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(group: Group, desc: &str) -> Entry {
        Entry::new("super+x".into(), "x".into(), desc.into(), group, "mandara".into(), None)
    }

    #[test]
    fn groups_follow_sheet_order_and_filter() {
        let entries = vec![entry(Group::Apps, "Run firefox"), entry(Group::Mandara, "Open the launcher"), entry(Group::Apps, "Run alacritty"), entry(Group::Windows, "Close window")];
        let g = grouped_indices(&entries, "");
        assert_eq!(g.iter().map(|(g, _)| *g).collect::<Vec<_>>(), [Group::Mandara, Group::Windows, Group::Apps]);
        assert_eq!(g[2].1, [2, 0], "sorted by description");
        let g = grouped_indices(&entries, "run");
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn columns_balance_and_repeat_headers() {
        let groups = vec![(Group::Mandara, (0..12).collect::<Vec<_>>()), (Group::Windows, (12..18).collect()), (Group::Apps, (18..21).collect())];
        let cols = columns(&groups, 3);
        assert_eq!(cols.len(), 3);
        let binds: usize = cols.iter().flatten().filter(|c| matches!(c, Slot::Bind(_))).count();
        assert_eq!(binds, 21, "every bind is placed once");
        for col in &cols {
            assert!(matches!(col.first(), Some(Slot::Head { .. })), "each column starts with a header");
            assert!(!matches!(col.last(), Some(Slot::Head { .. })), "no header ends a column");
        }
        assert!(cols.iter().skip(1).any(|c| matches!(c.first(), Some(Slot::Head { continued: true, .. }))), "a long group continues");
        let heights: Vec<f32> = cols.iter().map(|c| column_height(c)).collect();
        let (lo, hi) = heights.iter().fold((f32::MAX, 0.0f32), |(lo, hi), h| (lo.min(*h), hi.max(*h)));
        assert!(hi - lo <= GROUP_H + GROUP_GAP + ROW_H * 2.0, "{heights:?}");
        assert!(columns(&[], 3).iter().all(Vec::is_empty));
    }
}
