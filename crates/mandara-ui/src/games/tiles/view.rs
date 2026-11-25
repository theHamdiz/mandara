//! The 2048 board: one view for the bar's panel and the desktop widget.
//!
//! Tiles slide to where the last move took them and pop when they merge or
//! appear, on the shell's springs; frames are requested only while a tile
//! is still moving (reduced motion, or the setting off: they jump). Keys:
//! arrows or WASD move, U or Ctrl+Z undoes, Escape lets go of the keyboard
//! on the desktop. A drag with the mouse or a swipe with a finger moves too.

use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, FocusHandle, FontWeight, HitboxBehavior, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, TouchDragEvent, TouchPhase, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::super::swipe::{SWIPE_PX, Swipe};
use super::board::{Dir, Game, MAX_CELLS, WIN_EXPONENT};
use super::model::Tiles;
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::icon;

pub use super::super::sudoku::view::{GameStyle, Surface};

/// Reference layout (logical px at scale 1).
pub const REF_W: f32 = 340.0;
const PAD: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_H: f32 = 48.0;
const BOARD_PAD: f32 = 8.0;
const TILE_GAP: f32 = 8.0;
const TILE_RADIUS: f32 = 10.0;
const BOARD_RADIUS: f32 = 16.0;
const BOX_RADIUS: f32 = 10.0;
const BOX_PAD_X: f32 = 12.0;
const BOX_LABEL: f32 = 9.5;
const BOX_VALUE: f32 = 16.0;
const TITLE: f32 = 24.0;
const BUTTON: f32 = 34.0;
const OVERLAY_TITLE: f32 = 22.0;
const OVERLAY_TEXT: f32 = 12.5;
const CHIP_H: f32 = 32.0;
const CHIP_PAD_X: f32 = 14.0;
const HOVER_OPACITY: f32 = 0.86;
/// Tile numbers keep at least this contrast (WCAG large text).
const TEXT_CONTRAST: f32 = 3.0;
const SLOT_ALPHA: f32 = 0.07;
const OVERLAY_ALPHA: f32 = 0.9;
/// Tile text as a share of the tile, by the number of digits.
const TEXT_FRAC: [f32; 6] = [0.48, 0.48, 0.42, 0.34, 0.27, 0.23];
/// Tile colour: how far toward the accent the smallest and the 2048 tile go.
const RAMP_START: f32 = 0.18;
const RAMP_SPAN: f32 = 0.88;
/// Past 2048, each doubling turns the hue this far.
const HUE_STEP_DEG: f32 = 22.0;
/// Small tiles stand off the board: lighter in dark themes, deeper in light ones.
const LIGHTNESS_SWING: f32 = 0.08;
/// Light themes deepen small tiles less (they'd turn grey).
const LIGHT_SWING_SHARE: f32 = 0.4;
/// Springs: tiles glide critically damped; pops overshoot once.
const SLIDE: SpringConfig = SpringConfig::new(900.0, 60.0, 1.0);
const POP: SpringConfig = SpringConfig::new(700.0, 22.0, 1.0);
/// A merged tile starts a little large, a new one from nothing.
const MERGE_FROM: f32 = 1.18;
const SPAWN_FROM: f32 = 0.0;
/// Below these a tile has arrived (cell units, scale).
const REST_POS: f32 = 0.004;
const REST_SCALE: f32 = 0.004;
/// Element-id slots.
const ID_UNDO: usize = 0;
const ID_NEW: usize = 1;
const ID_KEEP: usize = 2;
const ID_AGAIN: usize = 3;

/// The view's whole size at width scale `u`.
pub fn size(u: f32) -> (f32, f32) {
    (REF_W * u, (2.0 * PAD + HEADER_H + GAP + (REF_W - 2.0 * PAD)) * u)
}

/// One tile as drawn: where it is (cell units), how large, and whether it
/// is a merged tile's partner sliding in to disappear.
struct Drawn {
    id: u32,
    exp: u8,
    x: SpringValue<f32>,
    y: SpringValue<f32>,
    scale: SpringValue<f32>,
    ghost: bool,
}

