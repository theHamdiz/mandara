# Commands

Everything Mandara's interface can do is also a documented command, for
scripts, keybinds and AI agents. The catalog is generated from the running
code, so it is always complete:

```bash
mandara commands            # every command, its arguments and examples
mandara commands --json     # the same, machine-readable
mandara msg <command> [args]          # run one against the running shell
mandara msg --json <command> [args]   # reply as JSON ({"ok": …, "data": …})
```

The lists below are a guided tour, grouped by area. Words after `#` are the
command's siblings and arguments.

## Updating Mandara

```bash
mandara update                   # check: "Mandara 0.2.0 is available … smart update: 7.2 MB instead of 42 MB", then what's in it
mandara update apply             # binary patch first, full download as the fallback; restarts the shell
mandara update apply --full      # skip the patch
mandara update apply --dry-run   # what would be downloaded
mandara update rollback          # put the previous binary (mandara.old) back
mandara update check --json      # machine-readable
mandara update plugins           # marketplace plugins with a newer release
mandara update plugins apply [ID] [--full] [--dry-run] [--force]   # --force: a release that fails the compliance check
mandara update plugins rollback ID
```

Studio → About → Update now does the same with one button. Either way, only
releases signed by Mandara's release key are installed: a release whose
`SHA256SUMS.sig` is missing or wrong is refused.

## What changed

Every release comes with notes in plain words (`CHANGELOG.md`, compiled into
the binary). Studio → About → What's new shows the same.

```bash
mandara changelog                   # this version's notes
mandara changelog 0.1.1             # one release
mandara changelog --since 0.1.0     # everything newer than 0.1.0, up to this version
mandara changelog --all             # every release, newest first
mandara changelog --markdown        # as written in CHANGELOG.md
mandara changelog --json            # machine-readable
```

Only installs Mandara made itself are replaced (`mandara install`'s
`~/.local/bin/mandara` or a `--prefix` you can write to). A package-managed
binary reports "Installed with pacman — update through your package
manager"; a cargo build (`target/…/mandara`) needs `--force-dev`. Offline
mode blocks every update request.

## Shell, panels and look

