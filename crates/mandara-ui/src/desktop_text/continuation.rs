//! One latest desktop action waits for an editor's successful save.

pub(super) struct Continuation<T>(Option<T>);

impl<T> Continuation<T> {
    pub fn new() -> Self { Self(None) }
    /// Later navigation replaces an earlier unsent navigation, keeping storage bounded.
    pub fn defer(&mut self, action: T) { self.0 = Some(action); }
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
}
