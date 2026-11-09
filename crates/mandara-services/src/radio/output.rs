//! The sound: one playback stream ("Mandara Radio") on the PulseAudio
//! native protocol that PipeWire's `pipewire-pulse` speaks, the way
//! Soundscapes and the Qur'an play, and the per-block renderer that turns
//! decoded frames into the stream's bytes with a smooth volume.
//!
//! [`Renderer::render`] runs for every block the server asks for; it works
//! in buffers made once per session and never allocates (checked by
//! `tests/radio_alloc.rs`).

use pulseaudio::protocol::{self, Command};

/// Output: interleaved stereo f32.
pub const CHANNELS: usize = 2;
pub const BYTES_PER_FRAME: usize = CHANNELS * std::mem::size_of::<f32>();
/// Largest single write to the server (frames).
pub const MAX_WRITE_FRAMES: usize = 4096;
/// The server-side buffer: short, so volume and pause feel immediate.
pub const TARGET_LATENCY: std::time::Duration = std::time::Duration::from_millis(250);
/// Socket read timeout: how often the audio thread looks at the stop flag.
pub const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);
/// Time constant of the volume glide.
const GAIN_GLIDE_S: f32 = 0.04;
/// Fade-in when a stream (re)starts, so it never clicks.
const FADE_IN_S: f32 = 0.03;

/// Turns frames into output bytes with a gliding gain and reports the
/// output level.
pub struct Renderer {
    gain: f32,
    glide: f32,
    fade: u32,
    fade_len: u32,
}

impl Renderer {
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        Self { gain: 0.0, glide: 1.0 - (-1.0 / (GAIN_GLIDE_S * rate)).exp(), fade: 0, fade_len: (FADE_IN_S * rate) as u32 }
    }

    /// Starts the fade-in again (after a reconnect).
    pub fn restart(&mut self) {
        self.fade = 0;
    }

    /// Clears `out` and writes `frames` at `target` gain (0–1) into it.
    /// Returns the block's RMS level before the gain. `out` must hold
    /// `frames.len() * BYTES_PER_FRAME` bytes of capacity to stay
    /// allocation-free.
    pub fn render(&mut self, frames: &[[f32; 2]], target: f32, out: &mut Vec<u8>) -> f32 {
        out.clear();
        let target = target.clamp(0.0, 1.0);
        let mut energy = 0.0f32;
        for f in frames {
            self.gain += (target - self.gain) * self.glide;
            let fade = if self.fade < self.fade_len {
                self.fade += 1;
                self.fade as f32 / self.fade_len as f32
            } else {
                1.0
            };
            let g = self.gain * fade;
            for s in f {
                // The level is the signal's, before the volume: meters show
                // the music, not the slider.
                let c = s.clamp(-1.0, 1.0);
                energy += c * c;
                out.extend_from_slice(&(s * g).clamp(-1.0, 1.0).to_le_bytes());
            }
        }
        if frames.is_empty() { 0.0 } else { (energy / (frames.len() * CHANNELS) as f32).sqrt() }
    }
}

/// A playback stream on the sound server.
pub struct Output {
    pulse: crate::audio::Pulse,
    channel: u32,
    pub rate: u32,
    /// Bytes the server asked for and hasn't got yet.
    pub owed: usize,
}

/// What the server said while the audio thread waited.
pub enum Wake {
    /// It asked for more (already added to [`Output::owed`]).
    More,
    /// Nothing yet.
    Idle,
}

