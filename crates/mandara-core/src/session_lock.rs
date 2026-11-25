//! `ext-session-lock-v1` lock surfaces rendered into `wl_shm` buffers.
//!
//! GPUI cannot host a session-lock surface: every GPUI window receives its
//! role (`xdg_toplevel` / `zwlr_layer_surface_v1`) the moment it is created,
//! while `ext_session_lock_v1.get_lock_surface` requires a role-less
//! `wl_surface`, and GPUI has no off-screen renderer on Wayland to borrow
//! pixels from. The lock therefore owns its own Wayland connection and asks a
//! [`LockApp`] to paint each surface with `tiny-skia` (premultiplied RGBA),
//! which is then copied into shared-memory buffers.
//!
//! Two modes share every line of rendering and input code:
//!
//! * [`LockMode::Session`] — locks the session with `ext_session_lock_v1`,
//!   one lock surface per output (outputs hot-plugged while locked get one
//!   too). The process only ever leaves through `unlock_and_destroy` after
//!   the app reports a successful authentication, or when the compositor
//!   refuses / ends the lock (`finished`).
//! * [`LockMode::Demo`] — a preview: plain `zwlr_layer_shell_v1` overlay
//!   surfaces with exclusive keyboard focus; nothing is locked.
//!
//! Fractional scaling uses `wp_fractional_scale_v1` + `wp_viewporter` when
//! available (crisp at 1.25× / 1.5×), else the integer buffer scale.

use std::fs::File;
use std::os::fd::AsFd as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::session_lock::v1::client::{ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1};
use wayland_protocols::wp::fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use xkbcommon::xkb;

// ───────────────────────────── public API ─────────────────────────────

/// How the lock screen is presented.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockMode {
    /// Really lock the session (`ext-session-lock-v1`).
    Session,
    /// Preview on overlay layer surfaces; nothing is locked.
    Demo,
}

/// What the app wants after an update.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AppControl {
    Continue,
    /// Authentication succeeded and the exit animation finished.
    Unlock,
    /// Leave without unlocking (honoured only in demo mode).
    Exit,
}

/// One key press (or auto-repeat) after xkb translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    /// Raw xkb keysym (`xkb::keysyms::KEY_*`).
    pub keysym: u32,
    /// Text the key produces (empty for control keys).
    pub utf8: String,
    pub ctrl: bool,
    pub alt: bool,
}

/// Keyboard state worth showing (Caps Lock warning, layout name).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardState {
    pub caps_lock: bool,
    pub layout: String,
}

/// The surface being painted.
#[derive(Clone, Debug)]
pub struct SurfaceInfo {
    /// Output name (`DP-1`), when the compositor reports it.
    pub output: String,
    /// Logical size.
    pub width: u32,
    pub height: u32,
    /// Physical pixels per logical pixel.
    pub scale: f32,
    /// The output that currently holds keyboard focus (or the first one).
    pub focused: bool,
}

/// The lock screen's brain: input, animation and painting.
pub trait LockApp {
    /// The compositor confirmed the lock (immediately in demo mode).
    fn locked(&mut self) {}
    fn key(&mut self, key: KeyInput);
    /// The pointer moved or a button was pressed (lock grace periods).
    fn pointer_activity(&mut self) {}
    fn keyboard(&mut self, _state: &KeyboardState) {}
    /// Advances animations and polls authentication.
    fn update(&mut self, now: Instant) -> AppControl;
    /// Whether the surfaces must be repainted.
    fn needs_redraw(&self) -> bool;
    /// Paints one surface (premultiplied RGBA, physical pixels).
    fn draw(&mut self, surface: &SurfaceInfo, pixmap: &mut tiny_skia::PixmapMut<'_>, now: Instant);
    /// Every surface that needed it was painted this round.
    fn drawn(&mut self) {}
    /// Longest sleep before the next [`LockApp::update`] (input wakes earlier).
    fn next_wakeup(&self, now: Instant) -> Duration;
}

/// Why [`run`] returned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockOutcome {
    /// Authenticated and unlocked (or the demo was dismissed).
    Unlocked,
    /// The compositor ended the lock on its own after confirming it.
    EndedByCompositor,
}

/// Why [`run`] failed, and whether the session may still be locked.
#[derive(Debug)]
pub struct LockError {
    pub error: anyhow::Error,
    /// The lock request had been sent and was not resolved (neither unlocked
    /// nor refused or ended by the compositor) when this happened, so the
    /// session is locked or about to be, with no one left to unlock it. That
    /// covers a lock the compositor has not confirmed yet: it may confirm it
    /// after this process is gone, so the caller must put a new locker up
    /// rather than assume nothing was locked. Always `false` for the demo.
    pub locked: bool,
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.error)
    }
}

impl std::error::Error for LockError {}

// ───────────────────────── protocol state machine ─────────────────────────

