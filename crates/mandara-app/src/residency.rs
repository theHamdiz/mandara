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
//! How much comes back depends on the page cache's folio size: a fault maps
//! the whole folio it lands in (64 KiB of small ones around it, the
//! kernel's fault-around window). Writing the binary (an install, an
//! update, `cp`) leaves it cached in 256 KiB and larger folios, so every
//! function the idle shell runs keeps a quarter megabyte of code around it
//! resident: about twice the resident code of a binary read from disk.
//! [`refresh_executable_page_cache`] notices that at the first drop and
//! evicts the binary's page cache, so it is read back in small folios.


use std::time::Duration;

/// How long after start the first cold-page drop waits (startup, the first
/// frames and lazy loads done).
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
    Some(Mapping { start: usize::from_str_radix(start, 16).ok()?, end: usize::from_str_radix(end, 16).ok()?, perms, inode, path })
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
    let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") else { return 0 };
    let mut dropped_kib = 0;
    let mut current: Option<(usize, usize, bool)> = None;
    let mut rss = 0usize;
    let mut flush = |current: &mut Option<(usize, usize, bool)>, rss: usize| {
        if let Some((start, end, true)) = current.take()
            && rss > 0
            // SAFETY: a whole read-only, unmodified file mapping of this
            // process: dropping its pages loses nothing, the next access
            // maps the same file page again.
            && unsafe { libc::madvise(start as *mut libc::c_void, end - start, libc::MADV_DONTNEED) } == 0
        {
            dropped_kib += rss;
        }
    };
    for line in smaps.lines() {
        if let Some(m) = parse_mapping(line).filter(|m| m.perms.len() == 4 && line.as_bytes().first().is_some_and(u8::is_ascii_hexdigit) && line.contains('-')) {
            flush(&mut current, rss);
            rss = 0;
            current = Some((m.start, m.end, is_droppable_file(&m)));
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let kib = || value.trim().trim_end_matches(" kB").trim().parse::<usize>().unwrap_or(0);
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

/// Pages one fault maps around its address in a small-folio page cache (the
/// kernel's default `fault_around_bytes`, 64 KiB).
const FAULT_AROUND_PAGES: usize = 16;
/// Pages probed around each sample (2 MiB, the largest folio a file holds).
const PROBE_WINDOW_PAGES: usize = 512;
/// Samples spread over the executable's code.
const PROBES: usize = 32;

/// Whether the page cache holds `file` in folios larger than the
/// fault-around window: maps it again privately, reads one already-cached
/// page at a few places (never one that would need disk I/O) and counts
/// what each of those faults mapped.
fn has_large_folios(file: &std::fs::File, code: std::ops::Range<usize>) -> bool {
    use std::os::fd::AsRawFd as _;
    const PAGE: usize = 4096;
    let Ok(len) = file.metadata().map(|m| m.len() as usize / PAGE * PAGE) else { return false };
    if len == 0 || code.end > len {
        return false;
    }
    // SAFETY: a fresh read-only private mapping of a file we opened; unmapped below.
    let base = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0) };
    if base == libc::MAP_FAILED {
        return false;
    }
    let Ok(pagemap) = std::fs::File::open("/proc/self/pagemap") else {
        // SAFETY: unmapping what we mapped.
        unsafe { libc::munmap(base, len) };
        return false;
    };
    let step = ((code.end - code.start) / PROBES).max(PROBE_WINDOW_PAGES * PAGE);
    let mut large = false;
    let mut offset = code.start / PAGE * PAGE;
    while offset < code.end && !large {
        // SAFETY: `offset` is a page inside the mapping.
        let page = unsafe { base.cast::<u8>().add(offset) };
        let mut cached = 0u8;
        // SAFETY: one page of our mapping, one result byte.
        let cached_ok = unsafe { libc::mincore(page.cast(), PAGE, &mut cached) } == 0 && cached & 1 == 1;
        if cached_ok {
            // SAFETY: a readable page of the mapping; the fault maps cached pages only.
            unsafe { std::ptr::read_volatile(page) };
            let window = offset / (PROBE_WINDOW_PAGES * PAGE) * (PROBE_WINDOW_PAGES * PAGE);
            let pages = PROBE_WINDOW_PAGES.min((len - window) / PAGE);
            let mut entries = vec![0u8; pages * 8];
            let at = (base as usize + window) / PAGE * 8;
            use std::os::unix::fs::FileExt as _;
            if pagemap.read_exact_at(&mut entries, at as u64).is_ok() {
                let present = entries.as_chunks::<8>().0.iter().filter(|e| u64::from_le_bytes(**e) >> 63 == 1).count();
                large = present > FAULT_AROUND_PAGES;
            }
        }
        offset += step;
    }
    // SAFETY: unmapping what we mapped.
    unsafe { libc::munmap(base, len) };
    large
}

