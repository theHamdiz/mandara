//! Wallpaper generator plugin (`wallpaper-generator`, opt-in): procedural
//! wallpapers from the pure-Rust engine in `mandara-wallgen`, saved as
//! compact lossless WebP (or PNG / JPEG) or as seamless AV1 video loops,
//! each with a recipe sidecar so it can be re-rendered or remixed.
//!
//! * **Jobs** ([`jobs`]): one at a time on a background thread; progress
//!   and cancel are shared through the [`Wallgen`] entity, so the UI thread
//!   only reads numbers. Results land in the output folder (by default
//!   `Generated` inside the wallpaper folder, which the picker scans) and
//!   become the wallpaper when asked.
//! * **Schedule** ([`schedule`]): opt-in. While it is off there is no
//!   timer and no thread; on, one timer waits for the next run, the last
//!   run survives restarts and missed runs never pile up.
//! * **Commands** (`wallgen …`): generate, styles, palettes, recipe,
//!   remix, schedule, cancel, list and status, all answering JSON.
//! * Studio's Wallpaper page shows the generator when the plugin is on,
//!   and the wallpaper picker gains a Generate button, a "Generated"
//!   marker and Remix.
//!
//! Settings: `[plugins.settings.wallpaper-generator]`
//! ([`mandara_config::wallgen::WallgenSettings`]).

pub mod cli;
pub mod jobs;
pub mod model;
pub mod schedule;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::wallgen::{ScheduleMode, WALLGEN_ID, WallgenKind, WallgenSettings, WallgenSize};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginManifest, PluginRegistry};
use mandara_wallgen::{COMMON_PARAMS, PaletteSpec, Recipe, Style};
use serde_json::{Value, json};

use crate::state::{config, update_config};
use cli::{Generate, PaletteChoice, Size, Source};
use jobs::{Finished, JobRunner, Origin, Request};
use schedule::{Action, Plan, ScheduleState, Scheduler};

pub const ID: &str = WALLGEN_ID;

/// A waiting `wallgen generate` answers "still running" after this long,
/// inside the IPC client's 15 s reply timeout (the job carries on; its
/// result is in `wallgen status` and `wallgen list`).
const REPLY_BUDGET: Duration = Duration::from_secs(12);
/// Views showing a running job repaint this often (progress, elapsed time).
pub const PROGRESS_TICK: Duration = Duration::from_millis(120);
/// The schedule's timer wakes at least this often while it is on, so a
/// suspend (monotonic clocks stop) can't push a run back by hours.
const MAX_TIMER: Duration = Duration::from_secs(10 * 60);
/// `wallgen list` shows this many files by default.
const LIST_DEFAULT: usize = 50;

/// The generator's live state (shell process).
pub struct Wallgen {
    runner: JobRunner,
    scheduler: Scheduler,
    /// The schedule's timer: `Some` only while the schedule is on.
    timer: Option<Task<()>>,
    state: ScheduleState,
    state_path: PathBuf,
    /// Repaints watchers while a job runs; `None` when idle.
    ticker: Option<Task<()>>,
    /// `wallgen generate` calls waiting for their job.
    waiters: Vec<(u64, async_channel::Sender<Result<String, String>>)>,
    /// The newest finished job (picker insertion, status).
    pub finished: Option<Finished>,
}

#[derive(Clone)]
struct GlobalWallgen(Entity<Wallgen>);
impl Global for GlobalWallgen {}

/// The generator entity (created on first use).
pub fn wallgen(cx: &mut App) -> Entity<Wallgen> {
    if let Some(g) = cx.try_global::<GlobalWallgen>() {
        return g.0.clone();
    }
    let state_path = ScheduleState::path();
    let entity = cx.new(|_| Wallgen {
        runner: JobRunner::default(),
        scheduler: Scheduler::starting(SystemTime::now()),
        timer: None,
        state: ScheduleState::load(&state_path),
        state_path,
        ticker: None,
        waiters: Vec::new(),
        finished: None,
    });
    cx.set_global(GlobalWallgen(entity.clone()));
    entity
}

/// The entity if it exists (views that only show state don't create it).
pub fn existing(cx: &App) -> Option<Entity<Wallgen>> {
    cx.try_global::<GlobalWallgen>().map(|g| g.0.clone())
}

