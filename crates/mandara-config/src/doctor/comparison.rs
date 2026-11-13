//! Compare native checks without tying a theme-only checker to a compositor backend.

use super::{CheckerStatus, Report};

pub trait CheckResult {
    fn ran(&self) -> bool;
    fn counts(&self) -> (usize, usize);
}
impl CheckResult for Report {
    fn ran(&self) -> bool {
        self.status == CheckerStatus::Ran
    }
    fn counts(&self) -> (usize, usize) {
        (self.errors(), self.warnings())
    }
}

/// A stopped checker, more errors, or as many errors and more warnings is worse.
pub fn is_worse(before: &impl CheckResult, after: &impl CheckResult) -> bool {
    (before.ran() && !after.ran()) || after.counts() > before.counts()
}
