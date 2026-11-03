# Changelog

What changed in each Mandara release, written for the people who use it.
Newest first. `mandara changelog` prints it in a terminal, and Studio →
About → What's new shows it too.

## Unreleased

### New

- **A Modern style for the Power panel.** Pick it in Studio → Power &
  idle (*Power panel style*) and the battery leads: a large charge ring
  with the percentage and its state inside, the time left right under it,
  health, capacity and chemistry at a glance, and a smooth graph of the
  last hours of charge. The power profiles become one simple Power saver /
  Balanced / Performance switch showing only the profiles your computer
  has, and the charge limit is a single switch that says when charging
  stops and resumes (shown only where it can really be changed). Voltage,
  cycles, temperature and the CPU driver wait in a *Details* section that
  stays folded until you open it. The panel you know stays the default,
  as the Classic style. The design follows a suggestion in #26.
- **The battery breathes while it charges.** Plug in and the battery in
  the bar gently pulses until it stops charging, so you can tell at a
  glance that power is going in. It stays still with Reduce motion on, and
  you can turn it off in Studio → Power & idle (*Pulse the battery while
  charging*).

### Changed

- **No more CPU driver names in the Power panel.** The Power panel keeps
  its look, but no longer shows the CPU driver (`intel_pstate` and
  friends) in its header or under each power profile; each profile keeps
  its short description.
- **A tidier layout designer, with search.** In Studio → Bar, the Left,
  Center and Right zones are now neat columns with their plugins stacked
  in order, and every plugin sits in an aligned grid underneath, grouped
  by category, with a search box that filters as you type. Plugins already
  on the bar show where they are. Click any plugin (or bar item) for a
  small menu to add it to a zone, move it up, down or across, or take it
  off, so dragging is no longer the only way. Thanks for the suggestion
  in #26!

## 0.9.0 — 2026-11-03

Notifications can join the bar. Pointing at a notification always pauses
it.

### New

- **Notifications can join the bar.** Turn on Studio → Notifications →
  **Join the bar** and new notifications drop out of the bar in a drawer of
  its own colour, curving into it, so the two read as one piece. It works
  with a top or bottom bar, notifications on the left, middle or right, and
  follows the bar when it hides. Floating pill bars keep the usual floating
  cards. Off by default.

### Fixed

- **Pointing at a notification always pauses it.** A notification that
  popped up right where the pointer had last left the notifications kept
  counting down while you pointed at it. Now it waits for you.

## 0.8.3 — 2026-11-03

Lighter wallpapers.

### Changed

- **Lighter wallpapers.** A wallpaper smaller than your screen, and the
  depth clock's mask, are now scaled up a row at a time instead of through
  a full-size working copy. On a 4K display, switching to a 2880 × 1620
  wallpaper peaks at 125 MiB instead of 200 MiB, and the start-up peak
  with a 1080p wallpaper and the depth clock drops from 205 to 170 MiB
  (147 to 116 MiB on a 1600 × 1000 display). The scaling is faster too:
  53 ms instead of 96 ms to bring a 1080p picture to 4K. The picture is
  pixel-for-pixel the same, and memory once settled, video memory and
  frame times are unchanged.

## 0.8.2 — 2026-11-03

The sidebar stays open at the screen edge. The session dock stays on its
edge.

### Fixed

- **The sidebar stays open at the screen edge.** With hover to open on,
  resting the pointer on the sidebar's outer edge no longer makes it close
  and reopen over and over. Its scrollbar sits there, and the sidebar took
  the pointer reaching the scrollbar for the pointer leaving.
- **The session dock stays on its edge.** While it springs open, the
  session dock no longer pulls away from the screen edge and leaves a gap;
  it stretches through the bounce instead.

## 0.8.1 — 2026-11-03

A lighter start.

### Changed

- **A lighter start.** Right after it starts, the shell now holds far less
  memory: 15 seconds in, 37–39 MiB instead of 111–123 MiB on a 4K display.
  Its start-up peak is 11 MiB lower, 30–37 MiB lower with a wallpaper,
  because fonts for other scripts load one face each until you see text in
  them, and a wallpaper is put on screen without full-size copies of
  itself. Memory once settled, video memory and frame times are unchanged.

## 0.8.0 — 2026-11-02

Side bars. Mandara speaks Persian, Korean, Urdu and Bengali. Swipe a
notification away. App icons share one shape.

### New

- **Side bars.** A slim bar of icons and pills on the left and right
  edges, holding any plugins you like in three groups; their panels pour
  out sideways. By default they join the top bar into one frame with
  curved corners, or they float apart as glass pills, an island or a slab.
  Pin each one, or let it slide away until your pointer touches its edge,
  and style them to your heart's content: width, item size and style,
  corners, glass, rim light, an accent colour per side and more. Turn them
  on in Studio → Bar → Side bars (or `mandara msg side-bars on`). While
  they're on they take over the widget sidebar's edge; your sidebar comes
  back just as you left it when you turn them off.
- **Mandara speaks Persian, Korean, Urdu and Bengali.** The whole
  interface, Studio and the login screen, with Persian and Urdu laid out
  right to left. Pick yours in Studio → General → Language, or leave it on
  System to follow your desktop.
