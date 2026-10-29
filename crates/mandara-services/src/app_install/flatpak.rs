//! Flatpak bundles (`.flatpak`) and references (`.flatpakref`): read
//! their app id without Flatpak, then install with
//! `flatpak install --user --noninteractive` (rootless; `--system` only
//! when the user prefers it — Flatpak asks polkit itself then).

use std::io::Read as _;
use std::path::Path;

use serde::Serialize;

use super::meta::{AppStream, IconData, parse_appstream};

/// Bytes of a bundle scanned for its `app/<id>/<arch>/<branch>` ref.
const BUNDLE_SCAN_BYTES: u64 = 1 << 20;
const REF_GROUP: &str = "[Flatpak Ref]";
/// Longest `.flatpakref` read.
const MAX_REF_BYTES: u64 = 256 << 10;

/// What a bundle or reference names.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FlatpakInfo {
    /// `org.gnome.Maps`.
    pub id: String,
    pub branch: String,
    pub arch: String,
    pub title: String,
    pub summary: String,
    /// The remote a reference installs from.
    pub url: String,
    pub runtime: bool,
    /// What the app may reach (bundles carry it; references don't).
    pub permissions: Option<Permissions>,
}

/// A Flatpak app's sandbox holes (its metadata's `[Context]` group).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Permissions {
    /// `network`, `ipc`.
    pub shared: Vec<String>,
    /// `x11`, `wayland`, `pulseaudio`, `session-bus`, `ssh-auth`, …
    pub sockets: Vec<String>,
    /// `dri`, `all`, `kvm`, `input`, …
    pub devices: Vec<String>,
    /// `home`, `host`, `xdg-download`, `~/Games:ro`, …
    pub filesystems: Vec<String>,
}

impl Permissions {
    pub fn is_empty(&self) -> bool {
        self.shared.is_empty() && self.sockets.is_empty() && self.devices.is_empty() && self.filesystems.is_empty()
    }
}

/// What a bundle carries besides its ref (all optional).
#[derive(Clone, Debug, Default)]
pub struct BundleExtras {
    pub appstream: Option<AppStream>,
    /// The largest embedded PNG icon.
    pub icon: Option<IconData>,
}

const CONTEXT_GROUP: &[u8] = b"[Context]";
/// Gzip streams tried for the bundle's AppStream data.
const MAX_GZIP_TRIES: usize = 16;
/// Longest AppStream text inflated from a bundle.
const MAX_APPDATA_BYTES: u64 = 1 << 20;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_IEND: &[u8] = b"IEND";
/// The chunk-length / type / CRC framing around `IEND`.
const PNG_CHUNK_TAIL: usize = 8;

/// Parses a metadata key file's `[Context]` group.
pub fn parse_context(text: &str) -> Option<Permissions> {
    let mut on = false;
    let mut p = Permissions::default();
    let mut seen = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            on = l == "[Context]";
            seen |= on;
            continue;
        }
        let Some((k, v)) = l.split_once('=').filter(|_| on) else { continue };
        // `!x` revokes: not a permission.
        let list = || v.split(';').map(str::trim).filter(|x| !x.is_empty() && !x.starts_with('!')).map(String::from).collect::<Vec<_>>();
        match k.trim() {
            "shared" => p.shared = list(),
            "sockets" => p.sockets = list(),
            "devices" => p.devices = list(),
            "filesystems" => p.filesystems = list(),
            _ => {}
        }
    }
    seen.then_some(p)
}

/// The metadata key file's `[Context]` in a bundle's bytes.
fn bundle_context(bytes: &[u8]) -> Option<Permissions> {
    let at = bytes.windows(CONTEXT_GROUP.len()).position(|w| w == CONTEXT_GROUP)?;
    // The key file is a NUL-terminated string; its start is the previous NUL.
    let start = bytes[..at].iter().rposition(|b| *b == 0).map_or(0, |s| s + 1);
    let end = bytes[at..].iter().position(|b| *b == 0).map_or(bytes.len(), |e| at + e);
    parse_context(&String::from_utf8_lossy(&bytes[start..end]))
}

