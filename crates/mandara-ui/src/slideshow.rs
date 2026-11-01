//! Wallpaper slideshow (`[wallpaper.slideshow]`): every `interval_minutes`
//! the next wallpaper of the slideshow's source becomes the wallpaper.
//!
//! * **Source**: a folder (the wallpaper folder when none is typed, its
//!   sub-folders optionally, a separate folder for the light and the dark
//!   theme) or a playlist of picked wallpapers.
//! * **Order**: shuffled without repeats, random, by name or newest first.
//! * **When**: optionally only between two times of day, paused while a
//!   fullscreen window is open, on battery or while locked, and once at
//!   startup if asked.
//! * **Displays**: the same wallpaper everywhere, or each display its own.
//! * **Transition**: the desktop's, or the slideshow's own style and
//!   length (applied only to wallpapers the slideshow put on screen).
//!
//! No timer runs while the slideshow is off. While it is on, one timer
//! sleeps until the next change is due; a paused change is checked again
//! after [`PAUSED_RECHECK`] (or when the time window opens). Next and
//! previous walk a short history.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{App, Global, Task};
use mandara_config::desktop::{ShuffleDeck, SlideshowOrder, SlideshowSource, SlideshowTransition, WallpaperSlideshow, defaults, newest_first, next_slide, parse_hhmm, parse_interval};
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use serde_json::{Value, json};

use crate::state::{config, update_config};

/// A due change that is paused (fullscreen, battery, locked) is checked
/// again this often: rare enough to cost nothing, soon enough to feel
/// prompt once the pause ends.
const PAUSED_RECHECK: Duration = Duration::from_secs(60);
/// "Change at startup" waits this long so every display is known.
const STARTUP_SETTLE: Duration = Duration::from_secs(2);
/// Seconds per minute.
const S_PER_MIN: u64 = 60;
/// Minutes per hour (local time of day).
const MIN_PER_HOUR: u32 = 60;
/// Slides kept for "previous".
const HISTORY_MAX: usize = 50;

/// What the slideshow put on screen in one step.
#[derive(Clone, Debug, Default, PartialEq)]
struct Slide {
    /// The main wallpaper (`wallpaper.path`).
    main: Option<PathBuf>,
    /// Displays given their own wallpaper (per-display slideshows).
    outputs: Vec<(String, PathBuf)>,
}

impl Slide {
    fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.main.iter().chain(self.outputs.iter().map(|(_, p)| p))
    }
}

/// Why a due change waits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Pause {
    Fullscreen,
    Battery,
    Locked,
    Hours,
}

impl Pause {
    fn slug(self) -> &'static str {
        match self {
            Pause::Fullscreen => "fullscreen",
            Pause::Battery => "battery",
            Pause::Locked => "locked",
            Pause::Hours => "hours",
        }
    }
}

/// The settings a plan was made for (a change to any of them replans).
#[derive(Clone, PartialEq)]
struct Seen {
    slideshow: WallpaperSlideshow,
    drawing: bool,
    dark: bool,
}

#[derive(Default)]
struct Engine {
    /// The last change (or when the slideshow was switched on).
    last: Option<Instant>,
    /// The pending wake-up and when it fires.
    timer: Option<Task<()>>,
    due: Option<Instant>,
    pause: Option<Pause>,
    back: Vec<Slide>,
    ahead: Vec<Slide>,
    deck: ShuffleDeck,
    /// The slide on screen when the slideshow made it (its own
    /// transition applies to these wallpapers only).
    shown: Slide,
    seen: Option<Seen>,
}
impl Global for Engine {}

fn engine(cx: &mut App) -> &mut Engine {
    cx.default_global::<Engine>()
}

/// A seed that differs between calls (no `rand` dependency needed).
fn seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// Minutes since local midnight, and seconds into the current minute.
fn local_minute() -> (u32, u64) {
    use chrono::Timelike as _;
    let now = chrono::Local::now();
    (now.hour() * MIN_PER_HOUR + now.minute(), u64::from(now.second()))
}

