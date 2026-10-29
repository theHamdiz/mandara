//! Window thumbnails over `ext-image-copy-capture-v1` with
//! `ext-foreign-toplevel-image-capture-source-v1` sources: each window's
//! own contents (even when covered or on another workspace), copied into a
//! shared-memory buffer and downscaled. Used by the Alt-Tab switcher.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, event_created_child};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{ext_foreign_toplevel_handle_v1 as ext_handle, ext_foreign_toplevel_list_v1 as ext_list};
use wayland_protocols::ext::image_capture_source::v1::client::{ext_foreign_toplevel_image_capture_source_manager_v1 as source_manager, ext_image_capture_source_v1 as source};
use wayland_protocols::ext::image_copy_capture::v1::client::{ext_image_copy_capture_frame_v1 as frame, ext_image_copy_capture_manager_v1 as copy_manager, ext_image_copy_capture_session_v1 as session};

/// A whole batch gives up after this long (a stuck window never blocks the
/// switcher).
const BATCH_TIMEOUT: Duration = Duration::from_millis(1500);
/// Windows copied at the same time (each holds a full-size buffer until it
/// is downscaled).
const CONCURRENT: usize = 2;
/// Bytes per pixel of the 32-bit shm formats used.
const BYTES_PER_PX: u32 = 4;

/// A window to capture, identified the way both window lists report it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub app_id: String,
    pub title: String,
}

/// Matches targets to listed windows: same app id and title, the n-th
/// target with a given pair taking the n-th window with it.
pub fn match_targets(targets: &[Target], listed: &[(String, String)]) -> Vec<Option<usize>> {
    let mut used = vec![false; listed.len()];
    targets
        .iter()
        .map(|t| {
            let hit = listed.iter().enumerate().position(|(i, (a, ti))| !used[i] && *a == t.app_id && *ti == t.title).or_else(|| {
                // Titles change fast (terminals, players): fall back to the app id.
                listed.iter().enumerate().position(|(i, (a, _))| !used[i] && *a == t.app_id)
            });
            if let Some(i) = hit {
                used[i] = true;
            }
            hit
        })
        .collect()
}

/// Largest size that fits in `max_w × max_h` keeping the aspect ratio.
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    let k = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1))
}

#[derive(Default)]
struct ListedWindow {
    app_id: String,
    title: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Waiting,
    Ready,
    Failed,
}

#[derive(Default)]
struct Job {
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    stopped: bool,
    phase: Phase,
}

struct State {
    listed: Vec<(ext_handle::ExtForeignToplevelHandleV1, ListedWindow)>,
    jobs: Vec<Job>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
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
impl Dispatch<ext_list::ExtForeignToplevelListV1, ()> for State {
    fn event(state: &mut Self, _: &ext_list::ExtForeignToplevelListV1, event: ext_list::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_list::Event::Toplevel { toplevel } = event {
            state.listed.push((toplevel, ListedWindow::default()));
        }
    }

