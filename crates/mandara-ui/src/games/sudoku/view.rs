//! The Sudoku board, number pad and tools: one view for the bar's panel
//! and the desktop widget (scaled to the widget's width, in its style).
//!
//! Keys: arrows move, 1–9 write (Shift or notes mode: pencil marks),
//! Backspace / Delete / 0 erase, N toggles notes, H hints, Ctrl+Z / Ctrl+Y
//! (or Ctrl+Shift+Z) undo and redo, Escape lets go of the keyboard on the
//! desktop. Mouse and touch: tap a cell, then the pad. The board stays left
//! to right in every language; the rows around it follow the reading order.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::PanelCx;
use mandara_theme::{Color, MandaraPalette};

use super::super::clock::format_ms;
use super::grid::{Difficulty, bit, col, row};
use super::model::Sudoku;
use super::play::{Game, is_set};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::icon;

/// Reference layout (logical px at scale 1): the panel is drawn at this
/// width; a desktop widget scales everything with its own width.
pub const REF_W: f32 = 380.0;
const PAD: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_H: f32 = 28.0;
const PAD_ROW_H: f32 = 46.0;
const TOOLS_H: f32 = 40.0;
/// Lines between cells and between boxes.
const THIN: f32 = 1.0;
const THICK: f32 = 3.0;
const CELL_RADIUS: f32 = 4.0;
/// The frame round the board (the box lines' colour).
const BOARD_FRAME: f32 = 2.0;
/// Pressable things dim a little under the pointer.
const HOVER_OPACITY: f32 = 0.86;
/// The header's button is a little narrower than a chip.
const NEW_PAD_X: f32 = 8.0;
const CHIP_GAP: f32 = 6.0;
const BOARD_RADIUS: f32 = 12.0;
const BUTTON_RADIUS: f32 = 10.0;
const PAD_GAP: f32 = 4.0;
const TOOL_ICON: f32 = 17.0;
const TOOL_LABEL: f32 = 10.0;
const HEAD_TEXT: f32 = 13.0;
const HEAD_ICON: f32 = 15.0;
const PAD_DIGIT: f32 = 19.0;
const PAD_COUNT: f32 = 9.5;
const OVERLAY_TITLE: f32 = 22.0;
const OVERLAY_TEXT: f32 = 12.5;
const CHIP_H: f32 = 30.0;
const CHIP_PAD_X: f32 = 12.0;
/// Digits and pencil marks as a share of a cell.
const DIGIT_FRAC: f32 = 0.56;
const NOTE_FRAC: f32 = 0.25;
/// Shades.
const SELECTED_ALPHA: f32 = 0.34;
const SAME_ALPHA: f32 = 0.17;
const PEER_ALPHA: f32 = 0.1;
const LINE_ALPHA: f32 = 0.1;
const BOX_LINE_ALPHA: f32 = 0.6;
const MUTED_ALPHA: f32 = 0.38;
const OVERLAY_ALPHA: f32 = 0.94;
const NOTES_ON_ALPHA: f32 = 0.2;
/// The solved wave: how long it runs, how fast it spreads (cells / s) and
/// how bright its crest is.
const CELEBRATE: Duration = Duration::from_millis(1600);
const WAVE_SPEED: f32 = 14.0;
const WAVE_WIDTH: f32 = 2.2;
const WAVE_ALPHA: f32 = 0.55;
/// The clock redraws this often while it runs and is shown.
const TICK: Duration = Duration::from_secs(1);

const DIFFICULTY_NAMES: [&str; 4] = [tk("Easy"), tk("Medium"), tk("Hard"), tk("Expert")];

pub fn difficulty_name(d: Difficulty) -> String {
    tr(DIFFICULTY_NAMES[d.index()])
}

/// How a desktop widget is drawn.
pub use mandara_config::desktop_cards::GameStyle;

/// Where the view lives.
pub enum Surface {
    Panel(PanelCx),
    Desktop { widget: String },
    /// A card in the sidebar, as wide as the sidebar lets it be.
    Sidebar,
}

