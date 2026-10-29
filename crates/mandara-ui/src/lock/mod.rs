//! Mandara Lock — the screen locker.
//!
//! `mandara lock` locks the session with `ext-session-lock-v1` (one lock
//! surface per output, see [`mandara_core::session_lock`]) and checks the
//! password with PAM ([`pam`]) on a worker thread. It is its own process,
//! like every Wayland locker: if it ever crashed, the compositor keeps the
//! session locked, and the shell (which spawned it) starts a new one.
//!
//! The look follows the Mandara design system, painted in software
//! ([`paint`]) because GPUI cannot host lock surfaces: the blurred,
//! palette-scrimmed wallpaper with anti-banding grain, a big tabular clock
//! and localized date, the avatar with an accent-gradient rim, and a
//! chromatic-glass password pill whose dots pop in on a spring. A wrong
//! password shakes the pill (damped spring impulse) and flashes its rim
//! coral; success fades everything up and away before unlocking.
//!
//! `mandara lock --demo` (or `mandara msg lock --demo`) previews the very
//! same screen on overlay layer surfaces without locking anything: any
//! non-empty password "unlocks", Esc closes it, and it closes itself after
//! `--timeout` seconds (60 by default).
//!
//! In the shell, `mandara msg lock` spawns the locker, and logind lock
//! requests (`loginctl lock-session`, idle daemons, the session menu's Lock)
//! do too while `general.lock_screen` is on.

pub mod look;
pub mod paint;
pub mod pam;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mandara_core::session_lock::{AppControl, KeyInput, KeyboardState, LockApp, LockMode, LockOutcome, SurfaceInfo};
use mandara_core::{FrameClock, SpringConfig, SpringValue};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_theme::{Color, MandaraPalette, ThemeEngine};
use serde_json::json;
use tiny_skia::{Pixmap, PixmapMut};
use xkbcommon_keysyms as keys;

use self::paint::{Font, TextPainter};
use self::pam::{AuthResult, AuthWorker, Authenticator, DemoAuthenticator, PamAuthenticator, wipe};

/// Fake PAM round-trip of the lock-screen preview.
const DEMO_AUTH_DELAY: Duration = Duration::from_millis(450);
/// The lock-screen preview closes itself after this long unless the caller sets a timeout.
const DEMO_TIMEOUT: Duration = Duration::from_secs(60);
/// A password dot pops for this long after its key press (part of the dot animation).
const DOT_POP: Duration = Duration::from_millis(260);
/// Wake-up period while animating (~120 Hz frame pacing).
const ANIMATION_TICK: Duration = Duration::from_millis(8);
/// Crash-loop guard: restarts counted within this window.
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Keysyms the lock screen reacts to (from `xkbcommon/xkbcommon-keysyms.h`).
mod xkbcommon_keysyms {
    pub const BACKSPACE: u32 = 0xff08;
    pub const RETURN: u32 = 0xff0d;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const ESCAPE: u32 = 0xff1b;
}

/// Options for `mandara lock`.
#[derive(Clone, Debug, Default)]
pub struct LockOptions {
    /// Preview on overlay surfaces; nothing is locked and any password works.
    pub demo: bool,
    /// Demo only: close after this long (default 60 s). Ignored for a real
    /// lock, which never unlocks by itself.
    pub timeout: Option<Duration>,
}

/// The longest password accepted (keeps a stuck key from growing forever).
const MAX_PASSWORD: usize = 512;
/// Pill geometry (logical px).
const PILL_W: f32 = 340.0;
const PILL_H: f32 = 54.0;
const AVATAR_R: f32 = 46.0;
const SHAKE: SpringConfig = SpringConfig::new(620.0, 13.0, 1.0);
const REVEAL: SpringConfig = SpringConfig::new(170.0, 22.0, 1.0);
const EXIT: SpringConfig = SpringConfig::new(240.0, 31.0, 1.0);
/// Scrim alpha per unit of `lock.dim` at the top and bottom of the screen.
const SCRIM_TOP: f32 = 0.97;
const SCRIM_BOTTOM: f32 = 1.89;
/// The lock clock's pattern when a custom one is invalid.
const LOCK_TIME_FALLBACK: &str = "%H:%M";
/// Clock size relative to the default for the compact style.
const COMPACT_CLOCK: f32 = 0.45;
/// Stacked clock: line height relative to the digits and weight.
const STACK_LINE: f32 = 0.86;
const STACK_WEIGHT: u16 = 600;

/// Runs the locker (the `mandara lock` subcommand).
pub fn run(options: LockOptions) -> anyhow::Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        anyhow::bail!("Mandara Lock requires a Wayland session (WAYLAND_DISPLAY is not set)");
    }
    let cfg = mandara_config::ConfigStore::open_default().map(|s| s.snapshot()).unwrap_or_default();
    mandara_i18n::set_language(cfg.general.locale.lang());
    // Date language (Studio → Date, time & location): this process never
    // runs `style::init`, so set it here too.
    crate::widgets::clock::set_locale_override(&cfg.datetime.time_locale);
    let palette = ThemeEngine::new(cfg.theme.source.clone(), cfg.theme.mode.is_dark(), cfg.theme.adjustments).target().clone();
    let user = current_user();
    let authenticator: Box<dyn Authenticator> =
        if options.demo { Box::new(DemoAuthenticator { delay: DEMO_AUTH_DELAY }) } else { Box::new(PamAuthenticator::system()) };
    let auth = AuthWorker::spawn(authenticator)?;
    let look = look::LockLook::from_config(&cfg);
    let wallpaper = mandara_config::desktop::effective_wallpaper(cfg.wallpaper.path.as_ref(), &cfg.wallpaper.variants, None, cfg.theme.mode.is_dark())
        .map(|p| mandara_config::paths::expand_tilde(&p.to_string_lossy()));
    let background = look.background_image(wallpaper);
    let mut screen = LockScreen::new(
        options.demo,
        palette,
        Fonts { display: cfg.typography.display_font.clone(), ui: cfg.typography.ui_font.clone() },
        cfg.general.clock_24h,
        user,
        auth,
        None,
        options.demo.then(|| Instant::now() + options.timeout.unwrap_or(DEMO_TIMEOUT)),
    );
    screen.set_look(look, background);
    if !options.demo {
        screen.grace = grace_from_env();
    }
    let mode = if options.demo { LockMode::Demo } else { LockMode::Session };
    let outcome = mandara_core::session_lock::run(mode, &mut screen);
    if !options.demo {
        let _ = std::fs::remove_file(locked_marker());
    }
    match outcome {
        Ok(LockOutcome::Unlocked) => tracing::info!("lock screen: unlocked"),
        Ok(LockOutcome::EndedByCompositor) => tracing::warn!("lock screen: the compositor ended the lock"),
        // Failing after the compositor confirmed the lock leaves the session
        // locked with nobody to unlock it: ask the shell for a new locker.
        Err(err) if screen.was_locked && !options.demo => {
            tracing::error!("lock screen failed while locked: {err:#}");
            std::process::exit(RELOCK_EXIT);
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

/// Exit code of a locker that failed while the session stayed locked.
pub const RELOCK_EXIT: i32 = 70;

// ─────────────────────────────── the user ───────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub name: String,
    pub display: String,
    pub home: PathBuf,
}

/// The `/etc/passwd` entry for `uid` (`display` from the GECOS full name).
pub fn parse_passwd(passwd: &str, uid: u32) -> Option<User> {
    passwd.lines().filter(|l| !l.starts_with('#')).find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 7 || f[2].parse::<u32>().ok()? != uid {
            return None;
        }
        let full = f[4].split(',').next().unwrap_or("").trim();
        Some(User { name: f[0].to_string(), display: if full.is_empty() { f[0].to_string() } else { full.to_string() }, home: PathBuf::from(f[5]) })
    })
}

