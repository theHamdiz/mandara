use super::*;

/// Positions of every shown symbol with `n` typed.
fn positions(row: &Row, n: usize, left: f32, width: f32, rtl: bool) -> Vec<(usize, f32)> {
    (0..n).filter_map(|i| row.left(i, n, left, width, rtl).map(|x| (i, x))).collect()
}

#[test]
fn earlier_symbols_never_move_while_typing_or_deleting() {
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
        for rtl in [false, true] {
            let (left, width) = (37.3, 247.0);
            let row = Row::new(width, 15.0, scale);
            let mut before = positions(&row, 0, left, width, rtl);
            // Type up to the capacity, then delete back to empty.
            let ups = 0..=row.capacity;
            let downs = (0..row.capacity).rev();
            for n in ups.chain(downs) {
                let now = positions(&row, n, left, width, rtl);
                for (i, x) in &now {
                    if let Some((_, old)) = before.iter().find(|(j, _)| j == i) {
                        assert_eq!(x, old, "symbol {i} moved at n={n}, scale {scale}, rtl {rtl}");
                    }
                }
                before = now;
            }
        }
    }
}

#[test]
fn sizes_and_advances_are_whole_device_pixels() {
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25] {
        for text in [12.0, 14.0, 15.0, 18.5] {
            let row = Row::new(300.0, text, scale);
            for v in [row.size, row.advance] {
                let device = v * scale;
                assert!((device - device.round()).abs() < 1e-3, "{v} at {scale}");
            }
            assert!(row.advance >= row.size);
            // Offsets are whole multiples of the advance, so they stay on
            // the device grid too.
            let o = row.offset(3, 5).unwrap();
            assert!((o * scale - (o * scale).round()).abs() < 1e-3);
        }
    }
}

#[test]
fn ltr_and_rtl_mirror_each_other() {
    let (left, width) = (10.0, 200.0);
    let row = Row::new(width, 15.0, 1.5);
    for n in 1..row.capacity {
        for i in 0..n {
            let l = row.left(i, n, left, width, false).unwrap();
            let r = row.left(i, n, left, width, true).unwrap();
            assert!(((l - left) - (left + width - (r + row.size))).abs() < 1e-4);
        }
    }
    // The first symbol hugs the start edge.
    assert_eq!(row.left(0, 1, left, width, false), Some(left));
    assert_eq!(row.left(0, 1, left, width, true), Some(left + width - row.size));
}

#[test]
fn a_full_row_scrolls_and_keeps_the_newest_visible() {
    let row = Row::new(120.0, 15.0, 1.0);
    let cap = row.capacity;
    assert!(cap >= 2);
    for n in cap..cap * 3 {
        let shown = positions(&row, n, 0.0, 120.0, false);
        assert_eq!(shown.len(), cap);
        assert_eq!(shown.last().map(|(i, _)| *i), Some(n - 1));
        // Everything shown fits inside the room.
        assert!(shown.iter().all(|(_, x)| *x >= 0.0 && x + row.size <= 120.0 + 1e-3));
        // The whole row moves one slot per character: predictable.
        let next = positions(&row, n + 1, 0.0, 120.0, false);
        for (i, x) in &next {
            if let Some((_, old)) = shown.iter().find(|(j, _)| j == i) {
                assert!((old - x - row.advance).abs() < 1e-4);
            }
        }
    }
    // A tiny field still shows one.
    assert_eq!(Row::new(1.0, 15.0, 1.0).capacity, 1);
}

#[test]
fn a_deleted_symbol_fades_out_in_its_old_slot() {
    let row = Row::new(200.0, 15.0, 1.0);
    // Five typed, the fifth deleted: n = 4, the ghost is index 4.
    assert_eq!(row.offset(4, 4), Some(4.0 * row.advance));
    assert_eq!(row.offset(5, 4), None);
}

#[test]
fn caret_sits_after_the_last_symbol() {
    let row = Row::new(200.0, 15.0, 1.0);
    assert_eq!(row.caret_offset(0), 0.0);
    for n in 1..row.capacity {
        let last_end = row.offset(n - 1, n).unwrap() + row.size;
        let c = row.caret_offset(n);
        assert!(c >= last_end && c <= last_end + row.advance - row.size + 1e-4);
    }
}

