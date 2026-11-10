//! Meridian: no card, no boxes. One calm field, one hairline across it at
//! the golden height, and the sign-in set on that line like type.
//!
//! * [`GreeterView::horizon_overlay`] (`background.overlay = "horizon"`):
//!   the field (dawn ivory to pale stone, or deep ink with the light
//!   colours off) and the line, which draws itself in from the centre and
//!   brightens and thickens a little around the caret.
//! * [`GreeterView::vast_clock`] (`clock.face = "vast"`): enormous
//!   hairline numerals at a whisper of contrast, cropped by the edges.
//! * [`GreeterView::horizon_band`] (`card.style = "horizon"`): the name
//!   and the typed characters riding on the line from the golden point,
//!   the status under them, and small words at the line's ends: the
//!   session and the keyboard layout at the start, power and the date at
//!   the end. Right-to-left languages mirror it.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Transformation, canvas, div, prelude::FluentBuilder as _, px, svg};
use mandara_core::password_symbols::{self as symbols, Symbol};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_theme::Color;

use super::ghostline::hairline;
use super::{CARET_HOLD, CLOCK_IDLE_GROWTH, Field, Frame, GreeterView, Phase, Status, element_index, glyph_str, snap_size};
use crate::config::{CardStyle, ClockFace, Element, PasswordMask, UserList};
use crate::power::PowerAction;
use crate::sessions::SessionKind;

/// The line's height on the screen: the golden section from the top.
pub(super) const GOLDEN: f32 = 0.618;
/// The vast numerals are this many times the clock's size, their line
/// this share of it.
pub(super) const VAST_SCALE: f32 = 4.0;
const VAST_LINE: f32 = 0.9;
/// The field: light (dawn ivory to pale stone) and dark (deep ink), as
/// OKLCH lightness, chroma and hue at the top and the bottom.
const DAWN: [(f32, f32, f32); 2] = [(0.975, 0.012, 85.0), (0.905, 0.01, 70.0)];
const NIGHT: [(f32, f32, f32); 2] = [(0.205, 0.022, 262.0), (0.125, 0.018, 258.0)];
/// The line's ink, and around the caret.
const LINE_ALPHA: f32 = 0.24;
const SEGMENT_W: f32 = 280.0;
const SEGMENT_GLOW: f32 = 16.0;
/// The band holding the type: its half-height, the margins at the line's
/// ends, the words' gap from the line.
pub(super) const BAND_HALF: f32 = 90.0;
pub(super) const END_MARGIN: f32 = 64.0;
pub(super) const WORD_GAP: f32 = 14.0;
pub(super) const WORD_PX: f32 = 11.0;
const WORD_ALPHA: f32 = 0.7;
/// The placeholder's and the status's tone.
const SOFT_TONE: f32 = 0.68;
pub(super) const POWER_GAP: f32 = 24.0;
/// A second row of end words sits this many word sizes lower.
const WORD_ROW: f32 = 1.9;

/// Screens narrower than 3:2 (portrait, 4:3, 5:4) have no room for the
/// power words above the line's end.
const NARROW_ASPECT: f32 = 1.5;

pub(super) fn narrow(vw: f32, vh: f32) -> bool {
    vw < vh * NARROW_ASPECT
}
/// The name's weight, its descent below the line (share of its size),
/// and between it and the typed characters.
const NAME_WEIGHT: f32 = 300.0;
const DESCENT: f32 = 0.24;
const RUN_GAP: f32 = 26.0;
/// Typed characters: their slot and size (share of the text size).
pub(super) const RUN_SLOT: f32 = 15.0;
pub(super) const RUN_DOT: f32 = 0.5;
pub(super) const RUN_LIFT: f32 = 4.0;
/// Room kept for the name when fitting the typed characters before the
/// line's end (a longer entry scrolls one slot per character).
const NAME_ROOM: f32 = 300.0;
/// The caret: its width and height (share of the name's size).
const CARET_W: f32 = 1.5;
const CARET_SHARE: f32 = 0.55;
const BLINK_MS: u128 = 530;
pub(super) const STATUS_PX: f32 = 12.5;
const MENU_PX: f32 = 12.5;

