//! AppImage (type 2) inspection without running it: the file is an ELF
//! runtime followed by a squashfs image; [`read`] opens the image at the
//! ELF's end with `backhand` and pulls out the desktop entry, the icon
//! (`.DirIcon` or the entry's `Icon=`) and the AppStream metadata.
//!
//! backhand's own xz / zstd codecs link C libraries, so [`PureCompressor`]
//! decodes those blocks with lzma-rs and ruzstd and hands gzip / lz4 (pure
//! Rust already) back to backhand.

use std::collections::HashMap;
use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};

use backhand::compression::{CompressionAction, Compressor, DefaultCompressor};
use backhand::kind::Kind;
use backhand::{BackhandError, FilesystemCompressor, FilesystemReader, InnerNode, SuperBlock};
use mandara_i18n::{t, tf};

use super::detect;
use super::meta::{AppStream, IconData, parse_appstream};

/// Largest desktop entry / AppStream file / icon read from an image.
const MAX_DESKTOP_BYTES: u64 = 256 << 10;
const MAX_APPSTREAM_BYTES: u64 = 2 << 20;
pub const MAX_ICON_BYTES: u64 = 8 << 20;
/// Symlink hops followed inside the image (`.DirIcon` → `app.png` → …).
const MAX_LINK_HOPS: usize = 8;
/// Bytes of the file read to find the ELF header.
const ELF_HEAD_BYTES: usize = 4096;
const METAINFO_DIRS: &[&str] = &["/usr/share/metainfo", "/usr/share/appdata"];
const ICON_EXTENSIONS: &[&str] = &["png", "svg", "svgz", "xpm"];

/// Decodes squashfs blocks in pure Rust.
pub struct PureCompressor;

pub static PURE: PureCompressor = PureCompressor;

impl CompressionAction for PureCompressor {
    type Error = BackhandError;
    type Compressor = Compressor;
    type FilesystemCompressor = FilesystemCompressor;
    type SuperBlock = SuperBlock;

    fn decompress(&self, bytes: &[u8], out: &mut Vec<u8>, compressor: Self::Compressor) -> Result<(), Self::Error> {
        match compressor {
            Compressor::Xz => {
                let mut input = bytes;
                lzma_rs::xz_decompress(&mut input, out).map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)
            }
            Compressor::Zstd => {
                let mut decoder = ruzstd::decoding::StreamingDecoder::new(bytes).map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)?;
                decoder.read_to_end(out).map(|_| ()).map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)
            }
            other => DefaultCompressor.decompress(bytes, out, other),
        }
    }

    fn compress(&self, bytes: &[u8], fc: Self::FilesystemCompressor, block_size: u32) -> Result<Vec<u8>, Self::Error> {
        DefaultCompressor.compress(bytes, fc, block_size)
    }

    fn compression_options(&self, superblock: &mut Self::SuperBlock, kind: &Kind, fs_compressor: Self::FilesystemCompressor) -> Result<Option<Vec<u8>>, Self::Error> {
        DefaultCompressor.compression_options(superblock, kind, fs_compressor)
    }
}

/// What an AppImage carries.
#[derive(Clone, Debug, Default)]
pub struct AppImageMeta {
    pub arch: Option<detect::Arch>,
    /// Where the squashfs image starts.
    pub offset: u64,
    /// The root `.desktop` file: name and text.
    pub desktop: Option<(String, String)>,
    pub icon: Option<IconData>,
    pub appstream: Option<AppStream>,
    /// Up to [`super::meta::MAX_SCREENSHOTS`] screenshots shipped inside
    /// the image (AppStream paths, else a `screenshots` folder), as bytes.
    pub screenshots: Vec<Vec<u8>>,
}

