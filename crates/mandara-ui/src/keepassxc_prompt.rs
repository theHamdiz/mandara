//! The KeePassXC unlock dialog: type a database password in Mandara's
//! style instead of KeePassXC's window.
//!
//! The same chromatic-glass modal as the keyring dialog: KeePassXC's icon
//! and name, a database picker when it knows several (name + path, lock
//! state), the password pill (show / hide, Caps Lock hint), an optional key
//! file (portal file chooser; KeePassXC's remembered one is preselected),
//! and Cancel / Unlock. The password goes to KeePassXC with
//! `openDatabase(file, password[, key file])` and is wiped right after
//! ([`mandara_services::keepassxc`]). KeePassXC reports no failure, so the
//! dialog re-checks the lock state and says so when it's still locked.
//! Databases opened with a hardware key (YubiKey challenge-response) can't
//! be unlocked this way: the dialog says so and offers KeePassXC's window.
//!
//! Entry points: the quick-settings tile (only when KeePassXC is installed),
//! the launcher, `mandara msg keepassxc unlock [<db>] | lock | status`, and
//! `general.keepassxc_ask_at_login` (once per login, when KeePassXC runs
//! with a locked database). `general.keepassxc_prompt` turns it all off.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, BoxShadow, ClickEvent, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent,
    ModifiersChangedEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowHandle, div, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::keepassxc::{self, Database, LockState, Status, UnlockOutcome};
use serde_json::json;
use zeroize::Zeroizing;

use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::keyring_prompt::{flow_text, soft_shadow};

/// Text caret blink half-period (platform convention).
const CARET_BLINK: Duration = Duration::from_millis(530);
/// After login, KeePassXC gets this long to appear (autostart) …
const LOGIN_WAIT: Duration = Duration::from_secs(90);
/// … checked this often.
const LOGIN_POLL: Duration = Duration::from_secs(2);
/// "Start KeePassXC": how long to wait for its window to register.
const START_WAIT: Duration = Duration::from_secs(20);
const START_POLL: Duration = Duration::from_millis(500);
/// Marker in `$XDG_RUNTIME_DIR/mandara`: the login question was asked
/// (shell restarts don't ask again).
const ASKED_MARKER: &str = "keepassxc-asked";
/// KeePassXC's icon names (Flatpak / desktop-file id, then the classic one).
const APP_ICONS: [&str; 2] = [keepassxc::FLATPAK_ID, "keepassxc"];
const FLATPAK: &str = "flatpak";
/// Longest password the field accepts (bytes).
const MAX_INPUT: usize = 1024;
/// Dots drawn at most for a hidden password.
const MAX_DOTS: usize = 28;
/// Database rows shown before the list scrolls.
const MAX_ROWS: usize = 4;
const SHAKE: SpringConfig = SpringConfig::new(620.0, 13.0, 1.0);
const APPEAR: SpringConfig = SpringConfig::new(300.0, 23.0, 1.0);
const CLOSE_STIFFNESS: f32 = 420.0;
const FLASH_STIFFNESS: f32 = 90.0;
/// Kick given to the shake spring: still locked / empty submit.
const SHAKE_WRONG: f32 = -2400.0;
const SHAKE_EMPTY: f32 = -1400.0;

/// Geometry (a fixed design, mirroring the keyring dialog).
mod dim {
    pub const CARD_W: f32 = 468.0;
    pub const CARD_PAD: f32 = 24.0;
    pub const CARD_RADIUS: f32 = 26.0;
    pub const CARD_GAP: f32 = 16.0;
    pub const CARD_LIFT: f32 = 26.0;
    pub const ICON_DISC: f32 = 52.0;
    pub const APP_ICON: f32 = 30.0;
    pub const ICON_GLYPH: f32 = 24.0;
    pub const HEADER_GAP: f32 = 14.0;
    pub const TITLE: f32 = 17.0;
    pub const SUBTITLE: f32 = 12.0;
    pub const TINY_GAP: f32 = 2.0;
    pub const GAP_S: f32 = 6.0;
    pub const GAP: f32 = 8.0;
    pub const GAP_M: f32 = 10.0;
    pub const ROW_H: f32 = 48.0;
    pub const ROW_PAD: f32 = 12.0;
    pub const ROW_RADIUS: f32 = 14.0;
    pub const ROW_ICON: f32 = 16.0;
    pub const PATH_TEXT: f32 = 11.5;
    pub const NAME_TEXT: f32 = 13.5;
    pub const FIELD_H: f32 = 46.0;
    pub const FIELD_PAD_L: f32 = 18.0;
    pub const FIELD_PAD_R: f32 = 8.0;
    pub const FIELD_RADIUS: f32 = 23.0;
    pub const FIELD_TEXT: f32 = 14.0;
    pub const FIELD_ICON: f32 = 15.0;
    pub const DOT: f32 = 8.0;
    pub const DOT_GAP: f32 = 7.0;
    pub const CARET_W: f32 = 1.5;
    pub const CARET_H: f32 = 18.0;
    pub const REVEAL: f32 = 32.0;
    pub const REVEAL_ICON: f32 = 16.0;
    pub const SMALL_BUTTON_H: f32 = 30.0;
    pub const SMALL_BUTTON_PAD: f32 = 12.0;
    pub const SMALL_TEXT: f32 = 12.0;
    pub const NOTE_TEXT: f32 = 12.5;
    pub const NOTE_LINE: f32 = 17.0;
    pub const NOTE_ICON: f32 = 14.0;
    pub const CAPS_ICON: f32 = 13.0;
    pub const BUTTON_H: f32 = 40.0;
    pub const BUTTON_PAD: f32 = 20.0;
    pub const BUTTON_PAD_S: f32 = 18.0;
    pub const BUTTON_RADIUS: f32 = 20.0;
    pub const BUTTON_TEXT: f32 = 13.5;
    pub const FIELD_SHADOW_BLUR: f32 = 14.0;
    pub const FIELD_SHADOW_Y: f32 = 4.0;
    pub const CARD_SHADOW_BLUR: f32 = 48.0;
    pub const CARD_SHADOW_Y: f32 = 18.0;
    pub const GLOW_BLUR: f32 = 16.0;
    pub const GLOW_Y: f32 = 4.0;
    /// The viewport is "settled" when within this of the output size.
    pub const SETTLE_EPS: f32 = 2.0;
}

