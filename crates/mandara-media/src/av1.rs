//! A safe wrapper over `rav1d` (the pure-Rust port of dav1d), built without
//! its assembly so no C toolchain or nasm is involved.
//!
//! rav1d exposes dav1d's C-shaped API as Rust functions; this module owns
//! the context and picture references and turns them into plain values.

use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::time::Duration;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{
    DAV1D_INLOOPFILTER_ALL, DAV1D_INLOOPFILTER_DEBLOCK, Dav1dContext, Dav1dSettings,
};
use rav1d::include::dav1d::headers::{
    DAV1D_MC_BT470BG, DAV1D_MC_BT601, DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};

use super::yuv::{Crop, Layout, Matrix, Sample, Target, YuvFrame, convert_region, to_bgra};

const EAGAIN: i32 = -libc::EAGAIN;

/// Upper bound for automatic (`threads: 0`) worker counts.
const MAX_THREADS: usize = 16;
/// dav1d's largest accepted `max_frame_delay`.
const MAX_FRAME_DELAY: u32 = 256;

/// Decoder tuning.
#[derive(Copy, Clone, Debug)]
pub struct DecoderSettings {
    /// Worker threads (0 = one per core).
    pub threads: usize,
    /// Faster, slightly softer decoding for slow machines: CDEF and loop
    /// restoration off, no film-grain synthesis.
    pub lite: bool,
    /// Frames decoded in parallel (0 = automatic, from the thread count).
    /// Every frame in flight holds a picture, so a low value saves memory.
    pub frame_delay: u32,
}

impl Default for DecoderSettings {
    fn default() -> Self {
        Self { threads: 0, lite: false, frame_delay: 0 }
    }
}

pub struct Av1Decoder {
    ctx: Option<Dav1dContext>,
}

/// A decoded picture (holds a reference into the decoder's pool).
pub struct Picture {
    pic: Dav1dPicture,
}