```bash
mandara msg launcher toggle
mandara msg shell state                     # what is open (panel, sidebar, launcher, menus) and whether the launcher opens on hover
mandara msg hotcorners status               # each corner's action and state per display, recent fires; hotcorners trigger top-left
mandara msg panel open media
mandara msg panel capture media             # copy a visible docked or floating panel as PNG
mandara msg theme set mandara-corniche      # theme list, current, next, mode toggle, wallpaper, seeds, export
mandara msg theme mode toggle
mandara msg look apply corniche-dusk        # look list; look restore brings back your own setup
mandara msg bar position left               # top|bottom|left|right (left/right = the side bar)
mandara msg bar layout hairline             # edge-frame|glass-pills|island|hairline|micro|notch|next
mandara msg bar style expressive            # expressive|tile-dock|rail|segmented: a style preset's look (your items stay); none puts back the plain bar
mandara msg bar hide                        # put the bar away and free its space until bar show (bar toggle flips it; not saved)
mandara msg side-bars on                    # the left and right side bars of plugin chips: on|off|toggle; side-bars status
mandara msg side-bars join off              # one frame with the top bar, or floating apart: on|off|toggle
mandara msg side-bar pin right off          # pinned (reserves space) or sliding away until the pointer touches the edge
mandara msg side-bar show left off          # show or hide one side: on|off|toggle
mandara msg side-bar style right tile-dock  # one side bar's own item look: expressive|tile-dock|rail|segmented|none
mandara msg desktop edit                    # move, resize, add and remove desktop widgets on the desktop (off|toggle)
mandara msg desktop widgets hide            # show|hide|toggle every desktop widget at once
mandara msg desktop widgets press todo-1 todo:3   # what a click on a widget does: todo:<id>, toggle:<wifi|dnd|…>, pomodoro, media:<play-pause|next|previous>, level:volume:0.4, step:brightness:1, copy:<text>, open:<path|url>, launch:<app>, next, refresh, panel:<id>, search, studio:<page>, read:<jannah|quran|hadith>:<ref>
mandara msg desktop widgets press todo-1 todo-edit:3  # edit task 3 at this visible widget; todo-add starts a new task
mandara msg desktop widgets press note-1 note    # edit the note directly on its visible desktop widget
mandara msg diagnostics frames              # frame build times, plus what the desktop widgets cost (desktop_ms, desktop_live_ms)
mandara msg desktop menu DP-1 800 500       # the desktop's right-click menu at a point (logical px)
mandara msg wallpaper picker toggle         # wallpaper set|next|random|list|current
mandara msg wallpaper slideshow on          # off|toggle|<interval: 45, 30m, 2h, 1d>
mandara msg wallpaper slideshow status      # settings, wallpapers in rotation, next_in_seconds, paused (fullscreen|battery|locked|hours)
mandara msg wallpaper slideshow next        # previous; interval 2h; order shuffle|random|alphabetical|newest
mandara msg wallpaper slideshow source folder ~/Pictures/Favourites   # source wallpapers|folder <dir>|playlist
mandara msg wallpaper slideshow playlist add ~/Pictures/Wallpapers/dunes.jpg   # playlist list|add|remove|clear
mandara msg wallpaper slideshow folder dark ~/Pictures/Night   # a folder for the light or dark theme (clear to remove)
mandara msg wallpaper slideshow hours 08:00-22:00   # only change between two times (off: always)
mandara msg wallpaper slideshow transition grow 1500   # desktop|fade|wipe|grow|none [ms]
mandara msg wallpaper slideshow option per-display on   # on-start|pause-fullscreen|pause-battery|pause-locked|subfolders|per-display on|off|toggle
mandara msg wallpaper output DP-1 ~/Pictures/side.jpg   # per-display wallpaper (clear to remove)
mandara msg wallpaper set ~/Pictures/Wallpapers/tide.webm  # AV1 video wallpapers (.webm, .mkv, .ivf) loop on the desktop
mandara msg wallpaper render-preview /tmp/desktop.png 3840x2160@1.5   # the desktop canvas offscreen (same painters as the live one)
mandara msg depth status                   # depth effect: mode (hand-tuned | model), current mask, model install, last model run
mandara msg depth download                 # the depth model (27 MB, checksum-verified) for the model mode
mandara msg depth regenerate               # find the current wallpaper's subject again with the chosen mode
mandara msg depth place-clock              # move the clock to where the subject rises into it
mandara msg language set ar                 # language list
mandara msg plugins reload                  # plugins list|enable|disable|remove|restore <id>
mandara msg plugins remove prayer-times     # built-ins: off + out of bar/sidebar until `plugins restore` (core plugins refuse)
mandara msg plugins enable wallhaven        # opt-in built-ins: wallhaven, cursors, dock, notes, todo, pomodoro, … ([plugins](plugins.md): "ship switched off")
mandara msg config set /integrations/gtk4 true   # theme an app (all off by default); false takes Mandara's changes back
mandara msg plugins vet ./my-plugin          # the marketplace's compliance check: {"ok":true} or {"ok":false,"class":"broken|malicious|placeholder","reason":…}
mandara msg config get /bar/height         # config set /bar/height 30, show, reload, export, import, reset
mandara msg studio displays                 # open Studio on a page (docs/studio.md)
mandara msg studio bars/sidebar             # Bars → Sidebar tab and widget editor
mandara msg studio bars/side-bars           # Bars → Side bars layout and settings
```

The Bars page also accepts `bars/bar`, `bars/panels` and `bars/edges`.
`mandara studio bars --tab sidebar` opens the same tab from the CLI.
The former `bar` and `sidebar` Studio targets remain aliases. Config
commands still use their original paths, such as `/bar/height` and
`/sidebar/width`; `config reset bar` continues to reset only `[bar]`.

`shell state` reports `desktop_editor_ready`: `null` when no desktop text
editor is open, `false` while it awaits keyboard focus, and `true` when it can
receive keys. Automation should wait for `true` before typing into an editor.

