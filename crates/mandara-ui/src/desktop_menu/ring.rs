//! The ring menu: the desktop menu's items as icon buttons on a ring of
//! chromatic glass around the right-click.
//!
//! * The hub in the middle names the item under the pointer (with its
//!   keybind) and closes the menu when clicked; it is the dead zone of a
//!   drag. A glowing arc follows the pointer's angle on a spring, tinted
//!   toward the accent in OKLCH, and the icons it passes swell and take
//!   the accent with it.
//! * "Add widget" opens an outer ring of glass pearls anchored to its
//!   sector, turned (or the whole menu moved) so that every pearl stays on
//!   screen.
//! * Hold the right button, flick toward an item and let go to pick it
//!   (the wallpaper canvas reports the held drag: the press belongs to it,
//!   [`super::drag_moved`]); a plain right-click leaves the ring open.
//!   Arrows, Tab, Home / End turn the selection, Enter picks, Esc closes.
//! * It blooms out of the click: the glass springs open, the items fly out
//!   one after another; closing is a quick scale-and-fade collapse. Studio
//!   → Motion's speed and feel apply (or the menu's own speed); reduced
//!   motion leaves only a short fade.
//!
//! The geometry is computed when the menu opens (or the outer ring
//! opens, or the output's size changes) by [`super::ring_geom`]; frames
//! only read it. The glass, the arc and the pearls are painted on one
//! canvas with SDF quads and shadows, so the edges stay smooth at any
//! scale. The surface covers the output for as long as the menu shows
//! (a click outside closes it, as everywhere in the shell) and gives the
//! pointer back the moment it starts closing.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Background, BorderStyle, BoxShadow, Bounds, Context, Corners, Div, Edges, FocusHandle, FontWeight, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, ParentElement, Pixels, Point, Render, SharedString, Styled, StyledImage as _, Window, canvas, div, fill, img, point, prelude::FluentBuilder as _, px, quad, size, svg, transparent_black,
};
use mandara_config::desktop_menu::{DesktopMenuConfig, MenuGesture, MenuLabels, MenuSubmenu};
use mandara_core::spring::{MotionScope, motion_profile, time_scale};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_theme::{Color, MandaraPalette};

use super::ring_geom::{self as geom, Arc, Hit, Nav, Ring};
use super::{Action, Entry};
use crate::kit::text;
use crate::plugins::panel_fx::{ANGLE_DOWN_DEG, elevation, elevation_scaled, readable, rim};

// ── Motion ───────────────────────────────────────────────────────────────

/// The glass opening (a little overshoot) and closing (none).
const OPEN_SPRING: SpringConfig = SpringConfig::new(420.0, 30.0, 1.0);
const CLOSE_SPRING: SpringConfig = SpringConfig::new(640.0, 52.0, 1.0);
/// Each item's flight out of the click, solved in closed form from its
/// own start time.
const ITEM_SPRING: SpringConfig = SpringConfig::new(460.0, 27.0, 1.0);
/// The highlight arc chasing the pointer, and the menu's centre moving to
/// make room for an outer ring.
const ARC_SPRING: SpringConfig = SpringConfig::new(540.0, 40.0, 1.0);
const CENTER_SPRING: SpringConfig = SpringConfig::new(320.0, 34.0, 1.0);
/// The outer ring appearing / folding back.
const OUTER_SPRING: SpringConfig = SpringConfig::new(520.0, 36.0, 1.0);
/// Delay between neighbouring items' flights, seconds.
const STAGGER_S: f32 = 0.016;
const OUTER_STAGGER_S: f32 = 0.011;
/// An item's flight has settled this long after it started.
const SETTLE_S: f32 = 0.75;
/// Reduced motion: the fade's length, seconds.
const FADE_S: f32 = 0.12;
/// Below this openness a closing menu is gone; it is removed after the
/// deadline even if no frame arrives.
const CLOSED_EPSILON: f32 = 0.02;
const CLOSE_DEADLINE: Duration = Duration::from_millis(700);
/// The glass grows from this scale, the items fly from this share of the
/// radius.
const PLATE_FROM: f32 = 0.7;
const ITEM_FROM: f32 = 0.28;
/// While the glass closes it shrinks to this scale.
const PLATE_CLOSED: f32 = 0.86;

// ── Look ─────────────────────────────────────────────────────────────────

/// Icon size against the button.
const GLYPH_RATIO: f32 = 0.45;
/// Icons the arc passes swell by this much.
const GLYPH_SWELL: f32 = 0.16;
/// OKLCH share of the accent in the arc's fill.
const ARC_TINT: f32 = 0.24;
/// The edge arc's span against its item's sector, and how much larger
/// than the button the lit disc is.
const ARC_SPAN: f32 = 0.86;
const ARC_PAD: f32 = 2.0;
/// Spacing of the discs the arc is drawn with, against their radius
/// (overlapping SDF circles make one smooth band with round ends: at a
/// third of the radius the scallops stay far below a pixel), its bounds,
/// and the most discs.
const BEAD_STEP: f32 = 0.3;
const BEAD_STEP_PX: (f32, f32) = (0.35, 2.0);
const MAX_BEADS: usize = 320;
/// The bright strip along the glass's edge under the arc: inset, width,
/// and its glow.
const STRIP_INSET: f32 = 3.5;
const STRIP_W: f32 = 3.0;
const STRIP_GLOW_BLUR: f32 = 16.0;
const STRIP_GLOW_ALPHA: f32 = 0.55;
/// Each item's socket (on-surface alpha, `(dark, light)`).
const SOCKET_ALPHA: (f32, f32) = (0.045, 0.06);
/// The glow under the arc: blur and alpha `(dark, light)`.
const GLOW_BLUR: f32 = 28.0;
const GLOW_ALPHA: (f32, f32) = (0.40, 0.26);
/// The glass: its accent tint (OKLCH), sheen lift / drop and the hub's
/// lift over it.
const GLASS_TINT: f32 = 0.05;
const SHEEN: (f32, f32) = (0.026, -0.008);
const HUB_LIFT: f32 = 0.03;
/// The groove between the band and the hub.
const GROOVE_ALPHA: f32 = 0.07;
/// The hub's tint toward "close" under the pointer.
const HUB_CLOSE_TINT: f32 = 0.14;
/// Text in the hub: its box against the hub, and the hint's dimming.
const HUB_TEXT: f32 = 0.84;
/// The hub's label: line height and gap to its keybind.
const HUB_LINE_H: f32 = 16.0;
const HUB_TEXT_GAP: f32 = 4.0;
const DISABLED_ALPHA: f32 = 0.42;
/// The close glyph in the hub.
const CLOSE_GLYPH: f32 = 16.0;
/// A label beside the ring: gap, the box it sits in, height and padding.
const LABEL_GAP: f32 = 10.0;
const LABEL_BOX_W: f32 = 260.0;
const LABEL_H: f32 = 28.0;
const LABEL_PAD: f32 = 12.0;
/// Captions under the icons: the icon moves up by this.
const CAPTION_LIFT: f32 = 8.0;
/// How lit the outer ring's parent stays.
const PARENT_LIT: f32 = 0.6;
/// A pearl's shadow strength.
const PEARL_SHADOW: f32 = 0.55;

// ── Items ────────────────────────────────────────────────────────────────

/// A menu row as a ring item, its icon path made once.
#[derive(Clone)]
pub struct Item {
    pub entry: Entry,
    path: SharedString,
}

