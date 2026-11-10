//! The login screen as a reusable GPUI view.
//!
//! One view renders the greeter from a [`GreeterConfig`] at any size:
//!
//! * **live** — full screen, talking to greetd (or the demo's fake daemon);
//! * **preview** — embedded in Mandara Studio at a scaled size with mock
//!   state; elements can be selected and dragged (snapping to the 9-point
//!   anchors), and edits are emitted as [`PreviewEvent`]s.
//!
//! Every element is placed by the config's layout (anchor, offset, scale
//! and z-order) and animated with springs: a staggered entrance (the avatar
//! pops with overshoot, the password pill unfolds), an idle state with a big
//! clock that wakes on any key or pointer move, rolling clock digits, a
//! breathing ring around the avatar, keystroke shapes that pop in with a
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
    RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Transformation, Window, canvas, div, img, point, prelude::FluentBuilder as _,
    px, svg,
};
use mandara_core::password_symbols::{self as symbols, Fades, Row, Symbol};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::fonts::{Fonts, tabular_nums};

use crate::auth::{AuthCommand, AuthEvent, AuthWorker};
use crate::background::{Background, BackgroundOptions};
use crate::config::{
    Anchor, AvatarShape, AvatarSource, BackgroundMode, CardStyle, ClockFace, Element, FieldStyle, GreeterConfig, Overlay, PasswordMask, PowerStyle, SessionStyle,
    SubmitButton, UserList, UserSwitcher, VideoMode, apply_playlist, list_images,
};
use mandara_core::avatar_shape;
use mandara_ui::components::motion::Feel;
use mandara_ui::components::tonal::Tonal;
use mandara_ui::components::{ButtonDesign, InputDesign, buttons, inputs};
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
/// The demo's top chrome (the preview pill and Exit demo), in reference
/// px: its inset from the screen edges and the height of its row.
const DEMO_CHROME_INSET: f32 = 22.0;
const DEMO_CHROME_HEIGHT: f32 = 32.0;
/// Space kept between the demo's chrome and an element moved below it.
const DEMO_CHROME_GAP: f32 = 12.0;
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
/// The person glyph without a plate fills more of its box.
const BARE_ICON_SCALE: f32 = 0.78;
/// The square sign-in button beside the field is this much of its height
/// in corner radius (times the theme's roundness).
const BESIDE_RADIUS: f32 = 0.22;
/// Gap between the field and the button beside it.
const BESIDE_GAP: f32 = 8.0;
/// Password field padding before the text and after the trailing buttons,
/// and the gap between them (times the layout scale).
const PILL_PAD_START: f32 = 16.0;
const PILL_PAD_END: f32 = 5.0;
const PILL_GAP: f32 = 6.0;
/// Gap between the reveal and submit buttons inside the field.
const TRAIL_GAP: f32 = 2.0;
/// Reveal button, submit button and spinner, as shares of the field height.
const REVEAL_SHARE: f32 = 0.62;
const SUBMIT_SHARE: f32 = 0.72;
const SPINNER_SHARE: f32 = 0.62;
/// The field's left and right borders, logical px (kept out of the text room).
const FIELD_BORDERS: f32 = 2.0;
/// Text caret: width (logical px) and height (share of the field height).
const CARET_W: f32 = 1.5;
const CARET_SHARE: f32 = 0.46;

/// Studio preview: how long background edits that rescan folders or reload
/// pictures (folders typed, the softening slider dragged) wait for the
/// edits to settle before the backdrop is rebuilt.
const BACKGROUND_REBUILD_DEBOUNCE: Duration = Duration::from_millis(300);
/// Studio preview: device-pixel ratio assumed before the first frame
/// reports the window's real one.
const PREVIEW_SCALE_GUESS: f32 = 1.5;

/// Size of the status line under the password field (px per unit scale).
const STATUS_TEXT_PX: f32 = 13.0;

// ── Material You (the scalloped avatar, filled field, chips) ────────────
/// Material's loading indicator: turn (radians per second), breathing
/// rate and its smallest size (share of the largest).
const LOADER_TURN: f32 = 4.0;
const LOADER_BREATH: f32 = 3.0;
const LOADER_REST: f32 = 0.8;
/// A dynamic-colour change cross-fades the palette over this long (OKLCH).
const PALETTE_FADE: Duration = Duration::from_millis(450);
/// The idle scalloped avatar turns this far per second (none while still).
const SPIN_DEG_PER_S: f32 = 6.0;
/// Each keystroke turns it a little further (a spring, radians).
const SPIN_PER_KEY: f32 = std::f32::consts::PI / 12.0;
/// How far toward a circle it morphs while checking, and on success.
const MORPH_BUSY: f32 = 0.6;
const MORPH_DONE: f32 = 1.0;
/// A picture or monogram inside the scalloped shape, share of its size.
const COOKIE_INNER: f32 = 0.84;
/// The variable clock weight moves in steps of this many units (each
/// weight is rasterized once).
const WEIGHT_STEP: f32 = 50.0;
/// Stacked clock lines, as a share of the digits' size.
const STACKED_LINE: f32 = 0.98;
/// The tinted hours' lightness floor (a light tone of the accent).
const TINT_LIGHTNESS: f32 = 0.84;
/// Material's corner sizes (px at the reference scale): chips and menu
/// items, fields' top corners, menus.
const M3_CHIP_RADIUS: f32 = 8.0;
const M3_FIELD_RADIUS: f32 = 16.0;
const M3_MENU_RADIUS: f32 = 16.0;
const M3_ITEM_RADIUS: f32 = 12.0;
/// Chip heights, menu item height (px at the reference scale).
const M3_CHIP_H: f32 = 32.0;
const M3_USER_CHIP_H: f32 = 36.0;
const M3_ITEM_H: f32 = 44.0;
/// A filled field's floating label size.
const M3_LABEL_PX: f32 = 11.5;
/// State layer over a container while hovered.
const M3_HOVER_LAYER: f32 = 0.08;
/// A filled field's container alpha over the card.
const M3_FIELD_ALPHA: f32 = 0.96;
/// The tonal fill of other controls in the filled style (keyboard pill,
/// soft buttons), alpha.
const M3_TONAL_ALPHA: f32 = 0.9;

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

/// A Material button that morphs while held.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Press {
    Submit,
    Power(PowerAction),
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

impl Mode {
    fn permits_weather_requests(self) -> bool {
        matches!(self, Self::Live { demo: false })
    }
}

/// Edits coming out of the Studio preview.
#[derive(Clone, Debug)]
#[expect(clippy::large_enum_variant, reason = "a handful per drag; the edited config is a fresh copy either way")]
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

mod chrome_halo;
mod ghostline;
#[cfg(test)]
mod look_tests;
/// Meridian's field and line, shared with Studio's look previews.
pub mod meridian;
mod preview_clock;

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

impl Roll {
    fn progress(&self, now: Instant) -> f32 {
        self.at.map(|at| pop_at(ROLL_SPRING, now.saturating_duration_since(at).as_secs_f32()).clamp(0.0, ROLL_OVERSHOOT_CAP)).unwrap_or(1.0)
    }

    /// Keep frames scheduled until every rolling digit reaches its destination.
    fn moving(&self, now: Instant) -> bool {
        self.progress(now) < 1.0 && self.prev.chars().count() == self.text.chars().count()
    }
}