fn field_ink(stop: (f32, f32, f32)) -> Color {
    Color::oklch(stop.0, stop.1, stop.2)
}

/// Type `level` of the way from the field to the ink (1 = full ink), as an
/// opaque OKLab tone: translucent ink over a light field blends in linear
/// light and reads far weaker than its alpha suggests.
pub(super) fn tone(cfg: &crate::config::GreeterConfig, level: f32) -> gpui::Hsla {
    let [top, bottom] = if cfg.theme.light { DAWN } else { NIGHT };
    let field = field_ink(top).mix(field_ink(bottom), 0.5);
    field.mix(super::foreground(cfg), level).hsla()
}

/// The words in capitals, made once per language / day / session (see
/// [`super::ghostline::Telemetry`]).
pub(super) struct Words {
    pub power: [SharedString; 3],
    /// The session name, and the session it is for.
    pub session: (usize, SharedString),
    /// The date, and the text it came from.
    pub date: (SharedString, SharedString),
    lang: Option<mandara_i18n::Lang>,
}

impl Default for Words {
    fn default() -> Self {
        Self { power: Default::default(), session: (usize::MAX, SharedString::default()), date: Default::default(), lang: None }
    }
}

impl GreeterView {
    /// Keeps the capitalised words current (cheap when nothing changed).
    pub(super) fn refresh_words(&mut self) {
        if self.config.card.style != CardStyle::Horizon || !self.config.card.enabled {
            self.words = None;
            return;
        }
        let lang = mandara_i18n::language();
        let w = self.words.get_or_insert_with(Words::default);
        if w.lang != Some(lang) {
            w.lang = Some(lang);
            w.power = PowerAction::ALL.map(|a| SharedString::from(a.label().to_uppercase()));
            w.session.0 = usize::MAX;
            w.date.0 = SharedString::default();
        }
        if w.session.0 != self.session_ix {
            let name = self.sessions.get(self.session_ix).map(|s| if s.kind == SessionKind::X11 { format!("{} (X11)", s.name) } else { s.name.clone() }).unwrap_or_else(|| crate::i18n::tl("Login shell"));
            w.session = (self.session_ix, name.to_uppercase().into());
        }
        if let Some((_, text)) = &self.date_text
            && w.date.0 != *text
        {
            w.date = (text.clone(), text.to_uppercase().into());
        }
    }