#[test]
fn shape_sequence_is_deterministic_and_neighbours_differ() {
    for seed in [1u64, 2, 42, 0xDEAD_BEEF, u64::MAX, seed_from_clock()] {
        let seq: Vec<Symbol> = (0..200).map(|i| symbol_at(seed, i)).collect();
        assert_eq!(seq, (0..200).map(|i| symbol_at(seed, i)).collect::<Vec<_>>());
        for w in seq.windows(2) {
            assert_ne!(w[0], w[1], "seed {seed}");
        }
        // Every run of the set uses every shape.
        let n = Symbol::ALL.len();
        for run in seq.chunks_exact(n) {
            for s in Symbol::ALL {
                assert!(run.contains(&s));
            }
        }
    }
    // Different entries look different.
    let a: Vec<Symbol> = (0..8).map(|i| symbol_at(7, i)).collect();
    let b: Vec<Symbol> = (0..8).map(|i| symbol_at(next_seed(7), i)).collect();
    assert_ne!(a, b);
}

#[test]
fn reduced_motion_is_instant() {
    assert_eq!(appear(Duration::ZERO, false), (1.0, 1.0));
    assert_eq!(vanish(Duration::ZERO, false), None);
    let mut f = Fades::default();
    let now = Instant::now();
    f.push(3, now);
    assert!(!f.active(now, false));
    assert!(f.active(now, true));
}

#[test]
fn pop_grows_with_a_small_overshoot_then_settles() {
    let (s0, a0) = appear(Duration::ZERO, true);
    assert!((s0 - POP_FROM).abs() < 1e-4 && a0 == 0.0);
    let peak = (0..300).map(|ms| appear(Duration::from_millis(ms), true).0).fold(0.0f32, f32::max);
    assert!(peak > 1.0 && peak < 1.2, "peak {peak}");
    let (end, _) = appear(POP - Duration::from_millis(1), true);
    assert!((end - 1.0).abs() < 0.03, "end {end}");
    assert_eq!(appear(POP, true), (1.0, 1.0));
}

#[test]
fn vanish_shrinks_and_fades_then_ends() {
    let (s, a) = vanish(Duration::ZERO, true).unwrap();
    assert_eq!((s, a), (1.0, 1.0));
    let (s, a) = vanish(FADE / 2, true).unwrap();
    assert!(s < 1.0 && a < 1.0);
    assert!(vanish(FADE, true).is_none());
}

#[test]
fn fades_are_bounded_and_retyping_supersedes_them() {
    let now = Instant::now();
    let mut f = Fades::default();
    for ix in 0..10 {
        f.push(ix, now + Duration::from_millis(ix as u64));
    }
    let later = now + Duration::from_millis(10);
    assert_eq!(f.live(later, true).count(), FADE_SLOTS);
    // The newest deletions are kept.
    assert!(f.live(later, true).any(|(ix, ..)| ix == 9));
    f.typed(8);
    assert!(f.live(later, true).all(|(ix, ..)| ix < 8));
    f.clear();
    assert!(!f.active(later, true));
}

/// Coverage of each shape rendered at `px`.
fn mask(s: Symbol, px: u32) -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(px, px).unwrap();
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let t = tiny_skia::Transform::from_scale(px as f32, px as f32);
    pm.fill_path(s.skia_path().unwrap(), &paint, tiny_skia::FillRule::Winding, t, None);
    pm.pixels().iter().map(|p| p.alpha()).collect()
}

#[test]
fn outlines_fill_their_box_and_read_as_distinct_shapes() {
    let px = 24;
    let masks: Vec<(Symbol, Vec<u8>)> = Symbol::ALL.iter().map(|&s| (s, mask(s, px))).collect();
    for (s, m) in &masks {
        let pts = s.outline();
        assert!(pts.len() >= 8, "{s:?}");
        assert!(pts.iter().all(|(x, y)| (-1e-4..=1.0001).contains(x) && (-1e-4..=1.0001).contains(y)), "{s:?} leaves its box");
        let cover = m.iter().map(|&a| f32::from(a) / 255.0).sum::<f32>() / (px * px) as f32;
        assert!((0.25..0.95).contains(&cover), "{s:?} covers {cover}");
    }
    // Pairwise, at least 3 % of the box differs (a square and a four-leaf clover differ by about 4 %).
    for (i, (a, ma)) in masks.iter().enumerate() {
        for (b, mb) in &masks[i + 1..] {
            let diff = ma.iter().zip(mb).map(|(x, y)| (f32::from(*x) - f32::from(*y)).abs() / 255.0).sum::<f32>() / (px * px) as f32;
            assert!(diff > 0.03, "{a:?} and {b:?} look alike ({diff})");
        }
    }
}

#[test]
fn svgs_are_shared_and_well_formed() {
    for s in Symbol::ALL {
        let svg = s.svg();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(std::ptr::eq(svg, s.svg()), "built once");
        assert_eq!(Symbol::from_asset_path(s.asset_path()), Some(s));
    }
    assert_eq!(Symbol::from_asset_path("icons/circle.svg"), None);
}
