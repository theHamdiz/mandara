//! The curated catalog: eight open sound themes that desktops ship as their
//! defaults, each pinned to its upstream release (or tag) download and its
//! SHA-256, HTTPS only.
//!
//! Picked for reach (the theme a desktop or distribution ships by default)
//! among themes whose licence, read from files inside the archive, allows
//! redistribution: freedesktop (the spec's fallback), KDE's Ocean (Plasma
//! 6) and Oxygen (Plasma 4 / 5), Ubuntu's classic set, elementary OS,
//! COSMIC (System76, also Pop!_OS), deepin and Materia (Google's Material
//! product sounds, CC-BY-4.0). Yaru ships only inside a 55 MiB icon
//! repository, Borealis's licence forbids commercial use, and themes made
//! from other systems' sounds (macOS, Windows, MIUI) aren't licensed for
//! sharing, so they are left out. Nothing is bundled in the binary: themes
//! download on demand (2 MB at most here).
//!
//! Some releases need assembling, never building: freedesktop ships
//! `index.theme.in` (an index is written instead and its `make install`
//! aliases become links), COSMIC ships `src/index.theme.in` with one
//! placeholder, and Materia's files are renamed into `stereo/` as its meson
//! file does. No script from an archive is ever run.

use mandara_i18n::tk;

pub use crate::cursors::catalog::Asset;

/// One theme in the catalog.
#[derive(Debug)]
pub struct SoundTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub author: &'static str,
    /// SPDX licence expression. Licence and credit files from the archive
    /// are installed with the theme.
    pub license: &'static str,
    pub homepage: &'static str,
    /// One line (`tk` text; display with `tr`).
    pub description: &'static str,
    /// Where it comes from or who ships it (`tk` text).
    pub origin: &'static str,
    /// The folder installed into `~/.local/share/sounds/` and the name
    /// apps are given.
    pub theme: &'static str,
    /// The card's accent (`0xRRGGBB`).
    pub swatch: u32,
    pub asset: Asset,
    pub layout: Layout,
    /// Links made after copying, `stereo/<name>` → `stereo/<target>`, when
    /// the release leaves them to its install step.
    pub aliases: &'static [(&'static str, &'static str)],
}

