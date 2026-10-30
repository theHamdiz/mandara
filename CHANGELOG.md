# Changelog

What changed in each Mandara release, written for the people who use it.
Newest first. `mandara changelog` prints it in a terminal, and Studio →
About → What's new shows it too.

## Unreleased

Multi-monitor setups get the right sizes everywhere, the clock keeps up
with time changes, and a handful of fixes from your reports on GitHub.

### Fixed

- **The bar and dock fit each monitor again.** With two monitors, starting
  Mandara right after a compositor reload could give each monitor's bar
  and dock the other one's size, leaving the laptop without a bar and a
  small bar floating in the middle of the big screen. Mandara now matches
  every surface to its monitor by name, so the order the monitors are
  listed in no longer matters.
- **Full-screen overlays cover the screen you're on.** The session menu,
  wallpaper picker, window switcher and password dialogs opened on a
  bigger second monitor used to be laptop-sized and centred. They now open
  on the monitor you're using, at its size.
- **The clock follows time changes right away.** Changing the time zone,
  switching on automatic time, or the clock being corrected from the
  internet now updates the bar, the desktop clock and the lock screen at
  once, instead of after you change the clock format or wait for the next
  minute.
- **Even buttons in Themes → Looks.** "Restore my look" was taller than
  the buttons next to it. They're all the same size now.
- **The new-note button works again.** Pressing + in the Notes panel or
  card created a note and threw it away in the same moment, because
  Mandara tidies up empty notes you leave behind and the brand-new note
  was still empty. Now the note you just opened stays put until you've
  typed something, and an empty one is only cleaned up once you leave it.

### Improved

- **Pomodoro settings are one click away.** The Pomodoro panel and card
  have a settings button that opens their Studio page, where you choose
  how long focus sessions, short breaks and long breaks last, and how many
  sessions come before a long break.
- **Your wallpaper folder follows your language.** New installs keep
  wallpapers in the Wallpapers folder inside your own Pictures folder, for
  example ~/Imagens/Wallpapers on a Portuguese system, instead of always
  ~/Pictures/Wallpapers. If you already have wallpapers in
  ~/Pictures/Wallpapers, Mandara keeps using it.
- **New lengths show right away.** Change the focus or break length while
  the timer hasn't started and it updates on the spot. A session that's
  running or paused keeps its time.

## 0.3.0 — 2026-10-30

A calmer, tidier Studio, and your clock and your location finally agree
with each other.

### New

- **Time zone and internet time in Studio.** Studio → Date, time &
  location now shows your system clock: its time zone, with a search to
  change it, and a switch to set the time automatically from internet time
  servers. Mandara asks for your password before changing either.
- **A heads-up when your clock is in the wrong time zone.** If your
  clock's time zone doesn't match the place you've set, Studio tells you,
  for example "Alexandria is in Africa/Cairo, but your clock uses
  Asia/Riyadh", and fixes it in one click. A wrong time zone puts the bar,
  the lock screen and prayer times off by an hour, and it often only shows
  when daylight saving time starts or ends. Scripts can use `mandara msg
  time` too.

### Improved

- **Studio is much easier to find your way around.** Every page is now a
  short list of collapsible sections, so instead of one long scroll you
  see what a page holds at a glance. A closed section shows a preview of
  its settings and how many you've changed, and its Reset works without
  opening it. The first section opens for you, Studio remembers the ones
  you open, and pages with several sections have Expand all and Collapse
  all next to the title. Search still opens everything.
- **Related settings live together.** The bar's layout, spacing and screen
  frame are now separate sections, and so are its drop-down panels and how
  they animate. Pop-ups are split into where they appear and what they
  show, the launcher's results have their own section, and the Themes,
  Wallpaper and Launcher sections are ordered from everyday to occasional.
- **Long extras tuck away.** Designed themes, MangoWM's tag & layout
  studio, which displays show each part of the shell, and the sound
  previews start folded, so the settings you use most come first.
- **One location really is one location.** Weather and prayer times now
  follow the location in Studio → Date, time & location by default, so
  typing your coordinates there is enough. A plugin only uses a place of
  its own if you give it one, and "Use one location everywhere" still
  overrides even that.
- **Clearer "Locate me" errors.** When your system's location service
  (GeoClue) can't find you, which happens a lot because many Linux systems
  ship it without a lookup service, Mandara now says so and suggests IP
  lookup or typing your city.

## 0.2.0 — 2026-10-30

A lock screen that accepts your password, brightness controls for laptops,
and release notes you can read right inside Mandara.

### New

- **Brightness slider.** The sidebar's quick settings now have a
  brightness slider, right under the toggles. It shows up on laptops, and
  on desktops whose monitors support DDC/CI.
- **Brightness keys just work.** The laptop brightness keys now change the
  screen brightness out of the box, with the on-screen display. If your
  compositor config already binds them, Mandara leaves yours alone.
- **What's new in Studio.** Studio → About has a new What's new card with
  every release's notes. The version you're running starts open, and you
  can open any other release to read what it brought.
- **See what's coming before you update.** When a new version is out, the
  update card in Studio and `mandara update` list what's in it before you
  install it, and `mandara update apply` shows what's new once it's done.
- **`mandara changelog` in your terminal.** It shows this version's notes,
  or `mandara changelog 0.1.1` for one release, `--since 0.1.0` for
  everything an update brought, and `--all` for the whole history.
  `--markdown` and `--json` are there for scripts.

