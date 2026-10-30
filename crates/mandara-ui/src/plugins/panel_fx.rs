//! Shared visual vocabulary for the rich dashboard panels (media, prayer
//! times, resources, network, Bluetooth, clipboard, sound, notifications):
//! chromatic-glass cards, theme-aware accents, the panel silhouette for
//! clean rounded clipping, headers, switches, signal bars, thick sliders,
//! SDF bar sparklines, dotted progress rings, the live spectrum tap, a
//! coalesced per-view redraw timer that only runs while a panel is on
//! screen, and the design tokens every built-in plugin shares.

use crate::bidi::Bidi as _;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AsyncApp, BoxShadow, Div, EntityId, FontWeight, ParentElement, SharedString, Styled, Window, div, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui, glass_rim, oklab_gradient};
use mandara_i18n::tf;
use mandara_theme::{Color, MandaraPalette};
use smallvec::SmallVec;

use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::text;

// ── Design tokens shared by every built-in plugin ────────────────────────
//
// Spacing steps and text sizes live in `crate::kit::{tokens, text}`; these
// are the panel-specific ones. Alpha pairs are `(dark, light)` theme.

/// Corner radius of a chromatic-glass card.
pub const CARD_RADIUS_PX: f32 = 18.0;
/// Smallest radius a card nested in a panel may get.
pub const MIN_INNER_RADIUS_PX: f32 = 8.0;
/// Height of a section label row.
pub const SECTION_H_PX: f32 = 18.0;
/// Height of [`header`].
pub const HEADER_H: f32 = 38.0;
/// Side of the gradient icon chip in a panel [`header`].
pub const HEADER_CHIP_PX: f32 = 36.0;
/// Header icon chip corner radius as a fraction of its side.
const ICON_CHIP_RADIUS_FRAC: f32 = 0.34;
/// Header icon glyph size as a fraction of its chip.
const ICON_CHIP_GLYPH_FRAC: f32 = 0.53;
/// Header gap between the icon chip and the titles.
const HEADER_GAP_PX: f32 = 12.0;
/// Height of a value [`chip`].
pub const CHIP_H_PX: f32 = 24.0;
const CHIP_PAD_X_PX: f32 = 9.0;
const CHIP_GAP_PX: f32 = 5.0;
/// Small glyphs inside chips, links and buttons.
pub const SMALL_ICON_PX: f32 = 12.0;
/// Height of a [`pill_button`].
pub const PILL_H_PX: f32 = 28.0;
const PILL_PAD_X_PX: f32 = 11.0;
const PILL_ICON_PX: f32 = 13.0;
/// Pill label size: half a step above the label size.
const PILL_TEXT_PX: f32 = text::LABEL + 0.5;
/// Height of a text link / "show more" pill.
pub const LINK_H_PX: f32 = 20.0;
/// Horizontal padding of a text link pill.
pub const LINK_PAD_X_PX: f32 = 7.0;

/// Faint ink washes over the surface: `(dark, light)` alphas.
pub const WASH_FAINT: (f32, f32) = (0.035, 0.04);
pub const WASH_SOFT: (f32, f32) = (0.06, 0.05);
pub const WASH_TRACK: (f32, f32) = (0.08, 0.07);
pub const WASH_STRONG: (f32, f32) = (0.14, 0.12);
pub const WASH_IDLE_BAR: (f32, f32) = (0.16, 0.14);
/// Tinted fills behind tonal buttons and their hover: `(dark, light)`.
pub const TINT_FILL: (f32, f32) = (0.16, 0.12);
pub const TINT_HOVER: (f32, f32) = (0.26, 0.2);
/// Hover wash behind text links.
pub const LINK_HOVER_ALPHA: f32 = 0.1;

/// Picks the dark or light half of an alpha pair for `palette`.
pub fn alpha((dark, light): (f32, f32), palette: &MandaraPalette) -> f32 {
    if palette.is_dark { dark } else { light }
}

/// `on_surface` at the theme's alpha from `pair`.
pub fn wash(pair: (f32, f32), palette: &MandaraPalette) -> Color {
    palette.on_surface.with_alpha(alpha(pair, palette))
}

// ── Redraw scheduling ────────────────────────────────────────────────────

thread_local! {
    /// View → deadline of its pending redraw timer.
    static PENDING: RefCell<HashMap<EntityId, Instant>> = RefCell::new(HashMap::new());
}

/// Redraw requests whose deadlines are closer than this are merged.
const REDRAW_SLACK: Duration = Duration::from_millis(4);
/// Added to "redraw when this expires" delays so the deadline has passed.
pub const EXPIRY_MARGIN: Duration = Duration::from_millis(20);

/// Re-renders the view currently being rendered after `delay` (coalesced
/// per view: an earlier pending redraw wins). Call it from a panel's render
/// while the panel is visible so live readouts tick without a timer running
/// when it is closed. Only the calling view is notified — never a whole
/// window refresh, which would defeat GPUI's view caching.
pub fn redraw_in(delay: Duration, window: &mut Window, cx: &mut App) {
    let view = window.current_view();
    let due = Instant::now() + delay;
    let schedule = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        match p.get(&view) {
            Some(existing) if *existing <= due + REDRAW_SLACK => false,
            _ => {
                p.insert(view, due);
                true
            }
        }
    });
    if !schedule {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        PENDING.with(|p| {
            let mut p = p.borrow_mut();
            if p.get(&view).is_some_and(|d| *d <= Instant::now() + REDRAW_SLACK) {
                p.remove(&view);
            }
        });
        cx.update(|cx| cx.notify(view));
    })
    .detach();
}

