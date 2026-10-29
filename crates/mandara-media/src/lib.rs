//! The pure-Rust video pipeline: WebM/Matroska and IVF demuxing, AV1
//! decoding with rav1d, YUV → BGRA / RGBA conversion (whole frames, or a
//! cropped region scaled straight into a caller's buffer) and threaded
//! players.
//!
//! No GUI types: the greeter wraps [`Player`] frames in GPUI images, the
//! desktop background converts [`FrameStream`] pictures directly into its
//! `wl_shm` buffers, and [`first_frame`] / [`representative_frame`] give
//! stills for thumbnails, crossfades and colour extraction.

pub mod av1;
pub mod ivf;
pub mod player;
pub mod webm;
pub mod yuv;

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use av1::{DecoderSettings, Picture};
pub use player::{FrameStream, Player, PlayerEvent, PlayerOptions, PlayerStats, StreamEvent, StreamFrame, StreamOptions, VideoFrame};
pub use yuv::{Crop, PixelOrder, Sampling, Target};

/// One compressed AV1 temporal unit.
#[derive(Clone, Debug)]
pub struct Packet {
    pub data: Vec<u8>,
    pub pts: Duration,
    pub key: bool,
}

/// What the container says about the clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipInfo {
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate (0 when the container doesn't say).
    pub fps: f32,
    pub duration: Option<Duration>,
    /// Sequence-header OBUs from the container (Matroska `av1C`).
    pub codec_config: Vec<u8>,
}

/// File extensions the player understands.
pub const VIDEO_EXTENSIONS: [&str; 3] = ["webm", "mkv", "ivf"];

/// Decoder threads for one-off stills (thumbnails, colour extraction):
/// a still needs one frame, so a big pool would only cost start-up time.
const STILL_THREADS: usize = 2;
/// Where in the clip [`representative_frame`] looks: the keyframe closest
/// to this fraction of its duration.
pub const REPRESENTATIVE_AT: f64 = 0.5;

/// Whether `path` names a video clip (by extension, case-insensitive).
pub fn is_video(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

pub enum Demuxer {
    Webm(webm::WebmDemuxer<BufReader<File>>),
    Ivf(ivf::IvfDemuxer<BufReader<File>>),
}

impl Demuxer {
    /// Opens a clip by sniffing its magic bytes.
    pub fn open(path: &Path) -> io::Result<Demuxer> {
        use std::io::Read as _;
        let mut magic = [0u8; 4];
        File::open(path)?.read_exact(&mut magic)?;
        match &magic {
            b"DKIF" => Ok(Demuxer::Ivf(ivf::IvfDemuxer::open(path)?)),
            [0x1A, 0x45, 0xDF, 0xA3] => Ok(Demuxer::Webm(webm::WebmDemuxer::open(path)?)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "not a WebM, MKV or IVF file")),
        }
    }

    pub fn info(&self) -> ClipInfo {
        match self {
            Demuxer::Webm(d) => d.info(),
            Demuxer::Ivf(d) => d.info(),
        }
    }

    pub fn next_packet(&mut self) -> io::Result<Option<Packet>> {
        match self {
            Demuxer::Webm(d) => d.next_packet(),
            Demuxer::Ivf(d) => d.next_packet(),
        }
    }

    pub fn rewind(&mut self) -> io::Result<()> {
        match self {
            Demuxer::Webm(d) => d.rewind(),
            Demuxer::Ivf(d) => d.rewind(),
        }
    }
}

/// Video clips in `dirs` (sorted by name, missing folders ignored).
pub fn list_clips(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_video(p))
            .collect();
        found.sort();
        out.extend(found);
    }
    out
}

/// Throughput of decoding (and optionally converting) a clip, for the
/// `decode_bench` example and the docs.
#[derive(Clone, Debug, Default)]
pub struct BenchResult {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub decode: Duration,
    pub convert: Duration,
}

impl BenchResult {
    pub fn decode_fps(&self) -> f64 {
        self.frames as f64 / self.decode.as_secs_f64().max(1e-9)
    }

    pub fn total_fps(&self) -> f64 {
        self.frames as f64 / (self.decode + self.convert).as_secs_f64().max(1e-9)
    }

    /// Throughput when decoding and conversion overlap (the player runs
    /// them on separate threads): bounded by the slower stage.
    pub fn pipelined_fps(&self) -> f64 {
        self.frames as f64 / self.decode.max(self.convert).as_secs_f64().max(1e-9)
    }
}

