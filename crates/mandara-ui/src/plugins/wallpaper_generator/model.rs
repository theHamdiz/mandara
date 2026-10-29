//! The generator's model, shared by the shell plugin and Studio: the
//! settings' recipe and output choices as engine types, theme palettes,
//! fresh seeds and the library of generated files in the output folder.
//! No GUI types; everything here is unit-tested.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mandara_config::wallgen::{WallgenFormat, WallgenSettings, WallgenSize};
use mandara_theme::MandaraPalette;
use mandara_wallgen::{Format, OutputOptions, PaletteSpec, Recipe, Resolution, Style, VideoOptions};

/// The recipe shown before the user has made one: the first gallery style
/// in the signature Corniche palette, at a fixed layout.
pub const DEFAULT_SEED: u64 = 0x004D_414E_4441_5241;
/// Long side of the class [`WallgenSize::Display`] fits to the display
/// (the 4K class).
const DISPLAY_CLASS: Resolution = Resolution::Uhd4k;
/// Video loops fitted to the display use the 1080p class.
const VIDEO_DISPLAY_CLASS: Resolution = Resolution::Fhd1080;

/// The settings' recipe (sanitized), or the default one when it is empty
/// or unreadable; `light` follows the settings' tone.
pub fn recipe(settings: &WallgenSettings, theme_dark: bool) -> Recipe {
    let mut r = parse_recipe(&settings.recipe).unwrap_or_else(default_recipe);
    r.light = settings.tone.is_light(theme_dark);
    r
}

/// A recipe stored as a TOML table.
pub fn parse_recipe(table: &toml::Table) -> Option<Recipe> {
    if table.is_empty() {
        return None;
    }
    Recipe::from_toml(&toml::to_string(table).ok()?).ok()
}

pub fn default_recipe() -> Recipe {
    Recipe::new(Style::ALL[0], DEFAULT_SEED)
}

/// Stores `recipe` as the settings' current recipe.
pub fn store_recipe(settings: &mut WallgenSettings, recipe: &Recipe) {
    if let Ok(table) = toml::Table::try_from(recipe) {
        settings.recipe = table;
    }
}

/// The engine resolution for a size class. `display` is the primary
/// display's size (any unit: only its aspect ratio counts).
pub fn resolution(size: WallgenSize, custom: (u32, u32), display: Option<(u32, u32)>) -> Resolution {
    let fit = |class: Resolution| match display {
        Some((w, h)) if w > 0 && h > 0 => {
            let (fw, fh) = class.fit_aspect(w, h);
            Resolution::from_id(&format!("{fw}x{fh}")).unwrap_or(class)
        }
        _ => class,
    };
    match size {
        WallgenSize::Display => fit(DISPLAY_CLASS),
        WallgenSize::Fhd1080 => Resolution::Fhd1080,
        WallgenSize::Qhd1440 => Resolution::Qhd1440,
        WallgenSize::Uhd4k => Resolution::Uhd4k,
        WallgenSize::Uhd5k => Resolution::Uhd5k,
        WallgenSize::Uhd8k => Resolution::Uhd8k,
        WallgenSize::Ultrawide1440 => Resolution::Ultrawide1440,
        WallgenSize::Portrait4k => Resolution::Portrait4k,
        WallgenSize::Custom => Resolution::from_id(&format!("{}x{}", custom.0, custom.1)).unwrap_or_default(),
    }
}

/// The still resolution the settings ask for.
pub fn still_resolution(settings: &WallgenSettings, display: Option<(u32, u32)>) -> Resolution {
    resolution(settings.size, (settings.custom_width, settings.custom_height), display)
}

/// The resolution video loops use: [`WallgenSize::Display`] is the
/// 1080p class fitted to the display.
pub fn video_resolution(settings: &WallgenSettings, display: Option<(u32, u32)>) -> Resolution {
    match (settings.video.size, display) {
        (WallgenSize::Display, Some((w, h))) if w > 0 && h > 0 => {
            let (fw, fh) = VIDEO_DISPLAY_CLASS.fit_aspect(w, h);
            Resolution::from_id(&format!("{fw}x{fh}")).unwrap_or(VIDEO_DISPLAY_CLASS)
        }
        (WallgenSize::Display, _) => VIDEO_DISPLAY_CLASS,
        (size, _) => resolution(size, (settings.custom_width, settings.custom_height), display),
    }
}

