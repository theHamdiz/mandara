//! Studio's segmented choice control: a pill per option on one track.
//! Every choice in Studio uses it: settings rows, plugin settings, the
//! screen recorder's pickers, the desktop widget editor and the login
//! screen designer.
//!
//! An option that can't be picked right now (a codec the recorder in use
//! can't encode, a backend that isn't installed) is dimmed and inert, and
//! its reason is written under the track, once per distinct reason.
//!
//! When every option fits on one line the control is a compact row of
//! pills at their natural width. When they don't, it becomes a balanced
//! grid instead of wrapping greedily (which left one or two orphans on a
//! second row): the fewest rows that hold the options, rows that differ by
//! at most one option (longer rows first), every cell as wide as the
//! longest label so the cells line up in columns, and a shorter row
//! centred under the full ones. Right-to-left languages mirror it: the
//! first option sits at the right of the first row.
//!
//! The width the control may take comes from Studio's known page metrics
//! (the caller works it out), and labels are measured with GPUI's text
//! system in the control's own font and size, so the shape is right in the
//! frame it is first drawn (no measure-then-relayout flicker). Widths are
//! cached per label, size, font and language, so a repaint measures nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowTextSystem, div, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui as _;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;

use mandara_ui::components::ChipDesign;
use mandara_ui::components::chips::Place;
use mandara_ui::components::surfaces::Surface as _;
use mandara_ui::style::ClipRounded as _;

use super::grid::{self, GridFit};
use crate::tokens::*;

/// The outline of Material's segmented group.
const MATERIAL_EDGE_PX: f32 = 1.0;

/// Space between neighbouring pills on a settings or greeter track.
pub const CHOICE_GAP_PX: f32 = 2.0;

/// Sizes of one family of choice controls (the track's look is shared).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChoiceStyle {
    /// Horizontal padding inside each pill.
    pub pad_x: f32,
    /// Label size.
    pub text_px: f32,
    /// Pill height.
    pub pill_h: f32,
    /// Gap between pills and between rows.
    pub gap: f32,
    /// Padding of the track around the pills.
    pub inset: f32,
}

/// A setting row's choice (`Control::Choice`), in pages and the plugin
/// details window.
pub const SETTING_CHOICE: ChoiceStyle = ChoiceStyle { pad_x: SPACE_12_PX, text_px: TEXT_12_5_PX, pill_h: DIM_28_PX, gap: CHOICE_GAP_PX, inset: SPACE_3_PX };
/// A desktop widget editor's choice (shapes, layouts, languages…).
pub const WIDGET_CHOICE: ChoiceStyle = ChoiceStyle { pad_x: SPACE_10_PX, text_px: TEXT_12_PX, pill_h: DIM_28_PX, gap: SPACE_3_PX, inset: SPACE_3_PX };
/// Login screen choices (session, language, styles).
pub const GREETER_CHOICE: ChoiceStyle = ChoiceStyle { pad_x: SPACE_10_PX, text_px: TEXT_12_PX, pill_h: DIM_26_PX, gap: CHOICE_GAP_PX, inset: SPACE_3_PX };

/// Weight labels are measured at: the active pill's, the widest one drawn.
const MEASURE_WEIGHT: FontWeight = FontWeight::MEDIUM;

/// Where a choice sits and how wide it may grow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// The most width the control may take.
    pub width: f32,
    /// It sits under its label (notes start at its leading edge), not
    /// beside it at the row's end (notes end at its trailing edge).
    pub under: bool,
}

impl Room {
    pub fn beside(width: f32) -> Self {
        Room { width, under: false }
    }

    pub fn under(width: f32) -> Self {
        Room { width, under: true }
    }
}

/// How a choice lays out its options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// One row, each pill at its natural width.
    Line,
    /// Balanced rows of equal cells.
    Grid(GridFit),
}