## Sidebar tabs

Tabs are named by their name (one word), id or 1-based position; the
built-in Notifications tab is `notifications`.

```bash
mandara msg sidebar tab list                        # tabs, their widgets, notifications_tab, always_first_tab
mandara msg sidebar tab add Focus --icon timer      # rest of the line is the name
mandara msg sidebar tab rename 2 Deep work
mandara msg sidebar tab icon Focus leaf             # a Mandara or icon-theme icon name
mandara msg sidebar tab move Focus 1                # 1 = first
mandara msg sidebar tab remove Focus                # the last tab stays
mandara msg sidebar tab widget add Focus pomodoro 1 # any plugin with a sidebar card; turns it on
mandara msg sidebar tab widget remove Overview weather
mandara msg sidebar tab widget move media Overview Focus 1   # <plugin> <from> <to> [position]
mandara msg sidebar tab open notifications          # opens the sidebar on that tab
```

## To-do lists and notes

Every task belongs to a list; the Inbox (id 0) can be renamed but not
deleted. A leading `#List` files a task there and **creates the list when
no list matches** (matching ignores case, spaces and dashes, so `#side-project`
finds "Side project"). The launcher's `/todo #Work send the report` and
`/note #Recipes koshary` use the same rule.

```bash
mandara msg todo list [Work]                 # tasks (every list, or one) with list ids and names
mandara msg todo add #Work send the report   # without #List: the shown list, else the default list
mandara msg todo done 12                     # undo, remove, clear-done [list]
mandara msg todo move 12 Inbox
mandara msg todo lists                       # lists with open / done counts and what the card shows
mandara msg todo lists add Side project --color hue:160 --icon folder
mandara msg todo lists rename 0 Later        # color <list> <tone|hue:deg>, move <list> <position>
mandara msg todo lists remove Work           # tasks move to the Inbox; `… Work delete` deletes them
mandara msg todo lists show Work             # the card and panel show one list (or `all`)

mandara msg notes list [Recipes]             # notes with topic, file path and edit time
mandara msg notes add #Recipes Koshary for six   # Markdown; without #Topic: the shown topic, else the default
mandara msg notes show 7                     # full Markdown text; append <id> <line>, delete <id>
mandara msg notes search koshary             # fuzzy over every title and text
mandara msg notes move 7 Work
mandara msg notes topics                     # topics with folders and counts, the notes root
mandara msg notes topics add Recipes --color warning   # rename (renames the folder), color, move, show
mandara msg notes topics remove Old [delete] # notes move to the default topic unless `delete`
mandara msg notes export Recipes ~/Documents/recipes   # or `all`; writes .md files
```

Task and notes commands wait for their disk writes before reporting success.
Notes operations run in order, so an append sees earlier pending changes and
a failed save is reported to the caller. Rapid task changes share the latest
JSON save; a failed write can be retried without dropping the in-memory edits.

## Launcher, dock, windows

```bash
mandara msg launcher pin firefox     # unpin|hide|unhide|rename|icon <app>; launcher usage list|reset; launcher emoji <words>
mandara msg launcher placement top-left   # center|top-*|bottom-* (left/center/right); no argument reports it; re-places an open launcher live
mandara msg launcher look grid       # classic|grid|beacon: a ready-made look (no argument reports it); launcher looks lists them (JSON)
mandara msg dock on                  # off|show (reveals a hidden dock for 3 s); dock status (works while off)
mandara msg dock pin firefox         # unpin|toggle-pin <app> (desktop id or name; pin also fuzzy-matches)
mandara msg dock move firefox 0      # reorder a pinned app (0 = first)
mandara msg dock add                 # the searchable app picker over the dock
mandara msg dock stack add ~/Documents  # stack list|remove <folder|index>
mandara msg dock stack open Downloads   # open a stack's fan / grid by name, path or index; dock stack close
mandara msg dock applications open     # the Applications folder's app grid; close|toggle
mandara msg dock trash               # item count; dock trash empty --yes (deletes for good)
mandara msg switcher next            # prev|open|commit|cancel|demo|status|bind (Alt+Tab)
mandara msg window list              # window focus|close|minimize|maximize|fullscreen <id>, window active
mandara msg screentime today         # week, app <id>, reset today|all, track on|off
mandara msg clipboard pin <id>       # unpin, list, copy, delete, clear [all], record on|off, edit <id>
mandara msg clipboard kinds          # entries per kind; clipboard kind <text|link|markdown|code|css|color|image|files> [words]
mandara msg clipboard search rust    # entries matching every word (text, kind, language, site, app, collection)
mandara msg clipboard collections    # your collections with counts; collection show <name|id> [words]
mandara msg clipboard collection create Palette   # collection rename <name|id> <new name>, delete <name|id>
mandara msg clipboard collection add Palette <id> # collection remove <name|id> <id>
mandara msg clipboard view color     # the panel's filter: all, pinned, a kind or a collection, plus optional search words
```

