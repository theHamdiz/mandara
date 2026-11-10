//! Chrome Halo: the luminous pieces. Liquid chrome and iridescent light
//! on deep ink, warm: every hue is the theme's own (primary, secondary,
//! tertiary) at fixed OKLCH lightness and chroma, so the light never turns
//! muddy.
//!
//! * [`prism_overlay`] (`background.overlay = "prism"`): wide soft glows
//!   over the backdrop, drifting very slowly.
//! * [`GreeterView::halo_clock`] (`clock.face = "halo"`): light digits and
//!   the date inside a ring of iridescent arcs that turn slowly, step with
//!   each keystroke, brighten while typing and flash coral on a wrong
//!   password; a satellite traces an outer orbit.
//! * [`GreeterView::bezel_card`] (`card.style = "bezel"`): a machined
//!   double-bezel module, an outer tray around an inner core.
//! * The recessed field (`password.style = "recessed"`, drawn by the
//!   shared field with [`field_fill`], [`field_shadows`], [`field_edge`])
//!   and [`GreeterView::pill_submit`] (`password.submit = "pill"`).

use std::sync::OnceLock;

use gpui::{AnyElement, BoxShadow, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Transformation, div, point, prelude::FluentBuilder as _, px, svg};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};

use super::ghostline::hairline;
use super::{CLOCK_IDLE_GROWTH, CLOCK_SUCCESS_GROWTH, Field, Frame, GreeterView, Phase, glow, icon, snap_size, spinner};
use crate::config::{Element, GreeterConfig};

// ── Light ───────────────────────────────────────────────────────────────
/// The arcs' and the glows' OKLCH lightness and chroma.
/// The machined rim's inner light and shade: a hairline lit from above
/// (offset up) and a soft shade under it.
const RIM_LIFT: f32 = -1.0;
const RIM_SHADE_BLUR: f32 = 2.0;
/// The halo core's faint inner rim.
const CORE_RIM_BLUR: f32 = 1.5;
const ARC_LIGHTNESS: f32 = 0.85;
const ARC_CHROMA: f32 = 0.13;
const GLOW_LIGHTNESS: f32 = 0.52;
const GLOW_CHROMA: f32 = 0.15;
/// The deep ink of the core and the field (lightness, chroma; the hue is
/// the theme surface's).
const CORE_INK: (f32, f32) = (0.165, 0.03);
const FIELD_INK: (f32, f32) = (0.11, 0.025);

// ── The halo ────────────────────────────────────────────────────────────
/// The ring's diameter as a multiple of the clock's size (seconds need a
/// wider ring).
pub(super) const HALO_RATIO: f32 = 4.0;
pub(super) const HALO_RATIO_SECONDS: f32 = 5.6;
/// The outer orbit, as a multiple of the ring.
pub(super) const ORBIT_RATIO: f32 = 1.14;
/// How fast each arc turns (degrees per second) and where it starts.
const ARC_SPEEDS: [f32; 3] = [9.0, -6.0, 4.0];
const ARC_PHASES: [f32; 3] = [0.0, 130.0, 245.0];
/// The satellite's turn (radians per second), start and size.
const SATELLITE_TURN: f32 = 0.11;
const SATELLITE_START: f32 = -0.8;
const SATELLITE: f32 = 5.0;
/// Between the digits and the date inside the ring.
pub(super) const HALO_DATE_GAP: f32 = 4.0;

// ── The module ──────────────────────────────────────────────────────────
/// The tray's padding around the core.
pub(super) const TRAY: f32 = 7.0;
pub(super) const EYEBROW_H: f32 = 26.0;
pub(super) const EYEBROW_GAP: f32 = 22.0;
pub(super) const USER_BLOCK_GAP: f32 = 24.0;
/// Between the field and the pill button.
pub(super) const PILL_GAP: f32 = 12.0;
pub(super) const SESSION_GAP: f32 = 14.0;
#[cfg(test)]
/// A compact session pill's height.
pub(super) const SESSION_PILL_H: f32 = 36.0;
const BACK_PX: f32 = 28.0;
/// The pill button: its text and its inner circle's inset.
const PILL_TEXT_PX: f32 = 15.0;
const PILL_INSET: f32 = 6.0;

