//! Pure geometry of floating cards and of the docked panel's corner tab:
//! where a card may go on its output, where it first lands, its size, the
//! input region of the floating surface, and the tab hanging off a docked
//! panel. Logical pixels, output-relative (the floating surface and the
//! shell surface both cover their whole output).

use mandara_config::BarPosition;
use mandara_core::Edge;

use crate::dismiss::Rect;

/// The part of an `output` a card may cover: all of it but the band the
/// bar reserves (`zone` px) along its edge.
pub fn usable_area((w, h): (f32, f32), position: BarPosition, zone: f32) -> Rect {
    let z = if zone.is_finite() { zone.clamp(0.0, w.min(h)) } else { 0.0 };
    match position {
        BarPosition::Top => Rect::new(0.0, z, w, h - z),
        BarPosition::Bottom => Rect::new(0.0, 0.0, w, h - z),
        BarPosition::Left => Rect::new(z, 0.0, w - z, h),
        BarPosition::Right => Rect::new(0.0, 0.0, w - z, h),
    }
}

/// The top-left that keeps a card of `size` inside `area`, `margin` px
/// clear of its edges. A card larger than the room pins to the room's
/// top-left, so its grab handle always stays reachable; a non-finite
/// position (a broken hand edit) lands there too.
pub fn clamp_origin((x, y): (f32, f32), (w, h): (f32, f32), area: Rect, margin: f32) -> (f32, f32) {
    let axis = |v: f32, start: f32, room: f32, len: f32| {
        let (lo, hi) = (start + margin, start + room - margin - len);
        if hi < lo || !v.is_finite() { lo } else { v.clamp(lo, hi) }
    };
    (axis(x, area.x, area.w, w), axis(y, area.y, area.h, h))
}

/// What a card floats out of.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Anchor {
    /// The open docked panel: the card lifts off it.
    Panel(Rect),
    /// The plugin's bar chip: the card lands where its panel would hang.
    Chip(Rect),
}

/// One step away from the bar: `d` px along the direction panels hang.
fn away(position: BarPosition, d: f32) -> (f32, f32) {
    match position {
        BarPosition::Top => (0.0, d),
        BarPosition::Bottom => (0.0, -d),
        BarPosition::Left => (d, 0.0),
        BarPosition::Right => (-d, 0.0),
    }
}

/// Where a card first floats (nothing remembered): lifted `lift` px off
/// the docked panel it was, or hanging `lift` px past its chip, or in the
/// middle of the room without either; then clamped.
pub fn float_origin(anchor: Option<Anchor>, (w, h): (f32, f32), position: BarPosition, area: Rect, margin: f32, lift: f32) -> (f32, f32) {
    let raw = match anchor {
        None => (area.x + (area.w - w) / 2.0, area.y + (area.h - h) / 2.0),
        Some(Anchor::Panel(r)) => {
            let (dx, dy) = away(position, lift);
            (r.x + dx, r.y + dy)
        }
        Some(Anchor::Chip(r)) => {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            match position {
                BarPosition::Top => (cx - w / 2.0, r.y + r.h + lift),
                BarPosition::Bottom => (cx - w / 2.0, r.y - lift - h),
                BarPosition::Left => (r.x + r.w + lift, cy - h / 2.0),
                BarPosition::Right => (r.x - lift - w, cy - h / 2.0),
            }
        }
    };
    clamp_origin(raw, (w, h), area, margin)
}

/// A card's chrome: inner padding, the grab strip along its top, and the
/// gap kept to the output's edges.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Chrome {
    pub pad: f32,
    pub strip: f32,
    pub margin: f32,
}

/// The content size of a card whose plugin asks for `want`: the width
/// scaled like docked panels (`width_scale`), both capped to what fits in
/// `area` with the chrome around it.
pub fn content_size((w, h): (f32, f32), width_scale: f32, area: Rect, c: Chrome) -> (f32, f32) {
    let max_w = (area.w - 2.0 * (c.margin + c.pad)).max(0.0);
    let max_h = (area.h - 2.0 * c.margin - c.strip - c.pad).max(0.0);
    ((w * width_scale).clamp(0.0, max_w), h.clamp(0.0, max_h))
}

/// The card around `content`: padding at the sides and bottom, the grab
/// strip on top.
pub fn card_size((w, h): (f32, f32), c: Chrome) -> (f32, f32) {
    (w + 2.0 * c.pad, c.strip + h + c.pad)
}

/// Whether `p` lies on the chrome of a card drawn at `card` with
/// `content` inside it: the grab strip and the padding around the
/// plugin's content, where a press drags the card. Presses on the content
/// itself belong to the plugin.
pub fn on_chrome(card: Rect, (w, h): (f32, f32), c: Chrome, (x, y): (f32, f32)) -> bool {
    let inside = |r: Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
    inside(card) && !inside(Rect::new(card.x + c.pad, card.y + c.strip, w, h))
}

