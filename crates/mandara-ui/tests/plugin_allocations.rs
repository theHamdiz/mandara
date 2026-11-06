//! Counts only the calling test thread, outside cold view-state construction.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::rc::Rc;

use gpui::{Bounds, Pixels, point, px, size};
use mandara_services::net_speed::RateHistory;

#[path = "../src/plugins/net_speed/chart_samples.rs"]
mod chart_samples;
#[path = "../src/plugins/prayer/day_events.rs"]
mod day_events;
#[path = "../src/plugins/media/scrub_state.rs"]
mod scrub_state;

struct Counting;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// Leaves allocations on other test and runtime threads out of the measurement.
fn record(bytes: usize) {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| {
            let (calls, total) = count.get();
            count.set((calls + 1, total + bytes));
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

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Measures allocation calls and requested bytes after all fixtures are prepared.
fn allocations(work: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|count| count.set((0, 0)));
    TRACKING.with(|tracking| tracking.set(true));
    work();
    TRACKING.with(|tracking| tracking.set(false));
    ALLOCATIONS.with(Cell::get)
}

/// Warm redraws share gesture geometry and borrow either history window.
#[test]
fn scrubber_and_chart_updates_reuse_existing_storage() {
    const UPDATES: usize = 32;
    let scrubber = Rc::new(scrub_state::ScrubState::default());
    scrubber.bounds.set(Bounds {
        origin: point(px(10.0), px(0.0)),
        size: size(px(100.0), px(10.0)),
    });
    let mut history = RateHistory::default();
    history.seconds.extend([(1.5, 2.25); 60]);
    history.buckets.extend([(0.125, 0.0625); 60]);
    // The previous render/cache expressions provide a live positive control.
    let before = allocations(|| {
        for _ in 0..UPDATES {
            black_box(Rc::new(Cell::new(Bounds::<Pixels>::default())));
            black_box(history.seconds.iter().copied().collect::<Vec<_>>());
        }
    });
    let after = allocations(|| {
        for frame in 0..UPDATES {
            let handler = black_box(scrubber.clone());
            black_box(handler.fraction(px(35.0)));
            let samples = chart_samples::chart_samples(Some(&history), frame % 2 == 0).unwrap();
            for sample in chart_samples::chart_columns(samples, 60) {
                black_box(sample);
            }
        }
    });
    eprintln!(
        "{UPDATES} view/cache updates: old expressions {before:?}, reused state {after:?}; ScrubState={} B, bounds={} B",
        size_of::<scrub_state::ScrubState>(),
        size_of::<Bounds<Pixels>>()
    );
    assert_eq!(
        before.0,
        UPDATES * 2,
        "the counter must see both old temporary allocations"
    );
    assert_eq!(
        after,
        (0, 0),
        "warm gesture and history access must not allocate"
    );
}

/// Boundary selection no longer allocates a temporary list each visible tick.
#[test]
fn prayer_boundary_ticks_do_not_allocate() {
    use chrono::{NaiveDate, NaiveTime};
    use mandara_services::prayer::{Prayer, PrayerTimes};

    const TICKS: usize = 32;
    let date = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
    let day = |date| PrayerTimes {
        date,
        times: std::array::from_fn(|index| {
            (
                [
                    Prayer::Fajr,
                    Prayer::Sunrise,
                    Prayer::Dhuhr,
                    Prayer::Asr,
                    Prayer::Maghrib,
                    Prayer::Isha,
                ][index],
                NaiveTime::from_hms_opt([5, 6, 12, 15, 18, 20][index], 0, 0).unwrap(),
            )
        }),
    };
    let days = [
        day(date.pred_opt().unwrap()),
        day(date),
        day(date.succ_opt().unwrap()),
    ];
    let now = date.and_hms_opt(6, 0, 0).unwrap();
    let before = allocations(|| {
        for _ in 0..TICKS {
            let mut events = Vec::with_capacity(8);
            events.push((
                Prayer::Isha,
                days[0].date.and_time(days[0].get(Prayer::Isha)),
            ));
            events.extend(
                days[1]
                    .times
                    .map(|(prayer, time)| (prayer, date.and_time(time))),
            );
            events.push((
                Prayer::Fajr,
                days[2].date.and_time(days[2].get(Prayer::Fajr)),
            ));
            black_box(
                events
                    .iter()
                    .rev()
                    .find(|(prayer, time)| prayer.is_prayer() && *time <= now),
            );
            black_box(events);
        }
    });
    let after = allocations(|| {
        for _ in 0..TICKS {
            black_box(day_events::surrounding_prayers(now, &days));
        }
    });
    eprintln!("{TICKS} prayer ticks: event Vec {before:?}, iterator {after:?}");
    assert_eq!(
        before.0, TICKS,
        "the positive control must observe every temporary list"
    );
    assert_eq!(after, (0, 0));
}
