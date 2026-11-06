//! Software-rendered layer surfaces (`wl_shm`) made of stacked *planes*.
//!
//! The desktop background does not need a GPU renderer: it changes once a
//! minute (the clock) or when the wallpaper changes, and a GPUI window costs
//! four full-size swapchain images (~131 MiB at 3840 × 2160, Mesa's Wayland
//! WSI forces `minImageCount = 4`). This module owns its own Wayland
//! connection — the approach the lock screen ([`crate::session_lock`])
//! proved — and gives every output one `zwlr_layer_surface_v1` *canvas*:
//!
//! * the root surface is a 1 × 1 shared-memory pixel stretched over the
//!   output by `wp_viewporter` (the fill colour, or transparent);
//! * each *plane* is a synchronized `wl_subsurface` with its own shm buffer,
//!   its own position and size (only the pixels it covers), an optional
//!   `wp_alpha_modifier_v1` multiplier (crossfades done by the compositor at
//!   zero CPU cost) and an optional viewport crop (wipe / grow reveals, or a
//!   low-resolution buffer the compositor scales up);
//! * stacking order is explicit ([`Host::set_order`]).
//!
//! Changes are staged per plane and applied atomically by [`Host::commit`],
//! which commits every touched plane (cached: they are synchronized) and
//! then the root. Static planes drop their buffers as soon as the
//! compositor releases them: a compositor that uploads shm contents into a
//! texture (wlroots does) keeps showing them, and the shell keeps no copy.
//!
//! On the background layer the root surface takes pointer input (planes
//! never do): the host binds the seats' pointers, keeps the surface-local
//! position and queues button presses for the app ([`Host::take_presses`]),
//! so the desktop can open its menu on a right-click. Motion only updates
//! the stored position: [`Host::wait`] reports wake-ups caused by pointer
//! input alone, which the app sleeps through without redrawing.
//!
//! Plane geometry is given in *physical* pixels of the output. Subsurface
//! positions and viewport destinations are logical integers, so rectangles
//! must lie on the grid where both are whole numbers ([`snap`]): at 1.5×
//! that is every 3 physical / 2 logical pixels. Buffers then map 1:1 onto
//! output pixels and text stays crisp at fractional scales
//! (`wp_fractional_scale_v1`).

use std::fs::File;
use std::os::fd::{AsFd as _, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_subcompositor,
    wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::alpha_modifier::v1::client::{wp_alpha_modifier_surface_v1, wp_alpha_modifier_v1};
use wayland_protocols::wp::cursor_shape::v1::client::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1};
use wayland_protocols::wp::fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// `wp_fractional_scale_v1` reports scales as multiples of 1/120.
pub const SCALE_DENOMINATOR: u32 = 120;
/// Bytes per pixel of the `ABGR8888` / `ARGB8888` buffers.
const BYTES_PER_PIXEL: u32 = 4;
/// Most buffers a plane keeps in flight (the compositor holds one while the
/// next is drawn).
const MAX_BUFFERS_PER_PLANE: usize = 3;
/// A frame callback that has not arrived after this long is given up on:
/// a fully covered background gets none (the compositor skips it).
pub const FRAME_TIMEOUT: Duration = Duration::from_millis(100);
/// `wp_alpha_modifier_surface_v1` multiplier for full opacity.
const ALPHA_ONE: f32 = u32::MAX as f32;
/// Newest `wl_seat` version used (pointer frames, seat names).
const SEAT_VERSION: u32 = 7;
/// Most button presses queued between two [`Host::take_presses`] calls
/// (a stuck app must not grow the queue without bound).
const MAX_PRESSES: usize = 16;
/// Pseudo button codes of [`Press`] for a scroll step up / down (beyond
/// every Linux input code).
pub const SCROLL_UP: u32 = 0x1_0000;
pub const SCROLL_DOWN: u32 = 0x1_0001;
/// Scroll distance (surface px, as `wl_pointer.axis` reports it) that
/// makes one step: one wheel notch on common compositors.
const SCROLL_STEP: f64 = 10.0;

/// Which layer the canvases live on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CanvasLayer {
    Background,
    /// For screenshots / previews above all windows.
    Overlay,
}

/// A rectangle in physical (buffer) pixels of an output.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// From float edges, rounded outward.
    pub fn from_edges(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        let (l, t) = (x0.floor() as i32, y0.floor() as i32);
        let (r, b) = (x1.ceil() as i32, y1.ceil() as i32);
        Self { x: l, y: t, w: (r - l).max(0) as u32, h: (b - t).max(0) as u32 }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect { x, y, w: (self.right().max(o.right()) - x) as u32, h: (self.bottom().max(o.bottom()) - y) as u32 }
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        let (r, b) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        if r <= x || b <= y { Rect::default() } else { Rect { x, y, w: (r - x) as u32, h: (b - y) as u32 } }
    }
}

/// Greatest common divisor.
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

/// Physical-pixel grid step at which logical coordinates are whole numbers
/// for a scale of `scale120 / 120` (3 at 1.5×, 5 at 1.25×, 1 at 1× / 2×).
pub fn grid_step(scale120: u32) -> u32 {
    let s = scale120.max(1);
    s / gcd(s, SCALE_DENOMINATOR)
}

/// Expands `r` outward onto the [`grid_step`] grid.
pub fn snap(r: Rect, scale120: u32) -> Rect {
    let g = grid_step(scale120) as i32;
    let down = |v: i32| v.div_euclid(g) * g;
    let up = |v: i32| (v + g - 1).div_euclid(g) * g;
    let (x0, y0, x1, y1) = (down(r.x), down(r.y), up(r.right()), up(r.bottom()));
    Rect { x: x0, y: y0, w: (x1 - x0).max(0) as u32, h: (y1 - y0).max(0) as u32 }
}

/// Where a plane sits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The whole output, buffer at physical size.
    Full,
    /// The whole output at its exact logical size, scaled from this buffer.
    /// Needs `wp_viewporter`; avoids rounding the output through physical pixels.
    FullScaled { buffer: (u32, u32) },
    /// A [`snap`]ped rectangle, buffer at its physical size.
    Rect(Rect),
    /// A [`snap`]ped rectangle covered by a smaller buffer the compositor
    /// scales up (soft content such as glows). Needs `wp_viewporter`.
    Scaled { rect: Rect, buffer: (u32, u32) },
}

impl Place {
    /// Destination in logical surface coordinates. Full placements use the
    /// advertised logical output directly, avoiding a fractional-scale round trip.
    fn logical_rect(self, output: (u32, u32), scale120: u32) -> Rect {
        let to_logical = |v: i32| (v as i64 * SCALE_DENOMINATOR as i64 / scale120.max(1) as i64) as i32;
        match self {
            Self::Full | Self::FullScaled { .. } => Rect::new(0, 0, output.0.max(1), output.1.max(1)),
            Self::Rect(rect) | Self::Scaled { rect, .. } => Rect::new(to_logical(rect.x), to_logical(rect.y), to_logical(rect.w as i32).max(1) as u32, to_logical(rect.h as i32).max(1) as u32),
        }
    }
}