/// The folder the slideshow reads now (`None` for a playlist).
pub fn folder(cx: &App) -> Option<PathBuf> {
    let cfg = config(cx).read(cx).snapshot();
    let s = &cfg.wallpaper.slideshow;
    if s.source == SlideshowSource::Playlist {
        return None;
    }
    let typed = s.folder_for(cfg.theme.mode.is_dark());
    Some(if typed.is_empty() { crate::wallpaper::directory(cx) } else { mandara_config::paths::expand_tilde(typed) })
}

/// Where the slideshow's wallpapers come from: read from the settings on
/// the UI thread, listed off it ([`list_pool`]).
#[derive(Clone)]
struct PoolSpec {
    folder: Option<PathBuf>,
    depth: u8,
    playlist: Vec<PathBuf>,
    newest: bool,
}

fn pool_spec(order: SlideshowOrder, cx: &App) -> PoolSpec {
    let s = &config(cx).read(cx).get().wallpaper.slideshow;
    PoolSpec {
        folder: folder(cx),
        depth: if s.subfolders { defaults::SLIDESHOW_FOLDER_DEPTH } else { 1 },
        playlist: s.playlist.clone(),
        newest: order == SlideshowOrder::Newest,
    }
}

/// The wallpapers the slideshow cycles through, in its order's sequence.
/// Blocking (it reads folders): run it on the background executor.
fn list_pool(spec: &PoolSpec) -> Result<Vec<PathBuf>, String> {
    let mut images = match &spec.folder {
        Some(dir) => {
            let found = crate::wallpaper::list_depth(dir, spec.depth);
            if found.is_empty() {
                return Err(mandara_i18n::tf("No images in {folder}", &[("folder", &dir.display())]));
            }
            found
        }
        None => {
            let picked: Vec<PathBuf> = spec.playlist.iter().map(|p| mandara_config::paths::expand_tilde(&p.to_string_lossy())).filter(|p| p.is_file()).collect();
            if picked.is_empty() {
                return Err(mandara_i18n::t("The slideshow playlist is empty").to_string());
            }
            picked
        }
    };
    if spec.newest {
        newest_first(&mut images);
    }
    Ok(images)
}

/// Lists the pool off the UI thread, then hands it to `then`.
fn with_pool(order: SlideshowOrder, cx: &mut App, then: impl FnOnce(Result<Vec<PathBuf>, String>, &mut App) + 'static) {
    let spec = pool_spec(order, cx);
    let task = cx.background_executor().spawn(async move { list_pool(&spec) });
    cx.spawn(async move |cx| {
        let images = task.await;
        cx.update(|cx| then(images, cx));
    })
    .detach();
}

/// The next wallpaper after `current` in `order`.
fn pick(images: &[PathBuf], current: Option<&PathBuf>, order: SlideshowOrder, cx: &mut App) -> Option<PathBuf> {
    match order {
        SlideshowOrder::Shuffle => engine(cx).deck.draw(images, current, seed()),
        other => next_slide(images, current, other, seed()),
    }
}

/// The slide on screen now.
fn on_screen(per_display: bool, cx: &App) -> Slide {
    let w = &config(cx).read(cx).get().wallpaper;
    let outputs = if per_display { crate::backdrop::outputs(cx).into_iter().filter_map(|o| w.variants.monitors.get(&o).map(|p| (o, p.clone()))).collect() } else { Vec::new() };
    Slide { main: w.path.clone(), outputs }
}

/// The next slide of `images` in `order`.
fn next_slide_for(order: SlideshowOrder, images: &[PathBuf], cx: &mut App) -> Result<Slide, String> {
    let s = config(cx).read(cx).get().wallpaper.slideshow.clone();
    // Typed paths may start with `~/`; the folder lists absolute ones.
    let current = config(cx).read(cx).get().wallpaper.path.as_ref().map(|p| mandara_config::paths::expand_tilde(&p.to_string_lossy()));
    let outputs = crate::backdrop::outputs(cx);
    if !s.per_display || outputs.len() < 2 {
        let next = pick(images, current.as_ref(), order, cx).ok_or_else(|| mandara_i18n::t("No wallpapers to show").to_string())?;
        return Ok(Slide { main: Some(next), outputs: Vec::new() });
    }
    // Each display its own: a run of the order, distinct while there are
    // enough wallpapers.
    let mut chosen: Vec<PathBuf> = Vec::new();
    let mut prev = current;
    for _ in &outputs {
        let mut next = pick(images, prev.as_ref(), order, cx);
        if next.as_ref().is_some_and(|n| chosen.contains(n)) && images.len() > chosen.len() {
            next = images.iter().find(|p| !chosen.contains(p)).cloned();
        }
        let Some(next) = next else { break };
        prev = Some(next.clone());
        chosen.push(next);
    }
    Ok(Slide { main: chosen.first().cloned(), outputs: outputs.into_iter().zip(chosen).collect() })
}

