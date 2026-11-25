//! A small streaming Matroska / WebM demuxer for one AV1 video track.
//!
//! The element tree is read *flattened*: the masters we care about
//! (Segment, Info, Tracks, TrackEntry, Video, Cluster, BlockGroup) are
//! entered without skipping their bodies, every other master is skipped
//! whole, and leaf elements are interpreted in document order. That handles
//! unknown-size Segments and Clusters (live / streamed files) for free,
//! because nothing depends on a master's end offset.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use super::{ClipInfo, Packet};

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const CONTENT_ENCODINGS: u32 = 0x6D80;
const CLUSTER: u32 = 0x1F43_B675;
const CLUSTER_TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;

/// Masters whose children we read in place.
const TRANSPARENT: [u32; 7] = [SEGMENT, INFO, TRACKS, TRACK_ENTRY, VIDEO, CLUSTER, BLOCK_GROUP];
/// Largest leaf we are willing to buffer (a 4K keyframe is a few MB).
const MAX_LEAF: u64 = 64 << 20;

#[derive(Clone, Debug, Default)]
struct Track {
    number: u64,
    kind: u64,
    codec: String,
    private: Vec<u8>,
    default_duration_ns: u64,
    width: u32,
    height: u32,
    encoded: bool,
}

/// Streaming reader over any `Read + Seek` source.
pub struct WebmDemuxer<R> {
    r: R,
    pos: u64,
    timecode_scale: u64,
    duration: Option<f64>,
    tracks: Vec<Track>,
    video: Option<Track>,
    cluster_tc: i64,
    first_cluster: Option<u64>,
    pending: std::collections::VecDeque<Packet>,
}

impl WebmDemuxer<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::new(BufReader::with_capacity(1 << 20, File::open(path)?))
    }
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn be_uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |acc, x| (acc << 8) | *x as u64)
}

fn be_float(b: &[u8]) -> Option<f64> {
    match b.len() {
        4 => Some(f32::from_be_bytes(b.try_into().ok()?) as f64),
        8 => Some(f64::from_be_bytes(b.try_into().ok()?)),
        _ => None,
    }
}

/// Reads a variable-length integer from a byte slice (marker removed).
/// Returns `(value, length)`.
pub fn vint_slice(b: &[u8]) -> Option<(u64, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    let mut v = (first as u64) & (0xFF >> len);
    for x in &b[1..len] {
        v = (v << 8) | *x as u64;
    }
    Some((v, len))
}

impl<R: Read + Seek> WebmDemuxer<R> {
    /// Parses the header up to the first cluster.
    pub fn new(r: R) -> io::Result<Self> {
        let mut d = WebmDemuxer {
            r,
            pos: 0,
            timecode_scale: 1_000_000,
            duration: None,
            tracks: Vec::new(),
            video: None,
            cluster_tc: 0,
            first_cluster: None,
            pending: Default::default(),
        };
        let (id, size) = d.header()?.ok_or_else(|| invalid("empty file"))?;
        if id != EBML {
            return Err(invalid("not a Matroska/WebM file"));
        }
        d.skip(size)?;
        // Read until the first cluster starts; tracks come before it.
        loop {
            let at = d.pos;
            let Some((id, size)) = d.header()? else { break };
            if id == CLUSTER {
                d.first_cluster = Some(at);
                break;
            }
            d.element(id, size)?;
        }
        let video = d
            .tracks
            .iter()
            .find(|t| t.kind == 1 && t.codec == "V_AV1")
            .cloned()
            .ok_or_else(|| {
                let codecs: Vec<&str> = d.tracks.iter().filter(|t| t.kind == 1).map(|t| t.codec.as_str()).collect();
                invalid(if codecs.is_empty() { "no video track".to_string() } else { format!("unsupported video codec {} (only AV1 is supported)", codecs.join(", ")) })
            })?;
        if video.encoded {
            return Err(invalid("compressed Matroska tracks are not supported"));
        }
        d.video = Some(video);
        d.rewind()?;
        Ok(d)
    }

    pub fn info(&self) -> ClipInfo {
        let v = self.video.clone().unwrap_or_default();
        let fps = if v.default_duration_ns > 0 { 1e9 / v.default_duration_ns as f64 } else { 0.0 };
        let duration = self.duration.filter(|d| *d > 0.0).map(|d| Duration::from_nanos((d * self.timecode_scale as f64) as u64));
        ClipInfo { width: v.width, height: v.height, fps: fps as f32, duration, codec_config: config_obus(&v.private) }
    }

