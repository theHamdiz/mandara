//! A progress fill: a card or button whose own background fills with a
//! task's real progress (a language pack or an update downloading), turns
//! red when it fails and green, with a check mark that pops in, when it
//! succeeds, then settles back into its normal look.
//!
//! The model is pure ([`ProgressFill::observe`] takes what the task reports,
//! [`ProgressFill::frame`] steps the springs and says what to draw), so it
//! is unit-tested without a window; [`layer`] and [`check_badge`] draw it.
//!
//! - The fill grows from the reading start (left, or right in
//!   right-to-left languages) in proportion to the bytes received, gliding
//!   between updates on a spring instead of jumping.
//! - Nothing redraws while nothing changes: the caller requests animation
//!   frames only while [`Visual::animating`], and wakes once for the end of
//!   the success hold ([`Visual::wake_in`]).
//! - Reduce motion: every change is instant, nothing pops.
//! - The fills are tints, light enough that the card's text keeps its
//!   contrast in every state (checked by the tests for both default
//!   palettes).

use std::time::{Duration, Instant};

use gpui::{AnyElement, IntoElement, ParentElement, Styled, div, px, relative};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_theme::{Color, MandaraPalette};

use crate::icons::icon;

/// How long the success state shows before the card settles.
pub const SUCCESS_HOLD: Duration = Duration::from_millis(1100);
/// Stiffness of the fill's (critically damped) glide between progress updates.
const FILL_STIFFNESS: f32 = 140.0;
/// The check mark's pop: a little under-damped, so it overshoots once.
const CHECK_SPRING: SpringConfig = SpringConfig { stiffness: 420.0, damping: 18.0, mass: 1.0 };
/// Tint of the fill while working (primary).
pub const PROGRESS_ALPHA: f32 = 0.18;
/// Tint of a failed card (the palette's error colour).
pub const ERROR_ALPHA: f32 = 0.18;
/// Tint of a card that just succeeded (the palette's success colour).
pub const SUCCESS_ALPHA: f32 = 0.18;
/// Size of the success check badge, px.
pub const CHECK_SIZE_PX: f32 = 20.0;
/// Size of the check glyph inside the badge, px.
const CHECK_GLYPH_PX: f32 = 12.0;

/// What the task reports right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    /// Nothing running (or nothing this card shows).
    Idle,
    /// Downloading: bytes so far of `total` (0: size unknown yet).
    Progress { done: u64, total: u64 },
    /// Working without a byte count (applying, verifying, installing).
    Working,
    /// The task failed.
    Failed,
    /// The task finished.
    Done,
}

/// Where the fill is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Failed,
    /// Succeeded at this instant; settles after [`SUCCESS_HOLD`].
    Succeeded(Instant),
}

/// The colour role of the fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    None,
    Progress,
    Error,
    Success,
}

/// What to draw this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    /// Width of the fill, 0–1 of the card.
    pub fill: f32,
    pub tone: Tone,
    /// Scale of the success check badge (0: hidden; overshoots past 1 once).
    pub check: f32,
    /// A spring is still moving: request the next animation frame.
    pub animating: bool,
    /// Redraw once after this long (the end of the success hold).
    pub wake_in: Option<Duration>,
}

/// `done` of `total` bytes as a fill fraction, 0–1 (0 while the size is
/// unknown).
pub fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 }
}

/// Which side the fill grows from: `true` for the right edge (right-to-left
/// reading), `false` for the left.
pub fn grows_from_right(rtl: bool) -> bool {
    rtl
}

/// One card's or button's fill.
pub struct ProgressFill {
    phase: Phase,
    fill: SpringValue<f32>,
    check: SpringValue<f32>,
    clock: FrameClock,
    /// A wake-up for the hold's end is already scheduled.
    wake_pending: bool,
}

impl Default for ProgressFill {
    fn default() -> Self {
        Self { phase: Phase::Idle, fill: SpringValue::new(0.0, SpringConfig::critical(FILL_STIFFNESS)), check: SpringValue::new(0.0, CHECK_SPRING), clock: FrameClock::new(), wake_pending: false }
    }
}