/// Whether the plugin is on.
pub fn enabled(cx: &App) -> bool {
    cx.try_global::<PluginRegistry>().is_some_and(|r| r.is_enabled(ID, &config(cx).read(cx).get().plugins))
}

pub fn settings(cx: &App) -> WallgenSettings {
    WallgenSettings::from_plugins(&config(cx).read(cx).get().plugins)
}

/// Edits the settings (one config write).
pub fn update_settings(cx: &mut App, f: impl FnOnce(&mut WallgenSettings)) {
    update_config(cx, |c| {
        let mut s = WallgenSettings::from_plugins(&c.plugins);
        f(&mut s);
        s.store(&mut c.plugins);
    });
}

/// Watches the config so the schedule follows its settings and the
/// plugin's on / off state (shell process). Nothing runs while it's off.
pub fn init(cx: &mut App) {
    let store = config(cx);
    let sync = |cx: &mut App| {
        // No entity, no schedule: stay at zero cost until something is on.
        let on = settings(cx).schedule.enabled && enabled(cx);
        if on || existing(cx).is_some() {
            wallgen(cx).update(cx, |w, cx| w.reconcile(cx));
        }
    };
    sync(cx);
    cx.observe(&store, move |_, cx| sync(cx)).detach();
}

/// Everything a request is built from, read once from the shell.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub settings: WallgenSettings,
    pub theme_dark: bool,
    /// The live theme as a palette (`--palette theme`, "Match theme").
    pub theme_palette: PaletteSpec,
    pub wallpaper_dir: PathBuf,
    /// The primary display's logical size.
    pub display: Option<(u32, u32)>,
    /// A recipe to vary instead of the settings' one (the picker's focused
    /// generated wallpaper).
    pub base: Option<Recipe>,
    /// Seed for random draws and variations.
    pub seed: u64,
}

impl Inputs {
    pub fn read(cx: &App) -> Self {
        let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
        let (w, h) = crate::overlays::primary_output_size(cx);
        Inputs {
            settings: settings(cx),
            theme_dark: palette.is_dark,
            theme_palette: model::theme_palette(&palette),
            wallpaper_dir: crate::wallpaper::directory(cx),
            display: (w >= 1.0 && h >= 1.0).then_some((w as u32, h as u32)),
            base: None,
            seed: model::fresh_seed(),
        }
    }
}

/// Builds the request `g` describes (pure: no shell access).
pub fn build_request(inputs: &Inputs, g: &Generate, origin: Origin) -> Result<Request, String> {
    let s = &inputs.settings;
    let light = g.light.unwrap_or_else(|| s.tone.is_light(inputs.theme_dark));
    let current = model::recipe(s, inputs.theme_dark);
    let lock = s.seed_locked.then_some(current.seed);
    let mut recipe = match g.source.clone().unwrap_or(Source::Recipe) {
        Source::Recipe => current,
        Source::Random(style) => model::random_recipe(style, inputs.seed, light, lock),
        Source::Variation => model::variation(inputs.base.as_ref().unwrap_or(&current), s.variation, inputs.seed, s.seed_locked),
        Source::Reseed => model::reseeded(&current, inputs.seed),
    };
    recipe.light = light;
    if let Some(seed) = g.seed {
        recipe.seed = seed;
    }
    match &g.palette {
        Some(PaletteChoice::Spec(spec)) => recipe.palette = spec.clone(),
        Some(PaletteChoice::Theme) => recipe.palette = inputs.theme_palette.clone(),
        None => {}
    }
    recipe.sanitize();
    let dir = s.output_dir(&inputs.wallpaper_dir);
    let exact = |size: Size| match size {
        Size::Class(class) => model::resolution(class, (s.custom_width, s.custom_height), inputs.display),
        Size::Exact(r) => r,
    };
    let output = if g.video {
        let mut options = model::video_options(s, inputs.display);
        if let Some(size) = g.size {
            options.resolution = match size {
                Size::Class(WallgenSize::Display) => model::video_resolution(&WallgenSettings { video: mandara_config::wallgen::WallgenVideo { size: WallgenSize::Display, ..s.video.clone() }, ..s.clone() }, inputs.display),
                other => exact(other),
            };
        }
        options.seconds = g.seconds.unwrap_or(options.seconds);
        options.fps = g.fps.unwrap_or(options.fps);
        jobs::Output::Video { options, dir }
    } else {
        let mut options = model::output_options(s, &inputs.wallpaper_dir, inputs.display);
        if let Some(size) = g.size {
            options.resolution = exact(size);
        }
        if let Some(f) = g.format {
            options.format = model::format(&WallgenSettings { format: f, ..s.clone() });
        }
        jobs::Output::Still(options)
    };
    let (apply, keep) = match origin {
        Origin::Schedule => (s.schedule.apply, (s.schedule.keep > 0).then_some(s.schedule.keep as usize)),
        Origin::Manual | Origin::Picker => (g.apply.unwrap_or(s.apply), None),
    };
    Ok(Request { recipe, output, apply, keep, origin })
}

