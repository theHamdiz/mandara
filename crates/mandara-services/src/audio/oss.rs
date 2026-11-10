//! FreeBSD without PulseAudio or PipeWire: the audio state and commands on
//! the default OSS device through `mixer(8)` ([`crate::freebsd::oss`]).
//! Per-app volume, stream moves and "mute all" need a sound server, so
//! they answer with the reason.

use super::{AudioCommand, AudioState, Channel, Device, Mixer, volume_limit};
use crate::freebsd::oss;

/// The default device's playback and recording levels, or `None` without
/// a sound device.
pub(super) fn state() -> Option<AudioState> {
    let controls = oss::controls()?;
    let out = oss::output_control(&controls)?;
    let device = oss::devices().into_iter().find(|d| d.default);
    let (name, dev) = device.map(|d| (d.description, d.name)).unwrap_or_default();
    let mut state = AudioState { available: true, output: Channel { volume: out.level(), muted: out.muted, name: name.clone(), device: dev.clone() }, ..AudioState::default() };
    if let Some(input) = oss::input_control(&controls) {
        state.input = Channel { volume: input.level(), muted: input.muted, name, device: dev };
    }
    Some(state)
}

/// Why a command needs a sound server.
fn needs_server() -> String {
    mandara_i18n::t("Per-app sound needs PulseAudio or PipeWire; FreeBSD's own sound system has one volume per device").into()
}

pub(super) fn execute(state: &AudioState, command: AudioCommand) -> Result<String, String> {
    let controls = oss::controls().ok_or_else(|| mandara_i18n::t("No sound device").to_string())?;
    let out = oss::output_control(&controls).map(|c| c.name.clone()).ok_or_else(|| mandara_i18n::t("No sound device").to_string())?;
    let input = oss::input_control(&controls).map(|c| c.name.clone());
    let set_level = |control: &str, v: f32| -> Result<String, String> {
        // OSS levels top out at 100 %.
        let v = v.clamp(0.0, volume_limit().min(1.0));
        oss::set(&[oss::level_arg(control, v)])?;
        Ok(format!("{:.0}%", v * 100.0))
    };
    let input_name = || input.clone().ok_or_else(|| mandara_i18n::t("No microphone").to_string());
    match command {
        AudioCommand::SetVolume(v) => set_level(&out, v),
        AudioCommand::AdjustVolume(d) => set_level(&out, state.output.volume + d),
        AudioCommand::SetMicVolume(v) => set_level(&input_name()?, v).map(|p| format!("mic {p}")),
        AudioCommand::ToggleMute | AudioCommand::ToggleMuteAll => {
            let mute = !state.output.muted;
            oss::set(&[oss::mute_arg(&out, mute)])?;
            Ok(if mute { "muted" } else { "unmuted" }.into())
        }
        AudioCommand::SetMuteAll(on) => {
            oss::set(&[oss::mute_arg(&out, on)])?;
            Ok(if on { "muted" } else { "unmuted" }.into())
        }
        AudioCommand::ToggleMicMute => {
            let mute = !state.input.muted;
            oss::set(&[oss::mute_arg(&input_name()?, mute)])?;
            Ok(if mute { "mic muted" } else { "mic unmuted" }.into())
        }
        AudioCommand::SetInputMute(_, mute) => {
            oss::set(&[oss::mute_arg(&input_name()?, mute)])?;
            Ok(if mute { "input muted" } else { "input unmuted" }.into())
        }
        AudioCommand::SetStreamVolume(..) | AudioCommand::SetStreamMute(..) | AudioCommand::MoveStreams(..) => Err(needs_server()),
    }
}

/// The sound devices as the mixer page lists them (no app streams).
pub(super) fn mixer() -> Result<Mixer, String> {
    let controls = oss::controls().unwrap_or_default();
    let level = |c: Option<&oss::Control>| c.map(|c| (c.level(), c.muted)).unwrap_or_default();
    let (out_level, out_muted) = level(oss::output_control(&controls));
    let (in_level, in_muted) = level(oss::input_control(&controls));
    let mut mixer = Mixer::default();
    for d in oss::devices() {
        let device = |volume: f32, muted: bool| Device { index: d.unit, name: d.name.clone(), description: d.description.clone(), is_default: d.default, volume: if d.default { volume } else { 0.0 }, muted: d.default && muted };
        if d.play {
            mixer.outputs.push(device(out_level, out_muted));
        }
        if d.record {
            mixer.inputs.push(device(in_level, in_muted));
        }
    }
    if mixer.outputs.is_empty() && mixer.inputs.is_empty() {
        return Err(mandara_i18n::t("No sound device").into());
    }
    Ok(mixer)
}

/// One device is the default for playing and recording on FreeBSD.
pub(super) fn set_default(name: &str) -> Result<(), String> {
    oss::set_default(name)
}
