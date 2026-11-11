//! The complete greeter configuration.
//!
//! The real greeter reads `/etc/mandara/greeter.toml` (system-wide: it runs
//! as greetd's greeter user, which can't read home directories). Mandara
//! Studio edits a draft at `~/.config/mandara/greeter.toml`, which the demo
//! uses; `mandara greeter apply-config` validates the draft and installs it
//! into `/etc` (root only).
//!
//! Every field has a default, so a config file only needs the keys it
//! changes. Sizes and offsets are logical pixels at a 1080-pixel-tall
//! reference screen and scale with the output height.

use std::path::{Path, PathBuf};

use mandara_config::login_layout as L;
use serde::{Deserialize, Serialize};

pub const SYSTEM_CONFIG: &str = "/etc/mandara/greeter.toml";
pub const SYSTEM_VIDEOS: &str = "/usr/share/mandara/login/videos";
pub const SYSTEM_IMAGES: &str = "/usr/share/mandara/login/backgrounds";
/// The media folders on image-based systems, whose `/usr/share` is
/// read-only: searched too when they exist.
pub const LOCAL_VIDEOS: &str = "/usr/local/share/mandara/login/videos";
pub const LOCAL_IMAGES: &str = "/usr/local/share/mandara/login/backgrounds";

/// `dirs` plus `local` when that folder exists and isn't listed yet.
fn with_local(mut dirs: Vec<PathBuf>, local: &str) -> Vec<PathBuf> {
    let local = Path::new(local);
    if local.is_dir() && !dirs.iter().any(|d| d == local) {
        dirs.push(local.to_path_buf());
    }
    dirs
}

/// `~/.config/mandara/greeter.toml`: Studio's draft, used by the demo.
pub fn user_draft_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("mandara/greeter.toml")
}

// ── Background ──────────────────────────────────────────────────────────

/// Which backdrop to use.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundMode {
    /// Videos if any, else stills, else the aurora.
    #[default]
    Auto,
    Video,
    Stills,
    Aurora,
    Solid,
}

impl BackgroundMode {
    pub const ALL: [BackgroundMode; 5] = [Self::Auto, Self::Video, Self::Stills, Self::Aurora, Self::Solid];

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "video" | "videos" => Some(Self::Video),
            "stills" | "images" | "slideshow" => Some(Self::Stills),
            "aurora" | "gradient" => Some(Self::Aurora),
            "solid" | "color" => Some(Self::Solid),
            _ => None,
        }
    }
}

/// Something drawn over the backdrop, under the controls.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Overlay {
    #[default]
    None,
    /// A measured frame: corner brackets, a faint grid, tick rulers along
    /// the edges and a slow scan line (still with reduced motion).
    Hud,
    /// Soft holographic light: wide glows in the theme's accents over a
    /// deep ink sky, drifting very slowly.
    Prism,
    /// One calm field (dawn ivory to pale stone, or deep ink with the
    /// light colours off) crossed by a single hairline at the golden
    /// height, which draws itself in from the centre and brightens around
    /// the caret. It covers the backdrop.
    Horizon,
}

/// How videos are decoded.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VideoMode {
    /// Full quality, degrading automatically on slow machines.
    #[default]
    Auto,
    /// Always use the fast decoding path.
    Lite,
    /// Never play videos (stills / aurora only).
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub mode: BackgroundMode,
    pub video: VideoMode,
    /// Folders with `.webm` / `.mkv` / `.ivf` AV1 clips.
    pub video_dirs: Vec<PathBuf>,
    /// Folders with `.png` / `.jpg` / `.webp` stills.
    pub image_dirs: Vec<PathBuf>,
    /// Play clips / stills in random order (else by file name, or in
    /// `playlist` order).
    pub shuffle: bool,
    /// Explicit order by file name; files not listed follow.
    pub playlist: Vec<String>,
    /// With several clips, show each at least this long (s).
    pub clip_seconds: u32,
    /// Clip-to-clip crossfade (ms).
    pub crossfade_ms: u32,
    /// How long each still is shown (s).
    pub still_seconds: u32,
    /// Still-to-still crossfade (ms).
    pub still_fade_ms: u32,
    /// Ken Burns zoom range (0 = static, 0.12 = gentle, 0.3 = dramatic).
    pub ken_burns: f32,
    /// Video speed (1 = normal; 0.5 = dreamy slow motion).
    pub playback_rate: f32,
    /// Darken the backdrop (0–1).
    pub dim: f32,
    /// Edge vignette strength (0–1).
    pub vignette: f32,
    /// Anti-banding grain (0–1).
    pub grain: f32,
    /// Softens the backdrop (0–1): stills are blurred, videos decoded at
    /// reduced resolution.
    pub blur: f32,
    /// Color wash over the backdrop (`#rrggbb`, empty = none).
    pub tint: String,
    pub tint_strength: f32,
    /// Color for `mode = "solid"` (empty = the theme's surface).
    pub solid: String,
    /// Drifting light orbs over any backdrop.
    pub orbs: bool,
    pub orb_intensity: f32,
    /// Show the picked person's own desktop wallpaper (shared by their
    /// Mandara session into `/var/lib/mandara-login/wallpapers`) instead
    /// of the clips and stills above, when they have one.
    pub user_wallpaper: bool,
    /// Pointer parallax depth in px (0 = off).
    pub parallax: f32,
    /// Drawn over the backdrop.
    pub overlay: Overlay,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            mode: BackgroundMode::Auto,
            video: VideoMode::Auto,
            video_dirs: vec![PathBuf::from(SYSTEM_VIDEOS)],
            image_dirs: vec![PathBuf::from(SYSTEM_IMAGES)],
            shuffle: true,
            playlist: Vec::new(),
            clip_seconds: 30,
            crossfade_ms: 1400,
            still_seconds: 16,
            still_fade_ms: 2600,
            ken_burns: 0.12,
            playback_rate: 1.0,
            dim: 0.08,
            vignette: 0.4,
            grain: 1.0,
            blur: 0.0,
            tint: String::new(),
            tint_strength: 0.0,
            solid: String::new(),
            orbs: true,
            orb_intensity: 0.5,
            user_wallpaper: true,
            parallax: 14.0,
            overlay: Overlay::None,
        }
    }
}

// ── Clock & date ────────────────────────────────────────────────────────

