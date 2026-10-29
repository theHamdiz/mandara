//! Native screen capture over `zwlr_screencopy_manager_v1` (pure Rust, no
//! external tools). Frames are copied into a memfd-backed `wl_shm` buffer
//! and read back through the file descriptor.

use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsFd;

use anyhow::{Context as _, Result, anyhow, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1};

/// One captured output.
#[derive(Clone, Debug)]
pub struct Capture {
    pub output: String,
    /// Logical geometry of the output in the compositor layout.
    pub logical: (i32, i32, i32, i32),
    /// Physical pixel size of `rgba`.
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8.
    pub rgba: Vec<u8>,
}

/// Bytes per RGBA pixel.
const RGBA_BYTES: usize = 4;

impl Capture {
    /// Physical pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        if self.logical.2 > 0 { self.width as f32 / self.logical.2 as f32 } else { 1.0 }
    }

    /// Crops a logical-space rect (relative to this output) into an image.
    pub fn crop_logical(&self, x: f32, y: f32, w: f32, h: f32) -> image::RgbaImage {
        let s = self.scale();
        let x0 = ((x * s).round().max(0.0) as u32).min(self.width);
        let y0 = ((y * s).round().max(0.0) as u32).min(self.height);
        let x1 = (((x + w) * s).round().max(0.0) as u32).clamp(x0, self.width);
        let y1 = (((y + h) * s).round().max(0.0) as u32).clamp(y0, self.height);
        // A borrowed view: only the cropped pixels are copied (the whole
        // frame used to be cloned first — 33 MB at 4K).
        let (w, h) = ((x1 - x0).max(1), (y1 - y0).max(1));
        let mut out = image::RgbaImage::new(w, h);
        let (stride, row) = (self.width as usize * RGBA_BYTES, w as usize * RGBA_BYTES);
        for (dy, dst) in out.chunks_exact_mut(row).enumerate() {
            let start = (y0 as usize + dy) * stride + x0 as usize * RGBA_BYTES;
            if let Some(src) = self.rgba.get(start..start + row) {
                dst.copy_from_slice(src);
            }
        }
        out
    }

    pub fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone()).unwrap_or_else(|| image::RgbaImage::new(self.width, self.height))
    }
}

#[derive(Default)]
struct OutputInfo {
    name: String,
    logical: (i32, i32, i32, i32),
}

#[derive(Default)]
struct FrameState {
    format: Option<(wl_shm::Format, u32, u32, u32)>,
    buffer_done: bool,
    ready: bool,
    failed: bool,
    y_invert: bool,
}

