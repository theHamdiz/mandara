//! End-to-end: encode tiny AV1 clips with rav1e, then demux + decode +
//! convert them with the pure-Rust pipeline (the greeter's player, the
//! desktop's frame stream and the stills).

#[path = "../examples/clipgen/mod.rs"]
mod clipgen;

use std::time::{Duration, Instant};

use mandara_media::av1::DecoderSettings;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mandara_media::{Crop, Demuxer, FrameStream, PixelOrder, Player, PlayerEvent, PlayerOptions, StreamEvent, StreamOptions, Target, bench, first_frame, representative_frame};

/// The test player's frame type: the converted BGRA pixels themselves.
fn keep_bgra(px: Vec<u8>, _w: u32, _h: u32) -> Option<Vec<u8>> {
    Some(px)
}

fn clip(dir: &std::path::Path, name: &str, webm: bool, frames: u32) -> std::path::PathBuf {
    let path = dir.join(name);
    let spec = clipgen::ClipSpec { width: 160, height: 96, fps: 24, frames, detail: 1, webm, speed: 10 };
    clipgen::write_clip(&path, &spec).unwrap();
    path
}

#[test]
fn webm_and_ivf_decode_every_frame() {
    let dir = tempfile::tempdir().unwrap();
    for (name, webm) in [("a.webm", true), ("a.ivf", false)] {
        let path = clip(dir.path(), name, webm, 12);
        let d = Demuxer::open(&path).unwrap();
        let info = d.info();
        assert_eq!((info.width, info.height), (160, 96), "{name}");
        if !webm {
            assert!((info.fps - 24.0).abs() < 0.01);
        }
        assert!(info.duration.is_some_and(|d| (d.as_secs_f64() - 0.5).abs() < 0.01), "{name}: {:?}", info.duration);
        for lite in [false, true] {
            let r = bench(&path, DecoderSettings { threads: 2, lite, ..DecoderSettings::default() }, false, 1000).unwrap();
            assert_eq!(r.frames, 12, "{name} lite={lite}");
            assert_eq!((r.width, r.height), (160, 96));
        }
    }
}

#[test]
fn decoded_colours_match_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = clip(dir.path(), "c.ivf", false, 2);
    let mut demux = Demuxer::open(&path).unwrap();
    let mut dec = mandara_media::av1::Av1Decoder::new(DecoderSettings::default()).unwrap();
    let mut pics = Vec::new();
    while let Some(p) = demux.next_packet().unwrap() {
        pics.extend(dec.decode(&p.data, p.pts).unwrap());
    }
    pics.extend(dec.drain().unwrap());
    let (bgra, w, h) = pics[0].to_bgra(false).unwrap();
    assert_eq!((w, h), (160, 96));
    // The generator's frame 0 is a dark blue-teal aurora: blue dominates
    // red in the top-left corner and nothing is pure black or white.
    let px = &bgra[..4];
    assert!(px[0] > px[2], "top-left should be bluish, got BGRA {px:?}");
    let mean: f64 = bgra.chunks_exact(4).map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0).sum::<f64>() / (w * h) as f64;
    assert!((20.0..200.0).contains(&mean), "mean {mean}");
}

