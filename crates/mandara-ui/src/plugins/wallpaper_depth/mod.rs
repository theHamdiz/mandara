//! The wallpaper depth plugin (`wallpaper-depth`): depth layer 3 of the
//! desktop, the wallpaper's foreground subject drawn *over* the desktop
//! clock and the widgets behind it, so mountains, buildings or people in
//! the photo overlap the typography.
//!
//! First-party, on by default and part of the core (it can't be switched
//! off or removed; the effect itself is the `wallpaper.depth_effect`
//! switch). It finds the subject in one of two exclusive modes
//! (`[wallpaper.depth] mode`):
//!
//! * **Hand-tuned** (default, [`hand_tuned`]): Mandara's own pure-Rust sky
//!   / subject segmentation. No download; it skips wallpapers it isn't sure
//!   about.
//! * **Model** ([`mandara_services::depth_model`]): Depth Anything V2 Small
//!   (Apache-2.0, 27 MB), downloaded once when this mode is picked (or with
//!   `depth download`, or Download in Studio), run once per wallpaper in a
//!   low-priority helper process with time, CPU and memory caps, cached on
//!   disk. The depth map also lets you choose *where* in the scene the
//!   clock sits (`split`): automatically per wallpaper, or by hand.
//!
//! A mask you make yourself (`~/.local/share/mandara/depth_masks/<wallpaper
//! name>.png`) wins over both. Masks, the cache and the cut-out live in
//! [`masks`]. Nothing here runs at idle: work happens once per wallpaper
//! (or setting change), and everything is dropped after.
//!
//! Commands: `depth status`, `depth download`, `depth regenerate`,
//! `depth place-clock`. Settings: Studio → Wallpaper → Depth clock.

pub mod hand_tuned;
pub mod masks;

pub use hand_tuned::{Segmentation, segment};
pub use masks::*;

use image::GrayImage;
use mandara_config::WallpaperConfig;
use mandara_i18n::tk;
use mandara_plugins::{CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::depth_model;
use serde_json::json;

use crate::state::{config, update_config};

pub const ID: &str = "wallpaper-depth";

/// Longest side of a stored mask.
pub const MASK_MAX: u32 = 1600;

/// `depth place-clock`: the subject should cover about this much of the
/// digits — enough to read as depth, little enough that no digit is lost
/// (at a quarter, a whole digit could vanish) — and
/// candidates further than [`PLACE_TOLERANCE`] from that are refused.
const PLACE_TARGET: f32 = 0.15;
const PLACE_TOLERANCE: f32 = 0.1;
/// Candidate clock centres (fractions of the screen) and their step.
const PLACE_X: (f32, f32) = (0.2, 0.8);
const PLACE_Y: (f32, f32) = (0.15, 0.7);
const PLACE_STEP: f32 = 0.025;
/// Score cost per unit of distance from the current clock position (keeps
/// the clock near where it was among equally good spots).
const PLACE_STAY: f32 = 0.15;
/// The screen the placement is computed for (16:9, logical px).
const PLACE_SCREEN: (u32, u32) = (1920, 1080);

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("Wallpaper depth"), tk("Draws the wallpaper's subject over the desktop clock and widgets: hand-tuned, or with a small depth model downloaded on demand"), None),
        |_, _, _| PluginInstance::empty(),
    );
    r.command(ID, CommandSpec::new("depth status", tk("The depth effect's mode, model, current mask and last model run"), |_, cx| Ok(status(cx))).example("depth status"));
    r.command(
        ID,
        CommandSpec::new_output("depth download", tk("Download the depth model (27 MB, checksum-verified) for the model mode"), |_, cx| {
            if depth_model::installed() {
                return Ok(Output::Value(json!({ "model": depth_model::MODEL_ID, "installed": true, "path": mandara_services::models::dir(depth_model::MODEL_ID) })));
            }
            let (tx, rx) = async_channel::bounded(1);
            let entity = masks(cx);
            entity.update(cx, |m, cx| m.download(Some(tx), cx));
            Ok(Output::Pending(rx))
        })
        .example("depth download"),
    );
    r.command(
        ID,
        CommandSpec::new_output("depth regenerate", tk("Find the current wallpaper's subject again with the chosen mode"), |_, cx| {
            let w = config(cx).read(cx).get().wallpaper.clone();
            let path = current_wallpaper(cx).ok_or("no wallpaper is set")?;
            let request = Request::of(&w).ok_or("the depth effect is off (Mandara Studio → Wallpaper)")?;
            let (tx, rx) = async_channel::bounded(1);
            let entity = masks(cx);
            entity.update(cx, |m, cx| m.regenerate(&path, request, Some(tx), cx));
            Ok(Output::Pending(rx))
        })
        .example("depth regenerate"),
    );
    r.command(
        ID,
        CommandSpec::new_output("depth place-clock", tk("Move the desktop clock to where the wallpaper's subject tucks into it"), |_, cx| {
            let w = config(cx).read(cx).get().wallpaper.clone();
            let path = current_wallpaper(cx).ok_or("no wallpaper is set")?;
            let request = Request::of(&w).ok_or("the depth effect is off (Mandara Studio → Wallpaper)")?;
            let (tx, rx) = async_channel::bounded(1);
            let (place_tx, place_rx) = async_channel::bounded::<Result<(f32, f32, f32), String>>(1);
            let wall = w.clone();
            cx.background_executor()
                .spawn(async move {
                    let result = resolve(&path, Some(request), false).and_then(|r| {
                        let alpha = load_alpha(&r.path)?;
                        suggest_clock(&alpha, &wall).ok_or_else(|| "no spot where the subject tucks into the clock: it is too small, or fills the frame".to_string())
                    });
                    let _ = place_tx.send(result).await;
                })
                .detach();
            cx.spawn(async move |cx| {
                let reply = match place_rx.recv().await {
                    Ok(Ok((x, y, cover))) => {
                        cx.update(|cx| update_config(cx, |c| move_clock(c, x, y)));
                        Ok(json!({ "clock_x": x, "clock_y": y, "coverage": cover }).to_string())
                    }
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(reply).await;
            })
            .detach();
            Ok(Output::Pending(rx))
        })
        .example("depth place-clock"),
    );
}

