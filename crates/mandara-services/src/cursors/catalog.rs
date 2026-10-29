//! The curated catalog: ten cursor themes the Linux ricing community uses
//! most, each pinned to an upstream GitHub release download and its
//! SHA-256.
//!
//! Chosen by GitHub stars (October 2026) among themes that publish
//! ready-built XCursor folders on GitHub: Bibata (4.2k), macOS / apple_cursor
//! (2.1k), Qogir (940), Phinger (820), Catppuccin (760), BreezeX (480),
//! Vimix (460), Banana (360), Nordzy (320) and Rosé Pine (190). Capitaine
//! (1.3k), Oreo and Volantes only publish sources that need a build,
//! Simp1e lives on GitLab and Sweet's releases are GTK themes, so they are
//! left out. Every licence allows redistribution, but nothing is bundled in
//! the binary: themes download on demand (a few MB each).
//!
//! Qogir and Vimix attach no separate files to their releases: the
//! release's source archive already contains the built cursors (`dist*`
//! folders), which is all that is copied out of it.

use mandara_i18n::tk;

/// One theme in the catalog.
#[derive(Debug)]
pub struct CursorTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub author: &'static str,
    /// SPDX licence identifier.
    pub license: &'static str,
    pub homepage: &'static str,
    /// One line (`tk` text; display with `tr`).
    pub description: &'static str,
    /// GitHub stars when the catalog was curated (why it is here).
    pub stars: u32,
    pub variants: &'static [CursorVariant],
}

/// A downloadable variant (a colour or style) of a theme.
#[derive(Debug)]
pub struct CursorVariant {
    pub id: &'static str,
    /// `tk` text; display with `tr`.
    pub label: &'static str,
    /// The XCursor theme name: the folder installed into
    /// `~/.local/share/icons/` and the name compositors are given.
    pub theme: &'static str,
    /// The cursor body's colour (`0xRRGGBB`), for the variant dot.
    pub swatch: u32,
    pub asset: Asset,
}

/// A pinned release download.
#[derive(Debug)]
pub struct Asset {
    pub url: &'static str,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: &'static str,
    /// Size in bytes (the download is cut off past it).
    pub size: u64,
    /// The folder inside the archive holding `index.theme` and `cursors/`.
    pub folder: &'static str,
}

#[allow(clippy::too_many_arguments)] // one table row
const fn v(id: &'static str, label: &'static str, theme: &'static str, swatch: u32, url: &'static str, sha256: &'static str, size: u64, folder: &'static str) -> CursorVariant {
    CursorVariant { id, label, theme, swatch, asset: Asset { url, sha256, size, folder } }
}

const QOGIR: &str = "https://github.com/vinceliuice/Qogir-icon-theme/archive/refs/tags/2025-02-15.tar.gz";
const QOGIR_SHA: &str = "b0d07cad5601e0341a53a62df0ed111823b75fc38741d435486620a59fb239ee";
const QOGIR_SIZE: u64 = 9_076_276;
const PHINGER: &str = "https://github.com/phisch/phinger-cursors/releases/download/v2.2/phinger-cursors-variants.tar.bz2";
const PHINGER_SHA: &str = "46d4cfc30a38c19cada3339027b5a78f7b0bf74f3220eba7001c4fdd9d5a6e56";
const PHINGER_SIZE: u64 = 1_922_305;
const VIMIX: &str = "https://github.com/vinceliuice/Vimix-cursors/archive/refs/tags/2020-02-24.tar.gz";
const VIMIX_SHA: &str = "69298d02264b5b15239c340f8fa899f91574c0eac49ad5745e8e588315423618";
const VIMIX_SIZE: u64 = 1_211_721;

