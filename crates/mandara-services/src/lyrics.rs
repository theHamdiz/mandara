//! Time-synced lyrics from [lrclib.net](https://lrclib.net) (free, no key):
//! an exact `/api/get` lookup by artist, title, album and duration with a
//! `/api/search` fallback, an LRC parser (multiple stamps per line,
//! `[offset:]`, enhanced `<mm:ss.xx>` word stamps stripped) and a disk cache
//! under `~/.cache/mandara/lyrics/` — found lyrics are kept, "not found" is
//! remembered for a day so a missing song isn't re-queried on every play.
//!
//! Everything here is blocking; callers run it off the UI thread and only
//! while lyrics are actually on screen.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

/// Whole-request timeout for lrclib.net (network safety bound).
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);

const API: &str = "https://lrclib.net/api";
const UA: &str = concat!("mandara-shell/", env!("CARGO_PKG_VERSION"), " (https://github.com/theHamdiz/mandara)");
/// How long a "no lyrics for this song" answer is trusted.
const NOT_FOUND_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Search results whose length differs more than this are another recording.
const DURATION_SLACK: f64 = 4.0;

/// The song to look up (from MPRIS metadata).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrackQuery {
    pub artist: String,
    pub title: String,
    pub album: String,
    /// Whole seconds; lrclib matches within ±2 s.
    pub duration: Option<u64>,
}

impl TrackQuery {
    /// Enough to search for (a title, and an artist or album).
    pub fn is_searchable(&self) -> bool {
        !self.title.trim().is_empty() && !(self.artist.trim().is_empty() && self.album.trim().is_empty())
    }

    /// Stable cache key (FNV-1a over the normalised fields).
    pub fn key(&self) -> String {
        let norm = |s: &str| s.trim().to_lowercase();
        let text = format!("{}\u{1f}{}\u{1f}{}\u{1f}{}", norm(&self.artist), norm(&self.title), norm(&self.album), self.duration.unwrap_or(0));
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// One timed line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub time: Duration,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lyrics {
    /// Timed lines, sorted by time (blank lines kept as pauses).
    Synced(Vec<Line>),
    /// Untimed text lines.
    Plain(Vec<String>),
    Instrumental,
    NotFound,
}

impl Lyrics {
    pub fn is_found(&self) -> bool {
        !matches!(self, Lyrics::NotFound)
    }
}

// ── LRC parsing ──────────────────────────────────────────────────────────

/// Parses `mm:ss`, `mm:ss.xx`, `mm:ss.xxx` or `mm:ss:xx` (also `h:mm:ss.xx`).
fn parse_stamp(s: &str) -> Option<Duration> {
    let s = s.trim();
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [m, sec] => (0.0, m.parse::<f64>().ok()?, sec.parse::<f64>().ok()?),
        // `mm:ss:xx` (centiseconds after a colon) vs `h:mm:ss.xx`.
        [a, b, c] if !c.contains('.') && c.len() == 2 && b.len() == 2 && a.len() >= 2 => {
            (0.0, a.parse::<f64>().ok()?, format!("{b}.{c}").parse::<f64>().ok()?)
        }
        [h, m, sec] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, sec.parse::<f64>().ok()?),
        _ => return None,
    };
    if !(0.0..60.5).contains(&sec) || m < 0.0 || h < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(h * 3600.0 + m * 60.0 + sec))
}

/// Removes enhanced-LRC word stamps (`<00:12.34>`).
fn strip_word_stamps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        match rest[start..].find('>') {
            Some(end) if parse_stamp(&rest[start + 1..start + end]).is_some() => {
                out.push_str(&rest[..start]);
                rest = &rest[start + end + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses LRC text into time-sorted lines. Lines may carry several stamps
/// (`[00:10.00][01:20.00]chorus`); metadata tags are skipped and
/// `[offset:±ms]` is applied (positive = lyrics earlier, per the spec).
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut offset_ms: i64 = 0;
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start_matches('\u{feff}').trim();
        let mut stamps = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some(end) = tag.find(']') else { break };
            let inner = &tag[..end];
            if let Some(t) = parse_stamp(inner) {
                stamps.push(t);
            } else if let Some(v) = inner.strip_prefix("offset:") {
                offset_ms = v.trim().trim_start_matches('+').parse().unwrap_or(0);
            }
            rest = tag[end + 1..].trim_start();
        }
        if stamps.is_empty() {
            continue;
        }
        let text = strip_word_stamps(rest);
        for t in stamps {
            let ms = t.as_millis() as i64 - offset_ms;
            lines.push(Line { time: Duration::from_millis(ms.max(0) as u64), text: text.clone() });
        }
    }
    lines.sort_by_key(|l| l.time);
    // Trailing blank stamps only mark the end; leading ones are intros.
    while lines.last().is_some_and(|l| l.text.is_empty()) {
        lines.pop();
    }
    lines
}

