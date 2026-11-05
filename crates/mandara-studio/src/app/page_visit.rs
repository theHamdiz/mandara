//! Identity of one visit, retained by replies without retaining page resources.

use std::rc::Rc;

#[derive(Clone)]
pub(super) struct PageVisit(Rc<()>);

impl Default for PageVisit {
    fn default() -> Self { Self(Rc::new(())) }
}

impl PageVisit {
    /// A reply belongs only to the visit that started it, even after reopening.
    pub(super) fn accepts(&self, reply: &Self) -> bool { Rc::ptr_eq(&self.0, &reply.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_reply_cannot_repopulate_a_reopened_page() {
        let mut current = PageVisit::default();
        let reply = current.clone();
        assert!(current.accepts(&reply));
        current = PageVisit::default();
        assert!(!current.accepts(&reply));
        let reopened_reply = current.clone();
        assert!(current.accepts(&reopened_reply));
        current = PageVisit::default();
        assert!(!current.accepts(&reply));
        assert!(!current.accepts(&reopened_reply));
    }
}