/// The bundle's gzipped AppStream data, parsed.
fn bundle_appstream(bytes: &[u8]) -> Option<AppStream> {
    let mut from = 0;
    for _ in 0..MAX_GZIP_TRIES {
        let off = bytes[from..].windows(3).position(|w| w == [0x1f, 0x8b, 0x08])?;
        let at = from + off;
        let mut text = String::new();
        let inflated = flate2::read::GzDecoder::new(&bytes[at..]).take(MAX_APPDATA_BYTES).read_to_string(&mut text).is_ok();
        if inflated && let Some(a) = parse_appstream(&text) {
            return Some(a);
        }
        from = at + 1;
    }
    None
}

/// The largest whole PNG in a bundle's bytes.
fn bundle_icon(bytes: &[u8]) -> Option<IconData> {
    let mut best: Option<IconData> = None;
    let mut from = 0;
    while let Some(off) = bytes[from..].windows(PNG_MAGIC.len()).position(|w| w == PNG_MAGIC) {
        let start = from + off;
        let Some(iend) = bytes[start..].windows(PNG_IEND.len()).position(|w| w == PNG_IEND) else { break };
        let end = (start + iend + PNG_CHUNK_TAIL).min(bytes.len());
        if let Some(icon) = IconData::from_bytes(bytes[start..end].to_vec())
            && icon.size.unwrap_or(0) > best.as_ref().and_then(|b| b.size).unwrap_or(0)
        {
            best = Some(icon);
        }
        from = end;
    }
    best
}

/// Parses a `.flatpakref` (`[Flatpak Ref]` key file).
pub fn parse_ref(text: &str) -> Option<FlatpakInfo> {
    let mut in_group = false;
    let mut info = FlatpakInfo::default();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_group = l == REF_GROUP;
            continue;
        }
        let Some((k, v)) = l.split_once('=').filter(|_| in_group) else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "Name" => info.id = v,
            "Branch" => info.branch = v,
            "Title" => info.title = v,
            "Comment" | "Description" => info.summary = v,
            "Url" => info.url = v,
            "IsRuntime" => info.runtime = v == "true",
            _ => {}
        }
    }
    valid_id(&info.id).then_some(info)
}

/// A reverse-DNS Flatpak id (at least two dots' worth of segments).
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

/// Reads a reference file.
pub fn read_ref(path: &Path) -> Option<FlatpakInfo> {
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(MAX_REF_BYTES).read_to_string(&mut text).ok()?;
    parse_ref(&text)
}

/// Finds `app/<id>/<arch>/<branch>` in a bundle's metadata.
pub fn read_bundle(path: &Path) -> Option<FlatpakInfo> {
    read_bundle_full(path).map(|(info, _)| info)
}

/// [`read_bundle`] plus its permissions, AppStream data and icon.
pub fn read_bundle_full(path: &Path) -> Option<(FlatpakInfo, BundleExtras)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(BUNDLE_SCAN_BYTES).read_to_end(&mut bytes).ok()?;
    bundle_details(&bytes)
}

/// Everything Mandara reads from a bundle's leading bytes.
pub fn bundle_details(bytes: &[u8]) -> Option<(FlatpakInfo, BundleExtras)> {
    let mut info = find_bundle_ref(bytes)?;
    info.permissions = bundle_context(bytes);
    let extras = BundleExtras { appstream: bundle_appstream(bytes), icon: bundle_icon(bytes) };
    if let Some(a) = &extras.appstream {
        if info.title.is_empty() {
            info.title = a.name.clone();
        }
        if info.summary.is_empty() {
            info.summary = a.summary.clone();
        }
    }
    Some((info, extras))
}

pub fn find_bundle_ref(bytes: &[u8]) -> Option<FlatpakInfo> {
    const PREFIX: &[u8] = b"app/";
    let mut at = 0;
    while let Some(off) = bytes[at..].windows(PREFIX.len()).position(|w| w == PREFIX) {
        let start = at + off + PREFIX.len();
        let end = bytes[start..].iter().position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))).map_or(bytes.len(), |e| start + e);
        let text = String::from_utf8_lossy(&bytes[start..end]);
        let parts: Vec<&str> = text.split('/').collect();
        if let [id, arch, branch] = parts.as_slice()
            && valid_id(id)
            && !arch.is_empty()
            && !branch.is_empty()
        {
            return Some(FlatpakInfo { id: id.to_string(), arch: arch.to_string(), branch: branch.to_string(), ..Default::default() });
        }
        at = start;
    }
    None
}