/// Index of the line being sung at `position` (`None` before the first).
pub fn current_line(lines: &[Line], position: Duration) -> Option<usize> {
    match lines.partition_point(|l| l.time <= position) {
        0 => None,
        n => Some(n - 1),
    }
}

// ── lrclib ───────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
}

fn from_record(r: &Record) -> Lyrics {
    if let Some(synced) = r.synced_lyrics.as_deref().filter(|s| !s.trim().is_empty()) {
        let lines = parse_lrc(synced);
        if !lines.is_empty() {
            return Lyrics::Synced(lines);
        }
    }
    if let Some(plain) = r.plain_lyrics.as_deref().filter(|s| !s.trim().is_empty()) {
        return Lyrics::Plain(plain.lines().map(|l| l.trim().to_string()).collect());
    }
    if r.instrumental { Lyrics::Instrumental } else { Lyrics::NotFound }
}

/// Picks the best search result: synced over plain, closest duration, the
/// right artist. `None` when nothing plausibly is the same recording.
fn best_match<'a>(records: &'a [Record], q: &TrackQuery) -> Option<&'a Record> {
    let norm = |s: &str| s.trim().to_lowercase();
    let (artist, title) = (norm(&q.artist), norm(&q.title));
    records
        .iter()
        .filter(|r| match (q.duration, r.duration) {
            (Some(want), Some(got)) => (want as f64 - got).abs() <= DURATION_SLACK,
            _ => true,
        })
        .filter(|r| artist.is_empty() || norm(&r.artist_name).contains(&artist) || artist.contains(&norm(&r.artist_name)))
        .filter(|r| r.synced_lyrics.is_some() || r.plain_lyrics.is_some() || r.instrumental)
        .max_by_key(|r| {
            let synced = r.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty()) as i64;
            let exact_title = (norm(&r.track_name) == title) as i64;
            let closeness = match (q.duration, r.duration) {
                (Some(want), Some(got)) => -((want as f64 - got).abs() * 10.0) as i64,
                _ => 0,
            };
            synced * 1000 + exact_title * 100 + closeness
        })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(HTTP_TIMEOUT)).http_status_as_error(false).build().into()
}

/// `Ok(None)` on 404.
fn get_json<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, url: &str, params: &[(&str, String)]) -> Result<Option<T>, String> {
    crate::offline::check()?;
    let mut req = agent.get(url).header("User-Agent", UA);
    for (k, v) in params {
        req = req.query(*k, v);
    }
    let mut response = req.call().map_err(|e| format!("lrclib: {e}"))?;
    match response.status().as_u16() {
        404 => Ok(None),
        200 => {
            let body = response.body_mut().read_to_string().map_err(|e| format!("lrclib: {e}"))?;
            serde_json::from_str(&body).map(Some).map_err(|e| format!("lrclib: unexpected response: {e}"))
        }
        code => Err(format!("lrclib: HTTP {code}")),
    }
}

/// Queries lrclib (no cache).
pub fn fetch_remote(q: &TrackQuery) -> Result<Lyrics, String> {
    let agent = agent();
    let mut params = vec![("artist_name", q.artist.trim().to_string()), ("track_name", q.title.trim().to_string())];
    if !q.album.trim().is_empty() {
        params.push(("album_name", q.album.trim().to_string()));
    }
    if let Some(d) = q.duration {
        params.push(("duration", d.to_string()));
        if let Some(record) = get_json::<Record>(&agent, &format!("{API}/get"), &params)? {
            let lyrics = from_record(&record);
            if lyrics.is_found() {
                return Ok(lyrics);
            }
        }
    }
    let mut search = vec![("track_name", q.title.trim().to_string())];
    if !q.artist.trim().is_empty() {
        search.push(("artist_name", q.artist.trim().to_string()));
    }
    let records = get_json::<Vec<Record>>(&agent, &format!("{API}/search"), &search)?.unwrap_or_default();
    Ok(best_match(&records, q).map(from_record).unwrap_or(Lyrics::NotFound))
}

