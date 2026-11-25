//! IVF (the simple container `aomenc` / `rav1e` / `SvtAv1EncApp` write).
//!
//! 32-byte file header (`DKIF`, fourcc, size, time base, frame count), then
//! per frame a 12-byte header (u32 size, u64 pts in time-base units) and
//! the temporal unit. All little-endian.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use super::{ClipInfo, Packet};

pub struct IvfDemuxer<R> {
    r: R,
    width: u32,
    height: u32,
    /// Seconds per pts unit = num / den.
    num: u64,
    den: u64,
    frames: u32,
    header_len: u64,
}

impl IvfDemuxer<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::new(BufReader::with_capacity(1 << 20, File::open(path)?))
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

impl<R: Read + Seek> IvfDemuxer<R> {
    pub fn new(mut r: R) -> io::Result<Self> {
        let mut h = [0u8; 32];
        r.read_exact(&mut h)?;
        if &h[0..4] != b"DKIF" {
            return Err(invalid("not an IVF file"));
        }
        if &h[8..12] != b"AV01" {
            return Err(invalid("unsupported IVF codec (only AV1 is supported)"));
        }
        let u16_at = |i: usize| u16::from_le_bytes([h[i], h[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
        let header_len = (u16_at(6) as u64).max(32);
        let (den, num) = (u32_at(16) as u64, u32_at(20) as u64);
        let mut d = IvfDemuxer {
            r,
            width: u16_at(12) as u32,
            height: u16_at(14) as u32,
            num: num.max(1),
            den: den.max(1),
            frames: u32_at(24),
            header_len,
        };
        d.rewind()?;
        Ok(d)
    }

    pub fn info(&self) -> ClipInfo {
        let fps = self.den as f64 / self.num as f64;
        let duration = (self.frames > 0).then(|| Duration::from_secs_f64(self.frames as f64 * self.num as f64 / self.den as f64));
        // Time bases like 1/1000 (ms pts) aren't frame rates.
        let fps = if (1.0..=240.0).contains(&fps) { fps as f32 } else { 0.0 };
        ClipInfo { width: self.width, height: self.height, fps, duration: if fps > 0.0 { duration } else { None }, codec_config: Vec::new() }
    }

    pub fn rewind(&mut self) -> io::Result<()> {
        self.r.seek(SeekFrom::Start(self.header_len))?;
        Ok(())
    }

    pub fn next_packet(&mut self) -> io::Result<Option<Packet>> {
        let mut fh = [0u8; 12];
        match self.r.read_exact(&mut fh) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let size = u32::from_le_bytes([fh[0], fh[1], fh[2], fh[3]]) as usize;
        if size > 64 << 20 {
            return Err(invalid("IVF frame too large"));
        }
        let pts = u64::from_le_bytes(fh[4..12].try_into().unwrap_or([0; 8]));
        let mut data = vec![0u8; size];
        self.r.read_exact(&mut data)?;
        let nanos = (pts as u128 * self.num as u128 * 1_000_000_000 / self.den as u128) as u64;
        Ok(Some(Packet { data, pts: Duration::from_nanos(nanos), key: false }))
    }
}

/// Writes an IVF file header (tests and the test-clip example).
pub fn header(width: u16, height: u16, fps: u32, frames: u32) -> [u8; 32] {
    let mut h = [0u8; 32];
    h[0..4].copy_from_slice(b"DKIF");
    h[6..8].copy_from_slice(&32u16.to_le_bytes());
    h[8..12].copy_from_slice(b"AV01");
    h[12..14].copy_from_slice(&width.to_le_bytes());
    h[14..16].copy_from_slice(&height.to_le_bytes());
    h[16..20].copy_from_slice(&fps.to_le_bytes());
    h[20..24].copy_from_slice(&1u32.to_le_bytes());
    h[24..28].copy_from_slice(&frames.to_le_bytes());
    h
}

/// An IVF frame header.
pub fn frame_header(size: u32, pts: u64) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0..4].copy_from_slice(&size.to_le_bytes());
    h[4..12].copy_from_slice(&pts.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_frames() {
        let mut file = header(320, 180, 30, 2).to_vec();
        file.extend(frame_header(3, 0));
        file.extend([1, 2, 3]);
        file.extend(frame_header(2, 1));
        file.extend([4, 5]);
        let mut d = IvfDemuxer::new(std::io::Cursor::new(file)).unwrap();
        let info = d.info();
        assert_eq!((info.width, info.height), (320, 180));
        assert!((info.fps - 30.0).abs() < 1e-3);
        assert_eq!(info.duration, Some(Duration::from_secs_f64(2.0 / 30.0)));
        let a = d.next_packet().unwrap().unwrap();
        assert_eq!(a.data, vec![1, 2, 3]);
        let b = d.next_packet().unwrap().unwrap();
        assert_eq!(b.pts, Duration::from_nanos(33_333_333));
        assert!(d.next_packet().unwrap().is_none());
        d.rewind().unwrap();
        assert_eq!(d.next_packet().unwrap().unwrap().data, vec![1, 2, 3]);
    }

    #[test]
    fn rejects_other_codecs() {
        let mut h = header(16, 16, 30, 0);
        h[8..12].copy_from_slice(b"VP90");
        assert!(IvfDemuxer::new(std::io::Cursor::new(h.to_vec())).is_err());
    }
}