/// Lifecycle of an `ext_session_lock_v1` object.
///
/// The protocol has sharp edges this encodes: `unlock_and_destroy` is a
/// protocol error before `locked`, plain `destroy` is one after `locked`
/// (unless `finished` came), and the app must never be able to leave a
/// confirmed lock without authenticating.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockPhase {
    /// `lock` sent, waiting for `locked` or `finished`.
    Pending,
    /// Authentication succeeded before the compositor confirmed the lock.
    PendingUnlock,
    /// The session is locked.
    Locked,
    /// `unlock_and_destroy` sent.
    Unlocked,
    /// The compositor refused the lock (`finished` before `locked`).
    Denied,
    /// The compositor ended a confirmed lock (`finished` after `locked`).
    Ended,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockEvent {
    Locked,
    Finished,
    AuthSucceeded,
    /// The app asked to leave without authenticating.
    ExitRequested,
}

/// What the client must send in response to a [`LockEvent`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockAction {
    None,
    UnlockAndDestroy,
    Destroy,
}

impl LockPhase {
    pub fn on(self, event: LockEvent) -> (LockPhase, LockAction) {
        use LockAction as A;
        use LockEvent as E;
        use LockPhase as P;
        match (self, event) {
            (P::Pending, E::Locked) => (P::Locked, A::None),
            (P::Pending | P::PendingUnlock, E::Finished) => (P::Denied, A::Destroy),
            (P::Pending, E::AuthSucceeded) => (P::PendingUnlock, A::None),
            (P::PendingUnlock, E::Locked) => (P::Unlocked, A::UnlockAndDestroy),
            (P::Locked, E::AuthSucceeded) => (P::Unlocked, A::UnlockAndDestroy),
            (P::Locked, E::Finished) => (P::Ended, A::Destroy),
            // Never leave a lock (pending or confirmed) without authenticating.
            (P::Pending | P::PendingUnlock | P::Locked, E::ExitRequested) => (self, A::None),
            (phase, _) => (phase, A::None),
        }
    }

    /// The event loop is over.
    pub fn is_terminal(self) -> bool {
        matches!(self, LockPhase::Unlocked | LockPhase::Denied | LockPhase::Ended)
    }
}

// ───────────────────────────── wayland state ─────────────────────────────

enum Role {
    Lock(ext_session_lock_surface_v1::ExtSessionLockSurfaceV1),
    Layer(zwlr_layer_surface_v1::ZwlrLayerSurfaceV1),
}

struct ShmBuffer {
    buffer: wl_buffer::WlBuffer,
    busy: Arc<AtomicBool>,
    map: memmap2::MmapMut,
    _file: File,
    width: u32,
    height: u32,
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

struct LockSurface {
    surface: wl_surface::WlSurface,
    role: Role,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    width: u32,
    height: u32,
    configured: bool,
    /// Preferred fractional scale ×120 (`wp_fractional_scale_v1`).
    scale120: u32,
    /// Integer buffer scale (fallback path).
    buffer_scale: i32,
    frame_pending: bool,
    dirty: bool,
    buffers: Vec<ShmBuffer>,
    /// Scratch pixmap the app paints into before the copy to shm.
    scratch: Option<tiny_skia::Pixmap>,
}

impl LockSurface {
    fn physical_size(&self) -> (u32, u32, f32) {
        physical_size_of(self.width, self.height, self.viewport.is_some(), self.scale120, self.buffer_scale)
    }

    fn uses_fractional_scale(&self) -> bool {
        uses_fractional_scale(self.viewport.is_some(), self.scale120)
    }

    fn attach_buffer_scale(&self) -> i32 {
        attach_buffer_scale(self.viewport.is_some(), self.scale120, self.buffer_scale)
    }

    fn destroy(self) {
        if let Some(f) = &self.fractional {
            f.destroy();
        }
        if let Some(v) = &self.viewport {
            v.destroy();
        }
        match &self.role {
            Role::Lock(l) => l.destroy(),
            Role::Layer(l) => l.destroy(),
        }
        drop(self.buffers);
        self.surface.destroy();
    }
}

struct Output {
    global: u32,
    output: wl_output::WlOutput,
    name: String,
    scale: i32,
    surface: Option<LockSurface>,
}

#[derive(Default)]
struct Keyboard {
    context: Option<xkb::Context>,
    keymap: Option<xkb::Keymap>,
    state: Option<xkb::State>,
    /// Keys per second and delay before repeating (`wl_keyboard.repeat_info`).
    rate: i32,
    delay: i32,
    /// Held key (evdev code) and when it fires next.
    repeat: Option<(u32, Instant)>,
    shown: KeyboardState,
}

/// A bound `wl_seat` and the devices taken from it.
struct Seat {
    /// The registry name, to release the seat when its global goes away.
    global: u32,
    seat: wl_seat::WlSeat,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
}

impl Seat {
    fn new(global: u32, seat: wl_seat::WlSeat) -> Self {
        Self { global, seat, keyboard: None, pointer: None }
    }

    /// Releases the keyboard if there is one; whether there was one. (A
    /// `release` request exists from the version that added it on.)
    fn release_keyboard(&mut self) -> bool {
        let Some(keyboard) = self.keyboard.take() else { return false };
        if keyboard.version() >= KEYBOARD_RELEASE_SINCE {
            keyboard.release();
        }
        true
    }

    fn release_pointer(&mut self) {
        if let Some(pointer) = self.pointer.take()
            && pointer.version() >= POINTER_RELEASE_SINCE
        {
            pointer.release();
        }
    }