/// Puts `slide` on screen (the shown wallpapers change with the
/// slideshow's transition).
fn show(slide: Slide, cx: &mut App) {
    engine(cx).shown = slide.clone();
    update_config(cx, move |c| {
        if let Some(main) = slide.main {
            c.wallpaper.path = Some(main);
        }
        for (output, path) in slide.outputs {
            c.wallpaper.variants.monitors.insert(output, path);
        }
    });
}

/// Remembers `slide` for "previous" (bounded).
fn remember(slide: Slide, cx: &mut App) {
    let e = engine(cx);
    if e.back.last() != Some(&slide) {
        e.back.push(slide);
    }
    if e.back.len() > HISTORY_MAX {
        e.back.remove(0);
    }
}

/// Shows the next slideshow wallpaper (`order` overrides the config's)
/// and restarts the interval; `done` gets the main wallpaper. The folder
/// is read off the UI thread.
pub fn advance(order: Option<SlideshowOrder>, cx: &mut App, done: impl FnOnce(Result<PathBuf, String>, &mut App) + 'static) {
    // Forward again through slides stepped back over first.
    if order.is_none()
        && let Some(slide) = engine(cx).ahead.pop()
    {
        let main = step_to(slide, cx);
        done(Ok(main), cx);
        return;
    }
    engine(cx).ahead.clear();
    let order = order.unwrap_or(config(cx).read(cx).get().wallpaper.slideshow.order);
    with_pool(order, cx, move |images, cx| {
        let result = images.and_then(|images| next_slide_for(order, &images, cx)).map(|slide| step_to(slide, cx));
        done(result, cx);
    });
}

/// [`advance`], logging a failure.
pub fn advance_logged(cx: &mut App) {
    advance(None, cx, |result, _| {
        if let Err(err) = result {
            tracing::warn!("slideshow: {err}");
        }
    });
}

/// Puts `slide` on screen, remembering the one it replaces.
fn step_to(slide: Slide, cx: &mut App) -> PathBuf {
    let per_display = config(cx).read(cx).get().wallpaper.slideshow.per_display;
    let main = slide.main.clone().unwrap_or_default();
    let before = on_screen(per_display, cx);
    remember(before, cx);
    show(slide, cx);
    restart(cx);
    main
}

/// Goes back to the wallpaper shown before the last slideshow change.
pub fn previous(cx: &mut App) -> Result<PathBuf, String> {
    let per_display = config(cx).read(cx).get().wallpaper.slideshow.per_display;
    let slide = engine(cx).back.pop().ok_or_else(|| mandara_i18n::t("No earlier wallpaper").to_string())?;
    let now = on_screen(per_display, cx);
    engine(cx).ahead.push(now);
    let main = slide.main.clone().unwrap_or_default();
    show(slide, cx);
    restart(cx);
    Ok(main)
}

/// Starts a full interval from now.
fn restart(cx: &mut App) {
    engine(cx).last = Some(Instant::now());
    schedule(cx);
}

fn fullscreen_open(cx: &App) -> bool {
    cx.try_global::<crate::hub::GlobalHub>().is_some_and(|g| g.0.read(cx).clients.iter().any(|c| c.fullscreen))
}

/// Why a change can't happen right now, if anything stops it.
fn pause_reason(s: &WallpaperSlideshow, cx: &App) -> Option<Pause> {
    if !s.in_hours(local_minute().0) {
        Some(Pause::Hours)
    } else if s.pause_when_locked && (crate::lock::is_locked() || crate::powerctl::idle::manager(cx).is_some_and(|m| m.read(cx).screens_off())) {
        Some(Pause::Locked)
    } else if s.pause_on_battery && crate::powerctl::battery::on_battery(cx) {
        Some(Pause::Battery)
    } else if s.pause_on_fullscreen && fullscreen_open(cx) {
        Some(Pause::Fullscreen)
    } else {
        None
    }
}