In the clipboard panel, **Tab** moves from search to the filtered results.
Use **↑/↓** to select and **Enter** to copy or paste according to the
panel’s click action. Search editing and collection-name entry keep their
own keyboard input.

## Notifications, OSD, sound and media

```bash
mandara msg notifications dnd on 45m  # status, list, send [--low|--critical], reply <id> <text>, rules list|add|remove
mandara msg osd volume                # osd show <title> [--icon x] [--level 0-100] [--on|--off] [text], osd kinds
mandara msg sounds preview screenshot # play <event>, status, themes, enable on|off
mandara msg audio output next         # audio devices, volume up|down|set, mute, mic mute, streams, stream volume
mandara msg audio mute-all toggle     # on|off|toggle|status: every output and app; off restores your own mutes
mandara msg privacy status            # who uses the microphone, camera or screen
mandara msg tray menu "Input Method"  # tray status|activate|secondary|scroll
mandara msg soundscapes preset corniche-night   # play|pause|toggle|status, volume <layer|master> <0-100>, sleep <min|off>
mandara msg radio play fip-jazz                 # play [station], pause|toggle|stop|next|previous|status, volume <0-100>, mute, sleep <min|off>
mandara msg radio stations jazz                 # list (a shelf); favourite <station>, add <url> [name], remove <station>
```

## Displays and night light

```bash
mandara msg display list               # set <output> mode|scale|transform|position|vrr|enabled <value>, confirm, revert
mandara msg display profile save Desk  # profile list|apply|delete <name>; persist / unpersist to the compositor drop-in
mandara msg display brightness all 60  # DDC/CI external monitors; display brightness status
mandara msg brightness up 10           # down, set, status (laptop backlight through logind)
mandara msg nightlight toggle          # on|off|auto, status, temperature <K>, schedule sun|fixed|always, preview <K> [s]
```

`nightlight status` lists the screens it warms (`outputs`), the ones that
refused (`held_by_another_app`), the night-light apps it found running
(`held_by`) and, if its connection dropped, `error`.

## Power, idle and session

```bash
mandara msg power profile balanced   # power status, cycle, battery, history, charge-limit on|off
mandara msg power alerts             # power alert-test low|critical
mandara msg idle status              # idle enable on|off, keep-awake [minutes|0], fade-preview
mandara msg session entries          # boot-entries, reboot-to <entry|firmware> confirm, do <action> [confirm], menu
mandara msg hooks list               # hooks status, test <event>
mandara msg lock                     # lock now (--demo previews, --stop-demo closes it)
mandara msg polkit status            # is the polkit agent registered? (polkit demo previews the dialog)
mandara msg keyring-prompt status    # does Mandara answer gnome-keyring's unlock prompts? (close cancels one)
mandara msg keyring-prompt demo      # preview the unlock dialog: demo unlock|create|confirm (no D-Bus; "wrong" is rejected once)
```

## Keyrings and passwords

```bash
mandara msg keepassxc unlock             # Mandara's KeePassXC unlock dialog (picks the last active / a locked database)
mandara msg keepassxc unlock Passwords   # …for one database: its name, file name or path
mandara msg keepassxc lock               # lock every open KeePassXC database
mandara msg keepassxc status             # installed / running / databases and whether they're locked (JSON)
mandara greeter keyring-unlock           # GNOME Keyring + KWallet at login: status and plan (sudo … --apply / --remove)
```

