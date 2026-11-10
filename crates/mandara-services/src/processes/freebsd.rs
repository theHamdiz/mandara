//! The process table on FreeBSD: `kern.proc.proc` records (see
//! [`crate::freebsd::procs`]) and each process's arguments.

use std::collections::HashMap;

use super::{Process, command_line};
use crate::freebsd::procs;

/// Appends every user-space process to `out`; kernel processes
/// (`P_SYSTEM`) are skipped before their arguments are read.
pub(super) fn read_all(users: &HashMap<u32, String>, out: &mut Vec<Process>) {
    for raw in procs::all() {
        if raw.system || raw.pid == 0 {
            continue;
        }
        let cmdline = command_line(&procs::args(raw.pid));
        let user = users.get(&raw.uid).cloned().unwrap_or_else(|| raw.uid.to_string());
        out.push(Process { pid: raw.pid, ppid: raw.ppid, name: raw.name, cmdline, user, uid: raw.uid, rss: raw.rss, cpu: 0.0, state: raw.state, ticks: raw.runtime_us });
    }
}