/// How the clock is drawn.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClockFace {
    /// Big digits, in a row or stacked.
    #[default]
    Digits,
    /// Thin monospace digits stacked beside a ruler of the day, whose
    /// marker shows the time; the date and time zone sit under them.
    Ruler,
    /// Light digits inside a luminous ring of iridescent light that turns
    /// slowly and brightens while you type; the date sits under them.
    Halo,
    /// Enormous hairline numerals behind everything at a whisper of
    /// contrast, cropped by the screen's edges (four times the size).
    Vast,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClockConfig {
    pub format_24h: bool,
    pub seconds: bool,
    /// Font family (empty = the display font, Alexandria).
    pub font: String,
    pub size: f32,
    /// CSS-style weight 100–900.
    pub weight: u16,
    pub opacity: f32,
    /// Digits roll when they change.
    pub roll_digits: bool,
    /// Hours over minutes in two big lines.
    pub stacked: bool,
    /// The hours in a light tone of the accent.
    pub tint: bool,
    /// Weight of the big idle clock (0 = the same as `weight`): waking
    /// the screen moves the font's weight between the two.
    pub idle_weight: u16,
    /// The ruler and halo faces draw the date themselves (when the date
    /// is shown).
    pub face: ClockFace,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self { format_24h: true, seconds: false, font: String::new(), size: L::CLOCK_SIZE, weight: L::CLOCK_WEIGHT, opacity: 0.95, roll_digits: true, stacked: false, tint: false, idle_weight: 0, face: ClockFace::Digits }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DateConfig {
    /// strftime pattern (empty = the language's default, e.g. "Tuesday, 27 October").
    pub format: String,
    pub font: String,
    pub size: f32,
    pub weight: u16,
    pub opacity: f32,
}

impl Default for DateConfig {
    fn default() -> Self {
        Self { format: String::new(), font: String::new(), size: L::DATE_SIZE, weight: L::DATE_WEIGHT, opacity: 0.9 }
    }
}

// ── User & password ─────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UserList {
    /// Everyone with a login shell (switcher under the card).
    #[default]
    All,
    /// Only the last user; others via "Other user".
    LastUser,
    /// Always type a username.
    Manual,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AvatarShape {
    #[default]
    Circle,
    Squircle,
    Rounded,
    /// No plate or frame: a picture stays round, a monogram or icon is
    /// drawn on its own.
    Bare,
    /// Material's scalloped twelve-sided shape, morphing into a circle
    /// while signing in.
    Cookie,
}

/// How the other users are offered.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UserSwitcher {
    /// Their faces in a row.
    #[default]
    Faces,
    /// Material chips: a small face and the first name.
    Chips,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AvatarSource {
    /// AccountsService icon or `~/.face`, else a monogram.
    #[default]
    Auto,
    Monogram,
    Icon,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    pub list: UserList,
    pub avatar_shape: AvatarShape,
    pub avatar_source: AvatarSource,
    pub avatar_size: f32,
    /// A softly breathing accent ring around the avatar while idle.
    pub glow_ring: bool,
    pub show_name: bool,
    pub name_size: f32,
    /// Login names never listed.
    pub hidden: Vec<String>,
    /// How the user switcher shows the other people.
    pub switcher: UserSwitcher,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            list: UserList::All,
            avatar_shape: AvatarShape::Circle,
            avatar_source: AvatarSource::Auto,
            avatar_size: L::AVATAR,
            glow_ring: true,
            show_name: true,
            name_size: L::NAME_SIZE,
            hidden: Vec::new(),
            switcher: UserSwitcher::Faces,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldStyle {
    /// Frosted translucent pill with a specular rim.
    #[default]
    Glass,
    /// Opaque theme surface.
    Solid,
    /// Hairline outline only.
    Outline,
    /// A single underline.
    Underline,
    /// A dark recessed field with a hairline border.
    Inset,
    /// Material's filled field: a tonal container with rounded top
    /// corners, an indicator line that thickens in the accent while
    /// focused, and the hint floating up as a label once you type.
    Filled,
    /// A terminal line: a prompt, one cell per character and a block
    /// caret, in hairlines.
    Terminal,
    /// A deep field sunk into its panel, with an inner highlight and a
    /// soft accent halo while focused.
    Recessed,
}

/// Where the sign-in button sits.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitButton {
    /// A round button inside the field, shown once you type.
    #[default]
    Inside,
    /// A square accent button beside the field, always shown.
    Beside,
    /// None: Enter signs in.
    Hidden,
    /// A full-width pill under the field, its arrow in a circle of its own.
    Pill,
}

/// How typed characters are masked.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordMask {
    /// Identical dots.
    Dots,
    /// A different small soft shape per character (circle, square,
    /// triangle, clover…), never derived from the character itself.
    #[default]
    Shapes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasswordConfig {
    pub style: FieldStyle,
    pub submit: SubmitButton,
    pub mask: PasswordMask,
    /// The focused field gets an accent-coloured border.
    pub accent_border: bool,
    /// Text size inside the field.
    pub text_size: f32,
    pub width: f32,
    pub height: f32,
    /// Placeholder (empty = "Enter password" in the current language).
    pub placeholder: String,
    pub caps_hint: bool,
    /// An eye button to reveal what was typed.
    pub reveal_toggle: bool,
    /// Each keystroke pops a dot with a tiny spring and ripples the pill.
    pub keystroke_ripple: bool,
    /// The pill glows with the accent while typing.
    pub glow: bool,
}

impl Default for PasswordConfig {
    fn default() -> Self {
        Self {
            style: FieldStyle::Glass,
            submit: SubmitButton::Inside,
            mask: PasswordMask::Shapes,
            accent_border: false,
            text_size: 15.0,
            width: L::FIELD_W,
            height: L::FIELD_H,
            placeholder: String::new(),
            caps_hint: true,
            reveal_toggle: true,
            keystroke_ripple: true,
            glow: true,
        }
    }
}

// ── Session, power, keyboard, banner ────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Session id preselected for new users (`niri`, `hyprland`, `mango`…;
    /// empty = first). A user's last choice wins.
    pub default: String,
    /// List X11 sessions too.
    pub x11: bool,
    pub style: SessionStyle,
}

/// How the session picker looks.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionStyle {
    /// A compact pill.
    #[default]
    Pill,
    /// A full-width field (a drop-down as wide as the password field, or
    /// the card).
    Field,
    /// A Material chip with a menu.
    Chip,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { default: String::new(), x11: true, style: SessionStyle::Pill }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    pub suspend: bool,
    pub reboot: bool,
    pub poweroff: bool,
    pub labels: bool,
    pub style: PowerStyle,
    /// Button size.
    pub size: f32,
}

/// How the power buttons look.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerStyle {
    /// Soft frosted circles.
    #[default]
    Soft,
    /// Dark circles with a hairline outline.
    Outlined,
    /// Material tonal icon buttons that square off while pressed.
    Tonal,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self { suspend: true, reboot: true, poweroff: true, labels: true, style: PowerStyle::Soft, size: 44.0 }
    }
}

/// How the sign-in card is built.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CardStyle {
    /// One panel with the user over the field.
    #[default]
    Plain,
    /// A wireframe panel: corner brackets, a ruled header with the host
    /// name and a footer of live readouts (keyboard layout, uptime, time
    /// zone).
    Hud,
    /// A machined module: an outer tray with a hairline rim around an
    /// inner core with its own highlight and a concentric radius.
    Bezel,
    /// No card at all: the name and the typed characters ride on a line
    /// across the screen at the golden height, set like type; the session,
    /// power, keyboard layout and date are small words at its ends.
    Horizon,
}

/// A card behind the user, the password field and (optionally) the
/// session picker, drawn where the user card is placed. Off: the pieces
/// float on the backdrop at their own places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CardConfig {
    pub enabled: bool,
    pub width: f32,
    pub padding: f32,
    pub radius: f32,
    /// Fill colour (`#rrggbb`, empty = the theme's raised surface).
    pub fill: String,
    pub opacity: f32,
    /// Border width (0 = none) and colour (empty = the theme's outline).
    pub border: f32,
    pub border_color: String,
    /// A back arrow in its corner returns to the user switcher.
    pub back_button: bool,
    /// The session picker sits inside the card, under the password.
    pub session_inside: bool,
    pub style: CardStyle,
}

impl Default for CardConfig {
    fn default() -> Self {
        Self { enabled: false, width: 460.0, padding: 22.0, radius: 22.0, fill: String::new(), opacity: 1.0, border: 1.0, border_color: String::new(), back_button: true, session_inside: true, style: CardStyle::Plain }
    }
}

/// An image on the login screen (a logo, an emblem), placed like the other
/// elements.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogoConfig {
    /// PNG / JPEG / WebP / SVG file (empty = Mandara's emblem).
    pub path: String,
    pub size: f32,
    pub opacity: f32,
    /// Mandara's emblem in a light tone of the accent instead of white.
    pub accent: bool,
}

impl Default for LogoConfig {
    fn default() -> Self {
        Self { path: String::new(), size: 56.0, opacity: 0.9, accent: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    /// Layout shown by the indicator (empty = detected from the system).
    pub layout: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BannerConfig {
    /// A message on the login screen ("Authorized use only", a welcome…).
    pub text: String,
    pub size: f32,
}

impl Default for BannerConfig {
    fn default() -> Self {
        Self { text: String::new(), size: 14.0 }
    }
}

// ── Theme, motion, accessibility ────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Catalog theme id (`mandara themes`).
    pub preset: String,
    /// Accent override (`#rrggbb`, empty = the theme's primary).
    pub accent: String,
    /// Glass fill opacity (0–1).
    pub glass: f32,
    /// Specular rim strength (0–1).
    pub rim: f32,
    /// Control corner roundness (0 = square, 1 = pill).
    pub roundness: f32,
    /// Themes the colour scheme switcher offers (catalog ids; empty = a
    /// handful of Mandara's own). The pick is remembered.
    pub switcher: Vec<String>,
    /// Colours made from the picture behind the login screen (Material's
    /// dynamic colour); the preset when it shows no picture.
    pub from_wallpaper: bool,
    /// The theme's light colours: light controls with dark text (text on
    /// the backdrop follows the backdrop, and dark pieces stay dark; see
    /// [`GreeterConfig::draws_light`]).
    pub light: bool,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self { preset: "montaza-twilight".into(), accent: String::new(), glass: 0.18, rim: 0.6, roundness: 1.0, switcher: Vec::new(), from_wallpaper: false, light: false }
    }
}

/// A spring: stiffness / damping (mass 1).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
}