/// The shape for options whose labels are `label_widths` wide, in a
/// control at most `avail` wide.
pub fn shape(label_widths: &[f32], avail: f32, style: &ChoiceStyle) -> Shape {
    let n = label_widths.len();
    if n == 0 {
        return Shape::Line;
    }
    let pills: f32 = label_widths.iter().map(|w| w + 2.0 * style.pad_x).sum();
    let line = pills + style.gap * (n - 1) as f32 + 2.0 * style.inset;
    if line <= avail {
        return Shape::Line;
    }
    // Whole pixels, so a label never wraps inside its cell.
    let cell = (label_widths.iter().copied().fold(0.0, f32::max) + 2.0 * style.pad_x).ceil();
    let inner = avail - 2.0 * style.inset;
    Shape::Grid(grid::fit(inner, CardGrid { min_w: cell, max_w: cell, gap: style.gap }).balanced(n))
}

/// How many options each row holds: as many rows as `cols` columns need,
/// sizes differing by at most one, the longer rows first (16 on 8 → 8 + 8,
/// 15 → 8 + 7, 10 on 4 → 4 + 3 + 3).
pub fn row_sizes(n: usize, cols: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let rows = n.div_ceil(cols.max(1));
    let (base, extra) = (n / rows, n % rows);
    (0..rows).map(|r| base + usize::from(r < extra)).collect()
}

/// The option indices of each row, left to right as drawn: in a
/// right-to-left language each row runs from its right end.
pub fn visual_rows(n: usize, cols: usize, rtl: bool) -> Vec<Vec<usize>> {
    let mut start = 0;
    row_sizes(n, cols)
        .into_iter()
        .map(|len| {
            let mut row: Vec<usize> = (start..start + len).collect();
            start += len;
            if rtl {
                row.reverse();
            }
            row
        })
        .collect()
}

/// How far a row of `len` cells sits in from the grid's leading edge so it
/// is centred under the full rows (half a cell per missing option).
pub fn row_offset(len: usize, fit: &GridFit) -> f32 {
    fit.cols.saturating_sub(len) as f32 * (fit.card_w + fit.gap) / 2.0
}

/// One option.
pub struct ChoiceItem {
    pub id: ElementId,
    pub label: SharedString,
    pub active: bool,
    /// Why it can't be picked right now (translated); `None`: it can.
    pub locked: Option<SharedString>,
    pub on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

impl ChoiceItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        Self { id: id.into(), label: label.into(), active, locked: None, on_click: Box::new(on_click) }
    }

    /// Disabled, with the reason shown under the control.
    pub fn locked(mut self, reason: Option<SharedString>) -> Self {
        self.locked = reason;
        self
    }
}

/// The distinct reasons of the disabled options, in order.
pub fn lock_notes(items: &[ChoiceItem]) -> Vec<SharedString> {
    let mut notes: Vec<SharedString> = Vec::new();
    for reason in items.iter().filter_map(|i| i.locked.as_ref()) {
        if !notes.contains(reason) {
            notes.push(reason.clone());
        }
    }
    notes
}

/// Whether options labelled `labels` fit on one line in `avail` logical px
/// (true until the first frame recorded a text system).
pub fn fits_line(labels: &[SharedString], avail: f32, style: &ChoiceStyle) -> bool {
    let widths: Option<Vec<f32>> = labels.iter().map(|l| label_width(l, style.text_px)).collect();
    widths.is_none_or(|w| shape(&w, avail, style) == Shape::Line)
}

/// A labelled choice: beside its label (`beside` px of room) when it fits
/// on one line there, else under the label, where its grid gets the whole
/// row (`under` px) and so fewer, wider rows. Returns the control and
/// whether it goes under the label.
pub fn placed(items: Vec<ChoiceItem>, beside: f32, under: f32, style: &ChoiceStyle, p: &MandaraPalette) -> (AnyElement, bool) {
    let labels: Vec<SharedString> = items.iter().map(|i| i.label.clone()).collect();
    let fits = fits_line(&labels, beside, style);
    (segmented(if fits { Room::beside(beside) } else { Room::under(under) }, style, items, p), !fits)
}

/// The control: options on a track, one line or a balanced grid that fits
/// `room` (see the module docs).
pub fn segmented(room: Room, style: &ChoiceStyle, items: Vec<ChoiceItem>, p: &MandaraPalette) -> AnyElement {
    segmented_as(mandara_ui::style::chip_design(), room, style, items, p)
}

