//! The ring menu's geometry: where its items sit, which one a point is
//! on, how the ring stays on screen and how the keys move around it.
//!
//! Pure functions of logical px and radians, no GPUI: computed once when
//! the menu opens (or the outer ring opens, or the surface is resized)
//! and only read while it animates. Angles are screen angles (`y` grows
//! downward): `-π/2` points up and growing angles turn clockwise. The
//! first item sits at the top and the rest follow the reading direction:
//! clockwise left to right, counter-clockwise right to left.

use std::f32::consts::{PI, TAU};

/// Room between an item's button and the edges of the ring's band.
pub const BAND_PAD: f32 = 10.0;
/// Gap between the band and the hub in the middle.
pub const HUB_GAP: f32 = 6.0;
/// Smallest hub radius (it names the item under the pointer).
pub const HUB_MIN: f32 = 34.0;
/// Least space between two neighbouring buttons along the ring.
pub const ITEM_GAP: f32 = 14.0;
/// Captions under the icons ("always" labels): width and height.
pub const CAPTION_W: f32 = 100.0;
pub const CAPTION_H: f32 = 16.0;
/// Least distance from the screen edges.
pub const EDGE: f32 = 8.0;
/// A press this far outside the ring still counts as on it.
pub const HIT_SLOP: f32 = 6.0;
/// The outer ring: gap to the main ring, gap between its buttons and its
/// buttons' size against the main ring's.
pub const SUB_GAP: f32 = 12.0;
pub const SUB_ITEM_GAP: f32 = 8.0;
pub const SUB_SCALE: f32 = 0.9;
/// Holding the button, moving this far from the press picks a direction.
pub const DRAG_DEAD: f32 = 22.0;
/// The outer ring turns in steps of this many degrees, at most half a
/// turn either way, to stay on screen.
const ROTATE_STEP_DEG: f32 = 2.0;
const ROTATE_STEPS: usize = 90;
/// Smallest ring radius when a tiny screen shrinks it.
const MIN_RADIUS: f32 = 56.0;
/// Up / straight above the centre.
pub const TOP: f32 = -PI / 2.0;

/// The main ring.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ring {
    pub n: usize,
    /// Centre to the items' centres.
    pub radius: f32,
    /// A button's diameter.
    pub item: f32,
    /// Caption height under each icon (0 without captions).
    pub caption: f32,
    /// The hub's radius, the dead zone.
    pub hub: f32,
    /// The item band's inner and outer radius (the outer is the glass).
    pub inner: f32,
    pub outer: f32,
    /// Angle of item 0 and from one item to the next (signed).
    pub start: f32,
    pub step: f32,
}

/// What a point on the ring is on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Hub,
    Item(usize),
    Outside,
}

impl Ring {
    /// `n` items of `item` px around `radius` (grown when they would not
    /// fit, or the hub would get too small), with `captions` under them.
    pub fn new(n: usize, radius: f32, item: f32, captions: bool, rtl: bool) -> Self {
        let caption = if captions { CAPTION_H } else { 0.0 };
        let cell = if captions { item.max(CAPTION_W) } else { item };
        let half = (item + caption) / 2.0;
        let mut r = radius.max(half + BAND_PAD + HUB_GAP + HUB_MIN);
        if n >= 2 {
            // The chord between neighbours holds a button and a gap.
            r = r.max((cell + ITEM_GAP) / (2.0 * (PI / n as f32).sin()));
        }
        Self::with_radius(n, r, item, caption, rtl)
    }

    fn with_radius(n: usize, radius: f32, item: f32, caption: f32, rtl: bool) -> Self {
        let half = (item + caption) / 2.0;
        let inner = (radius - half - BAND_PAD).max(0.0);
        let outer = radius + half + BAND_PAD;
        let hub = (inner - HUB_GAP).max(0.0);
        let step = if n == 0 { 0.0 } else { TAU / n as f32 } * if rtl { -1.0 } else { 1.0 };
        Self { n, radius, item, caption, hub, inner, outer, start: TOP, step }
    }