The KeePassXC dialog (also in quick settings and the launcher when KeePassXC
is installed) lists the databases KeePassXC remembers (`keepassxc.ini`:
`LastOpenedDatabases`, `LastDatabases`, the remembered key files), takes the
password and an optional key file (portal file chooser), and calls KeePassXC's
`org.keepassxc.KeePassXC.MainWindow.openDatabase` on the session bus. KeePassXC
reports no failure there, so Mandara re-checks: with KeePassXC's Secret Service
integration on it reads its collections' `Locked` property and says when the
database is still locked; without it the result can't be confirmed (KeePassXC
shows its own unlock screen if it failed). Databases opened with a hardware key
(YubiKey challenge-response) can't be unlocked this way: the dialog offers
KeePassXC's own window. **Security:** the password travels to KeePassXC in
clear text on the session bus — only programs running as you can read it, but
unlike gnome-keyring's prompts it isn't encrypted. Mandara wipes its own copy
right after the call. Studio → General → Keyrings & passwords turns the dialog
off (`general.keepassxc_prompt`) or has it ask once after login when KeePassXC
runs with a locked database (`general.keepassxc_ask_at_login`).

## Login screen

These run directly (not through `msg`); see [login-manager.md](login-manager.md).

```bash
mandara greeter list                       # the active login screen and every choice (Mandara Login is the default)
mandara greeter use tuigreet --dry-run     # use <name> [--apply] [--install] [--compositor cage|niri|hyprland]; needs sudo to switch; --install first adds what it lacks (greeter, greetd, cage) with your package manager
sudo mandara greeter restore latest        # undo a switch from its backup (--apply: the display manager too)
sudo mandara greeter keyring-unlock --apply  # unlock GNOME Keyring / KWallet at a password login (--remove undoes it)
mandara greeter --demo                     # preview Mandara Login; install, apply-config, uninstall
```

## Network and Bluetooth

```bash
mandara msg network vpn up Office    # vpn list|down|import <file> [name]|remove; hotspot start|stop|status|share
mandara msg network saved list       # saved forget|autoconnect|priority
mandara msg network ipv4 set Home manual 192.168.1.20/24 192.168.1.1 1.1.1.1   # ipv4 show <name>; network devices
mandara msg network wifi scan        # wifi list|connect|disconnect|power|hidden <ssid>
mandara msg bluetooth pair "WH-1000XM5"   # trust on|off, rename, forget, adapter hci1, auto-connect on, pairing-demo confirm
mandara msg net-speed status         # addresses, public-ip, test [MB]
```

## System tools

```bash
mandara msg processes kill 4242      # processes list
mandara msg drives list              # mount|unmount|eject|unlock|lock|open <device>
mandara msg containers list          # start|stop|restart|pause|unpause|logs <name>, stats
mandara msg services failed          # list|status|logs|start|stop|restart|enable|disable <unit> [--user]
mandara msg updates check            # list, sources, news, run [source]
mandara msg uptime status            # check: run every check now
mandara msg tailscale status         # up, down, exit-node <peer|none>
mandara msg crashes list             # info <pid>, seen
```

## Developer tools and progress

Scripts, builds and AI agents can show their progress in the bar with the
Task progress plugin:

```bash
mandara msg progress start build Building the release
mandara msg progress set build 40%           # or 3/10; an optional new label after it
mandara msg progress done build "All green"  # fail build "3 tests failed"; remove <id>; list; clear
```

```bash
mandara msg ports list               # kill <port>
mandara msg git list                 # fetch <repo>, refresh
mandara msg ci status                # refresh
mandara msg ai loaded                # models, load <model>, unload <model|all>
```

## Everyday helpers

