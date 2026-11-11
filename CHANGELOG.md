# Changelog

What changed in each Mandara release, written for the people who use it.
Newest first. `mandara changelog` prints it in a terminal, and Studio →
About → What's new shows it too.

## Unreleased

### New

- **Aqueous joins the supported compositors (#162).** On Aqueous the bar's workspace pills now show each display's workspaces and switch to one when you click it, and the window title, displays, keyboard layout, window moves and the Overview hot corner follow Aqueous too, all over its own IPC socket without ever making the bar wait. Aqueous keeps its settings in its own files, so Mandara writes nothing there; the layout pill can't change Aqueous layouts, because Aqueous has no command for that yet.

### Fixed

- **The password dialog no longer freezes, and asks once instead of twice.** When two requests came back to back (Studio refreshing its admin helper, then applying your login screen, or the update manager doing the same before updating), the second dialog lost the question polkit had already asked and waited forever, so your password went nowhere until you cancelled and tried again. A request that arrives while the previous dialog is still closing now keeps its prompt, and refreshing the admin helper happens in the same approval as the action itself. A system update on Arch with AUR packages still asks a second time for the AUR step, because that one goes through `sudo`.
- **Changing a display's scale no longer makes the top bar disappear.** Going between fractional scales (for example from 1.5 to 1.75) never resized the shell, so the bar ended up above the top of the screen. The shell now follows each screen's scale, and the wallpaper fills the screen again after you lower it.

## 0.26.1 — 2026-11-11

Applying a login screen from Studio no longer fails with "unknown variant
`logo`".

### Fixed

- **Applying a login screen from Studio no longer fails with "unknown variant `logo`".** Updating Mandara replaces your own copy without a password, but the root-owned copies it runs as an administrator stayed on the version they were first installed with, and an old one couldn't read the newer config. Studio now refreshes its admin helper when it's older than the Mandara you run (one extra approval, once per update), and applying the login screen also brings the login screen's own copy up to date, so the screen you see at boot reads what you designed.

## 0.26.0 — 2026-11-10

Mandara Login has three new looks that move the sign-in off the middle of
the screen: Ghostline, Chrome Halo and Meridian. The Productive bar is a
work desk of its own, on a portrait screen the login screen is no longer
drawn oversized, and the website is readable in every colour and never
wider than your phone.

### New

- **Three new login screen looks: Ghostline, Chrome Halo and Meridian.** Studio → Login screen → Looks now has six designs, three to a row, each with a small picture of where things sit. **Ghostline** puts the sign-in on the left in a bracketed wireframe panel on graphite, with live readouts (keyboard layout, uptime, time zone), a terminal password line that fills one cell per key, and a thin clock beside a ruler of the day on the right. **Chrome Halo** sets a light clock inside a slowly turning ring of iridescent light that brightens as you type, with a machined two-layer sign-in module and a pill button on the right. **Meridian** has no boxes at all: your name and password sit on one fine line across a calm dawn field, over enormous faint numerals, in light or dark. Every piece is also a setting you can mix into any look: clock faces, card styles, the terminal and recessed fields, the pill button, the HUD, prism and horizon overlays, and *Light colours*. All three work on FreeBSD too, and `mandara greeter --demo --look ghostline` (or `chrome-halo`, `meridian`, with `--scheme light` or `dark`) previews them.

### Improved

- **The Productive bar preset has a look of its own.** It used to be the Glass pills bar with a few items swapped. Now it's a work desk along the bottom edge: the focus timer and the window you're in sit in the middle, your to-dos and notes on the left, system load and the clock on the right, each group on a soft capsule of its own. Its card in Studio → Bars → Presets draws exactly that, and picking another preset afterwards takes the capsules away again.

### Fixed

- **The website is readable in every colour and never wider than your phone.** Every piece of text now meets the WCAG AA contrast minimum in light and dark, in all seven Looks: the notification caption, the FreeBSD and immutable-system cards' lists, links and label, the right-to-left language labels and the typed agent line, which disappeared completely in Pharos Ink. The design and its decorations are unchanged. On a phone the page stays exactly as wide as the screen: the turning shapes in the hero and the shortcut search box no longer push it sideways, at any width from 320 px up.
- **The login screen fits portrait screens.** On a screen turned upright, Mandara Login sized everything by the screen's height and drew it far too large. It now sizes by the shorter side, so a portrait screen looks like the same screen turned landscape.

## 0.25.1 — 2026-11-10

On FreeBSD, the login screen's power buttons no longer need the `operator`
group, which could read your disks. The website looks right on phones
again, and the text on the AI Shell Agent card no longer jitters.

### Fixed

- **The login screen on FreeBSD no longer needs the `operator` group, which could read your disks.** Mandara Login's Sleep, Restart and Shut Down buttons now go through ConsoleKit2 with one small polkit rule that lets the login screen's user do exactly those three things and nothing else. The next `mandara install` writes it and takes that user back out of `operator`, where 0.24 and 0.25 had put it, and tells you it did. The first press after boot no longer leaves the login screen waiting forever, and `mandara greeter power-rule` shows or undoes the rule.
- **The website fits phones again.** Its top bar is two tidy rows on a phone: the name with the Discord, Ko-fi and GitHub buttons, then the page links beside a small light / dark switch. The text on the AI Shell Agent card (and the NixOS, FreeBSD and immutable cards) no longer jitters while the shape behind it turns.

## 0.25.0 — 2026-11-10

Mandara runs on FreeBSD: every release now has a FreeBSD build, with its
system readings, updates and installer. On Arch, *Update all* updates your
AUR packages inside Mandara too. The System monitor widget shows NVIDIA and
AMD GPUs, files in the dock's Downloads open with your default app, and
the Trash opens in your file manager.

### New

- **Three new bar presets: Stanley Bridge, Pharos Beam and Raml Tram.** Studio → Bars → Presets has three more one-click bars named after Alexandria. **Stanley Bridge** splits the tile dock into three floating docks of squircle buttons that span the bottom like a bridge's arches. **Pharos Beam** hangs a narrow lantern of tonal tiles and scalloped accent buttons from the top edge. **Raml Tram** runs one line of flower stops along the top and down the left edge, a top bar joined to a rail. Each has its own little drawing on its card, everything in it stays adjustable afterwards, and picking another preset puts back exactly what it set. `mandara msg bar style stanley-bridge` (or `pharos-beam`, `raml-tram`) applies one from the terminal.
- **System monitor is a monitoring bar now, not Classic with other items.** The preset became a style of its own: a slim status line along the bottom edge where every reading sits on a square-cornered stat block, CPU and memory meters and network rates up front in the middle, in your monospace font. Its card shows a little strip of bar graphs, so it no longer looks like Classic's. `mandara msg bar style system-monitor` applies it.
- **App icons in the workspace buttons of every bar style.** The styled bars ignored *App icons* (Studio → Plugins → Workspaces): tags showed the plain pills, and on a rail only numbers. Now every style draws each tag as a button in its own shapes (a flower on Rail and Raml Tram, a cookie or circle on Expressive, a squircle on Tile dock, a burst on Pharos Beam, connected tiles on Segmented) with the icon of the app on it, the tag you're on in the accent and empty ones showing their number. Turning *App icons* off shows numbers only, as on the classic bar.
- **Your AUR packages update in Mandara too.** On Arch, *Update all* now runs pacman, then your AUR helper, then Flatpak, all in the Updates panel's one progress card instead of opening a terminal, and the AUR's own *Update* button updates just the AUR. Mandara uses the AUR helper you have (paru first, then yay, pikaur, Aura, trizen or CachyOS's Shelly), or the one you pick under Studio → Plugins → Updates → *AUR helper*. The helper always runs as you, never as root, and when it installs what it built, its sudo asks for your password in Mandara's own password dialog (once more after the system step, since sudo can't reuse that approval; the dialog says so). The card shows which package is downloading, building or installing, *Cancel* stops a build, and a package that doesn't build is named with the reason, with *Open in terminal* when it needs you.
- **`mandara install` on FreeBSD.** The installer recognises FreeBSD (and GhostBSD), installs everything from FreeBSD's own packages with pkg, turns on D-Bus and seatd in rc.conf and starts them, adds you to seatd's group so a compositor can open the seat, sets up the admin helper the FreeBSD way, and can install Niri, Hyprland, Sway or River Classic for you. What FreeBSD doesn't have (NetworkManager, BlueZ, monitor brightness over DDC/CI) is skipped with a short note instead of an error.
- **Your system at a glance on FreeBSD.** The process hunter, Resources, mini stats, the desktop system monitor and Studio's Performance page read FreeBSD's own counters, so CPU, memory, swap, load, uptime, temperatures (with coretemp or amdtemp loaded), drives and network traffic all show real numbers. Batteries come from UPower or straight from ACPI, brightness from FreeBSD's backlight devices, and the volume keys and sound panel drive FreeBSD's own sound system when neither PulseAudio nor PipeWire runs.
- **Network and power on FreeBSD.** The network panel shows your wired and Wi-Fi links from ifconfig and wpa_supplicant, updates the moment a link changes, and joins Wi-Fi networks without your passphrase ever showing up in a process list. Lock, sleep, restart and shut down go through ConsoleKit2, or through FreeBSD's own `shutdown` and `acpiconf` behind Mandara's password dialog. What FreeBSD can't do (Bluetooth, power profiles, hibernation, hotspots, and the plugins that manage systemd, coredumps, Linux sockets and Linux apps) is locked in Studio with a sentence saying why.
- **Mandara Login on FreeBSD.** Once greetd is built (it isn't packaged for FreeBSD yet; the guide shows how), `mandara greeter use mandara` switches login screens through rc.conf, reads greetd's config from `/usr/local/etc/greetd`, and *Unlock keyrings at login* writes PAM lines FreeBSD understands. Mandara's password dialog works with FreeBSD's polkit and ConsoleKit2 sessions too.
- **Mandara runs on FreeBSD.** Every release now comes with a FreeBSD build for FreeBSD 14.4 and newer on amd64, signed and checked like the Linux ones. The one-line install recognises FreeBSD, downloads that build (with FreeBSD's own `fetch` when curl isn't installed) and hands over to `mandara install`, and Studio → About → *Update now* keeps it current with small patches, just as on Linux. See the FreeBSD section of the install guide for the packages it uses and the compositors that run it.
- **The main menu in your bar's gaps.** On glass pills, the island, the micro pill, the notch and the *Segmented* and *Tile dock* styles, the space between the pieces belongs to the desktop, so a right-click there opened the desktop's menu. Turn on *Main menu in the bar's gaps* (Studio → Bars → Bar → Behaviour) and a right-click anywhere in the bar's strip opens the main menu instead. The desktop's menu is still a right-click anywhere else on the desktop, or one tile away on the main menu's Apps & tools page (*Desktop menu*). Off by default; full-width and side bars have no gaps, so Studio says so there.
- **A main menu rail as tall as the menu.** *Rail height* (Studio → Bars → Drop-down panels → Main menu) can stretch the main menu's rail to the height of the panel beside it. Its buttons spread out evenly from top to bottom, with equal gaps and the divider halfway between the pages and the shortcuts, and a mashrabiya lattice in the tiles' accent rises from the rail's foot and fades out on its way up, in every tile style, light and dark, and right to left. The rail still hugs its buttons by default.
- **Mandara installs on image-based systems.** On Bazzite, Silverblue, Kinoite, Bluefin, Aurora and the other Fedora Atomic images, on openSUSE Aeon, Kalpa and MicroOS, and on Vanilla OS, the same one-line install now puts Mandara in your home folder and adds only the system packages your image is missing, checked one by one so nothing it already ships is layered again. Everything that needs no reboot is set up right away; then, after one question, the missing packages go into your next image in one `rpm-ostree`, `transactional-update` or `abroot` step, and the installer tells you to restart. Mandara Login's greetd and cage come in the same step, and after the reboot it's one click away in Studio → Login screen. On SteamOS, whose system can't keep packages, Mandara installs into your home folder and tells you what it couldn't add. The Updates panel agrees: on these systems it shows your Flatpak updates and leaves the system itself to its own tool.
- **System updates on FreeBSD.** The Updates panel counts your package updates with pkg, and *Update all* updates the whole system right in the panel, behind Mandara's own password dialog. On a traditional base system it updates your packages with pkg, then your release's security and errata patches with freebsd-update. On a base system installed from packages (FreeBSD 15's VM and cloud images, or its installer's *Packages* choice) the panel counts the base system's updates too, and pkg updates it alongside everything else, never freebsd-update. Everything downloads first (and can be cancelled until then), then installs, with the same live phases, counts and log as elsewhere; a new kernel or kernel module suggests a restart, and a -STABLE system or a release past its end of life gets its packages updated with a clear warning about the base system.
- **Mandara runs on FreeBSD.** Every release now comes with a FreeBSD build for FreeBSD 14+ on amd64 (14.4 and newer, FreeBSD 15 included), signed and checked like the Linux ones. The one-line install recognises FreeBSD, downloads that build (with FreeBSD's own `fetch` when curl isn't installed) and hands over to `mandara install`, and Studio → About → *Update now* keeps it current with small patches, just as on Linux. See the FreeBSD section of the install guide for the packages it uses and the compositors that run it.

### Improved

- **Preset cards look like their bars.** The plain bar presets' cards now draw a dot for each item in each group, so a busy bar looks busy and Classic and Minimal no longer share one picture.
- **Fresh login and lock screen pictures.** The README and the lock and login guides now show Mandara Login in all three of its designs (Mandara with its emblem, Midnight card and Material You), the lock screen in its Mandara and Material You designs, and Studio's Login screen page with its three looks.
- **The System monitor desktop widget shows your NVIDIA GPU, and every reading is yours to pick.** The widget now reads its GPUs from the same place as the Resources panel, so NVIDIA cards (through `nvidia-smi`) show up next to AMD and Intel ones, with load, video memory, temperature and power draw, one line per GPU when you have several. Two new readings join the rest, **VRAM** and **Swap**, off until you switch them on. Pick what the widget shows in Studio → Desktop widgets → *Readings*, or right-click the widget in *Edit desktop* and choose *Readings* to tick them right there. A GPU reading your machine can't show is locked with the reason (no graphics card, `nvidia-smi` missing, or a card that doesn't report it), and the widget leaves it out instead of showing an empty row. Existing widgets look exactly as before.
- **Discord and Ko-fi in the website's top bar.** Join the community or support Mandara right from the floating bar, next to GitHub, in the page's own colours.
- **FreeBSD and immutable systems on the website.** The install section gets a FreeBSD card and an "Immutable? No problem!" card beside NixOS, each with how to install there. The "Ask in your words" card types its example without moving the text around it, and the bar styles' preview sits centred beside its list.
- **Mandara in motion, right on the homepage.** The website's first screen now plays a 40-second film: Looks recolouring the whole desktop in one click, the four bar styles, the launcher, panels springing open, live widgets, Studio changing colours as you watch, the Material You lock screen, and a nod to FreeBSD. It loops quietly with the sound off, *Sound on* plays its original soundtrack, and it loads only after the rest of the page.
- **The `!` shell agent uses far less memory and answers sooner.** Its compilers now read their weights straight from the model file instead of copying them into memory, and Qwen keeps its word table compressed. In our tests a Qwen request's private memory fell from about 1.6 GiB to under 700 MiB and FunctionGemma's from about 570 MiB to about 470 MiB, and an English request with Qwen shows its candidates in about a fifth of a second instead of over a second, because Qwen now loads after the search. The previews are the same as before.
- **A closed sidebar uses no memory for its cards.** Sudoku, 2048, Radio, Feeds, Reddit and every other sidebar card are now made when the sidebar opens and let go a moment after it closes: their views, rows, posts, pictures, timers and connections go, a game is saved and put away, a third-party plugin shown only in the sidebar is unloaded, and the memory goes back to the system. Before, every card was built at startup, even with the sidebar never opened, and stayed for good. Opening the sidebar is as quick as before, a quick close and reopen keeps the cards as they were, and anything you were in the middle of comes back: a half-typed task or list name in To-do, and in Notes the note you had open (in its editor, at the same spot), your search and a topic name you were typing.

### Fixed

- **Files in the dock's Downloads open again.** Clicking a file in a stack's fan did nothing: pressing on any file but the one next to the dock closed the fan before the click finished, and the files that did get through went to the desktop portal, which shows its own app chooser for files instead of using your default app and never said when that failed. Every file in the fan now takes the click, and files open straight in the app you picked for their type in Studio → Applications → Defaults, then your system's defaults (a type without an app of its own uses its parent's, so a `.log` opens in your text editor). Folders open in your file manager. If no app opens a file, or the app can't start, a notification says so and where to pick one.
- **The Trash opens in your file manager.** Opening the Trash from the dock handed `trash:///` to whatever claims that address. It now opens in your own file manager (the one in Studio → Applications, or your default for folders): Nautilus, Nemo, Caja, Thunar, PCManFM and PCManFM-Qt get `trash:///`, Dolphin gets `trash:/`, and COSMIC Files and any other manager get the Trash folder itself (`~/.local/share/Trash/files`, created if it's missing). A desktop widget that opens `trash:///` does the same.
- **The login screen preview keeps its buttons apart.** In `mandara greeter --demo`, the Midnight card's colour scheme switcher no longer sits on top of *Exit demo*: anything in a top corner moves just below the preview's buttons, in right-to-left languages too. The real login screen keeps the switcher where it was.
- **The Feeds and Reddit sidebar cards show up.** Both plugins came with a sidebar card, but Studio's *Add a widget* list, the *Add to sidebar* button on their plugin pages and `mandara msg sidebar tab widget add` didn't offer them, so you couldn't put them on a tab. They're there now, next to the Radio, Sudoku and 2048 cards, and a check keeps every future card on that list. The Reddit card also shows a *Find communities* button when you follow no community, instead of nothing at all.
- **Login screen backups only root can touch.** `mandara greeter use` kept its backups inside the login screen user's own folder, so that user could swap in a backup that `sudo mandara greeter restore` would then carry out. Backups now go to `/var/lib/mandara-greeter-backups`, which only root can change, and a restore runs only the service commands its switch could have made, for that switch's own login managers. Backups made by older releases still restore when root owns them and nobody else can change them.

### Under the hood

- **Mandara compiles for FreeBSD.** The whole workspace, its tests included, now builds for FreeBSD 14 (amd64) without a single warning, and the FreeBSD check in CI is a required one: a change that breaks the FreeBSD build can't land. The bar, panels and Studio use the same Wayland layer surfaces there as on Linux.
- **Mandara compiles for FreeBSD.** The whole workspace, its tests included, now builds for FreeBSD 14 and 15 (amd64) without a single warning, and the FreeBSD check in CI is a required one: a change that breaks the FreeBSD build can't land. The bar, panels and Studio use the same Wayland layer surfaces there as on Linux.

## 0.24.0 — 2026-11-09

Update your whole system right from the Updates panel. Pick how every
control looks, or set them all at once with a preset. Mandara Login and the
lock screen get a Material You design, the power menu can float as cards in
the middle of the screen, and four bar styles look nothing like the usual
strip. The Mandara emblem shows on the login screen again.

### New

- **Update your whole system from the Updates panel.** *Update all* now installs your updates right in the panel instead of opening a terminal: Mandara runs your distribution's own package manager (pacman, APT, DNF, Zypper, XBPS or apk) and asks for your password with its own dialog, then updates your Flatpak apps. You see each phase as it happens (refreshing, downloading 3/46, installing 12/46, running hooks) with a progress bar and the latest log lines, you can cancel while it's still downloading, and the update keeps going if you close the panel. When it's done you get a clear summary, a gentle "restart to finish" when the kernel or another core package changed, and plain explanations when something needs you (another program using the package manager, no network, a conflict, a full disk). Nothing is ever forced, and the AUR, image-based systems and the new *Run updates → In a terminal* setting still use your terminal.
- **NixOS generations in the Updates panel.** On NixOS the panel now lists your system generations (date, NixOS version, kernel, which one is running and which one booted) and your home-manager generations. Update your flake inputs or channels and rebuild with *Switch*, *Next boot*, *Test* or *Build only* while the build's own progress fills the bar, see which inputs changed, roll back or switch to any generation after a confirmation, compare a generation with the running one (changed packages and closure size), and clean up old generations, keeping the running and booted ones.
- **A Material You design for the login and lock screens.** Studio → Login screen → Looks has a third look next to Mandara and Midnight card, and Studio → Lock appearance has a new **Design** choice with it. Its colours come from your wallpaper (Material's tonal scheme from the picture behind, cross-fading when another person's wallpaper takes over); the clock is big and stacked, hours in a tone of your accent, and its variable weight grows from thin as the screen wakes; your picture sits in Material's scalloped twelve-sided shape, which turns a little with each key you press and morphs into a circle while your password is checked. The password field is Material's filled field (the hint floats up as you type, the line under it turns your accent or red), with a round button beside it that squares off while held, and the sessions and other people are Material chips and menus, with tonal power buttons. The shape password symbols, right-to-left languages and reduced motion all work as before. Every part stays editable in the Login screen's other tabs (new choices: *Hours over minutes*, *Hours in the accent colour*, *Weight while idle*, a *Scalloped* avatar, *Chips* for other users, a *Filled* field, a *Chip* session picker, *Tonal* power buttons, *Emblem in the accent colour* and *Colours from the wallpaper*), and the Mandara and Midnight card designs look exactly as they did.
- **Pick how your buttons, toggles, fields, chips, meters and cards look.** Studio → Themes has a new **Components** tab, next to *Colors & style*, for every control the shell and Studio draw. **Buttons** can be *Square*, *Expressive* (round at rest, the corners square off while you press and spring back), *Cut* (corners cut at an angle) or *Elevated* (a soft shadow that lifts on hover); **toggles** can be *Material* (an outlined switch whose thumb grows and shows a check), *Expressive* (the thumb stretches as it slides, check boxes round into circles as they fill, radio dots spring in) or *Round*; **text fields and search boxes** can be *Filled*, *Outlined* (the hint floats up into a gap in the outline as you type) or *Pill*; **chips and choices** can be *Material* (an outlined group with fine dividers) or *Connected* (separate buttons that round fully when chosen); **meters, progress bars and rings** can be *Material* (a gap and a stop dot), *Wavy* or *Segmented*, always in the same space as before; and **cards** get *Asymmetric*, *Square*, smooth *Squircle* or *Cut* corners and a *Raised* or *Outlined* style. The designs apply everywhere at once: panels, the sidebar, the main menu, the bar's resource rings and layout pill, notification actions, the volume and brightness OSD, the launcher's search field and chips, tray menus, the installer and Studio itself, which follows a change the moment you make it. Every design is drawn as a real preview of the control on cards of one size, and five **presets** at the top (Classic, Material You, Expressive, Minimal and Brutalist soft) set every component in one click, leaving your corner size, shadows, panel edges and slider handle alone. Nothing changes until you pick something: every component keeps its classic look by default, right-to-left languages are mirrored, and reduced motion turns the springs off.
- **Looks bring their own components.** Each of the six Looks now comes with fitting component designs (Pharos Ink with *Minimal*, Night Frame with *Brutalist soft*, Anfushi Wave and Montaza Crimson with *Expressive*, Qaitbay Forest with *Material You*, Corniche Dusk with the classic ones), shown on the Look as a small badge. Picking a Look applies them along with the rest of the Look; a design you choose afterwards stays yours until you pick a Look or preset again, and *Keep my component designs when changing looks* under Themes → Components keeps your own designs whatever Look you pick. Restore my look brings your designs back too.
- **Read Reddit in a panel.** The new Reddit plugin (Studio → Plugins, off until you switch it on) shows your communities, their posts and whole comment threads right from the bar: hot, new, top rated or rising, search within a community, galleries and pictures sized for the panel, comments you can fold and expand, more replies loaded in place, and a button to open a post in your browser or copy its link. Discover has a starter list of communities by topic, and you add, remove and reorder yours there. NSFW posts stay hidden and spoilers covered unless you choose otherwise, and nothing refreshes while the panel is closed. It works right away in a clearly labelled limited mode on Reddit's public feeds (one request a minute, no scores or reply threads); add a free "installed app" you create once on Reddit for full mode, and the panel walks you through it. Its token never leaves memory and renews by itself. Pick cards, compact rows or a picture-first grid from the panel's header, and add a sidebar card with a community's top posts. Type `reddit`, `subreddit` or `r/linux` in the launcher to get there.
- **Radio.** A new opt-in plugin plays internet radio: about seventy hand-checked stations on shelves for news, talk, pop, rock, jazz, classical, electronic, lo-fi, ambient, world music, Arabic, Qur'an recitation and kids, plus any station you add by its address (Mandara listens for a moment first to make sure it plays). The panel has a large now-playing card with the song title, live badge, format and buffer health, favourites you can reorder, search, a sleep timer that fades out and volume per station; the bar shows the station's art and a scrolling title, and a compact sidebar card (now playing, favourite, previous / play / next and volume) goes on any sidebar tab. Media keys, the media panel and the lock screen follow it, a dropped stream reconnects by itself, and you can search the radio-browser.info directory once you switch it on. Nothing runs until you press play. Turn it on in Studio → Plugins → Radio.
- **Sudoku and 2048, in the bar and right on your desktop.** Two new games, off until you switch them on in Studio → Plugins (or type "sudoku", "2048" or "games" in the launcher and pick *Turn on*). Play them in the bar's drop-down panel, as a card in the sidebar, or add them as a desktop widget and play right there: click the board to give it the keyboard, click anywhere else to give it back. One game is shared between the panel, the sidebar and the desktop and kept when you close it, restart the shell or log out, along with your best times and scores.
- **Sudoku** deals puzzles with exactly one solution at four levels (Easy, Medium, Hard and Expert), with pencil marks, highlights for the selected cell's row, column, box and digit, red conflicts, undo and redo, counted hints and mistakes, and a timer that only runs while the game is in sight. Play with the arrows and number keys (N for notes, H for a hint), the mouse or touch; a solved board gets a little wave of light (none with reduced motion).
- **2048** slides its tiles on the shell's springs and pops them as they merge, with a 4 × 4 board or a 3 × 3, 5 × 5 or 6 × 6 one, your score and best score, one step of undo, and the choice to keep going past 2048. Use the arrows or WASD, drag with the mouse or swipe with a finger. Studio → Plugins has each game's settings (level, highlights, timer, animations, undo, board size and accent colour), and Studio → Desktop widgets its width and style (Glass, Solid or Minimal). `mandara msg sudoku` and `mandara msg 2048` play them from a script.
- **A power menu made of cards.** Studio → Power & idle → Session menu → Style has a new **Cards** choice: lock, log out, sleep, restart, shut down and your other entries each float as their own card in the middle of the screen over the dimmed desktop, with the icon on a soft plate and the name underneath. Pick **Card layout → Row** to line them up side by side (wrapping onto a second row when there are many) or **Column** to stack them (wrapping into more columns). The cards pop in one after another, follow your card corners and style from Themes → Components, and work with the arrow keys (which move the way they point, also in right-to-left languages), Tab, Home and End, the number keys, Enter and Esc. Hold to confirm and the entry selected on open work as before, with the mouse, the keyboard or a finger on a touchscreen. Reduced motion shows them all at once, and small screens shrink the cards so they always fit.
- **Four bar styles that look nothing like the usual strip.** Studio → Bars → Presets has four new styles. **Expressive** is a tall bottom bar inside a thin frame round the whole screen, joined to a rail on the left: your tools sit on round accent buttons, widgets like the weather and the track playing on soft tonal tiles, and the launcher, clock, workspaces, sound, network and Bluetooth run down the rail on scalloped cookie badges. **Tile dock** is one floating island of large squircle tiles at the bottom, **Rail** a wide left rail of flower-shaped buttons in a screen frame, and **Segmented** drops the bar altogether for rows of connected tiles under the top edge. Each is made of settings you can keep tuning under Bars → Capsules and Spacing: items as capsules, tiles, squircles or connected groups, icon items as circles or Material 3 Expressive cookies, clovers, flowers and bursts in the accent or tertiary tone, how far items sit from the bar's edges, and a screen frame that now works round a top or bottom bar too. Panels still pour out of the new bars and morph back into their button. Picking any other preset or a Look puts back everything a style changed, and `mandara msg bar style expressive` (or `tile-dock`, `rail`, `segmented`, `none`) does the same from a script.
- **Side bars take the bar styles too.** Each side bar can now have a look of its own: pick *Expressive*, *Tile dock*, *Rail* or *Segmented* above its groups in Studio → Bars → Side bars, or with `mandara msg side-bar style right tile-dock`, and only that bar changes, so a rail of squircle tiles can sit beside a bar of connected groups. Its item style, item shape, icon shape, icon colour and inset can also be set on their own, each falling back to the shared side bar look when unset. *Shared* (or `none`) puts back exactly what a style changed, and a side bar you shaped by hand is never touched.

### Improved

- **Feeds is a real news reader now.** The panel opens as two panes: a sidebar with all your articles, the ones you saved, your folders and every feed with its unread count, and one timeline of article cards with the feed's icon and colour, how long ago each came out, an unread dot, a short excerpt and its thumbnail. Search and an **All | Unread** filter narrow it down, and the keyboard works throughout. Open an article to read a clean copy right in the panel (no scripts, frames or trackers; pictures inside articles only when you allow them), save it for later, mark it read or unread, copy its link or open it in your browser. The bar shows your unread count as a badge, a number or a dot.
- **Feeds in the sidebar.** A new Feeds card for the sidebar lists your newest unread articles, from every feed or one folder you choose, with each feed's colour, how long ago it came out and an unread dot. Click one to read it in the Feeds panel. The card uses what Feeds already fetched, so it never downloads anything on its own.
- **Adding feeds is easy.** In Studio → Plugins → Feeds → Settings, type a site's address and Mandara finds its feed for you; rename feeds, file them into folders, give them colours, reorder or remove them, import and export OPML from other readers, or pick from 26 starter feeds by topic, each checked to work today. In the launcher, searching "rss", "news" or "feeds" finds Feeds and its actions, and typing a web address offers **Subscribe in Feeds**.
- **Feeds that just work.** RSS 1.0 and 2.0, Atom and JSON Feed all read the same way, including feeds with odd encodings, stray text before the XML, HTML entities, missing or impossible dates and relative links. Downloads are compressed, follow safe redirects and remember when a feed moved; a feed that fails waits longer before trying again instead of hammering its server, and its last articles stay readable. Nothing downloads while nothing shows your feeds, unless you turn on the new background refresh (at most every 15 minutes), with optional, rate-limited notifications for new articles. Your existing feed list and read history carry over.
- **A new website.** Mandara's homepage is redesigned in the shell's own Material You language: expressive shapes, tonal surfaces and Alexandria type that breathes from hairline to black. Every screenshot now comes in a light and a dark version and follows your colour scheme, and an Auto / Light / Dark switch sits in the floating top bar. Pick a Look and the whole page takes on its palette. See the built-in surfaces side by side, flip through the four new bar styles, drag a slider through all five Studio designs, search every default shortcut, and browse the seven compositors and all 81 plugins. It works without JavaScript, follows reduced motion, and has a place ready for a short film of Mandara in motion.

### Fixed

- **Cut and squircle cards keep their corners.** With Themes → Components set to Cut or Squircle corners, cards in panels, the sidebar and Studio came out slightly rounded, because the clip that keeps their pictures and hover fills inside also trimmed the card's own outline. The outline now draws past that clip, so cut corners stay cut and squircles stay squircles, while the content inside is still clipped to them.
- **Live streams say "Live".** Internet radio and other live streams no longer show a running time and an empty progress line in the media panel, the sidebar, the now-playing card and the desktop media widget. They show "Live" instead, and nothing ticks every second while one plays.
- **Panels close in one smooth motion.** A drop-down panel folding back into the bar no longer pops a sliver of itself back out a moment after it looked closed, which made it seem to close in two steps (most noticeable with a low damping in Studio's motion settings). It now slides home and stays there, and a hidden sidebar no longer peeks back in after sliding away. Opening keeps its springy feel, and your motion speed, spring settings and reduced motion still apply.
- **Studio's mixer sets your music player's own volume.** Some players, such as Rufin, keep their own volume and put it back every time you play or pause, so a level set in Studio → Audio & media used to jump back. The row of the player the media panel follows now shows and sets that player's own volume, just like the shell's sound panel, whenever the player lets you change it. Other apps keep their usual sliders. You can also set it from a terminal with `mandara msg media volume set 40`.
- **The Mandara emblem shows on Mandara Login.** The default Mandara design hid its logo, so the phoenix only appeared in the Midnight card design. It now shows above the keyboard pill in the Mandara design too, including on login screens installed before this update (a logo you had moved keeps its place and setting). Turn it off with **Show a logo** in Studio → Login screen.
- **Jannah shows something new each time you open it.** Opening the Jannah panel or the sidebar used to bring back the same verse or hadith all day. Now every open picks a fresh one at random from the Qur'an and hadith you chose, never the one you just read or one of the last 16 you saw, and Jannah remembers them across restarts. A panel and a sidebar card open together still show the same reminder. *Next* on the reminder brings another fresh one and *Previous* goes back through the ones you saw. Each reminder shows its own number, such as #104 of 230, which stays the same for that verse or hadith. The desktop widgets keep theirs for the day (or the hour) and avoid what you saw lately, and today's step still changes once a day. The Qur'an's ayah of the day and Hadith's hadith of the day stay as they were.
- **Hold to confirm keeps holding when the mouse moves.** In the power menu, nudging the mouse even a pixel while holding Log out, Restart or Shut down used to cancel the hold, so it rarely finished with a real mouse. Now the hold lasts as long as the button stays down on that entry, and ends when you let go or slide onto another one.

### Under the hood

- **Pure Wayland, guarded.** Mandara has never needed X11, and now a test keeps it that way: the build fails if any X11 library ever slips into its dependencies. A new informational check also type-checks Mandara for FreeBSD, to show how far a port would be.

## 0.23.0 — 2026-11-09

A different shape for every character. Mandara Studio uses less memory.
Notes keep edits made in other apps. Clearer Copy feedback on the website.

### New

- **A different shape for every character.** The password field on the lock screen and on Mandara Login now shows each typed character as one of ten small, soft shapes (circles, rounded squares, triangles, diamonds, clovers and more) that pop in gently and shrink away when you delete them. Shapes never repeat side by side and never depend on what you typed, and each new attempt shuffles them. Prefer the classic look? Choose **Password symbols → Dots** in Studio → Lock appearance, or **Hide typing as → Dots** in Studio → Login screen. Reduced motion turns the animation off.

### Improved

- **Mandara Studio uses less memory.** Pages hand back their pictures when you leave them, images shrink without a full-size temporary copy, and searching, the sidebar and the Plugins page stop rebuilding text and copying plugin details on every frame. Over a tour of Studio's heaviest pages, its peak memory fell from 205 to 175 MiB, its memory after returning Home from 175 to 166 MiB, and its graphics memory from 100 to 73 MiB (release builds, median of three runs).
- **Notes keep edits made in other apps.** If another editor changes or removes a note's file while you are typing the same note in the Notes panel, Mandara no longer overwrites it. The panel keeps your text and says the note changed elsewhere, your other notes still save, and your next edit replaces the outside version on purpose, or recreates the note if its file was removed. (#111)
- **Clearer Copy feedback on the website.** Screen readers now announce "Copied" again each time you press Copy, the message clears after a few seconds, and the instructions shown when copying is blocked stay until you try again. (#111)
- **A clearer main menu.** Your name, login, uptime and version now sit on a soft dark scrim that keeps them readable on any wallpaper, light theme included. The rail no longer repeats the Studio and power buttons from the header: it holds the menu's pages, then, under a divider (and a "Panels" caption in the full rail), the panels it opens; Reload config moved to Apps & tools. Quick-toggle tiles give their detail line more room, so longer network and device names fit before they're cut off.
- **Pick how your sliders look.** The volume, brightness and mixer sliders keep their familiar look by default (the fill, a small gap, an upright handle and the rest of the track ending in a dot), and Studio → Themes has a new **Components** section where you can switch them to *Continuous* (one track with a short grip), *Pill* (a round knob that stretches into a pill while you hold it), *Wavy* (a gentle wave along the fill that flows while you drag) or *Segmented* (ten rounded steps). Every design is shown as a real slider you can drag to try before you pick it, and the slider handle setting sits right under them. Icons, right-to-left languages, muting, touch dragging and reduced motion work the same in every design.
- **Put the launcher button anywhere in the dock.** The dock's launcher button now lives among your pinned apps: drag it to the start, the middle or the end, drag it away or use its right-click menu to remove it, and bring it back with **Keep the launcher in the Dock** in the dock's right-click menu or **Add the launcher** in Studio → Dock, where it's listed and reordered with your apps. Your dock keeps its button where it was when you update (a button at the end moves just after your pinned apps).
- **Widget pictures that show the widget.** In Studio → Desktop widgets, every Desktop info picture now shows its widget filled with sample data, marked "Preview": a sample machine, a week of screen time, a day of prayer times, a GitHub year, exchange rates, AI usage and a shelf of games. No more empty boxes, lone icons or lines of grey text, and the search field fills its tile. Painting these pictures never reads your machine, your logs or the network.
- **Friendly empty desktop widgets.** When a desktop widget needs setting up or has nothing to show yet, it now says so properly: its icon in soft rings, a short title, one helpful line and, where a setting fixes it, a **Set up in Studio** button that opens that widget's settings or the plugin to switch on. GitHub contributions, exchange rates, AI usage, games, screen time and prayer times all do this, on light and dark wallpapers alike. Prayer times no longer disappears when its plugin is off, and screen time says when its plugin is off instead of showing old days.
- **A tidier machine card and screen time.** The machine card centres its logo beside the text (a screen glyph on a soft tile when there's no distribution logo), draws a smooth line under your user@host and shows your theme's colours as a row of round dots. Screen time's total is a little smaller, so the days and apps under it carry the widget.
- **Read every word of a verse or hadith.** The Jannah, Qur'an and Hadith sidebar cards, the Hadith panel's hadith of the day and Jannah's step evidence fold long text to a few lines that end on a whole word. *Read more* (or a click on the text) unfolds the card in place: it grows smoothly, the sidebar scrolls when it needs to, and *Show less* folds it back. Tab and Enter work too. *Open in the reader* opens that very item with every word, its chain of narrators and grades. A desktop widget says *Read more* when it had to shorten an item, and clicking it opens that verse or hadith in its reader instead of the panel's front page. Jannah's sidebar card now shows the same reminder as its panel.
- **Fresh reminders every time.** *Next* and *Previous* never land on the item you were just reading. Jannah's and Hadith's daily or hourly rotation keeps the last few items away from the start of each new round, Jannah's desktop widget walks its own list of items that fit instead of showing one twice, and the Jannah and Hadith readers stop at the end of a list instead of wrapping back to the first item. Jannah's reminder shows where it stands in its round, such as 12 / 230.

### Fixed

- **You can resize and move a screenshot selection.** In the screenshot tool, drag a corner or edge handle to resize the area you picked, or drag inside it to move it; only a press outside starts a new selection, and a stray click no longer throws your selection (and its annotations) away. The handles are easy to catch with the mouse or a finger, the pointer changes to show what a drag will do, and the selection stays on the screen. With "Capture on release" on, only letting go of a new selection captures; resizing or moving one never does. While an arrow, box or circle tool is active the handles hide and dragging draws, as before.
- **Touchscreen taps now work in the shell and Studio.** Tapping a bar item, a panel control or anything in Studio, including its close button, works like a click. Swiping scrolls lists and pages, and a finger on a slider drags it. Your mouse keeps working exactly as before. (#130)
- **Every launcher look tells you what you can type.** The empty search field in Classic, Grid and Beacon now lists the same prefixes, taken from your settings: `/` slash commands, `=` calculate, `!` to ask the AI Shell Agent while it's switched on, and each search provider you have on with the prefix you gave it. Beacon used to show only a generic line. Changes in Studio show up while the launcher is open.
- **The launcher follows search settings while it's open.** Turning a search provider on or off, or changing its prefix, in Studio or the config file now applies to an open launcher straight away. A Beacon mode whose provider you switch off closes back to plain search instead of staying stuck. (#111)
- **Your theme's name is translated even when typed by hand.** Studio's current-theme heading now shows the translated name when the config names a built-in theme in another spelling, such as `Rosé Pine` or `tokyo_night`. Your own themes keep the titles you gave them. (#111)
- **Drop-down panels stay open while you use them.** A click inside a bar panel or the main menu now always counts as using it, so a short focus change right after it (switching light and dark, a keyboard or mouse coming or going, the compositor rearranging its layers) no longer closes the panel under your pointer. Pressing a key while you rest on a panel no longer reads as leaving it either. Moving away, clicking outside, Escape and switching to another app still close it.
- **The machine card no longer looks broken.** Its gallery picture, and the moment before your machine had been read, showed a lone "@", a dash and "Uptime: 0 min". It now waits for the real values and leaves out anything it doesn't know yet instead of showing a zero. In demo mode it names the same made-up parts as the Resources panel, so screenshots never show your hardware.
- **No more cut words or stranded vowel marks.** Shortened Arabic and translations, in the shell and on desktop widgets, now end after a whole word, and a letter never loses its tashkeel where a line is cut. The Qur'an's desktop widget always gets the ayah of the day whole.
- **Playing or pausing no longer brings an app's old volume back.** Some music players keep their own volume and put it back on their sound every time you play or pause, so a level you set for the app in the sound panel or sidebar jumped back (from 25 % to 51 %, for example). The row of the player you control from the media panel now sets that player's own volume, the same level as the player's own slider, so it stays where you put it. Other apps, and players that don't let you change their volume, keep their usual per-app slider.
- **The password field holds still while you type.** On the lock screen every new character re-centred the whole row, so all the dots slid sideways with each key press; on Mandara Login the same happened in right-to-left languages, and long passwords hid their newest characters. Symbols now start from a fixed edge and keep their places as you type or delete, and a full field scrolls by one so the newest one stays in view, in every language and at fractional scales.

### Under the hood

- **Fewer needless rebuilds in git worktrees.** The build no longer treats every build in a linked worktree as changed, which rebuilt most of the workspace each time. It now watches the checkout's real `HEAD` and branch, so the embedded revision also follows new commits. (#111)
- **Documented helpers and test rules.** The background workers, saving and image-rendering code have fuller explanations, and the development guide describes how the desktop tests own, tear down and stop restoring their private desktop. (#111)

## 0.22.0 — 2026-11-07

Copy any panel as an image. Motion that suits you. Swipe notification
history away. More apps in your colours.

### New

- **Copy any panel as an image.** A camera beside the pin and float controls copies the visible panel straight to your clipboard and confirms with a green check. Floating panels have it too, including third-party plugins. Captures stay off the UI thread and release their working pixels when finished or cancelled.
- **Motion that suits you.** Choose a spring preset, replay a live preview, follow the desktop's reduced-motion preference, soften bounce and set transition speed separately for each shell surface. Existing tuning stays yours. (#102)
- **Swipe notification history away.** Drag a card left or right in the notification panel or sidebar to dismiss it. Vertical scrolling and small drags keep the card; Studio has an off switch and distance control. (#119)
- **More apps in your colours.** Cava, tmux, Zellij and Spicetify now have opt-in palette exports in Studio → Themes → Other apps, with generated files that preserve your own configs. (#123)

### Fixed

- **More reliable local commands.** Opening the Qur’an, Jannah or calendar opens its panel. The agent keeps alternative commands available after compilation, respects light/dark mode and explicit values, and uses FunctionGemma’s native tool-call format for arguments. Selecting an alternative compiles a fresh preview; selecting one step runs only that step.

- **Keep the main menu open while choosing a theme.** A temporary loss of layer-surface activation without another app taking focus no longer dismisses the panel under your pointer. Outside clicks, Escape and switching apps still close it. (#113)

## 0.21.0 — 2026-11-07

Your desktop, in your own words.

### New

- **Your desktop, in your own words.** Enable the AI Shell Agent in
  Studio, download its models, then type `!` in any launcher look. Review the
  exact shell actions before pressing Enter. Inference runs offline on your
  CPU, keeping inference off the UI thread so the desktop stays responsive.
  Its worker exits when finished, cancelled or closed. Studio has
  verified, resumable model downloads and a test bench that never runs commands.

## 0.20.0 — 2026-11-07

Share a complete reading card. Write on your desktop. A clearer launcher
placement map. More Qur'an reciters.

### New

- **Share a complete reading card.** Qur’an, Hadith and Jannah have an Image
  button beside text copy. It copies the whole entry with its current design,
  reference and displayed meaning, including content beyond the panel’s scroll.

### Improved

- **Write on your desktop.** Edit tasks and Markdown notes directly in their
  widgets, without opening a plugin panel. The checkbox still ticks a task;
  its words open the editor. Save failures and conflicting edits keep your
  text available, and a failed new-note write leaves no phantom note behind.
- **A clearer launcher placement map.** Studio shows the selected position
  beside a compact map, with keyboard-accessible choices and a Preview button.
- **More Qur'an reciters.** Choose Yasser Al-Dossary, Abdulrashid Sufi or
  Abdul Basit Abdul Samad in Studio. Yasser is the new default; saved
  Alafasy and Al-Ajamy choices use it without changing your settings.
  Sufi's chapter downloads show progress and keep their ayah timing for
  listening offline.

### Fixed

- **Fresh language and theme in plugin cards.** Soundscapes captions and
  progress rows follow language and colour changes without waiting for their
  underlying data to change. World Clock follows language, date-format and
  time-zone changes immediately too.
- **Less work while scrolling Launcher settings.** Studio prepares app names,
  icons and ranking data in the background and shares them between rows,
  instead of rescanning applications during scrolling. (#125)
- **Launchers follow compact bars.** Attached left and right positions align
  with the ends of a notch, micro pill or island instead of the screen corners.
  Button morphs keep that alignment and stay within the display. At a Notch
  end, the launcher and bar share one continuous outer curve.
- **Smoother after startup.** The shell leaves its executable in the kernel’s
  file cache instead of forcing it to be read from disk again during cleanup.
  It also leaves file mappings under kernel control, avoiding a race with
  temporary font and driver mappings.

### Under the hood

- **Quieter image decoding.** Clipboard previews decode one at a time and stop
  when their view closes. Clipboard and media caches request memory reclamation
  after releasing images, using the existing system allocator. Rapid track
  changes keep only the latest pending artwork request, and late results cannot
  bring back artwork or lyrics after a view closes.
- **Lighter plugin switches.** Enabling and disabling plugins no longer copies
  or retains unrelated plugin settings when other preferences change.
- **Shared theme labels.** Palette copies and color transitions share their
  name instead of allocating another copy for each frame.
- **Less temporary work.** Input regions reuse their frame buffers, network
  totals avoid temporary lists, and process sampling reuses small file buffers
  while reading CPU tables without a size limit. Plugin text and image
  conversions keep their existing storage when possible.
- **Safer tray icons.** Invalid icon dimensions are rejected before pixel-size
  arithmetic can overflow.
- **Less repeated work in plugin views.** Clipboard previews and selection share
  one layout, media seek controls keep their gesture geometry, and network
  charts read their existing history without keeping a second sample copy.
  Prayer times reuse resolved settings and keep their fixed daily timeline
  without a temporary event list. Search results keep reusable indices, and
  clipboard rows share their action state instead of rebuilding callbacks.
  Album art and sharing or dock previews consume their resized image buffers,
  and temperature rows borrow their readings and labels. Several plugin
  selectors also read text settings without making temporary string copies.
  Feeds, home controls and the faith plugins resolve their text and list settings once per
  config or language change. Daily progress snapshots also avoid repeated
  date formatting while a view animates. Keyboard badges compare borrowed
  labels, and Phone Connect filters its existing device list directly.
- **Shared font settings.** Text and clock draws reuse their empty and
  tabular-number feature lists while custom font changes stay independent.
- **Lean bar geometry.** Joined columns keep their two corner flares inline,
  and group capsules calculate their bounds without a temporary list.
- **Lighter clock and break updates.** The meeting planner keeps its drag
  geometry between frames, and break reminders follow their own effective
  settings without copying unrelated plugin preferences.
- **Background saves for notes and tasks.** File writes stay off the UI thread
  and keep their request order. Rapid task changes share a save, commands
  report disk failures, and a partly saved note can retry its filename or
  index without creating another copy. `shell state` also reports when a
  desktop editor is ready for keyboard input.
  Unreadable notes keep their original contents while other notes stay usable;
  drafts stay available for retry once the file can be read again.
- **Quieter distribution packages.** Language packs are staged separately
  from compiled binaries, avoiding a misleading packaging warning while
  keeping every language in the finished packages.
- **Safer release downloads.** The release job uses the current artifact
  download action and rejects corrupted build artifacts before signing.

## 0.19.1 — 2026-11-06

Copy installation commands. Fresh desktop pictures. Theme cards fill their
space. Keyboard navigation in Beacon.

### Improved

- **Copy installation commands.** The website and its guides offer a Copy
  button beside code examples, with keyboard access and clear feedback when
  copying succeeds or the browser blocks it.

- **Fresh desktop pictures.** The README keeps its familiar widgets with
  the repaired sticky notes on a new wallpaper. Its banner now says twenty
  languages, matching the available language packs.

### Fixed

- **Theme cards fill their space.** Main menu → Look → Themes uses the
  available grid height, and built-in names follow your language in the
  menu and Studio. Custom theme names stay as you wrote them.

- **Keyboard navigation in Beacon.** Its search and mode buttons are reachable
  with Tab and Shift+Tab, with visible focus and Enter or Space to select
  a mode or app category. (#120)

### Under the hood

- **Cleaner portable builds.** Linker tuning checks for support, avoiding
  ignored-option warnings with other linkers.
  CI and release uploads use the current artifact action to avoid its older
  runtime deprecations.

- **Reliable integration tests.** The private KeePassXC server listens before
  client requests arrive, and its waits have a deadline, preventing a CI hang.

- **Clearer helper contracts.** Internal documentation explains policy cache
  limits, view lifetimes, Studio routing and native output sizing. (#111)
  Performance figures now distinguish settled memory from startup peaks
  and describe the measured release setup.

## 0.19.0 — 2026-11-06

Bars and sidebars share one focused settings page, with clearer installation
steps and less repeated work behind the desktop.

### Improved

- **A clearer launcher icon.** The phoenix fills more of its button, and
  its icon chooser includes thirteen distinct colour SVG alternatives.
  Icons decode at their drawn size.

- **Bars in Studio.** Bar layouts, side bars, sidebar cards, panels and
  edge actions have their own tabs on one page. Existing settings and
  Studio shortcuts keep working, and translated directions use the tab labels.

### Fixed

- **Video wallpapers reach every edge.** They fill the screen at fractional
  display scales, including the solid color around centered and fitted clips.

- **Store checks can recover from a busy refresh.** A lock timeout reuses
  the last valid policy within its existing age limits, preserving blocked
  entries without starting another refresh. (#111)

- **Script fonts stay readable.** Hindi and Marathi text can use their
  selected script font directly, while primary font validation still
  protects text measurements.

- **Installer retries are clearer.** Numbered stages confirm the release
  signature and archive checksum, completed downloads show their sizes,
  and exhausted signature-download retries stop before fetching the archive.
  Manually downloaded binaries also stop when verification is unavailable,
  unless you explicitly choose `--no-verify`. (#115)

- **No bright wallpaper corners beside the Notch bar.** The power menu
  and attached launcher dim the wallpaper around the bar’s actual shape. (#114)

### Under the hood

- **Temporary folders can use symbolic links.** Resource tests accept
  folders reached through a symbolic link. (#111)

- **Reuse plugin views.** Theme and geometry edits keep the bar and sidebar’s
  live plugin slots.
  Wallpaper scheme previews cache their five displayed colors and reuse
  them when you return to a seed or mode.

- **Less background work in the dock.** It checks window geometry only for
  Smart hiding and keeps its slots cached across unrelated service updates.
  Media position updates and screen-time readouts avoid copying retained
  track data and history.
  Switching Feeds, Home Assistant or Phone Connect off releases their
  cached snapshots, including while an older request finishes.

- **Shared OCR dependencies.** Text recognition shares newer geometry,
  random-number and iterator libraries with the rest of the shell.
  Compatible dependency updates also cover configuration parsing, downloads,
  archives and background tasks.

## 0.18.0 — 2026-11-05

Easier sound switching, clearer menu cards, and fixes for notification focus
and compositor previews.

### New

- **Mandara speaks Portuguese, Marathi, Vietnamese, Telugu and Swahili.**
  Choose yours in Studio → General → Language, or follow the system
  language. Translations ship as separate language packs.

- **Choose speakers and microphones where you adjust volume.** Click the
  current device in Sound or the sidebar to pick another one. The lists use
  your labels and hidden-device choices and follow devices as they connect
  and disconnect. (#109)

### Improved

- **Plugin updates stay focused.** CI status and connected tools keep their
  cached views when unrelated services change. Closed panels stop their
  refresh ticks, and CI's running indicator respects reduced motion.

- **Widget previews show the real layout.** Jannah, Hadith and Qur’an cards
  use clearly marked examples instead of generic fallback icons.

- **Language packs cover the login screen too.** All twenty interface
  languages use separate packs, including the greeter’s own wording.

- **Community store moderation stays current.** Account and plugin rules can
  hide listings or block installs and updates without a Mandara release.
  Direct installs follow the same rules; plugins already installed keep
  running.

- **Theme cards show the theme.** Looks → Themes uses compact material
  previews, clearer names and a selected marker, with visible keyboard focus.
  User themes load in the background while the menu is open.

- **Home cards follow your menu decoration choices.** Clock, media, volume
  and quick toggles share the patterns, surfaces and shadows used by Apps &
  tools. Active switches keep their on/off highlight.

### Fixed

- **Restart keeps your latest preferences.** Pending edits are saved before
  restarting, manual light or dark overrides survive automatic-theme
  restarts, and invalid configuration files are protected from replacement.

- **Video wallpaper keeps its margins.** Playback retains the chosen fill
  around contained or centered pictures while overview roots stay transparent.

- **Studio leaves finished pages alone.** Late wallpaper and generator
  previews cannot restore released images or queue hidden work. Home and
  Performance sampling follows navigation and window visibility.

- **Community search stays on your query.** Reopening the store keeps the
  search shown in its field while checking the latest moderation rules.

- **A notification can bring its app forward.** Clicking a notification can
  focus its existing window even without a sender-provided click action.
  Advertised actions still open their own content, and app lookup avoids
  ambiguous matches and duplicate launches. (#107)

- **Keep Umbriel's overview clear.** The wallpaper layer no longer contributes
  a redundant solid rectangle to workspace previews. Desktop wallpaper and
  transitions keep using their existing buffers. (#110)

- **Quieter popup teardown.** Closing windows stop requesting frames, and
  notification popups use your configured panel font. Hyprland keeps a valid
  client snapshot if only the optional monitor lookup fails. (#111)

- **Prepare wallpapers safely on several displays.** Concurrent outputs use
  separate temporary files when preparing the same cached wallpaper.

### Under the hood

- Changed helpers and regression tests document their ownership, validation
  and teardown boundaries.

## 0.17.0 — 2026-11-05

Four connected tools, more desktop styles, and public widgets for login and
lock screens, with clearer previews and setup checks.

### New

- **Ten more ways to dress your desktop.** Add word, split-flap, glass-tube
  and binary clocks; date tickets, perpetual cards and a moon calendar;
  weather ribbons, thermometers and a humidity rose. They belong to the
  existing widget families, so Mandara still ships 77 first-party plugins.
  Word clocks use natural Turkish time phrases.

- **Put widgets on your sign-in screens.** Login screen → Layout and Lock
  appearance have a widget gallery, a copy-from-desktop option and placement
  controls. Both start empty. Only public, passive kinds are offered;
  weather needs a separately chosen public location. Plasma keeps its own
  secure locker, with a clear explanation in Studio. World clocks have
  public default cities; copied decoration text remains public; copied
  countdowns leave private desktop titles and dates behind. Embedded and
  full-screen previews stay offline. Forecast requests start only for
  widgets that actually display weather; palette labels follow each widget’s
  chosen language. Lock widgets appear promptly when background painting
  finishes, even after the entrance animation settles.

- **Four optional connected tools.** Translator, Phone Connect, Feeds and
  Home Assistant are first-party plugins. Translate when you ask, keep up
  with subscriptions, reach your paired phone and control your home from
  native panels. Mandara now ships 77 first-party plugins, each with its own
  artwork. Feeds ask for the final address when a server redirects; remote
  API keys use HTTPS. Key settings recheck the keyring when reopened and
  stop reading on close. Home Assistant favourites load individually and
  skip removed entities without hiding connection failures. Replacement keys
  stay saved when settings are closed and reopened during a write.

### Improved

- **Find missing icon themes.** Maintenance reports missing icon themes and
  opens Themes so you can choose a replacement without resetting your
  selection. (#87)

- **The project, a click away.** Studio’s About header has playful GitHub
  and Discord links that move gently on hover and respect reduced motion.

### Fixed

- **Switch to the window you picked.** Alt+Tab hands focus back after its
  overlay closes. Program keybindings also accept `exec`, and Hyprland
  instructions use its current Lua commands. (#105)

- **Capture the display you are using.** Screenshots and window or region
  selection follow the focused output, including monitors to the left of the
  rest of the layout. Frozen frames stay on the display they came from. (#106)

- **Icons that say what they do.** Quotes, wallpaper tools, widget menus and
  settings use symbols for their purpose. Sparkles are reserved for AI;
  procedural artwork uses flowing contours and a paint drop.

- **Tell centered bar presets apart.** The Micro pill and Dynamic island
  thumbnails use the bar’s real compact geometry and show their different
  item arrangements. (#103)

- **Sharper widget previews.** Calendar, sky and visualizer pictures fill
  their cards and follow display scaling. Media styles have labelled sample
  previews instead of empty tiles, in light and dark themes. Toggle and
  slider examples and current palette colours fill their tiles too. (#101)

- **Keep the login preview current.** The login designer keeps clocks and
  public widgets current while its preview is visible, including across
  midnight. Digit rolls finish smoothly, and enabling seconds takes effect
  without waiting for the next minute.

- **Dry runs stay compact.** The installer keeps its one-line heading on an
  interactive dry run and leaves the setup menu closed. (#66)

- **Keep CPU groups within the whole machine.** Process hunter and Resources
  cap grouped totals at 100%; hot-process highlighting follows the sampler’s
  host CPU count. Includes Hedayatbakhshi’s follow-ups. (#68)

- **Fast clipboard paging keeps up.** A quick wheel movement turns all
  requested pages and keeps the leftover distance for the next movement.
  Builds on Hedayatbakhshi’s keyboard navigation. (#70)

- **A quieter website in dark mode.** The compositor section and expanded
  questions follow the page’s theme with a subtle surface accent, readable
  links and visible keyboard focus.

- **Keep desktop forecasts live.** With weather enabled, a desktop weather
  widget keeps the forecast service running without a bar or sidebar item.
  New weather-card labels follow the widget’s reading direction. Daily
  ranges follow the forecast location’s current date, even when a refresh
  fails overnight.

- **Smoother battery rings.** The Power panel’s charge arc and round marker
  keep their smooth edges at normal and fractional display scales. (#104)

### Under the hood

- Snipping, eyedropper and login widget images keep one CPU pixel buffer
  after painting. GPU uploads avoid another image copy, and native builds
  leave out unused browser shader translation.

- Clipboard history shares its text between the service, panels and launcher.
  Window previews move their captured pixels directly to the renderer, and
  closing a window releases its preview without reopening the switcher.

## 0.16.0 — 2026-11-05

Put the power menu where you want it, plus a round of fixes for Sway, the
installer, display reconnection and native KWin support.

### New

- **Choose which screen corners are rounded.** Each corner can be switched
  off separately, and adjacent displays can keep their shared corners clear.
  Find these controls in Studio → Sidebars & hover → Screen corners. (#73)

- **Keep dock bounces for music time.** Studio → Dock → Behaviour has an
  optional **Bounce only while music plays** setting for launch and
  attention animations. Paused or stopped playback leaves them still. (#82)

- **Save a setup for just your bar.** Profiles & backup now offers a
  **Bar** scope, and the bar presets include a system-monitor arrangement.
  Applying a bar preset keeps your own items unless you explicitly choose
  **Replace bar items**. Desktop widgets stay as you left them. (#73, #81)

- **Capture all your displays at once.** The screenshot panel's
  **All displays** action saves or copies the whole desktop in one image,
  respecting monitor positions and different scales. The same action is
  available as `mandara msg screenshot all`. (#79)

- **The installer opens on a setup screen.** `mandara install` now starts
  with the MANDARA wordmark and an arrow-key menu: pick the compositor to
  integrate with (the list starts on one you already have; left alone, the
  row stays on *detect automatically* and `setup` picks what's running),
  switch autostart and Mandara Login on or off, then choose Install. A
  choice you pass as a flag is left out of the menu, and `--yes`,
  `--dry-run` or a run without a terminal behave as before, with the
  one-line title. Canceling either menu leaves the installation untouched.
  Thanks to @Hedayatbakhshi. (#66)

- **Mandara now runs on Plasma Wayland with a native KWin driver** (#45,
  #50). The bar follows virtual desktops and windows, and the dock and
  switcher control windows through Plasma’s own protocols. Clipboard history
  also works through the shared data-control protocol. Setup can add
  Mandara’s startup entry without changing KWin’s settings. Studio explains
  which settings stay in Plasma System Settings, and lock requests use
  Plasma’s secure locker.

- **Choose where the power menu opens.** Studio → Power & idle → Session
  menu → *Position* puts the edge dock on the left or right side, or at the
  top or bottom center. At the top it hangs from the bar as one piece with
  it, curves and all, just like the drop-down panels; next to a floating
  bar it opens as a card, and if the bar is on another edge it hangs from
  the screen edge (the preview tells you which). The default stays the side
  opposite the sidebar. (#57)

### Improved

- **The clipboard answers the keyboard and the wheel.** Arrow Up and Down
  move through the list and turn the page when they reach its end, Enter
  copies the entry under them (or pastes it, where the panel is set to
  paste), and the wheel now turns pages instead of doing nothing. Search
  keeps its own keyboard focus, hidden entries cannot stay selected, and
  a change in scroll direction discards the old wheel distance.
  Thanks to @Hedayatbakhshi. (#70)

- **A new home for Mandara.** The website has interactive Look previews,
  a tour of Studio and desktop widgets, and a clearer installation guide.
  It follows your light or dark theme, respects reduced motion, and keeps
  keyboard focus visible when a question is open.

- **A shorter, friendlier first install.** The new download command shows
  the phoenix and real download progress, picks the right Linux build,
  verifies the signed archive before opening the installer, and passes your
  configuration options through unchanged. Interrupted downloads report a
  failure, package-manager questions can read your terminal normally, and
  relative installation paths stay relative to where you ran the command.
  Downloads stay on HTTPS, temporary files are removed even after a failure,
  and the verified installer uses your cache when `/tmp` blocks programs.
  Its translated hints follow your terminal language, including in help.

- **The installer can start before keyboard libraries are installed.**
  Keyboard support loads when it is first used, so a fresh Linux system
  can run the installer and let it install the packages it needs.
  With `--no-deps`, shortcut checks explain which library is missing and
  leave configuration alone.

- **A lighter first test build** (#64). Tests no longer inherit the
  interactive shell’s dependency optimization or keep incremental caches.
  The development guide explains focused checks, timing reports and optional
  local linkers; the full workspace checks still run in CI.
  CI's build and test steps now share that profile instead of compiling
  the dependency tree twice with different optimization settings.

- **Guides that match today's shell.** The docs show the power menu at the
  top centre and at the side, each Look's picture follows your page's light
  or dark theme, and old panel captures that no longer looked like Mandara
  are gone.

### Fixed

- A bar preset stays selected after you apply its style while keeping your
  own items. Manual layout or item changes clear that selection. (#81)

- Installation finishes with translated first steps in every language.

- Installing or removing an application updates its dock pin and icon
  without a restart; missing saved pins stay hidden.

- Resetting a page keeps removed plugins out of its layout until you
  restore them.

- **KWin setup explains availability failures.** Only installed KWin
  appears in the picker. Failed probes keep their actual cause in setup
  and Studio, without treating them as configuration or shortcut errors.

- The download command preserves the installer's normal file permissions.
  Unnamed displays retain their logical size alongside named monitors, and
  Hyprland refreshes window state as soon as its event connection opens.

- **Reach clipboard matches after searching.** Tab moves from the search
  field to the results; arrows select and Enter uses the selected copy.
  Editing the search or a collection name keeps its own keyboard input.

- **Studio has one set of window buttons.** Its title bar now requests
  client decorations and yields to compositor-provided controls when
  those are required. (#89)

- **Reset-page summaries count custom arrangements too.** Restored bar
  items, side-bar groups, dock pins and stacks, and sidebar tabs no longer
  appear as “All defaults” when they differ from the defaults. Resetting
  their page restores those lists too. (#88)

- **The dock skips pinned apps that are not installed.** Their pins stay
  saved and return when the app is available; a running window still
  appears even when it has no launcher entry. (#73)

- **Frame timing stops showing an old interaction as a live frame rate.**
  Idle redraw history no longer produces a current cadence, and Studio
  labels the measurements as CPU view builds and redraw intervals. The
  dock's separate view contributes its own animation samples too. (#84)

- **Tailscale detection handles a shorter desktop PATH.** Mandara also
  checks common package locations and distinguishes a broken executable
  from a missing installation. The missing-package panel links to its
  official Linux download instructions. (#73)

- **Icons follow Plasma's selected theme too.** When no theme is chosen
  in Studio, Mandara reads Plasma's icon preference as well as GTK's. (#73)

- **Screenshots use KWin's own capture service on Plasma**, including
  monitor names, cursor choice and full-resolution pixels. (#78)

- **External brightness setup remembers the next boot.** Working I²C
  access no longer skips the installer's offer to load its module at
  startup, and a failed module load is reported. (#77)

- **The bar picker only offers things it can draw.** Desktop widget
  families and background tools stay in their own galleries instead of
  being added to the bar as invisible items. (#73)

- The Bengali input-seat hint and Persian login-screen choice are clearer.

- **Recover even without keyboard libraries.** Restoring a backup works
  without libxkbcommon and leaves the restored files in place. Shortcut
  checks explain a missing library before changing anything, without
  offering to retire conflicts that could not be checked.

- Display changes recover even when both geometry queries fail. A stale
  monitor list cannot give a new display another monitor’s name or dimensions.

- **The process table counts the whole machine.** A busy build used to
  read "405.5%" for `rustc`, because the list measured each process
  against a single core while the Resources panel measured all of them.
  Both now show a share of your whole machine, so no row goes past 100%,
  and the *hot* colour still starts at the same amount of work as before,
  including when Mandara runs with a CPU quota or affinity limit or CPUs
  are brought online or taken offline.
  Thanks to @Hedayatbakhshi. (#68)

- **First-install instructions account for your current terminal** (#72).
  If you added Mandara to PATH, the installer asks you to open a new
  terminal before using its commands. It also points to Studio in your
  application menu. Plasma Login Manager is recognised when switching to
  Mandara Login.

- **The website header stays readable in both themes.** The page and its
  transparent banner now follow the same theme, and the browser tab shows
  Mandara’s phoenix. Code examples keep their syntax colours with readable
  contrast in light and dark mode.

- **A slow compositor reply no longer holds up the first frame.** Initial
  workspace and window queries run in the background before the shell
  follows the live event stream.

- **Locking from a hot corner keeps the shell responsive.** Waiting for
  the secure lock screen now happens in the background.

- **Tape stays attached to notes.** The notes desktop widget uses the same
  corner strips as the to-do card, anchored to its visible paper. A header
  or extra frame padding no longer leaves the strips floating above it.

- **Source installs check the Rust version the dependencies actually need.**
  An older toolchain is updated before the build instead of failing partway
  through compiling the WebAssembly runtime.

- **The bar and dock come back when a display reconnects** (#63). Switching
  a monitor between computers, unplugging it or changing its resolution no
  longer leaves that screen without its shell. Other displays keep their
  surfaces, and your display assignments stay as you set them. A temporarily
  unavailable display query is retried without replacing working surfaces.
  If the compositor still omits a connected output, its last known logical
  size stays in use until a complete update arrives.
- **Installing again keeps your shell profiles safe** (#49). A leftover
  installer comment no longer removes the next setting. Profile updates
  preserve symlinks and permissions, and a failed write leaves the original
  intact. Changing the installation folder also refreshes old PATH entries
  when the new folder is already on PATH.
- **Named Sway workspaces work with automatic bar hiding and the window
  switcher** (#49). Includes can mix literal and wildcard characters, and
  shortcut conflicts use the variable value at the shortcut's own line.

- **Bar panels no longer hide behind the sidebar** (#62). With the sidebar
  open, a panel such as Power or Weather used to open partly underneath it,
  so the sidebar's cards looked washed out over the panel. The panel now
  moves aside and opens next to the sidebar. On a screen too narrow for
  both, it opens on top of the sidebar, fully opaque.

- **Empty panels look right again** (#61). In short panels such as Wi-Fi's
  *No networks found* and Bluetooth's *Bluetooth is off*, the icon sat high
  in its round badge and the badge ran into the title. The badge now shrinks
  to fit, with its icon centred.

- **The installer's PATH lines survive unusual folder names.** A home
  folder with spaces, quotes or a `$` in its name now ends up on your PATH
  as it is, in bash, zsh and fish alike, and installing again to another
  folder replaces the old line instead of keeping it. (#49)
- **Pressing Ctrl-D at an installer question means No.** Enter still picks
  the suggested answer, but a closed or failing terminal no longer says yes
  for you. (#49)
- **The launcher no longer vanishes when reopened quickly.** Closing it
  before it had fully appeared and opening it again right away could make
  it close itself two seconds later. (#49)
- **Sway: every workspace is reachable.** A workspace named with quotes now
  gets the right command, a named workspace such as `web` no longer blocks
  a numbered one that doesn't exist yet, and its id no longer changes when
  other workspaces come and go. (#49)
- **Sway: your config is read the way Sway reads it.** Includes with a
  wildcard in a folder (`conf.d/*/keys.conf`) or several files on one line
  are followed, a `set $mod` you change later counts with its new value, and
  a line that merely mentions `[ERROR]` (say, in a notification) is no
  longer taken for an error. (#49)
- **The shortcuts list keeps whole workspace names.** A Sway bind to
  `workspace "dev ops"` now reads "Go to workspace dev ops", and River binds
  with `-layout` show the right action. (#49)
- **The NixOS setup message speaks your language.** When your compositor
  config comes from Nix, the explanation of what to add is translated into
  all 14 languages. (#49)

### Under the hood

- Release jobs keep GitHub write credentials out of dependency builds and
  use them only for publishing and pushing release metadata.

- **Release builds use an immutable checkout action.** CI and packaging
  share the same verified action commit. Setup's availability tests also
  find their fixture programs through PATH, including on NixOS.

- **Build cleanup releases its locks promptly**, even when another tool
  starts at the same moment, so unused build caches can be removed reliably.

- **ARM builds avoid an unsupported linker option.** Packed relocations
  are requested only on architectures GNU ld supports, keeping ARM build
  output clear of the ignored-option warning.

- **Build checks use the current GitHub Actions runtime.** Checkout no
  longer reports the retired Node runtime during tests and releases.

- **Clearer docs for NixOS, River and Sway.** The README says NixOS users
  update through their flake, the install guide notes that River has no
  include line or checker, and the Studio guide's Sway notes are more
  precise. The flake's Home Manager check now runs as pure Nix assertions,
  and the screenshot setup notes regenerate their wallpapers reliably. (#49)

## 0.15.0 — 2026-11-03

Fresh screenshots everywhere. Cleaner main menu tiles.

### Improved

- **Cleaner main menu tiles.** The shortcut tiles are now flat by default,
  and every tile wears a faint geometric pattern of its own: a grid for the
  launcher, rising hills for wallpapers, a honeycomb for Studio and more.
  The Kufic calligraphy is still there if you liked it: turn on Studio →
  Bar → Main menu → *Kufic calligraphy*. Raised 3D and glass tiles are a
  choice away under *Tile surface*.

- **Fresh screenshots.** Most screenshots in the README and the guides
  were shot again, each on its own wallpaper, so they no longer blur together and the
  text is easy to read. The six Looks now sit in a neat grid in dark or light
  mode to match your page, Mandara Login shows both of its designs (Mandara
  and Midnight card) on bolder backdrops, and the sky widgets come together as
  one golden-hour sky dashboard.

## 0.14.0 — 2026-11-03

A Mandara Discord. Mandara on NixOS. Mandara now runs on Sway. Mandara now
runs on River Classic.

### New

- **A Mandara Discord.** Share your setup, ask for help and suggest ideas
  at [discord.gg/kPBaS2reQ3](https://discord.gg/kPBaS2reQ3). The link is
  also on the README.

- **Mandara on NixOS.** Mandara is now a Nix flake: try it with
  `nix run github:theHamdiz/mandara`, install it with
  `nix profile install`, or add it to your system flake and turn it on
  with `programs.mandara.enable = true;`. The NixOS module sets up what
  the installer does on other distributions (the lock screen's PAM
  service, polkit, desktop portals, fonts and the services Studio talks
  to), and `programs.mandara.greeter.enable` makes Mandara Login your
  login screen. A Home Manager module manages your settings and can start
  Mandara as a user service. Nix keeps Mandara updated, so Studio's
  updater steps aside and says so. See the
  [install guide](https://github.com/theHamdiz/mandara/blob/main/docs/install.md#nixos).
  Thanks for asking! ([#39](https://github.com/theHamdiz/mandara/issues/39))
- **Mandara now runs on Sway.** The bar shows each screen's workspaces
  (which are in use, which needs you, which is showing), the windows on
  them and the layout of the focused workspace, plus a badge while you're
  in a binding mode such as resize; clicking switches workspaces and
  moves windows. `mandara setup` adds one `include` line to the end of
  your sway config (starting your own copy of the system config if you
  have none) and keeps Mandara's colours, gaps, window rules, input,
  display and key settings in their own file. Where sway is installed,
  every line is checked with `sway --validate` first and a change sway
  rejects is rolled back; the Config doctor reads sway's own verdict on
  your config, and moving over from swaybar or swaybg is part of the
  usual takeover (#33).
- **Mandara now runs on River Classic.** River's tags work like MangoWM's:
  click to show one, right-click to add it to the view, middle-click to
  put the focused window on it too, with the tags in use and the urgent
  ones marked and the current mode shown. River's config is your own
  `init` program, so Mandara never touches it: it sends River its border
  colours, window rules, keyboard, mouse, touchpad and cursor settings and
  its keys over River's own protocol whenever the shell starts or a
  setting changes, leaving the keys your `init` maps alone. Add one line
  to `init` to start Mandara with River (Studio and the Config doctor show
  it). This is for River 0.3 (`river-classic`); River 0.4 moved window
  management out of River, and the Config doctor explains that if it finds
  it (#34).
- **A main menu rail with names.** Studio → Bar → Main menu → **Rail** now
  offers **Full**: every page and shortcut in the main menu's side rail
  shows its name beside its icon, and the open page is filled in the
  accent colour; the menu grows a little so the cards keep their size.
  **Compact**, the icon-only rail you know, stays the default.
  Contributed by Hedayatbakhshi (#41).
- **The installer can install your compositor.** When none of MangoWM,
  Niri, Hyprland or Umbriel is installed yet, `mandara install` lists the
  ones your distribution's own repositories carry and asks which one you'd
  like (it never adds a repository), installs it with its screen-sharing
  portal and hooks Mandara into it. Or name one: `mandara install
  --compositor niri` installs Niri first when it's missing. Thanks for the
  idea in #45.

### Improved

- **Ready for 1000 Hz displays.** A typical frame builds in 0.1 ms, and
  95% of frames finish inside the 1 ms a 1000 Hz display allows. The
  numbers and how to check them on your machine are in the performance
  docs.

- **A smaller Mandara.** The translations for all fifteen languages used
  to be built into Mandara itself. They now live in language packs next to
  it, so the program is 9.2 MB smaller (169 MB → 159 MB), which also makes
  this update's patch smaller. Packages and the release tarballs still
  include every language (the tarball stays about the same size, 69 →
  71 MB), so nothing changes when you install, and a language added later
  is a download of about 300 KB.
- **Languages download when you pick them.** Choose a language in Studio
  (or for the login screen, or as a plugin's own language) and Mandara
  fetches its pack if it's missing, checked against the release's
  signature, then switches over. The language's own card fills up as the
  download comes in ("Downloading Arabic… 180 KB"), from the right in
  right-to-left languages; it turns green with a check when it's done, or
  red with the reason and **Retry** if it can't finish (offline mode, no
  network), keeping everything in English, laid out left to right,
  meanwhile. The language panel in the bar does the same. Each pack
  belongs to exactly the version you run, and every update brings the new
  version's packs for the languages you use before it restarts the shell.
- **Update now shows the update as it happens.** In Studio → About, the
  update card fills with the real download (the patch or the full
  download), stays full while the update is applied and checked, then
  turns green with a check before the shell restarts, or red with the
  reason if it didn't install. With Reduce motion on, it changes without
  animation.
- **Switching languages is instant.** A language pack is read in place
  rather than parsed, so switching takes under a millisecond instead of
  about 5 ms, looking up a word is a little faster, and the shell uses
  about 2 MB less memory in another language.
- **Updates say how much smaller (or bigger) the new version is.** Studio
  → About and `mandara update` now show the size change next to the
  download size, for example "The new version is 9.2 MB smaller".
- **Long lists of choices line up neatly.** When a choice in Studio has
  more options than fit on one line (a plugin's language with sixteen
  languages, a hot corner's actions, a desktop widget's shapes, the login
  screen's languages), it no longer leaves one or two stragglers alone on
  a second line. The options now sit in tidy, equal rows of equal-width
  buttons that line up in columns, with a shorter last row centred under
  the others, and in Arabic and other right-to-left languages the first
  option starts on the right. Options that can't be picked right now,
  such as the screen recorder's, stay greyed out with the reason
  underneath. Short choices that fit on one line look exactly as before.
- **The main menu's shortcut tiles have character now.** Every tile on the
  Apps & tools, Look and Session pages wears its own faint ornament in its
  own shade of your theme's accent: the apps and tools get a word in
  Fatimid Kufic calligraphy (انطلق on the launcher, المندرة on Studio,
  لقطة on the screenshot tool…), and the look and session controls get
  Alexandrian geometry, from a mashrabiya screen on Lock and Corniche
  waves on Sleep to the Pharos sweeping its beams on Reload. They're
  engraved into the tile like etched glass, deepen gently under the
  pointer, keep every label at full readable contrast in light and dark
  themes, and follow right-to-left layouts. Studio → Bar → Main menu
  lets you make them yours: turn *Tile patterns* off for plain tiles, pick
  a *Tile surface* (Flat, 3D or see-through Glass) and give the tiles a
  *Tile shadow*.

### Fixed

- **The power menu no longer freezes a second display**: opened on a
  display of another size than your first one (by the right screen edge
  or the bar's Sessions button), the power menu waited for the wrong size,
  never appeared, and kept the keyboard and mouse, so the dock, launcher
  and bar stopped answering until you restarted the compositor. It now
  opens at the size of the display you're on. As a safety net, the
  launcher, power menu, wallpaper picker and password prompts now draw
  within a few frames whatever size they get, and one that still can't
  draw closes itself after two seconds and hands your keyboard and mouse
  back. Thanks for reporting it in #55.

- **Profiles & backup is smooth again**: the page used to re-check every
  setting against a freshly built default configuration each time it was
  drawn, so scrolling and hovering over it stuttered. That check now takes
  a fraction of a millisecond, and every edit you make in Studio is
  recorded for Undo much faster too. Thanks for reporting it in #51.

- **Mandara is on your PATH right after installing**: when `~/.local/bin`
  isn't on your PATH, the installer now offers to add it to your shell
  profiles (bash, zsh and fish) instead of only warning, so `mandara` works
  in the next terminal you open.

- **A mistyped compositor name no longer edits the wrong config**:
  `mandara msg compositor install-include` now stops on an unknown name
  instead of adding the include line to the running compositor's file.
  Closing the installer's prompt also counts as no answer.

- **Installing on a system whose package databases were never synced
  works**: instead of failing with "target not found" for every package,
  the installer notices the missing databases and, after asking, updates
  them the right way for your distribution (on Arch and CachyOS a full
  `sudo pacman -Syu`, never a partial upgrade; on Debian and Ubuntu
  `apt-get update`), then installs. Say no and it tells you what to run.
  Thanks for reporting it in #45.

### Under the hood

- **A leaner build.** Mandara no longer builds image formats it never
  opens (EXR, HDR, DDS, QOI, TGA, farbfeld, and an AVIF encoder nothing
  used), an unused date library and a second copy of a small folder
  helper: 18 fewer crates go into the shell. The `mandara` binary is
  3.1 MiB smaller (160.8 → 157.7 MiB), the release download 1.3 MiB
  smaller (66.2 → 65.0 MiB), and a clean release build took 1,037 s
  instead of 1,084 s on the same machine. Every image Mandara shows
  (PNG, JPEG, WebP, GIF, BMP, SVG, and TIFF, ICO and PNM previews) still
  opens.

## 0.13.0 — 2026-11-03

Mandara Studio wears the phoenix.

### Improved

- **Mandara Studio wears the phoenix.** Studio's app icon in your
  launcher, dock and app menus is now the new phoenix logo instead of the
  old shoreline mark. Installs that update themselves pick it up the next
  time the shell starts.

## 0.12.0 — 2026-11-03

A much better screen recorder.

### New

- **A much better screen recorder.** Recording now uses GPU Screen
  Recorder or wf-recorder when you have one installed (falling back to
  Mandara's own encoder), so you get MP4 and MKV files, H.264, HEVC or
  AV1, up to 120 fps and a resolution and quality of your choice, and
  sound too: turn on system sound, your microphone or both in Studio →
  Plugins → Screenshot (Mandara's own encoder records video only). Record
  the whole screen, a display you pick, an area or the focused window, with
  an optional countdown. While recording, the bar shows the time: click it
  to stop, right-click it to pause, resume or mute your microphone. The
  screenshot panel lists your recent recordings to play, show in the folder
  or move to the Trash. Studio → Plugins → Screenshot shows which recorders
  are installed and greys out what the one in use can't do, with the
  reason. Every action is a command you can put on a key, such as
  `record toggle` and `record pause-toggle`.

## 0.11.0 — 2026-11-03

A Modern style for the Power panel. The battery breathes while it charges.
No more CPU driver names in the Power panel.

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

### Improved

- **No more CPU driver names in the Power panel.** The Power panel keeps
  its look, but no longer shows the CPU driver (`intel_pstate` and
  friends) in its header or under each power profile; each profile keeps
  its short description.

## 0.10.2 — 2026-11-03

Notifications joined to the bar, on more setups. No notification is cut
off on a short screen. Studio shows what's saved, even when it can't
apply. Better translations.

### Fixed

- **Notifications joined to the bar, on more setups.** With a bottom dock
  on one display only, bottom notifications on another display now join
  the bar there instead of staying floating cards (Studio no longer locks
  *Join the bar* for them). Joined notifications also open on the display
  whose bar they're shaped for, rather than sometimes taking one
  display's shape on another. And on a narrow screen under a wide notch,
  notifications stay as floating cards instead of squeezing into a
  sliver.
- **No notification is cut off on a short screen.** When a screen is too
  short for as many notifications as you asked to see at once, Mandara
  shows the ones that fit and the rest wait their turn; before, the last
  ones could hang off the edge and time out unseen.
- **Studio shows what's saved, even when it can't apply.** A switch that
  doesn't work with your current setup (like *Join the bar* under a
  floating bar) used to show as off although it was still on. It now
  shows its saved state, dimmed, and you can still turn it off.
- **Better translations.** In the bar designer, Russian *Move up* and
  *Move down* now read as actions, and the presets' description is a
  full sentence in Hindi, Urdu and Turkish. The Bengali note about the
  dock and notifications reads more naturally.

### Under the hood

- **Small wallpaper and font tidy-ups.** A tiled wallpaper in grayscale or
  16-bit colour is converted once instead of once per tile, scaling a
  whole picture in those formats skips a needless copy, and the Arabic
  font chain accepts exactly the fonts the rest of the text system does.
  The changelog check now also catches section headings Studio can't show.

## 0.10.1 — 2026-11-03

Smart updates work after skipping releases.

### Fixed

- **Smart updates work after skipping releases.** A release only carried a
  binary patch from the release right before it, so if you skipped one,
  updating fell back to the full ~70 MB download. Each release now carries
  patches from the five releases before it: updating from any of them is a
  ~5 MB download.

## 0.10.0 — 2026-11-03

A tidier layout designer, with search.

### Improved

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

### Improved

- **Lighter wallpapers.** A wallpaper smaller than your screen, and the
  depth clock's mask, are now scaled up a row at a time instead of through
  a full-size working copy. On a 4K display, switching to a 2880 × 1620
  wallpaper peaks at 125 MiB instead of 202 MiB, and the start-up peak
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

### Improved

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

### Improved

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

### Improved

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

### Improved

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
