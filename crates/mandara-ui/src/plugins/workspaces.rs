//! Workspaces / tags plugin.
//!
//! Each tag (MangoWM) or workspace (Niri, Hyprland) is a pill showing its
//! number and the icons of the apps on it; empty ones show only the number.
//! Apps without a themed icon get a generated monogram tile. Every active tag
//! carries a liquid capsule whose leading edge springs fast and trailing edge
//! slow, so it stretches as it travels; capsules on neighbouring active tags
//! (a MangoWM composite view) melt into one shape through a smooth minimum of
//! their distance fields, and one Oklab gradient runs across them all.
//! Left-click views, right-click toggles into a composite view (MangoWM
//! `toggleview`), middle-click toggles the focused window onto the tag.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, Background, Bounds, ColorSpace, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement, Pixels, Point, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, Window, div, fill, img, linear_color_stop, linear_gradient, point, px, size,
};
use mandara_compositor::{ClientInfo, NavigationModel};
use mandara_core::liquid::{self, Axis, Capsule, Outline};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;
use smallvec::SmallVec;

use super::fn_view;
use super::panel_fx::{SMALL_ICON_PX, TINT_FILL, TINT_HOVER, WASH_TRACK, alpha};
use crate::fonts::tabular_nums;
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::text;

pub const ID: &str = "workspaces";

const PILL_H: f32 = 22.0;
const NUMBER_W: f32 = 22.0;
const ICON: f32 = 14.0;
const ICON_GAP: f32 = 3.0;
const MAX_ICONS: usize = 3;
const GAP: f32 = 2.0;
/// Extra width of the `+N` overflow label.
const OVERFLOW_W_PX: f32 = 16.0;
/// Pills with apps tuck the number closer to the icons…
const NUMBER_TUCK_PX: f32 = 4.0;
/// …and pad both ends.
const APPS_PAD_PX: f32 = 8.0;
/// Horizontal padding of the number beside app icons.
const NUMBER_PAD_PX: f32 = 2.0;
/// Empty tags dim their number to this alpha.
const EMPTY_ALPHA: f32 = 0.55;
/// The `+N` overflow label.
const OVERFLOW_TEXT_PX: f32 = 9.5;
const OVERFLOW_ALPHA: f32 = 0.8;
/// Highest tag / workspace number accepted from the CLI.
const MAX_TAG: i64 = 32;

// MangoWM layout pill and keymode badge.
const LAYOUT_PAD_X_PX: f32 = 9.0;
const LAYOUT_GAP_PX: f32 = 5.0;
const LAYOUT_TEXT_PX: f32 = 10.5;
/// The layout pill fades from this opacity as it pops in.
const LAYOUT_MIN_OPACITY: f32 = 0.35;
/// Its glyph grows from this share of its size.
const LAYOUT_GLYPH_MIN: f32 = 0.8;
/// The pop spring may overshoot to this.
const POP_MAX: f32 = 1.2;
/// The pop rests within one 8-bit opacity step.
const POP_REST: f32 = 1.0 / 255.0;
const KEYMODE_PAD_X_PX: f32 = 8.0;
const KEYMODE_TEXT_PX: f32 = 10.0;
/// Gap between the tag strip, layout pill and keymode badge.
const STRIP_GAP_PX: f32 = 6.0;

const FAST: SpringConfig = SpringConfig::new(420.0, 32.0, 1.0);
const SLOW: SpringConfig = SpringConfig::new(170.0, 24.0, 1.0);
/// The indicator rests once within a quarter pixel of its target.
const WORM_REST_PX: f32 = 0.25;
/// The lead edge is "moving right" past this much travel.
const WORM_DIRECTION_PX: f32 = 0.5;

/// A spring counts as resting below this speed, in units per second per
/// unit of rest distance.
const REST_SPEED_PER_UNIT: f32 = 10.0;

/// Steps `spring` and snaps it to rest once it is within `rest` of its
/// target and nearly still — the core's tiny default epsilon keeps a pixel
/// spring animating for many invisible frames. Returns whether it moves.
fn step_visible(spring: &mut SpringValue<f32>, dt: f32, rest: f32) -> bool {
    if !spring.step(dt) {
        return false;
    }
    let near = (spring.value() - spring.target()).abs() < rest;
    let slow = spring.velocity()[0].abs() < rest * REST_SPEED_PER_UNIT;
    if near && slow {
        spring.snap_to_target();
        return false;
    }
    true
}

