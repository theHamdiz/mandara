//! The Looks tab's previews: each look in miniature, drawn from its own
//! settings and placements (a 1920 × 1080 screen scaled to the card), so
//! the sign-in shows on the side it really takes and the clock in its own
//! face.

use gpui::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_greeter::config::{CardStyle, ClockFace, Element, GreeterConfig, Overlay};
use mandara_theme::{Color, MandaraPalette};

/// The reference screen's width (placements are written for 1920 × 1080).
const REF_W: f32 = 1920.0;
/// The miniature's corner radius.
const THUMB_RADIUS: f32 = 10.0;
/// A digit's width and a line's height, as shares of the clock's size.
const DIGIT_W: f32 = 0.62;
const CLOCK_LINE: f32 = 0.9;
/// Height of text bars, the user dot and the field bar (reference px).
const BAR_H: f32 = 34.0;
const AVATAR: f32 = 64.0;
const FIELD_W: f32 = 340.0;
const FIELD_H: f32 = 44.0;
/// The miniature cards' reference heights.
const CARD_H: f32 = 400.0;
const PLAIN_CARD_H: f32 = 300.0;
/// Meridian's field and line height.
const GOLDEN: f32 = 0.618;
const DAWN: [(f32, f32, f32); 2] = [(0.975, 0.012, 85.0), (0.905, 0.01, 70.0)];
const NIGHT: [(f32, f32, f32); 2] = [(0.205, 0.022, 262.0), (0.125, 0.018, 258.0)];
const SWATCH_DEG: f32 = 160.0;
const ACCENT_L: f32 = 0.78;

struct Mini {
    sc: f32,
    w: f32,
    h: f32,
}

impl Mini {
    /// The box of an element `size` (reference px) placed like the greeter.
    fn place(&self, cfg: &GreeterConfig, e: Element, size: (f32, f32)) -> (f32, f32, f32, f32) {
        let pl = cfg.placement(e);
        let (w, h) = (size.0 * self.sc * pl.scale, size.1 * self.sc * pl.scale);
        let (fx, fy) = pl.anchor.fraction();
        (pl.x * self.sc + fx * (self.w - w), pl.y * self.sc + fy * (self.h - h), w, h)
    }
}

fn at(x: f32, y: f32, w: f32, h: f32) -> gpui::Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
}

fn bordered(el: gpui::Div) -> gpui::Div {
    el.border_1()
}