```bash
mandara msg keybinds toggle          # show, hide, list (every shortcut as JSON)
mandara msg breaks status            # now [short|long], skip, snooze [minutes], pause [minutes], resume
mandara msg presentation toggle      # on, off, status
mandara msg share clipboard          # text <text>, file <path>, screenshot, stop, status
mandara msg jannah today             # the reminder on screen; next (a fresh one), prev (the one before), peek [n], show <ref>, search <query>, paths, step [done|peek [days]], saved, language [code|follow]
mandara msg quran open                 # open the panel without a verse reference
mandara msg jannah open                # open the panel without a hadith reference
mandara msg quran read 2:255           # surahs, today, peek [days], search <q>, play <ref|surah>, pause, resume, stop, bookmark add|remove <ref>, download, status, remove, language [code|follow]
mandara msg hadith show bukhari 1      # collections, download|remove <collection|all>, today, peek [n], search <q> [--collection c] [--grade g], bookmarks, language [code|follow]
mandara msg quran copy-image 2:255     # full ayah design; meaning follows Study mode
mandara msg hadith copy-image bukhari 1 # full hadith design; shown chain/meaning follow settings
mandara msg jannah copy-image 89:30    # full reminder design with reference
mandara msg prayer peek 1              # today, next, peek [days], language [code|follow], adhan [stop|play|download|remove]
```

## Compositor and Config doctor

```bash
mandara msg compositor status        # detected compositor, drop-in, include line
mandara msg compositor sync          # regenerate the drop-in (checked, rolled back if the compositor rejects it) and reload
mandara msg compositor install-include [mango|niri|hyprland|umbriel|sway]   # river: prints the line to add to init
mandara msg --json compositor check  # Config doctor: the compositor's own checker on your config; each problem with id, file, line, kind, owner, explanation and fixes
mandara msg compositor fix 1a2b3c4d comment   # comment | remove (your files) | regenerate (Mandara's drop-in); backed up, re-checked, rolled back if worse
mandara msg compositor snooze        # no reminders about the current problems until the config or the compositor changes
mandara msg compositor reload        # clients, monitors, dispatch <command> [args]
```

