//! The app installer's on-screen UI, on its own `mandara-installer` layer
//! surface (top-center, opened only while jobs exist):
//!
//! * a chromatic-glass **sheet** per file: the app's icon large, name,
//!   version and developer, summary, chips (format with a plain-language
//!   explanation on hover, size, categories), screenshots shipped in the
//!   file, the description, where it goes, its PATH command, a Flatpak's
//!   permissions as friendly rows, a native package's dependencies
//!   (collapsible) and password note, warnings, and Update / Reinstall /
//!   Keep both against the installed version;
//! * a **queue sheet** when several files are dropped at once (per-file
//!   status, Install all / Review each);
//! * a **progress pill** with a smooth ring, phase steps and elapsed time,
//!   ending in an icon bounce and check morph with Open, Keep in Dock and
//!   Show in Applications;
//! * a **failure card**: a plain explanation, a fix where there is one, and
//!   the technical detail behind a disclosure.
//!
//! Keyboard: Enter runs the focused action (the primary one by default),
//! Tab / Shift+Tab move between actions, Escape cancels or closes. Only the
//! sheet / pill take input (`set_input_region`). Images (icon, screenshots)
//! decode only while shown and are dropped from GPUI's cache when the
//! surface closes. [`drop_zone`] is the reusable "Drop to install" target.
//! Reduce motion makes every spring jump (the spinner stops turning).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    AnyElement, AnyView, App, AppContext as _, BoxShadow, ClickEvent, Context, ExternalPaths, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, ObjectFit, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Task, Window, WindowHandle, canvas, div, point, prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, InputMaskRegistry, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui, glass_rim, oklab_gradient};
use mandara_i18n::{t, tf};
use mandara_services::app_install::flatpak::Permissions;
use mandara_services::app_install::{Family, OnDuplicate, Phase, Plan, human_bytes, tilde};
use mandara_theme::{Color, MandaraPalette};

use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::plugins::app_installer::{self as installer, InstallOrigin, Job, Sheet};

// Surface.
/// Room around the card for its shadow.
const SHADOW_ROOM_PX: f32 = 28.0;
/// Below the bar (the surface respects other surfaces' exclusive zones).
const TOP_MARGIN_PX: f32 = 6.0;
/// Card height changes smaller than this don't resize the container.
const MEASURE_EPSILON_PX: f32 = 0.5;
const CARD_W_PX: f32 = 520.0;
/// The tallest sheet (longer ones scroll inside).
const CARD_MAX_H_PX: f32 = 700.0;
const CARD_GUESS_H_PX: f32 = 420.0;
const SURFACE_W_PX: f32 = CARD_W_PX + 2.0 * SHADOW_ROOM_PX;
const SURFACE_H_PX: f32 = CARD_MAX_H_PX + 2.0 * SHADOW_ROOM_PX;
// Sheet.
const CARD_PAD_PX: f32 = 24.0;
const CARD_RADIUS_PX: f32 = 30.0;
const CARD_GAP_PX: f32 = 16.0;
const HERO_GAP_PX: f32 = 18.0;
const APP_ICON_PX: f32 = 80.0;
const APP_TILE_PX: f32 = 100.0;
const APP_TILE_RADIUS_PX: f32 = 26.0;
const HERO_GLOW_BLUR_PX: f32 = 34.0;
const HERO_GLOW_ALPHA: f32 = 0.32;
const TITLE_PX: f32 = 22.0;
const TITLE_LEADING_PX: f32 = 28.0;
const BODY_PX: f32 = 13.0;
const SMALL_PX: f32 = 11.5;
const MONO_PX: f32 = 11.0;
const LEADING_PX: f32 = 18.0;
const CHIP_H_PX: f32 = 22.0;
const CHIP_PAD_PX: f32 = 9.0;
const ROW_GAP_PX: f32 = 8.0;
const STEP_DOT_PX: f32 = 5.0;
/// Title / chip rows and step lists.
const TIGHT_GAP_PX: f32 = 4.0;
const CHIP_GAP_PX: f32 = 6.0;
/// Inset of the duplicate choice's segments.
const SEGMENT_INSET_PX: f32 = 3.0;
/// Error / warning notices.
const NOTICE_PAD_PX: f32 = 12.0;
const NOTICE_GAP_PX: f32 = 10.0;
const NOTICE_LINE_GAP_PX: f32 = 2.0;
const NOTICE_RADIUS_PX: f32 = 14.0;
const NOTICE_BAR_W_PX: f32 = 3.0;
/// Info rows (where, command, permissions).
const ROWS_PAD_PX: f32 = 6.0;
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_PAD_Y_PX: f32 = 9.0;
const ROWS_RADIUS_PX: f32 = 18.0;
const ROW_ICON_TILE_PX: f32 = 28.0;
const ROW_ICON_RADIUS_PX: f32 = 9.0;
/// Screenshots shown above the description.
const SHOT_H_PX: f32 = 128.0;
const SHOT_RADIUS_PX: f32 = 14.0;
/// The description scrolls past this.
const DESCRIPTION_MAX_H_PX: f32 = 132.0;
/// Glyph sizes: buttons, small inline, row leaders.
const GLYPH_PX: f32 = 14.0;
const GLYPH_SMALL_PX: f32 = 13.0;
const GLYPH_ROW_PX: f32 = 15.0;
/// A stand-in glyph's size relative to the icon it replaces.
const FALLBACK_GLYPH_SCALE: f32 = 0.62;
const GLOW_Y_PX: f32 = 4.0;
const PILL_GAP_PX: f32 = 12.0;
const BUTTON_H_PX: f32 = 40.0;
const BUTTON_PAD_PX: f32 = 18.0;
const FOCUS_RING_PX: f32 = 2.0;
const SWITCH_W_PX: f32 = 36.0;
const SWITCH_H_PX: f32 = 20.0;
const KNOB_PX: f32 = 16.0;
const SEGMENT_H_PX: f32 = 30.0;
const TIP_PAD_X_PX: f32 = 10.0;
const TIP_PAD_Y_PX: f32 = 6.0;
const TIP_RADIUS_PX: f32 = 10.0;
const TIP_MAX_W_PX: f32 = 260.0;
/// Categories shown as chips.
const MAX_CATEGORY_CHIPS: usize = 3;
// Queue sheet.
const QUEUE_ROW_H_PX: f32 = 52.0;
const QUEUE_ICON_PX: f32 = 30.0;
const QUEUE_TILE_PX: f32 = 38.0;
const QUEUE_TILE_RADIUS_PX: f32 = 11.0;
// Pill.
const PILL_W_PX: f32 = 460.0;
const PILL_RUN_H_PX: f32 = 92.0;
const PILL_DONE_H_PX: f32 = 150.0;
const PILL_PAD_PX: f32 = 16.0;
const PILL_ICON_PX: f32 = 34.0;
const RING_PX: f32 = 56.0;
const RING_STROKE_PX: f32 = 3.5;
const PILL_BUTTON_H_PX: f32 = 32.0;
const PILL_BUTTON_PAD_PX: f32 = 14.0;
const ICON_BUTTON_PX: f32 = 28.0;
const PHASE_DOT_PX: f32 = 6.0;
/// The check badge on a finished icon.
const CHECK_BADGE_PX: f32 = 22.0;
const CHECK_GLYPH_PX: f32 = 13.0;
/// Indeterminate ring: sweep and turns per second.
const SPIN_SWEEP: f32 = 0.28;
const SPIN_HZ: f32 = 0.9;
/// The elapsed time ticks this often while installing.
const ELAPSED_TICK: Duration = Duration::from_secs(1);
// Colours.
const CARD_ALPHA: f32 = 0.97;
const TINT_ALPHA: f32 = 0.14;
const ROWS_ALPHA: f32 = 0.045;
const OUTLINE_ALPHA: f32 = 0.12;
const MUTED_ALPHA: f32 = 0.72;
const TRACK_ALPHA: f32 = 0.16;
const SHADOW_ALPHA: f32 = 0.34;
const SHADOW_BLUR_PX: f32 = 22.0;
const SHADOW_Y_PX: f32 = 10.0;
const GLOW_ALPHA: f32 = 0.4;
const GLOW_BLUR_PX: f32 = 14.0;
const DISABLED_ALPHA: f32 = 0.5;
const HOVER_ALPHA: f32 = 0.08;
const GRADIENT_DEG: f32 = 135.0;
// Motion.
const APPEAR: SpringConfig = SpringConfig::new(320.0, 26.0, 1.0);
const MORPH: SpringConfig = SpringConfig::new(260.0, 24.0, 1.0);
/// Underdamped: the finished icon overshoots and settles.
const BOUNCE: SpringConfig = SpringConfig::new(420.0, 11.0, 1.0);
const BOUNCE_FROM: f32 = 1.32;
/// The ring follows progress smoothly; the check badge pops in.
const RING_SPRING: SpringConfig = SpringConfig::new(200.0, 26.0, 1.0);
const CHECK_SPRING: SpringConfig = SpringConfig::new(380.0, 16.0, 1.0);
const DISAPPEAR_STIFFNESS: f32 = 420.0;
const RISE_PX: f32 = 18.0;
const GONE: f32 = 0.01;
const FADE_EDGE: f32 = 0.5;
// Drop zone.
const DROP_GLOW_BLUR_PX: f32 = 26.0;
const DROP_GLOW_ALPHA: f32 = 0.45;
const DROP_FILL_ALPHA: f32 = 0.10;
const DROP_LABEL_PX: f32 = 13.0;
const DROP_LABEL_H_PX: f32 = 32.0;
const DROP_LABEL_PAD_PX: f32 = 14.0;
/// Flathub's search, for "find it on Flathub" (the app's name appended).
const FLATHUB_SEARCH: &str = "https://flathub.org/apps/search?q=";
/// Studio's Applications page.
const STUDIO_APPS: &str = "applications/installed";
/// Studio's installer settings.
const STUDIO_INSTALLER: &str = "plugins/app-installer/settings";