    /// The calm field and the line at the golden height.
    pub(super) fn horizon_overlay(&self, f: &Frame) -> AnyElement {
        let (vw, vh, s) = (f.vw, f.vh, f.scale);
        let hair = hairline(s);
        let [top, bottom] = if self.config.theme.light { DAWN } else { NIGHT };
        let y = ((vh * GOLDEN) * s).round() / s;
        // Drawn in from the centre with the sign-in's entrance.
        let drawn = self.enter[element_index(Element::User)].value().clamp(0.0, 1.0);
        let w = vw * drawn;
        let mut root = div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(vw))
            .h(px(vh))
            .bg(oklab_gradient(180.0, field_ink(top), field_ink(bottom)))
            .child(div().absolute().left(px((vw - w) / 2.0)).top(px(y)).w(px(w)).h(px(hair)).bg(tone(&self.config, LINE_ALPHA)));
        // Around the caret the line brightens and thickens a little.
        if let Some(x) = self.caret_x.map(|x| x - f32::from(self.origin.get().x)) {
            let focused = matches!(self.field, Field::Password | Field::Username);
            let strength = self.wake.value().clamp(0.0, 1.0) * if focused { 1.0 } else { 0.4 } * (1.0 - f.success);
            if strength > 0.01 {
                let accent = self.palette.primary;
                let half = SEGMENT_W * f.u / 2.0;
                let thick = 2.0 * hair;
                let a = 0.85 * strength;
                root = root
                    .child(div().absolute().left(px(x - half)).top(px(y - hair / 2.0)).w(px(half)).h(px(thick)).bg(oklab_gradient(90.0, accent.with_alpha(0.0), accent.with_alpha(a))))
                    .child(div().absolute().left(px(x)).top(px(y - hair / 2.0)).w(px(half)).h(px(thick)).bg(oklab_gradient(90.0, accent.with_alpha(a), accent.with_alpha(0.0))))
                    .child(div().absolute().left(px(x - half / 3.0)).top(px(y)).w(px(half * 2.0 / 3.0)).h(px(hair)).shadow(vec![super::glow(accent.with_alpha(0.3 * strength).hsla(), SEGMENT_GLOW * f.u, 0.0)]));
            }
        }
        root.into_any_element()
    }

    /// Enormous hairline numerals at a whisper of contrast (no shadow: they
    /// are part of the field).
    pub(super) fn vast_clock(&self, f: &Frame, k: f32) -> AnyElement {
        let c = &self.config.clock;
        let idle = 1.0 - self.wake.value().clamp(0.0, 1.0);
        let size = snap_size(c.size * VAST_SCALE * k * (1.0 + CLOCK_IDLE_GROWTH * 0.25 * idle));
        let mut row = div()
            .flex()
            .font_family(self.clock_font(f))
            .font_features(mandara_ui::fonts::tabular_nums())
            .text_size(px(size))
            .line_height(px(size * VAST_LINE))
            .font_weight(FontWeight(c.weight as f32))
            .text_color(tone(&self.config, c.opacity));
        for ch in self.roll.text.chars() {
            row = row.child(glyph_str(ch));
        }
        row.into_any_element()
    }

    /// The sign-in on the line.
    pub(super) fn horizon_band(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let cfg = &self.config;
        let p = &self.palette;
        let vw = f.vw;
        let half = BAND_HALF * k;
        let ink = f.white;
        let accent = p.primary.hsla();
        let margin = END_MARGIN * k;
        let words = self.words.as_ref();
        let word = |text: SharedString| div().text_size(px(WORD_PX * k)).font_weight(FontWeight::MEDIUM).text_color(tone(cfg, WORD_ALPHA)).child(text);
        // Positions from the line's start (mirrored right to left).
        let start = |el: gpui::Div, x: f32| if f.rtl { el.right(px(x)) } else { el.left(px(x)) };
        let end = |el: gpui::Div, x: f32| if f.rtl { el.left(px(x)) } else { el.right(px(x)) };
        let above = |el: gpui::Div| el.absolute().bottom(px(half + WORD_GAP * k));
        let below = |el: gpui::Div| el.absolute().top(px(half + WORD_GAP * k));
        // On a narrow screen the typed line reaches the end words: power
        // and the date step under the line, one row each.
        let narrow = narrow(vw, f.vh);
        let second_row = |el: gpui::Div| el.absolute().top(px(half + (WORD_GAP + WORD_PX * WORD_ROW) * k));
        let mut band = div().relative().w(px(vw)).h(px(2.0 * half));

        // The session, at the start, above the line; its list opens upward.
        let session_on = cfg.card.session_inside && cfg.placement(Element::Session).visible && !self.sessions.is_empty();
        if session_on {
            let focused = self.field == Field::Session;
            let name = words.map(|w| w.session.1.clone()).unwrap_or_default();
            let mut sess = div()
                .id("session")
                .flex()
                .items_center()
                .gap(px(6.0 * k))
                .when(f.rtl, |el| el.flex_row_reverse())
                .cursor_pointer()
                .child(word(name).when(focused, |el| el.text_color(ink)))
                .when(self.sessions.len() > 1, |el| el.child(super::icon(if self.session_menu { "chevron-up" } else { "chevron-down" }, 9.0 * k, tone(cfg, WORD_ALPHA))))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if this.is_preview() {
                        return;
                    }
                    this.session_menu = !this.session_menu;
                    this.field = Field::Session;
                    this.activity();
                    cx.notify();
                }));
            if focused {
                sess = sess.child(div().w(px(4.0 * k)).h(px(1.0)).bg(accent));
            }
            band = band.child(start(above(div()), margin).child(sess));
            if self.session_menu {
                let mut list = div().flex().flex_col().gap(px(8.0 * k)).when(f.rtl, |el| el.items_end());
                for (i, s) in self.sessions.iter().enumerate() {
                    let selected = i == self.session_ix;
                    let label = if s.kind == SessionKind::X11 { format!("{} (X11)", s.name) } else { s.name.clone() };
                    list = list.child(
                        div()
                            .id(("session-item", i))
                            .cursor_pointer()
                            .text_size(px(MENU_PX * k))
                            .text_color(if selected { accent } else { tone(cfg, 0.8) })
                            .hover(|st| st.text_color(ink))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.session_ix = i;
                                this.session_menu = false;
                                this.field = Field::Password;
                                cx.notify();
                            }))
                            .child(label),
                    );
                }
                band = band.child(start(div().absolute().bottom(px(half + (WORD_GAP + WORD_PX * 2.4) * k)), margin).child(list));
            }
        }
        // The keyboard layout under it (when the separate pill is hidden).
        if !cfg.placement(Element::Keyboard).visible {
            band = band.child(start(below(div()), margin).child(word(f.keyboard.clone())));
        }
        // Power at the end, above the line (when the buttons are hidden).
        if !cfg.placement(Element::Power).visible {
            let pc = &cfg.power;
            let mut row = div().flex().gap(px(POWER_GAP * k)).when(f.rtl, |el| el.flex_row_reverse()).when(self.demo(), |el| el.opacity(0.7));
            for (i, action) in PowerAction::ALL.into_iter().enumerate() {
                let on = match action {
                    PowerAction::Suspend => pc.suspend,
                    PowerAction::Reboot => pc.reboot,
                    PowerAction::PowerOff => pc.poweroff,
                };
                if !on {
                    continue;
                }
                let label = words.map(|w| w.power[i].clone()).unwrap_or_default();
                row = row.child(div().id(action.method()).cursor_pointer().hover(|st| st.opacity(1.0)).child(word(label).hover(|st| st.text_color(ink))).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.power(action, cx))));
            }
            band = band.child(end(if narrow { below(div()) } else { above(div()) }, margin).child(row));
        }
        // The date at the end, under the line (the vast face leaves it here).
        if cfg.clock.face != ClockFace::Digits && cfg.placement(Element::Date).visible {
            let d = &cfg.date;
            let text = words.map(|w| w.date.1.clone()).unwrap_or_default();
            let place = if narrow && !cfg.placement(Element::Power).visible { second_row(div()) } else { below(div()) };
            band = band.child(end(place, margin).child(word(text).text_size(px(d.size * k)).text_color(tone(cfg, d.opacity))));
        }

        // The name and the typed characters, from the golden point.
        let gx = vw * (1.0 - GOLDEN);
        let name_px = cfg.user.name_size * k;
        let descent = DESCENT * name_px;
        let shake = self.shake.value() * f.u;
        band = band.child(start(div().absolute().bottom(px(half - descent)), gx + if f.rtl { -shake } else { shake }).child(self.typeset(f, k, name_px, descent, cx)));
        // The status under them.
        let status: Option<(String, gpui::Hsla)> = match &self.status {
            Status::Error(e) => Some((e.clone(), p.urgent_coral.hsla())),
            Status::Info(i) => Some((i.clone(), tone(cfg, SOFT_TONE))),
            Status::None if !self.is_preview() && f.caps && cfg.password.caps_hint => Some((super::tl("Caps Lock is on"), tone(cfg, SOFT_TONE))),
            Status::None if self.demo() => Some((super::tl("Demo: any password works — try “wrong” to see a failure"), tone(cfg, SOFT_TONE))),
            Status::None => None,
        };
        if let Some((text, color)) = status {
            band = band.child(start(below(div()), gx).child(div().text_size(px(STATUS_PX * k)).text_color(color).child(text)));
        }
        band.into_any_element()
    }

    /// The name (or the typed username) and the characters typed so far,
    /// set on one baseline, with the caret.
    fn typeset(&self, f: &Frame, k: f32, name_px: f32, descent: f32, cx: &mut Context<Self>) -> AnyElement {
        let cfg = &self.config;
        let p = &self.palette;
        let ink = f.white;
        let accent = p.primary.hsla();
        let busy = self.phase == Phase::Busy;
        let blink = f.reduced || (self.started.elapsed().as_millis() / BLINK_MS).is_multiple_of(2) || self.last_key.elapsed() < CARET_HOLD;
        let caret_on = !busy && !self.is_preview() && blink;
        let caret_h = name_px * CARET_SHARE;
        let measured = self.horizon_caret.clone();
        let caret = |shown: bool| {
            div()
                .flex_none()
                .w(px(CARET_W))
                .h(px(caret_h))
                .mb(px(descent))
                .when(shown, |el| el.bg(accent))
                .child({
                    let measured = measured.clone();
                    canvas(move |b, _, _| measured.set(Some(f32::from(b.origin.x))), |_, _, _, _| {}).size_full()
                })
        };
        let text_px = cfg.password.text_size * k;
        let name_el = |text: SharedString, dim: bool| {
            div().font_family(f.display_font.clone()).text_size(px(name_px)).line_height(px(name_px * 1.15)).font_weight(FontWeight(NAME_WEIGHT)).text_color(tone(cfg, if dim { SOFT_TONE } else { 0.96 })).child(text)
        };
        let mut row = div().flex().items_end().when(f.rtl, |el| el.flex_row_reverse());
        // Who: the picked person (click for the next one), or a typed name.
        let typing_name = self.manual.is_some();
        if typing_name {
            let name = self.manual.clone().unwrap_or_default();
            let empty = name.is_empty();
            let focused = self.field == Field::Username;
            row = row.child(
                div()
                    .id("username")
                    .flex()
                    .items_end()
                    .when(f.rtl, |el| el.flex_row_reverse())
                    .cursor_text()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.field = Field::Username;
                        this.activity();
                        cx.notify();
                    }))
                    .child(name_el(if empty { super::tl("Username").into() } else { name.into() }, empty))
                    .when(focused, |el| el.child(caret(caret_on))),
            );
        } else {
            let entry = self.users.get(self.user_ix);
            let name: SharedString = match entry {
                Some(e) => e.user.display.clone().into(),
                None => super::tl("Your Name").into(),
            };
            let others = self.users.len() > 1 || (!self.demo() && cfg.user.list != UserList::Manual);
            let next = (self.user_ix + 1) % (self.users.len() + usize::from(!self.demo())).max(1);
            row = row.child(
                div()
                    .id("avatar")
                    .when(others && !self.is_preview(), |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.field = Field::User;
                            this.activity();
                            this.select_user(next, cx);
                        }))
                    })
                    .child(name_el(name, false).when(self.field == Field::User, |el| el.text_color(ink))),
            );
        }
        row = row.child(div().flex_none().w(px(RUN_GAP * k)));
        // What has been typed: one small shape per character on the line.
        let pw_focused = self.field == Field::Password;
        let n = self.password.chars().count();
        let mut run = div()
            .id("password")
            .flex()
            .items_end()
            .mb(px(descent + RUN_LIFT * k))
            .when(f.rtl, |el| el.flex_row_reverse())
            .cursor_text()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.field = Field::Password;
                this.session_menu = false;
                this.activity();
                cx.notify();
            }));
        if n == 0 && !busy {
            let placeholder = match &self.prompt {
                Some((_, msg)) if !msg.is_empty() => msg.trim_end_matches(':').to_string(),
                _ if !cfg.password.placeholder.trim().is_empty() => cfg.password.placeholder.clone(),
                _ => super::tl("Enter password"),
            };
            run = run.when(pw_focused, |el| el.child(div().mb(px(-RUN_LIFT * k)).child(caret(caret_on)))).child(div().ml(px(8.0 * k)).text_size(px(text_px)).line_height(px(text_px)).text_color(tone(cfg, SOFT_TONE)).child(placeholder));
        } else {
            let dot = (text_px * RUN_DOT).round();
            let slot = RUN_SLOT * k;
            let born = self.dots.len().saturating_sub(n);
            let animate = cfg.password.keystroke_ripple && !f.reduced;
            let revealed = self.reveal || self.prompt.as_ref().is_some_and(|(secret, _)| !*secret);
            if revealed {
                run = run.child(div().text_size(px(text_px)).line_height(px(text_px)).text_color(ink.opacity(0.94)).child(self.password.clone()));
            } else {
                let fits = (((f.vw * GOLDEN - (END_MARGIN + NAME_ROOM) * k) / slot).floor() as usize).max(8);
                for ix in n.saturating_sub(fits)..n {
                    let shape = if cfg.password.mask == PasswordMask::Dots { Symbol::Circle } else { symbols::symbol_at(self.mask_seed, ix) };
                    let (scale, opacity) = self.dots.get(born + ix).map_or((1.0, 1.0), |at| symbols::appear(f.now.saturating_duration_since(*at), animate));
                    run = run.child(div().w(px(slot)).h(px(dot)).flex().justify_center().child(svg().path(shape.asset_path()).size(px(dot)).flex_none().text_color(ink.opacity(0.9 * opacity)).with_transformation(Transformation::scale(gpui::size(scale, scale)))));
                }
            }
            if busy {
                run = run.child(pulse(k, self.started.elapsed().as_secs_f32(), ink, f.still));
            } else if pw_focused {
                run = run.child(div().ml(px(4.0 * k)).mb(px(-RUN_LIFT * k)).child(caret(caret_on)));
            }
        }
        row.child(run).into_any_element()
    }
}