/// Alphas.
mod alpha {
    pub const DISC: f32 = 0.14;
    pub const DISC_RIM: f32 = 0.28;
    pub const SELECTED: f32 = 0.14;
    pub const SELECTED_RIM: f32 = 0.45;
    pub const ROW: f32 = 0.45;
    pub const HOVER: f32 = 0.08;
    pub const FIELD: f32 = 0.75;
    pub const RIM: f32 = 0.16;
    pub const PLACEHOLDER: f32 = 0.7;
    pub const FIELD_SHADOW: f32 = 0.18;
    pub const CARD: f32 = 0.94;
    pub const CARD_RIM: f32 = 0.14;
    pub const CARD_SHADOW: f32 = 0.38;
    pub const GLOW: f32 = 0.35;
    pub const SECONDARY: f32 = 0.55;
    pub const DISABLED: f32 = 0.55;
    pub const SCRIM: f32 = 0.52;
    pub const FLASH_MIN: f32 = 0.02;
    pub const CLOSED: f32 = 0.01;
}

/// Gradient angle of the primary button (as the keyring dialog's).
const BUTTON_GRADIENT_DEG: f32 = 135.0;

/// Shell-wide state.
#[derive(Default)]
pub struct KeepassUi {
    window: Option<WindowHandle<KeepassDialog>>,
    /// KeePassXC is installed (checked once at start).
    pub installed: bool,
    /// The last status read (tile detail).
    pub status: Option<Status>,
}

impl Global for KeepassUi {}

