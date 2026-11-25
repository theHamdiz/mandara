//! The radio's per-block audio path must not allocate: rendering a block
//! into the session's byte buffer, resampling into its frame buffer, and
//! moving bytes through the jitter buffer and the ICY demuxer all reuse
//! storage made when the session starts.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use mandara_services::quran::player::Resample;
use mandara_services::radio::icy::Demux;
use mandara_services::radio::output::{BYTES_PER_FRAME, MAX_WRITE_FRAMES, Renderer};
use mandara_services::radio::ring::{Adts, Ring};

struct Counting;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn record() {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(pointer, layout, bytes) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocations made by `work` on this thread.
fn allocations(work: impl FnOnce()) -> usize {
    COUNT.with(|c| c.set(0));
    TRACKING.with(|t| t.set(true));
    work();
    TRACKING.with(|t| t.set(false));
    COUNT.with(Cell::get)
}

#[test]
fn rendering_blocks_does_not_allocate() {
    let mut renderer = Renderer::new(48_000);
    let frames: Vec<[f32; 2]> = (0..MAX_WRITE_FRAMES).map(|i| [(i as f32 * 0.01).sin(), (i as f32 * 0.013).cos()]).collect();
    let mut bytes = Vec::with_capacity(MAX_WRITE_FRAMES * BYTES_PER_FRAME);
    let _ = renderer.render(&frames, 0.7, &mut bytes);
    let n = allocations(|| {
        for i in 0..200 {
            let block = &frames[..MAX_WRITE_FRAMES - (i % 7) * 100];
            let rms = renderer.render(block, if i % 50 < 25 { 0.7 } else { 0.0 }, &mut bytes);
            assert!(rms.is_finite());
        }
    });
    assert_eq!(n, 0, "rendering allocated {n} times");
    // Positive control: an under-sized buffer does allocate.
    let mut small = Vec::new();
    assert!(allocations(|| {
        let _ = renderer.render(&frames, 0.7, &mut small);
    }) > 0);
}

#[test]
fn resampling_into_the_session_buffer_does_not_allocate() {
    let mut pending: Vec<[f32; 2]> = Vec::with_capacity(8192 * 2 + 64);
    let mut resample = Resample::new(44_100, 48_000);
    let n = allocations(|| {
        for _ in 0..100 {
            pending.clear();
            for i in 0..4608 {
                resample.push([i as f32, -(i as f32)], &mut pending);
            }
        }
    });
    assert_eq!(n, 0, "resampling allocated {n} times");
}

#[test]
fn the_network_path_does_not_allocate() {
    let ring = Ring::new(64 * 1024, 1);
    let mut demux = Demux::new(Some(1000));
    // 16 runs of 1000 audio bytes, each followed by an empty metadata block.
    let mut chunk = vec![0x55u8; 16 * 1001];
    for i in (1000..chunk.len()).step_by(1001) {
        chunk[i] = 0;
    }
    let mut out = vec![0u8; 16 * 1024];
    let mut adts = Adts::default();
    let n = allocations(|| {
        for _ in 0..50 {
            demux.feed(&chunk, &mut |a| assert!(ring.write_all(a)), &mut |_| {});
            let mut got = 0;
            while got < 16_000 {
                let n = ring.read_into(&mut out);
                adts.fix(&mut out[..n]);
                got += n;
            }
            while !ring.is_empty() {
                let _ = ring.read_into(&mut out);
            }
        }
    });
    assert_eq!(n, 0, "the network path allocated {n} times");
}