/// What a scheduled run asks for.
pub fn scheduled(settings: &WallgenSettings) -> Generate {
    let source = match settings.schedule.mode {
        ScheduleMode::Random => Source::Random(None),
        ScheduleMode::Variation => Source::Variation,
        ScheduleMode::Reseed => Source::Reseed,
    };
    Generate { source: Some(source), video: settings.schedule.kind == WallgenKind::Video, ..Generate::default() }
}

impl Wallgen {
    pub fn busy(&self) -> bool {
        self.runner.busy()
    }

    pub fn status(&self) -> jobs::Status {
        self.runner.status()
    }

    /// The running job's progress (0–1, stills: `None`) and seconds.
    pub fn progress(&self) -> Option<(Option<f32>, f64)> {
        self.runner.status().running.map(|(_, req, p, secs, _)| (req.is_video().then_some(p), secs))
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) -> bool {
        let cancelled = self.runner.cancel();
        cx.notify();
        cancelled
    }

    /// Starts `request`; the result arrives through [`Self::finish`].
    pub fn start(&mut self, request: Request, cx: &mut Context<Self>) -> Result<u64, String> {
        let (tx, rx) = async_channel::bounded::<Finished>(1);
        let id = self.runner.start(request, move |f| {
            let _ = tx.send_blocking(f);
        })?;
        cx.spawn(async move |this, cx| {
            if let Ok(finished) = rx.recv().await {
                let _ = this.update(cx, |w, cx| w.finish(finished, cx));
            }
        })
        .detach();
        // Repaint watchers (Studio's status poll, the picker's pill) while it runs.
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_TICK).await;
                let Ok(busy) = this.update(cx, |w, cx| {
                    cx.notify();
                    w.busy()
                }) else {
                    break;
                };
                if !busy {
                    break;
                }
            }
        }));
        cx.notify();
        Ok(id)
    }

    /// A job ended: apply it when asked, answer waiting commands.
    fn finish(&mut self, finished: Finished, cx: &mut Context<Self>) {
        self.ticker = None;
        if let Ok(done) = &finished.result
            && finished.request.apply
        {
            crate::wallpaper::apply(done.file.path.clone(), cx);
        }
        let reply = finished.result.as_ref().map(|d| d.json().to_string()).map_err(Clone::clone);
        let id = finished.id;
        self.waiters.retain(|(waiting, tx)| {
            if *waiting == id {
                let _ = tx.try_send(reply.clone());
                false
            } else {
                true
            }
        });
        if let Err(e) = &finished.result {
            tracing::warn!("wallpaper generator: {e}");
        }
        self.finished = Some(finished);
        cx.notify();
    }

    /// A pending command reply for job `id`: the result, or "running"
    /// after [`REPLY_BUDGET`].
    fn wait(&mut self, id: u64, cx: &mut Context<Self>) -> async_channel::Receiver<Result<String, String>> {
        let (tx, rx) = async_channel::bounded(1);
        self.waiters.push((id, tx));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REPLY_BUDGET).await;
            let _ = this.update(cx, |w, cx| {
                let status = w.status().json();
                w.waiters.retain(|(waiting, tx)| {
                    if *waiting == id {
                        let reply = json!({ "status": "running", "id": id, "job": status["running"], "hint": "wallgen status" });
                        let _ = tx.try_send(Ok(reply.to_string()));
                        false
                    } else {
                        true
                    }
                });
                cx.notify();
            });
        })
        .detach();
        rx
    }

    /// Brings the schedule's timer in line with the settings.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let s = settings(cx);
        let active = enabled(cx);
        let now = SystemTime::now();
        let mut p = schedule::plan(&s.schedule, active, self.state.last(), now);
        if p == Plan::Start {
            // Turned on: the clock starts now (Generate runs one at once).
            self.state.set_last(now);
            self.persist();
            p = schedule::plan(&s.schedule, active, self.state.last(), now);
        }
        if p == Plan::Off {
            // Off again: the next "on" starts a fresh interval.
            if self.state.last_run.is_some() && !s.schedule.enabled {
                self.state = ScheduleState::default();
                self.persist();
            }
        }
        match self.scheduler.reconcile(p, now) {
            Action::Keep => {}
            Action::Disarm => self.timer = None,
            Action::Arm(after) => {
                let after = after.min(MAX_TIMER);
                self.timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(after).await;
                    let _ = this.update(cx, |w, cx| w.fire(cx));
                }));
            }
        }
    }

    /// The timer fired: run when due, then plan the next one.
    fn fire(&mut self, cx: &mut Context<Self>) {
        self.timer = None;
        let s = settings(cx);
        let now = SystemTime::now();
        let p = schedule::plan(&s.schedule, enabled(cx), self.state.last(), now);
        if self.scheduler.fire(p, now) {
            if self.busy() {
                self.scheduler.defer_until(now + schedule::BUSY_RETRY);
            } else {
                // Recorded before the run, so a crash mid-job can't repeat it.
                self.state.set_last(now);
                self.persist();
                let inputs = Inputs::read(cx);
                match build_request(&inputs, &scheduled(&s), Origin::Schedule).and_then(|req| self.start(req, cx)) {
                    Ok(id) => tracing::info!("wallpaper generator: scheduled job {id} started"),
                    Err(e) => tracing::warn!("wallpaper generator: scheduled run: {e}"),
                }
            }
        }
        self.reconcile(cx);
    }

    fn persist(&self) {
        if let Err(e) = self.state.save(&self.state_path) {
            tracing::warn!("wallpaper generator: saving {}: {e}", self.state_path.display());
        }
    }

    /// The schedule for status displays: `(enabled, next run, last run)`.
    pub fn schedule_status(&self, s: &WallgenSettings, active: bool) -> (bool, Option<SystemTime>, Option<SystemTime>) {
        let last = self.state.last();
        (s.schedule.enabled && active, schedule::next_run(&s.schedule, active, last, SystemTime::now()), last)
    }

    /// Whether a timer exists (tests and diagnostics: none while off).
    pub fn timer_armed(&self) -> bool {
        self.timer.is_some()
    }
}