pub fn format(settings: &WallgenSettings) -> Format {
    match settings.format {
        WallgenFormat::Lossless => Format::Lossless,
        WallgenFormat::Webp => Format::WebpLossless,
        WallgenFormat::Png => Format::Png,
        WallgenFormat::Jpeg => Format::Jpeg { quality: settings.jpeg_quality },
    }
}

/// Where and how a still is written.
pub fn output_options(settings: &WallgenSettings, wallpaper_dir: &Path, display: Option<(u32, u32)>) -> OutputOptions {
    OutputOptions { resolution: still_resolution(settings, display), format: format(settings), dir: settings.output_dir(wallpaper_dir), name: None }
}

/// How a loop is encoded.
pub fn video_options(settings: &WallgenSettings, display: Option<(u32, u32)>) -> VideoOptions {
    VideoOptions { seconds: settings.video.seconds, fps: settings.video.fps, resolution: video_resolution(settings, display), quality: settings.video.quality, ..VideoOptions::default() }
}

/// Live theme colors as a palette: surfaces, containers and accents (the
/// engine orders them from deepest to brightest itself).
pub fn theme_palette(p: &MandaraPalette) -> PaletteSpec {
    let colors = [p.surface_dim, p.surface_container_high, p.primary_container, p.primary, p.secondary, p.tertiary, p.accent_gradient_end];
    PaletteSpec::Theme(colors.iter().map(|c| format!("#{}", c.to_hex_rgb_bare())).take(mandara_wallgen::MAX_STOPS).collect())
}

/// A seed that differs between calls.
pub fn fresh_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// `recipe` with a new layout seed (the rest kept).
pub fn reseeded(recipe: &Recipe, seed: u64) -> Recipe {
    Recipe { seed, ..recipe.clone() }
}

/// A random recipe in `light` mode (of `style` when given); a locked seed
/// keeps `keep_seed`.
pub fn random_recipe(style: Option<Style>, seed: u64, light: bool, keep_seed: Option<u64>) -> Recipe {
    let mut r = Recipe::random(style, seed);
    r.light = light;
    if let Some(s) = keep_seed {
        r.seed = s;
    }
    r
}

/// A variation of `recipe` (a locked seed survives it).
pub fn variation(recipe: &Recipe, amount: f32, seed: u64, lock_seed: bool) -> Recipe {
    let mut r = recipe.mutate(amount, seed);
    if lock_seed {
        r.seed = recipe.seed;
    }
    r
}

/// One generated file in the output folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub recipe: Recipe,
    pub video: bool,
    pub bytes: u64,
    pub modified: SystemTime,
}

/// The generated wallpapers in `dir` (files with a recipe sidecar), newest
/// first, at most `limit`.
pub fn library(dir: &Path, limit: usize) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden || !path.is_file() || !crate::wallpaper::is_wallpaper(&path) {
                return None;
            }
            let recipe = mandara_wallgen::recipe_for(&path)?;
            let meta = e.metadata().ok()?;
            Some(Entry { video: mandara_media::is_video(&path), bytes: meta.len(), modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), recipe, path })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.path.cmp(&a.path)));
    out.truncate(limit);
    out
}

/// Which of `paths` carry a recipe sidecar (the picker's "Generated"
/// marker): one directory read per folder, no file contents.
pub fn generated_among(paths: &[PathBuf]) -> std::collections::HashSet<PathBuf> {
    use std::collections::{HashMap, HashSet};
    let mut sidecars: HashMap<&Path, HashSet<std::ffi::OsString>> = HashMap::new();
    let mut out = HashSet::new();
    for p in paths {
        let Some(dir) = p.parent() else { continue };
        let names = sidecars.entry(dir).or_insert_with(|| {
            std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().ends_with(SIDECAR_SUFFIX)).collect()).unwrap_or_default()
        });
        if let Some(name) = mandara_wallgen::sidecar_path(p).file_name()
            && names.contains(name)
        {
            out.insert(p.clone());
        }
    }
    out
}