impl Drawn {
    fn at(id: u32, exp: u8, cell: usize, side: usize, scale_from: f32) -> Self {
        let (x, y) = ((cell % side) as f32, (cell / side) as f32);
        let spring = |v: f32, c: SpringConfig, rest: f32| SpringValue::new(v, c).with_rest_epsilon(rest);
        let mut scale = spring(scale_from, POP, REST_SCALE);
        scale.set_target(1.0);
        Self { id, exp, x: spring(x, SLIDE, REST_POS), y: spring(y, SLIDE, REST_POS), scale, ghost: false }
    }

    fn moving(&self) -> bool {
        self.x.is_animating() || self.y.is_animating() || self.scale.is_animating()
    }
}

pub struct TilesView {
    model: Entity<Tiles>,
    focus: FocusHandle,
    surface: Surface,
    opened: bool,
    input: Option<(f32, f32)>,
    drawn: Vec<Drawn>,
    /// The model's epoch and move this view shows.
    seen: (u64, u64),
    clock: FrameClock,
    swipe: Swipe,
    /// The sidebar card's laid-out width.
    measured: std::rc::Rc<std::cell::Cell<f32>>,
    /// The model, theme (and config) observers and the model's hold on
    /// this view, dropped with it.
    _subscriptions: Vec<gpui::Subscription>,
}

impl TilesView {
    pub fn new(model: Entity<Tiles>, surface: Surface, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::with_capacity(4);
        subscriptions.push(crate::games::join_tiles(cx));
        subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()));
        if matches!(surface, Surface::Desktop { .. }) {
            subscriptions.push(cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()));
        }
        Self { model, focus: cx.focus_handle(), surface, opened: false, input: None, drawn: Vec::with_capacity(MAX_CELLS), seen: (u64::MAX, 0), clock: FrameClock::new(), swipe: Swipe::default(), measured: std::rc::Rc::new(std::cell::Cell::new(super::super::SIDEBAR_WIDTH_PX)), _subscriptions: subscriptions }
    }

    fn geometry(&self, cx: &App) -> (f32, GameStyle) {
        if matches!(self.surface, Surface::Sidebar) {
            return (self.measured.get().max(1.0) / REF_W, GameStyle::Solid);
        }
        let Surface::Desktop { widget } = &self.surface else { return (1.0, GameStyle::Solid) };
        let cfg = crate::state::config(cx).read(cx).get();
        cfg.desktop.widgets.get(widget).map_or((1.0, GameStyle::default()), |w| {
            let o: mandara_config::desktop_cards::GameCardOptions = mandara_config::desktop_cards::as_card(w).map(mandara_config::desktop_cards::options).unwrap_or_default();
            (o.width(mandara_config::desktop_cards::TILES) / REF_W * w.place.scale(), o.style)
        })
    }

    fn slide(&mut self, dir: Dir, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.slide(dir, cx));
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match key_action(&k.key, k.modifiers.control || k.modifiers.platform) {
            Some(TileKey::Move(dir)) => self.slide(dir, cx),
            Some(TileKey::Undo) => {
                self.model.update(cx, |m, cx| m.undo(cx));
            }
            Some(TileKey::Release) if matches!(self.surface, Surface::Desktop { .. }) => window.blur(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Brings the drawn tiles in line with the model: animates the move
    /// that came after the one shown, else snaps to the board.
    fn sync(&mut self, model: &Tiles, animate: bool) {
        let Some(g) = &model.game else {
            self.drawn.clear();
            return;
        };
        let side = usize::from(g.side);
        let next = model.last.as_ref().filter(|(seq, _)| animate && self.seen.0 == model.epoch && *seq == self.seen.1 + 1);
        match next {
            Some((seq, step)) => {
                self.seen.1 = *seq;
                self.drawn.retain(|d| !d.ghost);
                for slide in &step.slides {
                    if let Some(d) = self.drawn.iter_mut().find(|d| d.id == slide.id) {
                        d.x.set_target((usize::from(slide.to) % side) as f32);
                        d.y.set_target((usize::from(slide.to) / side) as f32);
                        d.ghost = slide.absorbed;
                    }
                }
                for id in &step.merged {
                    if let Some(d) = self.drawn.iter_mut().find(|d| d.id == *id) {
                        d.scale.snap_to(MERGE_FROM);
                        d.scale.set_target(1.0);
                    }
                }
                if let Some((id, cell)) = step.spawned {
                    self.drawn.push(Drawn::at(id, g.cells[usize::from(cell)], usize::from(cell), side, SPAWN_FROM));
                }
                // Values follow the board (a merged tile shows its new value).
                for d in self.drawn.iter_mut().filter(|d| !d.ghost) {
                    if let Some(i) = g.ids[..g.len()].iter().position(|&id| id == d.id) {
                        d.exp = g.cells[i];
                    }
                }
            }
            None if self.seen.0 == model.epoch && model.last.as_ref().is_none_or(|(seq, _)| *seq == self.seen.1) => {}
            None => {
                self.seen = (model.epoch, model.last.as_ref().map_or(0, |(seq, _)| *seq));
                self.drawn.clear();
                for i in 0..g.len() {
                    if g.cells[i] != 0 {
                        self.drawn.push(Drawn::at(g.ids[i], g.cells[i], i, side, 1.0));
                    }
                }
            }
        }
        if !animate {
            for d in &mut self.drawn {
                d.x.snap_to_target();
                d.y.snap_to_target();
                d.scale.snap_to_target();
            }
        }
    }

    /// Steps the springs; whether another frame is needed.
    fn step(&mut self, now: Instant) -> bool {
        let dt = self.clock.tick(now);
        let mut moving = false;
        for d in &mut self.drawn {
            d.x.step(dt);
            d.y.step(dt);
            d.scale.step(dt);
            moving |= d.moving();
        }
        // A merged tile's partner leaves once it has arrived.
        self.drawn.retain(|d| !d.ghost || d.moving());
        if !moving {
            self.clock.idle();
        }
        moving
    }
}

/// What a key does on the board.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TileKey {
    Move(Dir),
    Undo,
    /// Escape: the desktop widget lets go of the keyboard.
    Release,
}