/// Starts a generation from the UI or a command; stores a manual run's new
/// recipe as the current one so Studio and variations build on it.
pub fn generate(g: &Generate, base: Option<Recipe>, origin: Origin, cx: &mut App) -> Result<u64, String> {
    let mut inputs = Inputs::read(cx);
    inputs.base = base;
    let request = build_request(&inputs, g, origin)?;
    let keep_recipe = !matches!(g.source, None | Some(Source::Recipe)) && origin != Origin::Schedule;
    let recipe = request.recipe.clone();
    let id = wallgen(cx).update(cx, |w, cx| w.start(request, cx))?;
    if keep_recipe {
        update_settings(cx, |s| model::store_recipe(s, &recipe));
    }
    Ok(id)
}

fn unix(t: Option<SystemTime>) -> Value {
    t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(Value::Null, |d| json!(d.as_secs()))
}

fn recipe_json(r: &Recipe) -> Value {
    json!({
        "style": r.style.id(),
        "seed": format!("{:016x}", r.seed),
        "light": r.light,
        "palette": r.palette,
        "params": r.specs().map(|s| (s.key.to_string(), json!(r.param(s.key)))).collect::<serde_json::Map<_, _>>(),
        "toml": r.to_toml(),
    })
}

fn spec_json(s: &mandara_wallgen::ParamSpec) -> Value {
    json!({ "key": s.key, "label": tr(s.label), "min": s.min, "max": s.max, "step": s.step, "default": s.default, "unit": s.unit })
}

