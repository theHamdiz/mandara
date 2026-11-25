//! The curated catalog: the fifty most popular open-source icon themes
//! that install by copying ready-made folders out of one pinned archive
//! from the project's own GitHub repository.
//!
//! Ranked by GitHub stars (November 2026). Every licence allows
//! redistribution, but nothing is bundled in the binary: themes download
//! on demand. Left out: themes whose folders only exist after a build
//! (Yaru and elementary need meson, Fluent and Vimix need their install
//! scripts to recolor and assemble them), themes split across archives
//! (Numix Circle and Square link into Numix), non-commercial licences
//! (Newaita, Flatery, Surfn, Delta) and repositories without one
//! (Tela Circle, Dracula icons). Breeze and Adwaita live on their
//! desktops' own GitLab, with too few stars there to make the cut, and
//! come with most distributions anyway.
//!
//! Several themes are assembled from more than one folder of their
//! archive (`layers`: Tela, WhiteSur and Colloid keep their icons in `src/`
//! and their aliases in `links/`), and a variant may need sibling folders
//! its symlinks point into (`Papirus-Dark` needs `Papirus`): `dirs` lists
//! every folder a variant installs, its own first. Tela's `index.theme`
//! carries a `%NAME%` placeholder its script fills in; Mandara writes the
//! name itself (`fill_name`). No script from any archive is ever run.

use mandara_i18n::tk;

/// The look a theme's app icons have (the gallery's style filter).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Flat,
    Round,
    Outline,
    Colorful,
    Mono,
}

impl Style {
    pub const ALL: [Style; 5] = [Style::Flat, Style::Round, Style::Outline, Style::Colorful, Style::Mono];

    pub fn slug(self) -> &'static str {
        match self {
            Style::Flat => "flat",
            Style::Round => "round",
            Style::Outline => "outline",
            Style::Colorful => "colorful",
            Style::Mono => "mono",
        }
    }

    /// `tk` text; display with `tr`.
    pub fn label(self) -> &'static str {
        match self {
            Style::Flat => tk("Flat"),
            Style::Round => tk("Round"),
            Style::Outline => tk("Outline"),
            Style::Colorful => tk("Colorful"),
            Style::Mono => tk("Mono"),
        }
    }
}

/// The panel and window tones a theme has variants for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    Light,
    Dark,
    Both,
}

impl Tone {
    pub fn has_light(self) -> bool {
        matches!(self, Tone::Light | Tone::Both)
    }

    pub fn has_dark(self) -> bool {
        matches!(self, Tone::Dark | Tone::Both)
    }

    pub fn slug(self) -> &'static str {
        match self {
            Tone::Light => "light",
            Tone::Dark => "dark",
            Tone::Both => "both",
        }
    }
}

/// One theme in the catalog.
#[derive(Debug)]
pub struct IconTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub author: &'static str,
    /// SPDX licence expression.
    pub license: &'static str,
    pub homepage: &'static str,
    /// One line (`tk` text; display with `tr`).
    pub description: &'static str,
    /// GitHub stars when the catalog was curated (why it is here).
    pub stars: u32,
    pub style: Style,
    pub tone: Tone,
    /// The folder color (`0xRRGGBB`), for the card before it is installed.
    pub swatch: u32,
    /// The pinned download every variant comes from.
    pub archive: Asset,
    pub variants: &'static [IconVariant],
}

/// A pinned download.
#[derive(Debug)]
pub struct Asset {
    pub url: &'static str,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: &'static str,
    /// Size in bytes (the download is cut off past it).
    pub size: u64,
}

/// A variant: the theme folder apps are told to use, and every folder it
/// needs.
#[derive(Debug)]
pub struct IconVariant {
    pub id: &'static str,
    /// `tk` text; display with `tr`.
    pub label: &'static str,
    /// The icon theme name (its folder in `~/.local/share/icons/`).
    pub theme: &'static str,
    /// Made for dark panels and windows.
    pub dark: bool,
    /// The folders installed, `theme` first.
    pub dirs: &'static [Dir],
}

/// One installed folder, assembled from archive folders merged in order.
#[derive(Debug)]
pub struct Dir {
    pub name: &'static str,
    /// Paths inside the archive, merged into the folder in order.
    pub layers: &'static [&'static str],
    /// `index.theme` names the theme `%NAME%`: Mandara fills in this
    /// folder's name (dashes as spaces), as the theme's script would.
    pub fill_name: bool,
}