/// Decodes up to `max_frames` of a clip as fast as possible.
pub fn bench(path: &Path, settings: av1::DecoderSettings, half: bool, max_frames: u32) -> Result<BenchResult, String> {
    let mut demux = Demuxer::open(path).map_err(|e| e.to_string())?;
    let info = demux.info();
    let mut dec = av1::Av1Decoder::new(settings)?;
    let mut r = BenchResult::default();
    let convert = |pics: Vec<av1::Picture>, r: &mut BenchResult| {
        for p in pics {
            r.width = p.width() as u32;
            r.height = p.height() as u32;
            let t = Instant::now();
            let _ = std::hint::black_box(p.to_bgra(half));
            r.convert += t.elapsed();
            r.frames += 1;
        }
    };
    if !info.codec_config.is_empty() {
        let pics = dec.decode(&info.codec_config, Duration::ZERO)?;
        convert(pics, &mut r);
    }
    while r.frames < max_frames {
        let Some(p) = demux.next_packet().map_err(|e| e.to_string())? else { break };
        let t = Instant::now();
        let pics = dec.decode(&p.data, p.pts)?;
        r.decode += t.elapsed();
        convert(pics, &mut r);
    }
    let t = Instant::now();
    let pics = dec.drain()?;
    r.decode += t.elapsed();
    convert(pics, &mut r);
    Ok(r)
}

/// Decodes packets (after `skip` of them) until the first picture.
fn decode_first(path: &Path, skip: usize) -> Result<Picture, String> {
    let name = path.display().to_string();
    let mut demux = Demuxer::open(path).map_err(|e| format!("{name}: {e}"))?;
    let info = demux.info();
    let mut dec = av1::Av1Decoder::new(DecoderSettings { threads: STILL_THREADS, ..DecoderSettings::default() })?;
    if !info.codec_config.is_empty() {
        let _ = dec.decode(&info.codec_config, Duration::ZERO);
    }
    for _ in 0..skip {
        if demux.next_packet().map_err(|e| format!("{name}: {e}"))?.is_none() {
            break;
        }
    }
    while let Some(p) = demux.next_packet().map_err(|e| format!("{name}: {e}"))? {
        if let Some(pic) = dec.decode(&p.data, p.pts)?.into_iter().next() {
            return Ok(pic);
        }
    }
    dec.drain()?.into_iter().next().ok_or_else(|| format!("{name}: no decodable frames"))
}

/// The clip's first frame as an opaque RGBA image (thumbnails, and the
/// still a video wallpaper crossfades in with before it plays).
pub fn first_frame(path: &Path) -> Result<image::RgbaImage, String> {
    decode_first(path, 0)?.to_rgba_image().ok_or_else(|| format!("{}: empty frame", path.display()))
}

/// Index of the keyframe whose timestamp is closest to `at` × `duration`
/// (`keys` holds `(packet index, pts)` of every keyframe, in order).
pub fn keyframe_near(keys: &[(usize, Duration)], duration: Duration, at: f64) -> Option<usize> {
    let target = duration.mul_f64(at.clamp(0.0, 1.0));
    keys.iter().min_by_key(|(_, pts)| pts.abs_diff(target)).map(|(i, _)| *i)
}

/// A frame that represents the clip (colour extraction): the keyframe
/// nearest its middle ([`REPRESENTATIVE_AT`]), decoded on its own, so a
/// fade from black at the start doesn't decide the theme. Falls back to
/// the first frame when that keyframe can't be decoded alone.
pub fn representative_frame(path: &Path) -> Result<image::RgbaImage, String> {
    let name = path.display().to_string();
    let mut demux = Demuxer::open(path).map_err(|e| format!("{name}: {e}"))?;
    let mut keys = Vec::new();
    let mut last = Duration::ZERO;
    let mut index = 0usize;
    while let Some(p) = demux.next_packet().map_err(|e| format!("{name}: {e}"))? {
        if p.key {
            keys.push((index, p.pts));
        }
        last = last.max(p.pts);
        index += 1;
    }
    let skip = keyframe_near(&keys, last, REPRESENTATIVE_AT).unwrap_or(0);
    let picture = match decode_first(path, skip) {
        Ok(p) => p,
        Err(e) if skip > 0 => {
            tracing::debug!("{name}: keyframe {skip} alone: {e}; using the first frame");
            decode_first(path, 0)?
        }
        Err(e) => return Err(e),
    };
    picture.to_rgba_image().ok_or_else(|| format!("{name}: empty frame"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_video_extensions() {
        for p in ["a.webm", "B.MKV", "/x/c.Ivf"] {
            assert!(is_video(Path::new(p)), "{p}");
        }
        for p in ["a.jpg", "webm", "a.mp4", "a.webm.png"] {
            assert!(!is_video(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn picks_the_keyframe_nearest_the_middle() {
        let s = Duration::from_secs;
        let keys = [(0, s(0)), (96, s(4)), (192, s(8))];
        assert_eq!(keyframe_near(&keys, s(10), 0.5), Some(96));
        assert_eq!(keyframe_near(&keys, s(10), 0.9), Some(192));
        assert_eq!(keyframe_near(&keys, s(10), 0.0), Some(0));
        assert_eq!(keyframe_near(&[], s(10), 0.5), None);
    }
}