#[derive(Default)]
struct State {
    outputs: Vec<(wl_output::WlOutput, OutputInfo)>,
    frame: FrameState,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(_: &mut Self, _: &wl_output::WlOutput, _: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn event(_: &mut Self, _: &zxdg_output_manager_v1::ZxdgOutputManagerV1, _: zxdg_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for State {
    fn event(state: &mut Self, _: &zxdg_output_v1::ZxdgOutputV1, event: zxdg_output_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let Some((_, info)) = state.outputs.get_mut(*idx) else { return };
        match event {
            zxdg_output_v1::Event::Name { name } => info.name = name,
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                info.logical.0 = x;
                info.logical.1 = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                info.logical.2 = width;
                info.logical.3 = height;
            }
            _ => {}
        }
    }
}
impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for State {
    fn event(_: &mut Self, _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _: zwlr_screencopy_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, event: zwlr_screencopy_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer { format: WEnum::Value(format), width, height, stride } => {
                // Prefer the first shm format offered.
                if state.frame.format.is_none() {
                    state.frame.format = Some((format, width, height, stride));
                }
            }
            Event::BufferDone => state.frame.buffer_done = true,
            Event::Flags { flags: WEnum::Value(f) } => state.frame.y_invert = f.contains(zwlr_screencopy_frame_v1::Flags::YInvert),
            Event::Ready { .. } => state.frame.ready = true,
            Event::Failed => state.frame.failed = true,
            _ => {}
        }
    }
}

/// Converts a shm buffer to tightly packed RGBA.
pub fn to_rgba(format: wl_shm::Format, width: u32, height: u32, stride: u32, data: &[u8], y_invert: bool) -> Result<Vec<u8>> {
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    if data.len() < stride * h {
        bail!("short buffer");
    }
    let mut out = vec![0u8; w * h * 4];
    // Memory byte order (little-endian) for each wl_shm format.
    let order: [usize; 4] = match format {
        wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => [2, 1, 0, 3],
        wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => [0, 1, 2, 3],
        other => bail!("unsupported shm format {other:?}"),
    };
    let opaque = matches!(format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Xbgr8888);
    for row in 0..h {
        let src_row = if y_invert { h - 1 - row } else { row };
        let src = &data[src_row * stride..src_row * stride + w * 4];
        let dst = &mut out[row * w * 4..(row + 1) * w * 4];
        for (px, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            d[0] = px[order[0]];
            d[1] = px[order[1]];
            d[2] = px[order[2]];
            d[3] = if opaque { 255 } else { px[order[3]] };
        }
    }
    Ok(out)
}

/// Captures every output (cursor excluded).
pub fn capture_all() -> Result<Vec<Capture>> {
    capture_all_with(false)
}

/// Captures every output, with the pointer painted in when `cursor`.
pub fn capture_all_with(cursor: bool) -> Result<Vec<Capture>> {
    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let mut state = State::default();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
    let manager: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 =
        globals.bind(&qh, 1..=3, ()).map_err(|_| anyhow!("compositor lacks zwlr_screencopy_manager_v1"))?;
    let xdg: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1> = globals.bind(&qh, 1..=3, ()).ok();
    let output_globals: Vec<_> = globals
        .contents()
        .with_list(|list| list.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).cloned().collect());
    for g in output_globals {
        let output: wl_output::WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, ());
        let idx = state.outputs.len();
        if let Some(xdg) = &xdg {
            xdg.get_xdg_output(&output, &qh, idx);
        }
        state.outputs.push((output, OutputInfo::default()));
    }
    queue.roundtrip(&mut state)?;

    let mut captures = Vec::new();
    for i in 0..state.outputs.len() {
        state.frame = FrameState::default();
        let output = state.outputs[i].0.clone();
        let frame = manager.capture_output(i32::from(cursor), &output, &qh, ());
        let v3 = manager.version() >= 3;
        while state.frame.format.is_none() || (v3 && !state.frame.buffer_done) {
            if state.frame.failed {
                bail!("screencopy failed");
            }
            queue.blocking_dispatch(&mut state)?;
        }
        let (format, width, height, stride) = state.frame.format.expect("checked above");
        let size = (stride * height) as usize;
        let fd = rustix::fs::memfd_create("mandara-screencopy", rustix::fs::MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, size as u64)?;
        let pool = shm.create_pool(fd.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, &qh, ());
        frame.copy(&buffer);
        while !state.frame.ready {
            if state.frame.failed {
                bail!("screencopy copy failed");
            }
            queue.blocking_dispatch(&mut state)?;
        }
        let mut file = std::fs::File::from(fd);
        file.seek(SeekFrom::Start(0))?;
        let mut data = Vec::with_capacity(size);
        file.read_to_end(&mut data)?;
        let rgba = to_rgba(format, width, height, stride, &data, state.frame.y_invert)?;
        let info = &state.outputs[i].1;
        captures.push(Capture { output: info.name.clone(), logical: info.logical, width, height, rgba });
        buffer.destroy();
        pool.destroy();
        frame.destroy();
    }
    Ok(captures)
}

/// One frame borrowed from a [`FrameSource`].
pub struct RawFrame<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// Byte order is B,G,R,x (`true`) or R,G,B,x (`false`).
    pub bgr: bool,
    pub y_invert: bool,
}

/// A persistent capture session on one output (optionally a region of
/// it) for recording: one connection and one shm buffer, reused per frame.
pub struct FrameSource {
    queue: wayland_client::EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    shm: wl_shm::WlShm,
    manager: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    output: wl_output::WlOutput,
    region: Option<(i32, i32, i32, i32)>,
    cursor: bool,
    buffer: Option<(wl_shm_pool::WlShmPool, wl_buffer::WlBuffer, std::fs::File, (wl_shm::Format, u32, u32, u32))>,
    data: Vec<u8>,
}