fn current_user() -> User {
    let uid = rustix::process::getuid().as_raw();
    std::fs::read_to_string("/etc/passwd").ok().and_then(|p| parse_passwd(&p, uid)).unwrap_or_else(|| {
        let name = std::env::var("USER").unwrap_or_else(|_| uid.to_string());
        User { display: name.clone(), name, home: dirs::home_dir().unwrap_or_default() }
    })
}

/// Avatar candidates: AccountsService's `IconFile`, its icon directory,
/// then `~/.face` (same order as the greeter).
fn load_avatar(user: &User, size: u32) -> Option<Pixmap> {
    mandara_services::accounts::avatar_candidates(&user.name, &user.home).into_iter().find_map(|p| {
        let img = image::open(&p).ok()?;
        let side = img.width().min(img.height());
        let square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
        let rgba = square.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
        paint::pixmap_from_rgba(size, size, rgba.as_raw())
    })
}

/// Decodes, shrinks and blurs the wallpaper (off the UI thread).
fn load_wallpaper(path: PathBuf, sigma: f32) -> std::sync::mpsc::Receiver<Pixmap> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("mandara-lock-wallpaper".into()).spawn(move || {
        // A video wallpaper: its poster (first frame).
        let Ok(img) = crate::wallpaper::still(&path).and_then(|p| image::open(&p).map_err(|e| e.to_string())) else { return };
        let small = img.thumbnail(480, 480).to_rgba8();
        let blurred = if sigma > 0.0 { image::imageops::blur(&small, sigma) } else { small };
        if let Some(pm) = paint::pixmap_from_rgba(blurred.width(), blurred.height(), blurred.as_raw()) {
            let _ = tx.send(pm);
        }
    });
    rx
}

fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect()
}

// ───────────────────────────── the screen ─────────────────────────────

pub struct Fonts {
    pub display: String,
    pub ui: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Checking,
    Unlocking,
}

#[derive(Clone, Debug, PartialEq)]
enum Note {
    Error(String),
    Info(String),
}

pub struct LockScreen {
    demo: bool,
    palette: MandaraPalette,
    fonts: Fonts,
    clock_24h: bool,
    user: User,
    avatar: Option<Pixmap>,
    wallpaper: Option<Pixmap>,
    wallpaper_rx: Option<std::sync::mpsc::Receiver<Pixmap>>,
    backgrounds: HashMap<(u32, u32), Pixmap>,
    text: TextPainter,
    auth: AuthWorker,
    password: String,
    /// When each typed character arrived (dot pop-in); also its count.
    typed: Vec<Instant>,
    /// Dots shown while checking (the password itself is gone already).
    checking_len: usize,
    phase: Phase,
    note: Option<Note>,
    keyboard: KeyboardState,
    clock: FrameClock,
    reveal: SpringValue<f32>,
    shake: SpringValue<f32>,
    exit: SpringValue<f32>,
    flash: SpringValue<f32>,
    started: Instant,
    minute: i64,
    dirty: bool,
    animating: bool,
    deadline: Option<Instant>,
    attempts: u32,
    /// The compositor confirmed the lock.
    was_locked: bool,
    /// Lock grace period (`idle.lock_grace_s`, automatic locks only): any
    /// input this soon after the lock is confirmed unlocks without a password.
    grace: Option<Duration>,
    grace_until: Option<Instant>,
    /// Configurable look (`[lock]`).
    look: look::LockLook,
}