fn is_open(cx: &App) -> bool {
    cx.try_global::<KeepassUi>().and_then(|u| u.window).is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

fn enabled(cx: &App) -> bool {
    crate::state::config(cx).read(cx).get().general.keepassxc_prompt
}

/// KeePassXC is installed and Mandara's dialog is on (the tile shows).
pub fn available(cx: &App) -> bool {
    cx.try_global::<KeepassUi>().is_some_and(|u| u.installed) && enabled(cx)
}

/// Every database is unlocked (as far as KeePassXC's Secret Service says).
pub fn unlocked(cx: &App) -> bool {
    cx.try_global::<KeepassUi>().and_then(|u| u.status.as_ref()).is_some_and(|s| s.running && s.any_locked() == Some(false))
}

/// Shell start: checks for KeePassXC and, once per login, asks for the
/// password when `general.keepassxc_ask_at_login` is on.
pub fn init(cx: &mut App) {
    let installed = keepassxc::installed();
    cx.set_global(KeepassUi { installed, ..KeepassUi::default() });
    let cfg = crate::state::config(cx).read(cx).get().general.clone();
    if !installed || !cfg.keepassxc_prompt || !cfg.keepassxc_ask_at_login {
        return;
    }
    let marker = dirs::runtime_dir().unwrap_or_else(mandara_config::paths::cache_dir).join("mandara").join(ASKED_MARKER);
    if marker.exists() {
        return; // a shell restart, not a new login
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&marker, std::env::var("XDG_SESSION_ID").unwrap_or_default());
    let rx = keepassxc::spawn_wait_status(LOGIN_WAIT, LOGIN_POLL);
    cx.spawn(async move |cx| {
        let Ok(Ok(status)) = rx.recv().await else { return };
        // Without KeePassXC's Secret Service the state is unknown; it
        // reopens its last databases locked, so those count.
        let locked = status.any_locked().unwrap_or_else(|| status.databases.iter().any(|d| d.reopened));
        if status.running && locked {
            cx.update(|cx| {
                if enabled(cx) {
                    show(status, None, cx);
                }
            });
        }
    })
    .detach();
}

/// Opens the dialog (for database `query`, else the likeliest one).
pub fn open(query: Option<String>, cx: &mut App) -> Result<(), String> {
    if !enabled(cx) {
        return Err(t("Mandara's KeePassXC dialog is off (Studio → General → Keyrings & passwords)").into());
    }
    if is_open(cx) {
        return Ok(());
    }
    let rx = keepassxc::spawn_status();
    cx.spawn(async move |cx| {
        let status = rx.recv().await.ok().and_then(Result::ok).unwrap_or_else(|| Status { installed: keepassxc::installed(), ..Status::default() });
        cx.update(|cx| show(status, query, cx));
    })
    .detach();
    Ok(())
}


fn show(status: Status, query: Option<String>, cx: &mut App) {
    let selected = query.as_deref().and_then(|q| status.databases.iter().position(|d| Some(d) == status.find(q))).or_else(|| status.preferred().and_then(|p| status.databases.iter().position(|d| d == p))).unwrap_or(0);
    let key_file = status.databases.get(selected).and_then(|d| d.key_file.clone());
    let form = Form { status, selected, password: Zeroizing::new(String::with_capacity(MAX_INPUT)), reveal: false, key_file, message: None };
    open_window(form, cx);
}

fn open_window(form: Form, cx: &mut App) {
    if is_open(cx) {
        return;
    }
    if !cx.has_global::<KeepassUi>() {
        cx.set_global(KeepassUi::default());
    }
    cx.global_mut::<KeepassUi>().status = Some(form.status.clone());
    // The focused monitor: its display and size together (issue #8).
    let (display, (w, h)) = crate::outputs::overlay_target(cx);
    // The lock role's namespace carries every compositor's no-animation /
    // no-shadow layer rules (the polkit and keyring dialogs use it too).
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, gpui::size(px(w), px(h)), display).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    let handle = cx
        .open_window(spec.window_options(), move |window, cx| cx.new(|cx| KeepassDialog::new(form, (w, h), window, cx)))
        .map_err(|e| tracing::error!("keepassxc dialog: {e:#}"))
        .ok();
    cx.global_mut::<KeepassUi>().window = handle;
}

/// Closes the dialog (Cancel).
pub fn close(cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<KeepassUi>().and_then(|u| u.window) else { return false };
    handle.update(cx, |d, _, cx| d.cancel(cx)).is_ok()
}

// ─────────────────────────────── the dialog ───────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq)]
enum Tone {
    Error,
    Info,
}

/// What the dialog holds (kept while the key-file chooser is up).
struct Form {
    status: Status,
    selected: usize,
    password: Zeroizing<String>,
    reveal: bool,
    key_file: Option<PathBuf>,
    message: Option<(Tone, String)>,
}

impl Form {
    fn db(&self) -> Option<&Database> {
        self.status.databases.get(self.selected)
    }

