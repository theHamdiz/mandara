//! Decoding a live stream from the jitter buffer with `symphonia`: MP3,
//! AAC (ADTS) and Ogg Vorbis, packet by packet, into stereo frames. The
//! sample buffer is made once and reused, so steady playback allocates
//! only what the demuxer needs per packet.

use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_AAC, CODEC_TYPE_MP3, CODEC_TYPE_OPUS, CODEC_TYPE_VORBIS, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::ring::{Ring, SharedReader};
use super::station::Codec;

/// Damaged packets skipped in a row before the stream counts as broken.
const MAX_BAD_PACKETS: u32 = 64;

/// The codec a response's content type names (`Unknown` when it doesn't).
pub fn codec_of(content_type: Option<&str>) -> Codec {
    match content_type {
        Some("audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" | "audio/x-mp3" | "audio/mpg") => Codec::Mp3,
        Some("audio/aac" | "audio/aacp" | "audio/x-aac" | "audio/x-aacp" | "audio/mp4a-latm" | "audio/adts") => Codec::Aac,
        Some("audio/ogg" | "application/ogg" | "audio/x-ogg" | "audio/vorbis" | "audio/x-vorbis") => Codec::Vorbis,
        Some("audio/opus" | "audio/ogg; codecs=opus") => Codec::Opus,
        _ => Codec::Unknown,
    }
}

/// The codec the first body bytes look like (for servers that send
/// `application/octet-stream` or nothing).
pub fn sniff(head: &[u8]) -> Codec {
    if head.starts_with(b"OggS") {
        // The first page's packet names its codec.
        return if head.windows(8).take(64).any(|w| w == b"OpusHead") { Codec::Opus } else { Codec::Vorbis };
    }
    if head.starts_with(b"ID3") {
        return Codec::Mp3;
    }
    // Frame syncs anywhere near the start (streams join mid-frame).
    for w in head.windows(2).take(4096) {
        if w[0] == 0xFF {
            match w[1] & 0xF6 {
                0xF0 => return Codec::Aac,
                _ if w[1] & 0xE0 == 0xE0 && w[1] & 0x06 != 0 => return Codec::Mp3,
                _ => {}
            }
        }
    }
    Codec::Unknown
}

/// Whether a content type can carry a stream Mandara might play.
pub fn audio_like(content_type: Option<&str>) -> bool {
    match content_type {
        None => true,
        Some(ct) => ct.starts_with("audio/") || ct == "application/ogg" || ct == "application/octet-stream" || ct == "binary/octet-stream" || ct == "video/mp2t",
    }
}

/// The open decoder of one connection's audio.
pub struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    pub rate: u32,
    pub channels: u8,
    pub codec: Codec,
    buf: Option<SampleBuffer<f32>>,
    bad: u32,
}