/// Disclosures on a sheet (bits of [`InstallerOverlay::open`]).
const OPEN_DEPENDENCIES: u8 = 1;
const OPEN_STEPS: u8 = 2;
const OPEN_DETAIL: u8 = 4;

struct OverlayWindow(Option<WindowHandle<InstallerOverlay>>);
impl Global for OverlayWindow {}

fn is_open(cx: &App) -> bool {
    cx.try_global::<OverlayWindow>().and_then(|o| o.0.as_ref()).is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

/// Opens the surface when there is something to show (idempotent).
pub fn ensure_open(cx: &mut App) {
    if is_open(cx) {
        return;
    }
    let q = installer::queue(cx);
    if q.read(cx).jobs().iter().all(|j| j.background) {
        return;
    }
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Installer, gpui::size(px(SURFACE_W_PX), px(SURFACE_H_PX)), None).with_keyboard(KeyboardInteractivity::OnDemand);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP;
    spec.exclusive_zone = None;
    spec.margin = (px(TOP_MARGIN_PX), px(0.0), px(0.0), px(0.0));
    let handle = cx.open_window(spec.window_options(), |window, cx| cx.new(|cx| InstallerOverlay::new(window, cx))).map_err(|e| tracing::error!("installer overlay: {e:#}")).ok();
    cx.set_global(OverlayWindow(handle));
}

/// What a format is, in plain words (the format chip's tooltip).
pub fn format_explanation(family: Family) -> &'static str {
    match family {
        Family::AppImage => mandara_i18n::tk("AppImage: a portable app that runs from your home folder. No password needed."),
        Family::Portable => mandara_i18n::tk("Archive: a portable app unpacked into your home folder. No password needed."),
        Family::Flatpak => mandara_i18n::tk("Flatpak: a sandboxed app that only reaches what its permissions allow."),
        Family::Native => mandara_i18n::tk("Package: installed system-wide by your package manager, for every user."),
    }
}

/// One friendly row of a Flatpak's permissions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRow {
    pub glyph: &'static str,
    pub title: String,
    pub detail: String,
    /// Reaches far beyond the sandbox (drawn in the warning colour).
    pub broad: bool,
}

/// A Flatpak's `[Context]` as rows a person understands, broadest first.
pub fn permission_rows(p: &Permissions) -> Vec<PermissionRow> {
    let row = |glyph, title: &'static str, detail: String, broad| PermissionRow { glyph, title: t(title).to_string(), detail, broad };
    let has = |list: &[String], v: &str| list.iter().any(|x| x == v || x.starts_with(&format!("{v}:")));
    let mut rows = Vec::new();
    let fs = &p.filesystems;
    if has(fs, "host") || has(fs, "host-os") || has(fs, "host-etc") {
        rows.push(row("folder", mandara_i18n::tk("All your files and the system's"), t("It can read and change any file").to_string(), true));
    } else if has(fs, "home") {
        rows.push(row("folder", mandara_i18n::tk("Your home folder"), t("It can read and change your files").to_string(), true));
    } else if !fs.is_empty() {
        let names: Vec<String> = fs.iter().map(|f| f.split(':').next().unwrap_or(f).trim_start_matches("xdg-").to_string()).collect();
        rows.push(row("folder", mandara_i18n::tk("Some folders"), names.join(", "), false));
    }
    if has(&p.devices, "all") {
        rows.push(row("usb", mandara_i18n::tk("All devices"), t("Cameras, USB devices, game controllers…").to_string(), true));
    } else if has(&p.devices, "dri") {
        rows.push(row("monitor", mandara_i18n::tk("Graphics acceleration"), t("Uses your GPU").to_string(), false));
    }
    if has(&p.sockets, "system-bus") || has(&p.sockets, "session-bus") {
        rows.push(row("shield", mandara_i18n::tk("System services"), t("It can talk to other apps and services directly").to_string(), true));
    }
    if has(&p.sockets, "ssh-auth") || has(&p.sockets, "gpg-agent") {
        rows.push(row("key", mandara_i18n::tk("Your keys"), t("It can use your SSH or GPG keys").to_string(), true));
    }
    if has(&p.shared, "network") {
        rows.push(row("globe", mandara_i18n::tk("Network access"), t("It can connect to the internet").to_string(), false));
    }
    if has(&p.sockets, "pulseaudio") {
        rows.push(row("music", mandara_i18n::tk("Sound"), t("It can play sound and may use the microphone").to_string(), false));
    }
    if has(&p.sockets, "cups") {
        rows.push(row("note", mandara_i18n::tk("Printing"), String::new(), false));
    }
    rows
}

/// What to offer after a failure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    TryAgain,
    /// Open the App Installer's settings in Studio.
    Settings,
    /// Search Flathub for the app.
    Flathub,
    /// Open the folder the app was going into (space / size limits).
    OpenFolder,
    None,
}

/// A failure in plain words, and the fix to offer. `place` is where the
/// app was going (rootless installs).
pub fn explain_failure(error: &str, family: Option<Family>, place: Option<&Path>) -> (String, Fix) {
    let e = error.to_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| e.contains(w));
    // Space and size limits while copying or unpacking: ENOSPC, EDQUOT, EFBIG.
    if says(&["no space left", "os error 28", "disk quota", "os error 122", "quota exceeded"]) {
        let fix = if place.is_some() { Fix::OpenFolder } else { Fix::None };
        return match place {
            Some(dir) => (tf("Not enough space in {dir}.", &[("dir", &tilde(dir))]), fix),
            None => (t("There isn't enough free space on this disk.").to_string(), fix),
        };
    }
    if says(&["file too large", "os error 27"]) {
        return (t("The file is larger than this location allows.").to_string(), if place.is_some() { Fix::OpenFolder } else { Fix::None });
    }
    if says(&["couldn't read", "isn't an app", "isn't a valid", "doesn't name", "can't open"]) {
        (t("This file looks damaged, or isn't an app. Try downloading it again.").to_string(), Fix::None)
    } else if says(&["not authorized", "authentication", "dismissed", "request denied", "polkit"]) {
        (t("Installing needed your password, and it wasn't given.").to_string(), Fix::TryAgain)
    } else if says(&["disk full"]) {
        (t("There isn't enough free space on this disk.").to_string(), Fix::None)
    } else if says(&["turned off in the app installer settings"]) {
        (t("Installing system packages is turned off.").to_string(), Fix::Settings)
    } else if says(&["another distribution", "isn't installed", "not installed", "no package manager", "can't install"]) && family == Some(Family::Native) {
        (t("This package is made for a different kind of Linux. The same app may be on Flathub.").to_string(), Fix::Flathub)
    } else if says(&["conflict", "unsatisfiable", "nothing provides", "broken", "depend"]) {
        (t("Your package manager couldn't fit this package with what is already installed.").to_string(), if family == Some(Family::Native) { Fix::Flathub } else { Fix::None })
    } else if says(&["permission denied", "read-only"]) {
        (t("Mandara couldn't write where this app goes.").to_string(), Fix::TryAgain)
    } else if says(&["network", "resolve", "timed out", "connection"]) {
        (t("The download didn't get through. Check your connection.").to_string(), Fix::TryAgain)
    } else {
        (t("Something went wrong while installing. The details say what.").to_string(), Fix::TryAgain)
    }
}

/// The progress pill's phase steps, and the index of `phase` among them.
fn phase_steps() -> [&'static str; 4] {
    [mandara_i18n::tk("Inspecting"), mandara_i18n::tk("Installing"), mandara_i18n::tk("Adding to apps"), mandara_i18n::tk("Done")]
}

fn phase_index(phase: Phase) -> usize {
    match phase {
        Phase::Inspect | Phase::Confirm => 0,
        Phase::Install => 1,
        Phase::Integrate => 2,
        _ => 3,
    }
}

/// `m:ss` (tabular in the pill).
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// An action of the focused sheet: Enter runs the focused one.
type Action = Rc<dyn Fn(&mut App)>;

pub struct InstallerOverlay {
    focus: FocusHandle,
    clock: FrameClock,
    appear: SpringValue<f32>,
    /// 0 = card, 1 = pill.
    morph: SpringValue<f32>,
    bounce: SpringValue<f32>,
    ring: SpringValue<f32>,
    check: SpringValue<f32>,
    pill_h: SpringValue<f32>,
    card_h: Rc<Cell<f32>>,
    mask: InputMaskRegistry,
    /// Phases seen per job (to start the bounce once).
    seen: HashMap<u64, Phase>,
    spin_started: Instant,
    last_sheet: Sheet,
    /// Open disclosures, for the job they belong to.
    open: (u64, u8),
    /// The focused action and this frame's actions (primary last).
    focused: Option<usize>,
    actions: Vec<Action>,
    /// Image files drawn (dropped from GPUI's cache on close).
    drawn: Vec<PathBuf>,
    tick: Option<Task<()>>,
}

