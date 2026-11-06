//! Per-frame geometry stays inline without changing visible bounds.

use std::hint::black_box;

use gpui::{Bounds, Pixels, point, px, size};
use mandara_config::{BarLayout, BarPosition};
use mandara_core::FlareCenter;
use mandara_ui::bar::{layout, side};

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Exercise repeat redraws without counting fixture setup.
const REDRAWS: usize = 32;

/// Both ends of joined columns stay inline across layouts and visibility.
#[test]
fn column_redraws_keep_both_flares_without_heap_storage() {
    let old = allocations(|| {
        for _ in 0..REDRAWS {
            black_box(Vec::<(f32, f32, f32, FlareCenter)>::with_capacity(2));
        }
    });
    assert_eq!(old.0, REDRAWS, "the former expression must remain a live control");
    let fixtures = [BarPosition::Top, BarPosition::Bottom].map(|position| {
        (position, [BarLayout::Notch, BarLayout::MinimalEdge].map(|kind| layout::Metrics::default_for(kind, position)))
    });
    let current = allocations(|| {
        for (position, layouts) in &fixtures {
            for metrics in layouts {
                for right in [false, true] {
                    for shown in [0.0, 0.5, 1.0] {
                        let column = side::joined_column(right, 1600.0, 40.0, shown, (30.0, 900.0), 12.0, *position == BarPosition::Top);
                        assert!(!column.flares.spilled());
                        black_box(column);
                        let panel = layout::side_column(metrics, (1600.0, 900.0), 0.0, 380.0, right, (30.0, 870.0), 12.0);
                        assert!(!panel.flares.spilled());
                        black_box(panel);
                    }
                }
            }
        }
    });
    assert_eq!(current, (0, 0));
    eprintln!("flare buffers: old {old:?}, current {current:?}");
}

/// Hidden gaps and fractional coordinates preserve the same enclosing rect.
#[test]
fn capsule_union_borrows_visible_geometry() {
    let bounds = |x, y, w, h| Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    let slots = [None, Some(bounds(12.5, -3.25, 20.75, 15.5)), None, Some(bounds(-7.5, 8.25, 11.0, 30.0))];
    let expected = bounds(-7.5, -3.25, 40.75, 41.5);
    assert_eq!(layout::union_iter(slots.iter().flatten().copied()), Some(expected));
    assert_eq!(layout::union_iter([]), None);
    assert_eq!(layout::union(&[]), Bounds::default());
    let old = allocations(|| {
        for _ in 0..REDRAWS {
            let visible: Vec<Bounds<Pixels>> = black_box(&slots).iter().flatten().copied().collect();
            black_box(layout::union(&visible));
        }
    });
    assert!(old.0 >= REDRAWS);
    let current = allocations(|| {
        for _ in 0..REDRAWS {
            black_box(layout::union_iter(black_box(&slots).iter().flatten().copied()));
        }
    });
    assert_eq!(current, (0, 0));
    eprintln!("capsule union: old {old:?}, current {current:?}");
}

/// Inline storage can enlarge the value while removing its separate allocation.
#[test]
fn record_column_value_sizes_without_assuming_rust_field_order() {
    type Flare = (f32, f32, f32, FlareCenter);
    struct LegacyColumn {
        column: (f32, f32, f32, f32),
        flares: Vec<Flare>,
        notch: Option<(f32, f32, bool, bool)>,
    }
    let legacy = LegacyColumn { column: (0.0, 0.0, 380.0, 900.0), flares: Vec::with_capacity(2), notch: None };
    black_box((legacy.column, legacy.notch));
    let old = std::mem::size_of_val(&legacy);
    let current = std::mem::size_of::<layout::SideColumn>();
    let old_payload = legacy.flares.capacity() * std::mem::size_of::<Flare>();
    eprintln!("column value: old {old} bytes + {old_payload} payload, current {current} bytes inline");
    assert!(current <= old + old_payload, "inline storage must not cost more than the old value and its always-reserved payload");
}