    /// The selected database needs a hardware key KeePassXC can use.
    fn hardware(&self) -> bool {
        self.status.hardware_key_support && self.db().is_some_and(|d| d.hardware_key)
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Busy {
    Idle,
    Checking,
    Starting,
}

pub struct KeepassDialog {
    form: Form,
    busy: Busy,
    /// Sent without a way to check: the primary button just closes.
    done: bool,
    has_app_icon: bool,
    focus: FocusHandle,
    appear: SpringValue<f32>,
    shake: SpringValue<f32>,
    flash: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    /// Closing to show the key-file chooser (reopens afterwards).
    picking: bool,
    expected: (f32, f32),
    blink_from: Instant,
    _blink: gpui::Task<()>,
}

fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl KeepassDialog {
    fn new(form: Form, expected: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut appear = SpringValue::new(0.0, APPEAR);
        appear.set_target(1.0);
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            form,
            busy: Busy::Idle,
            done: false,
            has_app_icon: APP_ICONS.iter().any(|id| crate::apps::icon_for_app_id(id, false).is_some()),
            focus,
            appear,
            shake: SpringValue::new(0.0, SHAKE),
            flash: SpringValue::new(0.0, SpringConfig::critical(FLASH_STIFFNESS)),
            clock: FrameClock::new(),
            closing: false,
            picking: false,
            expected,
            blink_from: Instant::now(),
            _blink: blink,
        }
    }

    fn kick(&mut self, v: f32, flash: bool) {
        self.shake.add_velocity([v, 0.0, 0.0, 0.0]);
        if flash {
            self.flash.snap_to(1.0);
            self.flash.set_target(0.0);
        }
    }

    fn select(&mut self, ix: usize) {
        if self.busy != Busy::Idle || ix >= self.form.status.databases.len() || ix == self.form.selected {
            return;
        }
        self.form.selected = ix;
        self.form.key_file = self.form.db().and_then(|d| d.key_file.clone());
        self.form.message = None;
        self.done = false;
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy != Busy::Idle || self.closing {
            return;
        }
        if self.done {
            return self.cancel(cx);
        }
        if !self.form.status.running {
            return self.start_keepassxc(cx);
        }
        if self.form.status.databases.is_empty() {
            // No database known yet: bring up KeePassXC itself (a second
            // launch raises the running instance).
            if let Err(e) = launch_keepassxc() {
                tracing::warn!("keepassxc: {e}");
            }
            return self.cancel(cx);
        }
        let Some(db) = self.form.db().map(|d| d.path.clone()) else { return };
        if self.form.hardware() {
            return self.open_keepassxc(cx);
        }
        if self.form.password.is_empty() {
            return self.kick(SHAKE_EMPTY, false);
        }
        let password = Zeroizing::new(self.form.password.as_str().to_string());
        wipe(&mut self.form.password);
        self.busy = Busy::Checking;
        self.form.message = None;
        let rx = keepassxc::spawn_unlock(db, password, self.form.key_file.clone());
        cx.spawn(async move |this, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = Busy::Idle;
                match result {
                    Ok(UnlockOutcome::Unlocked) => {
                        if let Some(db) = this.form.status.databases.get_mut(this.form.selected) {
                            db.state = LockState::Unlocked;
                        }
                        if cx.has_global::<KeepassUi>() {
                            cx.global_mut::<KeepassUi>().status = Some(this.form.status.clone());
                        }
                        this.close_now(cx);
                    }
                    Ok(UnlockOutcome::StillLocked) => {
                        this.form.message = Some((Tone::Error, t("Still locked: the password or key file may be wrong. Try again, or unlock in KeePassXC's window.").into()));
                        this.kick(SHAKE_WRONG, true);
                    }
                    Ok(UnlockOutcome::Sent) => {
                        this.form.message = Some((Tone::Info, t("Sent to KeePassXC. It can't confirm the result without its Secret Service integration; if it's still locked, KeePassXC shows its own unlock screen.").into()));
                        this.done = true;
                    }
                    Err(e) => {
                        this.form.message = Some((Tone::Error, e));
                        this.kick(SHAKE_WRONG, true);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// KeePassXC's own window for the selected database (hardware keys).
    fn open_keepassxc(&mut self, cx: &mut Context<Self>) {
        if let Some(db) = self.form.db().map(|d| d.path.clone()) {
            let rx = keepassxc::spawn_open_window(db);
            cx.background_executor()
                .spawn(async move {
                    if let Ok(Err(e)) = rx.recv().await {
                        tracing::warn!("keepassxc: {e}");
                    }
                })
                .detach();
        }
        self.cancel(cx);
    }

    /// Starts KeePassXC and waits for it to show up on the bus.
    fn start_keepassxc(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = launch_keepassxc() {
            self.form.message = Some((Tone::Error, tf("Couldn't start KeePassXC: {error}", &[("error", &e)])));
            return cx.notify();
        }
        self.busy = Busy::Starting;
        self.form.message = None;
        let rx = keepassxc::spawn_wait_status(START_WAIT, START_POLL);
        cx.spawn(async move |this, cx| {
            let status = rx.recv().await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                this.busy = Busy::Idle;
                match status {
                    Some(s) if s.running => {
                        let path = this.form.db().map(|d| d.path.clone());
                        this.form.selected = path.and_then(|p| s.databases.iter().position(|d| d.path == p)).unwrap_or(0);
                        this.form.status = s;
                    }
                    _ => this.form.message = Some((Tone::Error, t("KeePassXC didn't start.").into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Hides the dialog, shows the portal's file chooser, comes back.
    fn pick_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy != Busy::Idle {
            return;
        }
        let start = self.form.key_file.as_ref().and_then(|k| k.parent().map(PathBuf::from)).or_else(dirs::home_dir);
        let form = std::mem::replace(&mut self.form, Form { status: Status::default(), selected: 0, password: Zeroizing::new(String::new()), reveal: false, key_file: None, message: None });
        self.picking = true;
        self.closing = true;
        window.remove_window();
        let title = t("Choose the key file").to_string();
        let task = cx.background_executor().spawn(async move { mandara_services::file_chooser::open_file(&title, &[], start) });
        cx.spawn(async move |_, cx| {
            let picked = task.await;
            let mut form = form;
            match picked {
                Ok(Some(path)) => form.key_file = Some(path),
                Ok(None) => {}
                Err(e) => form.message = Some((Tone::Error, tf("The file chooser failed: {error}", &[("error", &e)]))),
            }
            cx.update(|cx| open_window(form, cx));
        })
        .detach();
    }

    fn close_now(&mut self, cx: &mut Context<Self>) {
        wipe(&mut self.form.password);
        self.closing = true;
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.closing {
            self.close_now(cx);
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        self.blink_from = Instant::now();
        match k.key.as_str() {
            "escape" => return self.cancel(cx),
            "enter" => return self.submit(cx),
            "up" => self.select(self.form.selected.saturating_sub(1)),
            "down" => self.select(self.form.selected + 1),
            _ if self.busy != Busy::Idle => {}
            "backspace" if k.modifiers.control => wipe(&mut self.form.password),
            "backspace" => {
                self.form.password.pop();
            }
            "u" if k.modifiers.control => wipe(&mut self.form.password),
            _ => {
                if let Some(ch) = k.key_char.as_ref().filter(|_| !k.modifiers.control && !k.modifiers.alt)
                    && self.form.password.len() + ch.len() <= MAX_INPUT
                {
                    self.form.password.push_str(ch);
                    if self.form.message.as_ref().is_some_and(|(tone, _)| *tone == Tone::Error) {
                        self.form.message = None;
                    }
                }
            }
        }
        cx.notify();
    }
}

impl Drop for KeepassDialog {
    fn drop(&mut self) {
        wipe(&mut self.form.password);
    }
}

/// Starts KeePassXC (native, else the Flatpak), detached.
fn launch_keepassxc() -> std::io::Result<()> {
    let mut cmd = if which(keepassxc::BINARY_NAME) {
        std::process::Command::new(keepassxc::BINARY_NAME)
    } else {
        let mut c = std::process::Command::new(FLATPAK);
        c.args(["run", keepassxc::FLATPAK_ID]);
        c
    };
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    mandara_services::reaper::spawn(&mut cmd).map(|_| ())
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// `~/x` for paths in the home directory.
fn short_path(p: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h).ok().map(PathBuf::from)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

impl Render for KeepassDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let settled = (f32::from(vp.width) - self.expected.0).abs() < dim::SETTLE_EPS && (f32::from(vp.height) - self.expected.1).abs() < dim::SETTLE_EPS;
        if self.picking {
            return div().id("keepassxc-root").size_full().into_any_element();
        }
        if !settled && !self.closing {
            window.request_animation_frame();
            return div().id("keepassxc-root").size_full().track_focus(&self.focus).into_any_element();
        }
        let p = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        self.appear.set_config(if self.closing { SpringConfig::critical(CLOSE_STIFFNESS) } else { APPEAR });
        self.appear.set_target(if self.closing { 0.0 } else { 1.0 });
        let dt = self.clock.tick(Instant::now());
        let mut moving = self.appear.step(dt);
        moving |= self.shake.step(dt);
        moving |= self.flash.step(dt);
        if self.closing && self.appear.value() <= alpha::CLOSED {
            window.remove_window();
        } else if moving || self.busy != Busy::Idle {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let a = self.appear.value();
        let shown = a.clamp(0.0, 1.0);
        let lift = (1.0 - a) * dim::CARD_LIFT;
        let busy = self.busy;
        let form = &self.form;
        let running = form.status.running;
        let hardware = form.hardware();

        // Header: KeePassXC's icon in a tinted disc, title and subtitle.
        let glyph = if self.has_app_icon {
            crate::plugins::workspaces::app_icon(APP_ICONS[0], dim::APP_ICON, p.primary, false, &p)
        } else {
            icon("key", dim::ICON_GLYPH, p.primary).into_any_element()
        };
        let subtitle = match form.db() {
            Some(db) if form.status.databases.len() == 1 => db.name.clone(),
            Some(_) => tf("{count} databases", &[("count", &form.status.databases.len())]),
            None => t("KeePassXC").to_string(),
        };
        let header = div()
            .row()
            .items_center()
            .gap(px(dim::HEADER_GAP))
            .child(
                div()
                    .size(px(dim::ICON_DISC))
                    .flex_none()
                    .rounded_full()
                    .bg(p.primary.with_alpha(alpha::DISC).hsla())
                    .border_1()
                    .border_color(p.primary.with_alpha(alpha::DISC_RIM).hsla())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(dim::TINY_GAP))
                    .min_w_0()
                    .child(div().text_size(px(dim::TITLE)).font_weight(FontWeight::SEMIBOLD).child(t("Unlock KeePassXC")))
                    .child(div().text_size(px(dim::SUBTITLE)).text_color(p.on_surface_variant.hsla()).child(subtitle)),
            );

        // Database picker (several databases).
        let picker = (form.status.databases.len() > 1).then(|| {
            let rows = form.status.databases.iter().enumerate().map(|(ix, db)| {
                let selected = ix == form.selected;
                let (state_icon, state_color) = match db.state {
                    LockState::Unlocked => ("lock-open", p.success_emerald),
                    LockState::Locked => ("lock", p.on_surface_variant),
                    LockState::Unknown => ("key", p.on_surface_variant),
                };
                div()
                    .id(("keepassxc-db", ix))
                    .flex_none()
                    .h(px(dim::ROW_H))
                    .px(px(dim::ROW_PAD))
                    .rounded(px(crate::style::radius(dim::ROW_RADIUS)))
                    .border_1()
                    .border_color(if selected { p.primary.with_alpha(alpha::SELECTED_RIM).hsla() } else { p.outline.with_alpha(0.0).hsla() })
                    .bg(if selected { p.primary.with_alpha(alpha::SELECTED).hsla() } else { p.surface_container_high.with_alpha(alpha::ROW).hsla() })
                    .hover(|s| s.bg(p.on_surface.with_alpha(alpha::HOVER).hsla()))
                    .cursor_pointer()
                    .row()
                    .items_center()
                    .gap(px(dim::GAP_M))
                    .child(icon(state_icon, dim::ROW_ICON, state_color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(dim::NAME_TEXT)).font_weight(FontWeight::MEDIUM).truncate().child(db.name.clone()))
                            .child(div().text_size(px(dim::PATH_TEXT)).text_color(p.on_surface_variant.hsla()).truncate().child(short_path(&db.path))),
                    )
                    .when(selected, |d| d.child(icon("check", dim::ROW_ICON, p.primary)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.select(ix);
                        cx.notify();
                    }))
            });
            div()
                .id("keepassxc-dbs")
                .max_h(px(dim::ROW_H * MAX_ROWS as f32 + dim::GAP_S * (MAX_ROWS - 1) as f32))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(dim::GAP_S))
                .children(rows)
        });

        // Password pill.
        let caret_on = (self.blink_from.elapsed().as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2) && busy == Busy::Idle;
        let flash = self.flash.value().clamp(0.0, 1.0);
        let text = form.password.as_str();
        let content = if text.is_empty() {
            div().text_color(p.on_surface_variant.with_alpha(alpha::PLACEHOLDER).hsla()).child(t("Database password")).into_any_element()
        } else if form.reveal {
            div().child(text.to_string()).into_any_element()
        } else {
            let n = text.chars().count().min(MAX_DOTS);
            div().flex().items_center().gap(px(dim::DOT_GAP)).children((0..n).map(|_| div().size(px(dim::DOT)).rounded_full().bg(p.on_surface.hsla()))).into_any_element()
        };
        let mut shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        shadows.push(soft_shadow(dim::FIELD_SHADOW_BLUR, alpha::FIELD_SHADOW, dim::FIELD_SHADOW_Y));
        let rim = if flash > alpha::FLASH_MIN { p.urgent_coral.with_alpha(flash).hsla() } else { p.outline.with_alpha(alpha::RIM).hsla() };
        let empty = text.is_empty();
        let caret = || div().w(px(dim::CARET_W)).h(px(dim::CARET_H)).bg(p.primary.hsla());
        let field = (running && !hardware && form.db().is_some()).then(|| {
            div()
                .id("keepassxc-field")
                .ml(px(self.shake.value()))
                .h(px(dim::FIELD_H))
                .pl(px(dim::FIELD_PAD_L))
                .pr(px(dim::FIELD_PAD_R))
                .rounded(px(crate::style::radius(dim::FIELD_RADIUS)))
                .bg(p.surface_container_high.with_alpha(alpha::FIELD).hsla())
                .border_1()
                .border_color(rim)
                .shadow(shadows)
                .row()
                .items_center()
                .gap(px(dim::GAP_M))
                .text_size(px(dim::FIELD_TEXT))
                .cursor_text()
                .child(icon("lock", dim::FIELD_ICON, p.on_surface_variant))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .overflow_hidden()
                        .when(caret_on && empty, |d| d.child(caret()))
                        .child(content)
                        .when(caret_on && !empty, |d| d.child(caret())),
                )
                .child(
                    div()
                        .id("keepassxc-reveal")
                        .size(px(dim::REVEAL))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(p.on_surface.with_alpha(alpha::HOVER).hsla()))
                        .child(icon(if form.reveal { "eye-off" } else { "eye" }, dim::REVEAL_ICON, p.on_surface_variant))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.form.reveal = !this.form.reveal;
                            cx.notify();
                        })),
                )
        });

        let small_button = |id: &'static str, label: String| {
            div()
                .id(id)
                .h(px(dim::SMALL_BUTTON_H))
                .px(px(dim::SMALL_BUTTON_PAD))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .text_size(px(dim::SMALL_TEXT))
                .bg(p.surface_container_high.with_alpha(alpha::SECONDARY).hsla())
                .hover(|s| s.bg(p.surface_container_high.hsla()))
                .cursor_pointer()
                .child(label)
        };
        // Key file: the remembered one, or "none"; Choose… / clear.
        let key_row = (running && !hardware && form.db().is_some()).then(|| {
            let label = form.key_file.as_deref().map_or_else(|| t("No key file").to_string(), short_path);
            div()
                .row()
                .items_center()
                .gap(px(dim::GAP))
                .child(icon("folder", dim::FIELD_ICON, p.on_surface_variant))
                .child(div().flex_1().min_w_0().truncate().text_size(px(dim::SMALL_TEXT)).text_color(p.on_surface_variant.hsla()).child(label))
                .when(form.key_file.is_some(), |d| {
                    d.child(small_button("keepassxc-key-clear", t("Remove").to_string()).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.form.key_file = None;
                        cx.notify();
                    })))
                })
                .child(small_button("keepassxc-key-pick", t("Key file…").to_string()).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.pick_key_file(window, cx))))
        });

        let caps = (field.is_some() && window.capslock().on).then(|| {
            div()
                .row()
                .items_center()
                .gap(px(dim::GAP_S))
                .text_size(px(dim::SMALL_TEXT))
                .text_color(p.warning_amber.hsla())
                .child(icon("arrow-up", dim::CAPS_ICON, p.warning_amber))
                .child(t("Caps Lock is on"))
        });

        // Notes: not running, no databases, hardware key, result messages.
        let mut notes: Vec<(Tone, String)> = Vec::new();
        if !running {
            notes.push((Tone::Info, if busy == Busy::Starting { t("Starting KeePassXC…").to_string() } else { t("KeePassXC isn't running. Start it, then unlock your database here.").to_string() }));
        } else if form.status.databases.is_empty() {
            notes.push((Tone::Info, t("KeePassXC hasn't opened a database yet. Open one in KeePassXC once; after that Mandara can unlock it.").to_string()));
        } else if hardware {
            notes.push((Tone::Info, t("This database uses a hardware key (YubiKey challenge-response), which Mandara can't pass to KeePassXC. Unlock it in KeePassXC's window.").to_string()));
        }
        notes.extend(form.message.clone());
        let note_els: Vec<_> = notes
            .into_iter()
            .map(|(tone, text)| {
                let (glyph, color) = match tone {
                    Tone::Error => ("alert", p.urgent_coral),
                    Tone::Info => ("shield", p.on_surface_variant),
                };
                div().row().items_start().gap(px(dim::GAP_S)).text_color(color.hsla()).child(icon(glyph, dim::NOTE_ICON, color)).child(div().flex_1().min_w_0().child(flow_text(&text, dim::NOTE_TEXT, dim::NOTE_LINE)))
            })
            .collect();

        let primary_label = match busy {
            Busy::Checking => t("Checking…").to_string(),
            Busy::Starting => t("Starting…").to_string(),
            Busy::Idle if self.done => t("Done").to_string(),
            Busy::Idle if !running => t("Start KeePassXC").to_string(),
            Busy::Idle if hardware || form.status.databases.is_empty() => t("Open KeePassXC").to_string(),
            Busy::Idle => t("Unlock").to_string(),
        };
        let ready = busy == Busy::Idle && (self.done || !running || hardware || form.db().is_none() || !form.password.is_empty());
        let primary = div()
            .id("keepassxc-continue")
            .h(px(dim::BUTTON_H))
            .px(px(dim::BUTTON_PAD))
            .rounded(px(crate::style::radius(dim::BUTTON_RADIUS)))
            .flex()
            .items_center()
            .justify_center()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(dim::BUTTON_TEXT))
            .text_color(p.on_primary.hsla())
            .bg(mandara_core::oklab_gradient(BUTTON_GRADIENT_DEG, p.accent_gradient_start, p.accent_gradient_end))
            .shadow(vec![BoxShadow { color: p.primary_glow.with_alpha(alpha::GLOW).hsla(), offset: point(px(0.0), px(dim::GLOW_Y)), blur_radius: px(dim::GLOW_BLUR), spread_radius: px(0.0), inset: false }])
            .opacity(if ready || busy != Busy::Idle { 1.0 } else { alpha::DISABLED })
            .cursor_pointer()
            .child(primary_label)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.submit(cx);
                cx.notify();
            }));
        let cancel = div()
            .id("keepassxc-cancel")
            .h(px(dim::BUTTON_H))
            .px(px(dim::BUTTON_PAD_S))
            .rounded(px(crate::style::radius(dim::BUTTON_RADIUS)))
            .flex()
            .items_center()
            .text_size(px(dim::BUTTON_TEXT))
            .text_color(p.on_surface.hsla())
            .bg(p.surface_container_high.with_alpha(alpha::SECONDARY).hsla())
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .cursor_pointer()
            .child(t("Cancel"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.cancel(cx)));

        let mut card_shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        card_shadows.push(soft_shadow(dim::CARD_SHADOW_BLUR, alpha::CARD_SHADOW, dim::CARD_SHADOW_Y));
        let has_inputs = field.is_some() || caps.is_some() || key_row.is_some();
        let card = div()
            .id("keepassxc-card")
            .occlude()
            .w(px(dim::CARD_W))
            .mt(px(lift))
            .opacity(shown)
            .p(px(dim::CARD_PAD))
            .rounded(px(crate::style::radius(dim::CARD_RADIUS)))
            .bg(p.surface_container.with_alpha(alpha::CARD).hsla())
            .border_1()
            .border_color(p.outline.with_alpha(alpha::CARD_RIM).hsla())
            .shadow(card_shadows)
            .flex()
            .flex_col()
            .gap(px(dim::CARD_GAP))
            .child(header)
            .children(picker)
            .when(has_inputs, |d| d.child(div().flex().flex_col().gap(px(dim::GAP)).children(field).children(caps).children(key_row)))
            .when(!note_els.is_empty(), |d| d.child(div().flex().flex_col().gap(px(dim::GAP)).children(note_els)))
            .child(div().row().justify_end().gap(px(dim::GAP_M)).child(cancel).child(primary));

        div()
            .id("keepassxc-root")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_modifiers_changed(cx.listener(|_, _: &ModifiersChangedEvent, _, cx| cx.notify()))
            .font_family(f.ui.clone())
            .text_color(p.on_surface.hsla())
            .text_start()
            .bg(p.surface_dim.with_alpha(alpha::SCRIM * shown).hsla())
            .flex()
            .items_center()
            .justify_center()
            .child(card)
            .into_any_element()
    }
}