    /// The ring shrunk to fit a `screen` smaller than it (never below a
    /// usable size; items may then crowd).
    pub fn fit_screen(self, screen: (f32, f32)) -> Self {
        let room = (screen.0.min(screen.1) / 2.0 - EDGE).max(0.0);
        if self.outer <= room {
            return self;
        }
        let r = (self.radius - (self.outer - room)).max(MIN_RADIUS);
        Self::with_radius(self.n, r, self.item, self.caption, self.step < 0.0)
    }

    pub fn angle(&self, i: usize) -> f32 {
        self.start + i as f32 * self.step
    }

    /// Item `i`'s centre for a ring centred on `c`.
    pub fn pos(&self, c: (f32, f32), i: usize) -> (f32, f32) {
        polar(c, self.radius, self.angle(i))
    }

    /// The item whose sector holds angle `theta`.
    pub fn sector(&self, theta: f32) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        let rel = wrap(theta - self.start) / self.step;
        Some((rel.round() as i64).rem_euclid(self.n as i64) as usize)
    }

    /// What `p` is on, for a ring centred on `c`.
    pub fn hit(&self, c: (f32, f32), p: (f32, f32)) -> Hit {
        let d = dist(c, p);
        if d < self.hub + HUB_GAP / 2.0 {
            Hit::Hub
        } else if d <= self.outer + HIT_SLOP {
            self.sector(angle(c, p)).map_or(Hit::Outside, Hit::Item)
        } else {
            Hit::Outside
        }
    }

    /// The item a drag from `press` toward `p` points at: the one under the
    /// pointer, else (once it left the dead zone) the one whose direction
    /// from the press is nearest. The press is the centre unless the ring
    /// moved away from a screen edge.
    pub fn drag(&self, c: (f32, f32), press: (f32, f32), p: (f32, f32)) -> Option<usize> {
        // Nothing until the pointer leaves the press's dead zone, even
        // where the press itself sits on an item (a ring moved off an edge).
        if dist(press, p) < DRAG_DEAD || self.n == 0 {
            return None;
        }
        if let Hit::Item(i) = self.hit(c, p) {
            return Some(i);
        }
        let a = angle(press, p);
        (0..self.n).min_by(|&i, &j| {
            let di = wrap(angle(press, self.pos(c, i)) - a).abs();
            let dj = wrap(angle(press, self.pos(c, j)) - a).abs();
            di.total_cmp(&dj)
        })
    }
}

/// An outer ring (a submenu) around the same centre.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Arc {
    pub n: usize,
    pub radius: f32,
    pub item: f32,
    /// The arc's middle (the parent's direction, maybe turned to fit).
    pub mid: f32,
    pub step: f32,
    /// The items go all the way round.
    pub full: bool,
}

impl Arc {
    /// `n` items outside `ring`, centred on `mid`.
    pub fn new(n: usize, ring: &Ring, mid: f32, rtl: bool) -> Self {
        Self::around(n, ring.outer, (ring.item * SUB_SCALE).round(), mid, rtl)
    }

    /// `n` items of `item` px outside a circle of `inner` radius (the main
    /// glass, or the ring before), centred on `mid`.
    pub fn around(n: usize, inner: f32, item: f32, mid: f32, rtl: bool) -> Self {
        let pitch = item + SUB_ITEM_GAP;
        let mut radius = inner + SUB_GAP + item / 2.0;
        let mut step = pitch / radius;
        let full = n as f32 * step >= TAU;
        if full {
            radius = n as f32 * pitch / TAU;
            step = TAU / n as f32;
        }
        Self { n, radius, item, mid, step: step * if rtl { -1.0 } else { 1.0 }, full }
    }

    pub fn angle(&self, j: usize) -> f32 {
        if self.full { self.mid + j as f32 * self.step } else { self.mid + (j as f32 - (self.n as f32 - 1.0) / 2.0) * self.step }
    }

    pub fn pos(&self, c: (f32, f32), j: usize) -> (f32, f32) {
        polar(c, self.radius, self.angle(j))
    }

    /// Every item fully inside `screen` (with the edge margin).
    pub fn fits(&self, c: (f32, f32), screen: (f32, f32)) -> bool {
        let h = self.item / 2.0;
        (0..self.n).all(|j| {
            let (x, y) = self.pos(c, j);
            x - h >= EDGE && y - h >= EDGE && x + h <= screen.0 - EDGE && y + h <= screen.1 - EDGE
        })
    }