/// The views that display one plugin model. Async results (a finished
/// scan, a command's reply) notify exactly these views instead of
/// refreshing every window of the shell, which also bypasses GPUI's view
/// caches. Views register themselves from their render.
#[derive(Clone, Default)]
pub struct ViewSet(Rc<RefCell<SmallVec<[EntityId; 4]>>>);

impl ViewSet {
    /// Registers the view being rendered (call from its render).
    pub fn track(&self, window: &Window) {
        let view = window.current_view();
        let mut views = self.0.borrow_mut();
        if !views.contains(&view) {
            views.push(view);
        }
    }

    /// Re-renders every registered view (hidden ones cost nothing).
    pub fn notify(&self, cx: &mut App) {
        let views = self.0.borrow().clone();
        for view in views {
            cx.notify(view);
        }
    }

    /// [`Self::notify`] from an async task.
    pub fn notify_async(&self, cx: &AsyncApp) {
        cx.update(|cx| self.notify(cx));
    }
}

// ── Live readouts ────────────────────────────────────────────────────────

/// Changes smaller than this (one displayed percent) are invisible in a
/// rounded percentage readout.
pub const PERCENT_STEP: f32 = 0.01;

/// Percent labels `0%..=100%`, formatted once per thread instead of every
/// frame.
pub fn percent_text(frac: f32) -> SharedString {
    const STEPS: usize = 101;
    thread_local! {
        static LABELS: Vec<SharedString> = (0..STEPS).map(|p| SharedString::from(format!("{p}%"))).collect();
    }
    let i = (frac.clamp(0.0, 1.0) / PERCENT_STEP).round() as usize;
    LABELS.with(|l| l[i.min(STEPS - 1)].clone())
}

const SECS_PER_MINUTE: i64 = 60;
const SECS_PER_HOUR: i64 = 60 * SECS_PER_MINUTE;
const SECS_PER_DAY: i64 = 24 * SECS_PER_HOUR;

/// A length of time (not an age): `45 sec`, `8 min`, `4 h 12 min`,
/// `3 d 2 h`, for "up {span}" / "created {span} ago". Its own i18n keys:
/// the compact `{n}m` / `{n}h` keys mean "N minutes / hours ago" in
/// several catalogs.
pub fn duration_short(secs: i64) -> String {
    let s = secs.max(0);
    if s < SECS_PER_MINUTE {
        tf("{n} sec", &[("n", &s)])
    } else if s < SECS_PER_HOUR {
        tf("{n} min", &[("n", &(s / SECS_PER_MINUTE))])
    } else if s < SECS_PER_DAY {
        tf("{h} h {m} min", &[("h", &(s / SECS_PER_HOUR)), ("m", &((s % SECS_PER_HOUR) / SECS_PER_MINUTE))])
    } else {
        tf("{d} d {h} h", &[("d", &(s / SECS_PER_DAY)), ("h", &((s % SECS_PER_DAY) / SECS_PER_HOUR))])
    }
}

/// A formatted label cached with the value it was formatted from, so a
/// readout re-formats only when its value changes (not on every frame).
#[derive(Clone, Debug, Default)]
pub struct Cached<K: PartialEq> {
    key: Option<K>,
    text: SharedString,
}

impl<K: PartialEq> Cached<K> {
    /// The label for `key`, formatting it with `format` only on change.
    pub fn get(&mut self, key: K, format: impl FnOnce(&K) -> String) -> SharedString {
        if self.key.as_ref() != Some(&key) {
            self.text = format(&key).into();
            self.key = Some(key);
        }
        self.text.clone()
    }
}

/// A displayed value easing toward live samples (bar chips, meters).
/// Moves smaller than `snap_below` jump straight to the sample — springing
/// a 2 % CPU wiggle drove half a second of full-window frames per sample —
/// and the spring rests as soon as it is within `settle` of its target, so
/// animation frames are requested only while the motion is visible.
pub struct Readout {
    spring: SpringValue<f32>,
    clock: FrameClock,
    snap_below: f32,
    settle: f32,
}

impl Readout {
    pub fn new(config: SpringConfig, snap_below: f32, settle: f32) -> Self {
        Self { spring: SpringValue::new(0.0, config), clock: FrameClock::new(), snap_below, settle }
    }

    /// The value to draw this frame for the live sample `target`; requests
    /// the next frame while the spring is visibly moving.
    pub fn show(&mut self, target: f32, window: &Window) -> f32 {
        if target != self.spring.target() {
            if (target - self.spring.value()).abs() < self.snap_below {
                self.spring.snap_to(target);
            } else {
                self.spring.set_target(target);
            }
        }
        if !self.spring.is_animating() {
            self.clock.idle();
            return self.spring.value();
        }
        let dt = self.clock.tick(Instant::now());
        self.spring.step(dt);
        if (self.spring.value() - target).abs() < self.settle {
            self.spring.snap_to_target();
            self.clock.idle();
        } else {
            window.request_animation_frame();
        }
        self.spring.value()
    }
}

// ── Silhouette & surfaces ────────────────────────────────────────────────

/// The rounded silhouette of the panel content area. GPUI clips children to
/// a parent's *rectangle* only, so every full-bleed layer (backdrops,
/// gradients, grain) must carry these radii itself or its square corners
/// poke past the panel's curve.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PanelShape {
    /// Radius of the two corners facing away from the bar / toward it.
    pub top: f32,
    pub bottom: f32,
}

impl PanelShape {
    pub fn of(cx: &App) -> Self {
        Self::for_config(crate::state::config(cx).read(cx).get())
    }