impl Item {
    pub fn new(entry: Entry) -> Self {
        let path = SharedString::from(format!("icons/{}.svg", entry.icon));
        Self { entry, path }
    }
}

/// What is selected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    None,
    /// The hub: "close".
    Hub,
    Main(usize),
    Outer(usize),
}

/// The outer ring ("Add widget").
#[derive(Clone)]
struct Outer {
    arc: Arc,
    /// The menu's centre while it shows (moved inward near an edge).
    center: (f32, f32),
    parent: usize,
    /// Seconds of its bloom.
    t: f32,
    presence: SpringValue<f32>,
    shown: bool,
}

/// A right button held down: where it was pressed, whether that was on an
/// item (letting go there picks it) and whether the press was on this
/// surface (GPUI then sees the release too) or on the wallpaper canvas.
#[derive(Copy, Clone, Debug)]
struct Held {
    press: (f32, f32),
    on_item: bool,
    ours: bool,
}

/// What the menu opens with.
pub struct Opening {
    pub output: String,
    pub at: (f32, f32),
    pub size: (f32, f32),
    pub entries: Vec<Entry>,
    pub menu: DesktopMenuConfig,
    /// Studio → Motion's playback speed for popups.
    pub follow_speed: f32,
    /// The right button that opened it is still held.
    pub held: bool,
}

// ── One frame ────────────────────────────────────────────────────────────

/// Everything one frame draws, by value (the canvas keeps a copy).
#[derive(Clone, Copy)]
pub struct Scene {
    pub ring: Ring,
    /// Centre, glass openness (may overshoot) and overall alpha.
    pub center: (f32, f32),
    pub p: f32,
    pub alpha: f32,
    /// Bloom time, seconds (`f32::INFINITY` = settled).
    pub t: f32,
    pub closing: bool,
    pub sel: Sel,
    /// The arc: angle, radius, span, thickness and presence.
    pub arc: (f32, f32, f32, f32, f32),
    pub outer: Option<OuterScene>,
    pub labels: MenuLabels,
    pub opacity: f32,
}

#[derive(Clone, Copy)]
pub struct OuterScene {
    pub arc: Arc,
    pub parent: usize,
    pub parent_angle: f32,
    pub t: f32,
    pub presence: f32,
}

impl Scene {
    /// Item `i`'s flight: 0 at the click, 1 on the ring (overshoots).
    fn flight(&self, i: usize) -> f32 {
        let q = flight(self.t - i as f32 * STAGGER_S);
        if self.closing { q.min(self.p) } else { q }
    }

    fn scale(&self) -> f32 {
        if self.closing { PLATE_CLOSED + (1.0 - PLATE_CLOSED) * self.p } else { PLATE_FROM + (1.0 - PLATE_FROM) * self.p }
    }

    /// Item `i`'s centre and how far it has come.
    fn item(&self, i: usize) -> ((f32, f32), f32) {
        let q = self.flight(i);
        let r = self.ring.radius * (ITEM_FROM + (1.0 - ITEM_FROM) * q);
        (geom::polar(self.center, r, self.ring.angle(i)), q)
    }

    /// Outer item `j`'s centre, its size share and alpha.
    fn pearl(&self, o: &OuterScene, j: usize) -> ((f32, f32), f32) {
        let q = flight(o.t - j as f32 * OUTER_STAGGER_S).min(o.presence.max(0.0) * 1.2);
        let target = o.arc.angle(j);
        let a = o.parent_angle + geom::wrap(target - o.parent_angle) * q;
        let r = self.ring.radius + (o.arc.radius - self.ring.radius) * q;
        (geom::polar(self.center, r, a), q)
    }

    /// How close the arc is to angle `a` with a sector of `step` (1 on it).
    fn near(&self, a: f32, step: f32) -> f32 {
        let (angle, _, _, _, on) = self.arc;
        if step == 0.0 {
            return 0.0;
        }
        ((1.0 - geom::wrap(angle - a).abs() / step.abs()).max(0.0) * on).min(1.0)
    }

    /// How far an icon sits above its item's centre (to make room for its
    /// caption).
    fn lift(&self) -> f32 {
        if self.labels == MenuLabels::Always { CAPTION_LIFT } else { 0.0 }
    }

    /// How much the arc is on the main ring (1) rather than the outer one
    /// (0): it slides between them.
    fn on_main(&self) -> f32 {
        let Some(o) = &self.outer else { return 1.0 };
        let gap = (o.arc.radius - self.ring.radius).max(1.0);
        (1.0 - (self.arc.1 - self.ring.radius).abs() / gap).clamp(0.0, 1.0)
    }
}

/// The closed-form position of a unit spring released at `t = 0` (0
/// before it), under the shell's spring feel.
fn flight(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if !t.is_finite() {
        return 1.0;
    }
    1.0 + motion_profile().tune(ITEM_SPRING).solve(-1.0, 0.0, t).0
}

/// The glass's shadows, made again only when the theme changes.
struct Shade {
    key: (bool, Color, Color),
    /// Drop shadows of the glass and the pearls, and the rim (inset).
    plate: Vec<BoxShadow>,
    pearl: Vec<BoxShadow>,
    rim: Vec<BoxShadow>,
}

impl Shade {
    fn for_palette(palette: &MandaraPalette) -> Self {
        Self { key: Self::key(palette), plate: elevation(palette, 1.0), pearl: elevation_scaled(palette, 1.0, PEARL_SHADOW), rim: rim(palette) }
    }

    fn key(palette: &MandaraPalette) -> (bool, Color, Color) {
        (palette.is_dark, palette.surface_base, palette.specular_rim_light)
    }
}

/// The ring's colours for a palette.
#[derive(Clone, Copy)]
struct Ink {
    glass: Color,
    accent: Color,
    arc: Color,
    glow: Color,
    hub: Color,
    groove: Color,
    socket: Color,
    close: Color,
    fg: Color,
    dim: Color,
}

impl Ink {
    fn new(palette: &MandaraPalette, opacity: f32) -> Self {
        // Chromatic glass: the frame colour with a breath of the accent.
        let frame = crate::shell::frame_color(palette).mix(palette.primary, GLASS_TINT);
        let accent = readable(palette.primary, palette);
        let glow = if palette.is_dark { GLOW_ALPHA.0 } else { GLOW_ALPHA.1 };
        Self {
            glass: frame.with_alpha(opacity),
            accent,
            arc: frame.mix(accent, ARC_TINT).with_alpha(1.0),
            glow: accent.with_alpha(glow),
            hub: frame.lighten(HUB_LIFT).with_alpha(opacity.max(0.9)),
            groove: palette.on_surface.with_alpha(GROOVE_ALPHA),
            socket: palette.on_surface.with_alpha(if palette.is_dark { SOCKET_ALPHA.0 } else { SOCKET_ALPHA.1 }),
            close: palette.urgent_coral,
            fg: palette.on_surface,
            dim: palette.on_surface_variant,
        }
    }
}

fn disc_bounds(c: (f32, f32), r: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(c.0 - r), px(c.1 - r)), size: size(px(2.0 * r), px(2.0 * r)) }
}

