//! The now-playing desktop card: album art (decoded at its display size),
//! title / artist, a wavy ("squiggle") progress line, previous / play /
//! next, and optionally a few synced lyric lines with the current line
//! glowing (the media panel's karaoke pane).
//!
//! Cost: it redraws once a second while playing (the progress), only on
//! media changes while paused, and runs frames only while its fade or the
//! lyrics scroll animate. The art is decoded once per track at the card's
//! pixel size and released with the surface.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, FontWeight, IntoElement, ParentElement, Pixels, Render, SharedString, Styled, Window, canvas, div, fill, point,
    prelude::FluentBuilder as _, px, size,
};
use mandara_config::desktop_cards::{self as dc, NowPlayingOptions};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_services::{MediaState, PlaybackStatus};
use mandara_theme::MandaraPalette;

use super::SHADOW_PAD_PX;
use crate::fonts::tabular_nums;
use crate::plugins::media::lyrics_view::{self, LyricsPane};
use crate::plugins::media::{ArtTheme, CoverSlot, Transport, art, transport};

/// Card geometry (logical px at scale 1).
const PAD_PX: f32 = 14.0;
const GAP_PX: f32 = 14.0;
const ART_PX: f32 = 92.0;
const TITLE_PX: f32 = 15.0;
const ARTIST_PX: f32 = 12.5;
const TITLE_ROW_PX: f32 = 21.0;
const ARTIST_ROW_PX: f32 = 18.0;
const SQUIGGLE_ROW_PX: f32 = 17.0;
const CONTROLS_ROW_PX: f32 = 36.0;
const PLAY_PX: f32 = 32.0;
const TIME_PX: f32 = 11.0;
const LYRICS_GAP_PX: f32 = 10.0;
/// Squiggle: amplitude, wavelength, stroke, dot step, knob and track.
const WAVE_AMP_PX: f32 = 2.6;
const WAVE_LEN_PX: f32 = 18.0;
const STROKE_PX: f32 = 3.0;
const DOT_STEP_PX: f32 = 1.25;
/// Never a smaller step (a zero scale must not stall the loop).
const MIN_DOT_STEP_PX: f32 = 0.5;
const KNOB_PX: f32 = 9.0;
const TRACK_ALPHA: f32 = 0.22;
/// Art corner radius, before the global roundness.
const ART_RADIUS_PX: f32 = 12.0;
const ART_GLOW: (f32, f32) = (22.0, 2.0);
/// Card fill alpha over the wallpaper and its sheen.
const SHEEN: (f32, f32) = (0.02, -0.01);
const RIM_LIGHT_ALPHA: f32 = 0.14;
const RIM_SHADOW_ALPHA: f32 = 0.28;
/// How often the progress redraws while playing.
const PROGRESS_TICK: Duration = Duration::from_secs(1);
/// Fade in / out.
const FADE: SpringConfig = SpringConfig::new(170.0, 26.0, 1.0);
const FADE_GONE: f32 = 0.02;
/// The fade-out's length before the surface goes.
const FADE_OUT: Duration = Duration::from_millis(360);
const ONE_SECOND: f32 = 1.0;

/// Card size `(w, h)` for `opts` at scale `k` (without the shadow pad).
pub fn card_size(opts: &NowPlayingOptions, k: f32) -> (f32, f32) {
    let body = ART_PX.max(TITLE_ROW_PX + ARTIST_ROW_PX + SQUIGGLE_ROW_PX + CONTROLS_ROW_PX);
    let rows = opts.lyric_lines();
    let lyrics = if rows > 0 { LYRICS_GAP_PX + lyrics_view::pane_height(rows) } else { 0.0 };
    (opts.width() * k, (PAD_PX * 2.0 + body + lyrics) * k)
}

pub struct NowPlayingView {
    widget: String,
    cover: Rc<RefCell<CoverSlot>>,
    lyrics: Rc<RefCell<LyricsPane>>,
    fade: SpringValue<f32>,
    clock: FrameClock,
    leaving: bool,
    /// The card rect last given to the compositor as input region.
    input: Option<(f32, f32)>,
    /// The last options seen (kept while fading out after the card was
    /// removed from the config).
    last: (NowPlayingOptions, f32),
}

