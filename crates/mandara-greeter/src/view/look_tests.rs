//! The side looks (Ghostline on the left, Chrome Halo on the right) fit
//! every screen from 1366 × 768 to 4K, ultrawide and portrait: nothing
//! leaves the screen, nothing overlaps, and the sign-in stays on its side.

use super::*;
use crate::config::{LOOKS, Side};

#[derive(Copy, Clone, Debug)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn overlaps(self, o: Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    fn inside(self, w: f32, h: f32, margin: f32) -> bool {
        self.x0 >= margin && self.y0 >= margin && self.x1 <= w - margin && self.y1 <= h - margin
    }

    /// Grown by `by` on every side (absolute decorations around an element).
    fn grown(self, by: f32) -> Rect {
        Rect { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }
}

/// Where [`GreeterView::place`] puts an element of reference size `size`
/// on a real `vw × vh` login screen.
fn rect(cfg: &GreeterConfig, e: Element, size: (f32, f32), vw: f32, vh: f32) -> Rect {
    let u = mandara_config::login_layout::screen_scale(vw, vh);
    let pl = cfg.placement(e);
    let k = u * pl.scale;
    let (w, h) = (size.0 * k, size.1 * k);
    let (fx, fy) = pl.anchor.fraction();
    let x0 = pl.x * u + fx * (vw - w);
    let y0 = pl.y * u + fy * (vh - h);
    Rect { x0, y0, x1: x0 + w, y1: y0 + h }
}

const SCREENS: [(f32, f32); 11] = [
    (1366.0, 768.0),
    (1600.0, 900.0),
    (1920.0, 1080.0),
    (2560.0, 1440.0),
    (3440.0, 1440.0),
    (3840.0, 2160.0),
    (1280.0, 1024.0),
    (768.0, 1366.0),
    (1080.0, 1920.0),
    (1440.0, 2560.0),
    (2160.0, 3840.0),
];

/// The power row: three buttons and their gaps.
fn power_size(cfg: &GreeterConfig) -> (f32, f32) {
    (3.0 * cfg.power.size + 2.0 * 18.0, cfg.power.size)
}

fn check(id: &str, card: (f32, f32), face: (f32, f32), face_reach: f32, side: Side) {
    let mut cfg = GreeterConfig::default();
    assert!(cfg.apply_look(id));
    assert_eq!(cfg.credentials_side(), side, "{id}");
    for (vw, vh) in SCREENS {
        let u = mandara_config::login_layout::screen_scale(vw, vh);
        let margin = 8.0 * u;
        for manual in [false, true] {
            let card_h = if manual { card.1 + cfg.password.height + ghostline::USERNAME_GAP } else { card.1 };
            let card = rect(&cfg, Element::User, (card.0, card_h), vw, vh);
            let face = rect(&cfg, Element::Clock, face, vw, vh).grown(face_reach * u);
            let power = rect(&cfg, Element::Power, power_size(&cfg), vw, vh);
            let logo = rect(&cfg, Element::Logo, (cfg.logo.size, cfg.logo.size), vw, vh);
            let at = format!("{id} on {vw}×{vh}{}", if manual { ", typing a name" } else { "" });
            for (what, r) in [("the sign-in", card), ("the clock", face), ("the power buttons", power), ("the logo", logo)] {
                assert!(r.inside(vw, vh, margin), "{at}: {what} leaves the screen: {r:?}");
            }
            for (a, b, ra, rb) in [("sign-in", "clock", card, face), ("sign-in", "power", card, power), ("sign-in", "logo", card, logo), ("clock", "logo", face, logo), ("clock", "power", face, power)] {
                assert!(!ra.overlaps(rb), "{at}: the {a} {ra:?} overlaps the {b} {rb:?}");
            }
            match side {
                Side::Left => assert!(card.x1 <= vw / 2.0, "{at}: the sign-in reaches past the middle: {card:?}"),
                Side::Right => assert!(card.x0 >= vw / 2.0, "{at}: the sign-in reaches past the middle: {card:?}"),
                Side::Center => unreachable!(),
            }
        }
    }
}

#[test]
fn ghostline_fits_every_screen_on_the_left() {
    let mut cfg = GreeterConfig::default();
    cfg.apply_look("ghostline");
    let card = (cfg.card.width, ghostline::hud_card_height(&cfg, false));
    check("ghostline", card, ghostline::ruler_clock_size(&cfg), 0.0, Side::Left);
}

