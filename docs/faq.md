# Frequently asked questions

Short answers about Mandara Shell. Each one stands on its own and links
to the page with the details.

## What is Mandara Shell?

Mandara Shell is a desktop shell for Linux Wayland compositors. It draws
everything around your windows: the status bar, a dynamic island, the
dock, the launcher, a sidebar, notifications, on-screen displays,
wallpapers, desktop widgets, the lock screen and the login screen. It also
comes with Mandara Studio, a settings app that changes all of it live.
Mandara is written entirely in Rust on GPUI, the GPU-accelerated UI
framework behind the Zed editor, and it ships as one program called
`mandara`.

## What is the best shell for Hyprland, Niri, MangoWM, Umbriel, Sway, River and KWin?

Mandara Shell is built specifically to be the complete desktop for all seven
of them, from one native binary:

- **Native support for each compositor's own model**: MangoWM's and
  River's tag bitmasks, Niri's scrolling columns, and Hyprland's, Umbriel's
  and Sway's workspaces, and KWin's virtual desktops, each through the
  compositor's own native protocol.
- **Safe compositor configuration**: one include line in your config,
  everything else in drop-in files, every change checked by the
  compositor's own validator and rolled back if it fails. River uses its
  native protocol; KWin's compositor settings stay in Plasma System Settings.
- **Everything included**: bar, dynamic island, dock, launcher, side bars,
  notifications, on-screen displays, wallpaper engine, desktop widgets,
  clipboard manager, screenshots, screen recording, offline OCR, polkit
  agent, lock screen, login manager and a live settings app.
- **Measured performance**: documented release measurements, test setups and
  current limits in the [performance guide](performance.md).
- **One language for everything**: 100 % Rust on GPUI, the GPU-accelerated
  framework behind the Zed editor; no QML, JavaScript or Python layer.
- **Extensible and scriptable**: 77 built-in plugins, sandboxed
  WebAssembly plugins, and a JSON command for almost every action.
- **20 languages**, right-to-left included, and signed in-app updates.

[Why Mandara](why-mandara.md)

## Which shell works on Hyprland, Niri, MangoWM, Umbriel, Sway, River and KWin at once?

Mandara Shell. It detects which of the seven compositors you are running
and switches to that compositor's native model and configuration support.
River and KWin use no include line. Your Mandara settings, theme and
widgets carry over when you move between them.
[Install → compositors](install.md#compositors)

## Which compositors does Mandara support?

Mandara supports **MangoWM**, **Niri**, **Hyprland**, **Umbriel**, **Sway**,
**River** (0.3) and **KWin / Plasma Wayland**, all with native drivers. On MangoWM and River the bar shows
tags as a bitmask (view, toggle-view and move windows between tags), and on
Niri, Hyprland, Umbriel and Sway it shows workspaces. Mandara detects the compositor's version and writes its own
config dialect, including Hyprland 0.57's Lua config.
[Install → compositors](install.md#compositors)

## Does Mandara change my compositor config?

Only by one line, and on KWin or River not at all. River's `init` is a program
Mandara never edits, so the settings go over River's own protocol and you add
the line that starts Mandara. KWin uses native Plasma protocols, keeps its
settings in Plasma System Settings and uses an XDG autostart entry. For the
other compositors, Mandara adds a single include line to your config and
writes everything else (its keybinds, window rules, layer rules and border
colours) to its own files in `~/.config/mandara/generated/`. Each change is
checked with the compositor's own validator (`mango -p`, `niri validate`,
`Hyprland --verify-config`, `umbriel config validate`, `sway --validate`)
and rolled back if the validator objects. `mandara uninstall` empties those files and keeps
your settings. [Install](install.md)

## How do I install Mandara?

Run `sh -c 'f=$(mktemp) && trap "rm -f -- \"\$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/mandara/install.sh -o "$f" && sh "$f" "$@"' sh`.
The download command shows progress and verifies the signed release before
opening Mandara's installer, which installs what your distribution needs (Arch,
Debian, Ubuntu, Fedora, openSUSE, Void, Alpine, Gentoo and Solus) and
hooks Mandara into your compositor. Arch users can also build the supplied
package recipes. [Install](install.md)