impl Output {
    /// Opens the stream at `rate`.
    pub fn open(rate: u32) -> Result<Self, String> {
        let mut pulse = crate::audio::Pulse::connect("Mandara Radio")?;
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::MediaName, std::ffi::CString::from(c"Internet radio"));
        props.set(protocol::Prop::ApplicationId, std::ffi::CString::from(c"org.mandara.radio"));
        props.set(protocol::Prop::MediaRole, std::ffi::CString::from(c"music"));
        let target = (TARGET_LATENCY.as_secs_f32() * rate as f32) as u32 * BYTES_PER_FRAME as u32;
        let params = protocol::PlaybackStreamParams {
            sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: CHANNELS as u8, sample_rate: rate },
            channel_map: protocol::ChannelMap::stereo(),
            cvolume: Some(protocol::ChannelVolume::norm(CHANNELS as u8)),
            sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
            buffer_attr: protocol::stream::BufferAttr { max_length: u32::MAX, target_length: target, pre_buffering: u32::MAX, minimum_request_length: u32::MAX, fragment_size: u32::MAX },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            props,
            ..Default::default()
        };
        let reply: protocol::CreatePlaybackStreamReply = pulse.request(Command::CreatePlaybackStream(params))?;
        if reply.sample_spec.format != protocol::SampleFormat::Float32Le || usize::from(reply.sample_spec.channels) != CHANNELS {
            return Err(format!("unexpected stream format {:?}", reply.sample_spec));
        }
        pulse.sock.get_ref().set_read_timeout(Some(READ_TIMEOUT)).map_err(|e| e.to_string())?;
        Ok(Self { pulse, channel: reply.channel, rate, owed: reply.requested_bytes as usize })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.pulse.write_data(self.channel, bytes)?;
        self.owed = self.owed.saturating_sub(bytes.len());
        Ok(())
    }

    /// Waits up to [`READ_TIMEOUT`] for the server.
    pub fn wait(&mut self) -> Result<Wake, String> {
        match protocol::read_command_message(&mut self.pulse.sock, self.pulse.version) {
            Ok((_, Command::Request(protocol::Request { length, .. }))) => {
                self.owed += length as usize;
                Ok(Wake::More)
            }
            Ok((_, Command::PlaybackStreamKilled(_))) => Err(mandara_i18n::t("The sound server closed the stream").into()),
            Ok((_, Command::Error(e))) => Err(format!("{e:?}")),
            Ok(_) => Ok(Wake::Idle),
            Err(protocol::ProtocolError::Io(io)) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(Wake::Idle),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gain_glides_and_fades_in() {
        let mut r = Renderer::new(48_000);
        let frames = vec![[0.5f32, -0.5]; 48_000];
        let mut out = Vec::with_capacity(frames.len() * BYTES_PER_FRAME);
        let rms = r.render(&frames, 1.0, &mut out);
        assert_eq!(out.len(), frames.len() * BYTES_PER_FRAME);
        let first = f32::from_le_bytes(out[..4].try_into().unwrap());
        let last = f32::from_le_bytes(out[out.len() - 8..out.len() - 4].try_into().unwrap());
        assert!(first.abs() < 1e-3, "starts silent: {first}");
        assert!((last - 0.5).abs() < 1e-3, "reaches the target: {last}");
        assert!(rms > 0.4 && rms <= 0.5, "{rms}");
        // Muting glides down instead of clicking.
        let rms = r.render(&frames[..480], 0.0, &mut out);
        let early = f32::from_le_bytes(out[..4].try_into().unwrap());
        assert!(early > 0.45, "the first sample after mute is still near full: {early}");
        assert!((rms - 0.5).abs() < 1e-3, "the level is the signal's, not the volume's: {rms}");
        let _ = r.render(&frames, 0.0, &mut out);
        let tail = f32::from_le_bytes(out[out.len() - 8..out.len() - 4].try_into().unwrap());
        assert!(tail.abs() < 1e-4);
        assert_eq!(r.render(&[], 1.0, &mut out), 0.0);
        assert!(out.is_empty());
    }

    #[test]
    fn output_is_clipped() {
        let mut r = Renderer::new(8_000);
        let mut out = Vec::new();
        let _ = r.render(&[[4.0, -4.0]; 8_000], 1.0, &mut out);
        let s = f32::from_le_bytes(out[out.len() - 8..out.len() - 4].try_into().unwrap());
        assert_eq!(s, 1.0);
    }
}