    /// The silhouette for `cfg`: the panel radius (after the global
    /// roundness) less the panel's inner padding, so content corners stay
    /// concentric with the panel's own.
    pub fn for_config(cfg: &mandara_config::ShellConfig) -> Self {
        let bar = &cfg.bar;
        let attached = bar.effective_layout().is_attached();
        let r = crate::style::concentric(crate::style::panel_radius(bar), cfg.panels.padding_for(attached));
        // (Beside a side bar the panel's top and bottom are both free.)
        if attached && !bar.position.is_vertical() {
            // Attached panels grow out of the bar: square on the bar side.
            if bar.position == mandara_config::BarPosition::Bottom { Self { top: r, bottom: 0.0 } } else { Self { top: 0.0, bottom: r } }
        } else {
            Self { top: r, bottom: r }
        }
    }

    /// Applies the silhouette's corner radii to `el`.
    pub fn clip<E: Styled>(self, el: E) -> E {
        el.rounded_tl(px(self.top)).rounded_tr(px(self.top)).rounded_bl(px(self.bottom)).rounded_br(px(self.bottom))
    }

    /// Concentric radius for a card inset `pad` from the panel edge.
    pub fn inner(self, pad: f32) -> f32 {
        crate::style::concentric(self.top.max(self.bottom), pad).max(crate::style::radius(MIN_INNER_RADIUS_PX))
    }
}

/// OKLCH lightness band accents are clamped into on dark glass.
const READABLE_DARK_L: (f32, f32) = (0.72, 0.86);
/// OKLCH lightness band accents are clamped into on light paper.
const READABLE_LIGHT_L: (f32, f32) = (0.40, 0.52);
/// Chroma boost that keeps darkened accents saturated on light paper.
const READABLE_LIGHT_CHROMA: f32 = 1.05;

/// An accent nudged into a band that reads on the current surface: luminous
/// on dark glass, deep and saturated on light paper.
pub fn readable(c: Color, palette: &MandaraPalette) -> Color {
    if palette.is_dark {
        c.with_lightness(c.lightness().clamp(READABLE_DARK_L.0, READABLE_DARK_L.1))
    } else {
        c.with_lightness(c.lightness().clamp(READABLE_LIGHT_L.0, READABLE_LIGHT_L.1)).scale_chroma(READABLE_LIGHT_CHROMA)
    }
}

/// Hue rotation (degrees) from an accent to its gradient partner.
pub const PARTNER_HUE_DEG: f32 = 25.0;
/// Chroma boost of the gradient partner.
const PARTNER_CHROMA: f32 = 1.12;

/// The hue-rotated partner of an accent used as a gradient end.
pub fn partner(c: Color) -> Color {
    c.rotate_hue(PARTNER_HUE_DEG).scale_chroma(PARTNER_CHROMA)
}

/// Specular rim alphas `(light edge, shade edge)` on dark glass.
const RIM_DARK: (f32, f32) = (0.10, 0.28);
/// Specular rim alphas `(light edge, shade edge)` on light paper.
const RIM_LIGHT: (f32, f32) = (0.7, 0.06);

/// Subtle 1 px specular rim (top-left light, bottom shade) for cards.
pub fn rim(palette: &MandaraPalette) -> Vec<BoxShadow> {
    if palette.is_dark {
        glass_rim(palette.specular_rim_light.with_alpha(RIM_DARK.0), palette.specular_rim_shadow.with_alpha(RIM_DARK.1))
    } else {
        glass_rim(Color::oklch(1.0, 0.0, 0.0).with_alpha(RIM_LIGHT.0), palette.specular_rim_shadow.with_alpha(RIM_LIGHT.1))
    }
}

/// Hairline edge alpha of a floating surface `(dark, light)`.
const SURFACE_EDGE_ALPHA: (f32, f32) = (0.085, 0.10);
/// Upper bound of the theme's rim-intensity multiplier.
const MAX_RIM_INTENSITY: f32 = 2.0;

/// The edge of a floating surface (dropdown panel): one even hairline of
/// constant opacity all the way round — no directional highlight that fades
/// along the curve and reads as patchy over busy wallpapers. Drawn as the
/// surface's 1 px border (an SDF quad edge, blended over the fill).
/// `intensity` is the theme's rim-intensity adjustment.
pub fn surface_edge(palette: &MandaraPalette, intensity: f32) -> Color {
    let k = if crate::style::borders() { intensity.clamp(0.0, MAX_RIM_INTENSITY) } else { 0.0 };
    palette.on_surface.with_alpha(alpha(SURFACE_EDGE_ALPHA, palette) * k)
}

/// Lightness of the shadow ink under floating panels.
const SHADOW_INK_L: f32 = 0.12;
/// Share of the surface chroma the shadow ink keeps, and its cap.
const SHADOW_INK_CHROMA: (f32, f32) = (0.6, 0.03);
/// `(contact, ambient)` shadow alphas on dark and on light themes.
const SHADOW_ALPHA_DARK: (f32, f32) = (0.30, 0.34);
const SHADOW_ALPHA_LIGHT: (f32, f32) = (0.07, 0.11);
/// Contact shadow: y offset and blur.
const CONTACT_DY_PX: f32 = 1.0;
const CONTACT_BLUR_PX: f32 = 2.5;
/// Ambient shadow: base alpha share, y offset / blur at rest and their
/// growth as the panel lifts, and the inward spread.
const AMBIENT_BASE: f32 = 0.4;
const AMBIENT_DY_PX: (f32, f32) = (2.0, 4.0);
const AMBIENT_BLUR_PX: (f32, f32) = (8.0, 8.0);
const AMBIENT_SPREAD_PX: f32 = -2.0;