/// The end of every sidecar's file name (`.<file>.recipe.toml`).
const SIDECAR_SUFFIX: &str = ".recipe.toml";

/// Deletes a generated wallpaper and its sidecar (refusing other files).
pub fn delete(path: &Path) -> Result<(), String> {
    if !mandara_wallgen::is_generated(path) {
        return Err(mandara_i18n::tf("{path} wasn't made by the wallpaper generator", &[("path", &path.display())]));
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(mandara_wallgen::sidecar_path(path));
    Ok(())
}

/// A file size for people: `840 KB`, `3.2 MB`.
pub fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    const MB: f64 = KB * KB;
    /// Below this many MB one decimal is shown.
    const DECIMAL_BELOW_MB: f64 = 10.0;
    let b = bytes as f64;
    if b >= MB {
        let mb = b / MB;
        if mb < DECIMAL_BELOW_MB { format!("{mb:.1} {}", mandara_i18n::t("MB")) } else { format!("{mb:.0} {}", mandara_i18n::t("MB")) }
    } else {
        format!("{:.0} {}", (b / KB).max(1.0), mandara_i18n::t("KB"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::wallgen::WallgenTone;

    #[test]
    fn recipes_round_trip_through_settings() {
        let mut s = WallgenSettings::default();
        assert_eq!(recipe(&s, true), default_recipe(), "empty settings: the default recipe");
        let mut r = Recipe::random(Some(Style::Aurora), 9);
        r.seed = u64::MAX - 1;
        store_recipe(&mut s, &r);
        let back = recipe(&s, true);
        assert_eq!(back.seed, r.seed, "64-bit seeds survive");
        assert_eq!(back.style, Style::Aurora);
        assert_eq!(back.params, r.params);
        // Light follows the tone, whatever the stored recipe says.
        assert!(recipe(&s, false).light, "light theme → light wallpaper");
        s.tone = WallgenTone::Dark;
        assert!(!recipe(&s, false).light);
        // Garbage falls back to the default.
        s.recipe.insert("style".into(), toml::Value::String("nope".into()));
        assert_eq!(recipe(&s, true).style, default_recipe().style);
    }

    #[test]
    fn sizes_map_to_engine_resolutions() {
        assert_eq!(resolution(WallgenSize::Display, (0, 0), Some((1920, 1080))), Resolution::Uhd4k);
        assert_eq!(resolution(WallgenSize::Display, (0, 0), Some((2560, 1600))).size(), (3456, 2160));
        assert_eq!(resolution(WallgenSize::Display, (0, 0), Some((3440, 1440))).size(), (5160, 2160));
        assert_eq!(resolution(WallgenSize::Display, (0, 0), None), Resolution::Uhd4k, "unknown display: plain 4K");
        assert_eq!(resolution(WallgenSize::Uhd8k, (0, 0), Some((1080, 1920))), Resolution::Uhd8k);
        assert_eq!(resolution(WallgenSize::Custom, (3000, 2000), None).size(), (3000, 2000));
        assert_eq!(resolution(WallgenSize::Custom, (1, 1), None), Resolution::default(), "an invalid custom size falls back");
        let mut s = WallgenSettings::default();
        assert_eq!(video_resolution(&s, Some((1920, 1200))), Resolution::Fhd1080, "loops default to plain 1080p");
        s.video.size = WallgenSize::Display;
        assert_eq!(video_resolution(&s, Some((1920, 1200))).size(), (1728, 1080));
        assert_eq!(video_resolution(&s, None), Resolution::Fhd1080);
        s.video.size = WallgenSize::Uhd4k;
        assert_eq!(video_resolution(&s, Some((1920, 1080))), Resolution::Uhd4k);
    }

    #[test]
    fn formats_and_options_follow_the_settings() {
        let mut s = WallgenSettings::default();
        assert_eq!(format(&s), Format::Lossless, "the smallest exact file by default");
        s.format = WallgenFormat::Webp;
        assert_eq!(format(&s), Format::WebpLossless);
        s.format = WallgenFormat::Jpeg;
        s.jpeg_quality = 80;
        assert_eq!(format(&s), Format::Jpeg { quality: 80 });
        s.format = WallgenFormat::Png;
        let o = output_options(&s, Path::new("/w"), None);
        assert_eq!((o.format, o.dir.clone(), o.resolution), (Format::Png, PathBuf::from("/w/Generated"), Resolution::Uhd4k));
        s.video.seconds = 3.0;
        s.video.fps = 12;
        let v = video_options(&s, None);
        assert_eq!((v.seconds, v.fps, v.frames()), (3.0, 12, 36));
    }

    #[test]
    fn variations_and_randoms_respect_the_lock() {
        let r = Recipe::random(Some(Style::Waves), 3);
        assert_eq!(variation(&r, 1.0, 77, true).seed, r.seed);
        assert_eq!(variation(&r, 1.0, 77, true).style, r.style);
        let random = random_recipe(None, 5, true, Some(42));
        assert!(random.light);
        assert_eq!(random.seed, 42);
        assert_ne!(fresh_seed(), fresh_seed());
        assert_eq!(reseeded(&r, 9).params, r.params);
    }

    #[test]
    fn theme_palettes_are_valid() {
        let p = mandara_theme::engine::default_palette(true);
        let spec = theme_palette(&p);
        assert!(spec.is_valid(), "{spec:?}");
        assert!(matches!(spec, PaletteSpec::Theme(_)));
    }

    #[test]
    fn library_lists_only_generated_files_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("photo.jpg"), b"not ours").unwrap();
        let opts = |name: &str| OutputOptions { resolution: Resolution::Custom { width: 32, height: 18 }, format: Format::Png, dir: dir.path().to_path_buf(), name: Some(name.into()) };
        let a = mandara_wallgen::save(&Recipe::new(Style::Mesh, 1), &opts("a")).unwrap();
        // Distinct modification times.
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&a.path).unwrap().set_modified(old).unwrap();
        let b = mandara_wallgen::save(&Recipe::new(Style::Bokeh, 2), &opts("b")).unwrap();
        let items = library(dir.path(), 10);
        assert_eq!(items.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![b.path.clone(), a.path.clone()]);
        assert_eq!(items[0].recipe.style, Style::Bokeh);
        assert!(!items[0].video && items[0].bytes > 0);
        assert_eq!(library(dir.path(), 1).len(), 1);
        // Deleting removes the sidecar; foreign files are refused.
        delete(&a.path).unwrap();
        assert!(!a.path.exists() && !a.recipe_path.exists());
        assert!(delete(&dir.path().join("photo.jpg")).is_err());
        assert!(dir.path().join("photo.jpg").exists());
        assert!(library(&dir.path().join("missing"), 5).is_empty());
    }

    #[test]
    fn the_picker_listing_includes_and_flags_generated_files() {
        let wallpapers = tempfile::tempdir().unwrap();
        std::fs::write(wallpapers.path().join("beach.jpg"), b"x").unwrap();
        let out = WallgenSettings::default().output_dir(wallpapers.path());
        let opts = OutputOptions { resolution: Resolution::Custom { width: 32, height: 18 }, format: Format::WebpLossless, dir: out.clone(), name: None };
        let made = mandara_wallgen::save(&Recipe::new(Style::Dunes, 3), &opts).unwrap();
        let listed = crate::wallpaper::list(wallpapers.path());
        assert!(listed.contains(&made.path), "the picker scans the Generated sub-folder: {listed:?}");
        assert!(!listed.iter().any(|p| p.to_string_lossy().ends_with(SIDECAR_SUFFIX)), "sidecars stay hidden");
        let flagged = generated_among(&listed);
        assert!(flagged.contains(&made.path));
        assert!(!flagged.contains(&wallpapers.path().join("beach.jpg")));
        assert_eq!(flagged.len(), 1);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_bytes(512), "1 KB");
        assert_eq!(human_bytes(840_000), "840 KB");
        assert_eq!(human_bytes(3_240_000), "3.2 MB");
        assert_eq!(human_bytes(42_000_000), "42 MB");
    }
}
