//! The process-wide memory allocator, picked at build time by exactly one
//! `alloc-*` cargo feature of `mandara-app`:
//!
//! * `alloc-system-tuned` (default) — glibc malloc with at most [`ARENA_MAX`]
//!   arenas and a fixed [`MMAP_THRESHOLD`]. Released caches request a trim;
//!   a [`TRIM_EVERY`] check requests one for other freed storage, but only
//!   when the heap's free bytes have grown by [`TRIM_GROWTH`] since the last
//!   check, so an idle shell never walks its heap.
//! * `alloc-mimalloc` — mimalloc v3 (C, vendored by the `mimalloc` crate);
//!   frees return to the OS through its own page purging.
//! * `alloc-mimalloc-v2` — the same, on the older v2 line.
//! * `alloc-jemalloc` — jemalloc (C, `tikv-jemallocator`) with a background
//!   purge thread, few arenas and short dirty / muzzy decay.
//! * `alloc-system` — glibc malloc, untouched.
//!
//! Rust allocations only: C libraries (Mesa, fontconfig, FreeType, Wayland)
//! keep using glibc malloc in every variant.
//!
//! On musl the "tuned" variant is plain musl malloc (mallocng hands freed
//! memory back by itself and has no `mallopt` / `malloc_trim`).

#[cfg(any(
    all(feature = "alloc-mimalloc", any(feature = "alloc-mimalloc-v2", feature = "alloc-jemalloc", feature = "alloc-system", feature = "alloc-system-tuned")),
    all(feature = "alloc-mimalloc-v2", any(feature = "alloc-jemalloc", feature = "alloc-system", feature = "alloc-system-tuned")),
    all(feature = "alloc-jemalloc", any(feature = "alloc-system", feature = "alloc-system-tuned")),
    all(feature = "alloc-system", feature = "alloc-system-tuned"),
))]
compile_error!("enable exactly one `alloc-*` feature of mandara-app (use --no-default-features to switch)");

#[cfg(any(feature = "alloc-mimalloc", feature = "alloc-mimalloc-v2"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "alloc-jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// jemalloc options, read by jemalloc at its first allocation: a background
/// thread purges unused pages (instead of the allocating threads), two
/// arenas instead of four per CPU, dirty pages returned after one second
/// and muzzy (lazily freed) pages immediately.
#[cfg(feature = "alloc-jemalloc")]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: Option<&'static u8> = Some(&b"background_thread:true,narenas:2,dirty_decay_ms:1000,muzzy_decay_ms:0\0"[0]);

/// glibc arenas allowed by `alloc-system-tuned` (glibc's default is eight
/// per core, each holding on to the memory its threads freed).
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
pub const ARENA_MAX: libc::c_int = 2;
/// Allocations from this size up get their own mapping, returned to the
/// OS on free (`alloc-system-tuned`). Setting it also turns off glibc's
/// dynamic threshold, which rose to 32 MiB after the first large free and
/// doubled the trim threshold with it: from then on decoded wallpapers,
/// icon grids and cut-outs landed in a malloc arena where longer-lived
/// allocations could keep freed chunks from being returned, so
/// opening the dock's Applications grid left ~16 MiB behind for good.
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
pub const MMAP_THRESHOLD: libc::c_int = 512 * 1024;
/// How often `alloc-system-tuned` checks whether other freed heap is worth
/// handing back to the OS.
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
pub const TRIM_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// Growth of the heap's free bytes between two checks that earns a trim.
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
pub const TRIM_GROWTH: usize = 1024 * 1024;

/// Whether the free bytes of the heap rose enough since `before` to trim.
/// What a trim leaves behind is the next check's baseline, so a heap that
/// stays fragmented but quiet is not trimmed again and again.
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
fn trim_due(before: usize, now: usize) -> bool {
    now.saturating_sub(before) >= TRIM_GROWTH
}

/// Bytes malloc holds free in its arenas (mmapped chunks are returned on
/// free and not counted).
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
fn free_heap_bytes() -> usize {
    // SAFETY: mallinfo2 only reads allocator statistics and is thread-safe.
    let info = unsafe { libc::mallinfo2() };
    info.fordblks
}

/// Name of the allocator this binary was built with (for `--version`
/// style diagnostics and the benchmark logs).
pub const NAME: &str = if cfg!(feature = "alloc-mimalloc") {
    "mimalloc v3"
} else if cfg!(feature = "alloc-mimalloc-v2") {
    "mimalloc v2"
} else if cfg!(feature = "alloc-jemalloc") {
    "jemalloc"
} else if cfg!(target_env = "musl") {
    "musl"
} else if cfg!(target_os = "freebsd") {
    // The system allocator: FreeBSD's libc malloc is its own jemalloc. The
    // glibc tuning and trim hook don't apply.
    "FreeBSD libc"
} else if cfg!(feature = "alloc-system-tuned") {
    "glibc (tuned)"
} else {
    "glibc"
};

/// Applies allocator tuning that must happen before threads start.
pub fn init() {
    #[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
    // SAFETY: mallopt only changes glibc malloc parameters; called before
    // any other thread exists.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, ARENA_MAX);
        libc::mallopt(libc::M_MMAP_THRESHOLD, MMAP_THRESHOLD);
    }
    #[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
    mandara_services::memory::install_trim_hook(trim);
}

#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
fn trim() {
    // SAFETY: malloc_trim is thread-safe and has no caller preconditions.
    unsafe { libc::malloc_trim(0); }
}

/// Starts background upkeep (the tuned glibc variant's periodic trim).
pub fn start_upkeep() {
    #[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
    mandara_services::runtime::spawn("malloc-trim", async {
        let mut seen = free_heap_bytes();
        loop {
            mandara_services::runtime::sleep(TRIM_EVERY).await;
            let now = free_heap_bytes();
            if trim_due(seen, now) {
                mandara_services::memory::request_trim();
            }
            seen = now;
        }
    });
}

#[cfg(all(test, feature = "alloc-system-tuned", target_env = "gnu"))]
mod tests {
    use super::*;

    #[test]
    fn only_growth_of_the_free_heap_earns_a_trim() {
        assert!(!trim_due(10 << 20, 10 << 20));
        assert!(!trim_due(10 << 20, (10 << 20) + TRIM_GROWTH - 1));
        assert!(trim_due(10 << 20, (10 << 20) + TRIM_GROWTH));
        // A shrinking heap (a trim ran, memory was reused) never does.
        assert!(!trim_due(10 << 20, 1 << 20));
    }
}