Without a running shell (a session that won't start, a TTY), the same check
and fixes work from the terminal:

```bash
mandara setup --check                       # the Config doctor's report and conflicts, nothing changed; exit 10 = conflicts, 11 = errors
mandara setup --fix 1a2b3c4d=comment        # ID=comment|remove|regenerate, repeatable
mandara setup --restore ~/.local/share/mandara/doctor-backups/<backup>   # undo a fix
```

## Capture

```bash
mandara msg screenshot region
mandara msg screenshot region --delay 3   # count down first (Esc cancels); --delay alone uses the Countdown setting
mandara msg screenshot screen --delay=5   # window / screen capture directly after the countdown
mandara msg screenshot all                # every display in one image, in monitor-layout order
mandara msg record start                  # the default source (Studio → Plugins → Screenshot); the countdown setting applies
mandara msg record start window           # start screen|region|window; record screen DP-2 picks a display
mandara msg record toggle                 # stop a recording (or a countdown), else start one: bind it to a key
mandara msg record pause-toggle           # pause | resume | pause-toggle (GPU Screen Recorder and the built-in encoder)
mandara msg record mic off                # mute the recorded microphone (on | off | toggle); unmuted again at the end
mandara msg record stop                   # replies once the file is saved
mandara msg --json record status          # recording, paused, countdown, elapsed, path, backend, settings in use
mandara msg --json record backends        # installed recorders and what each supports
mandara msg --json record recent          # the last recordings: path, length, size
mandara msg ocr ~/Pictures/shot.png arabic
mandara msg color-picker pick        # replies with the picked color; show|select <#hex>, history, clear
```

## Wallpaper generator

The `wallpaper-generator` plugin (opt-in: `plugins enable wallpaper-generator`).
Replies are JSON with paths, sizes in bytes and times in seconds. A
`generate` that takes longer than about 12 s answers `{"status":
"running", "id": …}` and keeps going; `wallgen status` has the result.

```bash
mandara msg wallgen generate                 # the current recipe at the configured size and format
mandara msg wallgen generate random --apply  # random|variation|reseed|recipe|<style>; --apply / --no-apply
mandara msg wallgen generate aurora --size 8k --format png --palette nord --seed 0x2a --light   # lossless (default: the smaller of WebP and PNG)|webp|png|jpeg
mandara msg wallgen generate --size 3440x1440 --palette '#101820,#2aa3a8,#e9c98b'   # WxH or display|1080p|1440p|4k|5k|8k|uw1440|portrait4k; theme = live colors
mandara msg wallgen generate --video --seconds 8 --fps 24 --detach   # a seamless AV1 loop; --detach replies at once with the job id
mandara msg wallgen status                   # running job (progress, seconds), the last result, the schedule and its next run
mandara msg wallgen cancel
mandara msg wallgen list 20                  # generated files in the output folder, newest first (style, seed, palette, bytes)
mandara msg wallgen remix ~/Pictures/Wallpapers/Generated/aurora-1a2b3c-3840x2160.webp   # its recipe becomes the current one
mandara msg wallgen recipe show              # recipe set style=dunes palette=desert vignette=40 | set '<TOML recipe>', random [style], variation [0–1]
mandara msg wallgen styles                   # every style with its parameters (key, range, step, default, unit); wallgen palettes
mandara msg wallgen schedule on --every 6h --mode variation --keep 30   # off, status; --video|--still, --apply|--no-apply
```

## Apps, profiles, time and place

```bash
mandara msg apps default set browser firefox   # apps defaults, autostart list|enable|disable|add|remove, preferred
mandara msg apps installed           # every app with its source (pacman, apt, dnf, flatpak, snap, appimage, nix, mandara, local…)
mandara msg apps installed --by-mandara  # what the app installer installed, with every file it wrote (registry.toml)
mandara msg apps install ~/Downloads/Obsidian-1.6.7.AppImage  # [--yes] skip the on-screen card, [--no-path-shim], [--dry-run] print the plan
mandara msg apps jobs                # installer jobs: phase (inspect/confirm/install/integrate/done/failed/cancelled), progress 0..1, outcome
mandara msg apps jobs cancel 3       # stop a job (rootless installs roll back)
mandara msg apps owner firefox       # who installed it, the exact removal command, what else goes, leftovers (changes nothing)
mandara msg apps uninstall org.gimp.GIMP --dry-run  # [--purge-data] leftovers to the Trash, [--confirm-default] for a default app
mandara msg profile save look Night  # profile list|apply|delete; config export <file> [changes] [redact], import, reset <section>
mandara msg profile save bar Desk    # only bar and side-bar settings; keep the theme and desktop widgets
mandara msg cursor list              # the cursor catalog: variants, licence, stars, installed / by Mandara / active, other installed themes
mandara msg cursor install bibata/ice   # download the pinned GitHub release, verify SHA-256, install to ~/.local/share/icons
mandara msg cursor apply bibata/ice 32  # theme (catalog id[/variant] or XCursor name) and optional size, live; `none` = your compositor's
mandara msg cursor remove bibata     # every variant Mandara installed (or one: bibata/ice); never a package's or your own
mandara msg calendar add webcal://example.com/holidays.ics Holidays  # calendar [days], sources, remove <n>, refresh
mandara msg calendar open            # open the shell calendar without a calendar source
mandara msg calendar goto 2026-12     # open the clock panel's calendar there: today, next, prev, +N / -N days, YYYY-MM, YYYY-MM-DD
mandara msg calendar day 2026-12-25   # one day as JSON: its events, ISO week, day of the year and Hijri date
mandara msg location locate          # geoclue|ip; location search <city>, geocode, set <lat> <lon> [name]
mandara msg time                     # time zone, network time, and your location's time zone (suggested_timezone when they differ)
mandara msg time zone Africa/Cairo   # set the system time zone (asks for your password); time zones [word] lists them
mandara msg time auto on             # set the clock from internet time servers (on / off / toggle; asks for your password)
mandara msg world-clock add Mecca    # list, remove, search <query>, at <HH:MM>
mandara msg authenticator code github  # list, copy, add <otpauth://…>, remove <id>, scan, demo on|off
```

The `cursor` commands belong to the opt-in **Cursor themes** plugin
(`mandara msg plugins enable cursors`); while it is off they answer that the
plugin is off and change nothing, and Mandara writes no cursor setting at
all. `cursor install` fetches only from the theme's own GitHub repository, keeps
the file only when its size and SHA-256 match the catalog, unpacks it with
pure-Rust readers that refuse paths leaving the folder, and installs just the
theme folder; it never runs anything from the archive. `cursor apply` checks
the theme is installed, then writes the compositor drop-in and reloads it
(Hyprland also gets `setcursor`) and points `~/.icons/default` at it when
that file is Mandara's. Removing the active theme returns to your
compositor's cursor.

`apps uninstall` uses the app's own package manager, detected at runtime:
system packages run `pkexec <manager> …` (polkit asks for your password;
protected packages such as the shell, compositor and portals are refused),
Flatpak / Nix user installs need no password, AppImages and local entries
move to the Trash. Leftovers in your home folder are only touched with
`--purge-data`, and only ever moved to the Trash. `MANDARA_UNINSTALL_DRY_RUN=1`
in the shell's environment forces a dry run.

`apps install` (the **App installer** plugin) detects each file's format by
its magic bytes and never runs it: **AppImage** (read through its embedded
squashfs) and **portable archives** (`.tar.gz`/`.tar.xz`/`.tar.zst`/`.zip`,
unpacked with path-traversal, link and size checks) install rootless into
`~/.local/share/mandara/apps/<id>/` with a desktop entry
(`~/.local/share/applications/mandara-<id>.desktop`), an icon in
`~/.local/share/icons/hicolor` and an optional `~/.local/bin/<name>` command
(never replacing an existing file); **Flatpak** bundles / `.flatpakref` run
`flatpak install --user --noninteractive`; **native packages** (`.deb`,
`.rpm`, `.pkg.tar.zst`, `.apk`, `.xbps`) go only through the system's own
package manager via `pkexec`, after its simulation, and a package for another
distribution is refused with an explanation (no conversion, no containers).
Without `--yes` the confirmation card appears on screen and the command
returns the queued job ids at once (follow them with `apps jobs`). A
duplicate (same app id) offers Update / Keep both. Relative paths are made
absolute by `mandara msg`. `MANDARA_INSTALL_DRY_RUN=1` in the shell's
environment forces dry runs.

## Connected plugins

Enable these in Studio → Plugins and configure each plugin’s Settings first.
Requests wait for service work to finish, then return one result or error.
The UI stays responsive while the request runs.

```bash
mandara msg translate languages
mandara msg translate text ar Good morning
mandara msg feeds list
mandara msg feeds subscribe blog.rust-lang.org News
mandara msg feeds discover example.org
mandara msg feeds refresh
mandara msg feeds refresh https://example.org/feed.xml
mandara msg feeds items https://example.org/feed.xml
mandara msg feeds read https://example.org/feed.xml https://example.org/post
mandara msg feeds save https://example.org/feed.xml https://example.org/post
mandara msg feeds read-all
mandara msg feeds import ~/Downloads/subscriptions.opml
mandara msg feeds export ~/feeds.opml
mandara msg reddit open r/linux
mandara msg reddit posts linux top week
mandara msg reddit thread https://www.reddit.com/r/linux/comments/abc123/
mandara msg reddit subscribe r/rust
mandara msg reddit status
mandara msg phone devices
mandara msg phone action DEVICE_ID ring
mandara msg phone send DEVICE_ID Hello from my desktop
mandara msg home states
mandara msg home action light.desk toggle
mandara msg home action scene.evening activate
```

The translation provider and home server keys are saved through Studio in
your desktop keyring. [Setup and behaviour](integrations.md).

## Local command agent

`agent status` reports active inference requests and installed model artifacts.
After installing the models, `agent prepare` builds the semantic command index.
`agent preview <request>` returns retrieved candidates and a compiled JSON
preview without running its actions. `agent purge` cancels the shell's active
requests. See [Local shell commands](command-agent.md).
