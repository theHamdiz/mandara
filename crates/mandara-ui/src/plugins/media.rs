//! Media plugin (MPRIS2): now-playing in the bar; a Caelestia-style media
//! island panel themed entirely by the current artwork (surface tint, glow,
//! accents, scrubber and play button derived from the album-art palette in
//! OKLCH, crossfading between tracks), a blurred art backdrop, a live audio
//! spectrum visualizer fed by `mandara_services::spectrum`, a draggable seek
//! scrubber and transport controls; a compact sidebar card.
//!
//! Performance notes: every view shares one [`Snap`] of the player (cloned
//! from the hub only when it changes, with its display strings prepared
//! once); the art theme is cached per artwork + palette; album art is
//! decoded once per track off the UI thread at the size it is drawn (never
//! through GPUI's path-keyed image cache, which keeps every cover ever shown
//! decoded); and panel-only state (decoded art, lyrics, labels) is freed a
//! moment after the panel or card stops rendering.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWindowHandle, App, FontWeight, ImageSource, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, canvas,
    div, img, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, size,
};
use image::imageops::FilterType;
use mandara_core::{ToGpui, oklab_gradient};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_i18n::{t, tf, tk};
use mandara_services::{MediaCommand, MediaState, PlaybackStatus};
use mandara_theme::{Color, MandaraPalette, MiniPalette, SeedColor};
use serde_json::json;

use super::fn_view;
pub(crate) mod lyrics_view;
use lyrics_view::LyricsPane;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, ANGLE_RIGHT_DEG, Cached, PanelShape, SpectrumTap, alpha, glow, readable, redraw_in, wash,
};
use crate::fonts::tabular_nums;
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{card, ellipsize, format_duration, reveal, text, tokens};

pub const ID: &str = "media";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 420.0;
const PAD: f32 = 18.0;
const HEAD_H: f32 = 26.0;
const ART: f32 = 136.0;
const VIZ_H: f32 = 40.0;
const SCRUB_H: f32 = 38.0;
const TRANSPORT_H: f32 = 60.0;
/// Gaps between the panel's rows (head → info → visualizer → scrubber →
/// transport).
const HEAD_GAP: f32 = 14.0;
const INFO_GAP: f32 = 12.0;
const VIZ_GAP: f32 = 8.0;
const SCRUB_GAP: f32 = 4.0;
/// The playing panel without lyrics: every row has a fixed height (the
/// title clamps to [`TITLE_MAX_LINES`], the info row is the art box).
const PLAYING_H: f32 = PAD * 2.0 + HEAD_H + HEAD_GAP + ART + INFO_GAP + VIZ_H + VIZ_GAP + SCRUB_H + SCRUB_GAP + TRANSPORT_H;
const EMPTY_H: f32 = 216.0;
/// Space above the lyrics pane.
const LYRICS_GAP: f32 = 12.0;
/// Panel size changes smaller than this are ignored (no relayout jitter).
const SIZE_EPSILON_PX: f32 = 0.5;

/// Head: player pill padding / gap, its icon, and the level glyph.
const HEAD_ROW_GAP_PX: f32 = 8.0;
const PLAYER_PAD_L_PX: f32 = 4.0;
const PLAYER_PAD_R_PX: f32 = 10.0;
const PLAYER_GAP_PX: f32 = 6.0;
const PLAYER_ICON_PX: f32 = 18.0;
const PLAYER_GLYPH_PX: f32 = 11.0;
const PLAYER_GLYPH_BG_ALPHA: f32 = 0.2;
/// Player pill wash `(dark, light)`.
const PLAYER_WASH: (f32, f32) = (0.08, 0.06);
const LEVEL_H_PX: f32 = 12.0;
const LEVEL_BAR_W_PX: f32 = 3.0;
const LEVEL_GAP_PX: f32 = 2.0;
/// Level bar height at silence (share of the glyph) and its minimum.
const LEVEL_FLOOR: f32 = 0.25;
const LEVEL_MIN_PX: f32 = 3.0;
/// Level glyph bands: low, mid and high thirds of the spectrum.
const LEVEL_BANDS: usize = 3;

/// Characters kept of the title in the bar chip and of the player name.
const CHIP_TITLE_CHARS: usize = 34;
const PLAYER_NAME_CHARS: usize = 24;
/// Bar chip: gap, icon size and title width cap.
const CHIP_GAP_PX: f32 = 7.0;
const CHIP_ICON_PX: f32 = 13.0;
const CHIP_TITLE_MAX_W_PX: f32 = 260.0;

/// Info row: gap, art radius and glow (blur, spread, y offset), text sizes.
const INFO_ROW_GAP_PX: f32 = 16.0;
const ART_RADIUS_PX: f32 = 20.0;
const ART_GLOW: (f32, f32, f32) = (22.0, -4.0, 8.0);
const INFO_TEXT_GAP_PX: f32 = 4.0;
const TITLE_PX: f32 = 18.0;
/// Title line height for Latin / Arabic script.
const TITLE_LINE_PX: f32 = 23.0;
const TITLE_LINE_RTL_PX: f32 = 28.0;
const TITLE_MAX_LINES: usize = 2;
const ALBUM_GAP_PX: f32 = 5.0;
const ALBUM_ICON_PX: f32 = 11.0;
const ALBUM_ICON_ALPHA: f32 = 0.7;
/// Placeholder art: gradient alphas and glyph size share.
const ART_PLACEHOLDER_FILL: (f32, f32) = (0.35, 0.18);
const ART_PLACEHOLDER_GLYPH: f32 = 0.36;

/// Scrubber: track height / knob size (compact, full), label gap, knob
/// headroom, track wash `(dark, light)`, knob colours and glows.
const TRACK_H_PX: (f32, f32) = (4.0, 6.0);
const KNOB_PX: (f32, f32) = (12.0, 15.0);
const SCRUB_LABEL_GAP_PX: (f32, f32) = (3.0, 5.0);
const KNOB_HEADROOM_PX: f32 = 4.0;
const SCRUB_TRACK_WASH: (f32, f32) = (0.12, 0.1);
/// Knob OKLCH lightness / chroma on dark (tinted by the accent hue) and
/// on light themes.
const KNOB_DARK: (f32, f32) = (0.97, 0.01);
const KNOB_LIGHT_L: f32 = 0.99;
const KNOB_GLOW: (f32, f32) = (0.5, 8.0);
const KNOB_SHADOW: (f32, f32, f32) = (0.2, 3.0, 1.0);
/// Shown instead of the remaining time when the length is unknown.
const UNKNOWN_TIME: &str = "--:--";

/// Transport: side button size, icon share and disabled alpha; gaps and
/// play button glow as shares of the play button size; contrast inks.
const SIDE_BUTTON_SCALE: f32 = 0.74;
const BUTTON_ICON_SCALE: f32 = 0.42;
const PLAY_ICON_SCALE: f32 = 0.4;
const DISABLED_ALPHA: f32 = 0.3;
const BUTTON_HOVER_ALPHA: f32 = 0.08;
const TRANSPORT_GAP_SCALE: f32 = 0.32;
const PLAY_GLOW_BLUR_SCALE: f32 = 0.36;
const PLAY_GLOW_SPREAD_PX: f32 = -2.0;
const PLAY_GLOW_DY_SCALE: f32 = 0.1;
const PLAY_HOVER_OPACITY: f32 = 0.9;
const PLAY_PANEL_PX: f32 = 56.0;
const PLAY_SIDEBAR_PX: f32 = 40.0;
/// Light / dark ink choices on the play button (OKLCH L, C).
const INK_LIGHT: (f32, f32) = (0.99, 0.0);
const INK_DARK: (f32, f32) = (0.17, 0.02);