/// Smooth-minimum blend distance of the indicator blobs, px. Two active
/// pills [`GAP`] apart fuse into one liquid shape with a soft waist about
/// three quarters of the pill's thickness (smaller values pinch it into a
/// sharp notch, larger ones flatten it into a plain bar); with an inactive
/// pill between them they sit 26 px or more apart, past this reach, and stay
/// two clean capsules.
const LIQUID_K_PX: f32 = 14.0;
/// Outline sample spacing along the strip, logical px: one device pixel or
/// finer up to 2× scale, and cosine-packed even finer at the round tips.
const LIQUID_SPACING_PX: f32 = 0.5;
/// CSS gradient angles that run along a horizontal / vertical strip.
const ALONG_X_DEG: f32 = 90.0;
const ALONG_Y_DEG: f32 = 180.0;
/// Below this length a shape or the gradient run is drawn in one colour
/// (the gradient's stops would collapse onto each other).
const MIN_GRADIENT_PX: f32 = 0.5;
/// Half-width of the anti-aliased ring along a merged outline, logical px.
/// The fade itself is one device pixel wide whatever this is; the ring only
/// has to hold it (half a device pixel each side) at every supported scale.
const FRINGE_PX: f32 = 1.0;
/// GPUI's path shader anti-aliases a triangle along the curve
/// `s² − t = 0` of its interpolated `(s, t)` coordinates, at
/// `alpha = clamp(0.5 − f / |∇f|)` with `f = s² − t`, and only where `s`
/// varies. Setting `t` to the signed distance from the edge (inside
/// positive) and `s = FRINGE_S × t` makes `f ≈ −t`, so the shader fades
/// exactly across the edge; this keeps `s²` negligible while `∇s` stays
/// above the shader's 0.001-per-pixel cut-off up to 50× scale.
const FRINGE_S: f32 = 0.05;
/// Active blobs kept inline before spilling to the heap.
const BLOBS_INLINE: usize = 9;

/// One active-tag indicator blob: a capsule whose two ends spring
/// independently.
struct Blob {
    /// The pill it rests on, or `None` while it shrinks away.
    pill: Option<usize>,
    start: SpringValue<f32>,
    end: SpringValue<f32>,
}

impl Blob {
    /// A dot at `at`, about to grow.
    fn seed(pill: usize, at: f32) -> Self {
        Self { pill: Some(pill), start: SpringValue::new(at, FAST), end: SpringValue::new(at, FAST) }
    }

    fn span(&self) -> (f32, f32) {
        let (a, b) = (self.start.value(), self.end.value());
        (a.min(b), a.max(b))
    }

    fn centre(&self) -> f32 {
        (self.start.value() + self.end.value()) * 0.5
    }

    fn animating(&self) -> bool {
        self.start.is_animating() || self.end.is_animating()
    }

    fn set_targets(&mut self, (start, end): (f32, f32)) {
        self.start.set_target(start);
        self.end.set_target(end);
    }

    /// Picks the springs for this frame. Travelling, the edge in the
    /// direction of travel leads (fast) and the other trails (slow), so the
    /// blob stretches like a worm; growing, shrinking or settling, both ends
    /// move briskly and symmetrically.
    fn choose_springs(&mut self) {
        let ds = self.start.target() - self.start.value();
        let de = self.end.target() - self.end.value();
        let (start, end) = if ds > WORM_DIRECTION_PX && de > WORM_DIRECTION_PX {
            (SLOW, FAST)
        } else if ds < -WORM_DIRECTION_PX && de < -WORM_DIRECTION_PX {
            (FAST, SLOW)
        } else {
            (FAST, FAST)
        };
        self.start.set_config(start);
        self.end.set_config(end);
    }
}

/// The active indicator: one blob per active tag, merged with a smooth
/// minimum where they meet (see [`mandara_core::liquid`]). The shapes are
/// rebuilt only when a blob moved; idle, it costs one comparison a render.
struct Liquid {
    blobs: Vec<Blob>,
    clock: FrameClock,
    init: bool,
    /// Blob spans (sorted) and half-thickness the shapes were built from.
    spans: SmallVec<[(f32, f32); BLOBS_INLINE]>,
    half: f32,
    /// Lone blobs, drawn as exact anti-aliased capsules.
    quads: SmallVec<[(f32, f32); BLOBS_INLINE]>,
    /// Merged groups, traced.
    outline: Outline,
    /// The gradient runs across this span of the strip, the same for every
    /// shape, so a blob keeps its colours as it joins or leaves a group.
    gradient: (f32, f32),
    caps: SmallVec<[Capsule; BLOBS_INLINE]>,
}

impl Liquid {
    fn new() -> Self {
        Self {
            blobs: Vec::new(),
            clock: FrameClock::new(),
            init: false,
            spans: SmallVec::new(),
            half: 0.0,
            quads: SmallVec::new(),
            outline: Outline::default(),
            gradient: (0.0, 0.0),
            caps: SmallVec::new(),
        }
    }

    /// Matches blobs to the `active` pills (sorted indices into `pills`,
    /// each pill's `(start, end)` along the strip). Blobs on pills that stay
    /// active keep their place; a pill that turns active takes over the
    /// nearest blob that lost its pill — so viewing another tag sends the
    /// indicator travelling there — or else grows a new blob from its centre;
    /// leftover blobs shrink into their pill's centre and disappear.
    fn reconcile(&mut self, active: &[usize], pills: &[(f32, f32)]) {
        if !self.init {
            self.init = true;
            self.blobs.extend(active.iter().filter_map(|&i| {
                let (s, e) = *pills.get(i)?;
                let mut b = Blob::seed(i, s);
                b.end.snap_to(e);
                Some(b)
            }));
            return;
        }
        let centre = |(s, e): (f32, f32)| (s + e) * 0.5;
        for b in &mut self.blobs {
            if b.pill.is_some_and(|i| !active.contains(&i) || i >= pills.len()) {
                b.pill = None;
                let c = centre((b.start.target(), b.end.target()));
                b.set_targets((c, c));
            }
        }
        for &i in active {
            let Some(&span) = pills.get(i) else { continue };
            if let Some(b) = self.blobs.iter_mut().find(|b| b.pill == Some(i)) {
                b.set_targets(span);
                continue;
            }
            let free = self.blobs.iter_mut().filter(|b| b.pill.is_none()).min_by(|a, b| (a.centre() - centre(span)).abs().total_cmp(&(b.centre() - centre(span)).abs()));
            match free {
                Some(b) => {
                    b.pill = Some(i);
                    b.set_targets(span);
                }
                None => {
                    let mut b = Blob::seed(i, centre(span));
                    b.set_targets(span);
                    self.blobs.push(b);
                }
            }
        }
    }

