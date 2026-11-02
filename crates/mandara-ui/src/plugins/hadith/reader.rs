//! The reading card, in Jannah's design family: an OKLCH two-tone ground
//! (Jannah's hero tones), its faint eight-pointed-star lattice fading
//! downward and the soft dawn glow, under a gilded manuscript headpiece.
//! The words of the hadith come first in Amiri; the chain of narrators,
//! when shown, sits above them smaller and quieter. Then the narrator, the
//! meaning (with its translator, or "English" when English stands in),
//! grade badges in their colours (sahih emerald, hasan teal, da'if amber,
//! the grader named as given), the reference chip and the actions.
//!
//! One card serves the Home hero (today's hadith, its meaning clamped,
//! "Read" opening the reader) and the reader (everything, the chain
//! toggle, every grade, previous / next sliding in).

use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, svg};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf};
use mandara_services::hadith::{Collection, Grade, GradeClass};
use mandara_theme::{Color, MandaraPalette};

use super::super::extras::caption;
use super::super::jannah::art::LATTICE_PATH;
use super::super::jannah::gold;
use super::super::jannah::panel::{CHIP_FILL, DAWN_GLOW, HERO_ANGLE_DEG, HERO_DARK, HERO_LIGHT, LATTICE_ALPHA, MEANING_LINE_EM, tone};
use super::super::panel_fx::{alpha, glow, rim};
use super::art::{HEADPIECE_PATH, HEADPIECE_VIEW};
use super::panel::Ctx;
use super::{ARABIC_LINE_EM, citation, grade_color, grade_label, short_ref};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, text, tokens};

// ── Layout ───────────────────────────────────────────────────────────────

/// Padding inside the card, and the gap between its blocks.
const PAD_X: f32 = 20.0;
const PAD_TOP: f32 = 16.0;
const PAD_BOTTOM: f32 = 16.0;
const BLOCK_GAP: f32 = 12.0;
/// The headpiece spans this share of the card's width.
const HEADPIECE_SHARE: f32 = 0.86;
/// How much of the primary tints the ground (Jannah mixes emerald in; the
/// library leans to the tertiary, a scholar's ink).
const GROUND_PRIMARY_SHARE: f32 = 0.3;
const GROUND_EMERALD_SHARE: f32 = 0.45;
/// Dawn light: a fill-less source whose blurred shadow is the glow.
const DAWN_W: f32 = 96.0;
const DAWN_H: f32 = 16.0;
const DAWN_TOP: f32 = 22.0;
/// The chain and what follows the words: size and alpha.
const CHAIN_PX: f32 = 14.5;
const CHAIN_ALPHA: f32 = 0.62;
/// A short gold rule between the Arabic and the meaning.
const RULE_W: f32 = 36.0;
const RULE_H: f32 = 1.5;
const RULE_ALPHA: f32 = 0.6;
/// Meanings longer than this read from the start edge, not centred.
const CENTRED_MEANING_MAX: usize = 220;
/// Lines of the meaning on the Home hero.
const HERO_MEANING_LINES: usize = 5;
/// Small caps label tracking-free size, and the grade badges.
const LABEL_PX: f32 = 10.5;
const BADGE_H: f32 = 22.0;
const BADGE_PAD_X: f32 = 9.0;
const BADGE_DOT: f32 = 6.0;
const BADGE_FILL: (f32, f32) = (0.16, 0.14);
/// Reference chip and the action buttons.
const CHIP_H: f32 = 26.0;
const CHIP_PAD_X: f32 = 10.0;
const ACTION_BTN: f32 = 30.0;
/// Slide on previous / next: distance, how long it may run, fade, rest.
const SLIDE_PX: f32 = 36.0;
const SLIDE_FOR: Duration = Duration::from_millis(700);
const SLIDE_FADE: f32 = 0.85;
const SLIDE_REST_PX: f32 = 0.3;