impl Spring {
    pub const fn new(stiffness: f32, damping: f32) -> Self {
        Self { stiffness, damping }
    }

    pub fn config(self) -> mandara_core::SpringConfig {
        mandara_core::SpringConfig::new(self.stiffness.max(1.0), self.damping.max(0.0), 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionConfig {
    /// Staggered spring entrance.
    pub entrance: bool,
    /// Delay between elements entering (ms).
    pub stagger_ms: u32,
    pub entrance_spring: Spring,
    /// Avatar pop overshoot spring.
    pub pop_spring: Spring,
    /// Wrong-password shake.
    pub shake: bool,
    pub shake_strength: f32,
    pub shake_spring: Spring,
    /// Success transition (zoom + dissolve).
    pub success_spring: Spring,
    pub success_zoom: f32,
    /// Seconds without input before returning to the big idle clock
    /// (0 = never idle).
    pub idle_seconds: u32,
    /// Start on the idle clock (wake with any key or pointer move).
    pub start_idle: bool,
    pub idle_spring: Spring,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            entrance: true,
            stagger_ms: 70,
            entrance_spring: Spring::new(120.0, 17.0),
            pop_spring: Spring::new(260.0, 13.0),
            shake: true,
            shake_strength: 1.0,
            shake_spring: Spring::new(1100.0, 16.0),
            success_spring: Spring::new(55.0, 15.0),
            success_zoom: 0.10,
            idle_seconds: 30,
            start_idle: true,
            idle_spring: Spring::new(140.0, 22.0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilityConfig {
    /// No springs, parallax, drifting or rolling: everything cross-fades.
    pub reduced_motion: bool,
    /// 20 % larger text and controls.
    pub large_text: bool,
    /// Stronger scrims and opaque controls.
    pub high_contrast: bool,
}

// ── Layout ──────────────────────────────────────────────────────────────

/// Something placed on the login screen.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Element {
    Clock,
    Date,
    /// Avatar + name.
    User,
    /// Password field + status line.
    Password,
    /// Other users.
    Users,
    Session,
    Power,
    Keyboard,
    Banner,
    /// An image (`[logo]`).
    Logo,
    /// A colour scheme switcher (`[theme] switcher`).
    Themes,
}

impl Element {
    pub const ALL: [Element; 11] = [Self::Clock, Self::Date, Self::User, Self::Password, Self::Users, Self::Session, Self::Power, Self::Keyboard, Self::Banner, Self::Logo, Self::Themes];

    /// English name (translate for display).
    pub fn name(self) -> &'static str {
        match self {
            Self::Clock => "Clock",
            Self::Date => "Date",
            Self::User => "User card",
            Self::Password => "Password field",
            Self::Users => "User switcher",
            Self::Session => "Session picker",
            Self::Power => "Power buttons",
            Self::Keyboard => "Keyboard layout",
            Self::Banner => "Message",
            Self::Logo => "Logo",
            Self::Themes => "Colour schemes",
        }
    }

    /// Hidden while idle (the idle screen is the clock).
    pub fn wakes(self) -> bool {
        !matches!(self, Self::Clock | Self::Date)
    }
}

/// 9-point anchor: where on the screen (and on the element) it is pinned.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [Self::TopLeft, Self::Top, Self::TopRight, Self::Left, Self::Center, Self::Right, Self::BottomLeft, Self::Bottom, Self::BottomRight];

    /// (fx, fy) in 0, 0.5, 1.
    pub fn fraction(self) -> (f32, f32) {
        let i = Self::ALL.iter().position(|a| *a == self).unwrap_or(4);
        ((i % 3) as f32 * 0.5, (i / 3) as f32 * 0.5)
    }

    pub fn from_fraction(fx: f32, fy: f32) -> Anchor {
        let c = |f: f32| if f < 0.25 { 0 } else if f > 0.75 { 2 } else { 1 };
        Self::ALL[c(fy) * 3 + c(fx)]
    }

    /// The anchor point on a `w × h` rect at (x, y).
    pub fn point(self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
        let (fx, fy) = self.fraction();
        (x + w * fx, y + h * fy)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub element: Element,
    pub anchor: Anchor,
    /// Offset from the anchor, reference pixels.
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub visible: bool,
    /// Paint order (higher on top).
    pub z: i32,
}

impl Default for Placement {
    fn default() -> Self {
        Self { element: Element::Banner, anchor: Anchor::Center, x: 0.0, y: 0.0, scale: 1.0, visible: true, z: 0 }
    }
}

fn place(element: Element, anchor: Anchor, x: f32, y: f32, visible: bool, z: i32) -> Placement {
    Placement { element, anchor, x, y, scale: 1.0, visible, z }
}

/// The card positions (user, password, user list) and field size before
/// the login screen and the lock screen shared their proportions.
const OLD_CARD_CENTERS: [(Element, f32); 3] = [(Element::User, 118.0), (Element::Password, 254.0), (Element::Users, 350.0)];
const OLD_FIELD: (f32, f32) = (290.0, 44.0);

/// The layout defaults a config was written for (`layout_version`).
/// 1: Mandara's emblem shows in the default layout.
pub const LAYOUT_VERSION: u32 = 1;
/// The logo's placement in layouts before version 1: hidden above the
/// keyboard pill.
const OLD_LOGO: (Anchor, f32, f32, f32, i32) = (Anchor::Bottom, 0.0, -96.0, 1.0, 2);

impl GreeterConfig {
    /// A config still at the old card defaults (written in full by the
    /// installer, never edited) gets the new ones: the card closer to the
    /// clock and the wider field. Anything moved or resized stays.
    fn upgrade_untouched_layout(&mut self) {
        let untouched = OLD_CARD_CENTERS.iter().all(|(e, y)| self.layout.iter().any(|p| p.element == *e && p.anchor == Anchor::Center && p.x == 0.0 && p.y == *y && p.scale == 1.0));
        if untouched {
            let defaults = default_layout();
            for p in &mut self.layout {
                if let Some(d) = OLD_CARD_CENTERS.iter().find(|(e, _)| *e == p.element).and_then(|(e, _)| defaults.iter().find(|d| d.element == *e)) {
                    p.y = d.y;
                }
            }
        }
        if (self.password.width, self.password.height) == OLD_FIELD {
            (self.password.width, self.password.height) = (L::FIELD_W, L::FIELD_H);
        }
        // Before version 1 the default layout hid the logo, so Mandara's
        // emblem never showed in the Mandara look. A logo still where that
        // layout put it was never turned on: show it. One that was moved,
        // resized or (since version 1) turned off stays as it is.
        if self.layout_version < 1 {
            if let Some(p) = self.layout.iter_mut().find(|p| p.element == Element::Logo)
                && !p.visible
                && (p.anchor, p.x, p.y, p.scale, p.z) == OLD_LOGO
            {
                p.visible = true;
            }
            self.layout_version = LAYOUT_VERSION;
        }
    }
}

/// Ready-made looks for the whole screen: `(id, English name, summary)`.
/// Applying one ([`GreeterConfig::apply_look`]) changes how the screen
/// looks and where things sit; who is listed, sessions, folders, language
/// and accessibility stay as they are.
pub const LOOKS: [(&str, &str, &str); 6] = [
    ("mandara", "Mandara", "A big clock, your picture over a glass field, everything floating on the backdrop"),
    ("midnight-card", "Midnight card", "A quiet clock over one outlined card with the user, password and session, in midnight navy and citrine"),
    ("material-you", "Material You", "Colours from your wallpaper, a big stacked clock and a tonal card with a scalloped avatar, chips and tonal buttons"),
    ("ghostline", "Ghostline", "Graphite and ice-white hairlines: a wireframe sign-in panel on the left with live readouts, a terminal password line and a ruler clock, in cold cyan"),
    ("chrome-halo", "Chrome Halo", "Deep ink and holographic light: a luminous ring around a light clock, and a machined sign-in module on the right with a pill button"),
    ("meridian", "Meridian", "No boxes: your name and password ride on one hairline across a calm dawn field, over enormous whisper-thin numerals; light or dark"),
];

/// Where a look puts the sign-in: the horizontal anchor of its user card.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Center,
    Right,
}

impl GreeterConfig {
    /// Which side of the screen the sign-in sits on.
    pub fn credentials_side(&self) -> Side {
        match self.placement(Element::User).anchor.fraction().0 {
            x if x < 0.25 => Side::Left,
            x if x > 0.75 => Side::Right,
            _ => Side::Center,
        }
    }
}

impl GreeterConfig {
    /// Applies look `id` from [`LOOKS`]; `false` for an unknown id.
    pub fn apply_look(&mut self, id: &str) -> bool {
        let base = GreeterConfig::default();
        // What a look owns, starting from Mandara's defaults.
        let keep_dirs = (self.background.video_dirs.clone(), self.background.image_dirs.clone(), self.background.playlist.clone());
        self.background = BackgroundConfig { video_dirs: keep_dirs.0, image_dirs: keep_dirs.1, playlist: keep_dirs.2, ..base.background.clone() };
        self.clock = ClockConfig { format_24h: self.clock.format_24h, seconds: self.clock.seconds, ..base.clock.clone() };
        self.date = DateConfig { format: self.date.format.clone(), ..base.date.clone() };
        self.user = UserConfig { list: self.user.list, hidden: self.user.hidden.clone(), ..base.user.clone() };
        self.password = PasswordConfig { placeholder: self.password.placeholder.clone(), caps_hint: self.password.caps_hint, ..base.password.clone() };
        self.session.style = base.session.style;
        self.power = PowerConfig { suspend: self.power.suspend, reboot: self.power.reboot, poweroff: self.power.poweroff, ..base.power.clone() };
        self.theme = ThemeConfig { switcher: self.theme.switcher.clone(), ..base.theme.clone() };
        self.card = base.card.clone();
        self.logo = LogoConfig { path: self.logo.path.clone(), ..base.logo.clone() };
        self.layout = default_layout();
        self.layout_version = LAYOUT_VERSION;
        match id {
            "mandara" => {}
            "midnight-card" => {
                self.theme.preset = "night-citrine".into();
                self.theme.roundness = 0.32;
                self.theme.glass = 0.0;
                self.theme.rim = 0.0;
                self.background.mode = BackgroundMode::Solid;
                self.background.solid = "#070722".into();
                self.background.dim = 0.32;
                self.background.vignette = 0.0;
                self.background.orbs = false;
                self.background.grain = 0.0;
                self.clock.size = 96.0;
                self.clock.weight = 400;
                self.clock.opacity = 1.0;
                self.clock.roll_digits = false;
                self.date.size = 15.0;
                self.date.weight = 400;
                self.date.opacity = 0.6;
                self.user.avatar_source = AvatarSource::Icon;
                self.user.avatar_shape = AvatarShape::Bare;
                self.user.avatar_size = 64.0;
                self.user.glow_ring = false;
                self.user.name_size = 16.0;
                self.password.style = FieldStyle::Inset;
                self.password.submit = SubmitButton::Beside;
                self.password.mask = PasswordMask::Shapes;
                self.password.accent_border = true;
                self.password.height = 40.0;
                self.password.text_size = 14.0;
                self.password.glow = false;
                self.password.keystroke_ripple = false;
                self.password.reveal_toggle = false;
                self.session.style = SessionStyle::Field;
                self.power.style = PowerStyle::Outlined;
                self.power.labels = false;
                self.power.size = 36.0;
                self.card = CardConfig { enabled: true, width: 440.0, padding: 20.0, radius: 18.0, border: 1.0, ..CardConfig::default() };
                for pl in &mut self.layout {
                    match pl.element {
                        Element::Clock => (pl.anchor, pl.y) = (Anchor::Top, 112.0),
                        Element::Date => (pl.anchor, pl.y) = (Anchor::Top, 226.0),
                        Element::User => pl.y = 30.0,
                        Element::Users | Element::Keyboard => pl.visible = false,
                        Element::Logo => (pl.visible, pl.y) = (true, -36.0),
                        Element::Themes => pl.visible = true,
                        Element::Power => (pl.x, pl.y) = (-28.0, -28.0),
                        _ => {}
                    }
                }
            }
            "material-you" => {
                // Material 3 Expressive: dynamic colour, tonal surfaces,
                // large type and shapes that morph (corner sizes are
                // Material's: 32 px cards, 16 px fields).
                self.theme.preset = "mandara-corniche".into();
                self.theme.from_wallpaper = true;
                self.theme.glass = 0.0;
                self.theme.rim = 0.0;
                self.background.dim = 0.2;
                self.background.vignette = 0.25;
                self.background.orbs = false;
                self.background.grain = 0.5;
                self.background.ken_burns = 0.06;
                self.background.parallax = 8.0;
                self.clock.size = 132.0;
                self.clock.weight = 500;
                self.clock.idle_weight = 250;
                self.clock.stacked = true;
                self.clock.tint = true;
                self.clock.opacity = 1.0;
                self.date.size = 20.0;
                self.date.weight = 500;
                self.date.opacity = 0.92;
                self.user.avatar_shape = AvatarShape::Cookie;
                self.user.avatar_size = 112.0;
                self.user.glow_ring = false;
                self.user.name_size = 22.0;
                self.user.switcher = UserSwitcher::Chips;
                self.password.style = FieldStyle::Filled;
                self.password.submit = SubmitButton::Beside;
                self.password.mask = PasswordMask::Shapes;
                self.password.height = 56.0;
                self.password.text_size = 16.0;
                self.password.glow = false;
                self.session.style = SessionStyle::Chip;
                self.power.style = PowerStyle::Tonal;
                self.power.labels = false;
                self.power.size = 48.0;
                self.card = CardConfig { enabled: true, width: 420.0, padding: 28.0, radius: 32.0, border: 0.0, opacity: 0.92, ..CardConfig::default() };
                self.logo.size = 40.0;
                self.logo.opacity = 1.0;
                self.logo.accent = true;
                for pl in &mut self.layout {
                    match pl.element {
                        Element::Date => pl.y = 64.0,
                        Element::Clock => pl.y = 96.0,
                        Element::User => pl.y = 150.0,
                        Element::Users => pl.y = 420.0,
                        Element::Keyboard => pl.y = -32.0,
                        // The bottom corner the session picker leaves free
                        // (it sits in the card), across from the power buttons.
                        Element::Logo => (pl.anchor, pl.x, pl.y) = (Anchor::BottomLeft, 36.0, -36.0),
                        Element::Power => (pl.x, pl.y) = (-32.0, -32.0),
                        Element::Session => (pl.x, pl.y) = (32.0, -32.0),
                        _ => {}
                    }
                }
            }
            "ghostline" => {
                // Cyberpunk made clean: graphite, ice-white hairlines, one
                // cold cyan accent (the theme's magenta-pink tertiary is the
                // sparing second one), everything measured.
                self.theme.preset = "vesper".into();
                self.theme.accent = "#5fd7ff".into();
                self.theme.glass = 0.0;
                self.theme.rim = 0.0;
                self.theme.roundness = 0.12;
                self.background.mode = BackgroundMode::Solid;
                self.background.solid = "#0b0d10".into();
                self.background.user_wallpaper = false;
                self.background.orbs = false;
                self.background.dim = 0.0;
                self.background.vignette = 0.3;
                self.background.grain = 0.4;
                self.background.parallax = 0.0;
                self.background.overlay = Overlay::Hud;
                self.clock.face = ClockFace::Ruler;
                self.clock.size = 168.0;
                self.clock.weight = 200;
                self.clock.stacked = true;
                self.clock.opacity = 0.96;
                self.date.size = 13.0;
                self.date.weight = 500;
                self.date.opacity = 0.62;
                self.user.avatar_shape = AvatarShape::Rounded;
                self.user.avatar_size = 52.0;
                self.user.glow_ring = false;
                self.user.name_size = 18.0;
                self.password.style = FieldStyle::Terminal;
                self.password.mask = PasswordMask::Dots;
                self.password.accent_border = true;
                self.password.height = 46.0;
                self.password.text_size = 14.0;
                self.password.glow = false;
                self.session.style = SessionStyle::Field;
                self.power.style = PowerStyle::Outlined;
                self.power.labels = false;
                self.power.size = 36.0;
                self.card = CardConfig { enabled: true, style: CardStyle::Hud, width: 420.0, padding: 26.0, radius: 2.0, border: 1.0, opacity: 0.86, ..CardConfig::default() };
                self.logo.size = 26.0;
                self.logo.opacity = 0.92;
                self.logo.accent = true;
                for pl in &mut self.layout {
                    match pl.element {
                        Element::User => (pl.anchor, pl.x, pl.y) = (Anchor::Left, GHOSTLINE_INSET, 0.0),
                        Element::Users => (pl.anchor, pl.x, pl.y) = (Anchor::Left, GHOSTLINE_INSET, 380.0),
                        Element::Clock => (pl.anchor, pl.x, pl.y) = (Anchor::Right, -GHOSTLINE_INSET, -10.0),
                        Element::Keyboard => pl.visible = false,
                        Element::Logo => (pl.anchor, pl.x, pl.y) = (Anchor::TopLeft, 48.0, 48.0),
                        Element::Power => (pl.x, pl.y) = (-40.0, -40.0),
                        _ => {}
                    }
                }
            }
            "chrome-halo" => {
                // Liquid chrome and iridescent light on deep ink, warm: the
                // ring takes the theme's gold, periwinkle and rose.
                self.theme.preset = "pharos-night".into();
                self.theme.glass = 0.0;
                self.theme.rim = 0.0;
                self.background.mode = BackgroundMode::Solid;
                self.background.solid = "#06070e".into();
                self.background.user_wallpaper = false;
                self.background.orbs = false;
                self.background.dim = 0.0;
                self.background.vignette = 0.3;
                self.background.grain = 0.6;
                self.background.parallax = 8.0;
                self.background.overlay = Overlay::Prism;
                self.clock.face = ClockFace::Halo;
                self.clock.size = 124.0;
                self.clock.weight = 200;
                self.clock.opacity = 1.0;
                self.date.size = 15.0;
                self.date.weight = 400;
                self.date.opacity = 0.72;
                self.user.avatar_size = 60.0;
                self.user.glow_ring = false;
                self.user.name_size = 20.0;
                self.password.style = FieldStyle::Recessed;
                self.password.submit = SubmitButton::Pill;
                self.password.height = 52.0;
                self.password.text_size = 15.0;
                self.password.reveal_toggle = true;
                self.session.style = SessionStyle::Pill;
                self.power.labels = false;
                self.power.size = 40.0;
                self.card = CardConfig { enabled: true, style: CardStyle::Bezel, width: 404.0, padding: 28.0, radius: 34.0, border: 1.0, opacity: 0.92, ..CardConfig::default() };
                self.logo.size = 30.0;
                self.logo.opacity = 0.95;
                self.logo.accent = true;
                for pl in &mut self.layout {
                    match pl.element {
                        Element::User => (pl.anchor, pl.x, pl.y) = (Anchor::Right, -CHROME_HALO_INSET, 120.0),
                        Element::Users => (pl.anchor, pl.x, pl.y) = (Anchor::Right, -CHROME_HALO_INSET, 400.0),
                        Element::Clock => (pl.anchor, pl.x, pl.y) = (Anchor::TopLeft, 150.0, 250.0),
                        Element::Logo => (pl.anchor, pl.x, pl.y) = (Anchor::BottomLeft, 40.0, -40.0),
                        Element::Power => (pl.x, pl.y) = (-40.0, -40.0),
                        _ => {}
                    }
                }
            }
            "meridian" => {
                // Editorial calm: one field, one line, type on it, one
                // accent at most (a warm terracotta, for the caret alone).
                self.theme.preset = "mandara-corniche".into();
                self.theme.accent = "#c0623a".into();
                self.theme.light = true;
                self.theme.glass = 0.0;
                self.theme.rim = 0.0;
                self.background.mode = BackgroundMode::Solid;
                self.background.solid = "#f3efe6".into();
                self.background.user_wallpaper = false;
                self.background.orbs = false;
                self.background.dim = 0.0;
                self.background.vignette = 0.0;
                self.background.grain = 0.35;
                self.background.parallax = 0.0;
                self.background.overlay = Overlay::Horizon;
                self.clock.face = ClockFace::Vast;
                self.clock.size = 150.0;
                self.clock.weight = 100;
                self.clock.opacity = 0.075;
                self.clock.roll_digits = false;
                self.date.size = 11.5;
                self.date.weight = 500;
                self.date.opacity = 0.7;
                self.user.avatar_source = AvatarSource::Icon;
                self.user.name_size = 40.0;
                self.password.mask = PasswordMask::Dots;
                self.password.glow = false;
                self.password.text_size = 15.0;
                self.password.reveal_toggle = false;
                self.card = CardConfig { enabled: true, style: CardStyle::Horizon, ..CardConfig::default() };
                self.logo.size = 22.0;
                self.logo.opacity = 0.85;
                for pl in &mut self.layout {
                    match pl.element {
                        Element::User => (pl.anchor, pl.x, pl.y) = (Anchor::Left, 0.0, 0.0),
                        Element::Clock => (pl.anchor, pl.x, pl.y) = (Anchor::BottomRight, 40.0, 70.0),
                        Element::Users | Element::Keyboard | Element::Power => pl.visible = false,
                        Element::Logo => (pl.anchor, pl.x, pl.y) = (Anchor::TopLeft, 56.0, 56.0),
                        _ => {}
                    }
                }
            }
            _ => return false,
        }
        self.normalize();
        true
    }

    /// Whether the login screen is drawn in the light colours: they are on
    /// and nothing on it is machined from a fixed dark ink. Ghostline's and
    /// Chrome Halo's pieces (the wireframe and double-bezel cards, the
    /// terminal and recessed fields) are dark by design, so with any of
    /// them the screen keeps its dark colours and light text.
    pub fn draws_light(&self) -> bool {
        let dark_card = self.card.enabled && matches!(self.card.style, CardStyle::Hud | CardStyle::Bezel);
        let dark_field = matches!(self.password.style, FieldStyle::Terminal | FieldStyle::Recessed);
        self.theme.light && !dark_card && !dark_field
    }

    /// The look this config matches exactly, if any (Meridian with its
    /// light colours on or off).
    pub fn current_look(&self) -> Option<&'static str> {
        LOOKS.iter().map(|(id, _, _)| *id).find(|id| {
            let mut probe = self.clone();
            probe.apply_look(id);
            if *id == "meridian" {
                probe.theme.light = self.theme.light;
            }
            probe == *self
        })
    }
}

/// Ghostline's sign-in panel and clock: this far in from their edges.
const GHOSTLINE_INSET: f32 = 104.0;
/// Chrome Halo's sign-in module: this far in from the right edge.
const CHROME_HALO_INSET: f32 = 120.0;

/// The flagship arrangement: date over a big clock at the top, the user
/// card below center, session bottom-left, power bottom-right.
pub fn default_layout() -> Vec<Placement> {
    vec![
        place(Element::Date, Anchor::Top, 0.0, L::DATE_TOP, true, 1),
        place(Element::Clock, Anchor::Top, 0.0, L::CLOCK_TOP, true, 1),
        place(Element::User, Anchor::Center, 0.0, L::CARD_CENTER, true, 2),
        place(Element::Password, Anchor::Center, 0.0, L::FIELD_CENTER, true, 3),
        place(Element::Users, Anchor::Center, 0.0, L::USERS_CENTER, true, 2),
        place(Element::Session, Anchor::BottomLeft, 34.0, -34.0, true, 2),
        place(Element::Power, Anchor::BottomRight, -34.0, -34.0, true, 2),
        place(Element::Keyboard, Anchor::Bottom, 0.0, -38.0, true, 2),
        place(Element::Banner, Anchor::Top, 0.0, 34.0, false, 2),
        // Mandara's emblem, above the keyboard pill.
        place(Element::Logo, Anchor::Bottom, 0.0, -96.0, true, 2),
        place(Element::Themes, Anchor::TopRight, -28.0, 28.0, false, 2),
    ]
}

// ── Root ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GreeterConfig {
    /// Public decorations; never loaded from a signed-in user's data.
    pub widgets: mandara_config::preauth::Widgets,
    /// `auto` (from the environment) or a language code.
    pub language: String,
    /// The family for the login screen's text (empty = the interface font;
    /// the clock and date have their own).
    pub font: String,
    pub background: BackgroundConfig,
    pub clock: ClockConfig,
    pub date: DateConfig,
    pub user: UserConfig,
    pub password: PasswordConfig,
    pub session: SessionConfig,
    pub power: PowerConfig,
    pub keyboard: KeyboardConfig,
    pub banner: BannerConfig,
    pub card: CardConfig,
    pub logo: LogoConfig,
    pub theme: ThemeConfig,
    pub motion: MotionConfig,
    pub accessibility: AccessibilityConfig,
    pub layout: Vec<Placement>,
    /// The layout defaults this file was written for ([`LAYOUT_VERSION`]);
    /// missing (0) in older files, which [`GreeterConfig::normalize`]
    /// brings up to date.
    #[serde(default)]
    pub layout_version: u32,
}