    /// Steps every blob by `dt`; returns whether any still moves. Blobs that
    /// finished shrinking are dropped.
    fn step(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for b in &mut self.blobs {
            b.choose_springs();
            moving |= step_visible(&mut b.start, dt, WORM_REST_PX) | step_visible(&mut b.end, dt, WORM_REST_PX);
        }
        self.blobs.retain(|b| b.pill.is_some() || b.animating());
        moving
    }

    /// [`Self::step`] on the frame clock; zero work while everything rests.
    fn advance(&mut self, now: Instant) -> bool {
        if !self.blobs.iter().any(Blob::animating) {
            self.blobs.retain(|b| b.pill.is_some());
            self.clock.idle();
            return false;
        }
        let dt = self.clock.tick(now);
        let moving = self.step(dt);
        if !moving {
            self.clock.idle();
        }
        moving
    }

    /// Rebuilds the drawn shapes for a strip `2 × half` thick, if any blob
    /// moved since the last build.
    fn rebuild(&mut self, half: f32) {
        let mut spans: SmallVec<[(f32, f32); BLOBS_INLINE]> = self.blobs.iter().map(Blob::span).collect();
        spans.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        if spans == self.spans && half == self.half {
            return;
        }
        self.spans = spans;
        self.half = half;
        self.caps.clear();
        self.caps.extend(self.spans.iter().map(|&(s, e)| Capsule::from_span(s, e, half)));
        self.quads.clear();
        self.outline.clear();
        for group in liquid::groups(&self.caps, LIQUID_K_PX) {
            if group.len() == 1 {
                self.quads.push(self.spans[group.start]);
            } else {
                self.outline.trace(&self.caps[group], LIQUID_K_PX, half, LIQUID_SPACING_PX);
            }
        }
        self.gradient = self.spans.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(s, e)| (lo.min(s), hi.max(e)));
    }

    /// Whether `u` (along the strip) lies inside a blob.
    fn covers(&self, u: f32) -> bool {
        self.spans.iter().any(|&(s, e)| u >= s && u <= e)
    }

    /// The fill of a shape spanning `a..b` along the strip: its slice of the
    /// shared `from → to` gradient, interpolated in Oklab so it never greys.
    fn fill_between(&self, a: f32, b: f32, from: Color, to: Color, angle: f32) -> Background {
        let (len, (g0, g1)) = (b - a, self.gradient);
        if len < MIN_GRADIENT_PX || g1 - g0 < MIN_GRADIENT_PX {
            return from.hsla().into();
        }
        linear_gradient(angle, linear_color_stop(from.hsla(), (g0 - a) / len), linear_color_stop(to.hsla(), (g1 - a) / len)).color_space(ColorSpace::Oklab)
    }

    /// Paints the shapes with the strip's top-left at `origin`.
    fn paint(&self, window: &mut Window, origin: Point<Pixels>, axis: Axis, from: Color, to: Color) {
        let angle = if axis == Axis::Vertical { ALONG_Y_DEG } else { ALONG_X_DEG };
        let o = [f32::from(origin.x), f32::from(origin.y)];
        let half = self.half;
        let at = |p: [f32; 2]| {
            let [x, y] = axis.to_xy(o, half, p);
            point(px(x), px(y))
        };
        for &(a, b) in &self.quads {
            let r = ((b - a) * 0.5).min(half);
            let (along, across) = (b - a, r * 2.0);
            let size = if axis == Axis::Vertical { size(px(across), px(along)) } else { size(px(along), px(across)) };
            window.paint_quad(fill(Bounds::new(at([a, -r]), size), self.fill_between(a, b, from, to, angle)).corner_radii(px(r)));
        }
        // Each merged piece is a band |v| < e(u), built from raw triangles
        // (no tessellator, no overlap): a solid strip between its edges inset
        // by the fringe, and a ring straddling the true edge whose fragments
        // fade analytically over one device pixel, matching the capsules'
        // SDF edges instead of the coarser MSAA staircase.
        let solid = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));
        for contour in self.outline.contours() {
            let top = liquid::top_edge(contour);
            let (Some(first), Some(last)) = (top.first(), top.last()) else { continue };
            let thickness = top.iter().map(|p| p[1].abs()).fold(0.0, f32::max);
            let d = FRINGE_PX.min(thickness * 0.5);
            let offset = |i: usize, by: f32| {
                let ([u, v], [nu, nv]) = (contour[i], liquid::normal(contour, i));
                [u + nu * by, v + nv * by]
            };
            let mut path = gpui::Path::new(at(*first));
            for i in 0..top.len() - 1 {
                let ([u0, v0], [u1, v1]) = (offset(i, -d), offset(i + 1, -d));
                let (t0, t1, b0, b1) = (at([u0, v0]), at([u1, v1]), at([u0, -v0]), at([u1, -v1]));
                path.push_triangle((t0, t1, b1), solid);
                path.push_triangle((t0, b1, b0), solid);
            }
            let (inner, outer) = (point(FRINGE_S * d, d), point(-FRINGE_S * d, -d));
            for i in 0..contour.len() {
                let j = (i + 1) % contour.len();
                let (i0, i1, o0, o1) = (at(offset(i, -d)), at(offset(j, -d)), at(offset(i, d)), at(offset(j, d)));
                path.push_triangle((i0, o0, o1), (inner, outer, outer));
                path.push_triangle((i0, o1, i1), (inner, outer, inner));
            }
            window.paint_path(path, self.fill_between(first[0] - d, last[0] + d, from, to, angle));
        }
    }
}

