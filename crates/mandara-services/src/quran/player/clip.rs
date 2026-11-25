//! Packet decoding and accurate chapter intervals.

use mandara_i18n::t;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

use super::super::audio::{Observer, Prepared};

/// Two maximum-length Layer III frames cover compressed-reference recovery
/// and the hybrid/polyphase history, including MPEG-2's shorter granules.
const MP3_PREROLL_FRAMES: u64 = 2 * 1152;

/// One ayah's file, decoded packet by packet.
pub(super) struct Clip {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    pub(super) rate: u32,
    time_base: TimeBase,
    path: std::path::PathBuf,
    position: u64,
    end: Option<u64>,
    buf: Option<SampleBuffer<f32>>,
    /// The last packet's frames as stereo, and how many are used.
    frames: Vec<[f32; 2]>,
    used: usize,
}

impl Clip {
    /// Opens the prepared file (gapless decoding for timed chapters) and
    /// positions it `skip` frames into its interval.
    pub(super) fn open(prepared: &Prepared, skip: u64, observer: &dyn Observer) -> Result<Self, String> {
        let path = &prepared.path;
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let options = FormatOptions { enable_gapless: prepared.interval.is_some(), ..FormatOptions::default() };
        let probed = symphonia::default::get_probe().format(&hint, mss, &options, &MetadataOptions::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        let format = probed.format;
        let track = format.default_track().ok_or_else(|| format!("{}: no audio", path.display()))?;
        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        let rate = track.codec_params.sample_rate.filter(|rate| *rate != 0).unwrap_or(44_100);
        let time_base = track.codec_params.time_base.unwrap_or_else(|| TimeBase::new(1, rate));
        let mut clip = Self { track: track.id, format, decoder, rate, time_base, path: path.clone(), position: 0, end: None, buf: None, frames: Vec::new(), used: 0 };
        clip.select(prepared, skip, observer)?;
        Ok(clip)
    }

    /// Whether `prepared` is another interval of this open chapter file, so
    /// [`Clip::select`] can reuse the decoder instead of reopening it.
    pub(super) fn same_file(&self, prepared: &Prepared) -> bool {
        self.path == prepared.path && prepared.interval.is_some()
    }

    /// Reuses the decoder and its unread packet for adjacent ayahs.
    pub(super) fn select(&mut self, prepared: &Prepared, skip: u64, observer: &dyn Observer) -> Result<(), String> {
        let (start, end) = bounds(prepared, self.rate, skip)?;
        self.end = end;
        if start == self.position {
            return Ok(());
        }
        if start > self.position && !self.frames.is_empty() && start - self.position <= MP3_PREROLL_FRAMES * 2 {
            return self.advance_to(start, observer);
        }
        observer.check()?;
        let seek = start.saturating_sub(MP3_PREROLL_FRAMES);
        self.format.seek(SeekMode::Accurate, SeekTo::Time { time: Time::new(seek / u64::from(self.rate), (seek % u64::from(self.rate)) as f64 / f64::from(self.rate)), track_id: Some(self.track) }).map_err(|e| e.to_string())?;
        self.decoder.reset();
        self.frames.clear();
        self.used = 0;
        // The demuxer restores compressed reference frames; the decoder
        // also needs a bounded preroll to restore its synthesis history.
        if !self.refill() {
            return Err(t("The recording ended before this ayah").into());
        }
        if self.position > start {
            return Err(t("The recitation timing could not be matched to the recording").into());
        }
        self.advance_to(start, observer)
    }

    /// Decodes forward, discarding frames, until the position is `start`.
    fn advance_to(&mut self, start: u64, observer: &dyn Observer) -> Result<(), String> {
        while self.position < start {
            observer.check()?;
            let take = (start - self.position).min(self.frames.len().saturating_sub(self.used) as u64);
            self.used += take as usize;
            self.position += take;
            if self.position < start && !self.refill() {
                return Err(t("The recording ended before this ayah").into());
            }
        }
        if self.position != start {
            return Err(t("The recitation timing could not be matched to the recording").into());
        }
        Ok(())
    }

    /// Decodes the next packet into `frames`; `false` at the end.
    fn refill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return false,
            };
            if packet.track_id() != self.track {
                continue;
            }
            let Some(position) = packet_frame(packet.ts(), self.time_base, self.rate) else {
                return false;
            };
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                // A damaged frame: skip it.
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => return false,
            };
            let spec = *decoded.spec();
            let channels = spec.channels.count().max(1);
            if self.buf.as_ref().is_none_or(|b| b.capacity() < decoded.capacity() * channels) {
                self.buf = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
            }
            let Some(buf) = self.buf.as_mut() else {
                return false;
            };
            buf.copy_interleaved_ref(decoded);
            self.frames.clear();
            self.frames.extend(buf.samples().chunks(channels).map(|f| match f {
                [m] => [*m, *m],
                [l, r, ..] => [*l, *r],
                [] => [0.0, 0.0],
            }));
            self.used = 0;
            self.position = position;
            if !self.frames.is_empty() {
                return true;
            }
        }
    }

    /// The next frame at the clip's own rate.
    pub(super) fn next(&mut self) -> Result<Option<[f32; 2]>, String> {
        if self.end.is_some_and(|end| self.position >= end) {
            return Ok(None);
        }
        if self.used >= self.frames.len() && !self.refill() {
            return if self.end.is_some_and(|end| self.position < end) { Err(t("The recording ended before this ayah").into()) } else { Ok(None) };
        }
        if self.end.is_some_and(|end| self.position >= end) {
            return Ok(None);
        }
        let f = self.frames[self.used];
        self.used += 1;
        self.position += 1;
        Ok(Some(f))
    }
}