impl ProgressFill {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Takes what the task reports; `true` when the look changes. A task
    /// that is already done when first seen (a pack that was installed
    /// before) shows nothing: only a finish this fill watched celebrates.
    pub fn observe(&mut self, now: Observed, at: Instant) -> bool {
        let before = (self.phase, self.fill.target(), self.check.target());
        match now {
            Observed::Progress { done, total } => {
                self.phase = Phase::Running;
                self.fill.set_target(fraction(done, total));
                self.check.set_target(0.0);
            }
            Observed::Working => {
                self.phase = Phase::Running;
                self.fill.set_target(1.0);
                self.check.set_target(0.0);
            }
            Observed::Failed => {
                self.phase = Phase::Failed;
                self.fill.set_target(1.0);
                self.check.set_target(0.0);
            }
            Observed::Done => match self.phase {
                Phase::Running | Phase::Failed => {
                    self.phase = Phase::Succeeded(at);
                    self.fill.set_target(1.0);
                    self.check.set_target(1.0);
                    self.wake_pending = false;
                }
                Phase::Idle | Phase::Succeeded(_) => {}
            },
            Observed::Idle => {
                if matches!(self.phase, Phase::Running | Phase::Failed) {
                    self.settle();
                }
            }
        }
        before != (self.phase, self.fill.target(), self.check.target())
    }

    /// Back to the normal look at once.
    fn settle(&mut self) {
        self.phase = Phase::Idle;
        self.fill.snap_to(0.0);
        self.check.snap_to(0.0);
        self.wake_pending = false;
    }

    /// Steps the springs to `at` and says what to draw. With `reduce`
    /// (Reduce motion) every change is instant.
    pub fn frame(&mut self, reduce: bool, at: Instant) -> Visual {
        if let Phase::Succeeded(since) = self.phase
            && at.saturating_duration_since(since) >= SUCCESS_HOLD
        {
            self.settle();
        }
        self.fill.set_instant(reduce);
        self.check.set_instant(reduce);
        let dt = self.clock.tick(at);
        let moving = self.fill.step(dt) | self.check.step(dt);
        if !moving {
            self.clock.idle();
        }
        let tone = match self.phase {
            Phase::Idle => Tone::None,
            Phase::Running => Tone::Progress,
            Phase::Failed => Tone::Error,
            Phase::Succeeded(_) => Tone::Success,
        };
        let wake_in = match self.phase {
            Phase::Succeeded(since) if !self.wake_pending => {
                self.wake_pending = true;
                Some(SUCCESS_HOLD.saturating_sub(at.saturating_duration_since(since)))
            }
            _ => None,
        };
        let fill = if tone == Tone::None { 0.0 } else { self.fill.value().clamp(0.0, 1.0) };
        Visual { fill, tone, check: self.check.value().max(0.0), animating: moving, wake_in }
    }

    /// Whether anything shows (a fill, red or green).
    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }
}

/// What `lang`'s language pack download reports in this process, and the
/// line a card shows for it: the bytes so far ("180 KB / 299 KB", short
/// enough for a card that already names the language), or why it failed.
pub fn language_pack(lang: mandara_i18n::Lang) -> (Observed, Option<String>) {
    use mandara_services::self_update::{human_size, langpacks::{self, State}};
    match langpacks::state(lang) {
        None => (Observed::Idle, None),
        Some(State::Ready) => (Observed::Done, None),
        Some(State::Failed(why)) => (Observed::Failed, Some(why)),
        Some(State::Downloading { done, total }) if total > 0 => (Observed::Progress { done, total }, Some(format!("{} / {}", human_size(done), human_size(total)))),
        Some(s @ (State::Checking | State::Downloading { .. })) => {
            let (done, total) = if let State::Downloading { done, total } = s { (done, total) } else { (0, 0) };
            (Observed::Progress { done, total }, langpacks::describe(lang, &s))
        }
    }
}