fn dispatch(cx: &mut gpui::App, f: impl FnOnce(&dyn mandara_compositor::CompositorBackend) -> anyhow::Result<()>) -> Result<(), String> {
    let backend = hub(cx).read(cx).compositor.clone().ok_or("no supported compositor is running")?;
    f(backend.as_ref()).map_err(|e| format!("{e:#}"))
}

/// Tag-number labels, formatted once per thread.
fn number_label(id: u32) -> SharedString {
    thread_local! {
        static LABELS: RefCell<HashMap<u32, SharedString>> = RefCell::new(HashMap::new());
    }
    LABELS.with(|l| l.borrow_mut().entry(id).or_insert_with(|| id.to_string().into()).clone())
}

/// One pill, normalized from either navigation model. Borrows the hub's
/// state for the duration of a render (no per-frame `String` clones).
struct Pill<'a> {
    id: u32,
    label: SharedString,
    active: bool,
    urgent: bool,
    apps: SmallVec<[&'a str; MAX_ICONS]>,
    extra: usize,
}

impl Pill<'_> {
    fn width(&self) -> f32 {
        if self.apps.is_empty() {
            NUMBER_W
        } else {
            let n = self.apps.len() as f32;
            let extra = if self.extra > 0 { OVERFLOW_W_PX } else { 0.0 };
            NUMBER_W - NUMBER_TUCK_PX + n * crate::style::bar_icon(ICON) + (n - 1.0) * ICON_GAP + extra + APPS_PAD_PX
        }
    }
}

/// Distinct apps per tag counted without a heap allocation.
const APPS_INLINE: usize = 8;
/// Workspace names this short are shown instead of their number.
const SHORT_NAME_CHARS: usize = 2;

fn pills<'a>(model: &'a NavigationModel, clients: &'a [ClientInfo], monitor: &str) -> Vec<Pill<'a>> {
    let apps_on = |id: u32| -> (SmallVec<[&'a str; MAX_ICONS]>, usize) {
        let mut seen: SmallVec<[&'a str; APPS_INLINE]> = SmallVec::new();
        for c in clients.iter().filter(|c| (monitor.is_empty() || c.monitor.is_empty() || c.monitor == monitor) && c.tags.contains(&id)) {
            if !c.app_id.is_empty() && !seen.contains(&c.app_id.as_str()) {
                seen.push(c.app_id.as_str());
            }
        }
        let extra = seen.len().saturating_sub(MAX_ICONS);
        (seen.into_iter().take(MAX_ICONS).collect(), extra)
    };
    let mk = |id: u32, label: SharedString, active: bool, urgent: bool| {
        let (apps, extra) = apps_on(id);
        Pill { id, label, active, urgent, apps, extra }
    };
    match model {
        NavigationModel::BitmaskTags { tags, .. } => tags.iter().map(|t| mk(t.id, number_label(t.id), t.active, t.urgent)).collect(),
        NavigationModel::Workspaces { workspaces } => workspaces
            .iter()
            .map(|w| mk(w.id, if w.name.chars().count() <= SHORT_NAME_CHARS { w.name.clone().into() } else { number_label(w.id) }, w.active, w.urgent))
            .collect(),
    }
}

/// Monogram tile: OKLCH lightness on dark / light themes and chroma.
const MONO_L: (f32, f32) = (0.62, 0.70);
const MONO_CHROMA: f32 = 0.11;
/// Corner radius and letter size as fractions of the tile.
const MONO_RADIUS_FRAC: f32 = 0.3;
const MONO_TEXT_FRAC: f32 = 0.62;
/// Gradient angle and the lightness swing across it.
const MONO_ANGLE_DEG: f32 = 150.0;
const MONO_SHADE: f32 = 0.06;
/// Hue hash: seed, multiplier and the hue circle in degrees.
const HASH_SEED: u32 = 7;
const HASH_MUL: u32 = 31;
const HUE_DEGREES: u32 = 360;
/// Monogram letters / hues cached before the cache is reset.
const MONO_CACHE_CAP: usize = 256;

/// A stable hue (degrees) for `name`.
pub fn name_hue(name: &str) -> f32 {
    (name.bytes().fold(HASH_SEED, |h, b| h.wrapping_mul(HASH_MUL).wrapping_add(u32::from(b))) % HUE_DEGREES) as f32
}

/// A generated icon for apps without a themed one: a soft rounded tile with
/// the app's initial in a stable, hue-hashed tint.
pub fn monogram(app_id: &str, size: f32, palette: &MandaraPalette) -> AnyElement {
    thread_local! {
        /// app id → (initial, hue): resolving the app name reads desktop
        /// entries, so it is done once per app, not per frame.
        static LETTERS: RefCell<HashMap<String, (SharedString, f32)>> = RefCell::new(HashMap::new());
    }
    let (letter, hue) = LETTERS.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(hit) = m.get(app_id) {
            return hit.clone();
        }
        if m.len() >= MONO_CACHE_CAP {
            m.clear();
        }
        let name = crate::plugins::window::app_name(app_id);
        let letter: SharedString = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "•".into()).into();
        let hit = (letter, name_hue(app_id));
        m.insert(app_id.to_string(), hit.clone());
        hit
    });
    let bg = Color::oklch(if palette.is_dark { MONO_L.0 } else { MONO_L.1 }, MONO_CHROMA, hue);
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(crate::style::radius(size * MONO_RADIUS_FRAC)))
        .bg(mandara_core::oklab_gradient(MONO_ANGLE_DEG, bg.lighten(MONO_SHADE), bg.lighten(-MONO_SHADE)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * MONO_TEXT_FRAC))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(bg.best_on(Color::hex("#ffffff"), Color::hex("#14110f")).hsla())
        .child(letter)
        .into_any_element()
}