/// Largest embedded screenshot read.
const MAX_SCREENSHOT_BYTES: u64 = 8 << 20;
const SCREENSHOT_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Reads `path`'s metadata (nothing is executed or extracted to disk).
pub fn read(path: &Path) -> Result<AppImageMeta, String> {
    let head = detect::head(path).map_err(|e| e.to_string())?;
    let elf = detect::elf_info(&head[..head.len().min(ELF_HEAD_BYTES)]).ok_or_else(|| t("This AppImage's runtime isn't a valid program").to_string())?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let fs = FilesystemReader::from_reader_with_offset_and_kind(BufReader::new(file), elf.end, Kind::new_v4(&PURE))
        .map_err(|e| tf("Couldn't read the app inside this AppImage: {error}", &[("error", &e)]))?;
    let mut meta = AppImageMeta { arch: Some(elf.arch), offset: elf.end, ..Default::default() };
    let nodes: HashMap<PathBuf, &backhand::Node<backhand::SquashfsFileReader>> = fs.files().map(|n| (n.fullpath.clone(), n)).collect();
    let read_file = |p: &Path, limit: u64| -> Option<Vec<u8>> {
        let target = resolve(&nodes, p)?;
        let InnerNode::File(f) = &nodes.get(&target)?.inner else { return None };
        if f.file_len() as u64 > limit {
            return None;
        }
        let mut out = Vec::with_capacity(f.file_len());
        fs.file(f).reader().take(limit).read_to_end(&mut out).ok()?;
        Some(out)
    };
    // The desktop entry sits at the root (`/app.desktop`).
    let mut desktops: Vec<&PathBuf> = nodes.keys().filter(|p| p.parent() == Some(Path::new("/")) && p.extension().is_some_and(|e| e == "desktop")).collect();
    desktops.sort();
    if let Some(p) = desktops.first()
        && let Some(bytes) = read_file(p, MAX_DESKTOP_BYTES)
    {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        meta.desktop = Some((name, String::from_utf8_lossy(&bytes).into_owned()));
    }
    // AppStream: `usr/share/metainfo/<id>.(appdata|metainfo).xml`.
    let mut streams: Vec<&PathBuf> = nodes.keys().filter(|p| METAINFO_DIRS.iter().any(|d| p.parent() == Some(Path::new(d))) && p.to_string_lossy().ends_with(".xml")).collect();
    streams.sort();
    meta.appstream = streams.first().and_then(|p| read_file(p, MAX_APPSTREAM_BYTES)).and_then(|b| parse_appstream(&String::from_utf8_lossy(&b)));
    // Screenshots: AppStream image paths inside the image, else any file in
    // a `screenshots` folder (URLs are never fetched).
    let mut shots: Vec<PathBuf> = meta
        .appstream
        .as_ref()
        .map(|a| a.screenshots.iter().filter(|s| !s.contains("://")).map(|s| Path::new("/").join(s.trim_start_matches('/'))).collect())
        .unwrap_or_default();
    if shots.is_empty() {
        let mut found: Vec<PathBuf> = nodes
            .keys()
            .filter(|p| p.parent().and_then(|d| d.file_name()).is_some_and(|d| d == "screenshots") && p.extension().and_then(|e| e.to_str()).is_some_and(|e| SCREENSHOT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())))
            .cloned()
            .collect();
        found.sort();
        shots = found;
    }
    meta.screenshots = shots.iter().filter_map(|p| read_file(p, MAX_SCREENSHOT_BYTES)).take(super::meta::MAX_SCREENSHOTS).collect();
    // Icon: `.DirIcon`, else the entry's `Icon=` at the root.
    let icon_name = meta.desktop.as_ref().and_then(|(_, text)| super::meta::desktop_value(text, "Icon")).unwrap_or_default();
    let mut candidates = vec![PathBuf::from("/.DirIcon")];
    candidates.extend(ICON_EXTENSIONS.iter().map(|ext| PathBuf::from(format!("/{icon_name}.{ext}"))));
    for c in candidates {
        if let Some(bytes) = read_file(&c, MAX_ICON_BYTES)
            && let Some(icon) = IconData::from_bytes(bytes)
        {
            meta.icon = Some(icon);
            break;
        }
    }
    Ok(meta)
}

/// Follows symlinks inside the image (relative or image-absolute), never
/// leaving it; `None` for dangling links or loops.
fn resolve<T>(nodes: &HashMap<PathBuf, &backhand::Node<T>>, path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINK_HOPS {
        match &nodes.get(&current)?.inner {
            InnerNode::Symlink(link) => {
                let base = current.parent().unwrap_or(Path::new("/"));
                current = normalize(&if link.link.is_absolute() { link.link.clone() } else { base.join(&link.link) })?;
            }
            _ => return Some(current),
        }
    }
    None
}

