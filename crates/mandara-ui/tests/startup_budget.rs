//! Resource guards for shell startup (docs/releasing.md, "Resource
//! budget"): registering every built-in plugin must stay a cheap table of
//! commands and views. A plugin that loads data, decodes images, scans
//! folders or starts threads while registering (instead of when it is first
//! shown) trips these budgets.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

/// Bytes still allocated after registering every built-in plugin.
const REGISTRY_LIVE_BUDGET: isize = 4 * 1024 * 1024;
/// Bytes allocated in total while registering (transient included).
const REGISTRY_TOTAL_BUDGET: usize = 16 * 1024 * 1024;

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        TOTAL.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        TOTAL.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map(|d| d.count()).unwrap_or(0)
}

#[test]
fn registering_every_plugin_stays_within_budget() {
    let threads_before = threads();
    let (live0, total0) = (LIVE.load(Ordering::Relaxed), TOTAL.load(Ordering::Relaxed));
    let mut registry = mandara_plugins::PluginRegistry::default();
    mandara_ui::plugins::register_builtin(&mut registry);
    let live = LIVE.load(Ordering::Relaxed) - live0;
    let total = TOTAL.load(Ordering::Relaxed) - total0;
    let spawned = threads().saturating_sub(threads_before);
    eprintln!("registry: {} KiB live, {} KiB allocated, {spawned} threads", live / 1024, total / 1024);
    assert!(live <= REGISTRY_LIVE_BUDGET, "registering plugins keeps {} KiB (budget {} KiB): initialize lazily", live / 1024, REGISTRY_LIVE_BUDGET / 1024);
    assert!(total <= REGISTRY_TOTAL_BUDGET, "registering plugins allocates {} KiB (budget {} KiB)", total / 1024, REGISTRY_TOTAL_BUDGET / 1024);
    assert_eq!(spawned, 0, "registering plugins started threads");
    drop(registry);
}
