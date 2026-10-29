//! The wallpaper generator's settings, `[plugins.settings.wallpaper-generator]`
//! (Studio → Wallpaper → Generator).
//!
//! The current recipe is kept as the engine's own TOML table
//! (`mandara_wallgen::Recipe`); the plugin parses and sanitizes it, so this
//! crate needs no engine dependency. Everything else is typed here: output
//! size and format, where files go, the opt-in schedule and video loops.
//!
//! Unknown keys are ignored and invalid values fall back to their defaults
//! one by one (in nested tables too); [`WallgenSettings::from_plugins`]
//! returns values already clamped to their ranges.

use serde::{Deserialize, Serialize};

/// The plugin id, also the name of its settings table.
pub const WALLGEN_ID: &str = "wallpaper-generator";

/// Defaults and ranges, with their units in the name.
pub mod defaults {
    /// Scheduled runs: every 6 hours.
    pub const SCHEDULE_EVERY_MIN: u32 = 360;
    /// Shortest and longest schedule interval (15 minutes to a week).
    pub const SCHEDULE_EVERY_RANGE_MIN: (u32, u32) = (15, 7 * 24 * 60);
    /// Generated files kept by the schedule (0 keeps everything).
    pub const KEEP: u32 = 30;
    pub const KEEP_RANGE: (u32, u32) = (0, 1000);
    /// Variation strength: share of each parameter's nice range a
    /// variation may move (0–1).
    pub const VARIATION: f32 = 0.6;
    pub const VARIATION_RANGE: (f32, f32) = (0.05, 1.0);
    /// JPEG quality when JPEG is chosen (visually lossless default).
    pub const JPEG_QUALITY: u8 = 92;
    pub const JPEG_QUALITY_RANGE: (u8, u8) = (60, 100);
    /// Custom output size: default and per-side limits, px.
    pub const CUSTOM_W_PX: u32 = 3840;
    pub const CUSTOM_H_PX: u32 = 2160;
    pub const CUSTOM_SIDE_RANGE_PX: (u32, u32) = (640, 16384);
    /// Video loops: length, frame rate and quality (0 smallest – 100 best).
    pub const VIDEO_SECONDS: f32 = 10.0;
    pub const VIDEO_SECONDS_RANGE: (f32, f32) = (2.0, 60.0);
    pub const VIDEO_FPS: u32 = 24;
    pub const VIDEO_FPS_RANGE: (u32, u32) = (12, 60);
    pub const VIDEO_QUALITY: u8 = 70;
    pub const VIDEO_QUALITY_RANGE: (u8, u8) = (0, 100);
    /// Output sub-folder of the wallpaper folder when none is set (the
    /// picker scans one level of sub-folders, so results show up there).
    pub const SUBFOLDER: &str = "Generated";
}

/// Light or dark wallpapers.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallgenTone {
    /// Follow the shell's light / dark mode.
    #[default]
    Theme,
    Dark,
    Light,
}

impl WallgenTone {
    pub const ALL: [WallgenTone; 3] = [WallgenTone::Theme, WallgenTone::Dark, WallgenTone::Light];

    /// Whether wallpapers come out light while the shell is `theme_dark`.
    pub fn is_light(self, theme_dark: bool) -> bool {
        match self {
            WallgenTone::Theme => !theme_dark,
            WallgenTone::Dark => false,
            WallgenTone::Light => true,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            WallgenTone::Theme => "theme",
            WallgenTone::Dark => "dark",
            WallgenTone::Light => "light",
        }
    }
}

/// Output size class. `Display` is the 4K class fitted to the primary
/// display's aspect ratio (3840 × 2160 on 16:9, 3456 × 2160 on 16:10).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WallgenSize {
    #[default]
    #[serde(rename = "display")]
    Display,
    #[serde(rename = "1080p")]
    Fhd1080,
    #[serde(rename = "1440p")]
    Qhd1440,
    #[serde(rename = "4k")]
    Uhd4k,
    #[serde(rename = "5k")]
    Uhd5k,
    #[serde(rename = "8k")]
    Uhd8k,
    #[serde(rename = "uw1440")]
    Ultrawide1440,
    #[serde(rename = "portrait4k")]
    Portrait4k,
    /// `custom_width × custom_height`.
    #[serde(rename = "custom")]
    Custom,
}

impl WallgenSize {
    pub const ALL: [WallgenSize; 9] = [
        WallgenSize::Display,
        WallgenSize::Fhd1080,
        WallgenSize::Qhd1440,
        WallgenSize::Uhd4k,
        WallgenSize::Uhd5k,
        WallgenSize::Uhd8k,
        WallgenSize::Ultrawide1440,
        WallgenSize::Portrait4k,
        WallgenSize::Custom,
    ];

