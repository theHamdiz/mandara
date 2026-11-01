# Apps and updates

Installing and removing apps, and how Mandara updates itself.

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

## Updates by binary patch

- **Smart updates.** Studio → About (or `mandara update apply`) checks
  GitHub for a new release and downloads only a **binary patch** from your
  version to the new one: in our measurements 7–16 MB instead of a 42–50 MB
  download. The patch is verified, the rebuilt binary is checked byte for
  byte against the published SHA-256, and it is swapped in atomically before
  the shell restarts. If a patch can't be used for any reason, Mandara falls
  back to the full download, verified the same way; `mandara update rollback`
  restores the previous version.
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