impl InstallerOverlay {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        let q = installer::queue(cx);
        cx.observe(&q, |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut appear = SpringValue::new(0.0, APPEAR);
        appear.set_target(1.0);
        Self {
            focus,
            clock: FrameClock::new(),
            appear,
            morph: SpringValue::new(1.0, MORPH),
            bounce: SpringValue::new(1.0, BOUNCE),
            ring: SpringValue::new(0.0, RING_SPRING),
            check: SpringValue::new(0.0, CHECK_SPRING),
            pill_h: SpringValue::new(PILL_RUN_H_PX, MORPH),
            card_h: Rc::new(Cell::new(CARD_GUESS_H_PX)),
            mask: InputMaskRegistry::new(),
            seen: HashMap::new(),
            spin_started: Instant::now(),
            last_sheet: Sheet::Nothing,
            open: (0, 0),
            focused: None,
            actions: Vec::new(),
            drawn: Vec::new(),
            tick: None,
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let n = self.actions.len();
        match k.key.as_str() {
            "tab" if n > 0 => {
                let at = self.focused.unwrap_or(n - 1);
                self.focused = Some(if k.modifiers.shift { (at + n - 1) % n } else { (at + 1) % n });
                cx.notify();
            }
            "enter" | "space" if n > 0 => {
                let action = self.actions[self.focused.unwrap_or(n - 1).min(n - 1)].clone();
                action(cx);
            }
            "escape" => self.escape(cx),
            _ => {}
        }
    }

    /// Escape: cancels a pending decision, hides a running install, closes
    /// anything finished.
    fn escape(&mut self, cx: &mut Context<Self>) {
        let jobs = installer::queue(cx).read(cx).jobs().to_vec();
        match installer::sheet(&jobs) {
            Sheet::Queue(batch) => installer::cancel_batch(batch, cx),
            Sheet::Card(id) => match jobs.iter().find(|j| j.id == id).map(|j| j.phase) {
                Some(Phase::Confirm) => installer::cancel(id, cx),
                _ => installer::dismiss(id, cx),
            },
            Sheet::Pill(id) => match jobs.iter().find(|j| j.id == id) {
                Some(j) if j.finished() => installer::dismiss(id, cx),
                Some(_) => installer::send_to_background(id, cx),
                None => {}
            },
            Sheet::Nothing => {}
        }
    }

    /// Registers an action for Enter / Tab; returns whether it is focused.
    fn action(&mut self, f: Action) -> bool {
        self.actions.push(f);
        self.focused == Some(self.actions.len() - 1)
    }

    fn remember_image(&mut self, path: &Path) {
        if !self.drawn.iter().any(|p| p == path) {
            self.drawn.push(path.to_path_buf());
        }
    }

    /// Drops every image this surface decoded (it is closing).
    fn forget_images(&mut self, cx: &mut App) {
        for p in self.drawn.drain(..) {
            crate::image_cache::forget_path(&p, cx);
        }
    }
}

impl Render for InstallerOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let jobs: Vec<Job> = installer::queue(cx).read(cx).jobs().to_vec();
        let sheet = installer::sheet(&jobs);
        // Start the finish bounce and check morph once per job.
        for j in &jobs {
            let before = self.seen.insert(j.id, j.phase);
            if j.phase == Phase::Done && before.is_some_and(|b| b != Phase::Done) {
                self.bounce.snap_to(BOUNCE_FROM);
                self.bounce.set_target(1.0);
                self.check.snap_to(0.0);
                self.check.set_target(1.0);
            }
        }
        self.seen.retain(|id, _| jobs.iter().any(|j| j.id == *id));
        let showing = sheet != Sheet::Nothing;
        if showing && self.last_sheet == Sheet::Nothing {
            self.focus.focus(window, cx);
        }
        if showing && sheet != self.last_sheet {
            // A new sheet starts on its primary action, disclosures closed.
            self.focused = None;
        }
        if showing {
            self.last_sheet = sheet;
        }
        // While closing, keep drawing what was shown last.
        let drawn = if showing { sheet } else { self.last_sheet };
        self.appear.set_config(if showing { APPEAR } else { SpringConfig::critical(DISAPPEAR_STIFFNESS) });
        self.appear.set_target(if showing { 1.0 } else { 0.0 });
        self.morph.set_target(if matches!(drawn, Sheet::Pill(_)) { 1.0 } else { 0.0 });
        let pill_job = match drawn {
            Sheet::Pill(id) => jobs.iter().find(|j| j.id == id).cloned(),
            _ => None,
        };
        if let Some(j) = &pill_job {
            let fraction = match j.phase {
                Phase::Done => 1.0,
                Phase::Failed | Phase::Cancelled => 0.0,
                _ => j.fraction.unwrap_or(0.0),
            };
            self.ring.set_target(fraction);
            self.pill_h.set_target(if j.phase == Phase::Done && !dry_run(j) { PILL_DONE_H_PX } else { PILL_RUN_H_PX });
        }
        let dt = self.clock.tick(Instant::now());
        let moving = [self.appear.step(dt), self.morph.step(dt), self.bounce.step(dt), self.ring.step(dt), self.check.step(dt), self.pill_h.step(dt)].contains(&true);
        let reduce = mandara_core::spring::motion_profile().reduce;
        let spinning = !reduce && jobs.iter().any(|j| !j.background && j.fraction.is_none() && matches!(j.phase, Phase::Inspect | Phase::Install | Phase::Integrate));
        if !showing && self.appear.value() <= GONE {
            self.forget_images(cx);
            self.tick = None;
            window.remove_window();
            cx.set_global(OverlayWindow(None));
            return div().into_any_element();
        }
        if moving || spinning {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        // The elapsed time ticks once a second while something installs.
        let running = jobs.iter().any(|j| !j.background && matches!(j.phase, Phase::Install | Phase::Integrate));
        if running && self.tick.is_none() {
            self.tick = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ELAPSED_TICK).await;
                let _ = this.update(cx, |this, cx| {
                    this.tick = None;
                    cx.notify();
                });
            }));
        }
        self.actions.clear();
        let shown = self.appear.value().clamp(0.0, 1.0);
        let m = self.morph.value().clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| a + (b - a) * m;
        let job = |id: u64| jobs.iter().find(|j| j.id == id).cloned();
        let card_h = self.card_h.get().min(CARD_MAX_H_PX);
        let pill_h = self.pill_h.value();
        let w = lerp(CARD_W_PX, PILL_W_PX);
        let h = lerp(card_h, pill_h);
        let radius = lerp(CARD_RADIUS_PX, PILL_RUN_H_PX / 2.0);
        let spin = self.spin_started.elapsed().as_secs_f32() * SPIN_HZ * TAU;
        let mut shadows = glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        shadows.push(shadow(Color::hex("#000000").with_alpha(SHADOW_ALPHA).hsla(), SHADOW_BLUR_PX, SHADOW_Y_PX));
        let sheet_body: Option<AnyElement> = if m < 1.0 - GONE {
            match drawn {
                Sheet::Card(id) => job(id).map(|j| if j.phase == Phase::Failed { self.failure(&j, &p, &f, cx) } else { self.card(&j, &p, &f, cx) }),
                Sheet::Queue(batch) => Some(self.queue_sheet(batch, &jobs, spin, &p, &f, cx)),
                _ => None,
            }
        } else {
            None
        };
        let card_layer = sheet_body.map(|body| {
            let measured = self.card_h.clone();
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(CARD_W_PX))
                .opacity(fade(1.0 - m))
                .child(body)
                .child(
                    canvas(
                        move |b, window, _| {
                            let h = f32::from(b.size.height);
                            if (measured.get() - h).abs() > MEASURE_EPSILON_PX {
                                measured.set(h);
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
        });
        let count = jobs.iter().filter(|j| !j.background).count();
        let pill_layer = pill_job.filter(|_| m > GONE).map(|j| div().absolute().top_0().left_0().w(px(PILL_W_PX)).h(px(pill_h)).opacity(fade(m)).child(self.pill(&j, count, spin, &p, cx)));
        let container = div()
            .id("installer-surface")
            .occlude()
            .relative()
            .w(px(w))
            .h(px(h))
            .mt(px((1.0 - shown) * -RISE_PX))
            .opacity(shown)
            .rounded(px(crate::style::radius(radius)))
            .overflow_hidden()
            .bg(p.surface_container.with_alpha(CARD_ALPHA).hsla())
            .border_1()
            .border_color(p.outline.with_alpha(OUTLINE_ALPHA).hsla())
            .shadow(shadows)
            .children(card_layer)
            .children(pill_layer)
            .child(self.mask.region());
        div()
            .id("installer-root")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .font_family(f.ui.clone())
            .text_color(p.on_surface.hsla())
            .flex()
            .flex_col()
            .items_center()
            .pt(px(SHADOW_ROOM_PX))
            .child(container)
            .child(self.mask.commit())
            .into_any_element()
    }
}

fn dry_run(j: &Job) -> bool {
    j.result.as_ref().is_some_and(|r| r.as_ref().is_ok_and(|o| o.dry_run))
}

/// Crossfade curve: content only shows in the second half of its share.
fn fade(v: f32) -> f32 {
    ((v - FADE_EDGE) / (1.0 - FADE_EDGE)).clamp(0.0, 1.0)
}

fn shadow(color: gpui::Hsla, blur: f32, y: f32) -> BoxShadow {
    BoxShadow { color, offset: point(px(0.0), px(y)), blur_radius: px(blur), spread_radius: px(0.0), inset: false }
}

/// The app's icon (an image file) or a package glyph.
fn app_image(path: Option<&Path>, size: f32, p: &MandaraPalette) -> AnyElement {
    match path {
        Some(path) => div().size(px(size)).flex_none().child(gpui::img(path.to_path_buf()).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
        None => div().size(px(size)).flex_none().flex().items_center().justify_center().child(icon("package", size * FALLBACK_GLYPH_SCALE, p.primary)).into_any_element(),
    }
}

fn chip(text: impl Into<SharedString>, tint: Color) -> gpui::Div {
    div()
        .h(px(CHIP_H_PX))
        .px(px(CHIP_PAD_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(TIGHT_GAP_PX))
        .bg(tint.with_alpha(TINT_ALPHA).hsla())
        .text_color(tint.hsla())
        .text_size(px(SMALL_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A small hover tooltip (the format chip's explanation).
struct Tip {
    text: SharedString,
    bg: Color,
    fg: Color,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(TIP_MAX_W_PX))
            .px(px(TIP_PAD_X_PX))
            .py(px(TIP_PAD_Y_PX))
            .rounded(px(crate::style::radius(TIP_RADIUS_PX)))
            .bg(self.bg.hsla())
            .text_color(self.fg.hsla())
            .text_size(px(SMALL_PX))
            .line_height(px(LEADING_PX))
            .shadow_md()
            .child(crate::bidi::paragraph(self.text.clone()))
    }
}

fn tip(text: impl Into<SharedString>, p: &MandaraPalette) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let (text, bg, fg) = (text.into(), p.on_surface, p.surface_base);
    move |_, cx| cx.new(|_| Tip { text: text.clone(), bg, fg }).into()
}

fn button(id: &'static str, label: impl Into<SharedString>, primary: bool, focused: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let el = div()
        .id(id)
        .h(px(BUTTON_H_PX))
        .px(px(BUTTON_PAD_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .text_size(px(BODY_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(focused, |el| el.border(px(FOCUS_RING_PX)).border_color(p.primary.hsla()))
        .child(label.into());
    if primary {
        el.text_color(p.on_primary.hsla()).bg(oklab_gradient(GRADIENT_DEG, p.accent_gradient_start, p.accent_gradient_end)).shadow(vec![shadow(p.primary_glow.with_alpha(GLOW_ALPHA).hsla(), GLOW_BLUR_PX, GLOW_Y_PX)])
    } else {
        let hover = p.on_surface.with_alpha(HOVER_ALPHA);
        el.bg(p.surface_container_high.hsla()).hover(move |s| s.bg(hover.hsla()))
    }
}

fn switch(on: bool, p: &MandaraPalette) -> AnyElement {
    div()
        .w(px(SWITCH_W_PX))
        .h(px(SWITCH_H_PX))
        .flex_none()
        .rounded_full()
        .p(px((SWITCH_H_PX - KNOB_PX) / 2.0))
        .flex()
        .when(on, |el| el.justify_end().bg(p.primary.hsla()))
        .when(!on, |el| el.bg(p.on_surface.with_alpha(TRACK_ALPHA).hsla()))
        .child(div().size(px(KNOB_PX)).rounded_full().bg(if on { p.on_primary.hsla() } else { p.surface_base.hsla() }))
        .into_any_element()
}

fn muted(text: impl Into<SharedString>, p: &MandaraPalette) -> gpui::Div {
    div().text_size(px(SMALL_PX)).line_height(px(LEADING_PX)).text_color(p.on_surface_variant.with_alpha(MUTED_ALPHA).hsla()).child(crate::bidi::paragraph(text))
}

fn section_title(text: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    div().text_size(px(SMALL_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(text.into()).into_any_element()
}

/// A group of info rows on a faint tray.
fn rows(children: Vec<AnyElement>, p: &MandaraPalette) -> AnyElement {
    div().p(px(ROWS_PAD_PX)).rounded(px(crate::style::radius(ROWS_RADIUS_PX))).bg(p.on_surface.with_alpha(ROWS_ALPHA).hsla()).flex().flex_col().children(children).into_any_element()
}

/// An info row: glyph tile, title, detail, and an optional trailing part.
fn info_row(glyph: &'static str, title: impl Into<SharedString>, detail: Option<String>, tint: Color, trailing: Option<AnyElement>, p: &MandaraPalette) -> gpui::Div {
    div()
        .px(px(ROW_PAD_X_PX))
        .py(px(ROW_PAD_Y_PX))
        .row()
        .items_center()
        .gap(px(PILL_GAP_PX))
        .child(div().size(px(ROW_ICON_TILE_PX)).flex_none().rounded(px(crate::style::radius(ROW_ICON_RADIUS_PX))).bg(tint.with_alpha(TINT_ALPHA).hsla()).flex().items_center().justify_center().child(icon(glyph, GLYPH_ROW_PX, tint)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::MEDIUM).child(crate::bidi::paragraph(title.into())))
                .when_some(detail.filter(|d| !d.is_empty()), |el, d| el.child(muted(d, p))),
        )
        .children(trailing)
}

/// A progress ring: determinate from 12 o'clock, else a spinning arc.
fn ring(fraction: Option<f32>, spin: f32, color: Color, track: Color, inner: AnyElement) -> AnyElement {
    div()
        .relative()
        .size(px(RING_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    crate::icons::stroke_arc(window, bounds, RING_STROKE_PX, 0.0, TAU, track);
                    match fraction {
                        Some(v) if v > 0.0 => crate::icons::stroke_arc(window, bounds, RING_STROKE_PX, -FRAC_PI_2, TAU * v.min(1.0), color),
                        Some(_) => {}
                        None => crate::icons::stroke_arc(window, bounds, RING_STROKE_PX, spin - FRAC_PI_2, TAU * SPIN_SWEEP, color),
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(inner)
        .into_any_element()
}

fn notice(glyph: &'static str, title: &str, text: &str, tint: Color, p: &MandaraPalette) -> AnyElement {
    div()
        .relative()
        .p(px(NOTICE_PAD_PX))
        .rounded(px(crate::style::radius(NOTICE_RADIUS_PX)))
        .bg(tint.with_alpha(TINT_ALPHA * 0.8).hsla())
        .overflow_hidden()
        .row()
        .gap(px(NOTICE_GAP_PX))
        .child(div().absolute().top_0().bottom_0().left_0().w(px(NOTICE_BAR_W_PX)).bg(tint.hsla()))
        .child(icon(glyph, GLYPH_ROW_PX, tint))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(NOTICE_LINE_GAP_PX))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(tint.hsla()).child(title.to_string()))
                .child(div().text_size(px(SMALL_PX)).line_height(px(LEADING_PX)).text_color(p.on_surface.hsla()).child(crate::bidi::paragraph(text.to_string()))),
        )
        .into_any_element()
}

/// The description, a line per paragraph or `• ` item (scrolls when long).
fn description(text: &str, p: &MandaraPalette) -> AnyElement {
    let lines = text.lines().map(|l| match l.strip_prefix("• ") {
        Some(item) => div()
            .row()
            .gap(px(ROW_GAP_PX))
            .child(div().mt(px((LEADING_PX - STEP_DOT_PX) / 2.0)).size(px(STEP_DOT_PX)).flex_none().rounded_full().bg(p.primary.hsla()))
            .child(div().flex_1().min_w_0().child(crate::bidi::paragraph(item.to_string())))
            .into_any_element(),
        None => div().child(crate::bidi::paragraph(l.to_string())).into_any_element(),
    });
    div()
        .id("installer-description")
        // A scroller would otherwise shrink to nothing in a full sheet.
        .flex_none()
        .max_h(px(DESCRIPTION_MAX_H_PX))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(TIGHT_GAP_PX))
        .text_size(px(SMALL_PX))
        .line_height(px(LEADING_PX))
        .text_color(p.on_surface.with_alpha(MUTED_ALPHA).hsla())
        .children(lines)
        .into_any_element()
}

/// "What happens": the plan's steps as a dotted list.
fn steps(plan: &Plan, p: &MandaraPalette) -> AnyElement {
    let rows = plan.steps.iter().map(|s| {
        div()
            .row()
            .gap(px(ROW_GAP_PX))
            .child(div().mt(px((LEADING_PX - STEP_DOT_PX) / 2.0)).size(px(STEP_DOT_PX)).flex_none().rounded_full().bg(p.primary.hsla()))
            .child(div().flex_1().min_w_0().text_size(px(SMALL_PX)).line_height(px(LEADING_PX)).child(crate::bidi::paragraph(s.clone())))
    });
    div().flex().flex_col().gap(px(TIGHT_GAP_PX)).children(rows).into_any_element()
}

fn pill_button(id: &'static str, label: impl Into<SharedString>, primary: bool, focused: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    button(id, label, primary, focused, p).h(px(PILL_BUTTON_H_PX)).px(px(PILL_BUTTON_PAD_PX)).text_size(px(SMALL_PX))
}

fn icon_button(id: &'static str, glyph: &'static str, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let hover = p.on_surface.with_alpha(HOVER_ALPHA);
    div().id(id).size(px(ICON_BUTTON_PX)).flex_none().rounded_full().flex().items_center().justify_center().cursor_pointer().hover(move |s| s.bg(hover.hsla())).child(icon(glyph, GLYPH_PX, p.on_surface_variant))
}

/// Opens a file's folder.
fn reveal(path: &Path) {
    let dir = if path.is_dir() { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
    crate::plugins::dock::stacks::open_path(&dir);
}

fn open_url(url: String) {
    std::thread::spawn(move || {
        if let Err(err) = mandara_services::session::open_uri(&url) {
            tracing::warn!("installer: opening {url}: {err}");
        }
    });
}

/// `query` made safe for a URL query (unreserved characters kept).
fn url_query(q: &str) -> String {
    q.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl InstallerOverlay {
    fn toggle(&mut self, id: u64, bit: u8) {
        if self.open.0 != id {
            self.open = (id, 0);
        }
        self.open.1 ^= bit;
    }

    fn is_open(&self, id: u64, bit: u8) -> bool {
        self.open.0 == id && self.open.1 & bit != 0
    }

    /// A disclosure header ("What happens", "Also installs …").
    fn disclosure(&self, key: &'static str, id: u64, bit: u8, label: String, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.is_open(id, bit);
        div()
            .id(key)
            .row()
            .items_center()
            .gap(px(TIGHT_GAP_PX))
            .cursor_pointer()
            .text_size(px(SMALL_PX))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.on_surface_variant.hsla())
            .child(icon(if open { "minus" } else { "plus" }, GLYPH_SMALL_PX, p.on_surface_variant))
            .child(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.toggle(id, bit);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The icon tile with a soft glow in the theme's accent.
    fn hero_icon(&mut self, path: Option<&Path>, badge: Option<(&'static str, Color)>, p: &MandaraPalette) -> AnyElement {
        if let Some(path) = path {
            self.remember_image(path);
        }
        div()
            .relative()
            .size(px(APP_TILE_PX))
            .flex_none()
            .rounded(px(crate::style::radius(APP_TILE_RADIUS_PX)))
            .bg(p.surface_container_high.hsla())
            .shadow({
                let mut s = glass_rim(p.specular_rim_light, p.specular_rim_shadow);
                s.push(shadow(p.primary_glow.with_alpha(HERO_GLOW_ALPHA).hsla(), HERO_GLOW_BLUR_PX, 0.0));
                s
            })
            .flex()
            .items_center()
            .justify_center()
            .child(app_image(path, APP_ICON_PX, p))
            .when_some(badge, |el, (glyph, tint)| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(-TIGHT_GAP_PX))
                        .right(px(-TIGHT_GAP_PX))
                        .size(px(CHECK_BADGE_PX))
                        .rounded_full()
                        .bg(tint.hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph, CHECK_GLYPH_PX, p.on_primary)),
                )
            })
            .into_any_element()
    }

    fn card(&mut self, j: &Job, p: &MandaraPalette, f: &crate::fonts::Fonts, cx: &mut Context<Self>) -> AnyElement {
        let id = j.id;
        let Some(plan) = j.plan.clone() else { return div().into_any_element() };
        let i = &plan.inspection;
        let family = i.family();
        // ── Hero ──
        let mut byline: Vec<String> = Vec::new();
        if !i.version.is_empty() {
            byline.push(tf("Version {version}", &[("version", &i.version)]));
        }
        if !i.developer.is_empty() {
            byline.push(tf("by {developer}", &[("developer", &i.developer)]));
        }
        let size = match (family, i.unpacked) {
            (Family::AppImage, _) | (_, None) => human_bytes(i.size),
            (_, Some(u)) => tf("{download} file · {installed} installed", &[("download", &human_bytes(i.size)), ("installed", &human_bytes(u))]),
        };
        let format_tint = if plan.needs_password { p.warning_amber } else { p.primary };
        let chips = div()
            .row()
            .flex_wrap()
            .gap(px(CHIP_GAP_PX))
            .child(div().id("installer-format").child(chip(plan.badge(), format_tint)).tooltip(tip(t(format_explanation(family)), p)))
            .child(chip(size, p.tertiary))
            .children(i.categories.iter().take(MAX_CATEGORY_CHIPS).map(|c| chip(c.clone(), p.secondary)));
        let hero = div()
            .row()
            .items_center()
            .gap(px(HERO_GAP_PX))
            .child(self.hero_icon(j.icon.as_deref(), None, p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(TIGHT_GAP_PX))
                    .child(div().text_size(px(TITLE_PX)).font_family(f.display.clone()).font_weight(FontWeight::SEMIBOLD).truncate().child(i.name.clone()))
                    .when(!byline.is_empty(), |el| el.child(muted(byline.join(" · "), p)))
                    .when(!i.summary.is_empty(), |el| el.child(div().text_size(px(BODY_PX)).line_height(px(LEADING_PX)).child(crate::bidi::paragraph(i.summary.clone()))))
                    .child(chips),
            );
        let mut body: Vec<AnyElement> = vec![hero.into_any_element()];
        for b in &plan.blockers {
            body.push(notice("close", t("Can't install this file"), b, p.urgent_coral, p));
        }
        for w in &plan.warnings {
            body.push(notice("alert", t("Heads up"), w, p.warning_amber, p));
        }
        // ── Screenshots ──
        if !i.screenshots.is_empty() {
            let shots: Vec<AnyElement> = i
                .screenshots
                .iter()
                .map(|s| {
                    self.remember_image(s);
                    div().flex_1().min_w_0().h(px(SHOT_H_PX)).rounded(px(crate::style::radius(SHOT_RADIUS_PX))).overflow_hidden().bg(p.surface_container_high.hsla()).child(gpui::img(s.clone()).size_full().object_fit(ObjectFit::Cover)).into_any_element()
                })
                .collect();
            body.push(div().flex_none().row().gap(px(ROW_GAP_PX)).children(shots).into_any_element());
        }
        if !i.description.is_empty() {
            body.push(description(&i.description, p));
        }
        if plan.can_install() {
            // ── Duplicate ──
            if let Some(d) = &plan.duplicate {
                body.push(self.duplicate_choice(id, d.same_version, &d.version, &i.version, j.choices.on_duplicate, p, cx));
            }
            // ── Where, command, password ──
            let mut info: Vec<AnyElement> = Vec::new();
            let user_flatpak = family == Family::Flatpak && plan.rootless;
            let (glyph, place) = match (&plan.target, family) {
                (Some(target), _) => ("folder", tf("Installs to {dir}", &[("dir", &tilde(target))])),
                (None, Family::Flatpak) if user_flatpak => ("user", t("Installs with Flatpak, for you only").to_string()),
                (None, Family::Flatpak) => ("user", t("Installs with Flatpak, for everyone").to_string()),
                (None, _) => ("package", tf("Installs system-wide with {manager}", &[("manager", &plan.manager.map(|m| m.label()).unwrap_or_default())])),
            };
            info.push(info_row(glyph, place, None, p.primary, None, p).into_any_element());
            if let Some(shim) = &plan.shim {
                info.push(self.shim_row(id, &shim.name, shim.conflict.as_deref(), j.choices.path_shim, p, cx));
            }
            if plan.needs_password {
                info.push(info_row("lock", t("Needs your password"), Some(t("You'll be asked once, by the system, before anything changes").to_string()), p.warning_amber, None, p).into_any_element());
            }
            body.push(rows(info, p));
            // ── Flatpak permissions ──
            if family == Family::Flatpak {
                body.push(section_title(t("Permissions"), p));
                let perms = i.flatpak.as_ref().and_then(|x| x.permissions.as_ref());
                let list: Vec<AnyElement> = match perms {
                    Some(perms) if !perms.is_empty() => permission_rows(perms).into_iter().map(|r| info_row(r.glyph, r.title, Some(r.detail), if r.broad { p.warning_amber } else { p.secondary }, None, p).into_any_element()).collect(),
                    Some(_) => vec![info_row("shield", t("Fully sandboxed"), Some(t("No access beyond its own window").to_string()), p.success_emerald, None, p).into_any_element()],
                    None => vec![info_row("shield", t("Permissions come with the download"), Some(t("Flatpak applies the app's sandbox from its remote").to_string()), p.secondary, None, p).into_any_element()],
                };
                body.push(rows(list, p));
            }
            // ── Native dependencies ──
            if let Some(native) = &plan.native {
                let pkg = &native.package;
                // What the package manager adds, else what the package declares.
                let extra: Vec<String> = native.simulation.installs.iter().filter(|n| *n != pkg).cloned().collect();
                let (deps, adds) = if extra.is_empty() { (i.package.as_ref().map(|m| m.depends.clone()).unwrap_or_default(), false) } else { (extra, true) };
                if !deps.is_empty() {
                    let label = if adds { tf("Also installs {count} packages", &[("count", &deps.len())]) } else { tf("Needs {count} packages", &[("count", &deps.len())]) };
                    body.push(self.disclosure("installer-deps", id, OPEN_DEPENDENCIES, label, p, cx));
                    if self.is_open(id, OPEN_DEPENDENCIES) {
                        body.push(div().row().flex_wrap().gap(px(CHIP_GAP_PX)).children(deps.iter().map(|d| chip(d.clone(), p.on_surface_variant))).into_any_element());
                    }
                }
            }
            if !plan.steps.is_empty() {
                body.push(self.disclosure("installer-steps", id, OPEN_STEPS, if plan.needs_password { t("What happens (needs your password)").to_string() } else { t("What happens").to_string() }, p, cx));
                if self.is_open(id, OPEN_STEPS) {
                    body.push(steps(&plan, p));
                }
            }
        }
        // ── Actions ──
        let can = plan.can_install();
        let primary_label = match (&plan.duplicate, j.choices.on_duplicate) {
            (Some(d), OnDuplicate::Update | OnDuplicate::Reinstall) if d.same_version => t("Reinstall"),
            (Some(_), OnDuplicate::Update | OnDuplicate::Reinstall) => t("Update"),
            _ => t("Install"),
        };
        let mut actions: Vec<AnyElement> = Vec::new();
        if can {
            let focused = self.action(Rc::new(move |cx| installer::cancel(id, cx)));
            actions.push(button("installer-cancel", t("Cancel"), false, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::cancel(id, cx))).into_any_element());
            let focused = self.action(Rc::new(move |cx| installer::confirm(id, cx)));
            actions.push(
                button("installer-install", primary_label, true, focused, p)
                    .when(plan.needs_password, |el| el.child(icon("lock", GLYPH_SMALL_PX, p.on_primary)))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::confirm(id, cx)))
                    .into_any_element(),
            );
        } else {
            // A package for another kind of Linux: the app may be on Flathub.
            let native = family == Family::Native;
            let focused = self.action(Rc::new(move |cx| installer::cancel(id, cx)));
            actions.push(button("installer-close", t("Close"), !native, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::cancel(id, cx))).into_any_element());
            if native {
                let name = i.name.clone();
                let find: Action = Rc::new(move |cx| {
                    open_url(format!("{FLATHUB_SEARCH}{}", url_query(&name)));
                    installer::cancel(id, cx);
                });
                let focused = self.action(find.clone());
                actions.push(button("installer-flathub", t("Find it on Flathub"), true, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| find(cx))).into_any_element());
            }
        }
        let actions = div().row().justify_end().gap(px(ROW_GAP_PX)).children(actions);
        sheet_frame("installer-card", body, actions.into_any_element())
    }

    fn shim_row(&self, id: u64, name: &str, conflict: Option<&str>, on: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let free = conflict.is_none();
        div()
            .id("installer-shim")
            .when(free, |el| el.cursor_pointer().on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::set_choices(id, |c| c.path_shim = !c.path_shim, cx))))
            .child(info_row(
                "terminal",
                tf("Add to PATH as {name}", &[("name", &name)]),
                Some(conflict.map_or_else(|| t("Run it from a terminal by that name").to_string(), str::to_string)),
                p.secondary,
                free.then(|| switch(on, p)),
                p,
            ))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn duplicate_choice(&self, id: u64, same: bool, installed: &str, new: &str, current: OnDuplicate, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let title = if same || installed.is_empty() { t("This app is already installed").to_string() } else { tf("Version {version} is already installed", &[("version", &installed)]) };
        let options = [(OnDuplicate::Update, if same { t("Reinstall") } else { t("Update") }, "installer-dup-update"), (OnDuplicate::KeepBoth, t("Keep both"), "installer-dup-both")];
        let segments = options.into_iter().map(|(choice, label, key)| {
            let active = current == choice || (choice == OnDuplicate::Update && current == OnDuplicate::Reinstall);
            div()
                .id(key)
                .flex_1()
                .h(px(SEGMENT_H_PX))
                .px(px(BUTTON_PAD_PX))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(SMALL_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()))
                .when(!active, |el| el.text_color(p.on_surface_variant.hsla()))
                .child(label)
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::set_choices(id, |c| c.on_duplicate = choice, cx)))
        });
        let versions = (!installed.is_empty() || !new.is_empty()).then(|| {
            div()
                .row()
                .items_center()
                .gap(px(CHIP_GAP_PX))
                .child(chip(tf("Installed {version}", &[("version", &if installed.is_empty() { "?" } else { installed })]), p.on_surface_variant))
                .child(icon(crate::bidi::dir_icon("chevron-right"), GLYPH_SMALL_PX, p.on_surface_variant))
                .child(chip(tf("This file {version}", &[("version", &if new.is_empty() { "?" } else { new })]), p.primary))
        });
        rows(
            vec![
                div()
                    .px(px(ROW_PAD_X_PX))
                    .py(px(ROW_PAD_Y_PX))
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP_PX))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .children(versions)
                    .child(div().flex().p(px(SEGMENT_INSET_PX)).gap(px(SEGMENT_INSET_PX)).rounded_full().bg(p.surface_container_high.hsla()).children(segments))
                    .into_any_element(),
            ],
            p,
        )
    }

    /// The failure card: plain words, a fix, the detail behind a disclosure.
    fn failure(&mut self, j: &Job, p: &MandaraPalette, f: &crate::fonts::Fonts, cx: &mut Context<Self>) -> AnyElement {
        let id = j.id;
        let error = j.result.as_ref().and_then(|r| r.as_ref().err()).cloned().unwrap_or_default();
        let family = j.plan.as_ref().map(|p| p.inspection.family());
        let place = j.plan.as_ref().and_then(|p| p.target.clone());
        let (plain, fix) = explain_failure(&error, family, place.as_deref());
        let hero = div()
            .row()
            .items_center()
            .gap(px(HERO_GAP_PX))
            .child(self.hero_icon(j.icon.as_deref(), Some(("close", p.urgent_coral)), p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(TIGHT_GAP_PX))
                    .child(div().text_size(px(TITLE_PX)).line_height(px(TITLE_LEADING_PX)).font_family(f.display.clone()).font_weight(FontWeight::SEMIBOLD).child(crate::bidi::paragraph(tf("Couldn't install {app}", &[("app", &j.name)]))))
                    .child(div().text_size(px(BODY_PX)).line_height(px(LEADING_PX)).child(crate::bidi::paragraph(plain))),
            );
        let mut body = vec![hero.into_any_element()];
        if !error.is_empty() {
            body.push(self.disclosure("installer-detail", id, OPEN_DETAIL, t("Technical details").to_string(), p, cx));
            if self.is_open(id, OPEN_DETAIL) {
                body.push(
                    div()
                        .p(px(NOTICE_PAD_PX))
                        .rounded(px(crate::style::radius(NOTICE_RADIUS_PX)))
                        .bg(p.on_surface.with_alpha(ROWS_ALPHA).hsla())
                        .font_family(f.mono.clone())
                        .text_size(px(MONO_PX))
                        .line_height(px(LEADING_PX))
                        .flex()
                        .flex_col()
                        .children(error.lines().filter(|l| !l.trim().is_empty()).map(|l| crate::bidi::paragraph(l.to_string())))
                        .into_any_element(),
                );
            }
        }
        let path = j.path.clone();
        let name = j.name.clone();
        let mut actions: Vec<AnyElement> = Vec::new();
        let focused = self.action(Rc::new(move |cx| installer::dismiss(id, cx)));
        actions.push(button("installer-fail-close", t("Close"), false, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::dismiss(id, cx))).into_any_element());
        let show = path.clone();
        let focused = self.action(Rc::new(move |_| reveal(&show)));
        let show = path.clone();
        actions.push(button("installer-fail-folder", t("Show file"), false, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, _| reveal(&show))).into_any_element());
        let primary: Option<(&'static str, &str, Action)> = match fix {
            Fix::TryAgain => Some(("installer-fail-retry", t("Try again"), Rc::new(move |cx| installer::retry(id, cx)))),
            Fix::Settings => Some(("installer-fail-settings", t("Open settings"), Rc::new(move |cx| {
                crate::state::open_studio_at(Some(STUDIO_INSTALLER.into()), cx);
                installer::dismiss(id, cx);
            }))),
            Fix::Flathub => Some(("installer-fail-flathub", t("Find it on Flathub"), Rc::new(move |cx| {
                open_url(format!("{FLATHUB_SEARCH}{}", url_query(&name)));
                installer::dismiss(id, cx);
            }))),
            Fix::OpenFolder => place.map(|dir| -> (&'static str, &str, Action) {
                // The app's own folder is rolled back: open the one it sits in.
                let at = std::iter::successors(Some(dir.as_path()), |d| d.parent()).find(|d| d.is_dir()).map(Path::to_path_buf).unwrap_or(dir);
                ("installer-fail-open-folder", t("Open folder"), Rc::new(move |_| reveal(&at)))
            }),
            Fix::None => None,
        };
        if let Some((key, label, run)) = primary {
            let focused = self.action(run.clone());
            actions.push(button(key, label, true, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| run(cx))).into_any_element());
        }
        let actions = div().row().justify_end().flex_wrap().gap(px(ROW_GAP_PX)).children(actions);
        sheet_frame("installer-failure", body, actions.into_any_element())
    }

    /// Several files dropped together: one row each, then Install all.
    fn queue_sheet(&mut self, batch: u64, jobs: &[Job], spin: f32, p: &MandaraPalette, f: &crate::fonts::Fonts, cx: &mut Context<Self>) -> AnyElement {
        let batch_jobs: Vec<&Job> = jobs.iter().filter(|j| j.batch == batch).collect();
        let ready = batch_jobs.iter().filter(|j| j.phase == Phase::Confirm && j.plan.as_ref().is_some_and(|p| p.can_install())).count();
        let inspecting = batch_jobs.iter().filter(|j| j.phase == Phase::Inspect).count();
        let title = tf("Install {count} apps", &[("count", &batch_jobs.len())]);
        let subtitle = if inspecting > 0 { tf("Looking inside {count} files…", &[("count", &inspecting)]) } else { tf("{count} ready to install", &[("count", &ready)]) };
        let mut list: Vec<AnyElement> = Vec::new();
        for (n, j) in batch_jobs.iter().enumerate() {
            let id = j.id;
            if let Some(icon_path) = &j.icon {
                self.remember_image(icon_path);
            }
            let plan = j.plan.as_ref();
            let (status, tint): (String, Color) = match j.phase {
                Phase::Inspect => (t("Inspecting…").to_string(), p.on_surface_variant),
                Phase::Confirm if plan.is_some_and(|p| !p.can_install()) => (t("Can't install").to_string(), p.urgent_coral),
                Phase::Confirm if plan.is_some_and(|p| !p.warnings.is_empty()) => (t("Ready, with a warning").to_string(), p.warning_amber),
                Phase::Confirm => (t("Ready").to_string(), p.success_emerald),
                Phase::Install | Phase::Integrate => (match j.fraction {
                    Some(v) => tf("Installing {percent}%", &[("percent", &((v * 100.0).round() as u32))]),
                    None => t("Installing…").to_string(),
                }, p.primary),
                Phase::Done => (t("Installed").to_string(), p.success_emerald),
                Phase::Failed => (j.result.as_ref().and_then(|r| r.as_ref().err()).cloned().unwrap_or_else(|| t("Failed").to_string()), p.urgent_coral),
                Phase::Cancelled => (t("Cancelled").to_string(), p.on_surface_variant),
            };
            let sub = plan.map(|pl| {
                let v = &pl.inspection.version;
                if v.is_empty() { pl.badge() } else { format!("{} · {v}", pl.badge()) }
            });
            let lead: AnyElement = if j.phase == Phase::Inspect {
                ring(None, spin, p.primary, p.on_surface.with_alpha(TRACK_ALPHA), div().into_any_element())
            } else {
                div().size(px(QUEUE_TILE_PX)).flex_none().rounded(px(crate::style::radius(QUEUE_TILE_RADIUS_PX))).bg(p.surface_container_high.hsla()).flex().items_center().justify_center().child(app_image(j.icon.as_deref(), QUEUE_ICON_PX, p)).into_any_element()
            };
            list.push(
                div()
                    .id(("installer-queue-row", n))
                    .min_h(px(QUEUE_ROW_H_PX))
                    .px(px(ROW_PAD_X_PX))
                    .row()
                    .items_center()
                    .gap(px(PILL_GAP_PX))
                    .child(div().size(px(QUEUE_TILE_PX)).flex_none().flex().items_center().justify_center().child(lead))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(j.name.clone()))
                            .when_some(sub, |el, s| el.child(div().text_size(px(SMALL_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(s))),
                    )
                    .child(div().flex_none().text_size(px(SMALL_PX)).font_weight(FontWeight::SEMIBOLD).font_features(crate::fonts::tabular_nums()).text_color(tint.hsla()).child(status))
                    .when(j.phase == Phase::Confirm, |el| el.child(pill_button(if n == 0 { "installer-queue-review-0" } else { "installer-queue-review" }, t("Review"), false, false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::review_one(id, cx)))))
                    .into_any_element(),
            );
        }
        let header = div()
            .row()
            .items_center()
            .gap(px(HERO_GAP_PX))
            .child(div().size(px(APP_TILE_PX)).flex_none().rounded(px(crate::style::radius(APP_TILE_RADIUS_PX))).bg(p.primary.with_alpha(TINT_ALPHA).hsla()).flex().items_center().justify_center().child(icon("download", APP_ICON_PX * FALLBACK_GLYPH_SCALE, p.primary)))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(TIGHT_GAP_PX)).child(div().text_size(px(TITLE_PX)).font_family(f.display.clone()).font_weight(FontWeight::SEMIBOLD).child(title)).child(muted(subtitle, p)));
        let body = vec![header.into_any_element(), rows(list, p)];
        let mut actions: Vec<AnyElement> = Vec::new();
        let focused = self.action(Rc::new(move |cx| installer::cancel_batch(batch, cx)));
        actions.push(button("installer-queue-cancel", t("Cancel"), false, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::cancel_batch(batch, cx))).into_any_element());
        let focused = self.action(Rc::new(move |cx| installer::review_each(batch, cx)));
        actions.push(button("installer-queue-each", t("Review each"), false, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::review_each(batch, cx))).into_any_element());
        let any = ready > 0 || inspecting > 0;
        let focused = self.action(Rc::new(move |cx| installer::install_all(batch, cx)));
        actions.push(
            button("installer-queue-all", t("Install all"), true, focused, p)
                .when(!any, |el| el.opacity(DISABLED_ALPHA))
                .when(any, |el| el.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::install_all(batch, cx))))
                .into_any_element(),
        );
        sheet_frame("installer-queue", body, div().row().justify_end().gap(px(ROW_GAP_PX)).children(actions).into_any_element())
    }

    fn pill(&mut self, j: &Job, count: usize, spin: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = j.id;
        let dry = dry_run(j);
        let done = j.phase == Phase::Done && !dry;
        let tint = match j.phase {
            Phase::Done if dry => p.warning_amber,
            Phase::Done => p.success_emerald,
            Phase::Failed => p.urgent_coral,
            Phase::Cancelled => p.on_surface_variant,
            _ => p.primary,
        };
        let scale = if j.phase == Phase::Done { self.bounce.value() } else { 1.0 };
        if let Some(icon_path) = &j.icon {
            self.remember_image(icon_path);
        }
        // The check grows in over the icon's corner as the ring closes.
        let check = self.check.value().clamp(0.0, BOUNCE_FROM);
        let inner = div()
            .relative()
            .size(px(PILL_ICON_PX * scale))
            .flex()
            .items_center()
            .justify_center()
            .child(app_image(j.icon.as_deref(), PILL_ICON_PX * scale, p))
            .when(j.phase == Phase::Done && check > GONE, |el| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(-TIGHT_GAP_PX * 2.0))
                        .right(px(-TIGHT_GAP_PX * 2.0))
                        .size(px(CHECK_BADGE_PX * check))
                        .rounded_full()
                        .bg(tint.hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("check", CHECK_GLYPH_PX * check, p.on_primary)),
                )
            })
            .into_any_element();
        let fraction = if j.fraction.is_none() && matches!(j.phase, Phase::Inspect | Phase::Install | Phase::Integrate) { None } else { Some(self.ring.value()) };
        let ring_el = ring(fraction, spin, tint, p.on_surface.with_alpha(TRACK_ALPHA), inner);
        let elapsed = clock(j.elapsed());
        let headline = match j.phase {
            Phase::Done if dry => t("Dry run: nothing was installed").to_string(),
            Phase::Done => tf("{app} is ready", &[("app", &j.name)]),
            Phase::Failed => tf("Couldn't install {app}", &[("app", &j.name)]),
            Phase::Cancelled => tf("{app} wasn't installed", &[("app", &j.name)]),
            _ => j.name.clone(),
        };
        // Phase steps: done ones filled, the current one in the accent.
        let at = phase_index(j.phase);
        let step_row = div().row().items_center().gap(px(PILL_GAP_PX)).children(phase_steps().into_iter().enumerate().map(|(n, label)| {
            let (dot, text) = if n < at || j.phase == Phase::Done {
                (p.primary, p.on_surface_variant)
            } else if n == at {
                (tint, p.on_surface)
            } else {
                (p.on_surface.with_alpha(TRACK_ALPHA), p.on_surface_variant.with_alpha(MUTED_ALPHA))
            };
            div()
                .row()
                .items_center()
                .gap(px(TIGHT_GAP_PX))
                .child(div().size(px(PHASE_DOT_PX)).rounded_full().bg(dot.hsla()))
                .child(div().text_size(px(SMALL_PX)).text_color(text.hsla()).when(n == at, |el| el.font_weight(FontWeight::SEMIBOLD)).child(t(label)))
        }));
        let more = count.saturating_sub(1);
        let mut trailing: Vec<AnyElement> = Vec::new();
        let finished = j.finished();
        if !finished {
            trailing.push(icon_button("installer-hide", "minus", p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::send_to_background(id, cx))).into_any_element());
            trailing.push(icon_button("installer-stop", "close", p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::cancel(id, cx))).into_any_element());
            self.action(Rc::new(move |cx| installer::send_to_background(id, cx)));
            self.action(Rc::new(move |cx| installer::cancel(id, cx)));
        } else {
            trailing.push(icon_button("installer-close-pill", "close", p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::dismiss(id, cx))).into_any_element());
        }
        let top = div()
            .row()
            .items_center()
            .gap(px(PILL_GAP_PX))
            .child(ring_el)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(TIGHT_GAP_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(ROW_GAP_PX))
                            .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).truncate().child(headline))
                            .when(more > 0, |el| el.child(chip(format!("+{more}"), p.secondary)))
                            .child(div().flex_none().text_size(px(SMALL_PX)).text_color(p.on_surface_variant.hsla()).font_features(crate::fonts::tabular_nums()).child(elapsed)),
                    )
                    .when(!finished, |el| el.child(step_row))
                    .when(finished && !done, |el| {
                        let detail = match j.phase {
                            Phase::Failed => j.result.as_ref().and_then(|r| r.as_ref().err()).cloned().unwrap_or_default(),
                            _ => String::new(),
                        };
                        el.when(!detail.is_empty(), |el| el.child(div().text_size(px(SMALL_PX)).text_color(tint.hsla()).truncate().child(detail)))
                    })
                    .when(done, |el| el.child(div().text_size(px(SMALL_PX)).text_color(p.on_surface_variant.hsla()).child(t("Find it in your apps, or open it now"))))
            )
            .children(trailing);
        let finish_row = done.then(|| {
            let desktop = j.desktop_id.clone();
            let keep = j.keep_in_dock;
            let mut row: Vec<AnyElement> = Vec::new();
            let focused = self.action(Rc::new(move |cx| installer::set_keep_in_dock(id, !keep, cx)));
            row.push(
                div()
                    .id("installer-keep-dock")
                    .h(px(PILL_BUTTON_H_PX))
                    .px(px(PILL_BUTTON_PAD_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(ROW_GAP_PX))
                    .cursor_pointer()
                    .text_size(px(SMALL_PX))
                    .when(focused, |el| el.border(px(FOCUS_RING_PX)).border_color(p.primary.hsla()))
                    .child(switch(keep, p))
                    .child(t("Keep in Dock"))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| installer::set_keep_in_dock(id, !keep, cx)))
                    .into_any_element(),
            );
            row.push(div().flex_1().into_any_element());
            let focused = self.action(Rc::new(move |cx| {
                crate::state::open_studio_at(Some(STUDIO_APPS.into()), cx);
                installer::dismiss(id, cx);
            }));
            row.push(
                pill_button("installer-show-apps", t("Show in Applications"), false, focused, p)
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        crate::state::open_studio_at(Some(STUDIO_APPS.into()), cx);
                        installer::dismiss(id, cx);
                    }))
                    .into_any_element(),
            );
            if let Some(d) = desktop {
                let open: Action = Rc::new(move |cx| {
                    if let Err(e) = installer::open_app(&d, cx) {
                        tracing::warn!("open installed app: {e}");
                    }
                    installer::dismiss(id, cx);
                });
                let focused = self.action(open.clone());
                row.push(pill_button("installer-open", t("Open"), true, focused, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| open(cx))).into_any_element());
            }
            div().row().items_center().gap(px(ROW_GAP_PX)).children(row)
        });
        if finished && !done {
            self.action(Rc::new(move |cx| installer::dismiss(id, cx)));
        }
        div().size_full().p(px(PILL_PAD_PX)).flex().flex_col().justify_center().gap(px(PILL_GAP_PX)).child(top).children(finish_row).into_any_element()
    }
}