/// Empty state: ring diameters with alphas, glyph size, title gap.
const EMPTY_RINGS_PX: f32 = 96.0;
const EMPTY_RING_D_PX: [f32; 3] = [96.0, 72.0, 50.0];
const EMPTY_RING_ALPHA: [f32; 3] = [0.05, 0.08, 0.14];
const EMPTY_ICON_PX: f32 = 22.0;
const EMPTY_GAP_PX: f32 = 4.0;
const EMPTY_TITLE_GAP_PX: f32 = 10.0;

/// Backdrop: blurred art alpha `(dark, light)`, the bottom settle
/// gradient's alpha, the attached-panel seam extent and grain.
const BACKDROP_ALPHA: (f32, f32) = (0.42, 0.34);
const SETTLE_ALPHA: f32 = 0.7;
const SEAM_STOP: f32 = 0.3;
/// Gradient angle pointing up (attached bottom bars).
const ANGLE_UP_DEG: f32 = 0.0;

/// Sidebar card: art size / radius.
const SIDE_ART_PX: f32 = 52.0;
const SIDE_ART_RADIUS_PX: f32 = 12.0;
/// How far the card's tonal fill leans toward the art tint (top, bottom).
const SIDE_TINT: (f32, f32) = (0.55, 0.4);

// ── Timing ───────────────────────────────────────────────────────────────

/// Track-change crossfade of the art theme and backdrop.
const FADE: Duration = Duration::from_millis(650);
/// Panel redraw while something moves (spectrum, crossfade, lyrics, drag).
const LIVE_FRAME: Duration = Duration::from_millis(16);
/// Panel redraw while playing without live audio (waits for capture and
/// ticks the time readouts).
const PLAYING_FRAME: Duration = Duration::from_millis(250);
/// Panel redraw while paused.
const PAUSED_FRAME: Duration = Duration::from_secs(1);
/// Sidebar card redraw while playing (its scrubber and times).
const SIDEBAR_TICK: Duration = Duration::from_secs(1);
/// Between tracks some players publish metadata without a title for a
/// moment: the panel keeps showing the last track of the same player that
/// long instead of collapsing to the empty state and growing back.
const TITLE_GAP_HOLD: Duration = Duration::from_millis(1500);
/// How often the release watchdog checks a closed view.
const RELEASE_CHECK: Duration = Duration::from_secs(2);
/// Panel-only state is freed once its view hasn't rendered for this long.
const RELEASE_AFTER: Duration = Duration::from_secs(5);

// ── Art decoding ─────────────────────────────────────────────────────────

/// Backdrop: decoded cover is shrunk to this many pixels a side, then
/// blurred with this sigma; GPU upscaling reads as a deep gaussian blur.
const BACKDROP_PX: u32 = 56;
const BACKDROP_BLUR_SIGMA: f32 = 5.0;

// ── Art theme ────────────────────────────────────────────────────────────

/// Surface tint from the art's dominant colour: `(lightness, chroma share,
/// chroma cap)` of the top and bottom stops on dark and light themes.
const BG_DARK_TOP: (f32, f32, f32) = (0.27, 0.75, 0.085);
const BG_DARK_BOTTOM: (f32, f32, f32) = (0.165, 0.55, 0.06);
const BG_LIGHT_TOP: (f32, f32, f32) = (0.955, 0.35, 0.04);
const BG_LIGHT_BOTTOM: (f32, f32, f32) = (0.9, 0.5, 0.06);
/// Art glow alpha `(dark, light)`.
const ART_GLOW_ALPHA: (f32, f32) = (0.45, 0.35);

pub(crate) fn send(cx: &mut App, command: MediaCommand) {
    if let Some(service) = hub(cx).read(cx).media_service.clone() {
        service.send(command);
    }
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Arabic-script text gets the Arabic face and single-line truncation
/// (wrapped right-to-left runs don't lay out reliably).
fn is_arabic(s: &str) -> bool {
    s.chars().any(|c| matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF))
}

// ── Shared player snapshot ───────────────────────────────────────────────

/// A display string with its script direction, prepared once per change.
#[derive(Clone)]
struct Label {
    text: SharedString,
    arabic: bool,
}

impl Label {
    fn new(s: &str) -> Self {
        Self { text: SharedString::from(s.to_string()), arabic: is_arabic(s) }
    }
}

/// The followed player as every media view draws it: cloned from the hub
/// only when it changed, with its strings prepared once.
struct Snap {
    state: MediaState,
    title: Label,
    artist: Label,
    album: Label,
    /// Ellipsized title for the bar chip.
    chip_title: SharedString,
    /// Ellipsized player name for the panel head.
    player: SharedString,
    /// The player's (non-symbolic) app icon.
    app_icon: Option<PathBuf>,
}

impl Snap {
    fn new(state: &MediaState, previous: Option<&Snap>) -> Self {
        // Icon lookups touch the icon theme on disk: reuse the previous one
        // while the player is the same.
        let app_icon = match previous {
            Some(p) if p.state.identity == state.identity => p.app_icon.clone(),
            _ => crate::apps::icon_for_app_id(&state.identity, false).filter(|r| !r.symbolic).map(|r| r.path),
        };
        Self {
            title: Label::new(&state.title),
            artist: Label::new(&state.artist),
            album: Label::new(&state.album),
            chip_title: ellipsize(&state.title, CHIP_TITLE_CHARS).into(),
            player: ellipsize(&state.identity, PLAYER_NAME_CHARS).into(),
            app_icon,
            state: state.clone(),
        }
    }

    fn playing(&self) -> bool {
        self.state.status == PlaybackStatus::Playing
    }
}

thread_local! {
    static SNAP: RefCell<Option<Rc<Snap>>> = const { RefCell::new(None) };
}

/// The current player snapshot (any title), refreshed only on change.
fn snapshot(cx: &App) -> Option<Rc<Snap>> {
    let live = hub(cx).read(cx).media.as_ref();
    SNAP.with(|s| {
        let mut s = s.borrow_mut();
        match (live, s.as_ref()) {
            (None, _) => {
                *s = None;
                None
            }
            (Some(l), Some(snap)) if snap.state == *l => Some(snap.clone()),
            (Some(l), prev) => {
                let snap = Rc::new(Snap::new(l, prev.map(|p| &**p)));
                *s = Some(snap.clone());
                Some(snap)
            }
        }
    })
}

/// The snapshot when a track with a title is loaded.
fn playing_snapshot(cx: &App) -> Option<Rc<Snap>> {
    snapshot(cx).filter(|s| !s.state.title.is_empty())
}

/// `[plugins.settings.media] lyrics`, read once per config change.
fn lyrics_enabled(cx: &App) -> bool {
    thread_local! {
        static CACHE: RefCell<Option<(Weak<mandara_config::ShellConfig>, bool)>> = const { RefCell::new(None) };
    }
    let cfg = crate::state::config(cx).read(cx).snapshot();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        match c.as_ref() {
            Some((seen, on)) if Weak::ptr_eq(seen, &Arc::downgrade(&cfg)) => *on,
            _ => {
                let on = cfg.plugins.setting(ID, "lyrics", true);
                *c = Some((Arc::downgrade(&cfg), on));
                on
            }
        }
    })
}

// ── Art theme ────────────────────────────────────────────────────────────

/// Colours of the panel derived from the artwork (or the shell palette when
/// there is none), already tuned for the current light / dark mode.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ArtTheme {
    /// Vivid accents (artist text, scrubber, play button, visualizer).
    pub(crate) a: Color,
    pub(crate) b: Color,
    /// Coloured glow under the art and play button.
    pub(crate) glow: Color,
    /// Surface tint gradient (from the dominant colour).
    bg_top: Color,
    bg_bottom: Color,
}

