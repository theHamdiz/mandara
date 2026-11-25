use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Count {
    pub calls: usize,
    pub bytes: usize,
}

thread_local! {
    static ACTIVE: Cell<Option<Count>> = const { Cell::new(None) };
}

struct Counting;

fn record(bytes: usize) {
    let _ = ACTIVE.try_with(|active| {
        if let Some(mut count) = active.get() {
            count.calls += 1;
            count.bytes += bytes;
            active.set(Some(count));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            record(size);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Counts this thread only, so other unit tests can allocate concurrently.
pub(super) fn measure<T>(run: impl FnOnce() -> T) -> (T, Count) {
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            ACTIVE.set(None);
        }
    }

    assert!(ACTIVE.replace(Some(Count::default())).is_none(), "allocation counters cannot nest");
    let scope = Scope;
    let result = run();
    let count = ACTIVE.take().unwrap();
    drop(scope);
    (result, count)
}
