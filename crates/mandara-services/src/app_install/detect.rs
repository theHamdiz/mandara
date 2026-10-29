//! Format detection by magic bytes (the extension only breaks ties between
//! formats that share a container, e.g. a `.pkg.tar.zst` and a portable
//! `.tar.zst`), and ELF header reading (architecture, where an AppImage's
//! runtime ends and its squashfs payload starts).

use std::io::Read as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Bytes read for a sniff: enough for the ELF header, the ar header of a
/// `.deb` and the start of a `.flatpakref`.
pub const SNIFF_BYTES: usize = 4096;

const ELF_MAGIC: &[u8] = b"\x7fELF";
/// AppImage marker in the ELF identification padding (`e_ident[8..11]`).
const APPIMAGE_TYPE2: &[u8] = b"AI\x02";
const APPIMAGE_TYPE1: &[u8] = b"AI\x01";
const APPIMAGE_MARKER_AT: usize = 8;
const SQUASHFS_MAGIC: &[u8] = b"hsqs";
const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
const XZ_MAGIC: &[u8] = b"\xfd7zXZ\x00";
const ZSTD_MAGIC: &[u8] = b"\x28\xb5\x2f\xfd";
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const AR_MAGIC: &[u8] = b"!<arch>\n";
const DEB_FIRST_MEMBER: &[u8] = b"debian-binary";
const RPM_MAGIC: &[u8] = b"\xed\xab\xee\xdb";
/// Alpine's v3 package format.
const APK_V3_MAGIC: &[u8] = b"ADB";
const FLATPAKREF_GROUP: &str = "[Flatpak Ref]";
/// Flatpak bundles are OSTree static deltas (GVariant); their metadata
/// names the ref as `app/<id>/<arch>/<branch>` near the start.
const FLATPAK_BUNDLE_MARKER: &[u8] = b"flatpak";

/// A file Mandara can install.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    AppImage,
    FlatpakBundle,
    FlatpakRef,
    TarGz,
    TarXz,
    TarZst,
    Zip,
    Deb,
    Rpm,
    Pacman,
    Apk,
    Xbps,
}

/// How a format installs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    AppImage,
    Flatpak,
    Portable,
    Native,
}

impl Family {
    /// Installed into `$HOME` by Mandara itself (no package manager).
    pub fn is_rootless_files(self) -> bool {
        matches!(self, Family::AppImage | Family::Portable)
    }

    /// Badge text (proper names stay untranslated).
    pub fn label(self) -> &'static str {
        match self {
            Family::AppImage => "AppImage",
            Family::Flatpak => "Flatpak",
            Family::Portable => mandara_i18n::t("Portable"),
            Family::Native => mandara_i18n::t("Native"),
        }
    }
}

impl Format {
    pub const ALL: [Format; 12] = [
        Format::AppImage,
        Format::FlatpakBundle,
        Format::FlatpakRef,
        Format::TarGz,
        Format::TarXz,
        Format::TarZst,
        Format::Zip,
        Format::Deb,
        Format::Rpm,
        Format::Pacman,
        Format::Apk,
        Format::Xbps,
    ];

    pub fn family(self) -> Family {
        match self {
            Format::AppImage => Family::AppImage,
            Format::FlatpakBundle | Format::FlatpakRef => Family::Flatpak,
            Format::TarGz | Format::TarXz | Format::TarZst | Format::Zip => Family::Portable,
            Format::Deb | Format::Rpm | Format::Pacman | Format::Apk | Format::Xbps => Family::Native,
        }
    }