/// Paints `shadows` (drop or inset ones) of a disc with their alpha
/// scaled by `a`, one at a time (no list is built).
fn shadows(window: &mut Window, b: Bounds<Pixels>, r: f32, list: &[BoxShadow], a: f32, inset: bool) {
    for s in list.iter().filter(|s| s.inset == inset) {
        let mut s = s.clone();
        s.color = s.color.opacity(a);
        let one = std::slice::from_ref(&s);
        if inset {
            window.paint_inset_shadows(b, Corners::all(px(r)), one);
        } else {
            window.paint_drop_shadows(b, Corners::all(px(r)), one);
        }
    }
}

/// A glass disc: elevation, the sheen, the rim.
fn glass_disc(window: &mut Window, c: (f32, f32), r: f32, base: Color, drop: &[BoxShadow], rim: &[BoxShadow], a: f32) {
    if r <= 0.0 || a <= 0.0 {
        return;
    }
    let b = disc_bounds(c, r);
    shadows(window, b, r, drop, a, false);
    let (lift, sink) = SHEEN;
    let top = base.lighten(lift);
    let bottom = base.lighten(sink);
    let bg: Background = mandara_core::oklab_gradient(ANGLE_DOWN_DEG, top.with_alpha(top.alpha() * a), bottom.with_alpha(bottom.alpha() * a));
    window.paint_quad(fill(b, bg).corner_radii(px(r)));
    shadows(window, b, r, rim, a, true);
}

/// A band of `thick` along a circle of `radius` on `c`, from `a0` over
/// `span` radians, drawn as overlapping discs in `color`.
fn band(window: &mut Window, c: (f32, f32), radius: f32, a0: f32, span: f32, thick: f32, color: Color) {
    let r = thick / 2.0;
    if r <= 0.0 {
        return;
    }
    let len = (radius * span).abs();
    let step = (r * BEAD_STEP).clamp(BEAD_STEP_PX.0, BEAD_STEP_PX.1);
    let n = ((len / step).ceil() as usize + 1).clamp(1, MAX_BEADS);
    let hsla = color.hsla();
    for k in 0..n {
        let f = if n == 1 { 0.5 } else { k as f32 / (n - 1) as f32 };
        let p = geom::polar(c, radius, a0 + span * f);
        window.paint_quad(fill(disc_bounds(p, r), hsla).corner_radii(px(r)));
    }
}

/// Paints the glass, the arc, the hub and the pearls of `s` (in surface
/// coordinates offset by `origin`).
fn paint(s: &Scene, ink: &Ink, shade: &Shade, origin: Point<Pixels>, window: &mut Window) {
    let o = (f32::from(origin.x), f32::from(origin.y));
    let c = (s.center.0 + o.0, s.center.1 + o.1);
    let scale = s.scale();
    let a = s.alpha * (s.p * 1.6).clamp(0.0, 1.0);
    let ring = &s.ring;
    // 1 · the glass.
    let outer = ring.outer * scale;
    glass_disc(window, c, outer, ink.glass, &shade.plate, &shade.rim, a);
    // 2 · the groove round the hub.
    let groove = ring.inner * scale;
    if groove > 1.0 {
        let b = disc_bounds(c, groove);
        window.paint_quad(quad(b, Corners::all(px(groove)), transparent_black(), Edges::all(px(1.0)), ink.groove.with_alpha(ink.groove.alpha() * a).hsla(), BorderStyle::Solid));
    }
    // 3 · each item's button, a faint socket the light falls into (with
    // captions, the icon sits above its name).
    let lift = s.lift();
    for i in 0..ring.n {
        let (p, q) = s.item(i);
        let p = (p.0, p.1 - lift);
        let r = ring.item / 2.0 * (0.6 + 0.4 * q.min(1.1)) * scale;
        let qa = a * (q * 1.8).clamp(0.0, 1.0);
        if qa > 0.0 {
            window.paint_quad(fill(disc_bounds((p.0 + o.0, p.1 + o.1), r), ink.socket.with_alpha(ink.socket.alpha() * qa).hsla()).corner_radii(px(r)));
        }
    }
    // 4 · the outer ring's pearls.
    if let Some(os) = &s.outer {
        for j in 0..os.arc.n {
            let (p, q) = s.pearl(os, j);
            let pa = s.alpha * q.clamp(0.0, 1.0);
            let pr = os.arc.item / 2.0 * (0.5 + 0.5 * q.min(1.1));
            glass_disc(window, (p.0 + o.0, p.1 + o.1), pr, ink.glass, &shade.pearl, &shade.rim, pa);
        }
    }
    // 5 · the selection, following the pointer's angle on springs: a lit
    // button under the icon with its glow and, on the main ring, a
    // glowing arc along the glass's edge over the item's sector.
    let (angle, radius, span, thick, on) = s.arc;
    if on > 0.01 && a > 0.0 {
        let on = on.min(1.0);
        let main = s.on_main();
        let mid = geom::polar(c, radius * scale, angle);
        let mid = (mid.0, mid.1 - lift * main);
        let r = thick / 2.0 * (0.72 + 0.28 * on) * scale;
        let glow = BoxShadow { color: ink.glow.with_alpha(ink.glow.alpha() * on * a).hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(GLOW_BLUR), spread_radius: px(0.0), inset: false };
        let gb = disc_bounds(mid, r);
        window.paint_drop_shadows(gb, Corners::all(px(r)), std::slice::from_ref(&glow));
        window.paint_quad(fill(gb, ink.arc.with_alpha(a.min(1.0)).hsla()).corner_radii(px(r)));
        if main > 0.01 {
            let span = span * (0.4 + 0.6 * on) * main;
            let strip = outer - STRIP_INSET;
            let sm = geom::polar(c, strip, angle);
            let sg = BoxShadow { color: ink.accent.with_alpha(STRIP_GLOW_ALPHA * on * main * a).hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(STRIP_GLOW_BLUR), spread_radius: px(0.0), inset: false };
            window.paint_drop_shadows(disc_bounds(sm, STRIP_W), Corners::all(px(STRIP_W)), std::slice::from_ref(&sg));
            band(window, c, strip, angle - span / 2.0, span, STRIP_W, ink.accent.with_alpha(a.min(1.0)));
        }
    }
    // 6 · the hub.
    let hub = ring.hub * scale;
    let base = if s.sel == Sel::Hub { ink.hub.mix(ink.close, HUB_CLOSE_TINT) } else { ink.hub };
    glass_disc(window, c, hub, base, &[], &shade.rim, a);
}

