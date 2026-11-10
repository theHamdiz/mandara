# Apps and updates

Installing and removing apps, updating your system, and how Mandara
updates itself.

## App Installer: drag, drop, done

Install apps the way you would on a Mac: drop the file on the **dock** or on
**Studio → Applications**, or pick it with **Install app…**
(`mandara msg apps install <file>` works too). App Installer is a Mandara
plugin, so you can switch it off or remove it like any other.

| You drop | What happens | Password? |
|---|---|---|
| **AppImage** | read without running it; its icon, name and launcher are extracted; installed in your home folder | no |
| **Flatpak** (`.flatpak`, `.flatpakref`) | `flatpak install --user` | no |
| **Portable app** (`.tar.gz`, `.zip`, …) | unpacked safely, main program and icon found, launcher created | no |
| **Native package** (`.deb`, `.rpm`, `.pkg.tar.zst`, `.apk`, `.xbps`) | your own package manager installs it, after showing what else it would pull in | yes, through the polkit dialog |

- **A proper installer sheet:** the app's icon, version, developer,
  summary, description and up to three screenshots from inside the package,
  size and category chips, a format badge that explains itself, Flatpak
  permissions as plain rows (broad ones highlighted), native dependencies on
  request, and clear warnings (wrong CPU, older than the installed version).
  Already installed? Choose Update, Reinstall or Keep both.
- **Feedback all the way:** a "Drop to install" glow while you drag, a queue
  when you drop several files, a progress ring with its steps and elapsed
  time, a spring bounce with Open / Keep in Dock / Show in Applications when
  it's ready, and failure cards that explain what went wrong and offer a fix
  (for example "Not enough space" with Open folder, or "Find it on Flathub"
  for a package built for another distribution).
- **In Studio → Applications:** a drop card ("Drop an app here, or choose a
  file") and an *Installed by Mandara* history with source file, date, size,
  Show in Files and Uninstall.
- **On your `$PATH` too, if you like:** a `~/.local/bin/<name>` link (or a
  two-line `flatpak run` wrapper), never overwriting anything of yours.
- **Where things go:** AppImages and portable apps live in
  `~/.local/share/mandara/apps/` with a launcher entry and icon; nothing
  needs root except a native package.
- **Scriptable:** `mandara msg apps install <file…> [--yes] [--dry-run]`,
  `apps jobs` (progress, `apps jobs cancel <id>`) and
  `apps installed --by-mandara`.
- **Clean removal:** Mandara records every file it installs, so dragging the
  app off the dock or choosing **Uninstall…** removes exactly those files,
  with the same optional leftover cleanup as the uninstaller below.
- **Safe by design:** a dropped file is never run during installation,
  installs are staged and rolled back if anything fails, and a package built
  for another distribution gets a clear explanation instead of a workaround.

## Applications and the uninstaller

Studio → Applications has four tabs (`mandara studio applications --tab
installed|defaults|startup|launching`):

- **Installed**: search every app, see where it came from (pacman, APT,
  DNF / Zypper, XBPS, APK, Portage, eopkg, Nix, Flatpak, Snap, AppImage or a
  local entry), and open, hide, rename or uninstall it.
- **Defaults**, **Startup** (apps at login) and **Launching** (wrapper,
  systemd scope, terminal for terminal apps).

The **uninstaller works on any distribution**. Mandara asks the package
managers that are actually installed who owns the app, then shows the exact
command and everything else the package manager would remove before you
confirm. System packages are removed through polkit (never sudo), with
essential packages, your compositor and Mandara itself refused; Flatpak and
Snap use their own tools, and AppImages and local entries go to the Trash.
**Leftover cleanup is optional**: the settings, data and cache folders the
app left in your home folder are listed with their sizes, all unchecked, and
whatever you tick moves to the Trash; nothing is deleted outright. It's
scriptable too: `mandara msg apps owner <id>` and
`mandara msg apps uninstall <id> [--purge-data] [--dry-run]`.

## System updates

The **Updates** panel (an opt-in plugin: Studio → Plugins → Updates) checks
your package managers every few hours and installs what it finds right in
the panel. Press **Update all**, or **Update** on one source:

- **Your password, in Mandara.** The update runs as administrator through
  polkit, so Mandara's own password dialog asks for it, never a terminal,
  and nothing stores it. The privileged part is Mandara's root-owned admin
  helper (`/usr/local/lib/mandara/mandara-admin`, or your package's
  `mandara`), which accepts only a handful of fixed actions and runs only
  the fixed commands below: no command line ever crosses over. The first
  time, or after an update brought a newer protocol, setting up the helper
  asks for one more approval.
- **Live progress.** The card in the panel shows the phase (refreshing
  databases, downloading 3/46, checking packages, installing 12/46, running
  hooks), a progress bar, the step, the elapsed time and the latest lines of
  the package manager's output (*Show log*; *Full log* opens
  `~/.cache/mandara/system-update.log`). Close the panel and the update
  carries on; open it again to see where it is.