impl Av1Decoder {
    pub fn new(settings: DecoderSettings) -> Result<Self, String> {
        let mut s = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` fully initializes the struct.
        let mut s = unsafe {
            dav1d_default_settings(NonNull::new_unchecked(s.as_mut_ptr()));
            s.assume_init()
        };
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        s.n_threads = if settings.threads == 0 { cores.min(MAX_THREADS) } else { settings.threads } as i32;
        // 0 = automatic frame delay, so frames decode in parallel.
        s.max_frame_delay = settings.frame_delay.min(MAX_FRAME_DELAY) as i32;
        if settings.lite {
            s.inloop_filters = DAV1D_INLOOPFILTER_DEBLOCK;
            s.apply_grain = 0;
        } else {
            s.inloop_filters = DAV1D_INLOOPFILTER_ALL;
        }
        let mut ctx: Option<Dav1dContext> = None;
        // SAFETY: both pointers are valid for the call's duration.
        let res = unsafe { dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut s))) };
        if res.0 != 0 || ctx.is_none() {
            return Err(format!("AV1 decoder failed to start (error {})", res.0));
        }
        Ok(Self { ctx })
    }

    /// Feeds one temporal unit and returns every picture that became ready.
    pub fn decode(&mut self, packet: &[u8], pts: Duration) -> Result<Vec<Picture>, String> {
        let mut out = Vec::new();
        if packet.is_empty() {
            return Ok(out);
        }
        let mut data = Dav1dData::default();
        // SAFETY: `data` is a valid, writable Dav1dData; the returned buffer
        // has `packet.len()` writable bytes.
        unsafe {
            let buf = dav1d_data_create(Some(NonNull::from(&mut data)), packet.len());
            if buf.is_null() {
                return Err("out of memory".into());
            }
            std::ptr::copy_nonoverlapping(packet.as_ptr(), buf, packet.len());
        }
        data.m.timestamp = pts.as_nanos() as i64;
        let mut guard = 0;
        while data.sz > 0 {
            // SAFETY: ctx is open; data was created by dav1d_data_create.
            let res = unsafe { dav1d_send_data(self.ctx, Some(NonNull::from(&mut data))) };
            if res.0 != 0 && res.0 != EAGAIN {
                // SAFETY: releases what's left of our data reference.
                unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
                return Err(format!("AV1 decode error {}", res.0));
            }
            match self.picture()? {
                Some(p) => out.push(p),
                None if res.0 == EAGAIN => {
                    guard += 1;
                    if guard > 1000 {
                        // SAFETY: as above.
                        unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
                        return Err("AV1 decoder stalled".into());
                    }
                }
                None => {}
            }
        }
        while let Some(p) = self.picture()? {
            out.push(p);
        }
        Ok(out)
    }

    /// Pictures still inside the decoder at end of stream.
    pub fn drain(&mut self) -> Result<Vec<Picture>, String> {
        let mut out = Vec::new();
        for _ in 0..256 {
            match self.picture()? {
                Some(p) => out.push(p),
                None => break,
            }
        }
        Ok(out)
    }

    fn picture(&mut self) -> Result<Option<Picture>, String> {
        let mut pic = Dav1dPicture::default();
        // SAFETY: ctx is open; pic is valid to write.
        let res = unsafe { dav1d_get_picture(self.ctx, Some(NonNull::from(&mut pic))) };
        match res.0 {
            0 => Ok(Some(Picture { pic })),
            EAGAIN => Ok(None),
            e => Err(format!("AV1 decode error {e}")),
        }
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        if self.ctx.is_some() {
            // SAFETY: ctx came from dav1d_open and is closed exactly once.
            unsafe { dav1d_close(Some(NonNull::from(&mut self.ctx))) };
        }
    }
}

// SAFETY: a picture is a reference-counted handle into rav1d's picture pool
// (atomic `Arc`s inside); dav1d's API allows pictures to be read and
// released on any thread, independently of the decoder context.
unsafe impl Send for Picture {}

// SAFETY: every `&self` method only reads the decoded planes and headers,
// which rav1d never mutates once a picture is output; shared readers on
// several threads (one per output converting the same frame) are fine.
unsafe impl Sync for Picture {}

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: pic was filled by dav1d_get_picture and is released once.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut self.pic))) };
    }
}

impl Picture {
    pub fn width(&self) -> usize {
        self.pic.p.w.max(0) as usize
    }

    pub fn height(&self) -> usize {
        self.pic.p.h.max(0) as usize
    }

    pub fn pts(&self) -> Duration {
        Duration::from_nanos(self.pic.m.timestamp.max(0) as u64)
    }

    pub fn bits(&self) -> u32 {
        self.pic.p.bpc.max(8) as u32
    }

    fn layout(&self) -> Layout {
        match self.pic.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => Layout::I400,
            DAV1D_PIXEL_LAYOUT_I420 => Layout::I420,
            DAV1D_PIXEL_LAYOUT_I422 => Layout::I422,
            _ => Layout::I444,
        }
    }

    /// (matrix, full range) from the sequence header; BT.709 limited when
    /// unspecified (what encoders assume for HD content).
    fn color(&self) -> (Matrix, bool) {
        let Some(seq) = self.pic.seq_hdr else { return (Matrix::Bt709, false) };
        // SAFETY: the sequence header lives as long as the picture reference.
        let seq = unsafe { seq.as_ref() };
        let matrix = match seq.mtrx {
            DAV1D_MC_BT470BG | DAV1D_MC_BT601 => Matrix::Bt601,
            9 | 10 => Matrix::Bt2020,
            _ => Matrix::Bt709,
        };
        (matrix, seq.color_range != 0)
    }

    /// Hands the picture's planes (8-bit or high bit depth) to `visit`;
    /// `None` for an empty picture.
    fn planes<V: Visit>(&self, visit: V) -> Option<V::Out> {
        let (w, h) = (self.width(), self.height());
        if w == 0 || h == 0 {
            return None;
        }
        let layout = self.layout();
        let (matrix, full_range) = self.color();
        let ch = match layout {
            Layout::I420 => h.div_ceil(2),
            _ => h,
        };
        let (ys, cs) = (self.pic.stride[0].max(0) as usize, self.pic.stride[1].max(0) as usize);
        let plane = |i: usize, bytes: usize| -> Option<NonNull<u8>> { if bytes == 0 { None } else { self.pic.data[i].map(|p| p.cast::<u8>()) } };
        let y_bytes = ys * h;
        let c_bytes = if layout == Layout::I400 { 0 } else { cs * ch };
        let y = plane(0, y_bytes)?;
        let (u, v) = (plane(1, c_bytes), plane(2, c_bytes));
        if self.bits() > 8 {
            // SAFETY: dav1d guarantees `stride * rows` bytes per plane,
            // 2-byte aligned for high bit depth, alive as long as `self`.
            let (yp, up, vp) = unsafe {
                (
                    std::slice::from_raw_parts(y.as_ptr() as *const u16, y_bytes / 2),
                    u.map_or(&[][..], |p| std::slice::from_raw_parts(p.as_ptr() as *const u16, c_bytes / 2)),
                    v.map_or(&[][..], |p| std::slice::from_raw_parts(p.as_ptr() as *const u16, c_bytes / 2)),
                )
            };
            Some(visit.frame(&YuvFrame { width: w, height: h, layout, y: yp, u: up, v: vp, y_stride: ys / 2, uv_stride: cs / 2, bits: self.bits(), full_range, matrix }))
        } else {
            // SAFETY: as above, for 8-bit planes.
            let (yp, up, vp) = unsafe {
                (
                    std::slice::from_raw_parts(y.as_ptr() as *const u8, y_bytes),
                    u.map_or(&[][..], |p| std::slice::from_raw_parts(p.as_ptr() as *const u8, c_bytes)),
                    v.map_or(&[][..], |p| std::slice::from_raw_parts(p.as_ptr() as *const u8, c_bytes)),
                )
            };
            Some(visit.frame(&YuvFrame { width: w, height: h, layout, y: yp, u: up, v: vp, y_stride: ys, uv_stride: cs, bits: 8, full_range, matrix }))
        }
    }

    /// Converts to BGRA (optionally at half resolution).
    pub fn to_bgra(&self, half: bool) -> Option<(Vec<u8>, usize, usize)> {
        self.planes(ToBgra(half))
    }

    /// Converts the source rectangle `crop` straight into `dst` (scaled to
    /// its size: 1:1, an exact 2× box filter, or nearest sampling) using up
    /// to `threads` threads. `false` for an empty picture.
    pub fn convert_region(&self, crop: Crop, dst: &mut Target<'_>, threads: usize) -> bool {
        self.planes(Region { crop, dst, threads }).is_some()
    }

    /// The whole picture as a straight-alpha RGBA image (opaque).
    pub fn to_rgba_image(&self) -> Option<image::RgbaImage> {
        let (w, h) = (self.width() as u32, self.height() as u32);
        let mut data = vec![0u8; w as usize * h as usize * 4];
        let mut dst = Target { data: &mut data, width: w, height: h, stride: w as usize * 4, order: super::yuv::PixelOrder::Rgba };
        if !self.convert_region(Crop { x: 0, y: 0, w, h }, &mut dst, 1) {
            return None;
        }
        image::RgbaImage::from_raw(w, h, data)
    }
}

/// Something done with a picture's planes, at either sample depth.
trait Visit {
    type Out;
    fn frame<S: Sample>(self, f: &YuvFrame<'_, S>) -> Self::Out;
}

/// Whole-frame BGRA conversion (optionally at half resolution).
struct ToBgra(bool);

impl Visit for ToBgra {
    type Out = (Vec<u8>, usize, usize);
    fn frame<S: Sample>(self, f: &YuvFrame<'_, S>) -> Self::Out {
        to_bgra(f, self.0)
    }
}

/// Region conversion into a caller's buffer.
struct Region<'a, 'b> {
    crop: Crop,
    dst: &'a mut Target<'b>,
    threads: usize,
}

impl Visit for Region<'_, '_> {
    type Out = ();
    fn frame<S: Sample>(self, f: &YuvFrame<'_, S>) {
        convert_region(f, self.crop, self.dst, self.threads);
    }
}