/// A viewport override for reveal animations: show the buffer region
/// `source` (buffer pixels) at logical rectangle `dest` (`x, y, w, h`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct View {
    pub source: (f64, f64, f64, f64),
    pub dest: (i32, i32, i32, i32),
}

/// What the app learns about an output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    pub id: u32,
    /// Connector (`DP-1`).
    pub name: String,
    /// `make model` from `wl_output.geometry`.
    pub identity: String,
    pub description: String,
    /// Canvas size once configured: logical and physical, and the scale ×120.
    pub logical: Option<(u32, u32)>,
    pub physical: Option<(u32, u32)>,
    pub scale120: u32,
    pub has_canvas: bool,
}

/// A pointer button pressed on a canvas: the output (its `wl_output`
/// global, as in [`OutputInfo::id`]), the surface-local position in
/// logical pixels and the Linux input code (`BTN_RIGHT` = `0x111`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Press {
    pub output: u32,
    pub x: f64,
    pub y: f64,
    pub button: u32,
}

/// One seat's pointer and its cursor-shape device.
struct Seat {
    seat: wl_seat::WlSeat,
    pointer: Option<wl_pointer::WlPointer>,
    shape: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
}

impl Seat {
    fn release_pointer(&mut self) {
        if let Some(d) = self.shape.take() {
            d.destroy();
        }
        if let Some(p) = self.pointer.take()
            && p.version() >= 3
        {
            p.release();
        }
    }
}

/// Wakes [`Host::wait`] from another thread (an `eventfd`).
pub struct Waker(OwnedFd);

impl Waker {
    pub fn new() -> Result<Self> {
        use rustix::event::{EventfdFlags, eventfd};
        Ok(Self(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).context("eventfd")?))
    }

    pub fn wake(&self) {
        let _ = rustix::io::write(&self.0, &1u64.to_ne_bytes());
    }

    fn drain(&self) {
        let mut buf = [0u8; 8];
        let _ = rustix::io::read(&self.0, &mut buf);
    }
}

// ───────────────────────────── wayland state ─────────────────────────────

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

fn new_buffer(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, width: u32, height: u32, abgr: bool) -> Result<ShmBuffer> {
    let stride = width.checked_mul(BYTES_PER_PIXEL).context("buffer stride overflow")?;
    let size = u64::from(stride) * u64::from(height);
    let wire = |v: u64| i32::try_from(v).context("buffer exceeds the wl_shm size limit");
    let (size_i, width_i, height_i, stride_i) = (wire(size)?, wire(width.into())?, wire(height.into())?, wire(stride.into())?);
    let fd = rustix::fs::memfd_create("mandara-canvas", rustix::fs::MemfdFlags::CLOEXEC).context("memfd_create")?;
    rustix::fs::ftruncate(&fd, size).context("ftruncate")?;
    let file = File::from(fd);
    // SAFETY: the memfd is private to this process and the compositor, which
    // only reads it; it is never shrunk while mapped.
    let map = unsafe { memmap2::MmapMut::map_mut(&file) }.context("mmap")?;
    let pool = shm.create_pool(file.as_fd(), size_i, qh, ());
    let busy = Arc::new(AtomicBool::new(false));
    let format = if abgr { wl_shm::Format::Abgr8888 } else { wl_shm::Format::Argb8888 };
    let buffer = pool.create_buffer(0, width_i, height_i, stride_i, format, qh, busy.clone());
    pool.destroy();
    Ok(ShmBuffer { buffer, busy, map, _file: file, width, height })
}

/// Premultiplied RGBA → `ARGB8888` (BGRA in memory on little-endian).
fn swizzle_into(src: &[u8], dst: &mut [u8]) {
    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
}

#[derive(Default)]
struct Plane {
    surface: Option<wl_surface::WlSurface>,
    sub: Option<wl_subsurface::WlSubsurface>,
    viewport: Option<wp_viewport::WpViewport>,
    alpha: Option<wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1>,
    buffers: Vec<ShmBuffer>,
    /// Keep released buffers for reuse (planes redrawn often).
    keep: bool,
    mapped: bool,
    place: Option<Place>,
    view: Option<View>,
    alpha_value: f32,
    /// Has uncommitted state.
    dirty: bool,
}

impl Plane {
    fn destroy(&mut self) {
        if let Some(a) = self.alpha.take() {
            a.destroy();
        }
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        if let Some(s) = self.sub.take() {
            s.destroy();
        }
        if let Some(s) = self.surface.take() {
            s.destroy();
        }
        self.buffers.clear();
        self.mapped = false;
    }
}

struct Canvas {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    width: u32,
    height: u32,
    /// Last requested logical size, independent of outstanding configure replies.
    requested: (u32, u32),
    configured: bool,
    scale120: u32,
    buffer_scale: i32,
    root: Option<ShmBuffer>,
    root_rgba: Option<[u8; 4]>,
    root_dirty: bool,
    planes: Vec<Plane>,
    order: Vec<usize>,
    order_dirty: bool,
    frame_pending: Option<Instant>,
    /// A frame callback was requested at this instant and hasn't fired
    /// (unlike `frame_pending`, never given up on: a compositor withholds
    /// callbacks from a surface nobody can see).
    awaiting_since: Option<Instant>,
}

impl Canvas {
    /// Scale ×120 in effect (fractional when available, else integer).
    fn scale120(&self) -> u32 {
        if self.viewport.is_some() && self.scale120 > 0 { self.scale120 } else { self.buffer_scale.max(1) as u32 * SCALE_DENOMINATOR }
    }

    /// Buffer dimensions rounded from logical size at the effective scale,
    /// with at least one pixel per axis.
    fn physical(&self) -> (u32, u32) {
        let s = self.scale120();
        let round = |v: u32| ((v * s + SCALE_DENOMINATOR / 2) / SCALE_DENOMINATOR).max(1);
        (round(self.width), round(self.height))
    }

    fn destroy(mut self) {
        for p in &mut self.planes {
            p.destroy();
        }
        if let Some(f) = &self.fractional {
            f.destroy();
        }
        if let Some(v) = &self.viewport {
            v.destroy();
        }
        self.layer.destroy();
        self.root = None;
        self.surface.destroy();
    }
}

struct Output {
    global: u32,
    output: wl_output::WlOutput,
    name: String,
    description: String,
    identity: String,
    legacy: LegacyOutputSize,
    xdg_output: Option<zxdg_output_v1::ZxdgOutputV1>,
    logical: Option<(u32, u32)>,
    canvas: Option<Canvas>,
}

struct LegacyOutputSize {
    mode: Option<(u32, u32)>,
    scale: i32,
    complete: bool,
}

impl Default for LegacyOutputSize {
    fn default() -> Self { Self { mode: None, scale: 1, complete: false } }
}

impl LegacyOutputSize {
    /// Version two batches mode and scale together; version one has no Done.
    /// Returns true when consumers may act on the accumulated geometry.
    /// Until then, no intermediate mode/scale pair may size a legacy canvas.
    fn event(&mut self, event: &wl_output::Event, version: u32) -> bool {
        match event {
            wl_output::Event::Mode { flags: WEnum::Value(flags), width, height, .. } if flags.contains(wl_output::Mode::Current) => {
                let Some(size) = changed_logical_size(None, *width, *height) else { return false; };
                self.mode = Some(size);
                self.complete = version < 2;
            }
            wl_output::Event::Scale { factor } => {
                self.scale = (*factor).max(1);
                self.complete = version < 2;
            }
            wl_output::Event::Done => self.complete = true,
            _ => return false,
        }
        self.complete
    }

