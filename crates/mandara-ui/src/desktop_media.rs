//! Pictures for the painted desktop widgets: app icons, your avatar,
//! photos and stickers, decoded off the UI thread at the size they are
//! drawn and kept while a widget shows them.
//!
//! The wallpaper canvas paints from [`crate::backdrop::widgets::WidgetData`];
//! a picture that isn't decoded yet is left out of the scene and the scene
//! is rebuilt when it arrives. Nothing here runs until a widget asks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Global};

use crate::backdrop::widgets::layout::Picture;

/// Pictures larger than this many px on their longest side are never
/// decoded (a widget is never that big).
const MAX_EDGE_PX: u32 = 2048;
/// Folders of pictures are listed again this often while shown.
const FOLDER_TTL: Duration = Duration::from_secs(300);
/// Picture files a photo frame shows.
const PICTURE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "svg"];

#[derive(Default)]
struct Pictures {
    ready: HashMap<(PathBuf, u32), Option<Picture>>,
    loading: HashSet<(PathBuf, u32)>,
    folders: HashMap<PathBuf, (Instant, Vec<PathBuf>)>,
    /// Installed apps: desktop id → (name, icon file).
    apps: Option<Arc<HashMap<String, (String, Option<PathBuf>)>>>,
    apps_loading: bool,
    /// Keys asked for since the last [`retain_asked`]: the rest is dropped.
    asked: HashSet<(PathBuf, u32)>,
}

impl Global for Pictures {}

fn state(cx: &mut App) -> &mut Pictures {
    if !cx.has_global::<Pictures>() {
        cx.set_global(Pictures::default());
    }
    cx.global_mut::<Pictures>()
}

/// `path` decoded to fit `edge` px (its shorter side at least `edge`,
/// for covering a box), when ready; starts decoding it otherwise.
pub fn picture(path: &Path, edge: u32, cx: &mut App) -> Option<Picture> {
    let key = (path.to_path_buf(), edge.clamp(1, MAX_EDGE_PX));
    let s = state(cx);
    s.asked.insert(key.clone());
    if let Some(p) = s.ready.get(&key) {
        return p.clone();
    }
    if s.loading.insert(key.clone()) {
        cx.spawn(async move |cx| {
            let k = key.clone();
            let decoded = cx.background_executor().spawn(async move { decode(&k.0, k.1) }).await;
            cx.update(|cx| {
                let s = state(cx);
                s.loading.remove(&key);
                if let Err(e) = &decoded {
                    tracing::warn!("desktop widget picture {}: {e}", key.0.display());
                }
                s.ready.insert(key.clone(), decoded.ok().map(|pm| Picture { key: crate::backdrop::widgets::key((&key.0, key.1)), pixmap: Arc::new(pm) }));
                crate::backdrop::refresh(cx);
            });
        })
        .detach();
    }
    None
}

/// Forgets every picture not asked for since the last call (a widget was
/// removed or shows another one).
pub fn retain_asked(cx: &mut App) {
    if !cx.has_global::<Pictures>() {
        return;
    }
    let s = cx.global_mut::<Pictures>();
    let asked = std::mem::take(&mut s.asked);
    s.ready.retain(|k, _| asked.contains(k));
}

/// Picture files in `folder`, sorted by name (listed again every few
/// minutes).
pub fn folder_pictures(folder: &Path, cx: &mut App) -> Vec<PathBuf> {
    let s = state(cx);
    if let Some((at, list)) = s.folders.get(folder)
        && at.elapsed() < FOLDER_TTL
    {
        return list.clone();
    }
    let mut list: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| PICTURE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))).collect())
        .unwrap_or_default();
    list.sort();
    s.folders.insert(folder.to_path_buf(), (Instant::now(), list.clone()));
    list
}