/// How long until the time window opens (zero inside it).
fn until_open(s: &WallpaperSlideshow) -> Duration {
    let (minute, second) = local_minute();
    match s.minutes_until_open(minute) {
        0 => Duration::ZERO,
        m => Duration::from_secs((u64::from(m) * S_PER_MIN).saturating_sub(second)),
    }
}

/// Plans the next wake-up: none while the slideshow is off.
fn schedule(cx: &mut App) {
    let (s, drawing) = {
        let c = config(cx).read(cx).get();
        (c.wallpaper.slideshow.clone(), c.wallpaper.enabled)
    };
    if !s.enabled || !drawing {
        let e = engine(cx);
        (e.timer, e.due, e.pause) = (None, None, None);
        return;
    }
    let now = Instant::now();
    let last = *engine(cx).last.get_or_insert(now);
    let interval = Duration::from_secs(u64::from(s.interval()) * S_PER_MIN);
    let wait = (last + interval).saturating_duration_since(now).max(until_open(&s));
    wake_in(wait, cx);
}

fn wake_in(wait: Duration, cx: &mut App) {
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(tick);
    });
    let e = engine(cx);
    e.due = Some(Instant::now() + wait);
    e.timer = Some(task);
}

/// A wake-up: change now, or wait out a pause.
fn tick(cx: &mut App) {
    let s = config(cx).read(cx).get().wallpaper.slideshow.clone();
    let reason = pause_reason(&s, cx);
    engine(cx).pause = reason;
    match reason {
        Some(Pause::Hours) => wake_in(until_open(&s).max(PAUSED_RECHECK), cx),
        Some(_) => wake_in(PAUSED_RECHECK, cx),
        None => advance(None, cx, |result, cx| {
            if let Err(err) = result {
                tracing::warn!("slideshow: {err}");
                // Try again after a full interval, not at once.
                restart(cx);
            }
        }),
    }
}

/// Follows settings changes: replans the timer, starts a fresh interval
/// when switched on, follows a light / dark folder when the theme flips.
fn on_config(cx: &mut App) {
    let seen = {
        let c = config(cx).read(cx).get();
        Seen { slideshow: c.wallpaper.slideshow.clone(), drawing: c.wallpaper.enabled, dark: c.theme.mode.is_dark() }
    };
    let Some(before) = engine(cx).seen.replace(seen.clone()) else {
        schedule(cx);
        return;
    };
    if before == seen {
        return;
    }
    let (was, now) = (&before.slideshow, &seen.slideshow);
    if !was.enabled && now.enabled {
        engine(cx).last = Some(Instant::now());
    }
    if was.order != now.order || was.source != now.source || was.directory != now.directory || was.playlist != now.playlist {
        engine(cx).deck = ShuffleDeck::default();
    }
    if was.per_display && !now.per_display {
        cx.defer(clear_display_slides);
    }
    let themed_folder = now.source == SlideshowSource::Folder && now.folder_for(seen.dark) != now.folder_for(before.dark);
    if now.enabled && seen.drawing && before.dark != seen.dark && themed_folder {
        cx.defer(advance_logged);
    }
    schedule(cx);
}

/// Per-display slides end: displays showing a slideshow wallpaper follow
/// the main one again (your own per-display choices stay).
fn clear_display_slides(cx: &mut App) {
    let order = config(cx).read(cx).get().wallpaper.slideshow.order;
    with_pool(order, cx, |images, cx| {
        let images = images.unwrap_or_default();
        update_config(cx, move |c| c.wallpaper.variants.monitors.retain(|_, p| !images.contains(p)));
    });
}

