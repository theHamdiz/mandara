//! Counts only the calling test thread, outside cold view-state construction.

use std::cell::Cell;
use std::hint::black_box;
use std::rc::Rc;

use gpui::{Bounds, Pixels, point, px, size};
use mandara_services::net_speed::RateHistory;
use mandara_ui::slider_model;

#[path = "../src/plugins/net_speed/chart_samples.rs"]
mod chart_samples;
#[path = "../src/plugins/prayer/day_events.rs"]
mod day_events;
#[path = "../src/plugins/jannah/progress.rs"]
mod jannah_progress;
#[path = "../src/plugins/keyboard/label.rs"]
mod keyboard_label;
#[path = "../src/plugins/phone_connect/devices.rs"]
mod phone_devices;
#[path = "../src/plugins/lifecycle/switches.rs"]
mod plugin_switches;
#[path = "../src/plugins/progress/order.rs"]
mod progress_order;
#[path = "../src/plugins/progress/state.rs"]
mod progress_state;
#[path = "../src/plugins/media/scrub_state.rs"]
mod scrub_state;
#[path = "../src/plugins/settings_cache.rs"]
mod settings_cache;
#[path = "../src/plugins/resources/temperature_rows.rs"]
mod temperature_rows;
#[path = "../src/plugins/world_clock/planner.rs"]
mod world_clock_planner;

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Planner redraws share the track cell while handlers keep seeing its latest layout.
#[test]
fn world_clock_planner_reuses_its_track_bounds() {
    const FRAMES: usize = 32;
    let bounds = Rc::new(Cell::new(Bounds::new(point(px(10.0), px(0.0)), size(px(100.0), px(10.0)))));
    let before = allocations(|| {
        for _ in 0..FRAMES { black_box(Rc::new(Cell::new(Bounds::<Pixels>::default()))); }
    });
    let after = allocations(|| {
        for _ in 0..FRAMES {
            let handler = black_box(bounds.clone());
            assert_eq!(world_clock_planner::fraction(&handler, px(35.0)), 0.25);
        }
    });
    assert_eq!(before.0, FRAMES, "the old per-render bounds cell remains an allocating positive control");
    assert_eq!(after, (0, 0));
    eprintln!("{FRAMES} World Clock planner frames: fresh bounds {before:?}, retained bounds {after:?}");
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

/// Consuming an already-RGBA resize result preserves its allocation.
#[test]
fn owned_rgba_conversion_does_not_clone_pixel_storage() {
    let old = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        128,
        64,
        image::Rgba([10, 20, 30, 79]),
    ));
    let owned = old.clone();
    let before = allocations(|| {
        black_box(old.to_rgba8());
    });
    let after = allocations(|| {
        black_box(owned.into_rgba8());
    });
    eprintln!("RGBA conversion: borrowed copy {before:?}, owned buffer {after:?}");
    assert_eq!(
        before,
        (1, 128 * 64 * 4),
        "the positive control must observe the pixel copy"
    );
    assert_eq!(after, (0, 0));
}

/// Rendering two-column temperature rows borrows readings instead of collecting pairs.
#[test]
fn temperature_rows_do_not_allocate_an_intermediate_list() {
    const FRAMES: usize = 32;
    let temperatures = [10, 20, 30, 40, 50];
    let labels = ["a", "b", "c", "d", "e"];
    let before = allocations(|| {
        for _ in 0..FRAMES {
            let pairs = black_box(&temperatures)
                .iter()
                .zip(black_box(&labels))
                .collect::<Vec<_>>();
            for row in pairs.chunks(2) {
                black_box(row);
            }
        }
    });
    let after = allocations(|| {
        for _ in 0..FRAMES {
            for row in
                temperature_rows::temperature_rows(black_box(&temperatures), black_box(&labels), 5)
            {
                black_box(row);
            }
        }
    });
    eprintln!("{FRAMES} temperature frames: pair Vec {before:?}, borrowed rows {after:?}");
    assert_eq!(before.0, FRAMES);
    assert_eq!(after, (0, 0));
}

/// Resolving unchanged text settings reuses the cached value without copying TOML.
#[test]
fn warm_plugin_settings_do_not_allocate() {
    use mandara_config::ShellConfig;
    use mandara_i18n::Lang;
    use settings_cache::{ConfigSettings, SettingsCache};
    use std::sync::Arc;

    struct TextSettings(String);
    impl ConfigSettings for TextSettings {
        type Scope = ();
        fn resolve(config: &ShellConfig, _: Lang, (): ()) -> Self {
            Self(
                config
                    .plugins
                    .setting_str("demo", "label")
                    .unwrap_or("follow")
                    .to_owned(),
            )
        }
    }
    const READS: usize = 32;
    let mut config = ShellConfig::default();
    config
        .plugins
        .settings
        .entry("demo".into())
        .or_default()
        .insert(
            "label".into(),
            toml::Value::String("Demo cached label".into()),
        );
    let config = Arc::new(config);
    let mut cache = SettingsCache::<TextSettings>::new();
    let warm = cache.get(&config, Lang::En, ());
    assert_eq!(warm.0, "Demo cached label");
    let before = allocations(|| {
        for _ in 0..READS {
            black_box(
                config
                    .plugins
                    .setting("demo", "label", String::from("follow")),
            );
        }
    });
    let after = allocations(|| {
        for _ in 0..READS {
            black_box(cache.get(&config, Lang::En, ()));
        }
    });
    eprintln!("{READS} settings reads: eager strings {before:?}, warm cache {after:?}");
    assert!(
        before.0 >= READS,
        "the positive control must copy owned labels"
    );
    assert_eq!(after, (0, 0));
}

