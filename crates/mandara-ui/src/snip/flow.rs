//! The snipping tool's lifecycle as a pure state machine. `snip::session`
//! drives the surfaces from it (frozen overlay, idle toolbar, countdown);
//! keeping the rules here makes every Esc / button path unit-testable.
//!
//! ```text
//!            Capture              Captured
//!  Closed ───────────▶ Capturing ─────────▶ Snipping
//!    ▲  ╲ Delay(n)        ▲  ▲                │ Esc / pause
//!    │   ╲                │  │ Capture now    ▼
//!    │    ▶ Countdown ────┘  └─────────────  Idle ── Esc / close ──▶ Closed
//!    │        │  ▲ Delay(n) (toolbar or snipping toolbar)
//!    └────────┘  └──────────────────────────────┘
//!   Esc (started from outside)   Esc (started from the tool) → Idle
//! ```

/// Where Esc during a countdown goes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Resume {
    /// Back to the idle toolbar (the tool started the countdown).
    Idle,
    /// Closed (a command or the panel timer started it).
    Close,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// Nothing on screen.
    #[default]
    Closed,
    /// Grabbing a frame: every snip surface is hidden first.
    Capturing,
    /// The frozen frame is up: drag, pick or take the screen.
    Snipping,
    /// Paused: only the compact toolbar; the desktop is usable.
    Idle,
    /// Waiting before a capture; `left` whole seconds remain.
    Countdown { left: u32, resume: Resume },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// Capture a fresh frame now (open, Capture now).
    Capture,
    /// Capture after `secs` seconds (0: now).
    Delay(u32),
    /// The frame is ready.
    Captured,
    /// One second of the countdown passed.
    Tick,
    Escape,
    /// The pause button: leave snipping, keep the tool.
    Pause,
    /// The close button, a finished capture, a failed capture.
    Close,
}

impl Stage {
    /// The stage after `input`. Inputs that make no sense in a stage
    /// (a stale `Captured`, a `Tick` after Esc) leave it unchanged.
    pub fn next(self, input: Input) -> Stage {
        use Stage::*;
        match (self, input) {
            (_, Input::Close) => Closed,
            (_, Input::Capture) | (_, Input::Delay(0)) => Capturing,
            (Capturing, Input::Delay(_)) => Capturing,
            (stage, Input::Delay(secs)) => {
                let resume = match stage {
                    Idle | Snipping => Resume::Idle,
                    Countdown { resume, .. } => resume,
                    Closed | Capturing => Resume::Close,
                };
                Countdown { left: secs, resume }
            }
            (Capturing, Input::Captured) => Snipping,
            (Countdown { left, .. }, Input::Tick) if left <= 1 => Capturing,
            (Countdown { left, resume }, Input::Tick) => Countdown { left: left - 1, resume },
            (Snipping, Input::Escape | Input::Pause) => Idle,
            (Idle, Input::Escape) => Closed,
            (Countdown { resume: Resume::Idle, .. }, Input::Escape) => Idle,
            (Countdown { resume: Resume::Close, .. }, Input::Escape) => Closed,
            (stage, _) => stage,
        }
    }

    /// Whether a snip surface (overlay, toolbar or countdown) is meant to
    /// be on screen.
    pub fn visible(self) -> bool {
        matches!(self, Stage::Snipping | Stage::Idle | Stage::Countdown { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: Stage, inputs: &[Input]) -> Stage {
        inputs.iter().fold(from, |s, i| s.next(*i))
    }

    #[test]
    fn snipping_pauses_to_idle_then_captures_again() {
        let s = run(Stage::Closed, &[Input::Capture, Input::Captured]);
        assert_eq!(s, Stage::Snipping);
        assert_eq!(s.next(Input::Escape), Stage::Idle, "Esc leaves snipping without closing");
        assert_eq!(s.next(Input::Pause), Stage::Idle, "so does the pause button");
        assert_eq!(run(s, &[Input::Escape, Input::Capture, Input::Captured]), Stage::Snipping, "Capture now re-enters snipping");
    }

    #[test]
    fn countdown_from_idle_ticks_into_snipping() {
        let idle = run(Stage::Closed, &[Input::Capture, Input::Captured, Input::Escape]);
        let s = idle.next(Input::Delay(3));
        assert_eq!(s, Stage::Countdown { left: 3, resume: Resume::Idle });
        let s = run(s, &[Input::Tick, Input::Tick]);
        assert_eq!(s, Stage::Countdown { left: 1, resume: Resume::Idle });
        let s = s.next(Input::Tick);
        assert_eq!(s, Stage::Capturing, "the frame is taken once the countdown is gone");
        assert_eq!(s.next(Input::Captured), Stage::Snipping);
    }

    #[test]
    fn countdown_from_the_snipping_toolbar_resumes_idle() {
        let s = run(Stage::Closed, &[Input::Capture, Input::Captured, Input::Delay(5)]);
        assert_eq!(s, Stage::Countdown { left: 5, resume: Resume::Idle });
        assert_eq!(s.next(Input::Escape), Stage::Idle);
    }

    #[test]
    fn esc_in_each_stage() {
        assert_eq!(Stage::Closed.next(Input::Escape), Stage::Closed);
        assert_eq!(Stage::Capturing.next(Input::Escape), Stage::Capturing, "no surface takes keys while capturing");
        assert_eq!(Stage::Snipping.next(Input::Escape), Stage::Idle);
        assert_eq!(Stage::Idle.next(Input::Escape), Stage::Closed, "Esc on the idle toolbar closes the tool");
        assert_eq!(Stage::Countdown { left: 2, resume: Resume::Idle }.next(Input::Escape), Stage::Idle);
        assert_eq!(Stage::Countdown { left: 2, resume: Resume::Close }.next(Input::Escape), Stage::Closed);
    }

    #[test]
    fn commands_count_down_from_closed_and_esc_closes() {
        let s = Stage::Closed.next(Input::Delay(3));
        assert_eq!(s, Stage::Countdown { left: 3, resume: Resume::Close });
        assert_eq!(s.next(Input::Escape), Stage::Closed);
        assert_eq!(run(s, &[Input::Tick, Input::Tick, Input::Tick]), Stage::Capturing);
        assert_eq!(Stage::Closed.next(Input::Delay(0)), Stage::Capturing, "no delay captures at once");
    }

    #[test]
    fn stale_inputs_are_ignored() {
        assert_eq!(Stage::Idle.next(Input::Captured), Stage::Idle, "a frame that arrives after Esc");
        assert_eq!(Stage::Idle.next(Input::Tick), Stage::Idle, "a tick after the countdown was cancelled");
        assert_eq!(Stage::Capturing.next(Input::Delay(3)), Stage::Capturing);
        let restarted = Stage::Countdown { left: 1, resume: Resume::Close }.next(Input::Delay(4));
        assert_eq!(restarted, Stage::Countdown { left: 4, resume: Resume::Close }, "a new delay restarts and keeps where Esc goes");
        for s in [Stage::Snipping, Stage::Idle, Stage::Countdown { left: 1, resume: Resume::Idle }, Stage::Capturing] {
            assert_eq!(s.next(Input::Close), Stage::Closed);
        }
    }

    #[test]
    fn visibility() {
        assert!(!Stage::Closed.visible());
        assert!(!Stage::Capturing.visible(), "nothing of the tool may be in the frame");
        assert!(Stage::Idle.visible() && Stage::Snipping.visible());
    }
}