pub struct GreeterView {
    mode: Mode,
    config: GreeterConfig,
    /// The palette drawn this frame (on its way to `palette_goal` while a
    /// dynamic-colour change cross-fades).
    palette: MandaraPalette,
    palette_goal: MandaraPalette,
    /// The palette a cross-fade started from, and when.
    palette_from: Option<(MandaraPalette, Instant)>,
    /// The leading colour of the picture behind (dynamic colour).
    seed: Option<Color>,
    users: Vec<UserEntry>,
    user_ix: usize,
    manual: Option<String>,
    sessions: Vec<Session>,
    session_ix: usize,
    session_menu: bool,
    /// Picks the mask shapes (fresh per entry, so they say nothing about
    /// the characters).
    mask_seed: u64,
    /// Deleted password symbols fading out.
    fades: Fades,
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
    widgets: mandara_ui::backdrop::widgets::passive::Canvas,
    /// The configured clips, stills and video switch, restored when the
    /// picked person has no wallpaper of their own.
    bg_base: (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>, bool),
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
    /// One visible-preview deadline; dropped when hidden or animating.
    preview_wake: Option<(bool, gpui::Task<()>)>,
    /// Formatted date and the (day, pattern, language) it was made for.
    date_text: Option<((chrono::NaiveDate, String, &'static str), SharedString)>,
    /// Keyboard layout label and the (configured, GPUI) ids it came from.
    kb_layout: Option<((String, String), SharedString)>,
    /// The ruler clock's and the wireframe card's readouts (made only for
    /// a look that shows them, refreshed once a minute).
    telemetry: Option<ghostline::Telemetry>,
    /// Minutes since local midnight (the ruler clock's marker).
    day_minute: u16,
    /// Meridian's words in capitals.
    words: Option<meridian::Words>,
    /// Where Meridian's caret was drawn (window x, set while painting) and
    /// last frame's value, which the line brightens around.
    horizon_caret: Rc<Cell<Option<f32>>>,
    caret_x: Option<f32>,
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
    /// The scalloped avatar: toward a circle (0 = scalloped), and its turn
    /// (radians) from keystrokes.
    morph: SpringValue<f32>,
    spin: SpringValue<f32>,
    /// The Material button being held (it squares off) and its press.
    pressed: Option<Press>,
    press: SpringValue<f32>,
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
    /// Stills bring their leading colour (dynamic colour).
    seed_colors: bool,
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
            seed_colors: cfg.theme.from_wallpaper,
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
    } else if old.mode != new.mode || old.video != new.video || old.solid != new.solid || old.reduced != new.reduced || old.seed_colors != new.seed_colors {
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

/// The theme for a config: the preset's palette (light when the screen
/// [draws light](GreeterConfig::draws_light), else dark) with the accent
/// override applied.
pub fn palette_for(cfg: &GreeterConfig) -> MandaraPalette {
    let dark = !cfg.draws_light();
    let p = mandara_theme::catalog::find(&cfg.theme.preset)
        .or_else(|| mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME))
        .map(|t| t.palette(dark))
        .unwrap_or_else(|| mandara_theme::engine::default_palette(dark));
    with_accent(cfg, p)
}

/// The theme for a config showing a picture whose leading colour is
/// `seed`: with `theme.from_wallpaper`, Material's dark tonal scheme made
/// from it; otherwise (or with no picture) [`palette_for`].
pub fn palette_for_seed(cfg: &GreeterConfig, seed: Option<Color>) -> MandaraPalette {
    match seed.filter(|_| cfg.theme.from_wallpaper) {
        Some(seed) => with_accent(cfg, mandara_theme::material::palette_from_seed(seed, mandara_theme::SchemeVariant::TonalSpot, !cfg.draws_light())),
        None => palette_for(cfg),
    }
}

/// Dark text over a light screen (the light colours).
const LIGHT_INK: (f32, f32, f32) = (0.23, 0.015, 255.0);

/// The colour text and hairlines are drawn in on the controls and cards,
/// whose surfaces follow the colours: white, or when the screen draws
/// light a deep ink.
pub fn foreground(cfg: &GreeterConfig) -> Color {
    if cfg.draws_light() { light_ink() } else { white_ink() }
}

fn white_ink() -> Color {
    Color::oklch(1.0, 0.0, 0.0)
}

fn light_ink() -> Color {
    Color::oklch(LIGHT_INK.0, LIGHT_INK.1, LIGHT_INK.2)
}

/// The colour of text straight on the backdrop (the clock, the date, a
/// name without a card, the logo): it follows the backdrop rather than
/// the colours. The aurora, pictures and footage are dark under their
/// scrims, so they take white; a solid floor (`solid`, resolved) takes
/// whichever ink reads better on it once dimmed; Meridian's field follows
/// the colours.
fn ground_ink(cfg: &GreeterConfig, solid: Option<Color>) -> Color {
    if cfg.background.overlay == Overlay::Horizon {
        return foreground(cfg);
    }
    let Some(floor) = solid.filter(|_| cfg.background.mode == BackgroundMode::Solid) else { return white_ink() };
    let ground = Color::oklcha(0.0, 0.0, 0.0, cfg.background.dim.clamp(0.0, 0.95)).over(floor);
    let dark = light_ink();
    let white = white_ink();
    if dark.contrast_ratio(ground) > white.contrast_ratio(ground) { dark } else { white }
}

/// Whether the clock face (or the line it sits on) draws the date itself,
/// so the date element stays empty: the ruler and halo faces always, the
/// vast face when Meridian's line is the card and puts it at its end.
pub(crate) fn face_holds_date(cfg: &GreeterConfig) -> bool {
    match cfg.clock.face {
        ClockFace::Ruler | ClockFace::Halo => true,
        ClockFace::Vast => cfg.card.enabled && cfg.card.style == CardStyle::Horizon,
        ClockFace::Digits => false,
    }
}

/// `p` with the config's accent override, if any.
fn with_accent(cfg: &GreeterConfig, mut p: MandaraPalette) -> MandaraPalette {
    if let Ok(accent) = Color::from_hex(cfg.theme.accent.trim()) {
        p.primary = accent;
        p.primary_glow = accent.with_alpha(0.5);
        p.accent_gradient_start = accent;
        p.accent_gradient_end = accent.rotate_hue(25.0).scale_chroma(1.15);
    }
    p
}

/// A Material motion token as a greeter spring (snapping with reduced
/// motion).
fn feel(f: Feel, reduced: bool) -> SpringConfig {
    let (ratio, stiffness) = f.tokens();
    spring(crate::config::Spring::new(stiffness, 2.0 * ratio * stiffness.sqrt()), reduced)
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

#[cfg(test)]
mod preview_clock_tests {
    use super::*;

    #[test]
    fn rolling_clock_digits_keep_preview_frames_active() {
        let start = Instant::now();
        let roll = Roll { text: "12:10".into(), prev: "12:09".into(), at: Some(start), ..Default::default() };
        assert!(roll.moving(start));
        assert!(roll.moving(start + Duration::from_millis(16)));
        assert!(!roll.moving(start + Duration::from_secs(10)));
        assert!(!Roll::default().moving(start));
    }

    #[test]
    fn previews_and_full_screen_demos_do_not_request_weather() {
        assert!(!Mode::Preview.permits_weather_requests());
        assert!(!Mode::Live { demo: true }.permits_weather_requests());
        assert!(Mode::Live { demo: false }.permits_weather_requests());
    }
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

    /// Initialize one live or preview view with independent session and animation state.
    fn build(mode: Mode, mut config: GreeterConfig, target: (u32, u32), cx: &mut Context<Self>) -> Self {
        config.normalize();
        let demo = !matches!(mode, Mode::Live { demo: false });
        let lang = mandara_i18n::language();
        // The demo and the preview show only you; the real greeter lists
        // every login user (minus hidden ones).
        let all: Vec<User> = if demo { crate::users::current_user().into_iter().collect() } else { list_users() };
        let mut users: Vec<User> = all.into_iter().filter(|u| !config.user.hidden.contains(&u.name)).collect();
        let state = GreeterState::load(&state_path(demo));
        // The colour scheme picked with the switcher last time.
        if config.placement(Element::Themes).visible
            && let Some(theme) = state.theme.clone().filter(|t| theme_choices(&config).contains(t))
        {
            config.theme.preset = theme;
        }
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
            palette_goal: p.clone(),
            palette_from: None,
            seed: None,
            palette: p,
            users,
            user_ix,
            field: if manual.is_some() { Field::Username } else { Field::Password },
            manual,
            sessions,
            session_ix,
            session_menu: false,
            mask_seed: symbols::seed_from_clock(),
            fades: Fades::default(),
            state,
            password: String::new(),
            reveal: false,
            prompt: None,
            phase: Phase::Idle,
            status: Status::None,
            focus: cx.focus_handle(),
            auth: None,
            bg_base: owned_sources(&background),
            background,
            widgets: mandara_ui::backdrop::widgets::passive::Canvas::for_images(),
            bg_key: BackgroundSources::of(&config),
            bg_rebuild_at: None,
            clock: FrameClock::new(),
            started: Instant::now(),
            entrance_at: None,
            warm_frames: 0,
            first_frame_at: None,
            clock_second: i64::MIN,
            preview_wake: None,
            date_text: None,
            kb_layout: None,
            telemetry: None,
            day_minute: 0,
            words: None,
            horizon_caret: Rc::new(Cell::new(None)),
            caret_x: None,
            enter,
            pop: SpringValue::new(if m.entrance { POP_FROM } else { 1.0 }, spring(m.pop_spring, reduced)),
            wake,
            awake,
            last_activity: Instant::now(),
            shake: SpringValue::new(0.0, spring(m.shake_spring, reduced)),
            flash_at: None,
            success: SpringValue::new(0.0, spring(m.success_spring, reduced)),
            glow: SpringValue::new(0.0, fast),
            morph: SpringValue::new(0.0, feel(Feel::DefaultSpatial, reduced)),
            spin: SpringValue::new(0.0, feel(Feel::FastSpatial, reduced)),
            pressed: None,
            press: SpringValue::new(0.0, feel(Feel::FastSpatial, reduced)),
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
        this.follow_user_wallpaper();
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
        // Studio's preview holds still: one picture, no clip, no drift (a
        // scaled-down moving backdrop inside Studio judders). A video
        // backdrop shows your wallpaper instead.
        if preview {
            if mode == BackgroundMode::Video && stills.is_empty() {
                stills.extend(own_wallpaper());
            }
            stills.truncate(1);
            clips.clear();
        }
        tracing::info!("greeter background: {:?}, {} clips, {} stills", mode, clips.len(), stills.len());
        let solid = (mode == BackgroundMode::Solid).then(|| Color::from_hex(b.solid.trim()).unwrap_or_else(|_| palette_for(config).surface_dim));
        Background::new(
            BackgroundOptions {
                clips,
                stills,
                target,
                lite: b.video == VideoMode::Lite || preview,
                video_enabled: b.video != VideoMode::Off && !preview,
                blur: b.blur,
                solid,
                still_motion: config.accessibility.reduced_motion || preview,
                seed_colors: config.theme.from_wallpaper,
                ..background_timing(config)
            },
            b.shuffle && b.playlist.is_empty(),
        )
    }

    /// Rebuilds the backdrop from the current config (preview).
    fn rebuild_background(&mut self) {
        let demo = !matches!(self.mode, Mode::Live { demo: false });
        self.background = Self::make_background(&self.config, demo, self.is_preview(), self.background.target());
        self.bg_base = owned_sources(&self.background);
        self.bg_key = BackgroundSources::of(&self.config);
        self.bg_rebuild_at = None;
        self.follow_user_wallpaper();
    }

    /// The picked person's own wallpaper, when the login screen shows it:
    /// the copy their session shared (see
    /// [`mandara_config::paths::shared_login_wallpaper`]), or in the demo
    /// and Studio's preview your current wallpaper.
    fn user_wallpaper(&self) -> Option<std::path::PathBuf> {
        if !self.config.background.user_wallpaper || self.manual.is_some() {
            return None;
        }
        if !matches!(self.mode, Mode::Live { demo: false }) {
            return own_wallpaper();
        }
        let entry = self.users.get(self.user_ix)?;
        mandara_config::paths::shared_login_wallpaper(std::path::Path::new(mandara_config::paths::LOGIN_WALLPAPER_DIR), &entry.user.name, entry.user.uid)
    }

    /// Shows the picked person's wallpaper, or the configured backdrop when
    /// they have none (crossfading either way).
    fn follow_user_wallpaper(&mut self) {
        match self.user_wallpaper() {
            Some(path) => self.background.set_sources(Vec::new(), vec![path], false),
            None => {
                let (clips, stills, video) = self.bg_base.clone();
                self.background.set_sources(clips, stills, video);
            }
        }
    }

    /// Moves to palette `p`: at once, or (`fade`) cross-fading from the
    /// one on screen in OKLCH over [`PALETTE_FADE`].
    fn set_palette(&mut self, p: MandaraPalette, fade: bool) {
        self.palette_from = (fade && p != self.palette_goal).then(|| (self.palette.clone(), Instant::now()));
        if self.palette_from.is_none() {
            self.palette = p.clone();
        }
        self.palette_goal = p;
    }

    /// Dynamic colour: follows the picture behind (its leading colour) and
    /// advances a palette cross-fade.
    fn follow_seed(&mut self, now: Instant) {
        let seed = if self.config.theme.from_wallpaper { self.background.seed() } else { None };
        if seed != self.seed {
            self.seed = seed;
            // The first picture's colours are there from the entrance on;
            // later ones (another person's wallpaper) cross-fade.
            let fade = self.entrance_at.is_some() && !self.config.accessibility.reduced_motion;
            self.set_palette(palette_for_seed(&self.config, seed), fade);
        }
        if let Some((from, at)) = &self.palette_from {
            let t = now.saturating_duration_since(*at).as_secs_f32() / PALETTE_FADE.as_secs_f32();
            if t >= 1.0 {
                self.palette = self.palette_goal.clone();
                self.palette_from = None;
            } else {
                self.palette = from.lerp(&self.palette_goal, t * t * (3.0 - 2.0 * t));
            }
        }
    }

    /// Holds a Material button (it squares off) until [`Self::release`].
    fn hold(&mut self, which: Press) {
        if self.pressed != Some(which) {
            self.press.snap_to(0.0);
        }
        self.pressed = Some(which);
        self.press.set_target(1.0);
    }

    fn release(&mut self) {
        self.press.set_target(0.0);
    }

    /// How far `which` is pressed (0 at rest; past 1 while springing).
    fn pressed(&self, which: Press) -> f32 {
        if self.pressed == Some(which) { self.press.value() } else { 0.0 }
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
        for (s, f) in [(&mut self.morph, Feel::DefaultSpatial), (&mut self.spin, Feel::FastSpatial), (&mut self.press, Feel::FastSpatial)] {
            s.set_config(feel(f, reduced));
        }
        let seed = if config.theme.from_wallpaper { self.seed } else { None };
        self.config = config;
        self.set_palette(palette_for_seed(&self.config, seed), false);
        if reduced || !self.config.clock.roll_digits { self.roll.at = None; }
        match change {
            BackgroundChange::Same => self.bg_rebuild_at = None,
            BackgroundChange::Rebuild => self.rebuild_background(),
            BackgroundChange::RebuildSettled => self.bg_rebuild_at = Some(Instant::now() + BACKGROUND_REBUILD_DEBOUNCE),
        }
        cx.notify();
    }

    pub fn set_preview_size(&mut self, w: f32, h: f32, cx: &mut Context<Self>) {
        let size = (w.max(64.0), h.max(36.0));
        if size != self.preview_size {
            // The preview holds still, so a new size needs a redraw.
            self.preview_size = size;
            cx.notify();
        }
    }

    /// Every picture a preview drew (backdrop, widgets, avatars, the shared
    /// grain), for its host to drop from the atlas right before it drops
    /// the view (avatars are not loaded again).
    pub fn take_images(&mut self) -> Vec<Arc<RenderImage>> {
        let mut images = self.background.take_images();
        self.widgets.clear();
        images.extend(self.widgets.take_trash());
        images.extend(self.users.iter_mut().filter_map(|u| u.avatar.take()));
        images.extend(crate::background::take_grain());
        images
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
                self.clear_dots();
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
                self.clear_dots();
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
        self.clear_dots();
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
        self.clear_dots();
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
                self.clear_dots();
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
        self.clear_dots();
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
        self.follow_user_wallpaper();
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
        let on_the_line = self.config.card.enabled && self.config.card.style == CardStyle::Horizon;
        if (self.visible(Element::Users) || on_the_line) && (self.users.len() > 1 || (!self.demo() && self.config.user.list != UserList::Manual)) {
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

    /// The password field emptied: no symbols, and no fades either.
    fn clear_dots(&mut self) {
        self.dots.clear();
        self.fades.clear();
    }

    fn type_char(&mut self, ch: &str) {
        let pw = self.field != Field::Username;
        let text = if pw { &mut self.password } else { self.manual.get_or_insert_with(String::new) };
        if text.len() < 512 {
            text.push_str(ch);
            if pw {
                if self.dots.is_empty() {
                    // A new entry: a new sequence of shapes.
                    self.mask_seed = symbols::next_seed(self.mask_seed);
                }
                self.fades.typed(self.dots.len());
                self.dots.push(Instant::now());
                self.ripple_at = Some(Instant::now());
                if self.config.user.avatar_shape == AvatarShape::Cookie || self.config.clock.face == ClockFace::Halo {
                    let turn = self.spin.target() + SPIN_PER_KEY;
                    self.spin.set_target(turn);
                }
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
                self.clear_dots();
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
                                self.clear_dots();
                            }
                        } else {
                            text.pop();
                            if pw && self.dots.pop().is_some() {
                                self.fades.push(self.dots.len(), Instant::now());
                            }
                        }
                    }
                    "u" if k.modifiers.control => {
                        wipe(&mut self.password);
                        self.clear_dots();
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

/// The demo's chrome fills both top corners (the preview pill on one side,
/// Exit demo on the other, swapped in right-to-left languages), so an
/// element anchored to a top corner sits below it instead of under it.
/// `y` is the element's offset from the top edge in reference px; other
/// anchors, and a real login screen, keep it as it is.
fn clear_demo_chrome(anchor: Anchor, y: f32) -> f32 {
    match anchor {
        Anchor::TopLeft | Anchor::TopRight => y.max(DEMO_CHROME_INSET + DEMO_CHROME_HEIGHT + DEMO_CHROME_GAP),
        _ => y,
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

/// Schemes the switcher offers: the configured ones that exist, else a
/// handful of Mandara's own.
fn theme_choices(cfg: &GreeterConfig) -> Vec<String> {
    const DEFAULTS: [&str; 5] = ["night-citrine", "montaza-twilight", "mandara-corniche", "mandara-obsidian", "catppuccin"];
    let listed: Vec<String> = cfg.theme.switcher.iter().filter(|t| mandara_theme::catalog::find(t).is_some()).cloned().collect();
    if !listed.is_empty() {
        return listed;
    }
    DEFAULTS.iter().filter(|t| mandara_theme::catalog::find(t).is_some()).map(|t| t.to_string()).collect()
}

/// A backdrop's sources, owned.
fn owned_sources(b: &Background) -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>, bool) {
    let (clips, stills, video) = b.sources();
    (clips.to_vec(), stills.to_vec(), video)
}

/// Your current desktop wallpaper (demo and preview run as you): a still
/// image from `~/.config/mandara/config.toml`, not a video.
fn own_wallpaper() -> Option<std::path::PathBuf> {
    let text = std::fs::read_to_string(mandara_config::paths::config_file()).ok()?;
    let cfg: mandara_config::ShellConfig = toml::from_str(&text).ok()?;
    // The one the shell shows (and shares with the login screen): the
    // light / dark variant when there is one.
    let path = mandara_config::desktop::effective_wallpaper(cfg.wallpaper.path.as_ref(), &cfg.wallpaper.variants, None, cfg.theme.mode.is_dark()).filter(|p| p.is_file())?;
    (!mandara_media::is_video(&path)).then_some(path)
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

/// The clock's weight `idle` of the way from awake (`awake`) to the idle
/// weight (unsnapped).
fn clock_weight(awake: u16, idle_weight: u16, idle: f32) -> f32 {
    let (a, b) = (f32::from(awake), f32::from(idle_weight));
    a + (b - a) * idle.clamp(0.0, 1.0)
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
    /// Ink on the controls and cards ([`foreground`]).
    white: Hsla,
    /// Ink straight on the backdrop ([`ground_ink`]).
    ground: Hsla,
    ui_font: SharedString,
    display_font: SharedString,
    mono_font: SharedString,
    rtl: bool,
    /// Device pixels per logical pixel.
    scale: f32,
    success: f32,
    reduced: bool,
    /// No ambient motion (drifting orbs, parallax, breathing glow): reduced
    /// motion, or Studio's preview, which holds still between changes.
    still: bool,
    high_contrast: bool,
    t: f32,
    /// Material's tonal roles of this frame's palette.
    tonal: Tonal,
    /// The keyboard layout's label.
    keyboard: SharedString,
    /// Caps Lock is on (the live login screen only).
    caps: bool,
}

impl Frame {
    /// Control fill for the configured style.
    fn fill(&self, cfg: &GreeterConfig, p: &MandaraPalette, focused: bool) -> Hsla {
        if self.high_contrast {
            return gpui::black().opacity(if focused { 0.72 } else { 0.6 });
        }
        match cfg.password.style {
            FieldStyle::Solid => p.surface_container.with_alpha(if focused { 0.96 } else { 0.9 }).hsla(),
            FieldStyle::Inset => p.surface_dim.with_alpha(if focused { 0.98 } else { 0.92 }).hsla(),
            FieldStyle::Filled => self.tonal.surface_highest.with_alpha(if focused { M3_FIELD_ALPHA } else { M3_TONAL_ALPHA }).hsla(),
            FieldStyle::Terminal => ghostline::field_fill(focused),
            FieldStyle::Recessed => chrome_halo::field_fill(p, focused),
            // Frosted white over light colours.
            _ if cfg.draws_light() => gpui::white().opacity((0.5 + cfg.theme.glass + if focused { 0.12 } else { 0.0 }).clamp(0.0, 1.0)),
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
        let font = self.clock_font(f);
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
        !window.is_visible() || top + self.preview_size.1 < 0.0 || top > height
    }

    /// Schedules the next frame, always on the display's frame clock (a
    /// timer-driven cadence beats against the refresh rate and judders):
    /// every frame on the login screen and in a visible Studio preview,
    /// where the page around it is cached so a frame costs only the
    /// preview; nothing while the preview is scrolled out of view (moving
    /// it back re-renders it, which resumes).
    ///
    /// The preview holds still between changes: it asks for frames only
    /// while something it shows is moving (a Try-it moment, an edit's
    /// springs, a picture fading in), and the next edit or click wakes it.
    fn schedule_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_preview() && self.offscreen(window) {
            self.preview_wake = None;
            self.clock.idle();
            return;
        }
        if self.is_preview() && !self.preview_moving(Instant::now()) {
            let seconds = self.config.clock.seconds || mandara_ui::backdrop::widgets::passive::ticks_seconds(&self.config.widgets);
            if let Some(wait) = preview_clock::plan(chrono::Local::now().timestamp_millis(), seconds, self.preview_wake.as_ref().map(|(seconds, _)| *seconds)) {
                self.preview_wake = Some((seconds, cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.preview_wake = None;
                        if !this.offscreen(window) { cx.notify(); }
                    });
                })));
            }
            // Restart the clock cleanly after the pause (no catch-up jump).
            self.clock.idle();
            return;
        }
        self.preview_wake = None;
        window.request_animation_frame();
    }

    /// Preview only: whether anything on it is still moving.
    fn preview_moving(&self, now: Instant) -> bool {
        let springs = self.enter.iter().chain([&self.pop, &self.wake, &self.shake, &self.success, &self.glow, &self.morph, &self.spin, &self.press, &self.pointer.0, &self.pointer.1]).any(|s| s.is_animating());
        let m = &self.config.motion;
        let entrance_s = ENTRANCE_LEAD_S + Element::ALL.len() as f32 * m.stagger_ms as f32 / MS_PER_S;
        let entering = self.entrance_at.is_some_and(|at| now.saturating_duration_since(at).as_secs_f32() < entrance_s);
        springs
            || self.palette_from.is_some()
            || self.roll.moving(now)
            || entering
            || self.sim_reset.is_some()
            || self.phase != Phase::Idle
            || self.toast.is_some()
            || self.drag.is_some()
            || self.bg_rebuild_at.is_some()
            || self.widgets.pending()
            || !self.background.ready()
            || self.background.fading(now)
    }

    fn tick(&mut self, now: Instant, dt: f32) {
        // Staggered entrance.
        let m = &self.config.motion;
        // Nothing moves until the warm-up is over.
        if let Some(entrance_at) = self.entrance_at {
            let since = now.saturating_duration_since(entrance_at).as_secs_f32();
            let stagger = m.stagger_ms as f32 / MS_PER_S;
            let order = [Element::Date, Element::Clock, Element::User, Element::Password, Element::Users, Element::Session, Element::Power, Element::Keyboard, Element::Banner, Element::Logo, Element::Themes];
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
        self.morph.set_target(match self.phase {
            Phase::Idle => 0.0,
            Phase::Busy => MORPH_BUSY,
            Phase::Success(_) => MORPH_DONE,
        });
        self.morph.step(dt);
        self.spin.step(dt);
        self.press.step(dt);
        self.pointer.0.step(dt);
        self.pointer.1.step(dt);
        // Rolling digits (formatted only when the second changes).
        let local = chrono::Local::now();
        let second = local.timestamp();
        if second != self.clock_second {
            self.clock_second = second;
            use chrono::Timelike as _;
            self.day_minute = (local.hour() * 60 + local.minute()) as u16;
            if ghostline::shows_telemetry(&self.config) {
                ghostline::Telemetry::refresh(&mut self.telemetry, &local);
            } else {
                self.telemetry = None;
            }
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
        self.refresh_words();
        // Preview simulations end by themselves.
        if let Some(at) = self.sim_reset
            && now >= at
        {
            self.sim_reset = None;
            self.password.clear();
            self.clear_dots();
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
        let y = if self.demo() { clear_demo_chrome(pl.anchor, pl.y) } else { pl.y };
        let mut dx = pl.x * f.u;
        let mut dy = y * f.u + 18.0 * f.u * (1.0 - enter.clamp(0.0, 1.2));
        // Meridian's line sits at the golden height, whatever the screen.
        if e == Element::User && self.config.card.enabled && self.config.card.style == CardStyle::Horizon {
            dy += (meridian::GOLDEN - fy) * f.vh;
        }
        let mut alpha = enter.clamp(0.0, 1.0);
        if e.wakes() {
            alpha *= wake;
            dy += 30.0 * f.u * (1.0 - wake);
        } else if fy < 0.25 && self.config.clock.face == ClockFace::Digits {
            // Idle: the clock and date drift toward the middle of the screen
            // (the ruler and halo faces are composed where they stand).
            dy += f.vh * 0.2 * (1.0 - wake);
        }
        alpha *= 1.0 - f.success;
        // Pointer parallax (foreground moves with the pointer).
        if !f.still {
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

    /// Use the same roll progress for painting digits and deciding when motion ends.
    fn clock_el(&self, f: &Frame, k: f32) -> AnyElement {
        let c = &self.config.clock;
        let idle = 1.0 - self.wake.value().clamp(0.0, 1.0);
        let size = snap_size(c.size * k * (1.0 + CLOCK_IDLE_GROWTH * idle) * (1.0 + CLOCK_SUCCESS_GROWTH * f.success));
        let line = size * if c.stacked { STACKED_LINE } else { 1.08 };
        let font = self.clock_font(f);
        let progress = self.roll.progress(f.now);
        let rolling = self.roll.moving(f.now);
        // The variable weight follows the idle state in steps (each weight
        // is rasterized once).
        let w = if c.idle_weight >= 100 { (clock_weight(c.weight, c.idle_weight, idle) / WEIGHT_STEP).round() * WEIGHT_STEP } else { c.weight as f32 };
        let tint = c.tint.then(|| {
            let accent = self.palette.primary;
            accent.with_lightness(accent.lightness().max(TINT_LIGHTNESS)).with_alpha(c.opacity).hsla()
        });
        let new_row = || {
            div().flex().font_family(font.clone()).font_features(tabular_nums()).text_size(px(size)).line_height(px(line)).font_weight(FontWeight(w)).text_color(f.ground.opacity(c.opacity))
        };
        let mut lines: smallvec::SmallVec<[gpui::Div; 3]> = smallvec::SmallVec::new();
        let mut row = new_row();
        // Hours, minutes, seconds: the colons split them.
        let mut segment = 0;
        let mut prev = self.roll.prev.chars();
        for ch in self.roll.text.chars() {
            let old = prev.next().filter(|o| rolling && *o != ch);
            if ch == ':' {
                segment += 1;
                if c.stacked {
                    lines.push(std::mem::replace(&mut row, new_row()));
                    continue;
                }
            }
            let cell = div().relative().h(px(line)).overflow_hidden();
            let cell = match old {
                Some(o) => cell
                    .child(div().relative().top(px(line * (1.0 - progress))).opacity(progress.clamp(0.0, 1.0)).child(shadowed(glyph_str(ch), CLOCK_SHADOW_DY * k, CLOCK_SHADOW_ALPHA)))
                    .child(div().absolute().left_0().top(px(-line * progress)).opacity((1.0 - progress).clamp(0.0, 1.0)).child(shadowed(glyph_str(o), CLOCK_SHADOW_DY * k, CLOCK_SHADOW_ALPHA))),
                None => cell.child(shadowed(glyph_str(ch), CLOCK_SHADOW_DY * k, CLOCK_SHADOW_ALPHA)),
            };
            row = row.child(match tint {
                Some(t) if segment == 0 => cell.text_color(t),
                _ => cell,
            });
        }
        if !c.stacked {
            return row.into_any_element();
        }
        lines.push(row);
        div().flex().flex_col().items_center().children(lines).into_any_element()
    }

    /// The clock's family: the configured one, else monospace for the
    /// ruler face and the display face for the others.
    fn clock_font(&self, f: &Frame) -> SharedString {
        let c = &self.config.clock;
        match c.font.trim() {
            "" if c.face == ClockFace::Ruler => f.mono_font.clone(),
            "" => f.display_font.clone(),
            family => SharedString::from(family.to_string()),
        }
    }

    fn date_el(&self, f: &Frame, k: f32) -> AnyElement {
        let d = &self.config.date;
        let text = self.date_text.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
        let font = if d.font.trim().is_empty() { f.ui_font.clone() } else { SharedString::from(d.font.clone()) };
        div().font_family(font).text_size(px(d.size * k)).font_weight(weight(d.weight)).text_color(f.ground.opacity(d.opacity)).child(shadowed(text, 1.5 * k, 0.3)).into_any_element()
    }

    /// Width of the password field (and a full-width session picker): the
    /// card's inner width when it holds them, else the configured width.
    fn field_width(&self, k: f32) -> f32 {
        let c = &self.config.card;
        if c.enabled { (c.width - c.padding * 2.0).max(120.0) * k } else { self.config.password.width * k }
    }

    fn avatar_radius(&self, d: f32) -> f32 {
        match self.config.user.avatar_shape {
            AvatarShape::Circle => d / 2.0,
            AvatarShape::Squircle => d * 0.32,
            AvatarShape::Rounded => d * 0.18,
            AvatarShape::Bare => d / 2.0,
            // A picture inside the scalloped shape is round.
            AvatarShape::Cookie => d / 2.0,
        }
    }

    /// The scalloped avatar (Material's twelve-sided cookie): a tonal shape
    /// that turns slowly and with each keystroke, and morphs into a circle
    /// while signing in; the picture sits round inside it, a monogram or
    /// the person icon straight on it.
    fn cookie_avatar(&self, entry: Option<&UserEntry>, d: f32, f: &Frame) -> AnyElement {
        let (lower, upper, blend) = avatar_shape::between(self.morph.value());
        let drift = if f.still { 0.0 } else { (f.t * SPIN_DEG_PER_S).to_radians() };
        let turn = Transformation::rotate(gpui::radians(drift + self.spin.value()));
        let ink = f.tonal.primary_container.hsla();
        let shape = |step: avatar_shape::Step, opacity: f32| svg().path(step.asset_path()).absolute().top_0().left_0().size(px(d)).text_color(ink.opacity(opacity)).with_transformation(turn);
        let inner = d * COOKIE_INNER;
        let content: AnyElement = match entry {
            Some(UserEntry { avatar: Some(image), .. }) if self.config.user.avatar_source == AvatarSource::Auto => {
                img(ImageSource::Render(image.clone())).size(px(inner)).rounded_full().object_fit(ObjectFit::Cover).into_any_element()
            }
            Some(UserEntry { user, .. }) if self.config.user.avatar_source != AvatarSource::Icon => div()
                .font_family(f.display_font.clone())
                .text_size(px(snap_size(d * INITIALS_SCALE)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(f.tonal.on_primary_container.hsla())
                .child(user.initials())
                .into_any_element(),
            _ => icon("user", snap_size(d * USER_ICON_SCALE), f.tonal.on_primary_container.hsla()).into_any_element(),
        };
        div()
            .relative()
            .size(px(d))
            .flex()
            .items_center()
            .justify_center()
            .child(shape(lower, 1.0))
            .when(blend > 0.0, |el| el.child(shape(upper, blend)))
            .child(div().relative().flex().items_center().justify_center().child(content))
            .into_any_element()
    }

    fn avatar_el(&self, entry: Option<&UserEntry>, d: f32, f: &Frame) -> AnyElement {
        if self.config.user.avatar_shape == AvatarShape::Cookie {
            return self.cookie_avatar(entry, d, f);
        }
        let p = &self.palette;
        let r = self.avatar_radius(d);
        let rimmed = rim(self.config.theme.rim.max(0.5));
        let bare = self.config.user.avatar_shape == AvatarShape::Bare;
        match entry {
            Some(UserEntry { avatar: Some(image), .. }) if self.config.user.avatar_source == AvatarSource::Auto => {
                img(ImageSource::Render(image.clone())).size(px(d)).rounded(px(r)).object_fit(ObjectFit::Cover).into_any_element()
            }
            Some(UserEntry { user, .. }) if self.config.user.avatar_source != AvatarSource::Icon => div()
                .size(px(d))
                .rounded(px(r))
                .when(!bare, |el| el.bg(oklab_gradient(145.0, p.accent_gradient_start.with_lightness(0.66), p.accent_gradient_end.with_lightness(0.42))).shadow(rimmed.clone()))
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
                .when(!bare, |el| el.bg(gpui::white().opacity(0.18)).shadow(rimmed))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("user", snap_size(d * if bare { BARE_ICON_SCALE } else { USER_ICON_SCALE }), gpui::white().opacity(0.92)))
                .into_any_element(),
        }
    }

    fn user_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let u = &self.config.user;
        let entry = if self.manual.is_none() { self.users.get(self.user_ix) } else { None };
        let grow = 1.0 + 0.5 * f.success;
        let d = u.avatar_size * k * self.pop.value().max(0.0) * grow;
        let ring_w = 2.5 * k;
        let breathing = u.glow_ring && self.password.is_empty() && !f.still;
        let b = if breathing { (f.t * 1.25).sin() * 0.5 + 0.5 } else { 0.4 };
        let accent = self.palette.primary;
        let ring_r = self.avatar_radius(d) + ring_w * 2.0;
        let mut ring = div().p(px(ring_w * 1.6)).rounded(px(ring_r));
        // The scalloped shape is its own frame: no ring or plate shadow.
        let framed = !matches!(u.avatar_shape, AvatarShape::Bare | AvatarShape::Cookie);
        if u.glow_ring && framed {
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
        } else if framed {
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
                        .text_color(if self.config.card.enabled { f.white } else { f.ground }.opacity(0.96))
                        .child(shadowed(name, 1.5 * k, 0.35)),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn pill(&self, f: &Frame, k: f32, id: &'static str, text: String, secret: bool, placeholder: String, focused: bool, unfold: f32, cx: &mut Context<Self>) -> AnyElement {
        if self.config.password.style == FieldStyle::Terminal {
            return self.terminal_field(f, k, id, text, secret, placeholder, focused, unfold, cx);
        }
        let cfg = &self.config;
        let pc = &cfg.password;
        let p = &self.palette;
        let white = f.white;
        let h = pc.height * k;
        let is_pw = id == "password";
        let beside = is_pw && pc.submit == SubmitButton::Beside;
        let full_w = self.field_width(k) - if beside { h + BESIDE_GAP * k } else { 0.0 };
        let w = h + (full_w - h) * unfold.clamp(0.0, 1.0);
        let radius = (h / 2.0) * cfg.theme.roundness;
        let busy = self.phase == Phase::Busy && is_pw;
        let empty = text.is_empty();
        let caret_on = focused && !busy && !self.is_preview() && ((self.started.elapsed().as_millis() / 530).is_multiple_of(2) || self.last_key.elapsed() < CARET_HOLD);
        let field = if is_pw { Field::Password } else { Field::Username };
        // Room for the text, left of the trailing buttons. Their box keeps
        // one width whether it holds the buttons or the spinner, so the room
        // (and every symbol's slot) holds still while typing and checking.
        let reveal_w = if pc.reveal_toggle { h * REVEAL_SHARE } else { 0.0 };
        let submit_w = if pc.submit == SubmitButton::Inside { h * SUBMIT_SHARE } else { 0.0 };
        let buttons_w = reveal_w + submit_w + if reveal_w > 0.0 && submit_w > 0.0 { TRAIL_GAP * k } else { 0.0 };
        let trail_w = buttons_w.max(h * SPINNER_SHARE);
        let has_trailing = busy || (is_pw && !empty);
        let room = (w - (PILL_PAD_START + PILL_PAD_END) * k - FIELD_BORDERS - if has_trailing { PILL_GAP * k + trail_w } else { 0.0 }).max(0.0);
        let caret = |el: gpui::Div, before: bool| {
            el.when(caret_on, |el| {
                let c = div().w(px(CARET_W)).h(px(h * CARET_SHARE)).rounded_full().bg(white.opacity(0.95));
                el.child(if before { c.mr(px(4.0 * k)) } else { c.ml(px(4.0 * k)) })
            })
        };
        let filled = pc.style == FieldStyle::Filled;
        // Material's filled field: once it holds text, the hint floats up
        // as a small label over it.
        let label = (filled && !empty).then(|| placeholder.clone());
        let inner = if secret && !empty && !(self.reveal && is_pw) {
            self.symbol_row(f, k, h, room, &text, is_pw, caret_on)
        } else {
            let content = if empty { div().text_color(white.opacity(0.72)).child(placeholder) } else { div().text_color(white.opacity(0.96)).child(text) };
            let inner = div().flex_1().flex().items_center().overflow_hidden().font_family(f.ui_font.clone()).text_size(px(pc.text_size * k)).when(f.rtl, |el| el.flex_row_reverse());
            if empty { caret(inner, true).child(content) } else { caret(inner.child(content), false) }.into_any_element()
        };
        let inner = match label {
            Some(label) => div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .when(f.rtl, |el| el.items_end())
                .child(div().font_family(f.ui_font.clone()).text_size(px(M3_LABEL_PX * k)).line_height(px(M3_LABEL_PX * k * 1.3)).text_color(if focused { p.primary.hsla() } else { p.on_surface_variant.hsla() }).child(label))
                .child(inner)
                .into_any_element(),
            None => inner,
        };
        let trailing: Option<AnyElement> = if busy && filled {
            Some(loading_indicator(h * SPINNER_SHARE, self.started.elapsed().as_secs_f32(), p.primary.hsla(), f.still))
        } else if busy {
            Some(spinner(h * SPINNER_SHARE, k, self.started.elapsed().as_secs_f32()))
        } else if is_pw && !empty {
            let inside = pc.submit == SubmitButton::Inside;
            let reveal = pc.reveal_toggle.then(|| {
                div()
                    .id("reveal")
                    .size(px(h * REVEAL_SHARE))
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
                    .gap(px(TRAIL_GAP * k))
                    .children(reveal)
                    .when(inside, |el| el.child(
                        div()
                            .id("submit")
                            .size(px(h * SUBMIT_SHARE))
                            .rounded_full()
                            .bg(p.primary.with_alpha(0.9).hsla())
                            .hover(|s| s.bg(p.primary.hsla()))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))
                            .child(icon(if f.rtl { "arrow-left" } else { "arrow-right" }, h * 0.4, p.on_primary.hsla())),
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };
        // Shadows: rim, depth, typing glow, keystroke ripple.
        // (The filled field is flat: Material shows state with its
        // indicator line, not glows.)
        let mut shadows = match pc.style {
            FieldStyle::Glass => rim(cfg.theme.rim),
            FieldStyle::Recessed => chrome_halo::field_shadows(k),
            _ => Vec::new(),
        };
        if !filled {
            // A recessed field is sunk into its panel: no drop shadow.
            if pc.style != FieldStyle::Recessed {
                shadows.push(soft_shadow(22.0 * k, 0.24, 6.0 * k));
            }
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
        }
        // The component designs' filled field: its container and indicator.
        let m3 = inputs::look(InputDesign::Filled, focused, p);
        // Over the card's tonal surface, Material's highest container.
        let mut fill = if filled && !f.high_contrast { f.tonal.surface_highest.with_alpha(M3_FIELD_ALPHA).hsla() } else { f.fill(cfg, p, focused) };
        let mut flash = 0.0;
        if let Some(at) = self.flash_at.filter(|_| is_pw) {
            let t = f.now.saturating_duration_since(at).as_secs_f32() / 0.9;
            if t < 1.0 {
                fill = blend(fill, p.urgent_coral.with_alpha(0.6 * (1.0 - t)).hsla());
                flash = 1.0 - t;
            }
        }
        let shake = if is_pw { self.shake.value() * f.u } else { 0.0 };
        let mut pill = div()
            .id(id)
            .ml(px(shake))
            .w(px(w))
            .h(px(h))
            .pl(px(PILL_PAD_START * k))
            .pr(px(PILL_PAD_END * k))
            .flex()
            .items_center()
            .gap(px(PILL_GAP * k))
            .cursor_text()
            .shadow(shadows)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.field = field;
                this.session_menu = false;
                this.activity();
                cx.notify();
            }));
        let accent = pc.accent_border && focused;
        let edge = |rest: Hsla| if accent { p.primary.hsla() } else { rest };
        pill = match pc.style {
            FieldStyle::Underline => {
                pill.style().border_widths.bottom = Some(px(1.5).into());
                pill.border_color(edge(white.opacity(if focused { 0.9 } else { 0.55 })))
            }
            FieldStyle::Outline => pill.rounded(px(radius)).border_1().border_color(edge(white.opacity(if focused { 0.85 } else { 0.5 }))),
            FieldStyle::Inset => pill.rounded(px(radius)).bg(fill).border_1().border_color(edge(p.outline.with_alpha(0.9).hsla())),
            FieldStyle::Recessed => pill.rounded(px(radius)).bg(fill).border_1().border_color(chrome_halo::field_edge(p, focused, flash)),
            FieldStyle::Filled => {
                let r = M3_FIELD_RADIUS * k * cfg.theme.roundness;
                let (ink, t) = m3.edge.unwrap_or((p.on_surface_variant, inputs::EDGE_PX.0));
                let ink = if flash > 0.0 { blend(ink.hsla(), p.urgent_coral.with_alpha(flash).hsla()) } else { ink.hsla() };
                let t = if flash > 0.0 { inputs::EDGE_PX.1 } else { t };
                pill.relative().rounded_tl(px(r)).rounded_tr(px(r)).bg(fill).child(div().absolute().left_0().right_0().bottom_0().h(px(t)).bg(ink))
            }
            _ => pill.rounded(px(radius)).bg(fill).border_1().border_color(edge(if focused { white.opacity(0.3) } else { gpui::transparent_black() })),
        };
        let trailing = trailing.map(|t| div().flex_none().w(px(trail_w)).flex().items_center().justify_end().child(t));
        pill.when(unfold > 0.6, |el| el.child(inner).children(trailing)).into_any_element()
    }

    /// The masked password: one symbol per character in fixed slots from
    /// the field's start edge (mirrored right-to-left), so earlier symbols
    /// never move; a full row scrolls one slot per character.
    #[allow(clippy::too_many_arguments)]
    fn symbol_row(&self, f: &Frame, k: f32, h: f32, room: f32, text: &str, is_pw: bool, caret_on: bool) -> AnyElement {
        let pc = &self.config.password;
        let text_size = pc.text_size * k;
        let row = Row::new(room, text_size, f.scale);
        let n = text.chars().count();
        let snap = |v: f32| (v * f.scale).round() / f.scale;
        let caret_h = h * CARET_SHARE;
        let box_h = snap(caret_h.max(row.size));
        let top = snap((box_h - row.size) / 2.0);
        let ink = f.white.opacity(0.95);
        let dots = pc.mask == PasswordMask::Dots;
        let dot = snap(text_size * symbols::DOT_EM);
        let animate = pc.keystroke_ripple && !f.reduced;
        let mirrored = f.rtl;
        let slot = |o: f32| {
            let el = div().absolute().top(px(top)).size(px(row.size)).flex().items_center().justify_center();
            if mirrored { el.right(px(o)) } else { el.left(px(o)) }
        };
        let glyph = |ix: usize, scale: f32, opacity: f32| {
            let (shape, d) = if dots { (Symbol::Circle, dot) } else { (symbols::symbol_at(self.mask_seed, ix), row.size) };
            svg().path(shape.asset_path()).size(px(d)).flex_none().text_color(ink.opacity(opacity)).with_transformation(Transformation::scale(gpui::size(scale, scale)))
        };
        let mut el = div().relative().flex_none().w(px(room)).h(px(box_h)).overflow_hidden();
        // The newest `n` arrival times belong to these characters.
        let born = self.dots.len().saturating_sub(n);
        for ix in row.first_shown(n)..n {
            let Some(o) = row.offset(ix, n) else { continue };
            let (scale, opacity) = self.dots.get(born + ix).map_or((1.0, 1.0), |at| symbols::appear(f.now.saturating_duration_since(*at), animate));
            el = el.child(slot(o).child(glyph(ix, scale, opacity)));
        }
        if is_pw {
            for (ix, scale, opacity) in self.fades.live(f.now, animate) {
                if let Some(o) = row.offset(ix, n) {
                    el = el.child(slot(o).child(glyph(ix, scale, opacity)));
                }
            }
        }
        if caret_on {
            let o = (row.caret_offset(n) - CARET_W / 2.0).clamp(0.0, (room - CARET_W).max(0.0));
            let c = div().absolute().top(px(snap((box_h - caret_h) / 2.0))).w(px(CARET_W)).h(px(caret_h)).rounded_full().bg(ink);
            el = el.child(if mirrored { c.right(px(o)) } else { c.left(px(o)) });
        }
        el.into_any_element()
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
            _ => tl("Enter password"),
        };
        let secret = self.prompt.as_ref().is_none_or(|(s, _)| *s);
        let field = self.pill(f, k, "password", self.password.clone(), secret, placeholder, self.field == Field::Password, unfold, cx);
        col = if cfg.password.submit == SubmitButton::Beside && unfold > 0.6 {
            let h = cfg.password.height * k;
            let p = &self.palette;
            let button = div().id("submit-beside").size(px(h)).flex_none();
            let button = if cfg.password.style == FieldStyle::Filled {
                // Material 3 Expressive's filled icon button: round at
                // rest, squaring off while held (the component designs'
                // expressive shape morph).
                let r = buttons::icon_radius(ButtonDesign::Expressive, h, self.pressed(Press::Submit));
                let hover = p.primary.mix(p.on_primary, M3_HOVER_LAYER).hsla();
                button.rounded(px(r)).bg(p.primary.hsla()).hover(move |s| s.bg(hover)).on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.hold(Press::Submit)))
            } else {
                button.rounded(px(h * BESIDE_RADIUS * cfg.theme.roundness.max(0.4) * 2.0)).bg(p.primary.hsla()).hover(|s| s.opacity(0.9))
            };
            let button = button
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))
                .child(icon(if f.rtl { "arrow-left" } else { "arrow-right" }, h * 0.42, p.on_primary.hsla()));
            col.child(div().flex().items_center().gap(px(BESIDE_GAP * k)).when(f.rtl, |el| el.flex_row_reverse()).child(field).child(button))
        } else if cfg.password.submit == SubmitButton::Pill && unfold > 0.6 {
            col.child(field).child(div().h(px(chrome_halo::PILL_GAP * k))).child(self.pill_submit(f, k, cx))
        } else {
            col.child(field)
        };
        let white = f.white;
        let status: Option<(Option<&'static str>, String, Hsla)> = match &self.status {
            Status::Error(e) => Some((None, e.clone(), self.palette.urgent_coral.with_lightness(0.85).hsla())),
            Status::Info(i) => Some((None, i.clone(), white.opacity(0.88))),
            Status::None if caps && cfg.password.caps_hint => Some((Some("caps"), tl("Caps Lock is on"), white.opacity(0.88))),
            Status::None if self.demo() => Some((None, tl("Demo: any password works — try “wrong” to see a failure"), white.opacity(0.78))),
            Status::None => None,
        };
        if matches!(cfg.password.style, FieldStyle::Terminal | FieldStyle::Recessed) {
            let error = matches!(self.status, Status::Error(_));
            return col.child(ghostline::led_status(f, k, self.field_width(k), status, error, &self.palette)).into_any_element();
        }
        if cfg.password.style == FieldStyle::Filled {
            // Material's supporting text: under the field, from its start
            // edge, in the error colour or the variant text colour.
            let p = &self.palette;
            let status = status.map(|(ic, text, _)| {
                let ink = if matches!(self.status, Status::Error(_)) { p.urgent_coral.with_lightness(p.urgent_coral.lightness().max(TINT_LIGHTNESS)).hsla() } else { p.on_surface_variant.hsla() };
                (ic, text, ink)
            });
            return col
                .child(
                    div()
                        .mt(px(6.0 * k))
                        .h(px(20.0 * k))
                        .w(px(self.field_width(k)))
                        .px(px(PILL_PAD_START * k))
                        .flex()
                        .items_center()
                        .gap(px(6.0 * k))
                        .when(f.rtl, |el| el.flex_row_reverse())
                        .font_family(f.ui_font.clone())
                        .text_size(px(12.5 * k))
                        .when_some(status, |el, (ic, text, color)| el.when_some(ic, |el, ic| el.child(icon(ic, 14.0 * k, color))).child(div().text_color(color).child(text))),
                )
                .into_any_element();
        }
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
        if self.config.user.switcher == UserSwitcher::Chips {
            return self.user_chips(f, k, count, focus_users, cx);
        }
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
                        .child(icon("user", 18.0 * k, f.ground.opacity(0.9)))
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
                            .border_color(if selected && focus_users { f.ground.opacity(0.85) } else if selected { f.ground.opacity(0.35) } else { gpui::transparent_black() })
                            .child(face),
                    )
                    .child(div().font_family(f.ui_font.clone()).text_size(px(11.5 * k)).text_color(f.ground.opacity(0.88)).child(label)),
            );
        }
        row.into_any_element()
    }

    /// Material input chips for the people on this computer: a small face
    /// and the first name; the chosen one tonal with a check.
    fn user_chips(&self, f: &Frame, k: f32, count: usize, focus_users: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let h = M3_USER_CHIP_H * k;
        let face = h - 8.0 * k;
        let mut row = div().flex().flex_wrap().justify_center().gap(px(8.0 * k)).when(f.rtl, |el| el.flex_row_reverse());
        for i in 0..count {
            let selected = (self.manual.is_none() && i == self.user_ix) || (self.manual.is_some() && i == self.users.len());
            let (avatar, label) = match self.users.get(i) {
                Some(e) => (
                    match &e.avatar {
                        Some(image) if self.config.user.avatar_source == AvatarSource::Auto => img(ImageSource::Render(image.clone())).size(px(face)).rounded_full().object_fit(ObjectFit::Cover).into_any_element(),
                        _ => div()
                            .size(px(face))
                            .rounded_full()
                            .bg(f.tonal.primary_container.hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(snap_size(face * INITIALS_SCALE)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(f.tonal.on_primary_container.hsla())
                            .child(e.user.initials())
                            .into_any_element(),
                    },
                    e.user.display.split_whitespace().next().unwrap_or(&e.user.name).to_string(),
                ),
                None => (icon("user", snap_size(face * 0.7), if selected { f.tonal.on_secondary_container.hsla() } else { f.ground }.opacity(0.9)).into_any_element(), tl("Other")),
            };
            // An unchosen chip is clear, on the backdrop.
            let (bg, ink) = if selected { (f.tonal.secondary_container.hsla(), f.tonal.on_secondary_container.hsla()) } else { (gpui::transparent_black(), f.ground.opacity(0.92)) };
            row = row.child(
                div()
                    .id(("user", i))
                    .h(px(h))
                    .pl(px(4.0 * k))
                    .pr(px(12.0 * k))
                    .flex()
                    .items_center()
                    .gap(px(8.0 * k))
                    .when(f.rtl, |el| el.flex_row_reverse().pl(px(12.0 * k)).pr(px(4.0 * k)))
                    .rounded(px(M3_CHIP_RADIUS * k))
                    .bg(bg)
                    .when(!selected, |el| el.border_1().border_color(p.outline.with_alpha(0.9).hsla()).hover(|s| s.bg(gpui::white().opacity(M3_HOVER_LAYER))))
                    .when(selected && focus_users, |el| el.border_1().border_color(p.primary.hsla()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_user(i, cx)))
                    .child(avatar)
                    .child(div().font_family(f.ui_font.clone()).text_size(px(13.0 * k)).font_weight(FontWeight::MEDIUM).text_color(ink).child(label))
                    .when(selected, |el| el.child(icon("check", 14.0 * k, ink))),
            );
        }
        row.into_any_element()
    }

    /// The session as a Material chip that opens a menu.
    fn session_chip(&self, f: &Frame, k: f32, name: String, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let cfg = &self.config;
        let h = M3_CHIP_H * k;
        let open = self.session_menu;
        // A resting chip is clear: on the card when it holds the session,
        // else on the backdrop.
        let resting = if cfg.card.enabled && cfg.card.session_inside { f.white } else { f.ground };
        let (bg, ink) = if focused || open { (f.tonal.secondary_container.hsla(), f.tonal.on_secondary_container.hsla()) } else { (gpui::transparent_black(), resting.opacity(0.92)) };
        let chip = div()
            .id("session")
            .h(px(h))
            .px(px(12.0 * k))
            .flex()
            .items_center()
            .gap(px(8.0 * k))
            .when(f.rtl, |el| el.flex_row_reverse())
            .rounded(px(M3_CHIP_RADIUS * k))
            .bg(bg)
            .when(!(focused || open), |el| el.border_1().border_color(p.outline.with_alpha(0.9).hsla()))
            .hover(|s| s.bg(gpui::white().opacity(M3_HOVER_LAYER)))
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.is_preview() {
                    return;
                }
                this.session_menu = !this.session_menu;
                this.field = Field::Session;
                this.activity();
                cx.notify();
            }))
            .child(icon("monitor", 16.0 * k, ink))
            .child(div().font_family(f.ui_font.clone()).text_size(px(13.0 * k)).font_weight(FontWeight::MEDIUM).text_color(ink).child(name))
            .when(self.sessions.len() > 1, |el| el.child(icon(if open { "chevron-up" } else { "chevron-down" }, 14.0 * k, ink)));
        let mut root = div().relative().child(chip);
        if open && !self.sessions.is_empty() {
            let below = (cfg.card.enabled && cfg.card.session_inside) || cfg.placement(Element::Session).anchor.fraction().1 < 0.5;
            let mut menu = div()
                .absolute()
                .when(below, |el| el.top(px(h + 6.0 * k)))
                .when(!below, |el| el.bottom(px(h + 6.0 * k)))
                .when(!f.rtl, |el| el.left_0())
                .when(f.rtl, |el| el.right_0())
                .min_w(px(220.0 * k))
                .p(px(4.0 * k))
                .rounded(px(M3_MENU_RADIUS * k))
                .bg(p.surface_container.hsla())
                .shadow(vec![soft_shadow(24.0 * k, 0.3, 8.0 * k)])
                .flex()
                .flex_col()
                .gap(px(2.0 * k));
            for (i, s) in self.sessions.iter().enumerate() {
                let selected = i == self.session_ix;
                let label = if s.kind == SessionKind::X11 { format!("{} (X11)", s.name) } else { s.name.clone() };
                let (bg, ink) = if selected { (f.tonal.secondary_container.hsla(), f.tonal.on_secondary_container.hsla()) } else { (gpui::transparent_black(), p.on_surface.hsla()) };
                menu = menu.child(
                    div()
                        .id(("session-item", i))
                        .h(px(M3_ITEM_H * k))
                        .px(px(12.0 * k))
                        .rounded(px(M3_ITEM_RADIUS * k))
                        .flex()
                        .items_center()
                        .gap(px(12.0 * k))
                        .when(f.rtl, |el| el.flex_row_reverse())
                        .bg(bg)
                        .hover(|st| st.bg(gpui::white().opacity(M3_HOVER_LAYER)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.session_ix = i;
                            this.session_menu = false;
                            this.field = Field::Password;
                            cx.notify();
                        }))
                        .child(div().w(px(18.0 * k)).when(selected, |el| el.child(icon("check", 16.0 * k, ink))))
                        .child(div().font_family(f.ui_font.clone()).text_size(px(14.0 * k)).text_color(ink).child(label)),
                );
            }
            root = root.child(menu);
        }
        root.into_any_element()
    }

    fn session_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let cfg = &self.config;
        let name = self.sessions.get(self.session_ix).map(|s| if s.kind == SessionKind::X11 { format!("{} (X11)", s.name) } else { s.name.clone() }).unwrap_or_else(|| tl("Login shell"));
        let focused = self.field == Field::Session;
        if cfg.session.style == SessionStyle::Chip {
            return self.session_chip(f, k, name, focused, cx);
        }
        let wide = cfg.session.style == SessionStyle::Field;
        let h = if wide { cfg.password.height * k * 0.9 } else { 36.0 * k };
        let pill = div()
            .id("session")
            .h(px(h))
            .px(px(14.0 * k))
            .flex()
            .items_center()
            .gap(px(8.0 * k))
            .rounded(px(h / 2.0 * cfg.theme.roundness))
            .when(wide, |el| el.w(px(self.field_width(k))).bg(p.surface_dim.with_alpha(0.92).hsla()).border_color(if focused { p.primary.hsla() } else { p.outline.with_alpha(0.9).hsla() }))
            .when(!wide, |el| el.bg(f.fill(cfg, p, focused)).shadow(rim(cfg.theme.rim)).border_color(if focused { f.white.opacity(0.6) } else { gpui::transparent_black() }))
            .border_1()
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
            .child(icon("monitor", 16.0 * k, f.white.opacity(if wide { 0.7 } else { 0.92 })))
            .child(div().font_family(f.ui_font.clone()).text_size(px(13.5 * k)).font_weight(FontWeight::MEDIUM).text_color(f.white.opacity(0.95)).child(name))
            .when(wide, |el| el.child(div().flex_1()))
            .when(self.sessions.len() > 1, |el| el.child(icon(if self.session_menu { "chevron-up" } else { "chevron-down" }, 14.0 * k, f.white.opacity(0.8))));
        let mut root = div().relative().child(pill);
        if self.session_menu && !self.sessions.is_empty() {
            let below = (cfg.card.enabled && cfg.card.session_inside) || cfg.placement(Element::Session).anchor.fraction().1 < 0.5;
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
        let tonal = pc.style == PowerStyle::Tonal;
        let mut row = div().flex().gap(px(if tonal { 8.0 } else { 18.0 } * k)).opacity(if self.demo() { 0.55 } else { 1.0 });
        for action in actions {
            row = row.child(
                div()
                    .id(action.method())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0 * k))
                    .cursor_pointer()
                    .when(tonal, |el| el.on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, _| this.hold(Press::Power(action)))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.power(action, cx)))
                    .child({
                        let d = pc.size * k;
                        let ink = if tonal { f.tonal.on_secondary_container.hsla() } else { f.white.opacity(0.95) };
                        let button = div().size(px(d)).flex().items_center().justify_center().child(icon(action.icon(), d * 0.43, ink));
                        match pc.style {
                            // Material 3 Expressive: tonal, round at rest,
                            // squaring off while held.
                            PowerStyle::Tonal => {
                                let hover = f.tonal.secondary_container.mix(f.tonal.on_secondary_container, M3_HOVER_LAYER).hsla();
                                button.rounded(px(buttons::icon_radius(ButtonDesign::Expressive, d, self.pressed(Press::Power(action))))).bg(f.tonal.secondary_container.hsla()).hover(move |s| s.bg(hover))
                            }
                            PowerStyle::Soft => button
                                .rounded(px(d / 2.0 * self.config.theme.roundness.max(0.3)))
                                .bg(f.fill(&self.config, &self.palette, false))
                                .shadow(rim(self.config.theme.rim))
                                .hover(|s| s.bg(gpui::white().opacity(0.28))),
                            PowerStyle::Outlined => button
                                .rounded_full()
                                .bg(self.palette.surface_dim.with_alpha(0.78).hsla())
                                .border_1()
                                .border_color(self.palette.outline.with_alpha(0.9).hsla())
                                .hover(|s| s.bg(gpui::white().opacity(0.14))),
                        }
                    })
                    .when(pc.labels, |el| el.child(div().font_family(f.ui_font.clone()).text_size(px(11.5 * k)).text_color(f.ground.opacity(0.85)).child(action.label()))),
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

