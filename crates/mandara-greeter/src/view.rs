//! The login screen as a reusable GPUI view.
//!
//! One view renders the greeter from a [`GreeterConfig`] at any size:
//!
//! * **live** — full screen, talking to greetd (or the demo's fake daemon);
//! * **preview** — embedded in Mandara Studio at a scaled size with mock
//!   state; elements can be selected and dragged (snapping to the 9-point
//!   anchors), and edits are emitted as [`PreviewEvent`]s.
//!
//! Every element is placed by the config's layout (anchor + offset + scale
//! + z-order) and animated with springs: a staggered entrance (the avatar
//! pops with overshoot, the password pill unfolds), an idle state with a big
//! clock that wakes on any key or pointer move, rolling clock digits, a
//! breathing ring around the avatar, keystroke dots that pop in with a
//! spring and ripple the pill, a shake + coral flash on a wrong password, a
//! cinematic push-in on success, and pointer parallax on the backdrop. With
//! reduced motion every spring snaps and nothing drifts.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, BoxShadow, ClickEvent, Context, EventEmitter, FocusHandle, FontWeight, Hsla, ImageSource, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, Render,
    RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, canvas, div, img, point, prelude::FluentBuilder as _,
    px, svg,
};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::fonts::{Fonts, tabular_nums};

use crate::auth::{AuthCommand, AuthEvent, AuthWorker};
use crate::background::{Background, BackgroundOptions};
use crate::config::{
    Anchor, AvatarShape, AvatarSource, BackgroundMode, Element, FieldStyle, GreeterConfig, UserList, VideoMode, apply_playlist,
    list_images,
};
use crate::greetd::{Notes, Step, wipe};
use crate::i18n::{tl, tlf};
use crate::power::PowerAction;
use crate::sessions::{GreeterState, Session, SessionKind, list_sessions, state_path};
use crate::users::{User, list_users, load_avatar};

/// The caret stays solid this long after a key press, then blinks.
const CARET_HOLD: Duration = Duration::from_millis(600);
/// A key press this long ago counts as no recent typing (the caret blinks from the start).
const LONG_AGO: Duration = Duration::from_secs(10);
/// Demo "typed" simulation: the submit ripple follows the last of the eight dots.
const DEMO_RIPPLE_AFTER: Duration = Duration::from_millis(630);
/// Demo simulations reset after this long (typed / error / success) so the preview loops.
const DEMO_TYPED_RESET: Duration = Duration::from_millis(3200);
/// See [`DEMO_TYPED_RESET`].
const DEMO_ERROR_RESET: Duration = Duration::from_millis(3000);
/// See [`DEMO_TYPED_RESET`].
const DEMO_SUCCESS_RESET: Duration = Duration::from_millis(1800);
/// Two Escape presses this close together count as a double press (input gesture, not a look setting).
const DOUBLE_ESCAPE: Duration = Duration::from_millis(700);
/// The success state shows this long before the session starts (covers the greetd hand-off).
const SUCCESS_HOLD: Duration = Duration::from_millis(1000);
/// How long a greeter toast stays up (the shell's quick toast at its default).
const TOAST_FOR: Duration = Duration::from_millis(2600);

/// How often the demo logs video and frame-pacing statistics.
const STATS_EVERY: Duration = Duration::from_secs(3);
/// Frames rendered (invisibly) before the entrance starts, so fonts are
/// loaded, glyphs rasterized and images uploaded before anything moves.
const WARMUP_FRAMES: u32 = 2;
/// Longest the entrance waits for the backdrop's first picture (a still
/// decoding or a clip spinning up); the aurora floor shows meanwhile.
const BACKDROP_WAIT: Duration = Duration::from_millis(700);
/// Delay between the warm-up and the first element of the entrance, s.
const ENTRANCE_LEAD_S: f32 = 0.12;
/// Milliseconds per second (stagger is configured in ms).
const MS_PER_S: f32 = 1000.0;
/// Animated text and icon sizes snap to this many steps per logical pixel:
/// every distinct size is rasterized (and cached) separately, so a size
/// that changes continuously would rasterize new glyphs every frame.
const SIZE_STEPS_PER_PX: f32 = 1.0;
/// Glyphs the clock can show, rasterized during the warm-up.
const CLOCK_GLYPHS: &str = "0123456789:";
/// Initial value of the avatar pop spring when the entrance is enabled.
const POP_FROM: f32 = 0.55;

/// Clock growth while idle and on success (fractions of its size).
const CLOCK_IDLE_GROWTH: f32 = 0.22;
const CLOCK_SUCCESS_GROWTH: f32 = 0.07;
/// Spring that rolls a changed clock digit in.
const ROLL_SPRING: SpringConfig = SpringConfig::new(180.0, 20.0, 1.0);
/// The roll may overshoot this far before clamping.
const ROLL_OVERSHOOT_CAP: f32 = 1.05;
/// Clock digit drop shadow: offset (px per unit scale) and alpha.
const CLOCK_SHADOW_DY: f32 = 3.0;
const CLOCK_SHADOW_ALPHA: f32 = 0.22;
/// Avatar initials and fallback icon size, fractions of the avatar.
const INITIALS_SCALE: f32 = 0.38;
const USER_ICON_SCALE: f32 = 0.42;

/// Studio preview: how long background edits that rescan folders or reload
/// pictures (folders typed, the softening slider dragged) wait for the
/// edits to settle before the backdrop is rebuilt.
const BACKGROUND_REBUILD_DEBOUNCE: Duration = Duration::from_millis(300);
/// Studio preview: device-pixel ratio assumed before the first frame
/// reports the window's real one.
const PREVIEW_SCALE_GUESS: f32 = 1.5;

/// Size of the status line under the password field (px per unit scale).
const STATUS_TEXT_PX: f32 = 13.0;

/// The friendly wrong-password messages.
fn playful_lines() -> [String; 3] {
    [tl("Not quite — try again"), tl("Hmm, that wasn't it"), tl("Close, but not quite")]
}

/// Snaps an animated text / icon size (px) to the rasterization grid.
fn snap_size(size: f32) -> f32 {
    (size * SIZE_STEPS_PER_PX).round() / SIZE_STEPS_PER_PX
}

/// One character as a shared string without allocating for ASCII digits
/// and separators (the clock re-renders every frame).
fn glyph_str(ch: char) -> SharedString {
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    match ch {
        '0'..='9' => SharedString::new_static(DIGITS[ch as usize - '0' as usize]),
        ':' => SharedString::new_static(":"),
        ' ' => SharedString::new_static(" "),
        _ => SharedString::from(ch.to_string()),
    }
}

/// Which control has the keyboard.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Field {
    Username,
    Password,
    Session,
    User,
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    Busy,
    /// Session started at this instant; the push-in runs, then we exit.
    Success(Instant),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    None,
    Info(String),
    Error(String),
}

struct UserEntry {
    user: User,
    avatar: Option<Arc<RenderImage>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    Live { demo: bool },
    Preview,
}

/// Edits coming out of the Studio preview.
#[derive(Clone, Debug)]
pub enum PreviewEvent {
    /// The layout changed by dragging; the new config.
    Edited(GreeterConfig),
    Selected(Option<Element>),
}

/// What the preview should show (Studio's state buttons).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Simulate {
    Entrance,
    Typing,
    Error,
    Success,
}

struct Drag {
    element: Element,
    start: Point<Pixels>,
    bounds: Bounds<Pixels>,
    moved: bool,
}

/// The clock's rolling digits.
#[derive(Default)]
struct Roll {
    text: String,
    prev: String,
    at: Option<Instant>,
    /// Second and format the text was made for.
    second: i64,
    format: &'static str,
}

pub struct GreeterView {
    mode: Mode,
    config: GreeterConfig,
    palette: MandaraPalette,
    users: Vec<UserEntry>,
    user_ix: usize,
    manual: Option<String>,
    sessions: Vec<Session>,
    session_ix: usize,
    session_menu: bool,
    state: GreeterState,
    password: String,
    reveal: bool,
    prompt: Option<(bool, String)>,
    phase: Phase,
    status: Status,
    focus: FocusHandle,
    field: Field,
    auth: Option<AuthWorker>,
    background: Background,
    /// What the backdrop was built from (a change rebuilds it).
    bg_key: BackgroundSources,
    /// Preview: a debounced backdrop rebuild is due at this instant.
    bg_rebuild_at: Option<Instant>,
    clock: FrameClock,
    started: Instant,
    /// When the entrance began (`None` while warming up).
    entrance_at: Option<Instant>,
    /// Frames rendered while warming up, and when the first one was.
    warm_frames: u32,
    first_frame_at: Option<Instant>,
    /// Second of the last clock format (the text only changes per second).
    clock_second: i64,
    /// Formatted date and the (day, pattern, language) it was made for.
    date_text: Option<((chrono::NaiveDate, String, &'static str), SharedString)>,
    /// Keyboard layout label and the (configured, GPUI) ids it came from.
    kb_layout: Option<((String, String), SharedString)>,
    // Motion.
    enter: Vec<SpringValue<f32>>,
    pop: SpringValue<f32>,
    wake: SpringValue<f32>,
    awake: bool,
    last_activity: Instant,
    shake: SpringValue<f32>,
    flash_at: Option<Instant>,
    success: SpringValue<f32>,
    glow: SpringValue<f32>,
    dots: Vec<Instant>,
    ripple_at: Option<Instant>,
    pointer: (SpringValue<f32>, SpringValue<f32>),
    last_pointer: Option<Point<Pixels>>,
    roll: Roll,
    last_key: Instant,
    last_escape: Option<Instant>,
    toast: Option<(String, Instant)>,
    exit: bool,
    stats_at: Instant,
    /// Frame pacing (logged by the demo, or with `MANDARA_FRAME_STATS=1`).
    pacing: crate::pacing::Pacing,
    /// The same, from the start of the entrance (what is actually seen).
    pacing_visible: crate::pacing::Pacing,
    autoplay: Option<u8>,
    // Preview / editing.
    preview_size: (f32, f32),
    preview_idle: bool,
    selected: Option<Element>,
    drag: Option<Drag>,
    bounds: Rc<RefCell<HashMap<Element, Bounds<Pixels>>>>,
    origin: Rc<Cell<Point<Pixels>>>,
    snapped: Option<(Anchor, bool, bool)>,
    sim_reset: Option<Instant>,
}

impl EventEmitter<PreviewEvent> for GreeterView {}

/// The background settings a backdrop is built from: changing any of them
/// needs a new [`Background`] (new sources, decoder or softening). Timings
/// are applied in place ([`Background::retime`]) and the rest is drawn per
/// frame.
#[derive(Clone, Debug, PartialEq)]
struct BackgroundSources {
    mode: BackgroundMode,
    video: VideoMode,
    video_dirs: Vec<std::path::PathBuf>,
    image_dirs: Vec<std::path::PathBuf>,
    shuffle: bool,
    playlist: Vec<String>,
    blur: f32,
    /// The solid floor's color as resolved (it follows the theme when unset).
    solid: Option<Color>,
    reduced: bool,
}

impl BackgroundSources {
    fn of(cfg: &GreeterConfig) -> Self {
        let b = &cfg.background;
        Self {
            mode: b.mode,
            video: b.video,
            video_dirs: b.video_dirs.clone(),
            image_dirs: b.image_dirs.clone(),
            shuffle: b.shuffle,
            playlist: b.playlist.clone(),
            blur: b.blur,
            solid: (b.mode == BackgroundMode::Solid).then(|| Color::from_hex(b.solid.trim()).unwrap_or_else(|_| palette_for(cfg).surface_dim)),
            reduced: cfg.accessibility.reduced_motion,
        }
    }
}

/// What an edit means for the backdrop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackgroundChange {
    /// Same sources (timings at most).
    Same,
    /// A switch (kind, decoding, solid color, reduced motion): rebuild now.
    Rebuild,
    /// Folders, playlist, shuffle or softening: rebuild once edits settle
    /// (typing a path or dragging the softening slider would otherwise
    /// rescan folders and reload pictures on every step).
    RebuildSettled,
}

fn background_change(old: &BackgroundSources, new: &BackgroundSources) -> BackgroundChange {
    if old == new {
        BackgroundChange::Same
    } else if old.mode != new.mode || old.video != new.video || old.solid != new.solid || old.reduced != new.reduced {
        BackgroundChange::Rebuild
    } else {
        BackgroundChange::RebuildSettled
    }
}

/// The backdrop timings of a config (the other options at their defaults).
fn background_timing(cfg: &GreeterConfig) -> BackgroundOptions {
    let b = &cfg.background;
    BackgroundOptions {
        clip_seconds: b.clip_seconds,
        video_fade: Duration::from_millis(b.crossfade_ms as u64),
        still_fade: Duration::from_millis(b.still_fade_ms as u64),
        still_life: Duration::from_secs(b.still_seconds as u64),
        ken_burns: if cfg.accessibility.reduced_motion { 0.0 } else { b.ken_burns },
        rate: b.playback_rate,
        ..BackgroundOptions::default()
    }
}

/// The theme for a config: the preset's dark palette with the accent
/// override applied.
pub fn palette_for(cfg: &GreeterConfig) -> MandaraPalette {
    let mut p = mandara_theme::catalog::find(&cfg.theme.preset)
        .or_else(|| mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME))
        .map(|t| t.palette(true))
        .unwrap_or_else(|| mandara_theme::engine::default_palette(true));
    if let Ok(accent) = Color::from_hex(cfg.theme.accent.trim()) {
        p.primary = accent;
        p.primary_glow = accent.with_alpha(0.5);
        p.accent_gradient_start = accent;
        p.accent_gradient_end = accent.rotate_hue(25.0).scale_chroma(1.15);
    }
    p
}