/// Weekly progress formatting runs once per saved-state/calendar change.
#[test]
fn warm_progress_views_do_not_reformat_date_keys() {
    use chrono::NaiveDate;
    use mandara_services::jannah::state::State;

    const FRAMES: usize = 32;
    let date = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    let mut state = State::default();
    state.mark_done(date);
    let mut cache = jannah_progress::ProgressCache::default();
    assert!(cache.get(&state, 1, date, 0).done);
    let before = allocations(|| {
        for _ in 0..FRAMES {
            black_box((
                state.done_on(date),
                state.week(date, 0),
                state.reminded_on(date),
            ));
        }
    });
    let after = allocations(|| {
        for _ in 0..FRAMES {
            black_box(cache.get(&state, 1, date, 0));
        }
    });
    eprintln!("{FRAMES} progress frames: date lookups {before:?}, snapshot {after:?}");
    assert!(before.0 >= FRAMES);
    assert_eq!(after, (0, 0));
}

/// A warm keyboard badge compares borrowed layout/custom text before copying it.
#[test]
fn keyboard_badge_does_not_rebuild_its_cache_key_per_frame() {
    const FRAMES: usize = 32;
    let mut labels = keyboard_label::LayoutLabel::default();
    labels.label("English (US)", Some("EN"));
    let before = allocations(|| {
        for _ in 0..FRAMES {
            let custom = black_box(Some("EN")).map(str::to_string);
            black_box(format!(
                "{}\u{1f}{}",
                black_box("English (US)"),
                custom.as_deref().unwrap_or_default()
            ));
        }
    });
    let after = allocations(|| {
        for _ in 0..FRAMES {
            black_box(labels.label("English (US)", Some("EN")));
        }
    });
    eprintln!("{FRAMES} keyboard frames: owned key {before:?}, borrowed comparison {after:?}");
    assert!(before.0 >= FRAMES * 2);
    assert_eq!(after, (0, 0));
}

/// Device-list empty-state detection and traversal require no temporary pointer list.
#[test]
fn phone_device_filter_does_not_collect_rows() {
    use mandara_services::phone_connect::Device;
    let devices = [false, true, true].map(|paired| Device {
        id: "demo".into(),
        name: "Demo".into(),
        kind: "phone".into(),
        reachable: true,
        paired,
        incoming_pair: false,
        battery: None,
        plugins: Vec::new(),
    });
    const FRAMES: usize = 32;
    let before = allocations(|| {
        for _ in 0..FRAMES {
            black_box(
                devices
                    .iter()
                    .filter(|device| device.paired)
                    .collect::<Vec<_>>(),
            );
        }
    });
    let after = allocations(|| {
        for _ in 0..FRAMES {
            let mut visible = phone_devices::visible_devices(black_box(&devices), false).peekable();
            black_box(visible.peek());
            for device in visible {
                black_box(device);
            }
        }
    });
    eprintln!("{FRAMES} phone frames: row Vec {before:?}, iterator {after:?}");
    assert!(before.0 >= FRAMES);
    assert_eq!(after, (0, 0));
}

/// Unchanged switch lists do not copy unrelated plugin tables on config notifications.
#[test]
fn plugin_lifecycle_does_not_clone_settings_for_unchanged_switches() {
    let mut config = mandara_config::ShellConfig::default().plugins;
    config.enabled.extend(["feeds".into(), "notes".into()]);
    config.settings.insert("feeds".into(), toml::toml! { subscriptions = "https://example.org/feed" });
    let mut switches = plugin_switches::Switches::new();
    assert!(switches.changed(&config));
    const UPDATES: usize = 32;
    let before = allocations(|| {
        for _ in 0..UPDATES { black_box(config.clone()); }
    });
    let after = allocations(|| {
        for _ in 0..UPDATES { assert!(!switches.changed(black_box(&config))); }
    });
    eprintln!("{UPDATES} plugin config notifications: whole settings clone {before:?}, borrowed switch comparison {after:?}");
    assert!(before.0 >= UPDATES, "the positive control must copy owned plugin settings");
    assert_eq!(after, (0, 0));
}

/// Display ordering and live semantic colours borrow the unchanged task snapshot.
#[test]
fn progress_rows_do_not_collect_an_intermediate_display_order() {
    use mandara_services::progress::{Board, Status};
    let mut board = Board::default();
    for id in ["first", "second", "third", "last"] { board.start(id, id, 0).unwrap(); }
    board.done("second", "", 1000).unwrap();
    let palette = mandara_theme::catalog::builtin()[0].palette(true);
    let _language = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
    for task in &board.tasks { black_box(progress_state::TaskState::from(task).label(1000)); }
    const FRAMES: usize = 32;
    let before = allocations(|| {
        for _ in 0..FRAMES {
            let mut order: Vec<_> = board.running().collect();
            order.extend(board.tasks.iter().filter(|task| task.status != Status::Running));
            black_box(order);
        }
    });
    let after = allocations(|| {
        for _ in 0..FRAMES {
            for task in progress_order::ordered_tasks(black_box(&board.tasks)) {
                let state = progress_state::TaskState::from(task);
                black_box((state.color(1000, &palette), state.label(1000)));
            }
        }
    });
    eprintln!("{FRAMES} progress row traversals: ordered Vec {before:?}, borrowed order and live roles {after:?}");
    assert!(before.0 >= FRAMES, "the old display order must remain a live allocation control");
    assert_eq!(after, (0, 0));
}