/// Arrows or WASD move; U or Ctrl+Z undo (`command`: Ctrl or Super held).
pub fn key_action(key: &str, command: bool) -> Option<TileKey> {
    if command {
        return (key == "z").then_some(TileKey::Undo);
    }
    Some(match key {
        "left" | "a" => TileKey::Move(Dir::Left),
        "right" | "d" => TileKey::Move(Dir::Right),
        "up" | "w" => TileKey::Move(Dir::Up),
        "down" | "s" => TileKey::Move(Dir::Down),
        "u" => TileKey::Undo,
        "escape" => TileKey::Release,
        _ => return None,
    })
}

/// The colour of a tile with exponent `exp`: from a quiet tint to the
/// accent at 2048, turning the hue past it.
pub fn tile_color(exp: u8, accent: Color, p: &MandaraPalette) -> Color {
    let t = (f32::from(exp.saturating_sub(1)) / f32::from(WIN_EXPONENT - 1)).min(1.0);
    let base = p.surface_container_high.mix(accent, RAMP_START + RAMP_SPAN * t);
    let swing = if p.is_dark { LIGHTNESS_SWING } else { -LIGHTNESS_SWING * LIGHT_SWING_SHARE } * (1.0 - t);
    let c = base.lighten(swing);
    if exp > WIN_EXPONENT { c.rotate_hue(HUE_STEP_DEG * f32::from(exp - WIN_EXPONENT)) } else { c }
}

/// A tile's number colour: the palette's light or dark text, pushed until
/// it reads on the tile.
pub fn tile_text(bg: Color, p: &MandaraPalette) -> Color {
    mandara_theme::catalog::ensure_contrast(bg.best_on(p.on_primary, p.on_surface), bg, TEXT_CONTRAST)
}

/// Every tile's number, by exponent (no formatting while tiles move).
const LABELS: [&str; 18] = ["", "2", "4", "8", "16", "32", "64", "128", "256", "512", "1024", "2048", "4096", "8192", "16384", "32768", "65536", "131072"];