impl FrameSource {
    /// `output`: xdg-output name (empty = first output). `region`: logical
    /// `x, y, w, h` relative to that output.
    pub fn new(output: &str, region: Option<(i32, i32, i32, i32)>) -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to Wayland")?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let mut state = State::default();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
        let manager: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 =
            globals.bind(&qh, 1..=3, ()).map_err(|_| anyhow!("compositor lacks zwlr_screencopy_manager_v1"))?;
        let xdg: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1> = globals.bind(&qh, 1..=3, ()).ok();
        let output_globals: Vec<_> = globals
            .contents()
            .with_list(|list| list.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).cloned().collect());
        for g in output_globals {
            let o: wl_output::WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, ());
            let idx = state.outputs.len();
            if let Some(xdg) = &xdg {
                xdg.get_xdg_output(&o, &qh, idx);
            }
            state.outputs.push((o, OutputInfo::default()));
        }
        queue.roundtrip(&mut state)?;
        let output = state
            .outputs
            .iter()
            .find(|(_, i)| output.is_empty() || i.name == output)
            .or(state.outputs.first())
            .map(|(o, _)| o.clone())
            .ok_or_else(|| anyhow!("no outputs"))?;
        Ok(Self { queue, qh, state, shm, manager, output, region, cursor: false, buffer: None, data: Vec::new() })
    }

    /// Include the pointer in captured frames (recordings).
    pub fn with_cursor(mut self, on: bool) -> Self {
        self.cursor = on;
        self
    }

    /// Captures the next frame (blocking until the compositor delivers it).
    pub fn next_frame(&mut self) -> Result<RawFrame<'_>> {
        self.state.frame = FrameState::default();
        let frame = match self.region {
            Some((x, y, w, h)) => self.manager.capture_output_region(self.cursor as i32, &self.output, x, y, w.max(1), h.max(1), &self.qh, ()),
            None => self.manager.capture_output(self.cursor as i32, &self.output, &self.qh, ()),
        };
        let v3 = self.manager.version() >= 3;
        while self.state.frame.format.is_none() || (v3 && !self.state.frame.buffer_done) {
            if self.state.frame.failed {
                frame.destroy();
                bail!("screencopy failed");
            }
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        let fmt = self.state.frame.format.ok_or_else(|| anyhow!("no buffer format"))?;
        if self.buffer.as_ref().is_none_or(|b| b.3 != fmt) {
            if let Some((pool, buffer, _, _)) = self.buffer.take() {
                buffer.destroy();
                pool.destroy();
            }
            let (format, width, height, stride) = fmt;
            let size = (stride * height) as usize;
            let fd = rustix::fs::memfd_create("mandara-record", rustix::fs::MemfdFlags::CLOEXEC)?;
            rustix::fs::ftruncate(&fd, size as u64)?;
            let pool = self.shm.create_pool(fd.as_fd(), size as i32, &self.qh, ());
            let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, &self.qh, ());
            self.buffer = Some((pool, buffer, std::fs::File::from(fd), fmt));
            self.data = vec![0; size];
        }
        let Some((_, buffer, file, (format, width, height, stride))) = self.buffer.as_ref() else { bail!("no buffer") };
        frame.copy(buffer);
        while !self.state.frame.ready {
            if self.state.frame.failed {
                frame.destroy();
                bail!("screencopy copy failed");
            }
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        frame.destroy();
        std::os::unix::fs::FileExt::read_exact_at(file, &mut self.data, 0)?;
        let bgr = match format {
            wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 => true,
            wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => false,
            other => bail!("unsupported shm format {other:?}"),
        };
        Ok(RawFrame { data: &self.data, width: *width, height: *height, stride: *stride, bgr, y_invert: self.state.frame.y_invert })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_xrgb_and_inverts() {
        // 2×2 XRGB8888 (memory order B,G,R,X), stride 8.
        let data = [10, 20, 30, 0, 40, 50, 60, 0, 70, 80, 90, 0, 100, 110, 120, 0];
        let rgba = to_rgba(wl_shm::Format::Xrgb8888, 2, 2, 8, &data, false).unwrap();
        assert_eq!(&rgba[..4], &[30, 20, 10, 255]);
        let flipped = to_rgba(wl_shm::Format::Xrgb8888, 2, 2, 8, &data, true).unwrap();
        assert_eq!(&flipped[..4], &[90, 80, 70, 255]);
        assert!(to_rgba(wl_shm::Format::Rgb565, 2, 2, 8, &data, false).is_err());
    }

    #[test]
    fn crops_in_logical_space() {
        let cap = Capture { output: "X".into(), logical: (0, 0, 2, 2), width: 4, height: 4, rgba: vec![255; 64] };
        assert_eq!(cap.scale(), 2.0);
        let img = cap.crop_logical(0.5, 0.5, 1.0, 1.0);
        assert_eq!(img.dimensions(), (2, 2));
    }

    #[test]
    fn live_capture_if_available() {
        if std::env::var_os("WAYLAND_DISPLAY").is_some()
            && let Ok(caps) = capture_all()
        {
            assert!(caps.iter().all(|c| c.rgba.len() == (c.width * c.height * 4) as usize));
        }
    }
}