/// `wallgen styles`: every style with its parameters, and the common ones.
pub fn styles_json() -> Value {
    let styles: Vec<Value> = Style::ALL
        .iter()
        .map(|s| json!({ "id": s.id(), "label": tr(s.label()), "description": tr(s.description()), "params": s.params().iter().map(spec_json).collect::<Vec<_>>() }))
        .collect();
    json!({ "styles": styles, "common_params": COMMON_PARAMS.iter().map(spec_json).collect::<Vec<_>>() })
}

/// `wallgen palettes`.
pub fn palettes_json() -> Value {
    let list: Vec<Value> = mandara_wallgen::palettes().iter().map(|p| json!({ "id": p.id, "label": tr(p.label), "colors": p.colors })).collect();
    json!({ "palettes": list, "also": ["theme", "#rrggbb,#rrggbb,…"] })
}

/// `wallgen list`: the generated files in `dir`, newest first.
pub fn list_json(dir: &Path, limit: usize) -> Value {
    let items: Vec<Value> = model::library(dir, limit)
        .into_iter()
        .map(|e| json!({ "path": e.path, "kind": if e.video { "video" } else { "image" }, "bytes": e.bytes, "modified": unix(Some(e.modified)), "style": e.recipe.style.id(), "seed": format!("{:016x}", e.recipe.seed), "palette": tr(e.recipe.palette.label()) }))
        .collect();
    json!({ "directory": dir, "count": items.len(), "items": items })
}

/// `wallgen remix`: the recipe of a generated file.
pub fn remix_recipe(path: &Path) -> Result<Recipe, String> {
    mandara_wallgen::recipe_for(path).ok_or_else(|| tf("{path} wasn't made by the wallpaper generator", &[("path", &path.display())]))
}

fn status_json(cx: &mut App) -> Value {
    let s = settings(cx);
    let active = enabled(cx);
    let w = wallgen(cx);
    let w = w.read(cx);
    let mut v = w.status().json();
    let (on, next, last) = w.schedule_status(&s, active);
    v["schedule"] = json!({
        "enabled": on,
        "every_minutes": s.schedule.every_minutes,
        "mode": s.schedule.mode.id(),
        "kind": if s.schedule.kind == WallgenKind::Video { "video" } else { "still" },
        "apply": s.schedule.apply,
        "keep": s.schedule.keep,
        "next_run": unix(next),
        "last_run": unix(last),
        "timer": w.timer_armed(),
    });
    v
}