fn surface_hue(p: &MandaraPalette) -> f32 {
    p.surface_base.hue()
}

/// A light tint of `c` (achromatic tokens borrow the primary's hue).
fn light(c: Color, p: &MandaraPalette, l: f32, chroma: f32) -> Color {
    let hue = if c.chroma() < 0.02 { p.primary.hue() } else { c.hue() };
    Color::oklch(l, chroma, hue)
}

/// The recessed field's deep ink.
pub(super) fn field_fill(p: &MandaraPalette, focused: bool) -> Hsla {
    Color::oklcha(FIELD_INK.0, FIELD_INK.1, surface_hue(p), if focused { 0.98 } else { 0.94 }).hsla()
}

/// Sunk into its panel: shade along the inner top, a highlight along the
/// inner bottom.
pub(super) fn field_shadows(k: f32) -> Vec<BoxShadow> {
    vec![
        BoxShadow { color: gpui::black().opacity(0.55), offset: point(px(0.0), px(2.0 * k)), blur_radius: px(5.0 * k), spread_radius: px(0.0), inset: true },
        BoxShadow { color: gpui::white().opacity(0.07), offset: point(px(0.0), px(RIM_LIFT)), blur_radius: px(1.0), spread_radius: px(0.0), inset: true },
    ]
}

/// The recessed field's rim: faint, the accent while focused, coral while
/// a wrong password flashes (`flash` 1 → 0).
pub(super) fn field_edge(p: &MandaraPalette, focused: bool, flash: f32) -> Hsla {
    if flash > 0.0 {
        p.urgent_coral.with_lightness(p.urgent_coral.lightness().max(0.72)).with_alpha(0.4 + 0.6 * flash).hsla()
    } else if focused {
        light(p.primary, p, 0.82, 0.12).with_alpha(0.55).hsla()
    } else {
        gpui::white().opacity(0.07)
    }
}

/// The ring's diameter at reference scale (awake).
pub(super) fn halo_diameter(cfg: &GreeterConfig) -> f32 {
    cfg.clock.size * if cfg.clock.seconds { HALO_RATIO_SECONDS } else { HALO_RATIO }
}

#[cfg(test)]
/// The module's reference height (scale 1).
pub(super) fn bezel_card_height(cfg: &GreeterConfig, manual: bool) -> f32 {
    let c = &cfg.card;
    let field = cfg.password.height;
    let username = if manual { field + super::ghostline::USERNAME_GAP } else { 0.0 };
    let pill = if cfg.password.submit == crate::config::SubmitButton::Pill { PILL_GAP + field } else { 0.0 };
    let session = if c.session_inside && cfg.placement(Element::Session).visible { SESSION_GAP + SESSION_PILL_H } else { 0.0 };
    let user = cfg.user.avatar_size.max(cfg.user.name_size * 1.3 + 18.0);
    2.0 * (TRAY + c.padding) + EYEBROW_H + EYEBROW_GAP + user + USER_BLOCK_GAP + username + field + pill + super::ghostline::STATUS_TOP + super::ghostline::STATUS_H + session
}

/// The arc drawing, from a file for Studio's preview (whose asset source
/// is the shell's), else from the greeter's assets.
fn arc() -> gpui::Svg {
    static PATH: OnceLock<SharedString> = OnceLock::new();
    let path = PATH.get_or_init(|| crate::assets::extra_icon_file("halo-arc").map(SharedString::from).unwrap_or_default());
    if path.is_empty() { svg().path("icons/halo-arc.svg") } else { svg().external_path(path.clone()) }
}