/// `flatpak install …` for a bundle (`--bundle`) or reference (`--from`).
pub fn install_argv(file: &Path, bundle: bool, user: bool) -> Vec<String> {
    let scope = if user { "--user" } else { "--system" };
    let kind = if bundle { "--bundle" } else { "--from" };
    ["flatpak", "install", scope, "--noninteractive", "-y", kind, &file.to_string_lossy()].iter().map(|s| s.to_string()).collect()
}

/// `flatpak info` for duplicate detection.
pub fn info_argv(id: &str, user: bool) -> Vec<String> {
    ["flatpak", "info", if user { "--user" } else { "--system" }, id].iter().map(|s| s.to_string()).collect()
}

/// The PATH shim for a Flatpak.
pub fn shim_script(id: &str) -> String {
    format!("#!/bin/sh\nexec flatpak run {id} \"$@\"\n")
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn reads_refs_and_bundles() {
        let r = parse_ref("[Flatpak Ref]\nName=org.gnome.Maps\nBranch=stable\nTitle=Maps\nUrl=https://dl.flathub.org/repo/\nIsRuntime=false\n").unwrap();
        assert_eq!((r.id.as_str(), r.branch.as_str(), r.title.as_str()), ("org.gnome.Maps", "stable", "Maps"));
        assert!(parse_ref("[Flatpak Ref]\nName=../../evil\n").is_none());
        let mut bundle = b"\0\0GVariant junk xa.metadata\0flatpak\0".to_vec();
        bundle.extend_from_slice(b"app/org.example.Hello/x86_64/stable\0more");
        let b = find_bundle_ref(&bundle).unwrap();
        assert_eq!((b.id.as_str(), b.arch.as_str(), b.branch.as_str()), ("org.example.Hello", "x86_64", "stable"));
        assert_eq!(install_argv(Path::new("/d/a.flatpakref"), false, true).join(" "), "flatpak install --user --noninteractive -y --from /d/a.flatpakref");
        assert_eq!(shim_script("org.x.Y"), "#!/bin/sh\nexec flatpak run org.x.Y \"$@\"\n");
    }

    /// A real (decodable) PNG of `w`×`h`.
    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(w, h, image::Rgba([40, 160, 170, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// A bundle-like blob: ref, metadata key file, gzipped AppStream, icon.
    pub fn bundle_fixture() -> Vec<u8> {
        use std::io::Write as _;
        let mut b = b"\0\0GVariant xa.metadata\0".to_vec();
        b.extend_from_slice(b"[Application]\nname=org.example.Hello\nruntime=org.gnome.Platform/x86_64/46\n\n[Context]\nshared=network;ipc;\nsockets=wayland;pulseaudio;!x11;\ndevices=dri;\nfilesystems=xdg-download;~/Music:ro;\n\0");
        let xml = "<components><component><id>org.example.Hello</id><name>Hello</name><summary>Says hi</summary><developer><name>Example</name></developer><description><p>A friendly app.</p></description><categories><category>Utility</category></categories></component></components>";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(xml.as_bytes()).unwrap();
        b.extend_from_slice(b"appdata\0");
        b.extend_from_slice(&gz.finish().unwrap());
        b.extend_from_slice(b"\0icon-64\0");
        b.extend_from_slice(&png(64, 64));
        b.extend_from_slice(b"\0app/org.example.Hello/x86_64/stable\0");
        b
    }

    #[test]
    fn bundles_carry_permissions_appstream_and_icon() {
        let (info, extras) = bundle_details(&bundle_fixture()).unwrap();
        assert_eq!(info.id, "org.example.Hello");
        let p = info.permissions.unwrap();
        assert_eq!(p.shared, vec!["network", "ipc"]);
        assert_eq!(p.sockets, vec!["wayland", "pulseaudio"], "revoked sockets are left out");
        assert_eq!((p.devices.as_slice(), p.filesystems.as_slice()), (&["dri".to_string()][..], &["xdg-download".to_string(), "~/Music:ro".to_string()][..]));
        let a = extras.appstream.unwrap();
        assert_eq!((a.developer.as_str(), a.description.as_str(), info.title.as_str()), ("Example", "A friendly app.", "Hello"));
        assert!(extras.icon.is_some());
        assert!(parse_context("[Application]\nname=x\n").is_none());
    }
}
