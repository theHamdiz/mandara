//! GPU audio visualizers on their own small surfaces: a glowing wave along
//! a screen edge, a framed box of segmented bars (hardware-spectrum look)
//! and a mirrored bar strip.
//!
//! Pacing and cost:
//!
//! * the spectrum is subscribed **only while a player is playing**; the
//!   capture thread stops with the last subscription;
//! * frames are capped at `[visualizers] fps` (default 30, 15–60): after
//!   each frame a timer for the interval notifies the view, which GPUI
//!   draws on the next vsync, so the compositor's 144 Hz never drives it;
//! * after `[visualizers] silence_s` of silence the view drops its
//!   subscription and stops scheduling frames — zero CPU — until the media
//!   state changes (or, while the player still says it is playing, a
//!   re-check every [`REPROBE`]); the card host closes the surface after
//!   the same window once playback stops;
//! * everything is painted as plain quads in one canvas (no per-bar
//!   elements, no tessellated paths and so no MSAA path textures).

use std::time::{Duration, Instant};

use gpui::{Bounds, Context, Hsla, IntoElement, ParentElement, Pixels, Render, Styled, Task, Window, canvas, div, fill, point, px, size};
use mandara_config::desktop_cards::{self as dc, FRAMED_BARS, MIRRORED_BARS};
use mandara_core::ToGpui;
use mandara_services::spectrum::{self, BANDS, Spectrum};
use mandara_theme::{Color, MandaraPalette};

use super::{MediaPresence, SHADOW_PAD_PX};

/// Which visualizer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Wave,
    Framed,
    Mirrored,
}

impl Kind {
    pub fn of(card: &str) -> Self {
        match card {
            FRAMED_BARS => Kind::Framed,
            MIRRORED_BARS => Kind::Mirrored,
            _ => Kind::Wave,
        }
    }
}

/// Smoothing toward louder / quieter levels, per frame at 60 Hz (scaled
/// to the real frame interval).
const ATTACK: f32 = 0.55;
const DECAY: f32 = 0.16;
const REFERENCE_HZ: f32 = 60.0;
/// Longest step a stalled frame may cover.
const MAX_DT_S: f32 = 0.1;
/// Below this everything has settled (stop drawing once silent).
const REST_EPS: f32 = 0.004;
/// Perceptual curve applied to band levels.
const GAMMA: f32 = 0.8;
/// Wave: column width, tallest share of the strip, resting height, the
/// two glow layers `(height ×, alpha ×)`.
const WAVE_COL_PX: f32 = 3.0;
const WAVE_MAX_FRAC: f32 = 0.78;
const WAVE_REST_PX: f32 = 2.0;
const WAVE_GLOW: [(f32, f32); 2] = [(1.28, 0.16), (1.12, 0.3)];
/// Mirrored strip / framed box: gap between bars as a share of a slot,
/// smallest bar.
const BAR_GAP_FRAC: f32 = 0.32;
const BAR_MIN_PX: f32 = 2.0;
/// The mirrored strip's widest bar.
const STRIP_BAR_MAX_PX: f32 = 7.0;
/// Framed box: inner padding, LED segment height and gap, peak cap and its
/// fall speed (share of the box per second), border alpha.
const BOX_PAD_PX: f32 = 12.0;
const SEG_PX: f32 = 4.0;
const SEG_GAP_PX: f32 = 2.0;
const PEAK_PX: f32 = 2.0;
const PEAK_FALL_PER_S: f32 = 0.35;
const BOX_RADIUS_PX: f32 = 14.0;
const BORDER_ALPHA: f32 = 0.16;
/// Asleep while the player still plays: listen again this often.
const REPROBE: Duration = Duration::from_secs(30);
/// How long a re-check listens before sleeping again.
const PROBE_LISTEN: Duration = Duration::from_millis(1000);
/// How often a subscribed but silent visualizer looks for sound (it draws
/// nothing new meanwhile).
const SILENT_POLL: Duration = Duration::from_millis(250);
/// Top segments of the box tint toward the warning colour.
const HOT_FROM: f32 = 0.82;

fn track_key(title: &str, artist: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (title, artist).hash(&mut h);
    h.finish()
}