impl ArtTheme {
    fn from_seeds(seeds: &[SeedColor], palette: &MandaraPalette) -> Self {
        let mini = MiniPalette::from_seeds(seeds, palette);
        // The surface takes the art's dominant colour; accents its most
        // chromatic one (MiniPalette's lead) and a hue-distinct partner.
        let dominant = seeds.iter().max_by(|x, y| x.proportion.total_cmp(&y.proportion)).map(|s| s.color).unwrap_or(palette.primary);
        let c = dominant.chroma();
        let stop = |(l, share, cap): (f32, f32, f32)| dominant.with_lightness(l).with_chroma((c * share).min(cap));
        let (bg_top, bg_bottom) = if palette.is_dark { (stop(BG_DARK_TOP), stop(BG_DARK_BOTTOM)) } else { (stop(BG_LIGHT_TOP), stop(BG_LIGHT_BOTTOM)) };
        ArtTheme {
            a: readable(mini.gradient_start, palette),
            b: readable(mini.gradient_end, palette),
            glow: mini.gradient_start.with_alpha(alpha(ART_GLOW_ALPHA, palette)),
            bg_top,
            bg_bottom,
        }
    }

    /// The theme for `state`'s artwork, recomputed only when the artwork
    /// clusters or the palette change (palette extraction is not free and
    /// every media view asks every frame).
    pub(crate) fn of(state: &MediaState, palette: &MandaraPalette) -> Self {
        Self::cached(&state.art_seeds, palette)
    }

    fn cached(seeds: &[SeedColor], palette: &MandaraPalette) -> Self {
        thread_local! {
            static CACHE: RefCell<Option<(Vec<SeedColor>, MandaraPalette, ArtTheme)>> = const { RefCell::new(None) };
        }
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            if let Some((seen, p, theme)) = c.as_ref()
                && seen.as_slice() == seeds
                && p == palette
            {
                return *theme;
            }
            let theme = Self::from_seeds(seeds, palette);
            *c = Some((seeds.to_vec(), palette.clone(), theme));
            theme
        })
    }

    fn lerp(self, o: ArtTheme, t: f32) -> ArtTheme {
        ArtTheme {
            a: self.a.mix(o.a, t),
            b: self.b.mix(o.b, t),
            glow: self.glow.mix(o.glow, t),
            bg_top: self.bg_top.mix(o.bg_top, t),
            bg_bottom: self.bg_bottom.mix(o.bg_bottom, t),
        }
    }
}

// ── Art decoding & lifetime ──────────────────────────────────────────────

fn to_render_image(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    // GPUI expects BGRA.
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))
}

/// Album art decoded once: a square cover at the size it is drawn and,
/// optionally, the blurred backdrop colour field.
#[derive(Default)]
struct ArtImages {
    cover: Option<Arc<RenderImage>>,
    backdrop: Option<Arc<RenderImage>>,
}

/// Decodes `path` once (off the UI thread) into a `cover_px`² cover
/// (centre-cropped like `ObjectFit::Cover`) and, when `blur`, the backdrop.
fn decode_art(path: &Path, cover_px: u32, blur: bool) -> ArtImages {
    let Ok(decoded) = image::open(path) else { return ArtImages::default() };
    let cover = (cover_px > 0).then(|| to_render_image(decoded.resize_to_fill(cover_px, cover_px, FilterType::Triangle).to_rgba8()));
    let backdrop = blur.then(|| {
        let small = decoded.thumbnail(BACKDROP_PX, BACKDROP_PX).to_rgba8();
        to_render_image(image::imageops::blur(&small, BACKDROP_BLUR_SIGMA))
    });
    ArtImages { cover, backdrop }
}

/// Device pixels for a `logical` px image in `window`.
fn device_px(logical: f32, window: &Window) -> u32 {
    (logical * window.scale_factor()).ceil().max(1.0) as u32
}

/// Frees images from the window's sprite atlas (dropping the `Arc` alone
/// would leave them on the GPU).
fn drop_images(window: &mut Window, images: impl IntoIterator<Item = Arc<RenderImage>>) {
    for image in images {
        let _ = window.drop_image(image);
    }
}

/// Frees a view's panel-only state once it hasn't rendered for
/// [`RELEASE_AFTER`] (the panel closed / the card scrolled away). A single
/// watchdog per view runs only while the view is on screen.
#[derive(Default)]
struct IdleRelease {
    last: Cell<Option<Instant>>,
    window: Cell<Option<AnyWindowHandle>>,
    armed: Cell<bool>,
}

impl IdleRelease {
    fn touch(this: &Rc<Self>, window: &Window, cx: &mut App, release: impl FnOnce(&mut Window) + 'static) {
        this.last.set(Some(Instant::now()));
        this.window.set(Some(window.window_handle()));
        if this.armed.replace(true) {
            return;
        }
        let this = this.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(RELEASE_CHECK).await;
                if this.last.get().is_none_or(|t| t.elapsed() >= RELEASE_AFTER) {
                    break;
                }
            }
            this.armed.set(false);
            if let Some(handle) = this.window.take() {
                let _ = handle.update(cx, |_, window, _| release(window));
            }
        })
        .detach();
    }
}

/// Per-panel art state: the art-theme crossfade and the decoded cover +
/// blurred backdrop (current + fading-out previous).
struct Fx {
    from: Option<ArtTheme>,
    to: Option<ArtTheme>,
    since: Instant,
    art_path: Option<PathBuf>,
    cover: Option<Arc<RenderImage>>,
    backdrop: Option<Arc<RenderImage>>,
    previous: Option<Arc<RenderImage>>,
    backdrop_since: Instant,
}

impl Fx {
    fn new() -> Self {
        let now = Instant::now();
        Self { from: None, to: None, since: now, art_path: None, cover: None, backdrop: None, previous: None, backdrop_since: now }
    }

    /// The displayed theme, easing toward `target`; `true` while animating.
    fn theme(&mut self, target: ArtTheme) -> (ArtTheme, bool) {
        let t = ease(self.since.elapsed().as_secs_f32() / FADE.as_secs_f32());
        let shown = match (self.from, self.to) {
            (Some(f), Some(to)) => f.lerp(to, t),
            _ => target,
        };
        if self.to != Some(target) {
            self.from = Some(if self.to.is_some() { shown } else { target });
            self.to = Some(target);
            self.since = Instant::now();
            return (shown, true);
        }
        (shown, t < 1.0)
    }

    /// Frees every decoded image (panel closed); the next open re-decodes.
    fn release(&mut self, window: &mut Window) {
        drop_images(window, [self.cover.take(), self.backdrop.take(), self.previous.take()].into_iter().flatten());
        self.art_path = None;
        self.from = None;
        self.to = None;
    }
}

/// What [`backdrop`] hands the panel for this frame.
struct Backdrop {
    cover: Option<Arc<RenderImage>>,
    current: Option<Arc<RenderImage>>,
    current_t: f32,
    previous: Option<Arc<RenderImage>>,
    previous_t: f32,
    fading: bool,
}