/// Elevation under a floating panel: a tight contact shadow plus a soft
/// ambient one, both short enough to fade out completely (no clipped
/// shadow edges) and tinted toward the surface hue. `progress` (`0..=1`)
/// lifts the panel as it opens out of its pill.
pub fn elevation(palette: &MandaraPalette, progress: f32) -> Vec<BoxShadow> {
    elevation_scaled(palette, progress, 1.0)
}

/// [`elevation`] with its alphas multiplied by `strength` (`[panels]
/// shadow_strength`, `bar.shadow_strength`); none at 0.
pub fn elevation_scaled(palette: &MandaraPalette, progress: f32, strength: f32) -> Vec<BoxShadow> {
    let strength = mandara_config::surfaces::ranges::INTENSITY.clamp(strength, 1.0);
    if !crate::style::shadows() || strength <= 0.0 {
        return Vec::new();
    }
    let p = progress.clamp(0.0, 1.0);
    let ink = Color::oklch(SHADOW_INK_L, (palette.surface_base.chroma() * SHADOW_INK_CHROMA.0).min(SHADOW_INK_CHROMA.1), palette.surface_base.hue());
    let (contact, ambient) = if palette.is_dark { SHADOW_ALPHA_DARK } else { SHADOW_ALPHA_LIGHT };
    let (contact, ambient) = ((contact * strength).min(1.0), (ambient * strength).min(1.0));
    vec![
        BoxShadow {
            color: ink.with_alpha(contact).hsla(),
            offset: point(px(0.0), px(CONTACT_DY_PX)),
            blur_radius: px(CONTACT_BLUR_PX),
            spread_radius: px(0.0),
            inset: false,
        },
        BoxShadow {
            color: ink.with_alpha(ambient * (AMBIENT_BASE + (1.0 - AMBIENT_BASE) * p)).hsla(),
            offset: point(px(0.0), px(AMBIENT_DY_PX.0 + AMBIENT_DY_PX.1 * p)),
            blur_radius: px(AMBIENT_BLUR_PX.0 + AMBIENT_BLUR_PX.1 * p),
            spread_radius: px(AMBIENT_SPREAD_PX),
            inset: false,
        },
    ]
}

/// Vertical sheen of a glass card: `(top lift, bottom drop)` in OKLCH
/// lightness on dark and on light themes.
const SHEEN_DARK: (f32, f32) = (0.018, -0.008);
const SHEEN_LIGHT: (f32, f32) = (0.012, -0.006);
/// Gradient angle (degrees) for top → bottom fills.
pub const ANGLE_DOWN_DEG: f32 = 180.0;
/// Gradient angle (degrees) for left → right fills.
pub const ANGLE_RIGHT_DEG: f32 = 90.0;
/// Half a turn: flips a gradient angle for right-to-left layouts.
pub const HALF_TURN_DEG: f32 = 180.0;
/// Gradient angle (degrees) for diagonal fills.
pub const ANGLE_DIAGONAL_DEG: f32 = 135.0;

/// A chromatic-glass card: tonal container fill with a faint vertical sheen
/// and the specular rim.
pub fn glass_card(palette: &MandaraPalette) -> Div {
    let base = palette.surface_container;
    let (top, bottom) = if palette.is_dark { SHEEN_DARK } else { SHEEN_LIGHT };
    // Children (art, hover fills, bands) are clipped to the rounded corners.
    crate::style::ClipRounded::clip_rounded(div().rounded(px(crate::style::radius(CARD_RADIUS_PX))).bg(oklab_gradient(ANGLE_DOWN_DEG, base.lighten(top), base.lighten(bottom))).shadow(rim(palette)))
}

/// A soft coloured glow behind an element (SDF shadow, no hard edge).
pub fn glow(color: Color, blur: f32, spread: f32, dy: f32) -> BoxShadow {
    BoxShadow { color: color.hsla(), offset: point(px(0.0), px(dy)), blur_radius: px(blur), spread_radius: px(spread), inset: false }
}

/// Small uppercase-ish section label with an optional trailing element.
pub fn section(label: &'static str, palette: &MandaraPalette, trailing: Option<AnyElement>) -> Div {
    div()
        .row()
        .items_center()
        .justify_between()
        .h(px(SECTION_H_PX))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(label))
        .when_some(trailing, |el, t| el.child(t))
}

/// A tiny pill with an icon and a tabular value (`up 4d 2h`, `Qibla 136°`).
pub fn chip(icon_name: &'static str, value: impl Into<SharedString>, tint: Color, palette: &MandaraPalette) -> Div {
    div()
        .h(px(CHIP_H_PX))
        .px(px(CHIP_PAD_X_PX))
        .rounded_full()
        .bg(wash(WASH_SOFT, palette).hsla())
        .row()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .flex_none()
        .font_features(tabular_nums())
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(icon(icon_name, SMALL_ICON_PX, tint))
        .child(value.into())
}

// ── Sample history & graphs ──────────────────────────────────────────────

/// Fixed-capacity sample history (oldest first).
#[derive(Clone, Debug)]
pub struct History {
    values: VecDeque<f32>,
    capacity: usize,
    last_push: Option<Instant>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self { values: VecDeque::with_capacity(capacity), capacity, last_push: None }
    }

    /// Appends `v` unless the previous sample is younger than `min_gap`.
    pub fn push_every(&mut self, v: f32, min_gap: Duration, now: Instant) -> bool {
        if self.last_push.is_some_and(|t| now.duration_since(t) < min_gap) {
            return false;
        }
        self.last_push = Some(now);
        if self.values.len() == self.capacity {
            self.values.pop_front();
        }
        self.values.push_back(v.clamp(0.0, 1.0));
        true
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        self.values.iter().copied()
    }

    pub fn peak(&self) -> f32 {
        self.values.iter().copied().fold(0.0, f32::max)
    }

    pub fn mean(&self) -> f32 {
        if self.values.is_empty() { 0.0 } else { self.values.iter().sum::<f32>() / self.values.len() as f32 }
    }
}