/// Levels of `bands` resampled to `n` points (linear between bands).
pub fn resample(bands: &[f32], n: usize) -> Vec<f32> {
    if n == 0 || bands.is_empty() {
        return Vec::new();
    }
    let last = (bands.len() - 1) as f32;
    (0..n)
        .map(|i| {
            let t = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 * last };
            let (a, f) = (t.floor() as usize, t.fract());
            let b = (a + 1).min(bands.len() - 1);
            bands[a] * (1.0 - f) + bands[b] * f
        })
        .collect()
}

/// The wave's height profile across `cols` columns: bass in the middle,
/// mirrored out to the treble at both ends, cosine-smoothed.
pub fn wave_profile(levels: &[f32], cols: usize) -> Vec<f32> {
    if cols == 0 || levels.is_empty() {
        return Vec::new();
    }
    let half = (cols as f32 - 1.0) / 2.0;
    let last = (levels.len() - 1) as f32;
    (0..cols)
        .map(|c| {
            let d = if half > 0.0 { ((c as f32 - half).abs() / half).min(1.0) } else { 0.0 };
            let t = d * last;
            let (a, f) = (t.floor() as usize, t.fract());
            let b = (a + 1).min(levels.len() - 1);
            let f = (1.0 - (f * std::f32::consts::PI).cos()) / 2.0;
            levels[a] * (1.0 - f) + levels[b] * f
        })
        .collect()
}

/// Whether to keep the spectrum subscription: sound was heard within the
/// last `silence` (tracked in `silent_since`).
pub fn keep_listening(audible: bool, silent_since: &mut Option<Instant>, now: Instant, silence: Duration) -> bool {
    if audible {
        *silent_since = None;
        return true;
    }
    now.duration_since(*silent_since.get_or_insert(now)) < silence
}

pub struct VisualizerView {
    widget: String,
    kind: Kind,
    spectrum: Option<Spectrum>,
    /// Smoothed band levels.
    levels: [f32; BANDS],
    /// Framed box peak caps (per bar).
    peaks: Vec<f32>,
    silent_since: Option<Instant>,
    /// Stopped after silence; re-armed by the next media change.
    asleep: bool,
    /// While asleep but the player still says "playing" (a quiet intro, a
    /// silent gap), listen again after [`REPROBE`].
    reprobe: Option<Task<()>>,
    /// Listening again after a re-check: give up after [`PROBE_LISTEN`].
    probing: bool,
    last_frame: Option<Instant>,
    next_frame: Option<Task<()>>,
    seen: MediaPresence,
    /// The track playing (hash of title and artist).
    track: u64,
    input_cleared: bool,
    /// Frames drawn (diagnostics / tests).
    pub frames: u64,
}