fn status(cx: &mut gpui::App) -> serde_json::Value {
    let w = config(cx).read(cx).get().wallpaper.clone();
    let request = Request::of(&w);
    let shown = current_wallpaper(cx);
    let entity = masks(cx);
    let state = entity.read(cx);
    let mask = shown.as_ref().map(|p| match state.known(p, request) {
        Some(MaskStatus::Ready(r)) => json!({ "mask": r.path, "source": r.source, "detail": r.detail }),
        Some(MaskStatus::Unavailable(why)) => json!({ "mask": null, "reason": why }),
        Some(MaskStatus::Pending) => json!({ "mask": null, "reason": "generating" }),
        None if state.download == Download::Running => json!({ "mask": null, "reason": "downloading the depth model" }),
        None => json!({ "mask": null, "reason": "not requested yet (shown with the depth clock or widgets behind the subject)", "custom_path": user_mask_path(p) }),
    });
    let download = match &state.download {
        Download::Idle => json!("idle"),
        Download::Running => json!("running"),
        Download::Failed(why) => json!({ "failed": why }),
    };
    let downloaded = (state.download == Download::Running).then(|| state.downloaded.load(std::sync::atomic::Ordering::Relaxed));
    let model = mandara_services::models::find(depth_model::MODEL_ID);
    json!({
        "effect": w.depth_effect,
        "mode": w.depth.mode,
        "auto_split": w.depth.auto_split,
        "split": w.depth.manual_split(),
        "wallpaper": shown,
        "current": mask,
        "model": {
            "id": depth_model::MODEL_ID,
            "installed": depth_model::installed(),
            "size": model.map(|m| m.download_size()),
            "license": model.map(|m| m.license),
            "source": depth_model::SOURCE,
            "sha256": depth_model::SHA256,
            "path": depth_model::model_path(),
            "download": download,
            // Bytes received so far, while downloading.
            "downloaded": downloaded,
        },
        "last_run": last_run().map(|(wallpaper, r)| json!({ "wallpaper": wallpaper, "ms": r.ms, "cpu_ms": r.cpu_ms, "peak_rss_kib": r.peak_rss_kib })),
    })
}