/// Every language card's fill in one place: steps them all for this frame
/// and returns each one's look and line. Asks for the next animation frame
/// while one moves; `wake` is called with the delay of a success hold's end.
#[derive(Default)]
pub struct LanguageFills(std::collections::HashMap<mandara_i18n::Lang, ProgressFill>);

impl LanguageFills {
    pub fn frame(&mut self, reduce: bool, window: &mut gpui::Window, mut wake: impl FnMut(Duration)) -> std::collections::HashMap<mandara_i18n::Lang, (Visual, Option<String>)> {
        let now = Instant::now();
        let mut out = std::collections::HashMap::new();
        for lang in mandara_i18n::Lang::ALL.into_iter().filter(|l| *l != mandara_i18n::Lang::En) {
            let (observed, line) = language_pack(lang);
            let f = self.0.entry(lang).or_default();
            f.observe(observed, now);
            let v = f.frame(reduce, now);
            if v.animating {
                window.request_animation_frame();
            }
            if let Some(after) = v.wake_in {
                wake(after);
            }
            out.insert(lang, (v, line));
        }
        out
    }
}

/// A card's own background under its fill: `normal` while nothing shows,
/// the plain `surface` while a fill does (a selected card's tint under the
/// fill would cost its text contrast; the selection still shows in its
/// outline).
pub fn base(v: &Visual, normal: Color, surface: Color) -> Color {
    if v.tone == Tone::None { normal } else { surface }
}

/// The look of a card with nothing going on.
pub const IDLE: Visual = Visual { fill: 0.0, tone: Tone::None, check: 0.0, animating: false, wake_in: None };

/// The tint for `tone` (`None` draws nothing).
pub fn tint(tone: Tone, p: &MandaraPalette) -> Option<Color> {
    match tone {
        Tone::None => None,
        Tone::Progress => Some(p.primary.with_alpha(PROGRESS_ALPHA)),
        Tone::Error => Some(p.urgent_coral.with_alpha(ERROR_ALPHA)),
        Tone::Success => Some(p.success_emerald.with_alpha(SUCCESS_ALPHA)),
    }
}

/// The fill layer: put it first inside a `relative()`, clipped card so the
/// card's content draws over it. `None` when there is nothing to draw.
pub fn layer(v: &Visual, rtl: bool, p: &MandaraPalette) -> Option<AnyElement> {
    let color = tint(v.tone, p)?;
    if v.fill <= 0.0 {
        return None;
    }
    let bar = div().absolute().top_0().bottom_0().w(relative(v.fill)).bg(color.hsla());
    Some(if grows_from_right(rtl) { bar.right_0() } else { bar.left_0() }.into_any_element())
}

