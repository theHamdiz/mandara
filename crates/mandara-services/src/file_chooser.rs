//! Native "Open file" dialogs through the XDG desktop portal
//! (`org.freedesktop.portal.FileChooser.OpenFile`): whichever portal
//! backend the desktop runs (GTK, GNOME, KDE, termfilechooser…) shows its
//! own picker, and the chosen file comes back as a URI.
//!
//! Blocking: call from a background thread.

use std::collections::HashMap;
use std::path::PathBuf;

use futures::StreamExt as _;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_CHOOSER_IFACE: &str = "org.freedesktop.portal.FileChooser";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
/// `Response` code for a confirmed choice (1 = cancelled, 2 = other).
const RESPONSE_OK: u32 = 0;
/// Portal filter entry kind: 0 = glob pattern, 1 = MIME type.
const FILTER_MIME: u32 = 1;

/// A named group of MIME types, e.g. ("Images", ["image/png", …]).
pub struct Filter<'a> {
    pub name: &'a str,
    pub mime_types: &'a [&'a str],
}

/// The image types Mandara can decode.
pub const IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

/// Portal filter entry kind for glob patterns.
const FILTER_GLOB: u32 = 0;

/// Shows the portal's file picker; `Ok(None)` when the user cancels.
pub fn open_file(title: &str, filters: &[Filter<'_>], start_dir: Option<PathBuf>) -> Result<Option<PathBuf>, String> {
    let list = filters.iter().map(|f| (f.name.to_string(), f.mime_types.iter().map(|m| (FILTER_MIME, m.to_string())).collect())).collect();
    crate::runtime::block_on(open_file_async(title, list, start_dir, false)).map(|v| v.into_iter().next()).map_err(|e| e.to_string())
}

/// Picks one or more files matching MIME types or glob patterns (one
/// named filter); empty when the user cancels.
pub fn open_files(title: &str, filter_name: &str, mime_types: &[&str], globs: &[&str], start_dir: Option<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let entries = globs.iter().map(|g| (FILTER_GLOB, g.to_string())).chain(mime_types.iter().map(|m| (FILTER_MIME, m.to_string()))).collect();
    crate::runtime::block_on(open_file_async(title, vec![(filter_name.to_string(), entries)], start_dir, true)).map_err(|e| e.to_string())
}

/// Picks one image (PNG / JPEG / WebP), starting in ~/Pictures.
pub fn pick_image(title: &str, filter_name: &str) -> Result<Option<PathBuf>, String> {
    let pictures = dirs::picture_dir().filter(|p| p.is_dir());
    open_file(title, &[Filter { name: filter_name, mime_types: IMAGE_MIME_TYPES }], pictures)
}

/// `:1.42` + token → the portal's predictable request object path.
fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// Portal filters: `(name, [(kind, pattern)])`.
type FilterList = Vec<(String, Vec<(u32, String)>)>;

async fn open_file_async(title: &str, filter_list: FilterList, start_dir: Option<PathBuf>, multiple: bool) -> zbus::Result<Vec<PathBuf>> {
    // Private connection: the response stream lives only for this dialog.
    let conn = crate::runtime::connect(zbus::connection::Builder::session()?).await?;
    let unique = conn.unique_name().map(|n| n.to_string()).unwrap_or_default();
    // Unique per dialog: the portal keys requests by sender + token.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let token = format!("mandara{}_{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let path = request_path(&unique, &token);
    // Subscribe before calling so a fast response isn't missed.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(REQUEST_IFACE)?
        .member("Response")?
        .path(path.clone())?
        .build();
    let mut responses = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;

    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    options.insert("multiple", Value::from(multiple));
    if !filter_list.is_empty() {
        options.insert("filters", Value::from(filter_list.clone()));
    }
    if let Some(dir) = start_dir {
        // `ay`, NUL-terminated.
        let mut bytes = dir.to_string_lossy().into_owned().into_bytes();
        bytes.push(0);
        options.insert("current_folder", Value::from(bytes));
    }
    let chooser = zbus::proxy::Builder::<zbus::Proxy>::new(&conn)
        .destination(PORTAL_DEST)?
        .path(PORTAL_PATH)?
        .interface(FILE_CHOOSER_IFACE)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let handle: OwnedObjectPath = chooser.call("OpenFile", &("", title, options)).await?;
    // Older portals ignore handle_token; follow the returned handle then.
    if handle.as_str() != path {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(REQUEST_IFACE)?
            .member("Response")?
            .path(handle.as_str().to_string())?
            .build();
        responses = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
    }
    let Some(msg) = responses.next().await else { return Ok(Vec::new()) };
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg?.body().deserialize()?;
    if code != RESPONSE_OK {
        return Ok(Vec::new());
    }
    let uris: Vec<String> = results.get("uris").and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    Ok(uris.iter().filter_map(|u| file_uri_to_path(u)).collect())
}

/// `file:///home/n/My%20Photo.jpg` → `/home/n/My Photo.jpg`.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Skip an authority (`file://localhost/…`).
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt as _;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(file_uri_to_path("file:///home/n/My%20Photo.jpg"), Some(PathBuf::from("/home/n/My Photo.jpg")));
        assert_eq!(file_uri_to_path("file://localhost/tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(file_uri_to_path("file:///home/n/%D8%B5%D9%88%D8%B1%D8%A9.png"), Some(PathBuf::from("/home/n/صورة.png")));
        assert_eq!(file_uri_to_path("https://example.org/a.png"), None);
    }

    #[test]
    fn builds_request_paths() {
        assert_eq!(request_path(":1.42", "mandara7"), "/org/freedesktop/portal/desktop/request/1_42/mandara7");
    }
}
