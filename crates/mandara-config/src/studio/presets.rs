//! One-click Studio Looks store complete appearance snapshots.

use mandara_i18n::tk;
use mandara_theme::{Color, SchemeVariant};
use serde::{Deserialize, Serialize};

use super::{FrameDesign, StudioAppearance, StudioFrame, StudioPalette};
use crate::components::ComponentPreset;
use crate::desktop::{InkStyle, SurfaceTexture};
use crate::{MotionConfig, ThemeMode, TypographyConfig};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StudioLook {
    #[default]
    Mandara,
    CupertinoGlass,
    Yaru,
    Gtk,
    Arcade,
    Comic,
    Paper,
    Telemetry,
    Terminal,
}

/// A palette seed and component recipe, with a distinct window structure.
pub struct StudioLookDef {
    pub id: StudioLook,
    pub name: &'static str,
    pub description: &'static str,
    pub seed: (f32, f32, f32),
    pub dark: bool,
    pub components: ComponentPreset,
    pub frame: FrameDesign,
    pub ui_font: &'static str,
    pub display_font: &'static str,
}

/// Seeds are OKLCH inputs to the same contrast-checked HCT engine as desktop Looks.
const LOOKS: [StudioLookDef; 9] = [
    StudioLookDef {
        id: StudioLook::Mandara,
        name: tk("Mandara"),
        description: tk("Coastal glass, soft cards and a spacious sidebar."),
        seed: (0.68, 0.12, 220.0),
        dark: true,
        components: ComponentPreset::Classic,
        frame: FrameDesign::Classic,
        ui_font: "Inter",
        display_font: "Alexandria",
    },
    StudioLookDef {
        id: StudioLook::CupertinoGlass,
        name: tk("Cupertino Glass"),
        description: tk("Floating glass, a quiet sidebar and compact window controls."),
        seed: (0.73, 0.06, 255.0),
        dark: false,
        components: ComponentPreset::Minimal,
        frame: FrameDesign::Glass,
        ui_font: "Inter",
        display_font: "Inter",
    },
    StudioLookDef {
        id: StudioLook::Yaru,
        name: tk("Yaru"),
        description: tk("A warm header bar, soft surfaces and orange accents."),
        seed: (0.65, 0.20, 38.0),
        dark: false,
        components: ComponentPreset::Minimal,
        frame: FrameDesign::Header,
        ui_font: "Ubuntu Sans",
        display_font: "Ubuntu Sans",
    },
    StudioLookDef {
        id: StudioLook::Gtk,
        name: tk("GTK"),
        description: tk("A plain header, boxed settings and restrained controls."),
        seed: (0.64, 0.12, 255.0),
        dark: false,
        components: ComponentPreset::Minimal,
        frame: FrameDesign::Header,
        ui_font: "Adwaita Sans",
        display_font: "Adwaita Sans",
    },
    StudioLookDef {
        id: StudioLook::Arcade,
        name: tk("Arcade"),
        description: tk("A cut-corner chassis, bright edges and compact controls."),
        seed: (0.66, 0.22, 24.0),
        dark: true,
        components: ComponentPreset::BrutalistSoft,
        frame: FrameDesign::Chassis,
        ui_font: "Inter",
        display_font: "JetBrains Mono",
    },
    StudioLookDef {
        id: StudioLook::Comic,
        name: tk("Comic"),
        description: tk("Bold ink, paper panels and playful rounded controls."),
        seed: (0.78, 0.18, 85.0),
        dark: false,
        components: ComponentPreset::Expressive,
        frame: FrameDesign::Comic,
        ui_font: "Inter",
        display_font: "Bangers",
    },
    StudioLookDef {
        id: StudioLook::Paper,
        name: tk("Paper"),
        description: tk("A quiet writing desk with generous margins and mixed corners."),
        seed: (0.70, 0.04, 80.0),
        dark: false,
        components: ComponentPreset::Minimal,
        frame: FrameDesign::Editorial,
        ui_font: "Inter",
        display_font: "Alexandria",
    },
    StudioLookDef {
        id: StudioLook::Telemetry,
        name: tk("Telemetry"),
        description: tk(
            "A control desk with end-side navigation, cyan highlights and squared controls.",
        ),
        seed: (0.72, 0.13, 195.0),
        dark: true,
        components: ComponentPreset::BrutalistSoft,
        frame: FrameDesign::Console,
        ui_font: "Inter",
        display_font: "JetBrains Mono",
    },
    StudioLookDef {
        id: StudioLook::Terminal,
        name: tk("Terminal"),
        description: tk("Monospace labels, crisp frames and phosphor-green accents."),
        seed: (0.74, 0.17, 145.0),
        dark: true,
        components: ComponentPreset::Minimal,
        frame: FrameDesign::Terminal,
        ui_font: "JetBrains Mono",
        display_font: "JetBrains Mono",
    },
];