/// A sheet's scrolling body above its action row.
fn sheet_frame(id: &'static str, body: Vec<AnyElement>, actions: AnyElement) -> AnyElement {
    div()
        .id(id)
        .w(px(CARD_W_PX))
        .max_h(px(CARD_MAX_H_PX))
        .flex()
        .flex_col()
        .text_size(px(BODY_PX))
        .child(div().id((id, 1usize)).flex_1().min_h(px(0.0)).overflow_y_scroll().p(px(CARD_PAD_PX)).pb(px(CARD_GAP_PX)).flex().flex_col().gap(px(CARD_GAP_PX)).children(body))
        .child(div().px(px(CARD_PAD_PX)).pb(px(CARD_PAD_PX)).child(actions))
        .into_any_element()
}

// ── Drop target ──────────────────────────────────────────────────────────

thread_local! {
    /// The last drag checked: paths → installable (drag_over runs per frame).
    static LAST_DRAG: RefCell<Option<(Vec<PathBuf>, bool)>> = const { RefCell::new(None) };
}

/// Whether a drag holds at least one installable file (cached per drag).
pub fn drag_installable(paths: &ExternalPaths) -> bool {
    let list = paths.paths();
    if let Some(hit) = LAST_DRAG.with(|l| l.borrow().as_ref().filter(|(p, _)| p.as_slice() == list).map(|(_, ok)| *ok)) {
        return hit;
    }
    let ok = list.iter().any(|p| installer::accepts(p));
    LAST_DRAG.with(|l| *l.borrow_mut() = Some((list.to_vec(), ok)));
    ok
}

