//! The sandbox's demo data, generated on first use (existing files are kept):
//! generic apps, Downloads, the demo account, wallpapers, album covers, the
//! music track, matte backgrounds and a hardware-free WirePlumber profile.

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, SystemTime};

use image::ImageFormat;

use super::paint::{self, Blob, Color, Ridge, hex};
use super::player::TRACKS;
use super::{Sandbox, write};
use crate::Result;

/// `(id suffix, name, comment, Papirus icon, categories)` of the demo apps.
const APPS: [(&str, &str, &str, &str, &str); 21] = [
    ("browser", "Browser", "Browse the web", "web-browser", "Network;WebBrowser;"),
    ("calculator", "Calculator", "Perform arithmetic and unit conversions", "accessories-calculator", "Utility;Calculator;"),
    ("calendar", "Calendar", "Plan your days and events", "office-calendar", "Office;Calendar;"),
    ("camera", "Camera", "Take photos and videos", "camera-photo", "AudioVideo;Video;"),
    ("chat", "Chat", "Talk with friends and teams", "internet-chat", "Network;Chat;InstantMessaging;"),
    ("clocks", "Clocks", "Alarms, timers and world time", "accessories-clock", "Utility;Clock;"),
    ("contacts", "Contacts", "Manage your address book", "x-office-address-book", "Office;ContactManagement;"),
    ("dictionary", "Dictionary", "Look up words and definitions", "accessories-dictionary", "Office;Dictionary;"),
    ("files", "Files", "Access and organize files", "system-file-manager", "System;FileManager;"),
    ("games", "Games", "Play a few quick games", "applications-games", "Game;"),
    ("help", "Help", "Guides for this desktop", "help-browser", "Documentation;"),
    ("mail", "Mail", "Read and write email", "internet-mail", "Network;Email;"),
    ("maps", "Maps", "Find places and directions", "org.gnome.Maps", "Utility;Maps;"),
    ("music", "Music", "Play and organize your music", "multimedia-audio-player", "AudioVideo;Audio;Player;"),
    ("photos", "Photos", "View and edit your pictures", "image-viewer", "Graphics;Viewer;"),
    ("settings", "Settings", "Configure the system", "preferences-system", "Settings;"),
    ("system-monitor", "System Monitor", "See what is running", "utilities-system-monitor", "System;Monitor;"),
    ("terminal", "Terminal", "Use the command line", "utilities-terminal", "System;TerminalEmulator;"),
    ("text-editor", "Text Editor", "Edit text files", "accessories-text-editor", "Utility;TextEditor;"),
    ("videos", "Videos", "Watch movies and clips", "multimedia-video-player", "AudioVideo;Video;Player;"),
    ("weather", "Weather", "Forecasts for your places", "org.gnome.Weather", "Utility;"),
];

