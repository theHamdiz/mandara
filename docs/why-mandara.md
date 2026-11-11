# Why Mandara

Mandara is a status bar, dynamic island, dock, launcher, sidebar,
notification center, wallpaper engine, lock screen, login manager,
screenshot and recording studio, and settings studio for **MangoWM, Niri,
Hyprland, Umbriel, Sway, River, KWin, Aqueous, labwc and COSMIC**. All of it ships as one native Rust binary built on
[GPUI](https://github.com/zed-industries/zed), the GPU-accelerated UI
framework behind the Zed editor.

There's no QML, JavaScript runtime, Python or shell-script glue. It's one
process, rendered on the GPU and animated with real spring physics.

## At a glance

<table>
<tr><td><b>Light</b></td><td>bounded workers, lazy plugin data and explicit cache release; <a href="performance.md">measurements and current resource limits</a></td></tr>
<tr><td><b>Native</b></td><td>100 % Rust in one binary; the exact line count and hashes of this build are in the <a href="building.md#build-fingerprint">build fingerprint</a></td></tr>
<tr><td><b>Extensible</b></td><td>81 first-party plugins built in; third-party plugins in sandboxed WebAssembly, from a GitHub marketplace or a folder</td></tr>
<tr><td><b>Compositors</b></td><td>MangoWM (tags), Niri, Hyprland, Umbriel, Sway, River (tags), KWin, Aqueous, labwc and COSMIC, all first-class, configured non-destructively</td></tr>
<tr><td><b>Complete</b></td><td>bar, dock, launcher, notifications, sound mixer, lock screen, login manager, wallpaper engine, drag-and-drop app installer and uninstaller, and more</td></tr>
<tr><td><b>Generative</b></td><td>a built-in <a href="wallpapers.md#wallpaper-generator">wallpaper generator</a>: twelve styles, up to 8K, the smallest lossless files and seamless video loops, all in pure Rust</td></tr>
<tr><td><b>Global</b></td><td>20 interface languages, including right-to-left Arabic, Persian and Urdu</td></tr>
<tr><td><b>Live</b></td><td>Mandara Studio: 30 pages of settings, every change applied instantly</td></tr>
<tr><td><b>Agent-ready</b></td><td>almost everything the UI does is a documented CLI command with JSON replies, so scripts and AI agents can drive and inspect the desktop (<a href="agent-ready.md">how</a>)</td></tr>
</table>

## How it compares

The Linux desktop has three excellent modern shells:

- **[Caelestia](https://github.com/caelestia-dots/shell)** has gorgeous spatial
  choreography.
- **[Noctalia](https://docs.noctalia.dev)** has depth, polish and ergonomics.
- **[Dux Shell](https://github.com/duxlabs-dev/dux-shell)** (DankMaterialShell
  until its 2.0 rename) has first-class compositor control.

They're built differently. Caelestia and Dux run on Quickshell (Qt Quick /
QML), with Go for Dux's CLI and helpers. Noctalia v5 is a ground-up rewrite in
native C++ with OpenGL ES, with no Qt or GTK, and uses Luau for scripted
widgets and plugins. Mandara takes the best idea from each and rebuilds them
in Rust as one native, compiled program, with plugins compiled to sandboxed
WebAssembly.

| | **Mandara** | Noctalia v5 | Dux (DMS 2.0) | Caelestia |
|---|---|---|---|---|
| Runtime | **Native Rust binary (GPUI, wgpu/Vulkan)** | Native C++ binary (OpenGL ES, no Qt/GTK) | Quickshell + QML (+ Go) | Quickshell + QML |
| Interpreted UI layer | **None** | None in the core; Luau for scripted widgets | QML/JS | QML/JS |
| Animation | **Analytical spring physics, interruptible, ready for 1000 Hz** | Native (C++) | Qt animations | Qt animations |
| Color engine | **Material You (HCT) palettes + OKLCH tweening** | Material You | Material You (matugen) | Material You |
| Plugins | **Rust → sandboxed WebAssembly, hot-reloaded, GitHub marketplace** | Luau | QML | — |
| Plugin crash isolation | **A trapping plugin is disabled; the shell keeps running** | In-process Luau VM | Shared process | Shared process |
| Compositors | **MangoWM (tags), Niri, Hyprland, Umbriel, Sway, River (tags), KWin, Aqueous, labwc, COSMIC, all with native drivers** | Niri, Hyprland, … | Niri, Hyprland, … | Hyprland |
| Compositor settings GUI | **Gaps, borders, rounding, blur, rules, keybinds; non-destructive drop-ins** | Partial | Yes | — |
| Bar layouts | **Edge frame, glass pills, dynamic island, hairline, micro pill, notch; any screen edge** | Several | Several | Island |
| Screen recording | **Built in (native screencopy → AV1/WebM, pure Rust)** | External tools | External tools | External tools |
| OCR (copy text from screen) | **Built in, offline: English, العربية, Español, Français…** | — | — | — |
| Interface languages | **20, with RTL Arabic, Persian and Urdu** | Several | Several | English |
| Settings app | **Mandara Studio: 30 pages in six groups, live preview, drag-and-drop layout designer, maintenance and repair** | Settings panel | Settings app | Config files |

Desktop and system features:

| | **Mandara** | Noctalia v5 | Dux (DMS 2.0) | Caelestia |
|---|---|---|---|---|
| Idle & screens off | **Lock / screens off / sleep, mains vs battery timeouts, lock before sleep (`ext-idle-notify`)** | Yes | Yes | n/s |
| Night light | **Sun schedule or fixed times, transitions, app exceptions (`wlr-gamma-control`)** | Yes | Yes | n/s |
| Display settings | **Arrangement, modes, scale, VRR, revert countdown, auto profiles, DDC/CI brightness** | External-monitor brightness | Yes | n/s |
| App dock | **Plugin: magnification, smart auto-hide, per display** | Yes | Yes | n/s |
| Alt-Tab window switcher | **Live previews, MRU, screen time** | Yes | Compositor overview | n/s |
| Hot corners | **Yes, any action or command** | Yes | Yes | n/s |
| Calendar events | **ICS / webcal feeds and folders, reminders (no CalDAV yet)** | CalDAV / ICS | Tasks only | n/s |
| VPN & hotspot | **VPN switch, WireGuard / OpenVPN import, hotspot with QR** | VPN | VPN + hotspot | n/s |
| Keyboard, mouse & touchpad | **Written to compositor drop-ins, layout labels** | Layout labels | Yes | n/s |
| Settings profiles & backup | **Profiles, undo, export / import, support report** | Export, support report | Theme registry | n/s |
| User accounts | **AccountsService: create, edit, photos, passwords** | — | Yes | n/s |
| Lock screen | **`ext-session-lock` + PAM, configurable look** | Yes | Yes | Yes |
| Polkit agent | **Built in** | Yes | n/s | n/s |
| Keyring unlock | **GCR prompter, unlock at login (GNOME Keyring, KWallet), KeePassXC dialog** | n/s | n/s | n/s |
| System tray | **StatusNotifierWatcher + menus** | Yes | Yes | Yes |

The comparison reflects the projects as we understand them at the time of
writing. The second table comes from a survey of Noctalia v5.4.0's and Dux
2.0's settings schemas (October 2026): "—" means we found no such setting,
**n/s** means not surveyed. Caelestia wasn't surveyed for it. Corrections are
welcome.