impl LockScreen {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        demo: bool,
        palette: MandaraPalette,
        fonts: Fonts,
        clock_24h: bool,
        user: User,
        auth: AuthWorker,
        wallpaper: Option<PathBuf>,
        deadline: Option<Instant>,
    ) -> Self {
        let avatar = load_avatar(&user, 256);
        let mut reveal = SpringValue::new(0.0, REVEAL);
        reveal.set_target(1.0);
        let mut flash = SpringValue::new(0.0, SpringConfig::critical(90.0));
        flash.set_target(0.0);
        Self {
            demo,
            palette,
            fonts,
            clock_24h,
            avatar,
            wallpaper: None,
            wallpaper_rx: wallpaper.filter(|p| p.is_file()).map(|p| load_wallpaper(p, look::LockLook::default().blur_sigma())),
            look: look::LockLook::default(),
            backgrounds: HashMap::new(),
            text: TextPainter::new(),
            auth,
            password: String::with_capacity(MAX_PASSWORD),
            typed: Vec::new(),
            checking_len: 0,
            phase: Phase::Idle,
            note: None,
            keyboard: KeyboardState::default(),
            clock: FrameClock::new(),
            reveal,
            shake: SpringValue::new(0.0, SHAKE),
            exit: SpringValue::new(0.0, EXIT),
            flash,
            started: Instant::now(),
            minute: -1,
            dirty: true,
            animating: true,
            deadline,
            attempts: 0,
            was_locked: false,
            grace: None,
            grace_until: None,
            user,
        }
    }

    /// Applies the configured look and starts loading its background.
    pub fn set_look(&mut self, look: look::LockLook, background: Option<PathBuf>) {
        self.wallpaper_rx = background.filter(|p| p.is_file()).map(|p| load_wallpaper(p, look.blur_sigma()));
        self.wallpaper = None;
        self.clock_24h = look.clock_24h;
        self.look = look;
        self.backgrounds.clear();
        self.dirty = true;
    }

    fn reject(&mut self, note: Note) {
        self.shake.add_velocity([-2600.0, 0.0, 0.0, 0.0]);
        self.flash.snap_to(1.0);
        self.flash.set_target(0.0);
        self.note = Some(note);
        self.dirty = true;
    }

    fn clear_password(&mut self) {
        wipe(&mut self.password);
        self.typed.clear();
    }

    fn submit(&mut self) {
        if self.password.is_empty() {
            self.reject(Note::Info(t("Type your password").into()));
            return;
        }
        self.checking_len = self.typed.len();
        // Copy out and wipe in place: the buffer keeps its reserved
        // capacity, so typing never reallocates (and strands) secret bytes.
        let password = self.password.clone();
        self.clear_password();
        self.auth.submit(&self.user.name, password);
        self.phase = Phase::Checking;
        self.note = None;
        self.attempts += 1;
    }

    fn finish_attempt(&mut self, result: AuthResult) {
        if result.ok {
            self.phase = Phase::Unlocking;
            self.exit.set_target(1.0);
            self.note = None;
            return;
        }
        self.phase = Phase::Idle;
        self.checking_len = 0;
        // A module's own message (e.g. pam_faillock's lockout notice) says
        // more than the generic reason.
        let text = result
            .messages
            .iter()
            .rev()
            .find(|(_, error)| *error)
            .map(|(m, _)| m.clone())
            .or(result.error)
            .unwrap_or_else(|| t("Wrong password").into());
        self.reject(Note::Error(text));
    }

    fn time_strings(&self) -> (String, Option<String>) {
        use chrono::Timelike as _;
        let now = chrono::Local::now();
        // The pattern already carries the 24-hour and leading-zero choices
        // (`%H:%M`, `%I:%M`, `%-I:%M`) or the custom format.
        let time = crate::widgets::clock::format_with(&now, &self.look.time_pattern, LOCK_TIME_FALLBACK);
        if self.look.custom_time || self.clock_24h {
            return (time, None);
        }
        let (pm, _) = now.hour12();
        (time, Some(if pm { "PM".into() } else { "AM".into() }))
    }

    /// The scrimmed, blurred wallpaper (or palette glow field) for one
    /// surface size, with anti-banding grain. Cached per size.
    fn background(&mut self, w: u32, h: u32) -> &Pixmap {
        let palette = &self.palette;
        let wallpaper = &self.wallpaper;
        let solid = self.look.cfg.background == mandara_config::desktop::LockBackground::Color;
        let dim = self.look.cfg.dim.clamp(0.0, 1.0);
        self.backgrounds.entry((w, h)).or_insert_with(|| {
            // Sizes are clamped to ≥ 1 px, for which `Pixmap::new` cannot fail.
            let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("non-empty surface");
            let (fw, fh) = (w as f32, h as f32);
            {
                let mut m = pm.as_mut();
                m.fill(paint::skia(palette.surface_dim, 1.0));
                match wallpaper {
                    Some(img) => paint::image_cover(&mut m, img),
                    None if solid => {}
                    None => {
                        paint::glow(&mut m, fw * 0.18, fh * 0.22, fw * 0.62, paint::skia(palette.primary, 0.34));
                        paint::glow(&mut m, fw * 0.88, fh * 0.82, fw * 0.55, paint::skia(palette.tertiary, 0.28));
                        paint::glow(&mut m, fw * 0.62, fh * 0.05, fw * 0.42, paint::skia(palette.accent_gradient_end, 0.22));
                    }
                }
                // Scrim: keeps text legible on any wallpaper, deeper toward
                // the pill.
                // `lock.dim` sets the scrim (default 0.35 → the original 0.34 → 0.66 ramp).
                let scrim = paint::linear((0.0, 0.0), (0.0, fh), paint::skia(palette.surface_dim, dim * SCRIM_TOP), paint::skia(palette.surface_dim, (dim * SCRIM_BOTTOM).min(1.0)));
                if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, fw, fh) {
                    m.fill_rect(rect, &scrim, tiny_skia::Transform::identity(), None);
                }
                // A faint primary bloom behind the clock.
                paint::glow(&mut m, fw * 0.5, fh * 0.26, fw.min(fh) * 0.55, paint::skia(palette.primary_glow, 0.10));
                paint::grain(&mut m, 0.015);
            }
            pm
        })
    }

    /// Inside the grace window: any input unlocks (no password). Returns
    /// whether it did.
    fn in_grace(&mut self) -> bool {
        let inside = self.phase == Phase::Idle && self.grace_until.is_some_and(|until| Instant::now() < until);
        if inside {
            self.grace_until = None;
            self.finish_attempt(AuthResult { ok: true, error: None, messages: Vec::new() });
            self.dirty = true;
        }
        inside
    }

    fn dots_visible(&self) -> usize {
        if self.phase == Phase::Checking || self.phase == Phase::Unlocking { self.checking_len } else { self.typed.len() }
    }
}

/// Back-out easing for the dots' pop (slight overshoot).
fn ease_out_back(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (p - 1.0).powi(3) + c1 * (p - 1.0).powi(2)
}

impl LockApp for LockScreen {
    fn key(&mut self, key: KeyInput) {
        if self.phase != Phase::Idle {
            return;
        }
        if self.in_grace() {
            return;
        }
        self.dirty = true;
        match key.keysym {
            keys::ESCAPE => {
                if self.demo {
                    self.deadline = Some(Instant::now());
                } else {
                    self.clear_password();
                    self.note = None;
                }
            }
            keys::RETURN | keys::KP_ENTER => self.submit(),
            keys::BACKSPACE => {
                if key.ctrl {
                    self.clear_password();
                } else if self.password.pop().is_some() {
                    self.typed.pop();
                }
            }
            _ if key.ctrl && key.utf8 == "\u{15}" || key.ctrl && key.utf8.eq_ignore_ascii_case("u") => self.clear_password(),
            _ if key.ctrl || key.alt => {}
            _ => {
                let printable: String = key.utf8.chars().filter(|c| !c.is_control()).collect();
                for c in printable.chars() {
                    if self.password.len() + c.len_utf8() > MAX_PASSWORD {
                        break;
                    }
                    self.password.push(c);
                    self.typed.push(Instant::now());
                    if matches!(self.note, Some(Note::Info(_))) {
                        self.note = None;
                    }
                }
            }
        }
    }

    fn locked(&mut self) {
        self.was_locked = true;
        self.grace_until = self.grace.map(|g| Instant::now() + g);
        if !self.demo {
            let marker = locked_marker();
            if let Some(dir) = marker.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(marker, std::process::id().to_string());
        }
    }