/// The installed apps (desktop id → name, icon file), once listed; the
/// first call lists them in the background.
pub fn apps(cx: &mut App) -> Option<Arc<HashMap<String, (String, Option<PathBuf>)>>> {
    let s = state(cx);
    if s.apps.is_some() || s.apps_loading {
        return s.apps.clone();
    }
    s.apps_loading = true;
    cx.spawn(async move |cx| {
        let list = cx
            .background_executor()
            .spawn(async move {
                crate::apps::discover()
                    .into_iter()
                    .map(|a| (a.id.trim_end_matches(".desktop").to_string(), (a.name, a.icon)))
                    .collect::<HashMap<_, _>>()
            })
            .await;
        cx.update(|cx| {
            let s = state(cx);
            s.apps = Some(Arc::new(list));
            s.apps_loading = false;
            crate::backdrop::refresh(cx);
        });
    })
    .detach();
    None
}

/// The current track's synced lyrics (ms, line) and whether they are
/// still being looked up: from the lyrics cache or lrclib, like the media
/// panel's, fetched once per track while a lyrics widget shows.
pub fn lyrics(state: &mandara_services::MediaState, cx: &mut App) -> (Vec<(i64, String)>, bool) {
    let q = crate::plugins::media::lyrics_view::query_for(state);
    let key = q.key();
    let s = state_lyrics(cx);
    if s.0.as_deref() == Some(key.as_str()) {
        return (s.1.clone(), s.2);
    }
    *s = (Some(key.clone()), Vec::new(), true);
    cx.spawn(async move |cx| {
        let found = cx.background_executor().spawn(async move { mandara_services::lyrics::fetch(&q) }).await;
        let lines: Vec<(i64, String)> = match found {
            Ok(mandara_services::lyrics::Lyrics::Synced(lines)) => lines.into_iter().map(|l| (i64::try_from(l.time.as_millis()).unwrap_or(i64::MAX), l.text)).collect(),
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::debug!("desktop lyrics: {e}");
                Vec::new()
            }
        };
        cx.update(|cx| {
            let s = state_lyrics(cx);
            if s.0.as_deref() == Some(key.as_str()) {
                *s = (Some(key), lines, false);
                crate::backdrop::refresh(cx);
            }
        });
    })
    .detach();
    (Vec::new(), true)
}

/// The track key, its lines and whether they are loading.
#[derive(Default)]
struct LyricsCache((Option<String>, Vec<(i64, String)>, bool));
impl Global for LyricsCache {}

fn state_lyrics(cx: &mut App) -> &mut (Option<String>, Vec<(i64, String)>, bool) {
    if !cx.has_global::<LyricsCache>() {
        cx.set_global(LyricsCache::default());
    }
    &mut cx.global_mut::<LyricsCache>().0
}

/// Decodes `path` so its shorter side is at least `edge` px (its longer
/// side at most [`MAX_EDGE_PX`]).
fn decode(path: &Path, edge: u32) -> Result<tiny_skia::Pixmap, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"));
    if svg {
        let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
        let size = tree.size();
        let k = edge as f32 / size.width().min(size.height()).max(1.0);
        let (w, h) = (((size.width() * k).round() as u32).clamp(1, MAX_EDGE_PX), ((size.height() * k).round() as u32).clamp(1, MAX_EDGE_PX));
        let mut pm = tiny_skia::Pixmap::new(w, h).ok_or("bad size")?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
        return Ok(pm);
    }
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let (w, h) = (img.width().max(1), img.height().max(1));
    let k = (edge as f32 / w.min(h) as f32).min(MAX_EDGE_PX as f32 / w.max(h) as f32).min(1.0);
    let img = if k < 1.0 { img.resize((w as f32 * k).round() as u32, (h as f32 * k).round() as u32, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.into_rgba8();
    crate::lock::paint::pixmap_from_rgba(rgba.width(), rgba.height(), rgba.as_raw()).ok_or_else(|| "bad picture".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_to_cover_the_edge() {
        let dir = std::env::temp_dir().join(format!("mandara-pic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 255])).save(&p).unwrap();
        let pm = decode(&p, 100).unwrap();
        assert_eq!((pm.width(), pm.height()), (200, 100), "the short side covers the edge");
        let pm = decode(&p, 1000).unwrap();
        assert_eq!(pm.width(), 400, "never enlarged");
        let _ = std::fs::remove_dir_all(dir);
    }
}
