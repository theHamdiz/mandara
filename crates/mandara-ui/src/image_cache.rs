//! Explicit eviction for images GPUI loads from file paths.
//!
//! `img(path)` decodes through GPUI's asset cache, which keeps every decoded
//! `RenderImage` (full-resolution BGRA in RAM) until it is removed, and each
//! window's sprite atlas keeps the uploaded texture until the image is
//! dropped. A 2560×1440 @1.5× wallpaper is ~33 MB in each place, so every
//! wallpaper change, picker session or album leaked that much forever.
//! Views call these helpers once an image can no longer be on screen.

use std::path::Path;
use std::sync::Arc;

use gpui::{App, ImgResourceLoader, Resource, Window};

fn resource(path: &Path) -> Resource {
    Resource::Path(Arc::from(path))
}

/// Drops `path`'s decoded image from GPUI's asset cache and its texture
/// from every window's atlas. Call it only for paths that were displayed
/// (the lookup would start a decode for an uncached path). Safe for images
/// still shown elsewhere: they are simply decoded again on their next use.
pub fn evict_path(path: &Path, window: &mut Window, cx: &mut App) {
    let resource = resource(path);
    if let Some(Ok(image)) = window.get_asset::<ImgResourceLoader>(&resource, cx) {
        cx.drop_image(image, Some(window));
    }
    cx.remove_asset::<ImgResourceLoader>(&resource);
}

/// Like [`evict_path`] for code without a window (the atlas textures go
/// with their windows; only the decoded copies are released here).
pub fn forget_path(path: &Path, cx: &mut App) {
    cx.remove_asset::<ImgResourceLoader>(&resource(path));
}