    fn pointer_activity(&mut self) {
        self.in_grace();
    }

    fn keyboard(&mut self, state: &KeyboardState) {
        self.keyboard = state.clone();
        self.dirty = true;
    }

    fn update(&mut self, now: Instant) -> AppControl {
        if let Some(rx) = &self.wallpaper_rx
            && let Ok(img) = rx.try_recv()
        {
            self.wallpaper = Some(img);
            self.wallpaper_rx = None;
            self.backgrounds.clear();
            self.dirty = true;
        }
        if self.phase == Phase::Checking
            && let Some(result) = self.auth.poll()
        {
            self.finish_attempt(result);
            self.dirty = true;
        }
        let minute = chrono::Local::now().timestamp() / 60;
        if minute != self.minute {
            self.minute = minute;
            self.dirty = true;
        }
        let dt = self.clock.tick(now);
        let mut moving = false;
        for s in [&mut self.reveal, &mut self.shake, &mut self.exit, &mut self.flash] {
            moving |= s.step(dt);
        }
        let popping = self.typed.last().is_some_and(|t| now.duration_since(*t) < DOT_POP);
        self.animating = moving || popping || self.phase == Phase::Checking;
        if !self.animating {
            self.clock.idle();
        }
        if self.phase == Phase::Unlocking && self.exit.value() >= 0.97 {
            return AppControl::Unlock;
        }
        if self.deadline.is_some_and(|d| now >= d) {
            return AppControl::Exit;
        }
        AppControl::Continue
    }

    fn needs_redraw(&self) -> bool {
        self.dirty || self.animating
    }

    fn drawn(&mut self) {
        self.dirty = false;
    }

    fn next_wakeup(&self, now: Instant) -> Duration {
        if self.animating {
            return ANIMATION_TICK;
        }
        let mut wait = Duration::from_millis(1000 - (chrono::Local::now().timestamp_subsec_millis() % 1000) as u64 + 5);
        if let Some(d) = self.deadline {
            wait = wait.min(d.saturating_duration_since(now));
        }
        wait
    }

    fn draw(&mut self, info: &SurfaceInfo, pm: &mut PixmapMut<'_>, now: Instant) {
        let s = info.scale;
        let (w, h) = (pm.width(), pm.height());
        let bg = self.background(w, h);
        pm.data_mut().copy_from_slice(bg.data());
        let p = self.palette.clone();
        let (fw, fh) = (w as f32, h as f32);
        let cx = fw / 2.0;
        let reveal = self.reveal.value().clamp(0.0, 1.2);
        let exit = self.exit.value().clamp(0.0, 1.0);
        let presence = (reveal.min(1.0) * (1.0 - exit)).clamp(0.0, 1.0);
        // In the demo the whole surface fades (the desktop shows through);
        // a real lock keeps its opaque backdrop and fades only the content.
        let alpha = if self.demo { 1.0 } else { presence };
        let lift = (1.0 - reveal) * 28.0 * s - exit * 22.0 * s;
        let logical_h = info.height as f32;

        // Clock + date.
        let blank = !info.focused && self.look.cfg.blank_other_outputs && !self.demo;
        if blank {
            // OLED care: other outputs stay black.
            pm.fill(tiny_skia::Color::BLACK);
            return;
        }
        let clock_size = (logical_h * 0.15).clamp(72.0, 176.0) * s * if self.look.compact() { COMPACT_CLOCK } else { 1.0 };
        let (time, ampm) = self.time_strings();
        let stacked = self.look.stacked() && !self.look.custom_time;
        let clock_font = Font { family: &self.fonts.display, size: clock_size, weight: if stacked { STACK_WEIGHT } else { 250 }, tabular: true, tracking: -0.02 };
        let clock_y = fh * 0.17 + lift;
        if stacked {
            // Hours over minutes, centred; the date follows the block.
            let (hours, minutes) = time.split_once(':').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((time.clone(), String::new()));
            let (_, line_h) = self.text.measure(&hours, &clock_font);
            paint::glow(pm, cx, clock_y + line_h, line_h, paint::skia(p.primary_glow, 0.14 * alpha));
            self.text.draw_centered(pm, &hours, &clock_font, cx, clock_y, p.on_surface, alpha);
            self.text.draw_centered(pm, &minutes, &clock_font, cx, clock_y + line_h * STACK_LINE, p.primary, alpha);
            let th = line_h * (1.0 + STACK_LINE);
            self.draw_date_and_info(pm, cx, clock_y + th * 0.98, alpha, s);
            if !info.focused {
                if self.demo {
                    paint::fade(pm, presence);
                }
                return;
            }
            self.draw_lower(info, pm, now, (fw, fh, cx, lift, alpha, presence, s));
            return;
        }
        let (tw, th) = self.text.measure(&time, &clock_font);
        let ampm_font = Font { family: &self.fonts.ui, size: clock_size * 0.2, weight: 500, tabular: false, tracking: 0.06 };
        let ampm_w = ampm.as_ref().map(|a| self.text.measure(a, &ampm_font).0 + 10.0 * s).unwrap_or(0.0);
        let clock_x = cx - (tw + ampm_w) / 2.0;
        // Soft glow behind the digits.
        paint::glow(pm, cx, clock_y + th * 0.5, tw * 0.7, paint::skia(p.primary_glow, 0.14 * alpha));
        self.text.draw(pm, &time, &clock_font, clock_x, clock_y, p.on_surface, alpha);
        if let Some(a) = &ampm {
            self.text.draw(pm, a, &ampm_font, clock_x + tw + 10.0 * s, clock_y + th * 0.62, p.on_surface_variant, alpha);
        }
        self.draw_date_and_info(pm, cx, clock_y + th * 0.98, alpha, s);

        if !info.focused {
            if self.demo {
                paint::fade(pm, presence);
            }
            return;
        }
        self.draw_lower(info, pm, now, (fw, fh, cx, lift, alpha, presence, s));
    }

}

impl LockScreen {
    /// The date line and the info line (weather · now playing ·
    /// notifications) under the clock.
    fn draw_date_and_info(&mut self, pm: &mut PixmapMut<'_>, cx: f32, y: f32, alpha: f32, s: f32) {
        let p = self.palette.clone();
        let mut y = y;
        if self.look.cfg.show_date {
            let now = chrono::Local::now();
            let date = match &self.look.date_pattern {
                Some(pattern) => crate::widgets::clock::format_with(&now, pattern, t("%A, %-d %B")),
                None => crate::widgets::clock::localized(&now, t("%A, %-d %B")),
            };
            let date_font = Font { family: &self.fonts.ui, size: 19.0 * s, weight: 450, tabular: true, tracking: 0.01 };
            y += self.text.draw_centered(pm, &date, &date_font, cx, y, p.on_surface_variant, alpha * 0.95).1 + 8.0 * s;
        }
        if let Some(line) = self.look.info_line() {
            let info_font = Font { family: &self.fonts.ui, size: 14.0 * s, weight: 450, tabular: true, tracking: 0.01 };
            self.text.draw_centered(pm, &line, &info_font, cx, y, p.on_surface_variant, alpha * 0.8);
        }
    }