/// The floating surface's input region: every card, or the whole surface
/// (`everything`) while a drag or typing must see presses anywhere.
/// Empty rects never enter it.
pub fn input_region(cards: &[Rect], everything: Option<Rect>) -> Vec<Rect> {
    match everything {
        Some(all) => vec![all],
        None => cards.iter().copied().filter(|r| r.w > 0.0 && r.h > 0.0).collect(),
    }
}

/// The tab of pin / pop-out buttons hanging off a docked panel's far edge
/// (the edge away from the bar), near its end corner.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tab {
    /// Its full rect, outside the panel.
    pub rect: Rect,
    /// The side it is attached by (drawn as a drawer on that edge).
    pub edge: Edge,
    /// It runs along a vertical edge (a side bar's panel).
    pub vertical: bool,
}

/// [`Tab`] of `len` × `depth` px on `panel`, `inset` px from the far
/// edge's end corner (the start corner in right-to-left layouts; the top
/// beside a side bar), centred when the panel is too short for that.
pub fn tab(panel: Rect, position: BarPosition, len: f32, depth: f32, inset: f32, rtl: bool) -> Tab {
    let along = |start: f32, room: f32, at_end: bool| {
        if room < len + 2.0 * inset {
            start + (room - len) / 2.0
        } else if at_end {
            start + room - inset - len
        } else {
            start + inset
        }
    };
    match position {
        BarPosition::Top => Tab { rect: Rect::new(along(panel.x, panel.w, !rtl), panel.y + panel.h, len, depth), edge: Edge::Top, vertical: false },
        BarPosition::Bottom => Tab { rect: Rect::new(along(panel.x, panel.w, !rtl), panel.y - depth, len, depth), edge: Edge::Bottom, vertical: false },
        BarPosition::Left => Tab { rect: Rect::new(panel.x + panel.w, along(panel.y, panel.h, false), depth, len), edge: Edge::Left, vertical: true },
        BarPosition::Right => Tab { rect: Rect::new(panel.x - depth, along(panel.y, panel.h, false), depth, len), edge: Edge::Right, vertical: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: (f32, f32) = (2560.0, 1440.0);

    #[test]
    fn the_bar_band_is_off_limits() {
        assert_eq!(usable_area(OUT, BarPosition::Top, 40.0), Rect::new(0.0, 40.0, 2560.0, 1400.0));
        assert_eq!(usable_area(OUT, BarPosition::Bottom, 40.0), Rect::new(0.0, 0.0, 2560.0, 1400.0));
        assert_eq!(usable_area(OUT, BarPosition::Left, 50.0), Rect::new(50.0, 0.0, 2510.0, 1440.0));
        assert_eq!(usable_area(OUT, BarPosition::Right, 50.0), Rect::new(0.0, 0.0, 2510.0, 1440.0));
        assert_eq!(usable_area(OUT, BarPosition::Top, f32::NAN), Rect::new(0.0, 0.0, 2560.0, 1440.0), "a broken zone reserves nothing");
    }

    #[test]
    fn positions_clamp_to_the_output() {
        let area = usable_area(OUT, BarPosition::Top, 40.0);
        let size = (400.0, 300.0);
        assert_eq!(clamp_origin((100.0, 200.0), size, area, 8.0), (100.0, 200.0), "inside stays put");
        assert_eq!(clamp_origin((-50.0, 0.0), size, area, 8.0), (8.0, 48.0), "never under the bar or off the left edge");
        assert_eq!(clamp_origin((5000.0, 5000.0), size, area, 8.0), (2560.0 - 8.0 - 400.0, 1440.0 - 8.0 - 300.0));
        assert_eq!(clamp_origin((300.0, 300.0), (3000.0, 2000.0), area, 8.0), (8.0, 48.0), "too big: the handle stays reachable");
        assert_eq!(clamp_origin((f32::NAN, f32::INFINITY), size, area, 8.0), (8.0, 48.0));
        // A position remembered on a bigger output comes back on screen.
        let small = usable_area((1920.0, 1080.0), BarPosition::Top, 40.0);
        let (x, y) = clamp_origin((2100.0, 1100.0), size, small, 8.0);
        assert!(x + 400.0 <= 1920.0 && y + 300.0 <= 1080.0);
    }

    #[test]
    fn first_float_lifts_off_the_panel_or_hangs_from_the_chip() {
        let area = usable_area(OUT, BarPosition::Top, 40.0);
        let panel = Rect::new(1000.0, 40.0, 420.0, 500.0);
        assert_eq!(float_origin(Some(Anchor::Panel(panel)), (420.0, 500.0), BarPosition::Top, area, 8.0, 24.0), (1000.0, 64.0));
        let chip = Rect::new(1180.0, 4.0, 60.0, 28.0);
        assert_eq!(float_origin(Some(Anchor::Chip(chip)), (400.0, 300.0), BarPosition::Top, area, 8.0, 24.0), (1010.0, 56.0));
        let bottom = usable_area(OUT, BarPosition::Bottom, 40.0);
        let chip = Rect::new(1180.0, 1408.0, 60.0, 28.0);
        assert_eq!(float_origin(Some(Anchor::Chip(chip)), (400.0, 300.0), BarPosition::Bottom, bottom, 8.0, 24.0), (1010.0, 1408.0 - 24.0 - 300.0));
        assert_eq!(float_origin(None, (400.0, 300.0), BarPosition::Top, area, 8.0, 24.0), (1080.0, 40.0 + 550.0), "centred without an anchor");
        // A chip at the screen's edge: the card stays on screen.
        let edge_chip = Rect::new(2540.0, 4.0, 20.0, 28.0);
        let (x, _) = float_origin(Some(Anchor::Chip(edge_chip)), (400.0, 300.0), BarPosition::Top, area, 8.0, 24.0);
        assert_eq!(x, 2560.0 - 8.0 - 400.0);
    }

    #[test]
    fn cards_fit_their_room() {
        let chrome = Chrome { pad: 12.0, strip: 22.0, margin: 8.0 };
        let area = Rect::new(0.0, 40.0, 1920.0, 1040.0);
        assert_eq!(content_size((400.0, 300.0), 1.0, area, chrome), (400.0, 300.0));
        assert_eq!(content_size((400.0, 300.0), 1.25, area, chrome), (500.0, 300.0), "the panel width scale applies");
        let (_, h) = content_size((400.0, 5000.0), 1.0, area, chrome);
        let (_, card_h) = card_size((400.0, h), chrome);
        assert_eq!(card_h, 1040.0 - 16.0, "a tall panel fits between the margins");
        assert_eq!(card_size((400.0, 300.0), chrome), (424.0, 334.0));
    }

    #[test]
    fn the_chrome_drags_and_the_content_does_not() {
        let chrome = Chrome { pad: 6.0, strip: 28.0, margin: 8.0 };
        let card = Rect::new(100.0, 200.0, 412.0, 334.0);
        let content = (400.0, 300.0);
        assert!(on_chrome(card, content, chrome, (300.0, 210.0)), "the grab strip, across its whole width");
        assert!(on_chrome(card, content, chrome, (102.0, 400.0)), "the left edge");
        assert!(on_chrome(card, content, chrome, (510.0, 400.0)), "the right edge");
        assert!(on_chrome(card, content, chrome, (300.0, 531.0)), "the bottom edge");
        assert!(!on_chrome(card, content, chrome, (300.0, 400.0)), "the plugin's content");
        assert!(!on_chrome(card, content, chrome, (106.0, 228.0)), "the content's top-left corner");
        assert!(!on_chrome(card, content, chrome, (90.0, 400.0)), "outside the card");
    }

    #[test]
    fn the_region_is_the_cards_or_everything() {
        let cards = [Rect::new(10.0, 10.0, 100.0, 80.0), Rect::new(0.0, 0.0, 0.0, 50.0), Rect::new(500.0, 40.0, 300.0, 200.0)];
        assert_eq!(input_region(&cards, None), vec![cards[0], cards[2]], "empty cards never take input");
        let all = Rect::new(0.0, 0.0, 2560.0, 1440.0);
        assert_eq!(input_region(&cards, Some(all)), vec![all], "a drag or typing sees every press");
        assert!(input_region(&[], None).is_empty(), "no cards: every click passes through");
    }

    #[test]
    fn the_tab_hangs_off_the_far_edge() {
        let panel = Rect::new(1000.0, 40.0, 420.0, 500.0);
        let t = tab(panel, BarPosition::Top, 60.0, 28.0, 20.0, false);
        assert_eq!((t.rect, t.edge, t.vertical), (Rect::new(1340.0, 540.0, 60.0, 28.0), Edge::Top, false), "under the bottom edge, at the right");
        assert_eq!(tab(panel, BarPosition::Top, 60.0, 28.0, 20.0, true).rect.x, 1020.0, "at the left in right-to-left layouts");
        let up = Rect::new(1000.0, 900.0, 420.0, 500.0);
        assert_eq!(tab(up, BarPosition::Bottom, 60.0, 28.0, 20.0, false).rect, Rect::new(1340.0, 872.0, 60.0, 28.0));
        let side = Rect::new(50.0, 100.0, 400.0, 600.0);
        let t = tab(side, BarPosition::Left, 60.0, 28.0, 20.0, false);
        assert_eq!((t.rect, t.edge, t.vertical), (Rect::new(450.0, 120.0, 28.0, 60.0), Edge::Left, true));
        assert_eq!(tab(side, BarPosition::Right, 60.0, 28.0, 20.0, false).rect, Rect::new(22.0, 120.0, 28.0, 60.0));
        let narrow = Rect::new(0.0, 40.0, 80.0, 100.0);
        assert_eq!(tab(narrow, BarPosition::Top, 60.0, 28.0, 20.0, false).rect.x, 10.0, "centred on a narrow panel");
    }
}