    /// How far its items reach from the centre.
    pub fn reach(&self) -> f32 {
        self.radius + self.item / 2.0
    }

    /// The item `p` is on (its band, the nearest by angle).
    pub fn hit(&self, c: (f32, f32), p: (f32, f32)) -> Option<usize> {
        let d = dist(c, p);
        let band = self.item / 2.0 + SUB_ITEM_GAP / 2.0 + HIT_SLOP;
        if (d - self.radius).abs() > band {
            return None;
        }
        self.by_angle(angle(c, p))
    }

    /// The item nearest to direction `theta`, if within the arc (half a
    /// step past either end).
    pub fn by_angle(&self, theta: f32) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        if self.full {
            let rel = wrap(theta - self.mid) / self.step;
            return Some((rel.round() as i64).rem_euclid(self.n as i64) as usize);
        }
        // A partial arc spans less than a turn: measure from its middle.
        let last = (self.n - 1) as f32;
        let rel = wrap(theta - self.mid) / self.step + last / 2.0;
        (-0.5..=last + 0.5).contains(&rel).then(|| rel.round().clamp(0.0, last) as usize)
    }
}

/// Places an outer ring of `n` items for the parent at `parent` (its
/// angle) of `ring` centred on `c`: turned as little as needed for every
/// item to stay on `screen`, and when no turn is enough, around a centre
/// moved inward so the whole outer ring fits. Returns it and its centre.
pub fn place_arc(n: usize, parent: f32, ring: &Ring, c: (f32, f32), screen: (f32, f32), rtl: bool) -> (Arc, (f32, f32)) {
    place(Arc::new(n, ring, parent, rtl), c, screen)
}

/// Places the next level out: `n` items outside `prev` (a ring of outer
/// items on `c`) for its item `parent`, as [`place_arc`] does.
pub fn place_next(n: usize, prev: &Arc, parent: usize, c: (f32, f32), screen: (f32, f32), rtl: bool) -> (Arc, (f32, f32)) {
    place(Arc::around(n, prev.reach(), prev.item, prev.angle(parent), rtl), c, screen)
}

/// `base` turned as little as needed to fit `screen` around `c`, else
/// around a centre moved inward so the whole ring fits.
fn place(base: Arc, c: (f32, f32), screen: (f32, f32)) -> (Arc, (f32, f32)) {
    let parent = base.mid;
    let turned = |c: (f32, f32)| {
        (0..=ROTATE_STEPS).find_map(|k| {
            let d = (k as f32 * ROTATE_STEP_DEG).to_radians();
            [d, -d].into_iter().map(|d| Arc { mid: parent + d, ..base }).find(|a| a.fits(c, screen))
        })
    };
    if let Some(a) = turned(c) {
        return (a, c);
    }
    let reach = base.reach();
    let moved = fit_center(c, (reach, reach), screen);
    (turned(moved).unwrap_or(base), moved)
}

/// The centre nearest to `at` that keeps a box of half-size `half` on
/// `screen` (centred on a screen too small for it).
pub fn fit_center(at: (f32, f32), half: (f32, f32), screen: (f32, f32)) -> (f32, f32) {
    let axis = |v: f32, h: f32, len: f32| if len - 2.0 * EDGE < 2.0 * h { len / 2.0 } else { v.clamp(EDGE + h, len - EDGE - h) };
    (axis(at.0, half.0, screen.0), axis(at.1, half.1, screen.1))
}

/// Where a label beside item at `theta` sits outside a ring of `outer`
/// radius on `c`: the anchor point and which way the label grows
/// horizontally (-1 left of it, 0 centred, 1 right) and vertically.
pub fn label_anchor(c: (f32, f32), outer: f32, gap: f32, theta: f32) -> ((f32, f32), (i8, i8)) {
    let p = polar(c, outer + gap, theta);
    let side = |v: f32| if v > 0.35 { 1 } else if v < -0.35 { -1 } else { 0 };
    (p, (side(theta.cos()), side(theta.sin())))
}

/// How a key moves the selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Nav {
    Next,
    Prev,
    First,
    Last,
    Choose,
    /// Out of the outer ring (or close when there is none).
    Back,
    Close,
}