/// What a card shows.
pub(super) struct Shown<'a> {
    pub collection: Collection,
    pub reference: &'a str,
    pub key: &'a str,
    pub url: &'a str,
    pub words: &'a str,
    /// The chain before the words and what follows them (`None`: folded).
    pub chain: Option<(&'a str, &'a str)>,
    /// The meaning without "Narrated X:", and verbatim (for copying).
    pub meaning: Option<&'a str>,
    pub meaning_full: Option<&'a str>,
    pub narrator: Option<&'a str>,
    /// English stands in for the interface language.
    pub english: bool,
    pub translator: Option<&'a str>,
    /// The lead family (always sahih for Bukhari and Muslim) and every
    /// grader's grade.
    pub class: Option<GradeClass>,
    pub grades: &'a [Grade],
    /// The small caps label at the top.
    pub label: String,
}

/// How the card behaves.
pub(super) enum Mode {
    /// The Home hero: the meaning clamped; "Read" opens the reader.
    Hero,
    /// The reader: everything; the chain toggle.
    Reader { chain_shown: bool, toggle: std::rc::Rc<dyn Fn(&mut App)>, has_chain: bool },
}

/// The ground's two tones.
fn ground(palette: &MandaraPalette) -> (Color, Color) {
    let tones = if palette.is_dark { HERO_DARK } else { HERO_LIGHT };
    let ink = palette.tertiary.mix(palette.primary, GROUND_PRIMARY_SHARE);
    (tone(ink, tones[0]), tone(palette.success_emerald.mix(ink, GROUND_EMERALD_SHARE), tones[1]))
}

/// A grade badge: a dot and the family's name, or the grade's own words
/// with its grader.
pub(super) fn badge(class: GradeClass, words: Option<&str>, grader: Option<&str>, palette: &MandaraPalette) -> AnyElement {
    let c = grade_color(class, palette);
    let label = match (words.filter(|w| !w.is_empty()), grader.filter(|g| !g.is_empty())) {
        (Some(w), Some(g)) => format!("{w} · {g}"),
        (Some(w), None) => w.to_string(),
        _ => t(grade_label(class)).to_string(),
    };
    div()
        .h(px(BADGE_H))
        .px(px(BADGE_PAD_X))
        .flex_none()
        .rounded_full()
        .bg(c.with_alpha(alpha(BADGE_FILL, palette)).hsla())
        .row()
        .items_center()
        .gap(px(tokens::XS + 2.0))
        .child(div().size(px(BADGE_DOT)).rounded_full().bg(c.hsla()))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(c.hsla()).whitespace_nowrap().child(label))
        .into_any_element()
}

/// The slide offset and fade of a card after previous / next.
fn slide(c: &Ctx, window: &mut Window) -> (f32, f32) {
    let Some((at, dir)) = c.ui.borrow().slide else { return (0.0, 1.0) };
    let (x, _) = mandara_core::SpringConfig::GENTLE.solve(SLIDE_PX * dir, 0.0, at.elapsed().as_secs_f32());
    if at.elapsed() < SLIDE_FOR && x.abs() > SLIDE_REST_PX && !mandara_core::spring::motion_profile().reduce {
        window.request_animation_frame();
        (x, 1.0 - (x.abs() / SLIDE_PX).min(1.0) * SLIDE_FADE)
    } else {
        c.ui.borrow_mut().slide = None;
        (0.0, 1.0)
    }
}