    /// Avatar, name, password pill, status and footer (focused output).
    #[allow(clippy::type_complexity)]
    fn draw_lower(&mut self, info: &SurfaceInfo, pm: &mut PixmapMut<'_>, now: Instant, (fw, fh, cx, lift, alpha, presence, s): (f32, f32, f32, f32, f32, f32, f32)) {
        let p = self.palette.clone();
        let _ = info;

        // Avatar with accent rim and glow.
        let ay = fh * 0.56 + lift * 0.6;
        let r = AVATAR_R * s;
        let show_avatar = self.look.cfg.show_avatar;
        if show_avatar {
        paint::glow(pm, cx, ay, r * 2.4, paint::skia(p.primary_glow, 0.28 * alpha));
        paint::soft_shadow(pm, cx - r, ay - r, r * 2.0, r * 2.0, r, 14.0 * s, 0.30 * alpha);
        match &self.avatar {
            Some(img) => paint::image_circle(pm, img, cx, ay, r, alpha),
            None => {
                let fillp = paint::linear((cx - r, ay - r), (cx + r, ay + r), paint::skia(p.accent_gradient_start, alpha), paint::skia(p.accent_gradient_end, alpha));
                paint::fill(pm, &paint::circle(cx, ay, r), &fillp);
                let mono = Font { family: &self.fonts.display, size: r * 0.82, weight: 500, tabular: false, tracking: 0.0 };
                let letters = initials(&self.user.display);
                let (_, lh) = self.text.measure(&letters, &mono);
                self.text.draw_centered(pm, &letters, &mono, cx, ay - lh / 2.0, p.on_primary, alpha);
            }
        }
        let rim = paint::linear((cx - r, ay - r), (cx + r, ay + r), paint::skia(p.accent_gradient_start, 0.95 * alpha), paint::skia(p.accent_gradient_end, 0.95 * alpha));
        paint::stroke(pm, &paint::circle(cx, ay, r + 3.0 * s), &rim, 2.5 * s);
        }

        let name_font = Font { family: &self.fonts.ui, size: 18.0 * s, weight: 600, tabular: false, tracking: 0.0 };
        // Without the avatar the name and field move up into its place.
        let name_y = if show_avatar { ay + r + 14.0 * s } else { ay };
        let (_, nh) = self.text.draw_centered(pm, &self.user.display, &name_font, cx, name_y, p.on_surface, alpha);

        // The glass password pill.
        let (pw, ph) = (PILL_W * s, PILL_H * s);
        let px = cx - pw / 2.0 + self.shake.value() * s;
        let py = name_y + nh + 18.0 * s;
        let radius = ph / 2.0;
        let pill = paint::rounded_rect(px, py, pw, ph, radius);
        paint::soft_shadow(pm, px, py, pw, ph, radius, 18.0 * s, 0.32 * alpha);
        paint::fill(pm, &pill, &paint::solid(p.surface_container_high, 0.58 * alpha));
        let sheen = paint::linear((px, py), (px, py + ph), paint::skia(Color::hex("#ffffff"), 0.07 * alpha), paint::skia(Color::hex("#ffffff"), 0.0));
        paint::fill(pm, &pill, &sheen);
        paint::glass_rim(pm, &pill, px, py, pw, ph, p.specular_rim_light, p.specular_rim_shadow, alpha, 1.0 * s);
        let flash = self.flash.value().clamp(0.0, 1.0);
        if flash > 0.01 {
            paint::stroke(pm, &pill, &paint::solid(p.urgent_coral, flash * alpha), 1.6 * s);
        }

        // Trailing button: lock glyph → arrow → spinner.
        let bx = px + pw - ph / 2.0;
        let by = py + ph / 2.0;
        let br = ph / 2.0 - 6.0 * s;
        let n = self.dots_visible();
        let active = n > 0 || self.phase != Phase::Idle;
        if active {
            let grad = paint::linear((bx - br, by - br), (bx + br, by + br), paint::skia(p.accent_gradient_start, alpha), paint::skia(p.accent_gradient_end, alpha));
            paint::fill(pm, &paint::circle(bx, by, br), &grad);
        }
        let ink = if active { p.on_primary } else { p.on_surface_variant };
        match self.phase {
            Phase::Checking => {
                let t = now.duration_since(self.started).as_secs_f32();
                let a0 = t * 7.0;
                let mut pb = tiny_skia::PathBuilder::new();
                let rr = br * 0.48;
                for i in 0..=24 {
                    let a = a0 + i as f32 / 24.0 * std::f32::consts::PI * 1.4;
                    let (x, y) = (bx + rr * a.cos(), by + rr * a.sin());
                    if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
                }
                let mut stroke = tiny_skia::Stroke { width: 2.2 * s, ..Default::default() };
                stroke.line_cap = tiny_skia::LineCap::Round;
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &paint::solid(ink, alpha), &stroke, tiny_skia::Transform::identity(), None);
                }
            }
            Phase::Unlocking => draw_check(pm, bx, by, br * 0.45, ink, alpha, s),
            Phase::Idle if active => draw_arrow(pm, bx, by, br * 0.45, ink, alpha, s),
            Phase::Idle => draw_padlock(pm, bx, by, br * 0.5, ink, alpha * 0.85, s),
        }

        // Dots (or placeholder).
        let field_l = px + 22.0 * s;
        let field_r = bx - br - 12.0 * s;
        let field_c = (field_l + field_r) / 2.0;
        if n == 0 && self.phase == Phase::Idle {
            let ph_font = Font { family: &self.fonts.ui, size: 15.0 * s, weight: 400, tabular: false, tracking: 0.0 };
            let label = t("Enter password");
            let (_, lh) = self.text.measure(label, &ph_font);
            self.text.draw_centered(pm, label, &ph_font, field_c, by - lh / 2.0, p.on_surface_variant, 0.75 * alpha);
        } else {
            let gap = 15.0 * s;
            let max_dots = (((field_r - field_l) / gap) as usize).max(1);
            let shown = n.min(max_dots);
            let start = field_c - (shown.saturating_sub(1)) as f32 * gap / 2.0;
            let t = now.duration_since(self.started).as_secs_f32();
            for i in 0..shown {
                let idx = n - shown + i;
                let pop = match (self.phase, self.typed.get(idx)) {
                    (Phase::Idle, Some(at)) => ease_out_back(now.duration_since(*at).as_secs_f32() / 0.22),
                    _ => 1.0,
                };
                let wave = if self.phase == Phase::Checking { 0.45 + 0.55 * ((t * 6.0 - i as f32 * 0.55).sin() * 0.5 + 0.5) } else { 1.0 };
                let dr = 4.6 * s * pop.max(0.0);
                paint::fill(pm, &paint::circle(start + i as f32 * gap, by, dr), &paint::solid(p.on_surface, alpha * wave));
            }
        }

        // Status line + Caps Lock.
        let mut y = py + ph + 14.0 * s;
        let note_font = Font { family: &self.fonts.ui, size: 14.0 * s, weight: 500, tabular: false, tracking: 0.0 };
        match &self.note {
            Some(Note::Error(m)) => y += self.text.draw_centered(pm, m, &note_font, cx, y, p.urgent_coral, alpha).1 + 6.0 * s,
            Some(Note::Info(m)) => y += self.text.draw_centered(pm, m, &note_font, cx, y, p.on_surface_variant, alpha).1 + 6.0 * s,
            None if self.phase == Phase::Checking => {
                y += self.text.draw_centered(pm, t("Checking…"), &note_font, cx, y, p.on_surface_variant, alpha * 0.8).1 + 6.0 * s;
            }
            None => {}
        }
        if self.keyboard.caps_lock {
            let caps = t("Caps Lock is on");
            let (cw, ch) = self.text.measure(caps, &note_font);
            let chip_w = cw + 34.0 * s;
            let chip_h = ch + 8.0 * s;
            let chip = paint::rounded_rect(cx - chip_w / 2.0, y, chip_w, chip_h, chip_h / 2.0);
            paint::fill(pm, &chip, &paint::solid(p.warning_amber, 0.16 * alpha));
            paint::fill(pm, &paint::circle(cx - chip_w / 2.0 + 14.0 * s, y + chip_h / 2.0, 3.5 * s), &paint::solid(p.warning_amber, alpha));
            self.text.draw(pm, caps, &note_font, cx - chip_w / 2.0 + 24.0 * s, y + 4.0 * s, p.warning_amber, alpha);
        }

        // Footer: demo hint, keyboard layout.
        let foot = Font { family: &self.fonts.ui, size: 12.5 * s, weight: 450, tabular: false, tracking: 0.02 };
        if self.demo {
            self.text.draw_centered(pm, t("Preview — any password unlocks · Esc closes"), &foot, cx, fh - 44.0 * s, p.on_surface_variant, 0.8 * alpha);
        }
        if !self.keyboard.layout.is_empty() {
            let (lw, _) = self.text.measure(&self.keyboard.layout, &foot);
            self.text.draw(pm, &self.keyboard.layout, &foot, fw - lw - 28.0 * s, fh - 44.0 * s, p.on_surface_variant, 0.65 * alpha);
        }
        if self.look.cfg.show_battery
            && let Some((pct, charging)) = look::battery()
        {
            let label = if charging { format!("{pct}% ⚡") } else { format!("{pct}%") };
            let ink = if pct <= self.look.cfg.low_battery() && !charging { p.urgent_coral } else { p.on_surface_variant };
            self.text.draw(pm, &label, &foot, 28.0 * s, fh - 44.0 * s, ink, 0.75 * alpha);
        }
        if self.demo {
            paint::fade(pm, presence);
        }
    }
}

