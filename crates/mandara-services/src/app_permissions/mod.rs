//! Inspect Flatpak sandbox access and change one personal override at a time.

mod model;
mod native;
mod transaction;
#[cfg(test)]
mod tests;

pub use model::{App, Capability, Constraint, Key, Session};
pub use native::Flatpak;
pub use transaction::Undo;

use anyhow::Result;
use std::sync::atomic::AtomicBool;

/// A native installation supplies effective permissions; views do not parse CLI output.
pub trait Backend {
    fn apps(&self, cancel: &AtomicBool) -> Result<Vec<App>>;
    fn inspect(&self, app: &App, cancel: &AtomicBool) -> Result<Session>;
    fn set(&self, session: &Session, capability: &Capability, allowed: bool, cancel: &AtomicBool) -> Result<Undo>;
}