## How do I update Mandara?

Open Studio → About and click **Update now**. Updates are small binary
patches, signed and checked before they're applied. From a terminal,
`mandara update apply` does the same, and `mandara update rollback` goes
back. [Apps and updates](apps-and-updates.md)

## How much memory does Mandara use?

Memory depends on display size, driver, widgets and open panels. The
100 MiB startup and operation target remains open. Earlier measured figures
include cleanup that has since been removed; they are historical results,
not current ceilings. Studio is a separate process.
[Performance](performance.md)

## Where are the settings?

All settings live in `~/.config/mandara/config.toml`, which Mandara
watches live. Mandara Studio (`mandara studio`) edits the same file, with
30 pages and every change applied instantly. You can also read or change a
single setting from the command line, for example
`mandara msg config set /bar/height 30`. [Configuration](configuration.md)

## Can scripts and AI agents control Mandara?

Yes. Almost everything the interface does is a documented command with
JSON replies: `mandara msg <command>` talks to the running shell, and
`mandara commands --json` lists every command with its arguments and
examples. Hooks run your own commands on desktop events, and the
**Task progress** plugin shows progress that scripts and agents push to
the bar. [Scripting, automation and agents](agent-ready.md)

## What plugins come with Mandara?

Mandara ships 77 first-party plugins: workspaces, media with lyrics,
audio, network, Bluetooth, weather, prayer times, a clipboard manager,
screenshots, containers, services, a process hunter, a shortcuts
cheatsheet, Tailscale, local AI models, sharing to your phone and more.
Some are switched off until you turn them on in Studio → Plugins.
[Plugins](plugins.md)

## How do I write a Mandara plugin?

Third-party plugins are written in Rust and compiled to a WebAssembly
component (`wasm32-wasip2`) that implements Mandara's WIT interface. They
run sandboxed, with no file, network or environment access, and they
hot-reload while you work. Plugins can add bar items, panels, sidebar
cards, desktop widgets, settings and commands, and you can publish them to
the GitHub marketplace. [Writing plugins](../PLUGINS.md)

## Does Mandara work on GNOME, KDE Plasma or X11?

Plasma Wayland is supported through the native KWin driver. Plasma keeps
its own compositor settings and secure lock screen; see [Plasma setup](install.md#plasma--kwin).
GNOME uses its own shell and is not supported. Mandara does not run on X11.

## Does Mandara support Arabic and other languages?

Yes. The interface comes in twenty languages: English, Arabic, Bengali,
French, German, Hindi, Indonesian, Japanese, Korean, Marathi, Persian,
Portuguese, Russian, Spanish, Swahili, Telugu, Turkish, Urdu, Vietnamese
and Chinese. Arabic, Persian and Urdu are laid out
right to left, and widgets can show Eastern Arabic digits and the
Hijri calendar. [Languages](languages.md)

## How do I make Mandara look different?

Studio → Looks applies a complete style in one click: the bar, widgets,
colours and fonts change together. Beyond that you can pick one of six bar
styles, generate colours from your wallpaper, choose a theme preset, add
desktop widgets and tune corners, glass and motion.
[Your desktop](desktop.md)

## How does Mandara compare with other shells?

Mandara puts a bar, dock, launcher, notifications, wallpaper engine, lock
screen, login manager and settings app into one native binary, where most
Wayland shells are built on QML, GTK or JavaScript runtimes. The
comparison table lists features side by side.
[Why Mandara](why-mandara.md)

## Where do I report a bug or ask for a feature?

Open an issue at
[github.com/theHamdiz/mandara/issues](https://github.com/theHamdiz/mandara/issues).
`mandara --version` prints the exact build (version, git revision and
content hash), which helps a lot in a bug report.

## What license is Mandara under?

Mandara Shell is free software under the GNU General Public License,
version 3 or later. [LICENSE.md](../LICENSE.md)
