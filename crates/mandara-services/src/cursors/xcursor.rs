//! A pure-Rust reader for XCursor files, the format every Linux cursor
//! theme ships in its `cursors/` folder: a 16-byte header, a table of
//! contents and one image chunk per nominal size and animation frame,
//! holding premultiplied ARGB pixels.
//!
//! Previews read only what they draw: the table of contents, then the one
//! chunk picked by size, so a multi-megabyte animated `wait` cursor costs a
//! seek and a single frame.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8; 4] = b"Xcur";
const FILE_HEADER_LEN: u32 = 16;
const FILE_VERSION: u32 = 0x0001_0000;
/// TOC / chunk type of an image (comments are `0xfffe0001`, skipped).
const IMAGE_TYPE: u32 = 0xfffd_0002;
const IMAGE_HEADER_LEN: u32 = 36;
const IMAGE_VERSION: u32 = 1;
/// The spec caps images at 0x7fff a side; previews never need more than
/// this (the largest sizes themes ship are 256).
const MAX_SIDE: u32 = 1024;
/// More TOC entries than any real theme has (sizes × frames).
const MAX_TOC: u32 = 4096;

/// One decoded cursor image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// The size the theme made this image for (24, 32, 48…).
    pub nominal: u32,
    pub width: u32,
    pub height: u32,
    pub xhot: u32,
    pub yhot: u32,
    /// Animation frame duration (0 for still cursors).
    pub delay_ms: u32,
    /// Straight-alpha RGBA, `width × height × 4` bytes.
    pub rgba: Vec<u8>,
}

impl Image {
    /// The pixels as an `image` buffer (for GPUI's `RenderImage`).
    pub fn to_rgba_image(&self) -> Option<image::RgbaImage> {
        image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
    }

    /// Average color of the visible pixels, weighted by alpha.
    pub fn average_rgb(&self) -> Option<[u8; 3]> {
        let (mut sum, mut weight) = ([0u64; 3], 0u64);
        for px in self.rgba.as_chunks::<4>().0 {
            let a = u64::from(px[3]);
            for (s, c) in sum.iter_mut().zip(px) {
                *s += u64::from(*c) * a;
            }
            weight += a;
        }
        (weight > 0).then(|| sum.map(|s| (s / weight) as u8))
    }
}

/// A table-of-contents entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TocEntry {
    pub kind: u32,
    /// The nominal size for images.
    pub subtype: u32,
    pub position: u32,
}

fn invalid(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("xcursor: {what}"))
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn read_u32s<R: Read>(r: &mut R, n: usize) -> std::io::Result<Vec<u32>> {
    let mut buf = vec![0u8; n * 4];
    r.read_exact(&mut buf)?;
    Ok(buf.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect())
}

/// Reads the header and table of contents.
pub fn read_toc<R: Read + Seek>(r: &mut R) -> std::io::Result<Vec<TocEntry>> {
    r.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; FILE_HEADER_LEN as usize];
    r.read_exact(&mut head)?;
    if &head[..4] != MAGIC {
        return Err(invalid("not an XCursor file"));
    }
    let header_len = u32_at(&head, 4);
    let ntoc = u32_at(&head, 12);
    if header_len < FILE_HEADER_LEN || ntoc > MAX_TOC {
        return Err(invalid("bad header"));
    }
    r.seek(SeekFrom::Start(u64::from(header_len)))?;
    let raw = read_u32s(r, ntoc as usize * 3)?;
    Ok(raw.as_chunks::<3>().0.iter().map(|[kind, subtype, position]| TocEntry { kind: *kind, subtype: *subtype, position: *position }).collect())
}

