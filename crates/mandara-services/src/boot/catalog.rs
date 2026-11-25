//! The curated catalog: popular open-source boot menu and splash themes,
//! each pinned to an archive from its author's GitHub repository (a
//! release download, or a tag or commit archive when the author publishes
//! no files) and its SHA-256.
//!
//! Chosen by GitHub stars (November 2026) among themes with a licence that
//! allows redistribution (GPL-3.0 or MIT), that ship ready theme files
//! (no build step). Themes with an unclear licence are left out. Nothing
//! is bundled in Mandara: a theme downloads when you preview or apply it.
//!
//! Some authors ship an install script that assembles the theme from
//! parts of the repository (grub2-themes): [`Layout::Map`] copies the same
//! files the script would (read from the script, never run), with only
//! theme file types kept ([`super::archive`]).

use mandara_i18n::tk;

use super::theme::Kind;

/// A pinned download.
#[derive(Debug)]
pub struct Asset {
    pub url: &'static str,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: &'static str,
    /// Size in bytes (the download is cut off past it).
    pub size: u64,
}

/// One file or folder copied into the theme: `from` is a path in the
/// archive (`dir/*.ext` for every file of that type in `dir`, a trailing
/// `/` for a whole folder), `to` its place in the theme (`""` the top).
#[derive(Debug)]
pub struct Copy {
    pub from: &'static str,
    pub to: &'static str,
}