/// Soft holographic light over the backdrop.
pub(super) fn prism_overlay(f: &Frame, p: &MandaraPalette, shift: (f32, f32)) -> AnyElement {
    let (vw, vh) = (f.vw, f.vh);
    let m = vw.max(vh);
    let t = if f.still { 0.0 } else { f.t };
    let wobble = |period: f32, phase: f32| (t / period * std::f32::consts::TAU + phase).sin();
    let tint = |c: Color, alpha: f32| light(c, p, GLOW_LIGHTNESS, GLOW_CHROMA).with_alpha(alpha).hsla();
    let violet = p.secondary.rotate_hue(40.0);
    // Each glow on its own: overlapping complementary light would grey.
    let glows = [
        (0.16 + 0.03 * wobble(97.0, 0.4), 0.84 + 0.03 * wobble(83.0, 1.2), 0.40, tint(p.primary, 0.34)),
        (0.92 + 0.02 * wobble(113.0, 2.1), 0.10 + 0.03 * wobble(89.0, 0.7), 0.48, tint(p.secondary, 0.62)),
        (0.68 + 0.03 * wobble(131.0, 3.9), 1.0 + 0.02 * wobble(101.0, 2.6), 0.42, tint(p.tertiary, 0.52)),
        (0.46 + 0.03 * wobble(149.0, 5.2), 0.0 + 0.02 * wobble(107.0, 4.4), 0.38, tint(violet, 0.46)),
    ];
    let hue = surface_hue(p);
    let mut root = div().absolute().top_0().left_0().w(px(vw)).h(px(vh)).bg(oklab_gradient(165.0, Color::oklcha(0.22, 0.05, hue, 0.0), Color::oklcha(0.05, 0.02, hue, 0.55)));
    for (x, y, r, color) in glows {
        let d = r * m;
        let depth = 1.2 - r;
        root = root.child(
            div()
                .absolute()
                .left(px(x * vw - d / 2.0 + shift.0 * depth))
                .top(px(y * vh - d / 2.0 + shift.1 * depth))
                .size(px(d))
                .rounded_full()
                .shadow(vec![BoxShadow { color, offset: point(px(0.0), px(0.0)), blur_radius: px(d * 0.9), spread_radius: px(d * 0.12), inset: false }]),
        );
    }
    root.into_any_element()
}