/// The success check: a round success-coloured badge, scaled by
/// [`Visual::check`]. `None` while hidden.
pub fn check_badge(v: &Visual, p: &MandaraPalette) -> Option<AnyElement> {
    if v.tone != Tone::Success || v.check <= 0.0 {
        return None;
    }
    let size = CHECK_SIZE_PX * v.check;
    Some(
        div()
            .size(px(CHECK_SIZE_PX))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(size)).rounded_full().bg(p.success_emerald.hsla()).flex().items_center().justify_center().child(icon("check", CHECK_GLYPH_PX * v.check.min(1.0), p.on_primary)))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    /// Runs frames until the springs rest (or a second passes).
    fn settle(f: &mut ProgressFill, mut at: Instant, reduce: bool) -> (Visual, Instant) {
        let mut v = f.frame(reduce, at);
        for _ in 0..120 {
            if !v.animating {
                break;
            }
            at += FRAME;
            v = f.frame(reduce, at);
        }
        (v, at)
    }

    #[test]
    fn progress_maps_to_a_clamped_fraction() {
        assert_eq!(fraction(0, 0), 0.0, "unknown size: empty");
        assert_eq!(fraction(50, 200), 0.25);
        assert_eq!(fraction(300, 200), 1.0, "never past full");
        assert_eq!(fraction(u64::MAX, u64::MAX), 1.0);
    }

    #[test]
    fn the_fill_glides_to_the_bytes_received() {
        let mut f = ProgressFill::default();
        let t0 = Instant::now();
        assert!(f.observe(Observed::Progress { done: 40, total: 100 }, t0));
        let first = f.frame(false, t0);
        assert!(first.animating && first.fill < 0.4, "it glides, it doesn't jump");
        let (v, at) = settle(&mut f, t0, false);
        assert!((v.fill - 0.4).abs() < 0.01 && v.tone == Tone::Progress);
        // The same report again changes nothing and asks for no frame.
        assert!(!f.observe(Observed::Progress { done: 40, total: 100 }, at));
        assert!(!f.frame(false, at + FRAME).animating);
    }

    #[test]
    fn failure_turns_red_and_success_pops_then_settles() {
        let t0 = Instant::now();
        let mut f = ProgressFill::default();
        f.observe(Observed::Progress { done: 10, total: 100 }, t0);
        f.observe(Observed::Failed, t0);
        let (v, at) = settle(&mut f, t0, false);
        assert_eq!((v.tone, f.phase()), (Tone::Error, Phase::Failed));
        assert!(v.fill > 0.99, "a failed card is tinted all over");
        // Retry, then success: green, the check overshoots once, and after
        // the hold the card is back to normal.
        f.observe(Observed::Working, at);
        f.observe(Observed::Done, at);
        let mut peak = 0.0f32;
        let mut t = at;
        let mut wake = None;
        for _ in 0..60 {
            let v = f.frame(false, t);
            peak = peak.max(v.check);
            wake = wake.or(v.wake_in);
            assert_eq!(v.tone, Tone::Success);
            t += FRAME;
        }
        assert!(peak > 1.0, "the check pops past its size once ({peak})");
        assert!(wake.is_some_and(|w| w <= SUCCESS_HOLD), "one wake-up for the end of the hold");
        let after = f.frame(false, at + SUCCESS_HOLD);
        assert_eq!((after.tone, after.fill, f.phase()), (Tone::None, 0.0, Phase::Idle));
    }

    #[test]
    fn a_task_already_done_when_first_seen_shows_nothing() {
        let mut f = ProgressFill::default();
        assert!(!f.observe(Observed::Done, Instant::now()));
        assert_eq!(f.frame(false, Instant::now()).tone, Tone::None);
        assert!(!f.active());
    }

    #[test]
    fn reduce_motion_changes_everything_at_once() {
        let t0 = Instant::now();
        let mut f = ProgressFill::default();
        f.observe(Observed::Progress { done: 70, total: 100 }, t0);
        let v = f.frame(true, t0);
        assert!((v.fill - 0.7).abs() < 1e-6 && !v.animating);
        f.observe(Observed::Done, t0);
        let v = f.frame(true, t0);
        assert_eq!((v.fill, v.check, v.animating), (1.0, 1.0, false), "no glide, no pop");
    }

    #[test]
    fn the_fill_grows_from_the_reading_start() {
        assert!(!grows_from_right(false));
        assert!(grows_from_right(true), "right to left: from the right edge");
    }

    #[test]
    fn text_stays_readable_on_every_fill() {
        for dark in [true, false] {
            let p = mandara_theme::engine::default_palette(dark);
            // While a fill shows, a card drops its own selected tint
            // ([`base`]), so the fill always lies on a plain surface.
            for card in [p.surface_container_high, p.surface_container] {
                for tone in [Tone::Progress, Tone::Error, Tone::Success] {
                    let bg = tint(tone, &p).unwrap().over(card);
                    let ratio = p.on_surface.contrast_ratio(bg);
                    assert!(ratio >= 4.5, "{tone:?} on {} card: contrast {ratio:.2}", if dark { "dark" } else { "light" });
                }
            }
        }
    }
}