/// A tile's number as text.
fn label(exp: u8) -> &'static str {
    LABELS[usize::from(exp).min(LABELS.len() - 1)]
}

impl Render for TilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let (u, style) = self.geometry(cx);
        let (w, h) = size(u);
        if let Surface::Panel(p) = &self.surface {
            if p.progress() > 0.0 {
                if !self.opened {
                    self.opened = true;
                    self.focus.focus(window, cx);
                }
            } else {
                self.opened = false;
            }
        } else if matches!(self.surface, Surface::Desktop { .. }) {
            crate::cards::now_playing::card_input(&mut self.input, w, h, window);
        }
        let s = |v: f32| v * u;
        let model = self.model.clone();
        let m = model.read(cx);
        let animate = m.settings.animations && !mandara_core::spring::motion_profile().reduce;
        self.sync(m, animate);
        if self.step(Instant::now()) {
            window.request_animation_frame();
        }
        let accent = m.settings.accent.color(&palette);
        let cx = &*cx;
        let fonts = crate::fonts::fonts(cx);
        let inner = s(REF_W - 2.0 * PAD);
        let body: AnyElement = match &m.game {
            None => div().flex_1().flex().items_center().justify_center().text_color(palette.on_surface_variant.hsla()).child(t("Loading…")).into_any_element(),
            Some(g) => {
                let header = self.header(m, g, accent, &palette, u, cx);
                let board = self.board(g, accent, &palette, u, inner, cx);
                div().flex().flex_col().gap(px(s(GAP))).child(header).child(board).into_any_element()
            }
        };
        let plate = match (style, &self.surface) {
            (_, Surface::Panel(_)) => div(),
            (_, Surface::Sidebar) => div().rounded(px(crate::style::card_radius())).bg(palette.surface_container.hsla()),
            (GameStyle::Minimal, _) => div(),
            (GameStyle::Glass, _) => crate::cards::now_playing::glass(&palette, crate::style::card_radius() * u, 1.0),
            (GameStyle::Solid, _) => div().rounded(px(crate::style::card_radius() * u)).bg(palette.surface_container.hsla()).shadow(crate::plugins::panel_fx::elevation(&palette, 1.0)),
        };
        let root = div()
            .id("tiles")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .font_family(fonts.ui.clone())
            .text_color(palette.on_surface.hsla())
            .child(plate.w(px(w)).h(px(h)).p(px(s(PAD))).child(body));
        match self.surface {
            Surface::Panel(_) => root.size_full(),
            Surface::Desktop { .. } => root.size_full().p(px(crate::cards::SHADOW_PAD_PX)),
            Surface::Sidebar => root.relative().w_full().h(px(h)).child(super::super::measure_width(self.measured.clone())),
        }
    }
}