/// [`segmented`] in `design` (Themes → Components → Chips): Classic's
/// pills on a tonal track, Material's outlined group with dividers, or
/// Connected's separate buttons with small inner corners.
pub fn segmented_as(design: ChipDesign, room: Room, style: &ChoiceStyle, items: Vec<ChoiceItem>, p: &MandaraPalette) -> AnyElement {
    let avail = room.width;
    let widths: Option<Vec<f32>> = items.iter().map(|i| label_width(&i.label, style.text_px)).collect();
    let shape = widths.map_or(Shape::Line, |w| shape(&w, avail, style));
    let notes = lock_notes(&items);
    let rtl = mandara_ui::bidi::rtl();
    let (inset, gap) = match design {
        ChipDesign::Classic => (style.inset, style.gap),
        ChipDesign::Material => (0.0, 0.0),
        ChipDesign::Connected => (0.0, style.gap),
    };
    // Material's group is outlined: its edge takes this much on each side.
    let edge = if design == ChipDesign::Material { MATERIAL_EDGE_PX } else { 0.0 };
    let track_radius = if matches!(shape, Shape::Line) && design == ChipDesign::Material { style.pill_h / 2.0 + edge } else { RADIUS_12_PX };
    let track = match design {
        ChipDesign::Classic => div().p(px(style.inset)).rounded(px(RADIUS_12_PX)).bg(p.surface_container_high.hsla()),
        ChipDesign::Material => div().clip_rounded().rounded(px(track_radius)).border_1().border_color(mandara_ui::components::chips::outline(p).hsla()),
        ChipDesign::Connected => div(),
    };
    let look = Look { design, style, p };
    let control = match shape {
        // A row that wraps only if a font changed under a cached width.
        Shape::Line => {
            let n = items.len();
            track.row().flex_wrap().gap(px(gap)).children(items.into_iter().enumerate().map(|(i, item)| pill(item, None, Place::in_reading_order(i, n, rtl), &look))).into_any_element()
        }
        Shape::Grid(fit) => {
            let n = items.len();
            let mut items: Vec<Option<ChoiceItem>> = items.into_iter().map(Some).collect();
            let rows = visual_rows(n, fit.cols, rtl);
            let row_count = rows.len();
            track
                .flex_none()
                .w(px(fit.width + 2.0 * inset + 2.0 * edge))
                .flex()
                .flex_col()
                .gap(px(gap))
                .children(rows.into_iter().enumerate().map(|(r, row)| {
                    let offset = row_offset(row.len(), &fit);
                    let len = row.len();
                    div()
                        .ltr_row()
                        .gap(px(gap))
                        .when(offset > 0.0, |el| el.ml(px(offset)))
                        // Material: a divider between rows.
                        .when(design == ChipDesign::Material && r + 1 < row_count, |el| el.border_b_1().border_color(mandara_ui::components::chips::outline(p).hsla()))
                        .children(row.into_iter().filter_map(|i| items[i].take()).enumerate().map(|(j, i)| pill(i, Some(fit.card_w), Place::visual(j, len), &look)))
                }))
                .into_any_element()
        }
    };
    if notes.is_empty() {
        return control;
    }
    // The reasons under the track: at its end beside a label, at its start
    // under one.
    div()
        .row()
        .flex_col()
        .when(!room.under, |el| el.cross_end())
        .when(room.under, |el| el.cross_start())
        .gap(px(SPACE_6_PX))
        .child(control)
        .children(notes.into_iter().map(|r| div().max_w(px(super::LOCKED_NOTE_MAX_W_PX)).text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(r))))
        .into_any_element()
}

/// How a choice's options are drawn.
struct Look<'a> {
    design: ChipDesign,
    style: &'a ChoiceStyle,
    p: &'a MandaraPalette,
}