    /// Logical fallback only after a complete advertisement. Existing canvases
    /// keep painting their configured dimensions while the next batch is pending.
    fn logical_size(&self) -> Option<(u32, u32)> {
        if !self.complete { return None; }
        self.mode.map(|(width, height)| ((width / self.scale.max(1) as u32).max(1), (height / self.scale.max(1) as u32).max(1)))
    }
}

/// Invalid or repeated advertisements never request a zero-sized surface.
fn changed_logical_size(current: Option<(u32, u32)>, width: i32, height: i32) -> Option<(u32, u32)> {
    let size = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    (size.0 > 0 && size.1 > 0 && current != Some(size)).then_some(size)
}

/// Records a positive, changed request before sending it. Compare requests,
/// not configure replies: a newer target may equal the old painted size
/// while an intermediate request is still waiting for acknowledgement.
fn request_logical_size(requested: &mut (u32, u32), size: (u32, u32)) -> bool {
    if size.0 == 0 || size.1 == 0 || *requested == size { return false; }
    *requested = size;
    true
}

impl Output {
    /// Prefers the compositor's logical dimensions and waits for them when
    /// xdg-output is available. Mode divided by integer scale is a fallback
    /// only for compositors without that protocol.
    fn logical_size(&self) -> Option<(u32, u32)> {
        if self.xdg_output.is_some() { return self.logical; }
        self.legacy.logical_size()
    }

    /// Requests changed logical dimensions without treating the request as a
    /// configure acknowledgement; buffer sizing follows the compositor's reply.
    fn resize_canvas(&mut self) {
        if let Some((width, height)) = self.logical_size()
            && let Some(canvas) = &mut self.canvas
            && request_logical_size(&mut canvas.requested, (width, height))
        {
            canvas.layer.set_size(width, height);
            canvas.surface.commit();
        }
    }
}

struct State {
    qh: QueueHandle<State>,
    compositor: wl_compositor::WlCompositor,
    subcompositor: wl_subcompositor::WlSubcompositor,
    shm: wl_shm::WlShm,
    abgr: bool,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    alpha: Option<wp_alpha_modifier_v1::WpAlphaModifierV1>,
    xdg_output: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: Vec<Output>,
    layer: CanvasLayer,
    namespace: String,
    changed: bool,
    /// Canvases take pointer input (background layer only).
    input: bool,
    seats: Vec<Seat>,
    cursor_shape: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    /// The canvas under the pointer (its output global) and the position.
    pointer: Option<(u32, f64, f64)>,
    presses: Vec<Press>,
    /// Vertical scroll not yet turned into steps.
    scroll: f64,
    /// Pointer events dispatched since [`Host::wait`] started (a wake-up
    /// with nothing else is input only).
    input_events: usize,
}

impl State {
    fn add_output(&mut self, output: wl_output::WlOutput, global: u32) {
        let xdg_output = self.xdg_output.as_ref().map(|manager| manager.get_xdg_output(&output, &self.qh, global));
        self.outputs.push(Output { global, output, name: String::new(), description: String::new(), identity: String::new(), legacy: LegacyOutputSize::default(), xdg_output, logical: None, canvas: None });
    }

    fn canvas_mut(&mut self, global: u32) -> Option<&mut Canvas> {
        self.outputs.iter_mut().find(|o| o.global == global).and_then(|o| o.canvas.as_mut())
    }

    fn empty_region(&self) -> wl_region::WlRegion {
        self.compositor.create_region(&self.qh, ())
    }
}

