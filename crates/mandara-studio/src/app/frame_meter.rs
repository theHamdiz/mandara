//! Whole-window frame pacing for Studio, logged with
//! `MANDARA_FRAME_STATS=1` (the same switch and definitions as the login
//! screen's `pacing`): the interval between two frames of one continuous
//! animation, and the CPU time from the root view's render to the end of
//! paint (every view rebuilt, laid out and painted that frame). Off, it
//! costs one cached flag check per frame.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, canvas, div};
use mandara_greeter::pacing::{self, Pacing};

/// How often the summary is logged.
const LOG_EVERY: Duration = Duration::from_secs(4);

struct Meter {
    pacing: Pacing,
    logged_at: Instant,
    /// When the current frame's root render began.
    started: Option<Instant>,
}

thread_local! {
    static METER: RefCell<Option<Meter>> = const { RefCell::new(None) };
}

/// Whether [`pacing::STATS_ENV`] is set (read once).
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(pacing::enabled)
}

/// Marks the start of a frame (call first thing in the root render).
pub(super) fn begin() {
    if !enabled() {
        return;
    }
    METER.with(|m| {
        let now = Instant::now();
        m.borrow_mut().get_or_insert_with(|| Meter { pacing: Pacing::new(), logged_at: now, started: None }).started = Some(now);
    });
}

/// An empty element, painted last, that ends the frame begun by [`begin`].
pub(super) fn end() -> Option<AnyElement> {
    enabled().then(|| {
        div()
            .absolute()
            .size_0()
            .child(
                canvas(
                    |_, _, _| {},
                    |_, _, _, _| {
                        METER.with(|m| {
                            let mut m = m.borrow_mut();
                            let Some(m) = m.as_mut() else { return };
                            let Some(start) = m.started.take() else { return };
                            let now = Instant::now();
                            m.pacing.record(start, now.saturating_duration_since(start));
                            if now.saturating_duration_since(m.logged_at) >= LOG_EVERY {
                                m.logged_at = now;
                                tracing::info!("studio frames: {}", m.pacing.summary());
                                m.pacing = Pacing::new();
                            }
                        });
                    },
                )
                .size_0(),
            )
            .into_any_element()
    })
}
