//! Video wallpapers end to end, without a compositor: a tiny AV1 clip is
//! encoded in-test (rav1e), then listed, thumbnailed, given a poster and
//! prepared for every fill mode exactly as the picker and the desktop
//! background do it.

#[path = "../../mandara-media/examples/clipgen/mod.rs"]
mod clipgen;

use std::path::{Path, PathBuf};

use mandara_config::desktop::WallpaperFill;
use mandara_ui::wallpaper::{list, still_in, thumbnail_in};
use mandara_ui::wallpaper_fill::prepare_in;

/// Clip size: small enough to encode in a fraction of a second.
const CLIP: (u32, u32) = (192, 108);

fn clip(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let webm = name.ends_with(".webm") || name.ends_with(".mkv");
    clipgen::write_clip(&path, &clipgen::ClipSpec { width: CLIP.0, height: CLIP.1, fps: 24, frames: 4, detail: 1, webm, speed: 10 }).unwrap();
    path
}

#[test]
fn the_library_lists_videos_with_images() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("loops");
    std::fs::create_dir_all(&sub).unwrap();
    for name in ["b.jpg", "a.WEBM", "notes.txt", "clip.mp4"] {
        std::fs::write(dir.path().join(name), b"x").unwrap();
    }
    std::fs::write(sub.join("c.ivf"), b"x").unwrap();
    std::fs::write(sub.join("d.mkv"), b"x").unwrap();
    let names: Vec<String> = list(dir.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["a.WEBM", "b.jpg", "c.ivf", "d.mkv"], "AV1 containers listed, other videos not");
}

#[test]
fn thumbnails_and_posters_come_from_the_first_frame() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    for name in ["tide.webm", "tide.ivf"] {
        let video = clip(dir.path(), name);
        let thumb = thumbnail_in(&video, &cache).unwrap();
        assert!(thumb.starts_with(cache.join("thumbnails")), "{thumb:?}");
        let img = image::open(&thumb).unwrap();
        assert_eq!((img.width(), img.height()), (720, 450), "picker thumbnail size");
        // The generator's aurora is bluish at the top-left.
        let px = img.to_rgb8().get_pixel(2, 2).0;
        assert!(px[2] > px[0], "{name}: thumbnail shows the frame, got {px:?}");
        // Cached: the second call doesn't decode (same file, same mtime).
        let modified = std::fs::metadata(&thumb).unwrap().modified().unwrap();
        assert_eq!(thumbnail_in(&video, &cache).unwrap(), thumb);
        assert_eq!(std::fs::metadata(&thumb).unwrap().modified().unwrap(), modified);

        let poster = still_in(&video, &cache).unwrap();
        assert!(poster.starts_with(cache.join("posters")));
        assert_eq!(image::image_dimensions(&poster).unwrap(), CLIP, "poster at the clip's size");
    }
    // Images are their own still.
    let png = dir.path().join("p.png");
    image::RgbImage::new(4, 4).save(&png).unwrap();
    assert_eq!(still_in(&png, &cache).unwrap(), png);
}

#[test]
fn fill_copies_of_a_video_use_its_poster() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let video = clip(dir.path(), "dunes.webm");
    for fill in [WallpaperFill::Cover, WallpaperFill::Contain, WallpaperFill::Stretch, WallpaperFill::Center, WallpaperFill::Tile] {
        let (file, size) = prepare_in(&video, fill, 1920, 1080, &cache).unwrap();
        assert!(!mandara_media::is_video(&file), "{fill:?}: a picture, not the clip");
        assert_eq!(image::image_dimensions(&file).unwrap(), size, "{fill:?}");
    }
    // Smaller than the output: the poster itself is drawn (scaled later).
    assert_eq!(prepare_in(&video, WallpaperFill::Cover, 1920, 1080, &cache).unwrap().1, CLIP);
}

#[test]
fn match_colors_reads_a_frame_of_the_clip() {
    let dir = tempfile::tempdir().unwrap();
    let video = clip(dir.path(), "colors.ivf");
    let seeds = mandara_theme::material::extract_seeds(&video, 4).unwrap();
    assert!(!seeds.is_empty(), "seed colours from a representative frame");
    // Not a video at all: a clear error, not a panic.
    let bad = dir.path().join("bad.webm");
    std::fs::write(&bad, b"not a clip").unwrap();
    assert!(mandara_theme::material::extract_seeds(&bad, 4).is_err());
}