/// What a demo download contains.
#[derive(Clone, Copy)]
enum Content {
    /// Deterministic filler bytes of this size (opaque formats).
    Bytes(usize),
    Text(&'static str),
    CoverPng,
    TexturePng,
    MoodboardJpeg,
    FloorPlanSvg,
}

/// The demo Downloads, newest first.
const DOWNLOADS: [(&str, Content); 18] = [
    ("Release notes 0.1.0.md", Content::Text(RELEASE_NOTES)),
    ("Corniche palette boards.pdf", Content::Bytes(116_000)),
    ("Cover sketch 03.png", Content::CoverPng),
    ("Lighthouse photos (final).zip", Content::Bytes(80_000)),
    ("Sea glass textures.png", Content::TexturePng),
    ("Moodboard.jpg", Content::MoodboardJpeg),
    ("Interview transcript.txt", Content::Text(TRANSCRIPT)),
    ("Studio floor plan.svg", Content::FloorPlanSvg),
    ("Team offsite agenda.docx", Content::Bytes(34_000)),
    ("Montaza garden walk.mp3", Content::Bytes(118_000)),
    ("Sunset timelapse.mp4", Content::Bytes(105_000)),
    ("Font specimens.pdf", Content::Bytes(110_000)),
    ("Harbour plan v3.pdf", Content::Bytes(6_200)),
    ("Budget 2026 draft.xlsx", Content::Bytes(103_000)),
    ("Field recording 02.wav", Content::Bytes(13_000)),
    ("Invoice 0142.pdf", Content::Bytes(119_000)),
    ("Recipe - basil pesto.pdf", Content::Bytes(92_000)),
    ("Wallpaper pack.tar.gz", Content::Bytes(120_000)),
];
/// Age step between consecutive downloads.
const DOWNLOAD_AGE_STEP: Duration = Duration::from_secs(7 * 60);

const RELEASE_NOTES: &str = "# Release notes 0.1.0\n\n- New glass pills for the bar\n- The dock learned stacks\n- Faster launcher search\n- Wallpaper colors crossfade between tracks\n";
const TRANSCRIPT: &str = "Interview transcript — studio visit\n\nQ: What drew you to the coast?\nA: The light. Mornings on the Corniche are silver, evenings turn amber.\n\nQ: How do you start a palette?\nA: With the sea glass we pick up on walks, then the limestone of the old buildings.\n";
const FLOOR_PLAN_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="400" viewBox="0 0 640 400"><rect width="640" height="400" fill="#f4efe6"/><g fill="none" stroke="#24405c" stroke-width="6"><rect x="40" y="40" width="560" height="320"/><path d="M300 40v180M300 280v80M40 220h180M380 220h220"/></g><g font-family="sans-serif" font-size="20" fill="#24405c"><text x="90" y="130">Studio</text><text x="420" y="130">Gallery</text><text x="110" y="300">Kitchen</text><text x="420" y="300">Office</text></g></svg>
"##;

/// Wallpaper size (physical pixels) and the downscaled `~/wall.png`.
const WALL_SIZE: (u32, u32) = (2880, 1620);
const WALL_PNG_SIZE: (u32, u32) = (1920, 1080);
/// The wallpaper `~/wall.png` is made from.
const WALL_SOURCE: &str = "harbour-dusk";
/// Album cover size.
const COVER_SIZE: u32 = 384;
/// Image sizes inside Downloads.
const DOWNLOAD_IMAGE: (u32, u32) = (800, 500);
const DOWNLOAD_COVER: u32 = 512;
/// Grain seeds of the mesh and landscape wallpapers.
const MESH_SEED: u64 = 7;
const LANDSCAPE_SEED: u64 = 3;

type MeshDef = (&'static str, &'static str, &'static [(f32, f32, f32, &'static str, f32)]);
type LandscapeDef = (&'static str, (&'static str, &'static str), &'static str, (f32, f32), &'static [(f32, f32, f32, f32, &'static str)]);

/// Mesh-gradient wallpapers: `(name, base, [(x, y, radius, color, alpha)])`.
const MESHES: [MeshDef; 7] = [
    ("tide-dark", "#04111f", &[(0.12, 0.12, 0.28, "#04404f", 0.9), (0.8, 0.22, 0.25, "#16245a", 0.85), (0.2, 0.92, 0.22, "#5a3a08", 0.75), (0.6, 0.85, 0.24, "#035650", 0.8)]),
    ("tide-light", "#eef4f2", &[(0.1, 0.1, 0.3, "#b4e6ee", 0.9), (0.9, 0.15, 0.25, "#fbd9c4", 0.85), (0.1, 0.95, 0.22, "#ffd0cc", 0.7), (0.65, 0.88, 0.26, "#a8e4d0", 0.85)]),
    ("lagoon", "#062a2c", &[(0.3, 0.3, 0.3, "#0d7a74", 0.85), (0.85, 0.7, 0.25, "#0c3f6e", 0.8), (0.1, 0.85, 0.2, "#1f5a3c", 0.6)]),
    ("citadel-ember", "#1a0a10", &[(0.2, 0.2, 0.3, "#6e1d2a", 0.85), (0.8, 0.8, 0.3, "#a8501e", 0.7), (0.75, 0.15, 0.2, "#3b1550", 0.75)]),
    ("pastel-sky", "#f2eefb", &[(0.15, 0.2, 0.3, "#9fdcf5", 0.9), (0.85, 0.3, 0.25, "#a8f0c8", 0.8), (0.4, 0.9, 0.25, "#d4c8ff", 0.85), (0.05, 0.95, 0.15, "#ffc4de", 0.8)]),
    ("violet-night", "#0b0820", &[(0.25, 0.25, 0.3, "#2c1a66", 0.85), (0.8, 0.6, 0.28, "#14316e", 0.8), (0.55, 0.95, 0.2, "#5a1d5e", 0.6)]),
    ("sea-glass", "#d8ece6", &[(0.2, 0.3, 0.3, "#86cfc4", 0.85), (0.8, 0.25, 0.28, "#c4e3f2", 0.8), (0.6, 0.9, 0.25, "#f0dcc0", 0.75)]),
];

