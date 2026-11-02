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

## Which compositors does Mandara support?

Mandara supports **MangoWM**, **Niri**, **Hyprland** and **Umbriel**, all
first-class. On MangoWM the bar shows tags as a bitmask (view, toggle-view
and move windows between tags), and on Niri, Hyprland and Umbriel it shows
workspaces. Mandara detects the compositor's version and writes its own
config dialect, including Hyprland 0.57's Lua config.
[Install → compositors](install.md#mangowm-niri-hyprland-and-umbriel)

## Does Mandara change my compositor config?

Only by one line. Mandara adds a single include line to your config and
writes everything else (its keybinds, window rules, layer rules and border
colours) to its own files in `~/.config/mandara/generated/`. Each change is
checked with the compositor's own validator (`mango -p`, `niri validate`,
`Hyprland --verify-config`, `umbriel config validate`) and rolled back if
the validator objects. `mandara uninstall` empties those files and keeps
your settings. [Install](install.md)

## How do I install Mandara?

Run the one-line installer from the [README](../README.md#install). It
checks its own signature, installs what your distribution needs (Arch,
Debian, Ubuntu, Fedora, openSUSE, Void, Alpine, Gentoo and Solus) and
hooks Mandara into your compositor. Arch users can also use the AUR
packages. [Install](install.md)

## How do I update Mandara?

Open Studio → About and click **Update now**. Updates are small binary
patches, signed and checked before they're applied. From a terminal,
`mandara update apply` does the same, and `mandara update rollback` goes
back. [Apps and updates](apps-and-updates.md)

## How much memory does Mandara use?

The whole shell uses under 100 MiB of RAM, under 190 MiB of video memory
and about 0.1 % CPU when idle. Wallpapers cost no video memory, and
nothing animates or samples while it's off screen.
[Performance](performance.md)

## Where are the settings?

All settings live in `~/.config/mandara/config.toml`, which Mandara
watches live. Mandara Studio (`mandara studio`) edits the same file, with
31 pages and every change applied instantly. You can also read or change a
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

Mandara ships 73 first-party plugins: workspaces, media with lyrics,
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

No. Mandara is a shell for the Wayland compositors listed above. GNOME and
KDE Plasma bring their own shells, and Mandara does not run on X11.

## Does Mandara support Arabic and other languages?

Yes. The interface comes in fifteen languages: English, Arabic, Bengali,
German, Spanish, French, Hindi, Indonesian, Japanese, Korean, Persian,
Russian, Turkish, Urdu and Chinese. Arabic, Persian and Urdu are laid out
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