impl Default for GreeterConfig {
    fn default() -> Self {
        Self {
            widgets: mandara_config::preauth::Widgets::default(),
            language: "auto".into(),
            font: String::new(),
            background: BackgroundConfig::default(),
            clock: ClockConfig::default(),
            date: DateConfig::default(),
            user: UserConfig::default(),
            password: PasswordConfig::default(),
            session: SessionConfig::default(),
            power: PowerConfig::default(),
            keyboard: KeyboardConfig::default(),
            banner: BannerConfig::default(),
            card: CardConfig::default(),
            logo: LogoConfig::default(),
            theme: ThemeConfig::default(),
            motion: MotionConfig::default(),
            accessibility: AccessibilityConfig::default(),
            layout: default_layout(),
            layout_version: LAYOUT_VERSION,
        }
    }
}

impl GreeterConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut c: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        c.normalize();
        Ok(c)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Clamps numbers into sane ranges and makes the layout complete (every
    /// element exactly once).
    pub fn normalize(&mut self) {
        self.widgets.normalize();
        let b = &mut self.background;
        b.playback_rate = b.playback_rate.clamp(0.1, 4.0);
        for v in [&mut b.dim, &mut b.vignette, &mut b.grain, &mut b.blur, &mut b.tint_strength, &mut b.orb_intensity] {
            *v = v.clamp(0.0, 1.0);
        }
        b.ken_burns = b.ken_burns.clamp(0.0, 0.5);
        b.parallax = b.parallax.clamp(0.0, 80.0);
        b.clip_seconds = b.clip_seconds.clamp(3, 3600);
        b.still_seconds = b.still_seconds.clamp(3, 3600);
        self.clock.size = self.clock.size.clamp(24.0, 420.0);
        self.clock.weight = self.clock.weight.clamp(100, 900);
        // Below 100 the idle clock keeps the awake weight.
        self.clock.idle_weight = self.clock.idle_weight.min(900);
        self.date.weight = self.date.weight.clamp(100, 900);
        self.date.size = self.date.size.clamp(8.0, 120.0);
        self.user.avatar_size = self.user.avatar_size.clamp(32.0, 320.0);
        self.password.width = self.password.width.clamp(160.0, 720.0);
        self.password.height = self.password.height.clamp(28.0, 96.0);
        self.upgrade_untouched_layout();
        let t = &mut self.theme;
        t.glass = t.glass.clamp(0.0, 1.0);
        t.rim = t.rim.clamp(0.0, 1.0);
        t.roundness = t.roundness.clamp(0.0, 1.0);
        let defaults = default_layout();
        let mut seen = std::collections::HashSet::new();
        self.layout.retain(|p| seen.insert(p.element));
        for d in defaults {
            if !seen.contains(&d.element) {
                self.layout.push(d);
            }
        }
        for p in &mut self.layout {
            p.scale = p.scale.clamp(0.25, 4.0);
            p.x = p.x.clamp(-4000.0, 4000.0);
            p.y = p.y.clamp(-4000.0, 4000.0);
        }
    }

    pub fn placement(&self, e: Element) -> Placement {
        self.layout.iter().find(|p| p.element == e).cloned().unwrap_or_default()
    }

    pub fn placement_mut(&mut self, e: Element) -> &mut Placement {
        if !self.layout.iter().any(|p| p.element == e) {
            self.layout.push(Placement { element: e, ..Default::default() });
        }
        self.layout.iter_mut().find(|p| p.element == e).expect("just inserted")
    }

    pub fn load_from(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        match Self::parse(&text) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("{}: {e}; using defaults", path.display());
                None
            }
        }
    }

    /// The system config, or the user's draft in demo mode.
    pub fn load(demo: bool) -> Self {
        if demo && let Some(c) = Self::load_from(&user_draft_path()) {
            return c;
        }
        Self::load_from(Path::new(SYSTEM_CONFIG)).unwrap_or_default()
    }

    /// Writes the user draft (Studio).
    pub fn save_draft(&self) -> std::io::Result<PathBuf> {
        self.save_draft_to(user_draft_path())
    }

    /// Writes a draft to `path` (atomically, with the install hint).
    pub fn save_draft_to(&self, path: PathBuf) -> std::io::Result<PathBuf> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("# Mandara Login draft (Studio → Login screen). Install with `mandara greeter apply-config`.\n{}", self.to_toml()))?;
        std::fs::rename(tmp, &path)?;
        Ok(path)
    }

    /// The UI language.
    pub fn lang(&self) -> mandara_i18n::Lang {
        match self.language.trim() {
            "" | "auto" => mandara_i18n::Lang::from_env(),
            code => mandara_i18n::Lang::from_code(code).unwrap_or_else(mandara_i18n::Lang::from_env),
        }
    }

    /// Video folders: the configured ones, plus the user's own in demo mode.
    pub fn video_dirs(&self, demo: bool) -> Vec<PathBuf> {
        let mut dirs = with_local(self.background.video_dirs.clone(), LOCAL_VIDEOS);
        if demo && let Some(data) = dirs::data_dir() {
            dirs.push(data.join("mandara/login/videos"));
        }
        dirs
    }

    /// Still-image folders (+ the user's wallpapers in demo mode).
    pub fn image_dirs(&self, demo: bool) -> Vec<PathBuf> {
        let mut dirs = with_local(self.background.image_dirs.clone(), LOCAL_IMAGES);
        if demo {
            if let Some(data) = dirs::data_dir() {
                dirs.push(data.join("mandara/login/backgrounds"));
            }
            if let Some(pics) = dirs::picture_dir().or_else(|| dirs::home_dir().map(|h| h.join("Pictures"))) {
                dirs.push(pics.join("Wallpapers"));
            }
        }
        dirs
    }
}