/// One pill: its natural width on a line, `cell` wide (label centred) in a
/// grid, at `place` in its row. A disabled one is dimmed and ignores the
/// pointer.
fn pill(item: ChoiceItem, cell: Option<f32>, place: Place, look: &Look) -> AnyElement {
    let Look { design, style, p } = *look;
    let ChoiceItem { id, label, active, locked, on_click } = item;
    let pill = div().id(id).px(px(style.pad_x)).h(px(style.pill_h));
    let pill = match design {
        ChipDesign::Classic => pill
            .rounded(px(RADIUS_9_PX))
            .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).font_weight(FontWeight::MEDIUM))
            .when(!active, |el| el.text_color(p.on_surface_variant.hsla())),
        // Square options in the outlined group (it clips their corners), a
        // divider before the next one on screen.
        ChipDesign::Material => pill
            .when(!place.right_end, |el| el.border_r_1().border_color(mandara_ui::components::chips::outline(p).hsla()))
            .when(active, |el| el.bg(mandara_ui::components::chips::chosen(p).hsla()).text_color(p.primary.hsla()).font_weight(FontWeight::MEDIUM))
            .when(!active, |el| el.text_color(p.on_surface_variant.hsla())),
        ChipDesign::Connected => pill
            .with_corners(mandara_ui::components::chips::connected_corners(place, active, style.pill_h))
            .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).font_weight(FontWeight::MEDIUM))
            .when(!active, |el| el.bg(p.surface_container_high.hsla()).text_color(p.on_surface_variant.hsla())),
    };
    let pill = pill
        .row()
        .items_center()
        .when_some(cell, |el, w| el.flex_none().w(px(w)).justify_center())
        .text_size(px(style.text_px))
        .child(mandara_ui::bidi::paragraph(label));
    match locked {
        Some(_) => pill.opacity(super::LOCKED_OPACITY).into_any_element(),
        None => pill.cursor_pointer().when(!active, |el| el.hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla()))).on_click(on_click).into_any_element(),
    }
}

/// What labels are measured with this frame: the window's text system and
/// Studio's font family.
struct Measure {
    text: Rc<WindowTextSystem>,
    family: SharedString,
}