// ─────────────────────────────── commands ───────────────────────────────

fn status_json(s: &Status) -> serde_json::Value {
    json!({
        "installed": s.installed,
        "running": s.running,
        "secret_service": s.secret_service,
        "any_locked": s.any_locked(),
        "hardware_key_support": s.hardware_key_support,
        "databases": s.databases.iter().map(|d| json!({ "name": d.name, "path": d.path, "state": d.state, "key_file": d.key_file, "hardware_key": d.hardware_key, "last_active": d.last_active })).collect::<Vec<_>>(),
    })
}

/// Bridges a service reply to the IPC layer (as JSON text).
fn pending<T: Send + 'static>(rx: async_channel::Receiver<Result<T, String>>, to_json: impl FnOnce(T) -> serde_json::Value + Send + 'static, cx: &mut App) -> Output {
    let (tx, out) = async_channel::bounded(1);
    cx.background_executor()
        .spawn(async move {
            let reply = match rx.recv().await {
                Ok(Ok(v)) => Ok(to_json(v).to_string()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(t("The service did not reply").to_string()),
            };
            let _ = tx.send(reply).await;
        })
        .detach();
    Output::Pending(out)
}

/// `keepassxc unlock [<db>] | lock | status | close`.
pub fn register(r: &mut PluginRegistry) {
    r.command(
        "keepassxc",
        CommandSpec::new_output("keepassxc", tk("Unlock a KeePassXC database with Mandara's dialog; lock them all; show their state"), |a, cx| match a.str(0)? {
            "unlock" => {
                open(a.get(1).map(str::to_string).filter(|s| !s.is_empty()), cx)?;
                Ok(json!({ "dialog": "open" }).into())
            }
            "lock" => Ok(pending(keepassxc::spawn_lock_all(), |_| json!({ "locked": true }), cx)),
            "close" => Ok(json!({ "closed": close(cx) }).into()),
            _ => {
                let (tx, out) = async_channel::bounded(1);
                let rx = keepassxc::spawn_status();
                cx.spawn(async move |cx| {
                    let reply = match rx.recv().await {
                        Ok(Ok(s)) => {
                            let json = status_json(&s);
                            cx.update(|cx| {
                                if cx.has_global::<KeepassUi>() {
                                    cx.global_mut::<KeepassUi>().status = Some(s);
                                }
                            });
                            Ok(json.to_string())
                        }
                        Ok(Err(e)) => Err(e),
                        Err(_) => Err(t("The service did not reply").to_string()),
                    };
                    let _ = tx.send(reply).await;
                })
                .detach();
                Ok(Output::Pending(out))
            }
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("what to do")).choices(&["unlock", "lock", "status", "close"]))
        .arg(ArgSpec::optional("database", ArgKind::String, tk("database name or path (unlock)")))
        .example("keepassxc unlock")
        .example("keepassxc unlock Passwords")
        .example("keepassxc lock")
        .example("keepassxc status"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_use_a_tilde_in_home() {
        let home = dirs::home_dir().unwrap_or_default();
        assert_eq!(short_path(&home.join("Vault/Passwords.kdbx")), "~/Vault/Passwords.kdbx");
        assert_eq!(short_path(std::path::Path::new("/srv/x.kdbx")), "/srv/x.kdbx");
    }

    #[test]
    fn hardware_keys_need_support_and_a_flagged_database() {
        let db = |hw| Database { path: "/a.kdbx".into(), name: "a".into(), key_file: None, hardware_key: hw, last_active: true, reopened: true, state: LockState::Locked };
        let form = |support, hw| Form { status: Status { hardware_key_support: support, databases: vec![db(hw)], ..Status::default() }, selected: 0, password: Zeroizing::new(String::new()), reveal: false, key_file: None, message: None };
        assert!(form(true, true).hardware());
        assert!(!form(false, true).hardware(), "KeePassXC built without hardware-key support");
        assert!(!form(true, false).hardware());
    }
}
