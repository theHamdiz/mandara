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

use serde::{Deserialize, Serialize};

pub const SYSTEM_CONFIG: &str = "/etc/mandara/greeter.toml";
pub const SYSTEM_VIDEOS: &str = "/usr/share/mandara/login/videos";
pub const SYSTEM_IMAGES: &str = "/usr/share/mandara/login/backgrounds";

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
    /// Pointer parallax depth in px (0 = off).
    pub parallax: f32,
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
            parallax: 14.0,
        }
    }
}

// ── Clock & date ────────────────────────────────────────────────────────

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
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self { format_24h: true, seconds: false, font: String::new(), size: 150.0, weight: 700, opacity: 0.95, roll_digits: true }
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
        Self { format: String::new(), font: String::new(), size: 24.0, weight: 600, opacity: 0.9 }
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
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            list: UserList::All,
            avatar_shape: AvatarShape::Circle,
            avatar_source: AvatarSource::Auto,
            avatar_size: 108.0,
            glow_ring: true,
            show_name: true,
            name_size: 20.0,
            hidden: Vec::new(),
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasswordConfig {
    pub style: FieldStyle,
    pub width: f32,
    pub height: f32,
    /// Placeholder (empty = "Enter Password" in the current language).
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
        Self { style: FieldStyle::Glass, width: 290.0, height: 44.0, placeholder: String::new(), caps_hint: true, reveal_toggle: true, keystroke_ripple: true, glow: true }
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
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { default: String::new(), x11: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    pub suspend: bool,
    pub reboot: bool,
    pub poweroff: bool,
    pub labels: bool,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self { suspend: true, reboot: true, poweroff: true, labels: true }
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
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self { preset: "montaza-twilight".into(), accent: String::new(), glass: 0.18, rim: 0.6, roundness: 1.0 }
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
}

impl Element {
    pub const ALL: [Element; 9] = [Self::Clock, Self::Date, Self::User, Self::Password, Self::Users, Self::Session, Self::Power, Self::Keyboard, Self::Banner];

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

/// The flagship arrangement: date over a big clock at the top, the user
/// card below center, session bottom-left, power bottom-right.
pub fn default_layout() -> Vec<Placement> {
    vec![
        place(Element::Date, Anchor::Top, 0.0, 78.0, true, 1),
        place(Element::Clock, Anchor::Top, 0.0, 108.0, true, 1),
        place(Element::User, Anchor::Center, 0.0, 118.0, true, 2),
        place(Element::Password, Anchor::Center, 0.0, 254.0, true, 3),
        place(Element::Users, Anchor::Center, 0.0, 350.0, true, 2),
        place(Element::Session, Anchor::BottomLeft, 34.0, -34.0, true, 2),
        place(Element::Power, Anchor::BottomRight, -34.0, -34.0, true, 2),
        place(Element::Keyboard, Anchor::Bottom, 0.0, -38.0, true, 2),
        place(Element::Banner, Anchor::Top, 0.0, 34.0, false, 2),
    ]
}

// ── Root ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GreeterConfig {
    /// `auto` (from the environment) or a language code.
    pub language: String,
    pub background: BackgroundConfig,
    pub clock: ClockConfig,
    pub date: DateConfig,
    pub user: UserConfig,
    pub password: PasswordConfig,
    pub session: SessionConfig,
    pub power: PowerConfig,
    pub keyboard: KeyboardConfig,
    pub banner: BannerConfig,
    pub theme: ThemeConfig,
    pub motion: MotionConfig,
    pub accessibility: AccessibilityConfig,
    pub layout: Vec<Placement>,
}

impl Default for GreeterConfig {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            background: BackgroundConfig::default(),
            clock: ClockConfig::default(),
            date: DateConfig::default(),
            user: UserConfig::default(),
            password: PasswordConfig::default(),
            session: SessionConfig::default(),
            power: PowerConfig::default(),
            keyboard: KeyboardConfig::default(),
            banner: BannerConfig::default(),
            theme: ThemeConfig::default(),
            motion: MotionConfig::default(),
            accessibility: AccessibilityConfig::default(),
            layout: default_layout(),
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
        self.date.weight = self.date.weight.clamp(100, 900);
        self.date.size = self.date.size.clamp(8.0, 120.0);
        self.user.avatar_size = self.user.avatar_size.clamp(32.0, 320.0);
        self.password.width = self.password.width.clamp(160.0, 720.0);
        self.password.height = self.password.height.clamp(28.0, 96.0);
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
        let mut dirs = self.background.video_dirs.clone();
        if demo && let Some(data) = dirs::data_dir() {
            dirs.push(data.join("mandara/login/videos"));
        }
        dirs
    }

    /// Still-image folders (+ the user's wallpapers in demo mode).
    pub fn image_dirs(&self, demo: bool) -> Vec<PathBuf> {
        let mut dirs = self.background.image_dirs.clone();
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
    let home_media: Vec<String> = cfg
        .background
        .video_dirs
        .iter()
        .chain(&cfg.background.image_dirs)
        .filter(|d| d.starts_with("/home") || d.starts_with("/root"))
        .map(|d| d.display().to_string())
        .collect();
    let mut msg = format!("Installed {SYSTEM_CONFIG}; it takes effect at the next login screen.");
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
    use super::*;

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