    /// The card: user, password and (optionally) session on one panel.
    fn card_el(&self, f: &Frame, k: f32, caps: bool, cx: &mut Context<Self>) -> AnyElement {
        match self.config.card.style {
            CardStyle::Hud => return self.hud_card(f, k, caps, cx),
            CardStyle::Bezel => return self.bezel_card(f, k, caps, cx),
            CardStyle::Horizon => return self.horizon_band(f, k, cx),
            CardStyle::Plain => {}
        }
        let c = &self.config.card;
        let p = &self.palette;
        let fill = Color::from_hex(c.fill.trim()).unwrap_or(p.surface_container).with_alpha(c.opacity.clamp(0.0, 1.0));
        let edge = Color::from_hex(c.border_color.trim()).unwrap_or(p.outline);
        let mut card = div()
            .relative()
            .w(px(c.width * k))
            .p(px(c.padding * k))
            .rounded(px(c.radius * k))
            .bg(fill.hsla())
            .shadow(vec![soft_shadow(40.0 * k, 0.32, 14.0 * k)])
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.0 * k));
        if c.border > 0.0 {
            let w: gpui::AbsoluteLength = px(c.border).into();
            let widths = &mut card.style().border_widths;
            (widths.top, widths.right, widths.bottom, widths.left) = (Some(w), Some(w), Some(w), Some(w));
            card = card.border_color(edge.hsla());
        }
        let switcher = self.users.len() > 1 || self.manual.is_some();
        if c.back_button && switcher && !self.is_preview() {
            let d = 30.0 * k;
            card = card.child(
                div()
                    .id("card-back")
                    .absolute()
                    .top(px(c.padding * k * 0.6))
                    .when(!f.rtl, |el| el.left(px(c.padding * k * 0.6)))
                    .when(f.rtl, |el| el.right(px(c.padding * k * 0.6)))
                    .size(px(d))
                    .rounded(px(d * 0.24 * self.config.theme.roundness.max(0.4) * 2.0))
                    .border_1()
                    .border_color(edge.hsla())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(gpui::white().opacity(0.08)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.field = Field::User;
                        this.activity();
                        cx.notify();
                    }))
                    .child(icon(if f.rtl { "arrow-right" } else { "arrow-left" }, d * 0.5, f.white.opacity(0.9))),
            );
        }
        card = card.child(self.user_el(f, k, cx)).child(self.password_el(f, k, caps, cx));
        if c.session_inside && !self.sessions.is_empty() && self.config.placement(Element::Session).visible {
            card = card.child(self.session_el(f, k, cx));
        }
        card.into_any_element()
    }

    /// The logo: an image file, or Mandara's emblem.
    fn logo_el(&self, f: &Frame, k: f32) -> AnyElement {
        let l = &self.config.logo;
        let d = l.size * k;
        let path = l.path.trim();
        if path.is_empty() {
            let ink = if l.accent {
                let a = self.palette.primary;
                a.with_lightness(a.lightness().max(TINT_LIGHTNESS)).with_alpha(l.opacity).hsla()
            } else {
                f.ground.opacity(l.opacity)
            };
            return div().child(icon("emblem", d, ink)).into_any_element();
        }
        img(std::path::PathBuf::from(path)).size(px(d)).object_fit(ObjectFit::Contain).opacity(l.opacity).into_any_element()
    }

    /// The colour scheme switcher: each press moves to the next scheme.
    fn themes_el(&self, f: &Frame, k: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let h = 30.0 * k;
        div()
            .id("themes")
            .h(px(h))
            .px(px(10.0 * k))
            .flex()
            .items_center()
            .gap(px(6.0 * k))
            .rounded(px(h * 0.3 * self.config.theme.roundness.max(0.4) * 2.0))
            .bg(p.surface_container.with_alpha(0.92).hsla())
            .border_1()
            .border_color(p.outline.with_alpha(0.9).hsla())
            .cursor_pointer()
            .hover(|s| s.bg(gpui::white().opacity(0.12)))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.next_theme(cx)))
            .child(icon("palette", 15.0 * k, p.on_surface_variant.hsla()))
            .child(icon("chevron-down", 13.0 * k, f.white.opacity(0.6)))
            .into_any_element()
    }

    fn next_theme(&mut self, cx: &mut Context<Self>) {
        if self.is_preview() {
            return;
        }
        let list = theme_choices(&self.config);
        let next = list.iter().position(|t| *t == self.config.theme.preset).map_or(0, |i| (i + 1) % list.len().max(1));
        let Some(theme) = list.get(next).cloned() else { return };
        self.config.theme.preset = theme.clone();
        self.set_palette(palette_for_seed(&self.config, self.seed), false);
        self.state.theme = Some(theme);
        if let Err(e) = self.state.save(&state_path(self.demo())) {
            tracing::warn!("greeter state: {e}");
        }
        self.activity();
        cx.notify();
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
            .text_color(f.ground.opacity(0.92))
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

/// Material's loading indicator: a soft cookie turning as it breathes
/// (held still with reduced motion), the same as the lock screen's.
fn loading_indicator(size: f32, t: f32, color: Hsla, still: bool) -> AnyElement {
    let t = if still { 0.0 } else { t };
    let breath = LOADER_REST + (1.0 - LOADER_REST) * ((t * LOADER_BREATH).sin() * 0.5 + 0.5);
    let turn = Transformation::rotate(gpui::radians(t * LOADER_TURN)).with_scaling(gpui::size(breath, breath));
    div()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .child(svg().path(Symbol::Cookie6.asset_path()).size(px(size * 0.8)).text_color(color).with_transformation(turn))
        .into_any_element()
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
    /// Place public decorations below the sign-in controls in live and preview views.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let dt = self.clock.tick(now);
        let preview = self.is_preview();
        // Last frame's caret (painting this frame sets it again).
        self.caret_x = self.horizon_caret.take();
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
        self.follow_seed(now);
        self.warm_up(now);
        self.tick(now, dt);
        self.run_autoplay(cx);
        if self.autoplay.is_some() {
            // Self-test: clicks pass through to the windows underneath.
            window.set_input_region(Some(&[]));
        }
        self.schedule_frame(window, cx);

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
        let widget_size = ((vw * sf).round() as u32, (vh * sf).round() as u32);
        let widgets = if !self.config.widgets.items.is_empty() && (!preview || !self.offscreen(window)) {
            let fonts = mandara_ui::backdrop::paint::FontNames::from(cx.global::<Fonts>());
            self.widgets.update(&self.config.widgets, widget_size, &fonts, &self.palette, self.config.clock.format_24h, self.mode.permits_weather_requests());
            if self.widgets.pending() { window.request_animation_frame(); }
            self.widgets.image(widget_size)
        } else {
            self.widgets.clear();
            None
        };
        for image in self.widgets.take_trash() {
            let _ = window.drop_image(image);
        }
        let cfg = &self.config;
        let p = &self.palette;
        let fonts: &Fonts = cx.global::<Fonts>();
        let rtl = mandara_i18n::language().is_rtl();
        let a11y = &cfg.accessibility;
        let large = if a11y.large_text { 1.2 } else { 1.0 };
        let u = if preview { vh / 1080.0 } else { mandara_config::login_layout::screen_scale(vw, vh) } * large;
        let success = self.success.value().clamp(0.0, 1.0);
        let f = Frame {
            u,
            vw,
            vh,
            now,
            white: foreground(cfg).hsla(),
            ground: ground_ink(cfg, self.bg_key.solid).hsla(),
            ui_font: match self.config.font.trim() {
                "" if rtl => fonts.arabic.clone(),
                "" => fonts.ui.clone(),
                family => SharedString::from(family.to_string()),
            },
            display_font: fonts.display.clone(),
            mono_font: fonts.mono.clone(),
            rtl,
            scale: window.scale_factor(),
            success,
            reduced: a11y.reduced_motion,
            still: a11y.reduced_motion || preview,
            high_contrast: a11y.high_contrast,
            t: now.saturating_duration_since(self.started).as_secs_f32(),
            tonal: Tonal::of(p),
            keyboard: layout.clone(),
            caps: !preview && window.capslock().on && self.field != Field::Session,
        };
        let caps = f.caps;

        // Backdrop with pointer parallax (moves against the pointer).
        let bgc = &cfg.background;
        let par = if f.still { 0.0 } else { bgc.parallax * u };
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
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.release();
                this.end_drag(cx)
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.release();
                this.end_drag(cx)
            }))
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
            .child(self.background.element(vw, vh, zoom, shift, p, now));
        let origin = self.origin.clone();
        root = root.child(canvas(move |b, _, _| origin.set(b.origin), |_, _, _, _| {}).absolute().top_0().left_0().size_full());
        if bgc.orbs && bgc.orb_intensity > 0.0 {
            let t = if f.still { 0.0 } else { f.t };
            root = root.child(orbs(vw, vh, p, t, bgc.orb_intensity, shift));
        }
        // Scrims: tint, dim (+ a little while typing), vignette, grain.
        if let Ok(tint) = Color::from_hex(bgc.tint.trim())
            && bgc.tint_strength > 0.0
        {
            root = root.child(div().absolute().top_0().left_0().size_full().bg(tint.with_alpha(bgc.tint_strength * 0.8).hsla()));
        }
        let typing_dim = 0.12 * self.glow.value().clamp(0.0, 1.0);
        let hc = if f.high_contrast { 0.25 } else { 0.0 };
        // Dynamic colour: a light wallpaper gets a deeper scrim, so the
        // clock and the controls over it stay readable.
        let light = if cfg.theme.from_wallpaper { self.background.luma().map_or(0.0, mandara_ui::components::tonal::scrim_for_luma) } else { 0.0 };
        root = root.child(div().absolute().top_0().left_0().size_full().bg(gpui::black().opacity((bgc.dim + typing_dim + hc + light).clamp(0.0, 0.95))));
        let v = bgc.vignette;
        if v > 0.0 {
            root = root
                .child(div().absolute().top_0().left_0().w(px(vw)).h(px(vh * 0.42)).bg(oklab_gradient(180.0, Color::oklcha(0.0, 0.0, 0.0, 0.7 * v), Color::oklcha(0.0, 0.0, 0.0, 0.0))))
                .child(div().absolute().bottom_0().left_0().w(px(vw)).h(px(vh * 0.45)).bg(oklab_gradient(180.0, Color::oklcha(0.0, 0.0, 0.0, 0.0), Color::oklcha(0.0, 0.0, 0.0, 0.95 * v))));
        }
        // A soft shade behind the user card keeps text legible on footage.
        let card = cfg.placement(Element::User);
        if card.visible && !cfg.card.enabled {
            let (fx, fy) = card.anchor.fraction();
            let (cxp, cyp) = (fx * vw + card.x * u, fy * vh + card.y * u + 60.0 * u);
            let r = 480.0 * u;
            let a = 0.28 * self.wake.value().clamp(0.0, 1.0) * (1.0 - success);
            root = root.child(div().absolute().left(px(cxp - r / 2.0)).top(px(cyp - r / 2.0)).size(px(r)).rounded_full().shadow(vec![glow(gpui::black().opacity(a), r * 0.8, 0.0)]));
        }
        match bgc.overlay {
            Overlay::None => {}
            Overlay::Hud => root = root.child(ghostline::hud_overlay(&f, p)),
            Overlay::Prism => root = root.child(chrome_halo::prism_overlay(&f, p, shift)),
            Overlay::Horizon => root = root.child(self.horizon_overlay(&f)),
        }
        if bgc.grain > 0.0 {
            root = root.child(div().absolute().top_0().left_0().size_full().opacity(bgc.grain).child(crate::background::grain(vw, vh)));
        }

        if let Some(widgets) = widgets {
            root = root.child(img(widgets).absolute().top_0().left_0().w(px(vw)).h(px(vh)).object_fit(ObjectFit::Fill));
        }

        // Elements in z order.
        let mut placements: smallvec::SmallVec<[&crate::config::Placement; 12]> = cfg.layout.iter().collect();
        placements.sort_by_key(|p| p.z);
        root = root.children(self.glyph_warmer(&f));
        for pl in placements {
            let k = u * pl.scale;
            let content = match pl.element {
                Element::Clock => match cfg.clock.face {
                    ClockFace::Digits => self.clock_el(&f, k),
                    ClockFace::Ruler => self.ruler_clock(&f, k),
                    ClockFace::Halo => self.halo_clock(&f, k),
                    ClockFace::Vast => self.vast_clock(&f, k),
                },
                // The ruler and halo faces hold the date, and Meridian's
                // line does with the vast face.
                Element::Date if face_holds_date(cfg) => div().into_any_element(),
                Element::Date => self.date_el(&f, k),
                Element::User if cfg.card.enabled => self.card_el(&f, k, caps, cx),
                Element::User => self.user_el(&f, k, cx),
                // Inside the card when there is one.
                Element::Password if cfg.card.enabled => div().into_any_element(),
                Element::Session if cfg.card.enabled && cfg.card.session_inside => div().into_any_element(),
                Element::Password => self.password_el(&f, k, caps, cx),
                Element::Users => self.users_el(&f, k, cx),
                Element::Session => self.session_el(&f, k, cx),
                Element::Power => self.power_el(&f, k, cx),
                Element::Keyboard => self.keyboard_el(&f, k, layout.clone()),
                Element::Banner => self.banner_el(&f, k),
                Element::Logo => self.logo_el(&f, k),
                Element::Themes => self.themes_el(&f, k, cx),
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
                    .top(px(DEMO_CHROME_INSET * k))
                    .left(px(DEMO_CHROME_INSET * k))
                    .w(px(vw - 2.0 * DEMO_CHROME_INSET * k))
                    .h(px(DEMO_CHROME_HEIGHT * k))
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
                            .child(div().text_size(px(12.0 * k)).font_weight(FontWeight::SEMIBOLD).text_color(f.ground.opacity(0.95)).child(tl("Login screen preview"))),
                    )
                    .child(
                        div()
                            .id("exit-demo")
                            .h(px(DEMO_CHROME_HEIGHT * k))
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
                            .child(icon("close", 14.0 * k, f.ground.opacity(0.95)))
                            .child(div().text_size(px(12.5 * k)).font_weight(FontWeight::MEDIUM).text_color(f.ground.opacity(0.95)).child(tl("Exit demo"))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_chrome_never_covers_top_corner_elements() {
        let below = DEMO_CHROME_INSET + DEMO_CHROME_HEIGHT + DEMO_CHROME_GAP;
        // The Midnight card shows its scheme switcher in the top-right
        // corner, inside the chrome's row: the demo moves it below.
        let mut cfg = GreeterConfig::default();
        assert!(cfg.apply_look("midnight-card"));
        let switcher = cfg.placement(Element::Themes);
        assert!(switcher.visible && switcher.anchor == Anchor::TopRight && switcher.y < below, "{switcher:?}");
        assert_eq!(clear_demo_chrome(switcher.anchor, switcher.y), below);
        // Above the edge too; one already lower, and every other anchor, stays put.
        assert_eq!(clear_demo_chrome(Anchor::TopLeft, -10.0), below);
        assert_eq!(clear_demo_chrome(Anchor::TopRight, 120.0), 120.0);
        for anchor in [Anchor::Top, Anchor::Left, Anchor::Center, Anchor::Right, Anchor::BottomLeft, Anchor::Bottom, Anchor::BottomRight] {
            assert_eq!(clear_demo_chrome(anchor, 28.0), 28.0, "{anchor:?}");
        }
    }

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

    /// With the light colours on, text straight on the default backdrop
    /// (the aurora, dark under its scrim) stays white and readable (WCAG
    /// AA), while the controls keep the light palette and its deep ink.
    #[test]
    fn light_colours_keep_backdrop_text_readable() {
        let mut cfg = GreeterConfig::default();
        cfg.theme.light = true;
        assert!(cfg.draws_light());
        let ink = ground_ink(&cfg, None);
        let scrim = Color::oklcha(0.0, 0.0, 0.0, cfg.background.dim);
        // The aurora's sky at its top and bottom, for any theme's hue.
        for hue in (0..360).step_by(30).map(|h| h as f32) {
            for sky in [Color::oklch(0.25, 0.09, hue), Color::oklch(0.11, 0.07, hue)] {
                let ground = scrim.over(sky);
                for (what, alpha) in [("the clock", cfg.clock.opacity), ("the date", cfg.date.opacity), ("the name", 0.96), ("the logo", cfg.logo.opacity)] {
                    let ratio = ink.with_alpha(alpha).over(ground).contrast_ratio(ground);
                    assert!(ratio >= 4.5, "{what} over the sky at {hue}°: {ratio:.2}");
                }
                // The frosted field keeps the light colours' deep ink.
                let field = Color::oklcha(1.0, 0.0, 0.0, 0.5 + cfg.theme.glass).over(ground);
                let typed = foreground(&cfg).with_alpha(0.95).over(field).contrast_ratio(field);
                assert!(typed >= 4.5, "typing in the frosted field at {hue}°: {typed:.2}");
            }
        }
    }

    /// Text on the backdrop follows the backdrop, not the colours: a light
    /// solid floor takes the deep ink even with the dark colours, a dark
    /// one white even with the light colours, and an unset one follows the
    /// colours (it is the theme's own floor).
    #[test]
    fn backdrop_ink_follows_the_floor() {
        let white = white_ink();
        let mut cfg = GreeterConfig::default();
        cfg.background.mode = BackgroundMode::Solid;
        cfg.background.dim = 0.0;
        let paper = Color::from_hex("#f3efe6").unwrap();
        let night = Color::from_hex("#0b0d10").unwrap();
        for light in [false, true] {
            cfg.theme.light = light;
            assert_eq!(ground_ink(&cfg, Some(paper)), light_ink(), "a light floor (light colours {light})");
            assert_eq!(ground_ink(&cfg, Some(night)), white, "a dark floor (light colours {light})");
            let floor = palette_for(&cfg).surface_dim;
            assert_eq!(ground_ink(&cfg, Some(floor)), foreground(&cfg), "the theme's floor (light colours {light})");
        }
        // A deep dim turns a light floor dark.
        cfg.background.dim = 0.9;
        assert_eq!(ground_ink(&cfg, Some(paper)), white);
        // Pictures, footage and the aurora are dark under their scrims.
        cfg.theme.light = true;
        for mode in [BackgroundMode::Auto, BackgroundMode::Video, BackgroundMode::Stills, BackgroundMode::Aurora] {
            cfg.background.mode = mode;
            assert_eq!(ground_ink(&cfg, Some(paper)), white, "{mode:?}");
        }
        // Meridian's field follows the colours.
        let mut meridian = GreeterConfig::default();
        meridian.apply_look("meridian");
        for light in [false, true] {
            meridian.theme.light = light;
            assert_eq!(ground_ink(&meridian, Some(paper)), foreground(&meridian));
        }
    }

    /// Ghostline's and Chrome Halo's dark pieces keep the dark colours with
    /// the light ones on; every other look takes them.
    #[test]
    fn dark_pieces_keep_the_dark_colours() {
        for (id, _, _) in crate::config::LOOKS {
            let mut cfg = GreeterConfig::default();
            cfg.apply_look(id);
            cfg.theme.light = true;
            let dark = matches!(id, "ghostline" | "chrome-halo");
            assert_eq!(cfg.draws_light(), !dark, "{id}");
            let mut night = cfg.clone();
            night.theme.light = false;
            assert_eq!(palette_for(&cfg) == palette_for(&night), dark, "{id}: the dark palette");
            if dark {
                assert_eq!(foreground(&cfg), white_ink(), "{id}: light text");
            }
        }
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