fn line_paint(c: Color, alpha: f32) -> tiny_skia::Paint<'static> {
    paint::solid(c, alpha)
}

fn round_stroke(width: f32) -> tiny_skia::Stroke {
    tiny_skia::Stroke { width, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() }
}

fn draw_arrow(pm: &mut PixmapMut<'_>, cx: f32, cy: f32, r: f32, c: Color, alpha: f32, s: f32) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx - r, cy);
    pb.line_to(cx + r, cy);
    pb.move_to(cx + r * 0.25, cy - r * 0.75);
    pb.line_to(cx + r, cy);
    pb.line_to(cx + r * 0.25, cy + r * 0.75);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &line_paint(c, alpha), &round_stroke(2.2 * s), tiny_skia::Transform::identity(), None);
    }
}

fn draw_check(pm: &mut PixmapMut<'_>, cx: f32, cy: f32, r: f32, c: Color, alpha: f32, s: f32) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx - r, cy);
    pb.line_to(cx - r * 0.25, cy + r * 0.7);
    pb.line_to(cx + r, cy - r * 0.65);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &line_paint(c, alpha), &round_stroke(2.4 * s), tiny_skia::Transform::identity(), None);
    }
}

fn draw_padlock(pm: &mut PixmapMut<'_>, cx: f32, cy: f32, r: f32, c: Color, alpha: f32, s: f32) {
    let body_w = r * 1.7;
    let body_h = r * 1.3;
    let top = cy - body_h * 0.15;
    paint::fill(pm, &paint::rounded_rect(cx - body_w / 2.0, top, body_w, body_h, r * 0.3), &line_paint(c, alpha));
    let mut pb = tiny_skia::PathBuilder::new();
    let sr = r * 0.55;
    pb.move_to(cx - sr, top);
    pb.line_to(cx - sr, top - sr * 0.6);
    pb.cubic_to(cx - sr, top - sr * 1.9, cx + sr, top - sr * 1.9, cx + sr, top - sr * 0.6);
    pb.line_to(cx + sr, top);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &line_paint(c, alpha), &round_stroke(1.8 * s), tiny_skia::Transform::identity(), None);
    }
}

// ─────────────────────────── shell integration ───────────────────────────

/// Locker processes started by this shell.
#[derive(Default)]
struct Children {
    /// The real locker is running (its supervisor thread is alive).
    real: bool,
    demo: Option<u32>,
}

static CHILDREN: Mutex<Children> = Mutex::new(Children { real: false, demo: None });
/// `general.lock_screen`: answer logind lock requests.
static ANSWER_LOGIND: AtomicBool = AtomicBool::new(true);

/// Environment variable passing the grace period (ms) to the locker.
const GRACE_ENV: &str = "MANDARA_LOCK_GRACE_MS";

