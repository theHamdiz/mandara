//! The output you actually hear.
//!
//! The default sink is usually a sound card, and its volume is the one to
//! show and change. With an effects processor in between it isn't: Easy
//! Effects makes `easyeffects_sink` the default, a null sink with
//! `monitor.channel-volumes = false`, so its volume and mute never reach
//! the monitor Easy Effects records from, and the processed sound plays at
//! full level on a real device. Volume, mute, media keys and the on-screen
//! display then follow that device instead:
//!
//! 1. the sink the virtual sink's own playback stream (same
//!    `node.link-group`) is connected to, while it's connected;
//! 2. for Easy Effects, the output device in its settings
//!    (`[StreamOutputs] outputDevice` in `easyeffectsrc`);
//! 3. the one real device that is playing.
//!
//! Anything else keeps the default sink as it is.

use std::path::PathBuf;

use pulseaudio::protocol;

/// PipeWire's null sink, the factory behind virtual sinks.
const NULL_SINK_FACTORY: &str = "support.null-audio-sink";
/// Easy Effects' application id.
const EASY_EFFECTS_ID: &str = "com.github.wwmm.easyeffects";
/// Easy Effects' settings, relative to the config directory (native and
/// Flatpak).
const EASY_EFFECTS_RC: &str = "easyeffects/db/easyeffectsrc";
const EASY_EFFECTS_FLATPAK_RC: &str = ".var/app/com.github.wwmm.easyeffects/config/easyeffects/db/easyeffectsrc";
/// The section and key holding Easy Effects' output device.
const EASY_EFFECTS_SECTION: &str = "[StreamOutputs]";
const EASY_EFFECTS_OUTPUT_KEY: &str = "outputDevice";

/// What the choice needs to know about a sink.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SinkFacts {
    pub index: u32,
    pub name: String,
    /// A null sink whose volume never reaches its monitor.
    pub volume_ignored: bool,
    pub virtual_sink: bool,
    pub running: bool,
    pub link_group: Option<String>,
    pub easy_effects: bool,
}

impl SinkFacts {
    pub fn of(s: &protocol::SinkInfo) -> Self {
        let p = |k: &str| raw_prop(&s.props, k);
        let virtual_sink = p("node.virtual").as_deref() == Some("true") || p("factory.name").as_deref() == Some(NULL_SINK_FACTORY);
        Self {
            index: s.index,
            name: s.name.to_string_lossy().into_owned(),
            volume_ignored: p("factory.name").as_deref() == Some(NULL_SINK_FACTORY) && p("monitor.channel-volumes").as_deref() != Some("true"),
            virtual_sink,
            running: s.state == protocol::SinkState::Running,
            link_group: p("node.link-group"),
            easy_effects: p("application.id").as_deref() == Some(EASY_EFFECTS_ID),
        }
    }
}

/// A playback stream: its sink and its `node.link-group`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamFacts {
    pub sink: u32,
    pub link_group: Option<String>,
}

/// Any property by its full key (the crate's [`protocol::Prop`] names only
/// the standard ones).
pub fn raw_prop(props: &protocol::Props, key: &str) -> Option<String> {
    props.iter().find(|(k, _)| k.to_bytes() == key.as_bytes()).map(|(_, v)| String::from_utf8_lossy(v.strip_suffix(&[0]).unwrap_or(v)).trim().to_string()).filter(|s| !s.is_empty())
}

/// The sink to read and change for "the output volume" (see the module
/// docs), by name.
pub fn heard(default: &str, sinks: &[SinkFacts], streams: &[StreamFacts], easy_effects_output: Option<&str>) -> String {
    let Some(d) = sinks.iter().find(|s| s.name == default) else { return default.to_string() };
    if !d.volume_ignored {
        return d.name.clone();
    }
    let real = |name: &str| sinks.iter().find(|s| s.name == name && !s.virtual_sink).map(|s| s.name.clone());
    let linked = d.link_group.as_ref().and_then(|group| streams.iter().find(|st| st.link_group.as_ref() == Some(group) && st.sink != d.index)).and_then(|st| sinks.iter().find(|s| s.index == st.sink && !s.virtual_sink)).map(|s| s.name.clone());
    let configured = d.easy_effects.then_some(easy_effects_output).flatten().and_then(real);
    let mut playing = sinks.iter().filter(|s| s.running && !s.virtual_sink);
    let only_playing = match (playing.next(), playing.next()) {
        (Some(s), None) => Some(s.name.clone()),
        _ => None,
    };
    linked.or(configured).or(only_playing).unwrap_or_else(|| d.name.clone())
}