    event_created_child!(State, ext_list::ExtForeignToplevelListV1, [
        ext_list::EVT_TOPLEVEL_OPCODE => (ext_handle::ExtForeignToplevelHandleV1, ()),
    ]);
}
impl Dispatch<ext_handle::ExtForeignToplevelHandleV1, ()> for State {
    fn event(state: &mut Self, proxy: &ext_handle::ExtForeignToplevelHandleV1, event: ext_handle::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some((_, w)) = state.listed.iter_mut().find(|(h, _)| h == proxy) else { return };
        match event {
            ext_handle::Event::AppId { app_id } => w.app_id = app_id,
            ext_handle::Event::Title { title } => w.title = title,
            _ => {}
        }
    }
}
impl Dispatch<source_manager::ExtForeignToplevelImageCaptureSourceManagerV1, ()> for State {
    fn event(_: &mut Self, _: &source_manager::ExtForeignToplevelImageCaptureSourceManagerV1, _: source_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<source::ExtImageCaptureSourceV1, ()> for State {
    fn event(_: &mut Self, _: &source::ExtImageCaptureSourceV1, _: source::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<copy_manager::ExtImageCopyCaptureManagerV1, ()> for State {
    fn event(_: &mut Self, _: &copy_manager::ExtImageCopyCaptureManagerV1, _: copy_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<session::ExtImageCopyCaptureSessionV1, usize> for State {
    fn event(state: &mut Self, _: &session::ExtImageCopyCaptureSessionV1, event: session::Event, job: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let Some(j) = state.jobs.get_mut(*job) else { return };
        match event {
            session::Event::BufferSize { width, height } => j.size = Some((width, height)),
            session::Event::ShmFormat { format: WEnum::Value(f) } => j.formats.push(f),
            session::Event::Done => j.constraints_done = true,
            session::Event::Stopped => {
                j.stopped = true;
                if j.phase == Phase::Waiting {
                    j.phase = Phase::Failed;
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<frame::ExtImageCopyCaptureFrameV1, usize> for State {
    fn event(state: &mut Self, _: &frame::ExtImageCopyCaptureFrameV1, event: frame::Event, job: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let Some(j) = state.jobs.get_mut(*job) else { return };
        match event {
            frame::Event::Ready => j.phase = Phase::Ready,
            frame::Event::Failed { .. } => j.phase = Phase::Failed,
            _ => {}
        }
    }
}

/// A full-size copy in flight.
struct Copy {
    file: std::fs::File,
    width: u32,
    height: u32,
    format: wl_shm::Format,
    buffer: wl_buffer::WlBuffer,
    pool: wl_shm_pool::WlShmPool,
    frame: frame::ExtImageCopyCaptureFrameV1,
}

fn pick_format(formats: &[wl_shm::Format]) -> Option<wl_shm::Format> {
    [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888, wl_shm::Format::Xbgr8888, wl_shm::Format::Abgr8888].into_iter().find(|f| formats.contains(f))
}

fn start_copy(shm: &wl_shm::WlShm, s: &session::ExtImageCopyCaptureSessionV1, job: &Job, index: usize, qh: &QueueHandle<State>) -> Result<Copy> {
    let (width, height) = job.size.context("no buffer size")?;
    let format = pick_format(&job.formats).context("no usable shm format")?;
    let stride = width * BYTES_PER_PX;
    let len = stride as u64 * u64::from(height);
    let fd = rustix::fs::memfd_create("mandara-window-capture", rustix::fs::MemfdFlags::CLOEXEC)?;
    let file = std::fs::File::from(fd);
    file.set_len(len)?;
    let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, qh, ());
    let frame = s.create_frame(qh, index);
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, width as i32, height as i32);
    frame.capture();
    Ok(Copy { file, width, height, format, buffer, pool, frame })
}

fn finish_copy(mut c: Copy, max: (u32, u32)) -> Result<image::RgbaImage> {
    let stride = c.width * BYTES_PER_PX;
    let mut data = vec![0u8; (stride * c.height) as usize];
    c.file.seek(SeekFrom::Start(0))?;
    c.file.read_exact(&mut data)?;
    c.frame.destroy();
    c.buffer.destroy();
    c.pool.destroy();
    let rgba = crate::screencopy::to_rgba(c.format, c.width, c.height, stride, &data, false)?;
    drop(data);
    let full = image::RgbaImage::from_raw(c.width, c.height, rgba).context("bad frame size")?;
    let (w, h) = fit(c.width, c.height, max.0, max.1);
    Ok(image::imageops::thumbnail(&full, w, h))
}

/// Captures thumbnails (each fitting `max_w × max_h` physical pixels) of
/// `targets`, in order; `None` for windows that couldn't be copied.
/// `on_ready` hears about each thumbnail as soon as it is made.
pub fn capture_windows(targets: &[Target], max: (u32, u32), paint_cursor: bool, mut on_ready: impl FnMut(usize, &image::RgbaImage)) -> Result<Vec<Option<image::RgbaImage>>> {
    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
    let list: ext_list::ExtForeignToplevelListV1 = globals.bind(&qh, 1..=1, ()).context("ext_foreign_toplevel_list_v1")?;
    let sources: source_manager::ExtForeignToplevelImageCaptureSourceManagerV1 = globals.bind(&qh, 1..=1, ()).context("ext_foreign_toplevel_image_capture_source_manager_v1")?;
    let copier: copy_manager::ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).context("ext_image_copy_capture_manager_v1")?;
    let mut state = State { listed: Vec::new(), jobs: Vec::new() };
    // Two round trips: the handles, then their app id / title / done.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    let listed: Vec<(String, String)> = state.listed.iter().map(|(_, w)| (w.app_id.clone(), w.title.clone())).collect();
    let matches = match_targets(targets, &listed);
    let options = if paint_cursor { copy_manager::Options::PaintCursors } else { copy_manager::Options::empty() };
    let mut out: Vec<Option<image::RgbaImage>> = vec![None; targets.len()];
    let deadline = Instant::now() + BATCH_TIMEOUT;
    // Process in small waves so only a few full-size buffers exist at once.
    let wanted: Vec<(usize, usize)> = matches.iter().enumerate().filter_map(|(t, m)| m.map(|i| (t, i))).collect();
    for wave in wanted.chunks(CONCURRENT) {
        let mut sessions = Vec::new();
        for (t, listed_index) in wave {
            let index = state.jobs.len();
            state.jobs.push(Job::default());
            let src = sources.create_source(&state.listed[*listed_index].0, &qh, ());
            let s = copier.create_session(&src, options, &qh, index);
            sessions.push((*t, index, src, s, None::<Copy>));
        }
        loop {
            if Instant::now() > deadline {
                break;
            }
            queue.roundtrip(&mut state)?;
            for (_, index, _, s, copy) in sessions.iter_mut() {
                let job = &state.jobs[*index];
                if copy.is_none() && job.constraints_done && job.phase == Phase::Waiting && !job.stopped {
                    match start_copy(&shm, s, job, *index, &qh) {
                        Ok(c) => *copy = Some(c),
                        Err(err) => {
                            tracing::debug!("window capture: {err:#}");
                            state.jobs[*index].phase = Phase::Failed;
                        }
                    }
                }
            }
            if sessions.iter().all(|(_, index, ..)| state.jobs[*index].phase != Phase::Waiting) {
                break;
            }
        }
        for (t, index, src, s, copy) in sessions {
            if state.jobs[index].phase == Phase::Ready
                && let Some(c) = copy
            {
                match finish_copy(c, max) {
                    Ok(img) => {
                        on_ready(t, &img);
                        out[t] = Some(img);
                    }
                    Err(err) => tracing::debug!("window capture: {err:#}"),
                }
            } else if let Some(c) = copy {
                c.frame.destroy();
                c.buffer.destroy();
                c.pool.destroy();
            }
            s.destroy();
            src.destroy();
        }
    }
    for (h, _) in &state.listed {
        h.destroy();
    }
    list.stop();
    let _ = conn.flush();
    if targets.iter().zip(&out).all(|(_, o)| o.is_none()) && !targets.is_empty() && wanted.is_empty() {
        bail!("no listed window matched");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(a: &str, ti: &str) -> Target {
        Target { app_id: a.into(), title: ti.into() }
    }

    #[test]
    fn targets_match_by_app_and_title_then_app() {
        let listed = vec![("term".to_string(), "a".to_string()), ("term".to_string(), "b".to_string()), ("web".to_string(), "x".to_string())];
        assert_eq!(match_targets(&[target("term", "b"), target("term", "zzz"), target("web", "y"), target("nope", "")], &listed), vec![Some(1), Some(0), Some(2), None]);
    }

    #[test]
    fn thumbnails_keep_aspect() {
        assert_eq!(fit(3840, 2160, 480, 300), (480, 270));
        assert_eq!(fit(100, 400, 480, 300), (75, 300));
        assert_eq!(fit(200, 100, 480, 300), (200, 100), "never upscales");
        assert_eq!(fit(0, 10, 4, 4), (1, 1));
    }
}