/// `$XDG_RUNTIME_DIR/mandara/locked`: exists while a real locker has the
/// session locked (written once the compositor confirms the lock), so the
/// shell can hold a suspend back until the screen really is locked.
pub fn locked_marker() -> PathBuf {
    dirs::runtime_dir().unwrap_or_else(mandara_config::paths::cache_dir).join("mandara").join("locked")
}

fn grace_from_env() -> Option<Duration> {
    let ms: u64 = std::env::var(GRACE_ENV).ok()?.parse().ok()?;
    (ms > 0).then(|| Duration::from_millis(ms))
}

/// Whether a real locker started by this shell is running.
pub fn is_locked() -> bool {
    CHILDREN.lock().is_ok_and(|c| c.real)
}

/// Listeners told about lock (`true`) / unlock (`false`).
static LISTENERS: Mutex<Vec<async_channel::Sender<bool>>> = Mutex::new(Vec::new());

/// A stream of lock (`true`) / unlock (`false`) transitions.
pub fn subscribe() -> async_channel::Receiver<bool> {
    let (tx, rx) = async_channel::unbounded();
    if let Ok(mut l) = LISTENERS.lock() {
        l.push(tx);
    }
    rx
}

fn announce(locked: bool) {
    if let Ok(mut l) = LISTENERS.lock() {
        l.retain(|tx| tx.try_send(locked).is_ok());
    }
    let event = if locked { mandara_config::power::HookEvent::Locked } else { mandara_config::power::HookEvent::Unlocked };
    mandara_services::hooks::emit(event, &[]);
}

fn locker_command(demo: bool) -> std::process::Command {
    use std::os::unix::process::CommandExt as _;
    let mut cmd = std::process::Command::new(mandara_config::paths::self_exe());
    cmd.arg("lock");
    if demo {
        cmd.args(["--demo", "--timeout", "60"]);
    }
    // Its own process group: the locker must outlive a dying shell.
    cmd.stdin(std::process::Stdio::null()).process_group(0);
    // What only the shell knows (now playing, unread count, weather).
    look::export(&mut cmd);
    cmd
}

/// Whether a locker exit means "it crashed while the session may still be
/// locked" (signal or panic) rather than "unlocked" / "refused".
pub fn crashed(status: std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().is_some() || matches!(status.code(), Some(101) | Some(RELOCK_EXIT))
}

/// Starts the real locker, restarting it if it crashes (the compositor
/// keeps the session locked meanwhile). No-op while one is running.
pub fn lock_now() -> Result<bool, String> {
    lock_with_grace(None)
}

/// Like [`lock_now`]; input within `grace` of the lock unlocks without a
/// password (automatic idle locks, `idle.lock_grace_s`).
pub fn lock_with_grace(grace: Option<Duration>) -> Result<bool, String> {
    let mut children = CHILDREN.lock().map_err(|e| e.to_string())?;
    if children.real {
        return Ok(false);
    }
    if mandara_services::sleep::dry_run() {
        tracing::info!("dry run: lock the screen (grace {grace:?})");
        return Ok(false);
    }
    let mut command = locker_command(false);
    if let Some(g) = grace {
        command.env(GRACE_ENV, g.as_millis().to_string());
    }
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", t("Could not start the lock screen")))?;
    children.real = true;
    drop(children);
    announce(true);
    std::thread::Builder::new()
        .name("mandara-lock-supervisor".into())
        .spawn(move || {
            let mut restarts: Vec<Instant> = Vec::new();
            loop {
                let status = child.wait();
                let again = status.as_ref().is_ok_and(|s| crashed(*s));
                if !again {
                    break;
                }
                restarts.retain(|t| t.elapsed() < RESTART_WINDOW);
                if restarts.len() >= 5 {
                    tracing::error!("lock screen crashed {} times in a minute; giving up (unlock from a TTY with `loginctl unlock-session`)", restarts.len());
                    break;
                }
                restarts.push(Instant::now());
                tracing::warn!("lock screen exited abnormally ({status:?}); restarting it");
                match locker_command(false).spawn() {
                    Ok(c) => child = c,
                    Err(err) => {
                        tracing::error!("restarting the lock screen: {err}");
                        break;
                    }
                }
            }
            if let Ok(mut c) = CHILDREN.lock() {
                c.real = false;
            }
            let _ = std::fs::remove_file(locked_marker());
            announce(false);
        })
        .map_err(|e| e.to_string())?;
    Ok(true)
}

fn start_demo() -> Result<u32, String> {
    stop_demo();
    let mut child = locker_command(true).spawn().map_err(|e| format!("{}: {e}", t("Could not start the lock screen")))?;
    let pid = child.id();
    CHILDREN.lock().map_err(|e| e.to_string())?.demo = Some(pid);
    let _ = std::thread::Builder::new().name("mandara-lock-demo".into()).spawn(move || {
        let _ = child.wait();
        if let Ok(mut c) = CHILDREN.lock()
            && c.demo == Some(pid)
        {
            c.demo = None;
        }
    });
    Ok(pid)
}

/// Closes a running demo (never touches the real locker).
fn stop_demo() -> bool {
    let pid = CHILDREN.lock().ok().and_then(|mut c| c.demo.take());
    if let Some(pid) = pid {
        // SAFETY: plain kill(2) on a child we spawned (reaped by its thread).
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        return true;
    }
    false
}

/// Shell startup: answer logind lock requests while `general.lock_screen`.
pub fn init(cx: &mut gpui::App) {
    look::init(cx);
    let store = crate::state::config(cx);
    ANSWER_LOGIND.store(store.read(cx).get().general.lock_screen, Ordering::Relaxed);
    cx.observe(&store, |store, cx| ANSWER_LOGIND.store(store.read(cx).get().general.lock_screen, Ordering::Relaxed)).detach();
    mandara_services::logind::watch_lock_requests(|| {
        if ANSWER_LOGIND.load(Ordering::Relaxed)
            && let Err(err) = lock_now()
        {
            tracing::error!("lock screen: {err}");
        }
    });
}