fn spring(s: crate::config::Spring, reduced: bool) -> SpringConfig {
    if reduced { SpringConfig::new(600.0, 49.0, 1.0) } else { s.config() }
}

fn element_index(e: Element) -> usize {
    Element::ALL.iter().position(|x| *x == e).unwrap_or(0)
}

/// The keyboard layout to show: configured, GPUI's, or the system's.
fn keyboard_layout(cfg: &GreeterConfig, cx: &gpui::App) -> String {
    if !cfg.keyboard.layout.trim().is_empty() {
        return cfg.keyboard.layout.trim().to_uppercase();
    }
    let id = cx.keyboard_layout().id().to_string();
    let from_gpui = id.rsplit(['.', ':', '/']).next().unwrap_or("").to_string();
    if !from_gpui.is_empty() && from_gpui.len() <= 6 {
        return from_gpui.to_uppercase();
    }
    let env = std::env::var("XKB_DEFAULT_LAYOUT").ok();
    let x11 = std::fs::read_to_string("/etc/X11/xorg.conf.d/00-keyboard.conf").ok().and_then(|t| {
        t.lines().find(|l| l.contains("XkbLayout")).and_then(|l| l.split('"').nth(3).map(str::to_string))
    });
    let vconsole = std::fs::read_to_string("/etc/vconsole.conf").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("KEYMAP=").map(|v| v.trim_matches('"').to_string())));
    env.or(x11).or(vconsole).map(|l| l.split(',').next().unwrap_or("").trim().to_uppercase()).filter(|l| !l.is_empty()).unwrap_or_else(|| "US".into())
}