/// Landscape wallpapers: `(name, (sky top, sky bottom), sun, sun position,
/// [(crest, amplitude, frequency, phase, color)])`.
const LANDSCAPES: [LandscapeDef; 3] = [
    ("harbour-dusk", ("#1c2560", "#5b4a7a"), "#f2b99a", (0.66, 0.42), &[(0.62, 0.02, 1.2, 0.3, "#2a2f5c"), (0.7, 0.025, 1.6, 1.4, "#1e2348"), (0.8, 0.02, 1.1, 2.2, "#141834")]),
    ("dune-dawn", ("#f4c7a6", "#fbe6cf"), "#fff3dc", (0.3, 0.5), &[(0.68, 0.03, 0.9, 0.5, "#e2a477"), (0.76, 0.03, 1.3, 2.0, "#c9825c"), (0.86, 0.02, 0.8, 3.1, "#a5603f")]),
    ("forest-mist", ("#1f3a33", "#7fa596"), "#dfeee2", (0.5, 0.35), &[(0.55, 0.04, 1.8, 0.2, "#3c6457"), (0.66, 0.035, 2.4, 1.0, "#2b4d42"), (0.78, 0.03, 1.5, 2.6, "#1b332c")]),
];

const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\ndemo:x:1000:1000:Mandara Demo:/home/demo:/bin/bash\nnobody:x:65534:65534:Nobody:/:/usr/bin/nologin\n";
const GROUP: &str = "root:x:0:\ndemo:x:1000:\nnobody:x:65534:\n";
const HOSTNAME: &str = "mandara-demo\n";
/// WirePlumber without device monitors: no real sound cards, cameras or
/// Bluetooth reach the sandbox.
const WIREPLUMBER_CONF: &str = "wireplumber.profiles = {\n  main = {\n    monitor.alsa = disabled\n    monitor.alsa-midi = disabled\n    monitor.bluez = disabled\n    monitor.bluez.seat-monitoring = disabled\n    monitor.v4l2 = disabled\n    monitor.libcamera = disabled\n  }\n}\n";
/// Host data the demo apps' icons and fonts come from.
const SHARED_FROM_HOST: [&str; 4] = ["fonts", "icons", "mime", "pixmaps"];

/// Generates whatever demo data is missing.
pub fn ensure(sb: &Sandbox) -> Result<()> {
    let home = sb.home();
    for d in ["config/mango", "config/mandara", "data", "cache", "state", "Downloads", "Pictures/Wallpapers"] {
        mkdir(&home.join(d))?;
    }
    mkdir(&sb.logs())?;
    write(&home.join("config/wireplumber/wireplumber.conf.d/90-sandbox.conf"), WIREPLUMBER_CONF)?;
    write(&sb.etc().join("passwd"), PASSWD)?;
    write(&sb.etc().join("group"), GROUP)?;
    write(&sb.etc().join("hostname"), HOSTNAME)?;
    apps(sb)?;
    downloads(&home.join("Downloads"))?;
    wallpapers(sb)?;
    covers(sb)?;
    track(sb)?;
    mattes(sb)
}

fn mkdir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

fn desktop_entry(name: &str, comment: &str, icon: &str, categories: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nComment={comment}\nIcon={icon}\nExec=true\nCategories={categories}\nTerminal=false\n")
}

/// `XDG_DATA_DIRS` for the sandbox: the demo apps only, with the host's
/// fonts, icons and MIME data linked in.
fn apps(sb: &Sandbox) -> Result<()> {
    let share = sb.share();
    let dir = share.join("applications");
    mkdir(&dir)?;
    for (id, name, comment, icon, categories) in APPS {
        write(&dir.join(format!("org.mandara.demo.{id}.desktop")), &desktop_entry(name, comment, icon, categories))?;
    }
    for d in SHARED_FROM_HOST {
        let link = share.join(d);
        if std::fs::symlink_metadata(&link).is_err() {
            std::os::unix::fs::symlink(Path::new("/usr/share").join(d), &link).map_err(|e| format!("{}: {e}", link.display()))?;
        }
    }
    Ok(())
}