    /// Back to the first cluster (for looping).
    pub fn rewind(&mut self) -> io::Result<()> {
        let at = self.first_cluster.unwrap_or(self.pos);
        self.r.seek(SeekFrom::Start(at))?;
        self.pos = at;
        self.cluster_tc = 0;
        self.pending.clear();
        Ok(())
    }

    /// The next video packet, or `None` at the end of the file.
    pub fn next_packet(&mut self) -> io::Result<Option<Packet>> {
        loop {
            if let Some(p) = self.pending.pop_front() {
                return Ok(Some(p));
            }
            let Some((id, size)) = self.header()? else { return Ok(None) };
            self.element(id, size)?;
        }
    }

    /// Reads an element header; `None` at a clean end of file.
    fn header(&mut self) -> io::Result<Option<(u32, Option<u64>)>> {
        let mut first = [0u8; 1];
        match self.r.read(&mut first)? {
            0 => return Ok(None),
            _ => self.pos += 1,
        }
        let id_len = first[0].leading_zeros() as usize + 1;
        if id_len > 4 {
            return Err(invalid(format!("bad element id at {}", self.pos - 1)));
        }
        let mut id = first[0] as u32;
        for _ in 1..id_len {
            id = (id << 8) | self.byte()? as u32;
        }
        let s0 = self.byte()?;
        let len = s0.leading_zeros() as usize + 1;
        if len > 8 {
            return Err(invalid("bad element size"));
        }
        let mut v = (s0 as u64) & (0xFF >> len);
        let mut all_ones = v == (0xFF >> len) as u64;
        for _ in 1..len {
            let b = self.byte()?;
            all_ones &= b == 0xFF;
            v = (v << 8) | b as u64;
        }
        Ok(Some((id, if all_ones { None } else { Some(v) })))
    }

    fn byte(&mut self) -> io::Result<u8> {
        let mut b = [0u8; 1];
        self.r.read_exact(&mut b)?;
        self.pos += 1;
        Ok(b[0])
    }

    fn skip(&mut self, size: Option<u64>) -> io::Result<()> {
        let size = size.ok_or_else(|| invalid("unknown-size element cannot be skipped"))?;
        self.r.seek(SeekFrom::Current(size as i64))?;
        self.pos += size;
        Ok(())
    }

    fn body(&mut self, size: Option<u64>) -> io::Result<Vec<u8>> {
        let size = size.ok_or_else(|| invalid("unknown-size leaf element"))?;
        if size > MAX_LEAF {
            return Err(invalid(format!("element of {size} bytes is too large")));
        }
        let mut buf = vec![0u8; size as usize];
        self.r.read_exact(&mut buf)?;
        self.pos += size;
        Ok(buf)
    }

    fn track(&mut self) -> Option<&mut Track> {
        self.tracks.last_mut()
    }

    /// Interprets one element whose header has just been read.
    fn element(&mut self, id: u32, size: Option<u64>) -> io::Result<()> {
        if TRANSPARENT.contains(&id) {
            if id == TRACK_ENTRY {
                self.tracks.push(Track::default());
            }
            return Ok(());
        }
        match id {
            TIMECODE_SCALE => self.timecode_scale = be_uint(&self.body(size)?).max(1),
            DURATION => self.duration = be_float(&self.body(size)?),
            TRACK_NUMBER => {
                let v = be_uint(&self.body(size)?);
                if let Some(t) = self.track() {
                    t.number = v;
                }
            }
            TRACK_TYPE => {
                let v = be_uint(&self.body(size)?);
                if let Some(t) = self.track() {
                    t.kind = v;
                }
            }
            CODEC_ID => {
                let v = String::from_utf8_lossy(&self.body(size)?).trim_end_matches('\0').to_string();
                if let Some(t) = self.track() {
                    t.codec = v;
                }
            }
            CODEC_PRIVATE => {
                let v = self.body(size)?;
                if let Some(t) = self.track() {
                    t.private = v;
                }
            }
            DEFAULT_DURATION => {
                let v = be_uint(&self.body(size)?);
                if let Some(t) = self.track() {
                    t.default_duration_ns = v;
                }
            }
            PIXEL_WIDTH => {
                let v = be_uint(&self.body(size)?) as u32;
                if let Some(t) = self.track() {
                    t.width = v;
                }
            }
            PIXEL_HEIGHT => {
                let v = be_uint(&self.body(size)?) as u32;
                if let Some(t) = self.track() {
                    t.height = v;
                }
            }
            CONTENT_ENCODINGS => {
                self.skip(size)?;
                if let Some(t) = self.track() {
                    t.encoded = true;
                }
            }
            CLUSTER_TIMECODE => self.cluster_tc = be_uint(&self.body(size)?) as i64,
            SIMPLE_BLOCK | BLOCK => {
                let body = self.body(size)?;
                self.block(&body, id == SIMPLE_BLOCK)?;
            }
            _ => self.skip(size)?,
        }
        Ok(())
    }