/// Sparkline well: inner padding, corner radius, column gap / radius /
/// minimum height, and how much the oldest column fades.
const SPARK_PAD_PX: f32 = 3.0;
const SPARK_RADIUS_PX: f32 = 8.0;
const SPARK_GAP_PX: f32 = 2.0;
const SPARK_COL_RADIUS_PX: f32 = 1.5;
const SPARK_COL_MIN_PX: f32 = 2.0;
const SPARK_AGE_FADE: f32 = 0.6;

/// A bar-column sparkline drawn with rounded SDF quads (no tessellated
/// paths). Columns are right-aligned so the newest sample sits at the right
/// edge; older columns fade out, and each column's colour blends from
/// `from` to `to` with its value in OKLab.
pub fn sparkline(history: &History, height: f32, from: Color, to: Color, palette: &MandaraPalette) -> Div {
    let cap = history.capacity();
    let len = history.len();
    let inner = height - SPARK_PAD_PX * 2.0;
    let columns = (0..cap).map(move |slot| {
        // Slots before the first sample stay empty.
        let sample = slot.checked_sub(cap - len).map(|i| history.values[i]);
        let age = 1.0 - slot as f32 / cap.max(1) as f32; // 1 = oldest
        let col = div().flex_1().min_w(px(1.0)).h_full().flex().flex_col().justify_end();
        match sample {
            Some(v) => {
                let h = (v * inner).max(SPARK_COL_MIN_PX);
                let c = from.mix(to, v).with_alpha(1.0 - age * SPARK_AGE_FADE);
                col.child(div().w_full().h(px(h)).rounded(px(crate::style::radius(SPARK_COL_RADIUS_PX))).bg(c.hsla()))
            }
            None => col,
        }
    });
    div()
        .h(px(height))
        .w_full()
        .p(px(SPARK_PAD_PX))
        .rounded(px(crate::style::radius(SPARK_RADIUS_PX)))
        .bg(wash(WASH_FAINT, palette).hsla())
        .flex()
        .items_end()
        .gap(px(SPARK_GAP_PX))
        .children(columns)
}

/// A horizontal meter: rounded track plus an OKLab gradient fill.
pub fn meter(value: f32, height: f32, from: Color, to: Color, palette: &MandaraPalette) -> Div {
    let v = value.clamp(0.0, 1.0);
    div()
        .w_full()
        .h(px(height))
        .rounded_full()
        .bg(wash(WASH_TRACK, palette).hsla())
        .child(div().h_full().w(gpui::relative(v)).min_w(px(if v > 0.0 { height } else { 0.0 })).rounded_full().bg(oklab_gradient(ANGLE_RIGHT_DEG, from, to)))
}

/// The leading dot of a [`dot_ring`] is this much larger than the others.
const RING_HEAD_SCALE: f32 = 1.9;
/// Glow under the leading dot: alpha and blur.
const RING_HEAD_GLOW: (f32, f32) = (0.55, 8.0);

/// A ring of `count` dots (SDF circles) filled clockwise from 12 o'clock up
/// to `frac`, coloured `from → to` along the lit arc; the leading dot is
/// larger and glows. `inner` is centred inside the ring.
#[allow(clippy::too_many_arguments)]
pub fn dot_ring(frac: f32, diameter: f32, count: usize, dot: f32, from: Color, to: Color, track: Color, inner: AnyElement) -> Div {
    let frac = frac.clamp(0.0, 1.0);
    let half = diameter / 2.0;
    let r = half - dot;
    let lit = (frac * count as f32).round() as usize;
    let head = lit.saturating_sub(1);
    let dots = (0..count).map(move |i| {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / count as f32;
        let on = i < lit;
        let is_head = on && i == head;
        let d = if is_head { dot * RING_HEAD_SCALE } else { dot };
        let c = if on { from.mix(to, i as f32 / count.max(1) as f32) } else { track };
        let (x, y) = (half + r * a.cos() - d / 2.0, half + r * a.sin() - d / 2.0);
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .size(px(d))
            .rounded_full()
            .bg(c.hsla())
            .when(is_head, |el| el.shadow(vec![glow(c.with_alpha(RING_HEAD_GLOW.0), RING_HEAD_GLOW.1, 1.0, 0.0)]))
    });
    div()
        .relative()
        .size(px(diameter))
        .flex_none()
        .children(dots)
        .child(div().absolute().top_0().left_0().size_full().flex().flex_col().items_center().justify_center().child(inner))
}

// ── Shared panel components ──────────────────────────────────────────────

/// Icon chip gradient alphas `(accent, partner)`.
const ICON_CHIP_FILL: (f32, f32) = (0.22, 0.12);

/// Gradient icon chip used at the head of every dashboard panel.
pub fn icon_chip(icon_name: &'static str, accent: Color, side: f32) -> Div {
    div()
        .size(px(side))
        .flex_none()
        .rounded(px(crate::style::radius(side * ICON_CHIP_RADIUS_FRAC)))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(ICON_CHIP_FILL.0), partner(accent).with_alpha(ICON_CHIP_FILL.1)))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(icon_name, side * ICON_CHIP_GLYPH_FRAC, accent))
}

/// Panel header: icon chip, title + subtitle and optional trailing controls.
pub fn header(
    icon_name: &'static str,
    accent: Color,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    palette: &MandaraPalette,
    trailing: Option<AnyElement>,
) -> Div {
    div()
        .h(px(HEADER_H))
        .row()
        .items_center()
        .gap(px(HEADER_GAP_PX))
        .child(icon_chip(icon_name, accent, HEADER_CHIP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().truncate().child(title.into()))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(subtitle.into())),
        )
        .when_some(trailing, |el, t| el.child(t))
}