#[derive(Debug)]
pub enum Layout {
    /// The theme is this folder of the archive (`""` its top).
    Folder(&'static str),
    Map(&'static [Copy]),
}

#[derive(Debug)]
pub struct Variant {
    pub id: &'static str,
    /// `tk` text for words ("Dark"), as is for names ("Mocha",
    /// "Arch Linux"); display with `tr`.
    pub label: &'static str,
    /// The folder it installs as (Plymouth and rEFInd themes name their
    /// own folder inside their files; GRUB ones get a `mandara-` name).
    pub install: &'static str,
    /// rEFInd: the file `refind.conf` includes.
    pub include: Option<&'static str>,
    /// Its main colour (`0xRRGGBB`) for the card.
    pub swatch: u32,
    pub asset: Asset,
    pub layout: Layout,
}

#[derive(Debug)]
pub struct BootTheme {
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
    pub kind: Kind,
    pub variants: &'static [Variant],
}

const fn a(url: &'static str, sha256: &'static str, size: u64) -> Asset {
    Asset { url, sha256, size }
}

#[allow(clippy::too_many_arguments)] // one table row
const fn v(id: &'static str, label: &'static str, install: &'static str, swatch: u32, asset: Asset, layout: Layout) -> Variant {
    Variant { id, label, install, include: None, swatch, asset, layout }
}

const GRUB2_THEMES: Asset = a("https://github.com/vinceliuice/grub2-themes/archive/refs/tags/2025-07-23.tar.gz", "7e10d0fcc8edd94597ffe3591132688ee243cdf9373b3d6e282eefe785d67c29", 11_756_257);

/// What grub2-themes' `install.sh` (`generate`) copies for one theme at
/// one screen size.
macro_rules! grub2 {
    ($theme:literal, $icons:literal, $res:literal) => {
        Layout::Map(&[
            Copy { from: "grub2-themes-2025-07-23/common/*.pf2", to: "" },
            Copy { from: concat!("grub2-themes-2025-07-23/config/theme-", $res, ".txt"), to: "theme.txt" },
            Copy { from: concat!("grub2-themes-2025-07-23/backgrounds/", $res, "/background-", $theme, ".jpg"), to: "background.jpg" },
            Copy { from: concat!("grub2-themes-2025-07-23/assets/assets-", $icons, "/icons-", $res, "/"), to: "icons" },
            Copy { from: concat!("grub2-themes-2025-07-23/assets/assets-select/select-", $res, "/*.png"), to: "" },
            Copy { from: concat!("grub2-themes-2025-07-23/assets/info-", $res, ".png"), to: "info.png" },
        ])
    };
}

macro_rules! grub2_theme {
    ($id:literal, $name:literal, $icons:literal, $swatch:literal, $desc:expr) => {
        BootTheme {
            id: $id,
            name: $name,
            author: "Vince Liuice",
            license: "GPL-3.0",
            homepage: "https://github.com/vinceliuice/grub2-themes",
            description: $desc,
            stars: 4680,
            kind: Kind::Grub,
            variants: &[
                v("1080p", "1080p", concat!("mandara-", $id, "-1080p"), $swatch, GRUB2_THEMES, grub2!($id, $icons, "1080p")),
                v("2k", "2K", concat!("mandara-", $id, "-2k"), $swatch, GRUB2_THEMES, grub2!($id, $icons, "2k")),
                v("4k", "4K", concat!("mandara-", $id, "-4k"), $swatch, GRUB2_THEMES, grub2!($id, $icons, "4k")),
            ],
        }
    };
}

const CATPPUCCIN_GRUB: &str = "https://github.com/catppuccin/grub/archive/0a37ab19f654e77129b409fed371891c01ffd0b9.tar.gz";
const CATPPUCCIN_GRUB_SHA: &str = "285a2f1942d9a7aaaa61d53fbf8eaea40c1bb1a2f5ed662b8355bf9b0499116b";
const CATPPUCCIN_GRUB_SIZE: u64 = 6_221_524;
#[cfg(test)]
const DISTRO: &str = "https://github.com/AdisonCavani/distro-grub-themes/releases/download/v3.2/";
#[cfg(test)]
const ADI: &str = "https://github.com/adi1090x/plymouth-themes/releases/download/v1.0/";
const CATPPUCCIN_PLYMOUTH: &str = "https://github.com/catppuccin/plymouth/archive/198eba2071d80e4a23b8f51a5859e8f4acf8de6c.tar.gz";
const CATPPUCCIN_PLYMOUTH_SHA: &str = "91f87e75c5cf505b02276c8163f8046bb4bede634dcb7b23da1c4d25190407c2";
const CATPPUCCIN_PLYMOUTH_SIZE: u64 = 123_355;
const CATPPUCCIN_REFIND: Asset = a("https://github.com/catppuccin/refind/archive/e92ad6f4673e30fbc79e69c9cbe3780fb9a3f05f.tar.gz", "8bfe9072e643f798c7d5073f36dd316fde4a43c76e082d39626d14e001c1c815", 1_207_688);

macro_rules! distro {
    ($id:literal, $label:expr, $swatch:literal, $sha:literal, $size:literal) => {
        v($id, $label, concat!("mandara-distro-", $id), $swatch, a(concat!("https://github.com/AdisonCavani/distro-grub-themes/releases/download/v3.2/", $id, ".tar"), $sha, $size), Layout::Folder(""))
    };
}

macro_rules! adi {
    ($id:literal, $label:expr, $swatch:literal, $sha:literal, $size:literal) => {
        v($id, $label, $id, $swatch, a(concat!("https://github.com/adi1090x/plymouth-themes/releases/download/v1.0/", $id, ".tar.gz"), $sha, $size), Layout::Folder($id))
    };
}

macro_rules! catppuccin_refind {
    ($id:literal, $label:expr, $swatch:literal) => {
        Variant { id: $id, label: $label, install: "catppuccin", include: Some(concat!($id, ".conf")), swatch: $swatch, asset: CATPPUCCIN_REFIND, layout: Layout::Folder("refind-e92ad6f4673e30fbc79e69c9cbe3780fb9a3f05f") }
    };
}

/// Every theme, by kind and most popular first.
pub const CATALOG: &[BootTheme] = &[
    // ── Boot menus: GRUB ──
    grub2_theme!("vimix", "Vimix", "color", 0x8c4fd8, tk("Soft glass panels over a violet mountain dusk, with coloured OS icons")),
    grub2_theme!("tela", "Tela", "color", 0x3d8be8, tk("A clean blue gradient with rounded selection and coloured OS icons")),
    grub2_theme!("stylish", "Stylish", "color", 0xe0566e, tk("A bold abstract wallpaper and a minimal centred menu")),
    grub2_theme!("whitesur", "WhiteSur", "whitesur", 0x5aa0f0, tk("A bright, macOS-inspired desktop look with matching icons")),
    BootTheme {
        id: "minegrub",
        name: "Minegrub",
        author: "Lxtharia",
        license: "MIT",
        homepage: "https://github.com/Lxtharia/minegrub-theme",
        description: tk("A blocky game-style title screen for your boot menu"),
        stars: 4647,
        kind: Kind::Grub,
        variants: &[v("classic", tk("Classic"), "mandara-minegrub", 0x6b8f3a, a("https://github.com/Lxtharia/minegrub-theme/archive/refs/tags/v3.1.0.tar.gz", "127ee3b112014dd6b26e2316ccd018084c300ecf5c401d6047efd0d5d4705112", 30_809_008), Layout::Folder("minegrub-theme-3.1.0/minegrub"))],
    },
    BootTheme {
        id: "distro",
        name: "Distro themes",
        author: "Adison Cavani",
        license: "GPL-3.0",
        homepage: "https://github.com/AdisonCavani/distro-grub-themes",
        description: tk("Your distribution's colours and emblem on a dark, rounded menu"),
        stars: 1167,
        kind: Kind::Grub,
        variants: &[
            distro!("arch-linux", "Arch Linux", 0x1793d1, "e589bb946d6cd7bf6303142cf3aeeb872fef416fb36ff3e168a11bccfeae850b", 430_080),
            distro!("debian", "Debian", 0xd70a53, "328634a5a17d96757cd8e806262133b2c481de86d59c1567f1bee42c7562c355", 430_080),
            distro!("fedora", "Fedora", 0x51a2da, "8574f466c0174749b9cce0f03b9a61fd1924bc8447fadc169993812f3a29f9fd", 430_080),
            distro!("ubuntu", "Ubuntu", 0xe95420, "5574b6604444044bee6b21b6d42417bf825c151a4a9529c7ac253c172f38f9b2", 419_840),
            distro!("linux-mint", "Linux Mint", 0x86be43, "6dbd4594aea89e0f7fd51b3a13dcef288e0d9ffe6fb8e2015a49c2ee22c9bec4", 430_080),
            distro!("opensuse", "openSUSE", 0x73ba25, "fb5dcbdc5239c8cca5f08aeeecf16c2ab68815f4a16fa5c17e2fd129dce19357", 430_080),
            distro!("nixos", "NixOS", 0x7ebae4, "a16e43c6e8d2b6278ed09b0523404197ee1793dc5efbc78d72592ae881a52016", 430_080),
            distro!("endeavouros", "EndeavourOS", 0x7f3fbf, "e49ae7e4298746ca8f83fcd058c65a431acb5d04f22228238aefbf81380fa396", 460_800),
            distro!("manjaro", "Manjaro", 0x35bf5c, "f888d14ac168810ffe79c14cc23c653186c64c1fd871abf7d50d3379bbfabee8", 430_080),
        ],
    },
    BootTheme {
        id: "fallout",
        name: "Fallout",
        author: "shvchk",
        license: "MIT",
        homepage: "https://github.com/shvchk/fallout-grub-theme",
        description: tk("A green retro terminal with a scanline glow"),
        stars: 1084,
        kind: Kind::Grub,
        variants: &[v("green", tk("Green"), "mandara-fallout", 0x1bff80, a("https://github.com/shvchk/fallout-grub-theme/archive/b441e25a6d115614dc00ee6d11355d019a4969bf.tar.gz", "f681bc8b093cfa1138b1b95aaec35541b08910ec8ec029599c7d667bbd225db3", 422_565), Layout::Folder("fallout-grub-theme-b441e25a6d115614dc00ee6d11355d019a4969bf"))],
    },
    BootTheme {
        id: "catppuccin",
        name: "Catppuccin",
        author: "Catppuccin",
        license: "MIT",
        homepage: "https://github.com/catppuccin/grub",
        description: tk("Soothing pastel flavours, from the dark Mocha to the light Latte"),
        stars: 649,
        kind: Kind::Grub,
        variants: &[
            v("mocha", "Mocha", "mandara-catppuccin-mocha", 0x1e1e2e, a(CATPPUCCIN_GRUB, CATPPUCCIN_GRUB_SHA, CATPPUCCIN_GRUB_SIZE), Layout::Folder("grub-0a37ab19f654e77129b409fed371891c01ffd0b9/src/catppuccin-mocha-grub-theme")),
            v("macchiato", "Macchiato", "mandara-catppuccin-macchiato", 0x24273a, a(CATPPUCCIN_GRUB, CATPPUCCIN_GRUB_SHA, CATPPUCCIN_GRUB_SIZE), Layout::Folder("grub-0a37ab19f654e77129b409fed371891c01ffd0b9/src/catppuccin-macchiato-grub-theme")),
            v("frappe", "Frappé", "mandara-catppuccin-frappe", 0x303446, a(CATPPUCCIN_GRUB, CATPPUCCIN_GRUB_SHA, CATPPUCCIN_GRUB_SIZE), Layout::Folder("grub-0a37ab19f654e77129b409fed371891c01ffd0b9/src/catppuccin-frappe-grub-theme")),
            v("latte", "Latte", "mandara-catppuccin-latte", 0xeff1f5, a(CATPPUCCIN_GRUB, CATPPUCCIN_GRUB_SHA, CATPPUCCIN_GRUB_SIZE), Layout::Folder("grub-0a37ab19f654e77129b409fed371891c01ffd0b9/src/catppuccin-latte-grub-theme")),
        ],
    },
    BootTheme {
        id: "poly-dark",
        name: "Poly dark",
        author: "shvchk",
        license: "MIT",
        homepage: "https://github.com/shvchk/poly-dark",
        description: tk("Dark low-poly shards with a calm, readable menu"),
        stars: 214,
        kind: Kind::Grub,
        variants: &[v("dark", tk("Dark"), "mandara-poly-dark", 0x2b2f3a, a("https://github.com/shvchk/poly-dark/archive/4850f0c917a0fa320cfd32779b4030baebb2ba8c.tar.gz", "1dd02c4a6079eb900525f67fefcc56cf3980cb5409c9b153318facae8585dc5c", 1_868_912), Layout::Folder("poly-dark-4850f0c917a0fa320cfd32779b4030baebb2ba8c"))],
    },
    BootTheme {
        id: "dracula",
        name: "Dracula",
        author: "Dracula Theme",
        license: "MIT",
        homepage: "https://github.com/dracula/grub",
        description: tk("The Dracula palette's purple and pink on deep night"),
        stars: 25,
        kind: Kind::Grub,
        variants: &[v("dark", tk("Dark"), "mandara-dracula", 0x282a36, a("https://github.com/dracula/grub/archive/0e721d99dbf0d5d6c4fd489b88248365b7a60d12.tar.gz", "1dc94774d7e0fb149ca7123fbcf99f2a5baa94e7a7cbe26734091167ea120180", 1_194_160), Layout::Folder("grub-0e721d99dbf0d5d6c4fd489b88248365b7a60d12/dracula"))],
    },
    // ── Boot menus: rEFInd ──
    BootTheme {
        id: "refind-minimal",
        name: "rEFInd minimal",
        author: "Evan Purkhiser",
        license: "MIT",
        homepage: "https://github.com/evanpurkhiser/rEFInd-minimal",
        description: tk("Flat white OS icons on a quiet dark background"),
        stars: 2284,
        kind: Kind::Refind,
        variants: &[Variant { id: "dark", label: tk("Dark"), install: "rEFInd-minimal", include: Some("theme.conf"), swatch: 0x1f2329, asset: a("https://github.com/evanpurkhiser/rEFInd-minimal/archive/56a06a4bd9ed76c05523a22017056dbecca4145d.tar.gz", "787bd81fbb16572a053437a855067b3b10b71e2840b3bff0b059619e2645436d", 851_399), layout: Layout::Folder("rEFInd-minimal-56a06a4bd9ed76c05523a22017056dbecca4145d") }],
    },
    BootTheme {
        id: "catppuccin-refind",
        name: "Catppuccin for rEFInd",
        author: "Catppuccin",
        license: "MIT",
        homepage: "https://github.com/catppuccin/refind",
        description: tk("Catppuccin's pastel flavours for the rEFInd menu"),
        stars: 147,
        kind: Kind::Refind,
        variants: &[catppuccin_refind!("mocha", "Mocha", 0x1e1e2e), catppuccin_refind!("macchiato", "Macchiato", 0x24273a), catppuccin_refind!("frappe", "Frappé", 0x303446), catppuccin_refind!("latte", "Latte", 0xeff1f5)],
    },
    // ── Splash: Plymouth ──
    BootTheme {
        id: "adi1090x",
        name: "Plymouth collection",
        author: "Aditya Shakya (adi1090x)",
        license: "GPL-3.0",
        homepage: "https://github.com/adi1090x/plymouth-themes",
        description: tk("Animated loaders from the most popular Plymouth collection"),
        stars: 3001,
        kind: Kind::Plymouth,
        variants: &[
            adi!("rings", "Rings", 0x30c0ff, "fcc6cc2c40a49a900d004dd1b65cff97adf5f99eb490bc89c9b8335872b71c76", 2_023_845),
            adi!("angular", "Angular", 0x00e0c0, "4cd26ecf180df5d6281c72b59254872f40c5a3144bf3289f660fdab0a3a43a97", 1_089_954),
            adi!("circle_flow", "Circle flow", 0x6ad0ff, "23f1c722a0309ff7a56b30dab3d7d1a453f03d40f8e2d79f5129e71c1b4c3581", 812_404),
            adi!("colorful_loop", "Colourful loop", 0xff5aa0, "4e8565dd866782507d23e8499c3dd817cdc80948213f4d48ed3dd88fb6816d59", 1_079_906),
            adi!("cubes", "Cubes", 0x4aa8ff, "0c41f707e575e161174a1f144ea7b0dcce73fd85106a8e7ecfc4fc4d5cc22687", 997_813),
            adi!("hexagon_dots", "Hexagon dots", 0x40e0d0, "380f44fe72256fc042bbc0361f8df0a4f9176f1c00dcde3e9a3f71a5914bb18e", 448_156),
            adi!("lone", "Lone", 0xffffff, "c89f3d94424271992af98344cbda597160a8c98d8327fbd76518f112442aa977", 775_283),
            adi!("sphere", "Sphere", 0x8f7bff, "f6cea910c7f0f0dcfa2795a71d5f8b17747fdd888c23add60e6af1935d305bd9", 466_288),
            adi!("spinner_alt", "Spinner", 0xdddddd, "7acd518be317a44d2a4db3168fd7b1c62c30deeded92cf65ca6a0a46fb85d68c", 112_245),
            adi!("connect", "Connect", 0x50e3a4, "e5f39dfd5a0138ddccf803e880c15162e8e14a698f0c5e815b7f884cf7118d3f", 443_589),
            adi!("liquid", "Liquid", 0x3fa9f5, "c8a28bfc223d47bc03e2479fcf644278cf4429f312302b5c58f0b4e8c5fa9c06", 534_731),
            adi!("abstract_ring", "Abstract ring", 0xff8a3d, "5866f24e418f805ee5b9216377ec507bc64ba0949efe7d8670a85a7252fdd941", 3_652_556),
            adi!("cuts", "Cuts", 0xe94560, "788f48280335987c597feaba50c7d3e901a9fb026c29e4ae5d71090579f6c8ad", 73_101),
        ],
    },
    BootTheme {
        id: "catppuccin-plymouth",
        name: "Catppuccin splash",
        author: "Catppuccin",
        license: "MIT",
        homepage: "https://github.com/catppuccin/plymouth",
        description: tk("A pastel spinner with a matching password prompt"),
        stars: 90,
        kind: Kind::Plymouth,
        variants: &[
            v("mocha", "Mocha", "catppuccin-mocha", 0x1e1e2e, a(CATPPUCCIN_PLYMOUTH, CATPPUCCIN_PLYMOUTH_SHA, CATPPUCCIN_PLYMOUTH_SIZE), Layout::Folder("plymouth-198eba2071d80e4a23b8f51a5859e8f4acf8de6c/themes/catppuccin-mocha")),
            v("macchiato", "Macchiato", "catppuccin-macchiato", 0x24273a, a(CATPPUCCIN_PLYMOUTH, CATPPUCCIN_PLYMOUTH_SHA, CATPPUCCIN_PLYMOUTH_SIZE), Layout::Folder("plymouth-198eba2071d80e4a23b8f51a5859e8f4acf8de6c/themes/catppuccin-macchiato")),
            v("frappe", "Frappé", "catppuccin-frappe", 0x303446, a(CATPPUCCIN_PLYMOUTH, CATPPUCCIN_PLYMOUTH_SHA, CATPPUCCIN_PLYMOUTH_SIZE), Layout::Folder("plymouth-198eba2071d80e4a23b8f51a5859e8f4acf8de6c/themes/catppuccin-frappe")),
            v("latte", "Latte", "catppuccin-latte", 0xeff1f5, a(CATPPUCCIN_PLYMOUTH, CATPPUCCIN_PLYMOUTH_SHA, CATPPUCCIN_PLYMOUTH_SIZE), Layout::Folder("plymouth-198eba2071d80e4a23b8f51a5859e8f4acf8de6c/themes/catppuccin-latte")),
        ],
    },
    BootTheme {
        id: "dracula-plymouth",
        name: "Dracula splash",
        author: "Dracula Theme",
        license: "MIT",
        homepage: "https://github.com/dracula/plymouth",
        description: tk("The Dracula emblem with spinning coloured stripes"),
        stars: 5,
        kind: Kind::Plymouth,
        variants: &[v("dark", tk("Dark"), "dracula", 0x282a36, a("https://github.com/dracula/plymouth/archive/37aa09b27ecee4a825b43d2c1d20b502e8f19c96.tar.gz", "44014e8c0ffb2605adb5f7e043edd7703dd455b71fcf206b632b17386a8ab546", 296_023), Layout::Folder("plymouth-37aa09b27ecee4a825b43d2c1d20b502e8f19c96/dracula"))],
    },
];

// The URL prefixes above are spelled out in the macros (concat! needs
// literals); these keep them in one place for the tests.
#[cfg(test)]
const PREFIXES: [&str; 2] = [DISTRO, ADI];

/// The themes of one kind.
pub fn of(kind: Kind) -> impl Iterator<Item = &'static BootTheme> {
    CATALOG.iter().filter(move |t| t.kind == kind)
}

pub fn find(id: &str) -> Option<&'static BootTheme> {
    CATALOG.iter().find(|t| t.id == id)
}

