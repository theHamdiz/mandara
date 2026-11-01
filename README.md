<p align="center">
  <img src="assets/brand/mandara-256.png" alt="Mandara Shell logo" width="112"/>
</p>

<h1 align="center">Mandara Shell</h1>

<p align="center">
  <b>A beautiful, fast Linux desktop that fits in under 100 MiB of RAM.</b>
  <br/>
  Design-first, spring-animated and written entirely in Rust.
  <br/>
  Named after <b>المندرة, El-Mandara</b>, the coastal district of Alexandria.
</p>

<p align="center">
  <a href="docs/building.md#build-fingerprint"><img src="https://img.shields.io/badge/100%25-Rust-B7410E?logo=rust&logoColor=white" alt="100% Rust"/></a>
  <a href="docs/desktop.md"><img src="https://img.shields.io/badge/Wayland-native-FFBC00?logo=wayland&logoColor=black" alt="Wayland native"/></a>
  <a href="docs/performance.md"><img src="https://img.shields.io/badge/GPU-GPUI%20%C2%B7%20wgpu%20%C2%B7%20Vulkan-5B4FE9" alt="GPU rendered"/></a>
  <a href="docs/install.md#mangowm-niri-and-hyprland"><img src="https://img.shields.io/badge/MangoWM%20%C2%B7%20Niri%20%C2%B7%20Hyprland-first--class-0A7EA4" alt="MangoWM, Niri, Hyprland"/></a>
  <a href="docs/plugins.md"><img src="https://img.shields.io/badge/plugins-43%20built--in%20%2B%20WebAssembly-8250DF" alt="43 plugins plus WebAssembly"/></a>
  <a href="docs/languages.md"><img src="https://img.shields.io/badge/languages-10%20%C2%B7%20RTL%20Arabic-0E7C86" alt="10 languages"/></a>
  <br/>
  <a href="docs/performance.md"><img src="https://img.shields.io/badge/RAM-under%20100%20MiB-2EA44F" alt="RAM under 100 MiB"/></a>
  <a href="docs/performance.md"><img src="https://img.shields.io/badge/VRAM-under%20190%20MiB-2EA44F" alt="VRAM under 190 MiB"/></a>
  <a href="docs/performance.md"><img src="https://img.shields.io/badge/idle%20CPU-0.1%25-2EA44F" alt="Idle CPU 0.1%"/></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="GPL-3.0-or-later"/></a>
  <a href="https://ko-fi.com/thehamdiz"><img src="https://img.shields.io/badge/Ko--fi-support-FF5E5B?logo=ko-fi&logoColor=white" alt="Support on Ko-fi"/></a>
</p>

<p align="center">
  <img src="docs/screenshots/hero.webp" alt="Mandara Shell on a demo desktop: the glass-pill bar, a Today sidebar tab with weather, prayer times, resources and sound, the media panel with its visualizer, notification cards, the Day block desktop clock and the dock" width="100%"/>
</p>

<p align="center">
  <a href="docs/install.md"><b>Install</b></a> ·
  <a href="docs/README.md"><b>Documentation</b></a> ·
  <a href="docs/studio.md">Studio</a> ·
  <a href="docs/plugins.md">Plugins</a> ·
  <a href="docs/commands.md">Commands</a> ·
  <a href="CHANGELOG.md">What's new</a>
</p>

---

Mandara is everything around your windows: the bar, a dynamic island, a
dock, a launcher, a sidebar, notifications, wallpapers, the lock screen and
the login screen, plus a settings app that changes all of it live. It runs
on **MangoWM, Niri and Hyprland**, and it's one small, fast program you
install with one command.

It's made to feel good to use. Panels pour out of the bar on real spring
physics, colours follow your wallpaper and your music, and the whole
desktop stays light enough to run all day without you noticing it.

## Why you'll like it

- **It's light.** The whole desktop needs less memory than a browser tab
  and sits at about 0.1 % CPU when idle. [How](docs/performance.md)
- **It looks the way you want.** Six bar styles (now including a notch that
  hangs from the top edge), one-click Looks, desktop widgets, a hundred
  dials for corners, glass and motion, and themes made from your wallpaper.
  [Your desktop](docs/desktop.md)
- **Everything is a click away in Studio.** 31 pages of settings, all
  live: drag a slider and the bar follows as you drag.
  [Mandara Studio](docs/studio.md)
- **It does a lot out of the box.** A media island with lyrics, a sound
  mixer per app, screenshots and screen recording, copying text from the
  screen, a clipboard, a drag-and-drop app installer, night light, and 43
  built-in plugins. [Everything else](docs/more-features.md)