/// Wraps `child` in a drop target: while an installable file is dragged
/// over it, it glows with a "Drop to install" label; dropping calls
/// `on_drop` with the installable paths. Works in any GPUI window (layer
/// surfaces and Studio): GPUI accepts `text/uri-list` drags on Wayland.
pub fn drop_zone(id: impl Into<gpui::ElementId>, child: AnyElement, radius: f32, p: &MandaraPalette, on_drop: Rc<dyn Fn(Vec<PathBuf>, &mut Window, &mut App)>) -> gpui::Stateful<gpui::Div> {
    let glow = p.primary;
    let fill = p.primary.with_alpha(DROP_FILL_ALPHA);
    let label_bg = p.primary;
    let label_fg = p.on_primary;
    let r = crate::style::radius(radius);
    let overlay = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(px(r))
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.0)
        .drag_over::<ExternalPaths>(move |s, paths, _, _| {
            if drag_installable(paths) {
                s.opacity(1.0).bg(fill.hsla()).border_2().border_color(glow.hsla()).shadow(vec![BoxShadow { color: glow.with_alpha(DROP_GLOW_ALPHA).hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(DROP_GLOW_BLUR_PX), spread_radius: px(0.0), inset: false }])
            } else {
                s
            }
        })
        .child(
            div()
                .h(px(DROP_LABEL_H_PX))
                .px(px(DROP_LABEL_PAD_PX))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(CHIP_GAP_PX))
                .bg(label_bg.hsla())
                .text_color(label_fg.hsla())
                .text_size(px(DROP_LABEL_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon("download", GLYPH_PX, label_fg))
                .child(t("Drop to install")),
        );
    div()
        .id(id)
        .relative()
        .child(child)
        .child(overlay)
        .on_drop::<ExternalPaths>(move |paths, window, cx| {
            let list: Vec<PathBuf> = paths.paths().iter().filter(|p| installer::accepts(p)).cloned().collect();
            LAST_DRAG.with(|l| *l.borrow_mut() = None);
            if !list.is_empty() {
                on_drop(list, window, cx);
            }
        })
}