    /// Releases the devices and the seat itself; whether a keyboard went.
    fn release(mut self) -> bool {
        let had_keyboard = self.release_keyboard();
        self.release_pointer();
        if self.seat.version() >= SEAT_RELEASE_SINCE {
            self.seat.release();
        }
        had_keyboard
    }
}

/// Interface versions that added `release` to `wl_keyboard`, `wl_pointer`
/// and `wl_seat`.
const KEYBOARD_RELEASE_SINCE: u32 = 3;
const POINTER_RELEASE_SINCE: u32 = 3;
const SEAT_RELEASE_SINCE: u32 = 5;

struct State {
    mode: LockMode,
    qh: QueueHandle<State>,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    abgr: bool,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    lock: Option<ext_session_lock_v1::ExtSessionLockV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    outputs: Vec<Output>,
    seats: Vec<Seat>,
    kb: Keyboard,
    /// Output (global name) whose surface has keyboard focus.
    focus: Option<u32>,
    phase: LockPhase,
    /// Lock events not yet handed to the app.
    pending_keys: Vec<KeyInput>,
    keyboard_changed: bool,
    locked_notice: bool,
    closed: bool,
    /// Pointer input since the last loop iteration.
    pointer_activity: bool,
}

impl State {
    fn output_mut(&mut self, global: u32) -> Option<&mut Output> {
        self.outputs.iter_mut().find(|o| o.global == global)
    }

    fn surface_mut(&mut self, global: u32) -> Option<&mut LockSurface> {
        self.output_mut(global).and_then(|o| o.surface.as_mut())
    }

    /// Creates the lock (or demo) surface for an output that lacks one.
    fn ensure_surface(&mut self, index: usize) {
        if self.phase.is_terminal() || self.outputs[index].surface.is_some() {
            return;
        }
        let qh = self.qh.clone();
        let global = self.outputs[index].global;
        // What the role needs is checked before the `wl_surface` exists, so
        // a missing prerequisite leaves nothing behind.
        let (surface, role) = match self.mode {
            LockMode::Session => {
                let Some(lock) = &self.lock else { return };
                let surface = self.compositor.create_surface(&qh, global);
                let role = Role::Lock(lock.get_lock_surface(&surface, &self.outputs[index].output, &qh, global));
                (surface, role)
            }
            LockMode::Demo => {
                let Some(shell) = &self.layer_shell else { return };
                let surface = self.compositor.create_surface(&qh, global);
                let layer = shell.get_layer_surface(
                    &surface,
                    Some(&self.outputs[index].output),
                    zwlr_layer_shell_v1::Layer::Overlay,
                    crate::SurfaceRole::Lock.namespace().into(),
                    &qh,
                    global,
                );
                use zwlr_layer_surface_v1::Anchor;
                layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
                layer.set_exclusive_zone(-1);
                layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
                surface.commit();
                (surface, Role::Layer(layer))
            }
        };
        let (viewport, fractional) = match (&self.viewporter, &self.fractional) {
            (Some(vp), Some(fm)) => (Some(vp.get_viewport(&surface, &qh, ())), Some(fm.get_fractional_scale(&surface, &qh, global))),
            _ => (None, None),
        };
        let buffer_scale = self.outputs[index].scale.max(1);
        self.outputs[index].surface = Some(LockSurface {
            surface,
            role,
            viewport,
            fractional,
            width: 0,
            height: 0,
            configured: false,
            scale120: 0,
            buffer_scale,
            frame_pending: false,
            dirty: true,
            buffers: Vec::new(),
            scratch: None,
        });
    }

    fn apply_lock_event(&mut self, event: LockEvent) {
        let (next, action) = self.phase.on(event);
        tracing::debug!("session lock: {:?} + {event:?} → {next:?} ({action:?})", self.phase);
        self.phase = next;
        if let Some(lock) = &self.lock {
            match action {
                LockAction::UnlockAndDestroy => lock.unlock_and_destroy(),
                LockAction::Destroy => lock.destroy(),
                LockAction::None => {}
            }
        }
        if action != LockAction::None {
            self.lock = None;
        }
        if next == LockPhase::Locked {
            self.locked_notice = true;
        }
    }

    fn handle_key(&mut self, code: u32) {
        let Some(state) = &self.kb.state else { return };
        let keycode = xkb::Keycode::new(code + EVDEV_XKB_OFFSET);
        let keysym = state.key_get_one_sym(keycode).raw();
        let utf8 = state.key_get_utf8(keycode);
        let ctrl = state.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        let alt = state.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
        self.pending_keys.push(KeyInput { keysym, utf8, ctrl, alt });
    }

    fn refresh_keyboard_state(&mut self) {
        let (Some(state), Some(keymap)) = (&self.kb.state, &self.kb.keymap) else { return };
        let caps_lock = state.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_LOCKED);
        let layout = keymap.layout_get_name(state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE)).to_string();
        let next = KeyboardState { caps_lock, layout };
        if next != self.kb.shown {
            self.kb.shown = next;
            self.keyboard_changed = true;
        }
    }
}