impl GreeterView {
    /// The clock and the date inside the luminous ring.
    pub(super) fn halo_clock(&self, f: &Frame, k: f32) -> AnyElement {
        let p = &self.palette;
        let cfg = &self.config;
        let s = f.scale;
        let hair = hairline(s);
        let idle = 1.0 - self.wake.value().clamp(0.0, 1.0);
        let d = snap_size(halo_diameter(cfg) * k * (1.0 + CLOCK_IDLE_GROWTH * idle) * (1.0 + CLOCK_SUCCESS_GROWTH * f.success));
        let typing = self.glow.value().clamp(0.0, 1.0);
        let flash = self.flash_amount(f);
        let coral = p.urgent_coral.with_lightness(ARC_LIGHTNESS - 0.1).with_chroma(ARC_CHROMA + 0.03);
        let t = if f.still { 0.0 } else { f.t };
        let spin = self.spin.value();
        let core = light(p.primary, p, GLOW_LIGHTNESS, GLOW_CHROMA);
        // A pale champagne inside the ring (a deep tint at low alpha would
        // read brown over the ink).
        let pearl = light(p.primary, p, 0.86, 0.05);
        let centred = |size: f32| div().absolute().left(px((d - size) / 2.0)).top(px((d - size) / 2.0)).size(px(size)).rounded_full();
        let mut root = div().relative().flex_none().size(px(d)).flex().items_center().justify_center();
        // Light gathered behind the digits, stronger while typing.
        let bloom = d * 0.7;
        root = root.child(centred(bloom).shadow(vec![glow(pearl.with_alpha(0.07 + 0.07 * typing).hsla(), d * 0.38, 0.0)]));
        // The outer orbit and its satellite.
        let orbit = d * ORBIT_RATIO;
        let a = SATELLITE_START + t * SATELLITE_TURN + spin * 0.5;
        let sat = SATELLITE * k;
        let (sx, sy) = (d / 2.0 + a.cos() * orbit / 2.0 - sat / 2.0, d / 2.0 + a.sin() * orbit / 2.0 - sat / 2.0);
        let mut ring_edge = div();
        {
            let w: gpui::AbsoluteLength = px(hair).into();
            let widths = &mut ring_edge.style().border_widths;
            (widths.top, widths.right, widths.bottom, widths.left) = (Some(w), Some(w), Some(w), Some(w));
        }
        let mut orbit_edge = div();
        {
            let w: gpui::AbsoluteLength = px(hair).into();
            let widths = &mut orbit_edge.style().border_widths;
            (widths.top, widths.right, widths.bottom, widths.left) = (Some(w), Some(w), Some(w), Some(w));
        }
        root = root
            .child(orbit_edge.absolute().left(px((d - orbit) / 2.0)).top(px((d - orbit) / 2.0)).size(px(orbit)).rounded_full().border_color(gpui::white().opacity(0.07)))
            .child(div().absolute().left(px(sx)).top(px(sy)).size(px(sat)).rounded_full().bg(gpui::white().opacity(0.85)).shadow(vec![glow(core.with_alpha(0.7).hsla(), sat * 3.0, 0.0)]));
        // The ring: a hairline with an inner glow.
        let inner = pearl.with_alpha(0.16 + 0.16 * typing);
        let inner = if flash > 0.0 { inner.mix(coral.with_alpha(0.5), flash) } else { inner };
        root = root.child(
            ring_edge
                .absolute()
                .top_0()
                .left_0()
                .size(px(d))
                .rounded_full()
                .border_color(gpui::white().opacity(0.16 + 0.08 * typing))
                .shadow(vec![
                    BoxShadow { color: inner.hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(d * 0.08), spread_radius: px(0.0), inset: true },
                    BoxShadow { color: gpui::white().opacity(0.07), offset: point(px(0.0), px(0.0)), blur_radius: px(CORE_RIM_BLUR), spread_radius: px(0.0), inset: true },
                ]),
        );
        // Iridescent arcs in the theme's three accents.
        for (i, c) in [p.primary, p.secondary, p.tertiary].into_iter().enumerate() {
            let tint = light(c, p, ARC_LIGHTNESS, ARC_CHROMA);
            let tint = if flash > 0.0 { tint.mix(coral, flash) } else { tint };
            let angle = (ARC_PHASES[i] + ARC_SPEEDS[i] * t).to_radians() + spin * (1.0 - 0.3 * i as f32);
            root = root.child(arc().absolute().top_0().left_0().size(px(d)).text_color(tint.with_alpha(0.9 + 0.1 * typing).hsla()).with_transformation(Transformation::rotate(gpui::radians(angle))));
        }
        // The time, and the date under it.
        let dc = &cfg.date;
        let date = cfg.placement(Element::Date).visible.then(|| {
            let font = if dc.font.trim().is_empty() { f.ui_font.clone() } else { SharedString::from(dc.font.clone()) };
            let text = self.date_text.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
            div().mt(px(HALO_DATE_GAP * k)).font_family(font).text_size(px(dc.size * k)).font_weight(super::weight(dc.weight)).text_color(f.white.opacity(dc.opacity)).child(text)
        });
        root.child(div().relative().flex().flex_col().items_center().child(self.clock_el(f, k)).children(date)).into_any_element()
    }