/// The reading card.
pub(super) fn card(c: &Ctx, s: &Shown, mode: Mode, window: &mut Window) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let (a, b) = ground(palette);
    let inner_w = c.width;
    let hero = matches!(mode, Mode::Hero);
    let (offset, fade) = if hero { (0.0, 1.0) } else { slide(c, window) };
    let quiet = palette.on_surface_variant.with_alpha(CHAIN_ALPHA);
    let arabic = |t: &str, size: f32, color: Color, centred: bool| {
        div()
            .w_full()
            .font_family(c.hadith_font.clone())
            .text_size(px(size))
            .line_height(gpui::relative(ARABIC_LINE_EM))
            .when(centred, |el| el.text_center())
            .when(!centred, |el| el.text_right())
            .text_color(color.hsla())
            .child(crate::bidi::paragraph(t.to_string()))
    };
    // Top: the label and the lead grade.
    let top = div()
        .w_full()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(LABEL_PX)).font_weight(FontWeight::BOLD).text_color(g.hsla()).child(s.label.clone()))
        .when_some(s.class, |el, class| {
            let lead = s.grades.iter().find(|x| x.class == class && x.grader.eq_ignore_ascii_case("Al-Albani")).or_else(|| s.grades.first());
            el.child(badge(class, lead.map(|l| l.grade.as_str()), lead.map(|l| l.grader.as_str()), palette))
        });
    let headpiece_w = inner_w * HEADPIECE_SHARE;
    let headpiece = div().w_full().flex().justify_center().child(svg().path(HEADPIECE_PATH).w(px(headpiece_w)).h(px(headpiece_w * HEADPIECE_VIEW.1 / HEADPIECE_VIEW.0)).text_color(g.hsla()));
    let mut body = div().w_full().flex().flex_col().items_center().gap(px(BLOCK_GAP));
    if let Some((before, _)) = s.chain.filter(|(b, _)| !b.is_empty()) {
        body = body.child(arabic(before, CHAIN_PX, quiet, false));
    }
    body = body.child(arabic(s.words, c.arabic_px, palette.on_surface, true));
    if let Some((_, after)) = s.chain.filter(|(_, a)| !a.is_empty()) {
        body = body.child(arabic(after, CHAIN_PX, quiet, false));
    }
    if let Mode::Reader { chain_shown, toggle, has_chain: true } = &mode {
        let toggle = toggle.clone();
        body = body.child(
            div()
                .id("hadith-chain-toggle")
                .flex()
                .items_center()
                .gap(px(tokens::XS))
                .cursor_pointer()
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(g.hsla())
                .child(icon(if *chain_shown { "eye-off" } else { "eye" }, text::LABEL + 1.0, g))
                .child(if *chain_shown { t("Hide the chain of narrators") } else { t("Show the chain of narrators") })
                .on_click(move |_, _, cx| toggle(cx)),
        );
    }
    let meaning = s.meaning.filter(|m| c.show_meaning && !m.trim().is_empty());
    if meaning.is_some() || (c.show_meaning && s.narrator.is_some()) {
        body = body.child(div().w(px(RULE_W)).h(px(RULE_H)).rounded_full().bg(g.with_alpha(RULE_ALPHA).hsla()));
    }
    if let Some(n) = s.narrator.filter(|_| c.show_meaning) {
        body = body.child(caption(tf("Narrated by {narrator}", &[("narrator", &n)]), palette).font_weight(FontWeight::SEMIBOLD));
    }
    if let Some(m) = meaning {
        let centred = m.chars().count() <= CENTRED_MEANING_MAX;
        let para = div()
            .w_full()
            .text_size(px(text::BODY))
            .line_height(gpui::relative(MEANING_LINE_EM))
            .text_color(palette.on_surface.hsla())
            .when(centred, |el| el.text_center())
            .when(!centred, |el| el.text_start())
            .when(hero, |el| el.line_clamp(HERO_MEANING_LINES))
            .child(crate::bidi::paragraph(m.to_string()));
        body = body.child(para);
        let credit: Option<String> = if s.english { Some(t("Meaning in English").into()) } else { s.translator.map(|w| format!("— {w}")) };
        if let Some(credit) = credit.filter(|_| !hero) {
            body = body.child(caption(credit, palette));
        }
    }
    // Every grader's grade, in the reader.
    if !hero && s.grades.len() > 1 {
        body = body.child(
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(px(tokens::XS + 2.0))
                .children(s.grades.iter().map(|gr| badge(gr.class, Some(&gr.grade), Some(&gr.grader), palette))),
        );
    }
    body = body.child(actions(c, s, &mode));
    let content = div()
        .relative()
        .left(px(offset))
        .opacity(fade)
        .px(px(PAD_X))
        .pt(px(PAD_TOP))
        .pb(px(PAD_BOTTOM))
        .flex()
        .flex_col()
        .gap(px(BLOCK_GAP))
        .child(top)
        .child(headpiece)
        .child(body);
    div()
        .relative()
        .w_full()
        .rounded(px(c.radius))
        .overflow_hidden()
        .bg(oklab_gradient(HERO_ANGLE_DEG, a, b))
        .shadow(rim(palette))
        .child(svg().path(LATTICE_PATH).absolute().top_0().left_0().size(px(inner_w)).text_color(g.with_alpha(alpha(LATTICE_ALPHA, palette)).hsla()))
        .child(div().absolute().top(px(DAWN_TOP)).left(px((inner_w - DAWN_W) / 2.0)).w(px(DAWN_W)).h(px(DAWN_H)).rounded_full().shadow(vec![glow(g.with_alpha(DAWN_GLOW.0), DAWN_GLOW.1, DAWN_GLOW.2, 0.0)]))
        .child(content)
        .into_any_element()
}