impl GreeterView {
    /// The real (or demo) login screen.
    pub fn new_live(demo: bool, autoplay: bool, config: GreeterConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scale = window.scale_factor();
        let vp = window.viewport_size();
        let target = ((f32::from(vp.width) * scale) as u32, (f32::from(vp.height) * scale) as u32);
        let mut this = Self::build(Mode::Live { demo }, config, target, cx);
        match AuthWorker::spawn(demo) {
            Ok((worker, events)) => {
                cx.spawn(async move |this, cx| {
                    while let Ok(ev) = events.recv().await {
                        if this.update(cx, |this, cx| this.on_auth(ev, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                this.auth = Some(worker);
            }
            Err(e) => this.status = Status::Error(tlf("Cannot reach greetd: {error}", &[("error", &e)])),
        }
        this.focus.focus(window, cx);
        this.autoplay = (demo && autoplay).then_some(0);
        this
    }

    /// An embeddable preview (Studio) of `w × h` logical pixels.
    pub fn new_preview(config: GreeterConfig, size: (f32, f32), cx: &mut Context<Self>) -> Self {
        let target = ((size.0 * PREVIEW_SCALE_GUESS) as u32, (size.1 * PREVIEW_SCALE_GUESS) as u32);
        let mut this = Self::build(Mode::Preview, config, target, cx);
        this.preview_size = size;
        this
    }

    fn build(mode: Mode, mut config: GreeterConfig, target: (u32, u32), cx: &mut Context<Self>) -> Self {
        config.normalize();
        let demo = !matches!(mode, Mode::Live { demo: false });
        let lang = mandara_i18n::language();
        // The demo and the preview show only you; the real greeter lists
        // every login user (minus hidden ones).
        let all: Vec<User> = if demo { crate::users::current_user().into_iter().collect() } else { list_users() };
        let mut users: Vec<User> = all.into_iter().filter(|u| !config.user.hidden.contains(&u.name)).collect();
        let state = GreeterState::load(&state_path(demo));
        if config.user.list == UserList::LastUser
            && let Some(last) = state.last_user.clone()
        {
            users.retain(|u| u.name == last);
        }
        if config.user.list == UserList::Manual {
            users.clear();
        }
        let users: Vec<UserEntry> = users
            .into_iter()
            .map(|u| UserEntry { avatar: if config.user.avatar_source == AvatarSource::Auto { load_avatar(&u, 256) } else { None }, user: u })
            .collect();
        let mut sessions = list_sessions(lang.posix());
        if !config.session.x11 {
            sessions.retain(|s| s.kind == SessionKind::Wayland);
        }
        let user_ix = state.last_user.as_ref().and_then(|n| users.iter().position(|u| &u.user.name == n)).unwrap_or(0);
        let default_session = sessions.iter().position(|s| s.id == config.session.default).unwrap_or(0);
        let session_ix = users.get(user_ix).and_then(|u| state.sessions.get(&u.user.name)).and_then(|id| sessions.iter().position(|s| &s.id == id)).unwrap_or(default_session);
        let manual = users.is_empty().then(String::new);
        let background = Self::make_background(&config, demo, mode == Mode::Preview, target);
        tracing::info!("greeter: {} users, {} sessions", users.len(), sessions.len());
        let reduced = config.accessibility.reduced_motion;
        let m = &config.motion;
        let enter: Vec<SpringValue<f32>> = Element::ALL.iter().map(|_| SpringValue::new(if m.entrance { 0.0 } else { 1.0 }, spring(m.entrance_spring, reduced))).collect();
        let awake = !(m.start_idle && m.idle_seconds > 0) || mode == Mode::Preview;
        let mut wake = SpringValue::new(if awake { 1.0 } else { 0.0 }, spring(m.idle_spring, reduced));
        wake.set_target(if awake { 1.0 } else { 0.0 });
        let p = palette_for(&config);
        let fast = SpringConfig::new(170.0, 20.0, 1.0);
        let mut this = Self {
            mode,
            palette: p,
            users,
            user_ix,
            field: if manual.is_some() { Field::Username } else { Field::Password },
            manual,
            sessions,
            session_ix,
            session_menu: false,
            state,
            password: String::new(),
            reveal: false,
            prompt: None,
            phase: Phase::Idle,
            status: Status::None,
            focus: cx.focus_handle(),
            auth: None,
            background,
            bg_key: BackgroundSources::of(&config),
            bg_rebuild_at: None,
            clock: FrameClock::new(),
            started: Instant::now(),
            entrance_at: None,
            warm_frames: 0,
            first_frame_at: None,
            clock_second: i64::MIN,
            date_text: None,
            kb_layout: None,
            enter,
            pop: SpringValue::new(if m.entrance { POP_FROM } else { 1.0 }, spring(m.pop_spring, reduced)),
            wake,
            awake,
            last_activity: Instant::now(),
            shake: SpringValue::new(0.0, spring(m.shake_spring, reduced)),
            flash_at: None,
            success: SpringValue::new(0.0, spring(m.success_spring, reduced)),
            glow: SpringValue::new(0.0, fast),
            dots: Vec::new(),
            ripple_at: None,
            pointer: (SpringValue::new(0.0, SpringConfig::new(40.0, 12.0, 1.0)), SpringValue::new(0.0, SpringConfig::new(40.0, 12.0, 1.0))),
            last_pointer: None,
            roll: Roll::default(),
            last_key: Instant::now() - LONG_AGO,
            last_escape: None,
            toast: None,
            exit: false,
            stats_at: Instant::now(),
            pacing: crate::pacing::Pacing::new(),
            pacing_visible: crate::pacing::Pacing::new(),
            autoplay: None,
            preview_size: (960.0, 540.0),
            preview_idle: false,
            selected: None,
            drag: None,
            bounds: Rc::new(RefCell::new(HashMap::new())),
            origin: Rc::new(Cell::new(point(px(0.0), px(0.0)))),
            snapped: None,
            sim_reset: None,
            config,
        };
        this.started = Instant::now();
        this
    }

    /// `preview`: the small Studio preview decodes clips in lite mode (the
    /// skipped in-loop filters are invisible at that size and it costs far
    /// less CPU).
    fn make_background(config: &GreeterConfig, demo: bool, preview: bool, target: (u32, u32)) -> Background {
        let b = &config.background;
        let mode = b.mode;
        let mut clips = if b.video == VideoMode::Off || matches!(mode, BackgroundMode::Stills | BackgroundMode::Aurora | BackgroundMode::Solid) {
            Vec::new()
        } else {
            crate::media::list_clips(&config.video_dirs(demo))
        };
        let mut stills = if matches!(mode, BackgroundMode::Video | BackgroundMode::Aurora | BackgroundMode::Solid) { Vec::new() } else { list_images(&config.image_dirs(demo)) };
        apply_playlist(&mut clips, &b.playlist);
        apply_playlist(&mut stills, &b.playlist);
        tracing::info!("greeter background: {:?}, {} clips, {} stills", mode, clips.len(), stills.len());
        let solid = (mode == BackgroundMode::Solid).then(|| Color::from_hex(b.solid.trim()).unwrap_or_else(|_| palette_for(config).surface_dim));
        Background::new(
            BackgroundOptions {
                clips,
                stills,
                target,
                lite: b.video == VideoMode::Lite || preview,
                video_enabled: b.video != VideoMode::Off,
                blur: b.blur,
                solid,
                still_motion: config.accessibility.reduced_motion,
                ..background_timing(config)
            },
            b.shuffle && b.playlist.is_empty(),
        )
    }

    /// Rebuilds the backdrop from the current config (preview).
    fn rebuild_background(&mut self) {
        let demo = !matches!(self.mode, Mode::Live { demo: false });
        self.background = Self::make_background(&self.config, demo, self.is_preview(), self.background.target());
        self.bg_key = BackgroundSources::of(&self.config);
        self.bg_rebuild_at = None;
    }

    // ── Studio API ───────────────────────────────────────────────────────

    pub fn config(&self) -> &GreeterConfig {
        &self.config
    }

    /// Applies an edited config (live preview).
    pub fn set_config(&mut self, mut config: GreeterConfig, cx: &mut Context<Self>) {
        config.normalize();
        let reduced = config.accessibility.reduced_motion;
        // Timings apply in place; only new sources rebuild the backdrop —
        // at once for a switch, after the edits settle for typing / sliders.
        let change = background_change(&self.bg_key, &BackgroundSources::of(&config));
        self.background.retime(&background_timing(&config), Instant::now());
        let m = &config.motion;
        for s in &mut self.enter {
            s.set_config(spring(m.entrance_spring, reduced));
        }
        self.pop.set_config(spring(m.pop_spring, reduced));
        self.shake.set_config(spring(m.shake_spring, reduced));
        self.success.set_config(spring(m.success_spring, reduced));
        self.wake.set_config(spring(m.idle_spring, reduced));
        self.palette = palette_for(&config);
        self.config = config;
        match change {
            BackgroundChange::Same => self.bg_rebuild_at = None,
            BackgroundChange::Rebuild => self.rebuild_background(),
            BackgroundChange::RebuildSettled => self.bg_rebuild_at = Some(Instant::now() + BACKGROUND_REBUILD_DEBOUNCE),
        }
        cx.notify();
    }

    pub fn set_preview_size(&mut self, w: f32, h: f32) {
        self.preview_size = (w.max(64.0), h.max(36.0));
    }

    pub fn set_preview_idle(&mut self, idle: bool, cx: &mut Context<Self>) {
        self.preview_idle = idle;
        cx.notify();
    }

    pub fn select(&mut self, element: Option<Element>, cx: &mut Context<Self>) {
        self.selected = element;
        cx.notify();
    }

    /// Plays a state in the preview.
    pub fn simulate(&mut self, what: Simulate, cx: &mut Context<Self>) {
        let now = Instant::now();
        match what {
            Simulate::Entrance => {
                for s in &mut self.enter {
                    s.snap_to(0.0);
                }
                self.pop.snap_to(POP_FROM);
                self.started = now;
                self.entrance_at = Some(now);
            }
            Simulate::Typing => {
                self.password.clear();
                self.dots.clear();
                for i in 0..8u64 {
                    self.password.push('x');
                    self.dots.push(now + Duration::from_millis(i * 90));
                }
                self.ripple_at = Some(now + DEMO_RIPPLE_AFTER);
                self.last_key = now;
                self.sim_reset = Some(now + DEMO_TYPED_RESET);
            }
            Simulate::Error => {
                self.password.clear();
                self.dots.clear();
                let msg = self.playful_error();
                self.fail(msg, cx);
                self.sim_reset = Some(now + DEMO_ERROR_RESET);
            }
            Simulate::Success => {
                self.phase = Phase::Success(now);
                self.success.set_target(1.0);
                self.sim_reset = Some(now + DEMO_SUCCESS_RESET);
            }
        }
        cx.notify();
    }

    fn is_preview(&self) -> bool {
        self.mode == Mode::Preview
    }

    fn demo(&self) -> bool {
        matches!(self.mode, Mode::Live { demo: true })
    }

    // ── Login flow ───────────────────────────────────────────────────────

    fn username(&self) -> Option<String> {
        match &self.manual {
            Some(name) => Some(name.trim().to_string()).filter(|n| !n.is_empty()),
            None => self.users.get(self.user_ix).map(|u| u.user.name.clone()),
        }
    }

    fn activity(&mut self) {
        self.last_activity = Instant::now();
        self.awake = true;
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.phase != Phase::Idle || self.is_preview() {
            return;
        }
        let Some(auth) = &self.auth else { return };
        if self.field == Field::Username {
            if self.username().is_some() {
                self.field = Field::Password;
            }
            cx.notify();
            return;
        }
        let Some(user) = self.username() else {
            self.field = Field::Username;
            return;
        };
        if self.prompt.is_some() {
            let answer = std::mem::take(&mut self.password);
            auth.send(AuthCommand::Respond(Some(answer)));
        } else {
            if self.password.is_empty() {
                self.fail(tl("Enter your password"), cx);
                return;
            }
            let secret = std::mem::take(&mut self.password);
            auth.send(AuthCommand::Begin { user, secret });
        }
        self.dots.clear();
        self.phase = Phase::Busy;
        self.status = Status::None;
        cx.notify();
    }

    fn playful_error(&self) -> String {
        let lines = playful_lines();
        let i = (Instant::now().duration_since(self.started).as_millis() / 7) as usize % lines.len();
        lines[i].clone()
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        wipe(&mut self.password);
        self.dots.clear();
        self.prompt = None;
        self.phase = Phase::Idle;
        self.status = Status::Error(message);
        self.flash_at = Some(Instant::now());
        if self.config.motion.shake && !self.config.accessibility.reduced_motion {
            self.shake.add_velocity([-2400.0 * self.config.motion.shake_strength, 0.0, 0.0, 0.0]);
        }
        cx.notify();
    }

    fn on_auth(&mut self, ev: AuthEvent, cx: &mut Context<Self>) {
        let notes_status = |notes: &Notes| -> Option<Status> {
            notes.errors.last().map(|e| Status::Error(e.clone())).or_else(|| notes.info.last().map(|i| Status::Info(i.clone())))
        };
        match ev {
            AuthEvent::Step(Step::Authenticated, notes) => {
                if let Some(s) = notes_status(&notes) {
                    self.status = s;
                }
                let (cmd, env) = match self.sessions.get(self.session_ix) {
                    Some(s) => (s.command(), s.env()),
                    None => (vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()), "-l".into()], Vec::new()),
                };
                if let Some(auth) = &self.auth {
                    auth.send(AuthCommand::Start { cmd, env });
                }
            }
            AuthEvent::Step(Step::Prompt { secret, message }, notes) => {
                self.phase = Phase::Idle;
                self.status = notes_status(&notes).unwrap_or(Status::None);
                wipe(&mut self.password);
                self.dots.clear();
                self.prompt = Some((secret, message));
                self.field = Field::Password;
            }
            AuthEvent::Step(Step::Denied(_), notes) => {
                let msg = notes.errors.last().cloned().unwrap_or_else(|| self.playful_error());
                self.fail(msg, cx);
            }
            AuthEvent::Step(Step::Failed(desc), _) => self.fail(desc, cx),
            AuthEvent::Started(Ok(())) => {
                if let Some(user) = self.username() {
                    self.state.last_user = Some(user.clone());
                    if let Some(s) = self.sessions.get(self.session_ix) {
                        self.state.sessions.insert(user, s.id.clone());
                    }
                    if let Err(e) = self.state.save(&state_path(self.demo())) {
                        tracing::warn!("greeter state: {e}");
                    }
                }
                self.phase = Phase::Success(Instant::now());
                self.success.set_target(1.0);
                self.status = Status::None;
            }
            AuthEvent::Started(Err(e)) => self.fail(tlf("Could not start the session: {error}", &[("error", &e)]), cx),
        }
        cx.notify();
    }

    fn select_user(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.phase != Phase::Idle || self.is_preview() {
            return;
        }
        if let Some(auth) = &self.auth {
            auth.send(AuthCommand::Cancel);
        }
        wipe(&mut self.password);
        self.dots.clear();
        self.prompt = None;
        self.status = Status::None;
        if ix >= self.users.len() {
            self.manual = Some(String::new());
            self.field = Field::Username;
        } else {
            self.manual = None;
            self.user_ix = ix;
            let def = self.sessions.iter().position(|s| s.id == self.config.session.default).unwrap_or(0);
            self.session_ix = self.state.sessions.get(&self.users[ix].user.name).and_then(|id| self.sessions.iter().position(|s| &s.id == id)).unwrap_or(def);
            self.field = Field::Password;
        }
        self.pop.snap_to(0.8);
        self.pop.set_target(1.0);
        cx.notify();
    }

    fn cycle_session(&mut self, delta: isize) {
        let n = self.sessions.len() as isize;
        if n > 0 {
            self.session_ix = ((self.session_ix as isize + delta).rem_euclid(n)) as usize;
        }
    }

    fn visible(&self, e: Element) -> bool {
        self.config.placement(e).visible
    }

    fn fields(&self) -> Vec<Field> {
        let mut f = Vec::new();
        if self.manual.is_some() {
            f.push(Field::Username);
        }
        f.push(Field::Password);
        if self.sessions.len() > 1 && self.visible(Element::Session) {
            f.push(Field::Session);
        }
        if self.visible(Element::Users) && (self.users.len() > 1 || (!self.demo() && self.config.user.list != UserList::Manual)) {
            f.push(Field::User);
        }
        f
    }

    fn power(&mut self, action: PowerAction, cx: &mut Context<Self>) {
        if self.is_preview() {
            return;
        }
        if self.demo() {
            self.toast = Some((tl("Power actions are disabled in the demo"), Instant::now()));
        } else if let Err(e) = action.run(false) {
            self.toast = Some((e, Instant::now()));
        }
        cx.notify();
    }

    fn type_char(&mut self, ch: &str) {
        let pw = self.field != Field::Username;
        let text = if pw { &mut self.password } else { self.manual.get_or_insert_with(String::new) };
        if text.len() < 512 {
            text.push_str(ch);
            if pw {
                self.dots.push(Instant::now());
                self.ripple_at = Some(Instant::now());
            }
        }
        if matches!(self.status, Status::Error(_)) {
            self.status = Status::None;
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Success(_)) || self.is_preview() {
            return;
        }
        let was_awake = self.awake;
        self.activity();
        let k = &e.keystroke;
        self.last_key = Instant::now();
        let key = k.key.as_str();
        if key == "escape" {
            let now = Instant::now();
            let double = self.last_escape.is_some_and(|t| now.duration_since(t) < DOUBLE_ESCAPE);
            self.last_escape = Some(now);
            if self.demo() && double {
                self.exit = true;
            } else if self.session_menu {
                self.session_menu = false;
            } else {
                if self.phase == Phase::Busy
                    && let Some(auth) = &self.auth
                {
                    auth.send(AuthCommand::Cancel);
                    self.phase = Phase::Idle;
                }
                wipe(&mut self.password);
                self.dots.clear();
                if self.prompt.take().is_some()
                    && let Some(auth) = &self.auth
                {
                    auth.send(AuthCommand::Cancel);
                }
                self.status = Status::None;
                self.field = if self.manual.is_some() && self.username().is_none() { Field::Username } else { Field::Password };
            }
            cx.notify();
            return;
        }
        if !was_awake && matches!(key, "enter" | "space" | "tab") {
            // The first press only wakes the screen.
            cx.notify();
            return;
        }
        if key == "tab" {
            let fields = self.fields();
            let i = fields.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
            let step = if k.modifiers.shift { -1 } else { 1 };
            self.field = fields[((i + step).rem_euclid(fields.len() as isize)) as usize];
            self.session_menu = false;
            cx.notify();
            return;
        }
        match self.field {
            Field::Session => match key {
                "up" | "left" => self.cycle_session(-1),
                "down" | "right" => self.cycle_session(1),
                "enter" | "space" => {
                    if self.session_menu {
                        self.session_menu = false;
                        self.field = Field::Password;
                    } else {
                        self.session_menu = true;
                    }
                }
                _ => {}
            },
            Field::User => {
                let n = self.users.len() + usize::from(!self.demo());
                match key {
                    "left" | "up" => self.select_user((self.user_ix + n - 1) % n.max(1), cx),
                    "right" | "down" => self.select_user((self.user_ix + 1) % n.max(1), cx),
                    "enter" | "space" => self.field = Field::Password,
                    _ => {}
                }
            }
            Field::Username | Field::Password => {
                if self.phase != Phase::Idle {
                    return;
                }
                match key {
                    "enter" => self.submit(cx),
                    "backspace" => {
                        let pw = self.field == Field::Password;
                        let text = if pw { &mut self.password } else { self.manual.get_or_insert_with(String::new) };
                        if k.modifiers.control {
                            wipe(text);
                            if pw {
                                self.dots.clear();
                            }
                        } else {
                            text.pop();
                            if pw {
                                self.dots.pop();
                            }
                        }
                    }
                    "u" if k.modifiers.control => {
                        wipe(&mut self.password);
                        self.dots.clear();
                    }
                    _ => {
                        if let Some(ch) = k.key_char.clone().filter(|_| !k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform) {
                            self.type_char(&ch);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// `--demo-autoplay`: a scripted run for screenshots (demo only): wake,
    /// type "wrong", fail, type a right one, succeed.
    fn run_autoplay(&mut self, cx: &mut Context<Self>) {
        let Some(step) = self.autoplay else { return };
        let t = self.started.elapsed().as_secs_f32();
        let schedule = [2.2, 2.6, 2.75, 2.9, 3.05, 3.2, 3.6, 5.6, 5.75, 5.9, 6.05, 6.2, 6.35, 6.5, 6.65, 7.4];
        if step as usize >= schedule.len() || t < schedule[step as usize] || self.phase == Phase::Busy {
            return;
        }
        match step {
            0 => self.activity(),
            1..=5 => self.type_char(["w", "r", "o", "n", "g"][step as usize - 1]),
            6 | 15 => self.submit(cx),
            _ => self.type_char("k"),
        }
        self.activity();
        self.last_key = Instant::now();
        self.autoplay = Some(step + 1);
    }

    // ── Editing (preview) ────────────────────────────────────────────────

    fn begin_drag(&mut self, element: Element, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.selected = Some(element);
        cx.emit(PreviewEvent::Selected(Some(element)));
        if let Some(b) = self.bounds.borrow().get(&element).copied() {
            self.drag = Some(Drag { element, start: at, bounds: b, moved: false });
        }
        cx.notify();
    }

    fn drag_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else { return };
        let (dx, dy) = (f32::from(at.x - drag.start.x), f32::from(at.y - drag.start.y));
        if dx.abs() + dy.abs() < 2.0 && !drag.moved {
            return;
        }
        drag.moved = true;
        let (vw, vh) = self.preview_size;
        let u = vh / 1080.0 * if self.config.accessibility.large_text { 1.2 } else { 1.0 };
        let o = self.origin.get();
        let b = drag.bounds;
        let (x, y) = (f32::from(b.origin.x - o.x) + dx, f32::from(b.origin.y - o.y) + dy);
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let (anchor, ox, oy, snap_x, snap_y) = snap_placement((x, y, w, h), (vw, vh), u);
        let element = drag.element;
        let p = self.config.placement_mut(element);
        p.anchor = anchor;
        p.x = ox;
        p.y = oy;
        self.snapped = Some((anchor, snap_x, snap_y));
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.drag.take()
            && d.moved
        {
            cx.emit(PreviewEvent::Edited(self.config.clone()));
        }
        self.snapped = None;
        cx.notify();
    }
}

/// Where a dragged element lands: the anchor whose screen point is nearest
/// the element's matching point, and the offset from it in reference
/// pixels (`u` = output pixels per reference pixel). Offsets within 7 px
/// of the anchor snap to 0 (reported so guides can show).
pub fn snap_placement(rect: (f32, f32, f32, f32), view: (f32, f32), u: f32) -> (Anchor, f32, f32, bool, bool) {
    let (x, y, w, h) = rect;
    let (vw, vh) = view;
    let dist = |a: &Anchor| {
        let (ex, ey) = a.point(x, y, w, h);
        let (sx, sy) = a.point(0.0, 0.0, vw, vh);
        (ex - sx).powi(2) + (ey - sy).powi(2)
    };
    let anchor = Anchor::ALL.iter().copied().min_by(|a, b| dist(a).total_cmp(&dist(b))).unwrap_or(Anchor::Center);
    let (ex, ey) = anchor.point(x, y, w, h);
    let (sx, sy) = anchor.point(0.0, 0.0, vw, vh);
    let snap = 7.0;
    let (ox, oy) = (ex - sx, ey - sy);
    let (snap_x, snap_y) = (ox.abs() < snap, oy.abs() < snap);
    let u = u.max(1e-3);
    (anchor, if snap_x { 0.0 } else { (ox / u).round() }, if snap_y { 0.0 } else { (oy / u).round() }, snap_x, snap_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_edits_rebuild_only_for_new_sources() {
        let base = GreeterConfig::default();
        let key = |f: &dyn Fn(&mut GreeterConfig)| {
            let mut c = base.clone();
            f(&mut c);
            background_change(&BackgroundSources::of(&base), &BackgroundSources::of(&c))
        };
        // Timings and per-frame looks keep the backdrop.
        assert_eq!(key(&|c| c.background.crossfade_ms += 100), BackgroundChange::Same);
        assert_eq!(key(&|c| c.background.ken_burns += 0.05), BackgroundChange::Same);
        assert_eq!(key(&|c| c.background.playback_rate = 0.5), BackgroundChange::Same);
        assert_eq!(key(&|c| c.background.dim += 0.1), BackgroundChange::Same);
        assert_eq!(key(&|c| c.background.parallax += 1.0), BackgroundChange::Same);
        // Switches rebuild at once.
        assert_eq!(key(&|c| c.background.mode = BackgroundMode::Solid), BackgroundChange::Rebuild);
        assert_eq!(key(&|c| c.background.video = VideoMode::Off), BackgroundChange::Rebuild);
        assert_eq!(key(&|c| c.accessibility.reduced_motion = !c.accessibility.reduced_motion), BackgroundChange::Rebuild);
        // Typing folders / dragging the softening waits for the edits to settle.
        assert_eq!(key(&|c| c.background.blur += 0.1), BackgroundChange::RebuildSettled);
        assert_eq!(key(&|c| c.background.video_dirs.push("/tmp/x".into())), BackgroundChange::RebuildSettled);
    }

    #[test]
    fn solid_floor_follows_the_theme_when_unset() {
        let mut a = GreeterConfig::default();
        a.background.mode = BackgroundMode::Solid;
        a.background.solid.clear();
        let mut b = a.clone();
        b.background.solid = "#123456".into();
        assert_eq!(background_change(&BackgroundSources::of(&a), &BackgroundSources::of(&b)), BackgroundChange::Rebuild);
    }

    #[test]
    fn dragging_snaps_to_the_nearest_anchor() {
        // 960×540 preview (u = 0.5). An element near the bottom-left corner.
        let (a, x, y, sx, sy) = snap_placement((20.0, 480.0, 100.0, 40.0), (960.0, 540.0), 0.5);
        assert_eq!(a, Anchor::BottomLeft);
        assert_eq!((x, y), (40.0, -40.0));
        assert!(!sx && !sy);
        // Centered horizontally within the snap distance → offset 0.
        let (a, x, _, sx, _) = snap_placement((430.0 + 3.0, 10.0, 100.0, 30.0), (960.0, 540.0), 0.5);
        assert_eq!(a, Anchor::Top);
        assert_eq!(x, 0.0);
        assert!(sx);
        // The middle of the screen.
        let (a, ..) = snap_placement((400.0, 250.0, 160.0, 40.0), (960.0, 540.0), 0.5);
        assert_eq!(a, Anchor::Center);
    }

    #[test]
    fn blend_composites_over() {
        let out = gpui::Rgba::from(blend(gpui::black().opacity(1.0), gpui::white().opacity(0.5)));
        assert!((out.r - 0.5).abs() < 0.01 && (out.a - 1.0).abs() < 1e-4);
    }
}

// ── Rendering helpers ──────────────────────────────────────────────────

/// Stroke icon tinted with `color`: the shell's set, plus a few extras
/// served from files so the view also renders inside Studio.
fn icon(name: &str, size: f32, color: Hsla) -> impl IntoElement {
    let el = svg().size(px(size)).flex_none().text_color(color);
    match crate::assets::extra_icon_file(name) {
        Some(path) => el.external_path(SharedString::from(path)),
        None => el.path(SharedString::from(format!("icons/{name}.svg"))),
    }
}

fn soft_shadow(blur: f32, alpha: f32, y: f32) -> BoxShadow {
    BoxShadow { color: gpui::black().opacity(alpha), offset: point(px(0.0), px(y)), blur_radius: px(blur), spread_radius: px(0.0), inset: false }
}

fn glow(color: Hsla, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow { color, offset: point(px(0.0), px(0.0)), blur_radius: px(blur), spread_radius: px(spread), inset: false }
}

/// Specular rim of glass controls, scaled by `rim` (0–1).
fn rim(rim: f32) -> Vec<BoxShadow> {
    if rim <= 0.01 {
        return Vec::new();
    }
    mandara_core::glass_rim(Color::oklcha(1.0, 0.0, 0.0, 0.42 * rim), Color::oklcha(0.0, 0.0, 0.0, 0.18 * rim))
}

/// Text with a soft offset copy behind it (GPUI has no text-shadow).
fn shadowed(text: impl Into<SharedString>, dy: f32, alpha: f32) -> AnyElement {
    let text: SharedString = text.into();
    div()
        .relative()
        .child(div().absolute().top(px(dy)).left_0().text_color(gpui::black().opacity(alpha)).child(text.clone()))
        .child(div().relative().child(text))
        .into_any_element()
}

/// 0 → 1 with the spring's overshoot, `t` seconds after a start.
fn pop_at(cfg: SpringConfig, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    1.0 - cfg.solve(1.0, 0.0, t).0
}

fn weight(w: u16) -> FontWeight {
    FontWeight(w as f32)
}

/// Per-frame values shared by element builders.
struct Frame {
    u: f32,
    vw: f32,
    vh: f32,
    now: Instant,
    white: Hsla,
    ui_font: SharedString,
    display_font: SharedString,
    rtl: bool,
    success: f32,
    reduced: bool,
    high_contrast: bool,
    t: f32,
}

impl Frame {
    /// Control fill for the configured style.
    fn fill(&self, cfg: &GreeterConfig, p: &MandaraPalette, focused: bool) -> Hsla {
        if self.high_contrast {
            return gpui::black().opacity(if focused { 0.72 } else { 0.6 });
        }
        match cfg.password.style {
            FieldStyle::Solid => p.surface_container.with_alpha(if focused { 0.96 } else { 0.9 }).hsla(),
            _ => self.white.opacity((cfg.theme.glass + if focused { 0.08 } else { 0.0 }).clamp(0.0, 1.0)),
        }
    }
}

impl GreeterView {
    /// Starts the entrance once the view is warm: a few invisible frames
    /// have loaded fonts, rasterized glyphs and uploaded images, and the
    /// backdrop has its first picture (or [`BACKDROP_WAIT`] passed).
    fn warm_up(&mut self, now: Instant) {
        if self.entrance_at.is_some() {
            return;
        }
        let first = *self.first_frame_at.get_or_insert(now);
        self.warm_frames = self.warm_frames.saturating_add(1);
        let backdrop = self.background.ready() || now.saturating_duration_since(first) >= BACKDROP_WAIT;
        if self.warm_frames > WARMUP_FRAMES && backdrop {
            self.entrance_at = Some(now);
            // Breathing, caret blink and the autoplay script count from here.
            self.started = now;
            self.last_activity = now;
        }
    }

    /// The keyboard layout label, recomputed only when the configured or
    /// the compositor's layout changes (the fallback reads files).
    fn keyboard_label(&mut self, cx: &gpui::App) -> SharedString {
        let key = (self.config.keyboard.layout.clone(), cx.keyboard_layout().id().to_string());
        match &self.kb_layout {
            Some((k, label)) if *k == key => label.clone(),
            _ => {
                let label = SharedString::from(keyboard_layout(&self.config, cx));
                self.kb_layout = Some((key, label.clone()));
                label
            }
        }
    }

    /// Invisible text that makes the first frames rasterize every clock
    /// glyph at the clock's size, so a digit seen for the first time never
    /// costs a rasterization mid-animation.
    fn glyph_warmer(&self, f: &Frame) -> Option<AnyElement> {
        if self.entrance_at.is_some() {
            return None;
        }
        let c = &self.config.clock;
        let idle = 1.0 - self.wake.value().clamp(0.0, 1.0);
        let k = f.u * self.config.placement(Element::Clock).scale;
        let size = snap_size(c.size * k * (1.0 + CLOCK_IDLE_GROWTH * idle));
        let font = if c.font.trim().is_empty() { f.display_font.clone() } else { SharedString::from(c.font.clone()) };
        // Status lines appear mid-animation (shake + flash on a wrong
        // password), so their glyphs are rasterized up front too.
        let status_k = f.u * self.config.placement(Element::Password).scale;
        let mut status = div().font_family(f.ui_font.clone()).text_size(px(STATUS_TEXT_PX * status_k)).children(playful_lines());
        status = status.child(tl("Caps Lock is on"));
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .opacity(0.0)
                .child(div().font_family(font).font_features(tabular_nums()).text_size(px(size)).font_weight(weight(c.weight)).child(SharedString::new_static(CLOCK_GLYPHS)))
                .child(status)
                .into_any_element(),
        )
    }

    /// Preview only: whether the preview is scrolled out of the window.
    fn offscreen(&self, window: &Window) -> bool {
        let top = f32::from(self.origin.get().y);
        let height = f32::from(window.viewport_size().height);
        top + self.preview_size.1 < 0.0 || top > height
    }

    /// Schedules the next frame, always on the display's frame clock (a
    /// timer-driven cadence beats against the refresh rate and judders):
    /// every frame on the login screen and in a visible Studio preview,
    /// where the page around it is cached so a frame costs only the
    /// preview; nothing while the preview is scrolled out of view (moving
    /// it back re-renders it, which resumes).
    fn schedule_frame(&mut self, window: &mut Window) {
        if self.is_preview() && self.offscreen(window) {
            // Restart the clock cleanly after the pause (no catch-up jump).
            self.clock.idle();
            return;
        }
        window.request_animation_frame();
    }

    fn tick(&mut self, now: Instant, dt: f32) {
        // Staggered entrance.
        let m = &self.config.motion;
        // Nothing moves until the warm-up is over.
        if let Some(entrance_at) = self.entrance_at {
            let since = now.saturating_duration_since(entrance_at).as_secs_f32();
            let stagger = m.stagger_ms as f32 / MS_PER_S;
            let order = [Element::Date, Element::Clock, Element::User, Element::Password, Element::Users, Element::Session, Element::Power, Element::Keyboard, Element::Banner];
            // The avatar pops with the third element (the user card).
            let pop_slot = order.iter().position(|e| *e == Element::User).unwrap_or(0) as f32;
            for (i, e) in order.iter().enumerate() {
                if !m.entrance || since >= ENTRANCE_LEAD_S + i as f32 * stagger {
                    self.enter[element_index(*e)].set_target(1.0);
                }
            }
            if !m.entrance || since >= ENTRANCE_LEAD_S + pop_slot * stagger {
                self.pop.set_target(1.0);
            }
        }
        for s in &mut self.enter {
            s.step(dt);
        }
        self.pop.step(dt);
        // Idle ↔ awake.
        let idle_after = self.config.motion.idle_seconds;
        if !self.is_preview()
            && idle_after > 0
            && self.awake
            && self.password.is_empty()
            && self.phase == Phase::Idle
            && !self.session_menu
            && now.duration_since(self.last_activity) > Duration::from_secs(idle_after as u64)
        {
            self.awake = false;
        }
        let awake = if self.is_preview() { !self.preview_idle } else { self.awake };
        self.wake.set_target(if awake { 1.0 } else { 0.0 });
        self.wake.step(dt);
        self.shake.step(dt);
        self.success.step(dt);
        let typing = !self.password.is_empty() && self.config.password.glow;
        self.glow.set_target(if typing || self.phase == Phase::Busy { 1.0 } else { 0.0 });
        self.glow.step(dt);
        self.pointer.0.step(dt);
        self.pointer.1.step(dt);
        // Rolling digits (formatted only when the second changes).
        let local = chrono::Local::now();
        let second = local.timestamp();
        if second != self.clock_second {
            self.clock_second = second;
            let date_key = (local.date_naive(), self.config.date.format.clone(), mandara_i18n::language().posix());
            if self.date_text.as_ref().is_none_or(|(k, _)| *k != date_key) {
                let locale = chrono::Locale::try_from(date_key.2).unwrap_or(chrono::Locale::en_US);
                let pattern = if date_key.1.trim().is_empty() { tl("%A, %-d %B") } else { date_key.1.clone() };
                let text = SharedString::from(date_key.0.format_localized(&pattern, locale).to_string());
                self.date_text = Some((date_key, text));
            }
        }
        let fmt = match (self.config.clock.format_24h, self.config.clock.seconds) {
            (true, false) => "%H:%M",
            (true, true) => "%H:%M:%S",
            (false, false) => "%-I:%M",
            (false, true) => "%-I:%M:%S",
        };
        let changed = self.roll.second != second || self.roll.format != fmt;
        let text = if changed { local.format(fmt).to_string() } else { String::new() };
        self.roll.second = second;
        self.roll.format = fmt;
        if changed && text != self.roll.text {
            if !self.roll.text.is_empty() && self.config.clock.roll_digits && !self.config.accessibility.reduced_motion {
                self.roll.prev = std::mem::take(&mut self.roll.text);
                self.roll.at = Some(now);
            }
            self.roll.text = text;
        }
        // Preview simulations end by themselves.
        if let Some(at) = self.sim_reset
            && now >= at
        {
            self.sim_reset = None;
            self.password.clear();
            self.dots.clear();
            self.status = Status::None;
            self.phase = Phase::Idle;
            self.success.snap_to(0.0);
            self.success.set_target(0.0);
        }
    }

    /// The anchored, animated container for one element.
    fn place(&self, f: &Frame, e: Element, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let pl = self.config.placement(e);
        let editing = self.is_preview();
        if !pl.visible && !editing {
            return div().into_any_element();
        }
        let enter = self.enter[element_index(e)].value();
        let wake = self.wake.value().clamp(0.0, 1.0);
        let (fx, fy) = pl.anchor.fraction();
        let mut dx = pl.x * f.u;
        let mut dy = pl.y * f.u + 18.0 * f.u * (1.0 - enter.clamp(0.0, 1.2));
        let mut alpha = enter.clamp(0.0, 1.0);
        if e.wakes() {
            alpha *= wake;
            dy += 30.0 * f.u * (1.0 - wake);
        } else if fy < 0.25 {
            // Idle: the clock and date drift toward the middle of the screen.
            dy += f.vh * 0.2 * (1.0 - wake);
        }
        alpha *= 1.0 - f.success;
        // Pointer parallax (foreground moves with the pointer).
        if !f.reduced {
            let k = self.config.background.parallax * 0.35 * f.u;
            dx += self.pointer.0.value() * k;
            dy += self.pointer.1.value() * k;
        }
        if editing && !pl.visible {
            alpha = 0.3;
        }
        let selected = editing && self.selected == Some(e);
        let bounds = self.bounds.clone();
        let mut wrapper = div().id(("element", element_index(e))).relative().child(content).child(
            canvas(
                move |b, _, _| {
                    bounds.borrow_mut().insert(e, b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        if editing {
            let accent = self.palette.primary.with_lightness(0.8).hsla();
            wrapper = wrapper
                .cursor_grab()
                .rounded(px(8.0))
                .border_1()
                .border_color(if selected { accent } else { gpui::transparent_black() })
                .hover(|s| s.border_color(gpui::white().opacity(0.45)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.begin_drag(e, ev.position, cx);
                    }),
                );
        }
        let mut container = div().absolute().left(px(dx)).top(px(dy)).w(px(f.vw)).h(px(f.vh)).flex().opacity(alpha);
        container = if fx < 0.25 {
            container.justify_start()
        } else if fx > 0.75 {
            container.justify_end()
        } else {
            container.justify_center()
        };
        container = if fy < 0.25 {
            container.items_start()
        } else if fy > 0.75 {
            container.items_end()
        } else {
            container.items_center()
        };
        container.child(wrapper).into_any_element()
    }

    fn clock_el(&self, f: &Frame, k: f32) -> AnyElement {
        let c = &self.config.clock;
        let idle = 1.0 - self.wake.value().clamp(0.0, 1.0);
        let size = snap_size(c.size * k * (1.0 + CLOCK_IDLE_GROWTH * idle) * (1.0 + CLOCK_SUCCESS_GROWTH * f.success));
        let line = size * 1.08;
        let font = if c.font.trim().is_empty() { f.display_font.clone() } else { SharedString::from(c.font.clone()) };
        let roll_t = self.roll.at.map(|t| f.now.saturating_duration_since(t).as_secs_f32()).unwrap_or(10.0);
        let progress = pop_at(ROLL_SPRING, roll_t).clamp(0.0, ROLL_OVERSHOOT_CAP);
        let rolling = progress < 1.0 && self.roll.prev.chars().count() == self.roll.text.chars().count();
        let mut row = div()
            .flex()
            .font_family(font)
            .font_features(tabular_nums())
            .text_size(px(size))
            .line_height(px(line))
            .font_weight(weight(c.weight))
            .text_color(f.white.opacity(c.opacity));
        let mut prev = self.roll.prev.chars();
        for ch in self.roll.text.chars() {
            let old = prev.next().filter(|o| rolling && *o != ch);
            let cell = div().relative().h(px(line)).overflow_hidden();
            let cell = match old {
                Some(o) => cell
                    .child(div().relative().top(px(line * (1.0 - progress))).opacity(progress.clamp(0.0, 1.0)).child(shadowed(glyph_str(ch), CLOCK_SHADOW_DY * k, CLOCK_SHADOW_ALPHA)))
                    .child(div().absolute().left_0().top(px(-line * progress)).opacity((1.0 - progress).clamp(0.0, 1.0)).child(shadowed(glyph_str(o), CLOCK_SHADOW_DY * k, CLOCK_SHADOW_ALPHA))),
                None => cell.child(shadowed(glyph_str(ch), CLOCK_SHADOW_DY * k, CLOCK_SHADOW_ALPHA)),
            };
            row = row.child(cell);
        }
        row.into_any_element()
    }

    fn date_el(&self, f: &Frame, k: f32) -> AnyElement {
        let d = &self.config.date;
        let text = self.date_text.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
        let font = if d.font.trim().is_empty() { f.ui_font.clone() } else { SharedString::from(d.font.clone()) };
        div().font_family(font).text_size(px(d.size * k)).font_weight(weight(d.weight)).text_color(f.white.opacity(d.opacity)).child(shadowed(text, 1.5 * k, 0.3)).into_any_element()
    }

    fn avatar_radius(&self, d: f32) -> f32 {
        match self.config.user.avatar_shape {
            AvatarShape::Circle => d / 2.0,
            AvatarShape::Squircle => d * 0.32,
            AvatarShape::Rounded => d * 0.18,
        }
    }

    fn avatar_el(&self, entry: Option<&UserEntry>, d: f32, f: &Frame) -> AnyElement {
        let p = &self.palette;
        let r = self.avatar_radius(d);
        let rimmed = rim(self.config.theme.rim.max(0.5));
        match entry {
            Some(UserEntry { avatar: Some(image), .. }) if self.config.user.avatar_source == AvatarSource::Auto => {
                img(ImageSource::Render(image.clone())).size(px(d)).rounded(px(r)).object_fit(ObjectFit::Cover).into_any_element()
            }
            Some(UserEntry { user, .. }) if self.config.user.avatar_source != AvatarSource::Icon => div()
                .size(px(d))
                .rounded(px(r))
                .bg(oklab_gradient(145.0, p.accent_gradient_start.with_lightness(0.66), p.accent_gradient_end.with_lightness(0.42)))
                .shadow(rimmed)
                .flex()
                .items_center()
                .justify_center()
                .font_family(f.display_font.clone())
                .text_size(px(snap_size(d * INITIALS_SCALE)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::white())
                .child(user.initials())
                .into_any_element(),
            _ => div()
                .size(px(d))
                .rounded(px(r))
                .bg(gpui::white().opacity(0.18))
                .shadow(rimmed)
                .flex()
                .items_center()
                .justify_center()
                .child(icon("user", snap_size(d * USER_ICON_SCALE), gpui::white().opacity(0.92)))
                .into_any_element(),
        }
    }

    fn user_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let u = &self.config.user;
        let entry = if self.manual.is_none() { self.users.get(self.user_ix) } else { None };
        let grow = 1.0 + 0.5 * f.success;
        let d = u.avatar_size * k * self.pop.value().max(0.0) * grow;
        let ring_w = 2.5 * k;
        let breathing = u.glow_ring && self.password.is_empty() && !f.reduced;
        let b = if breathing { (f.t * 1.25).sin() * 0.5 + 0.5 } else { 0.4 };
        let accent = self.palette.primary;
        let ring_r = self.avatar_radius(d) + ring_w * 2.0;
        let mut ring = div().p(px(ring_w * 1.6)).rounded(px(ring_r));
        if u.glow_ring {
            ring = ring.border_color(accent.with_lightness(0.8).with_alpha(0.35 + 0.35 * b).hsla()).shadow(vec![
                glow(accent.with_alpha(0.22 + 0.25 * b).hsla(), 26.0 * k, (2.0 + 5.0 * b) * k),
                soft_shadow(30.0 * k, 0.3, 10.0 * k),
            ]);
            let w: gpui::AbsoluteLength = px(ring_w).into();
            let widths = &mut ring.style().border_widths;
            widths.top = Some(w);
            widths.right = Some(w);
            widths.bottom = Some(w);
            widths.left = Some(w);
        } else {
            ring = ring.shadow(vec![soft_shadow(30.0 * k, 0.35, 10.0 * k)]);
        }
        let name = match (&self.manual, entry) {
            (None, Some(e)) => e.user.display.clone(),
            _ if self.is_preview() => tl("Your Name"),
            _ => tl("Other user"),
        };
        let avatar = self.avatar_el(entry, d, f);
        let clickable = !self.is_preview() && (self.users.len() > 1 || !self.demo());
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .id("avatar")
                    .when(clickable, |el| {
                        el.cursor_pointer().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.field = Field::User;
                            this.activity();
                            cx.notify();
                        }))
                    })
                    .child(ring.child(avatar)),
            )
            .when(u.show_name, |el| {
                el.child(
                    div()
                        .mt(px(14.0 * k))
                        .font_family(f.ui_font.clone())
                        .text_size(px(u.name_size * k))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(f.white.opacity(0.96))
                        .child(shadowed(name, 1.5 * k, 0.35)),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn pill(&self, f: &Frame, k: f32, id: &'static str, text: String, secret: bool, placeholder: String, focused: bool, unfold: f32, cx: &mut Context<Self>) -> AnyElement {
        let cfg = &self.config;
        let pc = &cfg.password;
        let p = &self.palette;
        let white = f.white;
        let h = pc.height * k;
        let full_w = pc.width * k;
        let w = h + (full_w - h) * unfold.clamp(0.0, 1.0);
        let radius = (h / 2.0) * cfg.theme.roundness;
        let is_pw = id == "password";
        let busy = self.phase == Phase::Busy && is_pw;
        let empty = text.is_empty();
        let caret_on = focused && !busy && !self.is_preview() && ((self.started.elapsed().as_millis() / 530) % 2 == 0 || self.last_key.elapsed() < CARET_HOLD);
        let field = if is_pw { Field::Password } else { Field::Username };
        // Keystroke dots pop in with a spring.
        let pop = SpringConfig::new(520.0, 14.0, 1.0);
        let content: AnyElement = if secret && !empty && !(self.reveal && is_pw) {
            let mut row = div().flex().items_center().gap(px(5.0 * k));
            let n = text.chars().count().min(24);
            for i in 0..n {
                let born = self.dots.get(self.dots.len().saturating_sub(n) + i).copied();
                let s = if pc.keystroke_ripple && !f.reduced { born.map(|b| pop_at(pop, f.now.saturating_duration_since(b).as_secs_f32())).unwrap_or(1.0) } else { 1.0 };
                let d = 8.0 * k * s.max(0.0);
                row = row.child(div().w(px(8.0 * k)).h(px(8.0 * k)).flex().items_center().justify_center().child(div().size(px(d)).rounded_full().bg(white.opacity(0.95))));
            }
            row.into_any_element()
        } else if empty {
            div().text_color(white.opacity(0.72)).child(placeholder).into_any_element()
        } else {
            div().text_color(white.opacity(0.96)).child(text).into_any_element()
        };
        let caret = |el: gpui::Div, before: bool| {
            el.when(caret_on, |el| {
                let c = div().w(px(1.5)).h(px(h * 0.46)).rounded_full().bg(white.opacity(0.95));
                el.child(if before { c.mr(px(4.0 * k)) } else { c.ml(px(4.0 * k)) })
            })
        };
        let mut inner = div().flex_1().flex().items_center().overflow_hidden().font_family(f.ui_font.clone()).text_size(px(15.0 * k)).when(f.rtl, |el| el.flex_row_reverse());
        if empty {
            inner = caret(inner, true).child(content);
        } else {
            inner = caret(inner.child(content), false);
        }
        let trailing: Option<AnyElement> = if busy {
            Some(spinner(h * 0.62, k, self.started.elapsed().as_secs_f32()))
        } else if is_pw && !empty {
            let reveal = pc.reveal_toggle.then(|| {
                div()
                    .id("reveal")
                    .size(px(h * 0.62))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(gpui::white().opacity(0.12)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.reveal = !this.reveal;
                        cx.notify();
                    }))
                    .child(icon(if self.reveal { "eye-off" } else { "eye" }, h * 0.36, white.opacity(0.85)))
            });
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0 * k))
                    .children(reveal)
                    .child(
                        div()
                            .id("submit")
                            .size(px(h * 0.72))
                            .rounded_full()
                            .bg(p.primary.with_alpha(0.9).hsla())
                            .hover(|s| s.bg(p.primary.hsla()))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))
                            .child(icon(if f.rtl { "arrow-left" } else { "arrow-right" }, h * 0.4, p.on_primary.hsla())),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };
        // Shadows: rim, depth, typing glow, keystroke ripple.
        let mut shadows = if pc.style == FieldStyle::Glass { rim(cfg.theme.rim) } else { Vec::new() };
        shadows.push(soft_shadow(22.0 * k, 0.24, 6.0 * k));
        let g = self.glow.value().clamp(0.0, 1.0);
        if g > 0.01 && is_pw {
            shadows.push(glow(p.primary.with_alpha(0.5 * g).hsla(), 28.0 * k, 1.0 * k));
        }
        if let Some(at) = self.ripple_at.filter(|_| pc.keystroke_ripple && !f.reduced && is_pw) {
            let t = f.now.saturating_duration_since(at).as_secs_f32() / 0.45;
            if (0.0..1.0).contains(&t) {
                shadows.push(glow(p.primary.with_lightness(0.82).with_alpha(0.55 * (1.0 - t)).hsla(), 4.0 * k, 12.0 * k * t));
            }
        }
        let mut fill = f.fill(cfg, p, focused);
        if let Some(at) = self.flash_at.filter(|_| is_pw) {
            let t = f.now.saturating_duration_since(at).as_secs_f32() / 0.9;
            if t < 1.0 {
                fill = blend(fill, p.urgent_coral.with_alpha(0.6 * (1.0 - t)).hsla());
            }
        }
        let shake = if is_pw { self.shake.value() * f.u } else { 0.0 };
        let mut pill = div()
            .id(id)
            .ml(px(shake))
            .w(px(w))
            .h(px(h))
            .pl(px(16.0 * k))
            .pr(px(5.0 * k))
            .flex()
            .items_center()
            .gap(px(6.0 * k))
            .cursor_text()
            .shadow(shadows)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.field = field;
                this.session_menu = false;
                this.activity();
                cx.notify();
            }));
        pill = match pc.style {
            FieldStyle::Underline => {
                pill.style().border_widths.bottom = Some(px(1.5).into());
                pill.border_color(white.opacity(if focused { 0.9 } else { 0.55 }))
            }
            FieldStyle::Outline => pill.rounded(px(radius)).border_1().border_color(white.opacity(if focused { 0.85 } else { 0.5 })),
            _ => pill.rounded(px(radius)).bg(fill).border_1().border_color(if focused { white.opacity(0.3) } else { gpui::transparent_black() }),
        };
        pill.when(unfold > 0.6, |el| el.child(inner).children(trailing)).into_any_element()
    }

    fn password_el(&self, f: &Frame, k: f32, caps: bool, cx: &mut Context<Self>) -> AnyElement {
        let cfg = &self.config;
        let unfold = self.enter[element_index(Element::Password)].value();
        let mut col = div().flex().flex_col().items_center();
        if self.manual.is_some() {
            let name = self.manual.clone().unwrap_or_default();
            col = col.child(self.pill(f, k, "username", name, false, tl("Username"), self.field == Field::Username, unfold, cx)).child(div().h(px(12.0 * k)));
        }
        let placeholder = match &self.prompt {
            Some((_, msg)) if !msg.is_empty() => msg.trim_end_matches(':').to_string(),
            _ if !cfg.password.placeholder.trim().is_empty() => cfg.password.placeholder.clone(),
            _ => tl("Enter Password"),
        };
        let secret = self.prompt.as_ref().is_none_or(|(s, _)| *s);
        col = col.child(self.pill(f, k, "password", self.password.clone(), secret, placeholder, self.field == Field::Password, unfold, cx));
        let white = f.white;
        let status: Option<(Option<&'static str>, String, Hsla)> = match &self.status {
            Status::Error(e) => Some((None, e.clone(), self.palette.urgent_coral.with_lightness(0.85).hsla())),
            Status::Info(i) => Some((None, i.clone(), white.opacity(0.88))),
            Status::None if caps && cfg.password.caps_hint => Some((Some("caps"), tl("Caps Lock is on"), white.opacity(0.88))),
            Status::None if self.demo() => Some((None, tl("Demo: any password works — try “wrong” to see a failure"), white.opacity(0.78))),
            Status::None => None,
        };
        col.child(
            div()
                .mt(px(12.0 * k))
                .h(px(24.0 * k))
                .flex()
                .items_center()
                .gap(px(6.0 * k))
                .font_family(f.ui_font.clone())
                .text_size(px(STATUS_TEXT_PX * k))
                .when_some(status, |el, (ic, text, color)| {
                    el.px(px(12.0 * k)).rounded_full().bg(gpui::black().opacity(0.26)).when_some(ic, |el, ic| el.child(icon(ic, 14.0 * k, color))).child(div().text_color(color).child(text))
                }),
        )
        .into_any_element()
    }

    fn users_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let show_other = !self.demo() && !self.is_preview() && self.config.user.list != UserList::Manual;
        let count = self.users.len() + usize::from(show_other) + usize::from(self.is_preview());
        if count <= 1 && !self.is_preview() {
            return div().into_any_element();
        }
        let focus_users = self.field == Field::User;
        let mut row = div().flex().gap(px(14.0 * k)).items_start();
        for i in 0..count {
            let selected = (self.manual.is_none() && i == self.user_ix) || (self.manual.is_some() && i == self.users.len());
            let (face, label) = match self.users.get(i) {
                Some(e) => (self.avatar_el(Some(e), 40.0 * k, f), e.user.display.split_whitespace().next().unwrap_or(&e.user.name).to_string()),
                None => (
                    div()
                        .size(px(40.0 * k))
                        .rounded(px(self.avatar_radius(40.0 * k)))
                        .bg(gpui::white().opacity(0.16))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("user", 18.0 * k, f.white.opacity(0.9)))
                        .into_any_element(),
                    tl("Other"),
                ),
            };
            row = row.child(
                div()
                    .id(("user", i))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(5.0 * k))
                    .cursor_pointer()
                    .opacity(if selected { 1.0 } else { 0.62 })
                    .hover(|s| s.opacity(1.0))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_user(i, cx)))
                    .child(
                        div()
                            .rounded(px(self.avatar_radius(48.0 * k)))
                            .p(px(2.0 * k))
                            .border_1()
                            .border_color(if selected && focus_users { f.white.opacity(0.85) } else if selected { f.white.opacity(0.35) } else { gpui::transparent_black() })
                            .child(face),
                    )
                    .child(div().font_family(f.ui_font.clone()).text_size(px(11.5 * k)).text_color(f.white.opacity(0.88)).child(label)),
            );
        }
        row.into_any_element()
    }

    fn session_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let cfg = &self.config;
        let name = self.sessions.get(self.session_ix).map(|s| if s.kind == SessionKind::X11 { format!("{} (X11)", s.name) } else { s.name.clone() }).unwrap_or_else(|| tl("Login shell"));
        let focused = self.field == Field::Session;
        let h = 36.0 * k;
        let pill = div()
            .id("session")
            .h(px(h))
            .px(px(14.0 * k))
            .flex()
            .items_center()
            .gap(px(8.0 * k))
            .rounded(px(h / 2.0 * cfg.theme.roundness))
            .bg(f.fill(cfg, p, focused))
            .shadow(rim(cfg.theme.rim))
            .border_1()
            .border_color(if focused { f.white.opacity(0.6) } else { gpui::transparent_black() })
            .cursor_pointer()
            .hover(|s| s.bg(gpui::white().opacity(0.24)))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.is_preview() {
                    return;
                }
                this.session_menu = !this.session_menu;
                this.field = Field::Session;
                this.activity();
                cx.notify();
            }))
            .child(icon("monitor", 16.0 * k, f.white.opacity(0.92)))
            .child(div().font_family(f.ui_font.clone()).text_size(px(13.5 * k)).font_weight(FontWeight::MEDIUM).text_color(f.white.opacity(0.95)).child(name))
            .when(self.sessions.len() > 1, |el| el.child(icon(if self.session_menu { "chevron-up" } else { "chevron-down" }, 14.0 * k, f.white.opacity(0.8))));
        let mut root = div().relative().child(pill);
        if self.session_menu && !self.sessions.is_empty() {
            let below = cfg.placement(Element::Session).anchor.fraction().1 < 0.5;
            let mut list = div()
                .absolute()
                .when(below, |el| el.top(px(h + 10.0 * k)))
                .when(!below, |el| el.bottom(px(h + 10.0 * k)))
                .left_0()
                .min_w(px(240.0 * k))
                .p(px(6.0 * k))
                .rounded(px(18.0 * k))
                .bg(p.surface_container.with_alpha(0.92).hsla())
                .shadow(vec![soft_shadow(40.0 * k, 0.4, 14.0 * k)])
                .flex()
                .flex_col()
                .gap(px(2.0 * k));
            for (i, s) in self.sessions.iter().enumerate() {
                let selected = i == self.session_ix;
                let label = if s.kind == SessionKind::X11 { format!("{} (X11)", s.name) } else { s.name.clone() };
                list = list.child(
                    div()
                        .id(("session-item", i))
                        .h(px(34.0 * k))
                        .px(px(12.0 * k))
                        .rounded(px(12.0 * k))
                        .flex()
                        .items_center()
                        .gap(px(10.0 * k))
                        .cursor_pointer()
                        .when(selected, |el| el.bg(p.primary.with_alpha(0.22).hsla()))
                        .hover(|st| st.bg(gpui::white().opacity(0.10)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.session_ix = i;
                            this.session_menu = false;
                            this.field = Field::Password;
                            cx.notify();
                        }))
                        .child(div().w(px(16.0 * k)).when(selected, |el| el.child(icon("check", 15.0 * k, p.primary.hsla()))))
                        .child(div().font_family(f.ui_font.clone()).text_size(px(13.5 * k)).text_color(p.on_surface.hsla()).child(label)),
                );
            }
            root = root.child(list);
        }
        root.into_any_element()
    }

    fn power_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let pc = &self.config.power;
        let actions: Vec<PowerAction> = PowerAction::ALL
            .into_iter()
            .filter(|a| match a {
                PowerAction::Suspend => pc.suspend,
                PowerAction::Reboot => pc.reboot,
                PowerAction::PowerOff => pc.poweroff,
            })
            .collect();
        let mut row = div().flex().gap(px(18.0 * k)).opacity(if self.demo() { 0.55 } else { 1.0 });
        for action in actions {
            row = row.child(
                div()
                    .id(action.method())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0 * k))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.power(action, cx)))
                    .child(
                        div()
                            .size(px(44.0 * k))
                            .rounded(px(22.0 * k * self.config.theme.roundness.max(0.3)))
                            .bg(f.fill(&self.config, &self.palette, false))
                            .shadow(rim(self.config.theme.rim))
                            .hover(|s| s.bg(gpui::white().opacity(0.28)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(action.icon(), 19.0 * k, f.white.opacity(0.95))),
                    )
                    .when(pc.labels, |el| el.child(div().font_family(f.ui_font.clone()).text_size(px(11.5 * k)).text_color(f.white.opacity(0.85)).child(action.label()))),
            );
        }
        row.into_any_element()
    }

    fn keyboard_el(&self, f: &Frame, k: f32, layout: SharedString) -> AnyElement {
        div()
            .h(px(28.0 * k))
            .px(px(11.0 * k))
            .rounded(px(14.0 * k * self.config.theme.roundness))
            .bg(f.fill(&self.config, &self.palette, false))
            .shadow(rim(self.config.theme.rim))
            .flex()
            .items_center()
            .gap(px(6.0 * k))
            .child(icon("keyboard", 15.0 * k, f.white.opacity(0.9)))
            .child(div().font_family(f.ui_font.clone()).text_size(px(12.0 * k)).font_weight(FontWeight::SEMIBOLD).text_color(f.white.opacity(0.95)).child(layout))
            .into_any_element()
    }

    fn banner_el(&self, f: &Frame, k: f32) -> AnyElement {
        let text = if self.config.banner.text.trim().is_empty() && self.is_preview() { tl("Your message here") } else { self.config.banner.text.clone() };
        if text.trim().is_empty() {
            return div().into_any_element();
        }
        div()
            .px(px(14.0 * k))
            .py(px(7.0 * k))
            .rounded(px(16.0 * k * self.config.theme.roundness))
            .bg(gpui::black().opacity(0.28))
            .font_family(f.ui_font.clone())
            .text_size(px(self.config.banner.size * k))
            .text_color(f.white.opacity(0.92))
            .child(text)
            .into_any_element()
    }
}