/// `wp_fractional_scale_v1` reports scales as multiples of 1/120.
const FRACTIONAL_SCALE_DENOMINATOR: u32 = 120;

/// Whether the surface paints through fractional scale + viewporter.
fn uses_fractional_scale(has_viewport: bool, scale120: u32) -> bool {
    has_viewport && scale120 > 0
}

/// Integer buffer scale for the next attach. Fractional mode is always 1.
fn attach_buffer_scale(has_viewport: bool, scale120: u32, buffer_scale: i32) -> i32 {
    if uses_fractional_scale(has_viewport, scale120) { 1 } else { buffer_scale.max(1) }
}

/// Physical pixel size for a logical surface at the given scale path.
fn physical_size_of(width: u32, height: u32, has_viewport: bool, scale120: u32, buffer_scale: i32) -> (u32, u32, f32) {
    if uses_fractional_scale(has_viewport, scale120) {
        let s = scale120 as f32 / FRACTIONAL_SCALE_DENOMINATOR as f32;
        let w = ((width * scale120 + FRACTIONAL_SCALE_DENOMINATOR / 2) / FRACTIONAL_SCALE_DENOMINATOR).max(1);
        let h = ((height * scale120 + FRACTIONAL_SCALE_DENOMINATOR / 2) / FRACTIONAL_SCALE_DENOMINATOR).max(1);
        (w, h, s)
    } else {
        let s = buffer_scale.max(1) as u32;
        (width * s, height * s, s as f32)
    }
}
/// xkb keycodes are evdev codes + 8 (X11 legacy offset).
const EVDEV_XKB_OFFSET: u32 = 8;
/// Shm buffers per lock surface (triple buffering while the compositor
/// holds the others).
const MAX_SHM_BUFFERS: usize = 3;
/// Bytes per pixel of the `ARGB8888` / `ABGR8888` shm buffers.
const BYTES_PER_PIXEL: u32 = 4;
/// Key-repeat defaults until the compositor sends `repeat_info`.
const DEFAULT_REPEAT_RATE_HZ: i32 = 25;
const DEFAULT_REPEAT_DELAY_MS: i32 = 600;

fn new_buffer(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, width: u32, height: u32, abgr: bool) -> Result<ShmBuffer> {
    let stride = width.checked_mul(BYTES_PER_PIXEL).context("lock buffer stride overflow")?;
    let size = u64::from(stride) * u64::from(height);
    let wire = |v: u64| i32::try_from(v).context("lock buffer exceeds the wl_shm size limit");
    let (size_i, width_i, height_i, stride_i) = (wire(size)?, wire(width.into())?, wire(height.into())?, wire(stride.into())?);
    let fd = rustix::fs::memfd_create("mandara-lock", rustix::fs::MemfdFlags::CLOEXEC).context("memfd_create")?;
    rustix::fs::ftruncate(&fd, size).context("ftruncate")?;
    let file = File::from(fd);
    // SAFETY: the memfd is private to this process and the compositor, which
    // only reads it; we never shrink it while mapped.
    let map = unsafe { memmap2::MmapMut::map_mut(&file) }.context("mmap")?;
    let pool = shm.create_pool(file.as_fd(), size_i, qh, ());
    let busy = Arc::new(AtomicBool::new(false));
    let format = if abgr { wl_shm::Format::Abgr8888 } else { wl_shm::Format::Argb8888 };
    let buffer = pool.create_buffer(0, width_i, height_i, stride_i, format, qh, busy.clone());
    pool.destroy();
    Ok(ShmBuffer { buffer, busy, map, _file: file, width, height })
}

/// When a repeating key fires after the one due at `due`: one period later,
/// but never in the past. A loop that stalled (a slow draw, a suspended
/// laptop) must not make up the repeats it missed in one burst: that would
/// type extra characters into the password.
fn next_repeat(due: Instant, now: Instant, rate_hz: i32) -> Instant {
    // Integer nanos avoid f32 rounding (1/25 ≠ 40 ms in float), which made
    // the catch-up test fail on aarch64 by a single nanosecond.
    let period = Duration::from_nanos(1_000_000_000 / u64::from(rate_hz.max(1) as u32));
    let next = due + period;
    if next > now { next } else { now + period }
}

/// Copies premultiplied RGBA into the shm buffer (`ARGB8888` is BGRA in
/// memory on little-endian; `ABGR8888` is RGBA).
fn copy_pixels(src: &[u8], dst: &mut [u8], abgr: bool) {
    if abgr {
        dst[..src.len()].copy_from_slice(src);
        return;
    }
    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
}

// ─────────────────────────────── the loop ───────────────────────────────

/// Locks (or previews) and runs until unlocked. In [`LockMode::Session`] an
/// error before the lock request went out means nothing was locked; after it,
/// [`LockError::locked`] says the session may be (or become) locked.
pub fn run(mode: LockMode, app: &mut dyn LockApp) -> Result<LockOutcome, LockError> {
    let (conn, mut queue, mut state, manager) = connect(mode).map_err(|error| LockError { error, locked: false })?;
    drive(&conn, &mut queue, &mut state, manager.as_ref(), app).map_err(|error| LockError { error, locked: state.lock.is_some() })
}