/// Keeps the cover and blurred backdrop in step with `path`: new art is
/// decoded once in the background (the old cover stays until it lands)
/// and the backdrop crossfades.
fn backdrop(fx: &Rc<RefCell<Fx>>, path: Option<&Path>, window: &mut Window, cx: &mut App) -> Backdrop {
    let changed = fx.borrow().art_path.as_deref() != path;
    if changed {
        {
            let mut f = fx.borrow_mut();
            f.art_path = path.map(Path::to_path_buf);
            if path.is_none() {
                let gone = [f.previous.take(), f.cover.take()];
                drop_images(window, gone.into_iter().flatten());
                f.previous = f.backdrop.take();
                f.backdrop_since = Instant::now();
            }
        }
        if let Some(path) = path.map(Path::to_path_buf) {
            let cover_px = device_px(ART, window);
            let task = cx.background_executor().spawn({
                let path = path.clone();
                async move { decode_art(&path, cover_px, true) }
            });
            let handle = window.window_handle();
            let view = window.current_view();
            let fx = fx.clone();
            cx.spawn(async move |cx| {
                let images = task.await;
                let _ = handle.update(cx, |_, window, cx| {
                    let mut f = fx.borrow_mut();
                    if f.art_path.as_deref() != Some(path.as_path()) {
                        // Superseded by another track (or released).
                        drop_images(window, [images.cover, images.backdrop].into_iter().flatten());
                        return;
                    }
                    let gone = [f.previous.take(), std::mem::replace(&mut f.cover, images.cover)];
                    drop_images(window, gone.into_iter().flatten());
                    f.previous = f.backdrop.take();
                    f.backdrop = images.backdrop;
                    f.backdrop_since = Instant::now();
                    cx.notify(view);
                });
            })
            .detach();
        }
    }
    let mut f = fx.borrow_mut();
    let t = ease(f.backdrop_since.elapsed().as_secs_f32() / FADE.as_secs_f32());
    if t >= 1.0
        && let Some(old) = f.previous.take()
    {
        let _ = window.drop_image(old);
    }
    Backdrop { cover: f.cover.clone(), current: f.backdrop.clone(), current_t: t, previous: f.previous.clone(), previous_t: 1.0 - t, fading: t < 1.0 }
}

/// A small decoded cover for the sidebar card (its own size and lifetime).
#[derive(Default)]
pub(crate) struct CoverSlot {
    path: Option<PathBuf>,
    image: Option<Arc<RenderImage>>,
}

impl CoverSlot {
    pub(crate) fn get(this: &Rc<RefCell<Self>>, path: Option<&Path>, side: f32, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
        if this.borrow().path.as_deref() != path {
            {
                let mut s = this.borrow_mut();
                s.path = path.map(Path::to_path_buf);
                if path.is_none() {
                    drop_images(window, s.image.take());
                }
            }
            if let Some(path) = path.map(Path::to_path_buf) {
                let px_side = device_px(side, window);
                let task = cx.background_executor().spawn({
                    let path = path.clone();
                    async move { decode_art(&path, px_side, false).cover }
                });
                let handle = window.window_handle();
                let view = window.current_view();
                let this = this.clone();
                cx.spawn(async move |cx| {
                    let image = task.await;
                    let _ = handle.update(cx, |_, window, cx| {
                        let mut s = this.borrow_mut();
                        if s.path.as_deref() != Some(path.as_path()) {
                            drop_images(window, image);
                            return;
                        }
                        let old = std::mem::replace(&mut s.image, image);
                        drop_images(window, old);
                        cx.notify(view);
                    });
                })
                .detach();
            }
        }
        this.borrow().image.clone()
    }

    pub(crate) fn release(&mut self, window: &mut Window) {
        drop_images(window, self.image.take());
        self.path = None;
    }
}

// ── Pieces ───────────────────────────────────────────────────────────────

/// The cover in a fixed `side` slot; a themed placeholder until (or
/// unless) a decoded cover is ready, so nothing moves when it lands.
pub(crate) fn art(cover: Option<Arc<RenderImage>>, side: f32, radius: f32, m: &ArtTheme) -> AnyElement {
    match cover {
        // The image itself carries the radius: the slot has no clipping
        // container of its own.
        Some(image) => img(ImageSource::Render(image)).size(px(side)).flex_none().rounded(px(radius)).object_fit(ObjectFit::Cover).into_any_element(),
        None => div()
            .size(px(side))
            .flex_none()
            .rounded(px(radius))
            .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, m.a.with_alpha(ART_PLACEHOLDER_FILL.0), m.b.with_alpha(ART_PLACEHOLDER_FILL.1)))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("music", side * ART_PLACEHOLDER_GLYPH, m.a))
            .into_any_element(),
    }
}

/// Small live level glyph for the header: low / mid / high band averages.
fn level_glyph(bands: &[f32], color: Color) -> impl IntoElement {
    let n = bands.len();
    let level = |k: usize| -> f32 {
        let s = &bands[k * n / LEVEL_BANDS..(k + 1) * n / LEVEL_BANDS];
        if s.is_empty() { 0.0 } else { s.iter().sum::<f32>() / s.len() as f32 }
    };
    div().h(px(LEVEL_H_PX)).flex().items_end().gap(px(LEVEL_GAP_PX)).children((0..LEVEL_BANDS).map(move |k| {
        let v = level(k).clamp(0.0, 1.0).sqrt();
        div().w(px(LEVEL_BAR_W_PX)).h(px((LEVEL_H_PX * (LEVEL_FLOOR + (1.0 - LEVEL_FLOOR) * v)).max(LEVEL_MIN_PX))).rounded_full().bg(color.hsla())
    }))
}

fn head(snap: &Snap, bands: Option<&[f32]>, m: &ArtTheme, palette: &MandaraPalette) -> impl IntoElement {
    let playing = snap.playing();
    let status = match snap.state.status {
        PlaybackStatus::Playing => t("Now playing"),
        PlaybackStatus::Paused => t("Paused"),
        PlaybackStatus::Stopped => t("Stopped"),
    };
    div()
        .h(px(HEAD_H))
        .flex()
        .items_center()
        .gap(px(HEAD_ROW_GAP_PX))
        .child(
            div()
                .h(px(HEAD_H))
                .pl(px(PLAYER_PAD_L_PX))
                .pr(px(PLAYER_PAD_R_PX))
                .rounded_full()
                .bg(wash(PLAYER_WASH, palette).hsla())
                .flex()
                .items_center()
                .gap(px(PLAYER_GAP_PX))
                .child(match &snap.app_icon {
                    Some(path) => img(path.clone()).size(px(PLAYER_ICON_PX)).object_fit(ObjectFit::Contain).into_any_element(),
                    None => div()
                        .size(px(PLAYER_ICON_PX))
                        .rounded_full()
                        .bg(m.a.with_alpha(PLAYER_GLYPH_BG_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("music", PLAYER_GLYPH_PX, m.a))
                        .into_any_element(),
                })
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(snap.player.clone())),
        )
        .child(div().flex_1())
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::MEDIUM).text_color(if playing { m.a.hsla() } else { palette.on_surface_variant.hsla() }).child(status))
        .when_some(bands.filter(|_| playing), |el, b| el.child(level_glyph(b, m.a)))
}

fn script_text(label: &Label, arabic_font: &SharedString) -> gpui::Div {
    let el = div().min_w(px(0.0)).child(label.text.clone());
    if label.arabic { el.font_family(arabic_font.clone()).truncate() } else { el }
}

fn info(snap: &Snap, cover: Option<Arc<RenderImage>>, m: &ArtTheme, palette: &MandaraPalette, arabic: &SharedString) -> impl IntoElement {
    let rtl_title = snap.title.arabic;
    div()
        // As tall as the art box whatever the text: a long title, a missing
        // artist or album never changes the panel's layout.
        .h(px(ART))
        .flex()
        .gap(px(INFO_ROW_GAP_PX))
        .items_center()
        // A tight glow that fades out well inside the panel padding.
        .child(div().rounded(px(crate::style::radius(ART_RADIUS_PX))).shadow(vec![glow(m.glow, ART_GLOW.0, ART_GLOW.1, ART_GLOW.2)]).child(art(cover, ART, crate::style::radius(ART_RADIUS_PX), m)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(INFO_TEXT_GAP_PX))
                .child(
                    script_text(&snap.title, arabic)
                        .text_size(px(TITLE_PX))
                        .line_height(px(if rtl_title { TITLE_LINE_RTL_PX } else { TITLE_LINE_PX }))
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(!rtl_title, |el| el.line_clamp(TITLE_MAX_LINES).text_ellipsis()),
                )
                .when(!snap.state.artist.is_empty(), |el| {
                    el.child(script_text(&snap.artist, arabic).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).text_color(m.a.hsla()).truncate())
                })
                .when(!snap.state.album.is_empty(), |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(ALBUM_GAP_PX))
                            .min_w(px(0.0))
                            .child(icon("disk", ALBUM_ICON_PX, palette.on_surface_variant.with_alpha(ALBUM_ICON_ALPHA)))
                            .child(script_text(&snap.album, arabic).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).truncate()),
                    )
                }),
        )
}

