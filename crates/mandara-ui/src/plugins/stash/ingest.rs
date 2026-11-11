//! Turning what was dropped into items. Every function here blocks (it
//! reads pipes and files and writes the stash's own files): run them on a
//! background thread. Reads are bounded ([`model::MAX_DROP_BYTES`]).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::model::{self, DropPlan, ItemKind};
use super::store;

/// Largest link list or text read from a drop, bytes.
const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;

fn mime_dirs() -> Vec<PathBuf> {
    mandara_config::open::OpenEnv::system(Vec::new()).mime_dirs
}

/// Files and folders dropped (or added): kept by reference, except
/// pictures when `copy_images` (copied into the stash).
pub fn from_paths(dir: &Path, paths: &[PathBuf], copy_images: bool) -> Vec<ItemKind> {
    let dirs = mime_dirs();
    let mut out = Vec::with_capacity(paths.len().min(model::MAX_ITEMS_PER_DROP));
    for path in paths.iter().take(model::MAX_ITEMS_PER_DROP) {
        let Ok(meta) = std::fs::metadata(path) else {
            tracing::warn!("stash: {} is gone", path.display());
            continue;
        };
        let is_dir = meta.is_dir();
        let mime = mandara_config::open::mime_type_in(&dirs, path);
        if copy_images && !is_dir && mime.starts_with("image/") {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("picture");
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_else(|| model::extension_for(&mime));
            let copied = std::fs::File::open(path).and_then(|mut f| store::stream_blob(dir, stem, ext, &mut f, model::MAX_DROP_BYTES));
            match copied {
                Ok((blob, _)) => {
                    out.push(ItemKind::Image { blob, mime, source: Some(path.display().to_string()) });
                    continue;
                }
                Err(e) => tracing::warn!("stash: copying {}: {e}; keeping a reference", path.display()),
            }
        }
        out.push(ItemKind::File { path: path.clone(), dir: is_dir, mime });
    }
    out
}

fn read_capped(reader: &mut dyn std::io::Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(mandara_i18n::tf("Too large to keep (over {mib} MiB)", &[("mib", &(limit / (1024 * 1024)))]));
    }
    Ok(bytes)
}

/// A text item from `text` (and `html`): a single link becomes a link item.
pub fn text_item(dir: &Path, text: &str, html: Option<&[u8]>) -> Result<ItemKind, String> {
    if let Some(url) = model::single_link(text) {
        return Ok(ItemKind::Url { url: url.to_string(), title: None, icon: None });
    }
    if text.trim().is_empty() {
        return Err(mandara_i18n::t("Nothing to keep: the text is empty").to_string());
    }
    let blob = store::write_blob(dir, "text", "txt", text.as_bytes()).map_err(|e| e.to_string())?;
    let html = html.and_then(|h| store::write_blob(dir, "text", "html", h).ok());
    Ok(ItemKind::Text { blob, preview: model::preview_of(text), chars: text.chars().count() as u64, html })
}

/// Links (one item each); `file://` links become file items.
pub fn link_items(dir: &Path, links: Vec<model::Link>, copy_images: bool) -> Vec<ItemKind> {
    let (files, links): (Vec<_>, Vec<_>) = links.into_iter().partition(|l| model::file_uri_path(&l.url).is_some());
    let paths: Vec<PathBuf> = files.iter().filter_map(|l| model::file_uri_path(&l.url)).collect();
    let mut out = from_paths(dir, &paths, copy_images);
    out.extend(links.into_iter().filter(|l| l.url.contains(':')).map(|l| ItemKind::Url { url: l.url, title: l.title, icon: None }));
    out
}

/// Reads a data drop as `plan` says. `streams` are the plan's types in
/// order (a text plan: the text, then its HTML when offered).
pub fn from_streams(dir: &Path, plan: &DropPlan, mut streams: Vec<(String, Box<dyn std::io::Read + Send>)>, copy_images: bool) -> Result<Vec<ItemKind>, String> {
    streams.reverse();
    let mut next = || streams.pop();
    match plan {
        DropPlan::Image(mime) => {
            let (_, mut r) = next().ok_or("no data")?;
            let (blob, bytes) = store::stream_blob(dir, "picture", model::extension_for(mime), &mut r, model::MAX_DROP_BYTES).map_err(|e| e.to_string())?;
            if bytes == 0 {
                store::remove_blob(dir, &blob);
                return Err(mandara_i18n::t("The picture arrived empty").to_string());
            }
            Ok(vec![ItemKind::Image { blob, mime: mime.clone(), source: None }])
        }
        DropPlan::Links(mime) => {
            let (_, mut r) = next().ok_or("no data")?;
            let bytes = read_capped(&mut r, MAX_LIST_BYTES)?;
            let links = model::parse_links(mime, &bytes);
            if links.is_empty() {
                return Err(mandara_i18n::t("The drop had no links").to_string());
            }
            Ok(link_items(dir, links, copy_images))
        }
        DropPlan::Text { .. } => {
            let (_, mut r) = next().ok_or("no data")?;
            let text = model::decode_text(&read_capped(&mut r, MAX_TEXT_BYTES)?);
            let html = next().and_then(|(_, mut h)| read_capped(&mut h, MAX_TEXT_BYTES).ok()).filter(|h| !h.is_empty());
            Ok(vec![text_item(dir, &text, html.as_deref())?])
        }
        DropPlan::Html(_) => {
            let (_, mut r) = next().ok_or("no data")?;
            let html = read_capped(&mut r, MAX_TEXT_BYTES)?;
            let text = model::html_to_text(&model::decode_text(&html), usize::MAX);
            Ok(vec![text_item(dir, &text, Some(&html))?])
        }
        DropPlan::Data(mime) => {
            let (_, mut r) = next().ok_or("no data")?;
            let (blob, bytes) = store::stream_blob(dir, "data", model::extension_for(mime), &mut r, model::MAX_DROP_BYTES).map_err(|e| e.to_string())?;
            Ok(vec![ItemKind::Data { blob, mime: mime.clone(), bytes }])
        }
    }
}

