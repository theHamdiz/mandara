//! The executable each running process was started from, so an update can
//! tell whether the binary it replaces is in use (the shell, its
//! supervisor). Linux reads `/proc/<pid>/exe`; FreeBSD asks the kernel
//! (`kern.proc.proc` for the process table, `kern.proc.pathname.<pid>` for
//! each image), since its procfs is optional and usually not mounted.

use std::path::{Path, PathBuf};

/// Whether a process other than this one runs `exe`.
pub fn other_runs(exe: &Path) -> bool {
    let me = std::process::id();
    images().any(|(pid, image)| pid != me && image == exe)
}

/// `(pid, executable)` of every process whose image can be read.
#[cfg(target_os = "linux")]
fn images() -> impl Iterator<Item = (u32, PathBuf)> {
    std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|entry| {
        let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
        let link = std::fs::read_link(entry.path().join("exe")).ok()?;
        // A replaced or deleted image keeps its old path plus this suffix.
        let path = match link.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
            Some(stripped) => PathBuf::from(stripped),
            None => link,
        };
        Some((pid, path))
    })
}

#[cfg(target_os = "freebsd")]
fn images() -> impl Iterator<Item = (u32, PathBuf)> {
    freebsd::pids().into_iter().filter_map(|pid| Some((pid, freebsd::image(pid)?)))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn images() -> impl Iterator<Item = (u32, PathBuf)> {
    std::iter::empty()
}

/// Process IDs in a `kern.proc.proc` table: records of `ki_structsize`
/// bytes (the first `int` of each), the ID at `pid_offset`. A short or
/// inconsistent record ends the walk instead of reading past the table.
#[cfg(any(test, target_os = "freebsd"))]
fn pids_in(table: &[u8], pid_offset: usize) -> Vec<u32> {
    const INT: usize = size_of::<i32>();
    let int_at = |at: usize| table.get(at..at + INT).and_then(|b| b.try_into().ok()).map(i32::from_ne_bytes);
    let mut pids = Vec::with_capacity(table.len() / (pid_offset + INT).max(1));
    let mut at = 0;
    while let Some(size) = int_at(at) {
        let size = usize::try_from(size).unwrap_or_default();
        if size < pid_offset + INT || table.len() - at < size {
            break;
        }
        if let Some(pid) = int_at(at + pid_offset).and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0) {
            pids.push(pid);
        }
        at += size;
    }
    pids
}

#[cfg(target_os = "freebsd")]
mod freebsd {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::PathBuf;

    /// Attempts at reading a process table that grew between the size
    /// query and the read.
    const ATTEMPTS: usize = 4;
    /// Room for processes started between the size query and the read.
    const SPARE_RECORDS: usize = 32;

    fn sysctl(mib: &[libc::c_int], buf: *mut libc::c_void, len: &mut usize) -> bool {
        let Ok(depth) = libc::c_uint::try_from(mib.len()) else { return false };
        // SAFETY: `mib` is a valid name of `depth` integers, and `buf` is
        // either null (a size query) or writable for `*len` bytes; the
        // kernel writes at most `*len` bytes and stores the length used.
        unsafe { libc::sysctl(mib.as_ptr(), depth, buf, len, std::ptr::null(), 0) == 0 }
    }

    /// Every process ID (`kern.proc.proc`: processes, not threads).
    pub(super) fn pids() -> Vec<u32> {
        let mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PROC];
        for _ in 0..ATTEMPTS {
            let mut len = 0;
            if !sysctl(&mib, std::ptr::null_mut(), &mut len) {
                return Vec::new();
            }
            len += SPARE_RECORDS * size_of::<libc::kinfo_proc>();
            let mut table = vec![0u8; len];
            if sysctl(&mib, table.as_mut_ptr().cast(), &mut len) {
                table.truncate(len);
                return super::pids_in(&table, std::mem::offset_of!(libc::kinfo_proc, ki_pid));
            }
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOMEM) {
                return Vec::new();
            }
        }
        Vec::new()
    }

    /// The executable `pid` runs (`kern.proc.pathname.<pid>`).
    pub(super) fn image(pid: u32) -> Option<PathBuf> {
        let mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PATHNAME, libc::c_int::try_from(pid).ok()?];
        let mut path = [0u8; libc::PATH_MAX as usize];
        let mut len = path.len();
        if !sysctl(&mib, path.as_mut_ptr().cast(), &mut len) {
            return None;
        }
        let path = &path[..len.min(path.len())];
        let path = path.split(|b| *b == 0).next().filter(|p| !p.is_empty())?;
        Some(PathBuf::from(OsStr::from_bytes(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(size: i32, pid: i32, pid_offset: usize) -> Vec<u8> {
        let mut r = vec![0u8; usize::try_from(size).unwrap_or_default().max(8)];
        r[..4].copy_from_slice(&size.to_ne_bytes());
        if r.len() >= pid_offset + 4 {
            r[pid_offset..pid_offset + 4].copy_from_slice(&pid.to_ne_bytes());
        }
        r
    }

    #[test]
    fn process_tables_are_walked_by_record_size() {
        const OFFSET: usize = 72;
        let table = [record(1088, 1, OFFSET), record(1088, 0, OFFSET), record(1088, 4242, OFFSET)].concat();
        assert_eq!(pids_in(&table, OFFSET), [1, 4242], "pid 0 (the kernel) is skipped");
        // A truncated last record, or a size too small to hold the ID,
        // stops the walk without reading past the table.
        assert_eq!(pids_in(&table[..1088 * 2 + 100], OFFSET), [1]);
        assert_eq!(pids_in(&[record(1088, 7, OFFSET), record(16, 9, OFFSET)].concat(), OFFSET), [7]);
        assert_eq!(pids_in(&[record(-1, 7, OFFSET)].concat(), OFFSET), Vec::<u32>::new());
        assert!(pids_in(&[], OFFSET).is_empty());
    }

    /// A real child process is found by the executable it was started from,
    /// and no longer once it has exited; this process never counts.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn a_running_copy_of_a_program_is_found_by_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let sleep = dir.path().join("sleep-copy");
        let original = ["/bin/sleep", "/usr/bin/sleep"].into_iter().find(|p| Path::new(p).is_file()).unwrap();
        std::fs::copy(original, &sleep).unwrap();
        assert!(!other_runs(&sleep));
        let mut child = std::process::Command::new(&sleep).arg("30").spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut found = false;
        while !found && std::time::Instant::now() < deadline {
            found = other_runs(&sleep);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(found, "{} not found among running processes", sleep.display());
        assert!(!other_runs(&sleep));
        // This process is in the table, under its own image, but never
        // counts as "another process".
        let me = std::env::current_exe().unwrap();
        assert!(images().any(|(pid, image)| pid == std::process::id() && image == me));
        assert!(!other_runs(&me) || images().any(|(pid, image)| pid != std::process::id() && image == me));
    }
}