/// Three dots lighting in turn while checking (lit and still with reduced
/// motion), in the text's ink.
fn pulse(k: f32, t: f32, ink: gpui::Hsla, still: bool) -> AnyElement {
    let mut row = div().ml(px(8.0 * k)).flex().gap(px(5.0 * k));
    for i in 0..3 {
        let phase = if still { 1.0 } else { (t * 5.0 - i as f32 * 0.7).sin() * 0.5 + 0.5 };
        row = row.child(div().size(px(5.0 * k)).rounded_full().bg(ink.opacity(0.25 + 0.65 * phase)));
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GreeterConfig;

    /// Type on the field stays readable in both schemes (WCAG AA), the
    /// vast numerals stay a whisper, and the accent reads on the light
    /// field.
    #[test]
    fn meridian_text_meets_aa_contrast_light_and_dark() {
        let mut cfg = GreeterConfig::default();
        cfg.apply_look("meridian");
        let opaque = |c: gpui::Hsla| {
            let rgba = gpui::Rgba::from(c);
            Color::from_rgba8((rgba.r * 255.0).round() as u8, (rgba.g * 255.0).round() as u8, (rgba.b * 255.0).round() as u8, 255)
        };
        for light in [true, false] {
            cfg.theme.light = light;
            let [top, bottom] = if light { DAWN } else { NIGHT };
            let scheme = if light { "light" } else { "dark" };
            for ground in [field_ink(top), field_ink(bottom)] {
                for (what, level) in [("the words", WORD_ALPHA), ("the date", cfg.date.opacity), ("the placeholder and status", SOFT_TONE), ("the name", 0.96)] {
                    let ratio = opaque(tone(&cfg, level)).contrast_ratio(ground);
                    assert!(ratio >= 4.5, "{scheme}: {what}: {ratio:.2}");
                }
                let clock = opaque(tone(&cfg, cfg.clock.opacity)).contrast_ratio(ground);
                assert!(clock < 1.35, "{scheme}: the numerals stay in the background: {clock:.2}");
                let line = opaque(tone(&cfg, LINE_ALPHA)).contrast_ratio(ground);
                assert!(line > 1.3, "{scheme}: the line shows: {line:.2}");
                let p = super::super::palette_for(&cfg);
                assert!(p.urgent_coral.contrast_ratio(ground) >= 3.0, "{scheme}: errors read: {:.2}", p.urgent_coral.contrast_ratio(ground));
                assert!(p.primary.contrast_ratio(ground) >= 3.0, "{scheme}: the caret's accent shows: {:.2}", p.primary.contrast_ratio(ground));
            }
        }
    }
}