/// A resolved app icon kept ready to draw (no per-frame path clones).
#[derive(Clone)]
enum AppIcon {
    /// Monochrome symbolic SVG, tinted at draw time.
    Symbolic(SharedString),
    Raster(Arc<Path>),
    Missing,
}

/// Resolved icons cached before the cache is reset.
const ICON_CACHE_CAP: usize = 256;

thread_local! {
    /// `[full colour, monochrome]` app id → icon, plus the config generation
    /// the entries were resolved under (a config change can switch the
    /// icon theme, so it drops the cache).
    static ICONS: RefCell<(usize, [HashMap<String, AppIcon>; 2])> = RefCell::new((0, Default::default()));
}

/// Drops every cached app icon (the icon theme changed).
pub fn clear_icon_cache() {
    ICONS.with(|c| c.borrow_mut().1.iter_mut().for_each(HashMap::clear));
}

/// Drops cached icons when the shell config changed since they were
/// resolved. Call from a render that draws app icons.
pub fn sync_icon_cache(cx: &gpui::App) {
    let generation = Arc::as_ptr(&crate::state::config(cx).read(cx).snapshot()) as usize;
    ICONS.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != generation {
            c.0 = generation;
            c.1.iter_mut().for_each(HashMap::clear);
        }
    });
}

fn resolve_icon(app_id: &str, monochrome: bool) -> AppIcon {
    ICONS.with(|c| {
        let mut c = c.borrow_mut();
        let map = &mut c.1[usize::from(monochrome)];
        if let Some(hit) = map.get(app_id) {
            return hit.clone();
        }
        if map.len() >= ICON_CACHE_CAP {
            map.clear();
        }
        let icon = match crate::apps::icon_for_app_id(app_id, monochrome) {
            Some(i) if i.symbolic => AppIcon::Symbolic(i.path.to_string_lossy().into_owned().into()),
            Some(i) => AppIcon::Raster(i.path.into()),
            None => AppIcon::Missing,
        };
        map.insert(app_id.to_string(), icon.clone());
        icon
    })
}

/// The app's icon from the installed theme — monochrome symbolic (tinted
/// with `tint`) when preferred and available, else full color — or a
/// generated monogram.
pub fn app_icon(app_id: &str, size: f32, tint: Color, monochrome: bool, palette: &MandaraPalette) -> AnyElement {
    app_icon_at(app_id, size, size, tint, monochrome, palette)
}