    /// The serde id (`display`, `1080p`, …, `custom`).
    pub fn id(self) -> &'static str {
        match self {
            WallgenSize::Display => "display",
            WallgenSize::Fhd1080 => "1080p",
            WallgenSize::Qhd1440 => "1440p",
            WallgenSize::Uhd4k => "4k",
            WallgenSize::Uhd5k => "5k",
            WallgenSize::Uhd8k => "8k",
            WallgenSize::Ultrawide1440 => "uw1440",
            WallgenSize::Portrait4k => "portrait4k",
            WallgenSize::Custom => "custom",
        }
    }

    pub fn from_id(id: &str) -> Option<WallgenSize> {
        WallgenSize::ALL.into_iter().find(|s| s.id() == id.trim().to_ascii_lowercase())
    }
}

/// Image format of generated stills.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallgenFormat {
    /// Exact pixels in the smallest file: lossless WebP or PNG, whichever
    /// comes out smaller for the picture.
    #[default]
    Lossless,
    /// Lossless WebP, always.
    Webp,
    /// PNG, always.
    Png,
    Jpeg,
}

impl WallgenFormat {
    pub const ALL: [WallgenFormat; 4] = [WallgenFormat::Lossless, WallgenFormat::Webp, WallgenFormat::Png, WallgenFormat::Jpeg];

    pub fn id(self) -> &'static str {
        match self {
            WallgenFormat::Lossless => "lossless",
            WallgenFormat::Webp => "webp",
            WallgenFormat::Png => "png",
            WallgenFormat::Jpeg => "jpeg",
        }
    }

    pub fn from_id(id: &str) -> Option<WallgenFormat> {
        match id.trim().to_ascii_lowercase().as_str() {
            "lossless" => Some(WallgenFormat::Lossless),
            "webp" => Some(WallgenFormat::Webp),
            "png" => Some(WallgenFormat::Png),
            "jpeg" | "jpg" => Some(WallgenFormat::Jpeg),
            _ => None,
        }
    }
}

/// What a scheduled run generates.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScheduleMode {
    /// A random style, palette and parameters.
    #[default]
    Random,
    /// A variation of the current recipe.
    Variation,
    /// The current recipe with a new layout seed.
    Reseed,
}

impl ScheduleMode {
    pub const ALL: [ScheduleMode; 3] = [ScheduleMode::Random, ScheduleMode::Variation, ScheduleMode::Reseed];

    pub fn id(self) -> &'static str {
        match self {
            ScheduleMode::Random => "random",
            ScheduleMode::Variation => "variation",
            ScheduleMode::Reseed => "reseed",
        }
    }
}

/// Stills or video loops.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallgenKind {
    #[default]
    Still,
    Video,
}

/// "A new wallpaper every …": off until the user turns it on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallgenSchedule {
    pub enabled: bool,
    /// Minutes between runs.
    pub every_minutes: u32,
    pub mode: ScheduleMode,
    /// Set each result as the wallpaper.
    pub apply: bool,
    /// Keep the newest N generated files in the output folder (0 = all).
    pub keep: u32,
    pub kind: WallgenKind,
}

impl Default for WallgenSchedule {
    fn default() -> Self {
        Self { enabled: false, every_minutes: defaults::SCHEDULE_EVERY_MIN, mode: ScheduleMode::default(), apply: true, keep: defaults::KEEP, kind: WallgenKind::Still }
    }
}

impl WallgenSchedule {
    /// The interval, clamped.
    pub fn interval(&self) -> std::time::Duration {
        const SECS_PER_MIN: u64 = 60;
        let (lo, hi) = defaults::SCHEDULE_EVERY_RANGE_MIN;
        std::time::Duration::from_secs(u64::from(self.every_minutes.clamp(lo, hi)) * SECS_PER_MIN)
    }
}

/// Video loop options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallgenVideo {
    pub seconds: f32,
    pub fps: u32,
    /// Loops are encoded at this size class (1080p by default: AV1 in pure
    /// Rust takes about 30 s for a 10 s 1080p loop).
    pub size: WallgenSize,
    /// 0 (smallest file) – 100 (best picture).
    pub quality: u8,
}

impl Default for WallgenVideo {
    fn default() -> Self {
        Self { seconds: defaults::VIDEO_SECONDS, fps: defaults::VIDEO_FPS, size: WallgenSize::Fhd1080, quality: defaults::VIDEO_QUALITY }
    }
}