/// A packet timestamp as a frame index at `rate` (`None` on overflow).
fn packet_frame(timestamp: u64, time_base: TimeBase, rate: u32) -> Option<u64> {
    let value = u128::from(timestamp).checked_mul(u128::from(time_base.numer))?.checked_mul(u128::from(rate))? / u128::from(time_base.denom);
    u64::try_from(value).ok()
}

/// The first frame (after `skip`, never past the end) and the end frame
/// of the prepared interval; a whole file has no end.
fn bounds(prepared: &Prepared, rate: u32, skip: u64) -> Result<(u64, Option<u64>), String> {
    let frames = |ms: u64| ms.checked_mul(u64::from(rate)).map(|n| n / 1000).ok_or_else(|| t("The recitation timing could not be matched to the recording").to_string());
    match prepared.interval {
        Some(i) => {
            let start = frames(i.start_time)?;
            let end = frames(i.end_time)?;
            if end <= start {
                return Err(t("The recitation timing could not be matched to the recording").into());
            }
            Ok((start.saturating_add(skip).min(end), Some(end)))
        }
        None => Ok((skip, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::audio::Interval;
    use super::*;

    struct Live;
    impl Observer for Live {
        fn current(&self) -> bool {
            true
        }
        fn progress(&self, _: u64, _: u64) {}
    }

    fn samples(clip: &mut Clip) -> Vec<[f32; 2]> {
        std::iter::from_fn(|| clip.next().transpose()).collect::<Result<Vec<_>, _>>().unwrap()
    }

    #[test]
    fn real_mp3_seek_and_adjacent_ayahs_keep_exact_sample_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chapter.mp3");
        std::fs::write(&path, include_bytes!("../audio/fixtures/seek-tone.mp3")).unwrap();
        let complete = Prepared::file(path);
        let mut whole = Clip::open(&complete, 0, &Live).unwrap();
        let reference = samples(&mut whole);
        assert!(reference.len() >= 44_100);
        let prepared = |start_time, end_time| Prepared { path: complete.path.clone(), interval: Some(Interval { start_time, end_time }) };
        let first = prepared(300, 500);
        let mut chapter = Clip::open(&first, 0, &Live).unwrap();
        let first_samples = samples(&mut chapter);
        assert_eq!(first_samples.len(), 8820);
        let second = prepared(500, 800);
        assert!(chapter.same_file(&second));
        let decoder = std::ptr::from_ref(&*chapter.decoder);
        chapter.select(&second, 0, &Live).unwrap();
        assert!(std::ptr::addr_eq(decoder, std::ptr::from_ref(&*chapter.decoder)), "adjacent ayahs keep the decoder");
        let second_samples = samples(&mut chapter);
        assert_eq!(second_samples.len(), 13230);
        for (actual, expected) in first_samples.iter().chain(&second_samples).zip(&reference[13230..35280]) {
            assert!((actual[0] - expected[0]).abs() < 0.0001 && (actual[1] - expected[1]).abs() < 0.0001, "actual {actual:?}; expected {expected:?}");
        }
        let resumed = samples(&mut Clip::open(&first, 100, &Live).unwrap());
        assert_eq!(resumed.len(), first_samples.len() - 100);
        for (actual, expected) in resumed.iter().zip(&first_samples[100..]) {
            assert!((actual[0] - expected[0]).abs() < 0.0001 && (actual[1] - expected[1]).abs() < 0.0001);
        }
        assert!(Clip::open(&prepared(3000, 4000), 0, &Live).is_err(), "missing audio never plays a different ayah");
    }

    #[test]
    fn seek_ranges_match_sequential_decode_across_rates_channels_and_encoder_delay() {
        for (name, bytes, rate) in [
            ("stereo44", include_bytes!("../audio/fixtures/seek-tone.mp3").as_slice(), 44_100),
            ("mono22gapless", include_bytes!("../audio/fixtures/seek-mono-gapless.mp3").as_slice(), 22_050),
            ("stereo48", include_bytes!("../audio/fixtures/seek-stereo-48k.mp3").as_slice(), 48_000),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("chapter.mp3");
            std::fs::write(&path, bytes).unwrap();
            let prepared = |start_time, end_time| Prepared { path: path.clone(), interval: Some(Interval { start_time, end_time }) };
            let mut complete = Clip::open(&prepared(0, 1000), 0, &Live).unwrap();
            assert_eq!(complete.rate, rate);
            let reference = samples(&mut complete);
            assert_eq!(reference.len(), rate as usize, "{name}: timed origin excludes encoder delay/padding");
            if name == "mono22gapless" {
                assert!(reference.iter().all(|f| f[0] == f[1]), "mono remains centered");
                let raw = samples(&mut Clip::open(&Prepared::file(path.clone()), 0, &Live).unwrap());
                assert!(raw.len() > reference.len(), "the tagged fixture actually carries encoder delay/padding");
            }
            let mut reused = Clip::open(&prepared(0, 20), 0, &Live).unwrap();
            let _ = samples(&mut reused);
            for start in (40..800).step_by(20) {
                let source = prepared(start, start + 10);
                let expected = &reference[(start * u64::from(rate) / 1000) as usize..((start + 10) * u64::from(rate) / 1000) as usize];
                let mut clip = Clip::open(&source, 0, &Live).unwrap();
                let actual = samples(&mut clip);
                assert_eq!(actual.len(), expected.len());
                assert!(actual.iter().zip(expected).all(|(a, b)| (a[0] - b[0]).abs() < 0.0001 && (a[1] - b[1]).abs() < 0.0001), "{name}: fresh seek at {start}ms");
                reused.select(&source, 0, &Live).unwrap();
                let actual = samples(&mut reused);
                assert_eq!(actual.len(), expected.len());
                assert!(actual.iter().zip(expected).all(|(a, b)| (a[0] - b[0]).abs() < 0.0001 && (a[1] - b[1]).abs() < 0.0001), "{name}: forward gap at {start}ms");
            }
            let backward = prepared(100, 200);
            reused.select(&backward, 0, &Live).unwrap();
            let actual = samples(&mut reused);
            assert!(actual.iter().zip(&reference[(u64::from(rate) / 10) as usize..(u64::from(rate) / 5) as usize]).all(|(a, b)| (a[0] - b[0]).abs() < 0.0001 && (a[1] - b[1]).abs() < 0.0001), "{name}: backward seek");
            reused.select(&prepared(700, 750), 0, &Live).unwrap();
            let actual = samples(&mut reused);
            let expected = &reference[(u64::from(rate) * 7 / 10) as usize..(u64::from(rate) * 3 / 4) as usize];
            assert_eq!(actual.len(), expected.len());
            assert!(actual.iter().zip(expected).all(|(a, b)| (a[0] - b[0]).abs() < 0.0001 && (a[1] - b[1]).abs() < 0.0001), "{name}: large forward jump");
        }
    }

    #[test]
    fn chapter_ranges_and_resume_offsets_are_exact_and_bounded() {
        let mut prepared = Prepared { path: "surah.mp3".into(), interval: Some(Interval { start_time: 4400, end_time: 7780 }) };
        assert_eq!(bounds(&prepared, 44_100, 0).unwrap(), (194_040, Some(343_098)));
        assert_eq!(bounds(&prepared, 44_100, 100).unwrap(), (194_140, Some(343_098)));
        assert_eq!(bounds(&prepared, 44_100, u64::MAX).unwrap(), (343_098, Some(343_098)));
        prepared.interval = None;
        assert_eq!(bounds(&prepared, 44_100, 100).unwrap(), (100, None));
        prepared.interval = Some(Interval { start_time: 0, end_time: u64::MAX });
        assert!(bounds(&prepared, 44_100, 0).is_err());
        assert_eq!(packet_frame(1152, TimeBase::new(1, 44_100), 44_100), Some(1152));
    }
}

#[cfg(test)]
mod provider_checks;