/// The view's whole size at width scale `u` (`REF_W * u` wide).
pub fn size(u: f32) -> (f32, f32) {
    let board = board_px(REF_W - 2.0 * PAD);
    (REF_W * u, (2.0 * PAD + HEADER_H + board + PAD_ROW_H + TOOLS_H + 3.0 * GAP) * u)
}

/// Cell side for a board `width` wide (reference px).
fn cell_px(width: f32) -> f32 {
    ((width - 6.0 * THIN - 2.0 * THICK - 2.0 * BOARD_FRAME) / 9.0).floor()
}

fn board_px(width: f32) -> f32 {
    cell_px(width) * 9.0 + 6.0 * THIN + 2.0 * THICK + 2.0 * BOARD_FRAME
}

pub struct SudokuView {
    model: Entity<Sudoku>,
    focus: FocusHandle,
    surface: Surface,
    opened: bool,
    /// The card's input region was set for this size.
    input: Option<(f32, f32)>,
    /// The new-game chooser is open.
    choosing: bool,
    /// A pending keyboard loss on the desktop.
    settle: Option<gpui::Task<()>>,
    /// The sidebar card's laid-out width.
    measured: std::rc::Rc<std::cell::Cell<f32>>,
}

impl SudokuView {
    pub fn new(model: Entity<Sudoku>, surface: Surface, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        if matches!(surface, Surface::Desktop { .. }) {
            cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()).detach();
            // The clock runs while the widget has the keyboard.
            // Some compositors take the keyboard away and give it back
            // around a click, so a loss counts once it lasts.
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.settle = None;
                    return;
                }
                this.settle = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(crate::dismiss::DEACTIVATE_SETTLE).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.settle = None;
                        if !window.is_window_active() {
                            this.model.update(cx, |m, cx| m.desktop_left(cx));
                        }
                    });
                }));
            })
            .detach();
        }
        Self { model, focus: cx.focus_handle(), surface, opened: false, input: None, choosing: false, settle: None, measured: std::rc::Rc::new(std::cell::Cell::new(super::super::SIDEBAR_WIDTH_PX)) }
    }

    fn desktop(&self) -> Option<&str> {
        match &self.surface {
            Surface::Desktop { widget } => Some(widget),
            Surface::Panel(_) | Surface::Sidebar => None,
        }
    }

    /// Played by clicks and keys rather than shown while open (the desktop
    /// widget and the sidebar card): play there keeps the clock going.
    fn card_like(&self) -> bool {
        !matches!(self.surface, Surface::Panel(_))
    }

    /// Scale and style: the panel at reference size, a widget from its options.
    fn geometry(&self, cx: &App) -> (f32, GameStyle) {
        if matches!(self.surface, Surface::Sidebar) {
            return (self.measured.get().max(1.0) / REF_W, GameStyle::Solid);
        }
        let Some(widget) = self.desktop() else { return (1.0, GameStyle::Solid) };
        let cfg = crate::state::config(cx).read(cx).get();
        cfg.desktop.widgets.get(widget).map_or((1.0, GameStyle::default()), |w| {
            let o: mandara_config::desktop_cards::GameCardOptions = mandara_config::desktop_cards::as_card(w).map(mandara_config::desktop_cards::options).unwrap_or_default();
            (o.width(mandara_config::desktop_cards::SUDOKU) / REF_W * w.place.scale(), o.style)
        })
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if self.card_like() && k.key != "escape" {
            self.model.update(cx, |s, cx| s.desktop_input(cx));
        }
        let action = key_action(&k.key, m.control || m.platform, m.shift);
        let handled = action != KeyAction::Ignore && action != KeyAction::Release;
        self.model.update(cx, |s, cx| match action {
            KeyAction::Move(dx, dy) => s.step(dx, dy, cx),
            KeyAction::Digit(d, note) => {
                s.digit(d, note, cx);
            }
            KeyAction::Erase => {
                s.erase(cx);
            }
            KeyAction::Notes => s.toggle_notes_mode(cx),
            KeyAction::Hint => {
                s.hint(cx);
            }
            KeyAction::Undo => {
                s.undo(cx);
            }
            KeyAction::Redo => {
                s.redo(cx);
            }
            KeyAction::Release | KeyAction::Ignore => {}
        });
        if handled {
            cx.stop_propagation();
        } else if action == KeyAction::Release && self.desktop().is_some() {
            // Let go of the keyboard (the panel's Escape closes the panel).
            window.blur(cx);
            self.model.update(cx, |s, cx| s.desktop_left(cx));
            cx.stop_propagation();
        }
    }
}

