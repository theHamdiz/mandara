//! One export owns the worker slot until all of its work has finished.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub(crate) struct Gate(Arc<AtomicBool>);

pub(crate) struct Lease {
    gate: Arc<AtomicBool>,
    token: Token,
}
#[derive(Clone)]
pub(crate) struct Token(Arc<AtomicBool>);

impl Gate {
    pub fn acquire(&self) -> Option<Lease> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Lease {
            gate: self.0.clone(),
            token: Token(Arc::new(AtomicBool::new(false))),
        })
    }
}
impl Token {
    pub fn current(&self) -> bool {
        !self.0.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}
impl Lease {
    pub fn token(&self) -> Token {
        self.token.clone()
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.token.cancel();
        self.gate.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_does_not_open_a_second_slot_before_the_old_worker_finishes() {
        let gate = Gate::default();
        let lease = gate.acquire().unwrap();
        let token = lease.token();
        token.cancel();
        assert!(!token.current());
        assert!(gate.acquire().is_none());
        drop(lease);
        let reopened = gate.acquire().unwrap();
        assert!(reopened.token().current());
        assert!(
            !token.current(),
            "an older export cannot become current again"
        );
    }

    #[test]
    fn dropping_the_owner_invalidates_its_outstanding_token() {
        let gate = Gate::default();
        let lease = gate.acquire().unwrap();
        let token = lease.token();
        drop(lease);
        assert!(!token.current());
        assert!(gate.acquire().is_some());
    }
}
