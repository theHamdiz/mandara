//! The media panel's synced lyrics: fetched from lrclib.net (through
//! `mandara_services::lyrics`, cached on disk) only while the panel is open,
//! the sung line highlighted with a karaoke sweep and glow in the artwork's
//! accent, spring-smoothed scrolling between lines, neighbours dimmed by
//! distance, Arabic (RTL) lines right-aligned in the Arabic face, unsynced
//! lyrics drifting slowly with the track's progress.
//!
//! Fetched text is converted once, off the UI thread, into shared display
//! rows (text, direction, length) so a frame never clones the lyrics; the
//! rows are freed again when the panel closes ([`LyricsPane::release`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::t;
use mandara_services::lyrics::{self, Lyrics, TrackQuery};
use mandara_services::{MediaState, PlaybackStatus};
use mandara_theme::{Color, MandaraPalette};
use smallvec::SmallVec;

use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::glow;

/// One lyric row.
const SLOT: f32 = 26.0;
/// Rows on screen by default (`plugins.settings.media.lyrics_rows`): two
/// above, the sung line, two below.
pub const DEFAULT_ROWS: u32 = 5;
/// Bounds of the rows setting (odd counts keep the sung line centred).
pub const MIN_ROWS: u32 = 3;
pub const MAX_ROWS: u32 = 9;
/// Lyrics feel late without a small lead over the reported position.
const LEAD: Duration = Duration::from_millis(150);
const SCROLL: SpringConfig = SpringConfig::new(150.0, 21.0, 1.0);
/// A failed fetch is retried after this long.
const RETRY: Duration = Duration::from_secs(60);
/// The scroll spring rests once within this many rows of its target.
const SCROLL_REST_ROWS: f32 = 0.002;

/// Beyond half the visible rows: rows considered around the scroll
/// position, the distance (in rows) past which a row is not drawn, and
/// (inside it) where rows start fading out.
const ROW_WINDOW_EXTRA: f32 = 0.5;
const ROW_CUTOFF_EXTRA: f32 = 0.1;
const FADE_INSET_ROWS: f32 = 0.9;
/// The last synced line is highlighted for this long when no next line
/// bounds it.
const LAST_LINE_SPAN: Duration = Duration::from_secs(4);
/// Shortest line span used for the sweep (avoids dividing by ~0).
const MIN_LINE_SPAN_S: f32 = 0.1;
/// Sweep progress at which the lit text covers the full width.
const SWEEP_FULL: f32 = 0.999;

/// Lyric text size, plus the growth of the sung line.
const LINE_TEXT_PX: f32 = 14.0;
const LINE_GROW_PX: f32 = 2.5;
/// Emphasis above which a row is drawn semibold.
const BOLD_EMPHASIS: f32 = 0.5;
/// Row alpha at the centre, its fall-off per row and floor: synced lines.
const SYNCED_ALPHA: (f32, f32, f32) = (0.62, 0.2, 0.14);
/// Row alpha at the centre, its fall-off per row and floor: plain text.
const PLAIN_ALPHA: (f32, f32, f32) = (0.72, 0.18, 0.2);
/// Accent alpha of the unlit karaoke text and of near-centre rows.
const ACCENT_TEXT_ALPHA: f32 = 0.5;
/// Estimated glyph advance as a fraction of the text size (LTR / RTL),
/// used to size the karaoke glow and sweep.
const ADVANCE_LTR: f32 = 0.5;
const ADVANCE_RTL: f32 = 0.42;
/// Karaoke glow: vertical inset / height as fractions of a row, alpha,
/// blur and spread.
const GLOW_TOP_FRAC: f32 = 0.25;
const GLOW_H_FRAC: f32 = 0.5;
const GLOW_ALPHA: f32 = 0.22;
const GLOW_BLUR_PX: f32 = 18.0;
const GLOW_SPREAD_PX: f32 = 2.0;
/// Shown for blank synced lines (instrumental breaks).
const PAUSE_GLYPH: &str = "♪";

/// Note row: icon gap, icon size and text / icon alphas.
const NOTE_GAP_PX: f32 = 7.0;
const NOTE_ICON_PX: f32 = 13.0;
const NOTE_TEXT_ALPHA: f32 = 0.8;
const NOTE_ICON_ALPHA: f32 = 0.7;

/// One prepared lyric row.
struct Row {
    text: SharedString,
    rtl: bool,
    /// Character count, for the karaoke width estimate.
    chars: usize,
}

/// Lyrics converted for display.
struct Sheet {
    /// Start time of each row for synced lyrics; `None` for plain text.
    times: Option<Vec<Duration>>,
    rows: Vec<Row>,
}

impl Sheet {
    fn row(text: &str) -> Row {
        let text = if text.trim().is_empty() { PAUSE_GLYPH } else { text };
        Row { text: SharedString::from(text.to_string()), rtl: lyrics::is_rtl(text), chars: text.chars().count() }
    }