/// FNV-1a: a stable seed from a name.
fn seed(name: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    name.bytes().fold(FNV_OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

/// Deterministic filler bytes (xorshift64*).
fn filler(len: usize, mut state: u64) -> Vec<u8> {
    const MULTIPLIER: u64 = 0x2545_F491_4F6C_DD1D;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        out.extend_from_slice(&state.wrapping_mul(MULTIPLIER).to_le_bytes());
    }
    out.truncate(len);
    out
}

fn save_image(img: &image::RgbImage, path: &Path, format: ImageFormat) -> Result<()> {
    img.save_with_format(path, format).map_err(|e| format!("{}: {e}", path.display()))
}

fn downloads(dir: &Path) -> Result<()> {
    let now = SystemTime::now();
    for (i, (name, content)) in DOWNLOADS.iter().enumerate() {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        let (w, h) = DOWNLOAD_IMAGE;
        match *content {
            Content::Bytes(len) => std::fs::write(&path, filler(len, seed(name) | 1)).map_err(|e| e.to_string())?,
            Content::Text(text) => write(&path, text)?,
            Content::FloorPlanSvg => write(&path, FLOOR_PLAN_SVG)?,
            Content::CoverPng => save_image(&paint::cover(DOWNLOAD_COVER, TRACKS[2].colors), &path, ImageFormat::Png)?,
            Content::TexturePng => save_image(&mesh(MESHES[6], w, h), &path, ImageFormat::Png)?,
            Content::MoodboardJpeg => save_image(&landscape(LANDSCAPES[1], w, h), &path, ImageFormat::Jpeg)?,
        }
        let age = DOWNLOAD_AGE_STEP * u32::try_from(i).unwrap_or(0);
        let file = std::fs::File::options().write(true).open(&path).map_err(|e| e.to_string())?;
        file.set_modified(now - age).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn mesh((_, base, blobs): MeshDef, w: u32, h: u32) -> image::RgbImage {
    let blobs: Vec<Blob> = blobs.iter().map(|&(x, y, radius, c, alpha)| Blob { x, y, radius, color: hex(c), alpha }).collect();
    paint::mesh(w, h, hex(base), &blobs, MESH_SEED)
}

fn landscape((_, (top, bottom), sun, sun_at, ridges): LandscapeDef, w: u32, h: u32) -> image::RgbImage {
    let ridges: Vec<Ridge> = ridges.iter().map(|&(base, amp, freq, phase, c)| Ridge { base, amp, freq, phase, color: hex(c) }).collect();
    paint::landscape(w, h, (hex(top), hex(bottom)), hex(sun), sun_at, &ridges, LANDSCAPE_SEED)
}

fn wallpapers(sb: &Sandbox) -> Result<()> {
    let dir = sb.home().join("Pictures/Wallpapers");
    let (w, h) = WALL_SIZE;
    type Painter = Box<dyn Fn() -> image::RgbImage>;
    let jobs: Vec<(&str, Painter)> = MESHES
        .iter()
        .map(|&m| (m.0, Box::new(move || mesh(m, w, h)) as Painter))
        .chain(LANDSCAPES.iter().map(|&l| (l.0, Box::new(move || landscape(l, w, h)) as Painter)))
        .collect();
    for (name, paint) in &jobs {
        let path = dir.join(format!("{name}.png"));
        if !path.exists() {
            println!("generating {}", path.display());
            save_image(&paint(), &path, ImageFormat::Png)?;
        }
    }
    let wall = sb.home().join("wall.png");
    if !wall.exists() {
        let src = image::open(dir.join(format!("{WALL_SOURCE}.png"))).map_err(|e| e.to_string())?.to_rgb8();
        let (w, h) = WALL_PNG_SIZE;
        save_image(&image::imageops::resize(&src, w, h, image::imageops::FilterType::Lanczos3), &wall, ImageFormat::Png)?;
    }
    Ok(())
}

/// Album covers of the demo player's tracks: `art/cover-<i>.png`.
pub fn cover_path(sb: &Sandbox, index: usize) -> std::path::PathBuf {
    sb.root.join("art").join(format!("cover-{index}.png"))
}

fn covers(sb: &Sandbox) -> Result<()> {
    mkdir(&sb.root.join("art"))?;
    for (i, t) in TRACKS.iter().enumerate() {
        let path = cover_path(sb, i);
        if !path.exists() {
            save_image(&paint::cover(COVER_SIZE, t.colors), &path, ImageFormat::Png)?;
        }
    }
    Ok(())
}

/// Black and white wallpapers for `matte`.
pub fn matte_paths(sb: &Sandbox) -> (std::path::PathBuf, std::path::PathBuf) {
    (sb.root.join("mat/black.png"), sb.root.join("mat/white.png"))
}

fn mattes(sb: &Sandbox) -> Result<()> {
    mkdir(&sb.root.join("mat"))?;
    let (black, white) = matte_paths(sb);
    let (w, h) = WALL_SIZE;
    for (path, value) in [(black, u8::MIN), (white, u8::MAX)] {
        if !path.exists() {
            save_image(&paint::solid(w, h, value), &path, ImageFormat::Png)?;
        }
    }
    Ok(())
}

/// The demo track.
pub fn track_path(sb: &Sandbox) -> std::path::PathBuf {
    sb.root.join("demo.wav")
}

fn track(sb: &Sandbox) -> Result<()> {
    let path = track_path(sb);
    if path.exists() {
        return Ok(());
    }
    let samples = synth::render();
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    f.write_all(&synth::wav_header(samples.len())).map_err(|e| e.to_string())?;
    for s in samples {
        f.write_all(&s.to_le_bytes()).map_err(|e| e.to_string())?;
    }
    f.flush().map_err(|e| e.to_string())
}

/// The procedural demo track: a seamless 24 s loop at 80 bpm — a warm pad
/// over A minor, F, C and G, a plucked arpeggio, a soft kick and hats.
mod synth {
    use std::f32::consts::TAU;

    pub const SAMPLE_RATE: u32 = 48_000;
    pub const CHANNELS: u16 = 2;
    const BITS: u16 = 16;
    const BPM: f32 = 80.0;
    const BEATS_PER_CHORD: u32 = 8;
    /// Root, third and fifth of each chord (Hz).
    const CHORDS: [[f32; 3]; 4] = [[220.0, 261.63, 329.63], [174.61, 220.0, 261.63], [130.81, 164.81, 196.0], [196.0, 246.94, 293.66]];
    /// Detune between the stereo pad voices (ratio).
    const DETUNE: f32 = 0.003;
    /// Pad crossfade between chords (seconds).
    const PAD_FADE: f32 = 0.6;
    const PAD_LEVEL: f32 = 0.11;
    const BASS_LEVEL: f32 = 0.16;
    const ARP_LEVEL: f32 = 0.12;
    const ARP_DECAY: f32 = 7.0;
    const KICK_LEVEL: f32 = 0.5;
    const KICK_DECAY: f32 = 9.0;
    const KICK_FROM_HZ: f32 = 120.0;
    const KICK_TO_HZ: f32 = 45.0;
    const KICK_SWEEP: f32 = 30.0;
    const HAT_LEVEL: f32 = 0.05;
    const HAT_DECAY: f32 = 60.0;
    /// Arpeggio steps per beat and the order of chord tones.
    const ARP_PER_BEAT: f32 = 2.0;
    const ARP_ORDER: [usize; 4] = [0, 1, 2, 1];
    /// Master gain before the soft clipper.
    const MASTER: f32 = 0.9;
    /// Full scale of a 16-bit sample.
    const FULL_SCALE: f32 = i16::MAX as f32;
    /// RIFF header size in bytes.
    const HEADER: usize = 44;

    fn beat_secs() -> f32 {
        60.0 / BPM
    }

    /// Loop length in seconds.
    pub fn seconds() -> f32 {
        beat_secs() * (BEATS_PER_CHORD as usize * CHORDS.len()) as f32
    }

    /// Smooth 0→1 ramp.
    fn smooth(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Deterministic white noise in -1..1 for a sample index.
    fn noise(i: u32) -> f32 {
        let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
        x ^= x >> 15;
        x = x.wrapping_mul(0x2C1B_3C6D);
        x ^= x >> 12;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Interleaved stereo samples of the whole loop.
    pub fn render() -> Vec<i16> {
        let total = (seconds() * SAMPLE_RATE as f32) as u32;
        let chord_len = beat_secs() * BEATS_PER_CHORD as f32;
        let mut out = Vec::with_capacity(total as usize * usize::from(CHANNELS));
        for i in 0..total {
            let t = i as f32 / SAMPLE_RATE as f32;
            let chord_i = (t / chord_len) as usize % CHORDS.len();
            let in_chord = t % chord_len;
            // Crossfade into the next chord at the end of each one (wrapping,
            // so the loop is seamless).
            let next = (chord_i + 1) % CHORDS.len();
            let fade = smooth((in_chord - (chord_len - PAD_FADE)) / PAD_FADE);
            let mut frame = [0.0f32; 2];
            for (ch, sample) in frame.iter_mut().enumerate() {
                let detune = if ch == 0 { 1.0 - DETUNE } else { 1.0 + DETUNE };
                let pad = |c: &[f32; 3]| c.iter().map(|f| (TAU * f * detune * t).sin() + 0.3 * (TAU * 2.0 * f * detune * t).sin()).sum::<f32>();
                *sample += PAD_LEVEL * (pad(&CHORDS[chord_i]) * (1.0 - fade) + pad(&CHORDS[next]) * fade);
                let root = CHORDS[chord_i][0] / 2.0;
                *sample += BASS_LEVEL * (TAU * root * t).sin() * (1.0 - fade) + BASS_LEVEL * (TAU * CHORDS[next][0] / 2.0 * t).sin() * fade;
            }
            // Plucked arpeggio, an octave up.
            let step_len = beat_secs() / ARP_PER_BEAT;
            let step = (t / step_len) as usize;
            let tone = CHORDS[chord_i][ARP_ORDER[step % ARP_ORDER.len()]] * 2.0;
            let since = t % step_len;
            let pluck = ARP_LEVEL * (-since * ARP_DECAY).exp() * (TAU * tone * since).sin();
            // Kick on beats 1 and 3, hats on the off-beats.
            let beat = (t / beat_secs()) as u32;
            let in_beat = t % beat_secs();
            let kick = if beat.is_multiple_of(2) {
                let f = KICK_TO_HZ + (KICK_FROM_HZ - KICK_TO_HZ) * (-in_beat * KICK_SWEEP).exp();
                KICK_LEVEL * (-in_beat * KICK_DECAY).exp() * (TAU * f * in_beat).sin()
            } else {
                0.0
            };
            let off = (in_beat - beat_secs() / 2.0).max(0.0);
            let hat = if in_beat >= beat_secs() / 2.0 { HAT_LEVEL * (-off * HAT_DECAY).exp() * noise(i) } else { 0.0 };
            let pan = [0.6, 0.4];
            for (ch, s) in frame.iter().enumerate() {
                let v = (s + pluck * pan[ch] * 2.0 + kick + hat) * MASTER;
                out.push((v.tanh() * FULL_SCALE) as i16);
            }
        }
        out
    }

    /// A canonical 44-byte PCM WAV header for `samples` 16-bit samples.
    pub fn wav_header(samples: usize) -> Vec<u8> {
        let block = CHANNELS * BITS / 8;
        let data = (samples * usize::from(BITS / 8)) as u32;
        let mut h = Vec::with_capacity(HEADER);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(data + HEADER as u32 - 8).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        h.extend_from_slice(&1u16.to_le_bytes()); // PCM
        h.extend_from_slice(&CHANNELS.to_le_bytes());
        h.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        h.extend_from_slice(&(SAMPLE_RATE * u32::from(block)).to_le_bytes());
        h.extend_from_slice(&block.to_le_bytes());
        h.extend_from_slice(&BITS.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&data.to_le_bytes());
        h
    }
}

/// Pixel colour of a cover's sky as `[r, g, b]` (for `TRACKS`).
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    [r as f32, g as f32, b as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_sets_are_complete() {
        assert_eq!(APPS.len(), 21);
        assert_eq!(DOWNLOADS.len(), 18);
        assert_eq!(MESHES.len() + LANDSCAPES.len(), 10);
        assert!(LANDSCAPES.iter().any(|l| l.0 == WALL_SOURCE));
    }

    #[test]
    fn identity_is_the_demo_account() {
        assert!(PASSWD.contains("demo:x:1000:1000:Mandara Demo:/home/demo:"));
        assert_eq!(HOSTNAME.trim(), "mandara-demo");
    }

    #[test]
    fn filler_is_deterministic() {
        assert_eq!(filler(100, 42), filler(100, 42));
        assert_ne!(filler(100, 42), filler(100, 43));
        assert_eq!(filler(13, 1).len(), 13);
    }

    #[test]
    fn wav_header_is_canonical() {
        let h = synth::wav_header(4);
        assert_eq!(h.len(), 44);
        assert_eq!(&h[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 8);
    }

    #[test]
    fn track_is_a_24_second_loop() {
        assert!((synth::seconds() - 24.0).abs() < 1e-3);
    }
}
