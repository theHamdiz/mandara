//! Cached task metadata resolves its status and colours against the live frame.

use mandara_i18n::t;
use mandara_services::progress::{Status, Task};
use mandara_theme::{Color, MandaraPalette};

/// Completion state or the next stall boundary, independent of theme and locale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskState {
    Running { stalls_at: i64 },
    Done,
    Failed,
}

impl From<&Task> for TaskState {
    fn from(task: &Task) -> Self {
        match task.status {
            Status::Running => Self::Running { stalls_at: task.stalls_at().expect("running task has a stall deadline") },
            Status::Done => Self::Done,
            Status::Failed => Self::Failed,
        }
    }
}

impl TaskState {
    /// Theme transitions use current tokens even while formatted text stays cached.
    pub fn color(self, now: i64, palette: &MandaraPalette) -> Color {
        match self {
            Self::Running { stalls_at } if now >= stalls_at => palette.warning_amber,
            Self::Running { .. } => palette.primary,
            Self::Done => palette.success_emerald,
            Self::Failed => palette.urgent_coral,
        }
    }

    /// A deadline inside the current second must not wait for the text-cache tick.
    pub fn label(self, now: i64) -> &'static str {
        match self {
            Self::Running { stalls_at } if now >= stalls_at => t("Stalled"),
            Self::Running { .. } => t("Running"),
            Self::Done => t("Done"),
            Self::Failed => t("Failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_i18n::{Lang, scoped, t_in};
    use mandara_services::progress::{Board, STALL_AFTER_MS};

    /// The same cached state follows light/dark tokens, locale and an exact deadline.
    #[test]
    fn cached_task_state_uses_live_palette_language_and_stall_time() {
        let mut board = Board::default();
        board.start("build", "Build", 1).unwrap();
        let state = TaskState::from(&board.tasks[0]);
        let theme = &mandara_theme::catalog::builtin()[0];
        for dark in [false, true] {
            let palette = theme.palette(dark);
            assert_eq!(state.color(1, &palette), palette.primary);
            assert_eq!(state.color(STALL_AFTER_MS, &palette), palette.primary);
            assert_eq!(state.color(STALL_AFTER_MS + 1, &palette), palette.warning_amber);
            assert_eq!(TaskState::Done.color(1, &palette), palette.success_emerald);
            assert_eq!(TaskState::Failed.color(1, &palette), palette.urgent_coral);
        }
        for language in [Lang::En, Lang::Ar] {
            let _language = scoped(Some(language));
            assert_eq!(state.label(STALL_AFTER_MS), t_in(language, "Running"));
            assert_eq!(state.label(STALL_AFTER_MS + 1), t_in(language, "Stalled"));
        }
    }
}