impl NowPlayingView {
    pub fn new(widget: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&crate::hub::hub(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        let mut fade = SpringValue::new(0.0, FADE);
        fade.set_target(1.0);
        Self { widget, cover: Rc::new(RefCell::new(CoverSlot::default())), lyrics: LyricsPane::new(), fade, clock: FrameClock::new(), leaving: false, input: None, last: (NowPlayingOptions::default(), 1.0) }
    }

    /// Fades out, then closes the surface (from a task, never inside a
    /// render: removing a window mid-draw while another closes in the same
    /// cycle spun GPUI's effect loop).
    pub fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.leaving {
            return;
        }
        self.leaving = true;
        self.fade.set_target(0.0);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FADE_OUT).await;
            let _ = this.update_in(cx, |this, window, _| {
                this.cover.borrow_mut().release(window);
                window.remove_window();
            });
        })
        .detach();
    }

    fn options(&mut self, cx: &App) -> (NowPlayingOptions, f32) {
        let cfg = crate::state::config(cx).read(cx).get();
        if let Some(w) = cfg.desktop.widgets.get(&self.widget) {
            self.last = (dc::as_card(w).map(dc::options).unwrap_or_default(), w.place.scale());
        }
        self.last.clone()
    }
}

/// The squiggle: played part waves (accent gradient), the rest is a faint
/// straight track; a knob marks the head. Painted as overlapping round dots
/// (SDF quads), never a tessellated path.
fn squiggle(progress: f32, wavy: bool, k: f32, rtl: bool, m: ArtTheme, palette: &MandaraPalette) -> impl IntoElement {
    let track = palette.on_surface.with_alpha(TRACK_ALPHA).hsla();
    canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let (x0, w, mid) = (f32::from(b.origin.x), f32::from(b.size.width), f32::from(b.origin.y) + f32::from(b.size.height) / 2.0);
            let stroke = STROKE_PX * k;
            let head = w * progress.clamp(0.0, 1.0);
            let at = |d: f32| if rtl { x0 + w - d } else { x0 + d };
            // The rest: one straight quad.
            let (rest_from, rest_to) = (at(head), at(w));
            let (l, r) = (rest_from.min(rest_to), rest_from.max(rest_to));
            if r - l > 1.0 {
                window.paint_quad(fill(Bounds { origin: point(px(l), px(mid - stroke / 2.0)), size: size(px(r - l), px(stroke)) }, track).corner_radii(px(stroke / 2.0)));
            }
            // Played: dots along a sine, coloured along the gradient.
            let step = (DOT_STEP_PX * k).max(MIN_DOT_STEP_PX);
            let mut d = 0.0;
            while d <= head {
                let y = if wavy { mid + (d / (WAVE_LEN_PX * k) * std::f32::consts::TAU).sin() * WAVE_AMP_PX * k } else { mid };
                let c = m.a.mix(m.b, if w > 0.0 { d / w } else { 0.0 }).hsla();
                window.paint_quad(fill(Bounds { origin: point(px(at(d) - stroke / 2.0), px(y - stroke / 2.0)), size: size(px(stroke), px(stroke)) }, c).corner_radii(px(stroke / 2.0)));
                d += step;
            }
            let knob = KNOB_PX * k;
            let ky = if wavy { mid + (head / (WAVE_LEN_PX * k) * std::f32::consts::TAU).sin() * WAVE_AMP_PX * k } else { mid };
            window.paint_quad(fill(Bounds { origin: point(px(at(head) - knob / 2.0), px(ky - knob / 2.0)), size: size(px(knob), px(knob)) }, m.b.hsla()).corner_radii(px(knob / 2.0)));
        },
    )
    .w_full()
    .h(px(SQUIGGLE_ROW_PX * k))
}

fn time(d: Duration) -> SharedString {
    crate::kit::format_duration(d.as_secs()).into()
}

/// The card's glass: opaque tonal fill with a faint sheen, the specular rim
/// and a soft elevation (inside the shadow pad).
pub(crate) fn glass(palette: &MandaraPalette, radius: f32, opacity: f32) -> gpui::Div {
    let base = crate::shell::frame_color(palette);
    let rim = mandara_core::glass_rim(palette.specular_rim_light.with_alpha(RIM_LIGHT_ALPHA), palette.specular_rim_shadow.with_alpha(RIM_SHADOW_ALPHA));
    let shadows: Vec<_> = crate::plugins::panel_fx::elevation(palette, 1.0).into_iter().chain(rim).collect();
    div()
        .rounded(px(radius))
        .bg(mandara_core::oklab_gradient(crate::plugins::panel_fx::ANGLE_DOWN_DEG, base.lighten(SHEEN.0).with_alpha(opacity), base.lighten(SHEEN.1).with_alpha(opacity)))
        .shadow(shadows)
}

/// Sets the surface's input region to the card (the shadow pad and the
/// rounded corners' outside click through).
pub(crate) fn card_input(input: &mut Option<(f32, f32)>, w: f32, h: f32, window: &mut Window) {
    if *input != Some((w, h)) {
        *input = Some((w, h));
        window.set_input_region(Some(&[Bounds { origin: point(px(SHADOW_PAD_PX), px(SHADOW_PAD_PX)), size: size(px(w), px(h)) }]));
    }
}