/// Switch geometry: track width / height, inner padding and knob side.
const SWITCH_W_PX: f32 = 42.0;
const SWITCH_H_PX: f32 = 24.0;
const SWITCH_PAD_PX: f32 = 3.0;
const SWITCH_KNOB_PX: f32 = SWITCH_H_PX - SWITCH_PAD_PX * 2.0;
/// Knob alpha while off, and the on-state knob glow (alpha, blur).
const SWITCH_KNOB_OFF_ALPHA: f32 = 0.8;
const SWITCH_GLOW: (f32, f32) = (0.35, 6.0);

/// A pill switch with a gradient track when on.
pub fn switch(on: bool, palette: &MandaraPalette) -> Div {
    let accent = readable(palette.primary, palette);
    let knob = if on { palette.on_primary } else { palette.on_surface_variant.with_alpha(SWITCH_KNOB_OFF_ALPHA) };
    div()
        .w(px(SWITCH_W_PX))
        .h(px(SWITCH_H_PX))
        .flex_none()
        .rounded_full()
        .p(px(SWITCH_PAD_PX))
        // Mirrored in right-to-left languages: "on" puts the knob at the left.
        .row()
        .when(on, |el| el.justify_end().bg(oklab_gradient(ANGLE_RIGHT_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()))))
        .when(!on, |el| el.bg(wash(WASH_STRONG, palette).hsla()))
        .child(
            div()
                .size(px(SWITCH_KNOB_PX))
                .rounded_full()
                .bg(knob.hsla())
                .when(on, |el| el.shadow(vec![glow(accent.with_alpha(SWITCH_GLOW.0), SWITCH_GLOW.1, 0.0, 1.0)])),
        )
}

/// Signal-bar geometry: row height, bar width / gap, the shortest bar and
/// the step between bars.
const SIGNAL_ROW_PX: f32 = 14.0;
const SIGNAL_BAR_W_PX: f32 = 3.0;
const SIGNAL_GAP_PX: f32 = 2.0;
const SIGNAL_BASE_H_PX: f32 = 5.0;
const SIGNAL_STEP_PX: f32 = 3.0;
/// Number of bars, and the strength (`0..=100`) at which each further bar
/// lights.
const SIGNAL_BARS: usize = 4;
const SIGNAL_THRESHOLDS: [u8; SIGNAL_BARS - 1] = [20, 45, 70];

/// Four rising signal bars for a `0..=100` strength (SDF quads).
pub fn signal_bars(strength: u8, color: Color, palette: &MandaraPalette) -> Div {
    let lit = 1 + SIGNAL_THRESHOLDS.iter().filter(|t| strength >= **t).count();
    let off = wash(WASH_IDLE_BAR, palette);
    div().h(px(SIGNAL_ROW_PX)).flex_none().flex().items_end().gap(px(SIGNAL_GAP_PX)).children((0..SIGNAL_BARS).map(move |i| {
        div()
            .w(px(SIGNAL_BAR_W_PX))
            .h(px(SIGNAL_BASE_H_PX + i as f32 * SIGNAL_STEP_PX))
            .rounded_full()
            .bg(if i < lit { color.hsla() } else { off.hsla() })
    }))
}