/// One Wayland connection hosting a canvas per output.
pub struct Host {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl Host {
    /// Connects and binds the globals (`wl_subcompositor`, `wl_shm`,
    /// layer-shell are required; viewporter, fractional scale and alpha
    /// modifier are used when present).
    pub fn connect(layer: CanvasLayer, namespace: &str) -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).context("reading Wayland globals")?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).context("wl_compositor")?;
        let subcompositor: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ()).context("wl_subcompositor")?;
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
        let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).context("zwlr_layer_shell_v1")?;
        let viewporter = globals.bind::<wp_viewporter::WpViewporter, _, _>(&qh, 1..=1, ()).ok();
        let fractional = globals.bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let alpha = globals.bind::<wp_alpha_modifier_v1::WpAlphaModifierV1, _, _>(&qh, 1..=1, ()).ok();
        let xdg_output = globals.bind::<zxdg_output_manager_v1::ZxdgOutputManagerV1, _, _>(&qh, 1..=3, ()).ok();
        // The desktop takes pointer input (its menu); an overlay preview
        // never swallows clicks.
        let input = layer == CanvasLayer::Background;
        let cursor_shape = if input { globals.bind::<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, _, _>(&qh, 1..=1, ()).ok() } else { None };
        let mut state = State {
            qh: qh.clone(),
            compositor,
            subcompositor,
            shm,
            abgr: false,
            layer_shell,
            viewporter,
            fractional,
            alpha,
            xdg_output,
            outputs: Vec::new(),
            layer,
            namespace: namespace.to_string(),
            changed: true,
            input,
            seats: Vec::new(),
            cursor_shape,
            pointer: None,
            scroll: 0.0,
            presses: Vec::new(),
            input_events: 0,
        };
        globals.contents().with_list(|list| {
            for g in list.iter().filter(|g| g.interface == "wl_output") {
                let output = globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, g.name);
                state.add_output(output, g.name);
            }
            if input {
                for g in list.iter().filter(|g| g.interface == "wl_seat") {
                    let seat = globals.registry().bind::<wl_seat::WlSeat, _, _>(g.name, g.version.min(SEAT_VERSION), &qh, g.name);
                    state.seats.push(Seat { seat, pointer: None, shape: None });
                }
            }
        });
        queue.roundtrip(&mut state).context("Wayland roundtrip")?;
        Ok(Self { conn, queue, state })
    }

    /// `wp_alpha_modifier_v1` is available (compositor-side fades).
    pub fn supports_alpha(&self) -> bool {
        self.state.alpha.is_some()
    }

    /// `wp_viewporter` is available (crops, scaled planes, fractional scale).
    pub fn supports_viewport(&self) -> bool {
        self.state.viewporter.is_some()
    }

    /// Button presses on the canvases since the last call, oldest first.
    pub fn take_presses(&mut self) -> Vec<Press> {
        std::mem::take(&mut self.state.presses)
    }

    /// Outputs or canvas geometry changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.state.changed)
    }

    /// Snapshots known outputs. Geometry stays `None` until the canvas is
    /// configured and its preferred fractional scale, if requested, has arrived.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.state
            .outputs
            .iter()
            .map(|o| {
                // Not ready until the preferred fractional scale is known
                // (else the first paint would be at the wrong size).
                let c = o.canvas.as_ref().filter(|c| c.configured && c.width > 0 && c.height > 0 && (c.fractional.is_none() || c.scale120 > 0));
                OutputInfo {
                    id: o.global,
                    name: o.name.clone(),
                    identity: o.identity.clone(),
                    description: o.description.clone(),
                    logical: c.map(|c| (c.width, c.height)),
                    physical: c.map(Canvas::physical),
                    scale120: c.map(Canvas::scale120).unwrap_or(SCALE_DENOMINATOR),
                    has_canvas: o.canvas.is_some(),
                }
            })
            .collect()
    }

    /// Creates the canvas (layer surface) for output `id` if it has none.
    /// Waits for native logical dimensions, or a complete legacy batch, before
    /// requesting any surface size. Paint dimensions come from configure replies,
    /// never from treating the request itself as an acknowledgement.
    pub fn ensure_canvas(&mut self, id: u32) {
        let s = &mut self.state;
        let Some(i) = s.outputs.iter().position(|o| o.global == id) else { return };
        if s.outputs[i].canvas.is_some() {
            return;
        }
        let Some((width, height)) = s.outputs[i].logical_size() else { return };
        let qh = s.qh.clone();
        let surface = s.compositor.create_surface(&qh, id);
        let layer = match s.layer {
            CanvasLayer::Background => zwlr_layer_shell_v1::Layer::Background,
            CanvasLayer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
        };
        let ls = s.layer_shell.get_layer_surface(&surface, Some(&s.outputs[i].output), layer, s.namespace.clone(), &qh, id);
        ls.set_size(width, height);
        use zwlr_layer_surface_v1::Anchor;
        ls.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        ls.set_exclusive_zone(-1);
        ls.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        // On the background the whole root takes the pointer (the default,
        // infinite input region); elsewhere clicks pass through.
        if !s.input {
            let region = s.empty_region();
            surface.set_input_region(Some(&region));
            region.destroy();
        }
        let (viewport, fractional) = match (&s.viewporter, &s.fractional) {
            (Some(vp), Some(fm)) => (Some(vp.get_viewport(&surface, &qh, ())), Some(fm.get_fractional_scale(&surface, &qh, id))),
            (Some(vp), None) => (Some(vp.get_viewport(&surface, &qh, ())), None),
            _ => (None, None),
        };
        surface.commit();
        let buffer_scale = s.outputs[i].legacy.scale.max(1);
        s.outputs[i].canvas = Some(Canvas {
            surface,
            layer: ls,
            viewport,
            fractional,
            width: 0,
            height: 0,
            requested: (width, height),
            configured: false,
            scale120: 0,
            buffer_scale,
            root: None,
            root_rgba: None,
            root_dirty: true,
            planes: Vec::new(),
            order: Vec::new(),
            order_dirty: false,
            frame_pending: None,
            awaiting_since: None,
        });
        s.changed = true;
    }

    /// Destroys output `id`'s canvas.
    pub fn remove_canvas(&mut self, id: u32) {
        if let Some(c) = self.state.outputs.iter_mut().find(|o| o.global == id).and_then(|o| o.canvas.take()) {
            c.destroy();
            self.state.changed = true;
        }
    }

    /// The root colour (premultiplied RGBA) under every plane; `None` is
    /// transparent.
    pub fn set_base(&mut self, id: u32, rgba: Option<[u8; 4]>) {
        if let Some(c) = self.state.canvas_mut(id)
            && (c.root_rgba != rgba || c.root.is_none())
        {
            c.root_rgba = rgba;
            c.root_dirty = true;
        }
    }

    fn plane(&mut self, id: u32, index: usize) -> Option<&mut Plane> {
        let c = self.state.canvas_mut(id)?;
        if c.planes.len() <= index {
            c.planes.resize_with(index + 1, Plane::default);
        }
        c.planes.get_mut(index)
    }

    /// Creates the plane's surface objects on first use.
    fn realize(&mut self, id: u32, index: usize) {
        let s = &mut self.state;
        let qh = s.qh.clone();
        let Some(c) = s.outputs.iter_mut().find(|o| o.global == id).and_then(|o| o.canvas.as_mut()) else { return };
        if c.planes.len() <= index {
            c.planes.resize_with(index + 1, Plane::default);
        }
        if c.planes[index].surface.is_some() {
            return;
        }
        let surface = s.compositor.create_surface(&qh, ());
        let sub = s.subcompositor.get_subsurface(&surface, &c.surface, &qh, ());
        let region = s.compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        let p = &mut c.planes[index];
        p.viewport = s.viewporter.as_ref().map(|vp| vp.get_viewport(&surface, &qh, ()));
        p.alpha = s.alpha.as_ref().map(|a| a.get_surface(&surface, &qh, ()));
        p.alpha_value = 1.0;
        p.surface = Some(surface);
        p.sub = Some(sub);
        // A new subsurface goes on top; the requested order is re-applied.
        c.order_dirty = true;
    }

    /// Paints plane `index` of output `id`: `paint` receives a pixmap of the
    /// plane's buffer size (cleared to transparent when `clear`). `damage`
    /// limits the region the compositor re-uploads (plane-local buffer
    /// pixels; `None` = everything). `keep` keeps released buffers for the
    /// next paint (planes redrawn often); otherwise they are dropped as soon
    /// as the compositor releases them. Returns `false` when every buffer
    /// is still held by the compositor (try again after the next frame).
    #[allow(clippy::too_many_arguments)]
    pub fn paint(&mut self, id: u32, index: usize, place: Place, damage: Option<Rect>, keep: bool, clear: bool, paint: impl FnOnce(&mut tiny_skia::PixmapMut<'_>)) -> Result<bool> {
        self.paint_with(id, index, place, damage, keep, |map, bw, bh, abgr| {
            if abgr {
                let mut pm = tiny_skia::PixmapMut::from_bytes(map, bw, bh).context("canvas pixmap")?;
                if clear {
                    pm.fill(tiny_skia::Color::TRANSPARENT);
                }
                paint(&mut pm);
            } else {
                let mut scratch = tiny_skia::Pixmap::new(bw, bh).context("canvas scratch")?;
                paint(&mut scratch.as_mut());
                swizzle_into(scratch.data(), map);
            }
            Ok(())
        })
    }

    /// Like [`Host::paint`], but hands `fill` the buffer's raw pixels
    /// (`width × height`, rows `width × 4` bytes apart, every pixel to be
    /// written: nothing is cleared) and whether they are stored B, G, R, A
    /// (`ARGB8888`; otherwise R, G, B, A). For content produced straight
    /// into the buffer every frame (video), with no scratch copy.
    pub fn paint_pixels(&mut self, id: u32, index: usize, place: Place, keep: bool, fill: impl FnOnce(&mut [u8], u32, u32, bool)) -> Result<bool> {
        self.paint_with(id, index, place, None, keep, |map, bw, bh, abgr| {
            fill(map, bw, bh, !abgr);
            Ok(())
        })
    }

    /// Whether released buffers of a plane are kept for reuse (see
    /// [`Host::paint`]'s `keep`); turning it off lets [`Host::release_idle`]
    /// free them, e.g. when a video plane stops changing.
    pub fn set_keep(&mut self, id: u32, index: usize, keep: bool) {
        if let Some(p) = self.plane(id, index) {
            p.keep = keep;
        }
    }

    /// The shared part of the paint calls: picks a free buffer of the
    /// plane's size, lets `draw` fill its mapping (`map, width, height,
    /// abgr`), then attaches and damages it.
    fn paint_with(&mut self, id: u32, index: usize, place: Place, damage: Option<Rect>, keep: bool, draw: impl FnOnce(&mut [u8], u32, u32, bool) -> Result<()>) -> Result<bool> {
        let (abgr, shm, qh) = (self.state.abgr, self.state.shm.clone(), self.state.qh.clone());
        let Some(c) = self.state.canvas_mut(id) else { return Ok(false) };
        if !c.configured {
            return Ok(false);
        }
        let (bw, bh) = match place {
            Place::Full => c.physical(),
            Place::Rect(r) => (r.w.max(1), r.h.max(1)),
            Place::Scaled { buffer, .. } | Place::FullScaled { buffer } => (buffer.0.max(1), buffer.1.max(1)),
        };
        self.realize(id, index);
        let Some(p) = self.plane(id, index) else { return Ok(false) };
        p.buffers.retain(|b| (b.width == bw && b.height == bh) || b.busy.load(Ordering::Acquire));
        let slot = match p.buffers.iter().position(|b| !b.busy.load(Ordering::Acquire) && b.width == bw && b.height == bh) {
            Some(i) => i,
            None if p.buffers.len() < MAX_BUFFERS_PER_PLANE => {
                p.buffers.push(new_buffer(&shm, &qh, bw, bh, abgr)?);
                p.buffers.len() - 1
            }
            None => return Ok(false),
        };
        let buf = &mut p.buffers[slot];
        draw(&mut buf.map[..], bw, bh, abgr)?;
        buf.busy.store(true, Ordering::Release);
        let wl = buf.buffer.clone();
        p.keep = keep;
        let surface = p.surface.clone().context("plane surface")?;
        surface.attach(Some(&wl), 0, 0);
        match damage {
            Some(d) => surface.damage_buffer(d.x, d.y, d.w as i32, d.h as i32),
            None => surface.damage_buffer(0, 0, bw as i32, bh as i32),
        }
        p.mapped = true;
        let changed_place = p.place != Some(place);
        p.place = Some(place);
        p.dirty = true;
        if changed_place {
            self.apply_geometry(id, index);
        }
        Ok(true)
    }

    /// Re-sends position, viewport and buffer scale for plane `index`.
    fn apply_geometry(&mut self, id: u32, index: usize) {
        let Some(c) = self.state.canvas_mut(id) else { return };
        let fractional = c.viewport.is_some() && c.scale120 > 0;
        let bs = c.buffer_scale.max(1);
        let scale120 = c.scale120();
        let logical = (c.width, c.height);
        let Some(p) = c.planes.get_mut(index) else { return };
        let (Some(place), Some(sub), Some(surface)) = (p.place, p.sub.as_ref(), p.surface.as_ref()) else { return };
        let rect = place.logical_rect(logical, scale120);
        let (x, y, w, h) = (rect.x, rect.y, rect.w as i32, rect.h as i32);
        let (x, y, w, h) = match p.view {
            Some(v) => (v.dest.0, v.dest.1, v.dest.2.max(1), v.dest.3.max(1)),
            None => (x, y, w, h),
        };
        sub.set_position(x, y);
        match &p.viewport {
            Some(vp) => {
                match p.view {
                    Some(v) => vp.set_source(v.source.0, v.source.1, v.source.2.max(1.0), v.source.3.max(1.0)),
                    None => vp.set_source(-1.0, -1.0, -1.0, -1.0),
                }
                vp.set_destination(w, h);
            }
            None => surface.set_buffer_scale(if fractional { 1 } else { bs }),
        }
        p.dirty = true;
    }

    /// Moves a mapped plane placed at a [`Place::Rect`] to `to`, a
    /// [`snap`]ped rectangle of the same size, keeping its pixels: only the
    /// subsurface position changes (no repaint, no buffer upload). Returns
    /// `false` when the plane isn't mapped there at that size (paint it
    /// instead).
    pub fn move_plane(&mut self, id: u32, index: usize, to: Rect) -> bool {
        let Some(p) = self.plane(id, index) else { return false };
        match p.place {
            Some(Place::Rect(r)) if p.mapped && r.w == to.w && r.h == to.h => {
                if r != to {
                    p.place = Some(Place::Rect(to));
                    self.apply_geometry(id, index);
                }
                true
            }
            _ => false,
        }
    }

    /// Alpha multiplier of a plane (no-op without `wp_alpha_modifier_v1`).
    pub fn set_alpha(&mut self, id: u32, index: usize, alpha: f32) {
        let Some(p) = self.plane(id, index) else { return };
        let a = alpha.clamp(0.0, 1.0);
        if let Some(m) = &p.alpha
            && (p.alpha_value - a).abs() > f32::EPSILON
        {
            m.set_multiplier((a * ALPHA_ONE) as u32);
            p.alpha_value = a;
            p.dirty = true;
        }
    }

    /// Crops / moves a plane for a reveal; `None` restores its placement.
    pub fn set_view(&mut self, id: u32, index: usize, view: Option<View>) {
        let Some(p) = self.plane(id, index) else { return };
        if p.view == view {
            return;
        }
        p.view = view;
        if p.place.is_some() {
            self.apply_geometry(id, index);
        }
    }

    /// Unmaps a plane and frees its buffers.
    pub fn hide(&mut self, id: u32, index: usize) {
        let Some(p) = self.plane(id, index) else { return };
        if !p.mapped {
            return;
        }
        if let Some(s) = &p.surface {
            s.attach(None, 0, 0);
        }
        p.mapped = false;
        p.place = None;
        p.view = None;
        p.dirty = true;
        // Busy buffers are dropped too: their storage is never reused.
        p.buffers.clear();
    }

    /// Destroys plane `index` of output `id` (its subsurface, viewport,
    /// alpha modifier and buffers): for planes whose owner went away, so
    /// neither the shell nor the compositor keeps anything for them. A
    /// later [`Host::paint`] creates it again.
    pub fn remove_plane(&mut self, id: u32, index: usize) {
        let Some(c) = self.state.canvas_mut(id) else { return };
        let Some(p) = c.planes.get_mut(index) else { return };
        if p.surface.is_none() && p.buffers.is_empty() {
            return;
        }
        p.destroy();
        *p = Plane::default();
        // Makes the parent's next commit happen (it applies the removal).
        c.order_dirty = true;
    }

    pub fn is_mapped(&self, id: u32, index: usize) -> bool {
        self.state.outputs.iter().find(|o| o.global == id).and_then(|o| o.canvas.as_ref()).and_then(|c| c.planes.get(index)).is_some_and(|p| p.mapped)
    }

    /// Stacking order of the planes, bottom to top (planes not listed keep
    /// their place above those listed).
    pub fn set_order(&mut self, id: u32, order: &[usize]) {
        let Some(c) = self.state.canvas_mut(id) else { return };
        if c.order != order {
            c.order = order.to_vec();
            c.order_dirty = true;
        }
    }

    /// Applies everything staged for output `id` (planes, then the root).
    /// With `frame`, asks for a frame callback (see [`Host::frame_ready`]).
    pub fn commit(&mut self, id: u32, frame: bool) -> Result<()> {
        let (shm, qh, abgr, has_vp) = (self.state.shm.clone(), self.state.qh.clone(), self.state.abgr, self.state.viewporter.is_some());
        let Some(c) = self.state.canvas_mut(id) else { return Ok(()) };
        if !c.configured {
            return Ok(());
        }
        // Nothing staged: no commit (the compositor would redo its work).
        if !frame && !c.order_dirty && !c.root_dirty && c.root.is_some() && !c.planes.iter().any(|p| p.dirty) {
            return Ok(());
        }
        if c.order_dirty {
            let mut below: Option<wl_surface::WlSurface> = None;
            for &i in &c.order {
                let Some(p) = c.planes.get(i) else { continue };
                let (Some(sub), Some(surface)) = (&p.sub, &p.surface) else { continue };
                match &below {
                    Some(b) => sub.place_above(b),
                    None => sub.place_above(&c.surface),
                }
                below = Some(surface.clone());
            }
            c.order_dirty = false;
        }
        for p in c.planes.iter_mut().filter(|p| p.dirty) {
            if let Some(s) = &p.surface {
                s.commit();
            }
            p.dirty = false;
        }
        if c.root_dirty || c.root.is_none() {
            let rgba = c.root_rgba.unwrap_or([0; 4]);
            // Without a viewport the root buffer must cover the output.
            let (w, h) = if has_vp { (1, 1) } else { c.physical() };
            let mut buf = new_buffer(&shm, &qh, w, h, abgr)?;
            let px = if abgr { rgba } else { [rgba[2], rgba[1], rgba[0], rgba[3]] };
            for d in buf.map.as_chunks_mut::<4>().0 {
                d.copy_from_slice(&px);
            }
            c.surface.attach(Some(&buf.buffer), 0, 0);
            c.surface.damage_buffer(0, 0, w as i32, h as i32);
            match &c.viewport {
                Some(vp) => vp.set_destination(c.width.max(1) as i32, c.height.max(1) as i32),
                None => c.surface.set_buffer_scale(c.buffer_scale.max(1)),
            }
            c.root = Some(buf);
            c.root_dirty = false;
        }
        if frame && c.frame_pending.is_none() {
            c.surface.frame(&qh, id);
            let now = Instant::now();
            c.frame_pending = Some(now);
            c.awaiting_since.get_or_insert(now);
        }
        c.surface.commit();
        Ok(())
    }

    /// No frame callback is outstanding (or it timed out).
    pub fn frame_ready(&self, id: u32) -> bool {
        self.state
            .outputs
            .iter()
            .find(|o| o.global == id)
            .and_then(|o| o.canvas.as_ref())
            .is_none_or(|c| c.frame_pending.is_none_or(|t| t.elapsed() >= FRAME_TIMEOUT))
    }

    /// How long a requested frame callback has gone unanswered (`None` when
    /// none is outstanding). Compositors only send them to surfaces that
    /// are visible, so a long wait means the output's desktop is covered:
    /// continuous content (video) stops there and resumes with the
    /// callback, without polling.
    pub fn awaiting_frame(&self, id: u32) -> Option<Duration> {
        self.state.outputs.iter().find(|o| o.global == id).and_then(|o| o.canvas.as_ref()).and_then(|c| c.awaiting_since).map(|t| t.elapsed())
    }

    /// Drops buffers the compositor released on planes that don't keep them.
    pub fn release_idle(&mut self) {
        for c in self.state.outputs.iter_mut().filter_map(|o| o.canvas.as_mut()) {
            if c.frame_pending.is_some_and(|t| t.elapsed() >= FRAME_TIMEOUT) {
                c.frame_pending = None;
            }
            for p in c.planes.iter_mut().filter(|p| !p.keep) {
                p.buffers.retain(|b| b.busy.load(Ordering::Acquire));
            }
        }
    }

    /// Bytes of shared memory currently mapped by all canvases.
    pub fn mapped_bytes(&self) -> u64 {
        self.state
            .outputs
            .iter()
            .filter_map(|o| o.canvas.as_ref())
            .flat_map(|c| c.planes.iter().flat_map(|p| p.buffers.iter()).chain(c.root.iter()))
            .map(|b| u64::from(b.width) * u64::from(b.height) * u64::from(BYTES_PER_PIXEL))
            .sum()
    }

    /// Flushes requests, then sleeps until a Wayland event, a [`Waker`]
    /// wake-up or `timeout`, and dispatches what arrived. Returns `false`
    /// when only pointer input arrived (motion, enter / leave): nothing the
    /// app draws changed, so it can wait again without a redraw (presses
    /// are queued for [`Host::take_presses`] either way).
    pub fn wait(&mut self, timeout: Option<Duration>, waker: &Waker) -> Result<bool> {
        self.state.input_events = 0;
        let mut dispatched = self.queue.dispatch_pending(&mut self.state).context("Wayland dispatch")?;
        self.conn.flush().context("Wayland flush")?;
        let Some(guard) = self.queue.prepare_read() else {
            dispatched += self.queue.dispatch_pending(&mut self.state).context("Wayland dispatch")?;
            return Ok(!self.input_only(dispatched, false));
        };
        let wl_ready;
        let woken;
        {
            let fd = guard.connection_fd();
            let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN), rustix::event::PollFd::new(&waker.0, rustix::event::PollFlags::IN)];
            let ts = timeout.map(|t| rustix::event::Timespec { tv_sec: t.as_secs() as i64, tv_nsec: t.subsec_nanos() as i64 });
            match rustix::event::poll(&mut fds, ts.as_ref()) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => {}
                Err(err) => return Err(err).context("poll"),
            }
            wl_ready = !fds[0].revents().is_empty();
            woken = !fds[1].revents().is_empty();
            if woken {
                waker.drain();
            }
        }
        if wl_ready {
            if let Err(err) = guard.read()
                && !matches!(err, wayland_client::backend::WaylandError::Io(ref e) if e.kind() == std::io::ErrorKind::WouldBlock)
            {
                return Err(err).context("reading Wayland events");
            }
        } else {
            drop(guard);
        }
        dispatched += self.queue.dispatch_pending(&mut self.state).context("Wayland dispatch")?;
        // A timeout or a wake-up is never input only.
        Ok(!self.input_only(dispatched, woken || !wl_ready))
    }

    /// Everything dispatched was pointer input (and nothing else woke us).
    fn input_only(&self, dispatched: usize, other: bool) -> bool {
        !other && dispatched > 0 && dispatched == self.state.input_events && self.state.presses.is_empty()
    }

    pub fn flush(&self) -> Result<()> {
        self.conn.flush().context("Wayland flush")
    }
}

