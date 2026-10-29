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
never breaks the layout. `mandara msg config get <pointer>` and
`mandara msg config set <pointer> <value>` reach the same keys as JSON
pointers (for example `/plugins/settings/resources/refresh_ms`).

## What's configurable

Grouped by Studio page. Keys are config paths (`table.key`).

### Look & feel

| Page | Settings |
|---|---|
| Themes | **looks** — six one-click looks and *Restore my look* (`looks.*`, see [Looks](#looks)); light / dark / automatic and its schedule (`theme.*`), palette tuning (`theme.adjustments`), theme crossfade (`theme.transition_ms`), icons (`appearance.*`), corner roundness, shadows, borders, backdrop dim (`style.*`); the theme and app theming switches (`integrations.*`); **cursor** — theme, size, hide while typing / when idle, Hyprland's software cursor and hide on touch (`compositor.input.cursor.theme`, `.size`, `.hide_when_typing`, `.hide_after_secs`, `.no_hardware_cursors`, `.hide_on_touch`; themes Mandara downloaded are listed in `~/.local/share/mandara/cursor-themes.json`) |
| Wallpaper | folder, picker style / size / focus / names, fill and colour, transitions, crossfade (`wallpaper.crossfade_ms`), light / dark and per-display variants, slideshow interval and order, depth effect and subject mask (`wallpaper.*`); video wallpapers: pause behind windows / on battery, frame-rate limit, first frame only (`wallpaper.video.*`, see [Video wallpapers](#video-wallpapers)) |
| Desktop widgets | clocks (incl. the depth clock), system monitor, decorations, plane visualizer, show / hide them all (`desktop.widgets.*`, see [Desktop widgets](#desktop-widgets)); desktop cards and visualizers (`cards.*`, `visualizers.*`) |
| Typography | interface / display / Arabic / monospace fonts, text scale, weight (`typography.*`) |
| Motion | stiffness, damping, mass, animation speed, reduce motion (`motion.*`) |

### Desktop

| Page | Settings |
|---|---|
| Bar | style, position, height, gaps, radii, opacity, accent tint, rim width / intensity, shadow, text / icon scale, item padding / spacing, separators, center alignment, floating frame, **hairline / micro pill** (`bar.layout` = `hairline` / `micro`, `bar.compact_height`, `bar.accent_line`), **side bar** (`bar.position` = `left` / `right`, `bar.vertical_width`, `bar.screen_frame`, `bar.frame_thickness`, see [Side bar](#side-bar)), auto-hide mode and delay, click-through, over fullscreen (`bar.*`); drop-down panels: width, maximum height, padding, gap, anchor, opacity, shadow, animation, custom spring, **close delay** (`panels.leave_delay_ms`), **action message time** (`panels.toast_ms`) |
| Sidebars & hover | sidebar side, mode, width, tabs (`sidebar.tabs`, `always_first_tab`, `add_tab_button`, `notifications_tab`) and look (`sidebar.*`, `sidebar.look.*`), edge triggers and dwell (`edges.*`), hot corners with delay and size (`hot_corners.*`), screen corners (`screen_corners.*`) |
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
| Date, time & location | formats, calendars (refresh, reminders, agenda days), location, weather refresh (`datetime.*`, `calendar.*`, `location.*`, `weather.*`) |
| Applications | default and preferred apps, autostart (`apps.*`) |

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
| Media (`media`) | `lyrics_rows` (odd, 3–9) | 5 |
| Screenshot (`screenshot`) | `toolbar_position` (`top`, `bottom`; the toolbar's arrow button or M flips it live), `toolbar_scale` (0.75–1.5 ×, also its hint strip), `countdown` ("Capture in N s" and `--delay` without a number, 1–10 s) | top, 1 ×, 3 s |
| Clipboard (`[clipboard]`) | `pinned_shown` (pinned rows above the history, 0–12) | 4 |
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
| `look.tone`, `look.accent` | `auto` (light or dark against the wallpaper under it), `on-surface`, `accent`, `custom` with `look.custom_color` / `custom_accent` (`#rrggbb` or `oklch(L C H)`) |
| `look.font` | `display`, `ui`, `arabic`, `mono` (families from Typography) |
| `look.effect` | `none`, `shadow`, `glow` |

Kinds:

- **`clock`** — `face`: `superscript`, `headline`, `day-block`, `ring`,
  `greeting`, `day-numeral`, `minimal`, or the depth clock's own
  `stacked` / `classic` / `arabic` (with its glow, Gregorian + Hijri date
  pill and audio ring; the first enabled one is drawn). `seconds`
  (superscript: only that small plane redraws each second, and only while
  the desktop is visible), `eastern_numerals`, `date`, `weather` (needs the
  weather plugin and a location), `audio_ring` (depth faces),
  `ring_progress` = `hour` / `day`. Other clocks redraw once a minute.
- **`monitor`** — `rows` in order (`cpu`, `cpu-graph`, `gpu`, `memory`,
  `gpu-power`, `cpu-temp`, `gpu-temp`, `fan`, `battery`, `disk`,
  `download`, `upload`, `net-graph`; rows the machine can't read are
  hidden), `interval_s` (1 … 60, default 2), `width` (reference px),
  `title`. Sampling runs only while a monitor shows on a visible display
  (not covered by a fullscreen / tiled window, not locked, screens on) and
  only reads the shown rows; only the values plane redraws.
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