/// The whole ring for `s` as positioned children of a box whose top-left
/// is the scene's origin.
fn draw(s: Scene, items: &[Item], outer_items: &[Item], palette: &MandaraPalette, shade: Rc<Shade>, ui_font: SharedString) -> Div {
    let ink = Ink::new(palette, s.opacity);
    let scale = s.scale();
    let a = s.alpha * (s.p * 1.6).clamp(0.0, 1.0);
    let paint_ink = ink;
    let layer = canvas(|_, _, _| {}, move |bounds, (), window, _| paint(&s, &paint_ink, &shade, bounds.origin, window)).absolute().top_0().left_0().size_full();
    let mut root = div().absolute().top_0().left_0().size_full().font_family(ui_font).child(layer);

    // Anti-banding grain over the glass.
    let outer_r = s.ring.outer * scale;
    root = root.child(
        img(ImageSource::Render(mandara_core::material::grain_image()))
            .absolute()
            .left(px(s.center.0 - outer_r))
            .top(px(s.center.1 - outer_r))
            .size(px(2.0 * outer_r))
            .rounded_full()
            .object_fit(ObjectFit::Fill)
            .opacity(a),
    );

    // The icons (and captions).
    let glyph = s.ring.item * GLYPH_RATIO;
    let captions = s.labels == MenuLabels::Always;
    for (i, it) in items.iter().enumerate() {
        let ((x, y), q) = s.item(i);
        let k = s.near(s.ring.angle(i), s.ring.step) * s.on_main();
        // The parent of the outer ring keeps a little light while it shows.
        let k = match &s.outer {
            Some(o) if o.parent == i => k.max(PARENT_LIT * o.presence.clamp(0.0, 1.0)),
            _ => k,
        };
        let lit = k > 0.0 && it.entry.enabled;
        let tint = if it.entry.danger { palette.urgent_coral } else if lit { ink.dim.mix(ink.accent, k) } else { ink.dim };
        let g = glyph * (0.6 + 0.4 * q.min(1.1)) * (1.0 + GLYPH_SWELL * if lit { k } else { 0.0 });
        let ia = s.alpha * (q * 1.8).clamp(0.0, 1.0) * if it.entry.enabled { 1.0 } else { DISABLED_ALPHA };
        let lift = s.lift();
        root = root.child(svg().path(it.path.clone()).absolute().left(px(x - g / 2.0)).top(px(y - lift - g / 2.0)).size(px(g)).text_color(tint.hsla()).opacity(ia));
        if captions {
            root = root.child(
                div()
                    .absolute()
                    .left(px(x - geom::CAPTION_W / 2.0))
                    .top(px(y + glyph / 2.0 - lift + 3.0))
                    .w(px(geom::CAPTION_W))
                    .h(px(geom::CAPTION_H))
                    .text_size(px(text::CAPTION))
                    .text_center()
                    .truncate()
                    .text_color((if lit { ink.fg } else { ink.dim }).hsla())
                    .opacity(ia)
                    .child(crate::bidi::paragraph(it.entry.label.clone())),
            );
        }
    }
    if let Some(os) = &s.outer {
        let g = os.arc.item * GLYPH_RATIO;
        for (j, it) in outer_items.iter().enumerate().take(os.arc.n) {
            let ((x, y), q) = s.pearl(os, j);
            let k = s.near(os.arc.angle(j), os.arc.step) * (1.0 - s.on_main());
            let tint = ink.dim.mix(ink.accent, k);
            let gs = g * (0.6 + 0.4 * q.min(1.1)) * (1.0 + GLYPH_SWELL * k);
            root = root.child(svg().path(it.path.clone()).absolute().left(px(x - gs / 2.0)).top(px(y - gs / 2.0)).size(px(gs)).text_color(tint.hsla()).opacity(s.alpha * q.clamp(0.0, 1.0)));
        }
    }

    // The hub's words, or its close glyph.
    let focused = match s.sel {
        Sel::Main(i) => items.get(i),
        Sel::Outer(j) => outer_items.get(j),
        _ => None,
    };
    let hub = s.ring.hub * scale;
    let names_here = s.labels == MenuLabels::Hub;
    let content: AnyElement = match focused {
        Some(it) if names_here => {
            let e = &it.entry;
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(HUB_TEXT_GAP))
                .child(div().w_full().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).line_height(px(HUB_LINE_H)).text_center().line_clamp(2).text_color((if e.danger { palette.urgent_coral } else { ink.fg }).hsla()).child(crate::bidi::paragraph(e.label.clone())))
                .children(e.hint.as_ref().map(|h| super::hint_el(h, palette)))
                .into_any_element()
        }
        // Names around the ring: the hub keeps a keybind, else "close".
        Some(it) => match &it.entry.hint {
            Some(h @ super::Hint::Keys(_)) => super::hint_el(h, palette),
            _ => close_glyph(s.sel, &ink),
        },
        None => close_glyph(s.sel, &ink),
    };
    let box_w = 2.0 * hub * HUB_TEXT;
    root = root.child(div().absolute().left(px(s.center.0 - box_w / 2.0)).top(px(s.center.1 - box_w / 2.0)).size(px(box_w)).flex().items_center().justify_center().opacity(a).child(content));

    // A label beside the item under the pointer.
    if s.labels == MenuLabels::Around
        && let Some(it) = focused
    {
        let (theta, edge) = match (s.sel, &s.outer) {
            (Sel::Outer(j), Some(os)) => (os.arc.angle(j), os.arc.radius + os.arc.item / 2.0),
            (Sel::Main(i), _) => (s.ring.angle(i), s.ring.outer * scale),
            _ => (geom::TOP, s.ring.outer),
        };
        root = root.child(around_label(s.center, edge, theta, &it.entry, palette, &ink, a));
    }
    root
}

fn close_glyph(sel: Sel, ink: &Ink) -> AnyElement {
    let c = if sel == Sel::Hub { ink.close } else { ink.dim.with_alpha(0.7) };
    crate::icons::icon("close", CLOSE_GLYPH, c).into_any_element()
}

fn around_label(c: (f32, f32), edge: f32, theta: f32, e: &Entry, palette: &MandaraPalette, ink: &Ink, a: f32) -> AnyElement {
    let ((x, y), (hx, vy)) = geom::label_anchor(c, edge, LABEL_GAP, theta);
    let left = match hx {
        1 => x,
        -1 => x - LABEL_BOX_W,
        _ => x - LABEL_BOX_W / 2.0,
    };
    let top = match vy {
        1 => y,
        -1 => y - LABEL_H,
        _ => y - LABEL_H / 2.0,
    };
    let mut shadow = elevation(palette, a);
    shadow.extend(rim(palette));
    let pill = div()
        .h(px(LABEL_H))
        .px(px(LABEL_PAD))
        .rounded_full()
        .bg(ink.glass.with_alpha(ink.glass.alpha().max(0.92)).hsla())
        .shadow(shadow)
        .flex()
        .items_center()
        .text_size(px(text::BODY_S))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink.fg.hsla())
        .child(crate::bidi::paragraph(e.label.clone()));
    let row = div().absolute().left(px(left)).top(px(top)).w(px(LABEL_BOX_W)).h(px(LABEL_H)).flex().opacity(a);
    let row = match hx {
        1 => row.justify_start(),
        -1 => row.justify_end(),
        _ => row.justify_center(),
    };
    row.child(pill).into_any_element()
}

// ── The menu surface ─────────────────────────────────────────────────────

pub struct RingMenu {
    output: String,
    at: (f32, f32),
    size: (f32, f32),
    items: Vec<Item>,
    /// "Add widget"'s items, read the first time it opens.
    outer_items: Option<Vec<Item>>,
    ring: Ring,
    rtl: bool,
    /// Where the ring sits with no outer ring, and where it is now.
    home: (f32, f32),
    center: SpringValue<Point<Pixels>>,
    outer: Option<Outer>,
    sel: Sel,
    keyboard: bool,
    labels: MenuLabels,
    gesture: MenuGesture,
    submenu: MenuSubmenu,
    opacity: f32,
    speed: f32,
    /// A held right button (a drag in progress).
    held: Option<Held>,
    /// `[desktop.menu] radius` (the ring may have grown past it).
    radius_setting: f32,
    open: SpringValue<f32>,
    t: f32,
    fade: f32,
    closing: bool,
    released: bool,
    gone: bool,
    clock: FrameClock,
    arc_angle: SpringValue<f32>,
    arc_radius: SpringValue<f32>,
    arc_span: SpringValue<f32>,
    arc_thick: SpringValue<f32>,
    arc_on: SpringValue<f32>,
    shade: Option<Rc<Shade>>,
    focus: FocusHandle,
}