/// What a key does on the board.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Move(i32, i32),
    /// A digit; `true`: as a pencil mark.
    Digit(u8, bool),
    Erase,
    Notes,
    Hint,
    Undo,
    Redo,
    /// Escape: the desktop widget lets go of the keyboard.
    Release,
    Ignore,
}

/// The board's keys (`command`: Ctrl or Super held).
pub fn key_action(key: &str, command: bool, shift: bool) -> KeyAction {
    if command {
        return match key {
            "z" if shift => KeyAction::Redo,
            "z" => KeyAction::Undo,
            "y" => KeyAction::Redo,
            _ => KeyAction::Ignore,
        };
    }
    match key {
        "left" => KeyAction::Move(-1, 0),
        "right" => KeyAction::Move(1, 0),
        "up" => KeyAction::Move(0, -1),
        "down" => KeyAction::Move(0, 1),
        "backspace" | "delete" | "0" => KeyAction::Erase,
        "n" => KeyAction::Notes,
        "h" => KeyAction::Hint,
        "escape" => KeyAction::Release,
        // Shifted digits arrive as their symbols on many layouts.
        key => match key.parse::<u8>().ok().filter(|d| (1..=9).contains(d)) {
            Some(d) => KeyAction::Digit(d, shift),
            None => shifted_digit(key).map_or(KeyAction::Ignore, |d| KeyAction::Digit(d, true)),
        },
    }
}

/// The digit of a shifted number key on a US layout (`!` is Shift+1).
fn shifted_digit(key: &str) -> Option<u8> {
    const SHIFTED: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
    SHIFTED.iter().position(|s| *s == key).map(|i| i as u8 + 1)
}

/// What the board shades and colours, from the selection and the settings.
struct Marks {
    selected: Option<usize>,
    digit: u8,
    peers: u128,
}

fn marks(s: &Sudoku, g: &Game) -> Marks {
    let selected = s.selected.map(usize::from);
    let digit = selected.map_or(0, |i| g.values[i]);
    let peers = match selected {
        Some(i) if s.settings.peers => Game::peers_of(i),
        _ => 0,
    };
    Marks { selected, digit, peers }
}

impl Render for SudokuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let (u, style) = self.geometry(cx);
        let (w, h) = size(u);
        if let Surface::Panel(p) = &self.surface {
            // Take the keyboard as the panel opens.
            if p.progress() > 0.0 {
                if !self.opened {
                    self.opened = true;
                    self.focus.focus(window, cx);
                }
            } else {
                self.opened = false;
            }
        } else if self.desktop().is_some() {
            crate::cards::now_playing::card_input(&mut self.input, w, h, window);
        }
        let s = |v: f32| v * u;
        let now = Instant::now();
        // Timers first (they need the app mutably), then the read-only build.
        let (tick, celebrating) = {
            let m = self.model.read(cx);
            let celebrating = m.solved_at.map(|t| now.saturating_duration_since(t)).filter(|e| *e < CELEBRATE && !mandara_core::spring::motion_profile().reduce);
            let tick = (m.clock.running() && m.settings.timer && m.generating.is_none()).then(|| TICK - Duration::from_millis(m.clock.elapsed_ms(now) % 1000));
            (tick, celebrating)
        };
        if let Some(delay) = tick {
            crate::plugins::panel_fx::redraw_in(delay, window, cx);
            if self.card_like() {
                self.model.update(cx, |m, cx| m.desktop_tick(cx));
            }
        }
        if celebrating.is_some() {
            window.request_animation_frame();
        }
        let cx = &*cx;
        let model = self.model.read(cx);
        let accent = model.settings.accent.color(&palette);
        let fonts = crate::fonts::fonts(cx);
        let content: AnyElement = match (&model.game, model.generating) {
            (_, Some(d)) => centered(tf("Dealing a {level} puzzle…", &[("level", &difficulty_name(d))]), s(OVERLAY_TEXT), &palette),
            (None, None) => centered(t("Loading…").to_string(), s(OVERLAY_TEXT), &palette),
            (Some(g), None) => self.game_body(model, g, accent, &palette, u, celebrating, cx),
        };
        let plate = match (style, &self.surface) {
            (_, Surface::Panel(_)) => div(),
            (_, Surface::Sidebar) => div().rounded(px(crate::style::card_radius())).bg(palette.surface_container.hsla()),
            (GameStyle::Glass, _) => crate::cards::now_playing::glass(&palette, crate::style::card_radius() * u, 1.0),
            (GameStyle::Solid, _) => div().rounded(px(crate::style::card_radius() * u)).bg(palette.surface_container.hsla()).shadow(crate::plugins::panel_fx::elevation(&palette, 1.0)),
            (GameStyle::Minimal, _) => div(),
        };
        let root = div()
            .id("sudoku")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.focus.focus(window, cx);
                if this.card_like() {
                    this.model.update(cx, |m, cx| m.desktop_input(cx));
                }
            }))
            .font_family(fonts.ui.clone())
            .text_color(palette.on_surface.hsla())
            .child(plate.w(px(w)).h(px(h)).p(px(s(PAD))).flex().flex_col().gap(px(s(GAP))).child(content));
        match self.surface {
            Surface::Panel(_) => root.size_full(),
            Surface::Desktop { .. } => root.size_full().p(px(crate::cards::SHADOW_PAD_PX)),
            Surface::Sidebar => root.relative().w_full().h(px(h)).child(super::super::measure_width(self.measured.clone())),
        }
    }
}

