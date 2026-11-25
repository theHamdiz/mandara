//! Global mute through its public API against a mock sound server (never
//! the live one: `GlobalMute::set` doesn't start the guard thread).

use std::collections::BTreeMap;

use mandara_services::audio::mute::{Facility, GlobalMute, MuteBackend, MuteState, Node};

/// An in-memory PulseAudio stand-in that counts mute changes.
#[derive(Default)]
struct FakePulse {
    sinks: BTreeMap<u32, Node>,
    streams: BTreeMap<u32, Node>,
    changes: usize,
}

impl FakePulse {
    fn add(map: &mut BTreeMap<u32, Node>, index: u32, key: &str, muted: bool) {
        map.insert(index, Node { index, key: key.into(), muted });
    }

    fn mutes(&self) -> Vec<(String, bool)> {
        self.sinks.values().chain(self.streams.values()).map(|n| (n.key.clone(), n.muted)).collect()
    }
}

impl MuteBackend for FakePulse {
    fn sinks(&mut self) -> Result<Vec<Node>, String> {
        Ok(self.sinks.values().cloned().collect())
    }
    fn streams(&mut self) -> Result<Vec<Node>, String> {
        Ok(self.streams.values().cloned().collect())
    }
    fn sink(&mut self, index: u32) -> Option<Node> {
        self.sinks.get(&index).cloned()
    }
    fn stream(&mut self, index: u32) -> Option<Node> {
        self.streams.get(&index).cloned()
    }
    fn set_sink_mute(&mut self, index: u32, mute: bool) -> Result<(), String> {
        self.changes += 1;
        self.sinks.get_mut(&index).map(|n| n.muted = mute).ok_or_else(|| "gone".into())
    }
    fn set_stream_mute(&mut self, index: u32, mute: bool) -> Result<(), String> {
        self.changes += 1;
        self.streams.get_mut(&index).map(|n| n.muted = mute).ok_or_else(|| "gone".into())
    }
}

/// The machine this was built for: EasyEffects' virtual sink in front of
/// the hardware, a muted HDMI output and a corked player.
fn easyeffects_setup() -> FakePulse {
    let mut f = FakePulse::default();
    FakePulse::add(&mut f.sinks, 53, "alsa_output.usb-Generic_USB_Audio-00.pro-output-0", false);
    FakePulse::add(&mut f.sinks, 69, "alsa_output.pci-0000_03_00.1.hdmi-stereo-extra3", true);
    FakePulse::add(&mut f.sinks, 86, "easyeffects_sink", false);
    FakePulse::add(&mut f.streams, 12138, "application.name:rufin", false);
    f
}

#[test]
fn toggle_cycle_restores_every_device_and_stream() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audio-mute.toml");
    let mut pulse = easyeffects_setup();
    let before = pulse.mutes();
    let global = GlobalMute::load(path.clone());
    assert!(!global.enabled());

    let on = global.set(&mut pulse, true).unwrap();
    assert_eq!((on.sinks, on.streams), (2, 1), "the hardware sink behind EasyEffects is muted too");
    assert!(pulse.mutes().iter().all(|(_, m)| *m));
    assert!(MuteState::load(&path).enabled, "persisted while on");

    // A browser notification while muted.
    FakePulse::add(&mut pulse.streams, 12200, "application.name:Brave", false);
    let mut state = global.snapshot();
    state.on_event(&mut pulse, Facility::Stream, 12200, |s| s.save(&path).unwrap());
    assert!(pulse.streams[&12200].muted);

    // The shell restarts: the saved record is picked up and still restores exactly.
    let restarted = GlobalMute::load(path.clone());
    assert!(restarted.enabled());
    let off = restarted.set(&mut pulse, false).unwrap();
    assert_eq!((off.sinks, off.streams), (2, 2));
    pulse.streams.remove(&12200);
    assert_eq!(pulse.mutes(), before);
    assert!(!MuteState::load(&path).enabled);

    // Switching off twice is harmless.
    let changes = pulse.changes;
    restarted.set(&mut pulse, false).unwrap();
    assert_eq!(pulse.changes, changes);
}