impl RingMenu {
    pub fn new(o: Opening, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let rtl = crate::bidi::rtl();
        let items: Vec<Item> = o.entries.into_iter().map(Item::new).collect();
        let m = &o.menu;
        let ring = Ring::new(items.len(), m.radius(), m.item_size(), m.labels == MenuLabels::Always, rtl).fit_screen(o.size);
        let home = geom::fit_center(o.at, (ring.outer, ring.outer), o.size);
        let speed = m.speed(mandara_config::motion::surface_speed(o.follow_speed));
        let held = (o.held && m.gesture != MenuGesture::Click).then_some(Held { press: o.at, on_item: false, ours: false });
        Self {
            output: o.output,
            at: o.at,
            size: o.size,
            items,
            outer_items: None,
            ring,
            rtl,
            home,
            center: SpringValue::new(point(px(home.0), px(home.1)), CENTER_SPRING),
            outer: None,
            sel: Sel::None,
            keyboard: false,
            labels: m.labels,
            gesture: m.gesture,
            submenu: m.submenu,
            opacity: m.opacity(),
            speed,
            held,
            radius_setting: m.radius(),
            open: SpringValue::new(0.0, OPEN_SPRING),
            t: 0.0,
            fade: 0.0,
            closing: false,
            released: false,
            gone: false,
            clock: FrameClock::new(),
            arc_angle: SpringValue::new(geom::TOP, ARC_SPRING),
            arc_radius: SpringValue::new(ring.radius, ARC_SPRING),
            arc_span: SpringValue::new(ring.step.abs() * ARC_SPAN, ARC_SPRING),
            arc_thick: SpringValue::new(ring.item + ARC_PAD, ARC_SPRING),
            arc_on: SpringValue::new(0.0, SpringConfig::SNAPPY),
            shade: None,
            focus,
        }
    }

    /// Lays the ring out again at `at` (a right-click elsewhere, a new
    /// output size) and blooms it there.
    fn relayout(&mut self, at: (f32, f32), size: (f32, f32), replay: bool) {
        self.at = at;
        self.size = size;
        self.ring = Ring::new(self.items.len(), self.radius_setting, self.ring.item, self.labels == MenuLabels::Always, self.rtl).fit_screen(size);
        self.home = geom::fit_center(at, (self.ring.outer, self.ring.outer), size);
        self.center.snap_to(point(px(self.home.0), px(self.home.1)));
        self.outer = None;
        self.sel = Sel::None;
        self.arc_on.snap_to(0.0);
        if replay {
            self.t = 0.0;
            self.open.snap_to(0.0);
            self.fade = 0.0;
            self.clock.idle();
        }
    }

    fn reduce() -> bool {
        motion_profile().reduce
    }