/// [`app_icon`] drawn at `size` from a bitmap decoded for `decode_size`
/// (at least `size`). An icon whose size animates (the dock's
/// magnification) passes the largest size it reaches, so it keeps one
/// bitmap the whole way instead of blanking while each new size decodes.
pub fn app_icon_at(app_id: &str, size: f32, decode_size: f32, tint: Color, monochrome: bool, palette: &MandaraPalette) -> AnyElement {
    match resolve_icon(app_id, monochrome) {
        AppIcon::Symbolic(path) => gpui::svg().external_path(path).size(px(size)).flex_none().text_color(tint.hsla()).into_any_element(),
        AppIcon::Raster(path) if crate::icon_raster::handles(&path) => {
            div().size(px(size)).flex_none().child(img(crate::icon_raster::source(&path, decode_size.max(size))).size_full().object_fit(ObjectFit::Contain)).into_any_element()
        }
        AppIcon::Raster(path) => div().size(px(size)).flex_none().child(img(path).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
        AppIcon::Missing => monogram(app_id, size, palette),
    }
}

/// The app's symbolic (monochrome) icon tinted `tint`, when its icon theme
/// has one (Studio → Launcher → App icons: Monochrome).
pub fn symbolic_icon(app_id: &str, size: f32, tint: Color) -> Option<AnyElement> {
    match resolve_icon(app_id, true) {
        AppIcon::Symbolic(path) => Some(gpui::svg().external_path(path).size(px(size)).flex_none().text_color(tint.hsla()).into_any_element()),
        _ => None,
    }
}

/// Icon and display name of a MangoWM layout code (`T`, `S`, `VS`, …).
fn layout_label(code: &str) -> (&'static str, &'static str) {
    const LAYOUTS: &[(&str, &str, &str)] = &[
        ("T", "sidebar", mandara_i18n::tk("Tile")),
        ("S", "skip", mandara_i18n::tk("Scroller")),
        ("G", "grid", mandara_i18n::tk("Grid")),
        ("M", "maximize", mandara_i18n::tk("Monocle")),
        ("K", "window", mandara_i18n::tk("Deck")),
        ("CT", "sidebar", mandara_i18n::tk("Center tile")),
        ("RT", "sidebar", mandara_i18n::tk("Right tile")),
        ("VT", "sidebar", mandara_i18n::tk("Vertical tile")),
        ("VS", "skip", mandara_i18n::tk("Vertical scroller")),
        ("VG", "grid", mandara_i18n::tk("Vertical grid")),
        ("VK", "window", mandara_i18n::tk("Vertical deck")),
        ("TG", "grid", mandara_i18n::tk("Tgmix")),
    ];
    let code = code.trim();
    LAYOUTS
        .iter()
        .find(|(c, ..)| c.eq_ignore_ascii_case(code))
        .map_or(("grid", mandara_i18n::tk("Layout")), |&(_, glyph, name)| (glyph, name))
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Workspaces"), mandara_i18n::tk("Tags / workspaces with their apps"),Some(Zone::Left)), |window, cx, panel_cx| {
        let liquid = Rc::new(RefCell::new(Liquid::new()));
        // MangoWM layout pill: (layout shown, pop spring, clock).
        let layout_anim = Rc::new(RefCell::new((String::new(), SpringValue::new(1.0, FAST), FrameClock::new())));
        let monitor: SharedString = panel_cx.monitor().to_string().into();
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                sync_icon_cache(cx);
                let cx_ref: &gpui::App = cx;
                let h = hub(cx_ref).read(cx_ref);
                let Some(model) = h.navigation.get(monitor.as_ref()).or_else(|| h.navigation.values().next()) else { return div().into_any_element() };
                let list = pills(model, &h.clients, &monitor);
                let monochrome = crate::state::config(cx_ref).read(cx_ref).get().appearance.bar_icons == mandara_config::IconStyle::Monochrome;
                // A side bar stacks number-only pills in a column.
                let vertical = crate::bar::vertical(cx_ref);
                let along = |p: &Pill| if vertical { PILL_H } else { p.width() };

                // Pixel layout of every pill (along the strip).
                let mut xs: SmallVec<[(f32, f32); APPS_INLINE * 2]> = SmallVec::with_capacity(list.len());
                let mut x = 0.0;
                for p in &list {
                    xs.push((x, along(p)));
                    x += along(p) + GAP;
                }
                // One liquid blob per active tag; neighbours fuse.
                let spans: SmallVec<[(f32, f32); APPS_INLINE * 2]> = xs.iter().map(|&(x, w)| (x, x + w)).collect();
                let active: SmallVec<[usize; BLOBS_INLINE]> = list.iter().enumerate().filter(|(_, p)| p.active).map(|(i, _)| i).collect();
                let strip_len = (x - GAP).max(0.0);
                let (axis, half) = if vertical { (Axis::Vertical, NUMBER_W / 2.0) } else { (Axis::Horizontal, PILL_H / 2.0) };
                let mut blobs = liquid.borrow_mut();
                blobs.reconcile(&active, &spans);
                if blobs.advance(Instant::now()) {
                    window.request_animation_frame();
                }
                blobs.rebuild(half);
                let indicator = {
                    let liquid = liquid.clone();
                    let (from, to) = (palette.accent_gradient_start, palette.accent_gradient_end);
                    gpui::canvas(|_, _, _| {}, move |bounds: Bounds<Pixels>, (), window, _| liquid.borrow().paint(window, bounds.origin, axis, from, to))
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(if vertical { NUMBER_W } else { strip_len }))
                        .h(px(if vertical { strip_len } else { PILL_H }))
                };

                let hover = palette.on_surface.with_alpha(alpha(WASH_TRACK, palette)).hsla();
                let children = list.iter().enumerate().map(|(i, p)| {
                    let id = p.id;
                    let (m1, m2) = (monitor.clone(), monitor.clone());
                    let (px0, pw) = xs[i];
                    let centre = px0 + pw / 2.0;
                    let under = blobs.covers(centre);
                    let fg = if under {
                        palette.on_primary
                    } else if p.urgent {
                        palette.urgent_coral
                    } else if p.apps.is_empty() {
                        palette.on_surface_variant.with_alpha(EMPTY_ALPHA)
                    } else {
                        palette.on_surface
                    };
                    let mut pill = div()
                        .id(("tag", i))
                        .h(px(PILL_H))
                        .w(px(if vertical { NUMBER_W } else { pw }))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(ICON_GAP))
                        .cursor_pointer()
                        .text_size(px(text::LABEL))
                        .font_weight(if under || !p.apps.is_empty() { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL })
                        .text_color(fg.hsla())
                        .hover(move |s| s.bg(hover))
                        .child(div().px(px(if p.apps.is_empty() { 0.0 } else { NUMBER_PAD_PX })).child(p.label.clone()));
                    for app in p.apps.iter().filter(|_| !vertical) {
                        pill = pill.child(app_icon(app, crate::style::bar_icon(ICON), fg, monochrome, palette));
                    }
                    if p.extra > 0 && !vertical {
                        pill = pill.child(div().text_size(px(OVERFLOW_TEXT_PX)).text_color(fg.with_alpha(OVERFLOW_ALPHA).hsla()).child(format!("+{}", p.extra)));
                    }
                    pill.on_click(move |_, _, cx| {
                        let _ = dispatch(cx, |b| b.select_primary(&m1, id));
                    })
                    .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = dispatch(cx, |b| b.toggle_secondary(&m2, id));
                    })
                    .on_mouse_down(MouseButton::Middle, move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = dispatch(cx, |b| b.toggle_client_tag(None, id));
                    })
                });
                let strip = gpui::prelude::FluentBuilder::when(div().relative().flex(), vertical, |el| el.flex_col()).gap(px(GAP)).font_features(tabular_nums()).child(indicator).children(children);
                if vertical {
                    return strip.into_any_element();
                }
                // MangoWM: the active tag's layout (click cycles) and a
                // badge for non-default keymodes.
                let NavigationModel::BitmaskTags { active_layout, .. } = model else { return strip.into_any_element() };
                let keymode = h.keymode.as_str();
                let (glyph, name) = layout_label(active_layout);
                let pop = {
                    let mut l = layout_anim.borrow_mut();
                    if l.0 != *active_layout {
                        l.0.clone_from(active_layout);
                        l.1.snap_to(0.0);
                        l.1.set_target(1.0);
                    }
                    if l.1.is_animating() {
                        let dt = l.2.tick(Instant::now());
                        if step_visible(&mut l.1, dt, POP_REST) {
                            window.request_animation_frame();
                        } else {
                            l.2.idle();
                        }
                    }
                    l.1.value().clamp(0.0, POP_MAX)
                };
                let secondary = palette.secondary;
                let layout_pill = div()
                    .id("mango-layout")
                    .h(px(PILL_H))
                    .px(px(LAYOUT_PAD_X_PX))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(LAYOUT_GAP_PX))
                    .cursor_pointer()
                    .bg(secondary.with_alpha(TINT_FILL.0).hsla())
                    .text_color(secondary.hsla())
                    .text_size(px(LAYOUT_TEXT_PX))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .hover(move |s| s.bg(secondary.with_alpha(TINT_HOVER.0).hsla()))
                    .opacity(LAYOUT_MIN_OPACITY + (1.0 - LAYOUT_MIN_OPACITY) * pop.min(1.0))
                    .child(icon(glyph, SMALL_ICON_PX * (LAYOUT_GLYPH_MIN + (1.0 - LAYOUT_GLYPH_MIN) * pop), secondary))
                    .child(mandara_i18n::tr(name))
                    .on_click(|_, _, cx| {
                        let _ = dispatch(cx, |b| b.cycle_layout(""));
                    });
                let mode = (!keymode.is_empty() && keymode != "default").then(|| {
                    div()
                        .h(px(PILL_H))
                        .px(px(KEYMODE_PAD_X_PX))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .bg(palette.tertiary.hsla())
                        .text_color(palette.on_primary.hsla())
                        .text_size(px(KEYMODE_TEXT_PX))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(keymode.to_uppercase())
                });
                div().flex().items_center().gap(px(STRIP_GAP_PX)).child(strip).child(layout_pill).children(mode).into_any_element()
            },
            window,
            cx,
        );
        PluginInstance { bar: Some(bar), ..PluginInstance::empty() }
    });

    r.command(ID, CommandSpec::new("workspace list", mandara_i18n::tk("Tags / workspaces per monitor"), |_, cx| {
        Ok(serde_json::to_value(&hub(cx).read(cx).navigation).unwrap_or_default())
    }));
    for (path, summary, action) in [
        ("workspace view", mandara_i18n::tk("Show only this tag / focus this workspace"), 0u8),
        ("workspace toggle", mandara_i18n::tk("Toggle tag into the composite view (MangoWM `toggleview`)"), 1),
        ("workspace tag-window", mandara_i18n::tk("Toggle the focused window onto a tag (`toggletag`)"), 2),
    ] {
        r.command(
            ID,
            CommandSpec::new(path, summary, move |a, cx| {
                let tag = u32::try_from(a.int(0)?.clamp(1, MAX_TAG)).unwrap_or(1);
                dispatch(cx, |b| match action {
                    0 => b.select_primary("", tag),
                    1 => b.toggle_secondary("", tag),
                    _ => b.toggle_client_tag(None, tag),
                })?;
                Ok(json!({ "tag": tag }))
            })
            .arg(ArgSpec::required("tag", ArgKind::Integer, mandara_i18n::tk("tag / workspace number")))
            .example("workspace view 3"),
        );
    }
    r.command(ID, CommandSpec::new("workspace cycle-layout", mandara_i18n::tk("Switch to the next tiling layout"), |_, cx| {
        dispatch(cx, |b| b.cycle_layout(""))?;
        Ok(json!("ok"))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_compositor::TagStatus;

    fn tag(id: u32, active: bool) -> TagStatus {
        TagStatus { id, active, occupied: false, urgent: false, client_count: 0, layout: "T".into() }
    }

    fn client(app: &str, tag: u32) -> ClientInfo {
        ClientInfo { id: tag as u64, app_id: app.into(), monitor: "M".into(), tags: vec![tag], ..Default::default() }
    }

    #[test]
    fn pills_carry_unique_apps_capped_at_three() {
        let model = NavigationModel::BitmaskTags { tags: vec![tag(1, true), tag(2, false)], active_layout: "T".into(), keymode: String::new() };
        let clients = vec![client("zed", 1), client("zed", 1), client("firefox", 1), client("a", 1), client("b", 1), client("c", 1)];
        let p = pills(&model, &clients, "M");
        assert_eq!(p[0].apps.as_slice(), ["zed", "firefox", "a"]);
        assert_eq!(p[0].extra, 2);
        assert!(p[1].apps.is_empty());
        assert_eq!(p[1].width(), NUMBER_W, "empty tags show only their number");
        assert!(p[0].width() > NUMBER_W);
    }

    /// Five number-only pills laid out like the bar does.
    fn strip() -> Vec<(f32, f32)> {
        (0..5).map(|i| i as f32 * (NUMBER_W + GAP)).map(|x| (x, x + NUMBER_W)).collect()
    }

    /// Runs the blobs to rest at 144 Hz.
    fn settle(l: &mut Liquid) {
        let mut frames = 0;
        while l.step(1.0 / 144.0) {
            frames += 1;
            assert!(frames < 10_000, "the springs never rest");
        }
    }

    fn pills_of(l: &Liquid) -> Vec<Option<usize>> {
        let mut p: Vec<_> = l.blobs.iter().map(|b| b.pill).collect();
        p.sort();
        p
    }

    #[test]
    fn active_sets_map_to_blobs() {
        let s = strip();
        let half = PILL_H / 2.0;
        // Contiguous: two blobs that melt into one traced shape.
        let mut l = Liquid::new();
        l.reconcile(&[1, 2], &s);
        assert_eq!(pills_of(&l), [Some(1), Some(2)]);
        l.rebuild(half);
        assert!(l.quads.is_empty());
        assert_eq!(l.outline.contours().count(), 1, "adjacent tags fuse");
        assert!(l.covers(s[1].0 + NUMBER_W / 2.0) && l.covers(s[2].0 + NUMBER_W / 2.0));
        assert!(!l.covers(s[3].0 + NUMBER_W / 2.0));
        // Non-contiguous: two separate exact capsules.
        let mut l = Liquid::new();
        l.reconcile(&[0, 2], &s);
        l.rebuild(half);
        assert_eq!(l.quads.as_slice(), [s[0], s[2]]);
        assert!(l.outline.is_empty());
        assert!(!l.covers(s[1].0 + NUMBER_W / 2.0), "the tag between stays uncovered");
        // Empty: nothing drawn.
        let mut l = Liquid::new();
        l.reconcile(&[], &s);
        l.rebuild(half);
        assert!(l.blobs.is_empty() && l.quads.is_empty() && l.outline.is_empty());
    }

    #[test]
    fn viewing_another_tag_moves_the_same_blob() {
        let s = strip();
        let mut l = Liquid::new();
        l.reconcile(&[0], &s);
        l.reconcile(&[3], &s);
        assert_eq!(pills_of(&l), [Some(3)], "the indicator travels, it doesn't respawn");
        assert_eq!(l.blobs[0].start.value(), s[0].0, "starting from where it was");
        l.step(1.0 / 144.0);
        let (a, b) = l.blobs[0].span();
        assert!(b - a > NUMBER_W, "it stretches as it travels");
        settle(&mut l);
        assert_eq!(l.blobs[0].span(), s[3]);
    }

    #[test]
    fn toggled_tags_grow_and_shrink_in_place() {
        let s = strip();
        let mut l = Liquid::new();
        l.reconcile(&[0], &s);
        l.reconcile(&[0, 2], &s);
        assert_eq!(pills_of(&l), [Some(0), Some(2)]);
        let grown = l.blobs.iter().find(|b| b.pill == Some(2)).unwrap();
        let centre = (s[2].0 + s[2].1) / 2.0;
        assert_eq!(grown.span(), (centre, centre), "a new blob grows from its pill's centre");
        settle(&mut l);
        assert_eq!(l.blobs.iter().find(|b| b.pill == Some(2)).unwrap().span(), s[2]);
        // Toggling it off shrinks it into its centre, then drops it.
        l.reconcile(&[0], &s);
        assert_eq!(pills_of(&l), [None, Some(0)]);
        settle(&mut l);
        assert_eq!(pills_of(&l), [Some(0)]);
        assert_eq!(l.blobs[0].span(), s[0], "the remaining blob never moved");
    }

    #[test]
    fn idle_blobs_skip_rebuilds() {
        let s = strip();
        let mut l = Liquid::new();
        l.reconcile(&[1, 2], &s);
        l.rebuild(PILL_H / 2.0);
        let before = l.outline.contours().next().unwrap().as_ptr();
        l.reconcile(&[1, 2], &s);
        assert!(!l.advance(Instant::now()), "nothing moves");
        l.rebuild(PILL_H / 2.0);
        assert_eq!(l.outline.contours().next().unwrap().as_ptr(), before);
    }
}