    /// Short technical name (`.deb`, `.tar.zst`).
    pub fn extension(self) -> &'static str {
        match self {
            Format::AppImage => ".AppImage",
            Format::FlatpakBundle => ".flatpak",
            Format::FlatpakRef => ".flatpakref",
            Format::TarGz => ".tar.gz",
            Format::TarXz => ".tar.xz",
            Format::TarZst => ".tar.zst",
            Format::Zip => ".zip",
            Format::Deb => ".deb",
            Format::Rpm => ".rpm",
            Format::Pacman => ".pkg.tar.zst",
            Format::Apk => ".apk",
            Format::Xbps => ".xbps",
        }
    }

    /// Glob patterns for file pickers.
    pub fn globs(self) -> &'static [&'static str] {
        match self {
            Format::AppImage => &["*.AppImage", "*.appimage"],
            Format::FlatpakBundle => &["*.flatpak"],
            Format::FlatpakRef => &["*.flatpakref"],
            Format::TarGz => &["*.tar.gz", "*.tgz"],
            Format::TarXz => &["*.tar.xz", "*.txz"],
            Format::TarZst => &["*.tar.zst", "*.tzst"],
            Format::Zip => &["*.zip"],
            Format::Deb => &["*.deb"],
            Format::Rpm => &["*.rpm"],
            Format::Pacman => &["*.pkg.tar.zst", "*.pkg.tar.xz"],
            Format::Apk => &["*.apk"],
            Format::Xbps => &["*.xbps"],
        }
    }
}

/// CPU architectures an executable may target.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86_64,
    Aarch64,
    I686,
    Armv7,
    Other,
}

const EM_386: u16 = 3;
const EM_ARM: u16 = 40;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;

impl Arch {
    pub fn from_machine(e_machine: u16) -> Self {
        match e_machine {
            EM_X86_64 => Arch::X86_64,
            EM_AARCH64 => Arch::Aarch64,
            EM_386 => Arch::I686,
            EM_ARM => Arch::Armv7,
            _ => Arch::Other,
        }
    }

    /// The machine Mandara runs on.
    pub fn host() -> Self {
        match std::env::consts::ARCH {
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Aarch64,
            "x86" => Arch::I686,
            "arm" => Arch::Armv7,
            _ => Arch::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::I686 => "i686",
            Arch::Armv7 => "armv7",
            Arch::Other => mandara_i18n::t("another architecture"),
        }
    }

    /// Package-name spellings (`amd64`, `arm64`, `noarch`…): `None` for
    /// architecture-independent packages.
    pub fn from_package_word(word: &str) -> Option<Self> {
        Some(match word {
            "x86_64" | "amd64" | "x86-64" => Arch::X86_64,
            "aarch64" | "arm64" => Arch::Aarch64,
            "i386" | "i486" | "i586" | "i686" | "x86" => Arch::I686,
            "armv7" | "armv7h" | "armhf" | "armv7hl" | "armv7l" => Arch::Armv7,
            "any" | "all" | "noarch" => return None,
            _ => Arch::Other,
        })
    }
}

/// What an ELF header says.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ElfInfo {
    pub arch: Arch,
    /// Bytes up to the end of the section header table: where an
    /// AppImage's squashfs payload starts.
    pub end: u64,
}

const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ELF32_HEADER: usize = 52;
const ELF64_HEADER: usize = 64;

/// Reads the ELF header in `head` (little- and big-endian, 32/64-bit).
pub fn elf_info(head: &[u8]) -> Option<ElfInfo> {
    if !head.starts_with(ELF_MAGIC) {
        return None;
    }
    let little = *head.get(EI_DATA)? == ELFDATA2LSB;
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = head.get(at..at + 2)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = head.get(at..at + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let u64_at = |at: usize| -> Option<u64> {
        let b: [u8; 8] = head.get(at..at + 8)?.try_into().ok()?;
        Some(if little { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) })
    };
    let machine = u16_at(18)?;
    let (shoff, shentsize, shnum) = match *head.get(EI_CLASS)? {
        ELFCLASS64 if head.len() >= ELF64_HEADER => (u64_at(40)?, u16_at(58)?, u16_at(60)?),
        ELFCLASS32 if head.len() >= ELF32_HEADER => (u64::from(u32_at(32)?), u16_at(46)?, u16_at(48)?),
        _ => return None,
    };
    Some(ElfInfo { arch: Arch::from_machine(machine), end: shoff + u64::from(shentsize) * u64::from(shnum) })
}