/// The scrubber's elapsed / remaining labels, formatted only when the
/// displayed second changes.
#[derive(Default)]
struct ScrubLabels {
    elapsed: Cached<u64>,
    remaining: Cached<Option<u64>>,
}

/// Seek scrubber: click or drag (the knob previews while dragging; the seek
/// is sent on release).
fn scrubber(
    id: &'static str,
    state: &MediaState,
    m: &ArtTheme,
    palette: &MandaraPalette,
    drag: Rc<Cell<Option<f32>>>,
    labels: &RefCell<ScrubLabels>,
    compact: bool,
) -> impl IntoElement {
    let now = Instant::now();
    let length = state.length;
    let seekable = state.can_seek && length.is_some();
    let frac = drag.get().filter(|_| seekable).unwrap_or_else(|| state.progress(now));
    let shown_pos = match (drag.get(), length) {
        (Some(t), Some(len)) if seekable => len.mul_f32(t),
        _ => state.position_now(now),
    };
    let bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>> = Rc::default();
    let pick = {
        let bounds = bounds.clone();
        move |x: gpui::Pixels| -> f32 {
            let b = bounds.get();
            ((f32::from(x) - f32::from(b.origin.x)) / f32::from(b.size.width).max(1.0)).clamp(0.0, 1.0)
        }
    };
    let (pick_down, pick_move, pick_up, pick_out) = (pick.clone(), pick.clone(), pick.clone(), pick);
    let (d_down, d_move, d_up, d_out) = (drag.clone(), drag.clone(), drag.clone(), drag);
    let commit = move |t: f32, cx: &mut App| {
        if let (Some(len), true) = (length, seekable) {
            send(cx, MediaCommand::SeekTo(len.mul_f32(t)));
        }
    };
    let pick_size = |(small, big): (f32, f32)| if compact { small } else { big };
    let track_h = pick_size(TRACK_H_PX);
    let knob = pick_size(KNOB_PX);
    let knob_color = if palette.is_dark { Color::oklch(KNOB_DARK.0, KNOB_DARK.1, m.a.hue()) } else { Color::oklch(KNOB_LIGHT_L, 0.0, 0.0) };
    let (elapsed, remaining) = {
        let mut l = labels.borrow_mut();
        let elapsed = l.elapsed.get(shown_pos.as_secs(), |s| format_duration(*s));
        let remaining = l.remaining.get(length.map(|len| len.saturating_sub(shown_pos).as_secs()), |r| match r {
            Some(r) => format!("-{}", format_duration(*r)),
            None => UNKNOWN_TIME.into(),
        });
        (elapsed, remaining)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(pick_size(SCRUB_LABEL_GAP_PX)))
        .child(
            div()
                .id(id)
                .relative()
                .h(px(knob + KNOB_HEADROOM_PX))
                .flex()
                .items_center()
                .when(seekable, |el| el.cursor_pointer())
                .child(
                    div()
                        .w_full()
                        .h(px(track_h))
                        .rounded_full()
                        .bg(wash(SCRUB_TRACK_WASH, palette).hsla())
                        .child(div().h_full().w(gpui::relative(frac)).min_w(px(track_h)).rounded_full().bg(oklab_gradient(ANGLE_RIGHT_DEG, m.a, m.b))),
                )
                .when(length.is_some(), |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(gpui::relative(frac))
                            .ml(px(-knob / 2.0))
                            .size(px(knob))
                            .rounded_full()
                            .bg(knob_color.hsla())
                            .shadow(vec![
                                glow(m.b.with_alpha(KNOB_GLOW.0), KNOB_GLOW.1, 0.0, 0.0),
                                glow(Color::oklch(0.0, 0.0, 0.0).with_alpha(KNOB_SHADOW.0), KNOB_SHADOW.1, 0.0, KNOB_SHADOW.2),
                            ]),
                    )
                })
                .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
                .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, _| {
                    if seekable {
                        d_down.set(Some(pick_down(e.position.x)));
                        window.refresh();
                    }
                })
                .on_mouse_move(move |e: &MouseMoveEvent, window, _| {
                    if d_move.get().is_some() && e.pressed_button == Some(MouseButton::Left) {
                        d_move.set(Some(pick_move(e.position.x)));
                        window.refresh();
                    }
                })
                .on_mouse_up(MouseButton::Left, move |e: &MouseUpEvent, _, cx| {
                    if d_up.take().is_some() {
                        commit(pick_up(e.position.x), cx);
                    }
                })
                .on_mouse_up_out(MouseButton::Left, move |e: &MouseUpEvent, _, cx| {
                    if d_out.take().is_some() {
                        commit(pick_out(e.position.x), cx);
                    }
                }),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .font_features(tabular_nums())
                .text_size(px(text::LABEL))
                .text_color(palette.on_surface_variant.hsla())
                .child(elapsed)
                .child(remaining),
        )
}

fn transport_button(id: &'static str, icon_name: &'static str, side: f32, enabled: bool, palette: &MandaraPalette, command: MediaCommand) -> impl IntoElement {
    let fg = if enabled { palette.on_surface } else { palette.on_surface.with_alpha(DISABLED_ALPHA) };
    let hover = palette.on_surface.with_alpha(BUTTON_HOVER_ALPHA);
    div()
        .id(id)
        .size(px(side))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |el| el.cursor_pointer().hover(move |s| s.bg(hover.hsla())))
        .child(icon(icon_name, side * BUTTON_ICON_SCALE, fg))
        .on_click(move |_, _, cx| {
            if enabled {
                send(cx, command.clone());
            }
        })
}

/// Which transport row (their element ids must differ).
#[derive(Copy, Clone)]
pub(crate) enum Transport {
    Panel,
    Sidebar,
    /// The desktop now-playing card.
    Card,
}

pub(crate) fn transport(which: Transport, state: &MediaState, m: &ArtTheme, palette: &MandaraPalette, big: f32) -> impl IntoElement {
    let playing = state.status == PlaybackStatus::Playing;
    let mid = m.a.mix(m.b, 0.5);
    let fg = mid.best_on(Color::oklch(INK_LIGHT.0, INK_LIGHT.1, 0.0), Color::oklch(INK_DARK.0, INK_DARK.1, mid.hue()));
    let (prev_id, play_id, next_id) = match which {
        Transport::Panel => ("media-prev", "media-play", "media-next"),
        Transport::Sidebar => ("media-prev-s", "media-play-s", "media-next-s"),
        Transport::Card => ("media-prev-c", "media-play-c", "media-next-c"),
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(big * TRANSPORT_GAP_SCALE))
        .child(transport_button(prev_id, "previous", big * SIDE_BUTTON_SCALE, state.can_go_previous, palette, MediaCommand::Previous))
        .child(
            div()
                .id(play_id)
                .size(px(big))
                .flex_none()
                .rounded_full()
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, m.a, m.b))
                .shadow(vec![glow(m.glow, big * PLAY_GLOW_BLUR_SCALE, PLAY_GLOW_SPREAD_PX, big * PLAY_GLOW_DY_SCALE)])
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(PLAY_HOVER_OPACITY))
                .child(icon(if playing { "pause" } else { "play" }, big * PLAY_ICON_SCALE, fg))
                .on_click(|_, _, cx| send(cx, MediaCommand::PlayPause)),
        )
        .child(transport_button(next_id, "next", big * SIDE_BUTTON_SCALE, state.can_go_next, palette, MediaCommand::Next))
}