/// Lexically normalizes an image path (`/a/../b` → `/b`); `None` above `/`.
fn normalize(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut out = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(n) => out.push(n),
        }
    }
    Some(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use backhand::{FilesystemWriter, NodeHeader};
    use std::io::Cursor;

    /// A tiny AppImage-format fixture: an ELF header (with the type-2
    /// marker, section table ending at `ELF_END`) followed by a
    /// gzip-compressed squashfs holding `files` and `links`. Nothing in it
    /// can run.
    pub fn fixture(files: &[(&str, &[u8])], links: &[(&str, &str)]) -> Vec<u8> {
        const ELF_END: u64 = 4096;
        let mut out = vec![0u8; ELF_END as usize];
        out[..4].copy_from_slice(b"\x7fELF");
        out[4] = 2; // 64-bit
        out[5] = 1; // little-endian
        out[8..11].copy_from_slice(b"AI\x02");
        out[18..20].copy_from_slice(&(if cfg!(target_arch = "aarch64") { 183u16 } else { 62u16 }).to_le_bytes());
        // Section header table: 64 entries of 64 bytes ending at ELF_END.
        let shnum: u16 = 4;
        let shoff = ELF_END - 64 * u64::from(shnum);
        out[40..48].copy_from_slice(&shoff.to_le_bytes());
        out[58..60].copy_from_slice(&64u16.to_le_bytes());
        out[60..62].copy_from_slice(&shnum.to_le_bytes());
        let mut w = FilesystemWriter::default();
        w.set_kind(Kind::new_v4(&PURE));
        w.set_compressor(FilesystemCompressor::new(Compressor::Gzip, None).unwrap());
        let header = NodeHeader::new(0o644, 0, 0, 0);
        let owned: Vec<(String, Vec<u8>)> = files.iter().map(|(p, b)| (p.to_string(), b.to_vec())).collect();
        for (path, bytes) in &owned {
            if let Some(parent) = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty() && *p != Path::new("/")) {
                w.push_dir_all(parent, NodeHeader::new(0o755, 0, 0, 0)).unwrap();
            }
            w.push_file(Cursor::new(bytes.clone()), path, header).unwrap();
        }
        for (path, target) in links {
            w.push_symlink(target, path, header).unwrap();
        }
        let mut image = Cursor::new(Vec::new());
        w.write(&mut image).unwrap();
        out.extend_from_slice(&image.into_inner());
        out
    }

    pub const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 64, 0, 0, 0, 64, 8, 6, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn reads_desktop_icon_and_appstream_from_the_payload() {
        let desktop = b"[Desktop Entry]\nType=Application\nName=Hello\nExec=hello %U\nIcon=hello\nX-AppImage-Version=1.2.3\n";
        let appstream = b"<component type=\"desktop-application\"><id>org.example.Hello</id><name>Hello</name><summary>Says hi</summary><releases><release version=\"1.2.3\" date=\"2026-01-01\"/></releases></component>";
        let bytes = fixture(
            &[("/hello.desktop", desktop), ("/hello.png", PNG_1X1), ("/usr/share/metainfo/org.example.Hello.appdata.xml", appstream), ("/AppRun", b"#!/bin/sh\n")],
            &[("/.DirIcon", "hello.png")],
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Hello.AppImage");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(detect::detect(&path), Ok(detect::Format::AppImage));
        let meta = read(&path).unwrap();
        assert_eq!(meta.arch, Some(detect::Arch::host()));
        let (name, text) = meta.desktop.unwrap();
        assert_eq!(name, "hello.desktop");
        assert!(text.contains("Name=Hello"));
        let icon = meta.icon.unwrap();
        assert_eq!(icon.size, Some(64));
        let stream = meta.appstream.unwrap();
        assert_eq!(stream.id, "org.example.Hello");
        assert_eq!(stream.version, "1.2.3");
        assert_eq!(stream.summary, "Says hi");
    }

    #[test]
    fn links_never_leave_the_image() {
        assert_eq!(normalize(Path::new("/a/../../etc/passwd")), None);
        assert_eq!(normalize(Path::new("/usr/./share/../bin")), Some(PathBuf::from("/usr/bin")));
    }
}