- **Swipe a notification away.** Drag a notification card sideways and let
  go: it leaves the screen but stays in the notification center. The × still
  dismisses it for good. ([#24](https://github.com/theHamdiz/mandara/issues/24))

### Changed

- **App icons share one shape.** Every app icon in the launcher (all three
  looks), the dock and its Applications drawer now sits on the same
  squircle, so icons drawn as squares, circles or free shapes read as one
  set. Prefer circles, or each icon as its theme draws it? Studio →
  Launcher → App icon shape.
- **Tidier Launcher and Displays pages in Studio.** The launcher looks are
  now three cards of exactly the same size, followed by placement, the
  launcher button, search & results, behaviour and your apps, in that
  order. Displays shows the arrangement with a card per display (so a
  switched-off display is easy to pick again), the selected display's
  settings in neat aligned rows, which displays show each part of the
  shell, and your profiles as equal cards with Apply and Delete.
- **Desktop widgets page, redone.** The preview and your placed widgets
  sit side by side at the top, the selected widget's settings come in
  tidy cards (Style, Place, Look, Shape and frame, Content), and the
  Add a widget gallery is an even grid of tiles that are all the same size.
- **A proper font picker.** Font settings open a floating list next to the
  button instead of stretching the whole row: search right away, move with
  the arrow keys, Page Up / Page Down, Home and End, pick with Enter, and
  close with Esc or a click anywhere else.
- **Studio uses big screens.** On wide windows pages grow wider, card grids
  gain columns, settings sit in two columns, and the plugin store's
  category chips spread across the width; on narrow windows the chips
  become a strip you can scroll or drag.
- **A new logo.** Mandara's logo is now a phoenix rising from a
  Mediterranean wave, in violet, cyan and silver, and the launcher button
  in the bar shows it in colour. Prefer another icon? Pick one in Studio →
  Launcher → Button icon. ([#23](https://github.com/theHamdiz/mandara/issues/23))
- **A tidier widget gallery.** Look-alike widgets now share one tile in
  Studio → Desktop widgets: the clocks are Stacked, Headline, Day block,
  Analog, Digital, Progress, Greeting and Text (plus the depth clock), and
  the monitors are System monitor and Thermals. Pick a look from the
  widget's new **Style** row, with a small preview of each; the widget
  stays where it is and keeps your colours and font. Widgets you already
  placed keep their exact look. ([#23](https://github.com/theHamdiz/mandara/issues/23))
- **Less memory from the start.** Every surface the shell opens (the bar,
  the dock, the invisible strips that reserve the bar's space, popovers,
  the launcher) used to compile its own copy of the drawing shaders. They
  are built once now and shared. Measured right after start: about
  1.4 MiB less of the shell's own memory with the default setup on a 4K
  test session, and 4 MiB less (36.4 → 32.4 MiB) in the demo desktop with
  the dock. Windows that open later skip that work too.

### Fixed

- **Floating panels drag smoothly.** A panel popped out onto the desktop
  now follows your pointer exactly, frame by frame, wherever you drag it.
  Before, it only moved once the pointer had slipped off its grab strip,
  so it stuck and jumped, and letting go over the card often didn't save
  where you put it. You can grab it by its whole top strip or its edges
  (never the panel's own buttons), a plain click never nudges it, and its
  place is saved once, when you let go.
- **Wi-Fi switched off no longer says "Connecting…".** With Wi-Fi off and a
  container or virtual-machine network up, the Network card claimed it was
  connecting. It now reads "Wi-Fi is off".
  ([#24](https://github.com/theHamdiz/mandara/issues/24))
- **Notifications don't get stuck after a click.** Clicking a notification
  usually brings its app forward, and the card thought the pointer was still
  on it, so its countdown froze until you pressed ×. It now carries on as
  soon as the pointer is gone.
  ([#24](https://github.com/theHamdiz/mandara/issues/24))
- **The Screenshot panel gets out of the way at once.** Clicking Region,
  Window, Screen or Record region in the bar's Screenshot panel used to
  fold the panel away slowly, so it lingered on screen and could end up in
  the shot. It now disappears instantly, and the capture waits until it's
  gone.
- **A region selection never gets stuck.** Letting go of the mouse over the
  capture toolbar or past the edge of the screen left the selection stuck,
  and the next click didn't capture. The drag now always ends where you
  let go.
- **Japanese and Chinese stay sans-serif.** Semibold Japanese and Chinese
  text (headings, the selected row) no longer switches to a serif font, and
  Japanese characters use their Japanese forms even when your system
  language is something else.

## 0.7.0 — 2026-11-02

Shortcuts cheatsheet. Task progress. Eye breaks. Presentation mode.

### New

Twelve new plugins. They all ship switched off: turn on the ones you want
in Studio → Plugins, where a new **For developers** shelf collects the
developer tools.

- **Shortcuts cheatsheet.** Every keyboard shortcut, Mandara's and your
  compositor's, in one searchable sheet. Bind
  `mandara msg keybinds toggle` to a key of your choice.
- **Task progress.** Scripts, long builds and AI agents can put a progress
  ring in the bar (`mandara msg progress set build 40%`), with a
  notification when the task is done or fails.
- **Eye breaks.** Gentle 20-20-20 reminders that count only the time you're
  actually at the computer and wait while a game or video is fullscreen.
- **Presentation mode.** One switch for talks and screen sharing: do not
  disturb, keep the screen awake and hide the desktop widgets. Turning it
  off puts everything back the way it was, even if the shell restarted in
  between. It can also switch itself on while you share your screen.
- **Share to phone.** Send clipboard text, a screenshot or a file to your
  phone: scan the QR code and it downloads over your own network, once.
- **Tailscale.** Your tailnet at a glance: peers, direct or relayed
  connections, exit nodes, and connect or disconnect.
- **Uptime monitor.** Watches your sites and services and lets you know
  when one goes down or comes back.
- **Local AI.** See which Ollama models are loaded and how much GPU memory
  they use, and unload them before you start a game.
- **Listening ports.** Which apps listen on which ports and who can reach
  them; open one in the browser or end the app.
- **Git repositories.** Uncommitted changes and unpushed commits across
  your projects, with a terminal one click away.
- **CI status.** GitHub Actions runs of your repositories in the bar, with
  a notification when a run finishes.
- **Crash reports.** Apps that crashed this week, with the report to copy
  and a debugger one click away.

- **Pin a panel, or pop it out.** Point at any open panel and a small tab
  appears with two buttons. **Pin** keeps the panel open while you click
  around and work in other windows. **Pop out** lifts it off the bar into a
  floating card you can drag anywhere and keep beside your work, as many as
  you like. Floating cards come back where you left them after a restart
  (Studio → Bar → Drop-down panels). Also `mandara msg panel pin|float|dock`.
- **Jannah.** A gentle daily reminder of Jannah: a verse or hadith, its
  meaning and one good step for today, in the bar, the sidebar or on the
  desktop. Switched off by default; turn it on in Studio → Plugins.

- **Qur'an** and **Hadith**. Two companion plugins: a Qur'an reader with
  recitation, and a library of the hadith collections. Both are switched off
  by default and download their content only when you turn them on.

- **Mandara speaks Bahasa Indonesia.** The whole interface, Studio and the
  login screen are translated, and dates follow Indonesian conventions.
  Pick it in Studio → General → Language, or let Mandara follow your
  system language.
- **Many more quotes for the Quote widget, all real.** When you haven't
  written your own, the desktop Quote widget now picks from 152 sayings
  instead of six,
  from Ibn Khaldun, al-Mutanabbi and Gibran to Confucius, Seneca, Bashō,
  Tagore, Rumi, Marie Curie, Achebe and Frida Kahlo, plus a few proverbs.
  Every one was checked against its source, so no made-up "famous quotes".
  The card names the work under the author when it fits ("Seneca, On
  Providence"), and each quote is translated into all of Mandara's
  languages, in the author's own words where the original exists.

- **Hide the slider handle.** Prefer a plain fill? Set Themes → Style →
  **Slider handle** to *None* and sliders show just their rounded fill, with
  no handle line. ([#20](https://github.com/theHamdiz/mandara/issues/20))

- **Launcher looks.** Studio → Launcher has a new **Launcher looks** picker
  with three ready-made looks, one click each: **Classic** (the list
  launcher you know, still the default), **Grid** (every app as a large
  icon with category chips; arrows move in all four directions and the
  tiles glide into place as you type) and **Beacon** (a centred glass
  search bar in the style of macOS Spotlight, with Apps, Files, Actions
  and Clipboard buttons beside it and your results grouped under it). You
  can still change any setting afterwards. Scripts can switch too:
  `mandara msg launcher look grid`. The launcher now also frees every icon
  it loaded when it closes.

### Fixed

- **Sliders keep up with your pointer.** Dragging a slider in Studio, such
  as Bar → Height or Notch width, used to trail behind the pointer: every
  tiny move rewrote the whole configuration, and the bar's height made the
  compositor re-tile all your windows, Studio's own included, on every
  step. The handle and its value now follow the pointer on the very next
  frame, the setting is applied at most once a frame, and windows move
  once, when the slider rests. Compositor gaps and borders are rewritten
  once the slider rests too. ([#20](https://github.com/theHamdiz/mandara/issues/20))
- **Bar groups never overlap.** A long window title could run underneath
  the clock in the middle of the bar. Each group now gets the room up to
  its neighbour, a long title ends in an ellipsis there, and on a very
  narrow bar the items nearest the middle make way first. This works in
  every bar style and on every edge. ([#21](https://github.com/theHamdiz/mandara/issues/21))
- **Workspace numbers stay centred.** When an app's icon couldn't be drawn,
  its workspace pill kept an empty space for it and the number sat off to
  one side. A broken icon now falls back to the app's coloured letter, so
  the pill always shows what it made room for. ([#20](https://github.com/theHamdiz/mandara/issues/20))
- **The depth model's download shows its progress.** Studio → Wallpaper
  (and the Wallpaper depth plugin's settings) now shows a live progress bar
  while the depth model downloads, an error with **Retry** if it fails, and
  **Installed** once it's there, with a button to start using it. Before,
  the Download button gave no sign of anything happening.
- **Two monitors, one panel.** Clicking something in the bar opened its
  panel on every monitor's bar at once, and the sidebar slid in on every
  screen. A panel and the sidebar now open only on the monitor you used
  (a shortcut or command opens them on the monitor you're on).
  ([#22](https://github.com/theHamdiz/mandara/issues/22))
- **Desktop widgets per monitor.** Right-click a widget in edit mode and
  pick **Only on this display**, so each screen keeps its own widgets.
  Studio → Desktop widgets → Displays now lists every connected monitor,
  not just the ones in a saved display profile.
  ([#22](https://github.com/theHamdiz/mandara/issues/22))
- **The login screen stays on one monitor.** With two monitors, the
  login screen was stretched across both. It now shows on one, and
  updating Mandara moves an existing login screen over too.
  ([#22](https://github.com/theHamdiz/mandara/issues/22))
- **Filter chips scroll with any mouse.** The clipboard's categories, the
  to-do lists, the note topics and the dock's app categories now scroll
  sideways with a normal mouse wheel, can be grabbed and dragged, and show
  arrows when there are more chips off to the side.

### Changed

- **Sliders have a new look.** Every slider, in Studio and in the shell's
  panels and sidebar, now has a rounded fill, a slim upright handle with a
  small gap on each side, a softer track with a dot at its end, and a
  little spring when you press it. Studio's sliders show their value in a
  bubble above the handle while you drag. ([#20](https://github.com/theHamdiz/mandara/issues/20))
- **A quicker start.** Loading the built-in plugins at startup now takes a
  fraction of the memory work it did (under half a megabyte instead of
  about 16 MB), so the shell is ready a little sooner.
- **A roomier clipboard.** Each entry's buttons appear when you point at
  it, so code, links and text use the whole width of the row. Pinned and
  collected entries keep a small mark. Code previews are only as tall as
  the code, and short snippets read "1 line".

## 0.6.0 — 2026-11-02

Rounded screen corners are on by default.

### Changed

- **Rounded screen corners are on by default.** New setups round all four
  corners of every display, and the edge-frame bar's outer corners round
  with them. If you set Mandara up before this release, turn them on in
  Studio → Sidebars & hover → **Screen corners**; the **Radius** slider is
  next to the switch. ([#19](https://github.com/theHamdiz/mandara/issues/19))

## 0.5.2 — 2026-11-01

The launcher and Alt+Tab open on the right display. Choose the displays
for the launcher and pop-ups.

### Fixed

- **The launcher and Alt+Tab open on the right display.** With more than
  one monitor on Niri, the launcher, the window switcher and the password
  and pairing prompts always opened on the same monitor, whichever one
  Niri listed first, instead of the one you were on. They now follow your
  focus on Niri, as they already did on MangoWM, Hyprland and Umbriel.
  ([#18](https://github.com/theHamdiz/mandara/issues/18))

### New

- **Choose the displays for the launcher and pop-ups.** Displays → Shell
  on each display has a new **Launcher & pop-ups** row for the launcher,
  Alt+Tab, the app installer and the password and pairing prompts. Until you
  set it, it follows **On-screen displays**, so if those show only on your
  main display, the launcher and Alt+Tab now open there too. Choose **All**
  to have them open on whichever display you're on.

## 0.5.1 — 2026-11-01

A missing wallpaper file no longer spins the shell. Big wallpapers switch
with far less memory.

### Fixed

- **A missing wallpaper file no longer spins the shell.** If your
  wallpaper was deleted or moved, Mandara kept trying to load it in a tight
  loop, filling the log and, over time, crashing. It now tries again every
  30 seconds and stays idle in between.

### Improved

- **Big wallpapers switch with far less memory.** Switching to an 8K
  picture used to need about 770 MB for a moment; it now needs about
  300 MB, and a 4K switch needs no extra memory at all. The picture is
  decoded once for everything that uses it (the desktop, the frosted
  menu, the depth effect and the theme colours), and large shrinks no
  longer go through a huge temporary buffer.

## 0.5.0 — 2026-11-01

Umbriel joins MangoWM, Niri and Hyprland; a config doctor that asks your
compositor what's wrong and helps you fix it; desktop widgets as plugins,
with dozens of new widgets and shapes; a depth effect that can use a small
model; a richer clipboard, calendar, bar and wallpaper page; Alt+V for the
clipboard and Print for screenshots.

### New

- **Config doctor: your compositor checks your config.** Mandara asks
  MangoWM, Niri, Hyprland or Umbriel itself whether your config is fine,
  using its own checker on every file your config includes, and explains
  each problem in plain words: where it is, whether it's in your files or
  Mandara's, and what's wrong (an option a compositor update removed or
  renamed, a typo, a broken line, a duplicate keybind, a missing file).
  Studio → Compositor → Config doctor opens the line in your editor,
  comments it out or removes it — always with a backup, checked again and
  undone by itself if it made things worse — and regenerates Mandara's own
  file, leaving out anything your compositor version doesn't support. It
  runs with `mandara setup`, whenever Mandara rewrites its file, and once
  at start-up after your config or compositor changed, with one
  notification for new problems (and *Remind me later*). In a terminal:
  `mandara setup --check` and `mandara setup --fix <id>=comment`.
- **Desktop widgets take any shape.** Every desktop widget can now sit
  on a circle, a squircle, a scalloped cookie, a flower, a pentagon, a
  slanted card, chamfered corners or torn paper, with an outline, a soft
  or hard shadow in any direction, a header line and a frame: a classic
  window, washi tape or a polaroid. Turn and mirror any widget too.
  Studio → Desktop widgets → Shape and frame has them all; widgets you
  already have look exactly as before.
- **Nine new clock faces.** A stacked card clock, the time in a shape
  (that can turn a little every minute), dot-matrix and pixel digits (with
  Eastern Arabic digits drawn dot by dot), a digit in each box with a week
  strip, a slanted HUD clock with the part of the day and the moon, hands
  over giant numerals, an orbit clock, and text lines you write yourself
  from placeholders like `{weekday}` and `{HH}:{mm}`. The analog clock
  gains scalloped and flower dials, dots or numbers, blade hands, a
  seconds dot and the date in a bubble. The greeting clock takes your own
  greetings and a second line.
- **Day, week, month and year progress** as a ring, a wavy ring, a bar or
  a year of dots, with how much time is left.
- **Productivity on the desktop.** A to-do list you tick right on the
  desktop, a sticky note from Notes (pastel, white or your colour, ruled,
  squared or dotted), the focus timer you start and pause with a click, a
  quote card with sayings that change every day (or your own), and your
  notes as tabs along a screen edge. `mandara msg desktop widgets press`
  does what any click on a widget does.
- **Calendars painted on the desktop.** A month with today marked by
  the shape you like, this week on one row, what's coming up with date
  chips, and a big date tile, with your calendar's events.
- **The sky on your desktop.** The sun on its arc from sunrise to sunset
  (the moon walks it at night), tonight's moon and how much of it is lit,
  and a globe lit where the sun is right now with your place and the moon
  over it. Worked out from your location, nothing fetched.
- **More weather layouts:** a friendly sentence about the sky (or your
  own words for each kind of weather), a round disc with a dotted ring,
  and the next hours as a temperature curve.
- **You and your shortcuts on the desktop.** A profile card with your
  picture in any shape, launch tiles for apps, folders and links, quick
  toggles (Wi-Fi, Bluetooth, do not disturb, night light, keep awake, dark
  mode, mute) that change shape when on, volume and brightness sliders you
  click or scroll, and your theme's palette (click a swatch to copy it).
- **A little fun.** A desk cat that breathes, blinks, sleeps at night,
  bobs along to music and purrs when you click it; soap bubbles; a photo
  frame that shows pictures from a folder (a polaroid if you like); any
  picture as a cut-out sticker; and washi tape to stick on things.
- **What plays, painted six ways.** The cover on a turning record, a
  pocket player with an LCD and buttons, the cover cut to a new shape for
  every track, a tall side pill, a halftone (or one-bit) cover, and synced
  lyrics with the current line lit. They show only while a player is
  loaded, and move only while music plays and you can see the desktop.
- **An LED meter visualizer**, and the system monitor as segmented
  meters, tall bars, thermal arcs for temperatures and the fan, or ring
  gauges that are wavy or dotted and start wherever you like. Meters,
  bars and arcs turn amber, then coral, only when a reading needs you: a
  busy processor or a hot GPU when high, the battery when it runs low.
- **Information on the desktop:** a machine card (like a terminal fetch,
  in rows or as tiles), screen time with the last days and your top apps,
  the next prayer in a ring of the day or a list, a search field that
  opens the launcher, and an "at a glance" line with the date, the
  weather and the one thing worth knowing now.
- **Your GitHub year, exchange rates, AI usage and games.** A GitHub
  contribution calendar with your streak, exchange rates for the
  currencies you pick, Claude Code's tokens today and this week (from its
  logs on your computer, with an optional weekly budget) and your Steam
  games as covers you click to play. The GitHub and rates widgets only go
  online once you switch that on in Studio, ask rarely, keep their answer
  for the next start, and never ask while the desktop is covered.
- `mandara msg diagnostics frames` now also reports what the desktop
  widgets' live frames cost (`desktop_live_ms`).
- **Mandara now runs on Umbriel.** Umbriel joins MangoWM, Niri and
  Hyprland as a first-class compositor. The bar shows each screen's
  workspaces, the active workspace's own layout (click it to cycle
  scrolling, dwindle, master and stacking) and your keybind submap.
  `mandara setup` adds one optional include to
  `~/.config/umbriel/config.toml` and writes your binds, window rules,
  border colours, gaps, input and display settings to Mandara's own file;
  Studio's Compositor page gets an Umbriel tab, and old launcher binds or
  a second bar in your Umbriel config are found and retired like on the
  other compositors. Values you set in your own `config.toml` still win.
- **More ways to shape the bar.** Give every bar item a capsule of its
  own (in any bar style) in a soft wash, a card tone or an accent tint,
  with an optional outline and a strength slider. Pick the bar's text
  weight, switch off the hover highlight, and keep extra space between the
  bar and your windows. Studio → Bar has them all.
- **Empty bar space does more.** Besides right-click, a middle click can
  run any hot-corner action (launcher, overview, sidebar, a command…) and
  scrolling can switch workspaces or change the volume.
- **Bar items show what you want.** Workspace pills can show numbers,
  short names or dots, with or without app icons, and hide empty
  workspaces. The active window can show its app, its title or both, up to
  a width you choose. Volume, battery and the media track can show just
  their icons, and the media item can hide while paused.
- **`mandara msg bar hide`, `bar show` and `bar toggle`** put the bar away
  and give its space to your windows until you bring it back (touch the
  edge to peek at it).
- **Your clipboard sorts itself.** Everything you copy is recognised as
  you copy it: plain text, links, Markdown, code (with its language),
  CSS, colours (hex, `rgb()`, `hsl()`, `oklch()` and friends),
  screenshots and other images, or files from your file manager. The
  panel shows each one the way it looks best: a swatch next to a colour,
  tinted code in a monospace font, rendered Markdown, a thumbnail for
  images, a file's icon, name and folder, and a link's site.
- **Filter chips and collections in the clipboard.** Chips above the list
  narrow it to one kind, your pins or a collection, and the search box
  looks through text, kinds, languages, sites and collection names. Make
  collections for anything you want to keep together (a palette, your
  snippets, links for later): the folder button on an entry adds it, a
  right-click on a collection's chip renames it, and collected entries
  survive clears just like pinned ones. Studio → Plugins → Clipboard
  turns the chips off, sets how many lines of code and Markdown a row
  previews and how many collected entries are kept, and every panel action
  has a `mandara msg clipboard …` command too.
- **More desktop widgets.** An analog clock (with an optional sweeping
  seconds hand), the weather with the days ahead, a world clock for the
  cities you care about, a countdown to a date that matters, and system
  gauges: ring meters for CPU, GPU, memory, disk and battery.
- **Every widget can sit on a plate.** Give any widget a frosted glass or
  solid plate behind it, with its own opacity, corners and padding, so it
  reads over any wallpaper. Widgets also gained an alignment setting and
  a strength slider for their shadow or glow.
- **Your numerals, your calendar, your language.** Every widget that shows
  numbers or dates now lets you pick Western or Eastern Arabic digits (or
  follow the language), the Gregorian or Hijri calendar, both, or no date,
  and the language it writes day and month names in. The depth clock's
  date pill follows these too. Widgets you already have look exactly as
  before.
- **Desktop widgets are plugins now.** Clocks, the system monitor,
  decorations, visualizers, the now-playing and calendar cards, weather,
  world clock and countdown each come from their own plugin in Studio →
  Plugins (Desktop widgets). Turn one off and its widgets step aside until
  you turn it back on. Plugin authors can add their own desktop widgets
  too (see PLUGINS.md); the moon phase example now has one.
- **A font for all your widgets.** Typography → Desktop widgets now sets
  the font every widget uses, and any widget can pick its own family from
  the same searchable font list.
- **Your wallpaper greets you in Studio.** Studio → Wallpaper now opens on
  the wallpaper on your desktop, shown large at its own shape with its
  name, size, folder and colours, plus quick buttons to change or open the
  folder, pick one at random and share it with the login screen. Below it
  are all the other wallpapers in your folder: click one (or move with the
  arrow keys and press Enter) to switch, filter them by name, and add any
  of them to a slideshow playlist. Even folders with thousands of
  pictures stay quick.
- **A much richer slideshow.** The new Slideshow tab has a big on / off
  switch, the time of the next change, Previous and Next buttons, one-click
  intervals from 5 minutes to a day (or type your own, like `90m` or
  `2h`), and four orders: shuffle (every wallpaper once before any
  repeats), random, by name and newest first. It can use your wallpaper
  folder, another folder with all its sub-folders, separate folders for
  the light and dark theme, or a playlist you pick. It can change at
  login, only between certain hours, pause during fullscreen apps, on
  battery or while locked, use its own transition, and give each display
  its own wallpaper. Every option is also a `mandara msg wallpaper
  slideshow …` command.
- **A sharper depth clock, if you want it.** The part of your wallpaper
  drawn over the desktop clock now comes from its own built-in plugin,
  *Wallpaper depth*, with two modes in Studio → Wallpaper: *Hand-tuned*
  (what you had, no download) or *Model*, a small depth model (27 MB,
  downloaded once when you pick it) that sees what is near and far in the
  picture. It cuts out antlers, skylines and ridges cleanly, and lets you
  slide the clock deeper into the scene or nearer. It runs once per
  wallpaper at the lowest priority in a separate helper that exits right
  after, so the shell itself uses no extra memory.
- **Place the clock for you.** *Place the clock* (or `mandara msg depth
  place-clock`) moves the desktop clock to where the wallpaper's subject
  rises into it.

### Fixed

- **No more "Key binding conflict" warning on MangoWM.** MangoWM treats
  Play and Pause (and any two names for one key) as the same key, so a
  Mandara shortcut could clash with one you already had. Mandara now
  checks your keys the way MangoWM does, including your keyboard layouts
  and key modes, and leaves yours alone. Niri and Hyprland get the same
  care for keys with two names, like Page Up and Prior.
- **Arabic on desktop widgets stays joined.** Widgets that space out their
  letters (headers, captions, the countdown) no longer pull Arabic words
  apart; spacing now applies only to scripts whose letters stand alone.
- **The volume slider changes what you hear, even with Easy Effects.**
  Easy Effects puts its own output in front of your speakers, and that
  output ignores volume, so the slider in the main menu, the Sound panel,
  your volume keys and the on-screen display moved nothing. They now follow
  the device your sound really plays on.
- **The main menu's side rail sits exactly in the middle**, with the same
  space above it as below.
- **A calm login screen preview in Studio.** The preview no longer drifts
  or judders: it shows your wallpaper standing still and only moves when
  you try something out or change a setting.

### Improved

- **Alt+V opens the clipboard, and Print takes screenshots.** Alt+V joins
  Super+Shift+V for the clipboard; Print captures the screen, Alt+Print the
  focused window and Ctrl+Print opens the snipping tool. Keys you already
  use for something else stay yours. Change any of them in Studio →
  Keyboard shortcuts (the Clipboard and Screenshot chips add new ones).

- **New artwork for the new plugins.** The depth effect and every desktop
  widget plugin have their own logo and a painted store cover in the
  Corniche, Twilight and Obsidian palettes.

- **The date stays readable over a depth wallpaper.** The clock's digits can
  still tuck behind the wallpaper's subject, but the date line now comes to
  the front as soon as anything covers it, and *Place the clock* aims for a
  lighter overlap so no digit disappears.

- **Sidebars fit the bar like puzzle pieces.** A flush sidebar now uses
  the bar's own material and meets it with the same curved joins in every
  style: it merges into the notch, hangs from the edge frame, carries the
  hairline's accent line down its edge and continues a side bar's strip,
  with clean curves into the screen edges.
- **The main menu's glass always shows your wallpaper.** Until its frosted
  copy is ready (or when there is no wallpaper to frost) the menu stays
  solid instead of letting desktop widgets show through.
- **A calendar worth opening.** The clock's drop-down is redesigned: the
  time sits on a glass hero beside a page-a-day tile, and the month below
  slides from page to page with the arrows, **Today**, the scroll wheel or
  the keyboard (arrows, Page Up / Down, Home). Click any day to see its
  events; today lists what's coming up and marks the one happening now.
  Weekends and the neighbouring months' days are set apart, each calendar
  keeps its own dot colour, and the panel no longer cuts off its agenda.
  New in Studio → Date, time & location: Hijri dates, Eastern Arabic
  numerals and your weekend days (all off or automatic, so your calendar
  looks as before until you turn them on). Scripts get `calendar goto` and
  `calendar day`.

- **Arranging widgets is smooth now.** Dragging a widget in Studio's
  preview or on the desktop follows your pointer at your display's full
  frame rate: the widget slides as it is and is painted once where you
  drop it, sliders apply at most once per frame, and a widget behind the
  wallpaper's subject no longer makes the subject redraw on every move.
  Studio also stopped quietly reloading the wallpaper preview over and
  over when its path started with `~`, and the preview now uses the same
  fonts as your desktop.

- **A tidier Wallpaper page.** Its settings are now in tabs: Wallpaper,
  Slideshow, Fit & effects (fill, transitions, light and dark wallpapers,
  video and depth), and Generator and Online when those plugins are on.
  The wallpaper generator no longer sits at the top of the page.
  `mandara studio wallpaper --tab slideshow` opens a tab directly.
- **The slideshow costs nothing while it's off.** It used to check a timer
  every 30 seconds even when switched off; now it only wakes when a change
  is due.

- **"My masks only" is gone.** Your own masks still win over anything
  Mandara finds; if you had the subject mask switched off, the depth
  effect stays off.
- **Fresh screenshots everywhere.** The README has a new Desktop widgets
  section, and the guides show today's Studio pages, the new plugin
  covers, the main menu, the clipboard with its kinds and collections, the
  notch bar with a sidebar fitted to it and the depth clock.
- **A shorter README and a real documentation folder.** The front page on
  GitHub is now a quick tour with a few screenshots and the install
  command. Everything else moved, word for word, into the `docs` folder:
  one page per topic (install, usage, your desktop, dock and launcher,
  wallpapers, plugins, Studio and more), with an index to start from.
- **Back-to-back releases no longer trip over each other.** A release that
  waited behind another one used to fail when it tried to save its version
  bump. It now starts from the latest code, so both go through.

### Under the hood

- **Plugin art takes less space.** Built-in plugin icons are now embedded as
  lossless WebP: the same pixels, about a fifth fewer bytes, so 58 plugins
  with their new artwork still fit the old size budget.

- **Fresher, leaner building blocks.** Mandara's libraries are updated to
  their latest versions (including the engine that runs plugins), and
  parts of them it never used are switched off, so the shell links fewer
  duplicate libraries.

## 0.4.0 — 2026-11-01

A new main menu, a Notch bar that hangs from the top edge, your wallpaper
on the login and lock screens (which now share one design), a far more
customisable login screen with ready-made looks, plus fixes for night
light, media keys, album art, the process list and logging out.

### New

- **A new main menu.** Right-clicking empty bar space opens a control
  centre: your profile, what's playing, the time and weather, quick
  toggles and a volume slider on its home page, then pages for apps and
  tools, themes and your session (Lock, Log out, Sleep, Restart, Shut
  down). A rail down its side jumps to the sound, network, Bluetooth,
  system monitor, weather, calendar and notification panels, and to
  Studio. It's frosted glass over your wallpaper with solid cards on top;
  Studio → Bar → Main menu sets how see-through it is, or makes it solid.
- **Choose what right-clicking the bar opens.** Studio → Bar → "Right-click
  on empty bar space" picks the bar's main menu or the desktop's menu.
- **A Notch bar.** A new bar style that hangs from the top edge, narrower
  than the screen, with curved shoulders where it meets the edge and
  rounded corners below; panels pour out of it. Pick "Notch" in Studio →
  Bar → Presets for the full look, or set the style yourself and tune its
  width (Studio → Bar → Notch width).
- **Group capsules.** Each group of bar items can sit on a soft capsule of
  its own (Studio → Bar → Group capsules), on the edge frame, the hairline
  and the notch.
- **Design the login screen your way.** Mandara Login can now put the
  sign-in on a card (size, padding, corners, fill, border, a back button and
  the session picker inside it), show your picture without a frame, put the
  sign-in button beside the field, give the focused field an accent border,
  hide what you type as varied shapes instead of dots, use a full-width
  session field and outlined power buttons, show a logo, and offer a colour
  scheme switcher people can use right on the login screen.
- **Ready-made login looks.** Studio → Login screen → Looks applies a whole
  design in one click: Mandara's own, or "Midnight card", a quiet clock over
  one outlined card in midnight navy and citrine. Change anything after.
- **Pick fonts from a list.** Every font setting is now a searchable list of
  the fonts on your system, each shown in its own face. You can also type a
  name you haven't installed yet. Typography has new per-surface fonts too:
  the bar, panels and plugins, desktop widgets, the lock screen and its
  clock, and Studio itself, each following the interface font until you
  pick one. The login screen has its own text font as well.
- **A new theme, Night Citrine.** Deep midnight navy with pale citrine, soft
  lavender and mint, for the shell and the login screen.
- **Your wallpaper on the login screen.** Mandara Login now shows the
  wallpaper of whoever is picked, and switches as you pick someone else.
  Your session shares a copy for it (Studio → Wallpaper → "Show on the
  login screen", on by default; Studio → Login screen → "Each person's own
  wallpaper" turns it off for everyone). People without one see the usual
  backdrop. Apply your login screen settings once in Studio after
  updating, so the shared folder is created.
- **A settings button on the weather card.** The weather card in the bar
  panel and the sidebar has a button in its corner that opens Studio on
  every weather setting.

### Improved

- **A calmer Login screen page in Studio.** The preview sits beside what
  you do with it (apply, full screen, and "Try it" buttons that play the
  entrance, typing, a wrong password or a sign-in), so the settings get
  most of the page. One slim row of tabs follows the way you work: looks,
  layout, background, clock, sign-in, session, theme, motion, and the login
  manager last. Settings that depend on another one appear only when it's
  on (the card's size and colours once the card is on, the video options
  only for a video backdrop), and the logo and the colour scheme switcher
  have their own on/off switch. The preview also shows your real
  wallpaper, the same one the login screen gets.
- **The sound spectrum moves to the app that's playing.** In the sound
  panel the live bars now sit under the app actually playing, such as
  Spotify, instead of under the output, which with EasyEffects showed them
  under EasyEffects. With nothing playing they stay under the output.
- **The weather panel fits what it shows,** without the empty space at its
  bottom.

- **The login and lock screens look like one screen.** Both now have the
  date over a bold clock, your picture and name a little below the middle,
  and a wider password field, in the same proportions on every screen size.
  The lock screen also blurs and darkens your wallpaper much less, so you
  can actually see it. If you changed those settings yourself, they stay.

### Fixed

- **Night light works on Intel laptops.** Some graphics chips, Intel ones
  among them, use colour tables larger than Mandara expected, and Mandara
  sent a shortened table the compositor refused, so the screen never
  warmed. It now sends the full table.
- **Media keys work.** Play/pause, next and previous (often F4–F6 on
  laptops) and the microphone mute key now control your music, and the
  laptop brightness keys work on installs from before they were added.
  Keys your own compositor config binds are left alone.
- **Album art shows up.** Covers from players such as Spotify were saved
  without a file extension, so they never loaded. They're now saved and
  read properly, and images are recognised by their contents everywhere.
- **"All processes" opens the process list.** The link in the system
  monitor did nothing, because the process list is switched off on new
  installs. It now switches it on and opens it.

- **Log out works on MangoWM.** The power menu's Log out button (the dock
  that slides in from the right when you press the power button) did
  nothing on MangoWM: Mandara asked MangoWM to quit but hung up before
  MangoWM answered, and MangoWM drops a request whose sender has gone. It
  now waits for the answer, so Log out ends your session. If MangoWM can't
  be reached, it asks logind to end the session instead.
- **The power menu tells you when something fails.** Lock, Log out, Sleep,
  Restart and Shut down used to fail silently, for example when the system
  refused or a service wasn't running. Now a notification says what went
  wrong. Lock also uses Mandara's own lock screen directly when logind
  can't be reached, and Lock and Log out now always target your own
  session.

- **Night light says why when it can't warm your screen.** Turning night
  light on used to show "Warm" even when nothing changed, for example when
  another night-light app such as wlsunset or gammastep already controls
  the screen colours, or when the compositor or display driver refuses the
  change. Mandara now shows a notification that names the app when it finds
  one, and `mandara msg nightlight status` lists it too. If night light ever
  loses its connection to the compositor, turning it off and on again
  reconnects it.

### Under the hood

- **Warnings are errors.** Every compiler and linter warning now fails the
  build, across the whole project, and the ones already there are fixed.
- **Version numbers follow what changed.** A new feature anywhere in the
  shell now makes a minor release (like 0.4.0), not only one in its startup
  code. Signed updates came out as 0.3.4 because of this; the next release
  with a new feature gets the minor number.

## 0.3.4 — 2026-10-31

Signed releases: Mandara now checks that an update really comes from its
maker before installing it.

### Improved

- **Signed updates.** Every release is now signed with Mandara's own release
  key, and Studio's Update now, `mandara update` and `mandara install` check
  that signature before installing anything. If someone replaced the files
  of a release on GitHub without the key, Mandara refuses the update and
  tells you why, instead of installing it. To update, open Mandara Studio
  (click the grid button at the start of the bar, or press Super+Space and
  search for "Studio"), go to About and press Update now.

## 0.3.3 — 2026-10-31

The last open finding from the security review, and icons that say what
they do.

### Fixed

- **Your own copy of Mandara never runs as root anymore.** Studio's
  administrator actions (switching the login screen, applying its look,
  unlocking keyrings at login) used to run the `mandara` in your home folder
  as root, which your account, or malware in it, could replace before you
  approved the prompt. They now only run a copy that root owns and nobody
  else can change: a package's `mandara`, or Mandara's new admin helper.
  `mandara install` sets the helper up for you; on an existing install,
  Studio sets it up the first time it needs it, with one extra approval.

### Improved

- **Icons that say what they do.** The wallpaper generator now uses a
  brush, "Surprise me" a die, and What's new a release-notes page, instead
  of generic sparkles. Star counts for plugins and cursor themes are
  written out ("1.2k stars") instead of a star symbol.

## 0.3.2 — 2026-10-31

A security release. Thank you to the person who reviewed Mandara and
reported what they found on GitHub.

### Fixed

- **Plugins stay in their sandbox.** A plugin with the `commands`
  permission could ask Mandara to run any command, including ones that
  start programs or add themselves to your startup apps. Now plugins may
  only use the interface: open panels, control media and volume, post
  notifications, add notes and to-dos. Everything else is refused, and
  Studio describes the permission honestly.
- **A window's title can't change your compositor config.** Picking a
  running window in Studio's window rules copied its title into your
  MangoWM or Hyprland drop-in as-is, so an app could choose a title that
  added its own line. Titles are now cleaned before they're written, for
  every compositor.
- **Plugins can't freeze the shell or eat its memory.** Each plugin gets at
  most 64 MiB, a plugin can no longer sleep past its time limit, and the
  size of what it draws is capped. A plugin that crosses a limit is simply
  switched off.

### Improved

- **Marketplace plugins stay with their publisher.** If a plugin's
  repository is renamed or handed to someone else, Mandara stops updating
  it instead of quietly following, and the first install is now checked
  against the release's checksums too, not just updates.

## 0.3.1 — 2026-10-30

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
