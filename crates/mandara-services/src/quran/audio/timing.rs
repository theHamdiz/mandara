//! Verified ayah ranges from MP3Quran's timing API.

use serde::{Deserialize, Serialize};

/// MP3Quran gives millisecond offsets into a complete chapter recording.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub start_time: u64,
    pub end_time: u64,
}

#[derive(Deserialize)]
struct Row {
    ayah: u16,
    #[serde(flatten)]
    interval: Interval,
}

/// Parses a surah's timing table: exactly `count` rows, ayahs 1..=count
/// in order, each interval non-empty and starting no earlier than the
/// previous one ends. Only the intervals are kept.
pub fn parse(bytes: &[u8], count: u16) -> Result<Box<[Interval]>, String> {
    let rows: Vec<Row> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if count == 0 || rows.len() != usize::from(count) {
        return Err("the timing does not cover this surah".into());
    }
    let mut end = 0;
    for (i, row) in rows.iter().enumerate() {
        let interval = row.interval;
        if usize::from(row.ayah) != i + 1 || interval.start_time < end || interval.end_time <= interval.start_time {
            return Err("the ayah timing is not consecutive".into());
        }
        end = interval.end_time;
    }
    Ok(rows.into_iter().map(|r| r.interval).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timings_require_complete_ordered_nonoverlapping_ayahs() {
        let good = br#"[{"ayah":1,"start_time":4400,"end_time":7780,"polygon":"ignored"},{"ayah":2,"start_time":7780,"end_time":12060}]"#;
        assert_eq!(parse(good, 2).unwrap().as_ref(), &[Interval { start_time: 4400, end_time: 7780 }, Interval { start_time: 7780, end_time: 12060 }]);
        assert!(parse(good, 3).is_err());
        for bad in [
            r#"[]"#,
            r#"[{"ayah":2,"start_time":0,"end_time":1}]"#,
            r#"[{"ayah":1,"start_time":5,"end_time":5}]"#,
            r#"[{"ayah":1,"start_time":-1,"end_time":5}]"#,
            r#"[{"ayah":1,"start_time":0,"end_time":6},{"ayah":2,"start_time":5,"end_time":7}]"#,
        ] {
            assert!(parse(bad.as_bytes(), 1).is_err(), "{bad}");
        }
    }

    #[test]
    fn official_hafs_timings_cover_short_and_long_chapters() {
        // Captured from MP3Quran read258; page polygons are not needed for audio.
        for (bytes, count, first, last) in [
            (include_bytes!("fixtures/hafs-258-001.json").as_slice(), 7, 4400, 40400),
            (include_bytes!("fixtures/hafs-258-002.json").as_slice(), 286, 4840, 7180160),
            (include_bytes!("fixtures/hafs-258-114.json").as_slice(), 6, 3840, 29040),
        ] {
            let rows = parse(bytes, count).unwrap();
            assert_eq!(rows[0].start_time, first);
            assert_eq!(rows.last().unwrap().end_time, last);
        }
    }
}
