# Install

One command, from any shell (bash, zsh or fish):

```bash
sh -c 'f=$(mktemp) && trap "rm -f -- \"\$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/mandara/install.sh -o "$f" && sh "$f" "$@"' sh
```

The small [download script](../install.sh) shows Mandara's phoenix and the
download progress, chooses the build for your CPU and GNU or musl system,
then checks the release's Ed25519 signature and the archive's SHA-256
**before extracting or running it**. It uses `curl`, `tar`, OpenSSL 3 or
newer, `sha256sum` and the usual POSIX tools; it tells you when one is
missing. Downloads retry on temporary connection errors and stop if the
connection stalls. The command runs the script only after the download
succeeds. It removes temporary files on completion, failure or interruption.

The verified installer normally runs from your cache, and falls back to
`/tmp` if the cache is unavailable. Setting `TMPDIR` overrides both: choose
a writable folder on a filesystem that allows programs to run. A `noexec`
mount on that folder prevents the extracted installer from starting.

Its messages follow your terminal language, with the same fifteen languages
as Mandara.

Installation, configuration and administrator prompts stay in the Rust
`mandara install` command. Forward its options with `"$@"`:

```bash
sh -c 'f=$(mktemp) && trap "rm -f -- \"\$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/mandara/install.sh -o "$f" && sh "$f" "$@"' sh --dry-run
sh -c 'f=$(mktemp) && trap "rm -f -- \"\$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/mandara/install.sh -o "$f" && sh "$f" "$@"' sh --compositor kwin --no-greeter
```

Options are forwarded unchanged, including paths with spaces. Temporary
downloads stay private; the Rust installer receives your original file-permission
mask. The bootstrap
does not request administrator access or edit configuration itself. To read
it before running it, download `install.sh` to a file and run `sh install.sh`
with the same options.

You can also download a tarball by hand:

```bash
curl -LO https://github.com/theHamdiz/mandara/releases/latest/download/mandara-x86_64-linux-gnu.tar.gz
tar xf mandara-x86_64-linux-gnu.tar.gz
./mandara-*/bin/mandara install
```