/// `lock [--demo | --stop-demo]`.
pub fn register(r: &mut PluginRegistry) {
    r.command(
        "lock",
        CommandSpec::new("lock", tk("Lock the screen (--demo previews it without locking; --stop-demo closes the preview)"), |a, _| match a.get(0) {
            Some("--demo") => Ok(json!({ "demo": true, "pid": start_demo()? })),
            Some("--stop-demo") => Ok(json!({ "demo": false, "stopped": stop_demo() })),
            _ => Ok(json!({ "locked": true, "started": lock_now()? })),
        })
        .arg(ArgSpec::optional("mode", ArgKind::Enum, tk("--demo shows the lock screen as a harmless preview")).choices(&["--demo", "--stop-demo"]))
        .example("lock")
        .example("lock --demo"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Accept;
    impl Authenticator for Accept {
        fn authenticate(&mut self, _: &str, password: &str) -> AuthResult {
            AuthResult { ok: password == "open sesame", error: Some("nope".into()), messages: vec![("Account locked for 10 minutes".into(), true)] }
        }
    }

    fn screen(demo: bool) -> LockScreen {
        let user = User { name: "amira".into(), display: "Amira Said".into(), home: PathBuf::from("/nonexistent") };
        LockScreen::new(
            demo,
            mandara_theme::engine::default_palette(true),
            Fonts { display: "Inter".into(), ui: "Inter".into() },
            true,
            user,
            AuthWorker::spawn(Box::new(Accept)).unwrap(),
            None,
            None,
        )
    }

    fn type_text(s: &mut LockScreen, text: &str) {
        for c in text.chars() {
            s.key(KeyInput { keysym: c as u32, utf8: c.to_string(), ctrl: false, alt: false });
        }
    }

    fn press(s: &mut LockScreen, keysym: u32) {
        s.key(KeyInput { keysym, utf8: String::new(), ctrl: false, alt: false });
    }

    fn settle(s: &mut LockScreen) -> AppControl {
        let mut now = Instant::now();
        for _ in 0..2000 {
            now += Duration::from_millis(8);
            let c = s.update(now);
            if c != AppControl::Continue || (s.phase != Phase::Checking && !s.animating) {
                return c;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        AppControl::Continue
    }

    #[test]
    fn parses_passwd_entries() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nlayla:x:1000:1000:Layla Nour,,,:/home/layla:/usr/bin/fish\nbare:x:1001:1001::/home/bare:/bin/sh\n";
        assert_eq!(parse_passwd(passwd, 1000).unwrap().display, "Layla Nour");
        assert_eq!(parse_passwd(passwd, 1001).unwrap().display, "bare");
        assert!(parse_passwd(passwd, 4242).is_none());
        assert_eq!(initials("Layla Nour"), "LN");
    }

    #[test]
    fn wrong_password_shakes_and_shows_the_module_message() {
        let mut s = screen(false);
        type_text(&mut s, "guess");
        assert_eq!(s.typed.len(), 5);
        press(&mut s, keys::RETURN);
        assert_eq!(s.phase, Phase::Checking);
        assert!(s.password.is_empty(), "the password leaves the UI on submit");
        assert_eq!(settle(&mut s), AppControl::Continue);
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.note, Some(Note::Error("Account locked for 10 minutes".into())));
        assert_eq!(s.attempts, 1);
    }

    #[test]
    fn right_password_unlocks_after_the_exit_animation() {
        let mut s = screen(false);
        type_text(&mut s, "open sesame");
        press(&mut s, keys::RETURN);
        assert_eq!(settle(&mut s), AppControl::Unlock);
    }

    #[test]
    fn editing_keys() {
        let mut s = screen(false);
        type_text(&mut s, "abc");
        press(&mut s, keys::BACKSPACE);
        assert_eq!(s.password, "ab");
        s.key(KeyInput { keysym: keys::BACKSPACE, utf8: String::new(), ctrl: true, alt: false });
        assert!(s.password.is_empty() && s.typed.is_empty());
        // Empty submit nudges instead of calling PAM.
        press(&mut s, keys::RETURN);
        assert_eq!(s.phase, Phase::Idle);
        assert!(matches!(s.note, Some(Note::Info(_))));
        // Escape clears on a real lock but never exits.
        type_text(&mut s, "xy");
        press(&mut s, keys::ESCAPE);
        assert!(s.password.is_empty());
        assert_eq!(s.update(Instant::now()), AppControl::Continue);
    }

    #[test]
    fn demo_escape_exits() {
        let mut s = screen(true);
        press(&mut s, keys::ESCAPE);
        assert_eq!(s.update(Instant::now() + Duration::from_millis(1)), AppControl::Exit);
    }

    #[test]
    fn crash_detection() {
        use std::os::unix::process::ExitStatusExt as _;
        assert!(crashed(std::process::ExitStatus::from_raw(9))); // SIGKILL
        assert!(crashed(std::process::ExitStatus::from_raw(101 << 8))); // panic
        assert!(!crashed(std::process::ExitStatus::from_raw(0)));
        assert!(!crashed(std::process::ExitStatus::from_raw(1 << 8))); // refused
        assert!(crashed(std::process::ExitStatus::from_raw(RELOCK_EXIT << 8)));
    }

    #[test]
    fn draws_a_frame_without_panicking() {
        let mut s = screen(false);
        let mut pm = Pixmap::new(320, 240).unwrap();
        let info = SurfaceInfo { output: "TEST-1".into(), width: 320, height: 240, scale: 1.0, focused: true };
        s.keyboard(&KeyboardState { caps_lock: true, layout: "English (US)".into() });
        type_text(&mut s, "ab");
        s.draw(&info, &mut pm.as_mut(), Instant::now());
        // The backdrop is opaque on a real lock.
        assert!(pm.data().chunks_exact(4).all(|p| p[3] == 255));
    }

    /// Renders typing / error / Caps Lock states to PNGs for a visual check:
    /// `MANDARA_LOCK_SNAPSHOTS=/tmp/x cargo test -p mandara-ui lock_snapshots -- --ignored`.
    #[test]
    #[ignore]
    fn lock_snapshots() {
        let Some(dir) = std::env::var_os("MANDARA_LOCK_SNAPSHOTS").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let save = |s: &mut LockScreen, name: &str| {
            let (w, h) = (1280u32, 800u32);
            let mut pm = Pixmap::new(w * 3 / 2, h * 3 / 2).unwrap();
            let info = SurfaceInfo { output: "TEST-1".into(), width: w, height: h, scale: 1.5, focused: true };
            s.draw(&info, &mut pm.as_mut(), Instant::now() + Duration::from_secs(1));
            let rgba: Vec<u8> = pm.pixels().iter().flat_map(|p| { let c = p.demultiply(); [c.red(), c.green(), c.blue(), c.alpha()] }).collect();
            image::RgbaImage::from_raw(pm.width(), pm.height(), rgba).unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        };
        let mut s = screen(false);
        s.reveal.snap_to(1.0);
        type_text(&mut s, "secret");
        s.keyboard(&KeyboardState { caps_lock: true, layout: "English (US)".into() });
        save(&mut s, "typing");
        press(&mut s, keys::RETURN);
        settle(&mut s);
        s.flash.snap_to(1.0);
        s.shake.snap_to(-14.0);
        save(&mut s, "error");
    }
}
