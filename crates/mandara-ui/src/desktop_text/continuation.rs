//! One latest desktop action waits for an editor's successful save.

pub(super) struct Continuation<T>(Option<T>);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AfterWrite { KeepEditing, SaveAgain, Finish }

impl<T> Continuation<T> {
    pub fn new() -> Self { Self(None) }
    /// Later navigation replaces an earlier unsent navigation, keeping storage bounded.
    pub fn defer(&mut self, action: T) { self.0 = Some(action); }
    /// A queued action follows the latest field, while ordinary typing keeps its editor.
    pub fn after_write(&self, changed: bool) -> AfterWrite {
        if !changed { AfterWrite::Finish }
        else if self.0.is_some() { AfterWrite::SaveAgain }
        else { AfterWrite::KeepEditing }
    }
    /// A conflict or IO failure retains the action for the same draft's retry.
    pub fn complete(&mut self, succeeded: bool) -> Option<T> {
        if succeeded { self.0.take() } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_save_preserves_the_latest_action_and_success_delivers_it_once() {
        let mut next = Continuation::new();
        next.defer("first widget");
        assert_eq!(next.complete(false), None);
        next.defer("desktop menu");
        assert_eq!(next.complete(false), None);
        assert_eq!(next.complete(true), Some("desktop menu"));
        assert_eq!(next.complete(true), None);
    }

    #[test]
    fn typing_during_a_save_starts_a_followup_only_when_navigation_is_waiting() {
        let mut next = Continuation::new();
        assert_eq!(next.after_write(true), AfterWrite::KeepEditing);
        next.defer("other widget");
        assert_eq!(next.after_write(true), AfterWrite::SaveAgain);
        assert_eq!(next.complete(false), None);
        assert_eq!(next.after_write(true), AfterWrite::SaveAgain);
        assert_eq!(next.after_write(false), AfterWrite::Finish);
        assert_eq!(next.complete(true), Some("other widget"));
        assert_eq!(next.after_write(true), AfterWrite::KeepEditing);
    }
}
