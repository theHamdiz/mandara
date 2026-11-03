# Install

One command, from any shell (bash, zsh or fish). It downloads the release
for your CPU, unpacks it in a temporary folder and runs the installer,
which is the Mandara binary itself:

```bash
sh -c 'cd "$(mktemp -d)" && curl -fsSL "https://github.com/theHamdiz/mandara/releases/latest/download/mandara-$(uname -m)-linux-gnu.tar.gz" | tar xz && exec ./mandara-*/bin/mandara install'
```

Nothing is piped into a shell interpreter: `curl` feeds `tar`, and the only
program that runs is the release binary, which checks its own SHA-256
before it installs anything. Add installer options at the end (for example
`… mandara install --dry-run` to see the plan first).

Or step by step:

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

`mandara install` copies itself to `~/.local/bin`, after **checking its own
SHA-256** against the release's published checksums. Then it installs the
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

| Option | Meaning |
|---|---|
| `--latest` | Download and install the latest release instead of this binary |
| `--version X.Y.Z` | Download and install that release instead of this binary |
| `--from-source` | Build from source (installs Rust and build tools) |
| `--prefix DIR` | Install into `DIR/bin` (default `~/.local`) |
| `--no-deps` | Skip system packages |
| `--no-autostart` | Don't start Mandara with the compositor |
| `--no-greeter` | Keep your login screen |
| `--compositor mango\|niri\|hyprland` | Pick the compositor to integrate with |
| `--repo OWNER/NAME` | Releases and sources from another repository |
| `--dry-run` | Show everything that would happen, change nothing |
| `--yes` | Don't ask; take every default |
| `--no-verify` | Install even if this binary doesn't match its published checksum |

`mandara uninstall` removes the binary and desktop entry and empties the
compositor drop-ins; your settings are kept.

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
Mandara with it (see [below](#mangowm-niri-hyprland-and-umbriel)). If Home
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

## MangoWM, Niri, Hyprland and Umbriel

`mandara setup` (run by the installer) hooks Mandara into your compositor
with **one include line** in your own config, writes everything else to
`~/.config/mandara/generated/`, starts Mandara at login with an absolute
path, and checks the result with the compositor's own validator
(`mango -p`, `niri validate`, `Hyprland --verify-config`,
`umbriel config validate`), rolling back if it objects (see
[Config doctor](#config-doctor)).

| | Include line in your config | Autostart |
|---|---|---|
| **MangoWM** | `source_optional=~/.config/mandara/generated/mango.conf` | `exec_once=` |
| **Niri** 25.11+ | `include optional=true "…/mandara/generated/niri.kdl"` (plain `include` before 26.04) | `spawn-at-startup` |
| **Hyprland 0.57+** (Lua only) | `pcall(dofile, "…/mandara/generated/hyprland.lua")` in `hyprland.lua` | `hl.on("hyprland.start", …)` |
| **Hyprland ≤ 0.56** | `source = ~/.config/mandara/generated/hyprland.conf` | `exec-once =` |
| **Umbriel** | `"…/mandara/generated/umbriel.toml"` in `[include.optional] files` of `~/.config/umbriel/config.toml` (added to your list if you have one) | `general.autostart` |

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

## Coming from another setup

Mandara never rewrites your compositor config; it only adds one include
line. Leftovers from a previous setup can still fight it:

- an old launcher or wallpaper switcher on Super+Space or Ctrl+Shift+W;
- another bar (waybar, ironbar, Quickshell shells, AGS…);
- another notification daemon (mako, dunst, swaync);
- another wallpaper daemon (swww, hyprpaper, swaybg).

The installer finds them in your config **and every file it sources or
includes**, for MangoWM, Niri, Hyprland (`.conf` and `.lua`) and Umbriel
(`[include]` lists, `[keybinds]` and `general.autostart`).

```bash
mandara setup --check            # list conflicts, change nothing
mandara setup --fix-conflicts    # retire them
mandara setup --restore latest   # undo
```

What `--fix-conflicts` does:

- Each conflicting line is commented out with a `mandara-retired:` marker,
  after a timestamped backup of every file it touches. On Umbriel only the
  program's entry leaves the `general.autostart` list; the rest of the list
  stays.
- Keys an old tool used (an extra launcher shortcut, say) are re-bound to the
  Mandara equivalent.
- The compositor then validates the result (`mango -p`, `niri validate`,
  `Hyprland --verify-config`, `umbriel config validate`). If it reports
  more problems than before, everything is rolled back.

Binds that are yours, like quitting the compositor or launching your VPN,
are never touched; Mandara skips its own shortcut on those keys. Studio →
Maintenance shows the same check with a one-click fix.

## Config doctor

Compositors change their config languages between releases: an option is
renamed, a value tightens, a section moves. The **Config doctor** asks the
compositor itself: it runs its own checker (`mango -p`, `niri validate`,
`Hyprland --verify-config`, `umbriel config validate --json`) on your real
config and every file it includes, read-only (a private runtime folder, no
display, a time limit), and explains each problem it reports:

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
mandara setup --check                  # report only (exit 10 conflicts, 11 config errors)
mandara setup --fix 1a2b3c4d=comment   # fix by id from the report: comment, remove or regenerate
mandara msg --json compositor check    # the same report from the running shell, for agents
```

MangoWM, Niri and Umbriel output is read as those checkers print it (tested
against MangoWM 0.18.4, Niri 26.04 and Umbriel 0.1.0). Hyprland's is read
from its documented format and hasn't been tested against a real Hyprland
yet; an output the doctor doesn't recognise is still shown as it is.

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
releases for [smart updates](apps-and-updates.md#updates-by-binary-patch), `update.json` and
`SHA256SUMS`. The installer picks the right build for your system (glibc or
musl, x86-64-v3 when your CPU supports it); Gentoo, Solus and other glibc
systems use the tarball. NixOS builds from source with the
[flake](#nixos).

From a clone, `cargo xtask build && target/release/mandara install`
installs that checkout (or `mandara install --from-source` builds it).

**Requirements:** Linux with a Wayland compositor (MangoWM, Niri or
Hyprland), a Vulkan-capable GPU, and PipeWire or PulseAudio for sound
features.

## Versioning

Every crate, plugin and the shell carry their own semantic version, tracked by
content hash. Each push to `main` bumps exactly the components whose build
changed: Conventional Commits pick patch, minor or major. When the shell's
version changes, CI builds and publishes a release. `mandara --version` shows
the exact build. Details are in [docs/releasing.md](releasing.md).