/// The key `key` (GPUI names) with `shift` held: arrows turn the ring
/// (right clockwise, left counter-clockwise, so in right-to-left text
/// right goes back), up / down and Tab / Shift+Tab follow the reading
/// order.
pub fn nav_for(key: &str, shift: bool, rtl: bool) -> Option<Nav> {
    Some(match key {
        "right" => if rtl { Nav::Prev } else { Nav::Next },
        "left" => if rtl { Nav::Next } else { Nav::Prev },
        "down" => Nav::Next,
        "up" => Nav::Prev,
        "tab" => if shift { Nav::Prev } else { Nav::Next },
        "home" => Nav::First,
        "end" => Nav::Last,
        "enter" | "space" => Nav::Choose,
        "backspace" => Nav::Back,
        "escape" => Nav::Close,
        _ => return None,
    })
}

/// The selection after `nav` from `cur` among `n` items, skipping the ones
/// `usable` refuses (`None` when none is).
pub fn step(cur: Option<usize>, n: usize, nav: Nav, usable: impl Fn(usize) -> bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let (from, forward) = match (nav, cur) {
        (Nav::First, _) | (Nav::Next, None) => (n - 1, true),
        (Nav::Last, _) | (Nav::Prev, None) => (0, false),
        (Nav::Next, Some(c)) => (c.min(n - 1), true),
        (Nav::Prev, Some(c)) => (c.min(n - 1), false),
        _ => return cur,
    };
    (1..=n).map(|k| if forward { (from + k) % n } else { (from + n - k % n) % n }).find(|&i| usable(i))
}

pub fn polar(c: (f32, f32), r: f32, theta: f32) -> (f32, f32) {
    (c.0 + r * theta.cos(), c.1 + r * theta.sin())
}

pub fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// Direction from `a` to `b`.
pub fn angle(a: (f32, f32), b: (f32, f32)) -> f32 {
    (b.1 - a.1).atan2(b.0 - a.0)
}

