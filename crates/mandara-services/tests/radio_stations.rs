//! Network check of the curated radio stations: each one must connect,
//! send audio with an audio content type and decode. Ignored by default
//! (it needs the internet); run it before changing the list or its
//! verification date:
//!
//! ```text
//! cargo test -p mandara-services --test radio_stations -- --ignored --nocapture
//! ```

use mandara_services::radio::{CURATED, probe};

#[test]
#[ignore = "connects to every curated station"]
fn radio_curated_stations_play() {
    // A few at a time: bounded threads and sockets.
    const PARALLEL: usize = 6;
    // `RADIO_ONLY=id,id` checks just those.
    let only = std::env::var("RADIO_ONLY").unwrap_or_default();
    let chosen: Vec<_> = CURATED.iter().filter(|s| only.is_empty() || only.split(',').any(|id| id.trim() == s.id)).collect();
    let mut failures = Vec::new();
    for chunk in chosen.chunks(PARALLEL) {
        let handles: Vec<_> = chunk.iter().map(|&s| (s, std::thread::spawn(move || probe(s.url)))).collect();
        for (s, h) in handles {
            match h.join().expect("probe thread") {
                Ok(p) => {
                    println!("ok   {:<28} {:?} {:>3} kbit/s {} Hz {} ch, {} bytes, {} frames, {}", s.id, p.info.codec, p.info.kbps.unwrap_or(0), p.info.rate, p.info.channels, p.bytes, p.frames, p.url);
                    assert_eq!(p.info.codec, s.codec, "{} sends another codec than listed", s.id);
                }
                Err(e) => {
                    println!("FAIL {:<28} {e}", s.id);
                    failures.push((s.id, e));
                }
            }
        }
    }
    // Busy servers answer 500 to a burst from one address: once more, one
    // at a time.
    let mut still = Vec::new();
    for (id, first) in failures {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let s = CURATED.iter().find(|s| s.id == id).expect("listed");
        match probe(s.url) {
            Ok(p) => println!("ok   {:<28} on the second try ({first}), {:?} {} Hz", s.id, p.info.codec, p.info.rate),
            Err(e) => {
                println!("FAIL {:<28} {e}", s.id);
                still.push((s.id, e));
            }
        }
    }
    assert!(still.is_empty(), "stations that failed: {still:?}");
}

/// `RADIO_URL="<address> …" cargo test -p mandara-services --test
/// radio_stations radio_probe_urls -- --ignored --nocapture` checks any
/// stations the way adding one in the panel does.
#[test]
#[ignore = "connects to RADIO_URL"]
fn radio_probe_urls() {
    let urls = std::env::var("RADIO_URL").unwrap_or_default();
    for url in urls.split_whitespace() {
        match probe(url) {
            Ok(p) => println!("ok   {url}: {:?} {:?} kbit/s {} Hz {} ch, {} bytes, {} frames, {}", p.info.codec, p.info.kbps, p.info.rate, p.info.channels, p.bytes, p.frames, p.url),
            Err(e) => println!("FAIL {url}: {e}"),
        }
    }
}