/// A soft tonal pill button (icon + label).
pub fn pill_button(
    id: impl Into<gpui::ElementId>,
    icon_name: Option<&'static str>,
    label: impl Into<SharedString>,
    tint: Color,
    palette: &MandaraPalette,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Stateful<Div> {
    use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};
    let bg = tint.with_alpha(alpha(TINT_FILL, palette));
    let hover = tint.with_alpha(alpha(TINT_HOVER, palette));
    div()
        .id(id)
        .h(px(PILL_H_PX))
        .px(px(PILL_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .cursor_pointer()
        .bg(bg.hsla())
        .hover(move |s| s.bg(hover.hsla()))
        .text_size(px(PILL_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .when_some(icon_name, |el, i| el.child(icon(i, PILL_ICON_PX, tint)))
        .child(label.into())
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// Empty-state rings: outer diameter, the three ring diameters with their
/// alphas, the glyph size and the gap above the title.
const EMPTY_RINGS_PX: f32 = 76.0;
const EMPTY_RING_D_PX: [f32; 3] = [76.0, 56.0, 38.0];
const EMPTY_RING_ALPHA: [f32; 3] = [0.05, 0.08, 0.14];
const EMPTY_ICON_PX: f32 = 19.0;
const EMPTY_GAP_PX: f32 = 4.0;
const EMPTY_TITLE_GAP_PX: f32 = 8.0;

/// A designed empty state: concentric tinted rings around an icon, a title
/// and a hint, centred in `height`.
pub fn empty_state(icon_name: &'static str, title: impl Into<SharedString>, hint: impl Into<SharedString>, height: f32, palette: &MandaraPalette) -> Div {
    let accent = readable(palette.primary, palette);
    let rings = div().relative().size(px(EMPTY_RINGS_PX)).flex().items_center().justify_center().children(
        EMPTY_RING_D_PX.into_iter().zip(EMPTY_RING_ALPHA).map(move |(d, a)| {
            let inset = (EMPTY_RINGS_PX - d) / 2.0;
            div().absolute().left(px(inset)).top(px(inset)).size(px(d)).rounded_full().bg(accent.with_alpha(a).hsla())
        }),
    );
    div()
        .h(px(height))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(EMPTY_GAP_PX))
        .child(rings.child(icon(icon_name, EMPTY_ICON_PX, accent)))
        .child(div().mt(px(EMPTY_TITLE_GAP_PX)).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
        .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(hint.into()))
}

/// Big slider geometry: track height, the nominal track width used to tell
/// when the fill reaches past the icon, the icon inset / size, and the end
/// handle (inset from top and bottom, width, offset from the fill's end,
/// alpha, hide-below).
const SLIDER_H_PX: f32 = 40.0;
const SLIDER_NOMINAL_W_PX: f32 = 260.0;
const SLIDER_ICON_INSET_PX: f32 = 12.0;
const SLIDER_ICON_PX: f32 = 17.0;
/// The icon sits on the fill once the fill reaches this far.
const SLIDER_ICON_COVER_PX: f32 = 34.0;
const SLIDER_HANDLE_INSET_PX: f32 = 11.0;
const SLIDER_HANDLE_W_PX: f32 = 4.0;
const SLIDER_HANDLE_OFFSET_PX: f32 = -10.0;
const SLIDER_HANDLE_ALPHA: f32 = 0.7;
const SLIDER_HANDLE_HIDE_BELOW: f32 = 0.06;
/// From the fill's end back to the handle's centre.
const SLIDER_HANDLE_CENTER_INSET_PX: f32 = -SLIDER_HANDLE_OFFSET_PX - SLIDER_HANDLE_W_PX / 2.0;
/// Where values sit on the big slider: the fill is never shorter than its
/// round cap (one track height, holding the icon) and grows by the value
/// over the rest of the track, so the handle's centre runs from the cap's
/// end at 0 to the track's end at 1 and a press or drag puts it under the
/// pointer.
const BIG_SLIDER_TRACK: crate::slider_model::Track =
    crate::slider_model::Track { start_inset: SLIDER_H_PX - SLIDER_HANDLE_CENTER_INSET_PX, end_inset: SLIDER_HANDLE_CENTER_INSET_PX };
/// Muted fill alphas `(start, end)`.
const SLIDER_MUTED_FILL: (f32, f32) = (0.45, 0.3);

/// A thick Caelestia-style slider: a rounded pill whose gradient fill grows
/// with `value` (`0..=1`), the icon riding inside at the start. A press
/// jumps to the value under the pointer and drags until release, even
/// outside the pill; while held it draws your value, not the owner's echoes
/// ([`crate::slider_model`]). `on_change` writes a value and returns the
/// write's completion (`None` for a local value set at once, or nothing).
/// Mirrored (fills from the right) in right-to-left languages.
pub fn big_slider<R: crate::slider_model::IntoWrite>(
    id: impl Into<gpui::ElementId>,
    value: f32,
    icon_name: &'static str,
    muted: bool,
    palette: &MandaraPalette,
    on_change: impl Fn(f32, &mut Window, &mut App) -> R + 'static,
) -> gpui::Stateful<Div> {
    use gpui::InteractiveElement as _;
    let id = id.into();
    let v = crate::slider_model::shown(&id, value).clamp(0.0, 1.0);
    let rtl = crate::bidi::rtl();
    let (from, to) = if muted {
        (palette.on_surface_variant.with_alpha(SLIDER_MUTED_FILL.0), palette.on_surface_variant.with_alpha(SLIDER_MUTED_FILL.1))
    } else {
        (palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()))
    };
    let on_fill = if muted { palette.on_surface } else { palette.on_primary };
    let icon_color = if v * SLIDER_NOMINAL_W_PX > SLIDER_ICON_COVER_PX { on_fill } else { palette.on_surface_variant };
    // The value's share of the track: everything but one cap at the end;
    // the fill hangs one cap past it, the handle sits just inside the fill.
    let travel = div().absolute().top_0().bottom_0().inset_start(px(0.0)).inset_end(px(SLIDER_H_PX)).child(
        div()
            .relative()
            .h_full()
            .w(gpui::relative(v))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .inset_start(px(0.0))
                    .inset_end(px(-SLIDER_H_PX))
                    .rounded_full()
                    .bg(oklab_gradient(if rtl { ANGLE_RIGHT_DEG + HALF_TURN_DEG } else { ANGLE_RIGHT_DEG }, from, to)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(SLIDER_HANDLE_INSET_PX))
                    .inset_end(px(-(SLIDER_H_PX + SLIDER_HANDLE_OFFSET_PX + SLIDER_HANDLE_W_PX)))
                    .w(px(SLIDER_HANDLE_W_PX))
                    .h(px(SLIDER_H_PX - SLIDER_HANDLE_INSET_PX * 2.0))
                    .rounded_full()
                    .bg(on_fill.with_alpha(SLIDER_HANDLE_ALPHA).hsla())
                    .when(v < SLIDER_HANDLE_HIDE_BELOW, |el| el.opacity(0.0)),
            ),
    );
    let el = div()
        .id(id.clone())
        .relative()
        .h(px(SLIDER_H_PX))
        .w_full()
        .rounded_full()
        .cursor_pointer()
        .bg(wash(WASH_TRACK, palette).hsla())
        .child(travel)
        .child(div().absolute().top_0().inset_start(px(SLIDER_ICON_INSET_PX)).h_full().flex().items_center().child(icon(icon_name, SLIDER_ICON_PX, icon_color)));
    crate::slider_model::bind(el, id, BIG_SLIDER_TRACK, on_change)
}

// ── Live audio spectrum ──────────────────────────────────────────────────

/// After a failed subscribe, wait this long before trying again.
const SPECTRUM_RETRY: Duration = Duration::from_secs(5);
/// How often the watchdog checks whether the panel still renders.
const SPECTRUM_WATCHDOG: Duration = Duration::from_millis(700);
/// A panel that hasn't rendered for this long is closed: drop capture.
const SPECTRUM_IDLE: Duration = Duration::from_millis(1200);

/// Holds a [`mandara_services::spectrum::Spectrum`] subscription only while
/// a panel is on screen: [`SpectrumTap::bands`] subscribes on demand and a
/// watchdog drops the handle (stopping capture) once the panel hasn't
/// rendered for a moment, i.e. it was closed.
#[derive(Default)]
pub struct SpectrumTap {
    tap: Option<mandara_services::spectrum::Spectrum>,
    tried: Option<Instant>,
    last_render: Option<Instant>,
    watchdog: bool,
}

impl SpectrumTap {
    /// Current bands while `want` and sound is playing; `None` otherwise.
    pub fn bands(this: &Rc<RefCell<SpectrumTap>>, want: bool, cx: &mut App) -> Option<[f32; mandara_services::spectrum::BANDS]> {
        let mut s = this.borrow_mut();
        if !want {
            s.tap = None;
            return None;
        }
        s.last_render = Some(Instant::now());
        if s.tap.is_none() && s.tried.is_none_or(|t| t.elapsed() > SPECTRUM_RETRY) {
            s.tried = Some(Instant::now());
            s.tap = mandara_services::spectrum::subscribe();
        }
        if s.tap.is_some() && !s.watchdog {
            s.watchdog = true;
            let this = this.clone();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(SPECTRUM_WATCHDOG).await;
                    let mut s = this.borrow_mut();
                    if s.tap.is_none() || s.last_render.is_none_or(|t| t.elapsed() > SPECTRUM_IDLE) {
                        s.tap = None;
                        s.watchdog = false;
                        break;
                    }
                }
            })
            .detach();
        }
        s.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands())
    }
}

