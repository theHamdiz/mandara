//! Short sounds straight to FreeBSD's OSS device (`/dev/dsp`, the default
//! unit `hw.snd.default_unit` points it at) when no PulseAudio / PipeWire
//! server runs: the device is opened for one sound, set to signed 16-bit
//! little-endian at the clip's channels and rate, written and closed, so
//! nothing holds it between sounds.
//!
//! The ioctl numbers and the volume scaling are plain functions (tested on
//! every build host); the device calls are FreeBSD only.

/// `AFMT_S16_LE` from `sys/soundcard.h`.
pub const AFMT_S16_LE: i32 = 0x0000_0010;
/// The default playback device.
pub const DEVICE: &str = "/dev/dsp";

/// FreeBSD `_IOWR(group, num, int)` (`sys/ioccom.h`): in and out, the
/// argument's size in bits 16–28, the group and the number.
const fn iowr_int(group: u8, num: u8) -> u64 {
    const IOC_INOUT: u64 = 0x4000_0000 | 0x8000_0000;
    const IOCPARM_MASK: u64 = 0x1fff;
    const INT_BYTES: u64 = std::mem::size_of::<i32>() as u64;
    IOC_INOUT | ((INT_BYTES & IOCPARM_MASK) << 16) | ((group as u64) << 8) | num as u64
}

/// `SNDCTL_DSP_SPEED` = `_IOWR('P', 2, int)`.
pub const SNDCTL_DSP_SPEED: u64 = iowr_int(b'P', 2);
/// `SNDCTL_DSP_SETFMT` = `_IOWR('P', 5, int)`.
pub const SNDCTL_DSP_SETFMT: u64 = iowr_int(b'P', 5);
/// `SNDCTL_DSP_CHANNELS` = `_IOWR('P', 6, int)`.
pub const SNDCTL_DSP_CHANNELS: u64 = iowr_int(b'P', 6);

/// Writes `samples` scaled by `volume` (0–1) into `out` as little-endian
/// bytes. `out` is cleared first and reused between sounds. (The same
/// scaling the sound-server path bakes into its uploads.)
pub fn scaled(samples: &[i16], volume: f32, out: &mut Vec<u8>) {
    crate::sounds::scaled_le(samples, volume, out);
}

/// Plays interleaved `samples` (`channels`, `rate`) at `volume` on the
/// default device, blocking until the device took them all. `buf` is
/// scratch space kept by the caller.
#[cfg(target_os = "freebsd")]
pub fn play(samples: &[i16], channels: u8, rate: u32, volume: f32, buf: &mut Vec<u8>) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    let mut dev = std::fs::OpenOptions::new().write(true).open(DEVICE).map_err(|e| format!("{DEVICE}: {e}"))?;
    let fd = dev.as_raw_fd();
    let set = |request: u64, value: i32| -> Result<i32, String> {
        let mut v = value;
        // SAFETY: an OSS ioctl on an open descriptor with a valid pointer to
        // an int, as `sys/soundcard.h` declares these requests.
        let rc = unsafe { libc::ioctl(fd, request as libc::c_ulong, &mut v as *mut i32) };
        if rc < 0 { Err(std::io::Error::last_os_error().to_string()) } else { Ok(v) }
    };
    if set(SNDCTL_DSP_SETFMT, AFMT_S16_LE)? != AFMT_S16_LE {
        return Err("the sound device doesn't take 16-bit samples".into());
    }
    let got = set(SNDCTL_DSP_CHANNELS, i32::from(channels))?;
    if got != i32::from(channels) {
        return Err(format!("the sound device can't play {channels} channels"));
    }
    // The device picks the nearest rate it supports; small differences are
    // inaudible for short cues.
    set(SNDCTL_DSP_SPEED, i32::try_from(rate).map_err(|e| e.to_string())?)?;
    scaled(samples, volume, buf);
    dev.write_all(buf).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_sys_soundcard_h() {
        // `_IOWR('P', n, int)` from FreeBSD's `sys/ioccom.h`; the same values
        // as the OSS headers on Linux.
        assert_eq!(SNDCTL_DSP_SPEED, 0xc004_5002);
        assert_eq!(SNDCTL_DSP_SETFMT, 0xc004_5005);
        assert_eq!(SNDCTL_DSP_CHANNELS, 0xc004_5006);
    }

    #[test]
    fn volume_scales_and_reuses_the_buffer() {
        let mut buf = Vec::new();
        scaled(&[1000, -1000, i16::MAX], 0.5, &mut buf);
        assert_eq!(buf, [500i16.to_le_bytes(), (-500i16).to_le_bytes(), 16384i16.to_le_bytes()].concat());
        let cap = buf.capacity();
        scaled(&[1, 2], 2.0, &mut buf);
        assert_eq!(buf, [1i16.to_le_bytes(), 2i16.to_le_bytes()].concat(), "clamped to full volume");
        assert_eq!(buf.capacity(), cap, "no new allocation for a shorter sound");
        scaled(&[1, 2], -1.0, &mut buf);
        assert_eq!(buf, vec![0; 4]);
    }
}