/// A drop handler that installs into this shell's queue.
pub fn install_on_drop(origin: InstallOrigin) -> Rc<dyn Fn(Vec<PathBuf>, &mut Window, &mut App)> {
    Rc::new(move |paths, _, cx| installer::install(paths, origin, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_read_as_people_words() {
        let p = Permissions { shared: vec!["network".into(), "ipc".into()], sockets: vec!["wayland".into(), "pulseaudio".into(), "session-bus".into()], devices: vec!["dri".into()], filesystems: vec!["xdg-download".into(), "~/Music:ro".into()] };
        let rows = permission_rows(&p);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, vec!["Some folders", "Graphics acceleration", "System services", "Network access", "Sound"]);
        assert_eq!(rows[0].detail, "download, ~/Music");
        assert!(rows[2].broad && !rows[3].broad);
        let home = permission_rows(&Permissions { filesystems: vec!["home".into()], devices: vec!["all".into()], ..Default::default() });
        assert!(home.iter().all(|r| r.broad), "{home:?}");
        assert!(permission_rows(&Permissions::default()).is_empty());
    }

    #[test]
    fn failures_are_explained_with_a_fix() {
        let fix = |e: &str, f| explain_failure(e, f, None).1;
        assert_eq!(fix("Error executing command as another user: Not authorized", Some(Family::Native)), Fix::TryAgain);
        assert_eq!(fix("Installing native packages is turned off in the App Installer settings", Some(Family::Native)), Fix::Settings);
        assert_eq!(fix("nothing provides libfoo needed by demo", Some(Family::Native)), Fix::Flathub);
        assert_eq!(fix("weird", None), Fix::TryAgain);
        assert_eq!(fix("Couldn't read the app inside this AppImage: bad superblock", Some(Family::AppImage)), Fix::None);
        // Space and size limits name the place and open it.
        let dir = Path::new("/data/apps/seaglass");
        for e in ["No space left on device (os error 28)", "Disk quota exceeded (os error 122)"] {
            let (plain, fix) = explain_failure(e, Some(Family::AppImage), Some(dir));
            assert_eq!((plain.as_str(), fix), ("Not enough space in /data/apps/seaglass.", Fix::OpenFolder), "{e}");
        }
        assert_eq!(explain_failure("File too large (os error 27)", Some(Family::Portable), Some(dir)), ("The file is larger than this location allows.".to_string(), Fix::OpenFolder));
        assert_eq!(fix("No space left on device", Some(Family::Native)), Fix::None, "nowhere to open");
    }

    #[test]
    fn small_helpers() {
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(url_query("Sea Glass&x"), "Sea%20Glass%26x");
        assert_eq!((phase_index(Phase::Inspect), phase_index(Phase::Integrate), phase_index(Phase::Done)), (0, 2, 3));
    }
}
