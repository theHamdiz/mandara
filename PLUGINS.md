# Writing and publishing Mandara plugins

Everything in Mandara is a plugin: the bar items, the panels that drop from
them, and the sidebar cards. Third-party plugins are written in **Rust**,
compiled to a **WebAssembly component**, and run **sandboxed**. A plugin has
no filesystem, network or environment access, each call has a time budget,
and a crash only disables that plugin. The shell keeps running.

Plugins hot-reload: drop a folder in, rebuild, edit a translation, or delete
it, and Mandara picks the change up live, without restarting. Bar items,
panels and sidebar cards that are already open switch to the new build at
once.

**Studio → Plugins** is a store with five places: **Discover** (the store
front), **Installed** (your library), **Updates** (marketplace plugin
updates), **Community** (the GitHub marketplace) and **Develop** (the
author's handbook). Every card opens the plugin's **product page**: its
cover and icon, what it may access, its settings, where it fits in the bar
and sidebar, and its homepage and folder.

**Studio → Plugins → Develop** (`mandara studio plugins --tab develop`) is this
guide, built from your system. It shows your plugin folders resolved to real
paths, the format, the architecture with the real time budgets, the WIT
contract of your Mandara build, and copyable commands. Its **Validate**
section lists every plugin it finds with its load status and the actual
error, and can test-load a plugin. Jump to a section with `--scroll
<folders|format|architecture|contract|quick-start|validate>`. The tabs'
older names still work as deep links: `--tab local` opens Develop and
`--tab marketplace` opens Community.

Contributing to Mandara itself (built-in plugins, Studio, the shell)? See
[AGENTS.md](AGENTS.md), the contributor guide.

## Contents

1. [Quick start](#quick-start)
2. [Anatomy of a plugin](#anatomy-of-a-plugin)
3. [The API (WIT)](#the-api-wit)
4. [Views](#views)
5. [Walkthrough: the moon-phase example](#walkthrough-the-moon-phase-example)
6. [Settings, commands and permissions](#settings-commands-and-permissions)
7. [Localization](#localization)
8. [Where plugins live](#where-plugins-live)
9. [Publishing to the marketplace](#publishing-to-the-marketplace)
10. [Guidelines](#guidelines)

## Quick start

You need a running Mandara (download a release and run `bin/mandara
install`; see the [README](README.md#install)) and a Rust toolchain.

```bash
rustup target add wasm32-wasip2
git clone --depth 1 https://github.com/theHamdiz/mandara mandara-src
cp -r mandara-src/plugins/examples/moon-phase my-plugin
cp -r mandara-src/crates/mandara-plugins/wit my-plugin/wit
cd my-plugin
# The example points at the WIT inside Mandara's repository; use your copy.
sed -i 's#"../../../crates/mandara-plugins/wit"#"wit"#' src/lib.rs
sed -i 's/^name = "moon-phase"/name = "my-plugin"/' Cargo.toml
# edit plugin.toml (id, name) and src/lib.rs
cargo build --release --target wasm32-wasip2
mkdir -p ~/.local/share/mandara/plugins-dev/my-plugin
cp plugin.toml ~/.local/share/mandara/plugins-dev/my-plugin/
cp -r locales icon.png cover.png ~/.local/share/mandara/plugins-dev/my-plugin/ 2>/dev/null
cp target/wasm32-wasip2/release/my_plugin.wasm ~/.local/share/mandara/plugins-dev/my-plugin/plugin.wasm
```

The plugin appears in **Studio → Plugins** within a second: on Installed,
under *Third-party*, with a *Development* badge. Plugins in the development
folder are enabled automatically. Add it to the bar from its product page
or the bar designer (Studio → Bar), or open its panel with
`mandara msg panel open my-plugin`.

To iterate, rebuild and copy the `.wasm` again. Mandara notices the change
and reloads the plugin in place, including any of its surfaces that are
open.

## Anatomy of a plugin

An installed plugin is one folder:

```
my-plugin/
├── plugin.toml        # manifest (required)
├── plugin.wasm        # the compiled component (required)
├── locales/           # translations (optional)
│   ├── ar.toml
│   └── es.toml
├── icon.png           # 256×256 rounded-square tile: Studio cards and product page (optional)
└── cover.png          # 16:9 cover (1600×900) on top of the product page (optional)
```

Its source project is an ordinary Rust crate with the WIT contract beside
it:

```
my-plugin/
├── Cargo.toml
├── plugin.toml
├── src/lib.rs
├── wit/plugin.wit     # copied from crates/mandara-plugins/wit/
├── locales/
├── icon.png
└── cover.png
```

See [`assets/README.md`](assets/README.md) for the grid, palette and style the
built-in icons and covers use; matching it makes a plugin feel native.

### `plugin.toml`

Studio → Plugins → Develop shows an annotated `plugin.toml` with every field
and every setting type. A unit test checks it with the real parser
(`mandara_plugins::authoring::EXAMPLE_MANIFEST`). The essentials:

```toml
id = "moon-phase"            # 1–64 lowercase letters, digits, - and _; unique
name = "Moon phase"
description = "Tonight's moon: phase, illumination and days to the next full moon"
author = "Your Name"
version = "0.1.0"            # default "0.0.0"
zone = "right"               # default bar zone: left | center | right
entry = "plugin.wasm"        # the component, relative to this folder (default)
permissions = []             # ["commands"] to run Mandara commands
homepage = "https://github.com/you/mandara-moon-phase"   # "Homepage" button in Studio

# Name and description per language (ISO 639-1 codes).
[i18n.ar]
name = "طور القمر"
description = "قمر الليلة: طوره ونسبة إضاءته والأيام حتى البدر"
```

Only `id` and `name` are required. An invalid id, an empty name or an
unknown permission is reported (in the log and in Develop → Validate); the
marketplace refuses invalid ids.

### `Cargo.toml`

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.57"

[profile.release]
opt-level = "s"
lto = true
strip = true
```

## The API (WIT)

The interface is [`crates/mandara-plugins/wit/plugin.wit`](crates/mandara-plugins/wit/plugin.wit)
(package `mandara:plugin@0.1.0`). Generate the bindings from your copy of
the `wit/` folder:

```rust
wit_bindgen::generate!({ world: "plugin", path: "wit" });

use mandara::plugin::host;
use mandara::plugin::types::{Kind, Node, Tone};
// `Palette` and `View` come from the world itself (`use types.{palette, view}`).

struct MyPlugin;

impl Guest for MyPlugin {
    fn init() -> Result<(), String> { host::log("hello"); Ok(()) }
    fn bar(theme: Palette) -> Option<View> { /* … */ None }
    fn panel(theme: Palette) -> Option<View> { None }
    fn sidebar(theme: Palette) -> Option<View> { None }
    fn on_event(action: String) { /* a clickable node was clicked */ }
    fn refresh_ms() -> u32 { 60_000 } // re-render interval (0 = only after events)
}

export!(MyPlugin);
```

**Exports:** your plugin provides these.

| Function | Purpose |
|---|---|
| `init()` | Runs once after loading. Return `Err` to refuse to load. |
| `bar(theme)` | The bar item, or `None` for no bar item. Called on every render of the bar item. |
| `panel(theme)` | The panel that drops from the bar item. |
| `sidebar(theme)` | A card for the sidebar. |
| `on-event(action)` | A node with an `action` was clicked. Afterwards the bar item, panel and card all re-render. |
| `refresh-ms()` | How often to re-render while a surface is on screen, in milliseconds. |

**`refresh-ms` in detail:** `0` means "only after events" (a click
re-renders). Anything else is clamped to at least 250 ms. The interval is
read when a surface is created and only runs while that surface is drawn: a
closed panel or a hidden sidebar costs nothing.

**Host imports:** Mandara provides these.

| Function | Purpose |
|---|---|
| `log(message)` | Writes to Mandara's log, tagged with your id. |
| `now-unix-ms()` | The current time, in milliseconds since the Unix epoch. |
| `setting(key)` | Your `[plugins.settings.<id>]` value, as JSON (the declared default until the user changes it). |
| `command(line)` | Queues a Mandara command. Needs the `commands` permission. |
| `locale()` | The interface language: `en`, `ar`, `es`, `fr`, `de`, `tr`, `ru`, `hi`, `zh` or `ja`. |
| `translate(text)` | Looks `text` up in your `locales/` catalog. |

**The palette** every view call receives is the live theme as `#rrggbb`
strings: `primary`, `on-primary`, `surface`, `on-surface`,
`on-surface-variant`, `container`, `error`, `success`, `warning`, and
`dark` (whether the theme is dark). Views can't set colours directly (use
[tones](#views)), so the palette is for decisions, such as picking a
different glyph in light mode.

## Views

Views are trees of nodes stored as an arena: `nodes[root]` is the top node,
and `children` are indices into the list. Mandara renders them with its own
components, so plugins look native in every theme and in light or dark mode.

| Kind | Meaning |
|---|---|
| `row` / `column` | Lay out children horizontally / vertically, `gap` apart. |
| `card` | A rounded surface (sidebar style) stacking its children vertically. |
| `text` | A label; `size`, `bold` and `tone` style it. Text that is only emoji (pictographs with their joiners, variation selectors, skin tones and spaces, e.g. `🌕\u{FE0F}`) draws in colour from the system's emoji font, at any `size` up to 96 px. Text that mixes emoji and letters keeps the normal fonts. |
| `icon` | A bundled Mandara icon by name (`sun`, `moon`, `timer`, `music`, …; the full list is `ICON_NAMES` in [`crates/mandara-ui/src/icons.rs`](crates/mandara-ui/src/icons.rs)). Unknown names draw `circle`. |
| `button` | A pill with `text` (after any children, such as an icon); clicking sends its `action`. `tone = accent` fills it with the primary colour. |
| `meter` | A progress bar; `value` from 0 to 1, filled in the node's tone. |
| `spacer` | Flexible space. |

Node fields:

| Field | Meaning |
|---|---|
| `kind`, `text` | What the node is and its label (or icon name). |
| `value` | A meter's fill, 0 to 1. |
| `children` | Indices of child nodes. |
| `action` | Sent to `on-event` when clicked; any node with a non-empty `action` is clickable. |
| `tone` | `normal`, `muted`, `accent`, `error`, `success` or `warning`, mapped to the palette. |
| `size` | Text or icon size in logical px; `0` is the default (13 px text, 16 px icons). Clamped to 6–96. |
| `gap` | Spacing between children; a card with `0` uses 8 px. |
| `bold` | Semibold text. |
| `grow` | Take the remaining space in the parent. |

Trees deeper than 32 levels are cut off. A third-party panel opens at
360 × 380 px with 16 px of padding. One plugin instance backs the bar item,
the panel and the sidebar card, so state you keep in the plugin (statics,
atomics) is shared between them.

## Walkthrough: the moon-phase example

[`plugins/examples/moon-phase`](plugins/examples/moon-phase) is a complete
plugin: a moon icon with the lit percentage in the bar, and a panel and
sidebar card with tonight's moon in large colour emoji, three stat cards,
the whole cycle as a strip, and a toggle. Read
[`src/lib.rs`](plugins/examples/moon-phase/src/lib.rs) alongside this.

**Named sizes, not magic numbers.** Every size and gap is a constant:

```rust
const HERO_MOON_PX: f32 = 64.0;
const STAT_MOON_PX: f32 = 18.0;
const STRIP_MOON_PX: f32 = 16.0;
const STRIP_NOW_PX: f32 = 26.0;
```

**Colour emoji.** The phases are emoji ending in U+FE0F, so the colour
emoji font draws them rather than a text font's flat symbol:

```rust
const PHASE_GLYPHS: [&str; 8] = ["🌑\u{FE0F}", "🌒\u{FE0F}", "🌓\u{FE0F}", "🌔\u{FE0F}", "🌕\u{FE0F}", "🌖\u{FE0F}", "🌗\u{FE0F}", "🌘\u{FE0F}"];
```

**A tiny arena builder.** `Tree` pushes nodes and returns their index:

```rust
impl Tree {
    fn add(&mut self, kind: Kind, text: &str) -> u32 {
        self.nodes.push(Node { kind, text: text.into(), value: 0.0, children: Vec::new(), action: String::new(), tone: Tone::Normal, size: 0.0, gap: 6.0, bold: false, grow: false });
        (self.nodes.len() - 1) as u32
    }
    // node(i) -> &mut Node, children(parent, &[kids]), view(root) -> Option<View>
}
```

**Reusable pieces.** A stat card is a `card` with an emoji, a muted label
and a bold value; `grow` makes the three cards share the row equally:

```rust
fn stat(t: &mut Tree, moon: &str, label: &str, value: &str) -> u32 {
    let mark = t.add(Kind::Text, moon);
    t.node(mark).size = STAT_MOON_PX;
    let l = t.add(Kind::Text, label);
    t.node(l).tone = Tone::Muted;
    t.node(l).size = SMALL_PX;
    let v = t.add(Kind::Text, value);
    t.node(v).bold = true;
    let card = t.add(Kind::Card, "");
    t.node(card).gap = STACK_GAP;
    t.node(card).grow = true;
    t.children(card, &[mark, l, v])
}
```

**Settings arrive as JSON.** A choice is a quoted string, a switch is
`true` / `false`:

```rust
fn hemisphere() -> Hemisphere {
    match host::setting("hemisphere").as_deref() {
        Some("\"south\"") => Hemisphere::South,
        _ => Hemisphere::North,
    }
}
```

**Interaction.** The toggle is a button with an `action`; `on_event` flips
a static, and Mandara re-renders the bar item, panel and card:

```rust
static DETAILED: AtomicBool = AtomicBool::new(true);

let toggle = t.add(Kind::Button, if DETAILED.load(Ordering::Relaxed) { "Hide the cycle" } else { "Show the cycle" });
t.node(toggle).action = "toggle".into();

fn on_event(action: String) {
    if action == "toggle" {
        DETAILED.fetch_xor(true, Ordering::Relaxed);
    }
}
```

**Translation.** Plain labels such as `"Full moon"` and the button text are
translated by the host. Composed strings go through `host::translate` with
a placeholder:

```rust
host::translate("In {days} days").replace("{days}", &n.to_string())
```

**Same content, two frames.** `panel()` puts the rows from `details()` in a
`column`; `sidebar()` puts the same rows in a `card`. `refresh_ms()`
returns `60_000`: the moon changes slowly.

## Settings, commands and permissions

- **Settings:** declare them in `plugin.toml` with `[[settings]]` and
  Studio shows them, with native controls, in your plugin's **settings
  modal** (its product page → Settings, or
  `mandara studio plugins --plugin <id> --settings`). Edits apply live.
  The moon-phase example declares:

  ```toml
  [[settings]]
  key = "hemisphere"            # lowercase letters, digits, - and _
  type = "choice"               # bool | number | text | choice
  label = "Hemisphere"          # translated through locales/<code>.toml
  help = "Where you watch from: it decides which side of the moon is lit"
  default = "north"
  choices = [{ value = "north", label = "Northern" }, { value = "south", label = "Southern" }]

  [[settings]]
  key = "show_percent"
  type = "bool"
  label = "Illumination in the bar"
  help = "Show the lit percentage next to the moon icon"
  default = true
  ```

  A number with both `min` and `max` becomes a slider:

  ```toml
  [[settings]]
  key = "refresh"
  type = "number"
  label = "Refresh every"
  default = 15
  min = 5                       # min + max → a slider; otherwise a number field
  max = 60
  step = 5
  unit = " min"
  ```

  | Field | Applies to | Meaning |
  |---|---|---|
  | `key` | all | Required, at most 64 characters. The name you read with `host::setting`. |
  | `type` | all | `bool` (switch, the default), `number`, `text` or `choice`. |
  | `label`, `help` | all | Shown in Studio and translated through your catalog; the key is used when `label` is empty. |
  | `default` | all | Used until the user changes it; must match `type` (for `choice`, one of the values). Without one: `false`, `min` (or 0), `""`, or the first choice. |
  | `min`, `max`, `step`, `unit` | `number` | Slider range (both `min` and `max`), snapping step and suffix. |
  | `choices` | `choice` | Plain strings (`["north", "south"]`) or `{ value, label }` tables. |

  Invalid declarations (bad key, a choice without choices, a default of the
  wrong type, `min` ≥ `max`, a step that isn't positive, a duplicate key)
  are skipped with a warning in the log and in Develop → Validate. Values
  are stored in `~/.config/mandara/config.toml`:

  ```toml
  [plugins.settings.moon-phase]
  hemisphere = "south"
  ```

  Read them with `host::setting("hemisphere")`, which returns the JSON text
  `"south"` (the declared default when the user hasn't chosen yet). A change
  in Studio reaches your plugin before its next render.
- **Commands:** with `permissions = ["commands"]`, `host::command("theme next")`
  runs any [documented command](docs/commands.md), the same ones as
  `mandara msg …` (`mandara commands` lists them all). Commands run after
  your call returns; a failure is logged.
  At most 16 commands can be queued per call, and without the permission
  `host::command` returns an error.
  Users see the permission, with what it allows, on your plugin's product
  page in Studio.
- **No other access:** there is no filesystem, network, environment or
  subprocess access (WASI is linked but empty). Each call has a 100 ms time budget (1 s for `init`). A
  call that runs past it, or traps, stops the plugin: its surfaces show
  *plugin stopped* until it is reloaded (a rebuild or any change to its
  files, or a shell restart). Components are compiled to native code once
  and cached in `~/.cache/mandara/plugins/`. Loading is lazy: a plugin is
  instantiated the first time one of its surfaces is shown.

## Localization

Write your plugin in English. Ship translations as
`locales/<code>.toml` files containing `"English" = "Translation"` pairs:

```toml
# locales/ar.toml
"Full moon" = "بدر"
"{percent}% illuminated" = "مضاء بنسبة {percent}%"
"In {days} days" = "بعد {days} أيام"
```

- **Labels:** every `text` and `button` label in your views is translated
  automatically.
- **Composed strings:** call `host::translate` with a placeholder, then
  substitute the value:

  ```rust
  host::translate("{percent}% illuminated").replace("{percent}", &format!("{:.0}", m.illumination() * 100.0))
  ```

- **Settings:** `label`, `help` and choice labels go through the same
  catalog.
- **Name and description:** translate these in `plugin.toml` under
  `[i18n.<code>]`.
- **Fallback:** anything without a translation stays English, which is the
  fallback for every language.
- **RTL:** when `host::locale() == "ar"` the interface is right-to-left.

Mandara itself is translated into English, العربية, Español, Français,
Deutsch, Türkçe, Русский, हिन्दी, 简体中文 and 日本語. Each plugin decides which
of these it supports.

## Where plugins live

| Folder | Origin | Default |
|---|---|---|
| `~/.local/share/mandara/plugins-dev/` | your own, in development | enabled |
| `~/.local/share/mandara/plugins/` | installed (marketplace or by hand) | opt-in (a marketplace install switches it on) |
| `$XDG_DATA_DIRS/mandara/plugins/` (e.g. `/usr/share/mandara/plugins/`) | distribution packages | opt-in |

Mandara's built-in plugins are composable: a calm default set is on (the bar
essentials, quick settings, media, weather, wallpaper, screenshot and
clipboard) and the rest ship opt-in (`PluginManifest::opt_in`): the dock,
mini stats, screen time, prayer times, the wallpaper generator, Wallhaven,
cursor themes, containers, services, drives, updates, the process hunter,
the authenticator, soundscapes, world clock, network speed, the color
picker, Pomodoro, notes, to-do, privacy and the app installer. A config
written before a default changed keeps what it had on: `[plugins]
defaults_version` records the defaults a config was written for, and
`mandara_config::upgrade` writes the plugins an older config had on into
`enabled`. An opt-in plugin's state can be read without the registry with
`PluginsConfig::opted_in(id)` (the compositor emitters use it for
`cursors`).
First-party plugins without a bar item work the same way, for example the
**App installer** (`app-installer`): it installs apps dropped on the dock or
Studio (AppImage, Flatpak, portable archives, native packages through the
system package manager), with its settings in its Studio modal and the
`apps install` / `apps jobs` commands; other surfaces call its Rust API
(`mandara_ui::plugins::app_installer::{accepts, install, queue}`). The
**Wallpaper generator** (`wallpaper-generator`) is another: procedural
wallpapers and AV1 loops from `mandara-wallgen`, designed in Studio →
Wallpaper, with a Generate button in the wallpaper picker, an opt-in
schedule and the `wallgen …` commands; other surfaces call
`mandara_ui::plugins::wallpaper_generator::{generate, generate_from_picker, wallgen}`.

Any plugin can be switched off (`plugins disable <id>`), which stops its bar
item, sidebar card, launcher entries, commands (they answer that the plugin
is off) and background work. A built-in can also be removed
(`plugins remove <id>`): off, out of the bar and sidebar layouts, and listed
under *Removed Mandara plugins* in Studio until `plugins restore <id>` puts
it back where it was. The launcher, the session menu and the main menu are
core and refuse both. Config: `[plugins] enabled`, `disabled`, `removed`
(+ `removed_places`).

If the same id exists in several folders, the first one in this table wins.
An id that clashes with a built-in plugin is skipped with a warning.
Changes are picked up about half a second after the last write, by the
shell and by an open Studio window alike, with no restart.

To see why a plugin doesn't show up, open **Studio → Plugins → Develop →
Validate** (`mandara studio plugins --tab develop --scroll validate`). It
shows each folder Mandara found and its status: running, off, failed to load
(with the error), stopped after a trap, invalid manifest, component missing,
shadowed by another folder, or clashing with a built-in. **Test load** runs
the plugin once in Studio's own sandbox.

Useful commands:

```bash
mandara msg plugins list             # every plugin: origin, enabled, removed / essential, runtime (running / failed / disabled + error, or not-loaded)
mandara msg plugins reload           # rescan now: adds, reloads changed and drops deleted plugins (normally automatic)
mandara msg plugins enable my-plugin # disable <id> turns it off again
mandara msg panel open my-plugin     # open its panel
```

## Publishing to the marketplace

The marketplace is free and runs on GitHub. There is no account to create
and no review queue.

1. **Make a public repository**, for example `you/mandara-moon-phase`, with
   the plugin's source (including its `wit/` folder).
2. **Add the topic** `mandara-plugin` (repository → About → ⚙ → Topics).
3. **Package a release.** Attach an asset named `<id>.mandara.tar.gz`
   containing your plugin folder:

   ```bash
   cargo build --release --target wasm32-wasip2
   mkdir -p dist/moon-phase
   cp plugin.toml dist/moon-phase/
   cp target/wasm32-wasip2/release/moon_phase.wasm dist/moon-phase/plugin.wasm
   cp -r locales icon.png cover.png dist/moon-phase/ 2>/dev/null || true
   tar -czf moon-phase.mandara.tar.gz -C dist moon-phase
   gh release create v0.1.0 moon-phase.mandara.tar.gz --title "v0.1.0" --notes "First release"
   ```

4. **That's it.** Your plugin shows up in **Studio → Plugins → Community**
   (and in Discover's *New & updated* and *Community picks* shelves),
   ranked by stars. Users can install, update or remove it with one click.
   Installing switches it on (it's added to `plugins.enabled`) and it loads
   live, without restarting the shell.

The topic is the only filter: forks and archived repositories are listed
too (archived ones carry a badge). A repository without a usable release is
still listed; installing it says *No release yet* (or *No plugin file yet*
when releases exist but none has the archive) and offers *Check again*.

- **Which release:** the latest release if it has the archive, otherwise
  the newest published release that does, pre-releases included. Drafts
  are never used.
- **Which file:** any asset ending in `.mandara.tar.gz` (any case). If a
  release has several, the one named after the repository wins
  (`mandara-moon-phase` or `moon-phase`). The plugin id always comes from
  `plugin.toml`, not from the file name.
- **Archive layout:** `plugin.toml` at the root of the archive or inside
  one top-level folder.

Archives are checked when installed. Absolute paths and paths that escape
the plugin folder are rejected, links and device files are skipped,
archives are limited to 64 MB, and the id must be valid. The new version is
staged and swapped into `~/.local/share/mandara/plugins/<id>/` in one
rename. Releases are fetched over HTTPS from GitHub's API.

Searches are anonymous. GitHub allows ten a minute, so Studio reuses an
answer for a minute and fetches at most 300 results (it asks you to narrow
the search beyond that). When the limit is hit, Studio says how many
seconds to wait. Offline mode (Studio → Privacy) blocks the marketplace
entirely.

To test your release flow without publishing, point Mandara at a mirror
or mock of the API with `MANDARA_MARKETPLACE_API` (default
`https://api.github.com`), for example
`MANDARA_MARKETPLACE_API=http://127.0.0.1:8080 mandara studio plugins --tab community`.
It needs `/search/repositories`, `/repos/<owner>/<repo>/releases/latest`
and `/repos/<owner>/<repo>/releases`. `crates/mandara-plugins/tests/marketplace.rs`
does this end to end.

### Before you publish: compliance check

Every release is checked on the user's machine before it installs or
updates, in the real sandbox with the normal time budgets (it takes well
under a second). Run the same check on your folder first:

```bash
mandara msg plugins vet ./dist/moon-phase   # {"ok":true}, or {"ok":false,"class":…,"reason":…}
```

A release that fails is classed as one of three:

- **Broken:** `plugin.toml` doesn't parse or has problems, `plugin.wasm`
  is missing, isn't a `wasm32-wasip2` component or doesn't compile or
  start, or `init` or a render traps or runs past its time budget.
- **Malicious:** its id or name is a built-in plugin's, it imports
  interfaces beyond the plugin world and the empty WASI the host links, or
  it queues commands while loading or rendering, before any user event.
- **Placeholder:** it's the unchanged template (`my-plugin`) or the
  moon-phase example republished as it is, or its bar item, panel and
  sidebar card all show nothing.

A failing release doesn't install. Studio flags that version with the
reason ("This version (v1.2) isn't recommended: …") and offers *Install
anyway* (with a second confirmation for a malicious one); an update that
fails isn't applied and the installed version keeps running. The flag
belongs to that release only: your next release is checked afresh, and
when it passes the warning disappears.

### Automating releases (GitHub Actions)

```yaml
# .github/workflows/release.yml
on: { push: { tags: ["v*"] } }
jobs:
  release:
    runs-on: ubuntu-latest
    permissions: { contents: write }
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: wasm32-wasip2 }
      - run: cargo build --release --target wasm32-wasip2
      - run: |
          ID=$(grep '^id' plugin.toml | cut -d'"' -f2)
          mkdir -p dist/$ID && cp plugin.toml dist/$ID/
          cp target/wasm32-wasip2/release/*.wasm dist/$ID/plugin.wasm
          cp -r locales icon.png cover.png dist/$ID/ 2>/dev/null || true
          tar -czf $ID.mandara.tar.gz -C dist $ID
      # Optional: delta updates. Users on the previous version download a
      # small patch for plugin.wasm instead of the whole archive (Mandara
      # falls back to the archive whenever the patch can't be used).
      - name: Patch from the previous release
        env: { GH_TOKEN: "${{ github.token }}" }
        run: |
          cargo install --locked --git https://github.com/theHamdiz/mandara mandara-delta
          ID=$(grep '^id' plugin.toml | cut -d'"' -f2)
          PREV=$(gh release list --exclude-drafts --json tagName --jq '.[].tagName' | grep -vxF "$GITHUB_REF_NAME" | head -n 1 || true)
          mkdir -p previous
          [ -n "$PREV" ] && gh release download "$PREV" --pattern "$ID.mandara.tar.gz" --dir previous || true
          mandara-delta plugin --new $ID.mandara.tar.gz $( [ -f previous/$ID.mandara.tar.gz ] && echo --old previous/$ID.mandara.tar.gz ) --out delta
          (cd delta && sha256sum * ../$ID.mandara.tar.gz | sed 's#\.\./##' > SHA256SUMS)
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            *.mandara.tar.gz
            delta/*
```

The delta step writes `update.json` (the archive's and `plugin.wasm`'s
SHA-256s, plus the patch from the previous release), the patch
`<id>-<old>-to-<new>.patch.zst`, a small `<id>-<new>.meta.tar.gz` (every
file but `plugin.wasm`, so the manifest, icons and locales stay current)
and `SHA256SUMS`. `version` in `plugin.toml` names the versions. Releases
without these still update: Mandara downloads the archive (checked against
`SHA256SUMS` when present). To try it locally, run `mandara-delta plugin
--new new.mandara.tar.gz --old old.mandara.tar.gz --out delta` (or `cargo
xtask plugin-delta` in Mandara's repository).

### Updates

Mandara checks installed marketplace plugins with its own update check:
Studio → Plugins → **Updates**, the *N plugin updates · Update all* banner
on Installed and the About update card (**Include marketplace plugins**),
or the command line:

```bash
mandara update plugins                       # installed marketplace plugins with a newer release
mandara update plugins apply [ID] [--full] [--dry-run]
mandara update plugins rollback ID           # put the previous version back
```

It compares the release tag with the installed one (semver when both are),
updates by patch first and the archive as the fallback, swaps the folder
atomically and keeps the previous one. The shell hot-reloads the plugin and
its enabled state is kept.

## Guidelines

- **Be light.** Render small trees, prefer `refresh-ms` of 1000 ms or more,
  and do no heavy work in `bar()`, which runs on every render.
- **Be native.** Use tones, not hard-coded colors. Mandara themes your plugin
  for every palette, light or dark, with wallpaper colors.
- **Name your numbers.** Keep sizes and gaps in named constants, as the
  example does, so the layout is easy to tune.
- **Be clear.** Use short labels, and give sentences one placeholder per
  value.
- **Translate.** Ship at least the languages you can review; English is the
  fallback.
- **Be honest.** Never show made-up data as real (for example, fake
  visualizer bars).
- **Ask for little.** Request `commands` only if the plugin really runs
  Mandara commands; users see every permission before they install.