#[test]
fn player_loops_with_monotonic_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let path = clip(dir.path(), "loop.webm", true, 6);
    // 6 frames at 24 fps = 250 ms; ask for at least 600 ms → 3 passes.
    let player = Player::spawn(PlayerOptions { path, target: (160, 96), lite: false, play_for: Some(Duration::from_millis(600)), queue: 3 }, keep_bgra);
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut frames, mut last, mut ended, mut info) = (0, None::<Duration>, false, false);
    while Instant::now() < deadline && !ended {
        match player.try_recv() {
            Some(PlayerEvent::Info(i)) => {
                info = true;
                assert_eq!(i.width, 160);
            }
            Some(PlayerEvent::Frame(f)) => {
                assert_eq!(f.image.len(), 160 * 96 * 4);
                if let Some(prev) = last {
                    assert!(f.pts > prev, "pts must increase across loops: {prev:?} → {:?}", f.pts);
                }
                last = Some(f.pts);
                frames += 1;
            }
            Some(PlayerEvent::End) => ended = true,
            Some(PlayerEvent::Error(e)) => panic!("{e}"),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert!(info && ended, "info={info} ended={ended}");
    assert_eq!(frames, 18);
    assert!(last.unwrap() >= Duration::from_millis(700));
}

#[test]
fn broken_files_report_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.webm");
    std::fs::write(&bad, b"definitely not a video").unwrap();
    assert!(Demuxer::open(&bad).is_err());
    let player = Player::spawn(PlayerOptions { path: bad, target: (1920, 1080), lite: false, play_for: None, queue: 2 }, keep_bgra);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match player.try_recv() {
            Some(PlayerEvent::Error(_)) => break,
            Some(_) => panic!("expected an error"),
            None if Instant::now() > deadline => panic!("no error reported"),
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn stream(path: std::path::PathBuf, notify: impl Fn() + Send + Sync + 'static) -> FrameStream {
    FrameStream::spawn(StreamOptions { path, threads: 2, frame_delay: 1, lite: false, queue: mandara_media::player::STREAM_QUEUE }, notify)
}

/// Waits for the next stream event (panics after `limit`).
fn next_event(s: &FrameStream, limit: Duration) -> StreamEvent {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(ev) = s.try_recv() {
            return ev;
        }
        assert!(Instant::now() < deadline, "no stream event within {limit:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn frame_stream_loops_seamlessly_with_wrapping_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    // 6 frames at 24 fps: one pass is 250 ms.
    let path = clip(dir.path(), "wall.webm", true, 6);
    let s = stream(path, || {});
    let StreamEvent::Info(info) = next_event(&s, Duration::from_secs(20)) else { panic!("info first") };
    assert_eq!((info.width, info.height), (160, 96));
    let step = Duration::from_secs_f64(1.0 / 24.0);
    let mut pts = Vec::new();
    while pts.len() < 15 {
        match next_event(&s, Duration::from_secs(20)) {
            StreamEvent::Frame(f) => {
                assert_eq!((f.picture.width(), f.picture.height()), (160, 96));
                pts.push(f.pts);
            }
            StreamEvent::Error(e) => panic!("{e}"),
            StreamEvent::Info(_) => panic!("one info"),
        }
    }
    // Strictly increasing, evenly spaced — including across the loop seams
    // after frames 6 and 12 (no gap, no repeat).
    for w in pts.windows(2) {
        let d = w[1] - w[0];
        assert!(d.abs_diff(step) < Duration::from_millis(2), "step {d:?} between {:?} and {:?}", w[0], w[1]);
    }
    assert!(pts[6].abs_diff(step * 6) < Duration::from_millis(2), "second pass starts one clip later: {:?}", pts[6]);
}

#[test]
fn frame_stream_parks_when_unread_and_wakes_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = clip(dir.path(), "park.ivf", false, 8);
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let s = stream(path, move || {
        w.fetch_add(1, Ordering::SeqCst);
    });
    // Unread, the decoder fills the queue and blocks.
    std::thread::sleep(Duration::from_millis(1500));
    let mut queued = 0;
    while let Some(ev) = s.try_recv() {
        if matches!(ev, StreamEvent::Frame(_)) {
            queued += 1;
        }
    }
    assert!(queued <= mandara_media::player::STREAM_QUEUE + 1, "a parked decoder runs at most a queue (plus the one blocked send) ahead, got {queued}");
    assert_eq!(wakes.load(Ordering::SeqCst), 0, "nobody asked to be woken");
    // An empty queue + a request: either a picture is already there, or
    // the next one queued calls notify (exactly once) and is then readable.
    if s.wake_on_next().is_none() {
        let deadline = Instant::now() + Duration::from_secs(20);
        while wakes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "notify never ran");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(s.try_recv().is_some(), "woken for a queued picture");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "one request, one wake-up");
    }
}

#[test]
fn stills_come_from_real_frames() {
    let dir = tempfile::tempdir().unwrap();
    for (name, webm) in [("s.webm", true), ("s.ivf", false)] {
        let path = clip(dir.path(), name, webm, 12);
        let first = first_frame(&path).unwrap();
        assert_eq!(first.dimensions(), (160, 96));
        assert!(first.pixels().all(|p| p[3] == 255), "opaque");
        // The aurora's top-left is bluish (as in the colour test above).
        let p = first.get_pixel(0, 0);
        assert!(p[2] > p[0], "{name}: {p:?}");
        let mid = representative_frame(&path).unwrap();
        assert_eq!(mid.dimensions(), (160, 96));
    }
}

#[test]
fn region_conversion_of_a_decoded_picture_matches_the_full_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = clip(dir.path(), "r.ivf", false, 2);
    let mut demux = Demuxer::open(&path).unwrap();
    let mut dec = mandara_media::av1::Av1Decoder::new(DecoderSettings::default()).unwrap();
    let mut pics = Vec::new();
    while let Some(p) = demux.next_packet().unwrap() {
        pics.extend(dec.decode(&p.data, p.pts).unwrap());
    }
    pics.extend(dec.drain().unwrap());
    let (full, w, _) = pics[0].to_bgra(false).unwrap();
    // A centred 80 × 48 window, 1:1, into a padded buffer.
    let stride = 80 * 4 + 16;
    let mut data = vec![0u8; stride * 48];
    assert!(pics[0].convert_region(Crop { x: 40, y: 24, w: 80, h: 48 }, &mut Target { data: &mut data, width: 80, height: 48, stride, order: PixelOrder::Bgra }, 2));
    for r in 0..48 {
        let src = &full[((24 + r) * w + 40) * 4..((24 + r) * w + 120) * 4];
        assert_eq!(&data[r * stride..r * stride + 320], src, "row {r}");
    }
}