/// How the theme folder is assembled from `asset.folder`.
#[derive(Debug)]
pub enum Layout {
    /// The folder is the theme: `index.theme` and its sound directories.
    Ready,
    /// The sound directories are there; `index.theme` is written from
    /// this text.
    Index(&'static str),
    /// `index.theme` comes from `file` in the folder with each `(from,
    /// to)` replaced.
    Template { file: &'static str, replace: &'static [(&'static str, &'static str)] },
    /// The folder holds `index.theme`; each `(source, sound)` file (paths
    /// inside the folder) becomes `stereo/<sound>.<its extension>`.
    Renamed(&'static [(&'static str, &'static str)]),
}

const FREEDESKTOP_INDEX: &str = "[Sound Theme]\nName=Default\nComment=Default Desktop Sound Theme\nDirectories=stereo\n\n[stereo]\nOutputProfile=stereo\n";

/// freedesktop's `make install` links.
const FREEDESKTOP_ALIASES: &[(&str, &str)] = &[
    ("power-plug", "device-added"),
    ("network-connectivity-established", "device-added"),
    ("power-unplug", "device-removed"),
    ("network-connectivity-lost", "device-removed"),
    ("screen-capture", "camera-shutter"),
    ("dialog-error", "dialog-warning"),
    ("window-attention", "dialog-warning"),
    ("window-question", "dialog-warning"),
];

/// Materia's meson rename map (Google's file names → sound names).
const MATERIA_FILES: &[(&str, &str)] = &[
    ("material_product_sounds/alert_error-01.ogg", "dialog-error"),
    ("material_product_sounds/alert_error-02.ogg", "dialog-warning"),
    ("material_product_sounds/alert_high-intensity.ogg", "window-attention"),
    ("material_product_sounds/hero_decorative-celebration-02.ogg", "desktop-login"),
    ("material_product_sounds/hero_decorative-celebration-02.ogg", "system-bootup"),
    ("material_product_sounds/navigation_hover-tap.ogg", "audio-volume-change"),
    ("material_product_sounds/navigation_selection-complete-celebration.ogg", "trash-empty"),
    ("material_product_sounds/notification_high-intensity.ogg", "message-new-instant"),
    ("material_product_sounds/notification_simple-01.ogg", "window-question"),
    ("material_product_sounds/ringtone_minimal.ogg", "alarm-clock-elapsed"),
    ("material_product_sounds/state-change_confirm-down.ogg", "device-removed"),
    ("material_product_sounds/state-change_confirm-up.ogg", "device-added"),
    ("material_product_sounds/ui_camera-shutter.ogg", "camera-shutter"),
    ("material_product_sounds/ui_camera-shutter.ogg", "screen-capture"),
];

const fn asset(url: &'static str, sha256: &'static str, size: u64, folder: &'static str) -> Asset {
    Asset { url, sha256, size, folder }
}

/// Every theme, by reach.
pub const CATALOG: &[SoundTheme] = &[
    SoundTheme {
        id: "freedesktop",
        name: "freedesktop",
        author: "freedesktop.org",
        license: "GPL-2.0-or-later AND GPL-2.0-only AND CC-BY-SA-3.0 AND CC-BY-3.0",
        homepage: "https://www.freedesktop.org/wiki/Specifications/sound-theme-spec/",
        description: tk("The neutral baseline: short chimes and clicks every desktop falls back to"),
        origin: tk("The default of GNOME, Fedora, Arch and Ubuntu"),
        theme: "freedesktop",
        swatch: 0x5b8fd6,
        asset: asset("https://people.freedesktop.org/~mccann/dist/sound-theme-freedesktop-0.8.tar.bz2", "cb518b20eef05ec2e82dda1fa89a292c1760dc023aba91b8aa69bafac85e8a14", 478_237, "sound-theme-freedesktop-0.8"),
        layout: Layout::Index(FREEDESKTOP_INDEX),
        aliases: FREEDESKTOP_ALIASES,
    },
    SoundTheme {
        id: "ocean",
        name: "Ocean",
        author: "Guilherme Marçal Silva, KDE",
        license: "CC-BY-SA-4.0",
        homepage: "https://invent.kde.org/plasma/ocean-sound-theme",
        description: tk("Modern, melodic and airy, with a full set of device and battery cues"),
        origin: tk("The default of KDE Plasma 6"),
        theme: "ocean",
        swatch: 0x2ea3c7,
        asset: asset("https://download.kde.org/stable/plasma/6.8.3/ocean-sound-theme-6.8.3.tar.xz", "f13f85e49cac35be429c50e95b21ce281f4a6bba02ecd006a073adcd4f299ab8", 2_148_172, "ocean-sound-theme-6.8.3/ocean"),
        layout: Layout::Ready,
        aliases: &[],
    },
    SoundTheme {
        id: "oxygen",
        name: "Oxygen",
        author: "Nuno Filipe Povoa, KDE",
        license: "LGPL-3.0-or-later AND CC-BY-3.0",
        homepage: "https://invent.kde.org/plasma/oxygen-sounds",
        description: tk("KDE's classic set, warm and rounded, with an orchestral login"),
        origin: tk("The default of KDE 4 and Plasma 5"),
        theme: "oxygen",
        swatch: 0x4f8fc0,
        asset: asset("https://download.kde.org/stable/plasma/6.8.3/oxygen-sounds-6.8.3.tar.xz", "faab8fda3e37cd609f51703c9279d7e238f032e9c0df83de227c65bcf19b60d5", 1_935_672, "oxygen-sounds-6.8.3/sounds/oxygen"),
        layout: Layout::Ready,
        aliases: &[],
    },
    SoundTheme {
        id: "ubuntu",
        name: "Ubuntu",
        author: "Nathaniel McCallum, Canonical",
        license: "CC-BY-SA-2.0",
        homepage: "https://launchpad.net/ubuntu/+source/ubuntu-sounds",
        description: tk("Ubuntu's classic sounds, with a drum-led login"),
        origin: tk("Ubuntu's default before Yaru"),
        theme: "ubuntu",
        swatch: 0xe95420,
        asset: asset("https://launchpad.net/ubuntu/+archive/primary/+sourcefiles/ubuntu-sounds/0.14/ubuntu-sounds_0.14.tar.xz", "487359b8264ee2e5562336ac34cb4f58ab99ca133a5e07de935450b69394d4f7", 2_319_968, "ubuntu-sounds-0.14/ubuntu"),
        layout: Layout::Ready,
        aliases: &[],
    },
    SoundTheme {
        id: "elementary",
        name: "elementary",
        author: "elementary, Inc.",
        // One sound (dialog-warning) is CC BY, version unstated; its credit is in
        // the README installed with the theme.
        license: "Unlicense",
        homepage: "https://github.com/elementary/sound-theme",
        description: tk("Light, natural and pleasant: a few physical sounds over freedesktop"),
        origin: tk("The default of elementary OS"),
        theme: "elementary",
        swatch: 0x64baff,
        asset: asset("https://github.com/elementary/sound-theme/archive/refs/tags/1.1.0.tar.gz", "ef1989edb2fa0a0db12392242bea0678018b9070f3f202bc1c915f80f0abf982", 67_348, "sound-theme-1.1.0/elementary"),
        layout: Layout::Ready,
        aliases: &[],
    },
    SoundTheme {
        id: "cosmic",
        name: "COSMIC",
        author: "System76",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/pop-os/cosmic-sound-theme",
        description: tk("Short synthesised tones, the only set here with a lock sound"),
        origin: tk("The COSMIC desktop's set, from Pop!_OS"),
        theme: "COSMIC",
        swatch: 0x48b9c7,
        asset: asset("https://github.com/pop-os/cosmic-sound-theme/archive/refs/tags/epoch-1.11.0.tar.gz", "b523afe23c4b5bb58261156564b7527a1f719bc9b9917f9c6bd3c763d823291c", 2_175_893, "cosmic-sound-theme-epoch-1.11.0/src"),
        // Its directories are space-separated; the spec (and libcanberra)
        // wants commas.
        layout: Layout::Template { file: "index.theme.in", replace: &[("@ThemeName@", "COSMIC"), ("Directories=stereo/alert stereo/action stereo/notification", "Directories=stereo/alert,stereo/action,stereo/notification")] },
        aliases: &[],
    },
    SoundTheme {
        id: "deepin",
        name: "Deepin",
        author: "Deepin Technology",
        license: "GPL-3.0-or-later",
        homepage: "https://github.com/linuxdeepin/deepin-sound-theme",
        description: tk("Bright and polished, with long login and logout cues"),
        origin: tk("The default of deepin"),
        theme: "deepin",
        swatch: 0x2ca7f8,
        asset: asset("https://github.com/linuxdeepin/deepin-sound-theme/archive/refs/tags/15.10.6.tar.gz", "73206d000fe0e33597d33e05572d8e09aae7896d2640d114a0f19f8f1a44582b", 4_261_948, "deepin-sound-theme-15.10.6"),
        layout: Layout::Ready,
        aliases: &[],
    },
    SoundTheme {
        id: "materia",
        name: "Materia",
        author: "nana-4 (sounds by Google)",
        license: "GPL-3.0-only AND CC-BY-4.0",
        homepage: "https://github.com/nana-4/materia-sound-theme",
        description: tk("Google's Material Design product sounds as a desktop theme"),
        origin: tk("Material Design sounds, packaged for the desktop"),
        theme: "Materia",
        swatch: 0x8ab4f8,
        asset: asset("https://github.com/nana-4/materia-sound-theme/archive/refs/tags/v0.1.tar.gz", "9ebef1e51ee4e309b60a75c4cb8cc6e167d521ce3a3180b4b9d6c87a0630494e", 1_027_078, "materia-sound-theme-0.1"),
        layout: Layout::Renamed(MATERIA_FILES),
        aliases: &[],
    },
];

/// The events a gallery card offers to preview (Studio's play buttons).
pub const SIGNATURE: [crate::sounds::SoundEvent; 3] = [crate::sounds::SoundEvent::Notification, crate::sounds::SoundEvent::NotificationCritical, crate::sounds::SoundEvent::DeviceAdded];

pub fn find(id: &str) -> Option<&'static SoundTheme> {
    CATALOG.iter().find(|t| t.id == id)
}

/// A catalog theme by its id or its installed folder name.
pub fn resolve(spec: &str) -> Option<&'static SoundTheme> {
    find(spec).or_else(|| CATALOG.iter().find(|t| t.theme == spec))
}