    /// Runs off the UI thread.
    fn prepare(l: Lyrics) -> Outcome {
        match l {
            Lyrics::Synced(lines) => Outcome::Sheet(Sheet {
                times: Some(lines.iter().map(|l| l.time).collect()),
                rows: lines.iter().map(|l| Sheet::row(&l.text)).collect(),
            }),
            Lyrics::Plain(lines) => Outcome::Sheet(Sheet { times: None, rows: lines.iter().map(|l| Sheet::row(l)).collect() }),
            Lyrics::Instrumental => Outcome::Instrumental,
            Lyrics::NotFound => Outcome::NotFound,
        }
    }
}

enum Outcome {
    Sheet(Sheet),
    Instrumental,
    NotFound,
}

enum State {
    Idle,
    Loading,
    Ready(Outcome),
    Failed(Instant),
}

pub struct LyricsPane {
    /// Visible rows (odd, [`MIN_ROWS`]..=[`MAX_ROWS`]).
    rows: u32,
    query: Option<TrackQuery>,
    state: State,
    scroll: SpringValue<f32>,
    clock: FrameClock,
}

/// `rows` as a visible row count: odd, within [`MIN_ROWS`]..=[`MAX_ROWS`].
fn rows_shown(rows: u32) -> u32 {
    (rows | 1).clamp(MIN_ROWS, MAX_ROWS)
}

/// Height of the pane showing `rows` (the setting, normalised like
/// [`LyricsPane::set_rows`]). The pane is this tall in every state —
/// looking up, found, not found, instrumental, unreachable — so the media
/// panel never grows or shrinks when a lookup finishes.
pub fn pane_height(rows: u32) -> f32 {
    SLOT * rows_shown(rows) as f32
}

/// The lrclib query for what's playing.
pub fn query_for(state: &MediaState) -> TrackQuery {
    make_query(&state.artist, &state.title, &state.album, state.length)
}

fn whole_secs(length: Option<Duration>) -> Option<u64> {
    length.map(|l| l.as_secs_f64().round() as u64).filter(|d| *d > 0)
}

fn make_query(artist: &str, title: &str, album: &str, length: Option<Duration>) -> TrackQuery {
    TrackQuery { artist: artist.to_string(), title: title.to_string(), album: album.to_string(), duration: whole_secs(length) }
}

/// Whether `q` is the query for `state` (no allocation, unlike comparing
/// against a fresh [`query_for`] every frame).
fn same_track(q: &TrackQuery, state: &MediaState) -> bool {
    q.title == state.title && q.artist == state.artist && q.album == state.album && q.duration == whole_secs(state.length)
}