/// Every theme, most popular first.
pub const CATALOG: &[CursorTheme] = &[
    CursorTheme {
        id: "bibata",
        name: "Bibata Modern",
        author: "Abdulkaiz Khatri (ful1e5)",
        license: "GPL-3.0",
        homepage: "https://github.com/ful1e5/Bibata_Cursor",
        description: tk("Rounded, material-inspired cursors: the most installed theme in ricing setups"),
        stars: 4233,
        variants: &[
            v("classic", tk("Classic"), "Bibata-Modern-Classic", 0x1e1e1e, "https://github.com/ful1e5/Bibata_Cursor/releases/download/v2.0.7/Bibata-Modern-Classic.tar.xz", "7d3495864e5bbef02f5e77de760b2905903b63c71495a78ef6306d19a3b556d8", 1_767_748, "Bibata-Modern-Classic"),
            v("ice", tk("Ice"), "Bibata-Modern-Ice", 0xf5f5f5, "https://github.com/ful1e5/Bibata_Cursor/releases/download/v2.0.7/Bibata-Modern-Ice.tar.xz", "a68cae60c4dc706350e194ebc91c5fe48bc7bc9d59e119555834a2a7ee5078ef", 1_701_468, "Bibata-Modern-Ice"),
            v("amber", tk("Amber"), "Bibata-Modern-Amber", 0xff9a1f, "https://github.com/ful1e5/Bibata_Cursor/releases/download/v2.0.7/Bibata-Modern-Amber.tar.xz", "58b7ac00ed3cb45acb6b917cb9807a553873f1cca5c91348c4abaebe638e5884", 1_902_476, "Bibata-Modern-Amber"),
        ],
    },
    CursorTheme {
        id: "macos",
        name: "macOS",
        author: "Abdulkaiz Khatri (ful1e5)",
        license: "GPL-3.0",
        homepage: "https://github.com/ful1e5/apple_cursor",
        description: tk("Open-source cursors in the style of macOS, with the spinning wait wheel"),
        stars: 2086,
        variants: &[
            v("black", tk("Black"), "macOS", 0x161616, "https://github.com/ful1e5/apple_cursor/releases/download/v2.0.1/macOS.tar.xz", "9c6e5e13b068ce51a9e90a9abd6ce232ec7d25d4ceb05aa8ac76efbb99c76762", 5_514_788, "macOS"),
            v("white", tk("White"), "macOS-White", 0xfafafa, "https://github.com/ful1e5/apple_cursor/releases/download/v2.0.1/macOS-White.tar.xz", "3d2f731ade1828d73b19681db9afc62e6f8d059185e870edda64f8bedb783856", 2_721_420, "macOS-White"),
        ],
    },
    CursorTheme {
        id: "qogir",
        name: "Qogir",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/Qogir-icon-theme",
        description: tk("Flat, softly shadowed cursors from the Qogir icon theme"),
        stars: 940,
        variants: &[
            v("dark", tk("Dark"), "Qogir-cursors", 0x3b3e45, QOGIR, QOGIR_SHA, QOGIR_SIZE, "Qogir-icon-theme-2025-02-15/src/cursors/dist"),
            v("white", tk("White"), "Qogir-white-cursors", 0xf0f0f2, QOGIR, QOGIR_SHA, QOGIR_SIZE, "Qogir-icon-theme-2025-02-15/src/cursors/dist-Dark"),
        ],
    },
    CursorTheme {
        id: "phinger",
        name: "Phinger",
        author: "Philipp Schaffrath (phisch)",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/phisch/phinger-cursors",
        description: tk("Clean, slightly stylized cursors with a finger-shaped pointer"),
        stars: 821,
        variants: &[
            v("dark", tk("Dark"), "phinger-cursors-dark", 0x0e0e0e, PHINGER, PHINGER_SHA, PHINGER_SIZE, "phinger-cursors-dark"),
            v("light", tk("Light"), "phinger-cursors-light", 0xf7f7f7, PHINGER, PHINGER_SHA, PHINGER_SIZE, "phinger-cursors-light"),
        ],
    },
    CursorTheme {
        id: "catppuccin",
        name: "Catppuccin",
        author: "Catppuccin",
        license: "GPL-2.0",
        homepage: "https://github.com/catppuccin/cursors",
        description: tk("Soothing pastel cursors in the Catppuccin flavors and accents"),
        stars: 761,
        variants: &[
            v("mocha-dark", tk("Mocha Dark"), "catppuccin-mocha-dark-cursors", 0x45475a, "https://github.com/catppuccin/cursors/releases/download/v2.0.0/catppuccin-mocha-dark-cursors.zip", "a4d976491bdb1b1311b2de88327cad3f1c66c2d9da896e0c56362a660c802585", 1_349_763, "catppuccin-mocha-dark-cursors"),
            v("mocha-light", tk("Mocha Light"), "catppuccin-mocha-light-cursors", 0xcdd6f4, "https://github.com/catppuccin/cursors/releases/download/v2.0.0/catppuccin-mocha-light-cursors.zip", "29e530a7e228396c9e4e35fc4b457fb4e77d2872e0ba88cd1ac4bce3099414e3", 1_283_003, "catppuccin-mocha-light-cursors"),
            v("mocha-mauve", tk("Mocha Mauve"), "catppuccin-mocha-mauve-cursors", 0xcba6f7, "https://github.com/catppuccin/cursors/releases/download/v2.0.0/catppuccin-mocha-mauve-cursors.zip", "971e113a49de65529b3b706ce8529c1c80f22b2828b9c2f4732b3363fb69fcbb", 1_277_135, "catppuccin-mocha-mauve-cursors"),
            v("mocha-lavender", tk("Mocha Lavender"), "catppuccin-mocha-lavender-cursors", 0xb4befe, "https://github.com/catppuccin/cursors/releases/download/v2.0.0/catppuccin-mocha-lavender-cursors.zip", "9e8c67efd4c8ff75f09196fa8579ca366c5a4ea204f541edd7b49ab0cb44d9cf", 1_277_820, "catppuccin-mocha-lavender-cursors"),
            v("macchiato-dark", tk("Macchiato Dark"), "catppuccin-macchiato-dark-cursors", 0x494d64, "https://github.com/catppuccin/cursors/releases/download/v2.0.0/catppuccin-macchiato-dark-cursors.zip", "206ea3995f859baef5f4f7bd2669ecd16a60d70d9d4241b7eea7a89b0dde7572", 1_351_063, "catppuccin-macchiato-dark-cursors"),
            v("latte-light", tk("Latte Light"), "catppuccin-latte-light-cursors", 0xeff1f5, "https://github.com/catppuccin/cursors/releases/download/v2.0.0/catppuccin-latte-light-cursors.zip", "4c0752cfa2da6c4cbcfe9539ca92585144b34651f22acf6cecf18850cb938880", 1_310_915, "catppuccin-latte-light-cursors"),
        ],
    },
    CursorTheme {
        id: "breezex",
        name: "BreezeX",
        author: "Abdulkaiz Khatri (ful1e5)",
        license: "GPL-3.0",
        homepage: "https://github.com/ful1e5/BreezeX_Cursor",
        description: tk("KDE Breeze cursors, refined and extended with more shapes"),
        stars: 481,
        variants: &[
            v("dark", tk("Dark"), "BreezeX-Dark", 0x232627, "https://github.com/ful1e5/BreezeX_Cursor/releases/download/v2.0.1/BreezeX-Dark.tar.xz", "8cdf743466b0f1565fe5f290dd48ef4c02d91778458d0d3cc56437515255b653", 902_444, "BreezeX-Dark"),
            v("light", tk("Light"), "BreezeX-Light", 0xfcfcfc, "https://github.com/ful1e5/BreezeX_Cursor/releases/download/v2.0.1/BreezeX-Light.tar.xz", "40c1bdb224e611e03c9865a49a5b4f8131974cb6fab30af428c2a1a969484aa8", 921_760, "BreezeX-Light"),
            v("black", tk("Black"), "BreezeX-Black", 0x050505, "https://github.com/ful1e5/BreezeX_Cursor/releases/download/v2.0.1/BreezeX-Black.tar.xz", "773b75523748173409ee622841b0f74b1e80617a5c5b3dcbb53a7ac3d06cc233", 902_832, "BreezeX-Black"),
        ],
    },
    CursorTheme {
        id: "vimix",
        name: "Vimix",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/Vimix-cursors",
        description: tk("Crisp material cursors with a slim outline"),
        stars: 461,
        variants: &[
            v("dark", tk("Dark"), "Vimix-cursors", 0x444444, VIMIX, VIMIX_SHA, VIMIX_SIZE, "Vimix-cursors-2020-02-24/dist"),
            v("white", tk("White"), "Vimix-white-cursors", 0xececec, VIMIX, VIMIX_SHA, VIMIX_SIZE, "Vimix-cursors-2020-02-24/dist-white"),
        ],
    },
    CursorTheme {
        id: "banana",
        name: "Banana",
        author: "Abdulkaiz Khatri (ful1e5)",
        license: "GPL-3.0",
        homepage: "https://github.com/ful1e5/banana-cursor",
        description: tk("Playful, bright cursors in bold colors"),
        stars: 356,
        variants: &[
            v("yellow", tk("Yellow"), "Banana", 0xf9d71c, "https://github.com/ful1e5/banana-cursor/releases/download/v2.0.0/Banana.tar.xz", "9d2c4003315b3fc39b47c52dbfd6211d499db154b0fbb1c3c4dd9c0023561bf0", 3_575_776, "Banana"),
            v("blue", tk("Blue"), "Banana-Blue", 0x5fa8d3, "https://github.com/ful1e5/banana-cursor/releases/download/v2.0.0/Banana-Blue.tar.xz", "9a94ebbe062231f6a631e6edcad6346cba2e49b68fdea12a3fca6008597ec4f4", 3_491_640, "Banana-Blue"),
            v("green", tk("Green"), "Banana-Green", 0x6dc066, "https://github.com/ful1e5/banana-cursor/releases/download/v2.0.0/Banana-Green.tar.xz", "b920ba737e106d77f0ead57f8053607bd2182442fb2712756a62c5701af51f9f", 3_546_772, "Banana-Green"),
            v("red", tk("Red"), "Banana-Red", 0xe05a5a, "https://github.com/ful1e5/banana-cursor/releases/download/v2.0.0/Banana-Red.tar.xz", "39858505f3bf64e065e6090d30b5c0f8242c05b95bc91dd46ec9132a4fc1526c", 3_366_736, "Banana-Red"),
        ],
    },
    CursorTheme {
        id: "nordzy",
        name: "Nordzy",
        author: "Guillaume Boehm",
        license: "GPL-3.0",
        homepage: "https://github.com/guillaumeboehm/Nordzy-cursors",
        description: tk("Cursors in the cool Nord palette, based on Vimix"),
        stars: 324,
        variants: &[
            v("dark", tk("Dark"), "Nordzy-cursors", 0x3b4252, "https://github.com/guillaumeboehm/Nordzy-cursors/releases/download/v2.4.0/Nordzy-cursors.tar.gz", "3451c1221d58562a5eb647c45f3f7b5e2bbfe0aacf10d9cbc899bc36e5239e5a", 430_309, "Nordzy-cursors"),
            v("white", tk("White"), "Nordzy-cursors-white", 0xeceff4, "https://github.com/guillaumeboehm/Nordzy-cursors/releases/download/v2.4.0/Nordzy-cursors-white.tar.gz", "5873641192f7c89692f1335c525969879307482c0f70283142f3fa65cfa74ff6", 426_585, "Nordzy-cursors-white"),
        ],
    },
    CursorTheme {
        id: "rose-pine",
        name: "Rosé Pine",
        author: "Rosé Pine",
        license: "GPL-3.0",
        homepage: "https://github.com/rose-pine/cursors",
        description: tk("BreezeX cursors recolored in the Rosé Pine palette"),
        stars: 186,
        variants: &[
            v("main", tk("Main"), "BreezeX-RosePine-Linux", 0x26233a, "https://github.com/rose-pine/cursors/releases/download/v1.1.0/BreezeX-RosePine-Linux.tar.xz", "b330d59ce8e0e460209f638a97ce772b78d9e6b56ccf63c8a50e9d941289a1af", 864_184, "BreezeX-RosePine-Linux"),
            v("dawn", tk("Dawn"), "BreezeX-RosePineDawn-Linux", 0xfaf4ed, "https://github.com/rose-pine/cursors/releases/download/v1.1.0/BreezeX-RosePineDawn-Linux.tar.xz", "85a9dfc31f68a13d536e67200abeb72956300b5388cd3c239b1ce2e198dcadd8", 852_588, "BreezeX-RosePineDawn-Linux"),
        ],
    },
];