/// The plane operations the canvas painters use: implemented by [`Host`],
/// and by recorders in tests (plane bookkeeping is tested without a
/// compositor).
pub trait Planes {
    /// See [`Host::paint`].
    #[allow(clippy::too_many_arguments)]
    fn paint(&mut self, id: u32, index: usize, place: Place, damage: Option<Rect>, keep: bool, clear: bool, paint: impl FnOnce(&mut tiny_skia::PixmapMut<'_>)) -> Result<bool>;
    /// See [`Host::hide`].
    fn hide(&mut self, id: u32, index: usize);
    /// See [`Host::remove_plane`].
    fn remove_plane(&mut self, id: u32, index: usize);
    /// The plane exists (painted, or hidden but not removed).
    fn has_plane(&self, id: u32, index: usize) -> bool;
    /// See [`Host::frame_ready`].
    fn frame_ready(&self, id: u32) -> bool;
    /// See [`Host::move_plane`].
    fn move_plane(&mut self, id: u32, index: usize, to: Rect) -> bool;
}

impl Planes for Host {
    fn move_plane(&mut self, id: u32, index: usize, to: Rect) -> bool {
        Host::move_plane(self, id, index, to)
    }

    fn paint(&mut self, id: u32, index: usize, place: Place, damage: Option<Rect>, keep: bool, clear: bool, paint: impl FnOnce(&mut tiny_skia::PixmapMut<'_>)) -> Result<bool> {
        Host::paint(self, id, index, place, damage, keep, clear, paint)
    }

    fn hide(&mut self, id: u32, index: usize) {
        Host::hide(self, id, index);
    }

    fn remove_plane(&mut self, id: u32, index: usize) {
        Host::remove_plane(self, id, index);
    }

    fn has_plane(&self, id: u32, index: usize) -> bool {
        self.state.outputs.iter().find(|o| o.global == id).and_then(|o| o.canvas.as_ref()).and_then(|c| c.planes.get(index)).is_some_and(|p| p.surface.is_some() || !p.buffers.is_empty())
    }

    fn frame_ready(&self, id: u32) -> bool {
        Host::frame_ready(self, id)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        for o in &mut self.state.outputs {
            if let Some(c) = o.canvas.take() {
                c.destroy();
            }
        }
        for s in &mut self.state.seats {
            s.release_pointer();
            if s.seat.version() >= 5 {
                s.seat.release();
            }
        }
        let _ = self.conn.flush();
    }
}

// ─────────────────────────────── dispatch ───────────────────────────────

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let output = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, name);
                state.add_output(output, name);
                state.changed = true;
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_seat" && state.input => {
                let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(SEAT_VERSION), qh, name);
                state.seats.push(Seat { seat, pointer: None, shape: None });
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.seats.iter().position(|s| s.seat.data::<u32>() == Some(&name)) {
                    let mut seat = state.seats.remove(i);
                    seat.release_pointer();
                    if seat.seat.version() >= 5 {
                        seat.seat.release();
                    }
                }
                if let Some(i) = state.outputs.iter().position(|o| o.global == name) {
                    let mut out = state.outputs.remove(i);
                    if let Some(c) = out.canvas.take() {
                        c.destroy();
                    }
                    if let Some(xdg_output) = out.xdg_output.take() { xdg_output.destroy(); }
                    if out.output.version() >= 3 {
                        out.output.release();
                    }
                    state.changed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(out) = state.outputs.iter_mut().find(|o| o.global == *global) else { return };
        if out.legacy.event(&event, out.output.version()) {
            if out.xdg_output.is_none() { out.resize_canvas(); }
            if let Some(canvas) = out.canvas.as_mut()
                && canvas.fractional.is_none()
            {
                canvas.buffer_scale = out.legacy.scale.max(1);
            }
            state.changed = true;
        }
        match event {
            wl_output::Event::Name { name } => out.name = name,
            wl_output::Event::Description { description } => out.description = description,
            wl_output::Event::Geometry { make, model, .. } => out.identity = format!("{make} {model}").trim().to_string(),
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(state: &mut Self, _: &zxdg_output_v1::ZxdgOutputV1, event: zxdg_output_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(output) = state.outputs.iter_mut().find(|output| output.global == *global) else { return };
        if let zxdg_output_v1::Event::LogicalSize { width, height } = event
            && let Some(size) = changed_logical_size(output.logical, width, height)
        {
            output.logical = Some(size);
            output.resize_canvas();
            state.changed = true;
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
            && let Some(c) = state.canvas_mut(*global)
            && c.fractional.is_none()
            && c.buffer_scale != factor
        {
            c.buffer_scale = factor;
            state.changed = true;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, u32> for State {
    fn event(state: &mut Self, _: &wl_callback::WlCallback, event: wl_callback::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(c) = state.canvas_mut(*global)
        {
            c.frame_pending = None;
            c.awaiting_since = None;
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, u32> for State {
    fn event(state: &mut Self, layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                if let Some(c) = state.canvas_mut(*global) {
                    if (c.width, c.height) != (width, height) || !c.configured {
                        c.root_dirty = true;
                    }
                    c.width = width;
                    c.height = height;
                    c.configured = true;
                }
                state.changed = true;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(c) = state.outputs.iter_mut().find(|o| o.global == *global).and_then(|o| o.canvas.take()) {
                    c.destroy();
                }
                state.changed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &u32, _: &Connection, qh: &QueueHandle<Self>) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event else { return };
        let Some(entry) = state.seats.iter_mut().find(|s| s.seat == *seat) else { return };
        let has = caps.contains(wl_seat::Capability::Pointer);
        if has && entry.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            entry.shape = state.cursor_shape.as_ref().map(|m| m.get_pointer(&pointer, qh, ()));
            entry.pointer = Some(pointer);
        } else if !has && entry.pointer.is_some() {
            entry.release_pointer();
            state.pointer = None;
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut Self, pointer: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.input_events += 1;
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                let output = state.outputs.iter().find(|o| o.canvas.as_ref().is_some_and(|c| c.surface == surface)).map(|o| o.global);
                state.pointer = output.map(|o| (o, surface_x, surface_y));
                // The plain arrow (else the last window's resize cursor
                // would stay); without cursor-shape the compositor's stays.
                if let Some(d) = state.seats.iter().find(|s| s.pointer.as_ref() == Some(pointer)).and_then(|s| s.shape.as_ref()) {
                    d.set_shape(serial, wp_cursor_shape_device_v1::Shape::Default);
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some(p) = state.pointer.as_mut() {
                    (p.1, p.2) = (surface_x, surface_y);
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => {
                if let Some((output, x, y)) = state.pointer
                    && state.presses.len() < MAX_PRESSES
                {
                    state.presses.push(Press { output, x, y, button });
                }
            }
            wl_pointer::Event::Axis { axis: WEnum::Value(wl_pointer::Axis::VerticalScroll), value, .. } => {
                state.scroll += value;
                while state.scroll.abs() >= SCROLL_STEP {
                    let up = state.scroll < 0.0;
                    state.scroll -= SCROLL_STEP.copysign(state.scroll);
                    if let Some((output, x, y)) = state.pointer
                        && state.presses.len() < MAX_PRESSES
                    {
                        state.presses.push(Press { output, x, y, button: if up { SCROLL_UP } else { SCROLL_DOWN } });
                    }
                }
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer = None;
                state.scroll = 0.0;
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, u32> for State {
    fn event(state: &mut Self, _: &wp_fractional_scale_v1::WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(c) = state.canvas_mut(*global)
            && c.scale120 != scale
        {
            c.scale120 = scale;
            c.root_dirty = true;
            state.changed = true;
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
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    wl_compositor::WlCompositor,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    wl_surface::WlSurface,
    wl_region::WlRegion,
    wl_shm_pool::WlShmPool,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_alpha_modifier_v1::WpAlphaModifierV1,
    wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1,
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
    wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
);

#[cfg(test)]
mod tests {
    use super::*;

    fn current_mode(width: i32, height: i32) -> wl_output::Event {
        wl_output::Event::Mode { flags: WEnum::Value(wl_output::Mode::Current), width, height, refresh: 60_000 }
    }

    #[test]
    fn initial_legacy_size_waits_for_mode_scale_and_done() {
        let mut size = LegacyOutputSize::default();
        assert!(!size.event(&current_mode(3840, 2160), 2));
        assert_eq!(size.logical_size(), None);
        assert!(!size.event(&wl_output::Event::Scale { factor: 2 }, 2));
        assert_eq!(size.logical_size(), None);
        assert!(size.event(&wl_output::Event::Done, 2));
        assert_eq!(size.logical_size(), Some((1920, 1080)));
    }

    #[test]
    fn legacy_mode_and_scale_never_request_an_intermediate_size() {
        for events in [
            [current_mode(3840, 2160), wl_output::Event::Scale { factor: 2 }, wl_output::Event::Done],
            [wl_output::Event::Scale { factor: 2 }, current_mode(3840, 2160), wl_output::Event::Done],
        ] {
            let mut size = LegacyOutputSize { mode: Some((1920, 1080)), scale: 1, complete: true };
            let configured = (1920, 1080);
            let mut requests = Vec::new();
            for event in events {
                if size.event(&event, 4)
                    && let Some(target) = size.logical_size()
                    && target != configured
                {
                    requests.push(target);
                }
            }
            assert!(requests.is_empty(), "the final logical size matches the existing configure; an intermediate request would leave the canvas at the wrong size");
            assert_eq!(size.logical_size(), Some(configured));
        }
    }

    #[test]
    fn legacy_mode_only_and_scale_only_changes_complete_at_done() {
        let mut size = LegacyOutputSize { mode: Some((1920, 1080)), scale: 1, complete: true };
        assert!(!size.event(&current_mode(2560, 1440), 2));
        assert_eq!(size.logical_size(), None);
        assert!(size.event(&wl_output::Event::Done, 2));
        assert_eq!(size.logical_size(), Some((2560, 1440)));
        assert!(!size.event(&wl_output::Event::Scale { factor: 2 }, 2));
        assert_eq!(size.logical_size(), None);
        assert!(size.event(&wl_output::Event::Done, 2));
        assert_eq!(size.logical_size(), Some((1280, 720)));
    }

    #[test]
    fn completed_batches_can_return_to_the_old_size_before_a_configure_reply() {
        let configured = (1920, 1080);
        let mut requested = configured;
        let mut size = LegacyOutputSize { mode: Some(configured), scale: 1, complete: true };
        let mut requests = Vec::new();
        for event in [current_mode(3840, 2160), wl_output::Event::Done, wl_output::Event::Scale { factor: 2 }, wl_output::Event::Done] {
            if size.event(&event, 4)
                && let Some(target) = size.logical_size()
                && request_logical_size(&mut requested, target)
            {
                requests.push(target);
            }
        }
        assert_eq!(requests, [(3840, 2160), configured]);
        assert_eq!(requested, configured);
        assert!(!request_logical_size(&mut requested, configured));
        assert!(!request_logical_size(&mut requested, (0, 1080)));
        assert_eq!(requested, configured);
    }

    #[test]
    fn version_one_outputs_apply_modes_without_waiting_for_done() {
        let mut size = LegacyOutputSize::default();
        assert!(size.event(&current_mode(1920, 1080), 1));
        assert_eq!(size.logical_size(), Some((1920, 1080)));
        assert!(size.event(&current_mode(1280, 720), 1));
        assert_eq!(size.logical_size(), Some((1280, 720)));
        assert!(!size.event(&current_mode(0, 0), 1));
        assert_eq!(size.logical_size(), Some((1280, 720)));
    }

    #[test]
    fn native_logical_size_drives_initial_and_rescaled_requests() {
        let initial = changed_logical_size(None, 1365, 769);
        assert_eq!(initial, Some((1365, 769)));
        assert_eq!(changed_logical_size(initial, 1365, 769), None);
        let rescaled = changed_logical_size(initial, 1537, 865);
        assert_eq!(rescaled, Some((1537, 865)));
        for (width, height) in [(0, 769), (1365, 0), (-1, 769), (1365, -1)] {
            assert_eq!(changed_logical_size(rescaled, width, height), None);
        }
    }

    #[test]
    fn full_scaled_planes_keep_exact_logical_output_at_fractional_scales() {
        for (logical, scale120) in [((1365, 769), 150), ((1537, 865), 144)] {
            let physical = ((logical.0 * scale120 + SCALE_DENOMINATOR / 2) / SCALE_DENOMINATOR, (logical.1 * scale120 + SCALE_DENOMINATOR / 2) / SCALE_DENOMINATOR);
            let rounded = Place::Scaled { rect: Rect::new(0, 0, physical.0, physical.1), buffer: (1, 1) }.logical_rect(logical, scale120);
            assert!(rounded.w < logical.0 || rounded.h < logical.1, "fixture must expose the old rounding seam");
            for buffer in [(1, 1), (192, 144), physical] {
                assert_eq!(Place::FullScaled { buffer }.logical_rect(logical, scale120), Rect::new(0, 0, logical.0, logical.1));
            }
        }
        assert_eq!(Place::Scaled { rect: Rect::new(5, 10, 20, 30), buffer: (4, 6) }.logical_rect((1365, 769), 150), Rect::new(4, 8, 16, 24));
    }

    #[test]
    fn grid_steps_make_logical_coordinates_whole() {
        assert_eq!(grid_step(120), 1);
        assert_eq!(grid_step(240), 2);
        assert_eq!(grid_step(180), 3);
        assert_eq!(grid_step(150), 5);
        for s in [120u32, 144, 150, 180, 210, 240] {
            let g = grid_step(s);
            // A grid step in physical pixels is a whole number of logical ones.
            assert_eq!((g * SCALE_DENOMINATOR) % s, 0, "scale {s}");
        }
    }

    #[test]
    fn snapping_expands_outward() {
        let r = snap(Rect::new(10, 7, 20, 5), 180);
        assert_eq!(r, Rect::new(9, 6, 21, 6));
        assert!(r.x <= 10 && r.right() >= 30 && r.bottom() >= 12);
        assert_eq!(snap(Rect::new(-4, -1, 3, 3), 180), Rect::new(-6, -3, 6, 6));
        assert_eq!(snap(Rect::new(3, 3, 3, 3), 120), Rect::new(3, 3, 3, 3));
    }

    #[test]
    fn rect_union_and_intersection() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert!(a.intersect(&Rect::new(20, 20, 1, 1)).is_empty());
        assert_eq!(Rect::default().union(&b), b);
        assert_eq!(Rect::from_edges(0.5, 1.2, 9.1, 3.0), Rect::new(0, 1, 10, 2));
    }

    #[test]
    fn argb_swizzle() {
        let mut dst = [0u8; 8];
        swizzle_into(&[1, 2, 3, 4, 5, 6, 7, 8], &mut dst);
        assert_eq!(dst, [3, 2, 1, 4, 7, 6, 5, 8]);
    }
}