    /// The machined module: tray, core, and inside it a greeting, the
    /// person, the field, the pill button and the session.
    pub(super) fn bezel_card(&self, f: &Frame, k: f32, caps: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.config.card;
        let p = &self.palette;
        let hair = hairline(f.scale);
        let w = c.width * k;
        let tray = TRAY * k;
        let r = c.radius * k;
        let accent = light(p.primary, p, 0.84, 0.12);
        let tray_fill = gpui::white().opacity(if f.high_contrast { 0.1 } else { 0.045 });
        let core_fill = Color::oklcha(CORE_INK.0, CORE_INK.1, surface_hue(p), if f.high_contrast { 0.99 } else { c.opacity.clamp(0.0, 1.0) });
        let mut outer = div();
        if c.border > 0.0 {
            let bw: gpui::AbsoluteLength = px(hair.max(c.border.min(2.0) / f.scale.max(1.0))).into();
            let widths = &mut outer.style().border_widths;
            (widths.top, widths.right, widths.bottom, widths.left) = (Some(bw), Some(bw), Some(bw), Some(bw));
            outer = outer.border_color(gpui::white().opacity(0.1));
        }
        // Diffused shadows tinted with the accent's own hue.
        let shade = Color::oklcha(0.28, 0.07, accent.hue(), 0.5);
        let outer = outer
            .relative()
            .w(px(w))
            .p(px(tray))
            .rounded(px(r))
            .bg(tray_fill)
            .shadow(vec![
                BoxShadow { color: shade.hsla(), offset: point(px(0.0), px(30.0 * k)), blur_radius: px(80.0 * k), spread_radius: px(0.0), inset: false },
                BoxShadow { color: gpui::black().opacity(0.35), offset: point(px(0.0), px(10.0 * k)), blur_radius: px(30.0 * k), spread_radius: px(0.0), inset: false },
            ]);
        let switcher = self.users.len() > 1 || self.manual.is_some();
        let back = (c.back_button && switcher && !self.is_preview()).then(|| {
            div()
                .id("card-back")
                .flex_none()
                .size(px(BACK_PX * k))
                .rounded_full()
                .bg(gpui::white().opacity(0.06))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|st| st.bg(gpui::white().opacity(0.12)))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.field = Field::User;
                    this.activity();
                    cx.notify();
                }))
                .child(icon(if f.rtl { "arrow-right" } else { "arrow-left" }, BACK_PX * k * 0.45, f.white.opacity(0.9)))
        });
        let tel = self.telemetry.as_ref();
        let greeting = tel.map(|t| if self.manual.is_some() { t.sign_in.clone() } else { t.welcome.clone() }).unwrap_or_default();
        let eyebrow = div()
            .h(px(EYEBROW_H * k))
            .flex()
            .items_center()
            .when(f.rtl, |el| el.flex_row_reverse())
            .child(
                div()
                    .h(px(EYEBROW_H * k))
                    .px(px(11.0 * k))
                    .rounded_full()
                    .bg(gpui::white().opacity(0.06))
                    .flex()
                    .items_center()
                    .gap(px(7.0 * k))
                    .when(f.rtl, |el| el.flex_row_reverse())
                    .child(div().size(px(6.0 * k)).rounded_full().bg(accent.hsla()).shadow(vec![glow(accent.with_alpha(0.6).hsla(), 8.0 * k, 0.0)]))
                    .child(div().text_size(px(11.5 * k)).font_weight(FontWeight::MEDIUM).text_color(f.white.opacity(0.82)).child(greeting)),
            )
            .child(div().flex_1())
            .children(back);
        let mut core = div()
            .w_full()
            .p(px(c.padding * k))
            .rounded(px((r - tray).max(0.0)))
            .bg(core_fill.hsla())
            .shadow(vec![
                BoxShadow { color: gpui::white().opacity(0.11), offset: point(px(0.0), px(1.0)), blur_radius: px(1.0), spread_radius: px(0.0), inset: true },
                BoxShadow { color: gpui::black().opacity(0.3), offset: point(px(0.0), px(RIM_LIFT)), blur_radius: px(RIM_SHADE_BLUR), spread_radius: px(0.0), inset: true },
            ])
            .flex()
            .flex_col()
            .child(eyebrow)
            .child(div().h(px(EYEBROW_GAP * k)))
            .child(self.user_row(f, k, false, cx))
            .child(div().h(px(USER_BLOCK_GAP * k)))
            .child(self.password_el(f, k, caps, cx));
        if c.session_inside && !self.sessions.is_empty() && self.config.placement(Element::Session).visible {
            core = core.child(div().h(px(SESSION_GAP * k))).child(div().flex().when(f.rtl, |el| el.flex_row_reverse()).child(self.session_el(f, k, cx)));
        }
        outer.child(core).into_any_element()
    }

    /// The pill button: "Sign in" in a warm chrome gradient, its arrow in a
    /// circle of its own at the end.
    pub(super) fn pill_submit(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let h = self.config.password.height * k;
        let w = self.field_width(k);
        let busy = self.phase == Phase::Busy;
        let start = light(p.primary, p, 0.88, 0.1);
        let end = start.rotate_hue(-28.0).with_lightness(0.8).with_chroma(0.12);
        let ink = Color::oklch(0.2, 0.03, start.hue());
        let d = h - 2.0 * PILL_INSET * k;
        let label = self.telemetry.as_ref().map(|t| t.sign_in.clone()).unwrap_or_default();
        div()
            .id("submit-pill")
            .flex_none()
            .w(px(w))
            .h(px(h))
            .rounded_full()
            .bg(oklab_gradient(if f.rtl { 260.0 } else { 100.0 }, start, end))
            .shadow(vec![
                glow(start.with_alpha(0.22).hsla(), 26.0 * k, 0.0),
                BoxShadow { color: gpui::white().opacity(0.5), offset: point(px(0.0), px(1.0)), blur_radius: px(1.0), spread_radius: px(0.0), inset: true },
                BoxShadow { color: gpui::black().opacity(0.14), offset: point(px(0.0), px(RIM_LIFT)), blur_radius: px(RIM_SHADE_BLUR), spread_radius: px(0.0), inset: true },
            ])
            .flex()
            .items_center()
            .when(!f.rtl, |el| el.pl(px(24.0 * k)).pr(px(PILL_INSET * k)))
            .when(f.rtl, |el| el.pr(px(24.0 * k)).pl(px(PILL_INSET * k)).flex_row_reverse())
            .cursor_pointer()
            .hover(|st| st.opacity(0.92))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))
            .child(div().flex_1().text_size(px(PILL_TEXT_PX * k)).font_weight(FontWeight::SEMIBOLD).text_color(ink.hsla()).when(f.rtl, |el| el.flex().justify_end()).child(label))
            .child(
                div()
                    .flex_none()
                    .size(px(d))
                    .rounded_full()
                    .bg(ink.with_alpha(0.92).hsla())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if busy { spinner(d * 0.6, k, self.started.elapsed().as_secs_f32()) } else { icon(if f.rtl { "arrow-left" } else { "arrow-right" }, d * 0.42, start.hsla()).into_any_element() }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words on the module and the pill stay readable (WCAG AA).
    #[test]
    fn chrome_halo_text_meets_aa_contrast() {
        let mut cfg = GreeterConfig::default();
        cfg.apply_look("chrome-halo");
        let p = crate::view::palette_for(&cfg);
        let ground = Color::from_hex(&cfg.background.solid).unwrap();
        let core = Color::oklcha(CORE_INK.0, CORE_INK.1, surface_hue(&p), cfg.card.opacity).over(Color::oklcha(1.0, 0.0, 0.0, 0.045).over(ground));
        let white = |a: f32| Color::oklcha(1.0, 0.0, 0.0, a);
        for (what, alpha) in [("login line", 0.56), ("eyebrow", 0.82), ("name", 0.96)] {
            let ratio = white(alpha).over(core).contrast_ratio(core);
            assert!(ratio >= 4.5, "{what}: {ratio:.2}");
        }
        let start = light(p.primary, &p, 0.88, 0.1);
        let end = start.rotate_hue(-28.0).with_lightness(0.8).with_chroma(0.12);
        let ink = Color::oklch(0.2, 0.03, start.hue());
        for stop in [start, end] {
            assert!(ink.contrast_ratio(stop) >= 4.5, "the pill's label on {stop:?}");
        }
        assert!(white(cfg.date.opacity).over(ground).contrast_ratio(ground) >= 4.5, "the date in the ring");
    }

    #[test]
    fn the_ring_holds_the_clock() {
        let mut cfg = GreeterConfig::default();
        cfg.apply_look("chrome-halo");
        // Proportional digits at about 0.62 em and colons at 0.32 em fill
        // at most four fifths of the ring.
        assert!(halo_diameter(&cfg) * 0.8 > cfg.clock.size * (4.0 * 0.62 + 0.32));
        cfg.clock.seconds = true;
        assert!(halo_diameter(&cfg) * 0.8 > cfg.clock.size * (6.0 * 0.62 + 2.0 * 0.32));
    }
}
