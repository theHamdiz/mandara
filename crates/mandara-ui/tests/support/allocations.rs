//! Allocation counts scoped to the calling test thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static TOTAL: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// Keep unrelated test/runtime threads and thread-local teardown out of counts.
fn record(bytes: usize) {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = TOTAL.try_with(|total| {
            let (calls, bytes_before) = total.get();
            total.set((calls + 1, bytes_before + bytes));
        });
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        record(bytes);
        unsafe { System.realloc(pointer, layout, bytes) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

struct Scope;

impl Drop for Scope {
    /// Assertion failures must not leave later work on this thread measured.
    fn drop(&mut self) {
        TRACKING.with(|tracking| tracking.set(false));
    }
}

/// Count allocation/reallocation calls and requested bytes after preparing
/// fixtures and warming caches. Include a live positive control in each test.
pub fn allocations(work: impl FnOnce()) -> (usize, usize) {
    assert!(!TRACKING.with(Cell::get), "allocation scopes cannot nest");
    TOTAL.with(|total| total.set((0, 0)));
    TRACKING.with(|tracking| tracking.set(true));
    let scope = Scope;
    work();
    drop(scope);
    TOTAL.with(Cell::get)
}