/// `theme[/variant]` (the first variant when none is named).
pub fn resolve(spec: &str) -> Option<(&'static BootTheme, &'static Variant)> {
    let (id, variant) = spec.split_once('/').unwrap_or((spec, ""));
    let t = find(id)?;
    let v = if variant.is_empty() { t.variants.first() } else { t.variants.iter().find(|v| v.id == variant) }?;
    Some((t, v))
}

/// The catalog entry that installs as folder `install`.
pub fn by_install(install: &str) -> Option<(&'static BootTheme, &'static Variant)> {
    CATALOG.iter().find_map(|t| t.variants.iter().find(|v| v.install == install).map(|v| (t, v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_install_folders_are_unique() {
        let mut ids: Vec<_> = CATALOG.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        // Plymouth and GRUB folders are one per variant; rEFInd variants of
        // one theme share their folder (different include files).
        let mut installs: Vec<_> = CATALOG.iter().filter(|t| t.kind != Kind::Refind).flat_map(|t| t.variants).map(|v| v.install).collect();
        let n = installs.len();
        installs.sort_unstable();
        installs.dedup();
        assert_eq!(installs.len(), n);
        for t in CATALOG {
            let mut vids: Vec<_> = t.variants.iter().map(|v| v.id).collect();
            vids.sort_unstable();
            vids.dedup();
            assert_eq!(vids.len(), t.variants.len(), "{}", t.id);
        }
    }

    #[test]
    fn sorted_by_popularity_within_each_kind() {
        for kind in [Kind::Grub, Kind::Refind, Kind::Plymouth] {
            let stars: Vec<u32> = of(kind).map(|t| t.stars).collect();
            assert!(stars.windows(2).all(|w| w[0] >= w[1]), "{kind:?}: {stars:?}");
        }
        assert!(of(Kind::Grub).count() >= 8 && of(Kind::Plymouth).count() >= 3 && of(Kind::Refind).count() >= 2);
    }

    #[test]
    fn every_entry_is_pinned_open_and_contained() {
        let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        let safe = |p: &str| !p.starts_with('/') && !p.split('/').any(|c| c == ".." || c == ".");
        for t in CATALOG {
            assert!(["GPL-3.0", "MIT"].contains(&t.license), "{}: redistributable licences only", t.id);
            assert!(t.homepage.starts_with("https://github.com/") && !t.author.is_empty() && !t.description.is_empty(), "{}", t.id);
            for v in t.variants {
                let a = &v.asset;
                assert!(a.url.starts_with(t.homepage) || PREFIXES.iter().any(|p| a.url.starts_with(p) && p.starts_with(t.homepage)), "{}/{}: {} is outside {}", t.id, v.id, a.url, t.homepage);
                assert!(a.url.starts_with("https://"), "HTTPS only");
                assert!(hex(a.sha256) && a.size > 0 && a.size < super::super::stage::MAX_DOWNLOAD, "{}/{}", t.id, v.id);
                assert!(super::super::valid_name(v.install), "{}/{}", t.id, v.id);
                if t.kind == Kind::Grub {
                    assert!(v.install.starts_with(super::super::PREFIX), "GRUB folders are Mandara's own: {}", v.install);
                }
                assert_eq!(v.include.is_some(), t.kind == Kind::Refind, "{}/{}", t.id, v.id);
                match &v.layout {
                    Layout::Folder(f) => assert!(f.is_empty() || safe(f), "{}/{}", t.id, v.id),
                    Layout::Map(m) => assert!(m.iter().all(|c| safe(c.from) && (c.to.is_empty() || safe(c.to))), "{}/{}", t.id, v.id),
                }
            }
        }
    }

    #[test]
    fn resolves_specs() {
        assert_eq!(resolve("vimix").map(|(_, v)| v.install), Some("mandara-vimix-1080p"));
        assert_eq!(resolve("adi1090x/cubes").map(|(_, v)| v.install), Some("cubes"));
        assert_eq!(by_install("catppuccin-mocha").map(|(t, _)| t.id), Some("catppuccin-plymouth"));
        assert!(resolve("vimix/8k").is_none() && resolve("nope").is_none());
    }
}