/// The executable's code mapping (`r-xp`) as a file-offset range.
fn executable_code_range() -> Option<std::ops::Range<usize>> {
    let exe = std::fs::read_link("/proc/self/exe").ok()?;
    let exe = exe.to_str()?;
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines().find_map(|line| {
        let m = parse_mapping(line)?;
        let offset = line.split(' ').nth(2).and_then(|o| usize::from_str_radix(o, 16).ok())?;
        (m.perms == "r-xp" && m.path == exe).then(|| offset..offset + (m.end - m.start))
    })
}

/// Rebuilds the executable's page cache in small folios when it was cached
/// in large ones (written by an install or update): the cached pages no
/// process maps are evicted, so the code in use is read back from disk once,
/// as after a reboot. Run right after a drop (little of ours is mapped);
/// returns whether it evicted.
pub fn refresh_executable_page_cache() -> bool {
    let Some(code) = executable_code_range() else { return false };
    let Ok(file) = std::fs::File::open("/proc/self/exe") else { return false };
    if !has_large_folios(&file, code) {
        return false;
    }
    use std::os::fd::AsRawFd as _;
    // Dirty pages cannot be evicted: write back what an install left (by
    // the first drop the kernel has normally done it; then this returns at
    // once).
    let _ = file.sync_data();
    // SAFETY: advice on a file descriptor we own; mapped pages stay.
    let ok = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) } == 0;
    tracing::info!("residency: the executable was cached in large folios (written by an install); evicted it so code maps back in 64 KiB pieces");
    ok
}

/// Starts the periodic cold-page drop on the service runtime.
pub fn start() {
    mandara_services::runtime::spawn("cold-pages", async {
        mandara_services::runtime::sleep(SETTLE).await;
        let mut first = true;
        loop {
            let kib = drop_clean_file_pages();
            tracing::debug!("residency: unmapped {kib} KiB of clean file pages");
            if std::mem::take(&mut first) {
                // Now that nothing of ours maps it (and an install's
                // write-back has had time to finish), a large-folio page
                // cache can be rebuilt; the code in use reads back from disk.
                refresh_executable_page_cache();
            }
            mandara_services::runtime::sleep(DROP_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_mappings() {
        let heap = parse_mapping("55c0e5757000-55c0e71d9000 rw-p 00000000 00:00 0                          [heap]").unwrap();
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
        let font = parse_mapping("7f00-7f10 r--s 00000000 00:23 5     /usr/share/fonts/inter/Inter.ttc").unwrap();
        assert!(is_droppable_file(&font));
    }

    #[test]
    fn drops_only_clean_pages_and_keeps_working() {
        // A private copy of a file page must survive; code keeps running.
        let path = std::env::temp_dir().join(format!("mandara-residency-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; 4096 * 4]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        use std::os::fd::AsRawFd as _;
        // SAFETY: a fresh private read-write mapping of our own file.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), 4096 * 4, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE, file.as_raw_fd(), 0) } as *mut u8;
        assert!(!p.is_null());
        // SAFETY: inside the mapping.
        unsafe {
            *p = 42;
            libc::mprotect(p.cast(), 4096 * 4, libc::PROT_READ);
        }
        assert!(drop_clean_file_pages() > 0);
        // SAFETY: inside the mapping.
        assert_eq!(unsafe { *p }, 42, "a copied-on-write page was dropped");
        // SAFETY: unmapping what we mapped.
        unsafe { libc::munmap(p.cast(), 4096 * 4) };
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn probes_the_executable_without_leaking_a_mapping() {
        let code = executable_code_range().expect("the test binary has a code mapping");
        assert!(code.end > code.start);
        let exe = std::fs::read_link("/proc/self/exe").unwrap();
        let exe = exe.to_str().unwrap().to_owned();
        let maps = || std::fs::read_to_string("/proc/self/maps").unwrap().lines().filter(|l| l.ends_with(exe.as_str())).count();
        let before = maps();
        let file = std::fs::File::open("/proc/self/exe").unwrap();
        // Either answer is valid (it depends on how the binary was cached);
        // the probe must not leave its mapping behind.
        let _ = has_large_folios(&file, code.clone());
        assert_eq!(maps(), before, "probe mapping left behind");
        // A range past the end of the file is refused, not probed.
        assert!(!has_large_folios(&file, usize::MAX - 4096..usize::MAX));
    }
}