/// `[plugins.settings.wallpaper-generator]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallgenSettings {
    /// The current recipe, as the engine's TOML table (empty: the engine's
    /// default recipe).
    pub recipe: toml::Table,
    /// Light or dark wallpapers (default: follow the theme).
    pub tone: WallgenTone,
    pub size: WallgenSize,
    pub custom_width: u32,
    pub custom_height: u32,
    pub format: WallgenFormat,
    pub jpeg_quality: u8,
    /// Output folder (`~` allowed); empty = `Generated` inside the
    /// wallpaper folder.
    pub directory: String,
    /// Set a manually generated wallpaper as the desktop background.
    pub apply: bool,
    /// Keep the layout seed when shuffling or making variations.
    pub seed_locked: bool,
    /// How far variations stray from the recipe (0.05–1).
    pub variation: f32,
    pub schedule: WallgenSchedule,
    pub video: WallgenVideo,
}

impl Default for WallgenSettings {
    fn default() -> Self {
        Self {
            recipe: toml::Table::new(),
            tone: WallgenTone::Theme,
            size: WallgenSize::Display,
            custom_width: defaults::CUSTOM_W_PX,
            custom_height: defaults::CUSTOM_H_PX,
            format: WallgenFormat::Lossless,
            jpeg_quality: defaults::JPEG_QUALITY,
            directory: String::new(),
            apply: true,
            seed_locked: false,
            variation: defaults::VARIATION,
            schedule: WallgenSchedule::default(),
            video: WallgenVideo::default(),
        }
    }
}