pub fn find(id: &str) -> Option<&'static CursorTheme> {
    CATALOG.iter().find(|t| t.id == id)
}

/// The theme and variant installed as XCursor theme `name`.
pub fn by_theme_name(name: &str) -> Option<(&'static CursorTheme, &'static CursorVariant)> {
    CATALOG.iter().find_map(|t| t.variants.iter().find(|v| v.theme == name).map(|v| (t, v)))
}

/// A variant by `theme` or `theme/variant` id (the first variant for a
/// bare theme id), or by its XCursor theme name.
pub fn resolve(spec: &str) -> Option<(&'static CursorTheme, &'static CursorVariant)> {
    let (id, variant) = spec.split_once('/').unwrap_or((spec, ""));
    if let Some(t) = find(id) {
        let v = if variant.is_empty() { t.variants.first() } else { t.variants.iter().find(|v| v.id == variant) };
        return v.map(|v| (t, v));
    }
    by_theme_name(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_ten_themes_with_unique_ids() {
        assert_eq!(CATALOG.len(), 10);
        let mut ids: Vec<_> = CATALOG.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        let mut names: Vec<_> = CATALOG.iter().flat_map(|t| t.variants).map(|v| v.theme).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "XCursor theme names are unique");
        for t in CATALOG {
            let mut vids: Vec<_> = t.variants.iter().map(|v| v.id).collect();
            vids.sort_unstable();
            vids.dedup();
            assert_eq!(vids.len(), t.variants.len(), "{}: variant ids unique", t.id);
        }
    }

    #[test]
    fn sorted_by_popularity() {
        assert!(CATALOG.windows(2).all(|w| w[0].stars >= w[1].stars));
    }

    #[test]
    fn every_entry_is_complete_and_pinned() {
        let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        for t in CATALOG {
            assert!(!t.license.is_empty() && !t.author.is_empty() && !t.name.is_empty() && !t.description.is_empty(), "{}", t.id);
            assert!(t.homepage.starts_with("https://github.com/"), "{}", t.id);
            assert!(!t.variants.is_empty(), "{}", t.id);
            for v in t.variants {
                let a = &v.asset;
                assert!(a.url.starts_with("https://github.com/"), "{}/{}: {}", t.id, v.id, a.url);
                // Only the theme's own repository serves it.
                assert!(a.url.starts_with(t.homepage), "{}/{}: {} is outside {}", t.id, v.id, a.url, t.homepage);
                assert!(hex(a.sha256), "{}/{}: sha256", t.id, v.id);
                assert!(a.size > 0 && a.size < super::super::install::MAX_DOWNLOAD, "{}/{}", t.id, v.id);
                assert!(!a.folder.is_empty() && !a.folder.starts_with('/') && !a.folder.split('/').any(|c| c == ".." || c == "."), "{}/{}", t.id, v.id);
                assert!(!v.theme.contains('/') && !v.theme.starts_with('.'), "{}/{}", t.id, v.id);
                assert!(v.swatch <= 0xff_ffff);
            }
        }
    }

    #[test]
    fn resolves_specs() {
        assert_eq!(resolve("bibata").map(|(_, v)| v.theme), Some("Bibata-Modern-Classic"));
        assert_eq!(resolve("bibata/ice").map(|(_, v)| v.theme), Some("Bibata-Modern-Ice"));
        assert_eq!(resolve("Nordzy-cursors-white").map(|(t, _)| t.id), Some("nordzy"));
        assert!(resolve("bibata/neon").is_none());
        assert!(resolve("nope").is_none());
    }
}