fn empty(p: &PanelCx, palette: &MandaraPalette, idle_player: Option<&str>) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let rings = div()
        .relative()
        .size(px(EMPTY_RINGS_PX))
        .flex()
        .items_center()
        .justify_center()
        .children(EMPTY_RING_D_PX.into_iter().zip(EMPTY_RING_ALPHA).map(move |(d, a)| {
            let inset = (EMPTY_RINGS_PX - d) / 2.0;
            div().absolute().left(px(inset)).top(px(inset)).size(px(d)).rounded_full().bg(accent.with_alpha(a).hsla())
        }))
        .child(icon("music", EMPTY_ICON_PX, accent));
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(EMPTY_GAP_PX))
        .child(reveal(p, 0, rings))
        .child(reveal(p, 1, div().mt(px(EMPTY_TITLE_GAP_PX)).text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(t("Nothing playing"))))
        .child(reveal(
            p,
            2,
            div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(match idle_player {
                Some(name) => tf("{name} is idle — start something to see it here", &[("name", &name)]),
                None => t("Play music or video in any MPRIS player").to_string(),
            }),
        ))
        .into_any_element()
}

/// A full-bleed layer cut to the panel's rounded silhouette.
fn layer(shape: PanelShape) -> gpui::Div {
    shape.clip(div().absolute().top_0().left_0().size_full())
}

/// What the panel is showing, as far as its size is concerned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PanelState {
    /// No player, or one with nothing loaded.
    Empty,
    /// A track (whatever of its metadata, art or lyrics has arrived).
    Track,
}

/// The panel's height. It depends only on whether a track is shown and
/// on the lyrics setting (`Some(rows)` when lyrics are on) — never on the
/// track's metadata, its art, the playback status or the lyrics lookup —
/// so a track change, late metadata or a press on play / pause never
/// resizes the panel under the pointer.
fn panel_height(state: PanelState, lyrics_rows: Option<u32>) -> f32 {
    match state {
        PanelState::Empty => EMPTY_H,
        PanelState::Track => PLAYING_H + lyrics_rows.map_or(0.0, |rows| LYRICS_GAP + lyrics_view::pane_height(rows)),
    }
}

/// Holds the last titled track over a brief title gap of the same player
/// ([`TITLE_GAP_HOLD`]).
#[derive(Default)]
struct TitleGap {
    last: Option<Rc<Snap>>,
    since: Option<Instant>,
}

impl TitleGap {
    /// The snapshot to draw (`None`: the empty state), and how long a held
    /// track stays up unless a title arrives first.
    fn resolve(&mut self, live: Option<Rc<Snap>>, now: Instant) -> (Option<Rc<Snap>>, Option<Duration>) {
        match live {
            Some(snap) if !snap.state.title.is_empty() => {
                self.last = Some(snap.clone());
                self.since = None;
                (Some(snap), None)
            }
            live => {
                let same_player = match (&live, &self.last) {
                    (Some(l), Some(last)) => l.state.bus_name == last.state.bus_name,
                    _ => false,
                };
                let since = *self.since.get_or_insert(now);
                let left = TITLE_GAP_HOLD.saturating_sub(now.saturating_duration_since(since));
                if same_player && !left.is_zero() {
                    return (self.last.clone(), Some(left));
                }
                self.last = None;
                (live.filter(|s| !s.state.title.is_empty()), None)
            }
        }
    }
}

fn set_size(cell: &Cell<gpui::Size<gpui::Pixels>>, height: f32) {
    if (f32::from(cell.get().height) - height).abs() > SIZE_EPSILON_PX {
        cell.set(size(px(PANEL_W), px(height)));
    }
}

