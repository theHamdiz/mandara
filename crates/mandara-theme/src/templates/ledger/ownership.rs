//! Existing imports remain personal when an owned block changes.

use super::BlockEntry;

pub(super) fn conflicts(body: &str, previous: Option<&BlockEntry>, next: &[&str]) -> bool {
    body.lines().any(|actual| {
        let line = actual.trim();
        !line.is_empty()
            && !line.starts_with('[')
            && next.iter().any(|candidate| candidate.trim() == line)
            && !previous.is_some_and(|entry| entry.lines.iter().any(|owned| owned.trim() == line))
    })
}
