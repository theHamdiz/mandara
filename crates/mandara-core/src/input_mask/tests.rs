use super::*;

mod allocations;

fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

#[test]
fn snaps_outward_and_drops_contained() {
    let reg = InputMaskRegistry::new();
    for bounds in [b(10.4, 5.6, 100.2, 30.0), b(20.0, 10.0, 5.0, 5.0), b(300.0, 0.0, 50.0, 40.0)] {
        reg.register(bounds);
    }
    assert_eq!(reg.take_changed_region(), Some(vec![(10, 5, 101, 31), (300, 0, 50, 40)]));
}

#[test]
fn only_commits_on_change() {
    let reg = InputMaskRegistry::new();
    reg.register(b(0.0, 0.0, 100.0, 40.0));
    assert!(reg.take_changed_region().is_some());
    reg.register(b(0.0, 0.0, 100.0, 40.0));
    assert!(reg.take_changed_region().is_none(), "identical frame must not re-commit");
    reg.register(b(0.0, 0.0, 120.0, 40.0));
    assert_eq!(reg.take_changed_region(), Some(vec![(0, 0, 120, 40)]));
    assert_eq!(reg.commit_count(), 2);
}

#[test]
fn empty_frame_yields_empty_region() {
    let reg = InputMaskRegistry::new();
    reg.register(b(0.0, 0.0, 10.0, 10.0));
    reg.take_changed_region();
    assert_eq!(reg.take_changed_region(), Some(vec![]));
    reg.register(b(5.0, 5.0, 0.0, 10.0));
    assert_eq!(reg.take_changed_region(), None);
}

#[test]
fn normalization_matches_the_snapshot_algorithm() {
    let domain = [-3, 0, 2, 7];
    let mut all = Vec::new();
    for x in domain {
        for y in domain {
            for width in [1, 3, 8] {
                for height in [1, 3, 8] {
                    all.push((x, y, width, height));
                }
            }
        }
    }
    for offset in 0..all.len() {
        let mut rects: SmallVec<[IRect; 8]> = all.iter().cycle().skip(offset).take(offset + 1).copied().collect();
        rects.extend_from_slice(&[(0, 0, 1, 1), (0, 0, 1, 1)]);
        let mut expected = rects.to_vec();
        expected.sort_unstable();
        expected.dedup();
        let snapshot = expected.clone();
        expected.retain(|rect| !snapshot.iter().any(|outer| outer != rect && outer.0 <= rect.0 && outer.1 <= rect.1 && outer.0 + outer.2 >= rect.0 + rect.2 && outer.1 + outer.3 >= rect.1 + rect.3));
        normalize(&mut rects);
        assert_eq!(rects.as_slice(), expected.as_slice(), "rotation {offset}");
    }
}

#[test]
fn large_regions_keep_buffers_across_changed_and_identical_frames() {
    let reg = InputMaskRegistry::new();
    let frame = |offset| {
        for index in 0..32 {
            reg.register(b(index as f32 * 20.0, offset, 10.0, 10.0));
        }
    };
    frame(0.0);
    reg.flush_with(|bounds| assert_eq!(bounds.len(), 32));
    let pointers = {
        let inner = reg.inner.borrow();
        (inner.pending.as_ptr(), inner.committed.as_ref().unwrap().as_ptr(), inner.bounds.as_ptr())
    };
    for offset in [1.0, 1.0, 2.0, 3.0] {
        frame(offset);
        reg.flush_with(|bounds| assert_eq!(f32::from(bounds[0].origin.y), offset));
        let inner = reg.inner.borrow();
        assert!(inner.pending.is_empty());
        assert_eq!((inner.pending.as_ptr(), inner.committed.as_ref().unwrap().as_ptr(), inner.bounds.as_ptr()), pointers);
    }
    assert_eq!(reg.commit_count(), 4);
}

#[test]
fn borrowed_application_and_diagnostic_snapshot_are_independent() {
    let reg = InputMaskRegistry::new();
    reg.register(b(-1.1, -2.1, 4.0, 5.0));
    reg.flush_with(|bounds| assert_eq!(bounds, &[b(-2.0, -3.0, 5.0, 6.0)]));
    let mut snapshot = reg.committed_region().unwrap();
    snapshot.clear();
    assert_eq!(reg.committed_region(), Some(vec![(-2, -3, 5, 6)]));
    reg.register(b(-1.1, -2.1, 4.0, 5.0));
    reg.flush_with(|_| panic!("an identical region was applied twice"));
    reg.flush_with(|bounds| assert!(bounds.is_empty()));
    assert_eq!(reg.commit_count(), 2);
}

#[test]
fn containment_does_not_overflow_at_integer_limits() {
    let mut rects = SmallVec::from_slice(&[(i32::MAX - 2, 0, 5, 10), (i32::MAX - 1, 1, 1, 2)]);
    normalize(&mut rects);
    assert_eq!(rects.as_slice(), &[(i32::MAX - 2, 0, 5, 10)]);
}

#[test]
fn warmed_input_frames_do_not_allocate_for_unchanged_or_moving_regions() {
    for rectangles in [1, 8, 32] {
        let reg = InputMaskRegistry::new();
        let frame = |offset| {
            for index in 0..rectangles {
                reg.register(b(index as f32 * 20.0, offset, 10.0, 10.0));
            }
            reg.flush_with(|bounds| { std::hint::black_box(bounds); });
        };
        frame(0.0);
        let (_, count) = allocations::measure(|| {
            for index in 0..100 {
                frame((index / 2) as f32);
            }
        });
        assert_eq!(count, allocations::Count::default(), "{rectangles} rectangles: {count:?}");
    }
}