- **It welcomes you in.** A matching lock screen and login screen with
  your own wallpaper, and ready-made looks for both.
  [Lock screen and login](docs/lock-and-login.md)
- **It speaks your language.** Ten languages, with Arabic laid out right
  to left. [Languages](docs/languages.md)
- **It plays well with scripts and AI agents.** Nearly everything the
  interface does is also a command with JSON replies.
  [Automation](docs/agent-ready.md)

## A look around

<table>
<tr>
<td width="50%" valign="top"><img src="docs/screenshots/media.png" alt="Media island" width="100%"/><br/><sub><b>Media</b>: colours from the album art, a live spectrum and synced lyrics.</sub></td>
<td width="50%" valign="top"><img src="docs/screenshots/audio.png" alt="Sound panel with per-app volume" width="100%"/><br/><sub><b>Sound</b>: every output, your microphone and every app playing.</sub></td>
</tr>
<tr>
<td width="50%" valign="top"><img src="docs/screenshots/weather.png" alt="Weather" width="100%"/><br/><sub><b>Weather</b>: an animated sky, hourly and five-day forecasts.</sub></td>
<td width="50%" valign="top"><img src="docs/screenshots/resources.png" alt="System resources" width="100%"/><br/><sub><b>Resources</b>: CPU, memory, every GPU, temperatures and disks.</sub></td>
</tr>
</table>

<p align="center">
  <img src="docs/screenshots/looks.png" alt="The six Looks: Corniche Dusk, Pharos Ink, Montaza Crimson, Qaitbay Forest, Anfushi Wave and Night Frame" width="100%"/>
  <br/>
  <sub>Six Looks, one click each: the bar, widgets, colours and fonts change together.</sub>
</p>

<table>
<tr>
<td width="50%" valign="top"><img src="docs/screenshots/greeter.webp" alt="Mandara Login" width="100%"/><br/><sub><b>Mandara Login</b>, the login screen</sub></td>
<td width="50%" valign="top"><img src="docs/screenshots/lock-screen.webp" alt="Lock screen" width="100%"/><br/><sub><b>The lock screen</b>, made to match</sub></td>
</tr>
</table>

<p align="center">
  <img src="docs/screenshots/studio-login.png" alt="Studio: the login screen designer with a live preview" width="100%"/>
  <br/>
  <sub><b>Mandara Studio</b>: every setting, live, with previews like this login screen designer.</sub>
</p>

## Install

One command, from any shell:

```bash
sh -c 'cd "$(mktemp -d)" && curl -fsSL "https://github.com/theHamdiz/mandara/releases/latest/download/mandara-$(uname -m)-linux-gnu.tar.gz" | tar xz && exec ./mandara-*/bin/mandara install'
```

The installer checks its own signature, sets up what your distribution
needs, and hooks into your compositor without touching your own config
files. It works on Arch, Debian and Ubuntu, Fedora, openSUSE, Void, Alpine,
Gentoo and Solus. Updates arrive by small binary patch: Studio → About →
**Update now**.

→ **[Install guide](docs/install.md)**: Arch packages, other options,
coming from another setup.

## Learn more

- **[Documentation](docs/README.md)**: every feature, one topic per page
- **[Using Mandara](docs/usage.md)**: commands and default shortcuts
- **[Plugins](docs/plugins.md)** and **[writing your own](PLUGINS.md)**
- **[Why Mandara](docs/why-mandara.md)**: how it compares with other shells
- **[Building from source](docs/building.md)** and
  **[contributing](AGENTS.md)**

## Supporting Mandara

Mandara is free, open source and always will be. If it makes your desktop a
little nicer and you'd like to say thanks, you can chip in toward the AI
credits and developer time that go into it:

[![Buy me a coffee on Ko-fi](https://img.shields.io/badge/Ko--fi-Buy%20me%20a%20coffee-FF5E5B?logo=ko-fi&logoColor=white)](https://ko-fi.com/thehamdiz)

It's entirely optional. Using Mandara, telling a friend, writing a plugin
or reporting a bug helps just as much. Thank you!

## License

Mandara Shell is free software under the **GNU General Public License,
version 3 or later** ([LICENSE.md](LICENSE.md), notice in
[COPYRIGHT.md](COPYRIGHT.md)). Vendored code keeps the license in its
folder, and fonts and models downloaded on demand keep their own: OFL for
Google Fonts, Apache-2.0 for the PaddleOCR models.