/// `a` wrapped into (-π, π].
pub fn wrap(a: f32) -> f32 {
    let w = (a + PI).rem_euclid(TAU) - PI;
    if w <= -PI { w + TAU } else { w }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f32, f32) = (1920.0, 1080.0);

    fn ring(n: usize, rtl: bool) -> Ring {
        Ring::new(n, 100.0, 44.0, false, rtl)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn items_start_at_the_top_and_follow_the_reading_direction() {
        let r = ring(8, false);
        let c = (500.0, 500.0);
        let (x, y) = r.pos(c, 0);
        assert!(close(x, 500.0) && close(y, 400.0), "item 0 is straight above");
        let (x, y) = r.pos(c, 2);
        assert!(close(x, 600.0) && close(y, 500.0), "a quarter on, clockwise: to the right");
        let rtl = ring(8, true);
        let (x, _) = rtl.pos(c, 2);
        assert!(close(x, 400.0), "right to left goes the other way round");
        // Neighbours never overlap, however many items there are.
        for n in 2..=20 {
            let r = ring(n, false);
            assert!(dist(r.pos(c, 0), r.pos(c, 1)) >= r.item + ITEM_GAP - 0.01, "{n} items");
            assert!(r.hub >= HUB_MIN);
            assert!(r.inner < r.radius - r.item / 2.0 && r.outer > r.radius + r.item / 2.0);
        }
        // Captions widen the cells: the ring grows.
        assert!(Ring::new(10, 100.0, 44.0, true, false).radius > ring(10, false).radius);
    }

    #[test]
    fn hit_testing_has_a_dead_zone_in_the_middle_and_sectors_around_it() {
        let r = ring(8, false);
        let c = (500.0, 500.0);
        assert_eq!(r.hit(c, c), Hit::Hub);
        assert_eq!(r.hit(c, (500.0 + r.hub - 1.0, 500.0)), Hit::Hub);
        // On each item and just beside it, within its sector.
        for i in 0..8 {
            assert_eq!(r.hit(c, r.pos(c, i)), Hit::Item(i));
            let a = r.angle(i) + r.step * 0.45;
            assert_eq!(r.hit(c, polar(c, r.radius, a)), Hit::Item(i), "inside sector {i}");
            assert_eq!(r.hit(c, polar(c, r.radius, r.angle(i) + r.step * 0.55)), Hit::Item((i + 1) % 8));
        }
        // Past the glass (and its slop) is outside.
        assert_eq!(r.hit(c, (500.0, 500.0 - r.outer - HIT_SLOP - 1.0)), Hit::Outside);
        assert_eq!(r.hit(c, (500.0, 500.0 - r.outer - 1.0)), Hit::Item(0));
        // The same in right-to-left.
        let rtl = ring(8, true);
        for i in 0..8 {
            assert_eq!(rtl.hit(c, rtl.pos(c, i)), Hit::Item(i));
        }
        assert_eq!(rtl.hit(c, (500.0 - rtl.radius, 500.0)), Hit::Item(2), "a quarter on to the left");
        // No items: nothing to hit.
        assert_eq!(Ring::new(0, 100.0, 44.0, false, false).hit(c, (600.0, 500.0)), Hit::Outside);
    }

    #[test]
    fn dragging_picks_by_direction_once_past_the_dead_zone() {
        let r = ring(8, false);
        let c = (500.0, 500.0);
        assert_eq!(r.drag(c, c, (505.0, 505.0)), None, "still in the dead zone");
        assert_eq!(r.drag(c, c, (500.0 + 30.0, 500.0)), Some(2), "a short flick to the right");
        assert_eq!(r.drag(c, c, (500.0, 1000.0)), Some(4), "far past the ring, straight down");
        // A ring moved off a corner: directions count from the press.
        let press = (20.0, 20.0);
        let moved = fit_center(press, (r.outer, r.outer), SCREEN);
        assert_ne!(moved, press);
        let toward = r.pos(moved, 3);
        let along = |f: f32| (press.0 + (toward.0 - press.0) * f, press.1 + (toward.1 - press.1) * f);
        assert_eq!(r.drag(moved, press, along(0.05)), None, "still near the press");
        // Over the hub on the way: aimed from the press at item 3.
        assert_eq!(r.hit(moved, along(0.6)), Hit::Hub);
        assert_eq!(r.drag(moved, press, along(0.6)), Some(3));
        // Over an item on the way: that item, as it is under the pointer.
        let Hit::Item(under) = r.hit(moved, along(0.2)) else { panic!("over the ring") };
        assert_eq!(r.drag(moved, press, along(0.2)), Some(under));
        assert_eq!(r.drag(moved, press, toward), Some(3));
    }

    #[test]
    fn the_ring_moves_off_every_edge_and_corner() {
        let r = ring(10, false);
        let half = (r.outer, r.outer);
        let points = [(0.0, 0.0), (960.0, 0.0), (1920.0, 0.0), (0.0, 540.0), (1920.0, 540.0), (0.0, 1080.0), (960.0, 1080.0), (1920.0, 1080.0), (3.0, 1077.0)];
        for at in points {
            let c = fit_center(at, half, SCREEN);
            for i in 0..r.n {
                let (x, y) = r.pos(c, i);
                let h = r.item / 2.0;
                assert!(x - h >= EDGE && y - h >= EDGE && x + h <= SCREEN.0 - EDGE && y + h <= SCREEN.1 - EDGE, "{at:?}: item {i} at {x},{y}");
            }
            assert!(c.0 - r.outer >= EDGE - 0.01 && c.1 - r.outer >= EDGE - 0.01 && c.0 + r.outer <= SCREEN.0 - EDGE + 0.01 && c.1 + r.outer <= SCREEN.1 - EDGE + 0.01, "{at:?}: the glass fits");
        }
        // In the open it stays exactly where the click was.
        assert_eq!(fit_center((900.0, 500.0), half, SCREEN), (900.0, 500.0));
        // A screen too small for the ring centres it, after shrinking it.
        let tiny = (200.0, 180.0);
        let small = r.fit_screen(tiny);
        assert!(small.radius < r.radius && small.outer <= 90.0 - EDGE + 0.01 || small.radius == MIN_RADIUS);
        assert_eq!(fit_center((5.0, 5.0), (500.0, 500.0), tiny), (100.0, 90.0));
    }

    #[test]
    fn the_outer_ring_opens_toward_its_parent_and_turns_to_stay_on_screen() {
        let r = ring(10, false);
        let c = (960.0, 540.0);
        // In the open: centred on the parent.
        let parent = r.angle(1);
        let (a, at) = place_arc(5, parent, &r, c, SCREEN, false);
        assert_eq!(at, c);
        assert!(close(a.mid, parent) && !a.full);
        assert!(a.radius - a.item / 2.0 >= r.outer + SUB_GAP - 0.01, "outside the main glass");
        let (x0, y0) = a.pos(c, 0);
        let (x1, y1) = a.pos(c, 1);
        assert!(dist((x0, y0), (x1, y1)) >= a.item + SUB_ITEM_GAP - 0.5);
        // Every item answers its own position and none past the arc's ends.
        for j in 0..5 {
            assert_eq!(a.hit(c, a.pos(c, j)), Some(j));
        }
        assert_eq!(a.by_angle(a.angle(0) - a.step), None);
        assert_eq!(a.by_angle(a.angle(4) + a.step), None);
        assert_eq!(a.by_angle(a.angle(0) - a.step * 0.4), Some(0));
        assert_eq!(a.hit(c, c), None);
        // Near every edge and corner the arc turns (or the centre moves) so
        // every item stays on screen.
        for at in [(10.0, 10.0), (1910.0, 10.0), (10.0, 1070.0), (1910.0, 1070.0), (960.0, 5.0), (5.0, 540.0), (1915.0, 540.0), (960.0, 1075.0)] {
            let main = fit_center(at, (r.outer, r.outer), SCREEN);
            for parent in 0..r.n {
                for n in [3, 8, 20] {
                    let (a, c2) = place_arc(n, r.angle(parent), &r, main, SCREEN, false);
                    assert!(a.fits(c2, SCREEN), "{at:?} parent {parent}, {n} items");
                    // The main ring still fits around a moved centre.
                    assert!(c2.0 - r.outer >= EDGE - 0.01 && c2.1 - r.outer >= EDGE - 0.01);
                }
            }
        }
        // Many items go all the way round.
        let (a, _) = place_arc(30, TOP, &r, c, SCREEN, false);
        assert!(a.full);
        assert!(dist(a.pos(c, 0), a.pos(c, 1)) >= a.item + SUB_ITEM_GAP - 0.5);
        for j in 0..30 {
            assert_eq!(a.hit(c, a.pos(c, j)), Some(j));
        }
        // Right to left mirrors the order.
        let (rtl, _) = place_arc(5, TOP, &r, c, SCREEN, true);
        assert!(rtl.pos(c, 0).0 > rtl.pos(c, 4).0, "the first item on the right");
        let (ltr, _) = place_arc(5, TOP, &r, c, SCREEN, false);
        assert!(ltr.pos(c, 0).0 < ltr.pos(c, 4).0);
    }

    #[test]
    fn a_second_level_opens_outside_the_first_and_stays_on_screen() {
        let r = ring(10, false);
        let c = (960.0, 540.0);
        let (shelves, at) = place_arc(8, r.angle(1), &r, c, SCREEN, false);
        let (widgets, at2) = place_next(6, &shelves, 2, at, SCREEN, false);
        assert_eq!(at2, at, "room enough in the open");
        assert!(widgets.radius - widgets.item / 2.0 >= shelves.reach() + SUB_GAP - 0.01, "outside the shelves");
        assert!((widgets.mid - shelves.angle(2)).abs() < 1e-4, "centred on its parent");
        for j in 0..6 {
            assert_eq!(widgets.hit(at2, widgets.pos(at2, j)), Some(j));
            assert_eq!(shelves.hit(at2, widgets.pos(at2, j)), None, "the levels don't overlap");
        }
        for j in 0..8 {
            assert_eq!(widgets.hit(at2, shelves.pos(at2, j)), None);
        }
        // Near every edge and corner, every shelf and every widget fit.
        for at in [(10.0, 10.0), (1910.0, 10.0), (10.0, 1070.0), (1910.0, 1070.0), (960.0, 5.0), (5.0, 540.0), (1915.0, 540.0), (960.0, 1075.0)] {
            let main = fit_center(at, (r.outer, r.outer), SCREEN);
            for parent in [0, 1, 4, 9] {
                let (shelves, c1) = place_arc(9, r.angle(parent), &r, main, SCREEN, false);
                for shelf in 0..9 {
                    for n in [2, 7, 12] {
                        let (widgets, c2) = place_next(n, &shelves, shelf, c1, SCREEN, false);
                        assert!(widgets.fits(c2, SCREEN), "{at:?} {parent}/{shelf}: {n} widgets");
                        assert!(shelves.fits(c2, SCREEN), "{at:?}: the shelves still fit around the moved centre");
                        assert!(c2.0 - r.outer >= EDGE - 0.01 && c2.1 - r.outer >= EDGE - 0.01);
                    }
                }
            }
        }
        // Right to left mirrors this level too.
        let (rtl, _) = place_next(5, &shelves, 2, at, SCREEN, true);
        let (ltr, _) = place_next(5, &shelves, 2, at, SCREEN, false);
        assert!((rtl.angle(0) - ltr.angle(4)).abs() < 1e-4);
    }

    #[test]
    fn keys_turn_the_ring_in_reading_order() {
        assert_eq!(nav_for("right", false, false), Some(Nav::Next));
        assert_eq!(nav_for("right", false, true), Some(Nav::Prev), "clockwise is backwards in right-to-left");
        assert_eq!(nav_for("left", false, true), Some(Nav::Next));
        assert_eq!(nav_for("tab", false, true), Some(Nav::Next));
        assert_eq!(nav_for("tab", true, false), Some(Nav::Prev));
        assert_eq!(nav_for("down", false, true), Some(Nav::Next));
        assert_eq!(nav_for("escape", false, false), Some(Nav::Close));
        assert_eq!(nav_for("enter", false, false), Some(Nav::Choose));
        assert_eq!(nav_for("a", false, false), None);
        let all = |_: usize| true;
        assert_eq!(step(None, 5, Nav::Next, all), Some(0), "the first key lands on the first item");
        assert_eq!(step(None, 5, Nav::Prev, all), Some(4));
        assert_eq!(step(Some(4), 5, Nav::Next, all), Some(0), "it wraps round");
        assert_eq!(step(Some(0), 5, Nav::Prev, all), Some(4));
        assert_eq!(step(Some(2), 5, Nav::First, all), Some(0));
        assert_eq!(step(Some(2), 5, Nav::Last, all), Some(4));
        // Disabled items are skipped, in both directions.
        let skip = |i: usize| i != 1 && i != 2;
        assert_eq!(step(Some(0), 5, Nav::Next, skip), Some(3));
        assert_eq!(step(Some(3), 5, Nav::Prev, skip), Some(0));
        assert_eq!(step(None, 5, Nav::First, |i| i >= 3), Some(3));
        assert_eq!(step(Some(0), 3, Nav::Next, |_| false), None);
        assert_eq!(step(Some(1), 3, Nav::Choose, all), Some(1));
        assert_eq!(step(None, 0, Nav::Next, all), None);
        // Walking with Next visits every item once, in order.
        let order: Vec<usize> = std::iter::successors(step(None, 6, Nav::Next, all), |&c| step(Some(c), 6, Nav::Next, all).filter(|&n| n != 0)).collect();
        assert_eq!(order, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn labels_grow_away_from_the_ring() {
        let c = (500.0, 500.0);
        let (p, side) = label_anchor(c, 130.0, 10.0, 0.0);
        assert!(close(p.0, 640.0) && side == (1, 0), "right of the ring, growing right");
        assert_eq!(label_anchor(c, 130.0, 10.0, PI).1, (-1, 0));
        assert_eq!(label_anchor(c, 130.0, 10.0, TOP).1, (0, -1));
        assert_eq!(label_anchor(c, 130.0, 10.0, PI / 2.0).1, (0, 1));
    }

    #[test]
    fn angles_wrap_into_one_turn() {
        for a in [-7.0, -PI, -1.0, 0.0, 1.0, PI, 4.0, 13.0] {
            let w = wrap(a);
            assert!(w > -PI - 1e-5 && w <= PI + 1e-5, "{a} → {w}");
            assert!(close(w.sin(), a.sin()) && close(w.cos(), a.cos()));
        }
    }
}