impl StudioLook {
    pub const ALL: [Self; 9] = [
        Self::Mandara,
        Self::CupertinoGlass,
        Self::Yaru,
        Self::Gtk,
        Self::Arcade,
        Self::Comic,
        Self::Paper,
        Self::Telemetry,
        Self::Terminal,
    ];

    pub fn definition(self) -> &'static StudioLookDef {
        &LOOKS[Self::ALL
            .iter()
            .position(|look| *look == self)
            .unwrap_or_default()]
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Mandara => "mandara",
            Self::CupertinoGlass => "cupertino-glass",
            Self::Yaru => "yaru",
            Self::Gtk => "gtk",
            Self::Arcade => "arcade",
            Self::Comic => "comic",
            Self::Paper => "paper",
            Self::Telemetry => "telemetry",
            Self::Terminal => "terminal",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|look| look.slug().eq_ignore_ascii_case(text))
    }

    pub fn appearance(self) -> StudioAppearance {
        let def = self.definition();
        // Color settings persist as sRGB bytes; use that representation from the start.
        let [r, g, b, a] = Color::oklch(def.seed.0, def.seed.1, def.seed.2).to_rgba8();
        let color = Color::from_rgba8(r, g, b, a);
        let mut style = crate::desktop::StyleConfig::default();
        def.components.apply(&mut style);
        let mut frame = StudioFrame {
            design: def.frame,
            ..Default::default()
        };
        match self {
            Self::CupertinoGlass => {
                frame.opacity = 0.95;
                frame.controls_at_start = true;
                frame.inset = 10.0;
                frame.radius = 28.0;
                frame.sidebar_inset = 10.0;
                frame.nav_radius = 12.0;
                frame.card_radius = 16.0;
                style.radius_scale = 1.15;
                style.card_style = crate::desktop::CardStyle::Raised;
            }
            Self::Yaru => {
                frame.header_accent = true;
                frame.radius = 12.0;
                frame.nav_radius = 6.0;
                frame.nav_height = 40.0;
            }
            Self::Gtk => {
                frame.radius = 12.0;
                frame.nav_radius = 7.0;
                frame.card_radius = 10.0;
                frame.card_padding = 14.0;
            }
            Self::Arcade | Self::Telemetry => {
                frame.inset = 10.0;
                frame.radius = 4.0;
                frame.border = 2.0;
                frame.title_height = 56.0;
                frame.sidebar_inset = 8.0;
                frame.nav_radius = 2.0;
                frame.card_radius = 8.0;
                frame.card_padding = 14.0;
                style.radius_scale = 0.65;
            }
            Self::Comic => {
                frame.inset = 10.0;
                frame.radius = 24.0;
                frame.border = 3.0;
                frame.sidebar_inset = 12.0;
                frame.nav_height = 42.0;
                frame.nav_radius = 18.0;
                style.ink = InkStyle {
                    outline: 3.0,
                    shadow: 5.0,
                    texture: SurfaceTexture::Halftone,
                    bubbles: true,
                };
            }
            Self::Paper => {
                frame.inset = 16.0;
                frame.radius = 24.0;
                frame.title_height = 56.0;
                frame.sidebar_inset = 12.0;
                frame.card_radius = 12.0;
                style.surface_corners = crate::desktop::SurfaceCorners::Asymmetric;
                style.shadows = false;
            }
            Self::Terminal => {
                frame.radius = 0.0;
                frame.nav_radius = 0.0;
                frame.nav_height = 34.0;
                frame.title_height = 36.0;
                frame.card_radius = 0.0;
                style.radius_scale = 0.0;
                style.shadows = false;
            }
            Self::Mandara => {}
        }
        StudioAppearance {
            look: self,
            theme: StudioPalette {
                mode: if def.dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                color,
                variant: SchemeVariant::TonalSpot,
                ..Default::default()
            },
            style,
            typography: TypographyConfig {
                ui_font: def.ui_font.into(),
                display_font: def.display_font.into(),
                studio_font: def.ui_font.into(),
                ..Default::default()
            },
            motion: MotionConfig::default(),
            frame,
        }
    }
}