impl LyricsPane {
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self { rows: DEFAULT_ROWS, query: None, state: State::Idle, scroll: SpringValue::new(0.0, SCROLL), clock: FrameClock::new() }))
    }

    /// Follows the playing track; fetches only when `fetch` (panel open).
    /// The result notifies only the view being rendered.
    pub fn sync(this: &Rc<RefCell<Self>>, state: &MediaState, fetch: bool, window: &mut Window, cx: &mut App) {
        let mut me = this.borrow_mut();
        if !me.query.as_ref().is_some_and(|q| same_track(q, state)) {
            me.query = Some(query_for(state));
            me.state = State::Idle;
            me.scroll = SpringValue::new(0.0, SCROLL);
        }
        let retry = matches!(me.state, State::Failed(at) if at.elapsed() > RETRY);
        if !fetch || !(matches!(me.state, State::Idle) || retry) {
            return;
        }
        let Some(q) = me.query.clone() else { return };
        me.state = State::Loading;
        drop(me);
        let task = cx.background_executor().spawn({
            let q = q.clone();
            async move { lyrics::fetch(&q).map(Sheet::prepare) }
        });
        let view = window.current_view();
        let this = this.clone();
        cx.spawn(async move |cx| {
            let result = task.await;
            {
                let mut me = this.borrow_mut();
                if me.query.as_ref() != Some(&q) {
                    return;
                }
                me.state = match result {
                    Ok(outcome) => State::Ready(outcome),
                    Err(err) => {
                        tracing::debug!("lyrics: {err}");
                        State::Failed(Instant::now())
                    }
                };
            }
            cx.update(|cx| cx.notify(view));
        })
        .detach();
    }

    /// Frees fetched lyric text (the panel closed). Small outcomes (not
    /// found, instrumental, a failure) are kept so the pane reopens saying
    /// the same; lyrics are re-read from the disk cache on reopen.
    pub fn release(&mut self) {
        if matches!(self.state, State::Ready(Outcome::Sheet(_)) | State::Loading) {
            self.state = State::Idle;
        }
        self.scroll = SpringValue::new(0.0, SCROLL);
        self.clock.idle();
    }

    /// Shows `rows` lyric rows (clamped, rounded up to an odd count).
    pub fn set_rows(&mut self, rows: u32) {
        self.rows = rows_shown(rows);
    }

    /// Height of the pane, the same in every state ([`pane_height`]).
    pub fn height(&self) -> f32 {
        pane_height(self.rows)
    }

    /// The pane for this frame and whether it is animating.
    pub fn element(&mut self, state: &MediaState, accent: (Color, Color), palette: &MandaraPalette, ui: &SharedString, arabic: &SharedString, width: f32) -> (AnyElement, bool) {
        let full_h = self.height();
        // Notes fill the whole pane, centred: a finished lookup changes
        // what the pane says, never how tall it is.
        let note = |icon_name: &'static str, label: &'static str| {
            div()
                .h(px(full_h))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(NOTE_GAP_PX))
                .text_size(px(text::BODY_S))
                .text_color(palette.on_surface_variant.with_alpha(NOTE_TEXT_ALPHA).hsla())
                .child(icon(icon_name, NOTE_ICON_PX, palette.on_surface_variant.with_alpha(NOTE_ICON_ALPHA)))
                .child(label)
                .into_any_element()
        };
        let half = self.rows as f32 / 2.0;
        let (row_window, row_cutoff, fade_from) = (half + ROW_WINDOW_EXTRA, half + ROW_CUTOFF_EXTRA, half - FADE_INSET_ROWS);
        let sheet = match &self.state {
            State::Idle | State::Loading => return (note("search", t("Looking for lyrics…")), false),
            State::Failed(_) => return (note("cloud", t("Couldn't reach lrclib.net")), false),
            State::Ready(Outcome::NotFound) => return (note("music", t("No lyrics found")), false),
            State::Ready(Outcome::Instrumental) => return (note("music", t("Instrumental")), false),
            State::Ready(Outcome::Sheet(sheet)) => sheet,
        };
        let now = Instant::now();
        let playing = state.status == PlaybackStatus::Playing;
        let pos = state.position_now(now) + LEAD;
        let mut current: Option<usize> = None;
        let mut sweep = 0.0;
        let target = match &sheet.times {
            Some(times) => {
                current = times.partition_point(|t| *t <= pos).checked_sub(1);
                if let Some(i) = current {
                    let start = times[i];
                    let end = times.get(i + 1).copied().unwrap_or(start + LAST_LINE_SPAN);
                    let span = end.saturating_sub(start).as_secs_f32().max(MIN_LINE_SPAN_S);
                    sweep = (pos.saturating_sub(start).as_secs_f32() / span).clamp(0.0, 1.0);
                }
                current.map_or(0.0, |i| i as f32)
            }
            // Unsynced: drift through the text with the track's progress.
            None => state.progress(now) * sheet.rows.len().saturating_sub(1) as f32,
        };
        self.scroll.set_target(target);
        let dt = self.clock.tick(now);
        let mut moving = self.scroll.step(dt);
        if moving && (self.scroll.value() - target).abs() < SCROLL_REST_ROWS {
            self.scroll.snap_to_target();
            moving = false;
        }
        if !moving {
            self.clock.idle();
        }
        let s = self.scroll.value();
        let synced = sheet.times.is_some();
        let (a, b) = accent;
        let (alpha_top, alpha_step, alpha_floor) = if synced { SYNCED_ALPHA } else { PLAIN_ALPHA };
        let lo = (s - row_window).floor().max(0.0) as usize;
        let hi = ((s + row_window).ceil().max(0.0) as usize).min(sheet.rows.len());
        let mut rows: SmallVec<[gpui::Div; MAX_ROWS as usize + 1]> = SmallVec::new();
        for (i, row_data) in sheet.rows.iter().enumerate().take(hi).skip(lo) {
            let d = i as f32 - s;
            if d.abs() > row_cutoff {
                continue;
            }
            let top = full_h / 2.0 + d * SLOT - SLOT / 2.0;
            let emphasis = if synced && current.is_some() { (1.0 - d.abs()).clamp(0.0, 1.0) } else { 0.0 };
            let fade = (1.0 - (d.abs() - fade_from).max(0.0)).clamp(0.0, 1.0);
            let is_current = synced && current == Some(i);
            let rtl = row_data.rtl;
            let size = LINE_TEXT_PX + LINE_GROW_PX * emphasis;
            let base_alpha = (alpha_top - alpha_step * d.abs()).clamp(alpha_floor, alpha_top);
            let dim = palette.on_surface.with_alpha(base_alpha * fade);
            let text_el = |color: Color| {
                div()
                    .w(px(width))
                    .h(px(SLOT))
                    .flex()
                    .items_center()
                    .when(rtl, |el| el.justify_end())
                    .font_family(if rtl { arabic.clone() } else { ui.clone() })
                    .text_size(px(size))
                    .font_weight(if emphasis > BOLD_EMPHASIS { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(color.hsla())
                    .whitespace_nowrap()
                    .child(div().min_w(px(0.0)).truncate().child(row_data.text.clone()))
            };
            let mut row = div().absolute().left_0().top(px(top)).w(px(width)).h(px(SLOT));
            if is_current {
                // Karaoke: the accent sweeps across the line as it is sung,
                // over a soft glow sized to the text.
                let est = (row_data.chars as f32 * size * if rtl { ADVANCE_RTL } else { ADVANCE_LTR }).min(width);
                let glow_box = div()
                    .absolute()
                    .top(px(SLOT * GLOW_TOP_FRAC))
                    .h(px(SLOT * GLOW_H_FRAC))
                    .w(px(est))
                    .when(rtl, |el| el.right_0())
                    .when(!rtl, |el| el.left_0())
                    .rounded_full()
                    .shadow(vec![glow(a.with_alpha(GLOW_ALPHA), GLOW_BLUR_PX, GLOW_SPREAD_PX, 0.0)]);
                let lit = a.mix(b, sweep);
                let clip = div()
                    .absolute()
                    .top_0()
                    .h(px(SLOT))
                    .w(px(if sweep >= SWEEP_FULL { width } else { est * sweep }))
                    .overflow_hidden()
                    .when(rtl, |el| el.right_0())
                    .when(!rtl, |el| el.left_0())
                    .child(div().absolute().top_0().when(rtl, |el| el.right_0()).when(!rtl, |el| el.left_0()).child(text_el(lit)));
                row = row.child(glow_box).child(div().absolute().top_0().left_0().child(text_el(a.with_alpha(ACCENT_TEXT_ALPHA)))).child(clip);
            } else {
                let color = if emphasis > 0.0 { dim.mix(a.with_alpha(ACCENT_TEXT_ALPHA), emphasis) } else { dim };
                row = row.child(text_el(color));
            }
            rows.push(row);
        }
        let el = div().relative().h(px(full_h)).w(px(width)).overflow_hidden().children(rows).into_any_element();
        (el, moving || (synced && playing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_rounds_duration_and_drops_zero() {
        assert_eq!(make_query("A", "T", "", Some(Duration::from_millis(200_600))).duration, Some(201));
        assert_eq!(make_query("A", "T", "", Some(Duration::ZERO)).duration, None);
    }

    #[test]
    fn rows_setting_is_odd_and_bounded() {
        assert_eq!(rows_shown(5), 5);
        assert_eq!(rows_shown(4), 5);
        assert_eq!(rows_shown(0), MIN_ROWS);
        assert_eq!(rows_shown(40), MAX_ROWS);
        let pane = LyricsPane::new();
        pane.borrow_mut().set_rows(7);
        assert_eq!(pane.borrow().height(), SLOT * 7.0);
    }

    #[test]
    fn pane_height_is_the_same_in_every_lookup_state() {
        let pane = LyricsPane::new();
        let full = pane_height(DEFAULT_ROWS);
        assert_eq!(full, SLOT * DEFAULT_ROWS as f32);
        let states = [
            State::Idle,
            State::Loading,
            State::Ready(Sheet::prepare(Lyrics::Plain(vec!["x".into()]))),
            State::Ready(Outcome::NotFound),
            State::Ready(Outcome::Instrumental),
            State::Failed(Instant::now()),
        ];
        for state in states {
            pane.borrow_mut().state = state;
            assert_eq!(pane.borrow().height(), full, "a finished lookup never resizes the media panel");
            pane.borrow_mut().release();
            assert_eq!(pane.borrow().height(), full, "nor does closing and reopening it");
        }
        pane.borrow_mut().state = State::Ready(Sheet::prepare(Lyrics::Plain(vec!["x".into()])));
        pane.borrow_mut().release();
        assert!(matches!(pane.borrow().state, State::Idle), "text is freed on close");
        assert_eq!(pane_height(4), pane_height(5), "the setting is normalised like set_rows");
    }

    #[test]
    fn prepared_rows_mark_pauses_and_direction() {
        let Outcome::Sheet(sheet) = Sheet::prepare(Lyrics::Synced(vec![
            lyrics::Line { time: Duration::from_secs(1), text: "  ".into() },
            lyrics::Line { time: Duration::from_secs(2), text: "ألا لا تلوماني".into() },
        ])) else {
            panic!("synced lyrics prepare to a sheet")
        };
        assert_eq!(sheet.rows[0].text.as_ref(), PAUSE_GLYPH);
        assert!(sheet.rows[1].rtl && !sheet.rows[0].rtl);
        assert_eq!(sheet.times.as_deref(), Some(&[Duration::from_secs(1), Duration::from_secs(2)][..]));
    }
}
