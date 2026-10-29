//! Wallpaper slideshow (`[wallpaper.slideshow]`): every `interval_minutes`
//! the next image of the slideshow folder (or the picker folder) becomes
//! the wallpaper — alphabetically or at random, never the same one twice
//! in a row, optionally paused while a fullscreen window is open.

use std::time::{Duration, Instant};

use gpui::{App, Global};
use mandara_config::desktop::{SlideshowOrder, next_slide};
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use serde_json::json;

use crate::state::{config, update_config};

/// How often the slideshow checks whether a change is due.
const TICK: Duration = Duration::from_secs(30);
/// Seconds per minute.
const S_PER_MIN: u64 = 60;
/// Shortest interval honoured, minutes.
const MIN_INTERVAL_MIN: u32 = 1;

/// When the slideshow last changed the wallpaper.
struct LastSlide(Instant);
impl Global for LastSlide {}

/// The slideshow folder: its own, else the picker's.
fn folder(cx: &App) -> std::path::PathBuf {
    let cfg = config(cx).read(cx).snapshot();
    let dir = &cfg.wallpaper.slideshow.directory;
    if dir.trim().is_empty() { crate::wallpaper::directory(cx) } else { mandara_config::paths::expand_tilde(dir) }
}

/// A seed that differs between calls (no `rand` dependency needed).
fn seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// Shows the next slideshow image now (`order` overrides the config's).
pub fn advance(order: Option<SlideshowOrder>, cx: &mut App) -> Result<std::path::PathBuf, String> {
    let cfg = config(cx).read(cx).snapshot();
    let dir = folder(cx);
    let images = crate::wallpaper::list(&dir);
    let order = order.unwrap_or(cfg.wallpaper.slideshow.order);
    let next = next_slide(&images, cfg.wallpaper.path.as_ref(), order, seed()).ok_or_else(|| mandara_i18n::tf("No images in {folder}", &[("folder", &dir.display())]))?;
    cx.set_global(LastSlide(Instant::now()));
    crate::wallpaper::apply(next.clone(), cx);
    Ok(next)
}

fn fullscreen_open(cx: &App) -> bool {
    cx.try_global::<crate::hub::GlobalHub>().is_some_and(|g| g.0.read(cx).clients.iter().any(|c| c.fullscreen))
}

/// Runs the slideshow timer (shell process).
pub fn init(cx: &mut App) {
    cx.set_global(LastSlide(Instant::now()));
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(TICK).await;
        cx.update(|cx| {
            let s = config(cx).read(cx).get().wallpaper.slideshow.clone();
            if !s.enabled || !config(cx).read(cx).get().wallpaper.enabled {
                return;
            }
            let interval = Duration::from_secs(u64::from(s.interval_minutes.max(MIN_INTERVAL_MIN)) * S_PER_MIN);
            if cx.global::<LastSlide>().0.elapsed() < interval || (s.pause_on_fullscreen && fullscreen_open(cx)) {
                return;
            }
            if let Err(err) = advance(None, cx) {
                tracing::warn!("slideshow: {err}");
                // Try again after a full interval, not every tick.
                cx.set_global(LastSlide(Instant::now()));
            }
        });
    })
    .detach();
}

pub fn register(r: &mut PluginRegistry) {
    let owner = crate::plugins::wallpaper::ID;
    r.command(
        owner,
        CommandSpec::new("wallpaper output", tk("Give one display its own wallpaper (clear = follow the main one)"), |a, cx| {
            let output = a.str(0)?.to_string();
            let value = a.rest(1);
            if value.trim().is_empty() || value == "clear" {
                let o = output.clone();
                update_config(cx, move |c| {
                    c.wallpaper.variants.monitors.remove(&o);
                });
                return Ok(json!({ "output": output, "path": null }));
            }
            let path = mandara_config::paths::expand_tilde(value.trim());
            if !path.is_file() {
                return Err(mandara_i18n::tf("Not a file: {path}", &[("path", &path.display())]));
            }
            let (o, p) = (output.clone(), path.clone());
            update_config(cx, move |c| {
                c.wallpaper.variants.monitors.insert(o, p);
            });
            Ok(json!({ "output": output, "path": path }))
        })
        .arg(ArgSpec::required("output", ArgKind::String, tk("Connector name, e.g. DP-1 (see compositor monitors)")))
        .arg(ArgSpec::required("path", ArgKind::String, tk("Image or video path, or clear")))
        .example("wallpaper output DP-1 ~/Pictures/Wallpapers/side.jpg")
        .example("wallpaper output DP-1 clear"),
    );
    r.command(
        owner,
        CommandSpec::new("wallpaper next", tk("Show the next slideshow wallpaper now"), |_, cx| {
            let path = advance(None, cx)?;
            Ok(json!({ "path": path }))
        })
        .example("wallpaper next"),
    );
    r.command(
        owner,
        CommandSpec::new("wallpaper slideshow", tk("Turn the wallpaper slideshow on or off, or set its interval in minutes"), |a, cx| {
            let arg = a.str(0)?.to_string();
            match arg.as_str() {
                "on" | "off" => {
                    let on = arg == "on";
                    update_config(cx, move |c| c.wallpaper.slideshow.enabled = on);
                }
                "toggle" => update_config(cx, |c| c.wallpaper.slideshow.enabled = !c.wallpaper.slideshow.enabled),
                minutes => {
                    let m: u32 = minutes.parse().map_err(|_| mandara_i18n::t("Expected on, off, toggle or a number of minutes").to_string())?;
                    update_config(cx, move |c| c.wallpaper.slideshow.interval_minutes = m.max(MIN_INTERVAL_MIN));
                }
            }
            cx.set_global(LastSlide(Instant::now()));
            let s = config(cx).read(cx).get().wallpaper.slideshow.clone();
            Ok(json!({ "enabled": s.enabled, "interval_minutes": s.interval_minutes }))
        })
        .arg(ArgSpec::required("state", ArgKind::String, tk("on, off, toggle or minutes")))
        .example("wallpaper slideshow on")
        .example("wallpaper slideshow 15"),
    );
}