/// Alpha-composites `top` over `base` (straight alpha, sRGB).
fn blend(base: Hsla, top: Hsla) -> Hsla {
    let (b, t) = (gpui::Rgba::from(base), gpui::Rgba::from(top));
    let a = t.a + b.a * (1.0 - t.a);
    if a <= 0.0 {
        return base;
    }
    let mix = |x: f32, y: f32| (y * t.a + x * b.a * (1.0 - t.a)) / a;
    gpui::Rgba { r: mix(b.r, t.r), g: mix(b.g, t.g), b: mix(b.b, t.b), a }.into()
}

/// Three dots pulsing in sequence.
fn spinner(size: f32, k: f32, t: f32) -> AnyElement {
    let mut row = div().w(px(size * 1.4)).h(px(size)).flex().items_center().justify_center().gap(px(3.5 * k));
    for i in 0..3 {
        let phase = (t * 5.0 - i as f32 * 0.7).sin() * 0.5 + 0.5;
        row = row.child(div().size(px(5.5 * k)).rounded_full().bg(gpui::white().opacity(0.35 + 0.6 * phase)));
    }
    row.into_any_element()
}

/// Drifting light orbs (closer than the backdrop: stronger parallax).
fn orbs(vw: f32, vh: f32, p: &MandaraPalette, t: f32, intensity: f32, shift: (f32, f32)) -> AnyElement {
    let m = vw.max(vh);
    let w = |period: f32, phase: f32| (t / period * std::f32::consts::TAU + phase).sin();
    let specs = [
        (0.18 + 0.06 * w(37.0, 0.3), 0.72 + 0.05 * w(29.0, 1.1), 0.16, p.primary),
        (0.83 + 0.05 * w(41.0, 2.2), 0.62 + 0.07 * w(33.0, 0.4), 0.12, p.tertiary),
        (0.62 + 0.09 * w(53.0, 4.1), 0.18 + 0.05 * w(47.0, 2.9), 0.10, p.secondary),
        (0.36 + 0.10 * w(61.0, 1.7), 0.40 + 0.08 * w(43.0, 3.3), 0.07, p.accent_gradient_end),
    ];
    let mut root = div().absolute().top_0().left_0().w(px(vw)).h(px(vh));
    for (x, y, r, c) in specs {
        let d = r * m;
        let depth = 1.6 + (0.2 - r) * 4.0;
        root = root.child(
            div()
                .absolute()
                .left(px(x * vw - d / 2.0 - shift.0 * depth))
                .top(px(y * vh - d / 2.0 - shift.1 * depth))
                .size(px(d))
                .rounded_full()
                .shadow(vec![glow(c.with_lightness(0.75).with_alpha(0.32 * intensity).hsla(), d * 0.8, d * 0.1)]),
        );
    }
    root.into_any_element()
}

