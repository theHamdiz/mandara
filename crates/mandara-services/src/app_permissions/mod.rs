//! Inspect Flatpak sandbox access and change one personal override at a time.

mod model;
mod native;
mod output;
mod paths;
mod removal;
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

/// A checked edit lists apps, reads two metadata views twice and generates one override.
const MAX_OPERATION_COMMANDS: u32 = 6;
/// IPC waits for the complete sequence, including each native command's bounded deadline.
pub const OPERATION_TIMEOUT: std::time::Duration = crate::app_themes::process::TIMEOUT.saturating_mul(MAX_OPERATION_COMMANDS);