- **Cancel when it's safe.** *Cancel* is offered while databases refresh
  and packages download, and never once installing has started.
- **A clear ending.** The summary says how many packages changed and how
  long it took, and suggests a restart when the kernel, the init system,
  the C library or the graphics driver changed (or the package manager asks
  for one). *Restart…* opens the session menu.
- **Problems explained.** Another program holding the package database, no
  network, a conflict that needs your decision, a full disk or a signature
  problem each get their own message. Mandara never forces anything
  (no `--overwrite` or similar): a conflict stops the update and shows the
  output, so you can finish it in a terminal.
- **Flatpak** updates afterwards as you (`flatpak update -y
  --noninteractive`), no password needed for your own apps.

| Family | Refresh | Download | Install |
|---|---|---|---|
| Arch and derivatives | `pacman -Syuw --noconfirm` (sync and download) | | `pacman -Su --noconfirm` |
| Debian, Ubuntu | `apt-get update` | `apt-get -y -d dist-upgrade` | `apt-get -y dist-upgrade`, keeping your changed config files |
| Fedora (dnf or dnf5) | | `dnf -y upgrade --refresh --downloadonly` | `dnf -y upgrade` |
| openSUSE Tumbleweed / Slowroll | `zypper -n refresh` | `zypper -n dup --download-only` | `zypper -n dup` |
| openSUSE Leap | `zypper -n refresh` | `zypper -n update --download-only` | `zypper -n update` |
| Void | `xbps-install -S` | `xbps-install -uyD` | `xbps-install -uy` (xbps updates itself first when it must) |
| Alpine | `apk update` | | `apk upgrade` |
| FreeBSD | `pkg update` | `pkg upgrade -U -F -y`, then `freebsd-update fetch` | `pkg upgrade -U -y`, then `freebsd-update install` |

### The AUR

On Arch and its derivatives, **Update all** also updates your AUR packages,
in the same card: pacman updates the repositories first (as administrator),
then your AUR helper builds and installs the AUR packages **as you**, then
Flatpak. The AUR source's own *Update* button updates only the AUR. Mandara
finds the helper you have installed, preferring **paru**, then **yay**,
then pikaur, Aura, trizen and CachyOS's **Shelly**; pick another under
Studio → Plugins → Updates → *AUR helper* (locked, with the reason, when
none is installed).

- **Never as root.** The helper runs as you, without a terminal, with
  fixed flags that skip its questions and PKGBUILD reviews (`paru -Sua
  --noconfirm --skipreview`, `yay -Sua --noconfirm --answer… None`,
  `shelly upgrade aur --no-confirm`…). Only AUR packages are upgraded
  there (`-Sua`): the repositories are pacman's step.
- **Its sudo asks in Mandara.** When the helper installs what it built, its
  `sudo` asks for your password in Mandara's own password dialog. sudo
  can't reuse the system step's approval (that one went through polkit),
  so the AUR step asks once more, and the dialog says why; paru's and
  yay's keep-alive (`--sudoloop`) keeps that one approval for the whole
  step, even through long builds. If only AUR packages are waiting, there
  is just that one password.
- **How the password travels.** For the step, Mandara makes a private
  folder (only you can open it) with a socket, a one-time key and two
  links to itself: `sudo`, first on the helper's `PATH`, runs your real
  sudo as `sudo -A`, and `SUDO_ASKPASS` points at `mandara-askpass`, which
  asks the shell over the socket. The shell answers only programs started
  by that helper (it checks the caller's user and process tree as well as
  the key), only while the step runs, and gives up after three minutes
  without an answer. The password never touches a file, a command line,
  the environment or the log, and its memory is wiped right after use.
  Cancelling the dialog stops the step instead of asking again.
- **Progress and cancel.** The card names the helper and the package being
  downloaded, built or installed, with built/total counts. *Cancel* stops
  the build; while the helper's pacman installs what was built, a cancel
  waits until that package is in, so nothing is left half installed.
- **When a build fails**, the card names the package and the line that
  said why (`cbonsai didn't build: PKGBUILD: line 19: make: command not
  found`). Builds that fail, and conflicts or signatures that need your
  decision, offer **Open in terminal**, which runs the helper's own
  command (`paru -Sua`…) where it can ask you.

### Other systems

