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
and Solus**, and prints the configuration to add on NixOS. Besides the
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

## Compositors

Mandara supports **MangoWM, Niri, Hyprland, Umbriel, Sway and River Classic** (River 0.3, `river-classic`; see below).
`mandara setup` (run by the installer) hooks Mandara into your compositor
with **one include line** in your own config, writes everything else to
`~/.config/mandara/generated/`, starts Mandara at login with an absolute
path, and checks the result with the compositor's own validator
(`mango -p`, `niri validate`, `Hyprland --verify-config`,
`umbriel config validate`, `sway --validate`), rolling back if it objects
(see [Config doctor](#config-doctor)).

| | Include line in your config | Autostart |
|---|---|---|
| **MangoWM** | `source_optional=~/.config/mandara/generated/mango.conf` | `exec_once=` |
| **Niri** 25.11+ | `include optional=true "…/mandara/generated/niri.kdl"` (plain `include` before 26.04) | `spawn-at-startup` |
| **Hyprland 0.57+** (Lua only) | `pcall(dofile, "…/mandara/generated/hyprland.lua")` in `hyprland.lua` | `hl.on("hyprland.start", …)` |
| **Hyprland ≤ 0.56** | `source = ~/.config/mandara/generated/hyprland.conf` | `exec-once =` |
| **Umbriel** | `"…/mandara/generated/umbriel.toml"` in `[include.optional] files` of `~/.config/umbriel/config.toml` (added to your list if you have one) | `general.autostart` |
| **Sway** | `include …/mandara/generated/sway.conf` at the end of `~/.config/sway/config` (or whichever file sway reads: `~/.sway/config`, `~/.i3/config`…) | `exec` |
| **River** 0.3 | none: Mandara never edits `~/.config/river/init` | the one line you add to `init`: `riverctl spawn /path/to/mandara` |

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
mandara setup --check                  # report only (exit 10 conflicts, 11 config errors)
mandara setup --fix 1a2b3c4d=comment   # fix by id from the report: comment, remove or regenerate
mandara msg --json compositor check    # the same report from the running shell, for agents
```

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
musl, x86-64-v3 when your CPU supports it); Gentoo, Solus, NixOS and other
glibc systems use the tarball.

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