fn panel_view(p: PanelCx, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>, drag: Rc<Cell<Option<f32>>>) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement {
    let fx = Rc::new(RefCell::new(Fx::new()));
    let tap = Rc::new(RefCell::new(SpectrumTap::default()));
    let lyrics = LyricsPane::new();
    let labels = Rc::new(RefCell::new(ScrubLabels::default()));
    let keeper = Rc::new(IdleRelease::default());
    let gap = Rc::new(RefCell::new(TitleGap::default()));
    move |palette, window, cx| {
        let visible = p.progress() > 0.0;
        if visible {
            let (fx, lyrics, labels, gap) = (fx.clone(), lyrics.clone(), labels.clone(), gap.clone());
            IdleRelease::touch(&keeper, window, cx, move |window| {
                fx.borrow_mut().release(window);
                lyrics.borrow_mut().release();
                *labels.borrow_mut() = ScrubLabels::default();
                *gap.borrow_mut() = TitleGap::default();
            });
        }
        let (snap, held) = gap.borrow_mut().resolve(snapshot(cx), Instant::now());
        if let Some(left) = held.filter(|_| visible) {
            redraw_in(left, window, cx);
        }
        let Some(snap) = snap else {
            set_size(&size_cell, panel_height(PanelState::Empty, None));
            SpectrumTap::bands(&tap, false, cx);
            let idle = snapshot(cx);
            return empty(&p, palette, idle.as_ref().map(|s| s.state.identity.as_str()));
        };
        let state = &snap.state;
        let playing = snap.playing();
        // Synced lyrics (`plugins.settings.media.lyrics`): fetched only
        // while the panel is open.
        let show_lyrics = lyrics_enabled(cx);
        let lyrics_rows = show_lyrics.then(|| {
            let rows = crate::state::config(cx).read(cx).get().plugins.setting(ID, "lyrics_rows", f64::from(lyrics_view::DEFAULT_ROWS));
            rows.round().clamp(0.0, f64::from(lyrics_view::MAX_ROWS)) as u32
        });
        if let Some(rows) = lyrics_rows {
            lyrics.borrow_mut().set_rows(rows);
            LyricsPane::sync(&lyrics, state, visible, window, cx);
        }
        set_size(&size_cell, panel_height(PanelState::Track, lyrics_rows));
        let shape = PanelShape::of(cx);
        let (attached, bottom_bar) = {
            let bar = &crate::state::config(cx).read(cx).get().bar;
            (bar.effective_layout().is_attached() && !bar.position.is_vertical(), bar.position == mandara_config::BarPosition::Bottom)
        };
        let surface = crate::shell::frame_color(palette);

        let (m, theme_moving) = fx.borrow_mut().theme(ArtTheme::of(state, palette));
        let fonts = crate::fonts::fonts(cx);
        let (lyrics_el, lyrics_moving) = if show_lyrics {
            let (el, moving) = lyrics.borrow_mut().element(state, (m.a, m.b), palette, &fonts.ui, &fonts.arabic, PANEL_W - PAD * 2.0);
            (Some(el), moving)
        } else {
            (None, false)
        };
        let art = backdrop(&fx, state.art_path.as_deref(), window, cx);
        let bands = SpectrumTap::bands(&tap, visible && playing, cx);
        if visible {
            let frame = if bands.is_some() || theme_moving || art.fading || lyrics_moving || drag.get().is_some() {
                LIVE_FRAME
            } else if playing {
                // Waiting for sound (or no capture): poll gently, tick time.
                PLAYING_FRAME
            } else {
                PAUSED_FRAME
            };
            redraw_in(frame, window, cx);
        }

        let art_alpha = alpha(BACKDROP_ALPHA, palette) * p.progress();
        let backdrop_layer = |image: Arc<RenderImage>, a: f32| {
            layer(shape).opacity(a).child(shape.clip(img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Cover)))
        };
        // Attached panels blend into the bar: fade the tint in from the
        // frame colour along the bar-side edge so there is no seam.
        let seam = attached.then(|| {
            let angle = if bottom_bar { ANGLE_UP_DEG } else { ANGLE_DOWN_DEG };
            layer(shape).bg(linear_gradient(angle, linear_color_stop(surface.hsla(), 0.0), linear_color_stop(surface.with_alpha(0.0).hsla(), SEAM_STOP)))
        });
        let ring_art = art.cover.clone().map(ImageSource::Render);
        shape
            .clip(div().size_full().relative().overflow_hidden())
            // 1 · art-tinted surface gradient
            .child(layer(shape).bg(oklab_gradient(ANGLE_DOWN_DEG, m.bg_top, m.bg_bottom)))
            // 2 · blurred artwork colour field (crossfading between tracks)
            .when_some(art.previous.filter(|_| art.previous_t > 0.0), |el, image| el.child(backdrop_layer(image, art_alpha * art.previous_t)))
            .when_some(art.current, |el, image| el.child(backdrop_layer(image, art_alpha * art.current_t)))
            // 3 · settle the colour field toward the tint at the bottom
            .child(layer(shape).bg(oklab_gradient(ANGLE_DOWN_DEG, m.bg_bottom.with_alpha(0.0), m.bg_bottom.with_alpha(SETTLE_ALPHA))))
            .when_some(seam, |el, s| el.child(s))
            // 4 · anti-banding grain
            .child(shape.clip(img(ImageSource::Render(mandara_core::material::grain_image())).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Fill)))
            .child(
                div()
                    .size_full()
                    .relative()
                    .p(px(PAD))
                    .flex()
                    .flex_col()
                    .child(reveal(&p, 0, head(&snap, bands.as_ref().map(|b| &b[..]), &m, palette)))
                    .child(div().h(px(HEAD_GAP)))
                    .child(reveal(&p, 1, info(&snap, art.cover, &m, palette, &fonts.arabic)))
                    .when_some(lyrics_el, |el, lyr| el.child(div().h(px(LYRICS_GAP))).child(reveal(&p, 2, lyr)))
                    .child(div().h(px(INFO_GAP)))
                    .child(reveal(&p, 2, super::visualizer::media_visualizer(bands.as_ref().map_or(&[][..], |b| &b[..]), ring_art, m.a, m.b, palette, VIZ_H, cx)))
                    .child(div().h(px(VIZ_GAP)))
                    .child(reveal(&p, 3, div().h(px(SCRUB_H)).child(scrubber("media-progress", state, &m, palette, drag.clone(), &labels, false))))
                    .child(div().h(px(SCRUB_GAP)))
                    .child(reveal(&p, 4, div().h(px(TRANSPORT_H)).flex().items_center().justify_center().child(transport(Transport::Panel, state, &m, palette, PLAY_PANEL_PX)))),
            )
            .into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Media"), tk("MPRIS now playing and controls"), Some(Zone::Center)), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let Some(snap) = playing_snapshot(cx) else { return div().into_any_element() };
                let playing = snap.playing();
                let m = ArtTheme::of(&snap.state, palette);
                div()
                    .flex()
                    .items_center()
                    .gap(px(CHIP_GAP_PX))
                    .child(icon(if playing { "music" } else { "pause" }, crate::style::bar_icon(CHIP_ICON_PX), if playing { m.a } else { palette.on_surface_variant }))
                    .child(div().max_w(px(CHIP_TITLE_MAX_W_PX)).truncate().text_color(palette.on_surface_variant.hsla()).child(snap.chip_title.clone()))
                    .into_any_element()
            },
            window,
            cx,
        );
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PLAYING_H))));
        let panel = fn_view(None, panel_view(panel_cx.clone(), size_cell.clone(), Rc::default()), window, cx);
        let side_drag: Rc<Cell<Option<f32>>> = Rc::default();
        let side_labels = Rc::new(RefCell::new(ScrubLabels::default()));
        let side_cover = Rc::new(RefCell::new(CoverSlot::default()));
        let side_keeper = Rc::new(IdleRelease::default());
        let sidebar = fn_view(
            None,
            move |palette, window, cx| {
                let Some(snap) = playing_snapshot(cx) else { return div().into_any_element() };
                let state = &snap.state;
                {
                    let (cover, labels) = (side_cover.clone(), side_labels.clone());
                    IdleRelease::touch(&side_keeper, window, cx, move |window| {
                        cover.borrow_mut().release(window);
                        *labels.borrow_mut() = ScrubLabels::default();
                    });
                }
                // Only the scrubber and times move: tick while playing.
                if snap.playing() {
                    redraw_in(SIDEBAR_TICK, window, cx);
                }
                let m = ArtTheme::of(state, palette);
                let cover = CoverSlot::get(&side_cover, state.art_path.as_deref(), SIDE_ART_PX, window, cx);
                let arabic = crate::fonts::fonts(cx).arabic;
                card(palette)
                    .bg(oklab_gradient(ANGLE_DOWN_DEG, palette.surface_container.mix(m.bg_top, SIDE_TINT.0), palette.surface_container.mix(m.bg_bottom, SIDE_TINT.1)))
                    .flex()
                    .flex_col()
                    .gap(px(tokens::MD))
                    .child(
                        div()
                            .flex()
                            .gap(px(tokens::MD))
                            .items_center()
                            .child(art(cover, SIDE_ART_PX, crate::style::radius(SIDE_ART_RADIUS_PX), &m))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .child(script_text(&snap.title, &arabic).font_weight(FontWeight::SEMIBOLD).truncate())
                                    .child(script_text(&snap.artist, &arabic).text_size(px(text::LABEL)).text_color(m.a.hsla()).truncate()),
                            ),
                    )
                    .child(scrubber("media-progress-s", state, &m, palette, side_drag.clone(), &side_labels, true))
                    .child(transport(Transport::Sidebar, state, &m, palette, PLAY_SIDEBAR_PX))
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            sidebar: Some(sidebar),
            on_click: None,
            on_secondary_click: Some(Rc::new(|_, cx| send(cx, MediaCommand::PlayPause))),
        }
    });

    for (path, summary, command) in [
        ("media play-pause", tk("Toggle playback"), MediaCommand::PlayPause),
        ("media next", tk("Next track"), MediaCommand::Next),
        ("media previous", tk("Previous track"), MediaCommand::Previous),
    ] {
        r.command(ID, CommandSpec::new(path, summary, move |_, cx| {
            send(cx, command.clone());
            Ok(json!("ok"))
        }));
    }
    r.command(
        ID,
        CommandSpec::new("media seek", tk("Seek to a position in seconds"), |a, cx| {
            let secs = a.number(0)?.max(0.0);
            send(cx, MediaCommand::SeekTo(Duration::from_secs_f64(secs)));
            Ok(json!({ "position": secs }))
        })
        .arg(ArgSpec::required("seconds", ArgKind::Number, tk("absolute position")))
        .example("media seek 90"),
    );
    r.command(ID, CommandSpec::new("media status", tk("Now-playing information"), |_, cx| {
        Ok(match hub(cx).read(cx).media.clone() {
            None => json!(null),
            Some(m) => json!({
                "player": m.identity,
                "status": format!("{:?}", m.status).to_lowercase(),
                "title": m.title,
                "artist": m.artist,
                "album": m.album,
                "position": m.position_now(Instant::now()).as_secs_f64(),
                "length": m.length.map(|l| l.as_secs_f64()),
                "art": m.art_path,
            }),
        })
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_theme::SeedColor;

    #[test]
    fn blurred_backdrop_is_small_bgra() {
        let dir = std::env::temp_dir().join(format!("mandara-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("art.png");
        image::RgbaImage::from_fn(200, 200, |x, _| image::Rgba([if x < 100 { 255 } else { 0 }, 0, 40, 255])).save(&path).unwrap();
        let art = decode_art(&path, 64, true);
        let img = art.backdrop.expect("decodes");
        let bytes = img.as_bytes(0).unwrap();
        // Top-left pixel was pure red (RGBA) → stored as BGRA: blue byte last.
        assert_eq!(bytes.len(), (BACKDROP_PX * BACKDROP_PX * 4) as usize);
        assert!(bytes[2] > 150 && bytes[0] < 100, "{:?}", &bytes[..4]);
        let cover = art.cover.expect("cover decoded at the drawn size");
        assert_eq!(cover.as_bytes(0).unwrap().len(), 64 * 64 * 4);
        let missing = decode_art(&dir.join("missing.png"), 64, true);
        assert!(missing.cover.is_none() && missing.backdrop.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn detects_arabic_script() {
        assert!(is_arabic("أبا هندٍ فلا تعجل علينا"));
        assert!(is_arabic("Live: ذات المجد"));
        assert!(!is_arabic("Sea Breeze (Extended Mix)"));
    }

    #[test]
    fn art_theme_follows_the_artwork() {
        let dark = mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME).unwrap().palette(true);
        let seed = |hex: &str, proportion: f32| {
            let color = Color::hex(hex);
            SeedColor { color, proportion, hue: color.hue(), chroma: color.chroma() * 300.0, tone: color.lightness() * 100.0 }
        };
        // A mostly red cover with a teal detail.
        let t = ArtTheme::from_seeds(&[seed("#c0392b", 0.7), seed("#1abc9c", 0.3)], &dark);
        let red = Color::hex("#c0392b").hue();
        assert!(mandara_theme::color::shortest_hue_delta(t.bg_top.hue(), red).abs() < 20.0, "surface follows the dominant hue");
        assert!(t.bg_top.lightness() < 0.35, "dark mode keeps a deep surface");
        // No art → the theme's own accent.
        let none = ArtTheme::from_seeds(&[], &dark);
        assert!(mandara_theme::color::shortest_hue_delta(none.bg_top.hue(), dark.primary.hue()).abs() < 1.0);
        // Crossfade endpoints.
        assert_eq!(t.lerp(none, 0.0), t);
        assert!(t.lerp(none, 1.0).bg_top.lightness() - none.bg_top.lightness() < 1e-4);
    }

    #[test]
    fn theme_tween_eases_between_tracks() {
        let p = mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME).unwrap().palette(true);
        let a = ArtTheme::from_seeds(&[], &p);
        let b = ArtTheme { bg_top: Color::hex("#402020"), ..a };
        let mut fx = Fx::new();
        assert_eq!(fx.theme(a).0, a);
        let (shown, moving) = fx.theme(b);
        assert!(moving);
        assert_eq!(shown, a, "a new target starts from what is on screen");
    }

    #[test]
    fn art_theme_is_cached_per_artwork_and_palette() {
        let p = mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME).unwrap().palette(true);
        assert_eq!(ArtTheme::cached(&[], &p), ArtTheme::from_seeds(&[], &p));
        let light = mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME).unwrap().palette(false);
        assert_eq!(ArtTheme::cached(&[], &light), ArtTheme::from_seeds(&[], &light), "a palette change recomputes");
    }

    #[test]
    fn panel_heights_fit_content() {
        assert!(PLAYING_H > EMPTY_H);
        assert!(PLAYING_H < 420.0);
        // The rows a playing panel stacks, all fixed: it can't be taller
        // than this sum, nor shorter.
        assert_eq!(panel_height(PanelState::Track, None), PLAYING_H);
        assert!(TITLE_MAX_LINES as f32 * TITLE_LINE_PX + text::BODY + text::BODY_S + 2.0 * INFO_TEXT_GAP_PX < ART, "the info text fits the art box");
    }

    fn snap(bus: &str, title: &str) -> Rc<Snap> {
        let mut state = MediaState::default();
        (state.bus_name, state.identity, state.title) = (bus.into(), "Demo".into(), title.into());
        Rc::new(Snap {
            title: Label::new(&state.title),
            artist: Label::new(&state.artist),
            album: Label::new(&state.album),
            chip_title: SharedString::default(),
            player: SharedString::default(),
            app_icon: None,
            state,
        })
    }

    /// The panel's height through everything that happens around a track
    /// change: late metadata, art arriving, play / pause, a stop.
    #[test]
    fn panel_height_is_constant_across_track_states() {
        for rows in [None, Some(lyrics_view::DEFAULT_ROWS), Some(lyrics_view::MAX_ROWS)] {
            let expected = panel_height(PanelState::Track, rows);
            let base = snap("org.mpris.MediaPlayer2.demo", "Short One");
            let mut states = vec![base.state.clone()];
            let mut s = base.state.clone();
            // Title first, the rest later (slow players).
            s.artist = "Nobody Known".into();
            states.push(s.clone());
            s.album = "An Album With A Title Long Enough To Need Truncating Twice Over".into();
            s.length = Some(Duration::from_secs(187));
            states.push(s.clone());
            s.art_path = Some(PathBuf::from("/nonexistent/art.png"));
            states.push(s.clone());
            s.title = "A Title So Long It Wraps Onto A Second Line And Then Some More Words Past The Clamp".into();
            states.push(s.clone());
            s.title = "أبا هندٍ فلا تعجل علينا".into();
            states.push(s.clone());
            for status in [PlaybackStatus::Playing, PlaybackStatus::Paused, PlaybackStatus::Stopped] {
                s.status = status;
                states.push(s.clone());
            }
            for st in &states {
                let shown = if st.title.is_empty() { PanelState::Empty } else { PanelState::Track };
                assert_eq!(panel_height(shown, rows), expected, "{:?} / {:?}", st.title, st.status);
            }
        }
        assert!(panel_height(PanelState::Track, Some(lyrics_view::DEFAULT_ROWS)) > panel_height(PanelState::Track, None));
    }

    #[test]
    fn a_brief_title_gap_keeps_the_track_up() {
        let t0 = Instant::now();
        let mut gap = TitleGap::default();
        let a = snap("org.mpris.MediaPlayer2.demo", "Short One");
        assert!(gap.resolve(Some(a.clone()), t0).0.is_some_and(|s| Rc::ptr_eq(&s, &a)));
        // The next track's metadata arrives without a title first.
        let untitled = snap("org.mpris.MediaPlayer2.demo", "");
        let (shown, left) = gap.resolve(Some(untitled.clone()), t0);
        assert!(shown.is_some_and(|s| Rc::ptr_eq(&s, &a)), "the last track stays up");
        assert_eq!(left, Some(TITLE_GAP_HOLD));
        let b = snap("org.mpris.MediaPlayer2.demo", "Montaza Garden Walk");
        assert!(gap.resolve(Some(b.clone()), t0 + TITLE_GAP_HOLD / 2).0.is_some_and(|s| Rc::ptr_eq(&s, &b)));
        // A gap that lasts is a real stop.
        gap.resolve(Some(untitled.clone()), t0);
        assert!(gap.resolve(Some(untitled.clone()), t0 + TITLE_GAP_HOLD).0.is_none(), "after the hold: empty state");
        // Another player, or none at all, shows the empty state at once.
        let mut gap = TitleGap::default();
        gap.resolve(Some(a.clone()), t0);
        assert!(gap.resolve(Some(snap("org.mpris.MediaPlayer2.other", "")), t0).0.is_none());
        let mut gap = TitleGap::default();
        gap.resolve(Some(a), t0);
        assert!(gap.resolve(None, t0).0.is_none(), "the player quit");
    }
}
