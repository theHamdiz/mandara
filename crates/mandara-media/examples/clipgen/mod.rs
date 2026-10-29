//! Synthetic AV1 test clips, encoded in pure Rust with rav1e: drifting
//! aurora bands with fine procedural detail (so the encoder has to spend
//! real bits), written as IVF or WebM. Shared by the `make_test_clip`
//! example and the integration tests.

#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use rav1e::prelude::*;

pub struct ClipSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u32,
    /// 0 = smooth gradients only; higher adds procedural texture.
    pub detail: u8,
    pub webm: bool,
    /// rav1e speed preset (0 slowest … 10 fastest).
    pub speed: u8,
}

/// BT.709 limited-range 4:2:0 planes of frame `i`.
fn frame_planes(spec: &ClipSpec, i: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (spec.width as usize, spec.height as usize);
    let t = i as f32 / spec.fps.max(1) as f32;
    let mut rgb = vec![[0f32; 3]; w * h];
    let mut seed = 0x9E37_79B9u32 ^ i.wrapping_mul(2654435761);
    for y in 0..h {
        let fy = y as f32 / h as f32;
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let band1 = (fx * 3.1 + (fy * 2.0 + t * 0.35).sin() * 0.8 + t * 0.2).sin() * 0.5 + 0.5;
            let band2 = (fy * 4.3 - fx * 1.7 + t * 0.5).cos() * 0.5 + 0.5;
            let glow = (-((fx - 0.5 - (t * 0.3).sin() * 0.25).powi(2) + (fy - 0.45).powi(2)) * 6.0).exp();
            let mut r = 0.05 + 0.25 * band2 + 0.55 * glow;
            let mut g = 0.08 + 0.55 * band1 * (1.0 - fy) + 0.15 * glow;
            let mut b = 0.22 + 0.5 * band2 * band1 + 0.25 * fy;
            if spec.detail > 0 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let n = (seed & 0xFF) as f32 / 255.0 - 0.5;
                let ripple = ((fx * 220.0 + t * 3.0).sin() * (fy * 170.0 - t * 2.0).cos()) * 0.5;
                let k = spec.detail as f32 * 0.03;
                r += (n + ripple) * k;
                g += (n * 0.8 + ripple) * k;
                b += (n * 0.6 - ripple) * k;
            }
            rgb[y * w + x] = [r.clamp(0.0, 1.0) * 255.0, g.clamp(0.0, 1.0) * 255.0, b.clamp(0.0, 1.0) * 255.0];
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let mut yp = vec![0u8; w * h];
    let mut up = vec![0u8; cw * ch];
    let mut vp = vec![0u8; cw * ch];
    for (i, p) in rgb.iter().enumerate() {
        let [r, g, b] = *p;
        yp[i] = (16.0 + 0.1826 * r + 0.6142 * g + 0.0620 * b).round().clamp(16.0, 235.0) as u8;
    }
    for cy in 0..ch {
        for cx in 0..cw {
            let mut acc = [0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = rgb[(cy * 2 + dy) * w + cx * 2 + dx];
                for k in 0..3 {
                    acc[k] += p[k] / 4.0;
                }
            }
            let [r, g, b] = acc;
            up[cy * cw + cx] = (128.0 - 0.1006 * r - 0.3386 * g + 0.4392 * b).round().clamp(16.0, 240.0) as u8;
            vp[cy * cw + cx] = (128.0 + 0.4392 * r - 0.3989 * g - 0.0403 * b).round().clamp(16.0, 240.0) as u8;
        }
    }
    (yp, up, vp)
}

/// Encodes the clip to `path`.
pub fn write_clip(path: &Path, spec: &ClipSpec) -> Result<(), String> {
    let (w, h) = (spec.width as usize, spec.height as usize);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let enc = EncoderConfig {
        width: w,
        height: h,
        time_base: Rational::new(1, spec.fps as u64),
        speed_settings: SpeedSettings::from_preset(spec.speed),
        tiles: threads.min(16),
        quantizer: 90,
        max_key_frame_interval: (spec.fps * 4) as u64,
        pixel_range: PixelRange::Limited,
        color_description: Some(ColorDescription {
            color_primaries: ColorPrimaries::BT709,
            transfer_characteristics: TransferCharacteristics::BT709,
            matrix_coefficients: MatrixCoefficients::BT709,
        }),
        ..EncoderConfig::with_speed_preset(spec.speed)
    };
    let mut ctx: Context<u8> = Config::new().with_encoder_config(enc).with_threads(threads).new_context().map_err(|e| format!("{e:?}"))?;
    let mut packets: Vec<(Vec<u8>, u64, bool)> = Vec::new();
    let drain = |ctx: &mut Context<u8>, packets: &mut Vec<(Vec<u8>, u64, bool)>| -> Result<bool, String> {
        loop {
            match ctx.receive_packet() {
                Ok(p) => packets.push((p.data, p.input_frameno, p.frame_type == FrameType::KEY)),
                Err(EncoderStatus::Encoded) => {}
                Err(EncoderStatus::NeedMoreData) => return Ok(false),
                Err(EncoderStatus::LimitReached) => return Ok(true),
                Err(e) => return Err(format!("{e:?}")),
            }
        }
    };
    for i in 0..spec.frames {
        let (y, u, v) = frame_planes(spec, i);
        let mut f = ctx.new_frame();
        f.planes[0].copy_from_raw_u8(&y, w, 1);
        f.planes[1].copy_from_raw_u8(&u, w / 2, 1);
        f.planes[2].copy_from_raw_u8(&v, w / 2, 1);
        ctx.send_frame(f).map_err(|e| format!("{e:?}"))?;
        drain(&mut ctx, &mut packets)?;
    }
    ctx.flush();
    while !drain(&mut ctx, &mut packets)? {}

    if spec.webm {
        let mut mux = mandara_services::recording::WebmWriter::create(path, spec.width, spec.height, &ctx.container_sequence_header()).map_err(|e| e.to_string())?;
        for (data, n, key) in &packets {
            mux.write_frame(data, Duration::from_secs_f64(*n as f64 / spec.fps as f64), *key).map_err(|e| e.to_string())?;
        }
        mux.finish(Duration::from_secs_f64(spec.frames as f64 / spec.fps as f64)).map_err(|e| e.to_string())?;
    } else {
        let mut out = mandara_media::ivf::header(spec.width as u16, spec.height as u16, spec.fps, spec.frames).to_vec();
        for (data, n, _) in &packets {
            out.extend(mandara_media::ivf::frame_header(data.len() as u32, *n));
            out.extend_from_slice(data);
        }
        std::fs::write(path, out).map_err(|e| e.to_string())?;
    }
    Ok(())
}