/// Connects, binds what the mode needs and reads the outputs and seats.
fn connect(
    mode: LockMode,
) -> Result<(Connection, wayland_client::EventQueue<State>, State, Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>)> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).context("reading Wayland globals")?;
    let qh = queue.handle();

    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).context("wl_compositor")?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
    let viewporter = globals.bind::<wp_viewporter::WpViewporter, _, _>(&qh, 1..=1, ()).ok();
    let fractional = globals.bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ()).ok();
    let (manager, layer_shell) = match mode {
        LockMode::Session => {
            let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
                globals.bind(&qh, 1..=1, ()).context("the compositor does not support ext-session-lock-v1")?;
            (Some(manager), None)
        }
        LockMode::Demo => {
            let shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).context("zwlr_layer_shell_v1")?;
            (None, Some(shell))
        }
    };

    let mut state = State {
        mode,
        qh: qh.clone(),
        compositor,
        shm,
        abgr: false,
        layer_shell,
        lock: None,
        viewporter,
        fractional,
        outputs: Vec::new(),
        seats: Vec::new(),
        kb: Keyboard { rate: DEFAULT_REPEAT_RATE_HZ, delay: DEFAULT_REPEAT_DELAY_MS, ..Default::default() },
        focus: None,
        phase: LockPhase::Pending,
        pending_keys: Vec::new(),
        keyboard_changed: false,
        locked_notice: false,
        closed: false,
        pointer_activity: false,
    };
    globals.contents().with_list(|list| {
        for g in list {
            match g.interface.as_str() {
                "wl_output" => {
                    let output = globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, g.name);
                    state.outputs.push(Output { global: g.name, output, name: String::new(), scale: 1, surface: None });
                }
                "wl_seat" => {
                    let seat = globals.registry().bind::<wl_seat::WlSeat, _, _>(g.name, g.version.min(7), &qh, ());
                    state.seats.push(Seat::new(g.name, seat));
                }
                _ => {}
            }
        }
    });
    // Output names / scales and shm formats before the first surface.
    queue.roundtrip(&mut state).context("Wayland roundtrip")?;
    Ok((conn, queue, state, manager))
}