/// `v` clamped, or `default` when it isn't finite.
fn clamp_f32(v: f32, (lo, hi): (f32, f32), default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

impl WallgenSettings {
    /// The settings in `[plugins.settings.wallpaper-generator]`, every value
    /// in range. Missing keys take their defaults and an invalid value
    /// falls back on its own, never resetting its neighbours.
    pub fn from_plugins(plugins: &crate::PluginsConfig) -> Self {
        match plugins.settings.get(WALLGEN_ID) {
            Some(table) => Self::from_table(table),
            None => Self::default(),
        }
    }

    /// [`Self::from_plugins`] for one table.
    pub fn from_table(table: &toml::Table) -> Self {
        let parse = |t: &toml::Table| toml::Value::Table(t.clone()).try_into::<Self>();
        // Fast path: the whole table is valid.
        if let Ok(s) = parse(table) {
            return s.sanitized();
        }
        let mut out = toml::Table::try_from(Self::default()).unwrap_or_default();
        for (key, value) in table {
            // Nested option tables (schedule, video) merge key by key too.
            if let (toml::Value::Table(inner), Some(toml::Value::Table(base))) = (value, out.get(key).cloned())
                && key != "recipe"
            {
                let mut merged = base;
                for (k, v) in inner {
                    let mut trial = out.clone();
                    let mut candidate = merged.clone();
                    candidate.insert(k.clone(), v.clone());
                    trial.insert(key.clone(), toml::Value::Table(candidate.clone()));
                    if parse(&trial).is_ok() {
                        merged = candidate;
                    }
                }
                out.insert(key.clone(), toml::Value::Table(merged));
                continue;
            }
            let mut trial = out.clone();
            trial.insert(key.clone(), value.clone());
            if parse(&trial).is_ok() {
                out = trial;
            }
        }
        parse(&out).unwrap_or_default().sanitized()
    }

    /// Writes these settings into `[plugins.settings.wallpaper-generator]`:
    /// only the values that differ from the defaults, so the file stays
    /// small and a better default reaches everyone who never changed it.
    pub fn store(&self, plugins: &mut crate::PluginsConfig) {
        /// `table` without the entries equal to `base`'s (nested tables too).
        fn diff(table: toml::Table, base: &toml::Table) -> toml::Table {
            table
                .into_iter()
                .filter_map(|(k, v)| match (v, base.get(&k)) {
                    (toml::Value::Table(inner), Some(toml::Value::Table(b))) if k != "recipe" => {
                        let d = diff(inner, b);
                        (!d.is_empty()).then_some((k, toml::Value::Table(d)))
                    }
                    (v, Some(b)) if &v == b => None,
                    (v, _) => Some((k, v)),
                })
                .collect()
        }
        let (Ok(table), Ok(base)) = (toml::Table::try_from(self), toml::Table::try_from(Self::default())) else { return };
        let table = diff(table, &base);
        if table.is_empty() {
            plugins.settings.remove(WALLGEN_ID);
        } else {
            plugins.settings.insert(WALLGEN_ID.to_string(), table);
        }
    }

    /// Every value clamped to its range.
    pub fn sanitized(mut self) -> Self {
        let side = defaults::CUSTOM_SIDE_RANGE_PX;
        self.custom_width = self.custom_width.clamp(side.0, side.1);
        self.custom_height = self.custom_height.clamp(side.0, side.1);
        self.jpeg_quality = self.jpeg_quality.clamp(defaults::JPEG_QUALITY_RANGE.0, defaults::JPEG_QUALITY_RANGE.1);
        self.variation = clamp_f32(self.variation, defaults::VARIATION_RANGE, defaults::VARIATION);
        let every = defaults::SCHEDULE_EVERY_RANGE_MIN;
        self.schedule.every_minutes = self.schedule.every_minutes.clamp(every.0, every.1);
        self.schedule.keep = self.schedule.keep.clamp(defaults::KEEP_RANGE.0, defaults::KEEP_RANGE.1);
        self.video.seconds = clamp_f32(self.video.seconds, defaults::VIDEO_SECONDS_RANGE, defaults::VIDEO_SECONDS);
        self.video.fps = self.video.fps.clamp(defaults::VIDEO_FPS_RANGE.0, defaults::VIDEO_FPS_RANGE.1);
        self.video.quality = self.video.quality.clamp(defaults::VIDEO_QUALITY_RANGE.0, defaults::VIDEO_QUALITY_RANGE.1);
        if self.video.size == WallgenSize::Custom {
            // Loops use the presets only (a custom 16K loop would take hours).
            self.video.size = WallgenSize::Fhd1080;
        }
        self
    }

    /// The output folder: [`Self::directory`], else `Generated` inside
    /// `wallpaper_dir` (the picker's folder).
    pub fn output_dir(&self, wallpaper_dir: &std::path::Path) -> std::path::PathBuf {
        let dir = self.directory.trim();
        if dir.is_empty() { wallpaper_dir.join(defaults::SUBFOLDER) } else { crate::paths::expand_tilde(dir) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PluginsConfig, ShellConfig};

    /// The settings from `text`, written as the plugin's table of a whole
    /// config file (`[schedule]` → `[plugins.settings.wallpaper-generator.schedule]`).
    fn with(text: &str) -> WallgenSettings {
        let prefix = "[plugins.settings.wallpaper-generator";
        let body = text.replace("[schedule]", &format!("{prefix}.schedule]")).replace("[video]", &format!("{prefix}.video]"));
        let cfg: ShellConfig = toml::from_str(&format!("{prefix}]\n{body}")).unwrap();
        WallgenSettings::from_plugins(&cfg.plugins)
    }

    #[test]
    fn defaults_are_safe_and_the_schedule_is_opt_in() {
        let s = WallgenSettings::from_plugins(&PluginsConfig::default());
        assert_eq!(s, WallgenSettings::default());
        assert!(!s.schedule.enabled, "scheduling is opt-in");
        assert!(s.apply, "manual runs apply by default");
        assert_eq!(s.format, WallgenFormat::Lossless);
        assert_eq!(s.size, WallgenSize::Display);
        assert_eq!(s.tone, WallgenTone::Theme);
        assert_eq!(s.schedule.interval(), std::time::Duration::from_secs(6 * 3600));
        assert!(s.recipe.is_empty());
        assert_eq!(s.video.size, WallgenSize::Fhd1080);
        assert_eq!(s.output_dir(std::path::Path::new("/w")), std::path::PathBuf::from("/w/Generated"));
    }

    #[test]
    fn round_trips_through_the_plugin_table() {
        let mut s = WallgenSettings { size: WallgenSize::Uhd8k, format: WallgenFormat::Jpeg, tone: WallgenTone::Light, ..Default::default() };
        s.schedule.enabled = true;
        s.schedule.mode = ScheduleMode::Reseed;
        s.schedule.kind = WallgenKind::Video;
        s.video.seconds = 6.5;
        s.recipe.insert("style".into(), toml::Value::String("aurora".into()));
        s.recipe.insert("seed".into(), toml::Value::String("00000000000000ff".into()));
        let mut plugins = PluginsConfig::default();
        s.store(&mut plugins);
        assert_eq!(WallgenSettings::from_plugins(&plugins), s);
        // And through a whole config file.
        let cfg = ShellConfig { plugins, ..Default::default() };
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("[plugins.settings.wallpaper-generator"), "{text}");
        let back: ShellConfig = toml::from_str(&text).unwrap();
        assert_eq!(WallgenSettings::from_plugins(&back.plugins), s);
        // Only what differs from the defaults is written.
        let table = &back.plugins.settings[WALLGEN_ID];
        assert!(table.get("apply").is_none() && table.get("variation").is_none(), "{table:?}");
        assert_eq!(table["schedule"].as_table().unwrap().get("keep"), None);
        assert_eq!(table["size"].as_str(), Some("8k"));
        let mut plugins = PluginsConfig::default();
        WallgenSettings::default().store(&mut plugins);
        assert!(!plugins.settings.contains_key(WALLGEN_ID), "defaults leave no table");
    }

    #[test]
    fn values_clamp_into_range() {
        let s = with("variation = 9.0\njpeg_quality = 3\ncustom_width = 1\ncustom_height = 99999\n[schedule]\nevery_minutes = 1\nkeep = 5000\n[video]\nseconds = 900.0\nfps = 1000\nquality = 255\nsize = \"custom\"\n");
        assert_eq!(s.variation, defaults::VARIATION_RANGE.1);
        assert_eq!(s.jpeg_quality, defaults::JPEG_QUALITY_RANGE.0);
        assert_eq!((s.custom_width, s.custom_height), (defaults::CUSTOM_SIDE_RANGE_PX.0, defaults::CUSTOM_SIDE_RANGE_PX.1));
        assert_eq!(s.schedule.every_minutes, defaults::SCHEDULE_EVERY_RANGE_MIN.0);
        assert_eq!(s.schedule.keep, defaults::KEEP_RANGE.1);
        assert_eq!(s.video.seconds, defaults::VIDEO_SECONDS_RANGE.1);
        assert_eq!(s.video.fps, defaults::VIDEO_FPS_RANGE.1);
        assert_eq!(s.video.quality, defaults::VIDEO_QUALITY_RANGE.1);
        assert_eq!(s.video.size, WallgenSize::Fhd1080, "loops never use a custom size");
        let nan = WallgenSettings { variation: f32::NAN, ..Default::default() }.sanitized();
        assert_eq!(nan.variation, defaults::VARIATION);
    }

    #[test]
    fn bad_values_fall_back_one_by_one() {
        let s = with("format = \"gif\"\napply = false\nsize = \"8k\"\nfuture_key = 1\n[schedule]\nenabled = true\nevery_minutes = \"often\"\nmode = \"variation\"\n[video]\nfps = \"fast\"\nseconds = 4.0\n");
        assert_eq!(s.format, WallgenFormat::Lossless, "unknown format → default");
        // Configs from before the lossless default keep their explicit choice.
        assert_eq!(with("format = \"webp\"\n").format, WallgenFormat::Webp);
        assert_eq!(with("format = \"lossless\"\n").format, WallgenFormat::Lossless);
        assert!(!s.apply);
        assert_eq!(s.size, WallgenSize::Uhd8k);
        assert!(s.schedule.enabled);
        assert_eq!(s.schedule.every_minutes, defaults::SCHEDULE_EVERY_MIN, "bad interval → default, the rest kept");
        assert_eq!(s.schedule.mode, ScheduleMode::Variation);
        assert_eq!(s.video.fps, defaults::VIDEO_FPS);
        assert_eq!(s.video.seconds, 4.0);
        // A recipe that isn't a table is dropped.
        assert!(with("recipe = 5\n").recipe.is_empty());
    }

    #[test]
    fn ids_round_trip() {
        for s in WallgenSize::ALL {
            assert_eq!(WallgenSize::from_id(s.id()), Some(s));
            let t: toml::Table = toml::from_str(&format!("size = \"{}\"", s.id())).unwrap();
            assert_eq!(WallgenSettings::from_table(&t).size, s);
        }
        for f in WallgenFormat::ALL {
            assert_eq!(WallgenFormat::from_id(f.id()), Some(f));
        }
        assert_eq!(WallgenFormat::from_id("JPG"), Some(WallgenFormat::Jpeg));
        assert_eq!(WallgenSize::from_id("12k"), None);
        assert!(WallgenTone::Theme.is_light(false) && !WallgenTone::Theme.is_light(true));
        assert!(WallgenTone::Light.is_light(true) && !WallgenTone::Dark.is_light(false));
    }

    #[test]
    fn output_dir_expands_home() {
        let s = WallgenSettings { directory: "~/Art".into(), ..Default::default() };
        assert_eq!(s.output_dir(std::path::Path::new("/w")), crate::paths::expand_tilde("~/Art"));
        let blank = WallgenSettings { directory: "   ".into(), ..Default::default() };
        assert_eq!(blank.output_dir(std::path::Path::new("/w")), std::path::PathBuf::from("/w/Generated"));
    }
}