### Fixed

- The lock screen could say your password was wrong when it wasn't. This
  happened when Mandara's own PAM file was installed, for example by the
  Arch packages: it only described how to check passwords, so the extra
  account check fell back to a rule that refuses everyone. The lock screen
  now only checks your password, like other screen lockers do, and the PAM
  file is complete too. You don't need to change anything; updating fixes
  it. Thanks for reporting it on GitHub!

### Under the hood

- Each GitHub release now uses its changelog entry as its release notes,
  and the release pipeline dates the entry when it ships.
- The demo sandbox used for screenshots and tests can no longer reach real
  monitors over DDC/CI.

## 0.1.2 — 2026-10-30

A small release that makes notifications and the notification center get
along.

### Fixed

- Opening the notification center now clears the pop-up cards off the
  screen. The cards that were showing move into the list, and anything
  that arrives while the center is open lands straight in it, already
  marked as read, instead of popping up on top of it. Close the center
  and new notifications pop up as usual again. This works the same for
  the notification panel and the sidebar's Notifications tab. Thanks for
  the report on GitHub!

## 0.1.1 — 2026-10-30

The first round of fixes from your feedback on 0.1.0. Thank you to
everyone who tried it, reported bugs and opened issues.

### Improved

- **Calmer out of the box.** Fewer plugins start switched on (21 of 43),
  so a fresh install feels lighter. Wallhaven browsing and cursor themes
  are now optional plugins. They're off by default, and off really means
  off: no network calls and no cursor changes. If you're upgrading, you
  keep exactly the plugins you had.
- **Theming your other apps is now your choice.** GTK, Qt/Kvantum,
  terminals, editors and compositor borders all start switched off.
  Mandara remembers what it changes, and switching an app off puts its
  original files back. Coming from 0.1.0? Your existing files stay as
  they are. To hand an app back to Mandara, or to clean it up, switch it
  on (or on and off again) in Studio → Themes → Other apps.
- **Tidier Studio.** Scroll bars are slim and hide when you're not using
  them. Looks and Designed themes show as compact swatch lists, with big
  previews one click away, and Other apps is a neat grid of tiles.
- **Liquid MangoWM tags.** Every active tag gets its own pill, and
  neighbouring active tags melt into each other with a smooth liquid
  bridge. Before, viewing tags 1 and 3 together only highlighted tag 1.

### Fixed

- Volume sliders follow your pointer again, even on slower machines.
  They draw where your pointer is while you drag, keep dragging when the
  pointer slips off the thin track, and only send the newest value to the
  audio server, so nothing lags behind. Every slider in the shell gets
  this.
- Rows in Studio light up as soon as your pointer moves onto them, not
  only after you scroll. This was a small GPUI quirk, fixed for every
  hover effect in Studio and the shell's panels.
- The wallpaper picker no longer leaks memory. It used to grow by about
  40 MB every time you opened it. It also can't get stuck invisible while
  holding on to your keyboard, it opens at the right size on every
  display, and an unreadable image now shows a cross instead of loading
  forever.
- The desktop's right-click menu opens at your pointer on displays you
  plug in after Mandara starts, and it can no longer crash the shell by
  asking for a zero-size window.

### Under the hood

- Installing from the Arch recipes in `packaging/aur` works now. Every
  release fills in their real version and checksums.
- The release pipeline's version bump got two fixes, so releases build
  cleanly from start to finish.

## 0.1.0 — 2026-10-29

The first public release of Mandara Shell: a design-first Wayland desktop
shell for MangoWM, Niri and Hyprland, written entirely in Rust on GPUI.

### New

- **One fast native binary.** The bar, dynamic island, dock, launcher,
  sidebar, notification center, wallpaper engine, lock screen, login
  manager and settings studio all run in a single process, rendered on
  the GPU with real spring physics. The whole shell stays under 100 MiB
  of RAM, with about 0.1% CPU when idle.
- **First-class MangoWM, Niri and Hyprland.** Tags or workspaces, live
  window rules, keybinds and layouts. Mandara never overwrites your
  config: it writes its own drop-in file and adds a single include line.
- **Mandara Studio.** Every setting, applied the moment you change it:
  colours, bar styles, spring physics, panels, the compositor, audio,
  displays, login screen and plugins.
- **Colour that follows your wallpaper.** Material 3 palettes extracted
  from your wallpaper, smooth OKLCH colour transitions, and Alexandrian
  signature themes (Mandara Corniche, Montaza Twilight, Mandara Obsidian)
  next to community favourites.
- **Wallpapers with depth.** A desktop clock that tucks behind the
  subject of your photo, AV1 video wallpapers, and a built-in wallpaper
  generator with twelve styles, up to 8K and seamless video loops.
- **Plugins.** 43 built-in plugins, plus sandboxed WebAssembly plugins
  from a community marketplace. Community plugins are checked before they
  install, and ones that fail the check are flagged.
- **Updates by binary patch.** `mandara update` downloads a few
  megabytes instead of the whole release, with the full download as a
  fallback.
- **One-command install.** `mandara install` sets up the binary, the
  packages it needs, your compositor and the login screen, and
  `--dry-run` shows you everything first.
- **Ready for scripts and agents.** Almost everything the UI does is a
  documented `mandara msg` command with JSON replies.
- **Ten languages**, including right-to-left Arabic.