/// Asks for the lock, paints and handles input until the lock is over.
fn drive(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    manager: Option<&ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    app: &mut dyn LockApp,
) -> Result<LockOutcome> {
    let (mode, qh) = (state.mode, state.qh.clone());
    if let Some(manager) = manager {
        state.lock = Some(manager.lock(&qh, ()));
    } else {
        // Demo: "locked" right away.
        state.phase = LockPhase::Locked;
        state.locked_notice = true;
    }
    for i in 0..state.outputs.len() {
        state.ensure_surface(i);
    }
    queue.roundtrip(state).context("Wayland roundtrip")?;

    let mut outcome = LockOutcome::Unlocked;
    loop {
        queue.dispatch_pending(state).context("Wayland dispatch")?;
        for i in 0..state.outputs.len() {
            state.ensure_surface(i);
        }
        if state.locked_notice {
            state.locked_notice = false;
            app.locked();
            // The app may paint something else now (the real look replaces
            // the plain backdrop shown while the lock was pending): repaint
            // every surface, not only the first the loop gets to.
            for out in &mut state.outputs {
                if let Some(s) = out.surface.as_mut() {
                    s.dirty = true;
                }
            }
        }
        if state.keyboard_changed {
            state.keyboard_changed = false;
            app.keyboard(&state.kb.shown);
        }
        let now = Instant::now();
        // Auto-repeat.
        if let Some((code, at)) = state.kb.repeat
            && now >= at
            && state.kb.rate > 0
        {
            state.kb.repeat = Some((code, next_repeat(at, now, state.kb.rate)));
            state.handle_key(code);
        }
        for key in std::mem::take(&mut state.pending_keys) {
            app.key(key);
        }
        if std::mem::take(&mut state.pointer_activity) {
            app.pointer_activity();
        }

        match app.update(now) {
            AppControl::Continue => {}
            AppControl::Unlock => match mode {
                LockMode::Session => state.apply_lock_event(LockEvent::AuthSucceeded),
                LockMode::Demo => state.phase = LockPhase::Unlocked,
            },
            AppControl::Exit => match mode {
                LockMode::Session => state.apply_lock_event(LockEvent::ExitRequested),
                LockMode::Demo => state.phase = LockPhase::Unlocked,
            },
        }
        if state.closed && mode == LockMode::Demo {
            state.phase = LockPhase::Unlocked;
        }
        match state.phase {
            LockPhase::Denied => {
                let _ = conn.flush();
                bail!("the compositor refused to lock the session (is another screen locker running?)");
            }
            LockPhase::Ended => {
                outcome = LockOutcome::EndedByCompositor;
                break;
            }
            LockPhase::Unlocked => break,
            _ => {}
        }

        // Paint.
        let redraw = app.needs_redraw();
        let focus = state.focus.or_else(|| state.outputs.first().map(|o| o.global));
        let (shm, abgr) = (state.shm.clone(), state.abgr);
        let mut painted = false;
        for out in &mut state.outputs {
            let focused = Some(out.global) == focus;
            let name = out.name.clone();
            let Some(s) = out.surface.as_mut() else { continue };
            if !s.configured || s.frame_pending || !(redraw || s.dirty) || s.width == 0 || s.height == 0 {
                continue;
            }
            let (pw, ph, scale) = s.physical_size();
            s.buffers.retain(|b| b.width == pw && b.height == ph || b.busy.load(Ordering::Acquire));
            let slot = match s.buffers.iter().position(|b| !b.busy.load(Ordering::Acquire) && b.width == pw && b.height == ph) {
                Some(i) => i,
                None if s.buffers.len() < MAX_SHM_BUFFERS => {
                    s.buffers.push(new_buffer(&shm, &qh, pw, ph, abgr)?);
                    s.buffers.len() - 1
                }
                None => continue,
            };
            // Capture scale/destination before the scratch/buffer borrows.
            let attach_scale = s.attach_buffer_scale();
            let dest = s.uses_fractional_scale().then_some((s.width as i32, s.height as i32));
            let info = SurfaceInfo { output: name, width: s.width, height: s.height, scale, focused };
            let scratch = match &mut s.scratch {
                Some(p) if p.width() == pw && p.height() == ph => p,
                slot => slot.insert(tiny_skia::Pixmap::new(pw, ph).context("allocating lock surface pixmap")?),
            };
            app.draw(&info, &mut scratch.as_mut(), now);
            let buf = &mut s.buffers[slot];
            copy_pixels(scratch.data(), &mut buf.map, abgr);
            buf.busy.store(true, Ordering::Release);
            // Always re-assert buffer scale: a paint before `PreferredScale`
            // may have set the integer factor; fractional mode must reset to 1.
            s.surface.set_buffer_scale(attach_scale);
            if let (Some(vp), Some((w, h))) = (s.viewport.as_ref(), dest) {
                vp.set_destination(w, h);
            }
            s.surface.attach(Some(&buf.buffer), 0, 0);
            s.surface.damage_buffer(0, 0, pw as i32, ph as i32);
            s.surface.frame(&qh, out.global);
            s.frame_pending = true;
            s.surface.commit();
            s.dirty = false;
            painted = true;
        }
        if painted {
            app.drawn();
        }
        conn.flush().context("Wayland flush")?;

        // Sleep until Wayland input, a repeat, or the app's next wakeup.
        let mut timeout = app.next_wakeup(Instant::now());
        if let Some((_, at)) = state.kb.repeat {
            timeout = timeout.min(at.saturating_duration_since(Instant::now()));
        }
        if let Some(guard) = queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
            let ts = rustix::event::Timespec { tv_sec: timeout.as_secs() as i64, tv_nsec: timeout.subsec_nanos() as i64 };
            match rustix::event::poll(&mut fds, Some(&ts)) {
                Ok(n) if n > 0 => {
                    if let Err(err) = guard.read() {
                        // WouldBlock is fine; anything else is fatal.
                        if !matches!(err, wayland_client::backend::WaylandError::Io(ref e) if e.kind() == std::io::ErrorKind::WouldBlock) {
                            return Err(err).context("reading Wayland events");
                        }
                    }
                }
                Ok(_) => drop(guard),
                Err(rustix::io::Errno::INTR) => drop(guard),
                Err(err) => return Err(err).context("poll"),
            }
        }
    }

    // Tear down (the lock object is already gone).
    for out in &mut state.outputs {
        if let Some(s) = out.surface.take() {
            s.destroy();
        }
    }
    let _ = queue.roundtrip(state);
    Ok(outcome)
}