Pick the tarball that fits (or grab it from the
[Releases page](https://github.com/theHamdiz/mandara/releases/latest) in a
browser): `mandara-x86_64-linux-gnu-x86-64-v3` (AVX2-era CPUs, fastest),
`mandara-x86_64-linux-gnu`, `mandara-aarch64-linux-gnu`, or the `-musl`
builds for Alpine, Void musl and Chimera. `SHA256SUMS` sits next to them
(`sha256sum -c SHA256SUMS --ignore-missing` checks a download by hand),
signed by Mandara's release key in `SHA256SUMS.sig`: `mandara install` and
every update check that signature and refuse a release without it.

Adding Mandara to PATH updates your shell profiles. The terminal that ran
the installer keeps its old PATH: open a new terminal before running
`mandara` commands, or open Mandara Studio from your application menu.

`mandara install` copies itself to `~/.local/bin`, after **checking its own
SHA-256** against the release's published checksums (and offers to put that
folder on your PATH if it isn't). Then it installs the
runtime packages your distribution needs (asking for sudo, doas, run0 or
pkexec only for that), installs a root-owned **admin helper** (a copy of
Mandara only root can change, the one Studio's administrator actions run,
so your user-writable copy never runs as root), hooks into your compositor
non-destructively and, if
you agree, starts Mandara with it, switches your login screen to Mandara
Login (greetd + cage, backed up and reversible) and unlocks GNOME Keyring /
KWallet with your login password. Updates come later by binary patch
(`mandara update`).

It works on **Arch, Debian/Ubuntu, Fedora, openSUSE, Void, Alpine, Gentoo
and Solus**; on NixOS, use the [flake](#nixos) instead. Besides the
runtime libraries it installs, one by one so a missing package never stops
it, fonts and colour emoji, Papirus icons, `xdg-utils`, the desktop portals
of your compositors, and the services Studio talks to: PipeWire and
WirePlumber (sound), NetworkManager and its OpenVPN plugin (Network panel,
`.ovpn` import), BlueZ (Bluetooth, enabled for you), polkit, GNOME Keyring
with its PAM module, AccountsService (Users), GeoClue (location), UDisks2
(drives), UPower and power-profiles-daemon (battery, profiles), i2c-tools
(DDC/CI monitor brightness) and the freedesktop sound theme (event sounds).

Before installing anything it checks that your package databases can be
used, and brings them up to date when they can't (see
[Package databases missing or out of date](#package-databases-missing-or-out-of-date)).
When no compositor Mandara runs on is installed yet, it offers to install
one (see [No compositor yet](#no-compositor-yet)).

| Option | Meaning |
|---|---|
| `--latest` | Download and install the latest release instead of this binary |
| `--version X.Y.Z` | Download and install that release instead of this binary |
| `--from-source` | Build from source (installs Rust and build tools) |
| `--prefix DIR` | Install into `DIR/bin` (default `~/.local`) |
| `--no-deps` | Skip system packages; shortcut checks still need libxkbcommon |
| `--no-autostart` | Don't start Mandara with the compositor |
| `--no-greeter` | Keep your login screen |
| `--compositor mango\|niri\|hyprland\|umbriel\|sway\|river\|kwin` | Pick the compositor to integrate with. Missing compositors can be installed from your distribution's repositories after asking (`--yes` accepts); KWin requires an existing Plasma installation. |
| `--repo OWNER/NAME` | Releases and sources from another repository |
| `--dry-run` | Show everything that would happen, change nothing |
| `--yes` | Don't ask; take every default |
| `--no-verify` | Install even if this binary doesn't match its published checksum |

With a terminal, `mandara install` opens on its own screen: the MANDARA
wordmark and an arrow-key menu that picks the compositor to integrate with
(the cursor suggests one you already have; the row stays on *detect
automatically* until you choose) and switches autostart and the login screen
on or off before anything is installed. Every choice there is also a
flag: one you pass is left out of the menu, and `--yes`, `--dry-run` or a
run without a terminal skips the screen and prints the one-line plan
instead. Esc, `q` or Ctrl-C leaves it without changing anything.

`mandara uninstall` removes the binary and desktop entry and empties the
compositor drop-ins; your settings are kept.

### Languages

Translations aren't built into the binary: each language is a small
**language pack** for exactly one Mandara version
([details](languages.md#language-packs)). The tarballs carry every pack in
`share/mandara/locales/<version>/`, and `mandara install` copies them to
`~/.local/share/mandara/locales/<version>/`, so a fresh install speaks your
language without going online. The `.deb`, `.rpm` and Arch packages
install them in `/usr/share/mandara/locales/<version>/`. A self-installed
Mandara that is missing a pack (an install from an older tarball, or a
language picked later) downloads just that one from its own version's
signed release, and every update brings the new version's packs for the
languages you use.

## No compositor yet

The setup screen's *Compositor* row covers this in a terminal: leave it on
*detect automatically* and, when nothing is installed, the installer falls
back to this numbered list. It still needs an interactive terminal for your
answer, but it does not need arrow keys or the setup screen.

When none of MangoWM, Niri, Hyprland, Sway, River Classic, Umbriel or KWin is
installed, the installer
lists the ones your distribution's own repositories carry and asks which
to install, with the first one as the recommended default:

```text
==> Installing a compositor
    No supported compositor is installed (MangoWM, Niri, Hyprland, Sway, River Classic, Umbriel, KWin).
      1) MangoWM (recommended) · mangowm
      2) Niri · niri
      3) Hyprland · hyprland
      0) None: I'll install one myself
    Install which one? [1]
```

It installs your pick with its desktop portal (screen sharing), then hooks
Mandara into it as usual. Choose it on your login screen to start it. To
skip the question, name one: `mandara install --compositor niri` installs
Niri when it is missing (after asking; with `--yes` without asking). With
`--yes` alone, or without a terminal to answer on, nothing is installed:
the installer only says how.

Only official repositories are used; the installer never adds one. Where
a compositor comes only from a third-party source (the AUR, Copr, a PPA,
Gentoo's GURU), install it yourself and run `mandara setup`.

| | MangoWM | Niri | Hyprland |
|---|---|---|---|
| Arch and derivatives | `mangowm` | `niri` | `hyprland` |
| Debian, Ubuntu | | | `hyprland` (Debian unstable, Ubuntu 26.04; not Debian 13) |
| Fedora | | `niri` | |
| openSUSE Tumbleweed | | `niri` | `hyprland` |
| Void | | `niri` | |
| Alpine | `mangowm` | `niri` | `hyprland` |

Before offering one it also asks the package manager whether your release
has it (pacman, apt, xbps and apk can say so quickly), so Debian stable,
which has no Hyprland package, isn't offered one. Umbriel isn't packaged
by any distribution yet.

## Package databases missing or out of date

A system whose package databases were never downloaded (a fresh install,
a container, a cleared cache) can't install anything: pacman reports
`database file for 'core' does not exist` and `target not found` for every
package, apt says `Unable to locate package`. The installer checks the
files before it installs anything, the same in every language:

| | It checks | It runs |
|---|---|---|
| Arch (pacman) | a database in `/var/lib/pacman/sync/` for every repository in `pacman.conf` | `sudo pacman -Syu`, a full system upgrade, after asking |
| Debian, Ubuntu (apt) | the package lists in `/var/lib/apt/lists/` | `sudo apt-get update` (every time; it changes no packages) |
| Gentoo | the repository tree in `/var/db/repos/gentoo` | `sudo emerge --sync` |
| Fedora, openSUSE, Alpine, Solus | nothing: they fetch their metadata themselves | `dnf makecache --refresh`, `zypper refresh`, `apk update` or `eopkg update-repo`, only if installing failed |

On Arch it is always the full `pacman -Syu`, never `pacman -Sy` followed by
an install: that is a partial upgrade, which Arch doesn't support. Since it
upgrades the whole system, it is asked first:

```text
Your package databases are missing or out of date (core, extra). Update them now? This runs `sudo pacman -Syu`, a full system upgrade. [Y/n]
```

Answer no and the installer stops, telling you to run `sudo pacman -Syu`
yourself and then `mandara install` again (or `mandara install --no-deps`
to skip system packages). When the databases exist but installing still
fails (they may be out of date and the mirror no longer has those package
versions), it offers the same update once and tries again. `--dry-run`
shows the question and the commands; `--yes` takes the default answer.

## Arch Linux

Three package recipes, pick one:

| Package | What you get |
|---|---|
| `mandara-shell-bin` | the prebuilt release binary (fastest; x86_64 and aarch64) |
| `mandara-shell` | the same release, built from source on your machine |
| `mandara-shell-git` | the latest `main`, built from source |

They are on their way to the **AUR** (new AUR accounts are paused right
now; once the packages are published, `paru -S mandara-shell-bin` or
`yay -S mandara-shell-bin` will do). Until then, build the same recipes
from this repository with nothing but `git` and `base-devel`:

```bash
git clone https://github.com/theHamdiz/mandara.git
```

```bash
cd mandara/packaging/aur/mandara-shell-bin && makepkg -si
```

Each release fills these recipes in with its version and checksums, so a
fresh clone always builds the newest release. Every
[release](https://github.com/theHamdiz/mandara/releases) also attaches a
ready `PKGBUILD` for `mandara-shell-bin`: download it into an empty folder
and run `makepkg -si`.

The package installs the `mandara` binary, the Studio desktop entry and
icon, and the lock screen's PAM file (`/etc/pam.d/mandara-lock`). Then hook
Mandara into your compositor (non-destructive: one include line, with a
backup) and start it:

```bash
mandara setup
```

The recipes live in [`packaging/aur/`](../packaging/aur) and follow the
[AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines).

## NixOS

Mandara is a flake: Nix builds it from source (a release build, like the
tarballs) and keeps it updated, so the built-in updater stays out of the
way. Studio → About says "Installed by Nix" and `mandara update apply`
refuses; you update with `nix flake update` and a rebuild instead.

Try it without installing anything:

```bash
nix run github:theHamdiz/mandara
```

Or install it into your profile with
`nix profile install github:theHamdiz/mandara` (update with
`nix profile upgrade mandara`).

### NixOS module

The module sets up, system-wide, what `mandara install` sets up on other
distributions. Add the flake to your system flake:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    mandara.url = "github:theHamdiz/mandara";
    mandara.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, mandara, ... }: {
    nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
      modules = [
        ./configuration.nix
        mandara.nixosModules.default
        {
          programs.mandara.enable = true;
          # programs.mandara.greeter.enable = true;  # Mandara Login
        }
      ];
    };
  };
}
```

Then `sudo nixos-rebuild switch`. With `programs.mandara.enable` you get:

| | |
|---|---|
| The package | `mandara` on your `PATH`, Mandara Studio in your app launcher |
| Lock screen | the `mandara-lock` PAM service `mandara lock` checks your password with |
| polkit | enabled; Mandara is your session's authentication agent, so don't run another one |
| Desktop portals | `xdg.portal` with GTK's portal as the fallback; your compositor's NixOS module adds its own |
| Fonts and icons | Inter, JetBrains Mono, Noto (with CJK and colour emoji), Papirus icons, the freedesktop sound theme |
| Services | AccountsService, GeoClue, UDisks2, UPower, power-profiles-daemon (unless TLP or auto-cpufreq is on) and GNOME Keyring, each a default you can override |

| Option | Default | Meaning |
|---|---|---|
| `programs.mandara.enable` | `false` | Everything in the table above |
| `programs.mandara.package` | this flake's build | The Mandara package |
| `programs.mandara.fonts.enable` | `true` | Install the fonts |
| `programs.mandara.services.enable` | `true` | Enable the services Studio talks to |
| `programs.mandara.monitorBrightness.enable` | `false` | Load `i2c-dev` for external monitor brightness (DDC/CI); add yourself to the `i2c` group |
| `programs.mandara.greeter.enable` | `false` | Mandara Login on greetd, in `cage`, listing the sessions of every compositor you enabled; turn your other display manager off |
| `programs.mandara.greeter.unlockKeyring` | GNOME Keyring's state | Unlock GNOME Keyring with the password you type at Mandara Login |

The module doesn't enable a compositor (`programs.hyprland.enable`,
`programs.niri.enable`, … stay yours), PipeWire, NetworkManager or
Bluetooth: turn those on as usual for the sound, Network and Bluetooth
panels. The Alexandria and Cairo display fonts are Google Fonts; add them
with `fonts.packages = [ (pkgs.google-fonts.override { fonts = [ "Alexandria" "Cairo" ]; }) ];`.

Then hook Mandara into your compositor, as on any distribution:

```bash
mandara setup --autostart
```

That adds the one include line to your compositor config and starts
Mandara with it (see [below](#compositors)). If Home
Manager writes your compositor config, add the line there instead (next
section).

### Home Manager

For per-user settings, import the Home Manager module:

```nix
{
  imports = [ inputs.mandara.homeManagerModules.default ];

  programs.mandara = {
    enable = true;
    # Optional: start Mandara as a systemd user service with your session.
    systemd.enable = true;
    # Optional: manage ~/.config/mandara/config.toml from Nix (see below).
    # settings = { bar.height = 30; };
  };
}
```

| Option | Default | Meaning |
|---|---|---|
| `programs.mandara.enable` | `false` | Install Mandara for this user |
| `programs.mandara.package` | this flake's build | The Mandara package |
| `programs.mandara.settings` | `{ }` | Settings written to `~/.config/mandara/config.toml` ([configuration.md](configuration.md)) |
| `programs.mandara.systemd.enable` | `false` | Start Mandara as a systemd user service |
| `programs.mandara.systemd.target` | `graphical-session.target` | The user target that starts and stops it |

**Settings and Studio.** Studio edits `config.toml` live. While
`settings` is empty (the default) Home Manager leaves the file alone and
everything you change in Studio is kept. Once you set `settings`, the file
belongs to Home Manager: Studio's changes still preview live, but they
aren't saved, and the next `home-manager switch` brings your Nix settings
back. Keys you leave out keep Mandara's defaults.

**Starting Mandara.** Either let your compositor start it
(`mandara setup --autostart`, or a line in your Home Manager compositor
config), or use `systemd.enable`, not both. The service needs your
compositor to start `graphical-session.target` and pass `WAYLAND_DISPLAY`
and its socket variables to the systemd user manager: Niri does, and so
does Hyprland's Home Manager module with its systemd integration
(`wayland.windowManager.hyprland.systemd.enable`); for others, check your
compositor's documentation.

**Your compositor config.** Mandara writes its rules, binds and border
colours to `~/.config/mandara/generated/` and never edits a file Home
Manager made (it can't: those live in the read-only Nix store). Add the
include line to your compositor's Home Manager config yourself:

| Compositor | Add to your Home Manager config |
|---|---|
| **MangoWM** | `source_optional=~/.config/mandara/generated/mango.conf` in the config text |
| **Niri** 25.11+ | `include optional=true "/home/you/.config/mandara/generated/niri.kdl"` in `config.kdl` (plain `include` before 26.04) |
| **Hyprland 0.57+** | `pcall(dofile, "/home/you/.config/mandara/generated/hyprland.lua")` in `hyprland.lua` |
| **Hyprland ≤ 0.56** | `source = ~/.config/mandara/generated/hyprland.conf`, e.g. `wayland.windowManager.hyprland.settings.source = [ "~/.config/mandara/generated/hyprland.conf" ];` |
| **Umbriel** | `"/home/you/.config/mandara/generated/umbriel.toml"` in `[include.optional] files` of `config.toml` |
| **Sway** | `include ~/.config/mandara/generated/sway.conf` at the end, e.g. `wayland.windowManager.sway.extraConfig` |
| **River Classic** | no include line: add `riverctl spawn mandara` to your `init` |

After `home-manager switch`, run `mandara setup` once: it finds your line
and writes the drop-in. Run it before adding the line and it changes
nothing in the read-only config; it prints the line to add instead.

## Compositors

Mandara supports **MangoWM, Niri, Hyprland, Umbriel, Sway, River Classic and KWin** (River 0.3, `river-classic`; see below).
`mandara setup` (run by the installer) hooks Mandara into your compositor
with **one include line** in your own config, writes everything else to
`~/.config/mandara/generated/`, starts Mandara at login with an absolute
path, and checks the result with the compositor's own validator
(`mango -p`, `niri validate`, `Hyprland --verify-config`,
`umbriel config validate`, `sway --validate`), rolling back if it objects
(see [Config doctor](#config-doctor)). **River and KWin do not use include lines.** River’s
`init` is a program, not a config file, so there is no include line and
no checker. Mandara never edits `init`; it sends River's settings over
`river-control` while it runs, and you add the one autostart line
yourself (the table below shows it). KWin uses the native Plasma protocols
for workspaces and windows; its settings remain in Plasma System Settings.

| | Include line in your config | Autostart |
|---|---|---|
| **MangoWM** | `source_optional=~/.config/mandara/generated/mango.conf` | `exec_once=` |
| **Niri** 25.11+ | `include optional=true "…/mandara/generated/niri.kdl"` (plain `include` before 26.04) | `spawn-at-startup` |
| **Hyprland 0.57+** (Lua only) | `pcall(dofile, "…/mandara/generated/hyprland.lua")` in `hyprland.lua` | `hl.on("hyprland.start", …)` |
| **Hyprland ≤ 0.56** | `source = ~/.config/mandara/generated/hyprland.conf` | `exec-once =` |
| **Umbriel** | `"…/mandara/generated/umbriel.toml"` in `[include.optional] files` of `~/.config/umbriel/config.toml` (added to your list if you have one) | `general.autostart` |
| **Sway** | `include …/mandara/generated/sway.conf` at the end of `~/.config/sway/config` (or whichever file sway reads: `~/.sway/config`, `~/.i3/config`…) | `exec` |
| **River** 0.3 | none: Mandara never edits `~/.config/river/init` | the one line you add to `init`: `riverctl spawn /path/to/mandara` |
| **KWin / Plasma (Wayland)** | none: Mandara leaves `kwinrc` unchanged | its own `org.mandara.shell.desktop` XDG autostart entry, only for KDE |

The setup picker offers KWin only when `kwin_wayland` is installed. A
failed KWin availability check is reported directly; it does not offer to
retire shortcuts or label the failure a compositor-configuration error.

Mandara detects the installed version and speaks its dialect, including
Hyprland 0.57's Lua dispatches. On Niri the drop-in makes workspace
backgrounds transparent so the wallpaper shows outside the Overview; night
light needs a real Niri session on a tty (a nested Niri has no gamma
control).

On **Umbriel**, your own `config.toml` is applied after its includes, so a
value you set there wins over Mandara's drop-in (window and layer rules
from both files are kept, and binds merge by key). Two things follow from
that: live border colours only show when your config doesn't set
`[colors.border]`, and Mandara's autostart entry only counts when your
config has no `general.autostart` list of its own (add Mandara to that list
instead). Mandara reaches Umbriel over `$UMBRIEL_SOCKET`; the bar shows each
output's workspaces, the active workspace's layout (click to cycle
scrolling → dwindle → master → stacking) and the active keybind submap.

On **Sway**, Mandara talks to the compositor over its IPC socket
(`$SWAYSOCK`): the bar shows each output's workspaces (occupied, urgent,
the visible one), the windows on them, the workspace's container layout
(click to cycle stacking → tabbed → split) and the binding mode while it
isn't `default`. Sway reads its config in order, and the include line sits
at the end, so Mandara's border colours, gaps, window rules, input,
display and key settings apply on top of yours (binds you already have stay
yours: Mandara skips its own on those keys). If you have no config of your
own yet, `mandara setup` starts `~/.config/sway/config` as a copy of
`/etc/sway/config` (what sway asks you to do) before adding the line. Sway
has no rounded corners, blur, shadows, animations or per-focus opacity, so
those Studio rows are locked while it runs; `bar { … }` blocks (swaybar)
and `output * bg …` (swaybg) are found as another bar and wallpaper (see
below).

**River**'s config is its `init` file, an executable program (usually a
shell script of `riverctl` commands), so Mandara never edits it and adds no
include line. Instead the shell sends River its settings over River's own
`river-control` protocol when it starts and on every change: border colours
and width, window rules (`rule-add` float / no-float and `tags` for windows
on every tag; River matches with simple globs, so a rule whose regex needs
more than a `*` at either end is left out and reported), keyboard layout
and repeat, mouse and touchpad settings (every pointer first, then devices
named `…Touchpad…`), the cursor, focus-follows-cursor and its keys (`map
normal …`). Before each sync it takes its own previous binds and rules back
(`unmap`, `rule-del`), and keys your `init` maps are left alone; a record of
what it sent is kept in `~/.config/mandara/generated/river.conf` (River
doesn't read that file). To start Mandara with River, add one line to your
`init`:

```sh
riverctl spawn /home/you/.local/bin/mandara
```

The bar follows River over its `river-status` protocol: each output's tags
as a bitmask, like MangoWM's (click shows a tag, right-click adds it to the
view, middle-click toggles it on the focused window), which are in use or
urgent, the layout generator's name (with rivertile, the pill moves the
main area round the screen) and the active mode. River's protocols don't
say which window is on which tag, so the tag pills show no app icons;
windows come from the foreign-toplevel protocol. River has no gaps (they are
the layout generator's), rounded corners, blur, shadows, animations,
opacity rules, output settings or Num Lock option, so those Studio rows are
locked while it runs. This is River's classic line, 0.3 (`river-classic` in
Arch): River 0.4 moved window management into separate programs and dropped
`river-control` and `river-status`, so on 0.4 and later Mandara can't
drive it, and the Config doctor says so.

### External monitor brightness

The installer offers to enable external-monitor brightness through DDC/CI.
Accepting loads `i2c-dev` when needed, records it for future boots, and
adds your user to the device group if necessary. Log in again after a group
change, and enable DDC/CI in your monitor's own menu. Existing access in the
current boot does not skip the startup-module check. Studio → Displays →
External brightness shows module and permission diagnostics.

### Plasma / KWin

Start Mandara inside a **Plasma Wayland** session. The installer recognizes
an existing KWin installation; it does not install a complete Plasma desktop
as a standalone window-manager choice. To enable startup explicitly:

```bash
mandara setup --compositor kwin --autostart
```

The native KWin driver follows virtual desktops, focused windows, window
state and outputs through Plasma’s Wayland protocols. Workspace clicks,
window moves, the dock and the switcher use those same protocols. Clipboard
history uses ext-data-control, with the same privacy and size limits as on
the other compositors. No KWin
scripts or helper shell are installed. Mandara writes only its own XDG
autostart entry; a custom entry with the same filename is left alone.
Disable Mandara’s **Compositor → Integration → Start at login** setting to turn off that entry.

Keep resolution, scaling, arrangement, input, window rules, effects and
shortcuts in **Plasma System Settings**. Studio explains these limits; its
**Displays → Shell on each display** controls still select where Mandara’s
bar, dock and other surfaces appear. The Config doctor can report KWin’s
availability, but KWin has no standalone configuration checker. Screenshots
use KWin's native ScreenShot2 service; keep its screenshot effect available.
A failed
availability probe makes `mandara setup --check --compositor kwin` exit with
status 11, just as a missing KWin executable does.

Lock requests use **Plasma’s secure lock screen**, including confirmation
before sleep, because KWin does not expose the lock-surface protocol used
by Mandara’s own locker. The Mandara lock preview remains available. Choose
lock-screen fonts and the unlock grace period in Plasma System Settings;
Studio marks Mandara’s own locker controls unavailable on KWin. Plasma’s
own controls also choose which panels and desktop components to keep;
Mandara does not remove or disable them automatically.

## Coming from another setup

Mandara never rewrites your compositor config; it only adds one include
line. Leftovers from a previous setup can still fight it:

- an old launcher or wallpaper switcher on Super+Space or Ctrl+Shift+W;
- another bar (waybar, ironbar, Quickshell shells, AGS…);
- another notification daemon (mako, dunst, swaync);
- another wallpaper daemon (swww, hyprpaper, swaybg).

The installer finds them in your config **and every file it sources or
includes**, for MangoWM, Niri, Hyprland (`.conf` and `.lua`), Umbriel
(`[include]` lists, `[keybinds]` and `general.autostart`) and Sway
(`include` globs such as `/etc/sway/config.d/*`, `set $var` variables,
`bindsym` lines and blocks, `exec` / `exec_always`, `bar { … }` blocks and
`output … bg` lines). River's `init` is read too (`riverctl map`,
`riverctl spawn`, programs started with `&`) but never changed: what it
finds there is reported for you to remove.

```bash
mandara setup --check            # list conflicts, change nothing
mandara setup --fix-conflicts    # retire them
mandara setup --restore latest   # undo
```

What `--fix-conflicts` does:

- Each conflicting line is commented out with a `mandara-retired:` marker,
  after a timestamped backup of every file it touches. On Umbriel only the
  program's entry leaves the `general.autostart` list; the rest of the list
  stays. On Sway a `bar { … }` block is retired whole.
- Keys an old tool used (an extra launcher shortcut, say) are re-bound to the
  Mandara equivalent.
- The compositor then validates the result (`mango -p`, `niri validate`,
  `Hyprland --verify-config`, `umbriel config validate`,
  `sway --validate`). If it reports more problems than before, everything
  is rolled back.

Binds that are yours, like quitting the compositor or launching your VPN,
are never touched; Mandara skips its own shortcut on those keys. Studio →
Maintenance shows the same check with a one-click fix.

## Config doctor

Compositors change their config languages between releases: an option is
renamed, a value tightens, a section moves. The **Config doctor** asks the
compositor itself: it runs its own checker (`mango -p`, `niri validate`,
`Hyprland --verify-config`, `umbriel config validate --json`,
`sway --validate --verbose`) on your real config and every file it
includes, read-only (a private runtime folder, no display, a time limit),
and explains each problem it reports (River has no checker; see below):

- **where**: the file and line, with the line itself;
- **whose**: your own files, or Mandara's generated drop-in;
- **what**: an unknown option (and whether it was *removed in the update*
  you just installed), a deprecated one, a syntax error, an invalid value,
  a duplicate keybind, a missing included file, or something Mandara wrote
  that this compositor version doesn't support.

Each problem offers fixes, and none of them is final:

- **Open in editor** at that line (`$VISUAL`, else `$EDITOR` in your
  terminal, else your default app for the file).
- **Comment out** (with the file's own comment syntax and a
  `mandara-doctor:` marker; a whole Niri node or TOML table when the problem
  spans several lines) or **Remove line**. The file is backed up first
  (`~/.local/share/mandara/doctor-backups/`), written atomically and checked
  again; if the compositor then reports more problems, the backup is put
  back by itself.
- **Regenerate** for problems in Mandara's drop-in. An option the installed
  compositor version rejects is left out of the drop-in for that version
  from then on (an update gets to try it again).
- **Remind me later**: no reminders about these problems until your config or
  the compositor changes.

It runs when `mandara setup` runs (before and after it writes anything: a
drop-in or include line the compositor rejects is rolled back), every time
the shell rewrites the drop-in, and once at start-up when your config files
or the compositor's version changed since the last check (off the UI
thread). New problems are announced with one notification whose button opens
Studio → Compositor → Config doctor; turn that off with *Tell me about config
problems* on the same page.

```bash
mandara setup --check                  # report only; exit statuses are listed below
mandara setup --fix 1a2b3c4d=comment   # fix by id from the report: comment, remove or regenerate
mandara msg --json compositor check    # the same report from the running shell, for agents
```

`mandara setup --check` leaves compositor files unchanged. It exits with 10
for shortcut conflicts, 11 for compositor configuration errors, 12 when a
required keyboard library is unavailable, or 13 for failed KWin availability
checks. The installer offers conflict retirement only after status 10.


MangoWM, Niri, Umbriel and Sway output is read as those checkers print it
(tested against MangoWM 0.18.4, Niri 26.04, Umbriel 0.1.0 and Sway 1.12).
Hyprland's is read from its documented format and hasn't been tested
against a real Hyprland yet; an output the doctor doesn't recognise is
still shown as it is.

Sway sets up its backend before it reads a config, even to validate one,
so the doctor runs `sway --validate` with a headless backend and software
rendering (`WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`) in its private
runtime folder: nothing touches your screens, seat or GPU. Sway names no
line for a key bound twice (it says "Overwriting binding"), so the doctor
finds that line by its key combination; and it skips an `include` that
matches no file without a word, so a missing include isn't reported.

River has no config checker, so for River the doctor checks what it can,
without ever changing `init`: that River's version still has the protocols
Mandara needs (0.3; it explains what changed in 0.4), that `init` is
executable, that it starts Mandara (and the one line to add if it
doesn't), and which keys it maps that Mandara binds too. Those findings
offer no comment or remove fixes. Mandara's own tests run every River
command it can send in a headless River.

## Packages

Every [release](https://github.com/theHamdiz/mandara/releases) is built by
CI for each distribution family and CPU, with build provenance
attestations:

| | x86_64 | x86_64-v3 (AVX2-era CPUs) | aarch64 (ARM64) |
|---|---|---|---|
| Any glibc distribution (tarball) | ✓ | ✓ | ✓ |
| Alpine, Void musl, Chimera (musl tarball) | ✓ | | ✓ |
| Debian, Ubuntu, Mint, Pop!_OS (`.deb`) | ✓ | | ✓ |
| Fedora, openSUSE, RHEL family (`.rpm`) | ✓ | | ✓ |
| Arch, Manjaro, EndeavourOS, CachyOS (`PKGBUILD`) | ✓ | | ✓ |

Plus the bundled plugins, a build manifest per binary (source fingerprint,
toolchain, SHA-256, lines of code), binary patches from the last five
releases for [smart updates](apps-and-updates.md#updates-by-binary-patch), one language
pack per language (`mandara-<v>-<code>.mlang`, and the compressed
`.mlang.zst` the shell downloads), `update.json` and `SHA256SUMS`. Every
package and tarball ships all language packs. The installer picks the right build for your system (glibc or
musl, x86-64-v3 when your CPU supports it); Gentoo, Solus and other glibc
systems use the tarball. NixOS builds from source with the
[flake](#nixos).

From a clone, `cargo xtask build && target/release/mandara install`
installs that checkout (or `mandara install --from-source` builds it).

**Requirements:** Linux with a Wayland compositor (MangoWM, Niri,
Hyprland, Sway, River Classic, Umbriel or KWin; the installer can install one), a Vulkan-capable
GPU, and PipeWire or PulseAudio for sound features.

## Versioning

Every crate, plugin and the shell carry their own semantic version, tracked by
content hash. Each push to `main` bumps exactly the components whose build
changed: Conventional Commits pick patch, minor or major. When the shell's
version changes, CI builds and publishes a release. `mandara --version` shows
the exact build. Details are in [docs/releasing.md](releasing.md).
