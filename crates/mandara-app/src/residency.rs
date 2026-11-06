//! Startup-only file pages unmapped once the shell has settled (see
//! docs/releasing.md, "Resource budget").
//!
//! Clean, read-only file mappings stay resident after first use: the
//! binary's start-up code and read-only data, shared libraries that run
//! large static initializers at load (RADV links LLVM), font files scanned
//! once. Once settled, [`drop_clean_file_pages`] unmaps them with
//! `MADV_DONTNEED`. The pages stay in the page cache and the code still in
//! use maps itself back with a minor fault, so resident memory shows the
//! live working set. Mappings holding private copies (RELRO, relocated
//! data) are never touched: dropping those would lose writes.
//!
//! The kernel keeps ownership of the file cache. This cleanup never writes
//! back or evicts the running executable: forcing it to be read from disk
//! again can interrupt a frame long after startup.

use std::time::Duration;

/// How long after start an early cold-page drop runs. The shell is up and
/// has drawn its first frames within about a second; without it, the
/// start-up code, the driver's initializers and the scanned fonts stayed
/// resident until [`SETTLE`]. What the idle shell still runs maps back
/// with a minor fault.
pub const EARLY: Duration = Duration::from_secs(8);
/// How long after start the first full cold-page drop waits (startup, the
/// first frames and lazy loads done).
pub const SETTLE: Duration = Duration::from_secs(45);
/// How often the cold-page drop repeats afterwards (code paged in by a
/// one-off action is shed again).
pub const DROP_EVERY: Duration = Duration::from_secs(10 * 60);

/// One `/proc/self/maps` line: address range, permissions and path.
struct Mapping<'a> {
    start: usize,
    end: usize,
    perms: &'a str,
    inode: &'a str,
    path: &'a str,
}

fn parse_mapping(line: &str) -> Option<Mapping<'_>> {
    let mut fields = line.splitn(6, ' ');
    let range = fields.next()?;
    let perms = fields.next()?;
    let _offset = fields.next()?;
    let _dev = fields.next()?;
    let inode = fields.next()?;
    let path = fields.next().map(str::trim).unwrap_or("");
    let (start, end) = range.split_once('-')?;
    let (start, end) = (
        usize::from_str_radix(start, 16).ok()?,
        usize::from_str_radix(end, 16).ok()?,
    );
    (start < end).then_some(Mapping {
        start,
        end,
        perms,
        inode,
        path,
    })
}

/// A read-only mapping of a regular file whose pages can be dropped and
/// faulted back from the page cache unchanged.
fn is_droppable_file(m: &Mapping<'_>) -> bool {
    matches!(m.perms, "r-xp" | "r--p" | "r--s")
        && m.inode != "0"
        && m.path.starts_with('/')
        && !m.path.starts_with("/dev/")
        && !m.path.starts_with("/memfd:")
        && !m.path.ends_with("(deleted)")
}

/// Unmaps the resident pages of clean read-only file mappings; returns the
/// KiB that were resident. A mapping with any anonymous (copied-on-write)
/// page is skipped whole: those pages differ from the file.
pub fn drop_clean_file_pages() -> usize {
    let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") else {
        return 0;
    };
    drop_clean_mappings(&smaps, |start, len| {
        // SAFETY: a whole read-only, unmodified file mapping of this process.
        // The parser excludes private copies and dirty pages before calling us.
        unsafe { libc::madvise(start as *mut libc::c_void, len, libc::MADV_DONTNEED) == 0 }
    })
}

