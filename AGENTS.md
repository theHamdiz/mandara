# Contributing to Mandara Shell

Welcome! This is the guide for everyone who works on Mandara's code: people
and AI coding agents alike. It explains what Mandara is, how the repository
is laid out, how to build and check your work, the rules every change
follows, and how to add the common kinds of features. If you only want to
write a third-party plugin, [PLUGINS.md](PLUGINS.md) is all you need.

## Contents

1. [What Mandara is](#what-mandara-is)
2. [Repository map](#repository-map)
3. [Build, test and check](#build-test-and-check)
4. [Driving and inspecting the shell from the CLI](#driving-and-inspecting-the-shell-from-the-cli)
5. [Hard rules](#hard-rules)
6. [Design principles](#design-principles)
7. [Architecture notes and gotchas](#architecture-notes-and-gotchas)
8. [How to add things](#how-to-add-things)
9. [Writing a third-party WebAssembly plugin](#writing-a-third-party-webassembly-plugin)
10. [Keep the docs in sync](#keep-the-docs-in-sync)
11. [Commits and pull requests](#commits-and-pull-requests)

## What Mandara is

Mandara is a Wayland desktop shell for **MangoWM, Niri and Hyprland**: the
bar, panels, sidebar, dock, launcher, notifications, on-screen displays,
wallpaper engine, lock screen, login manager and more, shipped as **one
native Rust binary** (`mandara`) built on [GPUI](https://github.com/zed-industries/zed),
Zed's GPU-accelerated UI framework.

- **One shell process, many surfaces.** Each output gets a full-size
  `zwlr_layer_shell_v1` surface for the bar, plugin panels and sidebar,
  plus exclusion strips and wallpaper layers. Islands and panels morph
  inside that surface with spring physics, and the input region follows
  what is drawn so clicks on transparent pixels reach the windows below.
- **Mandara Studio** (`mandara studio`, `app_id = org.mandara.studio`) is
  the settings and design app, an `xdg_toplevel` running as **its own
  process**. Every edit is mirrored to the running shell over IPC, so it
  previews live, and Studio's CPU and memory never count against the
  shell's.
- **Everything is a plugin.** Bar items, panels and sidebar cards are
  plugins: first-party ones compiled into the shell, and third-party ones
  written in Rust, compiled to WebAssembly and run sandboxed.
- **Every compositor is first-class.** MangoWM's bitmask tags and per-tag
  layouts and Niri's and Hyprland's workspaces are modelled natively, not
  squeezed into a lowest common denominator.

## Repository map

| Path | What lives there |
|---|---|
| `crates/mandara-app` | The `mandara` binary: CLI (`msg`, `commands`, `studio`, `setup`, `update`, `install`, …), the restart supervisor, IPC and app bootstrap. |
| `crates/mandara-core` | GPUI layer-shell surface specs, the analytical spring solver, OKLCH tweening, frame-synced input-region masking, the chromatic-glass material, output geometry and the software (`wl_shm`) canvas. |
| `crates/mandara-config` | The TOML schema and defaults, the reactive `ConfigStore` with its file watcher, XDG paths, and the non-destructive MangoWM / Niri / Hyprland emitters and conflict scanner. |
| `crates/mandara-theme` | The color engine: `Color` (OKLCH, gamut mapping, WCAG contrast), `MandaraPalette` tokens, HCT wallpaper quantization, the theme catalog (`themes/*.toml`) and app theme templates. |
| `crates/mandara-compositor` | The `CompositorBackend` trait, the navigation model, and the MangoWM, Niri and Hyprland IPC drivers. |
| `crates/mandara-services` | D-Bus and system service actors (MPRIS, notifications, tray, UPower, NetworkManager, polkit, audio, keyrings, …), the marketplace client and the self-updater. |
| `crates/mandara-plugins` | The plugin runtime: manifests, the registry, `CommandSpec` and the command catalog, plugin lifecycle, the wasmtime host and the WIT contract (`wit/plugin.wit`). |
| `crates/mandara-ui` | The shell's views: bar, panels, sidebar, launcher, OSDs, lock screen, desktop and backdrop, the UI kit and every first-party plugin (`src/plugins/`). |
| `crates/mandara-studio` | Mandara Studio: pages, the declarative settings registry, design tokens and the plugin store. |
| `crates/mandara-greeter` | Mandara Login, the greetd greeter (see [docs/login-manager.md](docs/login-manager.md)). |
| `crates/mandara-media` | The pure-Rust video pipeline: WebM / IVF demuxing, AV1 decoding (rav1d), YUV conversion straight into caller buffers, the greeter's player, the desktop's frame stream and first / representative frames. No GUI types. |
| `crates/mandara-wallgen` | The procedural wallpaper engine: twelve resolution-independent styles with generic parameter specs, OKLab palettes, recipes (TOML sidecars), deterministic row-parallel rendering, a compact lossless WebP (VP8L) encoder and seamless AV1 video loops through the recorder's `VideoSink`. No GUI types. The `wallpaper-generator` plugin (`crates/mandara-ui/src/plugins/wallpaper_generator.rs` and its folder: jobs, schedule, CLI) and Studio's generator (`crates/mandara-studio/src/app/wallgen_page.rs`) build on it. |
| `crates/mandara-i18n` | Localization: English-keyed catalogs for ten languages and the coverage tests. |
| `crates/mandara-install` | `mandara install` / `uninstall`: per-distribution package tables and the package-manager abstraction. |
| `crates/mandara-version` | Build identity: component versions, content hashes, git revision, source fingerprint. |
| `vendor/gpui_wgpu` | GPUI's wgpu renderer, vendored with Mandara's patches (GPU memory, fonts, colour emoji, Vulkan first, rounded clipping). Every change is listed in [`MANDARA-PATCHES.md`](vendor/gpui_wgpu/MANDARA-PATCHES.md) and marked `Mandara patch` in the source. |
| `vendor/gpui` | GPUI core, vendored for rounded content masks (a rounded element whose overflow is hidden or clipped clips its children to its rounded shape) and for `.hover()` on elements without an `.id()`, which upstream only repaints when something else re-renders the view ([`MANDARA-PATCHES.md`](vendor/gpui/MANDARA-PATCHES.md)). |
| `xtask` | `cargo xtask`: versioning, fingerprinted builds, packaging, artwork, delta updates, the resource budget and the demo sandbox. |
| `tools/mandara-delta` | The delta-update format and CLI, outside the workspace so plugin authors can `cargo install` it alone. |
| `plugins/examples` | Example third-party plugins (`moon-phase`), built on their own for `wasm32-wasip2`. |
| `docs` | Guides: [Studio](docs/studio.md), [commands](docs/commands.md), [configuration](docs/configuration.md), [login manager](docs/login-manager.md), [releasing](docs/releasing.md), [development](docs/development.md), and `screenshots/`. |
| `assets` | Brand marks, plugin icons and covers, `catalog.json` and the artwork index ([`assets/README.md`](assets/README.md)). |
| `packaging` | The desktop entry and icon, the PAM file, the Arch `PKGBUILD` template and AUR recipes. |
| `.github/workflows` | CI (`ci.yml`) and the release pipeline (`release.yml`). |

## Build, test and check

Mandara builds with stable Rust on Linux. Besides a C toolchain and
`pkg-config`, it needs the development files for xkbcommon, Wayland,
Vulkan, fontconfig and freetype (CI installs `build-essential pkg-config
cmake libxkbcommon-dev libwayland-dev libvulkan-dev libfontconfig-dev
libfreetype-dev`).

```bash
cargo check --workspace --all-targets        # fast type check of everything, tests included
cargo test --workspace                       # unit, integration and doc tests
cargo clippy --workspace --all-targets       # lints (CI reports them)
cargo build --release -p mandara-app         # the shipped binary → target/release/mandara
```

Release builds use fat LTO and one codegen unit (`[profile.release]` in
`Cargo.toml`), so they take a while. Aliases in `.cargo/config.toml`:
`cargo dist` (the same release build), `cargo native` (tuned for your CPU),
`cargo profiling` (release code with symbols, for `perf` or `samply`).
Release builds **unwind** on panic: the plugin guard and the PAM
conversation rely on `catch_unwind`, so never switch them to `abort`.

`cargo xtask` runs the project's tooling (all Rust, no scripts):

| Command | What it does |
|---|---|
| `cargo xtask status` | Every component's version and whether its content changed since. |
| `cargo xtask bump --dry-run` | What a release would bump (CI does the real bump). |
| `cargo xtask verify` | Fails if a component changed without a bump. |
| `cargo xtask build` | A fingerprinted release build, its manifest and the README's build-fingerprint block. |
| `cargo xtask plugins` | Builds the example plugins and packages them in marketplace format. |
| `cargo xtask art [--check]` | Regenerates (or validates) the plugin artwork embedded in the binary. |
| `cargo xtask resource-check target/release/mandara` | Runs a release binary in a throw-away headless session and checks its idle memory, threads and CPU against the budget. |
| `cargo xtask sandbox up` / `shot` / `msg` / `studio` / `down` | A private headless demo desktop for screenshots and end-to-end tests ([docs/development.md](docs/development.md)). |

Checks that guard the codebase and fail `cargo test` when you break a rule:

- **Translations:** `cargo test -p mandara-i18n` fails when a string the
  code asks for is missing in any language. After adding strings, run
  `MANDARA_I18N_DUMP=1 cargo test -p mandara-i18n` to refresh
  `crates/mandara-i18n/locales/keys.txt`, then translate the new keys.
- **No hard-coded values:** `crates/mandara-ui/tests/no_hardcoded.rs` scans
  the shell, Studio, greeter and service sources for bare `px(<number>)`,
  `Duration::from_millis(<number>)` and `Duration::from_secs(<number>)`.
  The few remaining sites are listed per file with a reason in
  `crates/mandara-ui/tests/literal_allowlist.tsv`, and the counts must
  match exactly: it is a ratchet. A new literal fails; a removed one asks
  you to lower the count (`MANDARA_LITERALS_DUMP=1` rewrites the counts and
  keeps the reasons).
- **Startup budget:** `crates/mandara-ui/tests/startup_budget.rs` keeps
  registering every built-in plugin a cheap table of commands and views.
- **Plugin guide:** `mandara-plugins`' `authoring` tests check that
  PLUGINS.md states the real time budgets, limits and exports.
- **Artwork:** `cargo test -p xtask` validates the brand marks, plugin
  icons and covers, `assets/catalog.json`, the links in
  `assets/README.md` and the embedded art.
- **Compositor configs:** the emitter tests feed generated configs to each
  compositor's own checker when it is installed.

## Driving and inspecting the shell from the CLI

Mandara is agent-ready: almost everything the UI can do is a documented
command on the running shell, so you can drive it, inspect it and verify a
change without clicking.

```bash
mandara commands                     # every command, its arguments and examples
mandara commands --json              # the same catalog, machine-readable
mandara msg <command> [args]         # run one against the running shell
mandara msg --json <command> [args]  # reply as JSON: {"ok": …, "data": …} or {"ok": false, "error": …}
```

Useful starting points:

```bash
mandara msg ping                          # is the shell up?
mandara msg --json shell state            # what is open: panel, sidebar, launcher, menus
mandara msg config get /bar/height        # read a setting by JSON pointer (no argument: the whole config)
mandara msg config set /bar/height 30     # change one setting live (the value is JSON; bare words are strings)
mandara msg --json plugins list           # every plugin, enabled or not, and third-party load errors
mandara msg panel open media              # open a plugin's panel
mandara msg diagnostics frames            # frame timings, for performance work
mandara studio plugins --plugin weather --settings   # open Studio on a page or a plugin's settings
```

The catalog is generated from the code (`CommandSpec`s in the plugin
registry), so it is always complete; [docs/commands.md](docs/commands.md)
is a guided tour by area. Users can also chain commands into named
launcher *skills*, run them from slash commands and attach them to hooks on
desktop events, all through the same commands.

**Verify safely.** Never test against someone's real session:

- Use the demo sandbox: `cargo xtask sandbox up`, then
  `cargo xtask sandbox msg <command>`, `cargo xtask sandbox pointer click X Y`
  and `cargo xtask sandbox shot out.png`. Its buses, runtime dir and home
  are private and its system bus is dead, so nothing reaches the real
  NetworkManager, logind or BlueZ.
- Prefer dry runs and read-only modes: `mandara install --dry-run`,
  `mandara uninstall --dry-run`, `mandara setup --check`,
  `mandara update apply --dry-run`, `cargo xtask bump --dry-run`.
- To test config migration, point `XDG_CONFIG_HOME` at a scratch folder
  and set `MANDARA_NO_IPC=1` so `mandara setup` doesn't talk to a running
  shell.

## Hard rules

These hold for every change. Reviews check them, and several are enforced
by tests.

1. **100 % Rust.** No shell, Python or other scripts in the repository:
   tooling goes in `cargo xtask`, the installer is the binary itself
   (`mandara install`), and plugins are Rust compiled to WebAssembly. One
   toolchain builds, tests and reviews everything; nothing depends on an
   interpreter being present or behaving the same on every distribution;
   and every tool is covered by `cargo test`. (CI workflow steps are the
   only shell, and they only call cargo and packaging tools.)
2. **Every user-visible string in every language.** Wrap text in
   `mandara_i18n::t` (static), `tf` (with `{placeholders}`) or `tr`
   (runtime keys), or mark it with `tk` for a later `tr`. English is the
   key; add the translation to all nine catalogs in
   `crates/mandara-i18n/locales/`: `ar`, `de`, `es`, `fr`, `hi`, `ja`,
   `ru`, `tr` and `zh`. Keep placeholders intact, and one placeholder per
   value.
3. **No magic numbers.** Anything a user could notice (sizes, spacing,
   radii, opacity, colours, delays, refresh intervals, thresholds) is a
   setting with a default and a bounded range. What stays fixed is a named
   `const` with a doc comment saying why. Use the design tokens
   (`mandara_ui::kit::tokens` and `text`, `crates/mandara-studio/src/tokens.rs`)
   and palette roles, never raw hex colours.
4. **Non-destructive dotfiles.** Mandara never rewrites a user's
   compositor config. Rules, binds and border colours go to generated
   drop-ins in `~/.config/mandara/generated/`, and the user's main file
   gains one include line. Any edit to a user file takes a timestamped
   backup first, is validated with the compositor's own checker
   (`mango -p`, `niri validate`, `Hyprland --verify-config`) and rolls
   back on failure. Binds that collide with the user's are skipped and
   reported, never overridden.
5. **Never change a user's config unless they asked.** Defaults apply to
   new installs; existing settings, files and enabled plugins stay as the
   user left them.
6. **Resource discipline, but speed wins.** Nothing resident while idle:
   no timers or polling while a surface is hidden, load data when a plugin
   is first shown (not when it registers), decode images at the size they
   are drawn, release what is only needed to paint. But never trade speed
   for memory: an optimization that slows frames, view builds, startup or
   anything a user triggers is reverted. Measure release builds before and
   after (`cargo xtask resource-check`, `mandara msg diagnostics frames`)
   before claiming a win.
7. **Honest numbers.** Performance and memory figures in the docs come from
   measured release builds and are written as rounded ceilings ("under
   100 MiB"), never guessed, inflated or trimmed.
8. **Screenshots from the demo sandbox only.** Docs images come from
   `cargo xtask sandbox` with demo data: no other apps behind Mandara, no
   real notifications, and no personal data (names, networks, IP
   addresses, locations, project names).
9. **Develop and test in isolation.** Debug builds, synthetic input and
   experiments run only in a sandbox with its own Wayland display, runtime
   dir, session bus and a dead system bus. Never run a debug build or send
   synthetic clicks and keys to a user's real session, and never send
   commands to their real media player, network or power controls. Tools
   that act on a sandbox check its runtime dir and marker first.

## Design principles

Mandara is design-first: it should never look like an unstyled toolkit.

- **Chromatic glass.** Islands, pills and cards are glass surfaces with a
  1 px directional specular rim (bright top-left fading into a soft
  bottom-right shadow) and smooth, generous radii. Draw visible edges with
  SDF quads (GPUI rounded rects and shadows), not tessellated paths. The
  material lives in `crates/mandara-core/src/material.rs`.
- **Perceptual colour.** Palettes come from the HCT engine (wallpaper
  extraction or a theme's seeds) with guaranteed contrast; every runtime
  blend, gradient and transition is interpolated in **OKLCH** so it never
  passes through muddy greys. Surfaces use the semantic tokens of
  `MandaraPalette` (`surface_*`, `primary`, `on_surface`,
  `urgent_coral`, `success_emerald`, `warning_amber`, …).
- **Themes and Looks.** Themes are token files in
  `crates/mandara-theme/themes/`, expanded with contrast guarantees. A
  Look sets the bar style, desktop cards, palette, typography and shape
  scale together, and restoring brings back the user's own setup. Light
  mode is first-class: check both.
- **Spring motion.** Animate with `SpringValue` (an analytical damped
  oscillator: exact at any frame rate, and an interrupted animation keeps
  its velocity), driven by `request_animation_frame` only while something
  moves. Respect the **reduce motion** setting and the global spring and
  speed settings.
- **Typography.** Tabular numerals (`fonts::tabular_nums`) on clocks,
  percentages and counters so widths never jitter. Latin and Arabic are
  both first-class (Inter, JetBrains Mono, Alexandria, Cairo).
- **Right to left.** In Arabic the interface mirrors. Use the `Bidi`
  helpers (`crates/mandara-ui/src/bidi.rs`): `row` for reading order,
  `ltr_row` for things that stay left to right (times, key chords, media
  transport, charts), and `*_start` / `*_end` instead of left and right.
- **Accessible.** Keep text contrast within WCAG targets (`Color` can
  measure it), make everything reachable by keyboard, keep focus inside
  open modals and close them on Esc.
- **Locked, with a reason.** A setting that doesn't apply right now (the
  wrong compositor, a feature another setting overrides) is shown locked
  with a sentence saying why, through `registry::locked`, never silently
  ignored or hidden.
- **Settings live in Studio.** Bar dropdowns and panels show real value
  (media, calendar, toggles); configuration belongs in Studio.
- **Real data only.** Never show made-up values as real, such as fake
  visualizer bars or placeholder stats.

## Architecture notes and gotchas

Things that cost real debugging time:

- **The input-masking law.** Layer surfaces are allocated large and
  transparent so islands can morph without resizing the surface (resizing
  makes compositors jitter). Interactive containers register their
  *animated* bounds every frame with `InputMaskRegistry`, which diffs the
  union and calls `wl_surface.set_input_region` only when it changed. A new
  interactive surface that forgets to register swallows clicks or lets
  them through.
- **Size surfaces from the compositor's outputs.** GPUI's display bounds
  are wrong under fractional scaling; take logical output sizes from
  `mandara_core::query_outputs` (`zxdg_output_manager_v1`). Never request
  a layer surface with width or height 0: it triggers a viewport protocol
  error. Set `inactive_frame_interval: None` on layer surfaces, or GPUI
  throttles them to 30 fps.
- **The desktop background isn't a GPUI window.** `crates/mandara-ui/src/backdrop`
  paints it in software into `wl_shm` subsurface planes
  (`mandara_core::shm_canvas`) on its own Wayland connection and thread.
  Subsurface positions are whole logical pixels, so snap plane rectangles
  to `shm_canvas::grid_step` or text blurs.
- **Look plugin instances up per render.** Hot reload replaces a third-party
  plugin's instance; views fetch it on every render
  (`plugins/external.rs`, `instance()`) so open surfaces switch to the new
  build. Don't capture an instance in a closure once.
- **Never block the UI thread.** Service actors run on
  `mandara-services`' shared runtime and talk to the UI only through
  channels; file I/O and decoding go to background tasks.
- **Rounded clipping.** A rounded element whose overflow is not visible
  clips its children to its rounded shape (vendored GPUI's rounded content
  masks), so give any rounded card that holds covers, art, bands or
  edge-to-edge hover fills `.clip_rounded()` (`mandara_ui::style::ClipRounded`):
  it clips without changing layout, whereas `overflow_hidden` also lets a
  flex item shrink below its content. The shared card helpers already do.
- **Focus and dismissal.** Clicking a layer surface can make the compositor
  send keyboard leave and enter *before* the press, so never dismiss a
  panel synchronously on deactivation. Elements over a click-outside
  catcher must `.occlude()`.
- **Sliders.** Build every slider on `kit::slider` or `panel_fx::big_slider`
  (both use `crate::slider_model`). Their `on_change` returns the write's
  completion: a service request's reply receiver, or `None` for a local value.
  That lets the model keep one write in flight per slider, draw the dragged
  value instead of the server's late echoes, and keep following the pointer
  outside the track. Read readouts next to a slider through
  `slider_model::held` so they move with the fill. Never fire a service
  command for every pointer event: a slow sound server falls behind, and the
  bounded queue drops the newest values.
- **Blur halos.** MangoWM blurs layer surfaces wherever alpha is above 0,
  so wide translucent shadows become halos; keep shadows tight.
- **Killing processes.** Never `pkill -f "mandara studio"` (or any pattern
  that also matches your own shell's command line); match the binary path
  or use a recorded PID. GUI processes started from a tool shell die with
  it; detach them (`setsid -f`).
- **Sandbox paths.** A Wayland socket path must stay under 108 bytes, so
  keep a sandbox's `XDG_RUNTIME_DIR` short.

## How to add things

### A built-in plugin

1. Add a module under `crates/mandara-ui/src/plugins/` with a `pub const ID`
   and a `register(r: &mut PluginRegistry)` that calls
   `r.register(PluginManifest::builtin(ID, tk("Name"), tk("What it does"), Some(Zone::Right)), |window, cx, panel_cx| PluginInstance { … })`.
   Use `.opt_in()` for niche or faith-specific plugins.
2. Call it from `register_builtin` in `crates/mandara-ui/src/plugins/mod.rs`.
   Registration must stay cheap (the startup budget test): load data when
   the plugin is first shown.
3. Put it in a store category (`PLUGIN_CATEGORIES` in
   `crates/mandara-studio/src/app/store_page.rs`).
4. Add its art in `assets/plugins/<id>/` (`icon.svg`, a 256 × 256
   `icon.png`, a 1600 × 900 `cover.png`), an entry in `assets/catalog.json`
   and a row in `assets/README.md`, then run `cargo xtask art` and
   `cargo test -p xtask`.
5. Give its options settings (below), its actions commands (below), its
   strings translations, and add it to the README's plugin table and count.

### A Studio setting

Settings are data in the declarative registry
(`crates/mandara-studio/src/registry.rs` and `registry/*.rs`): one entry
names the section, group, JSON-pointer config path, label, help and
control (`Toggle`, `Slider`, `Choice`, `Text`, …), and one generic engine
reads and writes the config. Studio's settings audit checks that every
control is read by the shell. Follow
[Adding a setting](docs/configuration.md#adding-a-setting).

### A config key

Add the field to the schema in `crates/mandara-config/src/` with its
default in the nearby `defaults` module and a named range; read it from
the config snapshot where it is used, clamped to the range, so it applies
live. Document it in [docs/configuration.md](docs/configuration.md), and
add a Studio control.

### A compositor feature

Implement it for **all three** backends (`crates/mandara-compositor/src/`
`mango.rs`, `niri.rs`, `hyprland.rs`, behind `CompositorBackend` in
`backend.rs`), or lock it in Studio with a reason where a compositor can't
do it. Generated config goes through the emitters in
`crates/mandara-config/src/compositor.rs` (`emit_mango`, `emit_niri`,
`emit_hyprland`), with unit tests and, where the compositor is installed,
its own checker (`validator_tests.rs`). Window-rule options one compositor
lacks are gated with `rule_option_supported`, and include lines follow the
installed version (`installed_version`).

### A command

Register a `CommandSpec` with the plugin that owns it (`"core"` for shell
verbs). This is how `diagnostics frames` is registered:

```rust
r.command("core", CommandSpec::new("diagnostics frames", mandara_i18n::tk("Recent shell frame timings with p50 / p95 / p99 (build time and frame interval)"), |_, _| Ok(frames_report())).example("diagnostics frames"));
```

Add arguments with `.arg(ArgSpec::required(…))` / `ArgSpec::optional(…)`
and examples with `.example(…)`; replies are JSON. It then shows up in
`mandara commands`, the launcher and plugin `host::command`. Add it to
[docs/commands.md](docs/commands.md) in its area.

### A translated string

Write the English text in a `t`, `tf`, `tr` or `tk` call, run
`MANDARA_I18N_DUMP=1 cargo test -p mandara-i18n`, add the translation to
each of the nine catalogs in `crates/mandara-i18n/locales/`, and run
`cargo test -p mandara-i18n` until it passes.

## Writing a third-party WebAssembly plugin

Third-party plugins are Rust crates compiled to a WebAssembly component
against [`crates/mandara-plugins/wit/plugin.wit`](crates/mandara-plugins/wit/plugin.wit).
They return declarative view trees (rows, columns, cards, text, icons,
buttons, meters) that Mandara renders natively in every theme, and they
run sandboxed: no files, network, environment or subprocesses, a time
budget per call, and a trap disables only that plugin.

```bash
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
# copy plugin.toml, locales/, icons and the .wasm (as plugin.wasm) to
# ~/.local/share/mandara/plugins-dev/<id>/ — it hot-reloads
```

Start from [`plugins/examples/moon-phase`](plugins/examples/moon-phase). Studio →
Plugins → Develop (`mandara studio plugins --tab develop`) shows your
folders, the contract of your build and a validator. Publishing is a
public GitHub repository with the `mandara-plugin` topic and a release
asset named `<id>.mandara.tar.gz`.

**Before you publish: compliance check.** Installs and updates run
`mandara_plugins::vet` on every release, and a failing one is refused
(the user can still install it anyway; the warning stays on that version
only). Run it yourself with `mandara msg plugins vet <dir>` (JSON verdict).
It sorts failures into three classes: **Broken** (bad manifest, missing or
non-component `plugin.wasm`, a trap or time-budget overrun in `init` or a
render), **Malicious** (poses as a built-in id or name, imports beyond the
plugin world and the empty WASI, or queues commands before any user event)
and **Placeholder** (the unchanged template or example, or views that show
nothing). Its tests use the fixture in `crates/mandara-plugins/tests/data/vet`.

Everything else (the API, views,
settings, permissions, translations, releases and updates) is in
**[PLUGINS.md](PLUGINS.md)**.

## Keep the docs in sync

Every user-facing change updates its docs in the same change:

- [CHANGELOG.md](CHANGELOG.md): a line under `## Unreleased` in plain,
  friendly words, saying what people will notice (format in
  [docs/releasing.md](docs/releasing.md#changelog)). It becomes the release
  notes on GitHub, in Studio and in `mandara changelog`.
- [README.md](README.md): features, plugin table and counts, screenshots
  (from the sandbox).
- The guides in [`docs/`](docs): Studio pages, commands, configuration.
- [PLUGINS.md](PLUGINS.md) when the plugin interface or tooling changes
  (its tests check the budgets and exports).
- The installer's per-distribution package tables
  (`crates/mandara-install/src/packages.rs`) when a runtime dependency
  changes.

## Commits and pull requests

- Write [Conventional Commits](https://www.conventionalcommits.org):
  `fix:`, `feat:`, `perf:`, `refactor:`, `docs:`…, with `!` or a
  `BREAKING CHANGE:` footer for breaking changes. The release tooling reads
  them to pick each component's version bump (see
  [docs/releasing.md](docs/releasing.md)).
- Don't bump versions or edit `versions.toml` by hand: on every push to
  `main` the Release workflow runs `cargo xtask bump`, commits
  `chore(release): …` with tags, and publishes a release when the shell's
  version changed.
- Before opening a pull request, run `cargo check --workspace --all-targets`
  and `cargo test --workspace`, and look at `cargo clippy --workspace
  --all-targets`. CI runs the same on x86_64 and aarch64, builds the
  example plugins, checks the alternative allocators and dry-runs the
  installer.
- Keep a pull request to one topic, say how you tested it (sandbox
  commands, screenshots from the sandbox, measurements), and include the
  docs and translations it needs.