impl Source {
    /// Probes the buffered stream and opens its decoder.
    pub fn open(ring: &Arc<Ring>, codec: Codec) -> Result<Self, String> {
        let mss = MediaSourceStream::new(Box::new(ReadOnlySource::new(SharedReader::new(ring.clone(), codec == Codec::Aac))), MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        match codec {
            Codec::Mp3 => {
                hint.with_extension("mp3");
            }
            Codec::Aac => {
                hint.with_extension("aac");
            }
            Codec::Vorbis => {
                hint.with_extension("ogg");
            }
            Codec::Opus => return Err(mandara_i18n::t("Opus streams aren't supported yet").into()),
            Codec::Unknown => {}
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions { enable_gapless: false, ..FormatOptions::default() }, &MetadataOptions::default())
            .map_err(|e| match e {
                SymError::Unsupported(_) => mandara_i18n::t("This station uses a format Mandara can't play").to_string(),
                other => other.to_string(),
            })?;
        let format = probed.format;
        let track = format.default_track().ok_or_else(|| mandara_i18n::t("The stream has no audio").to_string())?;
        let params = &track.codec_params;
        let codec = match params.codec {
            CODEC_TYPE_MP3 => Codec::Mp3,
            CODEC_TYPE_AAC => Codec::Aac,
            CODEC_TYPE_VORBIS => Codec::Vorbis,
            _ => codec,
        };
        let decoder = symphonia::default::get_codecs().make(params, &DecoderOptions::default()).map_err(|_| {
            if params.codec == CODEC_TYPE_OPUS { mandara_i18n::t("Opus streams aren't supported yet") } else { mandara_i18n::t("This station uses a format Mandara can't play") }.to_string()
        })?;
        let rate = params.sample_rate.filter(|r| (8_000..=192_000).contains(r)).unwrap_or(44_100);
        let channels = params.channels.map_or(2, |c| c.count().clamp(1, 8) as u8);
        Ok(Self { track: track.id, format, decoder, rate, channels, codec, buf: None, bad: 0 })
    }

    /// Decodes the next packet, handing its frames (as stereo) to `frame`.
    /// `Ok(false)` at the end of the connection's audio.
    pub fn next(&mut self, frame: &mut impl FnMut([f32; 2])) -> Result<bool, String> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(_)) => return Ok(false),
                Err(SymError::ResetRequired) => return Ok(false),
                Err(e) => {
                    self.bad += 1;
                    if self.bad > MAX_BAD_PACKETS {
                        return Err(e.to_string());
                    }
                    continue;
                }
            };
            if packet.track_id() != self.track {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(_)) => {
                    self.bad += 1;
                    if self.bad > MAX_BAD_PACKETS {
                        return Err(mandara_i18n::t("The stream is damaged").into());
                    }
                    continue;
                }
                Err(SymError::IoError(_) | SymError::ResetRequired) => return Ok(false),
                Err(e) => return Err(e.to_string()),
            };
            self.bad = 0;
            let spec = *decoded.spec();
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            let need = decoded.capacity();
            let buf = match &mut self.buf {
                Some(b) if b.capacity() >= need * spec.channels.count() => b,
                slot => slot.insert(SampleBuffer::new(need as u64, spec)),
            };
            buf.copy_interleaved_ref(decoded);
            let ch = spec.channels.count().max(1);
            let samples = buf.samples();
            if spec.rate != self.rate && spec.rate != 0 {
                self.rate = spec.rate;
            }
            for f in samples.chunks_exact(ch) {
                frame(if ch == 1 { [f[0], f[0]] } else { [f[0], f[1]] });
            }
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_and_sniffing() {
        assert_eq!(codec_of(Some("audio/mpeg")), Codec::Mp3);
        assert_eq!(codec_of(Some("audio/aacp")), Codec::Aac);
        assert_eq!(codec_of(Some("application/ogg")), Codec::Vorbis);
        assert_eq!(codec_of(Some("audio/opus")), Codec::Opus);
        assert_eq!(codec_of(Some("text/html")), Codec::Unknown);
        assert_eq!(codec_of(None), Codec::Unknown);
        assert_eq!(sniff(b"ID3\x04\0\0"), Codec::Mp3);
        assert_eq!(sniff(&[0x12, 0xFF, 0xFB, 0x90, 0x00]), Codec::Mp3);
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80]), Codec::Aac);
        assert_eq!(sniff(&[0xFF, 0xF9, 0x50, 0x80]), Codec::Aac);
        assert_eq!(sniff(b"OggS\0\x02........................\x01vorbis"), Codec::Vorbis);
        assert_eq!(sniff(b"OggS\0\x02..........................OpusHead"), Codec::Opus);
        assert_eq!(sniff(b"<!DOCTYPE html>"), Codec::Unknown);
        assert!(audio_like(Some("audio/mpeg")) && audio_like(None) && audio_like(Some("application/octet-stream")));
        assert!(!audio_like(Some("text/html")) && !audio_like(Some("application/json")));
    }
}