/// The reference chip (copies the citation), sunnah.com, then bookmark,
/// copy and copy with the meaning; on the hero, "Read" too.
fn actions(c: &Ctx, s: &Shown, mode: &Mode) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let ink = palette.on_surface_variant;
    let cite = citation(s.collection, s.reference);
    let chip_text: SharedString = if super::arabic_ui() { cite.clone().into() } else { format!("{} · {}", short_ref(s.collection, s.reference), mandara_services::hadith::collections::info(s.collection).arabic).into() };
    let plain = super::copy_text(s.collection, s.reference, s.url, s.words, None);
    let full = super::copy_text(s.collection, s.reference, s.url, s.words, s.meaning_full);
    let has_meaning = s.meaning_full.is_some_and(|m| !m.trim().is_empty());
    let bookmarked = c.shared.borrow().state.is_bookmarked(s.key);
    let (url, key) = (s.url.to_string(), s.key.to_string());
    let (f1, f2, f3) = (c.ui.borrow().flash.clone(), c.ui.borrow().flash.clone(), c.ui.borrow().flash.clone());
    // Translated now, in the plugin's language (the clicks come later).
    let (cited, copied, copied_meaning) = (t("Citation copied"), t("Copied with its reference"), t("Copied with its meaning"));
    let chip = div()
        .id("hadith-ref")
        .h(px(CHIP_H))
        .px(px(CHIP_PAD_X))
        .min_w(px(0.0))
        .rounded_full()
        .cursor_pointer()
        .bg(g.with_alpha(alpha(CHIP_FILL, palette)).hsla())
        .row()
        .items_center()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular_nums())
        .text_color(g.hsla())
        .child(div().truncate().child(chip_text))
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(cite.clone()));
            f1.show(true, cited);
        });
    let shared = c.shared.clone();
    let mut row = div()
        .w_full()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .child(chip)
        .child(icon_button("hadith-link", "external", ACTION_BTN, ink, None, move |_, _| super::super::jannah::open_link(url.clone())))
        .child(div().flex_1())
        .child(icon_button("hadith-bookmark", "bookmark", ACTION_BTN, if bookmarked { g } else { ink }, bookmarked.then(|| g.with_alpha(alpha(CHIP_FILL, palette))), move |_, cx| {
            super::toggle_bookmark(&shared, &key, cx);
        }))
        .child(icon_button("hadith-copy", "copy", ACTION_BTN, ink, None, move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(plain.clone()));
            f2.show(true, copied);
        }))
        .when(has_meaning, |el| {
            el.child(icon_button("hadith-copy-meaning", "clipboard", ACTION_BTN, ink, None, move |_, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
                f3.show(true, copied_meaning);
            }))
        });
    if matches!(mode, Mode::Hero) {
        let (shared, key) = (c.shared.clone(), s.key.to_string());
        row = row.child(super::super::panel_fx::pill_button("hadith-read", Some(crate::bidi::dir_icon("chevron-right")), t("Read"), g, palette, move |_, cx| super::open_key(&shared, &key, cx)));
    }
    row.into_any_element()
}
