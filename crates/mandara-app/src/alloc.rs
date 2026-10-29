//! The process-wide memory allocator, picked at build time by exactly one
//! `alloc-*` cargo feature of `mandara-app`:
//!
//! * `alloc-system-tuned` (default) — see below; the leanest in the
//!   measurements in docs/releasing.md, and no C allocator.
//! * `alloc-mimalloc` — mimalloc v3 (C, vendored by the `mimalloc` crate);
//!   frees return to the OS through its own page purging.
//! * `alloc-mimalloc-v2` — the same, on the older v2 line.
//! * `alloc-jemalloc` — jemalloc (C, `tikv-jemallocator`) with a background
//!   purge thread, few arenas and short dirty / muzzy decay.
//! * `alloc-system` — glibc malloc, untouched.
//! * `alloc-system-tuned` — glibc malloc with at most [`ARENA_MAX`] arenas,
//!   a fixed [`MMAP_THRESHOLD`] and a periodic `malloc_trim(0)`
//!   ([`TRIM_EVERY`]) on the service runtime, so freed heap returns to the
//!   OS.
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
/// icon grids and cut-outs landed in a malloc arena whose free top
/// `malloc_trim` never returns (it only trims the main arena's top), so
/// opening the dock's Applications grid left ~16 MiB behind for good.
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
pub const MMAP_THRESHOLD: libc::c_int = 512 * 1024;
/// How often `alloc-system-tuned` hands freed heap back to the OS.
#[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
pub const TRIM_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

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
}

/// Starts background upkeep (the tuned glibc variant's periodic trim).
pub fn start_upkeep() {
    #[cfg(all(feature = "alloc-system-tuned", target_env = "gnu"))]
    mandara_services::runtime::spawn("malloc-trim", async {
        loop {
            mandara_services::runtime::sleep(TRIM_EVERY).await;
            // SAFETY: malloc_trim has no preconditions.
            unsafe {
                libc::malloc_trim(0);
            }
        }
    });
    crate::residency::start();
}