/// The MIME types to receive for `plan`, in the order [`from_streams`]
/// reads them.
pub fn plan_types(plan: &DropPlan) -> Vec<String> {
    match plan {
        DropPlan::Image(m) | DropPlan::Links(m) | DropPlan::Html(m) | DropPlan::Data(m) => vec![m.clone()],
        DropPlan::Text { text, html } => std::iter::once(text.clone()).chain(html.clone()).collect(),
    }
}

/// Downloads the picture a link points at into the stash (on your click).
pub fn fetch_picture(dir: &Path, url: &str) -> Result<ItemKind, String> {
    if !model::fetchable(url) {
        return Err(mandara_i18n::t("Only https:// links are fetched").to_string());
    }
    let got = mandara_services::stash_fetch::fetch_https(url, model::MAX_FETCH_BYTES, true)?;
    let mime = got.mime.unwrap_or_else(|| "image/png".into());
    // Check it decodes before keeping it.
    let reader = image::ImageReader::new(std::io::Cursor::new(&got.bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format().is_none() && mime != "image/svg+xml" {
        return Err(mandara_i18n::t("Not a picture Mandara can show").to_string());
    }
    let stem = url.split(['?', '#']).next().and_then(|u| u.rsplit('/').next()).and_then(|n| n.rsplit_once('.').map(|(s, _)| s)).filter(|s| !s.is_empty()).unwrap_or("picture");
    let blob = store::write_blob(dir, stem, model::extension_for(&mime), &got.bytes).map_err(|e| e.to_string())?;
    Ok(ItemKind::Image { blob, mime, source: Some(url.to_string()) })
}

/// Downloads a site's touch icon (a PNG most sites publish) for a link.
pub fn fetch_icon(dir: &Path, url: &str) -> Result<String, String> {
    let host = model::host_of(url).ok_or_else(|| mandara_i18n::t("Not a web link").to_string())?;
    let mut last = String::new();
    for path in ["/apple-touch-icon.png", "/apple-touch-icon-precomposed.png", "/favicon.png"] {
        match mandara_services::stash_fetch::fetch_https(&format!("https://{host}{path}"), 512 * 1024, true) {
            Ok(got) if image::guess_format(&got.bytes).is_ok() => return store::write_blob(dir, "icon", "png", &got.bytes).map_err(|e| e.to_string()),
            Ok(_) => last = mandara_i18n::t("Not a picture Mandara can show").to_string(),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(bytes: &'static [u8]) -> Box<dyn std::io::Read + Send> {
        Box::new(std::io::Cursor::new(bytes))
    }

    #[test]
    fn text_and_links_from_streams() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path();
        let plan = DropPlan::Text { text: "text/plain;charset=utf-8".into(), html: Some("text/html".into()) };
        let items = from_streams(dir, &plan, vec![("text/plain;charset=utf-8".into(), stream(b"Hello\r\nworld")), ("text/html".into(), stream(b"<b>Hello</b>"))], false).unwrap();
        let ItemKind::Text { blob, preview, chars, html } = &items[0] else { panic!("{items:?}") };
        assert_eq!(preview, "Hello\nworld");
        assert_eq!(*chars, 11);
        assert_eq!(std::fs::read_to_string(dir.join(blob)).unwrap(), "Hello\nworld");
        assert!(html.is_some());
        // A single link typed as text is a link item.
        let items = from_streams(dir, &plan, vec![("text/plain;charset=utf-8".into(), stream(b"https://example.org/x"))], false).unwrap();
        assert!(matches!(&items[0], ItemKind::Url { url, .. } if url == "https://example.org/x"));
        // A uri-list mixing a local file and a web link.
        let file = cache.path().join("note.txt");
        std::fs::write(&file, "x").unwrap();
        let list = format!("{}\r\nhttps://example.org/\r\n", model::file_uri(&file));
        let list: &'static [u8] = Box::leak(list.into_bytes().into_boxed_slice());
        let items = from_streams(dir, &DropPlan::Links("text/uri-list".into()), vec![("text/uri-list".into(), stream(list))], false).unwrap();
        assert!(matches!(&items[0], ItemKind::File { path, dir: false, .. } if *path == file));
        assert!(matches!(&items[1], ItemKind::Url { .. }));
        // Raw data keeps its MIME type and size.
        let items = from_streams(dir, &DropPlan::Data("application/x-thing".into()), vec![("application/x-thing".into(), stream(b"\x00\x01\x02"))], false).unwrap();
        assert!(matches!(&items[0], ItemKind::Data { bytes: 3, mime, .. } if mime == "application/x-thing"));
        // Empty text is refused.
        assert!(from_streams(dir, &plan, vec![("text/plain;charset=utf-8".into(), stream(b"   "))], false).is_err());
        assert_eq!(plan_types(&plan), ["text/plain;charset=utf-8", "text/html"]);
    }
}