impl Render for GreeterView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let dt = self.clock.tick(now);
        let preview = self.is_preview();
        let (vw, vh) = if preview {
            self.preview_size
        } else {
            let vp = window.viewport_size();
            (f32::from(vp.width), f32::from(vp.height))
        };
        let sf = window.scale_factor();
        self.background.set_target(((vw * sf) as u32, (vh * sf) as u32));
        if self.bg_rebuild_at.is_some_and(|at| now >= at) {
            self.rebuild_background();
        }
        self.background.tick(now);
        for image in self.background.take_trash() {
            let _ = window.drop_image(image.clone());
            self.background.dispose(image);
        }
        self.warm_up(now);
        self.tick(now, dt);
        self.run_autoplay(cx);
        if self.autoplay.is_some() {
            // Self-test: clicks pass through to the windows underneath.
            window.set_input_region(Some(&[]));
        }
        self.schedule_frame(window);

        if let Phase::Success(at) = self.phase
            && !preview
            && now.duration_since(at) > SUCCESS_HOLD
        {
            self.exit = true;
        }
        let logging = self.demo() || crate::pacing::enabled();
        let report = self.exit || (logging && now.duration_since(self.stats_at) >= STATS_EVERY);
        if report {
            self.stats_at = now;
            if logging {
                tracing::info!("greeter frames: {}", self.pacing.summary());
                tracing::info!("greeter frames since the entrance: {}", self.pacing_visible.summary());
            }
            if let Some((shown, dropped, cost, secs, lite)) = self.background.video_stats(now) {
                tracing::info!(
                    "video: {shown} frames shown in {secs:.1}s ({:.1} fps), {dropped} dropped, {:.1} ms decode+convert per frame{}",
                    shown as f32 / secs.max(0.001),
                    cost.as_secs_f64() * 1000.0,
                    if lite { " (lite)" } else { "" }
                );
            }
        }
        if self.exit {
            window.remove_window();
            cx.quit();
        }
        if self.toast.as_ref().is_some_and(|(_, t)| now.duration_since(*t) > TOAST_FOR) {
            self.toast = None;
        }

        let layout = self.keyboard_label(cx);
        let cfg = &self.config;
        let p = &self.palette;
        let fonts: &Fonts = cx.global::<Fonts>();
        let rtl = mandara_i18n::language().is_rtl();
        let a11y = &cfg.accessibility;
        let large = if a11y.large_text { 1.2 } else { 1.0 };
        let u = if preview { vh / 1080.0 } else { (vh / 1080.0).clamp(0.6, 2.0) } * large;
        let success = self.success.value().clamp(0.0, 1.0);
        let f = Frame {
            u,
            vw,
            vh,
            now,
            white: gpui::white(),
            ui_font: if rtl { fonts.arabic.clone() } else { fonts.ui.clone() },
            display_font: fonts.display.clone(),
            rtl,
            success,
            reduced: a11y.reduced_motion,
            high_contrast: a11y.high_contrast,
            t: now.saturating_duration_since(self.started).as_secs_f32(),
        };
        let caps = !preview && window.capslock().on && self.field != Field::Session;

        // Backdrop with pointer parallax (moves against the pointer).
        let bgc = &cfg.background;
        let par = if f.reduced { 0.0 } else { bgc.parallax * u };
        let shift = (-self.pointer.0.value() * par, -self.pointer.1.value() * par);
        let zoom = 1.0 + cfg.motion.success_zoom * success;
        let mut root = div()
            .id("greeter")
            .relative()
            .overflow_hidden()
            .font_family(f.ui_font.clone())
            .text_color(f.white)
            .bg(p.surface_dim.hsla())
            .when(preview, |el| el.w(px(vw)).h(px(vh)))
            .when(!preview, |el| el.size_full().track_focus(&self.focus).on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx))))
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.drag_to(e.position, cx);
                    return;
                }
                if this.is_preview() {
                    return;
                }
                let moved = this.last_pointer.is_some_and(|lp| (f32::from(lp.x - e.position.x)).abs() + (f32::from(lp.y - e.position.y)).abs() > 3.0);
                this.last_pointer = Some(e.position);
                if moved {
                    this.activity();
                }
                let (w, h) = (vw.max(1.0), vh.max(1.0));
                this.pointer.0.set_target((f32::from(e.position.x) / w * 2.0 - 1.0).clamp(-1.0, 1.0));
                this.pointer.1.set_target((f32::from(e.position.y) / h * 2.0 - 1.0).clamp(-1.0, 1.0));
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if this.is_preview() {
                        if this.selected.take().is_some() {
                            cx.emit(PreviewEvent::Selected(None));
                        }
                        cx.notify();
                        return;
                    }
                    this.focus.focus(window, cx);
                    this.activity();
                    this.session_menu = false;
                    cx.notify();
                }),
            )
            .child(self.background.element(vw, vh, zoom, shift, &p, now));
        let origin = self.origin.clone();
        root = root.child(canvas(move |b, _, _| origin.set(b.origin), |_, _, _, _| {}).absolute().top_0().left_0().size_full());
        if bgc.orbs && bgc.orb_intensity > 0.0 {
            let t = if f.reduced { 0.0 } else { f.t };
            root = root.child(orbs(vw, vh, &p, t, bgc.orb_intensity, shift));
        }
        // Scrims: tint, dim (+ a little while typing), vignette, grain.
        if let Ok(tint) = Color::from_hex(bgc.tint.trim())
            && bgc.tint_strength > 0.0
        {
            root = root.child(div().absolute().top_0().left_0().size_full().bg(tint.with_alpha(bgc.tint_strength * 0.8).hsla()));
        }
        let typing_dim = 0.12 * self.glow.value().clamp(0.0, 1.0);
        let hc = if f.high_contrast { 0.25 } else { 0.0 };
        root = root.child(div().absolute().top_0().left_0().size_full().bg(gpui::black().opacity((bgc.dim + typing_dim + hc).clamp(0.0, 0.95))));
        let v = bgc.vignette;
        if v > 0.0 {
            root = root
                .child(div().absolute().top_0().left_0().w(px(vw)).h(px(vh * 0.42)).bg(oklab_gradient(180.0, Color::oklcha(0.0, 0.0, 0.0, 0.7 * v), Color::oklcha(0.0, 0.0, 0.0, 0.0))))
                .child(div().absolute().bottom_0().left_0().w(px(vw)).h(px(vh * 0.45)).bg(oklab_gradient(180.0, Color::oklcha(0.0, 0.0, 0.0, 0.0), Color::oklcha(0.0, 0.0, 0.0, 0.95 * v))));
        }
        // A soft shade behind the user card keeps text legible on footage.
        let card = cfg.placement(Element::User);
        if card.visible {
            let (fx, fy) = card.anchor.fraction();
            let (cxp, cyp) = (fx * vw + card.x * u, fy * vh + card.y * u + 60.0 * u);
            let r = 480.0 * u;
            let a = 0.28 * self.wake.value().clamp(0.0, 1.0) * (1.0 - success);
            root = root.child(div().absolute().left(px(cxp - r / 2.0)).top(px(cyp - r / 2.0)).size(px(r)).rounded_full().shadow(vec![glow(gpui::black().opacity(a), r * 0.8, 0.0)]));
        }
        if bgc.grain > 0.0 {
            root = root.child(div().absolute().top_0().left_0().size_full().opacity(bgc.grain).child(crate::background::grain(vw, vh)));
        }

        // Elements in z order.
        let mut placements: smallvec::SmallVec<[&crate::config::Placement; 12]> = cfg.layout.iter().collect();
        placements.sort_by_key(|p| p.z);
        root = root.children(self.glyph_warmer(&f));
        for pl in placements {
            let k = u * pl.scale;
            let content = match pl.element {
                Element::Clock => self.clock_el(&f, k),
                Element::Date => self.date_el(&f, k),
                Element::User => self.user_el(&f, k, cx),
                Element::Password => self.password_el(&f, k, caps, cx),
                Element::Users => self.users_el(&f, k, cx),
                Element::Session => self.session_el(&f, k, cx),
                Element::Power => self.power_el(&f, k, cx),
                Element::Keyboard => self.keyboard_el(&f, k, layout.clone()),
                Element::Banner => self.banner_el(&f, k),
            };
            root = root.child(self.place(&f, pl.element, content, cx));
        }

        // Snap guides while dragging in the preview.
        if let Some((anchor, sx, sy)) = self.snapped {
            let accent = p.primary.with_lightness(0.8).hsla();
            let (fx, fy) = anchor.fraction();
            let (ax, ay) = (fx * vw, fy * vh);
            root = root
                .when(sx, |el| el.child(div().absolute().left(px(ax - 0.5)).top_0().w(px(1.0)).h(px(vh)).bg(accent.opacity(0.8))))
                .when(sy, |el| el.child(div().absolute().top(px(ay - 0.5)).left_0().h(px(1.0)).w(px(vw)).bg(accent.opacity(0.8))));
            for a in Anchor::ALL {
                let (gx, gy) = a.fraction();
                let on = a == anchor;
                let d = if on { 10.0 } else { 6.0 };
                root = root.child(div().absolute().left(px(gx * vw - d / 2.0)).top(px(gy * vh - d / 2.0)).size(px(d)).rounded_full().bg(if on { accent } else { gpui::white().opacity(0.4) }));
            }
        }

        // Demo chrome.
        if self.demo() {
            let k = u;
            root = root.child(
                div()
                    .absolute()
                    .top(px(22.0 * k))
                    .left(px(22.0 * k))
                    .w(px(vw - 44.0 * k))
                    .flex()
                    .when(rtl, |el| el.flex_row_reverse())
                    .justify_between()
                    .items_center()
                    .opacity(1.0 - success)
                    .child(
                        div()
                            .h(px(28.0 * k))
                            .px(px(12.0 * k))
                            .rounded_full()
                            .bg(p.primary.with_alpha(0.3).hsla())
                            .flex()
                            .items_center()
                            .gap(px(7.0 * k))
                            .child(div().size(px(7.0 * k)).rounded_full().bg(p.primary.with_lightness(0.85).hsla()))
                            .child(div().text_size(px(12.0 * k)).font_weight(FontWeight::SEMIBOLD).text_color(f.white.opacity(0.95)).child(tl("Login screen preview"))),
                    )
                    .child(
                        div()
                            .id("exit-demo")
                            .h(px(32.0 * k))
                            .px(px(14.0 * k))
                            .rounded_full()
                            .bg(gpui::white().opacity(0.16))
                            .shadow(rim(0.7))
                            .hover(|s| s.bg(gpui::white().opacity(0.28)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(7.0 * k))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.exit = true;
                                cx.notify();
                            }))
                            .child(icon("close", 14.0 * k, f.white.opacity(0.95)))
                            .child(div().text_size(px(12.5 * k)).font_weight(FontWeight::MEDIUM).text_color(f.white.opacity(0.95)).child(tl("Exit demo"))),
                    ),
            );
        }
        if let Some((text, _)) = &self.toast {
            root = root.child(
                div().absolute().bottom(px(120.0 * u)).left_0().w(px(vw)).flex().justify_center().child(
                    div()
                        .px(px(16.0 * u))
                        .py(px(9.0 * u))
                        .rounded_full()
                        .bg(p.surface_container.with_alpha(0.92).hsla())
                        .shadow(vec![soft_shadow(24.0 * u, 0.35, 8.0 * u)])
                        .text_size(px(13.0 * u))
                        .text_color(p.on_surface.hsla())
                        .child(text.clone()),
                ),
            );
        }
        // Dissolve into the session.
        let root = root.child(div().absolute().top_0().left_0().size_full().bg(gpui::black().opacity((success * success * 0.9).clamp(0.0, 1.0))));
        let build = now.elapsed();
        self.pacing.record(now, build);
        if self.entrance_at.is_some() {
            self.pacing_visible.record(now, build);
        }
        root
    }
}