/// Spectrum bars: perceptual curve on levels, minimum bar height / width,
/// and the alpha range from quiet to loud.
const SPECTRUM_GAMMA: f32 = 0.85;
const SPECTRUM_MIN_H_PX: f32 = 3.0;
const SPECTRUM_MIN_W_PX: f32 = 2.0;
const SPECTRUM_ALPHA: (f32, f32) = (0.55, 0.45);

/// The audio visualizer: one mirrored, rounded bar per band, tinted along
/// `from → to`. `bands` are live levels `0..=1`; an empty slice draws the
/// resting baseline (silence / paused / no capture) — never fake motion.
pub fn spectrum_bars(bands: &[f32], from: Color, to: Color, palette: &MandaraPalette, height: f32, gap: f32) -> Div {
    let n = if bands.is_empty() { mandara_services::spectrum::BANDS } else { bands.len() };
    let rest = wash(WASH_IDLE_BAR, palette);
    div().h(px(height)).w_full().flex().items_center().gap(px(gap)).children((0..n).map(move |i| {
        let x = i as f32 / (n - 1).max(1) as f32;
        let (h, c) = match bands.get(i) {
            Some(v) => {
                let v = v.clamp(0.0, 1.0).powf(SPECTRUM_GAMMA);
                ((v * height).max(SPECTRUM_MIN_H_PX), from.mix(to, x).with_alpha(SPECTRUM_ALPHA.0 + SPECTRUM_ALPHA.1 * v))
            }
            None => (SPECTRUM_MIN_H_PX, rest),
        };
        div().flex_1().min_w(px(SPECTRUM_MIN_W_PX)).h(px(h)).rounded_full().bg(c.hsla())
    }))
}

/// Code point of ARABIC-INDIC DIGIT ZERO (٠).
const ARABIC_INDIC_ZERO: u32 = 0x0660;

/// Converts ASCII digits to Arabic-Indic digits (٠١٢٣٤٥٦٧٨٩).
pub fn arabic_digits(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_digit() { char::from_u32(ARABIC_INDIC_ZERO + (c as u32 - '0' as u32)).unwrap_or(c) } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded_and_rate_limited() {
        let mut h = History::new(3);
        let t = Instant::now();
        assert!(h.push_every(0.2, Duration::from_secs(1), t));
        assert!(!h.push_every(0.9, Duration::from_secs(1), t + Duration::from_millis(500)));
        for (i, v) in [0.4, 0.6, 1.4].into_iter().enumerate() {
            assert!(h.push_every(v, Duration::from_secs(1), t + Duration::from_secs(i as u64 + 1)));
        }
        assert_eq!(h.iter().collect::<Vec<_>>(), vec![0.4, 0.6, 1.0]);
        assert_eq!(h.peak(), 1.0);
        assert!((h.mean() - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn arabic_indic_digits() {
        assert_eq!(arabic_digits("1448"), "١٤٤٨");
        assert_eq!(arabic_digits("4 x"), "٤ x");
    }

    #[test]
    fn percent_labels_are_cached_and_clamped() {
        assert_eq!(percent_text(0.424).as_ref(), "42%");
        assert_eq!(percent_text(1.7).as_ref(), "100%");
        assert_eq!(percent_text(-0.2).as_ref(), "0%");
    }

    #[test]
    fn cached_label_formats_only_on_change() {
        let mut c = Cached::default();
        let mut calls = 0;
        for v in [3, 3, 4] {
            c.get(v, |v| {
                calls += 1;
                v.to_string()
            });
        }
        assert_eq!(calls, 2);
    }

    #[test]
    fn signal_thresholds_match_strength_bands() {
        let lit = |s: u8| 1 + SIGNAL_THRESHOLDS.iter().filter(|t| s >= **t).count();
        assert_eq!([lit(0), lit(19), lit(20), lit(44), lit(45), lit(69), lit(70), lit(100)], [1, 1, 2, 2, 3, 3, 4, 4]);
    }
}
