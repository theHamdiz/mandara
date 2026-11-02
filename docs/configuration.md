# Configuration

Everything you can see or feel in Mandara is a setting: sizes, spacing,
corner radii, opacity, colours, fonts, animation springs, delays and
timeouts, refresh intervals, positions, alert thresholds, list lengths and
behaviour switches. Each one has a Studio control (with a sensible default,
a bounded range and a reset), takes effect live, and is stored in
`~/.config/mandara/config.toml`. Studio's search finds every setting by its
label or help text.

What stays fixed is what isn't a matter of taste: protocol and file-format
values (Wayland, D-Bus, IPC, greetd, PAM, API paths), maths and physics
constants, safety bounds (a slider's own min / max, memory caps, input
validation, network timeouts) and demo timings. Those are named constants
with a comment saying why they are fixed — never bare numbers. A test keeps
it that way (see [Guard](#guard)).

## How a setting is stored

| Kind | Where | Read by |
|---|---|---|
| Shell settings | a table per area: `[bar]`, `[panels]`, `[lock]`, `[notifications]` … | the shell, from the current config snapshot |
| Plugin settings | `[plugins.settings.<plugin-id>]` | that plugin; shown in its settings modal (Studio → Plugins → the plugin → Settings) |

Numbers outside a setting's range are clamped to it, so a hand-edited value
never breaks the layout.

### Defaults and upgrades

A changed default only affects new installs. Two tables record which
defaults they were written for, and an older config is brought forward on
load (in memory; the next write saves it), never losing what it had on:

| Key | Since | What an older config gets |
|---|---|---|
| `[plugins] defaults_version = 1` | 0.2 | Fifteen built-ins became opt-in (containers, services, drives, updates, process hunter, authenticator, soundscapes, world clock, network speed, color picker, Pomodoro, notes, to-do, privacy, app installer) and Wallhaven and cursor themes became the opt-in `wallhaven` and `cursors` plugins. A config without the key keeps each of these on (written into `enabled`) unless it had switched it off or removed it; `wallhaven` is on when the old `wallpaper.wallhaven` was, `cursors` when any `compositor.input.cursor.*` was set. |
| `[integrations] version = 1` | 0.2 | Every app theme (and compositor borders) is off by default. A table still exactly at the 0.1 defaults is recorded as off and the files 0.1 wrote stay as they were; a table you had changed keeps every value. Compositor borders keep their value (they live in Mandara's own drop-in). |

`[plugins] enabled` lists opt-in plugins you switched on, `disabled` the
ones you switched off; a plugin in neither follows its default. Exports
(`config export --changes-only`) keep these version keys, and imports of
older exports are upgraded the same way.

### App theming (`[integrations]`)

`ghostty`, `gtk4` (GTK 3 / 4 and libadwaita), `qt6` (qt5ct / qt6ct),
`kvantum`, `kvantum_apply`, `zed`, `fuzzel`, `btop`, `kitty`, `foot`,
`alacritty`, `wezterm`, `helix`, `neovim`, `firefox`, `vesktop`, `kde` and
`compositor_borders`, all `false` by default. While an app is off Mandara
writes nothing for it outside `~/.config/mandara` and `~/.cache/mandara`.
Switched on, Mandara writes its own theme file (backing up to
`<file>.mandara-backup` anything already at that path that isn't Mandara's)
or adds one include line to the app's config (backing the file up once
first), and records what it did in
`~/.config/mandara/generated/app-themes.toml`. Switched off, it takes back
exactly that: the line or block goes, files it created go if you didn't edit
them, replaced files come back from their backup, and Kvantum's link and
selection are undone. `mandara msg config get <pointer>` and
`mandara msg config set <pointer> <value>` reach the same keys as JSON
pointers (for example `/plugins/settings/resources/refresh_ms`).

## What's configurable

Grouped by Studio page. Keys are config paths (`table.key`).

### Look & feel

| Page | Settings |
|---|---|
| Themes | **looks** — six one-click looks and *Restore my look* (`looks.*`, see [Looks](#looks)); light / dark / automatic and its schedule (`theme.*`), palette tuning (`theme.adjustments`), theme crossfade (`theme.transition_ms`), icons (`appearance.*`), corner roundness, shadows, borders, backdrop dim (`style.*`); the theme; **other apps** — the app theming switches, all off by default (`integrations.*`, see [App theming](#app-theming-integrations)); **cursor** (the opt-in `cursors` plugin) — theme, size, hide while typing / when idle, Hyprland's software cursor and hide on touch (`compositor.input.cursor.theme`, `.size`, `.hide_when_typing`, `.hide_after_secs`, `.no_hardware_cursors`, `.hide_on_touch`; themes Mandara downloaded are listed in `~/.local/share/mandara/cursor-themes.json`) |
| Wallpaper | folder, picker style / size / focus / names, fill and colour, transitions, crossfade (`wallpaper.crossfade_ms`), light / dark and per-display variants, depth effect (`wallpaper.depth_effect`) and how its subject is found: `wallpaper.depth.mode` (`hand-tuned` or `model`), `auto_split`, `split` (0.05–0.95, model mode) (`wallpaper.*`); video wallpapers: pause behind windows / on battery, frame-rate limit, first frame only (`wallpaper.video.*`, see [Video wallpapers](#video-wallpapers)); the slideshow (`wallpaper.slideshow.*`, see [Wallpaper slideshow](#wallpaper-slideshow)) |
| Desktop widgets | clocks (incl. the depth clock, an analog dial and nine more faces), system monitor, gauges, meters and thermals, weather, world clock, countdown, progress, to-do, sticky note, focus timer, quote, notebook tabs, calendars, sun / moon / globe, profile, shortcuts, toggles, sliders, palette, desk cat, bubbles, photo frame, sticker, six media layouts, machine card, screen time, prayer times, search, at a glance, GitHub, exchange rates, AI usage, games, decorations, plane visualizer, per-widget plate shape, outline, shadow, header, frame, rotation and mirror, alignment, font, numerals, calendar and language, show / hide them all (`desktop.widgets.*`, see [Desktop widgets](#desktop-widgets)); desktop cards and visualizers (`cards.*`, `visualizers.*`) |
| Typography | interface / display / Arabic / monospace fonts, text scale, weight (`typography.*`) |
| Motion | stiffness, damping, mass, animation speed, reduce motion (`motion.*`) |

### Desktop

| Page | Settings |
|---|---|
| Bar | style, position, height, gaps, radii, opacity, accent tint, rim width / intensity, shadow, text / icon scale, item padding / spacing, separators, center alignment, floating frame, **hairline / micro pill** (`bar.layout` = `hairline` / `micro`, `bar.compact_height`, `bar.accent_line`), **notch** (`bar.layout` = `notch`, `bar.notch_width` 0.3–1, the share of the edge it spans), group capsules (`bar.capsules`), **item capsules** (`bar.item_capsules`: every item on its own capsule, any style) with their colour (`bar.capsule_fill` = `soft` / `container` / `accent`), strength (`bar.capsule_strength`, 0–2 ×) and hairline outline (`bar.capsule_outline`), **hover highlight** (`bar.hover_highlight`), **bar text weight** (`bar.font_weight`, 0 follows Typography's weight, else 100–900), **gap to windows** (`bar.window_gap`, 0–32 px added to the space the bar reserves), what right-clicking empty bar space opens (`bar.right_click` = `main-menu` / `desktop-menu`), what a **middle click** there does (`bar.middle_click.action`, any hot-corner action, and `bar.middle_click.command`), what **scrolling** there does (`bar.scroll` = `none` / `workspaces` / `volume`), **side bar** (`bar.position` = `left` / `right`, `bar.vertical_width`, `bar.screen_frame`, `bar.frame_thickness`, see [Side bar](#side-bar)), auto-hide mode and delay, click-through, over fullscreen (`bar.*`); drop-down panels: width, maximum height, padding, gap, anchor, opacity, shadow, animation, custom spring, **close delay** (`panels.leave_delay_ms`), **action message time** (`panels.toast_ms`), the main menu's frosted glass (`panels.menu_translucent`, `panels.menu_opacity`) |
| Sidebars & hover | sidebar side, mode, width, tabs (`sidebar.tabs`, `always_first_tab`, `add_tab_button`, `notifications_tab`) and look (`sidebar.*`, `sidebar.look.*`), edge triggers and dwell (`edges.*`), hot corners with delay and size (`hot_corners.*`), rounded screen corners, on by default (`screen_corners.*`) |
| Dock | layout (position, size, magnification), behaviour (auto-hide, delays, slide speed, bounces, minimize into icon), apps (`pinned` order, running, recent apps, indicators, badges), stacks (`[[dock.stacks]]` path / view / sort / display, `stack_items`, `fan_items`, `stack_names`), the Applications folder (`[dock.applications]` enabled / view / order / icon_size; replaces 0.1's `applications_slot`), Trash, the now-playing chip (`dock.media_chip`), appearance (opacity, tint, radius, padding, spacing, shadow, rim) — every number clamped to `mandara_config::dock::ranges` (`dock.*`) |
| Launcher | placement (`launcher.look.position`, see [Launcher placement](#launcher-placement)), align to the launcher button, width / height when attached and in the center, layout, columns, dim, results (`launcher.max_results`), providers and prefixes, ranking half-life and recent count, launching (`launcher.*`) |
| Window switcher | style, previews, scope, thumbnail width, columns (`switcher.*`); screen time retention, idle and limits (`screen_time.*`) |
| Notifications | position, width, scale, opacity, timeouts per urgency, maximum visible, history size and age, do not disturb, quiet hours, rules (`notifications.*`) |
| On-screen displays | kinds, hold time, scale, opacity, offsets (`osd.*`) |

### System

| Page | Settings |
|---|---|
| Displays | per-display shell surfaces, night light (`displays.*`, `night_light.*`) |
| Audio & media | volume step, over-amplification, devices, routes (`audio.*`), players (`media.*`), sounds (`sounds.*`); visualizer and lyrics (Media plugin) |
| Network & Bluetooth | `network.*`, `bluetooth.*` |
| Power & idle | idle stages per power source, lock grace, battery alert levels (`power.low_percent`, `power.critical_percent`, `power.peripheral_low_percent` — also the coral mark of the battery OSD and the Bluetooth device rings), power profiles, session menu, hooks (`idle.*`, `power.*`, `session_menu.*`, `hooks.*`) |
| Date, time & location | formats, the clock panel's calendar (week numbers, `weekend` days, `hijri_dates`, `eastern_numerals`), calendars (refresh, reminders, agenda days), location, weather refresh (`datetime.*`, `calendar.*`, `location.*`, `weather.*`) |
| Applications | default and preferred apps, autostart (`apps.*`) |
| Compositor | the generated drop-in (`compositor.manage`), window look, effects, rules, tags and layouts (`compositor.*`), border colours (`integrations.compositor_borders`); the Config doctor's start-up notification about new problems in your compositor config (`compositor.doctor_notify`, on by default) |

### Security & accounts

| Page | Settings |
|---|---|
| Login screen | the greeter's own designer (look, timings, backgrounds) |
| Lock appearance | clock, background, blur, dim, what it shows, OLED care, **low-battery mark** (`lock.low_battery_percent`) |
| Privacy | indicators, ignored apps, camera poll (`privacy.*`) |
| General → Security | polkit agent, keyring prompter, **password dialog width** (`general.auth_dialog_width`, 360–640 px, both the polkit and keyring-unlock dialogs) |

### Extend — plugin settings

Each plugin's modal also lists the shell settings it drives. New in this
pass are the refresh intervals, list lengths and thresholds that used to be
fixed:

| Plugin | Keys (`plugins.settings.<id>.…`) | Default |
|---|---|---|
| Resources (`resources`) | `refresh_ms` (also the bar rings), `disks_every_s`, `top_apps`, `temps_shown`, `disk_warn_percent`, `disk_full_percent` | 1500 ms, 15 s, 5, 8, 80 %, 90 % |
| Process hunter (`processes`) | `refresh_ms`, `hot_cpu_percent` | 1500 ms, 50 % |
| Containers (`containers`) | `hot_cpu_percent`, `refresh_s` (list refresh while the panel is closed, 5–600 s) (plus engine, stopped containers, log lines) | 80 %, 30 s |
| Updates (`updates`) | `first_check_delay_s` (0–600 s after login), `news_count` (Arch news headlines, 1–5) | 20 s, 1 |
| Media (`media`) | `lyrics_rows` (odd, 3–9), `bar_title` (the track beside the icon in the bar), `hide_when_paused` (the bar item only while something plays) | 5, on, off |
| Workspaces (`workspaces`) | `labels` (`auto`: the number or a name of up to two characters, `number`, `dots`), `app_icons` (the apps' icons on each pill), `hide_empty` (only tags / workspaces with windows, plus the active and urgent ones) | auto, on, off |
| Active window (`active-window`) | `show` (`app-and-title`, `app`, `title`), `max_width` (120–720 px) | app and title, 420 px |
| Audio (`audio`) | `bar_level` (the volume beside the speaker icon) | on |
| Power (`power`) | `bar_percent` (the charge beside the battery icon) | on |
| Screenshot (`screenshot`) | `toolbar_position` (`top`, `bottom`; the toolbar's arrow button or M flips it live), `toolbar_scale` (0.75–1.5 ×, also its hint strip), `countdown` ("Capture in N s" and `--delay` without a number, 1–10 s) | top, 1 ×, 3 s |
| Clipboard (`[clipboard]`) | `pinned_shown` (pinned rows above the history, 0–12), `preview_lines` (lines of code, CSS and Markdown per row, 0–8), `kind_chips` (filter chips by kind), `max_collected` (entries kept in collections) | 4, 3, on, 500 |
| Services (`services`) | `list_refresh_s`, `failed_check_s` (plus default view, alert, journal lines) | 4 s, 60 s |
| Drives (`drives`) | `usage_every_s`, `full_warn_percent` | 10 s, 90 % |
| Network speed (`net-speed`) | `interval_s` | 1 s |
| Network (`network`) | `networks_shown` | 7 |
| Clock (`clock`) | `agenda_rows` | 5 |
| To-do (`todo`) | `panel_rows`, `card_rows`, `default_list` (list name; empty: Inbox), `show_completed`, `sort` (`newest`, `oldest`, `alphabetical`), `bar_count` (empty: all lists, or a list name) | 14, 8, Inbox, on, newest, all |
| Notes (`notes`) | `default_topic` (empty: Notes), `storage_folder` (empty: `~/.local/share/mandara/notes`), `preview_by_default`, `editor_font` (`sans`, `mono`), `editor_size` | Notes, data folder, on, sans, 13 px |
| Bluetooth (`bluetooth`) | `battery_warn_percent` (coral follows Power → peripheral alerts) | 30 % |
| Authenticator (`authenticator`) | `visible_rows` | 6 |
| Wallpaper generator (`wallpaper-generator`) | recipe, size, format, folder, schedule and video loops: see [Wallpaper generator](#wallpaper-generator) | |
| Wallhaven (`wallhaven`, opt-in) | `wallpaper.wallhaven_query` (default search; empty: the toplist), `wallpaper.wallhaven_sorting` (`toplist`, `date_added`, `views`, `favorites`, `random`) — shown on Studio → Wallpaper while the plugin is on; off, no Wallhaven request is made | empty, toplist |
| Cursor themes (`cursors`, opt-in) | `compositor.input.cursor.*` — written to the compositor drop-in, `~/.icons/default` and applied live only while the plugin is on | unset |
| Task progress (`progress`, opt-in) | `notify` (a notification when a task is done or fails) | on |
| Eye breaks (`breaks`, opt-in) | `every_min` (active minutes between short breaks), `short_s`, `long_every_min` (0: no long breaks), `long_min`, `style` (`notification`, `overlay`), `skip_fullscreen` | 20 min, 20 s, 60 min, 5 min, notification, on |
| Presentation mode (`presentation`, opt-in) | `auto_screen_share` (on while an app shares the screen) | off |
| Share to phone (`share`, opt-in) | `minutes` (the link stops after this long, 1–120), `once` (stop after the first download) | 10 min, on |
| Uptime monitor (`uptime`, opt-in) | `checks` (`https://…`, `http://…` or `host:port`, optionally `Name=target`, comma-separated), `interval_s` (10–3600), `notify` | empty, 60 s, on |
| Local AI (`local-ai`, opt-in) | `url` (the Ollama server) | `http://127.0.0.1:11434` |
| Git repositories (`git-repos`, opt-in) | `roots` (folders to look in, comma-separated), `depth` (1–6) | `~/projects, ~/Projects, ~/code, ~/src, ~/dev, ~/git`, 3 |
| CI status (`ci`, opt-in) | `repos` (`owner/name`, comma-separated), `use_gh_token` (the `gh` CLI's login for private repos and higher limits; else `GITHUB_TOKEN`), `notify` | empty, on, on |
| Crash reports (`crashes`, opt-in) | `notify` (a notification for a new crash) | on |
| Jannah (`jannah`, opt-in) | `rotate` (`daily`, `hourly`), `sources` (`both`, `quran`, `hadith`), `show_translation`, `arabic_size` (`small`, `medium`, `large`), `bar_label` (`glyph`, `step`), `morning_reminder` | daily, both, on, medium, glyph, off |

Older plugin settings (weather, prayer times, pomodoro, screenshot and
recording, updates, world clock, power history, media lyrics, visualizer,
tray, drives, containers, services) are unchanged.

### Covered by a global setting

Some values deliberately have no knob of their own because a global one
already scales them:

| Values | Covered by |
|---|---|
| per-element corner radii | Themes → Corner roundness (`style.radius_scale`), bar / panel / card radii |
| panel and card spacing, padding, widths | `panels.width_scale`, `panels.padding`, `sidebar.look.*` |
| text and icon sizes | `typography.scale`, `bar.text_scale`, `bar.icon_scale` |
| animation durations and springs | `motion.*` (speed, reduce motion), panel / sidebar spring overrides |
| colours, glows and alphas | the theme's tokens, `*.opacity`, `bar.tint`, `style.backdrop_dim` |
| the plugins' quick toasts | `panels.toast_ms` (they stay 65 % as long) |

## Guard

`crates/mandara-ui/tests/no_hardcoded.rs` scans the shell, Studio, greeter
and service sources (tests, comments, strings and `const` items stripped)
for bare `px(<number>)`, `Duration::from_millis(<number>)` and
`Duration::from_secs(<number>)`. The few files that still have such
literals (fixed dialog and Studio geometry) are listed with a reason and an
exact count in `crates/mandara-ui/tests/literal_allowlist.tsv`; a new literal
fails the test — make it a setting, or a named constant whose comment says
why it is fixed. Studio's settings audit (`settings_audit.rs`) separately
checks that every Studio control has code that reads it and every plugin
setting the shell reads has a control.

### Adding a setting

1. A config field (or a plugin key) with its default = today's value, and a
   named range (`mandara_config::surfaces::ranges`, or a `*_RANGE` constant
   next to the plugin's default).
2. Read it from the config snapshot where it's used, clamped to the range —
   no restart, no per-frame allocation.
3. A registry entry (`crates/mandara-studio/src/registry*.rs`) on the right
   page; plugin keys go in the plugin's group so they appear in its modal.
4. Translate the label and help (`MANDARA_I18N_DUMP=1 cargo test -p
   mandara-i18n`, then the nine catalogs in `crates/mandara-i18n/locales`).

## Desktop widgets

`[desktop.widgets]` lists everything painted on the wallpaper layer
besides the image (model: `crates/mandara-config/src/desktop_widgets.rs`).
The list is empty by default and an empty list costs nothing — no plane,
no timer, no sampler. Each `[[desktop.widgets.items]]` is one widget; its
`kind` picks the variant and the variant's keys sit beside the common ones.
`desktop.widgets.hidden = true` hides every widget at once — the depth
clock and the cards too — without switching any off (the desktop menu's
*Hide desktop widgets*, `mandara msg desktop widgets hide|show|toggle`);
desktop edit mode shows them anyway.

| Key | Meaning |
|---|---|
| `id`, `enabled` | stable id (`clock-2`), on / off |
| `place.displays` | connector names it shows on; empty = every display |
| `place.anchor` | `top-left` … `bottom-right`, `center`: the display point and the widget point pinned together |
| `place.x`, `place.y` | offset from the anchor, fractions of the display (−1 … 1) |
| `place.scale` | 1 = designed size on a 1080 px tall display (0.25 … 4) |
| `place.opacity` | 0 … 1 |
| `place.depth` | `behind` (between the wallpaper and its subject; needs a depth mask and Fill = Cover) or `front` |
| `place.rotation`, `place.flip_x`, `place.flip_y` | turn the painted widget (−180 … 180 degrees) and mirror it; interactive cards stay upright |
| `look.tone`, `look.accent` | `auto` (light or dark against the wallpaper under it), `on-surface`, `accent`, `custom` with `look.custom_color` / `custom_accent` (`#rrggbb` or `oklch(L C H)`) |
| `look.font` | `widgets` (Typography → Desktop widgets, `typography.widgets_font`; the display face when that is empty), `display`, `ui`, `arabic`, `mono`, or `custom` with `look.font_family` |
| `look.effect`, `look.effect_strength` | `none`, `shadow`, `glow`; strength 0.1 … 2 (1 = as designed) |
| `look.align` | `auto` (with the anchor; decorations centre), `start`, `center`, `end` |
| `look.plate` | `none`, `glass` (theme surface at `look.plate_opacity`, 0.05 … 1, with a specular rim) or `solid`; `look.plate_radius` (0 … 64) and `look.plate_padding` (0 … 96), reference px. On a plate, `auto` colours read against the plate. |
| `look.plate_shape` | `rect` (corners from `plate_radius`), `circle`, `squircle`, `cookie` and `flower` (`plate_lobes` 3 … 24, `plate_depth` 0.02 … 0.3), `pentagon`, `chamfer` (`plate_chamfer` 0 … 64), `skew` (`plate_skew` −30 … 30 degrees), `deckle` (torn paper). The same shapes serve check boxes, cells, covers and markers where a kind has a shape option. |
| `look.outline_width`, `outline_tone`, `outline_color` | an outline round the plate (0 … 12 reference px), `auto` / `on-surface` / `accent` / `custom` |
| `look.shadow` | `none`, `soft` or `hard` (a solid offset copy, the sticker look); `shadow_x`, `shadow_y` (−40 … 40), `shadow_blur` (0 … 60), `shadow_opacity` (0 … 1), `shadow_tone` / `shadow_color` |
| `look.header`, `header_size`, `header_rule` | a title line over the widget, its size and a rule under it |
| `look.chrome` | `none`, `window` (a classic title bar with the header in it), `tape` (washi tape across the top, `tape_angle` 0 … 60) or `polaroid` (a white border with a deeper bottom); `chrome_tone` / `chrome_color` |
| `locale.numerals` | `auto` (Eastern Arabic in Arabic, else Western), `western`, `eastern-arabic` |
| `locale.calendar` | `gregorian`, `hijri`, `both` or `none` (clocks and the countdown; the Hijri day begins at Maghrib and follows Prayer times' correction) |
| `locale.language` | a language code for names and words (`ar`, `de`, …; empty = the interface's); it also sets the reading direction |
| `locale.hijri_in_arabic` | write the Hijri date in Arabic with its own digits (default `true`) |

`[desktop.widgets] version` records the defaults a list was written for:
a list from before version 1 (no numerals, calendar or widgets font yet)
is upgraded once to show exactly what it showed (Western digits unless
`eastern_numerals` was set, the depth pill's two calendars, the display
face through `widgets`). Every kind comes from a first-party plugin
(`desktop-clocks`, `desktop-monitor`, `desktop-decorations`,
`desktop-visualizers`, `desktop-now-playing`, `desktop-calendar`,
`desktop-weather`, `desktop-world-clock`, `desktop-countdown`,
`desktop-productivity`, `desktop-sky`, `desktop-identity`, `desktop-fun`,
`desktop-info`); a widget whose plugin is off isn't drawn.

Kinds:

- **`clock`** — `face`: `superscript`, `headline`, `day-block`, `ring`,
  `greeting`, `day-numeral`, `minimal`, `analog`, or the depth clock's own
  `stacked` / `classic` / `arabic` (with its glow, date pill and audio
  ring; the first enabled one is drawn). `seconds` (superscript and
  analog: only that small plane redraws each second, and only while the
  desktop is visible), `date`, `weather` (needs the weather plugin and a
  location), `audio_ring` (depth faces), `ring_progress` = `hour` / `day`,
  `dial_numbers` (analog). Other clocks redraw once a minute.
  More faces: `stack` (a stacked card), `shape` (the time in
  `face_options.shape`, which can `spin` a little each minute),
  `dot-matrix` and `pixel` (`square_dots`, `unlit` dots), `boxes` (a digit
  in each `box_shape`, `week_strip`), `hud` (`slant`, `moon`, the part of
  the day), `numerals` (hands over giant numerals), `orbit`, and
  `template`: `lines` you write, each `{ text, size, weight, opacity,
  accent, caps }`, with `{HH}`, `{H}`, `{hh}`, `{h}`, `{mm}`, `{ss}`,
  `{ampm}`, `{time}`, `{weekday}`, `{wd}`, `{day}`, `{dd}`, `{month}`,
  `{mon}`, `{MM}`, `{year}`, `{greeting}`, `{daypart}` and `{hijri}`. The
  analog dial takes `analog = { dial, dial_fill, marks (none / dots /
  ticks / numbers), hour_hand (classic / fill / hollow), hand_width,
  seconds_style (hand / dot), date_style (under / bubble / box),
  dial_turn }`; the greeting clock takes your own `phrases` and a
  `subtitle`.
- **`progress`** — `period` (`hour`, `day`, `week`, `month`, `year`),
  `style` (`ring`, `wavy-ring`, `bar`, `dots`), `label` (what's left),
  `thickness`.
- **`todo`**, **`note`**, **`pomodoro`**, **`quote`**, **`notebook`**
  (`desktop-productivity`; read from the To-do, Notes and Pomodoro
  plugins) — to-do: `list` (a list's name, empty = all), `rows`,
  `show_done`, `checkbox` (a shape), `ruled`, `title`, `add_row`, `width`,
  `text_size`; a click on a task ticks it. Note: `note` (a title, empty =
  the latest), `sheet` (`theme`, `pastel`, `paper`, `custom` with
  `sheet_color`), `ruling` (`plain`, `ruled`, `grid`, `dots`), `lines`,
  `word_count`. Pomodoro: `style` (`ring`, `dots`), `controls`, `size`;
  a click starts or pauses it. Quote: `quotes` (`text — author`; empty =
  152 built-in sayings, each checked against its source and shown with its
  work when it fits), `every` (`day`, `hour`, `click`), `mark`. Notebook:
  `tabs`, `tab_shape`.
- **`date`** — `layout` (`month`, `week`, `agenda`, `tile`), `marker`
  (today's shape), `week_numbers`, `event_dots`, `rows` (agenda). Events
  come from `[calendar]`.
- **`sky`** (`desktop-sky`) — `layout` (`sun-arc`, `moon`, `globe`),
  `size`, `dashed`, `ticks`, `arc_height`, `labels`, `shaded`, `moon`.
  Computed from the weather location and the clock; the globe shows the
  real day / night line (no map).
- **`weather`** also takes `layout` = `sentence` (`sentences`: your own
  lines per condition), `disc` (`disc` size, `dotted_ring`) and `hourly`
  (`hours`, `fill`).
- **`profile`**, **`shortcuts`**, **`toggles`**, **`sliders`**,
  **`palette`** (`desktop-identity`) — profile: `avatar` (a shape),
  `login`, `uptime`, `show_status`. Shortcuts: `items` (app ids, folders
  or links), `folders`, `layout` (`grid`, `list`), `columns`, `tile`,
  `colorful`, `labels`. Toggles: `toggles` (`wifi`, `bluetooth`, `dnd`,
  `night-light`, …), `shape_off`, `shape_on` (the tile morphs), `columns`,
  `labels`. Sliders: `levels` (`volume`, `brightness`), `vertical`,
  `length`, `thickness`; click or scroll to set. Palette: `swatch`,
  `labels`, `columns`; a click copies the colour.
- **`pet`**, **`bubbles`**, **`photo`**, **`sticker`** (`desktop-fun`) —
  the desk cat (`size`, `name`; 12 fps while seen, 2 fps asleep at night,
  a click pokes it), bubbles (`count`, `min_size`, `max_size`, `speed`,
  `width`, `height`; 24 fps while seen), a photo frame (`folder`,
  `minutes`, `crop`, `width`, `height`; a click shows the next) and a
  sticker (`image`, `size`, `border`: a die-cut outline).
- **`media`** — `layout` (`vinyl`, `device`, `shaped`, `side-pill`,
  `halftone`, `lyrics`), `size`, `spin` and `grooves` (vinyl), `shapes`
  (the shaped cover's shapes), `dot_pitch` and `dither` (halftone),
  `lines` (lyrics), `controls`. Only while a player is loaded; the record
  turns only while music plays and the desktop shows.
- **`fetch`**, **`screen-time`**, **`prayer`**, **`search`**, **`glance`**,
  **`github`**, **`currency`**, **`ai-usage`**, **`games`**
  (`desktop-info`) — the machine card (`layout` = `terminal`, `rows`,
  `grid`; `logo`, `colors`), screen time (`days`, `apps`; needs the Screen
  time plugin), prayer times (`layout` = `ring`, `list`; `arabic_names`;
  needs the Prayer times plugin), a search field (`width`, `shape`,
  `placeholder`; opens the launcher), at a glance (`weather`). GitHub:
  `user`, `online` (default `false`: nothing is fetched until it's on),
  `weeks`, `cell` (a shape), `summary`. Exchange rates: `base`, `pairs`,
  `decimals`, `online` (default `false`; from open.er-api.com). AI usage:
  `read_logs` (default `false`; Claude Code's logs in `~/.claude/projects`,
  never sent anywhere), `days`, `budget_millions` (a weekly meter, 0 =
  none). Games: `count`, `cover` (a shape), `names`; installed Steam
  games, a click starts one. Network answers are cached in
  `~/.cache/mandara/desktop-feeds/` (GitHub for six hours, rates for
  twelve) and nothing is fetched while the desktop is covered or locked.
- **`monitor`** also takes `style` = `meters`, `bars` or `thermals`, with
  `ring_width`, `wavy`, `dotted`, `start_angle` and `segments`; the
  **`visualizer`** takes `shape` = `led` (segmented LED meter) and the
  **`decoration`** `shape` = `tape`.
- **`monitor`** — `style` = `column` or `gauges` (ring gauges for the
  readings that are a share, the rest listed under them); `rows` in order
  (`cpu`, `cpu-graph`, `gpu`, `memory`, `gpu-power`, `cpu-temp`,
  `gpu-temp`, `fan`, `battery`, `disk`, `download`, `upload`, `net-graph`;
  rows the machine can't read are hidden), `interval_s` (1 … 60, default
  2), `width` (reference px), `title`. Sampling runs only while a monitor
  shows on a visible display (not covered by a fullscreen / tiled window,
  not locked, screens on) and only reads the shown rows; only the values
  plane redraws.
- **`weather`** — `layout` = `hero` or `compact`, `location`, `details`
  (feels like, humidity, wind), `days` (0 … 7 days ahead). From the
  Weather plugin's forecast; redrawn when it changes.
- **`world-clock`** — `cities` (names or IANA zones; empty = the World
  clock plugin's), `offsets` (hours ahead or behind, tomorrow / yesterday),
  `horizontal`. Redrawn once a minute.
- **`countdown`** — `title`, `date` (`YYYY-MM-DD`; empty = the next New
  Year's Day), `show_date`, `count_up` (days since, after the date).
- **`decoration`** — `shape`: `text` (`text`, `vertical` top-to-bottom
  columns right to left, `size`, `weight`, `tracking`), `disc`, `circle`,
  `stripes`, `waves`, `rule`, `frame` (`width`, `height`, `stroke`,
  `count`); `rotation` in degrees. Drawn once.
- **`visualizer`** — `shape`: `bars`, `mirrored`, `wave`, `radial`;
  `bars`, `length` (radial: diameter), `height`, `angle`, `gap`, `fps`
  (15 … 60, default 30), `silence_s` (freeze and stop listening after this
  much silence; music starting again resumes it). Listens to the spectrum
  only while a player is playing and the desktop is visible; its plane is
  sized to the shape.
- **`card`** — an interactive card or GPU visualizer on its own small
  surface (`card`, `options`), drawn by the shell's card host — see
  [Desktop cards and visualizers](#desktop-cards-and-visualizers).

The depth clock that predates widgets (`[wallpaper] desktop_clock`,
`clock_style`, `clock_x` / `clock_y` / `clock_scale`, `show_date`,
`show_weather`, `audio_ring`) is moved in once as the widget
`depth-clock` (`desktop.widgets.migrated = true`); from then on the widget
is the source and those `[wallpaper]` keys are rewritten from it on load.

```toml
[[desktop.widgets.items]]
id = "clock-1"
kind = "clock"
face = "day-block"
[desktop.widgets.items.place]
anchor = "top-right"
x = -0.06
y = 0.1
```

## Wallpaper slideshow

`[wallpaper.slideshow]`, edited in *Studio → Wallpaper → Slideshow* or with
`mandara msg wallpaper slideshow …`. Keys added after 0.4.0 default to the
behaviour before them, so an older table keeps its folder, order, interval
and pause.

```toml
[wallpaper.slideshow]
enabled = true
interval_minutes = 30          # 1 to 10080 (a week)
order = "shuffle"              # shuffle (each once per round) | random | alphabetical | newest
source = "folder"              # folder | playlist
directory = ""                 # empty: the wallpaper folder
subfolders = false             # every sub-folder (else one level, like the picker)
playlist = []                  # wallpapers picked for source = "playlist"
light_directory = ""           # used instead while the theme is light (empty: the folder above)
dark_directory = ""            # …and while it is dark
on_start = false               # a new wallpaper each time the shell starts
pause_on_fullscreen = true
pause_on_battery = false
pause_when_locked = true       # also while the screens are off
hours = false                  # only change between `from` and `until` (may cross midnight)
from = "08:00"
until = "22:00"
transition = "desktop"         # desktop (wallpaper.transition and crossfade_ms) | fade | wipe | grow | none
transition_ms = 1200           # 100 to 5000, when transition isn't "desktop"
per_display = false            # each display its own wallpaper (written to wallpaper.variants.monitors)
```

No timer runs while the slideshow is off. While it is on, one timer sleeps
until the next change; a change that is due but paused is looked at again a
minute later (or when the `from` time comes). Turning `per_display` off
hands the displays showing slideshow wallpapers back to the main one; your
own per-display choices stay.

## Video wallpapers

Any AV1 clip in WebM / MKV or IVF (`.webm`, `.mkv`, `.ivf`, 8- or 10-bit
4:2:0, audio ignored) can be the wallpaper: pick it in the picker (video
items carry a badge), set it with `mandara msg wallpaper set clip.webm`, or
let `wallpaper random` and the slideshow choose it. It loops seamlessly
behind the clock and the desktop widgets.

- **One decoder per clip.** Every display showing the same clip shares one
  decode thread (pure-Rust rav1d); each display converts the decoded
  picture straight into its own wallpaper buffer at the clip's resolution,
  and the compositor scales it with the fill mode (Tile shows a clip once,
  centred).
- **Stills where a picture is needed.** The first frame is cached as a
  poster (`~/.cache/mandara/posters/`): crossfades from and to images,
  thumbnails, the lock screen and `wallpaper render-preview` use it. Match
  colors takes the theme from a keyframe in the middle of the clip.
- **No depth subject.** The subject moves, so videos skip the depth effect's
  cut-out; the clock and widgets sit on top of the clip.
- **Paused, it costs nothing.** No decoding and no timers while the session
  is locked, the screens are off, windows cover the desktop
  (`pause_when`), on battery (`pause_on_battery`) or when the compositor
  stops drawing the desktop. The frame on screen stays; after a minute the
  decoder is let go and its memory with it. A still wallpaper never starts
  any of this.

| Key (`[wallpaper.video]`) | Default | |
|---|---|---|
| `pause_when` | `covered` | `covered` (a tiled or fullscreen window on the visible workspace), `fullscreen`, `never` |
| `pause_on_battery` | `true` | Keep the current frame while unplugged |
| `max_fps` | `30` | Highest frame rate drawn (1–60); frames above it are skipped, but every frame is still decoded |
| `still_frame` | `false` | Show the first frame only: the cost of a still image |

Decoding is the cost of a playing video wallpaper, and it depends mostly on
the clip's detail and bitrate. Measured with a release build in the demo
sandbox (2560 × 1440 output, 16-core Ryzen 9 9950X with other builds
running, synthetic 30 fps clips from `mandara-media`'s `make_test_clip`),
in CPU of one core for the whole shell process:

| clip | smooth gradients (≈ 0.3–0.5 Mbit/s) | dense texture and grain (≈ 5–9 Mbit/s) |
|---|---|---|
| 1080p30 | about 55 % | about 130–175 % |
| 1440p30 | about 95 % | about 200 % |

Paused, the shell is back at its idle cost (0.2 % in the same sandbox). A
playing clip also holds the decoder's pictures: plan for about 60–140 MiB
more resident memory at 1080p–1440p, released a minute after playback
stops. Smooth, slow loops at 1080p (what Mandara's wallpaper generator
makes) are the efficient choice; check a clip with
`cargo run --release -p mandara-media --example decode_bench -- clip.webm`.

## Wallpaper generator

The `wallpaper-generator` plugin (opt-in) keeps its settings in
`[plugins.settings.wallpaper-generator]`, edited in *Studio → Wallpaper →
Generator*. Only values that differ from the defaults are written; a
missing or invalid key falls back to its default on its own, and numbers
are clamped to their ranges.

| Key | Default | |
|---|---|---|
| `recipe` | *(empty)* | The current recipe as the engine's TOML table (`style`, hex `seed`, `palette = { named = "…" }` / `{ custom = [hex…] }` / `{ theme = […] }`, `[params]`); empty: a mesh gradient in Mandara Corniche |
| `tone` | `theme` | `theme` (follow light / dark mode), `dark`, `light` |
| `size` | `display` | `display` (the 4K class fitted to the primary display's shape), `1080p`, `1440p`, `4k`, `5k`, `8k`, `uw1440`, `portrait4k`, `custom` |
| `custom_width`, `custom_height` | `3840`, `2160` | For `custom` (640–16384 px) |
| `format` | `webp` | `webp` (lossless), `png` (lossless), `jpeg` |
| `jpeg_quality` | `92` | 60–100 |
| `directory` | *(empty)* | Output folder (`~` allowed); empty: `Generated` inside the wallpaper folder, which the picker lists |
| `apply` | `true` | Set a wallpaper you generate as the desktop background |
| `seed_locked` | `false` | Shuffles and variations keep the layout seed |
| `variation` | `0.6` | How far variations stray (0.05–1) |
| `schedule.enabled` | `false` | Opt-in: no timer and no thread exist while it is off |
| `schedule.every_minutes` | `360` | 15 minutes to a week |
| `schedule.mode` | `random` | `random` (any style), `variation` (of the current recipe), `reseed` (the recipe, new layout) |
| `schedule.kind` | `still` | `still` or `video` |
| `schedule.apply` | `true` | Each result becomes the wallpaper |
| `schedule.keep` | `30` | Newest generated files kept in the folder (0–1000; 0 keeps all); only files with a recipe sidecar are ever removed |
| `video.seconds`, `video.fps` | `10`, `24` | Loop length (2–60 s) and frame rate (12–60) |
| `video.size` | `1080p` | Size class of loops (`custom` isn't offered: AV1 in pure Rust takes about 30 s for a 10 s 1080p loop) |
| `video.quality` | `70` | 0 (smallest) – 100 (best picture) |

Every generated file has a hidden recipe sidecar next to it
(`.<file>.recipe.toml`): `wallgen remix`, the picker's Remix and Studio's
Recent read it. The schedule remembers its last run in
`~/.local/state/mandara/wallgen.toml`, so a restart keeps the rhythm, and a
schedule that missed runs while the machine was off runs once, shortly
after the shell starts, never in a burst. Turning it on starts the clock:
the first wallpaper comes one interval later.

## Desktop cards and visualizers

Items of kind `card` (model: `crates/mandara-config/src/desktop_cards.rs`)
are interactive, so each one is its **own small layer surface** on the
Bottom layer (`mandara-desktop`) — never a full-screen one — sized to the
card, with the pointer passing through everywhere outside the card
(visualizers take no input at all). A surface exists only while its card
shows and is destroyed when it hides. `place.*` works as for every widget
(anchor, offset, displays, scale, opacity); cards keep clear of the bar's
reserved edge, and right-to-left languages mirror the anchor.

| `card` | Shows | `options` |
|---|---|---|
| `now-playing` | album art (decoded at its display size), title, artist, a wavy progress line, previous / play / next, synced lyric lines with the current one glowing. Only while a player is loaded; fades out `grace_s` after it stops. Redraws once a second while playing. | `width` (280–520), `lyric_lines` (0–4), `squiggle`, `grace_s` (0–60, default 8) |
| `calendar` | this month, today highlighted, previous / next / Today, the next `[calendar]` event. Redraws at midnight and when calendars reload. | `width` (240–420), `agenda` |
| `wave` | a smoothed, mirrored spectrum filling a strip along the anchor's top or bottom edge, accent gradient with a soft glow | `height` (40–320), `glow`, `opacity` |
| `framed-bars` | a bordered glass box of segmented bars with falling peak caps | `width`, `height`, `bars` (8–96), `opacity` |
| `mirrored-bars` | a full-width strip of bars mirrored round its middle | `height` (24–200), `bars`, `opacity` |
| `plugin:<id>/<widget>` | a third-party plugin's desktop widget, drawn by its `desktop-widget` export on a glass card ([PLUGINS.md](../PLUGINS.md#desktop-widgets)) | `width` (120–720), `height` (60–640), `numerals`, `calendar`, `language` (told to the plugin) |

`[visualizers]` paces the three visualizers: `fps` (15–60, default 30 — a
timer asks for the next frame, the display's refresh never drives them)
and `silence_s` (1–60, default 5). They subscribe to the spectrum only
while a player is *playing*; after `silence_s` of silence they drop the
subscription and stop drawing (zero CPU); while the player still says it
is playing a new track wakes them, and they listen again for 1 s every 30 s (a quiet intro); a
quiet subscription is only checked four times a second, never drawn at
the frame rate, and once playback stops their surface is destroyed after the same window.

```toml
[[desktop.widgets.items]]
id = "card-1"
kind = "card"
card = "wave"
[desktop.widgets.items.options]
height = 160
glow = true
[desktop.widgets.items.place]
anchor = "bottom"
```

## Side bar

`bar.position = "left"` or `"right"` puts a slim vertical bar
(`bar.vertical_width`, 32–64, default 40) on that edge. It is always drawn
as the edge frame (`bar.layout` is kept for when you move it back), it
reserves exactly its width (an exclusive zone on its side) and takes input
only on itself. Zones run top → middle → bottom: start group at the top,
center group centred, end group at the bottom. Plugins draw a compact
column form — stacked clock digits, number-only tags in a column, tray
icons stacked, icon over value for sound and mini stats; an item whose
content is still wider than the bar is left out of the side bar. Panels
and the main menu grow sideways out of the bar (the same morph, turned
90°); the launcher keeps its own edge and opens beside the bar.

`bar.screen_frame` draws a thin border (`bar.frame_thickness`, 2–16,
default 6) round the rest of the display, joined to the bar with concave
corners. It reserves no space of its own (keep the compositor's outer
gaps at least that wide) and never takes clicks.

Interplay: auto-hide doesn't apply to a side bar; the dock can't share its
edge (it moves to the bottom there); the sidebar slides out beside the
bar when they share an edge (its hover trigger is then off — open it from
the bar or `mandara msg sidebar toggle`); the power dock's hover trigger
is off on the bar's edge. `mandara msg bar position left|right|top|bottom`.

### How a flush sidebar joins the bar

A flush sidebar (`sidebar.look.attach = "flush"`) is one piece with an
attached bar: the bar's own fill, and concave fillets of the bar's flare
radius wherever its inner edge meets the bar or a screen edge.

- **Edge frame:** it hangs from the frame, a fillet tucked under the
  frame and another into the far screen edge; the frame's screen corner on
  that side moves to the sidebar's edge as it slides out.
- **Hairline:** square joints (the hairline has no flares) and the accent
  line turns down the sidebar's inner edge.
- **Notch:** the sidebar rises to the notch's edge; once it reaches the
  notch the two merge, the notch ending square at the sidebar with a
  fillet under it. A sidebar narrower than the gap beside the notch hangs
  from the edge on its own shoulder.
- **Side bar:** beside a bar on its edge it continues the strip, carrying
  the bar's top and bottom fillets (or the screen frame's corners) to its
  own edge; on the other edge it flares into the top and bottom edges.

Glass pills, the island, the micro pill and a floating frame leave nothing
to join, so the sidebar floats there, `sidebar.look.margin` clear of the
screen edges and the bar.

### Bar commands

`mandara msg bar hide` puts the bar away and frees its space until
`mandara msg bar show` (or `bar toggle`); touching its edge still peeks it
out, as with auto-hide. It isn't saved: a restart brings the bar back. A
side bar always stays.

## Looks

`[looks]` remembers which one-click look is applied (`active`, empty = your
own setup) and `wallpaper_colors` (take the palette from the current
wallpaper with the look's scheme instead of the look's own seed). A look
(`crates/mandara-config/src/looks.rs`) sets the bar style, position and
items, the desktop widget list (clocks, monitor, decorations, cards,
visualizers), the theme (`theme.source` = an HCT palette from an OKLCH
seed), fonts, `style.radius_scale` and the bar opacity; it never changes
the wallpaper image. Applying one from your own setup first saves the
paths it touches as the look profile **Before looks** in
`~/.config/mandara/profiles/`; switching between looks keeps that
snapshot, and *Restore my look* / `mandara msg look restore` applies it
back. `mandara msg look list`, `mandara msg look apply <id>
[wallpaper-colors]`.

| id | Look |
|---|---|
| `corniche-dusk` | rose glass pills, headline clock, system monitor, now-playing card |
| `pharos-ink` | monochrome hairline bar with mini stats, greeting clock, calendar card |
| `montaza-crimson` | red island bar, framed spectrum box, Arabic clock, vertical calligraphy |
| `qaitbay-forest` | green edge frame, giant stacked clock behind the subject, dock now-playing chip |
| `anfushi-wave` | ocean micro pill, full-width glowing wave, katakana accent |
| `night-frame` | violet side bar with screen frame, greeting, calendar card, mirrored bars |

## Launcher placement

```toml
[launcher.look]
position = "bottom-center"   # center | top-left | top-center | top-right | bottom-left | bottom-center | bottom-right
align_to_button = true       # in the bar's launcher-button zone, grow out of the button
width = 600.0                # attached (440–960)
height = 560.0               # attached, tallest it grows (240–1000)
center_width = 720.0         # the floating card (480–1280)
center_height = 640.0        # (240–1000)
dim = 0.0                    # 0–0.7; a bar the launcher hangs from stays bright
```

- **Attached at the bar's edge** it grows out of the bar exactly like a
  drop-down panel: the edge frame pours it out with concave flares (same
  radius, flare and fill as panels); floating pills and the island get a
  floating card the panels' `gap` away. Left / right sit against the screen
  sides (clear of the frame's screen flares, or in line with the outer
  pills). With `align_to_button`, a placement whose third of the screen
  holds the bar's launcher button centers on the button and morphs out of
  it.
- **At the other edge** (or on a display without the bar) it opens from the
  bare screen edge: with flares in the edge-frame style (the shipped bottom
  drawer), flush with rounded inner corners for the floating styles.
- **Center** floats a larger card in the middle (scale + fade), clear of the
  bar and the dock.
- With the **dock** on, bottom placements and the center card float above
  the dock (never covered by it), and the launcher opens on click only.
- The open animation follows `[panels] animation` (morph / slide / none)
  and `[panels] spring`; reduce motion makes it appear at once. Left and
  right are physical sides, also in right-to-left languages.
- Older configs' `position = "bottom"` / `"top"` load as `bottom-center` /
  `top-center` (the next save writes the new spelling); `width` stays the
  attached width.
- `mandara msg launcher placement <mode>` sets it (live, even while the
  launcher is open); without an argument it reports the current one.

## Sidebar tabs

```toml
[sidebar]
notifications_tab = true    # the built-in Notifications tab, after yours
always_first_tab = false    # true: every opening shows the first tab
add_tab_button = true       # the "+" at the end of the tab strip

[[sidebar.tabs]]
id = "overview"             # stable id (commands, the last-tab state)
name = "Overview"
icon = "grid"
cards = ["quick-settings", "weather", "media", "todo"]

[[sidebar.tabs]]
id = "focus"
name = "Focus"
icon = "timer"
cards = ["pomodoro", "todo", "notes"]   # a widget may sit on several tabs (one shared instance)
```

Configs written before tabs had `[sidebar] cards = [...]`: that list loads
as the first tab, Overview, and the next save writes the `tabs` shape. The
tab shown last is state, not config: `$XDG_STATE_HOME/mandara/sidebar.toml`
(`last_tab = "focus"`).

## To-do lists and notes storage

To-dos live in `~/.local/share/mandara/productivity.json`: `lists`
(`id`, `name`, `color` — a tone `primary` … `urgent` or `hue:<0-359>` —
optional `icon`, `show_done`) and `todos` (each with its `list`). Files from
before lists load with every task in the Inbox.

Notes are Markdown files: the notes folder (`storage_folder`) holds the
default topic's notes, every sub-folder is a topic, and
`.mandara-notes.toml` in it keeps topic order, colours, icons and the
numeric note ids commands use. Files stay the source of truth — edits made
in another editor (Obsidian, a synced Nextcloud folder) show up live, new
folders become topics, and losing the index only loses colours. A note's
file is named after its title and renamed to follow it when you finish
editing. Notes kept in `productivity.json` by older versions are written
out as files on first start and only then removed from the JSON.