// ── Cache ────────────────────────────────────────────────────────────────

pub fn cache_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("lyrics")
}

const PLAIN_TAG: &str = "[mandara:plain]";
const INSTRUMENTAL_TAG: &str = "[mandara:instrumental]";

fn format_stamp(t: Duration) -> String {
    let cs = t.as_millis() / 10;
    format!("[{:02}:{:02}.{:02}]", cs / 6000, (cs / 100) % 60, cs % 100)
}

/// Serialises found lyrics as an `.lrc` file (plain text gets a marker tag).
fn to_cache_text(lyrics: &Lyrics) -> Option<String> {
    match lyrics {
        Lyrics::Synced(lines) => Some(lines.iter().map(|l| format!("{}{}\n", format_stamp(l.time), l.text)).collect()),
        Lyrics::Plain(lines) => Some(format!("{PLAIN_TAG}\n{}\n", lines.join("\n"))),
        Lyrics::Instrumental => Some(format!("{INSTRUMENTAL_TAG}\n")),
        Lyrics::NotFound => None,
    }
}

fn from_cache_text(text: &str) -> Lyrics {
    if let Some(rest) = text.strip_prefix(PLAIN_TAG) {
        return Lyrics::Plain(rest.trim_start_matches('\n').lines().map(str::to_string).collect());
    }
    if text.starts_with(INSTRUMENTAL_TAG) {
        return Lyrics::Instrumental;
    }
    let lines = parse_lrc(text);
    if lines.is_empty() { Lyrics::NotFound } else { Lyrics::Synced(lines) }
}

/// A cached answer: `Some(NotFound)` while a recent miss is remembered.
pub fn cached_in(dir: &Path, q: &TrackQuery) -> Option<Lyrics> {
    let key = q.key();
    if let Ok(text) = std::fs::read_to_string(dir.join(format!("{key}.lrc"))) {
        return Some(from_cache_text(&text));
    }
    let miss = dir.join(format!("{key}.none"));
    let age = std::fs::metadata(&miss).ok()?.modified().ok().and_then(|t| SystemTime::now().duration_since(t).ok())?;
    (age < NOT_FOUND_TTL).then_some(Lyrics::NotFound)
}

pub fn store_in(dir: &Path, q: &TrackQuery, lyrics: &Lyrics) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let key = q.key();
    let (path, text) = match to_cache_text(lyrics) {
        Some(text) => (dir.join(format!("{key}.lrc")), text),
        None => (dir.join(format!("{key}.none")), String::new()),
    };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    if lyrics.is_found() {
        let _ = std::fs::remove_file(dir.join(format!("{key}.none")));
    }
    Ok(())
}

/// Cached lyrics, else lrclib (and the answer cached). Network errors are
/// not cached, so a later attempt retries.
pub fn fetch(q: &TrackQuery) -> Result<Lyrics, String> {
    if !q.is_searchable() {
        return Ok(Lyrics::NotFound);
    }
    let dir = cache_dir();
    if let Some(hit) = cached_in(&dir, q) {
        return Ok(hit);
    }
    let lyrics = fetch_remote(q)?;
    if let Err(err) = store_in(&dir, q, &lyrics) {
        tracing::debug!("lyrics cache: {err}");
    }
    Ok(lyrics)
}