/// The first [`SNIFF_BYTES`] of a file.
pub fn head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0; SNIFF_BYTES];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    buf.truncate(n);
    Ok(buf)
}

/// The lowercase file name.
fn lower_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Why a file isn't installable, when it looks like something Mandara
/// recognizes but can't install (shown instead of "unknown file").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// An AppImage of the old ISO 9660 kind.
    AppImageType1,
    /// A Snap package (`.snap` squashfs).
    Snap,
    /// A plain program, not an app package.
    BareElf,
}

/// Detects the format of `path` from its first bytes (`head`) and name.
/// Cheap: no decompression, nothing executed.
pub fn sniff(path: &Path, head: &[u8]) -> Result<Format, Option<Unsupported>> {
    let name = lower_name(path);
    if head.starts_with(ELF_MAGIC) {
        let marker = head.get(APPIMAGE_MARKER_AT..APPIMAGE_MARKER_AT + APPIMAGE_TYPE2.len());
        return match marker {
            Some(m) if m == APPIMAGE_TYPE2 => Ok(Format::AppImage),
            Some(m) if m == APPIMAGE_TYPE1 => Err(Some(Unsupported::AppImageType1)),
            // An AppImage built without the marker still ends in squashfs;
            // trust the extension then (verified when the payload is read).
            _ if name.ends_with(".appimage") => Ok(Format::AppImage),
            _ => Err(Some(Unsupported::BareElf)),
        };
    }
    if head.starts_with(SQUASHFS_MAGIC) && name.ends_with(".snap") {
        return Err(Some(Unsupported::Snap));
    }
    if head.starts_with(AR_MAGIC) && head.get(AR_MAGIC.len()..).is_some_and(|r| r.starts_with(DEB_FIRST_MEMBER)) {
        return Ok(Format::Deb);
    }
    if head.starts_with(RPM_MAGIC) {
        return Ok(Format::Rpm);
    }
    if head.starts_with(ZIP_MAGIC) {
        return Ok(Format::Zip);
    }
    if head.starts_with(APK_V3_MAGIC) && name.ends_with(".apk") {
        return Ok(Format::Apk);
    }
    let text_start = String::from_utf8_lossy(&head[..head.len().min(SNIFF_BYTES)]);
    if text_start.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')) == Some(FLATPAKREF_GROUP) {
        return Ok(Format::FlatpakRef);
    }
    let compressed = if head.starts_with(GZIP_MAGIC) {
        Some(Format::TarGz)
    } else if head.starts_with(XZ_MAGIC) {
        Some(Format::TarXz)
    } else if head.starts_with(ZSTD_MAGIC) {
        Some(Format::TarZst)
    } else {
        None
    };
    if let Some(container) = compressed {
        // Same containers, told apart by name (contents are checked when
        // the file is inspected).
        if name.contains(".pkg.tar.") {
            return Ok(Format::Pacman);
        }
        if name.ends_with(".xbps") {
            return Ok(Format::Xbps);
        }
        if name.ends_with(".apk") && container == Format::TarGz {
            return Ok(Format::Apk);
        }
        return Ok(container);
    }
    if name.ends_with(".flatpak") && head.windows(FLATPAK_BUNDLE_MARKER.len()).any(|w| w == FLATPAK_BUNDLE_MARKER) {
        return Ok(Format::FlatpakBundle);
    }
    // Bundles keep their metadata deeper than the first page sometimes:
    // the name is the tie-breaker when nothing else matched.
    if name.ends_with(".flatpak") && !head.is_empty() && !head.iter().all(u8::is_ascii) {
        return Ok(Format::FlatpakBundle);
    }
    Err(None)
}