impl VisualizerView {
    pub fn new(widget: String, kind: Kind, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&crate::hub::hub(cx), |this: &mut Self, hub, cx| {
            let now = MediaPresence::of(cx);
            // A new track (or play / pause) wakes a sleeping visualizer.
            let track = hub.read(cx).media.as_ref().map(|m| track_key(&m.title, &m.artist)).unwrap_or_default();
            if now != this.seen || track != this.track {
                this.seen = now;
                this.track = track;
                this.asleep = false;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()).detach();
        Self {
            widget,
            kind,
            spectrum: None,
            levels: [0.0; BANDS],
            peaks: Vec::new(),
            silent_since: None,
            asleep: false,
            reprobe: None,
            probing: false,
            last_frame: None,
            next_frame: None,
            seen: MediaPresence::of(cx),
            track: 0,
            input_cleared: false,
            frames: 0,
        }
    }

    /// Advances the spectrum state; returns whether more frames are needed.
    /// Returns `(drawing, listening)`: frames are needed at the frame rate
    /// (sound or settling motion), or only the slow silence check.
    fn step(&mut self, playing: bool, silence: Duration, now: Instant) -> (bool, bool) {
        if playing && !self.asleep && self.spectrum.is_none() {
            self.spectrum = spectrum::subscribe();
            self.silent_since = None;
        }
        if !playing {
            self.spectrum = None;
        }
        // The capture couldn't start or stopped (no sound server, refused):
        // don't re-subscribe every frame — sleep until the next re-check.
        if self.spectrum.as_ref().is_some_and(|s| !s.is_running()) {
            self.spectrum = None;
            self.asleep = true;
        }
        let target = match &self.spectrum {
            Some(s) => {
                let audible = s.is_active();
                if audible {
                    self.probing = false;
                }
                // A re-check after sleeping listens only briefly.
                let window = if self.probing { silence.min(PROBE_LISTEN) } else { silence };
                if !keep_listening(audible, &mut self.silent_since, now, window) {
                    // Silent for the whole window: stop listening and drawing.
                    self.spectrum = None;
                    self.asleep = true;
                }
                self.spectrum.as_ref().map(|s| s.bands()).unwrap_or([0.0; BANDS])
            }
            None => [0.0; BANDS],
        };
        let dt = self.last_frame.map_or(1.0 / REFERENCE_HZ, |t| now.duration_since(t).as_secs_f32().min(MAX_DT_S));
        let rate = |k: f32| 1.0 - (1.0 - k).powf(dt * REFERENCE_HZ);
        let listening = self.spectrum.is_some();
        let mut moving = self.spectrum.as_ref().is_some_and(|s| s.is_active());
        for (l, t) in self.levels.iter_mut().zip(target) {
            let t = t.clamp(0.0, 1.0).powf(GAMMA);
            *l += (t - *l) * rate(if t > *l { ATTACK } else { DECAY });
            moving |= *l > REST_EPS;
        }
        if !moving {
            self.levels = [0.0; BANDS];
        }
        // Peak caps fall slowly toward the bars.
        let fall = PEAK_FALL_PER_S * dt;
        let bars = self.peaks.len();
        if bars > 0 {
            for (p, v) in self.peaks.iter_mut().zip(resample(&self.levels, bars)) {
                *p = if v >= *p { v } else { (*p - fall).max(v) };
                moving |= *p > REST_EPS;
            }
        }
        self.last_frame = Some(now);
        (moving, listening && !moving)
    }

    /// Whether the visualizer is drawing (sampling or settling).
    pub fn is_running(&self) -> bool {
        self.spectrum.is_some() || self.levels.iter().any(|l| *l > REST_EPS)
    }
}

fn gradient(palette: &MandaraPalette, n: usize, alpha: f32) -> Vec<Hsla> {
    let (a, b) = (palette.accent_gradient_start, palette.accent_gradient_end);
    (0..n).map(|i| a.mix(b, if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 }).with_alpha(alpha).hsla()).collect()
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

fn paint_wave(b: Bounds<Pixels>, levels: &[f32], on_top: bool, glow: bool, palette: &MandaraPalette, opacity: f32, window: &mut Window) {
    let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
    let cols = (w / WAVE_COL_PX).ceil().max(1.0) as usize;
    let profile = wave_profile(levels, cols);
    let colors = gradient(palette, cols, opacity);
    let max_h = h * WAVE_MAX_FRAC;
    let column = |x: f32, ch: f32| if on_top { rect(x, y0, WAVE_COL_PX + 0.5, ch) } else { rect(x, y0 + h - ch, WAVE_COL_PX + 0.5, ch) };
    if glow {
        for (grow, alpha) in WAVE_GLOW {
            for (i, v) in profile.iter().enumerate() {
                let ch = ((WAVE_REST_PX + v * max_h) * grow).min(h);
                let mut c = colors[i];
                c.a *= alpha;
                window.paint_quad(fill(column(x0 + i as f32 * WAVE_COL_PX, ch), c));
            }
        }
    }
    for (i, v) in profile.iter().enumerate() {
        let ch = WAVE_REST_PX + v * max_h;
        window.paint_quad(fill(column(x0 + i as f32 * WAVE_COL_PX, ch), colors[i]).corner_radii(px(WAVE_COL_PX / 2.0)));
    }
}

fn paint_mirrored(b: Bounds<Pixels>, levels: &[f32], bars: usize, palette: &MandaraPalette, opacity: f32, window: &mut Window) {
    let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
    let values = wave_profile(levels, bars);
    let colors = gradient(palette, bars, opacity);
    let slot = w / bars.max(1) as f32;
    // Slim bars with room between them, however wide the strip.
    let bw = (slot * (1.0 - BAR_GAP_FRAC)).clamp(1.0, STRIP_BAR_MAX_PX);
    let mid = y0 + h / 2.0;
    for (i, v) in values.iter().enumerate() {
        let bh = (BAR_MIN_PX + v * (h - BAR_MIN_PX)).min(h);
        let x = x0 + i as f32 * slot + (slot - bw) / 2.0;
        window.paint_quad(fill(rect(x, mid - bh / 2.0, bw, bh), colors[i]).corner_radii(px(bw / 2.0)));
    }
}

fn paint_framed(b: Bounds<Pixels>, levels: &[f32], peaks: &[f32], palette: &MandaraPalette, opacity: f32, window: &mut Window) {
    let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
    let bars = peaks.len().max(1);
    let values = resample(levels, bars);
    let (ix, iy, iw, ih) = (x0 + BOX_PAD_PX, y0 + BOX_PAD_PX, w - BOX_PAD_PX * 2.0, h - BOX_PAD_PX * 2.0);
    let slot = iw / bars as f32;
    let bw = (slot * (1.0 - BAR_GAP_FRAC)).max(1.0);
    let segments = ((ih + SEG_GAP_PX) / (SEG_PX + SEG_GAP_PX)).floor().max(1.0) as usize;
    let (start, end, hot) = (palette.accent_gradient_start, palette.accent_gradient_end, palette.warning_amber);
    let seg_color = |s: usize| -> Hsla {
        let t = s as f32 / segments.max(1) as f32;
        let c: Color = if t >= HOT_FROM { end.mix(hot, (t - HOT_FROM) / (1.0 - HOT_FROM)) } else { start.mix(end, t / HOT_FROM) };
        c.with_alpha(opacity).hsla()
    };
    let unlit = palette.on_surface.with_alpha(BORDER_ALPHA * opacity * 0.5).hsla();
    for (i, v) in values.iter().enumerate() {
        let x = ix + i as f32 * slot + (slot - bw) / 2.0;
        let lit = (v * segments as f32).round() as usize;
        for s in 0..segments {
            let y = iy + ih - (s as f32 + 1.0) * (SEG_PX + SEG_GAP_PX) + SEG_GAP_PX;
            window.paint_quad(fill(rect(x, y, bw, SEG_PX), if s < lit { seg_color(s) } else { unlit }).corner_radii(px(1.0)));
        }
        let peak = peaks[i];
        if peak > REST_EPS {
            let y = iy + ih - peak * ih;
            window.paint_quad(fill(rect(x, y.max(iy), bw, PEAK_PX), end.with_alpha(opacity).hsla()));
        }
    }
}

impl Render for VisualizerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.input_cleared {
            // Pure display: every click goes to what is underneath.
            window.set_input_region(Some(&[]));
            self.input_cleared = true;
        }
        let palette = mandara_core::frame_palette(window, cx);
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let widget = cfg.desktop.widgets.get(&self.widget).cloned();
        let card = widget.as_ref().and_then(dc::as_card).cloned().unwrap_or_default();
        let now = Instant::now();
        let playing = MediaPresence::of(cx).playing;
        if self.kind == Kind::Framed {
            let bars = dc::options::<dc::FramedBarsOptions>(&card).bars() as usize;
            if self.peaks.len() != bars {
                self.peaks = vec![0.0; bars];
            }
        }
        let was_asleep = self.asleep;
        let (moving, listening) = self.step(playing, cfg.visualizers.silence(), now);
        if self.asleep && !was_asleep && playing {
            self.reprobe = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(REPROBE).await;
                let _ = this.update(cx, |this, cx| {
                    this.asleep = false;
                    this.probing = true;
                    this.reprobe = None;
                    cx.notify();
                });
            }));
        }
        self.frames += 1;
        if (moving || listening) && self.next_frame.is_none() {
            // Frame cap: the next frame is due one interval after this one
            // (a quiet subscription is only checked a few times a second).
            let interval = if moving { cfg.visualizers.frame_interval() } else { SILENT_POLL };
            self.next_frame = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(interval).await;
                // (Notifying draws the next frame at the display's next
                // vsync; requesting an animation frame here, outside a
                // render, is not allowed.)
                let _ = this.update(cx, |this, cx| {
                    this.next_frame = None;
                    cx.notify();
                });
            }));
        } else if !moving && !listening {
            self.next_frame = None;
            self.last_frame = None;
        }
        let levels = self.levels;
        let peaks = self.peaks.clone();
        let kind = self.kind;
        let opacity = widget.as_ref().map_or(1.0, |w| w.place.opacity());
        let (wave, mirrored, framed) = (dc::options::<dc::WaveOptions>(&card), dc::options::<dc::MirroredBarsOptions>(&card), dc::options::<dc::FramedBarsOptions>(&card));
        let on_top = widget.as_ref().is_some_and(|w| dc::strip_on_top(w.place.anchor));
        let p = palette.clone();
        let framed_alpha = framed.opacity() * opacity;
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, _| match kind {
                Kind::Wave => paint_wave(b, &levels, on_top, wave.glow, &p, wave.opacity() * opacity, window),
                Kind::Mirrored => paint_mirrored(b, &levels, mirrored.bars() as usize, &p, mirrored.opacity() * opacity, window),
                Kind::Framed => paint_framed(b, &levels, &peaks, &p, framed_alpha, window),
            },
        )
        .size_full();
        match kind {
            Kind::Framed => {
                let k = widget.as_ref().map_or(1.0, |w| w.place.scale());
                let (w, h) = framed.size();
                div().size_full().p(px(SHADOW_PAD_PX)).child(
                    super::now_playing::glass(&palette, crate::style::radius(BOX_RADIUS_PX) * k, opacity)
                        .w(px(w * k))
                        .h(px(h * k))
                        .border_1()
                        .border_color(palette.on_surface.with_alpha(BORDER_ALPHA).hsla())
                        .child(paint),
                )
            }
            _ => div().size_full().child(paint),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare(kind: Kind) -> VisualizerView {
        VisualizerView {
            widget: String::new(),
            kind,
            spectrum: None,
            levels: [1.0; BANDS],
            peaks: vec![1.0; 8],
            silent_since: None,
            asleep: false,
            reprobe: None,
            probing: false,
            last_frame: None,
            next_frame: None,
            seen: MediaPresence::default(),
            track: 0,
            input_cleared: true,
            frames: 0,
        }
    }

    #[test]
    fn stopped_playback_settles_and_stops_drawing() {
        let mut v = bare(Kind::Framed);
        let t0 = Instant::now();
        let mut frames = 0;
        let mut t = t0;
        while v.step(false, Duration::from_secs(5), t).0 {
            frames += 1;
            t += Duration::from_millis(33);
            assert!(frames < 300, "settles within ten seconds at 30 fps");
        }
        assert!(!v.is_running(), "no subscription, no motion: no more frames");
        assert!(v.levels.iter().all(|l| *l == 0.0));
        assert!(v.spectrum.is_none());
    }

    #[test]
    fn silence_stops_listening_after_the_window() {
        let t0 = Instant::now();
        let silence = Duration::from_secs(5);
        let mut since = None;
        assert!(keep_listening(false, &mut since, t0, silence));
        assert!(keep_listening(false, &mut since, t0 + Duration::from_secs(4), silence));
        assert!(!keep_listening(false, &mut since, t0 + Duration::from_secs(5), silence), "five silent seconds stop it");
        assert!(keep_listening(true, &mut since, t0 + Duration::from_secs(6), silence));
        assert_eq!(since, None, "sound resets the window");
    }

    #[test]
    fn resample_keeps_the_ends_and_interpolates() {
        let bands = [0.0, 1.0];
        assert_eq!(resample(&bands, 3), vec![0.0, 0.5, 1.0]);
        assert_eq!(resample(&bands, 0), Vec::<f32>::new());
    }

    #[test]
    fn wave_puts_the_bass_in_the_middle_symmetrically() {
        let mut levels = [0.0; BANDS];
        levels[0] = 1.0;
        let p = wave_profile(&levels, 101);
        assert!((p[50] - 1.0).abs() < 1e-5);
        assert!(p[0] < 1e-5 && p[100] < 1e-5);
        for i in 0..50 {
            assert!((p[i] - p[100 - i]).abs() < 1e-5);
        }
    }
}