impl TilesView {
    fn header(&self, m: &Tiles, g: &Game, accent: Color, p: &MandaraPalette, u: f32, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let score_box = |label: SharedString, v: u64, lit: bool| {
            div()
                .h_full()
                .px(px(s(BOX_PAD_X)))
                .rounded(px(s(BOX_RADIUS)))
                .bg(p.surface_container_high.hsla())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(div().text_size(px(s(BOX_LABEL))).text_color(p.on_surface_variant.hsla()).child(label))
                .child(div().text_size(px(s(BOX_VALUE))).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(if lit { accent.hsla() } else { p.on_surface.hsla() }).child(SharedString::from(v.to_string())))
        };
        let button = |id: usize, glyph: &'static str, enabled: bool| {
            div()
                .id(("tiles-button", id))
                .size(px(s(BUTTON)))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(p.surface_container_high.hsla())
                .hover(|st| st.opacity(HOVER_OPACITY))
                .when(!enabled, |el| el.opacity(HOVER_OPACITY / 2.0))
                .child(icon(glyph, s(BUTTON) * crate::kit::metrics::ICON_BUTTON_GLYPH, p.on_surface_variant))
        };
        let can_undo = m.settings.undo && g.undo.is_some();
        div()
            .h(px(s(HEADER_H)))
            .row()
            .items_center()
            .gap(px(s(TILE_GAP)))
            // The game's name is a number: it reads the same in every language.
            .child(div().flex_1().text_size(px(s(TITLE))).font_weight(FontWeight::BOLD).text_color(accent.hsla()).font_features(tabular_nums()).child("2048"))
            .child(score_box(t("Score").into(), g.score, false))
            .child(score_box(t("Best").into(), m.best(), m.new_best))
            .when(m.settings.undo, |el| {
                el.child(button(ID_UNDO, "undo", can_undo).on_click(cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.model.update(cx, |m, cx| m.undo(cx));
                })))
            })
            .child(button(ID_NEW, "refresh", true).on_click(cx.listener(|this, _, window, cx| {
                this.focus.focus(window, cx);
                this.model.update(cx, |m, cx| m.new_game(None, cx));
            })))
            .into_any_element()
    }

    fn board(&self, g: &Game, accent: Color, p: &MandaraPalette, u: f32, inner: f32, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let side = usize::from(g.side);
        let pad = s(BOARD_PAD);
        let gap = s(TILE_GAP) * 4.0 / side as f32;
        let tile = (inner - 2.0 * pad - gap * (side as f32 - 1.0)) / side as f32;
        let radius = s(TILE_RADIUS) * 4.0 / side as f32;
        let at = |v: f32| pad + v * (tile + gap);
        let slots = (0..g.len()).map(|i| {
            div().absolute().left(px(at((i % side) as f32))).top(px(at((i / side) as f32))).size(px(tile)).rounded(px(radius)).bg(p.on_surface.with_alpha(SLOT_ALPHA).hsla())
        });
        // Partners sliding in to merge are drawn under the tile they join.
        let order = self.drawn.iter().filter(|d| d.ghost).chain(self.drawn.iter().filter(|d| !d.ghost));
        let tiles = order.map(|d| {
            let k = d.scale.value().max(0.0);
            let size = tile * k;
            let (x, y) = (at(d.x.value()) + (tile - size) / 2.0, at(d.y.value()) + (tile - size) / 2.0);
            let bg = tile_color(d.exp, accent, p);
            let digits = label(d.exp).len().min(TEXT_FRAC.len() - 1);
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .size(px(size))
                .rounded(px(radius * k))
                .bg(bg.hsla())
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(size * TEXT_FRAC[digits]))
                .font_weight(FontWeight::BOLD)
                .font_features(tabular_nums())
                .text_color(tile_text(bg, p).hsla())
                .child(label(d.exp))
        });
        let overlay = if g.showing_win() {
            Some(self.overlay(t("You made 2048!").into(), tf("Score {score}", &[("score", &g.score)]), [(ID_KEEP, t("Keep going").into(), true), (ID_AGAIN, t("New game").into(), false)], accent, p, u, cx))
        } else if g.over() {
            Some(self.overlay(t("No moves left").into(), tf("Score {score}", &[("score", &g.score)]), [(ID_AGAIN, t("Try again").into(), true), (usize::MAX, SharedString::default(), false)], accent, p, u, cx))
        } else {
            None
        };
        let view = cx.entity().downgrade();
        div()
            .id("tiles-board")
            .relative()
            .w(px(inner))
            .h(px(inner))
            .rounded(px(s(BOARD_RADIUS)))
            .bg(p.surface_container_high.hsla())
            .children(slots)
            .children(tiles)
            .when_some(overlay, |el, o| el.child(o))
            // Mouse drags and finger swipes.
            .child(
                canvas(
                    |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
                    move |_b: Bounds<Pixels>, hitbox, window, _| {
                        let (touch, moved, up) = (view.clone(), view.clone(), view.clone());
                        window.on_mouse_event(move |e: &TouchDragEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            let _ = touch.update(cx, |this, cx| match e.phase {
                                TouchPhase::Started if hitbox.is_hovered(window) && !window.default_prevented() => {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.swipe.press(f32::from(e.start_position.x), f32::from(e.start_position.y));
                                }
                                TouchPhase::Moved if this.swipe.active() => {
                                    if let Some(dir) = this.swipe.moved(f32::from(e.position.x), f32::from(e.position.y), SWIPE_PX) {
                                        this.slide(dir, cx);
                                    }
                                }
                                TouchPhase::Ended | TouchPhase::Cancelled => this.swipe.release(),
                                _ => {}
                            });
                        });
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && e.pressed_button == Some(MouseButton::Left) {
                                let _ = moved.update(cx, |this, cx| {
                                    if let Some(dir) = this.swipe.moved(f32::from(e.position.x), f32::from(e.position.y), SWIPE_PX) {
                                        this.slide(dir, cx);
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                let _ = up.update(cx, |this, _| this.swipe.release());
                            }
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                this.swipe.press(f32::from(e.position.x), f32::from(e.position.y));
                cx.notify();
            }))
            .into_any_element()
    }

    fn overlay(&self, title: SharedString, line: String, buttons: [(usize, SharedString, bool); 2], accent: Color, p: &MandaraPalette, u: f32, cx: &Context<Self>) -> AnyElement {
        let s = |v: f32| v * u;
        let chips = buttons.into_iter().filter(|(id, _, _)| *id != usize::MAX).map(|(id, text, main)| {
            div()
                .id(("tiles-choice", id))
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
                .child(text)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.model.update(cx, |m, cx| {
                        if id == ID_KEEP {
                            m.keep_going(cx);
                        } else {
                            m.new_game(None, cx);
                        }
                    });
                }))
        });
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
            .child(div().text_size(px(s(OVERLAY_TITLE))).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(title))
            .child(div().text_size(px(s(OVERLAY_TEXT))).text_color(p.on_surface_variant.hsla()).font_features(tabular_nums()).child(line))
            .child(div().row().gap(px(s(TILE_GAP))).children(chips))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_get_more_colourful_toward_2048_in_both_themes() {
        for p in mandara_theme::catalog::builtin().iter().flat_map(|t| [t.palette(true), t.palette(false)]) {
            let chroma: Vec<f32> = (1..=WIN_EXPONENT).map(|e| tile_color(e, p.primary, &p).chroma()).collect();
            assert!(chroma.first() < chroma.last(), "{chroma:?}");
            // Every tile's number stays readable on it.
            for e in 1..=14 {
                let bg = tile_color(e, p.primary, &p);
                assert!(tile_text(bg, &p).contrast_ratio(bg) >= TEXT_CONTRAST - 0.05, "{e} on {}", p.name);
            }
        }
    }

    #[test]
    fn tile_labels_are_the_values() {
        for e in 1..=super::super::board::MAX_EXPONENT {
            assert_eq!(label(e), super::super::board::value(e).to_string());
        }
    }

    #[test]
    fn keys_map_to_moves_and_undo() {
        assert_eq!(key_action("left", false), Some(TileKey::Move(Dir::Left)));
        assert_eq!(key_action("w", false), Some(TileKey::Move(Dir::Up)));
        assert_eq!(key_action("s", false), Some(TileKey::Move(Dir::Down)));
        assert_eq!(key_action("d", false), Some(TileKey::Move(Dir::Right)));
        assert_eq!(key_action("u", false), Some(TileKey::Undo));
        assert_eq!(key_action("z", true), Some(TileKey::Undo));
        assert_eq!(key_action("a", true), None, "Ctrl+A isn't a move");
        assert_eq!(key_action("escape", false), Some(TileKey::Release));
        assert_eq!(key_action("x", false), None);
    }

    #[test]
    fn a_tile_pops_from_nothing_and_settles() {
        let mut d = Drawn::at(1, 1, 5, 4, SPAWN_FROM);
        assert_eq!((d.x.value(), d.y.value()), (1.0, 1.0));
        assert!(d.moving());
        for _ in 0..200 {
            d.x.step_in(1.0 / 60.0, &mandara_core::spring::MotionProfile::default());
            d.y.step_in(1.0 / 60.0, &mandara_core::spring::MotionProfile::default());
            d.scale.step_in(1.0 / 60.0, &mandara_core::spring::MotionProfile::default());
        }
        assert!(!d.moving());
        assert_eq!(d.scale.value(), 1.0);
    }
}