fn drop_clean_mappings(smaps: &str, mut discard: impl FnMut(usize, usize) -> bool) -> usize {
    let mut dropped_kib = 0;
    let mut current: Option<(usize, usize, bool)> = None;
    let mut rss = 0usize;
    let mut flush = |current: &mut Option<(usize, usize, bool)>, rss: usize| {
        if let Some((start, end, true)) = current.take()
            && rss > 0
            && discard(start, end - start)
        {
            dropped_kib += rss;
        }
    };
    for line in smaps.lines() {
        if let Some(m) = parse_mapping(line).filter(|m| {
            m.perms.len() == 4
                && line.as_bytes().first().is_some_and(u8::is_ascii_hexdigit)
                && line.contains('-')
        }) {
            flush(&mut current, rss);
            rss = 0;
            current = Some((m.start, m.end, is_droppable_file(&m)));
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let kib = || {
            value
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse::<usize>()
                .unwrap_or(0)
        };
        match key {
            "Rss" => rss = kib(),
            "Anonymous" | "Private_Dirty" | "Shared_Dirty" if kib() > 0 => {
                if let Some(c) = current.as_mut() {
                    c.2 = false;
                }
            }
            _ => {}
        }
    }
    flush(&mut current, rss);
    dropped_kib
}

/// Starts the periodic cold-page drop on the service runtime.
pub fn start() {
    mandara_services::runtime::spawn("cold-pages", async {
        mandara_services::runtime::sleep(EARLY).await;
        let kib = drop_clean_file_pages();
        tracing::debug!("residency: unmapped {kib} KiB of clean file pages after start-up");
        mandara_services::runtime::sleep(SETTLE.saturating_sub(EARLY)).await;
        loop {
            let kib = drop_clean_file_pages();
            tracing::debug!("residency: unmapped {kib} KiB of clean file pages");
            mandara_services::runtime::sleep(DROP_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_mappings() {
        let heap = parse_mapping(
            "55c0e5757000-55c0e71d9000 rw-p 00000000 00:00 0                          [heap]",
        )
        .unwrap();
        assert!(!is_droppable_file(&heap));
        let text = parse_mapping("55c0e71d9000-55c0ecf1e000 r-xp 01a81000 00:39 134162                     /usr/bin/mandara").unwrap();
        assert!(is_droppable_file(&text));
        assert_eq!(text.end - text.start, 0x55c0ecf1e000 - 0x55c0e71d9000);
        for line in [
            "7f716eb9e000-7f716ed9f000 rw-s 00000000 00:39 148321     /home/u/.cache/mesa_shader_cache/index",
            "7f71000000-7f71001000 rw-p 00010000 00:39 1     /usr/lib/libc.so.6",
            "7f71000000-7f71001000 rw-s 101a00000 00:06 830     /dev/dri/renderD128",
            "7f71000000-7f71001000 r--s 00000000 00:01 9     /memfd:wayland-shm (deleted)",
            "7ffd1000-7ffd3000 r-xp 00000000 00:00 0                          [vdso]",
        ] {
            let m = parse_mapping(line).unwrap();
            assert!(!is_droppable_file(&m), "{line}");
        }
        let font =
            parse_mapping("7f00-7f10 r--s 00000000 00:23 5     /usr/share/fonts/inter/Inter.ttc")
                .unwrap();
        assert!(is_droppable_file(&font));
    }

    #[test]
    fn rejects_invalid_ranges_and_non_readonly_file_mappings() {
        for line in [
            "",
            "Rss: 4 kB",
            "1000-1000 r--p 0 00:00 1 /tmp/art",
            "2000-1000 r--p 0 00:00 1 /tmp/art",
            "no-hex r--p 0 00:00 1 /tmp/art",
        ] {
            assert!(parse_mapping(line).is_none(), "{line}");
        }
        for perms in ["rw-p", "rw-s", "r-xs", "---p", "--xp"] {
            let line = format!("1000-2000 {perms} 0 00:00 1 /tmp/art");
            assert!(
                !is_droppable_file(&parse_mapping(&line).unwrap()),
                "{perms}"
            );
        }
        for path in [
            "",
            "[heap]",
            "/dev/zero",
            "/memfd:art",
            "/tmp/art (deleted)",
        ] {
            let line = format!("1000-2000 r--p 0 00:00 1 {path}");
            assert!(!is_droppable_file(&parse_mapping(&line).unwrap()), "{path}");
        }
        let path = "/tmp/a file with spaces.png";
        let line = format!("1000-2000 r--p 0 00:00 1                  {path}");
        assert_eq!(parse_mapping(&line).unwrap().path, path);
    }

    #[test]
    fn skips_dirty_private_and_nonresident_pages_and_counts_only_successful_drops() {
        let smaps = concat!(
            "1000-2000 r--p 00000000 00:39 1 /tmp/clean\nRss: 4 kB\nAnonymous: 0 kB\nPrivate_Dirty: 0 kB\nShared_Dirty: 0 kB\n",
            "2000-3000 r--p 00000000 00:39 2 /tmp/cow\nRss: 4 kB\nAnonymous: 4 kB\n",
            "3000-4000 r--p 00000000 00:39 3 /tmp/private-dirty\nRss: 4 kB\nPrivate_Dirty: 4 kB\n",
            "4000-5000 r--s 00000000 00:39 4 /tmp/shared-dirty\nRss: 4 kB\nShared_Dirty: 4 kB\n",
            "5000-6000 rw-p 00000000 00:39 5 /tmp/writable\nRss: 4 kB\n",
            "6000-7000 r--p 00000000 00:39 6 /tmp/absent\nRss: 0 kB\n",
            "7000-8000 r--p 00000000 00:39 7 /tmp/refused\nRss: 4 kB\n",
        );
        let mut calls = Vec::new();
        let dropped = drop_clean_mappings(smaps, |start, len| {
            calls.push((start, len));
            start != 0x7000
        });
        assert_eq!(calls, [(0x1000, 0x1000), (0x7000, 0x1000)]);
        assert_eq!(dropped, 4);
    }

    /// An isolated file mapping; no other mapping in the test process is advised.
    struct TestMapping {
        pointer: *mut u8,
        length: usize,
        _file: tempfile::NamedTempFile,
    }

    /// tmpfs files remain dirty even after sync_all, so they cannot prove clean-page dropping.
    fn disk_backed_fixture() -> std::io::Result<tempfile::NamedTempFile> {
        use std::os::fd::AsRawFd as _;
        /// Linux UAPI's RAMFS_MAGIC is not exported by libc.
        const RAMFS_MAGIC: u32 = 0x8584_58f6;
        let candidates = [
            std::env::var_os("CARGO_TARGET_TMPDIR").map(std::path::PathBuf::from),
            std::env::var_os("TMPDIR").map(std::path::PathBuf::from),
            Some(std::path::PathBuf::from("/var/tmp")),
        ];
        for directory in candidates.into_iter().flatten() {
            let Ok(file) = tempfile::Builder::new()
                .prefix("mandara-residency-")
                .tempfile_in(directory)
            else {
                continue;
            };
            let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
            // SAFETY: this owns the live file descriptor and the output storage.
            if unsafe { libc::fstatfs(file.as_file().as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
                continue;
            }
            // SAFETY: a successful fstatfs initialized the complete output structure.
            let filesystem = unsafe { filesystem.assume_init() };
            if ![
                libc::TMPFS_MAGIC as u32,
                RAMFS_MAGIC,
                libc::HUGETLBFS_MAGIC as u32,
            ]
            .contains(&(filesystem.f_type as u32))
            {
                return Ok(file);
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no writable disk-backed scratch directory; set CARGO_TARGET_TMPDIR to a private non-tmpfs directory",
        ))
    }

    impl TestMapping {
        fn new() -> Self {
            use std::os::fd::AsRawFd as _;
            // SAFETY: sysconf has no pointer arguments or side effects.
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            assert!(page > 0);
            let length = usize::try_from(page).unwrap() * 4;
            let mut file = disk_backed_fixture()
                .expect("a clean-file residency test requires disk-backed scratch storage");
            use std::io::Write as _;
            file.write_all(&vec![7; length]).unwrap();
            file.as_file().sync_all().unwrap();
            // SAFETY: a private read-write mapping of the fixture file, retained below.
            let pointer = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE,
                    file.as_file().as_raw_fd(),
                    0,
                )
            };
            assert_ne!(
                pointer,
                libc::MAP_FAILED,
                "{}",
                std::io::Error::last_os_error()
            );
            Self {
                pointer: pointer.cast(),
                length,
                _file: file,
            }
        }

        fn make_readonly(&self) {
            // SAFETY: the complete live mapping owned by this fixture.
            let result =
                unsafe { libc::mprotect(self.pointer.cast(), self.length, libc::PROT_READ) };
            assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        }

        fn drop_clean_pages(&self) -> usize {
            let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
            drop_clean_mappings(&smaps, |start, length| {
                if start != self.pointer as usize {
                    return false;
                }
                assert_eq!(length, self.length);
                // SAFETY: only this fixture's clean, read-only mapping reaches this branch.
                unsafe { libc::madvise(self.pointer.cast(), self.length, libc::MADV_DONTNEED) == 0 }
            })
        }
    }

    impl Drop for TestMapping {
        fn drop(&mut self) {
            // SAFETY: unmapping the exact live region owned by this fixture.
            let result = unsafe { libc::munmap(self.pointer.cast(), self.length) };
            assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        }
    }

    #[test]
    fn a_clean_file_mapping_refaults_with_the_same_bytes() {
        let mapping = TestMapping::new();
        // SAFETY: first byte of our live readable mapping.
        assert_eq!(unsafe { mapping.pointer.read_volatile() }, 7);
        mapping.make_readonly();
        assert!(mapping.drop_clean_pages() > 0);
        // SAFETY: the mapping remains readable after dropping its resident pages.
        assert_eq!(unsafe { mapping.pointer.read_volatile() }, 7);
    }

    #[test]
    fn copied_on_write_data_survives_after_becoming_readonly() {
        let mapping = TestMapping::new();
        // SAFETY: first byte of our live writable private mapping.
        unsafe {
            mapping.pointer.write_volatile(42);
        }
        mapping.make_readonly();
        assert_eq!(
            mapping.drop_clean_pages(),
            0,
            "a private copy must not be advised away"
        );
        // SAFETY: the mapping remains readable.
        assert_eq!(unsafe { mapping.pointer.read_volatile() }, 42);
    }
}