impl Render for NowPlayingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let (opts, k) = self.options(cx);
        let (w, h) = card_size(&opts, k);
        card_input(&mut self.input, w, h, window);
        let dt = self.clock.tick(Instant::now());
        let mut animating = self.fade.step(dt) && !(self.leaving && self.fade.value() <= FADE_GONE);
        let state: MediaState = crate::hub::hub(cx).read(cx).media.clone().unwrap_or_default();
        let playing = state.status == PlaybackStatus::Playing;
        let m = ArtTheme::of(&state, &palette);
        let rtl = crate::bidi::rtl();
        let fonts = crate::fonts::fonts(cx);
        let s = |v: f32| v * k;
        let radius = crate::style::card_radius() * k;
        let art_side = s(ART_PX);
        let cover = CoverSlot::get(&self.cover, state.art_path.as_deref(), art_side, window, cx);
        let now = Instant::now();
        let progress = state.progress(now);
        let rows = opts.lyric_lines();
        let lyrics = (rows > 0).then(|| {
            self.lyrics.borrow_mut().set_rows(rows);
            LyricsPane::sync(&self.lyrics, &state, true, window, cx);
            let (el, moving) = self.lyrics.borrow_mut().element(&state, (m.a, m.b), &palette, &fonts.ui, &fonts.arabic, w - s(PAD_PX) * 2.0);
            animating |= moving;
            el
        });
        if playing {
            // The progress line moves on whole seconds.
            let frac = state.position_now(now).as_secs_f32().fract();
            crate::plugins::panel_fx::redraw_in(Duration::from_secs_f32((ONE_SECOND - frac).max(0.0)).min(PROGRESS_TICK), window, cx);
        }
        if animating {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let length = state.length.unwrap_or_default();
        let info = div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .child(div().h(px(s(TITLE_ROW_PX))).text_size(px(s(TITLE_PX))).font_weight(FontWeight::SEMIBOLD).truncate().child(SharedString::from(state.title.clone())))
            .child(div().h(px(s(ARTIST_ROW_PX))).text_size(px(s(ARTIST_PX))).text_color(m.a.hsla()).truncate().child(SharedString::from(state.artist.clone())))
            .child(squiggle(progress, opts.squiggle, k, rtl, m, &palette))
            .child(
                div()
                    .h(px(s(CONTROLS_ROW_PX)))
                    .flex()
                    .when(rtl, |el| el.flex_row_reverse())
                    .items_center()
                    .justify_between()
                    .text_size(px(s(TIME_PX)))
                    .font_features(tabular_nums())
                    .text_color(palette.on_surface_variant.hsla())
                    .child(time(state.position_now(now)))
                    .child(transport(Transport::Card, &state, &m, &palette, s(PLAY_PX)))
                    .child(time(length)),
            );
        let body = div()
            .flex()
            .when(rtl, |el| el.flex_row_reverse())
            .gap(px(s(GAP_PX)))
            .items_center()
            .child(div().rounded(px(s(ART_RADIUS_PX))).shadow(vec![crate::plugins::panel_fx::glow(m.glow, s(ART_GLOW.0), 0.0, s(ART_GLOW.1))]).child(art(cover, art_side, crate::style::radius(ART_RADIUS_PX) * k, &m)))
            .child(info);
        div()
            .size_full()
            .p(px(SHADOW_PAD_PX))
            .opacity(self.fade.value().clamp(0.0, 1.0))
            .font_family(fonts.ui.clone())
            .text_color(palette.on_surface.hsla())
            .child(
                glass(&palette, radius, 1.0)
                    .w(px(w))
                    .h(px(h))
                    .p(px(s(PAD_PX)))
                    .flex()
                    .flex_col()
                    .gap(px(s(LYRICS_GAP_PX)))
                    .child(body)
                    .children(lyrics),
            )
            .into_any_element()
    }
}

/// The accent colours of the card for `state` (tests).
#[cfg(test)]
fn accents(state: &MediaState, palette: &MandaraPalette) -> (mandara_theme::Color, mandara_theme::Color) {
    let m = ArtTheme::of(state, palette);
    (m.a, m.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_grows_with_lyrics_and_scale() {
        let mut o = NowPlayingOptions { lyric_lines: 0, ..Default::default() };
        let (w0, h0) = card_size(&o, 1.0);
        assert_eq!(w0, o.width());
        o.lyric_lines = 3;
        let (_, h3) = card_size(&o, 1.0);
        assert!(h3 > h0 + lyrics_view::pane_height(3) - 0.5);
        let (w2, h2) = card_size(&o, 2.0);
        assert!((w2 - w0 * 2.0).abs() < 0.01 && (h2 - h3 * 2.0).abs() < 0.01);
    }

    #[test]
    fn accents_follow_the_palette_without_art() {
        let p = mandara_theme::engine::default_palette(true);
        let (a, b) = accents(&MediaState::default(), &p);
        assert!(a.alpha() > 0.9 && b.alpha() > 0.9);
    }
}