/// The look `cfg` (already applied) in a `w × h` miniature.
pub(super) fn thumb(cfg: &GreeterConfig, pal: &MandaraPalette, w: f32, h: f32) -> AnyElement {
    let m = Mini { sc: w / REF_W, w, h };
    let ink = mandara_greeter::view::foreground(cfg);
    let accent = pal.primary.with_lightness(pal.primary.lightness().max(ACCENT_L));
    let bg = match cfg.background.overlay {
        Overlay::Horizon => {
            let [top, bottom] = if cfg.theme.light { DAWN } else { NIGHT };
            oklab_gradient(180.0, Color::oklch(top.0, top.1, top.2), Color::oklch(bottom.0, bottom.1, bottom.2))
        }
        _ => oklab_gradient(SWATCH_DEG, pal.surface_dim, pal.primary.with_lightness(0.32).with_chroma(pal.primary.chroma().min(0.08))),
    };
    let mut root = div().relative().w(px(w)).h(px(h)).overflow_hidden().rounded(px(THUMB_RADIUS)).bg(bg);
    // The overlay.
    match cfg.background.overlay {
        Overlay::Hud => {
            let arm = 18.0 * m.sc * 2.0;
            let i = 28.0 * m.sc * 2.0;
            for (left, top) in [(true, true), (false, true), (true, false), (false, false)] {
                let x = if left { i } else { w - i - arm };
                let y = if top { i } else { h - i - 1.0 };
                root = root.child(at(x, y, arm, 1.0).bg(ink.with_alpha(0.35).hsla()));
                let vx = if left { i } else { w - i - 1.0 };
                let vy = if top { i } else { h - i - arm };
                root = root.child(at(vx, vy, 1.0, arm).bg(ink.with_alpha(0.35).hsla()));
            }
        }
        Overlay::Prism => {
            for (x, y, c) in [(0.2, 0.75, pal.primary), (0.88, 0.15, pal.secondary), (0.74, 0.95, pal.tertiary)] {
                let d = 0.4 * w;
                let glow = Color::oklcha(0.6, 0.13, c.hue(), 0.35).hsla();
                root = root.child(at(x * w - d / 2.0, y * h - d / 2.0, d, d).rounded_full().shadow(vec![gpui::BoxShadow { color: glow, offset: gpui::point(px(0.0), px(0.0)), blur_radius: px(d * 0.8), spread_radius: px(0.0), inset: false }]));
            }
        }
        Overlay::Horizon => root = root.child(at(0.0, (h * GOLDEN).round(), w, 1.0).bg(ink.with_alpha(0.3).hsla())),
        Overlay::None => {}
    }
    // The clock in its face.
    let c = &cfg.clock;
    let bar = |x: f32, y: f32, bw: f32, bh: f32, alpha: f32| at(x, y, bw, bh).rounded(px(bh / 2.0)).bg(ink.with_alpha(alpha).hsla());
    match c.face {
        ClockFace::Digits => {
            let (cols, lines) = if c.stacked { (2.0, 2.0) } else { (4.4, 1.0) };
            let (x, y, cw, ch) = m.place(cfg, Element::Clock, (cols * DIGIT_W * c.size, lines * CLOCK_LINE * c.size));
            for l in 0..lines as usize {
                let lh = ch / lines;
                root = root.child(bar(x, y + l as f32 * lh + lh * 0.2, cw, lh * 0.6, 0.85));
            }
        }
        ClockFace::Ruler => {
            let (x, y, cw, ch) = m.place(cfg, Element::Clock, (78.0 + 2.0 * DIGIT_W * c.size, 2.0 * CLOCK_LINE * c.size));
            root = root.child(at(x, y, 1.0, ch).bg(ink.with_alpha(0.4).hsla())).child(at(x + 1.0, y + ch * 0.55, 4.0, 1.0).bg(pal.tertiary.hsla()));
            let dx = x + cw - 2.0 * DIGIT_W * c.size * m.sc;
            for l in 0..2 {
                root = root.child(bar(dx, y + l as f32 * ch / 2.0 + ch * 0.1, cw - (dx - x), ch * 0.3, 0.8));
            }
        }
        ClockFace::Halo => {
            let d = c.size * 4.0;
            let (x, y, dw, _) = m.place(cfg, Element::Clock, (d, d));
            root = root
                .child(at(x, y, dw, dw).rounded_full().border_1().border_color(accent.with_alpha(0.8).hsla()).shadow(vec![gpui::BoxShadow { color: accent.with_alpha(0.35).hsla(), offset: gpui::point(px(0.0), px(0.0)), blur_radius: px(dw * 0.25), spread_radius: px(0.0), inset: true }]))
                .child(bar(x + dw * 0.22, y + dw * 0.44, dw * 0.56, dw * 0.12, 0.85));
        }
        ClockFace::Vast => {
            let s = c.size * 4.0;
            let (x, y, cw, ch) = m.place(cfg, Element::Clock, (4.4 * DIGIT_W * s, CLOCK_LINE * s));
            root = root.child(at(x, y, cw, ch).rounded(px(ch * 0.2)).bg(ink.with_alpha(0.07).hsla()));
        }
    }
    // The sign-in.
    let card = &cfg.card;
    let field_bar = |x: f32, y: f32, fw: f32| at(x, y, fw, FIELD_H * m.sc).rounded(px(FIELD_H * m.sc / 2.0)).bg(ink.with_alpha(0.22).hsla());
    if card.enabled {
        match card.style {
            CardStyle::Horizon => {
                let y = (h * GOLDEN).round();
                let x = w * (1.0 - GOLDEN);
                root = root.child(bar(x, y - BAR_H * m.sc * 1.2, 150.0 * m.sc * 2.0, BAR_H * m.sc, 0.85));
                for i in 0..5 {
                    root = root.child(at(x + 320.0 * m.sc + i as f32 * 4.0, y - 4.0, 2.5, 2.5).rounded_full().bg(ink.with_alpha(0.8).hsla()));
                }
                root = root.child(at(x + 320.0 * m.sc + 22.0, y - 8.0, 1.0, 7.0).bg(accent.hsla()));
            }
            style => {
                let ch = if style == CardStyle::Plain { PLAIN_CARD_H } else { CARD_H };
                let (x, y, cw, chh) = m.place(cfg, Element::User, (card.width, ch));
                let pad = card.padding * m.sc;
                let mut el = at(x, y, cw, chh);
                el = match style {
                    CardStyle::Hud => bordered(el).border_color(ink.with_alpha(0.25).hsla()).bg(Color::oklcha(0.18, 0.006, 250.0, 0.8).hsla()),
                    CardStyle::Bezel => bordered(el.p(px(2.0))).border_color(ink.with_alpha(0.14).hsla()).rounded(px(card.radius * m.sc)).bg(ink.with_alpha(0.06).hsla()),
                    _ => bordered(el).rounded(px(card.radius * m.sc)).border_color(pal.outline.with_alpha(card.border.min(1.0) * 0.9).hsla()).bg(pal.surface_container.with_alpha(card.opacity).hsla()),
                };
                if style == CardStyle::Bezel {
                    el = el.child(div().size_full().rounded(px((card.radius * m.sc - 2.0).max(0.0))).bg(Color::oklcha(0.165, 0.03, pal.surface_base.hue(), 0.95).hsla()));
                }
                root = root.child(el);
                let inner = cw - 2.0 * pad;
                let a = AVATAR * m.sc;
                root = root.child(at(x + pad + 2.0, y + chh * 0.22, a, a).rounded_full().bg(accent.hsla()));
                root = root.child(field_bar(x + pad + 2.0, y + chh * 0.58, inner - 4.0));
                if cfg.password.submit == mandara_greeter::config::SubmitButton::Pill {
                    root = root.child(at(x + pad + 2.0, y + chh * 0.74, inner - 4.0, FIELD_H * m.sc).rounded_full().bg(accent.hsla()));
                }
                if style == CardStyle::Hud {
                    let arm = 4.0;
                    for (bx, by) in [(x, y), (x + cw - arm, y), (x, y + chh - 1.0), (x + cw - arm, y + chh - 1.0)] {
                        root = root.child(at(bx, by, arm, 1.0).bg(accent.hsla()));
                    }
                }
            }
        }
    } else {
        let (x, y, aw, _) = m.place(cfg, Element::User, (AVATAR * 1.4, AVATAR * 1.4));
        root = root.child(at(x, y, aw, aw).rounded_full().bg(accent.hsla()));
        let (fx, fy, fw, _) = m.place(cfg, Element::Password, (FIELD_W, FIELD_H));
        root = root.child(field_bar(fx, fy, fw));
    }
    root.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The miniature puts each look's sign-in on its side.
    #[test]
    fn miniatures_follow_the_placements() {
        let m = Mini { sc: 0.1, w: 192.0, h: 108.0 };
        for (id, left) in [("ghostline", true), ("chrome-halo", false)] {
            let mut cfg = GreeterConfig::default();
            cfg.apply_look(id);
            let (x, _, w, _) = m.place(&cfg, Element::User, (cfg.card.width, CARD_H));
            assert_eq!(x + w / 2.0 < m.w / 2.0, left, "{id}");
        }
    }
}
