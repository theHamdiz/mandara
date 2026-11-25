//! The process table from `kern.proc.proc` (one `struct kinfo_proc` per
//! process, no procfs), command lines from `kern.proc.args.<pid>`.

/// `kinfo_proc.ki_stat` values (`sys/proc.h`).
const SIDL: i32 = 1;
const SRUN: i32 = 2;
const SSLEEP: i32 = 3;
const SSTOP: i32 = 4;
const SZOMB: i32 = 5;
const SWAIT: i32 = 6;
const SLOCK: i32 = 7;
/// `ki_runtime` is in microseconds.
pub const RUNTIME_PER_SECOND: f32 = 1_000_000.0;

/// One process as the kernel reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawProc {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    /// `ki_comm`, e.g. `firefox`.
    pub name: String,
    /// Resident set, bytes.
    pub rss: u64,
    /// CPU time used, microseconds (`ki_runtime`).
    pub runtime_us: u64,
    pub state: char,
    pub threads: u32,
    /// A kernel process (`P_SYSTEM`): `idle`, `geom`, `usb`…
    pub system: bool,
}

/// The letter `ps(1)` shows for a `ki_stat` value.
pub fn state_letter(stat: i32) -> char {
    match stat {
        SIDL => 'I',
        SRUN => 'R',
        SSLEEP => 'S',
        SSTOP => 'T',
        SZOMB => 'Z',
        SWAIT => 'W',
        SLOCK => 'L',
        _ => '?',
    }
}

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use super::*;
    use crate::freebsd::sysctl;
    use std::mem::size_of;

    fn page_size() -> u64 {
        rustix::param::page_size() as u64
    }

    fn comm(raw: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = raw.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn decode(k: &libc::kinfo_proc, page: u64) -> RawProc {
        RawProc {
            pid: u32::try_from(k.ki_pid).unwrap_or(0),
            ppid: u32::try_from(k.ki_ppid).unwrap_or(0),
            uid: k.ki_uid,
            name: comm(&k.ki_comm),
            rss: u64::try_from(k.ki_rssize).unwrap_or(0).saturating_mul(page),
            runtime_us: k.ki_runtime,
            state: state_letter(i32::from(k.ki_stat)),
            threads: u32::try_from(k.ki_numthreads).unwrap_or(0),
            system: k.ki_flag & libc::c_long::from(libc::P_SYSTEM) != 0,
        }
    }

    /// Each `kinfo_proc` record in a `kern.proc.*` buffer. Records are
    /// walked by their own `ki_structsize`, and only when it matches the
    /// layout this build was compiled against.
    fn records(buf: &[u8], page: u64, mut each: impl FnMut(RawProc)) {
        let size = size_of::<libc::kinfo_proc>();
        for chunk in buf.chunks_exact(size) {
            // SAFETY: `chunk` is exactly one `kinfo_proc` of plain data;
            // `read_unaligned` copes with the Vec's byte alignment.
            let k: libc::kinfo_proc = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
            if usize::try_from(k.ki_structsize).ok() != Some(size) {
                return;
            }
            each(decode(&k, page));
        }
    }

    /// Every process (one record per process, not per thread).
    pub fn all() -> Vec<RawProc> {
        let Some(buf) = sysctl::bytes(&[libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PROC, 0]) else { return Vec::new() };
        let mut out = Vec::with_capacity(buf.len() / size_of::<libc::kinfo_proc>());
        records(&buf, page_size(), |p| out.push(p));
        out
    }

    /// One process, or `None` once it has exited.
    pub fn one(pid: u32) -> Option<RawProc> {
        let buf = sysctl::bytes(&[libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, i32::try_from(pid).ok()?])?;
        let mut found = None;
        records(&buf, page_size(), |p| found = Some(p));
        found
    }

    /// The process's arguments, NUL-separated (`kern.proc.args.<pid>`);
    /// empty for kernel processes and processes of other users the
    /// kernel hides (`security.bsd.see_other_uids=0`).
    pub fn args(pid: u32) -> Vec<u8> {
        let Ok(pid) = i32::try_from(pid) else { return Vec::new() };
        sysctl::bytes(&[libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ARGS, pid]).unwrap_or_default()
    }

    /// Whether `pid` is still running (and not a zombie).
    pub fn is_alive(pid: u32) -> bool {
        one(pid).is_some_and(|p| p.state != 'Z')
    }

    /// Calls `each` with the path of every file (vnode) `pid` has open
    /// (`kern.proc.filedesc.<pid>`: packed `kinfo_file` records, each
    /// `kf_structsize` long). Other users' processes are skipped unless
    /// this runs as root.
    pub fn open_vnodes(pid: u32, mut each: impl FnMut(&std::path::Path)) {
        use std::os::unix::ffi::OsStrExt as _;
        let Ok(pid) = i32::try_from(pid) else { return };
        let Some(buf) = sysctl::bytes(&[libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_FILEDESC, pid]) else { return };
        let full = size_of::<libc::kinfo_file>();
        let mut at = 0;
        while at + size_of::<libc::c_int>() <= buf.len() {
            let size = i32::from_ne_bytes(buf[at..at + 4].try_into().unwrap_or_default());
            let Ok(size) = usize::try_from(size) else { return };
            if size == 0 || at + size > buf.len() {
                return;
            }
            // SAFETY: an all-zero kinfo_file is valid plain data; the record's
            // own bytes (at most the full struct) are copied over it.
            let mut k: libc::kinfo_file = unsafe { std::mem::zeroed() };
            let n = size.min(full);
            // SAFETY: `n` bytes are in bounds of both `buf` and `k`.
            unsafe { std::ptr::copy_nonoverlapping(buf[at..].as_ptr(), std::ptr::from_mut(&mut k).cast::<u8>(), n) };
            if k.kf_type == libc::KF_TYPE_VNODE {
                let path: Vec<u8> = k.kf_path.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
                if !path.is_empty() {
                    each(std::path::Path::new(std::ffi::OsStr::from_bytes(&path)));
                }
            }
            at += size;
        }
    }

    /// The executable of `pid` (`kern.proc.pathname.<pid>`).
    pub fn executable(pid: u32) -> Option<std::path::PathBuf> {
        let pid = i32::try_from(pid).ok()?;
        let buf = sysctl::bytes(&[libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PATHNAME, pid])?;
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        use std::os::unix::ffi::OsStrExt as _;
        (end > 0).then(|| std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..end])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "freebsd")]
    fn lists_live_processes() {
        let me = one(std::process::id()).expect("own kinfo_proc");
        assert!(me.rss > 0 && me.threads >= 1 && !me.system && me.state != 'Z');
        assert!(all().iter().any(|p| p.pid == me.pid));
        assert!(all().iter().any(|p| p.system), "kernel processes are marked");
        assert!(!args(me.pid).is_empty());
        assert!(executable(me.pid).is_some_and(|e| e.is_absolute()));
        let mut opened = 0;
        let _f = std::fs::File::open("/etc/passwd").unwrap();
        open_vnodes(me.pid, |p| opened += usize::from(p == std::path::Path::new("/etc/passwd")));
        assert_eq!(opened, 1, "kern.proc.filedesc lists an open file");
    }

    #[test]
    fn states_read_like_ps() {
        assert_eq!([SIDL, SRUN, SSLEEP, SSTOP, SZOMB, SWAIT, SLOCK, 0].map(state_letter), ['I', 'R', 'S', 'T', 'Z', 'W', 'L', '?']);
    }
}
