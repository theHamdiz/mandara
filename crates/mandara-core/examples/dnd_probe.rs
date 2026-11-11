//! A drag-and-drop test client for headless checks (Stash's end-to-end
//! tests): a plain coloured layer surface that either *starts* a drag when
//! pressed (`give`) or *takes* drops (`take`), printing what happens.
//!
//! ```sh
//! # A drag source at 100,100 (200×120) offering a file, a picture and text:
//! cargo run -p mandara-core --example dnd_probe -- give 100 100 200 120 \
//!     text/uri-list=text:file:///tmp/a.txt image/png=file:/tmp/a.png text/plain=text:hello
//! # Then press on it and drag with `cargo xtask sandbox pointer drag …`.
//!
//! # A drop target at 1800,100 that accepts anything and prints the bytes
//! # of the first type it prefers:
//! cargo run -p mandara-core --example dnd_probe -- take 1800 100 300 200
//! ```
//!
//! Output lines (stdout): `ready`, `offer <mime>`, `enter`, `drop <mime>
//! <bytes> <head>`, `accepted <mime>`, `send <mime>`, `finished`,
//! `cancelled`. Exits after a finished drag or drop, or after 60 s.

use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const TIMEOUT: Duration = Duration::from_secs(60);
/// Types a target prefers to read, best first.
const PREFERRED: [&str; 7] = ["image/png", "text/uri-list", "text/x-moz-url", "text/html", "text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

enum Content {
    Text(String),
    File(std::path::PathBuf),
}

struct State {
    source_mode: bool,
    offers: Vec<(String, Content)>,
    configured: Option<(u32, u32)>,
    surface: Option<wl_surface::WlSurface>,
    layer: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    device: Option<wl_data_device::WlDataDevice>,
    drag: Option<wl_data_source::WlDataSource>,
    pointer: Option<wl_pointer::WlPointer>,
    offer: Option<(wl_data_offer::WlDataOffer, Vec<String>)>,
    pending: Vec<(wl_data_offer::WlDataOffer, Vec<String>)>,
    done: bool,
}

fn say(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "dnd_probe give|take X Y W H [mime=text:…|mime=file:/path …]";
    let mode = args.first().ok_or_else(|| anyhow::anyhow!(usage))?.clone();
    let num = |i: usize| args.get(i).and_then(|v| v.parse::<i32>().ok()).ok_or_else(|| anyhow::anyhow!(usage));
    let (x, y, w, h) = (num(1)?, num(2)?, num(3)?, num(4)?);
    let offers = args[5..]
        .iter()
        .filter_map(|a| {
            // The MIME type may hold `=` itself (`text/plain;charset=utf-8`).
            let at = a.find("=text:").or_else(|| a.find("=file:"))?;
            let (mime, value) = (&a[..at], &a[at + 1..]);
            // `\r\n` in a text value is a line break (uri-lists need CRLF).
            let content = if let Some(t) = value.strip_prefix("text:") { Content::Text(t.replace("\\r", "\r").replace("\\n", "\n")) } else { Content::File(value.strip_prefix("file:").unwrap_or(value).into()) };
            Some((mime.to_string(), content))
        })
        .collect();

    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ())?;
    let manager: wl_data_device_manager::WlDataDeviceManager = globals.bind(&qh, 3..=3, ())?;
    let mut state = State { source_mode: mode == "give", offers, configured: None, surface: None, layer: None, manager: Some(manager.clone()), device: None, drag: None, pointer: None, offer: None, pending: Vec::new(), done: false };
    state.device = Some(manager.get_data_device(&seat, &qh, ()));

    let surface = compositor.create_surface(&qh, ());
    let layer = layer_shell.get_layer_surface(&surface, None, zwlr_layer_shell_v1::Layer::Top, "mandara-test-dnd".into(), &qh, ());
    layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
    layer.set_margin(y, 0, 0, x);
    layer.set_size(w as u32, h as u32);
    layer.set_exclusive_zone(-1);
    surface.commit();
    state.surface = Some(surface.clone());
    state.layer = Some(layer);
    while state.configured.is_none() {
        queue.blocking_dispatch(&mut state)?;
    }
    // A flat colour: orange for a source, teal for a target.
    let (bw, bh) = state.configured.unwrap_or((w as u32, h as u32));
    let stride = bw * 4;
    let size = (stride * bh) as usize;
    let fd = rustix::fs::memfd_create("dnd-probe", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    let px: [u8; 4] = if state.source_mode { [0x30, 0x90, 0xF0, 0xFF] } else { [0xB0, 0xA0, 0x30, 0xFF] };
    file.write_all(&px.repeat(size / 4))?;
    let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
    let buffer = pool.create_buffer(0, bw as i32, bh as i32, stride as i32, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, bw as i32, bh as i32);
    surface.commit();
    say("ready");
    let started = Instant::now();
    while !state.done && started.elapsed() < TIMEOUT {
        queue.blocking_dispatch(&mut state)?;
    }
    if !state.done {
        say("timeout");
    }
    Ok(())
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { serial, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } = event
            && state.source_mode
            && state.drag.is_none()
        {
            let (Some(manager), Some(device), Some(surface)) = (&state.manager, &state.device, &state.surface) else { return };
            let source = manager.create_data_source(qh, ());
            for (mime, _) in &state.offers {
                source.offer(mime.clone());
            }
            source.set_actions(wl_data_device_manager::DndAction::Copy | wl_data_device_manager::DndAction::Move);
            device.start_drag(Some(&source), surface, None, serial);
            state.drag = Some(source);
            say("drag-start");
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for State {
    fn event(state: &mut Self, _: &wl_data_source::WlDataSource, event: wl_data_source::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_source::Event::Target { mime_type } => say(&format!("accepted {}", mime_type.unwrap_or_default())),
            wl_data_source::Event::Send { mime_type, fd } => {
                say(&format!("send {mime_type}"));
                let mut out = std::fs::File::from(fd);
                match state.offers.iter().find(|(m, _)| *m == mime_type).map(|(_, c)| c) {
                    Some(Content::Text(t)) => {
                        let _ = out.write_all(t.as_bytes());
                    }
                    Some(Content::File(p)) => {
                        if let Ok(mut f) = std::fs::File::open(p) {
                            let _ = std::io::copy(&mut f, &mut out);
                        }
                    }
                    None => {}
                }
            }
            wl_data_source::Event::Action { dnd_action } => say(&format!("action {dnd_action:?}")),
            wl_data_source::Event::DndFinished => {
                say("finished");
                state.done = true;
            }
            wl_data_source::Event::Cancelled => {
                say("cancelled");
                state.done = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(state: &mut Self, _: &wl_data_device::WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_device::Event::DataOffer { id } => state.pending.push((id, Vec::new())),
            wl_data_device::Event::Enter { serial, id: Some(offer), .. } if !state.source_mode => {
                let mimes = state.pending.iter().find(|(o, _)| *o == offer).map(|(_, m)| m.clone()).unwrap_or_default();
                say(&format!("enter {}", mimes.join(" ")));
                let pick = PREFERRED.iter().find(|p| mimes.iter().any(|m| m == *p)).map(|s| s.to_string()).or_else(|| mimes.first().cloned());
                offer.accept(serial, pick);
                offer.set_actions(wl_data_device_manager::DndAction::Copy, wl_data_device_manager::DndAction::Copy);
                state.offer = Some((offer, mimes));
            }
            wl_data_device::Event::Drop if !state.source_mode => {
                let Some((offer, mimes)) = state.offer.take() else { return };
                for mime in PREFERRED.iter().filter(|p| mimes.iter().any(|m| m == **p)).take(2) {
                    let (read, write) = rustix::pipe::pipe().expect("pipe");
                    offer.receive(mime.to_string(), write.as_fd());
                    drop(write);
                    if let Some(conn) = offer.backend().upgrade() {
                        let _ = conn.flush();
                    }
                    let mut bytes = Vec::new();
                    let _ = std::fs::File::from(read).read_to_end(&mut bytes);
                    let head: String = String::from_utf8_lossy(&bytes[..bytes.len().min(160)]).replace(['\r', '\n'], "⏎");
                    say(&format!("drop {mime} {} {head}", bytes.len()));
                }
                offer.finish();
                offer.destroy();
                state.done = true;
            }
            wl_data_device::Event::Leave => say("leave"),
            wl_data_device::Event::Enter { x, y, .. } => say(&format!("enter-own {x:.0},{y:.0}")),
            wl_data_device::Event::Motion { x, y, .. } if std::env::var_os("DND_PROBE_MOTION").is_some() => say(&format!("motion {x:.0},{y:.0}")),
            _ => {}
        }
    }

    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(state: &mut Self, offer: &wl_data_offer::WlDataOffer, event: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some((_, mimes)) = state.pending.iter_mut().find(|(o, _)| o == offer)
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(state: &mut Self, layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                state.configured = Some((width.max(1), height.max(1)));
            }
            zwlr_layer_surface_v1::Event::Closed => state.done = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

macro_rules! ignore {
    ($($ty:ty),*) => {$(
        impl Dispatch<$ty, ()> for State {
            fn event(_: &mut Self, _: &$ty, _: <$ty as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    // The pointer exists once the seat has one (a headless compositor gets
    // it with the first virtual pointer).
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

ignore!(wl_compositor::WlCompositor, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer, wl_surface::WlSurface, wl_data_device_manager::WlDataDeviceManager, zwlr_layer_shell_v1::ZwlrLayerShellV1);