/// The catalog theme installed as folder `name`.
pub fn by_theme_name(name: &str) -> Option<&'static SoundTheme> {
    CATALOG.iter().find(|t| t.theme == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relative path that stays inside its folder.
    fn safe(p: &str) -> bool {
        !p.is_empty() && !p.starts_with('/') && !p.split('/').any(|c| c.is_empty() || c == ".." || c == ".")
    }

    #[test]
    fn ids_and_folders_are_unique() {
        let mut ids: Vec<_> = CATALOG.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        let mut names: Vec<_> = CATALOG.iter().map(|t| t.theme.to_lowercase()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CATALOG.len(), "installed folder names are unique");
    }

    #[test]
    fn every_entry_is_complete_pinned_and_https() {
        let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        for t in CATALOG {
            assert!(!t.license.is_empty() && !t.author.is_empty() && !t.name.is_empty() && !t.description.is_empty() && !t.origin.is_empty(), "{}", t.id);
            assert!(t.homepage.starts_with("https://"), "{}", t.id);
            let a = &t.asset;
            assert!(a.url.starts_with("https://"), "{}: {}", t.id, a.url);
            // Official release / tag locations only.
            let official = ["https://github.com/", "https://download.kde.org/stable/", "https://launchpad.net/ubuntu/+archive/", "https://people.freedesktop.org/"];
            assert!(official.iter().any(|o| a.url.starts_with(o)), "{}: {}", t.id, a.url);
            if a.url.starts_with("https://github.com/") {
                assert!(a.url.starts_with(t.homepage) && (a.url.contains("/archive/refs/tags/") || a.url.contains("/releases/download/")), "{}: a tag or release of its own repository", t.id);
            }
            assert!(hex(a.sha256), "{}: sha256", t.id);
            assert!(a.size > 0 && a.size < super::super::install::MAX_DOWNLOAD, "{}", t.id);
            assert!(safe(a.folder), "{}: {}", t.id, a.folder);
            assert!(!t.theme.is_empty() && !t.theme.contains('/') && !t.theme.starts_with('.'), "{}", t.id);
            assert!(t.swatch <= 0xff_ffff);
            match &t.layout {
                Layout::Ready => {}
                Layout::Index(text) => assert!(text.starts_with("[Sound Theme]") && text.contains("Directories="), "{}", t.id),
                Layout::Template { file, replace } => assert!(safe(file) && !replace.is_empty(), "{}", t.id),
                Layout::Renamed(files) => {
                    assert!(!files.is_empty(), "{}", t.id);
                    for (from, to) in *files {
                        assert!(safe(from) && !to.contains('/') && !to.contains('.') && !to.is_empty(), "{}: {from} → {to}", t.id);
                    }
                }
            }
            for (name, target) in t.aliases {
                assert!(!name.contains('/') && !target.contains('/') && name != target, "{}: {name}", t.id);
            }
        }
    }

    #[test]
    fn resolves_ids_and_folder_names() {
        assert_eq!(resolve("ocean").map(|t| t.theme), Some("ocean"));
        assert_eq!(resolve("COSMIC").map(|t| t.id), Some("cosmic"));
        assert_eq!(resolve("cosmic").map(|t| t.theme), Some("COSMIC"));
        assert!(resolve("nope").is_none());
        assert_eq!(by_theme_name("Materia").map(|t| t.id), Some("materia"));
    }

    #[test]
    fn signature_events_have_spec_names() {
        for e in SIGNATURE {
            assert!(!e.names().is_empty());
        }
    }
}