/// Parses `wallgen schedule`'s flags into the settings.
fn schedule_flags(s: &mut WallgenSettings, words: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let mut next = || {
            i += 1;
            words.get(i).cloned().ok_or_else(|| tf("{flag} needs a value", &[("flag", &w)]))
        };
        match w {
            "--every" => {
                let v = next()?;
                s.schedule.every_minutes = schedule::parse_every(&v).ok_or_else(|| tf("\"{value}\" isn't a duration like 30m, 2h or 1h30m", &[("value", &v)]))?;
            }
            "--mode" => {
                let v = next()?;
                s.schedule.mode = ScheduleMode::ALL.into_iter().find(|m| m.id() == v).ok_or_else(|| tf("Unknown mode {mode}: use random, variation or reseed", &[("mode", &v)]))?;
            }
            "--keep" => {
                let v = next()?;
                s.schedule.keep = v.parse().map_err(|_| tf("{flag} needs a number, got {value}", &[("flag", &w), ("value", &v)]))?;
            }
            "--video" => s.schedule.kind = WallgenKind::Video,
            "--still" => s.schedule.kind = WallgenKind::Still,
            "--apply" => s.schedule.apply = true,
            "--no-apply" => s.schedule.apply = false,
            other => return Err(tf("Unknown option {flag}", &[("flag", &other)])),
        }
        i += 1;
    }
    Ok(())
}

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("Wallpaper generator"), tk("Procedural wallpapers and seamless video loops, made on your machine in any size"), None).opt_in(),
        |_, _, _| mandara_plugins::PluginInstance::empty(),
    );
    r.command(
        ID,
        CommandSpec::new_output("wallgen generate", tk("Generate a wallpaper (the current recipe, random, a variation or a style) and save it; JSON with the path, size and time"), |a, cx| {
            let g = cli::parse_generate(&a.0)?;
            let id = generate(&g, None, Origin::Manual, cx)?;
            if g.detach {
                return Ok(Output::Value(json!({ "status": "started", "id": id })));
            }
            let rx = wallgen(cx).update(cx, |w, cx| w.wait(id, cx));
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::optional("options", ArgKind::String, tk("[recipe|random|variation|reseed|<style>] [--size ID|WxH] [--format lossless|webp|png|jpeg] [--palette ID|theme|#hex,…] [--seed N] [--light|--dark] [--apply|--no-apply] [--video [--seconds N] [--fps N]] [--detach]")))
        .example("wallgen generate")
        .example("wallgen generate random --apply")
        .example("wallgen generate aurora --size 8k --palette nord")
        .example("wallgen generate --video --seconds 8 --fps 24 --detach"),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen styles", tk("The generator's styles with their parameters"), |_, _| Ok(styles_json())),
    );
    r.command(ID, CommandSpec::new("wallgen palettes", tk("The generator's curated palettes"), |_, _| Ok(palettes_json())));
    r.command(
        ID,
        CommandSpec::new("wallgen recipe show", tk("The current recipe (style, seed, palette and every parameter)"), |_, cx| {
            let dark = mandara_core::theme_entity(cx).read(cx).target().is_dark;
            Ok(recipe_json(&model::recipe(&settings(cx), dark)))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen recipe set", tk("Change the current recipe: key=value pairs (style, seed, palette, light or any parameter) or a whole TOML recipe"), |a, cx| {
            let dark = mandara_core::theme_entity(cx).read(cx).target().is_dark;
            let next = cli::parse_recipe_set(&model::recipe(&settings(cx), dark), &a.0)?;
            update_settings(cx, |s| model::store_recipe(s, &next));
            Ok(recipe_json(&next))
        })
        .arg(ArgSpec::required("assignments", ArgKind::String, tk("key=value pairs, or a TOML recipe")))
        .example("wallgen recipe set style=aurora palette=nord vignette=40")
        .example("wallgen recipe set seed=0x2a"),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen recipe random", tk("Make the current recipe a random one (of one style when given)"), |a, cx| {
            let style = match a.get(0) {
                Some(id) => Some(Style::from_id(id).ok_or_else(|| tf("Unknown style {style}; see `wallgen styles`", &[("style", &id)]))?),
                None => None,
            };
            let inputs = Inputs::read(cx);
            let s = &inputs.settings;
            let lock = s.seed_locked.then(|| model::recipe(s, inputs.theme_dark).seed);
            let next = model::random_recipe(style, inputs.seed, s.tone.is_light(inputs.theme_dark), lock);
            update_settings(cx, |s| model::store_recipe(s, &next));
            Ok(recipe_json(&next))
        })
        .arg(ArgSpec::optional("style", ArgKind::String, tk("a style id from `wallgen styles`")))
        .example("wallgen recipe random dunes"),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen recipe variation", tk("Nudge the current recipe into a variation (amount 0–1, default from the settings)"), |a, cx| {
            let inputs = Inputs::read(cx);
            let s = &inputs.settings;
            let amount = match a.get(0) {
                Some(v) => v.parse::<f32>().map_err(|_| tf("{flag} needs a number, got {value}", &[("flag", &"amount"), ("value", &v)]))?,
                None => s.variation,
            };
            let next = model::variation(&model::recipe(s, inputs.theme_dark), amount, inputs.seed, s.seed_locked);
            update_settings(cx, |s| model::store_recipe(s, &next));
            Ok(recipe_json(&next))
        })
        .arg(ArgSpec::optional("amount", ArgKind::Number, tk("how far to stray, 0–1")))
        .example("wallgen recipe variation 0.5"),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen remix", tk("Load a generated wallpaper's recipe as the current one"), |a, cx| {
            let path = mandara_config::paths::expand_tilde(&a.rest(0));
            let recipe = remix_recipe(&path)?;
            update_settings(cx, |s| model::store_recipe(s, &recipe));
            Ok(json!({ "path": path, "recipe": recipe_json(&recipe) }))
        })
        .arg(ArgSpec::required("path", ArgKind::String, tk("a generated wallpaper")))
        .example("wallgen remix ~/Pictures/Wallpapers/Generated/aurora-1a2b3c-3840x2160.webp"),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen schedule", tk("Turn the opt-in schedule on or off, or show it; flags: --every 6h, --mode random|variation|reseed, --keep N, --video|--still, --apply|--no-apply"), |a, cx| {
            let action = a.str(0)?.to_string();
            let rest: Vec<String> = a.0.iter().skip(1).cloned().collect();
            if action != "status" || !rest.is_empty() {
                let mut next = settings(cx);
                schedule_flags(&mut next, &rest)?;
                match action.as_str() {
                    "on" => next.schedule.enabled = true,
                    "off" => next.schedule.enabled = false,
                    _ => {}
                }
                let next = next.sanitized();
                update_settings(cx, move |s| s.schedule = next.schedule);
            }
            Ok(status_json(cx)["schedule"].clone())
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("what to do")).choices(&["on", "off", "status"]))
        .arg(ArgSpec::optional("flags", ArgKind::String, tk("--every 6h, --mode, --keep N, --video, --apply")))
        .example("wallgen schedule on --every 6h")
        .example("wallgen schedule on --every 1d --mode variation --keep 20")
        .example("wallgen schedule status"),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen cancel", tk("Stop the wallpaper being generated"), |_, cx| {
            let cancelled = existing(cx).is_some_and(|w| w.update(cx, |w, cx| w.cancel(cx)));
            Ok(json!({ "cancelled": cancelled }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen status", tk("The running job (progress, time), the last result and the schedule"), |_, cx| Ok(status_json(cx))),
    );
    r.command(
        ID,
        CommandSpec::new("wallgen list", tk("Generated wallpapers in the output folder, newest first"), |a, cx| {
            let limit = match a.get(0) {
                Some(n) => n.parse().map_err(|_| t("Expected a number"))?,
                None => LIST_DEFAULT,
            };
            let dir = settings(cx).output_dir(&crate::wallpaper::directory(cx));
            Ok(list_json(&dir, limit))
        })
        .arg(ArgSpec::optional("limit", ArgKind::Integer, tk("how many (default 50)"))),
    );
}