    /// Advances every spring one frame; returns whether anything moves.
    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let _speed = MotionScope::new(self.speed);
        let reduce = Self::reduce();
        let instant = reduce || self.speed == 0.0;
        let dt = self.clock.tick_vsync(Instant::now(), window.refresh_interval(cx));
        let anim = if instant { f32::INFINITY } else { dt * time_scale() * self.speed };
        self.t += anim;
        // Closing keeps the exit target `close` set (no swing back).
        if !self.closing {
            self.open.set_target(1.0);
        }
        let mut moving = self.open.step(dt);
        if reduce && self.speed > 0.0 {
            let target = if self.closing { 0.0 } else { 1.0 };
            let step = dt / FADE_S;
            self.fade = if target > self.fade { (self.fade + step).min(1.0) } else { (self.fade - step).max(0.0) };
            moving |= self.fade != target;
        } else {
            self.fade = if self.closing && instant { 0.0 } else { 1.0 };
        }
        let last = self.items.len().saturating_sub(1) as f32 * STAGGER_S;
        moving |= self.t.is_finite() && self.t < last + SETTLE_S;
        for s in [&mut self.arc_angle, &mut self.arc_radius, &mut self.arc_span, &mut self.arc_thick, &mut self.arc_on] {
            moving |= s.step(dt);
        }
        moving |= self.center.step(dt);
        if let Some(o) = self.outer.as_mut() {
            o.t += anim;
            moving |= o.presence.step(dt);
            let last = o.arc.n.saturating_sub(1) as f32 * OUTER_STAGGER_S;
            moving |= o.shown && o.t.is_finite() && o.t < last + SETTLE_S;
            if !o.shown && o.presence.value() <= CLOSED_EPSILON {
                self.outer = None;
            }
        }
        if self.closing && !self.released {
            self.released = true;
            // Clicks fall through at once while the ring folds away.
            window.set_input_region(Some(&[]));
            let handle = window.window_handle();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(CLOSE_DEADLINE).await;
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                cx.update(|cx| super::closed(handle, cx));
            })
            .detach();
        }
        let shown = if reduce && self.speed > 0.0 { self.fade } else { self.open.value() };
        if self.closing && shown <= CLOSED_EPSILON {
            self.gone = true;
            window.remove_window();
            let handle = window.window_handle();
            cx.defer(move |cx| super::closed(handle, cx));
            return false;
        }
        moving
    }

    fn scene(&self) -> Scene {
        let reduce = Self::reduce() && self.speed > 0.0;
        let c = self.center.value();
        let outer = self.outer.as_ref().map(|o| OuterScene { arc: o.arc, parent: o.parent, parent_angle: self.ring.angle(o.parent), t: if reduce { f32::INFINITY } else { o.t }, presence: o.presence.value() });
        Scene {
            ring: self.ring,
            center: (f32::from(c.x), f32::from(c.y)),
            p: if reduce { 1.0 } else { self.open.value() },
            alpha: if reduce { self.fade } else { 1.0 },
            t: if reduce { f32::INFINITY } else { self.t },
            closing: self.closing && !reduce,
            sel: self.sel,
            arc: (self.arc_angle.value(), self.arc_radius.value(), self.arc_span.value(), self.arc_thick.value(), self.arc_on.value()),
            outer,
            labels: self.labels,
            opacity: self.opacity,
        }
    }

    // ── Selection ────────────────────────────────────────────────────────

    /// Where hits are tested: the centre the ring is settling on.
    fn hit_center(&self) -> (f32, f32) {
        self.outer.as_ref().filter(|o| o.shown).map_or(self.home, |o| o.center)
    }

    fn add_index(&self) -> Option<usize> {
        self.items.iter().position(|it| it.entry.action == Action::AddMenu)
    }

    fn select(&mut self, sel: Sel, cx: &mut Context<Self>) {
        if sel == self.sel {
            return;
        }
        let was_on = matches!(self.sel, Sel::Main(_) | Sel::Outer(_));
        self.sel = sel;
        let target = match sel {
            Sel::Main(i) => Some((self.ring.angle(i), self.ring.radius, self.ring.step.abs() * ARC_SPAN, self.ring.item + ARC_PAD)),
            Sel::Outer(j) => self.outer.as_ref().map(|o| (o.arc.angle(j), o.arc.radius, o.arc.step.abs() * ARC_SPAN, o.arc.item + ARC_PAD)),
            _ => None,
        };
        match target {
            Some((angle, radius, span, thick)) => {
                // Turn the short way round; appear where it is.
                let cur = self.arc_angle.value();
                let angle = cur + geom::wrap(angle - cur);
                if was_on {
                    self.arc_angle.set_target(angle);
                    self.arc_radius.set_target(radius);
                    self.arc_span.set_target(span);
                    self.arc_thick.set_target(thick);
                } else {
                    self.arc_angle.snap_to(angle);
                    self.arc_radius.snap_to(radius);
                    self.arc_span.snap_to(span);
                    self.arc_thick.snap_to(thick);
                }
                self.arc_on.set_target(1.0);
            }
            None => {
                self.arc_on.set_target(0.0);
            }
        }
        cx.notify();
    }

    /// Opens (or keeps) the outer ring of item `parent`.
    fn open_outer(&mut self, parent: usize, cx: &mut Context<Self>) {
        if self.outer.as_ref().is_some_and(|o| o.shown && o.parent == parent) {
            return;
        }
        if self.outer_items.is_none() {
            self.outer_items = Some(super::add_entries(cx).into_iter().map(Item::new).collect());
        }
        let n = self.outer_items.as_ref().map_or(0, Vec::len);
        if n == 0 {
            return;
        }
        let (arc, center) = geom::place_arc(n, self.ring.angle(parent), &self.ring, self.home, self.size, self.rtl);
        let mut presence = SpringValue::new(0.0, OUTER_SPRING);
        presence.set_target(1.0);
        self.outer = Some(Outer { arc, center, parent, t: 0.0, presence, shown: true });
        self.center.set_target(point(px(center.0), px(center.1)));
        cx.notify();
    }

    fn close_outer(&mut self, cx: &mut Context<Self>) {
        if let Some(o) = self.outer.as_mut().filter(|o| o.shown) {
            o.shown = false;
            o.presence.set_exit_target(0.0);
            self.center.set_target(point(px(self.home.0), px(self.home.1)));
            if matches!(self.sel, Sel::Outer(_)) {
                let parent = o.parent;
                self.select(Sel::Main(parent), cx);
            }
            cx.notify();
        }
    }

    /// The selection for a pointer resting at `p` (no button held).
    fn hover_target(&self, p: (f32, f32)) -> Sel {
        let c = self.hit_center();
        if let Some(o) = self.outer.as_ref().filter(|o| o.shown)
            && let Some(j) = o.arc.hit(c, p)
        {
            return Sel::Outer(j);
        }
        match self.ring.hit(c, p) {
            Hit::Hub => Sel::Hub,
            Hit::Item(i) => Sel::Main(i),
            Hit::Outside => {
                // Travelling from the parent to its outer ring keeps both.
                let between = self.outer.as_ref().filter(|o| o.shown).is_some_and(|o| geom::dist(c, p) < o.arc.radius + o.arc.item);
                if between { self.sel } else { Sel::None }
            }
        }
    }

    /// The selection a held drag from `press` to `p` points at.
    fn drag_target(&self, press: (f32, f32), p: (f32, f32)) -> Sel {
        let c = self.hit_center();
        if let Some(o) = self.outer.as_ref().filter(|o| o.shown) {
            if let Some(j) = o.arc.hit(c, p) {
                return Sel::Outer(j);
            }
            // Past the main glass, toward the outer ring.
            if geom::dist(c, p) > self.ring.outer + geom::SUB_GAP / 2.0
                && let Some(j) = o.arc.by_angle(geom::angle(c, p))
            {
                return Sel::Outer(j);
            }
        }
        self.ring.drag(c, press, p).map_or(Sel::None, Sel::Main)
    }

    /// Follows a selection with the outer ring (opened on its parent in
    /// hover mode, closed on any other item).
    fn follow(&mut self, sel: Sel, dragging: bool, cx: &mut Context<Self>) {
        self.select(sel, cx);
        if let Sel::Main(i) = sel {
            let parent = Some(i) == self.add_index();
            if parent && (dragging || self.submenu == MenuSubmenu::Hover) {
                self.open_outer(i, cx);
            } else if !parent && (dragging || self.submenu == MenuSubmenu::Hover) {
                self.close_outer(cx);
            }
        }
    }

    fn pointer_moved(&mut self, p: (f32, f32), cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.keyboard = false;
        let sel = match self.held {
            Some(h) => self.drag_target(h.press, p),
            None => self.hover_target(p),
        };
        self.follow(sel, self.held.is_some(), cx);
    }

    /// The held button moved to `at` on `output`, or was let go there.
    pub fn drag_to(&mut self, output: &str, at: (f32, f32), released: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.held else { return };
        if self.closing {
            return;
        }
        let here = output == self.output && at.0.is_finite() && at.1.is_finite();
        // A press on an item and a release on that same item, without
        // moving away: a right-click on it.
        let still = h.on_item && geom::dist(h.press, at) < geom::DRAG_DEAD;
        let target = |this: &Self| if still { this.hover_target(at) } else { this.drag_target(h.press, at) };
        if here {
            self.keyboard = false;
            let sel = target(self);
            self.follow(sel, true, cx);
        }
        if !released {
            return;
        }
        self.held = None;
        let sel = if here { target(self) } else { Sel::None };
        match sel {
            Sel::Main(i) if Some(i) != self.add_index() => self.activate(i, window, cx),
            Sel::Outer(j) => self.choose_outer(j, window, cx),
            // Let go on "Add widget": its outer ring stays open to click.
            Sel::Main(_) => {}
            _ if self.gesture == MenuGesture::Drag => self.close(cx),
            // A plain right-click: the ring stays, for clicks.
            _ => {}
        }
    }

    fn activate(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.items.get(i).map(|it| &it.entry).filter(|e| e.enabled) else { return };
        if e.action == Action::AddMenu {
            if self.outer.as_ref().is_some_and(|o| o.shown) && !self.keyboard && self.submenu == MenuSubmenu::Click {
                self.close_outer(cx);
            } else {
                self.open_outer(i, cx);
                if self.keyboard {
                    let first = geom::step(None, self.outer_len(), Nav::First, |_| true);
                    if let Some(j) = first {
                        self.select(Sel::Outer(j), cx);
                    }
                }
            }
            return;
        }
        let action = e.action.clone();
        self.finish(action, window, cx);
    }

    fn outer_len(&self) -> usize {
        self.outer.as_ref().filter(|o| o.shown).map_or(0, |o| o.arc.n)
    }

    fn choose_outer(&mut self, j: usize, window: &mut Window, cx: &mut Context<Self>) {
        let action = self.outer_items.as_ref().and_then(|v| v.get(j)).map(|it| it.entry.action.clone());
        if let Some(action) = action {
            self.finish(action, window, cx);
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.closing {
            self.closing = true;
            self.open.set_config(CLOSE_SPRING);
            self.open.set_exit_target(0.0);
            self.arc_on.set_target(0.0);
            cx.notify();
        }
    }

    /// Closes the menu, then runs `action`.
    fn finish(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if action == Action::Screenshot {
            // No outro: the capture must not see the menu.
            self.gone = true;
            window.remove_window();
            let handle = window.window_handle();
            cx.defer(move |cx| super::closed(handle, cx));
        } else {
            self.close(cx);
        }
        let (output, at, size) = (self.output.clone(), self.at, self.size);
        cx.defer(move |cx| super::run(&action, &output, at, size, cx));
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let Some(nav) = geom::nav_for(&e.keystroke.key, e.keystroke.modifiers.shift, self.rtl) else { return };
        self.keyboard = true;
        let in_outer = matches!(self.sel, Sel::Outer(_)) && self.outer_len() > 0;
        match nav {
            Nav::Close | Nav::Back if in_outer || self.outer.as_ref().is_some_and(|o| o.shown) => self.close_outer(cx),
            Nav::Close => self.close(cx),
            Nav::Back => {}
            Nav::Choose => match self.sel {
                Sel::Main(i) => self.activate(i, window, cx),
                Sel::Outer(j) => self.choose_outer(j, window, cx),
                Sel::Hub => self.close(cx),
                Sel::None => {}
            },
            nav if in_outer => {
                let cur = match self.sel {
                    Sel::Outer(j) => Some(j),
                    _ => None,
                };
                if let Some(j) = geom::step(cur, self.outer_len(), nav, |_| true) {
                    self.select(Sel::Outer(j), cx);
                }
            }
            nav => {
                let cur = match self.sel {
                    Sel::Main(i) => Some(i),
                    _ => None,
                };
                let items = &self.items;
                if let Some(i) = geom::step(cur, items.len(), nav, |i| items[i].entry.enabled) {
                    self.follow(Sel::Main(i), false, cx);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn shade(&mut self, palette: &MandaraPalette) -> Rc<Shade> {
        match &self.shade {
            Some(s) if s.key == Shade::key(palette) => s.clone(),
            _ => {
                let s = Rc::new(Shade::for_palette(palette));
                self.shade = Some(s.clone());
                s
            }
        }
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let p = (f32::from(e.position.x), f32::from(e.position.y));
        self.keyboard = false;
        match e.button {
            MouseButton::Left => {
                self.held = None;
                match self.hover_target(p) {
                    Sel::Main(i) => self.activate(i, window, cx),
                    Sel::Outer(j) => self.choose_outer(j, window, cx),
                    _ => self.close(cx),
                }
            }
            MouseButton::Right => {
                if let Sel::Main(_) | Sel::Outer(_) = self.hover_target(p) {
                    // A right press on an item: picked when let go on it
                    // (or on another one it was dragged to).
                    self.held = Some(Held { press: p, on_item: true, ours: true });
                    return;
                }
                // In a split bar's gaps (`bar.right_click_gaps`) the bar's
                // main menu takes over from this one.
                if crate::bar::gaps::hit(&self.output, p, Some(self.size), cx).is_some() {
                    let (output, size) = (self.output.clone(), self.size);
                    cx.defer(move |cx| {
                        super::close(cx);
                        let _ = crate::bar::gaps::route(output, p, Some(size), cx);
                    });
                    return;
                }
                // Elsewhere: the ring moves there (and follows a drag).
                self.relayout(p, self.size, true);
                self.held = (self.gesture != MenuGesture::Click).then_some(Held { press: p, on_item: false, ours: true });
                cx.notify();
            }
            _ => self.close(cx),
        }
    }
}

impl Render for RingMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _frame = crate::plugins::diagnostics::frame(cx.entity_id().as_u64());
        let palette = mandara_core::frame_palette(window, cx);
        let vp = window.viewport_size();
        let screen = (f32::from(vp.width), f32::from(vp.height));
        if screen.0 >= 1.0 && screen.1 >= 1.0 && ((screen.0 - self.size.0).abs() > 0.5 || (screen.1 - self.size.1).abs() > 0.5) {
            let at = self.at;
            self.relayout(at, screen, false);
        }
        let moving = self.advance(window, cx);
        if self.gone {
            return div().into_any_element();
        }
        if moving {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let shade = self.shade(&palette);
        let font = crate::fonts::fonts(cx).ui.clone();
        let scene = self.scene();
        let outer_items: &[Item] = self.outer_items.as_deref().unwrap_or(&[]);
        let ring = draw(scene, &self.items, outer_items, &palette, shade, font);
        let hot = match self.sel {
            Sel::Main(i) => self.items.get(i).is_some_and(|it| it.entry.enabled),
            Sel::Outer(_) | Sel::Hub => true,
            Sel::None => false,
        };
        div()
            .id("desktop-ring-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .text_color(palette.on_surface.hsla())
            .when(hot, |el| el.cursor_pointer())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.key(e, window, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                let p = (f32::from(e.position.x), f32::from(e.position.y));
                match this.held {
                    // Our own press, and the button is up: the release
                    // went elsewhere.
                    Some(h) if h.ours && e.pressed_button != Some(MouseButton::Right) => {
                        this.held = None;
                        this.pointer_moved(p, cx);
                    }
                    Some(_) => {
                        let output = this.output.clone();
                        this.drag_to(&output, p, false, window, cx);
                    }
                    None => this.pointer_moved(p, cx),
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| this.mouse_down(e, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, window, cx| this.mouse_down(e, window, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, e: &MouseDownEvent, window, cx| this.mouse_down(e, window, cx)))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, e: &MouseUpEvent, window, cx| {
                let p = (f32::from(e.position.x), f32::from(e.position.y));
                let output = this.output.clone();
                this.drag_to(&output, p, true, window, cx);
            }))
            .child(ring)
            .into_any_element()
    }
}

// ── Studio's preview ─────────────────────────────────────────────────────

/// Room around a preview's ring, logical px.
const PREVIEW_PAD: f32 = 24.0;
/// Shortest preview box.
const PREVIEW_MIN_H: f32 = 340.0;
/// The outer ring of a replay starts this late.
const PREVIEW_OUTER_DELAY_S: f32 = 0.12;

/// A still (or replayed) ring for Studio's Context menu tab: the same
/// layout and painting as the real menu, in a box `width` wide and as
/// tall as the ring with its outer ring needs.
pub struct Preview {
    items: Vec<Item>,
    outer_items: Vec<Item>,
    ring: Ring,
    parent: Option<usize>,
    outer: Option<Arc>,
    center: (f32, f32),
    size: (f32, f32),
    labels: MenuLabels,
    opacity: f32,
    shade: Option<Rc<Shade>>,
}

impl Preview {
    /// `entries` on a ring as `menu` lays it out; "Add widget" (when in
    /// `entries`) shows `outer` round it while pointed at.
    pub fn new(menu: &DesktopMenuConfig, entries: Vec<Entry>, outer: Vec<Entry>, width: f32, rtl: bool) -> Self {
        let items: Vec<Item> = entries.into_iter().map(Item::new).collect();
        let outer_items: Vec<Item> = outer.into_iter().map(Item::new).collect();
        let ring = Ring::new(items.len(), menu.radius(), menu.item_size(), menu.labels == MenuLabels::Always, rtl);
        let parent = items.iter().position(|it| it.entry.action == Action::AddMenu).filter(|_| !outer_items.is_empty());
        let reach = parent.map_or(ring.outer, |i| {
            let a = Arc::new(outer_items.len(), &ring, ring.angle(i), rtl);
            a.radius + a.item / 2.0
        });
        let size = (width.max(2.0 * (reach + PREVIEW_PAD)), (2.0 * (reach + PREVIEW_PAD)).max(PREVIEW_MIN_H));
        let center = (size.0 / 2.0, size.1 / 2.0);
        let outer = parent.map(|i| geom::place_arc(outer_items.len(), ring.angle(i), &ring, center, size, rtl).0);
        Self { items, outer_items, ring, parent, outer, center, size, labels: menu.labels, opacity: menu.opacity(), shade: None }
    }

    /// The box's size.
    pub fn size(&self) -> (f32, f32) {
        self.size
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn outer_shown(&self, sel: Sel) -> bool {
        self.outer.is_some() && (matches!(sel, Sel::Outer(_)) || (matches!(sel, Sel::Main(i) if Some(i) == self.parent)))
    }

    /// What a pointer at `p` (box coordinates) is on, with `sel` lit now
    /// (the outer ring shows while its parent or one of its items is).
    pub fn hit(&self, p: (f32, f32), sel: Sel) -> Sel {
        if self.outer_shown(sel)
            && let Some(j) = self.outer.and_then(|a| a.hit(self.center, p))
        {
            return Sel::Outer(j);
        }
        match self.ring.hit(self.center, p) {
            Hit::Hub => Sel::Hub,
            Hit::Item(i) => Sel::Main(i),
            // Between the ring and its outer ring: keep what is lit.
            Hit::Outside if self.outer_shown(sel) && geom::dist(self.center, p) < self.outer.map_or(0.0, |a| a.radius + a.item) => sel,
            Hit::Outside => Sel::None,
        }
    }

    /// The ring at bloom time `t` (seconds; `f32::INFINITY` settled) with
    /// `sel` lit, positioned in a relative box of [`Self::size`].
    pub fn element(&mut self, sel: Sel, t: f32, palette: &MandaraPalette, cx: &App) -> AnyElement {
        let shade = match &self.shade {
            Some(s) if s.key == Shade::key(palette) => s.clone(),
            _ => {
                let s = Rc::new(Shade::for_palette(palette));
                self.shade = Some(s.clone());
                s
            }
        };
        let none = (geom::TOP, self.ring.radius, 0.0, 0.0, 0.0);
        let arc = match sel {
            Sel::Main(i) => (self.ring.angle(i), self.ring.radius, self.ring.step.abs() * ARC_SPAN, self.ring.item + ARC_PAD, 1.0),
            Sel::Outer(j) => self.outer.map_or(none, |a| (a.angle(j), a.radius, a.step.abs() * ARC_SPAN, a.item + ARC_PAD, 1.0)),
            _ => none,
        };
        let outer = self.outer.filter(|_| self.outer_shown(sel)).map(|a| OuterScene {
            arc: a,
            parent: self.parent.unwrap_or(0),
            parent_angle: self.ring.angle(self.parent.unwrap_or(0)),
            t: if t.is_finite() { (t - PREVIEW_OUTER_DELAY_S).max(0.0) } else { t },
            presence: 1.0,
        });
        let p = if t.is_finite() { 1.0 + motion_profile().tune(OPEN_SPRING).solve(-1.0, 0.0, t.max(0.0)).0 } else { 1.0 };
        let scene = Scene { ring: self.ring, center: self.center, p, alpha: 1.0, t, closing: false, sel, arc, outer, labels: self.labels, opacity: self.opacity };
        let font = crate::fonts::fonts(cx).ui.clone();
        draw(scene, &self.items, &self.outer_items, palette, shade, font).into_any_element()
    }

    /// Whether a replay at time `t` still moves.
    pub fn animating(&self, t: f32) -> bool {
        let n = self.items.len().max(self.outer_items.len()) as f32;
        t.is_finite() && t < n * STAGGER_S + SETTLE_S + PREVIEW_OUTER_DELAY_S
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_menu::{Hint, entry};

    fn entries(n: usize) -> Vec<Entry> {
        (0..n).map(|i| entry(0, "box", format!("Item {i}"), if i == 1 { Action::AddMenu } else { Action::Studio }).hint((i == 0).then(|| Hint::Text("1".into())))).collect()
    }

    #[test]
    fn the_preview_box_holds_the_ring_and_its_outer_ring() {
        let menu = DesktopMenuConfig::default();
        let p = Preview::new(&menu, entries(9), entries(14), 300.0, false);
        let (w, h) = p.size();
        let reach = p.outer.map(|a| a.radius + a.item / 2.0).unwrap();
        assert!(w >= 2.0 * reach && h >= 2.0 * reach, "the box grows for the outer ring");
        assert!(p.outer.unwrap().fits(p.center, p.size()), "and the outer ring fits it unturned");
        // The outer ring answers only while its parent (or it) is lit.
        let a = p.outer.unwrap();
        let on_outer = a.pos(p.center, 3);
        assert_eq!(p.hit(on_outer, Sel::None), Sel::None);
        assert_eq!(p.hit(on_outer, Sel::Main(1)), Sel::Outer(3));
        assert_eq!(p.hit(p.ring.pos(p.center, 4), Sel::None), Sel::Main(4));
        assert_eq!(p.hit(p.center, Sel::None), Sel::Hub);
        // No outer items: no outer ring.
        let bare = Preview::new(&menu, entries(9), Vec::new(), 300.0, false);
        assert!(bare.outer.is_none() && bare.size().1 >= PREVIEW_MIN_H);
        assert!(bare.animating(0.1) && !bare.animating(f32::INFINITY) && !bare.animating(5.0));
    }

    #[test]
    fn the_scene_blooms_out_of_the_click_and_settles_on_the_ring() {
        let ring = Ring::new(8, 100.0, 44.0, false, false);
        let mut s = Scene { ring, center: (300.0, 300.0), p: 0.0, alpha: 1.0, t: 0.0, closing: false, sel: Sel::None, arc: (geom::TOP, 100.0, 0.5, 52.0, 0.0), outer: None, labels: MenuLabels::Hub, opacity: 0.94 };
        // At the click: nothing has left yet, and later items start later.
        assert_eq!(s.item(0).1, 0.0);
        s.t = 0.05;
        assert!(s.item(0).1 > s.item(7).1, "staggered outward");
        let near = geom::dist(s.center, s.item(0).0);
        assert!(near < ring.radius);
        // Settled: every item sits on the ring.
        s.t = f32::INFINITY;
        s.p = 1.0;
        for i in 0..8 {
            assert!((geom::dist(s.center, s.item(i).0) - ring.radius).abs() < 0.01);
        }
        assert!((s.scale() - 1.0).abs() < 1e-6);
        // Closing folds the items in with the glass.
        s.closing = true;
        s.p = 0.3;
        assert!(s.item(0).1 <= 0.3 + 1e-6);
        assert!(s.scale() < 1.0);
        // The arc lights the item it sits on, and fades to its neighbours.
        s.arc = (ring.angle(2), ring.radius, 0.5, 52.0, 1.0);
        assert!((s.near(ring.angle(2), ring.step) - 1.0).abs() < 1e-5);
        assert_eq!(s.near(ring.angle(4), ring.step), 0.0);
        assert!(s.near(ring.angle(2) + ring.step * 0.5, ring.step) > 0.4);
    }

    #[test]
    fn a_flight_is_a_spring_from_zero_to_one() {
        assert_eq!(flight(-1.0), 0.0);
        assert_eq!(flight(0.0), 0.0);
        assert_eq!(flight(f32::INFINITY), 1.0);
        let peak = (1..200).map(|i| flight(i as f32 * 0.005)).fold(0.0_f32, f32::max);
        assert!(peak > 1.0 && peak < 1.15, "a small overshoot ({peak})");
        assert!((flight(2.0) - 1.0).abs() < 1e-3, "settled");
    }
}