/// The desktop's transition settings for this scene: the slideshow's own
/// transition and length while a wallpaper it put on screen is shown.
pub(crate) fn scene_transition(w: &mut mandara_config::WallpaperConfig, cx: &App) {
    let Some(kind) = w.slideshow.transition.own() else { return };
    let Some(e) = cx.try_global::<Engine>() else { return };
    let showing = e.shown.paths().any(|p| w.path.as_ref() == Some(p) || w.variants.monitors.values().any(|m| m == p));
    if showing {
        w.transition = kind;
        w.crossfade_ms = w.slideshow.transition_len();
    }
}

/// Runs the slideshow (shell process).
pub fn init(cx: &mut App) {
    cx.set_global(Engine::default());
    on_config(cx);
    cx.observe(&config(cx), |_, cx| on_config(cx)).detach();
    let s = config(cx).read(cx).get().wallpaper.slideshow.clone();
    if s.enabled && s.on_start {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(STARTUP_SETTLE).await;
            cx.update(advance_logged);
        })
        .detach();
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

fn order_slug(o: SlideshowOrder) -> &'static str {
    match o {
        SlideshowOrder::Random => "random",
        SlideshowOrder::Shuffle => "shuffle",
        SlideshowOrder::Alphabetical => "alphabetical",
        SlideshowOrder::Newest => "newest",
    }
}

fn transition_slug(t: SlideshowTransition) -> &'static str {
    match t {
        SlideshowTransition::Desktop => "desktop",
        SlideshowTransition::Fade => "fade",
        SlideshowTransition::Wipe => "wipe",
        SlideshowTransition::Grow => "grow",
        SlideshowTransition::None => "none",
    }
}

/// The slideshow's settings and state as JSON (every command's reply),
/// with how many wallpapers are in rotation counted off the UI thread.
fn status(cx: &mut App) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let mut value = status_now(cx);
    let order = config(cx).read(cx).get().wallpaper.slideshow.order;
    with_pool(order, cx, move |pool, _| {
        value["count"] = json!(pool.as_ref().map_or(0, Vec::len));
        value["error"] = json!(pool.err());
        let _ = tx.try_send(Ok(value.to_string()));
    });
    Output::Pending(rx)
}

/// Shows the next wallpaper, then replies with it (`{"path": …}`) or,
/// with `with_status`, the slideshow's status.
fn after_advance(with_status: bool, cx: &mut App) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    advance(None, cx, move |result, cx| match result {
        Err(e) => {
            let _ = tx.try_send(Err(e));
        }
        Ok(path) if !with_status => {
            let _ = tx.try_send(Ok(json!({ "path": path }).to_string()));
        }
        Ok(_) => match status(cx) {
            Output::Value(v) => {
                let _ = tx.try_send(Ok(v.to_string()));
            }
            Output::Pending(reply) => cx
                .spawn(async move |_| {
                    if let Ok(r) = reply.recv().await {
                        let _ = tx.send(r).await;
                    }
                })
                .detach(),
        },
    });
    Output::Pending(rx)
}

/// [`status`] without the count.
fn status_now(cx: &mut App) -> Value {
    let cfg = config(cx).read(cx).snapshot();
    let s = &cfg.wallpaper.slideshow;
    let e = engine(cx);
    let next_in = e.due.filter(|_| e.timer.is_some()).map(|d| d.saturating_duration_since(Instant::now()).as_secs());
    json!({
        "enabled": s.enabled,
        "running": e.timer.is_some(),
        "interval_minutes": s.interval(),
        "order": order_slug(s.order),
        "source": if s.source == SlideshowSource::Playlist { "playlist" } else { "folder" },
        "folder": folder(cx),
        "subfolders": s.subfolders,
        "light_folder": s.light_directory,
        "dark_folder": s.dark_directory,
        "playlist": s.playlist,
        "current": cfg.wallpaper.path,
        "next_in_seconds": next_in,
        "paused": engine(cx).pause.map(Pause::slug),
        "can_go_back": !engine(cx).back.is_empty(),
        "on_start": s.on_start,
        "pause_on_fullscreen": s.pause_on_fullscreen,
        "pause_on_battery": s.pause_on_battery,
        "pause_when_locked": s.pause_when_locked,
        "hours": if s.hours { json!({ "from": s.from, "until": s.until }) } else { Value::Null },
        "transition": transition_slug(s.transition),
        "transition_ms": s.transition_len(),
        "per_display": s.per_display,
    })
}