    /// Splits a (Simple)Block into packets for the video track.
    fn block(&mut self, b: &[u8], simple: bool) -> io::Result<()> {
        let Some(video) = self.video.as_ref() else { return Ok(()) };
        let (track, n) = vint_slice(b).ok_or_else(|| invalid("bad block"))?;
        if track != video.number || b.len() < n + 3 {
            return Ok(());
        }
        let rel = i16::from_be_bytes([b[n], b[n + 1]]) as i64;
        let flags = b[n + 2];
        let key = simple && flags & 0x80 != 0;
        let frames = lace(&b[n + 3..], (flags >> 1) & 0x03).ok_or_else(|| invalid("bad lacing"))?;
        let step = video.default_duration_ns;
        let pts_ns = (self.cluster_tc + rel).max(0) as u64 * self.timecode_scale;
        for (i, data) in frames.into_iter().enumerate() {
            self.pending.push_back(Packet { data, pts: Duration::from_nanos(pts_ns + i as u64 * step), key: key && i == 0 });
        }
        Ok(())
    }
}

/// Splits laced block data (0 = none, 1 = Xiph, 2 = fixed, 3 = EBML).
fn lace(data: &[u8], lacing: u8) -> Option<Vec<Vec<u8>>> {
    if lacing == 0 {
        return Some(vec![data.to_vec()]);
    }
    let count = *data.first()? as usize + 1;
    let mut at = 1;
    let mut sizes = Vec::with_capacity(count);
    match lacing {
        1 => {
            for _ in 0..count - 1 {
                let mut s = 0usize;
                loop {
                    let b = *data.get(at)?;
                    at += 1;
                    s += b as usize;
                    if b != 255 {
                        break;
                    }
                }
                sizes.push(s);
            }
        }
        2 => {
            let each = (data.len() - 1) / count;
            sizes = vec![each; count - 1];
        }
        _ => {
            let (first, n) = vint_slice(&data[at..])?;
            at += n;
            sizes.push(first as usize);
            let mut prev = first as i64;
            for _ in 1..count - 1 {
                let (raw, n) = vint_slice(&data[at..])?;
                at += n;
                let bias = (1i64 << (7 * n - 1)) - 1;
                prev += raw as i64 - bias;
                sizes.push(usize::try_from(prev).ok()?);
            }
        }
    }
    let mut out = Vec::with_capacity(count);
    for s in sizes {
        out.push(data.get(at..at + s)?.to_vec());
        at += s;
    }
    out.push(data.get(at..)?.to_vec());
    Some(out)
}

/// The `configOBUs` of an `av1C` box (the sequence header), if any.
pub fn config_obus(av1c: &[u8]) -> Vec<u8> {
    if av1c.len() > 4 && av1c[0] & 0x80 != 0 { av1c[4..].to_vec() } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vints() {
        assert_eq!(vint_slice(&[0x81]), Some((1, 1)));
        assert_eq!(vint_slice(&[0x40, 0x02]), Some((2, 2)));
        assert_eq!(vint_slice(&[0x10, 0x00, 0x00, 0x05]), Some((5, 4)));
        assert_eq!(vint_slice(&[]), None);
    }

    #[test]
    fn lacing_modes() {
        assert_eq!(lace(&[1, 2, 3], 0).unwrap(), vec![vec![1, 2, 3]]);
        // Fixed: 3 frames of 2 bytes.
        assert_eq!(lace(&[2, 1, 1, 2, 2, 3, 3], 2).unwrap(), vec![vec![1, 1], vec![2, 2], vec![3, 3]]);
        // Xiph: sizes 1 and 2, last takes the rest.
        assert_eq!(lace(&[2, 1, 2, 9, 8, 8, 7], 1).unwrap(), vec![vec![9], vec![8, 8], vec![7]]);
        // EBML: first size 1, second 1 + (0xBF - 63) = 1 + 0 = 1.
        assert_eq!(lace(&[2, 0x81, 0xBF, 5, 6, 7, 7], 3).unwrap(), vec![vec![5], vec![6], vec![7, 7]]);
    }

    #[test]
    fn rejects_non_matroska() {
        assert!(WebmDemuxer::new(std::io::Cursor::new(b"RIFF....WAVE".to_vec())).is_err());
    }
}
