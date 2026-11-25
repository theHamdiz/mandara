//! FreeBSD's native sound (OSS) when neither PulseAudio nor PipeWire runs:
//! the default device's `vol` and recording controls through `mixer(8)`,
//! the devices from `/dev/sndstat`, and the default device
//! (`hw.snd.default_unit`, changed with `mixer -d` as root).

/// One mixer control (`vol`, `pcm`, `mic`, `rec`…).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Control {
    pub name: String,
    /// Left and right levels, `0..=1`.
    pub left: f32,
    pub right: f32,
    pub muted: bool,
}

impl Control {
    /// The louder side, as the volume shown.
    pub fn level(&self) -> f32 {
        self.left.max(self.right)
    }
}

/// `mixer -o` output: `dev.volume=L:R`, `dev.mute=on|off`, `dev.recsrc=…`.
pub fn parse_mixer(text: &str) -> Vec<Control> {
    let mut out: Vec<Control> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else { continue };
        let Some((dev, field)) = key.split_once('.') else { continue };
        let at = match out.iter().position(|c| c.name == dev) {
            Some(i) => i,
            None => {
                out.push(Control { name: dev.to_string(), ..Control::default() });
                out.len() - 1
            }
        };
        let c = &mut out[at];
        match field {
            "volume" => {
                let mut sides = value.split(':').filter_map(|v| v.trim().parse::<f32>().ok());
                c.left = sides.next().unwrap_or(0.0).clamp(0.0, 1.0);
                c.right = sides.next().map_or(c.left, |r| r.clamp(0.0, 1.0));
            }
            "mute" => c.muted = matches!(value, "on" | "1"),
            _ => {}
        }
    }
    out
}

/// The control that sets playback volume (`vol`, else `pcm`).
pub fn output_control(controls: &[Control]) -> Option<&Control> {
    controls.iter().find(|c| c.name == "vol").or_else(|| controls.iter().find(|c| c.name == "pcm"))
}

/// The control that sets recording level (`mic`, else `rec`).
pub fn input_control(controls: &[Control]) -> Option<&Control> {
    controls.iter().find(|c| c.name == "mic").or_else(|| controls.iter().find(|c| c.name == "rec"))
}

/// A sound device listed in `/dev/sndstat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// `pcm0`.
    pub name: String,
    pub unit: u32,
    pub description: String,
    pub play: bool,
    pub record: bool,
    pub default: bool,
}

/// `/dev/sndstat`: `pcm0: <Realtek ALC892 (Analog)> (play/rec) default`.
pub fn parse_sndstat(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(": <")?;
            let unit = name.strip_prefix("pcm")?.parse().ok()?;
            let (description, tail) = rest.rsplit_once('>')?;
            let caps = tail.split_once('(').and_then(|(_, c)| c.split_once(')')).map(|(c, _)| c).unwrap_or_default();
            Some(Device { name: name.to_string(), unit, description: description.to_string(), play: caps.contains("play"), record: caps.contains("rec"), default: tail.split_whitespace().any(|w| w == "default") })
        })
        .collect()
}

/// A level as `mixer` accepts it (two decimals at most).
pub fn level_arg(control: &str, level: f32) -> String {
    format!("{control}.volume={:.2}", level.clamp(0.0, 1.0))
}

