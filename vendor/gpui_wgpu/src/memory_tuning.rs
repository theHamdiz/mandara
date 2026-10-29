//! Process-wide knobs for the GPU memory every window's renderer allocates
//! (Mandara patch, see `MANDARA-PATCHES.md`).
//!
//! Each knob resolves as: environment variable (for experiments) > value set by
//! the application through the setter > upstream default. Setters only affect
//! renderers created afterwards, so applications call them before opening the
//! first window.

use std::sync::atomic::{AtomicU32, Ordering};

/// Environment override for [`set_path_sample_count`] (`1`, `2` or `4`).
pub const PATH_SAMPLE_COUNT_ENV: &str = "GPUI_PATH_SAMPLE_COUNT";
/// Environment override for [`set_present_mode`]
/// (`fifo`, `fifo-relaxed`, `mailbox`, `immediate`).
pub const PRESENT_MODE_ENV: &str = "GPUI_PRESENT_MODE";
/// Environment override for [`set_max_frame_latency`] (frames, ≥ 1).
pub const MAX_FRAME_LATENCY_ENV: &str = "GPUI_MAX_FRAME_LATENCY";

/// Upstream's MSAA sample count for path rasterization.
pub const DEFAULT_PATH_SAMPLE_COUNT: u32 = 4;
/// Upstream's `desired_maximum_frame_latency`, in frames.
pub const DEFAULT_MAX_FRAME_LATENCY: u32 = 2;

/// Sentinel for "not set by the application".
const UNSET: u32 = 0;

static PATH_SAMPLE_COUNT: AtomicU32 = AtomicU32::new(UNSET);
static PRESENT_MODE: AtomicU32 = AtomicU32::new(UNSET);
static MAX_FRAME_LATENCY: AtomicU32 = AtomicU32::new(UNSET);

/// MSAA sample count used to rasterize paths (`1` disables MSAA; curves stay
/// analytically anti-aliased, straight tessellated edges do not). The renderer
/// uses the largest count ≤ `count` the device supports (WebGPU guarantees 1
/// and 4; 2 and 8 need `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, which GPUI
/// does not request, so `2` means `1` there). The MSAA target costs
/// `count × 4` bytes per pixel of the paths' bounding box.
pub fn set_path_sample_count(count: u32) {
    PATH_SAMPLE_COUNT.store(count.max(1), Ordering::Relaxed);
}

/// Presentation mode for every new surface, overriding the platform's
/// preference (`None` restores it). The swapchain holds
/// `max(max_frame_latency + 1, driver minimum for the mode)` images; Mesa's
/// Wayland WSI needs 4 for `Mailbox` but only 3 for `Fifo`.
pub fn set_present_mode(mode: Option<wgpu::PresentMode>) {
    PRESENT_MODE.store(mode.map_or(UNSET, encode_present_mode), Ordering::Relaxed);
}

/// `desired_maximum_frame_latency` for new surfaces, in frames (≥ 1).
pub fn set_max_frame_latency(frames: u32) {
    MAX_FRAME_LATENCY.store(frames.max(1), Ordering::Relaxed);
}

pub(crate) fn path_sample_count() -> u32 {
    env_u32(PATH_SAMPLE_COUNT_ENV)
        .or_else(|| stored(&PATH_SAMPLE_COUNT))
        .unwrap_or(DEFAULT_PATH_SAMPLE_COUNT)
        .max(1)
}

pub(crate) fn present_mode_override() -> Option<wgpu::PresentMode> {
    std::env::var(PRESENT_MODE_ENV)
        .ok()
        .and_then(|value| parse_present_mode(&value))
        .or_else(|| stored(&PRESENT_MODE).and_then(decode_present_mode))
}

pub(crate) fn max_frame_latency() -> u32 {
    env_u32(MAX_FRAME_LATENCY_ENV)
        .or_else(|| stored(&MAX_FRAME_LATENCY))
        .unwrap_or(DEFAULT_MAX_FRAME_LATENCY)
        .max(1)
}

fn stored(knob: &AtomicU32) -> Option<u32> {
    Some(knob.load(Ordering::Relaxed)).filter(|&value| value != UNSET)
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn parse_present_mode(value: &str) -> Option<wgpu::PresentMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fifo" => Some(wgpu::PresentMode::Fifo),
        "fifo-relaxed" | "fifo_relaxed" => Some(wgpu::PresentMode::FifoRelaxed),
        "mailbox" => Some(wgpu::PresentMode::Mailbox),
        "immediate" => Some(wgpu::PresentMode::Immediate),
        _ => None,
    }
}

fn encode_present_mode(mode: wgpu::PresentMode) -> u32 {
    match mode {
        wgpu::PresentMode::Fifo => 1,
        wgpu::PresentMode::FifoRelaxed => 2,
        wgpu::PresentMode::Mailbox => 3,
        wgpu::PresentMode::Immediate => 4,
        wgpu::PresentMode::AutoVsync => 5,
        wgpu::PresentMode::AutoNoVsync => 6,
    }
}

fn decode_present_mode(code: u32) -> Option<wgpu::PresentMode> {
    Some(match code {
        1 => wgpu::PresentMode::Fifo,
        2 => wgpu::PresentMode::FifoRelaxed,
        3 => wgpu::PresentMode::Mailbox,
        4 => wgpu::PresentMode::Immediate,
        5 => wgpu::PresentMode::AutoVsync,
        6 => wgpu::PresentMode::AutoNoVsync,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_mode_codes_round_trip() {
        for mode in [
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::FifoRelaxed,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::AutoVsync,
            wgpu::PresentMode::AutoNoVsync,
        ] {
            assert_eq!(decode_present_mode(encode_present_mode(mode)), Some(mode));
        }
        assert_eq!(decode_present_mode(UNSET), None);
    }
}