/// Cache key: label, size (bits), font family and language (the fallback
/// fonts follow the language).
type WidthKey = (SharedString, u32, SharedString, &'static str);

thread_local! {
    /// The text system and family of the (single) Studio window.
    static MEASURE: RefCell<Option<Measure>> = const { RefCell::new(None) };
    /// Measured label widths.
    static WIDTHS: RefCell<HashMap<WidthKey, f32>> = RefCell::default();
}

/// Records the window's text system and Studio's font for this frame's
/// choices (call at the start of the root render).
pub fn begin_frame(window: &Window, family: SharedString) {
    MEASURE.with(|m| {
        let mut m = m.borrow_mut();
        match m.as_mut() {
            Some(m) if Rc::ptr_eq(&m.text, window.text_system()) => m.family = family,
            _ => *m = Some(Measure { text: window.text_system().clone(), family }),
        }
    });
}

/// `label`'s width at `size` in Studio's font, or `None` before the first
/// frame recorded a text system.
pub fn label_width(label: &SharedString, size: f32) -> Option<f32> {
    MEASURE.with(|m| {
        let m = m.borrow();
        let m = m.as_ref()?;
        let key = (label.clone(), size.to_bits(), m.family.clone(), mandara_i18n::language().code());
        if let Some(w) = WIDTHS.with(|c| c.borrow().get(&key).copied()) {
            return Some(w);
        }
        let font = gpui::Font { weight: MEASURE_WEIGHT, ..gpui::font(m.family.clone()) };
        let run = gpui::TextRun { len: label.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        let w = f32::from(m.text.layout_line(label, px(size), &[run], None).width);
        WIDTHS.with(|c| c.borrow_mut().insert(key, w));
        Some(w)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: ChoiceStyle = SETTING_CHOICE;

    /// Labels of 40 px: 64 px pills, 66 px a step on a line.
    fn labels(n: usize) -> Vec<f32> {
        vec![40.0; n]
    }

    fn line_w(n: usize) -> f32 {
        n as f32 * (40.0 + 2.0 * STYLE.pad_x) + (n - 1) as f32 * STYLE.gap + 2.0 * STYLE.inset
    }

    fn grid(n: usize, avail: f32) -> GridFit {
        match shape(&labels(n), avail, &STYLE) {
            Shape::Grid(f) => f,
            Shape::Line => panic!("{n} options in {avail} px should not fit on a line"),
        }
    }

    #[test]
    fn short_choices_stay_on_one_line() {
        // Off / 3 s / 5 s / 10 s, exactly as wide as the room, or narrower.
        assert_eq!(shape(&labels(4), line_w(4), &STYLE), Shape::Line);
        assert_eq!(shape(&labels(4), 1000.0, &STYLE), Shape::Line);
        assert_eq!(shape(&[], 0.0, &STYLE), Shape::Line);
    }

    #[test]
    fn one_pixel_short_becomes_a_grid() {
        let f = grid(4, line_w(4) - 1.0);
        assert_eq!(f.cols, 2, "4 that don't fit become 2 + 2, not 3 + 1");
        assert_eq!(row_sizes(4, f.cols), [2, 2]);
    }

    #[test]
    fn rows_are_balanced() {
        // Room for 9 cells of 64 px (+2 gap) after the track's insets.
        let avail = 9.0 * 64.0 + 8.0 * STYLE.gap + 2.0 * STYLE.inset;
        for (n, cols, rows) in [(16, 8, vec![8, 8]), (15, 8, vec![8, 7]), (11, 6, vec![6, 5]), (10, 5, vec![5, 5]), (19, 7, vec![7, 6, 6]), (27, 9, vec![9, 9, 9])] {
            let f = grid(n, avail);
            assert_eq!(f.cols, cols, "{n} options");
            assert_eq!(row_sizes(n, f.cols), rows, "{n} options");
        }
        assert_eq!(row_sizes(10, 4), [4, 3, 3]);
        assert!(row_sizes(0, 3).is_empty());
    }

    #[test]
    fn cells_share_the_longest_label_width() {
        let f = match shape(&[30.0, 95.5, 41.0, 60.0, 22.0], 200.0, &STYLE) {
            Shape::Grid(f) => f,
            Shape::Line => panic!("should be a grid"),
        };
        assert_eq!(f.card_w, (95.5f32 + 2.0 * STYLE.pad_x).ceil());
        assert_eq!(f.cols, 1, "one 120 px cell fits in 194 px, two don't");
        // Every full row is the same width, and the grid fits the room.
        assert!(f.width + 2.0 * STYLE.inset <= 200.0);
    }

    #[test]
    fn grid_never_overflows_its_room() {
        for n in 2..=20 {
            let mut avail = 80.0;
            while avail < 1600.0 {
                if let Shape::Grid(f) = shape(&labels(n), avail, &STYLE) {
                    assert!(f.width + 2.0 * STYLE.inset <= avail + 0.01, "{n} in {avail}: {f:?}");
                    let sizes = row_sizes(n, f.cols);
                    assert_eq!(sizes.iter().sum::<usize>(), n);
                    assert!(sizes.iter().all(|&s| s <= f.cols && s + 1 >= f.cols), "{n} in {avail}: {sizes:?}");
                    // Fewest rows: as many as the columns that fit need.
                    let most = grid::fit(avail - 2.0 * STYLE.inset, CardGrid { min_w: f.card_w, max_w: f.card_w, gap: STYLE.gap }).cols;
                    assert_eq!(sizes.len(), n.div_ceil(most), "{n} in {avail}");
                }
                avail += 13.0;
            }
        }
    }

    #[test]
    fn each_lock_reason_is_noted_once() {
        let item = |reason: Option<&'static str>| ChoiceItem::new("x", "x", false, |_, _, _| {}).locked(reason.map(SharedString::from));
        let items = [item(Some("WebM needs AV1")), item(None), item(Some("Not installed")), item(Some("WebM needs AV1"))];
        assert_eq!(lock_notes(&items), [SharedString::from("WebM needs AV1"), SharedString::from("Not installed")]);
        assert!(lock_notes(&[item(None)]).is_empty());
    }

    #[test]
    fn right_to_left_mirrors_each_row() {
        assert_eq!(visual_rows(5, 3, false), [vec![0, 1, 2], vec![3, 4]]);
        assert_eq!(visual_rows(5, 3, true), [vec![2, 1, 0], vec![4, 3]]);
    }

    #[test]
    fn a_short_last_row_is_centred() {
        let f = grid(15, 9.0 * 64.0 + 8.0 * STYLE.gap + 2.0 * STYLE.inset);
        assert_eq!(row_offset(8, &f), 0.0);
        let off = row_offset(7, &f);
        assert_eq!(off, (f.card_w + f.gap) / 2.0);
        // Equal space on both sides of the short row.
        let row7 = 7.0 * f.card_w + 6.0 * f.gap;
        assert!((f.width - row7 - 2.0 * off).abs() < 0.01);
    }
}