Image-based systems (Fedora Atomic's `rpm-ostree`, Bazzite, Bluefin and
Aurora included, openSUSE MicroOS / Aeon / Kalpa's `transactional-update`,
Vanilla OS's `abroot`, SteamOS's `steamos-update`) update with their own
tool, and the panel says so. It checks only Flatpak there, since the
package manager such a system carries (dnf, zypper, apt, pacman) can't
update its image ([Immutable systems](install.md#immutable-systems)). Prefer a terminal anyway? Set **Run updates → In a terminal** in
the plugin's settings (the AUR included). Scriptable: `mandara msg updates
apply [all|system|aur|flatpak]`, `updates job` (phase, progress, recent
log lines, summary) and `updates cancel`.

**If no password dialog appears**, polkit is missing or its agent isn't
running: install your distribution's polkit package (`polkit`, `polkitd`
and `pkexec` on Debian), and keep Studio → General → Security → *Authentication dialogs* on.
`mandara install` installs both.

### FreeBSD

On FreeBSD the panel counts your package updates with
`pkg version -vRUl '<'` (read-only, against the catalogue as pkg last
fetched it) and **Update all** updates both halves of the system, with
the same password dialog and progress:

- **Packages** with pkg: the catalogues refresh, every package downloads
  first, then they install. A kernel module from packages (`drm-61-kmod`
  and friends) suggests a restart.
- **The base system** with freebsd-update, the security and errata patches
  of your release (`14.4-RELEASE-p4`): Mandara runs it unattended
  (`--not-running-from-cron`, file lists printed into the log instead of a
  pager), downloads the patches together with the packages, and installs
  them after the packages. A patch to the kernel suggests a restart.
  Where ZFS boot environments work, freebsd-update snapshots the running
  system first, so you can boot back into it.

Downloads can be cancelled; installing never is. A system freebsd-update
doesn't manage (a -STABLE or -CURRENT build, or a packaged base) gets its
packages updated and a warning that the base system was left alone. A
release past its end of life still installs what was published for it,
with the same warning; plan the upgrade to a supported release
(`freebsd-update upgrade -r …`), which Mandara leaves to you. In a
terminal, the same update is `sudo pkg upgrade` and
`sudo freebsd-update fetch install`.

The admin helper runs these programs only from root's own folders
(`/usr/local/sbin/pkg`, `/usr/sbin/freebsd-update`), and pkexec comes with
FreeBSD's `polkit` package.

### NixOS: generations

NixOS updates through its own configuration, so on NixOS the panel manages
**generations** instead of packages:

- **Generations list:** every system generation with its number, date,
  NixOS version, kernel and configuration revision, which one is running
  and which one booted, plus your home-manager generations.
- **Update or rebuild:** *Update* refreshes your flake's inputs
  (`/etc/nixos/flake.nix`, followed when it links to your own folder) or
  your channels, then builds; *Rebuild* builds the configuration as it is.
  Pick what happens to the result: **Switch** (now and at boot), **Next
  boot**, **Test** (now, not at boot) or **Build only**. The build's own
  structured progress (paths copied, derivations built) drives the bar, and
  the summary lists the inputs that changed (old → new revision). A flake
  in your home folder is updated as you; root only builds and switches.
- **Roll back or switch** to any generation, after a clear confirmation.
- **Compare** a generation with the running one: the packages that changed
  (old → new version) and how much the closure grew or shrank.
- **Clean up:** keep the newest N generations (5 by default, Studio →
  Plugins → Updates → *Generations to keep*), delete the rest and collect
  garbage, showing the space freed. The running and the booted generation
  are never deleted.
- Scriptable: `mandara msg updates nixos generations`,
  `updates nixos diff 41 44` and `updates nixos run update switch`
  (`rebuild boot`, `rollback`, `switch-to 41`, `clean keep 5`,
  `clean older-than 30`).

## Updates by binary patch

- **Smart updates.** Studio → About (or `mandara update apply`) checks
  GitHub for a new release and downloads only a **binary patch** from your
  version to the new one: in our measurements 7–16 MB instead of a 42–50 MB
  download. The patch is verified, the rebuilt binary is checked byte for
  byte against the published SHA-256, and it is swapped in atomically before
  the shell restarts. Each release carries patches from the five releases
  before it, so skipping a few updates still takes the small download. If a
  patch can't be used for any reason (or your version is older than that),
  Mandara falls back to the full download, verified the same way;
  `mandara update rollback` restores the previous version. Before the
  restart it also fetches the new version's
  [language packs](languages.md#language-packs) for the languages you use
  (about 300 KB each), and it says how the new version's size compares
  with yours.
- **Plugins too.** Mandara's own plugins are built in and update with the
  shell. Marketplace plugins update the same way, patch first with a full
  fallback (`mandara update plugins`, or **Update all** in Studio), reload
  live and stay enabled. Plugin authors get patches generated by the release
  template in [PLUGINS.md](../PLUGINS.md).
- **Polite.** Each new version is announced once, background checks respect
  offline mode, and installs owned by a package manager (pacman, apt, dnf)
  are left to it.
- **Know what changed.** Every release comes with notes in plain words: the
  update card shows what's in a new version before you install it, Studio →
  About → What's new keeps every release's notes, and `mandara changelog`
  prints them in a terminal (`--since 0.1.0` for everything an update
  brought). The full history is in [CHANGELOG.md](../CHANGELOG.md).
