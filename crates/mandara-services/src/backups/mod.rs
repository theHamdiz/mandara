//! User-started encrypted snapshots. No service, scan or timer exists while idle.

mod model;
mod restic;
pub mod filesystem;
pub mod cleanup;
#[cfg(test)]
mod tests;

pub use model::{MAX_PATH_BYTES, MAX_SOURCES, Operation, Progress, Settings, Snapshot, Summary};
pub use restic::Restic;

use anyhow::Result;
use std::sync::atomic::AtomicBool;

/// Storage implementations keep their protocol and process handling out of views.
pub trait Backend {
    fn snapshots(&self, settings: &Settings, cancel: &AtomicBool) -> Result<Vec<Snapshot>>;
    fn execute(&self, settings: &Settings, operation: Operation, cancel: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<Summary>;
}