// ─────────────────────────────── dispatch ───────────────────────────────

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let output = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, name);
                state.outputs.push(Output { global: name, output, name: String::new(), scale: 1, surface: None });
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_seat" => {
                state.seats.push(Seat::new(name, registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), qh, ())));
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.seats.iter().position(|s| s.global == name) {
                    // The held key can no longer be released: stop repeating it.
                    if state.seats.remove(i).release() {
                        state.kb.repeat = None;
                    }
                }
                if let Some(i) = state.outputs.iter().position(|o| o.global == name) {
                    let mut out = state.outputs.remove(i);
                    if let Some(s) = out.surface.take() {
                        s.destroy();
                    }
                    if out.output.version() >= 3 {
                        out.output.release();
                    }
                    if state.focus == Some(name) {
                        state.focus = None;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(out) = state.output_mut(*global) else { return };
        match event {
            wl_output::Event::Name { name } => out.name = name,
            wl_output::Event::Scale { factor } => {
                out.scale = factor;
                if let Some(s) = out.surface.as_mut()
                    && s.fractional.is_none()
                    && s.surface.version() < 6
                {
                    s.buffer_scale = factor;
                    s.dirty = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(state: &mut Self, _: &wl_shm::WlShm, event: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_shm::Event::Format { format: WEnum::Value(wl_shm::Format::Abgr8888) } = event {
            state.abgr = true;
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for State {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, event: wl_buffer::Event, busy: &Arc<AtomicBool>, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            busy.store(false, Ordering::Release);
        }
    }
}

impl Dispatch<wl_surface::WlSurface, u32> for State {
    fn event(state: &mut Self, _: &wl_surface::WlSurface, event: wl_surface::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event
            && let Some(s) = state.surface_mut(*global)
            // Fractional mode ignores `buffer_scale` once `scale120 > 0`; until
            // then the integer preference is the attach fallback.
            && s.buffer_scale != factor
        {
            s.buffer_scale = factor;
            s.dirty = true;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, u32> for State {
    fn event(state: &mut Self, _: &wl_callback::WlCallback, event: wl_callback::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(s) = state.surface_mut(*global)
        {
            s.frame_pending = false;
        }
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for State {
    fn event(state: &mut Self, _: &ext_session_lock_v1::ExtSessionLockV1, event: ext_session_lock_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_session_lock_v1::Event::Locked => state.apply_lock_event(LockEvent::Locked),
            ext_session_lock_v1::Event::Finished => state.apply_lock_event(LockEvent::Finished),
            _ => {}
        }
    }
}

impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, u32> for State {
    fn event(
        state: &mut Self,
        lock_surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, width, height } = event {
            lock_surface.ack_configure(serial);
            if let Some(s) = state.surface_mut(*global) {
                s.width = width;
                s.height = height;
                s.configured = true;
                s.dirty = true;
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, u32> for State {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                if let Some(s) = state.surface_mut(*global) {
                    s.width = width;
                    s.height = height;
                    s.configured = true;
                    s.dirty = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, u32> for State {
    fn event(state: &mut Self, _: &wp_fractional_scale_v1::WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(s) = state.surface_mut(*global)
            && s.scale120 != scale
        {
            s.scale120 = scale;
            s.dirty = true;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event
            && let Some(entry) = state.seats.iter_mut().find(|s| s.seat == *seat)
        {
            if !caps.contains(wl_seat::Capability::Keyboard) && entry.release_keyboard() {
                state.kb.repeat = None;
            } else if caps.contains(wl_seat::Capability::Keyboard) && entry.keyboard.is_none() {
                entry.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if !caps.contains(wl_seat::Capability::Pointer) {
                entry.release_pointer();
            } else if entry.pointer.is_none() {
                entry.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap { format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                let context = state.kb.context.get_or_insert_with(|| xkb::Context::new(xkb::CONTEXT_NO_FLAGS));
                // SAFETY: the compositor hands us a readable keymap fd of `size` bytes.
                match unsafe { xkb::Keymap::new_from_fd(context, fd, size as usize, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS) } {
                    Ok(Some(keymap)) => {
                        state.kb.state = Some(xkb::State::new(&keymap));
                        state.kb.keymap = Some(keymap);
                        state.refresh_keyboard_state();
                    }
                    Ok(None) => tracing::warn!("lock: the compositor sent an unusable keymap"),
                    Err(err) => tracing::warn!("lock: mapping the keymap failed: {err}"),
                }
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                state.focus = surface.data::<u32>().copied();
            }
            wl_keyboard::Event::Leave { .. } => state.kb.repeat = None,
            wl_keyboard::Event::Key { key, state: WEnum::Value(key_state), .. } => match key_state {
                wl_keyboard::KeyState::Pressed => {
                    state.handle_key(key);
                    let repeats = state.kb.keymap.as_ref().is_some_and(|k| k.key_repeats(xkb::Keycode::new(key + EVDEV_XKB_OFFSET)));
                    state.kb.repeat = (repeats && state.kb.rate > 0).then(|| (key, Instant::now() + Duration::from_millis(state.kb.delay.max(0) as u64)));
                }
                _ => {
                    if state.kb.repeat.is_some_and(|(k, _)| k == key) {
                        state.kb.repeat = None;
                    }
                }
            },
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(s) = state.kb.state.as_mut() {
                    s.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
                state.refresh_keyboard_state();
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.kb.rate = rate;
                state.kb.delay = delay;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut Self, pointer: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            // Hide the cursor over the lock screen.
            wl_pointer::Event::Enter { serial, .. } => pointer.set_cursor(serial, None, 0, 0),
            wl_pointer::Event::Motion { .. } | wl_pointer::Event::Button { .. } => state.pointer_activity = true,
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($ty:ty),* $(,)?) => {$(
        impl Dispatch<$ty, ()> for State {
            fn event(_: &mut Self, _: &$ty, _: <$ty as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm_pool::WlShmPool,
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
);

#[cfg(test)]
mod tests {
    use super::*;
    use LockAction as A;
    use LockEvent as E;
    use LockPhase as P;

    fn run_events(events: &[LockEvent]) -> (LockPhase, Vec<LockAction>) {
        let mut phase = P::Pending;
        let mut actions = Vec::new();
        for e in events {
            let (next, action) = phase.on(*e);
            phase = next;
            actions.push(action);
        }
        (phase, actions)
    }

    #[test]
    fn deferred_keyboard_loading_preserves_state_and_compose() {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(&context, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap();
        let mut state = xkb::State::new(&keymap);
        const EVDEV_A: u32 = 30;
        let key = xkb::Keycode::new(EVDEV_A + EVDEV_XKB_OFFSET);
        assert_eq!(state.key_get_utf8(key), "a");
        let caps = keymap.mod_get_index(xkb::MOD_NAME_CAPS);
        state.update_mask(0, 0, 1 << caps, 0, 0, 0);
        assert_eq!(state.key_get_utf8(key), "A");

        let table = xkb::compose::Table::new_from_buffer(&context,
            "<dead_acute> <e> : \"é\" U00E9\n", "C", xkb::compose::FORMAT_TEXT_V1, xkb::compose::COMPILE_NO_FLAGS).unwrap();
        let mut compose = xkb::compose::State::new(&table, xkb::compose::STATE_NO_FLAGS);
        for key in ["dead_acute", "e"] { compose.feed(xkb::keysym_from_name(key, xkb::KEYSYM_NO_FLAGS)); }
        assert_eq!(compose.status(), xkb::compose::Status::Composed);
        assert_eq!(compose.utf8().as_deref(), Some("é"));
    }

    #[test]
    fn normal_lock_then_unlock() {
        let (phase, actions) = run_events(&[E::Locked, E::AuthSucceeded]);
        assert_eq!(phase, P::Unlocked);
        assert_eq!(actions, vec![A::None, A::UnlockAndDestroy]);
        assert!(phase.is_terminal());
    }

    #[test]
    fn refused_lock_destroys_without_unlock() {
        let (phase, actions) = run_events(&[E::Finished]);
        assert_eq!(phase, P::Denied);
        assert_eq!(actions, vec![A::Destroy]);
    }

    #[test]
    fn early_success_waits_for_locked_before_unlocking() {
        // unlock_and_destroy before `locked` is a protocol error.
        let (phase, actions) = run_events(&[E::AuthSucceeded, E::Locked]);
        assert_eq!(phase, P::Unlocked);
        assert_eq!(actions, vec![A::None, A::UnlockAndDestroy]);
        let (phase, actions) = run_events(&[E::AuthSucceeded, E::Finished]);
        assert_eq!(phase, P::Denied);
        assert_eq!(actions, vec![A::None, A::Destroy]);
    }

    #[test]
    fn exit_requests_never_leave_a_lock() {
        for prefix in [&[][..], &[E::Locked][..], &[E::AuthSucceeded][..]] {
            let mut events = prefix.to_vec();
            events.push(E::ExitRequested);
            let (phase, actions) = run_events(&events);
            assert!(!phase.is_terminal(), "{prefix:?} + exit must stay locked");
            assert_eq!(*actions.last().unwrap(), A::None);
        }
    }

    #[test]
    fn compositor_ending_a_confirmed_lock_destroys_plainly() {
        let (phase, actions) = run_events(&[E::Locked, E::Finished]);
        assert_eq!(phase, P::Ended);
        assert_eq!(actions, vec![A::None, A::Destroy]);
        // Terminal phases ignore everything afterwards.
        assert_eq!(P::Ended.on(E::AuthSucceeded), (P::Ended, A::None));
        assert_eq!(P::Unlocked.on(E::Finished), (P::Unlocked, A::None));
    }

    #[test]
    fn key_repeat_never_catches_up_after_a_stall() {
        let start = Instant::now();
        let period = Duration::from_millis(40);
        // On time: the next repeat follows one period after the one due.
        assert_eq!(next_repeat(start, start + Duration::from_millis(1), 25), start + period);
        // A second-long stall: the next repeat is a period from now, not a
        // burst of the 25 repeats that were due meanwhile.
        let late = start + Duration::from_secs(1);
        let next = next_repeat(start, late, 25);
        assert!(next > late && next <= late + period, "next repeat {:?} after the stall", next - late);
    }

    #[test]
    fn pixel_copy_swizzles_for_argb() {
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut dst = [0u8; 8];
        copy_pixels(&src, &mut dst, false);
        assert_eq!(dst, [3, 2, 1, 4, 7, 6, 5, 8]);
        copy_pixels(&src, &mut dst, true);
        assert_eq!(dst, src);
    }

    /// A surface that already took an integer buffer scale (e.g. from an early
    /// paint) must attach at scale 1 once fractional scale arrives — otherwise
    /// 2559×1599 under scale 2 is a protocol error.
    #[test]
    fn fractional_attach_scale_is_one_even_after_integer_preference() {
        const W: u32 = 1706;
        const H: u32 = 1066;
        const SCALE120: u32 = 180; // 1.5×
        assert!(uses_fractional_scale(true, SCALE120));
        assert_eq!(attach_buffer_scale(true, SCALE120, 2), 1);
        let (pw, ph, scale) = physical_size_of(W, H, true, SCALE120, 2);
        assert_eq!((pw, ph), (2559, 1599));
        assert!((scale - 1.5).abs() < 1e-6);
        assert_eq!(pw % attach_buffer_scale(true, SCALE120, 2) as u32, 0);
        assert_eq!(ph % attach_buffer_scale(true, SCALE120, 2) as u32, 0);
        // Without a preferred fractional scale yet, keep the integer factor.
        assert!(!uses_fractional_scale(true, 0));
        assert_eq!(attach_buffer_scale(true, 0, 2), 2);
        assert_eq!(physical_size_of(W, H, true, 0, 2), (W * 2, H * 2, 2.0));
    }
}
