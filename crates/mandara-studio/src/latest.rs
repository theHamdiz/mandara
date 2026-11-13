//! One running read with one replaceable request waiting behind it.

pub(crate) struct Request<T> {
    pub key: T,
    revision: u64,
}

pub(crate) struct Latest<T> {
    desired: Option<T>,
    active: Option<u64>,
    revision: u64,
    finished: u64,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            desired: None,
            active: None,
            revision: 0,
            finished: 0,
        }
    }
}

impl<T: Clone + PartialEq> Latest<T> {
    pub fn desired(&self) -> Option<&T> {
        self.desired.as_ref()
    }

    pub fn set(&mut self, desired: Option<T>) {
        if self.desired != desired {
            self.desired = desired;
            self.refresh();
        }
    }

    pub fn refresh(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn begin(&mut self) -> Option<Request<T>> {
        if self.active.is_some() || self.finished == self.revision {
            return None;
        }
        let key = self.desired.clone()?;
        self.active = Some(self.revision);
        Some(Request {
            key,
            revision: self.revision,
        })
    }

    /// Reject a result from before cancellation, refresh, or a newer request.
    pub fn finish(&mut self, request: Request<T>) -> bool {
        if self.active != Some(request.revision) {
            return false;
        }
        self.active = None;
        self.finished = request.revision;
        self.revision == request.revision && self.desired.as_ref() == Some(&request.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_requests_keep_one_running_job_and_only_the_latest_waiter() {
        let mut queue = Latest::default();
        queue.set(Some("first"));
        let first = queue.begin().unwrap();
        for next in ["second", "third", "latest"] {
            queue.set(Some(next));
        }
        assert!(queue.begin().is_none());
        assert!(!queue.finish(first));
        let next = queue.begin().unwrap();
        assert_eq!(next.key, "latest");
        assert!(queue.finish(next));
        assert!(queue.begin().is_none());
    }

    #[test]
    fn cancellation_drops_the_pending_owner_and_rejects_a_late_result() {
        let value = std::rc::Rc::new("source");
        let weak = std::rc::Rc::downgrade(&value);
        let mut queue = Latest::default();
        queue.set(Some(value));
        let request = queue.begin().unwrap();
        queue.set(None);
        assert!(!queue.finish(request));
        assert!(queue.begin().is_none());
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn refresh_of_the_same_input_does_not_accept_the_previous_result() {
        let mut queue = Latest::default();
        queue.set(Some("source"));
        let previous = queue.begin().unwrap();
        queue.refresh();
        assert!(!queue.finish(previous));
        let current = queue.begin().unwrap();
        assert!(queue.finish(current));
    }
}