/// Easy Effects' configured output device, if its settings name one.
pub fn easy_effects_output() -> Option<String> {
    let files: Vec<PathBuf> = [dirs::config_dir().map(|c| c.join(EASY_EFFECTS_RC)), dirs::home_dir().map(|h| h.join(EASY_EFFECTS_FLATPAK_RC))].into_iter().flatten().collect();
    files.iter().find_map(|f| std::fs::read_to_string(f).ok().and_then(|text| output_in(&text)))
}

fn output_in(text: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == EASY_EFFECTS_SECTION;
        } else if in_section && let Some((k, v)) = line.split_once('=') && k.trim() == EASY_EFFECTS_OUTPUT_KEY {
            return Some(v.trim().to_string()).filter(|v| !v.is_empty());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDMI: &str = "alsa_output.pci-0000_03_00.1.hdmi-stereo-extra3";
    const USB: &str = "alsa_output.usb-Generic_USB_Audio-00.pro-output-0";

    fn card(index: u32, name: &str, running: bool) -> SinkFacts {
        SinkFacts { index, name: name.into(), running, ..Default::default() }
    }

    fn easy_effects() -> SinkFacts {
        SinkFacts { index: 1874, name: "easyeffects_sink".into(), volume_ignored: true, virtual_sink: true, link_group: Some("ee_sink_group".into()), easy_effects: true, ..Default::default() }
    }

    #[test]
    fn a_sound_card_is_its_own_output() {
        let sinks = [card(1, USB, false), card(2, HDMI, true)];
        assert_eq!(heard(USB, &sinks, &[], Some(HDMI)), USB);
    }

    #[test]
    fn easy_effects_follows_its_live_connection_first() {
        let sinks = [card(1, USB, true), card(2, HDMI, false), easy_effects()];
        let streams = [StreamFacts { sink: 1, link_group: Some("ee_sink_group".into()) }, StreamFacts { sink: 1874, link_group: None }];
        assert_eq!(heard("easyeffects_sink", &sinks, &streams, Some(HDMI)), USB);
    }

    #[test]
    fn then_its_configured_device_then_the_playing_one() {
        let sinks = [card(1, USB, true), card(2, HDMI, false), easy_effects()];
        assert_eq!(heard("easyeffects_sink", &sinks, &[], Some(HDMI)), HDMI, "the device Easy Effects plays on");
        assert_eq!(heard("easyeffects_sink", &sinks, &[], Some("unplugged")), USB, "a device that's gone falls back to the one playing");
        assert_eq!(heard("easyeffects_sink", &sinks, &[], None), USB);
    }

    #[test]
    fn nothing_to_go_on_keeps_the_default() {
        let sinks = [card(1, USB, false), card(2, HDMI, false), easy_effects()];
        assert_eq!(heard("easyeffects_sink", &sinks, &[], None), "easyeffects_sink");
        let two_playing = [card(1, USB, true), card(2, HDMI, true), easy_effects()];
        assert_eq!(heard("easyeffects_sink", &two_playing, &[], None), "easyeffects_sink", "two devices playing: no guessing");
    }

    #[test]
    fn reads_easy_effects_settings() {
        let rc = "[General]\nfoo=1\n\n[StreamInputs]\ninputDevice=mic\n[StreamOutputs]\noutputDevice=alsa_output.hdmi\n";
        assert_eq!(output_in(rc).as_deref(), Some("alsa_output.hdmi"));
        assert_eq!(output_in("[StreamInputs]\noutputDevice=x\n"), None, "only its output section");
    }
}