const fn d(name: &'static str, layers: &'static [&'static str]) -> Dir {
    Dir { name, layers, fill_name: false }
}

const fn dn(name: &'static str, layers: &'static [&'static str]) -> Dir {
    Dir { name, layers, fill_name: true }
}

/// Every theme, most popular first.
pub const CATALOG: &[IconTheme] = &[
    IconTheme {
        id: "papirus",
        name: "Papirus",
        author: "Papirus Development Team",
        license: "GPL-3.0",
        homepage: "https://github.com/PapirusDevelopmentTeam/papirus-icon-theme",
        description: tk("Flat icons with very wide app coverage, in light and dark variants"),
        stars: 8204,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x5294e2,
        archive: Asset { url: "https://github.com/PapirusDevelopmentTeam/papirus-icon-theme/archive/refs/tags/20260801.tar.gz", sha256: "646f622e9e7e9e65eef9d0ab58999d4920ddb33d98e6a75232627cfe3bd508f9", size: 33_053_628 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Papirus", dark: false, dirs: &[d("Papirus", &["papirus-icon-theme-20260801/Papirus"])] },
            IconVariant { id: "dark", label: tk("Dark"), theme: "Papirus-Dark", dark: true, dirs: &[d("Papirus-Dark", &["papirus-icon-theme-20260801/Papirus-Dark"]), d("Papirus", &["papirus-icon-theme-20260801/Papirus"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Papirus-Light", dark: false, dirs: &[d("Papirus-Light", &["papirus-icon-theme-20260801/Papirus-Light"]), d("Papirus", &["papirus-icon-theme-20260801/Papirus"])] },
        ],
    },
    IconTheme {
        id: "whitesur",
        name: "WhiteSur",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/WhiteSur-icon-theme",
        description: tk("Rounded-square app icons and blue folders in a macOS Big Sur style"),
        stars: 2130,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x6dc6f3,
        archive: Asset { url: "https://github.com/vinceliuice/WhiteSur-icon-theme/archive/refs/tags/2026-09-10.tar.gz", sha256: "406c9cd59705583f1754b0eaca96cc48bafda042b88ef143f16d8ae1820ecd95", size: 7_978_082 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "WhiteSur", dark: false, dirs: &[d("WhiteSur", &["WhiteSur-icon-theme-2026-09-10/src", "WhiteSur-icon-theme-2026-09-10/links"])] },
        ],
    },
    IconTheme {
        id: "la-capitaine",
        name: "La Capitaine",
        author: "Keefer Rourke",
        license: "GPL-3.0-or-later",
        homepage: "https://github.com/keeferrourke/la-capitaine-icon-theme",
        description: tk("macOS- and Material-inspired icons with soft gradients"),
        stars: 2000,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0x23b0fb,
        archive: Asset { url: "https://github.com/keeferrourke/la-capitaine-icon-theme/archive/refs/tags/v0.6.2.tar.gz", sha256: "e63e4bc97a7bbf4db71f17fa3b1c71086e3ed8b1dc44b4d10f95d97aa7db2c9d", size: 11_110_830 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "La-Capitaine", dark: false, dirs: &[d("La-Capitaine", &["la-capitaine-icon-theme-0.6.2"])] },
        ],
    },
    IconTheme {
        id: "tela",
        name: "Tela",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/Tela-icon-theme",
        description: tk("Flat, colorful icons with blue folders and a clean, modern look"),
        stars: 1899,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x5294e2,
        archive: Asset { url: "https://github.com/vinceliuice/Tela-icon-theme/archive/refs/tags/2026-07-07.tar.gz", sha256: "6b8c28f637067a15551459ab90e6720c5aba1215b777b6f1f506fea188a1ddf9", size: 3_519_076 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Tela", dark: false, dirs: &[dn("Tela", &["Tela-icon-theme-2026-07-07/src", "Tela-icon-theme-2026-07-07/links"])] },
        ],
    },
    IconTheme {
        id: "flat-remix",
        name: "Flat Remix",
        author: "Daniel Ruiz de Alegría",
        license: "GPL-3.0",
        homepage: "https://github.com/daniruiz/flat-remix",
        description: tk("Flat icons with subtle shading, in many folder colors and light or dark tones"),
        stars: 1767,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x367bf0,
        archive: Asset { url: "https://github.com/daniruiz/flat-remix/archive/refs/tags/20251119.tar.gz", sha256: "eba0547045a1714b47f1951d342624b70da150bdcc69d480883ab4667fe3e09b", size: 113_755_665 },
        variants: &[
            IconVariant { id: "blue-light", label: tk("Blue Light"), theme: "Flat-Remix-Blue-Light", dark: false, dirs: &[d("Flat-Remix-Blue-Light", &["flat-remix-20251119/Flat-Remix-Blue-Light"]), d("Flat-Remix-Blue-Dark", &["flat-remix-20251119/Flat-Remix-Blue-Dark"])] },
            IconVariant { id: "blue-dark", label: tk("Blue Dark"), theme: "Flat-Remix-Blue-Dark", dark: true, dirs: &[d("Flat-Remix-Blue-Dark", &["flat-remix-20251119/Flat-Remix-Blue-Dark"])] },
            IconVariant { id: "blue-light-dark-panel", label: tk("Blue Light, dark panel"), theme: "Flat-Remix-Blue-Light-darkPanel", dark: false, dirs: &[d("Flat-Remix-Blue-Light-darkPanel", &["flat-remix-20251119/Flat-Remix-Blue-Light-darkPanel"]), d("Flat-Remix-Blue-Light", &["flat-remix-20251119/Flat-Remix-Blue-Light"]), d("Flat-Remix-Blue-Dark", &["flat-remix-20251119/Flat-Remix-Blue-Dark"])] },
            IconVariant { id: "teal-dark", label: tk("Teal Dark"), theme: "Flat-Remix-Teal-Dark", dark: true, dirs: &[d("Flat-Remix-Teal-Dark", &["flat-remix-20251119/Flat-Remix-Teal-Dark"])] },
            IconVariant { id: "red-light", label: tk("Red Light"), theme: "Flat-Remix-Red-Light", dark: false, dirs: &[d("Flat-Remix-Red-Light", &["flat-remix-20251119/Flat-Remix-Red-Light"]), d("Flat-Remix-Red-Dark", &["flat-remix-20251119/Flat-Remix-Red-Dark"])] },
            IconVariant { id: "grey-dark", label: tk("Grey Dark"), theme: "Flat-Remix-Grey-Dark", dark: true, dirs: &[d("Flat-Remix-Grey-Dark", &["flat-remix-20251119/Flat-Remix-Grey-Dark"])] },
        ],
    },
    IconTheme {
        id: "paper",
        name: "Paper",
        author: "Sam Hewitt",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/snwh/paper-icon-theme",
        description: tk("Flat icons with bold colors and long shadows; no longer maintained"),
        stars: 1587,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x7a929e,
        archive: Asset { url: "https://github.com/snwh/paper-icon-theme/archive/refs/tags/v.1.5.0.tar.gz", sha256: "62f21dfe95ece481e5c635480f32347f1ad27ea66b2ef0526fe799090b298ece", size: 48_464_220 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Paper", dark: false, dirs: &[d("Paper", &["paper-icon-theme-v.1.5.0/Paper"])] },
            IconVariant { id: "mono-dark", label: tk("Mono Dark panel"), theme: "Paper-Mono-Dark", dark: true, dirs: &[d("Paper-Mono-Dark", &["paper-icon-theme-v.1.5.0/Paper-Mono-Dark"]), d("Paper", &["paper-icon-theme-v.1.5.0/Paper"])] },
        ],
    },
    IconTheme {
        id: "candy",
        name: "Candy",
        author: "Eliver Lara",
        license: "GPL-3.0",
        homepage: "https://github.com/EliverLara/candy-icons",
        description: tk("App and folder icons drawn with vivid gradients"),
        stars: 1337,
        style: Style::Colorful,
        tone: Tone::Dark,
        swatch: 0x6bd1cc,
        archive: Asset { url: "https://github.com/EliverLara/candy-icons/archive/750fe2ca7909a721179ec34336042b15808d8ab9.tar.gz", sha256: "3d938b9ed524ca33ecb6448bb57c07db8c1ec99239c61eae228252b74a3e5e03", size: 2_207_816 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "candy-icons", dark: true, dirs: &[d("candy-icons", &["candy-icons-750fe2ca7909a721179ec34336042b15808d8ab9"])] },
        ],
    },
    IconTheme {
        id: "colloid",
        name: "Colloid",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/Colloid-icon-theme",
        description: tk("Clean flat icons with light-blue folders"),
        stars: 1211,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x6ac4f1,
        archive: Asset { url: "https://github.com/vinceliuice/Colloid-icon-theme/archive/refs/tags/2026-08-10.tar.gz", sha256: "367c3ce3ab85721e41ca9aa4fccd1761ec236d1b8cd7daf65f5e9ec88c1a8a15", size: 5_829_734 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Colloid", dark: false, dirs: &[d("Colloid", &["Colloid-icon-theme-2026-08-10/src", "Colloid-icon-theme-2026-08-10/links"])] },
        ],
    },
    IconTheme {
        id: "morewaita",
        name: "MoreWaita",
        author: "somepaulo",
        license: "GPL-3.0",
        homepage: "https://github.com/somepaulo/MoreWaita",
        description: tk("Extra app, folder and file icons that extend the Adwaita set"),
        stars: 1118,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0xa4caee,
        archive: Asset { url: "https://github.com/somepaulo/MoreWaita/archive/refs/tags/v50.1.tar.gz", sha256: "000e858d346e64bfdb87fb305e8046e09f5554385cf9c213fe648887fe218940", size: 11_076_721 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "MoreWaita", dark: false, dirs: &[d("MoreWaita", &["MoreWaita-50.1"])] },
        ],
    },
    IconTheme {
        id: "arc",
        name: "Arc",
        author: "horst3180",
        license: "GPL-3.0",
        homepage: "https://github.com/horst3180/arc-icon-theme",
        description: tk("Folder, file and action icons made to match the Arc look; no longer maintained"),
        stars: 1099,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x6ba4e7,
        archive: Asset { url: "https://github.com/horst3180/arc-icon-theme/archive/refs/tags/20161122.tar.gz", sha256: "0b448dde34138b12ab2d5f224f001cf8203fff55e6b19b013b0461b4376641f9", size: 5_033_333 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Arc", dark: false, dirs: &[d("Arc", &["arc-icon-theme-20161122/Arc"])] },
        ],
    },
    IconTheme {
        id: "gruvbox-gtk",
        name: "Gruvbox",
        author: "Fausto-Korpsvart",
        license: "GPL-3.0",
        homepage: "https://github.com/Fausto-Korpsvart/Gruvbox-GTK-Theme",
        description: tk("Flat icons recolored to the warm Gruvbox palette"),
        stars: 1005,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x83a598,
        archive: Asset { url: "https://github.com/Fausto-Korpsvart/Gruvbox-GTK-Theme/archive/578cd220b5ff6e86b078a6111d26bb20ec8c733f.tar.gz", sha256: "f45da23a6c4123148cf8f78983e391a86c2e2ee1b7a80aa66790f148cb9619bd", size: 64_715_067 },
        variants: &[
            IconVariant { id: "dark", label: tk("Dark"), theme: "Gruvbox-Dark", dark: true, dirs: &[d("Gruvbox-Dark", &["Gruvbox-GTK-Theme-578cd220b5ff6e86b078a6111d26bb20ec8c733f/icons/Gruvbox-Dark"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Gruvbox-Light", dark: false, dirs: &[d("Gruvbox-Light", &["Gruvbox-GTK-Theme-578cd220b5ff6e86b078a6111d26bb20ec8c733f/icons/Gruvbox-Light"])] },
        ],
    },
    IconTheme {
        id: "kora",
        name: "Kora",
        author: "bikass",
        license: "GPL-3.0",
        homepage: "https://github.com/bikass/kora",
        description: tk("Detailed icons with rounded shapes and soft gradients, made for dark desktops"),
        stars: 964,
        style: Style::Round,
        tone: Tone::Dark,
        swatch: 0x1084f8,
        archive: Asset { url: "https://github.com/bikass/kora/archive/refs/tags/v2.0.6.tar.gz", sha256: "159bdb7a09409a12e54a71136ec8889c51b0bf7c145c8b20e3509dc1e90089bd", size: 6_135_730 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "kora", dark: true, dirs: &[d("kora", &["kora-2.0.6/kora"])] },
            IconVariant { id: "grey", label: tk("Grey folders"), theme: "kora-pgrey", dark: true, dirs: &[d("kora-pgrey", &["kora-2.0.6/kora-pgrey"]), d("kora", &["kora-2.0.6/kora"])] },
        ],
    },
    IconTheme {
        id: "qogir",
        name: "Qogir",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/Qogir-icon-theme",
        description: tk("Flat colorful icons with blue folders"),
        stars: 945,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x6ba4e7,
        archive: Asset { url: "https://github.com/vinceliuice/Qogir-icon-theme/archive/refs/tags/2025-02-15.tar.gz", sha256: "b0d07cad5601e0341a53a62df0ed111823b75fc38741d435486620a59fb239ee", size: 9_076_276 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Qogir", dark: false, dirs: &[d("Qogir", &["Qogir-icon-theme-2025-02-15/src", "Qogir-icon-theme-2025-02-15/links"])] },
        ],
    },
    IconTheme {
        id: "luv",
        name: "Lüv",
        author: "Nitrux",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/Nitrux/luv-icon-theme",
        description: tk("Minimal flat icons with soft pastel colors"),
        stars: 929,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x4cd0e1,
        archive: Asset { url: "https://github.com/Nitrux/luv-icon-theme/archive/eb2603fd56dd71d4d9bd3eab397864f6b5da60b2.tar.gz", sha256: "3efb2ade5e3b09e02026ddf0daf0578cfb56ba3d86ccb6379c38fd47f3c5366e", size: 3_378_994 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Luv", dark: false, dirs: &[d("Luv", &["luv-icon-theme-eb2603fd56dd71d4d9bd3eab397864f6b5da60b2/Luv"])] },
        ],
    },
    IconTheme {
        id: "gruvbox-plus",
        name: "Gruvbox Plus",
        author: "Syl Eleuth",
        license: "GPL-3.0",
        homepage: "https://github.com/SylEleuth/gruvbox-plus-icon-pack",
        description: tk("Large icon set in the Gruvbox palette, with dark and light variants"),
        stars: 831,
        style: Style::Colorful,
        tone: Tone::Both,
        swatch: 0x83a598,
        archive: Asset { url: "https://github.com/SylEleuth/gruvbox-plus-icon-pack/releases/download/v6.7.0/gruvbox-plus-icon-pack-6.7.0.zip", sha256: "107ae9cadc473f81aaea88e06a24bcb8cacd41af3bb75d81d0d5e19f0babd7d9", size: 49_554_847 },
        variants: &[
            IconVariant { id: "dark", label: tk("Dark"), theme: "Gruvbox-Plus-Dark", dark: true, dirs: &[d("Gruvbox-Plus-Dark", &["Gruvbox-Plus-Dark"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Gruvbox-Plus-Light", dark: false, dirs: &[d("Gruvbox-Plus-Light", &["Gruvbox-Plus-Light"]), d("Gruvbox-Plus-Dark", &["Gruvbox-Plus-Dark"])] },
        ],
    },
    IconTheme {
        id: "tokyonight-gtk",
        name: "Tokyo Night",
        author: "Fausto-Korpsvart",
        license: "GPL-3.0",
        homepage: "https://github.com/Fausto-Korpsvart/Tokyonight-GTK-Theme",
        description: tk("Flat icons recolored to the Tokyo Night palette"),
        stars: 824,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x7aa2f7,
        archive: Asset { url: "https://github.com/Fausto-Korpsvart/Tokyonight-GTK-Theme/archive/6c340e058e84c1975a038a8e5d1e384477225dc0.tar.gz", sha256: "ecb39f8ca86a0e460a8bde52173a21f656f2740fc17bf393a21411c17378fd6c", size: 51_540_468 },
        variants: &[
            IconVariant { id: "dark", label: tk("Dark"), theme: "Tokyonight-Dark", dark: true, dirs: &[d("Tokyonight-Dark", &["Tokyonight-GTK-Theme-6c340e058e84c1975a038a8e5d1e384477225dc0/icons/Tokyonight-Dark"])] },
            IconVariant { id: "moon", label: tk("Moon"), theme: "Tokyonight-Moon", dark: true, dirs: &[d("Tokyonight-Moon", &["Tokyonight-GTK-Theme-6c340e058e84c1975a038a8e5d1e384477225dc0/icons/Tokyonight-Moon"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Tokyonight-Light", dark: false, dirs: &[d("Tokyonight-Light", &["Tokyonight-GTK-Theme-6c340e058e84c1975a038a8e5d1e384477225dc0/icons/Tokyonight-Light"])] },
        ],
    },
    IconTheme {
        id: "numix",
        name: "Numix",
        author: "Numix Project",
        license: "GPL-3.0",
        homepage: "https://github.com/numixproject/numix-icon-theme",
        description: tk("Flat icons with orange-accented folders and a light panel option"),
        stars: 803,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0xf2bb64,
        archive: Asset { url: "https://github.com/numixproject/numix-icon-theme/archive/refs/tags/25.12.15.tar.gz", sha256: "20db3921eecb024139dbf0c20b32101b48f464a56c5072cb2fd7d41581e1816d", size: 4_140_723 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Numix", dark: true, dirs: &[d("Numix", &["numix-icon-theme-25.12.15/Numix"])] },
            IconVariant { id: "light", label: tk("Light panel"), theme: "Numix-Light", dark: false, dirs: &[d("Numix-Light", &["numix-icon-theme-25.12.15/Numix-Light"]), d("Numix", &["numix-icon-theme-25.12.15/Numix"])] },
        ],
    },
    IconTheme {
        id: "catppuccin-gtk",
        name: "Catppuccin",
        author: "Fausto-Korpsvart",
        license: "GPL-3.0",
        homepage: "https://github.com/Fausto-Korpsvart/Catppuccin-GTK-Theme",
        description: tk("Flat icons recolored to the four Catppuccin flavours"),
        stars: 793,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0xcba6f7,
        archive: Asset { url: "https://github.com/Fausto-Korpsvart/Catppuccin-GTK-Theme/archive/refs/tags/v1.0.1.tar.gz", sha256: "0845cec40975faeab408efe45becbe31519380884df4a22277539b7af15bfd22", size: 105_841_071 },
        variants: &[
            IconVariant { id: "mocha", label: tk("Mocha"), theme: "Catppuccin-Mocha", dark: true, dirs: &[d("Catppuccin-Mocha", &["Catppuccin-GTK-Theme-1.0.1/icons/Catppuccin-Mocha"])] },
            IconVariant { id: "macchiato", label: tk("Macchiato"), theme: "Catppuccin-Macchiato", dark: true, dirs: &[d("Catppuccin-Macchiato", &["Catppuccin-GTK-Theme-1.0.1/icons/Catppuccin-Macchiato"])] },
            IconVariant { id: "frappe", label: tk("Frappé"), theme: "Catppuccin-Frappe", dark: true, dirs: &[d("Catppuccin-Frappe", &["Catppuccin-GTK-Theme-1.0.1/icons/Catppuccin-Frappe"])] },
            IconVariant { id: "latte", label: tk("Latte"), theme: "Catppuccin-Latte", dark: false, dirs: &[d("Catppuccin-Latte", &["Catppuccin-GTK-Theme-1.0.1/icons/Catppuccin-Latte"])] },
        ],
    },
    IconTheme {
        id: "reversal",
        name: "Reversal",
        author: "yeyushengfan258",
        license: "GPL-3.0",
        homepage: "https://github.com/yeyushengfan258/Reversal-icon-theme",
        description: tk("Colorful rounded-rectangle app icons"),
        stars: 730,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0xffc841,
        archive: Asset { url: "https://github.com/yeyushengfan258/Reversal-icon-theme/archive/2c8122287e3bad5a6bf860ddd8c7e3b78cf4d451.tar.gz", sha256: "3053cef3034a424be0dc5a81a70550c6f9653e8839b1a1bdc13fbd19fb11c344", size: 7_961_875 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Reversal", dark: false, dirs: &[d("Reversal", &["Reversal-icon-theme-2c8122287e3bad5a6bf860ddd8c7e3b78cf4d451/src", "Reversal-icon-theme-2c8122287e3bad5a6bf860ddd8c7e3b78cf4d451/links"])] },
        ],
    },
    IconTheme {
        id: "mactahoe",
        name: "MacTahoe",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/MacTahoe-icon-theme",
        description: tk("Rounded-square app icons in a macOS Tahoe style"),
        stars: 539,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x6db6e2,
        archive: Asset { url: "https://github.com/vinceliuice/MacTahoe-icon-theme/archive/refs/tags/2026-09-10.tar.gz", sha256: "6330369e9e10a28cfc8da598ebf63a7204be705403519963ff84e1cb84719d35", size: 9_828_248 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "MacTahoe", dark: false, dirs: &[d("MacTahoe", &["MacTahoe-icon-theme-2026-09-10/src", "MacTahoe-icon-theme-2026-09-10/links"])] },
        ],
    },
    IconTheme {
        id: "suru",
        name: "Suru",
        author: "Sam Hewitt",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/snwh/suru-icon-theme",
        description: tk("Flat icons with a paper-cut look; no longer maintained"),
        stars: 476,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0xfc9856,
        archive: Asset { url: "https://github.com/snwh/suru-icon-theme/archive/2d8102084eaf194f04076ec6949feacb0eb4a1ba.tar.gz", sha256: "3cb6d4b296737aa84f6d74e3edbb98d098bcbf2ca3e62717a21a4519324589a3", size: 25_989_005 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Suru", dark: false, dirs: &[d("Suru", &["suru-icon-theme-2d8102084eaf194f04076ec6949feacb0eb4a1ba/Suru"])] },
        ],
    },
    IconTheme {
        id: "moka",
        name: "Moka",
        author: "Sam Hewitt",
        license: "GPL-3.0 OR CC-BY-SA-4.0",
        homepage: "https://github.com/snwh/moka-icon-theme",
        description: tk("Round flat app icons with soft colors; no longer maintained"),
        stars: 467,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x3e51ca,
        archive: Asset { url: "https://github.com/snwh/moka-icon-theme/archive/refs/tags/v5.4.0.tar.gz", sha256: "b32e82aea4358e087620e852aa20042e791aa957a93de568da1d2223a46c7cd9", size: 67_442_220 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Moka", dark: false, dirs: &[d("Moka", &["moka-icon-theme-5.4.0/Moka"])] },
        ],
    },
    IconTheme {
        id: "zafiro",
        name: "Zafiro",
        author: "zayronxio",
        license: "GPL-3.0",
        homepage: "https://github.com/zayronxio/Zafiro-icons",
        description: tk("Flat icons with light washed colors"),
        stars: 465,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x6f8a91,
        archive: Asset { url: "https://github.com/zayronxio/Zafiro-icons/archive/refs/tags/1.3.tar.gz", sha256: "d4e81453d050ae07b157c65e388e748639a711afc1a6d1b033bdfdd90230ecb7", size: 18_400_435 },
        variants: &[
            IconVariant { id: "light", label: tk("Light"), theme: "Zafiro-icons-Light", dark: false, dirs: &[d("Zafiro-icons-Light", &["Zafiro-icons-1.3/Light"])] },
            IconVariant { id: "dark", label: tk("Dark"), theme: "Zafiro-icons-Dark", dark: true, dirs: &[d("Zafiro-icons-Dark", &["Zafiro-icons-1.3/Dark"])] },
        ],
    },
    IconTheme {
        id: "mcmojave-circle",
        name: "McMojave Circle",
        author: "Vince Liuice",
        license: "GPL-3.0",
        homepage: "https://github.com/vinceliuice/McMojave-circle",
        description: tk("Circular app icons with macOS Mojave style folders"),
        stars: 453,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x4ed1fc,
        archive: Asset { url: "https://github.com/vinceliuice/McMojave-circle/archive/refs/tags/2025-08-07.tar.gz", sha256: "409506b5ce2905c9b5a1cce109a7f1e6d0c76fdb012ad858d482a47c476e4ee6", size: 5_905_516 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "McMojave-circle", dark: false, dirs: &[d("McMojave-circle", &["McMojave-circle-2025-08-07/src", "McMojave-circle-2025-08-07/links"])] },
        ],
    },
    IconTheme {
        id: "nordzy",
        name: "Nordzy",
        author: "Molasses",
        license: "GPL-3.0",
        homepage: "https://github.com/MolassesLover/Nordzy-icon",
        description: tk("Icons recolored to the cool Nord palette"),
        stars: 430,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x7c9ab9,
        archive: Asset { url: "https://github.com/MolassesLover/Nordzy-icon/archive/refs/tags/1.8.7.tar.gz", sha256: "e3bdef7ce74bcbebc0e5c74481bc4d6ead022cee31b99aeb3e54fc26a5ef9bb5", size: 9_831_505 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Nordzy", dark: false, dirs: &[d("Nordzy", &["Nordzy-icon-1.8.7/src", "Nordzy-icon-1.8.7/links"])] },
        ],
    },
    IconTheme {
        id: "everforest-gtk",
        name: "Everforest",
        author: "Fausto-Korpsvart",
        license: "GPL-3.0",
        homepage: "https://github.com/Fausto-Korpsvart/Everforest-GTK-Theme",
        description: tk("Flat icons recolored to the green Everforest palette"),
        stars: 396,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x9da9a0,
        archive: Asset { url: "https://github.com/Fausto-Korpsvart/Everforest-GTK-Theme/archive/9b8be4d6648ae9eaae3dd550105081f8c9054825.tar.gz", sha256: "20966e9f5008efc7b23e44ca4ad1aec5e173cafdf120b9063b3687a1970500e0", size: 29_083_363 },
        variants: &[
            IconVariant { id: "dark", label: tk("Dark"), theme: "Everforest-Dark", dark: true, dirs: &[d("Everforest-Dark", &["Everforest-GTK-Theme-9b8be4d6648ae9eaae3dd550105081f8c9054825/icons/Everforest-Dark"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Everforest-Light", dark: false, dirs: &[d("Everforest-Light", &["Everforest-GTK-Theme-9b8be4d6648ae9eaae3dd550105081f8c9054825/icons/Everforest-Light"])] },
        ],
    },
    IconTheme {
        id: "suru-plus",
        name: "Suru++",
        author: "Suru++ Project",
        license: "GPL-3.0",
        homepage: "https://github.com/suru-plus/suru-plus",
        description: tk("Flat icons based on Suru, with many app icons and symbolic sets"),
        stars: 396,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x43abe7,
        archive: Asset { url: "https://github.com/suru-plus/suru-plus/archive/refs/tags/v30.0.tar.gz", sha256: "f329c1b1960838fe28bd8f905e06273d9a04826322d46ca91cdc051548ce4cd0", size: 14_206_182 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Suru++", dark: false, dirs: &[d("Suru++", &["suru-plus-30.0/Suru++"])] },
            IconVariant { id: "light", label: tk("Light panel"), theme: "Suru++-Light", dark: false, dirs: &[d("Suru++-Light", &["suru-plus-30.0/Suru++-Light"]), d("Suru++", &["suru-plus-30.0/Suru++"])] },
            IconVariant { id: "elementary", label: tk("eSuru++"), theme: "eSuru++", dark: false, dirs: &[d("eSuru++", &["suru-plus-30.0/eSuru++"]), d("Suru++", &["suru-plus-30.0/Suru++"])] },
        ],
    },
    IconTheme {
        id: "win11",
        name: "Win11",
        author: "yeyushengfan258",
        license: "GPL-3.0",
        homepage: "https://github.com/yeyushengfan258/Win11-icon-theme",
        description: tk("Colorful icons in a Windows 11 style"),
        stars: 392,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0xfde083,
        archive: Asset { url: "https://github.com/yeyushengfan258/Win11-icon-theme/archive/a5b460a407da143b32f19a503d7fcebb3edf2371.tar.gz", sha256: "358479b3113a96604eecf6a94ee099f72e8d46431242c7766c80c51db8bc981a", size: 9_208_319 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Win11", dark: false, dirs: &[d("Win11", &["Win11-icon-theme-a5b460a407da143b32f19a503d7fcebb3edf2371/src", "Win11-icon-theme-a5b460a407da143b32f19a503d7fcebb3edf2371/links"])] },
        ],
    },
    IconTheme {
        id: "elementary-plus",
        name: "elementary+",
        author: "Manuel Kehl",
        license: "GPL-3.0",
        homepage: "https://github.com/Manuel-Kehl/elementaryPlus",
        description: tk("Extra third-party app icons that extend the elementary set; no longer maintained"),
        stars: 357,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0x82573c,
        archive: Asset { url: "https://github.com/Manuel-Kehl/elementaryPlus/archive/2cb840bd7b1ab3d36e24660dba8b1f647f53cec3.tar.gz", sha256: "e29fb7e442b9d9070dd5297faf0c6dac71a7922b9abfcffdb3f00b49b784cbb5", size: 2_294_251 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "elementaryPlus", dark: false, dirs: &[d("elementaryPlus", &["elementaryPlus-2cb840bd7b1ab3d36e24660dba8b1f647f53cec3/elementaryPlus"])] },
        ],
    },
    IconTheme {
        id: "se98",
        name: "SE98",
        author: "Evgeny Dobromilsky",
        license: "GPL-2.0",
        homepage: "https://github.com/nestoris/Win98SE",
        description: tk("Pixel icons in the style of Windows 98 Second Edition"),
        stars: 351,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0xfffb99,
        archive: Asset { url: "https://github.com/nestoris/Win98SE/releases/download/v0.2.17/SE98.tar.bz2", sha256: "ab66ab6690f038abba1fcce1222b82525b9e5d135eafc2ea3c24b942ad05add6", size: 10_790_495 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "SE98", dark: false, dirs: &[d("SE98", &["SE98"])] },
        ],
    },
    IconTheme {
        id: "kanagawa-gtk",
        name: "Kanagawa",
        author: "Fausto-Korpsvart",
        license: "GPL-3.0",
        homepage: "https://github.com/Fausto-Korpsvart/Kanagawa-GKT-Theme",
        description: tk("Flat icons recolored to the Kanagawa palette"),
        stars: 315,
        style: Style::Flat,
        tone: Tone::Dark,
        swatch: 0x717c7c,
        archive: Asset { url: "https://github.com/Fausto-Korpsvart/Kanagawa-GKT-Theme/archive/55ca4ba249eba21f861b9866b71ab41bb8930318.tar.gz", sha256: "9c176f36fccc8383b156048205db0714a18b5f96f1c683d4496940802041ca5f", size: 16_169_974 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Kanagawa", dark: true, dirs: &[d("Kanagawa", &["Kanagawa-GKT-Theme-55ca4ba249eba21f861b9866b71ab41bb8930318/icons/Kanagawa"])] },
        ],
    },
    IconTheme {
        id: "oranchelo",
        name: "Oranchelo",
        author: "Oranchelo Team",
        license: "GPL-3.0",
        homepage: "https://github.com/OrancheloTeam/oranchelo-icon-theme",
        description: tk("Flat icons with orange folders; no longer maintained"),
        stars: 304,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x3771c8,
        archive: Asset { url: "https://github.com/OrancheloTeam/oranchelo-icon-theme/archive/refs/tags/v0.9.0.tar.gz", sha256: "8d15f8e16772d3cb585d1dd7334ff5a5c0fa6ba59d737c165d707281b4b4f969", size: 7_843_283 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Oranchelo", dark: true, dirs: &[d("Oranchelo", &["oranchelo-icon-theme-0.9.0/Oranchelo"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "oranchelo-light", dark: false, dirs: &[d("oranchelo-light", &["oranchelo-icon-theme-0.9.0/oranchelo-light"]), d("Oranchelo", &["oranchelo-icon-theme-0.9.0/Oranchelo"])] },
        ],
    },
    IconTheme {
        id: "elementary-xfce",
        name: "elementary Xfce",
        author: "Shimmer Project",
        license: "GPL-3.0",
        homepage: "https://github.com/shimmerproject/elementary-xfce",
        description: tk("Detailed icons based on the elementary set, extended with many extra icons"),
        stars: 302,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0xe9cc9e,
        archive: Asset { url: "https://github.com/shimmerproject/elementary-xfce/archive/refs/tags/v0.23.tar.gz", sha256: "2f4e6b6e4e46692c18c2f41cf7b7c9a2bac4433d5528e98c8a301631c5b285d2", size: 7_446_213 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "elementary-xfce", dark: false, dirs: &[d("elementary-xfce", &["elementary-xfce-0.23/elementary-xfce"])] },
        ],
    },
    IconTheme {
        id: "we10x",
        name: "We10X",
        author: "yeyushengfan258",
        license: "GPL-3.0",
        homepage: "https://github.com/yeyushengfan258/We10X-icon-theme",
        description: tk("Colorful icons in a Windows 10 style"),
        stars: 278,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0xffd354,
        archive: Asset { url: "https://github.com/yeyushengfan258/We10X-icon-theme/archive/0f52ff2dce554146f8f76ab2c7a5968fd773400d.tar.gz", sha256: "77a103035d9628423d3ca080e09eb73b262bdc1fe51bfe34ae8b86990109f91c", size: 9_261_429 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "We10X", dark: false, dirs: &[d("We10X", &["We10X-icon-theme-0f52ff2dce554146f8f76ab2c7a5968fd773400d/src", "We10X-icon-theme-0f52ff2dce554146f8f76ab2c7a5968fd773400d/links"])] },
        ],
    },
    IconTheme {
        id: "conflux",
        name: "Conflux",
        author: "Moshiur Rahman Adib",
        license: "GPL-3.0",
        homepage: "https://github.com/MoshiurRahmanAdib/Conflux-Icon-Theme",
        description: tk("Neutral app icons drawn in one consistent rounded shape"),
        stars: 266,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x6db6e2,
        archive: Asset { url: "https://github.com/MoshiurRahmanAdib/Conflux-Icon-Theme/archive/c9a7f7e313e735c96f7f6a11462c2e1cc96572d8.tar.gz", sha256: "ddea3bab2f8a9d646f126332dc18e40bfe32a411d675e189933f87d5673cc4e4", size: 2_231_489 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Conflux", dark: false, dirs: &[d("Conflux", &["Conflux-Icon-Theme-c9a7f7e313e735c96f7f6a11462c2e1cc96572d8"])] },
        ],
    },
    IconTheme {
        id: "flatwoken",
        name: "FlatWoken",
        author: "Alessandro Roncone",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/alecive/FlatWoken",
        description: tk("Round flat app icons with long shadows; no longer maintained"),
        stars: 265,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0xcba366,
        archive: Asset { url: "https://github.com/alecive/FlatWoken/archive/aa7b4a4b9d63a96afab44cecbf89e1795a79504d.tar.gz", sha256: "c29f8e94ac7235456eeaefa35f8d8a4767c07805633356da5088beec0119291e", size: 9_587_626 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "FlatWoken", dark: false, dirs: &[d("FlatWoken", &["FlatWoken-aa7b4a4b9d63a96afab44cecbf89e1795a79504d/FlatWoken"])] },
        ],
    },
    IconTheme {
        id: "hatter",
        name: "Hatter",
        author: "Waarisbal",
        license: "GPL-3.0",
        homepage: "https://github.com/Mibea/Hatter",
        description: tk("Rounded-square app icons that keep each app's own colors"),
        stars: 260,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x9ac3ee,
        archive: Asset { url: "https://github.com/Mibea/Hatter/archive/refs/tags/V4.0.tar.gz", sha256: "74fae86c677809198497e735fd3a50c4ff192327bec4ce7bbe35c6844cfaf5fa", size: 100_267_190 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Hatter", dark: false, dirs: &[d("Hatter", &["Hatter-4.0/Hatter"])] },
        ],
    },
    IconTheme {
        id: "pixora",
        name: "Pixora",
        author: "Cesar Neiva",
        license: "CC-BY-4.0",
        homepage: "https://github.com/tsora1603/pixora-icons",
        description: tk("16-bit pixel art icons"),
        stars: 252,
        style: Style::Colorful,
        tone: Tone::Both,
        swatch: 0xe3c896,
        archive: Asset { url: "https://github.com/tsora1603/pixora-icons/archive/ea994cfa6dc9da1a468405d6bcb6aeffa80aa52e.tar.gz", sha256: "b374567fb70db9cb524d0251bdd3ac737fe91546ce38c111eccd6b2850407e19", size: 658_524 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "pixora", dark: false, dirs: &[d("pixora", &["pixora-icons-ea994cfa6dc9da1a468405d6bcb6aeffa80aa52e/pixora"])] },
            IconVariant { id: "dark", label: tk("Dark"), theme: "pixora-dark", dark: true, dirs: &[d("pixora-dark", &["pixora-icons-ea994cfa6dc9da1a468405d6bcb6aeffa80aa52e/pixora-dark"]), d("pixora", &["pixora-icons-ea994cfa6dc9da1a468405d6bcb6aeffa80aa52e/pixora"])] },
            IconVariant { id: "accent", label: tk("Accent folders"), theme: "pixora-accent", dark: false, dirs: &[d("pixora-accent", &["pixora-icons-ea994cfa6dc9da1a468405d6bcb6aeffa80aa52e/pixora-accent"]), d("pixora", &["pixora-icons-ea994cfa6dc9da1a468405d6bcb6aeffa80aa52e/pixora"])] },
        ],
    },
    IconTheme {
        id: "obsidian",
        name: "Obsidian",
        author: "Marcus",
        license: "GPL-3.0",
        homepage: "https://github.com/madmaxms/iconpack-obsidian",
        description: tk("Glossy detailed icons based on Faenza, made for dark themes"),
        stars: 232,
        style: Style::Colorful,
        tone: Tone::Both,
        swatch: 0x3f96e7,
        archive: Asset { url: "https://github.com/madmaxms/iconpack-obsidian/releases/download/v4.15/obsidian-iconpack.tar.xz", sha256: "01ba573694c95889d8b4ed16bbe21cb4098e14d02ce4e73063529780f67992c7", size: 28_388_220 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Obsidian", dark: true, dirs: &[d("Obsidian", &["Obsidian"])] },
            IconVariant { id: "semi-light", label: tk("Semi-light"), theme: "Obsidian-SemiLight", dark: false, dirs: &[d("Obsidian-SemiLight", &["Obsidian-SemiLight"]), d("Obsidian", &["Obsidian"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Obsidian-Light", dark: false, dirs: &[d("Obsidian-Light", &["Obsidian-Light"]), d("Obsidian-SemiLight", &["Obsidian-SemiLight"]), d("Obsidian", &["Obsidian"])] },
            IconVariant { id: "aqua", label: tk("Aqua folders"), theme: "Obsidian-Aqua", dark: true, dirs: &[d("Obsidian-Aqua", &["Obsidian-Aqua"]), d("Obsidian", &["Obsidian"])] },
        ],
    },
    IconTheme {
        id: "rose-pine-gtk",
        name: "Rosé Pine",
        author: "Fausto-Korpsvart",
        license: "GPL-3.0",
        homepage: "https://github.com/Fausto-Korpsvart/Rose-Pine-GTK-Theme",
        description: tk("Flat icons recolored to the muted Rosé Pine palette"),
        stars: 222,
        style: Style::Flat,
        tone: Tone::Dark,
        swatch: 0x31748f,
        archive: Asset { url: "https://github.com/Fausto-Korpsvart/Rose-Pine-GTK-Theme/archive/c4fdfa62a9eb6941a36b2cd5026fc64123aaa0dd.tar.gz", sha256: "9410203a7c2d4362c5e8696279e9c9d7b7925b97fd1cb5b1d0890fd6f99ebc58", size: 27_384_410 },
        variants: &[
            IconVariant { id: "main", label: tk("Main"), theme: "Rose-Pine", dark: true, dirs: &[d("Rose-Pine", &["Rose-Pine-GTK-Theme-c4fdfa62a9eb6941a36b2cd5026fc64123aaa0dd/icons/Rose-Pine"])] },
            IconVariant { id: "moon", label: tk("Moon"), theme: "Rose-Pine-Moon", dark: true, dirs: &[d("Rose-Pine-Moon", &["Rose-Pine-GTK-Theme-c4fdfa62a9eb6941a36b2cd5026fc64123aaa0dd/icons/Rose-Pine-Moon"])] },
        ],
    },
    IconTheme {
        id: "pop",
        name: "Pop",
        author: "Pop!_OS",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/pop-os/icon-theme",
        description: tk("Flat icons with teal accents, based on Paper"),
        stars: 221,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0x48b9c7,
        archive: Asset { url: "https://github.com/pop-os/icon-theme/archive/refs/tags/v3.5.0.tar.gz", sha256: "917f5e300e927fe2557355717793f3284aee869a6ae0b81cd31507f20090d796", size: 23_658_746 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Pop", dark: false, dirs: &[d("Pop", &["icon-theme-3.5.0/Pop"])] },
            IconVariant { id: "dark", label: tk("Mono Dark panel"), theme: "Pop-Dark", dark: true, dirs: &[d("Pop-Dark", &["icon-theme-3.5.0/Pop-Dark"]), d("Pop", &["icon-theme-3.5.0/Pop"])] },
        ],
    },
    IconTheme {
        id: "adwair",
        name: "Adwair",
        author: "kusaida",
        license: "GPL-3.0",
        homepage: "https://github.com/kusaida/Adwair",
        description: tk("Rounded app icons that blend a Big Sur look with Adwaita"),
        stars: 203,
        style: Style::Round,
        tone: Tone::Both,
        swatch: 0x93c0ea,
        archive: Asset { url: "https://github.com/kusaida/Adwair/releases/download/v1.0.0/Adwair.zip", sha256: "b33e40806b3cace4ecd8a82d2e57ee3f9385c6c73882800ee34d9e8cedcc9d5e", size: 40_190_463 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Adwair", dark: false, dirs: &[d("Adwair", &["Adwair/Adwair"])] },
            IconVariant { id: "dark", label: tk("Dark"), theme: "Adwair-dark", dark: true, dirs: &[d("Adwair-dark", &["Adwair/Adwair-dark"]), d("Adwair", &["Adwair/Adwair"])] },
        ],
    },
    IconTheme {
        id: "adwaita-plus",
        name: "Adwaita++",
        author: "Andrea Bonanni",
        license: "GPL-3.0",
        homepage: "https://github.com/Bonandry/adwaita-plus",
        description: tk("Adwaita-style icons with many extra app icons; no longer maintained"),
        stars: 178,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0xa89888,
        archive: Asset { url: "https://github.com/Bonandry/adwaita-plus/archive/refs/tags/v6.1.tar.gz", sha256: "f8c791754110b7b9c19f50fb19f5344ad0a50e5834ca25a1161312eacfbf027b", size: 8_274_025 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Adwaita++", dark: false, dirs: &[d("Adwaita++", &["adwaita-plus-6.1/Adwaita++"])] },
            IconVariant { id: "dark", label: tk("Dark"), theme: "Adwaita++-Dark", dark: true, dirs: &[d("Adwaita++-Dark", &["adwaita-plus-6.1/Adwaita++-Dark"]), d("Adwaita++", &["adwaita-plus-6.1/Adwaita++"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "Adwaita++-Light", dark: false, dirs: &[d("Adwaita++-Light", &["adwaita-plus-6.1/Adwaita++-Light"]), d("Adwaita++", &["adwaita-plus-6.1/Adwaita++"])] },
            IconVariant { id: "colorful", label: tk("Colorful"), theme: "Adwaita++-Colorful", dark: false, dirs: &[d("Adwaita++-Colorful", &["adwaita-plus-6.1/Adwaita++-Colorful"]), d("Adwaita++", &["adwaita-plus-6.1/Adwaita++"])] },
            IconVariant { id: "dark-colorful", label: tk("Dark colorful"), theme: "Adwaita++-Dark-Colorful", dark: true, dirs: &[d("Adwaita++-Dark-Colorful", &["adwaita-plus-6.1/Adwaita++-Dark-Colorful"]), d("Adwaita++-Dark", &["adwaita-plus-6.1/Adwaita++-Dark"]), d("Adwaita++", &["adwaita-plus-6.1/Adwaita++"])] },
        ],
    },
    IconTheme {
        id: "vertex",
        name: "Vertex",
        author: "horst3180",
        license: "GPL-3.0",
        homepage: "https://github.com/horst3180/vertex-icons",
        description: tk("Folder and file-type icons that pair with the Vertex look; no app icons"),
        stars: 176,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0xe8e9eb,
        archive: Asset { url: "https://github.com/horst3180/vertex-icons/archive/f27e47edf392596b7967b7d134d3c62ac3fda0c9.tar.gz", sha256: "ef297f04497efa4665e9bd53ced04177b24d7fa32c8422abe649ec0ea789524d", size: 5_578_183 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Vertex-Icons", dark: false, dirs: &[d("Vertex-Icons", &["vertex-icons-f27e47edf392596b7967b7d134d3c62ac3fda0c9"])] },
        ],
    },
    IconTheme {
        id: "windows-xp",
        name: "Windows XP",
        author: "B00merang Artwork",
        license: "GPL-2.0",
        homepage: "https://github.com/B00merang-Artwork/Windows-XP",
        description: tk("Icons in the style of Windows XP"),
        stars: 175,
        style: Style::Colorful,
        tone: Tone::Light,
        swatch: 0xfffd99,
        archive: Asset { url: "https://github.com/B00merang-Artwork/Windows-XP/archive/refs/tags/3.1.tar.gz", sha256: "ba6ab8fc60cd89101d8302b7f392f2aac715582da31aa370da5d569ad6e29d66", size: 5_947_147 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Windows-XP", dark: false, dirs: &[d("Windows-XP", &["Windows-XP-3.1"])] },
        ],
    },
    IconTheme {
        id: "mint-y",
        name: "Mint-Y",
        author: "Linux Mint",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/linuxmint/mint-y-icons",
        description: tk("Flat icons with a range of folder colors, from Linux Mint"),
        stars: 173,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x50c26f,
        archive: Asset { url: "https://github.com/linuxmint/mint-y-icons/archive/refs/tags/1.9.8.tar.gz", sha256: "911185c178842425b924d948f2c086d50006785a3fe0ade02e1f9ccf47987145", size: 92_910_199 },
        variants: &[
            IconVariant { id: "default", label: tk("Green"), theme: "Mint-Y", dark: false, dirs: &[d("Mint-Y", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y"])] },
            IconVariant { id: "blue", label: tk("Blue"), theme: "Mint-Y-Blue", dark: false, dirs: &[d("Mint-Y-Blue", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y-Blue"]), d("Mint-Y", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y"])] },
            IconVariant { id: "aqua", label: tk("Aqua"), theme: "Mint-Y-Aqua", dark: false, dirs: &[d("Mint-Y-Aqua", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y-Aqua"]), d("Mint-Y", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y"])] },
            IconVariant { id: "orange", label: tk("Orange"), theme: "Mint-Y-Orange", dark: false, dirs: &[d("Mint-Y-Orange", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y-Orange"]), d("Mint-Y", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y"])] },
            IconVariant { id: "purple", label: tk("Purple"), theme: "Mint-Y-Purple", dark: false, dirs: &[d("Mint-Y-Purple", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y-Purple"]), d("Mint-Y", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y"])] },
            IconVariant { id: "grey", label: tk("Grey"), theme: "Mint-Y-Grey", dark: false, dirs: &[d("Mint-Y-Grey", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y-Grey"]), d("Mint-Y", &["mint-y-icons-1.9.8/usr/share/icons/Mint-Y"])] },
        ],
    },
    IconTheme {
        id: "boston",
        name: "Boston",
        author: "Christian Díaz",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/thecheis/Boston-Icons",
        description: tk("Minimal icons built from simple geometric shapes"),
        stars: 165,
        style: Style::Flat,
        tone: Tone::Light,
        swatch: 0x6fb7ff,
        archive: Asset { url: "https://github.com/thecheis/Boston-Icons/archive/59382efe4838e87ee4df2c3a92e721569c694c1a.tar.gz", sha256: "a89299a3ba99fb7e4e94deb84ae5e621a1ed5c3c7a0bed88c512ec8259a2bf64", size: 7_049_790 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Boston", dark: false, dirs: &[d("Boston", &["Boston-Icons-59382efe4838e87ee4df2c3a92e721569c694c1a"])] },
        ],
    },
    IconTheme {
        id: "arashi",
        name: "Arashi",
        author: "Chloe",
        license: "CC-BY-SA-4.0",
        homepage: "https://github.com/0hStormy/Arashi",
        description: tk("Smooth rounded app and folder icons"),
        stars: 162,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x6b9fe3,
        archive: Asset { url: "https://github.com/0hStormy/Arashi/releases/download/25.10/Arashi-25.10.zip", sha256: "39386145008857c88b369688a5f03d0da0a0c85c8634c344e8ccb22ede80a4a2", size: 3_805_154 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Arashi", dark: false, dirs: &[d("Arashi", &["Arashi-25.10"])] },
        ],
    },
    IconTheme {
        id: "mojave-ct",
        name: "Mojave CT",
        author: "zayronxio",
        license: "GPL-3.0",
        homepage: "https://github.com/zayronxio/Mojave-CT",
        description: tk("Icons inspired by macOS Mojave"),
        stars: 156,
        style: Style::Round,
        tone: Tone::Light,
        swatch: 0x7fd6f9,
        archive: Asset { url: "https://github.com/zayronxio/Mojave-CT/archive/refs/tags/0.9.9.tar.gz", sha256: "bc069f6015960603c6125c39bbd07711cbd70e7ff934d86633c56ae4b6c60190", size: 19_598_913 },
        variants: &[
            IconVariant { id: "default", label: tk("Default"), theme: "Mojave-CT", dark: false, dirs: &[d("Mojave-CT", &["Mojave-CT-0.9.9"])] },
        ],
    },
    IconTheme {
        id: "pixelitos",
        name: "Pixelitos",
        author: "Zariep",
        license: "MIT",
        homepage: "https://github.com/ItsZariep/pixelitos-icon-theme",
        description: tk("16-bit style pixel icons in dark and light variants"),
        stars: 153,
        style: Style::Colorful,
        tone: Tone::Both,
        swatch: 0x91a666,
        archive: Asset { url: "https://github.com/ItsZariep/pixelitos-icon-theme/releases/download/20261026/pixelitos-icon-theme.tar.gz", sha256: "2ba74b1d3d9583650fda30939ff7e464fad7f983fca9c43eca08c817f8872ebb", size: 369_692 },
        variants: &[
            IconVariant { id: "dark", label: tk("Dark"), theme: "pixelitos-dark", dark: true, dirs: &[d("pixelitos-dark", &["pixelitos-icon-theme/pixelitos-dark"])] },
            IconVariant { id: "light", label: tk("Light"), theme: "pixelitos-light", dark: false, dirs: &[d("pixelitos-light", &["pixelitos-icon-theme/pixelitos-light"]), d("pixelitos-dark", &["pixelitos-icon-theme/pixelitos-dark"])] },
        ],
    },
];

pub fn find(id: &str) -> Option<&'static IconTheme> {
    CATALOG.iter().find(|t| t.id == id)
}

/// The theme and variant installed as icon theme `name`.
pub fn by_theme_name(name: &str) -> Option<(&'static IconTheme, &'static IconVariant)> {
    CATALOG.iter().find_map(|t| t.variants.iter().find(|v| v.theme == name).map(|v| (t, v)))
}

/// A variant by `theme` or `theme/variant` id (the first variant for a
/// bare theme id), or by its icon theme name.
pub fn resolve(spec: &str) -> Option<(&'static IconTheme, &'static IconVariant)> {
    let (id, variant) = spec.split_once('/').unwrap_or((spec, ""));
    if let Some(t) = find(id) {
        let v = if variant.is_empty() { t.variants.first() } else { t.variants.iter().find(|v| v.id == variant) };
        return v.map(|v| (t, v));
    }
    by_theme_name(spec)
}

/// Licences the catalog may list: each allows redistribution, changes
/// included, commercial use included.
pub const ALLOWED_LICENSES: &[&str] = &["GPL-2.0", "GPL-3.0", "GPL-3.0-or-later", "LGPL-3.0", "MIT", "Apache-2.0", "CC-BY-4.0", "CC-BY-SA-4.0", "CC0-1.0", "GPL-3.0 OR CC-BY-SA-4.0"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifty_themes_with_unique_ids_and_folders() {
        assert_eq!(CATALOG.len(), 50);
        let mut ids: Vec<_> = CATALOG.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        // A folder belongs to one theme (its variants may share it).
        let mut owner: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for t in CATALOG {
            for dir in t.variants.iter().flat_map(|v| v.dirs) {
                let first = *owner.entry(dir.name).or_insert(t.id);
                assert_eq!(first, t.id, "{} is installed by both {first} and {}", dir.name, t.id);
            }
            let mut vids: Vec<_> = t.variants.iter().map(|v| v.id).collect();
            vids.sort_unstable();
            vids.dedup();
            assert_eq!(vids.len(), t.variants.len(), "{}: variant ids unique", t.id);
            let mut names: Vec<_> = t.variants.iter().map(|v| v.theme).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), t.variants.len(), "{}: theme names unique", t.id);
        }
    }

    #[test]
    fn sorted_by_popularity() {
        assert!(CATALOG.windows(2).all(|w| w[0].stars >= w[1].stars));
    }

    #[test]
    fn every_entry_is_complete_pinned_and_free() {
        let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        let safe = |p: &str| !p.is_empty() && !p.starts_with('/') && !p.ends_with('/') && !p.split('/').any(|c| c.is_empty() || c == ".." || c == ".");
        for t in CATALOG {
            assert!(ALLOWED_LICENSES.contains(&t.license), "{}: licence {}", t.id, t.license);
            assert!(!t.author.is_empty() && !t.name.is_empty() && !t.description.is_empty(), "{}", t.id);
            assert!(t.homepage.starts_with("https://github.com/") && t.homepage.matches('/').count() == 4, "{}", t.id);
            let a = &t.archive;
            assert!(a.url.starts_with(&format!("{}/", t.homepage)), "{}: {} is outside {}", t.id, a.url, t.homepage);
            assert!(crate::theme_pkg::require_https(a.url).is_ok(), "{}", t.id);
            assert!(a.url.contains("/releases/download/") || a.url.contains("/archive/refs/tags/") || a.url.contains("/archive/"), "{}: pinned", t.id);
            assert!(!a.url.contains("/heads/") && !a.url.contains("/master") && !a.url.contains("/main."), "{}: never a branch", t.id);
            assert!(hex(a.sha256), "{}: sha256", t.id);
            assert!(a.size > 0 && a.size <= super::super::install::MAX_DOWNLOAD, "{}", t.id);
            assert!(t.swatch <= 0xff_ffff);
            assert!(!t.variants.is_empty(), "{}", t.id);
            for v in t.variants {
                assert!(crate::theme_pkg::valid_name(v.theme), "{}/{}", t.id, v.id);
                assert_eq!(v.dirs.first().map(|d| d.name), Some(v.theme), "{}/{}: its own folder first", t.id, v.id);
                for dir in v.dirs {
                    assert!(crate::theme_pkg::valid_name(dir.name) && dir.name != "hicolor" && dir.name != "default", "{}/{}", t.id, v.id);
                    assert!(!dir.layers.is_empty() && dir.layers.iter().all(|l| safe(l)), "{}/{}: {:?}", t.id, v.id, dir.layers);
                }
            }
        }
    }

    #[test]
    fn resolves_specs() {
        assert_eq!(resolve("papirus").map(|(_, v)| v.theme), Some("Papirus"));
        assert_eq!(resolve("papirus/dark").map(|(_, v)| v.theme), Some("Papirus-Dark"));
        assert_eq!(resolve("Papirus-Light").map(|(t, _)| t.id), Some("papirus"));
        assert!(resolve("papirus/neon").is_none());
        assert!(resolve("nope").is_none());
        assert!(find("tela").is_some_and(|t| t.variants[0].dirs[0].fill_name));
    }
}