/// Text direction of a line: Arabic-script (Hebrew too) reads right-to-left.
pub fn is_rtl(text: &str) -> bool {
    let mut rtl = 0usize;
    let mut ltr = 0usize;
    for c in text.chars() {
        match c as u32 {
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF => rtl += 1,
            _ if c.is_alphabetic() => ltr += 1,
            _ => {}
        }
    }
    rtl > 0 && rtl >= ltr
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}[ar:Fairuz]\n[ti:Shatti Ya Dini]\n[offset:+250]\n[00:05.00]\n[00:12.50]First line\n[00:17.20][01:02.00]Chorus <00:17.40>with <00:17.90>words\n[00:21:30]Colon centiseconds\n[01:10.123]Milliseconds\nnot a lyric line\n[02:00.00]\n";

    #[test]
    fn parses_lrc_stamps_offsets_and_repeats() {
        let lines = parse_lrc(SAMPLE);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["", "First line", "Chorus with words", "Colon centiseconds", "Chorus with words", "Milliseconds"]);
        // +250 ms offset shifts everything earlier.
        assert_eq!(lines[0].time, Duration::from_millis(4_750));
        assert_eq!(lines[1].time, Duration::from_millis(12_250));
        assert_eq!(lines[3].time, Duration::from_millis(21_050));
        assert_eq!(lines[4].time, Duration::from_millis(61_750));
        assert_eq!(lines[5].time, Duration::from_millis(69_873));
    }

    #[test]
    fn finds_the_current_line() {
        let lines = parse_lrc("[00:01.00]a\n[00:03.00]b\n[00:05.00]c\n");
        assert_eq!(current_line(&lines, Duration::from_millis(500)), None);
        assert_eq!(current_line(&lines, Duration::from_secs(1)), Some(0));
        assert_eq!(current_line(&lines, Duration::from_millis(4_999)), Some(1));
        assert_eq!(current_line(&lines, Duration::from_secs(60)), Some(2));
    }

    #[test]
    fn rejects_garbage_stamps() {
        assert!(parse_lrc("[length:03:20]\n[by:someone]\n[xx:yy]hello\n").is_empty());
        assert_eq!(parse_stamp("03:20.5"), Some(Duration::from_millis(200_500)));
        assert_eq!(parse_stamp("1:02:03.00"), Some(Duration::from_secs(3723)));
        assert!(parse_stamp("00:75.00").is_none());
    }

    #[test]
    fn cache_round_trips_and_remembers_misses() {
        let dir = std::env::temp_dir().join(format!("mandara-lyrics-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let q = TrackQuery { artist: "Fairuz".into(), title: "Shatti Ya Dini".into(), album: String::new(), duration: Some(200) };
        assert_eq!(cached_in(&dir, &q), None);
        store_in(&dir, &q, &Lyrics::NotFound).unwrap();
        assert_eq!(cached_in(&dir, &q), Some(Lyrics::NotFound));
        let synced = Lyrics::Synced(parse_lrc("[00:01.00]يا دارا\n[00:04.25]second\n"));
        store_in(&dir, &q, &synced).unwrap();
        assert_eq!(cached_in(&dir, &q), Some(synced));
        let other = TrackQuery { title: "Another".into(), ..q.clone() };
        store_in(&dir, &other, &Lyrics::Plain(vec!["one".into(), "".into(), "two".into()])).unwrap();
        assert_eq!(cached_in(&dir, &other), Some(Lyrics::Plain(vec!["one".into(), "".into(), "two".into()])));
        store_in(&dir, &other, &Lyrics::Instrumental).unwrap();
        assert_eq!(cached_in(&dir, &other), Some(Lyrics::Instrumental));
        assert_ne!(q.key(), other.key());
        assert_eq!(q.key(), TrackQuery { artist: " fairuz ".into(), ..q.clone() }.key(), "keys ignore case and padding");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn search_prefers_synced_and_close_durations() {
        let rec = |artist: &str, d: f64, synced: bool| Record {
            track_name: "Song".into(),
            artist_name: artist.into(),
            duration: Some(d),
            instrumental: false,
            plain_lyrics: Some("x".into()),
            synced_lyrics: synced.then(|| "[00:01.00]x".into()),
        };
        let q = TrackQuery { artist: "Band".into(), title: "Song".into(), album: String::new(), duration: Some(200) };
        let records = [rec("Band", 201.0, false), rec("Band", 199.0, true), rec("Band", 260.0, true), rec("Other", 200.0, true)];
        let best = best_match(&records, &q).unwrap();
        assert_eq!((best.duration, best.synced_lyrics.is_some()), (Some(199.0), true));
        assert!(best_match(&records[2..3], &q).is_none(), "another recording");
    }

    #[test]
    fn detects_rtl_lines() {
        assert!(is_rtl("أنا لحبيبي وحبيبي إلي"));
        assert!(!is_rtl("Hello, world"));
        assert!(!is_rtl("Oh baby يا"));
        assert!(!is_rtl("1 2 3"));
    }

    /// Live request against lrclib.net (`cargo test -p mandara-services -- --ignored lrclib`).
    #[test]
    #[ignore]
    fn lrclib_live_lookup() {
        let q = TrackQuery { artist: "Queen".into(), title: "Bohemian Rhapsody".into(), album: "A Night at the Opera".into(), duration: Some(355) };
        match fetch_remote(&q).expect("request") {
            Lyrics::Synced(lines) => {
                assert!(lines.len() > 20);
                assert!(lines.iter().any(|l| l.text.to_lowercase().contains("real life")));
            }
            other => panic!("expected synced lyrics, got {other:?}"),
        }
    }
}
