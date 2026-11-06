//! Borrow running tasks first without collecting a temporary display order.

use mandara_services::progress::{Status, Task};

/// Keeps each status group's snapshot order; callers apply their visible row limit.
pub(super) fn ordered_tasks(tasks: &[Task]) -> impl Iterator<Item = &Task> + Clone {
    tasks.iter().filter(|task| task.status == Status::Running)
        .chain(tasks.iter().filter(|task| task.status != Status::Running))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::progress::Board;

    /// Each group keeps the board's newest-first order before the row limit applies.
    #[test]
    fn ordered_rows_match_the_previous_materialized_groups() {
        let mut board = Board::default();
        for id in ["old", "finished", "new"] { board.start(id, id, 0).unwrap(); }
        board.done("finished", "", 1).unwrap();
        let mut old: Vec<_> = board.running().collect();
        old.extend(board.tasks.iter().filter(|task| task.status != Status::Running));
        for limit in 0..=4 {
            assert_eq!(ordered_tasks(&board.tasks).take(limit).map(|task| task.id.as_str()).collect::<Vec<_>>(), old.iter().take(limit).map(|task| task.id.as_str()).collect::<Vec<_>>());
        }
        assert_eq!(ordered_tasks(&board.tasks).map(|task| task.id.as_str()).collect::<Vec<_>>(), ["new", "old", "finished"]);
        assert!(ordered_tasks(&[]).next().is_none());
    }
}