/// The picker's Generate: a variation of `base` (the focused generated
/// wallpaper) or a random recipe.
pub fn generate_from_picker(base: Option<Recipe>, cx: &mut App) -> Result<u64, String> {
    let source = if base.is_some() { Source::Variation } else { Source::Random(None) };
    generate(&Generate { source: Some(source), ..Generate::default() }, base, Origin::Picker, cx)
}

/// Opens Studio's generator on `path`'s recipe (the picker's Remix).
pub fn remix_in_studio(path: &Path, cx: &mut App) -> Result<(), String> {
    let recipe = remix_recipe(path)?;
    update_settings(cx, |s| model::store_recipe(s, &recipe));
    crate::state::open_studio_at(Some("wallpaper".into()), cx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::wallgen::{WallgenFormat, WallgenTone};
    use mandara_wallgen::{Format, Resolution};

    fn inputs(dir: &Path) -> Inputs {
        Inputs {
            settings: WallgenSettings::default(),
            theme_dark: true,
            theme_palette: PaletteSpec::Theme(vec!["#101010".into(), "#f0f0f0".into()]),
            wallpaper_dir: dir.to_path_buf(),
            display: Some((1920, 1200)),
            base: None,
            seed: 99,
        }
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn requests_follow_settings_and_flags() {
        let dir = Path::new("/w");
        let i = inputs(dir);
        let req = build_request(&i, &Generate::default(), Origin::Manual).unwrap();
        assert_eq!(req.recipe, model::recipe(&i.settings, true), "no source: the current recipe");
        assert!(req.apply && req.keep.is_none());
        let jobs::Output::Still(o) = &req.output else { panic!("still expected") };
        assert_eq!((o.resolution.size(), o.format, o.dir.clone()), ((3456, 2160), Format::Lossless, PathBuf::from("/w/Generated")));

        let g = cli::parse_generate(&words("aurora --size 1080p --format jpeg --palette theme --seed 7 --light --no-apply")).unwrap();
        let req = build_request(&i, &g, Origin::Manual).unwrap();
        assert_eq!((req.recipe.style, req.recipe.seed, req.recipe.light, req.apply), (Style::Aurora, 7, true, false));
        assert_eq!(req.recipe.palette, i.theme_palette);
        let jobs::Output::Still(o) = &req.output else { panic!() };
        assert_eq!((o.resolution, o.format), (Resolution::Fhd1080, Format::Jpeg { quality: mandara_config::wallgen::defaults::JPEG_QUALITY }));

        let g = cli::parse_generate(&words("--video --seconds 3 --fps 12 --size 1440p")).unwrap();
        let req = build_request(&i, &g, Origin::Manual).unwrap();
        let jobs::Output::Video { options, dir } = &req.output else { panic!("video expected") };
        assert_eq!((options.seconds, options.fps, options.resolution, dir.clone()), (3.0, 12, Resolution::Qhd1440, PathBuf::from("/w/Generated")));
    }

    #[test]
    fn tone_lock_and_variation_base() {
        let mut i = inputs(Path::new("/w"));
        i.settings.tone = WallgenTone::Light;
        i.settings.seed_locked = true;
        let current = model::recipe(&i.settings, true);
        let req = build_request(&i, &Generate { source: Some(Source::Random(None)), ..Default::default() }, Origin::Manual).unwrap();
        assert!(req.recipe.light, "the tone overrides the dark theme");
        assert_eq!(req.recipe.seed, current.seed, "a locked seed survives a random draw");
        let base = Recipe::random(Some(Style::Dunes), 4);
        i.base = Some(base.clone());
        let req = build_request(&i, &Generate { source: Some(Source::Variation), ..Default::default() }, Origin::Picker).unwrap();
        assert_eq!(req.recipe.style, Style::Dunes, "variations build on the picker's wallpaper");
        assert_eq!(req.recipe.seed, base.seed);
        let req = build_request(&i, &Generate { source: Some(Source::Reseed), ..Default::default() }, Origin::Manual).unwrap();
        assert_eq!(req.recipe.seed, 99);
        assert_eq!(req.recipe.params, current.params);
    }

    #[test]
    fn scheduled_runs_prune_and_follow_their_mode() {
        let mut i = inputs(Path::new("/w"));
        i.settings.schedule.mode = ScheduleMode::Variation;
        i.settings.schedule.kind = WallgenKind::Video;
        i.settings.schedule.keep = 5;
        i.settings.schedule.apply = false;
        i.settings.format = WallgenFormat::Png;
        let g = scheduled(&i.settings);
        assert_eq!((g.source.clone(), g.video), (Some(Source::Variation), true));
        let req = build_request(&i, &g, Origin::Schedule).unwrap();
        assert_eq!((req.keep, req.apply, req.is_video()), (Some(5), false, true));
        i.settings.schedule.keep = 0;
        assert_eq!(build_request(&i, &scheduled(&i.settings), Origin::Schedule).unwrap().keep, None, "0 keeps everything");
    }

    #[test]
    fn schedule_flags_parse() {
        let mut s = WallgenSettings::default();
        schedule_flags(&mut s, &words("--every 1d --mode reseed --keep 9 --video --no-apply")).unwrap();
        assert_eq!((s.schedule.every_minutes, s.schedule.mode, s.schedule.keep, s.schedule.kind, s.schedule.apply), (1440, ScheduleMode::Reseed, 9, WallgenKind::Video, false));
        assert!(schedule_flags(&mut s, &words("--every soon")).is_err());
        assert!(schedule_flags(&mut s, &words("--mode chaos")).is_err());
        assert!(schedule_flags(&mut s, &words("--every")).is_err());
        assert!(schedule_flags(&mut s, &words("--bogus")).is_err());
    }
}