/// Applies a settings edit, then replies with the status.
fn edit(cx: &mut App, f: impl FnOnce(&mut WallpaperSlideshow) + 'static) -> Output {
    update_config(cx, move |c| f(&mut c.wallpaper.slideshow));
    // Replan now, so the reply already shows the next change.
    on_config(cx);
    status(cx)
}

/// `on`, `off` or `toggle` against `current`.
fn switch(word: &str, current: bool) -> Result<bool, String> {
    match word {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        "toggle" => Ok(!current),
        _ => Err(mandara_i18n::t("Expected on, off or toggle").to_string()),
    }
}

/// A slideshow option by its command name.
fn option_field(name: &str) -> Option<fn(&mut WallpaperSlideshow) -> &mut bool> {
    Some(match name {
        "on-start" => |s| &mut s.on_start,
        "pause-fullscreen" => |s| &mut s.pause_on_fullscreen,
        "pause-battery" => |s| &mut s.pause_on_battery,
        "pause-locked" => |s| &mut s.pause_when_locked,
        "subfolders" => |s| &mut s.subfolders,
        "per-display" => |s| &mut s.per_display,
        _ => return None,
    })
}

const OPTIONS: &[&str] = &["on-start", "pause-fullscreen", "pause-battery", "pause-locked", "subfolders", "per-display"];