fn centered(text: String, size: f32, p: &MandaraPalette) -> AnyElement {
    div().flex_1().flex().items_center().justify_center().text_size(px(size)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(text)).into_any_element()
}

impl SudokuView {
    fn game_body(&self, model: &Sudoku, g: &Game, accent: Color, p: &MandaraPalette, u: f32, celebrating: Option<Duration>, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let now = Instant::now();
        let set = model.settings;
        // Header: level, mistakes, clock, new game.
        let mut head = div().h(px(s(HEADER_H))).row().items_center().gap(px(s(GAP))).text_size(px(s(HEAD_TEXT)));
        head = head.child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(difficulty_name(g.difficulty)));
        if set.mistakes {
            head = head.child(
                div()
                    .text_color(if g.mistakes > 0 { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() })
                    .font_features(tabular_nums())
                    .child(tf("Mistakes: {count}", &[("count", &g.mistakes)])),
            );
        }
        if set.timer {
            head = head.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(s(PAD_GAP)))
                    .text_color(if model.clock.running() { p.on_surface.hsla() } else { p.on_surface_variant.hsla() })
                    .child(icon("timer", s(HEAD_ICON), p.on_surface_variant))
                    // Numbers stay left to right.
                    .child(div().font_features(tabular_nums()).child(format_ms(model.clock.elapsed_ms(now)))),
            );
        }
        head = head.child(
            div()
                .id("sudoku-new")
                .row()
                .items_center()
                .gap(px(s(PAD_GAP)))
                .px(px(s(NEW_PAD_X)))
                .h(px(s(HEADER_H)))
                .rounded(px(s(BUTTON_RADIUS)))
                .cursor_pointer()
                .bg(p.surface_container_high.hsla())
                .hover(|st| st.opacity(HOVER_OPACITY))
                .child(icon("plus", s(HEAD_ICON), accent))
                .child(t("New"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.choosing = !this.choosing;
                    cx.notify();
                })),
        );
        let board_w = s(board_px(REF_W - 2.0 * PAD));
        let board = self.board(model, g, accent, p, u, celebrating, cx);
        let overlay = if g.won && celebrating.is_none() {
            let best = model.stats[g.difficulty.index()].best_ms;
            let lines = [
                tf("Solved in {time}", &[("time", &format_ms(g.elapsed_ms.max(model.clock.elapsed_ms(now))))]),
                if best > 0 { tf("Best {time}", &[("time", &format_ms(best))]) } else { String::new() },
            ];
            Some(self.chooser(t("Solved!").to_string(), lines, accent, p, u, false, cx))
        } else if self.choosing {
            Some(self.chooser(t("New game").to_string(), [t("Pick a level").to_string(), String::new()], accent, p, u, true, cx))
        } else {
            None
        };
        let board = div().relative().w(px(board_w)).h(px(board_w)).child(board).when_some(overlay, |el, o| el.child(o));
        div()
            .flex()
            .flex_col()
            .gap(px(s(GAP)))
            .child(head)
            .child(div().flex().justify_center().child(board))
            .child(self.pad(g, accent, p, u, cx))
            .child(self.tools(model, g, accent, p, u, cx))
            .into_any_element()
    }

    fn board(&self, model: &Sudoku, g: &Game, accent: Color, p: &MandaraPalette, u: f32, celebrating: Option<Duration>, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let cell = s(cell_px(REF_W - 2.0 * PAD));
        let m = marks(model, g);
        let set = model.settings;
        let origin = m.selected.unwrap_or(40);
        let base = p.surface_container_high;
        let cell_el = |i: usize| {
            let v = g.values[i];
            let given = g.is_given(i);
            let mut bg = base;
            if is_set(m.peers, i) {
                bg = bg.mix(p.on_surface, PEER_ALPHA);
            }
            if set.same && m.digit != 0 && v == m.digit {
                bg = bg.mix(accent, SAME_ALPHA);
            }
            if m.selected == Some(i) {
                bg = bg.mix(accent, SELECTED_ALPHA);
            }
            if let Some(e) = celebrating {
                // A bright ring spreading from the last cell.
                let dist = ((row(i) as f32 - row(origin) as f32).powi(2) + (col(i) as f32 - col(origin) as f32).powi(2)).sqrt() / WAVE_SPEED;
                let phase = (e.as_secs_f32() - dist) * WAVE_SPEED / WAVE_WIDTH;
                if (0.0..1.0).contains(&phase) {
                    bg = bg.mix(accent, WAVE_ALPHA * (phase * std::f32::consts::PI).sin());
                }
            }
            let conflict = set.conflicts && g.conflicts(i);
            let fg = if conflict { p.urgent_coral } else if given { p.on_surface } else { accent };
            let content: AnyElement = if v != 0 {
                div()
                    .text_size(px(cell * DIGIT_FRAC))
                    .font_weight(if given { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                    .font_features(tabular_nums())
                    .text_color(fg.hsla())
                    .child(SharedString::from(DIGITS[usize::from(v) - 1]))
                    .into_any_element()
            } else if g.notes[i] != 0 {
                notes(g.notes[i], m.digit, cell, accent, p)
            } else {
                div().into_any_element()
            };
            div()
                .id(("sudoku-cell", i))
                .size(px(cell))
                .rounded(px(s(CELL_RADIUS)))
                .bg(bg.hsla())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(content)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.model.update(cx, |m, cx| m.select(i, cx));
                }))
        };
        // Three bands of three boxes of three rows of three cells: the
        // board's colour shows between boxes, a fainter one between cells.
        let boxes = |band: usize| {
            div().ltr_row().gap(px(s(THICK))).children((0..3).map(move |b| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(s(THIN)))
                    .rounded(px(s(CELL_RADIUS)))
                    .bg(p.on_surface.with_alpha(LINE_ALPHA).hsla())
                    .children((0..3).map(move |r| div().ltr_row().gap(px(s(THIN))).children((0..3).map(move |c| cell_el((band * 3 + r) * 9 + b * 3 + c)))))
            }))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(s(THICK)))
            .p(px(s(BOARD_FRAME)))
            .rounded(px(s(BOARD_RADIUS)))
            .bg(p.on_surface.with_alpha(BOX_LINE_ALPHA).hsla())
            .children((0..3).map(boxes))
            .into_any_element()
    }

    fn pad(&self, g: &Game, accent: Color, p: &MandaraPalette, u: f32, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let notes_mode = self.model.read(cx).notes_mode;
        div()
            .h(px(s(PAD_ROW_H)))
            .ltr_row()
            .gap(px(s(PAD_GAP)))
            .children((1..=9u8).map(|d| {
                let done = g.placed(d) >= 9;
                let left = 9usize.saturating_sub(g.placed(d));
                div()
                    .id(("sudoku-pad", usize::from(d)))
                    .flex_1()
                    .h_full()
                    .rounded(px(s(BUTTON_RADIUS)))
                    .bg(if notes_mode { accent.with_alpha(NOTES_ON_ALPHA).hsla() } else { p.surface_container_high.hsla() })
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|st| st.opacity(HOVER_OPACITY))
                    .when(done, |el| el.opacity(MUTED_ALPHA))
                    .child(div().text_size(px(s(PAD_DIGIT))).font_weight(FontWeight::MEDIUM).text_color(accent.hsla()).font_features(tabular_nums()).child(SharedString::from(DIGITS[usize::from(d) - 1])))
                    .child(div().text_size(px(s(PAD_COUNT))).text_color(p.on_surface_variant.hsla()).font_features(tabular_nums()).child(SharedString::from(left.to_string())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus.focus(window, cx);
                        this.model.update(cx, |m, cx| m.digit(d, false, cx));
                    }))
            }))
            .into_any_element()
    }

    fn tools(&self, model: &Sudoku, g: &Game, accent: Color, p: &MandaraPalette, u: f32, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        type Act = fn(&mut Sudoku, &mut Context<Sudoku>) -> bool;
        let hint: Option<(usize, &'static str, SharedString, bool, Act)> = model.settings.hints.then(|| {
            let label: SharedString = if g.hints > 0 { tf("Hint ({count})", &[("count", &g.hints)]).into() } else { t("Hint").into() };
            (TOOL_HINT, "lightbulb", label, !g.won, (|m, cx| m.hint(cx)) as Act)
        });
        let tools: [Option<(usize, &'static str, SharedString, bool, Act)>; 5] = [
            Some((TOOL_UNDO, "undo", t("Undo").into(), !g.undo.is_empty() && !g.won, |m, cx| m.undo(cx))),
            Some((TOOL_REDO, "redo", t("Redo").into(), !g.redo.is_empty() && !g.won, |m, cx| m.redo(cx))),
            Some((TOOL_ERASE, "eraser", t("Erase").into(), !g.won, |m, cx| m.erase(cx))),
            Some((TOOL_NOTES, "pencil", t("Notes").into(), !g.won, |m, cx| {
                m.toggle_notes_mode(cx);
                true
            })),
            hint,
        ];
        let notes_on = model.notes_mode;
        div()
            .h(px(s(TOOLS_H)))
            .row()
            .gap(px(s(PAD_GAP)))
            .children(tools.into_iter().flatten().map(|(id, glyph, label, enabled, act)| {
                let on = id == TOOL_NOTES && notes_on;
                div()
                    .id(("sudoku-tool", id))
                    .flex_1()
                    .h_full()
                    .rounded(px(s(BUTTON_RADIUS)))
                    .when(on, |el| el.bg(accent.with_alpha(NOTES_ON_ALPHA).hsla()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|st| st.bg(p.on_surface.with_alpha(crate::kit::alpha::HOVER).hsla()))
                    .when(!enabled, |el| el.opacity(MUTED_ALPHA))
                    .child(icon(glyph, s(TOOL_ICON), if on { accent } else { p.on_surface_variant }))
                    .child(div().text_size(px(s(TOOL_LABEL))).text_color(p.on_surface_variant.hsla()).child(label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus.focus(window, cx);
                        this.model.update(cx, act);
                    }))
            }))
            .into_any_element()
    }

    /// A card over the board: the solved message, or the new-game levels.
    fn chooser(&self, title: String, lines: [String; 2], accent: Color, p: &MandaraPalette, u: f32, cancel: bool, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let current = self.model.read(cx).settings.difficulty;
        let chips = div().row().flex_wrap().justify_center().gap(px(s(CHIP_GAP))).children(Difficulty::ALL.into_iter().map(|d| {
            let main = d == current;
            div()
                .id(("sudoku-level", d.index()))
                .h(px(s(CHIP_H)))
                .px(px(s(CHIP_PAD_X)))
                .rounded_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(s(OVERLAY_TEXT)))
                .bg(if main { accent.hsla() } else { p.surface_container_high.hsla() })
                .text_color(if main { accent.best_on(p.on_primary, p.on_surface).hsla() } else { p.on_surface.hsla() })
                .hover(|st| st.opacity(HOVER_OPACITY))
                .child(difficulty_name(d))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.choosing = false;
                    this.focus.focus(window, cx);
                    this.model.update(cx, |m, cx| m.new_game(d, None, cx));
                }))
        }));
        div()
            .absolute()
            .inset_0()
            .rounded(px(s(BOARD_RADIUS)))
            .bg(p.surface_container.with_alpha(OVERLAY_ALPHA).hsla())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(s(GAP)))
            .p(px(s(PAD)))
            .child(div().text_size(px(s(OVERLAY_TITLE))).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(title))
            .children(lines.into_iter().filter(|l| !l.is_empty()).map(|l| div().text_size(px(s(OVERLAY_TEXT))).text_color(p.on_surface_variant.hsla()).font_features(tabular_nums()).child(l)))
            .child(chips)
            .when(cancel, |el| {
                el.child(
                    div()
                        .id("sudoku-cancel")
                        .text_size(px(s(OVERLAY_TEXT)))
                        .text_color(p.on_surface_variant.hsla())
                        .cursor_pointer()
                        .child(t("Keep playing"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.choosing = false;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

/// Element-id slots of the tools.
const TOOL_UNDO: usize = 0;
const TOOL_REDO: usize = 1;
const TOOL_ERASE: usize = 2;
const TOOL_NOTES: usize = 3;
const TOOL_HINT: usize = 4;

/// The digits as text (no per-frame formatting).
const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// A cell's pencil marks: a 3 × 3 grid of small digits.
fn notes(mask: u16, selected: u8, cell: f32, accent: Color, p: &MandaraPalette) -> AnyElement {
    let size = cell * NOTE_FRAC;
    let third = cell / 3.0;
    div()
        .size_full()
        .flex()
        .flex_col()
        .children((0..3).map(|r| {
            div().ltr_row().children((0..3).map(move |c| {
                let d = (r * 3 + c + 1) as u8;
                let on = mask & bit(d) != 0;
                div()
                    .w(px(third))
                    .h(px(third))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(size))
                    .font_features(tabular_nums())
                    .text_color(if d == selected { accent.hsla() } else { p.on_surface_variant.hsla() })
                    .when(on, |el| el.child(SharedString::from(DIGITS[usize::from(d) - 1])))
            }))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_fits_its_width_at_every_scale() {
        let inner = REF_W - 2.0 * PAD;
        assert!(board_px(inner) <= inner);
        assert!(board_px(inner) > inner - 9.0);
        let (w, h) = size(1.5);
        assert_eq!(w, REF_W * 1.5);
        assert!(h > w, "taller than wide: the pad and tools sit under the board");
    }

    #[test]
    fn keys_map_to_moves_digits_notes_and_history() {
        assert_eq!(key_action("left", false, false), KeyAction::Move(-1, 0));
        assert_eq!(key_action("down", false, false), KeyAction::Move(0, 1));
        assert_eq!(key_action("7", false, false), KeyAction::Digit(7, false));
        assert_eq!(key_action("7", false, true), KeyAction::Digit(7, true));
        assert_eq!(key_action("&", false, true), KeyAction::Digit(7, true));
        assert_eq!(key_action("0", false, false), KeyAction::Erase);
        assert_eq!(key_action("backspace", false, false), KeyAction::Erase);
        assert_eq!(key_action("n", false, false), KeyAction::Notes);
        assert_eq!(key_action("h", false, false), KeyAction::Hint);
        assert_eq!(key_action("z", true, false), KeyAction::Undo);
        assert_eq!(key_action("z", true, true), KeyAction::Redo);
        assert_eq!(key_action("y", true, false), KeyAction::Redo);
        assert_eq!(key_action("7", true, false), KeyAction::Ignore, "Ctrl+digit is the compositor's");
        assert_eq!(key_action("escape", false, false), KeyAction::Release);
        assert_eq!(key_action("q", false, false), KeyAction::Ignore);
    }

    #[test]
    fn shifted_number_keys_are_notes() {
        assert_eq!(shifted_digit("!"), Some(1));
        assert_eq!(shifted_digit("("), Some(9));
        assert_eq!(shifted_digit("a"), None);
    }
}
