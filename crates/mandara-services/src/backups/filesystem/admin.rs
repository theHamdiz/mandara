//! The root-owned helper re-reads mount and snapshot identities before each fixed action.

use std::ffi::OsString;
use std::process::Command;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::io::Read as _;
use anyhow::{Context as _, Result, ensure};
use super::{Reply, Request};
use crate::backups::OPERATION_TIMEOUT;

/// Requests contain one mount and at most two paths, never a script or arbitrary argv.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// Snapshot history and diagnostic output, including JSON escaping.
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn run(request: &Request, cancel: &AtomicBool) -> Result<Reply> {
    let document = serde_json::to_string(request)?;
    ensure!(document.len() <= MAX_REQUEST_BYTES, "filesystem request is too large");
    let args = [OsString::from("storage-admin"), OsString::from(document)];
    let (program, args) = match mandara_config::admin::current_admin_exe() {
        Some(admin) => mandara_config::admin::pkexec(&admin, args),
        None => mandara_config::admin::setup_and_run(&std::env::current_exe()?, args),
    };
    let mut command = Command::new(program);
    command.args(args);
    let mut output = Vec::new();
    crate::app_themes::process::stream_native_controlled(command, cancel, OPERATION_TIMEOUT, &mut |bytes| {
        ensure!(bytes.len() <= MAX_REPLY_BYTES.saturating_sub(output.len()), "filesystem reply is too large");
        output.extend_from_slice(bytes);
        Ok(())
    })?;
    serde_json::from_slice(&output).context("reading filesystem operation result")
}

/// The hidden CLI enters here before shell startup or IPC initialization.
pub fn main(args: &[String]) -> i32 {
    let result = (|| -> Result<Reply> {
        ensure!(args.len() == 1 && args[0].len() <= MAX_REQUEST_BYTES, "usage: mandara storage-admin <request-json>");
        ensure!(cfg!(target_os = "linux"), "filesystem maintenance is available on Linux");
        ensure!(rustix::process::geteuid().is_root(), "filesystem administrator helper is not running as root");
        let request: Request = serde_json::from_str(&args[0])?;
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        std::thread::Builder::new().name("storage-control".into()).spawn(move || {
            let mut bytes = [0; 7];
            let _ = std::io::stdin().read_exact(&mut bytes);
            // A request or a closed parent pipe both end ownership of this job.
            token.store(true, Ordering::Release);
        })?;
        super::btrfs::apply(&super::btrfs::SystemRunner, &request, &super::mounted()?, &cancel)
    })();
    match result {
        Ok(reply) => match serde_json::to_string(&reply) { Ok(document) => { println!("{document}"); 0 }, Err(error) => { eprintln!("{error}"); 1 } },
        Err(error) => { eprintln!("{error:#}"); 1 }
    }
}