/// A wallpaper path from a command argument (must exist).
fn existing(arg: &str) -> Result<PathBuf, String> {
    let path = mandara_config::paths::expand_tilde(arg.trim());
    if path.is_file() { Ok(path) } else { Err(mandara_i18n::tf("Not a file: {path}", &[("path", &path.display())])) }
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
            let path = existing(&value)?;
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
        CommandSpec::new_output("wallpaper next", tk("Show the next slideshow wallpaper now"), |_, cx| Ok(after_advance(false, cx)))
        .example("wallpaper next"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow", tk("Turn the wallpaper slideshow on or off, or set its interval (minutes, or 30m, 2h, 1d)"), |a, cx| {
            let arg = a.str(0)?.to_string();
            match arg.as_str() {
                "on" | "off" | "toggle" => {
                    let on = switch(&arg, config(cx).read(cx).get().wallpaper.slideshow.enabled)?;
                    update_config(cx, move |c| c.wallpaper.slideshow.enabled = on);
                    on_config(cx);
                }
                spec => {
                    let m = parse_interval(spec).ok_or_else(|| mandara_i18n::t("Expected on, off, toggle or a number of minutes").to_string())?;
                    update_config(cx, move |c| c.wallpaper.slideshow.interval_minutes = m.clamp(defaults::SLIDESHOW_INTERVAL_RANGE.0, defaults::SLIDESHOW_INTERVAL_RANGE.1));
                    restart(cx);
                }
            }
            Ok(status(cx))
        })
        .arg(ArgSpec::required("state", ArgKind::String, tk("on, off, toggle or an interval")))
        .example("wallpaper slideshow on")
        .example("wallpaper slideshow 15")
        .example("wallpaper slideshow 2h"),
    );
    r.command(owner, CommandSpec::new_output("wallpaper slideshow status", tk("The slideshow's settings, source size, next change and pause"), |_, cx| Ok(status(cx))).example("wallpaper slideshow status"));
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow next", tk("Skip to the next slideshow wallpaper"), |_, cx| Ok(after_advance(true, cx)))
        .example("wallpaper slideshow next"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow previous", tk("Go back to the wallpaper shown before"), |_, cx| {
            previous(cx)?;
            Ok(status(cx))
        })
        .example("wallpaper slideshow previous"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow interval", tk("Set the time between wallpapers (minutes, or 30m, 2h, 1d)"), |a, cx| {
            let m = parse_interval(&a.rest(0)).ok_or_else(|| mandara_i18n::t("Expected a time such as 45, 30m, 2h or 1d").to_string())?;
            update_config(cx, move |c| c.wallpaper.slideshow.interval_minutes = m.clamp(defaults::SLIDESHOW_INTERVAL_RANGE.0, defaults::SLIDESHOW_INTERVAL_RANGE.1));
            restart(cx);
            Ok(status(cx))
        })
        .arg(ArgSpec::required("time", ArgKind::String, tk("minutes, or 30m, 2h, 1d")))
        .example("wallpaper slideshow interval 2h"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow order", tk("Set the slideshow order"), |a, cx| {
            let order = match a.str(0)? {
                "random" => SlideshowOrder::Random,
                "alphabetical" => SlideshowOrder::Alphabetical,
                "newest" => SlideshowOrder::Newest,
                _ => SlideshowOrder::Shuffle,
            };
            Ok(edit(cx, move |s| s.order = order))
        })
        .arg(ArgSpec::required("order", ArgKind::Enum, tk("how to go through the wallpapers")).choices(&["shuffle", "random", "alphabetical", "newest"]))
        .example("wallpaper slideshow order shuffle"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow source", tk("Cycle through the wallpaper folder, another folder or the playlist"), |a, cx| {
            let kind = a.str(0)?.to_string();
            let dir = a.rest(1);
            match kind.as_str() {
                "playlist" => Ok(edit(cx, |s| s.source = SlideshowSource::Playlist)),
                "folder" if !dir.trim().is_empty() => {
                    let path = mandara_config::paths::expand_tilde(dir.trim());
                    if !path.is_dir() {
                        return Err(mandara_i18n::tf("Not a folder: {path}", &[("path", &path.display())]));
                    }
                    let typed = dir.trim().to_string();
                    Ok(edit(cx, move |s| {
                        s.source = SlideshowSource::Folder;
                        s.directory = typed;
                    }))
                }
                _ => Ok(edit(cx, |s| {
                    s.source = SlideshowSource::Folder;
                    s.directory.clear();
                })),
            }
        })
        .arg(ArgSpec::required("source", ArgKind::Enum, tk("where the wallpapers come from")).choices(&["wallpapers", "folder", "playlist"]))
        .arg(ArgSpec::optional("folder", ArgKind::String, tk("a folder (with folder)")))
        .example("wallpaper slideshow source folder ~/Pictures/Favourites")
        .example("wallpaper slideshow source wallpapers")
        .example("wallpaper slideshow source playlist"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow folder", tk("Use another folder while the theme is light or dark (clear to remove)"), |a, cx| {
            let dark = a.str(0)? == "dark";
            let value = a.rest(1);
            let typed = if value.trim().is_empty() || value.trim() == "clear" {
                String::new()
            } else {
                let path = mandara_config::paths::expand_tilde(value.trim());
                if !path.is_dir() {
                    return Err(mandara_i18n::tf("Not a folder: {path}", &[("path", &path.display())]));
                }
                value.trim().to_string()
            };
            Ok(edit(cx, move |s| if dark { s.dark_directory = typed } else { s.light_directory = typed }))
        })
        .arg(ArgSpec::required("theme", ArgKind::Enum, tk("light or dark")).choices(&["light", "dark"]))
        .arg(ArgSpec::required("folder", ArgKind::String, tk("a folder, or clear")))
        .example("wallpaper slideshow folder dark ~/Pictures/Night"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow playlist", tk("List, add to, remove from or clear the slideshow playlist"), |a, cx| {
            let action = a.str(0)?.to_string();
            let arg = a.rest(1);
            match action.as_str() {
                "add" => {
                    let path = existing(&arg)?;
                    Ok(edit(cx, move |s| {
                        if !s.playlist.contains(&path) {
                            s.playlist.push(path);
                        }
                    }))
                }
                "remove" => {
                    let path = mandara_config::paths::expand_tilde(arg.trim());
                    Ok(edit(cx, move |s| s.playlist.retain(|p| *p != path)))
                }
                "clear" => Ok(edit(cx, |s| s.playlist.clear())),
                _ => Ok(Output::Value(json!({ "playlist": config(cx).read(cx).get().wallpaper.slideshow.playlist }))),
            }
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("what to do")).choices(&["list", "add", "remove", "clear"]))
        .arg(ArgSpec::optional("path", ArgKind::String, tk("a wallpaper (with add / remove)")))
        .example("wallpaper slideshow playlist add ~/Pictures/Wallpapers/dunes.jpg")
        .example("wallpaper slideshow playlist list"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow hours", tk("Change wallpapers only between two times of day, or always (off)"), |a, cx| {
            let arg = a.rest(0);
            if arg.trim() == "off" {
                return Ok(edit(cx, |s| s.hours = false));
            }
            let (from, until) = arg.split_once('-').filter(|(f, u)| parse_hhmm(f).is_some() && parse_hhmm(u).is_some()).ok_or_else(|| mandara_i18n::t("Expected off or a range such as 08:00-22:00").to_string())?;
            let (from, until) = (from.trim().to_string(), until.trim().to_string());
            Ok(edit(cx, move |s| {
                s.hours = true;
                s.from = from;
                s.until = until;
            }))
        })
        .arg(ArgSpec::required("hours", ArgKind::String, tk("off, or HH:MM-HH:MM")))
        .example("wallpaper slideshow hours 08:00-22:00")
        .example("wallpaper slideshow hours off"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow transition", tk("How slideshow changes look: the desktop's transition or their own, with a length in ms"), |a, cx| {
            let kind = match a.str(0)? {
                "fade" => SlideshowTransition::Fade,
                "wipe" => SlideshowTransition::Wipe,
                "grow" => SlideshowTransition::Grow,
                "none" => SlideshowTransition::None,
                _ => SlideshowTransition::Desktop,
            };
            let ms = a.get(1).map(|v| v.parse::<u64>().map_err(|_| mandara_i18n::t("Expected a length in milliseconds").to_string())).transpose()?;
            Ok(edit(cx, move |s| {
                s.transition = kind;
                if let Some(ms) = ms {
                    s.transition_ms = ms.clamp(defaults::SLIDESHOW_TRANSITION_RANGE.0, defaults::SLIDESHOW_TRANSITION_RANGE.1);
                }
            }))
        })
        .arg(ArgSpec::required("style", ArgKind::Enum, tk("transition style")).choices(&["desktop", "fade", "wipe", "grow", "none"]))
        .arg(ArgSpec::optional("ms", ArgKind::Integer, tk("length in milliseconds")))
        .example("wallpaper slideshow transition grow 1500"),
    );
    r.command(
        owner,
        CommandSpec::new_output("wallpaper slideshow option", tk("Turn a slideshow option on or off"), |a, cx| {
            let field = option_field(a.str(0)?).ok_or_else(|| mandara_i18n::t("Unknown option").to_string())?;
            let mut probe = config(cx).read(cx).get().wallpaper.slideshow.clone();
            let on = switch(a.str(1)?, *field(&mut probe))?;
            Ok(edit(cx, move |s| *field(s) = on))
        })
        .arg(ArgSpec::required("option", ArgKind::Enum, tk("which option")).choices(OPTIONS))
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on, off or toggle")).choices(&["on", "off", "toggle"]))
        .example("wallpaper slideshow option pause-battery on")
        .example("wallpaper slideshow option per-display toggle"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_name_their_fields() {
        for name in OPTIONS {
            let field = option_field(name).unwrap_or_else(|| panic!("{name} has no field"));
            let mut s = WallpaperSlideshow::default();
            let before = *field(&mut s);
            *field(&mut s) = !before;
            assert_ne!(s, WallpaperSlideshow::default(), "{name} changes a setting");
        }
        assert!(option_field("nope").is_none());
        assert_eq!(switch("toggle", true), Ok(false));
        assert!(switch("maybe", true).is_err());
    }

    #[test]
    fn slides_list_every_wallpaper_they_show() {
        let slide = Slide { main: Some("/w/a.jpg".into()), outputs: vec![("DP-1".into(), "/w/b.jpg".into())] };
        assert_eq!(slide.paths().collect::<Vec<_>>(), [&PathBuf::from("/w/a.jpg"), &PathBuf::from("/w/b.jpg")]);
        assert_eq!(Pause::Hours.slug(), "hours");
        assert_eq!(order_slug(SlideshowOrder::Newest), "newest");
        assert_eq!(transition_slug(SlideshowTransition::Grow), "grow");
    }
}