/// Detects `path`'s format (reads [`SNIFF_BYTES`]).
pub fn detect(path: &Path) -> Result<Format, Option<Unsupported>> {
    match head(path) {
        Ok(h) => sniff(path, &h),
        Err(_) => Err(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf64(machine: u16, shoff: u64, shnum: u16, marker: &[u8]) -> Vec<u8> {
        let mut h = vec![0u8; ELF64_HEADER];
        h[..4].copy_from_slice(ELF_MAGIC);
        h[EI_CLASS] = ELFCLASS64;
        h[EI_DATA] = ELFDATA2LSB;
        h[APPIMAGE_MARKER_AT..APPIMAGE_MARKER_AT + marker.len()].copy_from_slice(marker);
        h[18..20].copy_from_slice(&machine.to_le_bytes());
        h[40..48].copy_from_slice(&shoff.to_le_bytes());
        h[58..60].copy_from_slice(&64u16.to_le_bytes());
        h[60..62].copy_from_slice(&shnum.to_le_bytes());
        h
    }

    #[test]
    fn reads_elf_architecture_and_end() {
        let h = elf64(EM_AARCH64, 1000, 3, b"");
        assert_eq!(elf_info(&h), Some(ElfInfo { arch: Arch::Aarch64, end: 1000 + 64 * 3 }));
        assert_eq!(elf_info(b"nope"), None);
    }

    #[test]
    fn sniffs_by_magic_not_extension() {
        let p = |n: &str| std::path::PathBuf::from(n);
        assert_eq!(sniff(&p("x.bin"), &elf64(EM_X86_64, 0, 0, APPIMAGE_TYPE2)), Ok(Format::AppImage));
        assert_eq!(sniff(&p("x.AppImage"), &elf64(EM_X86_64, 0, 0, APPIMAGE_TYPE1)), Err(Some(Unsupported::AppImageType1)));
        assert_eq!(sniff(&p("tool"), &elf64(EM_X86_64, 0, 0, b"")), Err(Some(Unsupported::BareElf)));
        assert_eq!(sniff(&p("a.deb"), b"!<arch>\ndebian-binary   1342"), Ok(Format::Deb));
        assert_eq!(sniff(&p("renamed.zip"), b"!<arch>\ndebian-binary   1342"), Ok(Format::Deb), "magic wins over the name");
        assert_eq!(sniff(&p("a.rpm"), RPM_MAGIC), Ok(Format::Rpm));
        assert_eq!(sniff(&p("a.zip"), b"PK\x03\x04rest"), Ok(Format::Zip));
        assert_eq!(sniff(&p("app.flatpakref"), b"\n[Flatpak Ref]\nName=org.x.Y\n"), Ok(Format::FlatpakRef));
        assert_eq!(sniff(&p("app-1.0-1-x86_64.pkg.tar.zst"), ZSTD_MAGIC), Ok(Format::Pacman));
        assert_eq!(sniff(&p("app-1.0.tar.zst"), ZSTD_MAGIC), Ok(Format::TarZst));
        assert_eq!(sniff(&p("app-1.0_1.x86_64.xbps"), ZSTD_MAGIC), Ok(Format::Xbps));
        assert_eq!(sniff(&p("app.tar.xz"), XZ_MAGIC), Ok(Format::TarXz));
        assert_eq!(sniff(&p("app.tgz"), GZIP_MAGIC), Ok(Format::TarGz));
        assert_eq!(sniff(&p("app-1.0-r0.apk"), GZIP_MAGIC), Ok(Format::Apk));
        assert_eq!(sniff(&p("notes.txt"), b"hello"), Err(None));
        assert_eq!(sniff(&p("x.snap"), b"hsqs...."), Err(Some(Unsupported::Snap)));
    }

    #[test]
    fn package_arch_words() {
        assert_eq!(Arch::from_package_word("amd64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_package_word("arm64"), Some(Arch::Aarch64));
        assert_eq!(Arch::from_package_word("noarch"), None);
    }
}