pub fn mute_arg(control: &str, on: bool) -> String {
    format!("{control}.mute={}", if on { "on" } else { "off" })
}

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use super::*;
    use std::process::{Command, Stdio};

    const MIXER: &str = "/usr/sbin/mixer";
    const SNDSTAT: &str = "/dev/sndstat";

    /// The default device's controls (`None` without a sound device).
    pub fn controls() -> Option<Vec<Control>> {
        let out = Command::new(MIXER).arg("-o").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| parse_mixer(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Runs `mixer` with `args` on the default device.
    pub fn set(args: &[String]) -> Result<(), String> {
        let out = Command::new(MIXER).args(args).stdin(Stdio::null()).output().map_err(|e| format!("mixer: {e}"))?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
    }

    /// Every sound device, the default one marked.
    pub fn devices() -> Vec<Device> {
        let mut list = std::fs::read_to_string(SNDSTAT).map(|t| parse_sndstat(&t)).unwrap_or_default();
        if let Some(unit) = crate::freebsd::sysctl::int(c"hw.snd.default_unit").and_then(|u| u32::try_from(u).ok()) {
            list.iter_mut().for_each(|d| d.default = d.unit == unit);
        }
        list
    }

    /// Makes `pcmN` the default device: `mixer -d` as root through pkexec
    /// (it writes `hw.snd.default_unit`). Apps already playing keep their
    /// device until they reopen it.
    pub fn set_default(name: &str) -> Result<(), String> {
        let unit: u32 = name.strip_prefix("pcm").and_then(|u| u.parse().ok()).ok_or_else(|| mandara_i18n::tf("Unknown sound device {name}", &[("name", &name)]))?;
        let status = Command::new(crate::freebsd::power::PKEXEC).args([MIXER, "-d", &unit.to_string()]).stdin(Stdio::null()).status().map_err(|e| format!("pkexec: {e}"))?;
        if status.success() { Ok(()) } else { Err(mandara_i18n::tf("{command} failed ({status})", &[("command", &"mixer -d"), ("status", &status)])) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mixer -o` and `/dev/sndstat` on the FreeBSD 14.4 test machine
    /// (emulated Intel HDA).
    const VM_MIXER: &str = "vol.volume=0.86:0.86\nvol.mute=off\npcm.volume=1.00:1.00\npcm.mute=off\nline.volume=1.00:1.00\nline.mute=off\nline.recsrc=add\nrec.volume=1.00:1.00\nrec.mute=off\n";
    const VM_SNDSTAT: &str = "Installed devices:\npcm0: <Generic (0x1af40022) (Analog)> (play/rec) default\nNo devices installed from userspace.\n";

    #[test]
    fn reads_the_mixer() {
        let c = parse_mixer(VM_MIXER);
        assert_eq!(c.len(), 4);
        let vol = output_control(&c).unwrap();
        assert_eq!((vol.name.as_str(), vol.left, vol.right, vol.muted), ("vol", 0.86, 0.86, false));
        assert_eq!(input_control(&c).unwrap().name, "rec");
        let muted = parse_mixer("vol.volume=0.20:0.40\nvol.mute=on\nmic.volume=0.5\n");
        assert_eq!((muted[0].level(), muted[0].muted), (0.40, true));
        assert_eq!((muted[1].left, muted[1].right), (0.5, 0.5), "a mono control");
        assert_eq!(input_control(&muted).unwrap().name, "mic");
        assert!(output_control(&parse_mixer("")).is_none());
    }

    #[test]
    fn reads_sndstat() {
        let d = parse_sndstat(VM_SNDSTAT);
        assert_eq!(d, [Device { name: "pcm0".into(), unit: 0, description: "Generic (0x1af40022) (Analog)".into(), play: true, record: true, default: true }]);
        let two = parse_sndstat("Installed devices:\npcm0: <Realtek ALC892 (Analog)> (play/rec)\npcm1: <NVIDIA (0x009a) (HDMI/DP 8ch)> (play) default\n");
        assert_eq!((two[1].name.as_str(), two[1].play, two[1].record, two[1].default), ("pcm1", true, false, true));
        assert!(!two[0].default);
    }

    /// The VM's emulated card (skipped on machines without sound).
    #[test]
    #[cfg(target_os = "freebsd")]
    fn reads_the_live_mixer() {
        let Some(controls) = controls() else { return };
        assert!(output_control(&controls).is_some());
        assert!(devices().iter().any(|d| d.default));
    }

    #[test]
    fn mixer_arguments() {
        assert_eq!(level_arg("vol", 0.456), "vol.volume=0.46");
        assert_eq!(level_arg("vol", 1.7), "vol.volume=1.00", "OSS has no boost");
        assert_eq!(mute_arg("rec", true), "rec.mute=on");
    }
}