/// The nominal image sizes in a TOC, ascending.
pub fn sizes(toc: &[TocEntry]) -> Vec<u32> {
    let mut out: Vec<u32> = toc.iter().filter(|e| e.kind == IMAGE_TYPE).map(|e| e.subtype).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The size to decode for drawing at `wanted` device pixels: the smallest
/// one at least that big (scaling down stays sharp), else the largest.
pub fn pick_size(sizes: &[u32], wanted: u32) -> Option<u32> {
    sizes.iter().copied().find(|s| *s >= wanted).or_else(|| sizes.last().copied())
}

/// Frames the cursor has at `nominal` (1 for still cursors).
pub fn frame_count(toc: &[TocEntry], nominal: u32) -> usize {
    toc.iter().filter(|e| e.kind == IMAGE_TYPE && e.subtype == nominal).count()
}

/// Decodes the image chunk at `entry`.
fn read_image<R: Read + Seek>(r: &mut R, entry: TocEntry) -> std::io::Result<Image> {
    r.seek(SeekFrom::Start(u64::from(entry.position)))?;
    let h = read_u32s(r, (IMAGE_HEADER_LEN / 4) as usize)?;
    let [len, kind, nominal, version, width, height, xhot, yhot, delay] = h[..] else { return Err(invalid("short chunk header")) };
    if len != IMAGE_HEADER_LEN || kind != IMAGE_TYPE || version != IMAGE_VERSION || nominal != entry.subtype {
        return Err(invalid("bad image chunk"));
    }
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || xhot > width || yhot > height {
        return Err(invalid("bad image size"));
    }
    let argb = read_u32s(r, (width * height) as usize)?;
    let mut rgba = Vec::with_capacity(argb.len() * 4);
    for px in argb {
        let [a, red, green, blue] = px.to_be_bytes();
        // Premultiplied → straight alpha.
        let un = |c: u8| if a == 0 || a == u8::MAX { c } else { ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8 };
        rgba.extend_from_slice(&[un(red), un(green), un(blue), a]);
    }
    Ok(Image { nominal, width, height, xhot, yhot, delay_ms: delay, rgba })
}

/// Frame `frame` (wrapping) of the image best suited to `wanted` device
/// pixels.
pub fn read_frame<R: Read + Seek>(r: &mut R, wanted: u32, frame: usize) -> std::io::Result<Image> {
    let toc = read_toc(r)?;
    let nominal = pick_size(&sizes(&toc), wanted).ok_or_else(|| invalid("no images"))?;
    let frames: Vec<TocEntry> = toc.into_iter().filter(|e| e.kind == IMAGE_TYPE && e.subtype == nominal).collect();
    let entry = frames[frame % frames.len()];
    read_image(r, entry)
}

/// The first frame of `path` at the size best suited to `wanted`.
pub fn load(path: &Path, wanted: u32) -> std::io::Result<Image> {
    let mut f = std::io::BufReader::new(std::fs::File::open(path)?);
    read_frame(&mut f, wanted, 0)
}

/// An image to [`encode`]: `(nominal, width, height, xhot, yhot, delay,
/// premultiplied ARGB pixels)`.
pub type RawImage = (u32, u32, u32, u32, u32, u32, Vec<u32>);

/// Encodes images as an XCursor file: test fixtures and tools.
pub fn encode(images: &[RawImage]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    for v in [FILE_HEADER_LEN, FILE_VERSION, images.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut position = FILE_HEADER_LEN + images.len() as u32 * 12;
    for (nominal, w, h, ..) in images {
        for v in [IMAGE_TYPE, *nominal, position] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        position += IMAGE_HEADER_LEN + w * h * 4;
    }
    for (nominal, w, h, xhot, yhot, delay, argb) in images {
        for v in [IMAGE_HEADER_LEN, IMAGE_TYPE, *nominal, IMAGE_VERSION, *w, *h, *xhot, *yhot, *delay] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for px in argb {
            out.extend_from_slice(&px.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A 2×2 cursor at 24 (opaque red, half-transparent premultiplied
    /// green) and a two-frame 4×4 one at 48.
    fn fixture() -> Vec<u8> {
        let small = vec![0xffff_0000, 0x8000_8000, 0x0000_0000, 0xff00_00ff];
        let big = |c: u32| vec![c; 16];
        encode(&[(24, 2, 2, 1, 0, 0, small), (48, 4, 4, 3, 2, 50, big(0xff11_2233)), (48, 4, 4, 3, 2, 50, big(0xff44_5566))])
    }

    #[test]
    fn reads_toc_and_sizes() {
        let toc = read_toc(&mut Cursor::new(fixture())).unwrap();
        assert_eq!(toc.len(), 3);
        assert_eq!(sizes(&toc), vec![24, 48]);
        assert_eq!(frame_count(&toc, 48), 2);
        assert_eq!(frame_count(&toc, 24), 1);
    }

    #[test]
    fn picks_the_smallest_size_at_least_wanted() {
        assert_eq!(pick_size(&[24, 32, 48, 64], 30), Some(32));
        assert_eq!(pick_size(&[24, 32, 48, 64], 48), Some(48));
        assert_eq!(pick_size(&[24, 32], 96), Some(32), "the largest when none is big enough");
        assert_eq!(pick_size(&[], 24), None);
    }

    #[test]
    fn decodes_pixels_unpremultiplied() {
        let img = read_frame(&mut Cursor::new(fixture()), 16, 0).unwrap();
        assert_eq!((img.nominal, img.width, img.height, img.xhot, img.yhot), (24, 2, 2, 1, 0));
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
        // 0x80 green at 0x80 alpha → full green, half alpha.
        assert_eq!(&img.rgba[4..8], &[0, 255, 0, 128]);
        assert_eq!(&img.rgba[8..12], &[0, 0, 0, 0]);
        assert_eq!(img.to_rgba_image().unwrap().dimensions(), (2, 2));
    }

    #[test]
    fn reads_animation_frames() {
        let bytes = fixture();
        let f0 = read_frame(&mut Cursor::new(&bytes), 40, 0).unwrap();
        let f1 = read_frame(&mut Cursor::new(&bytes), 40, 1).unwrap();
        let wrapped = read_frame(&mut Cursor::new(&bytes), 40, 2).unwrap();
        assert_eq!((f0.nominal, f0.delay_ms), (48, 50));
        assert_eq!(&f0.rgba[..3], &[0x11, 0x22, 0x33]);
        assert_eq!(&f1.rgba[..3], &[0x44, 0x55, 0x66]);
        assert_eq!(wrapped, f0);
        assert_eq!(f1.average_rgb(), Some([0x44, 0x55, 0x66]));
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert!(read_toc(&mut Cursor::new(b"PNG\0garbage-garbage".to_vec())).is_err());
        let bytes = fixture();
        assert!(read_frame(&mut Cursor::new(&bytes[..bytes.len() - 8]), 48, 1).is_err(), "truncated pixels of the last frame");
        // A chunk claiming a huge image.
        let mut huge = encode(&[(24, 1, 1, 0, 0, 0, vec![0])]);
        let w_at = (FILE_HEADER_LEN + 12 + 16) as usize;
        huge[w_at..w_at + 4].copy_from_slice(&(MAX_SIDE + 1).to_le_bytes());
        assert!(read_frame(&mut Cursor::new(huge), 24, 0).is_err());
        // A TOC count past the cap.
        let mut many = encode(&[]);
        many[12..16].copy_from_slice(&(MAX_TOC + 1).to_le_bytes());
        assert!(read_toc(&mut Cursor::new(many)).is_err());
    }

    #[test]
    fn loads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("left_ptr");
        std::fs::write(&path, fixture()).unwrap();
        assert_eq!(load(&path, 24).unwrap().nominal, 24);
        assert!(load(&dir.path().join("missing"), 24).is_err());
    }
}