/// Orders `paths` by `playlist` (file names), unlisted ones after.
pub fn apply_playlist(paths: &mut [PathBuf], playlist: &[String]) {
    if playlist.is_empty() {
        return;
    }
    let rank = |p: &PathBuf| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        playlist.iter().position(|n| *n == name).unwrap_or(usize::MAX)
    };
    paths.sort_by_key(rank);
}

/// Still images in `dirs` (png / jpeg / webp), sorted per folder.
pub fn list_images(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
            })
            .collect();
        found.sort();
        out.extend(found);
    }
    out
}

/// `mandara greeter apply-config`: validates `from` and installs it as
/// the system config (root only; the previous file is kept as `.bak`).
pub fn apply_config(from: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(from).map_err(|e| format!("{}: {e}", from.display()))?;
    let cfg = GreeterConfig::parse(&text).map_err(|e| format!("{} is not a valid greeter config: {e}", from.display()))?;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Err("not running as root: run `sudo mandara greeter apply-config` (or `pkexec`); nothing was changed".into());
    }
    let dst = Path::new(SYSTEM_CONFIG);
    std::fs::create_dir_all(dst.parent().unwrap_or(Path::new("/etc"))).map_err(|e| e.to_string())?;
    if dst.exists() {
        let _ = std::fs::copy(dst, dst.with_extension("toml.bak"));
    }
    let body = format!("# Mandara Login — installed by `mandara greeter apply-config` from {}\n{}", from.display(), cfg.to_toml());
    let tmp = dst.with_extension("toml.new");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
    std::fs::rename(&tmp, dst).map_err(|e| e.to_string())?;
    // Where sessions share their wallpaper (installs from before it existed
    // get it the next time the config is applied).
    if let Err(e) = mandara_config::paths::ensure_login_wallpaper_dir(Path::new("/")) {
        tracing::warn!("{}: {e}", mandara_config::paths::LOGIN_WALLPAPER_DIR);
    }
    let home_media: Vec<String> = cfg
        .background
        .video_dirs
        .iter()
        .chain(&cfg.background.image_dirs)
        .filter(|d| d.starts_with("/home") || d.starts_with("/root"))
        .map(|d| d.display().to_string())
        .collect();
    let mut msg = format!("Installed {SYSTEM_CONFIG}; it takes effect at the next login screen.");
    // The login screen reads it with its own copy, which an update of your
    // copy leaves behind: an older one can't read what this build wrote.
    match std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| crate::install::refresh_installed_exe(Path::new("/"), &exe)) {
        Ok(true) => msg.push_str(&format!("\nUpdated the login screen's copy of Mandara ({}) to this version.", crate::install::INSTALLED_EXE)),
        Ok(false) => {}
        Err(e) => msg.push_str(&format!("\nThe login screen's copy of Mandara wasn't updated ({e}); it may not read this config until `sudo mandara greeter use mandara` runs.")),
    }
    // Its language's pack, where the login screen's copy reads it.
    match crate::install::install_language_pack(Path::new("/"), cfg.lang()) {
        Ok(Some(line)) => msg.push_str(&format!("\nInstalled the {line}.")),
        Ok(None) => {}
        Err(e) => msg.push_str(&format!("\nThe {} language pack wasn't installed ({e}); the login screen shows English until it is.", cfg.lang().code())),
    }
    if !home_media.is_empty() {
        msg.push_str(&format!(
            "\nNote: the greeter user usually can't read {}; put clips and images in {SYSTEM_VIDEOS} / {SYSTEM_IMAGES}.",
            home_media.join(", ")
        ));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_media_folders_are_searched_once_when_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("videos");
        let base = vec![PathBuf::from(SYSTEM_VIDEOS)];
        assert_eq!(with_local(base.clone(), &local.to_string_lossy()), base, "missing: not added");
        std::fs::create_dir(&local).unwrap();
        assert_eq!(with_local(base.clone(), &local.to_string_lossy()), [PathBuf::from(SYSTEM_VIDEOS), local.clone()]);
        assert_eq!(with_local(vec![local.clone()], &local.to_string_lossy()), std::slice::from_ref(&local), "listed once");
    }

    use super::*;

    #[test]
    fn the_public_widget_guide_parses_without_private_content() {
        let guide = include_str!("../../../docs/login-manager.md");
        let text = guide.split("```toml\n# Public login widgets\n").nth(1).unwrap().split("```").next().unwrap();
        let cfg = GreeterConfig::parse(text).unwrap();
        assert_eq!(cfg.widgets.items.len(), 1);
        assert!(cfg.widgets.weather.is_none());
        assert!(matches!(&cfg.widgets.items[0].kind, mandara_config::desktop_widgets::WidgetKind::Clock(c) if c.face == mandara_config::desktop_widgets::ClockFace::Word));
        let mut private = cfg;
        private.widgets.items.push(mandara_config::desktop_widgets::DesktopWidget { kind: mandara_config::desktop_widgets::WidgetKind::Note(Default::default()), ..Default::default() });
        let parsed = GreeterConfig::parse(&private.to_toml()).unwrap();
        assert_eq!(parsed.widgets.items.len(), 1);
    }

    #[test]
    fn looks_apply_and_are_recognised() {
        let mut cfg = GreeterConfig::default();
        cfg.user.hidden = vec!["guest".into()];
        cfg.language = "ar".into();
        assert_eq!(cfg.current_look(), Some("mandara"));
        assert!(cfg.apply_look("midnight-card"));
        assert!(cfg.card.enabled && cfg.password.submit == SubmitButton::Beside && cfg.theme.preset == "night-citrine");
        assert_eq!(cfg.current_look(), Some("midnight-card"));
        assert_eq!((cfg.user.hidden.clone(), cfg.language.clone()), (vec!["guest".to_string()], "ar".to_string()), "who and what language stay");
        assert!(mandara_theme::catalog::find(&cfg.theme.preset).is_some());
        cfg.clock.size = 120.0;
        assert_eq!(cfg.current_look(), None, "edited after applying");
        assert!(cfg.apply_look("mandara") && cfg.current_look() == Some("mandara") && !cfg.card.enabled);
        assert!(!cfg.apply_look("nope"));
    }

    /// Every look survives the draft file and is recognised again, from any
    /// other look, and each shows the logo.
    #[test]
    fn every_look_round_trips_and_shows_the_logo() {
        for (from, _, _) in LOOKS {
            for (id, _, _) in LOOKS {
                let mut cfg = GreeterConfig::default();
                assert!(cfg.apply_look(from));
                assert!(cfg.apply_look(id));
                assert_eq!(cfg.current_look(), Some(id), "{from} → {id}");
                let back = GreeterConfig::parse(&cfg.to_toml()).unwrap();
                assert_eq!(back, cfg, "{id} through TOML");
                assert_eq!(back.current_look(), Some(id));
                let logo = back.placement(Element::Logo);
                assert!(logo.visible && logo.scale > 0.0 && back.logo.size >= 16.0 && back.logo.opacity > 0.5, "{id}: the logo shows");
                assert!(back.logo.path.is_empty(), "{id}: Mandara's emblem");
            }
        }
        let mut you = GreeterConfig::default();
        you.apply_look("material-you");
        assert!(you.theme.from_wallpaper && you.card.enabled && you.clock.stacked);
        assert_eq!((you.user.avatar_shape, you.password.style, you.session.style, you.power.style), (AvatarShape::Cookie, FieldStyle::Filled, SessionStyle::Chip, PowerStyle::Tonal));
        assert!(mandara_theme::catalog::find(&you.theme.preset).is_some(), "the fallback theme exists");
        // Back to Mandara: nothing of the Material look is left.
        you.apply_look("mandara");
        assert_eq!(you, GreeterConfig::default());
    }

    /// The Material You look's stacked clock, tonal card and chips fit on
    /// screens from 600 to 2160 logical pixels tall (sizes scale with the
    /// height, clamped like the view's).
    #[test]
    fn the_material_layout_fits_every_screen() {
        let mut cfg = GreeterConfig::default();
        cfg.apply_look("material-you");
        let clock = cfg.placement(Element::Clock);
        let card = cfg.placement(Element::User);
        let users = cfg.placement(Element::Users);
        // Two lines of digits (the view's line is 0.98 of the size).
        let clock_bottom = clock.y + 2.0 * cfg.clock.size * 0.98;
        // Avatar, name, field, status and session chip with the card's gaps.
        let inner = cfg.user.avatar_size + 14.0 + cfg.user.name_size * 1.3 + cfg.password.height + 6.0 + 20.0 + 32.0;
        let card_h = inner + 2.0 * cfg.card.padding + 3.0 * 14.0;
        for height in [600.0, 768.0, 900.0, 1080.0, 1440.0, 2160.0] {
            let u = (height / 1080.0f32).clamp(0.6, 2.0);
            let top = height / 2.0 + (card.y - card_h / 2.0) * u;
            let bottom = height / 2.0 + (card.y + card_h / 2.0) * u;
            assert!(top > clock_bottom * u, "{height}: the card ({top}) clears the clock ({})", clock_bottom * u);
            assert!(height / 2.0 + users.y * u > bottom, "{height}: the user chips sit under the card");
            assert!(height / 2.0 + users.y * u + 36.0 * u < height, "{height}: the chips stay on screen");
        }
    }

    /// The emblem was hidden by the old default layout; installed configs
    /// written in full keep their other choices and now show it, once.
    #[test]
    fn old_layouts_show_the_emblem_once() {
        let mut old = toml::Value::try_from(GreeterConfig::default()).unwrap();
        let table = old.as_table_mut().unwrap();
        table.remove("layout_version");
        for p in table.get_mut("layout").unwrap().as_array_mut().unwrap() {
            if p.get("element").and_then(|e| e.as_str()) == Some("logo") {
                p.as_table_mut().unwrap().insert("visible".into(), false.into());
            }
        }
        let text = toml::to_string(&old).unwrap();
        let cfg = GreeterConfig::parse(&text).unwrap();
        assert!(cfg.placement(Element::Logo).visible, "the default layout shows the emblem now");
        assert_eq!(cfg.layout_version, LAYOUT_VERSION);
        assert_eq!(cfg.current_look(), Some("mandara"));
        // Turned off on purpose since: it stays off.
        let mut off = cfg;
        off.placement_mut(Element::Logo).visible = false;
        assert!(!GreeterConfig::parse(&off.to_toml()).unwrap().placement(Element::Logo).visible);
        // A moved logo was a choice too.
        let moved = text.replacen("y = -96.0", "y = -140.0", 1);
        assert_ne!(moved, text);
        let cfg = GreeterConfig::parse(&moved).unwrap();
        assert_eq!(cfg.placement(Element::Logo).y, -140.0);
        assert!(!cfg.placement(Element::Logo).visible);
        // A partial config gets the whole default layout, emblem included.
        assert!(GreeterConfig::parse("language = \"ar\"\n").unwrap().placement(Element::Logo).visible);
    }

    /// An installed config at the old card defaults moves to the shared
    /// proportions; a customised one stays.
    #[test]
    fn untouched_old_layouts_get_the_new_card_positions() {
        let mut old = GreeterConfig::default();
        for (e, y) in OLD_CARD_CENTERS {
            old.layout.iter_mut().find(|p| p.element == e).unwrap().y = y;
        }
        (old.password.width, old.password.height) = OLD_FIELD;
        let mut cfg = old.clone();
        cfg.normalize();
        assert_eq!(cfg.layout.iter().find(|p| p.element == Element::User).unwrap().y, L::CARD_CENTER);
        assert_eq!((cfg.password.width, cfg.password.height), (L::FIELD_W, L::FIELD_H));
        let mut moved = old;
        moved.layout.iter_mut().find(|p| p.element == Element::User).unwrap().y = 140.0;
        moved.normalize();
        assert_eq!(moved.layout.iter().find(|p| p.element == Element::Password).unwrap().y, 254.0, "a moved card keeps its layout");
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let c = GreeterConfig::parse("language = \"ar\"\n[background]\nmode = \"stills\"\nvideo = \"lite\"\n[theme]\npreset = \"mandara-obsidian\"\n").unwrap();
        assert_eq!(c.theme.preset, "mandara-obsidian");
        assert_eq!(c.background.video, VideoMode::Lite);
        assert_eq!(c.background.mode, BackgroundMode::Stills);
        assert_eq!(c.lang(), mandara_i18n::Lang::Ar);
        assert!(c.clock.format_24h);
        assert_eq!(c.layout.len(), Element::ALL.len());
        assert!(GreeterConfig::parse("[background]\nmode = 3").is_err());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut c = GreeterConfig::default();
        c.placement_mut(Element::Clock).anchor = Anchor::BottomLeft;
        c.banner.text = "Authorized use only".into();
        let back = GreeterConfig::parse(&c.to_toml()).unwrap();
        assert_eq!(back, c);
    }

    /// Every leaf moved off its default, within the ranges `normalize`
    /// keeps: numbers nudged, switches flipped.
    fn nudged(v: &toml::Value) -> toml::Value {
        use toml::Value as V;
        match v {
            V::Boolean(b) => V::Boolean(!b),
            V::Integer(i) => V::Integer(i + 1),
            V::Float(f) => V::Float(if *f > 0.5 { f - 0.05 } else { f + 0.05 }),
            V::Table(t) => V::Table(t.iter().map(|(k, v)| (k.clone(), nudged(v))).collect()),
            V::Array(a) => V::Array(a.iter().map(nudged).collect()),
            other => other.clone(),
        }
    }

    /// Paths where `a` and `b` differ (floats within f32 precision).
    fn differences(a: &toml::Value, b: &toml::Value, path: &str, out: &mut Vec<String>) {
        use toml::Value as V;
        match (a, b) {
            (V::Table(x), V::Table(y)) => {
                for (k, v) in x {
                    match y.get(k) {
                        Some(w) => differences(v, w, &format!("{path}.{k}"), out),
                        None => out.push(format!("{path}.{k} (missing)")),
                    }
                }
            }
            (V::Array(x), V::Array(y)) if x.len() == y.len() => x.iter().zip(y).enumerate().for_each(|(i, (v, w))| differences(v, w, &format!("{path}[{i}]"), out)),
            (V::Float(x), V::Float(y)) if (x - y).abs() < 1e-4 => {}
            (x, y) if x == y => {}
            _ => out.push(format!("{path}: {a} → {b}")),
        }
    }

    /// Every option Studio's Login screen page edits survives the draft
    /// file and `apply-config`'s parse (what the real greeter reads).
    #[test]
    fn every_field_survives_the_draft_file() {
        let defaults = toml::Value::try_from(GreeterConfig::default()).unwrap();
        let changed = nudged(&defaults);
        let cfg: GreeterConfig = changed.clone().try_into().unwrap();
        assert_ne!(cfg, GreeterConfig::default());
        let dir = tempfile::tempdir().unwrap();
        let path = cfg.save_draft_to(dir.path().join("mandara/greeter.toml")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Mandara Login draft"), "{text}");
        assert!(!dir.path().join("mandara/greeter.toml.tmp").exists(), "atomic write leaves no temp file");
        let back = GreeterConfig::load_from(&path).expect("the draft parses");
        let mut diff = Vec::new();
        differences(&changed, &toml::Value::try_from(&back).unwrap(), "", &mut diff);
        assert!(diff.is_empty(), "options lost or clamped on the way through the draft: {diff:#?}");
        // apply-config validates the same text before it needs root.
        assert!(GreeterConfig::parse(&text).is_ok());
    }

    #[test]
    fn layout_edits_round_trip() {
        let mut c = GreeterConfig::default();
        for (i, e) in Element::ALL.into_iter().enumerate() {
            let p = c.placement_mut(e);
            p.anchor = Anchor::ALL[i % Anchor::ALL.len()];
            p.x = 12.0 * i as f32;
            p.y = -8.0 * i as f32;
            p.scale = 1.25;
            p.visible = i % 2 == 0;
        }
        let back = GreeterConfig::parse(&c.to_toml()).unwrap();
        for e in Element::ALL {
            assert_eq!(back.placement(e), c.placement(e), "{e:?}");
        }
    }

    #[test]
    fn normalize_clamps_and_completes_the_layout() {
        let text = "[background]\ndim = 7.0\nplayback_rate = 0\n[[layout]]\nelement = \"clock\"\nscale = 99\n[[layout]]\nelement = \"clock\"\nx = 5\n";
        let c = GreeterConfig::parse(text).unwrap();
        assert_eq!(c.background.dim, 1.0);
        assert_eq!(c.background.playback_rate, 0.1);
        assert_eq!(c.layout.iter().filter(|p| p.element == Element::Clock).count(), 1);
        assert_eq!(c.placement(Element::Clock).scale, 4.0);
        assert_eq!(c.layout.len(), Element::ALL.len());
    }

    #[test]
    fn anchors() {
        assert_eq!(Anchor::TopLeft.fraction(), (0.0, 0.0));
        assert_eq!(Anchor::Center.fraction(), (0.5, 0.5));
        assert_eq!(Anchor::BottomRight.fraction(), (1.0, 1.0));
        for a in Anchor::ALL {
            let (fx, fy) = a.fraction();
            assert_eq!(Anchor::from_fraction(fx, fy), a);
        }
        assert_eq!(Anchor::Bottom.point(0.0, 0.0, 100.0, 50.0), (50.0, 50.0));
    }

    #[test]
    fn demo_adds_user_folders_only_in_demo() {
        let c = GreeterConfig::default();
        assert_eq!(c.video_dirs(false).len(), 1);
        assert!(c.video_dirs(true).len() >= c.video_dirs(false).len());
        assert!(!c.image_dirs(false).iter().any(|d| d.ends_with("Wallpapers")));
    }

    #[test]
    fn playlist_order() {
        let mut v = vec![PathBuf::from("/a/x.webm"), PathBuf::from("/a/y.webm"), PathBuf::from("/a/z.webm")];
        apply_playlist(&mut v, &["z.webm".into(), "x.webm".into()]);
        assert_eq!(v, vec![PathBuf::from("/a/z.webm"), PathBuf::from("/a/x.webm"), PathBuf::from("/a/y.webm")]);
    }

    #[test]
    fn lists_images() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["b.jpg", "a.PNG", "c.txt", "d.webp"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let found: Vec<String> = list_images(&[dir.path().to_path_buf()]).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(found, vec!["a.PNG", "b.jpg", "d.webp"]);
    }

    #[test]
    fn apply_config_validates_before_needing_root() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "[background]\nmode = 3\n").unwrap();
        assert!(apply_config(&bad).unwrap_err().contains("not a valid"));
        if unsafe { libc::geteuid() } != 0 {
            let good = dir.path().join("good.toml");
            std::fs::write(&good, GreeterConfig::default().to_toml()).unwrap();
            assert!(apply_config(&good).unwrap_err().contains("not running as root"));
        }
    }
}