/// The wallpaper the desktop shows: the light / dark variant for the
/// theme mode, `~` expanded (what the renderers ask masks for).
fn current_wallpaper(cx: &gpui::App) -> Option<std::path::PathBuf> {
    let cfg = config(cx).read(cx).get();
    mandara_config::desktop::effective_wallpaper(cfg.wallpaper.path.as_ref(), &cfg.wallpaper.variants, None, cfg.theme.mode.is_dark())
}

/// Moves the depth clock's anchor to `(x, y)` (fractions of the screen).
fn move_clock(c: &mut mandara_config::ShellConfig, x: f32, y: f32) {
    let id = c.desktop.widgets.depth_clock().map(|(w, _)| w.id.clone());
    match id.and_then(|id| c.desktop.widgets.get_mut(&id)) {
        Some(widget) => {
            let (fx, fy) = widget.place.anchor.fraction();
            widget.place.x = x - fx;
            widget.place.y = y - fy;
        }
        None => {
            c.wallpaper.clock_x = x;
            c.wallpaper.clock_y = y;
        }
    }
    mandara_config::desktop_widgets::sync_depth_clock(c);
}

/// Where to put the desktop clock so the subject (`alpha`, any size) rises
/// into the lower part of its digits: `(x, y, coverage)`, or `None` when no
/// spot comes close to [`PLACE_TARGET`].
pub fn suggest_clock(alpha: &GrayImage, w: &WallpaperConfig) -> Option<(f32, f32, f32)> {
    let (sw, sh) = PLACE_SCREEN;
    let aligned = align(alpha, (sw, sh), true);
    let map = image::imageops::thumbnail(&aligned, COVERAGE_MAP_W, (COVERAGE_MAP_W * sh / sw).max(1));
    let (fw, fh) = (sw as f32, sh as f32);
    let steps = |(a, b): (f32, f32)| ((b - a) / PLACE_STEP).round() as usize;
    let mut best: Option<(f32, f32, f32, f32)> = None;
    for iy in 0..=steps(PLACE_Y) {
        for ix in 0..=steps(PLACE_X) {
            let (x, y) = (PLACE_X.0 + ix as f32 * PLACE_STEP, PLACE_Y.0 + iy as f32 * PLACE_STEP);
            let cfg = WallpaperConfig { clock_x: x, clock_y: y, ..w.clone() };
            let l = crate::desktop::layout(fw, fh, &cfg);
            let (x0, y0, x1, y1) = crate::desktop::digit_box(&l, cfg.clock_style);
            let mid = (y0 + y1) / 2.0;
            let cover = coverage(&map, (x0 / fw, y0 / fh, x1 / fw, y1 / fh));
            // The subject should come up from below, not hang from above.
            let top = coverage(&map, (x0 / fw, y0 / fh, x1 / fw, mid / fh));
            let bottom = coverage(&map, (x0 / fw, mid / fh, x1 / fw, y1 / fh));
            let moved = ((x - w.clock_x).powi(2) + (y - w.clock_y).powi(2)).sqrt();
            let score = (cover - PLACE_TARGET).abs() + (top - bottom).max(0.0) + PLACE_STAY * moved;
            if (cover - PLACE_TARGET).abs() <= PLACE_TOLERANCE && best.is_none_or(|b| score < b.3) {
                best = Some((x, y, cover, score));
            }
        }
    }
    best.map(|(x, y, cover, _)| (x, y, cover))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_the_clock_where_a_hill_rises_into_it() {
        // A hill on the right half rising to 45 % of the height.
        let alpha = GrayImage::from_fn(320, 180, |x, y| image::Luma([if x > 160 && y > 81 { 255 } else { 0 }]));
        let w = WallpaperConfig::default();
        let (x, y, cover) = suggest_clock(&alpha, &w).expect("a spot");
        assert!(x > 0.5, "over the hill: {x}");
        assert!((cover - PLACE_TARGET).abs() <= PLACE_TOLERANCE, "{cover}");
        assert!((PLACE_Y.0..=PLACE_Y.1).contains(&y));
        // Nothing to tuck into: no suggestion.
        assert!(suggest_clock(&GrayImage::new(320, 180), &w).is_none());
    }
}