#[test]
fn chrome_halo_fits_every_screen_on_the_right() {
    let mut cfg = GreeterConfig::default();
    cfg.apply_look("chrome-halo");
    let card = (cfg.card.width, chrome_halo::bezel_card_height(&cfg, false));
    let d = chrome_halo::halo_diameter(&cfg);
    // The outer orbit and its satellite reach past the ring.
    let reach = d * (chrome_halo::ORBIT_RATIO - 1.0) / 2.0 + 5.0;
    check("chrome-halo", card, (d, d), reach, Side::Right);
}

/// Each look's sign-in side: the side looks keep theirs, the others are
/// centred.
#[test]
fn every_look_puts_the_sign_in_on_its_side() {
    for (id, _, _) in LOOKS {
        let mut cfg = GreeterConfig::default();
        cfg.apply_look(id);
        let want = match id {
            "ghostline" => Side::Left,
            "chrome-halo" => Side::Right,
            // From the golden point, left of the middle (mirrored right to left).
            "meridian" => Side::Left,
            _ => Side::Center,
        };
        assert_eq!(cfg.credentials_side(), want, "{id}");
    }
}

/// Meridian's line sits at the golden height and its type stays clear of
/// the words at the line's ends on every screen: the status under the
/// name ends before the date, the name starts after the session and the
/// keyboard layout, and the typed characters pass under the power words.
#[test]
fn meridian_fits_every_screen_on_its_line() {
    let mut cfg = GreeterConfig::default();
    cfg.apply_look("meridian");
    assert_eq!((cfg.clock.face, cfg.card.style, cfg.background.overlay), (ClockFace::Vast, CardStyle::Horizon, Overlay::Horizon));
    assert!(cfg.theme.light, "dawn first");
    // Hidden as elements, they are words on the line.
    for e in [Element::Power, Element::Keyboard, Element::Users] {
        assert!(!cfg.placement(e).visible, "{e:?}");
    }
    // Proportional text at about 0.62 em a character, capitals.
    let width = |chars: f32, size: f32| chars * 0.62 * size;
    for (vw, vh) in SCREENS {
        let k = mandara_config::login_layout::screen_scale(vw, vh);
        let line = vh * meridian::GOLDEN;
        assert!(line - meridian::BAND_HALF * k > 0.0 && line + meridian::BAND_HALF * k < vh, "{vw}×{vh}: the band fits");
        let margin = meridian::END_MARGIN * k;
        let gx = vw * (1.0 - meridian::GOLDEN);
        let start_words = margin + width(10.0, meridian::WORD_PX * k) + 14.0 * k;
        assert!(gx > start_words, "{vw}×{vh}: the name ({gx}) clears the session word ({start_words})");
        let status_end = gx + width(57.0, meridian::STATUS_PX * k) * 0.9;
        let date_start = vw - margin - width(24.0, cfg.date.size * k);
        assert!(status_end < date_start, "{vw}×{vh}: the status ({status_end}) clears the date ({date_start})");
        let power_start = vw - margin - width(21.0, meridian::WORD_PX * k) - 2.0 * meridian::POWER_GAP * k;
        if meridian::narrow(vw, vh) {
            // Under the line, beside the status.
            assert!(status_end < power_start, "{vw}×{vh}: the status ({status_end}) clears the power words ({power_start})");
        } else {
            // Above the line, past the name and up to 24 typed characters.
            let typed_end = gx + width(14.0, cfg.user.name_size * k) + (26.0 + 24.0 * meridian::RUN_SLOT) * k;
            assert!(typed_end < power_start, "{vw}×{vh}: the typing ({typed_end}) clears the power words ({power_start})");
        }
        let logo = rect(&cfg, Element::Logo, (cfg.logo.size, cfg.logo.size), vw, vh);
        assert!(logo.inside(vw, vh, 8.0 * k) && logo.y1 < line - meridian::BAND_HALF * k, "{vw}×{vh}: the logo");
    }
    let dots_top = meridian::RUN_LIFT + meridian::RUN_DOT * cfg.password.text_size;
    assert!(meridian::WORD_GAP > dots_top + 1.0, "typed characters ({dots_top}) pass under the power words");
}

/// The side looks round-trip through the draft file, are recognised
/// again, stop being recognised once edited, and switch cleanly to and
/// from every other look.
#[test]
fn the_side_looks_round_trip() {
    for id in ["ghostline", "chrome-halo", "meridian"] {
        let mut cfg = GreeterConfig::default();
        cfg.user.hidden = vec!["guest".into()];
        assert!(cfg.apply_look(id));
        assert_eq!(cfg.current_look(), Some(id));
        let back = GreeterConfig::parse(&cfg.to_toml()).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.current_look(), Some(id));
        assert_eq!(back.user.hidden, ["guest"], "{id}: who is listed stays");
        let mut edited = back.clone();
        edited.card.width += 10.0;
        assert_eq!(edited.current_look(), None, "{id}: edited after applying");
        assert!(mandara_theme::catalog::find(&cfg.theme.preset).is_some(), "{id}: its theme exists");
        for (other, _, _) in LOOKS {
            let mut there = cfg.clone();
            there.apply_look(other);
            assert_eq!(there.current_look(), Some(other), "{id} → {other}");
            there.apply_look(id);
            assert_eq!(there, cfg, "{id} → {other} → {id}");
        }
    }
    let mut ghost = GreeterConfig::default();
    ghost.apply_look("ghostline");
    assert_eq!((ghost.clock.face, ghost.card.style, ghost.password.style, ghost.background.overlay), (ClockFace::Ruler, CardStyle::Hud, FieldStyle::Terminal, Overlay::Hud));
    let mut halo = GreeterConfig::default();
    halo.apply_look("chrome-halo");
    assert_eq!((halo.clock.face, halo.card.style, halo.password.style, halo.password.submit, halo.background.overlay), (ClockFace::Halo, CardStyle::Bezel, FieldStyle::Recessed, SubmitButton::Pill, Overlay::Prism));
    // Meridian is recognised with its light colours on or off.
    let mut night = GreeterConfig::default();
    night.apply_look("meridian");
    night.theme.light = false;
    assert_eq!(night.current_look(), Some("meridian"));
    let back = GreeterConfig::parse(&night.to_toml()).unwrap();
    assert!(!back.theme.light && back.current_look() == Some("meridian"));
    // Back to Mandara: nothing of either look is left.
    halo.apply_look("mandara");
    assert_eq!(halo, GreeterConfig::default());
}

/// The new options are written by name and parse back; files from before
/// them keep the defaults.
#[test]
fn the_new_options_parse_by_name() {
    let cfg = GreeterConfig::parse("[clock]\nface = \"ruler\"\n[card]\nstyle = \"bezel\"\n[background]\noverlay = \"prism\"\n[password]\nstyle = \"terminal\"\nsubmit = \"pill\"\n").unwrap();
    assert_eq!((cfg.clock.face, cfg.card.style, cfg.background.overlay, cfg.password.style, cfg.password.submit), (ClockFace::Ruler, CardStyle::Bezel, Overlay::Prism, FieldStyle::Terminal, SubmitButton::Pill));
    let old = GreeterConfig::parse("[clock]\nsize = 120\n").unwrap();
    assert_eq!((old.clock.face, old.card.style, old.background.overlay), (ClockFace::Digits, CardStyle::Plain, Overlay::None));
    assert!(GreeterConfig::parse("[clock]\nface = \"sundial\"\n").is_err());
}

/// The date shows exactly once: the ruler and halo faces draw it, the vast
/// face leaves it to Meridian's line only when that line is the card, and
/// otherwise the date element draws it.
#[test]
fn the_date_shows_once_with_every_face_and_card() {
    let styles = [CardStyle::Plain, CardStyle::Hud, CardStyle::Bezel, CardStyle::Horizon];
    for face in [ClockFace::Digits, ClockFace::Ruler, ClockFace::Halo, ClockFace::Vast] {
        for style in styles {
            for enabled in [true, false] {
                let mut cfg = GreeterConfig::default();
                cfg.clock.face = face;
                cfg.card.style = style;
                cfg.card.enabled = enabled;
                let on_the_line = enabled && style == CardStyle::Horizon && face == ClockFace::Vast;
                let in_the_face = matches!(face, ClockFace::Ruler | ClockFace::Halo);
                assert_eq!(face_holds_date(&cfg), in_the_face || on_the_line, "{face:?} with {style:?} (card on: {enabled})");
            }
        }
    }
    // Meridian's own look keeps its date on the line.
    let mut meridian = GreeterConfig::default();
    meridian.apply_look("meridian");
    assert!(face_holds_date(&meridian));
    // The vast face on the plain card puts the date element back.
    meridian.card.style = CardStyle::Plain;
    assert!(!face_holds_date(&meridian));
}
