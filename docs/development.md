# Development tools

`cargo xtask bootstrap` refreshes the short download command’s translated
messages and public release keys. `cargo xtask bootstrap --check` verifies
the generated section without writing it. The optional POSIX download
script only verifies and opens a release; installation stays in Rust.

Everything here is Rust, run through `cargo xtask` (the `xtask/` crate). The
system programs it starts are MangoWM, `dbus-daemon`, `bwrap`, PipeWire
(`pipewire`, `pipewire-pulse`, `wireplumber`, `pactl`, `paplay`) and `grim`.

## Website

GitHub Pages serves `index.html` as the homepage and the Markdown guides as
documentation. The guides share `_includes/head-custom.html` (phoenix favicon,
light/dark colours, `assets/css/pages.css`). The homepage is plain static
HTML with no front matter or Liquid: Pages copies it as is, its links are
file-relative (so they work under the repository's `/mandara/` path and when
opened from disk), and it loads only `assets/css/site.css`,
`assets/js/site.js` and `assets/js/copy-code.js`. Open `index.html` in a
browser to preview it; no build step or server is needed.

The page follows the shell's own Material You language: tonal surfaces,
expressive shapes (the cookie, flower, clover and burst outlines in the page's
SVG sprite) and Alexandria, the shell's bilingual typeface, set from hairline
to black. Alexandria and JetBrains Mono are self-hosted WOFF2 subsets in
`assets/fonts/` under the SIL Open Font License (`assets/fonts/OFL.txt`), so
the page makes no third-party requests. Its default palette is Mandara
Corniche; picking a Look swaps the colour tokens for that Look's palette, and
the Auto / Light / Dark control in the top bar repaints the page in either
scheme. Both come from the theme engine: after changing a theme or a Look, run
`MANDARA_PAGES_DUMP=1 cargo test -p xtask pages` to rewrite the palette block
in `site.css` and the JSON in `index.html`. The shortcut browser lists
`default_keybinds()`, the plugin shelf follows `docs/plugins.md` (without the
opt-in faith plugins, which stay in their own guide section) and the language
chips follow the locale catalogs; the tests fail when any of them drift.

Every screenshot on the homepage is a pair, `name.webp` (dark) and
`name-light.webp` (light) of the same scene at the same size, inside a
`<picture>` whose light source follows the visitor's scheme; site.js switches
the sources when the visitor forces light or dark. The tests fail on a
screenshot without its light partner.

Everything works without JavaScript: the Looks and bar styles are native radio
controls, the questions are a plain list, and every shortcut is listed. The
script adds the palette and scheme switches, the breathing letters, the slider
designs, shortcut search, the footer's Alexandria time and the hero film. The
film slot loads `assets/video/mandara-promo.webm` (then `.mp4`) only after the
page has loaded, never autoplays with reduced motion, and shows its Play and
Sound buttons only once a frame has arrived, so the poster stands in until the
video file is added. Preview both colour schemes, phone width, keyboard
selection and reduced motion. `cargo test -p xtask pages` checks the links,
image sizes and pairs, palettes and their contrast, Looks, bar styles,
shortcuts, plugins, compositors, languages and the installation command
against their sources.

The shared `assets/js/copy-code.js` adds Copy buttons to the homepage's
installation command and fenced examples in the guides. It copies the code's
text, including quotes and options, without syntax markup or a trailing newline.
If the browser blocks copying, it selects the code for manual copying and
explains what happened. Commands remain readable and selectable without scripts.
Each block's feedback is a polite status region (`role="status"`), present
before the first message. A press clears it and writes the message 100 ms
later, so pressing Copy again announces the same words again. "Copied to
clipboard." clears after 4 seconds, so it never describes an older press.
The blocked-copy instructions stay until the next press. Only the latest
press writes a message.
When changing this control, verify actual clipboard contents, Enter and Space,
blocked permissions, both colour schemes and narrow screens in a private browser,
and repeated presses with a real screen reader (Orca, NVDA or VoiceOver).

## Warnings are errors

`[workspace.lints]` in `Cargo.toml` denies every compiler warning and
clippy's default lints in every crate, and CI runs
`cargo clippy --workspace --all-targets`, so any warning fails the build.
Run it before you call a change done. Fix the cause; when an exception is
truly right, put it on that one item with its reason:

```rust
#[expect(clippy::large_enum_variant, reason = "one per frame; boxing would allocate per frame")]
```

## Pure Wayland: no X11

Mandara talks only Wayland. GPUI and its platform crates are built with
`default-features = false` and without their `x11` features, so the shipped
binary links no X11 library. A test in `xtask` (`x11_guard_tests`, part of
`cargo test --workspace` and so of CI) keeps it that way: it reads every
lockfile (the workspace, `tools/mandara-delta` and the example plugins) and
fails when an X11 crate is resolved: `x11rb`, `xcb`, `x11` / `x11-dl`,
`x11-clipboard`, X11 input methods, or xkbcommon's `x11` feature (which
shows up as `as-raw-xcb-connection`). A lockfile is the graph for every
target and every enabled feature, so this also catches a crate only some
platform would link. When it fails, `cargo tree -i <crate>` shows what
pulled the crate in; build that dependency without its X11 feature.

Run it alone with `cargo test -p xtask x11`.

## FreeBSD: the cross check

CI's **FreeBSD build** job runs Clippy on the whole workspace, tests
included, for `x86_64-unknown-freebsd`. Like the Linux build, any warning
fails it, and it is a required check. It needs only Rust's FreeBSD standard
library, no FreeBSD system root. To run it locally:

```bash
rustup target add x86_64-unknown-freebsd
PKG_CONFIG_ALLOW_CROSS=1 cargo clippy --workspace --all-targets --target x86_64-unknown-freebsd
```

`PKG_CONFIG_ALLOW_CROSS=1` lets the fontconfig and freetype build scripts
read the host's pkg-config files (install their `-dev` packages), which is
harmless because a check never links. Without it they stop early and hide
GPUI's Linux platform and every crate above it.

FreeBSD runs GPUI's Linux platform (Wayland only, the same layer surfaces),
so code that differs between the two kernels is small and behind
`cfg(target_os = "linux")` / `cfg(target_os = "freebsd")`: the supervisor's
parent-death signal (`prctl` / `procctl`), the worker pool's CPU affinity
(`sched_setaffinity` / `cpuset_setaffinity`), the depth helper's idle class
(`SCHED_IDLE` / `rtprio`), the running binary (`/proc/self/exe` /
`kern.proc.pathname`, `mandara_config::paths::running_image`) and the
camera watcher's inotify (a timer elsewhere). Tests that drive a private
Linux desktop through procfs and `SO_PEERCRED` are Linux-only.

A clean check doesn't mean a feature works there: readers of `/proc` and
`/sys`, logind, NetworkManager, BlueZ and the package managers compile
everywhere and find out at run time.

### Building on FreeBSD

Mandara builds natively on FreeBSD 14 (amd64) with the packages below, and
runs under a Wayland compositor such as Sway. GPUI draws through Vulkan:
the GPU's Mesa driver, or lavapipe (`mesa-dri`) in a VM without one.

```sh
pkg install rust pkgconf cmake-core gmake wayland wayland-protocols libxkbcommon \
  fontconfig freetype2 vulkan-loader mesa-dri mesa-libs
cargo build --locked --release -p mandara-app
```

The binary links only FreeBSD's libc, libthr, libm and libgcc_s; Wayland,
xkbcommon and Vulkan are loaded at run time, and nothing links X11
(`ldd target/release/mandara | grep -i x11` prints nothing). A release
build (fat LTO, one codegen unit) needs 13 GiB or more of memory for its
final link.

## Local checks and cold builds

For a focused edit, start with the affected crate and its callers, for
example `cargo test -p mandara-install --locked`. Before a pull request,
run the full workspace tests and clippy; CI keeps both checks.
Its initial workspace build uses `--profile test`, so the following tests
reuse those dependencies and the installer dry run uses the tested binary.

The test profile uses unoptimized workspace code and basic dependency
optimization, without debug information or incremental caches. It retains assertions and the AV1 decoder’s required
optimization. Interactive sandbox binaries use the separate dev profile,
with optimized dependencies so the UI remains responsive. Release settings
are unchanged. A first dev build and a first test build therefore produce
separate artifacts.

To see what a cold build spends time on, use
`cargo test --workspace --locked --no-run --timings`; Cargo writes an HTML
report under `target/cargo-timings/`. Compare clean target directories with
the same toolchain, job count and compiler flags. A compiler cache helps
repeated builds; it does not remove the cost of a first cache miss.

A reference run on 5 November 2026, at `eda426a`, took under four minutes
including tests and used under 8 GiB for the test artifacts. It used Rust
1.99.1 and eight Cargo workers on a Ryzen 9 9950X, after cleaning the dev
and test outputs; the package-download cache was already populated. Other
profiles are excluded from that size. Compare results on the same hardware
before attributing a timing difference to the profile.

On Linux, an installed alternative linker can be selected locally without
requiring it for every contributor, for example
`RUSTFLAGS="-C link-arg=-fuse-ld=mold" cargo test --workspace --locked`.
Changing compiler flags creates another set of artifacts. Use the pruning
tool below to keep the total bounded.

## The demo sandbox: `cargo xtask sandbox`

A private, headless Mandara desktop for screenshots, demos and end-to-end
tests. It never touches the real session.

- **Compositor:** a headless MangoWM with its own Wayland socket.
- **Buses:** a private session bus and a dead system bus, so nothing
  reaches the real NetworkManager, logind or BlueZ.
- **Demo identity:** the shell runs as a demo user on a demo machine under
  `bwrap`: account `demo` ("Mandara Demo"), host `mandara-demo`, its own PID
  namespace, the real `/home` hidden and the sandbox home mounted at
  `/home/demo`, host Flatpak and Snap apps hidden.
- **Demo data:** generated on first use under `target/sandbox/`: 21 demo
  apps with Papirus icons, 18 Downloads, 10 procedural wallpapers, album
  covers and a 24 s procedural music track.

The sandbox compiles all 19 non-English language packs from the shell and
login catalogs. Each `.mlang` holds both catalogs, with login wording kept
separate under `greeter:` keys. The shell and greeter map only the selected
language; translation catalogs are not embedded in the binary. A missing
pack falls back to English. `--no-langpacks` exercises downloads in the shell;
the login process always uses installed files and never downloads a language.

```bash
cargo build --release -p mandara-app              # or pass --bin to `up`
cargo xtask sandbox up                            # 2560x1440 at 1.5, the demo config
cargo xtask sandbox up --config hero --size 2880x1620 --scale 1.5 --bin target/debug/mandara
cargo xtask sandbox up --no-langpacks             # no language packs: the download path
cargo xtask sandbox shot out.png                  # screenshot of the sandbox output
cargo xtask sandbox msg panel open media          # any `mandara msg` command
cargo xtask sandbox pointer click 900 500         # move / click / drag / flyby / scroll, logical px
cargo xtask sandbox studio home                   # open Studio on a page (`studio stop` closes it)
cargo xtask sandbox studio --size 1124x784 themes # float Studio at another size first
cargo xtask sandbox audio play                    # sound server + "Corniche Radio" MPRIS player
cargo xtask sandbox notify                        # three demo notifications
cargo xtask sandbox matte studio.png --region 0 70 2560 1320 --margin 40
cargo xtask sandbox shell restart                 # restart only the shell
cargo xtask sandbox down
eval "$(cargo xtask sandbox env)"                 # the sandbox env for manual commands (fish: `cargo xtask sandbox env --fish | source`)
```

- **Configs:** `--config demo` is the default the first time. It gives
  Papirus icons, the dock with demo apps, a stacked clock and the harbour
  wallpaper. `--config hero` is the README hero shot. The README's setups gallery
  is shot from `xtask/src/sandbox/setups/*.toml` the same way (`--config FILE`;
  each file says how). `--config FILE` copies
  your own config. Both presets are small overlays on the shell's defaults
  (`xtask/src/sandbox/config.*.toml`).
- **Screenshot setups:** the README's widget shots, the sky dashboard and
  the Looks come from `xtask/src/sandbox/setups/*.toml` too (`sky`,
  `widgets-desk`, `widgets-music`, `widgets-you`, `widgets-arabic`,
  `looks`). Their wallpapers are made once in the sandbox with the
  wallpaper generator (`sandbox msg wallgen generate STYLE --palette P
  --seed S --size 2880x1620 --format png --no-apply`); each file names
  its style, palette and seed, so every shot has a wallpaper of its own.
- **Languages:** `up` compiles every language pack for the binary's
  version (`mandara --version`) into the demo user's
  `data/mandara/locales/<version>/`, as `mandara install` would; a release
  build reads translations only from packs. `--no-langpacks` leaves them
  out, to try the download path (with the sandbox offline it stays in
  English and Studio shows the error with Retry). `cargo xtask langpacks
  --bin PATH --dir DIR` builds the same packs anywhere.
- **Updates and downloads against a mock release:** a build with
  `--features mock-release` (mandara-app; never shipped) also trusts the
  hex release keys in `MANDARA_MOCK_RELEASE_KEYS`, so a release you sign
  yourself and serve locally can be installed. `sandbox up` passes that
  variable and `MANDARA_UPDATE_API` through, so a slow local server shows
  the language cards and Studio → About filling with real progress.
- **Reddit against a local server:** `sandbox up` also passes
  `MANDARA_REDDIT_API` through, so the Reddit plugin can read a loopback
  stand-in for Reddit's API (token, listings, threads, `morechildren`
  and pictures under `/media/<host>/…`) instead of the real service.
- **Recorded system updates:** a debug build with
  `--features fake-system-update` replays package-manager output recorded
  in clean containers instead of running pkexec. `sandbox up` passes
  `MANDARA_FAKE_SYSTEM_UPDATE=<backend>[:uptodate|locked|fails]` (`pacman`,
  `apt`, `nixos`…), `MANDARA_FAKE_SYSTEM_UPDATE_DELAY_MS`,
  `MANDARA_FAKE_SYSTEM_UPDATE_HOLD=<step>:<line>` (pause there until
  cancelled, e.g. `aur:100`) and `MANDARA_FAKE_AUR=<helper>[:uptodate|fails]`
  (`paru` or `yay`) through. With the AUR fake, the Updates panel lists the
  recordings' packages, and the AUR step asks for its password through the
  real askpass socket and Mandara's dialog (type `wrong` to see a refused
  password). Nothing on the system is touched.
- **Matte:** `matte` cuts out the open surfaces as a transparent PNG. It
  shoots once over a black wallpaper and once over a white one, then
  difference-mattes the pair. Give it a region (physical pixels) that leaves
  out the bar and the dock.
- **Safety:**
  - Every command builds its children's environment from scratch and talks
    only to sockets inside the sandbox runtime dir.
  - It refuses to act unless that dir is private and carries the sandbox's
    marker.
  - It signals only processes it recorded and whose environment names the
    sandbox runtime dir.
  - The runtime dir is `$MANDARA_SANDBOX_RUNTIME`, by default
    `/tmp/mandara-sandbox-<uid>`. Paths under `/run` are refused.
  - `SANDBOX_TZ` sets the time zone (default `Africa/Cairo`).
- **Debug builds** are fine here. In the real session, run only release
  builds.

### Power gauge preview

The `power_gauge` example draws the real charge gauge with labelled sample
values, so it needs no battery or system bus. It requires the private
sandbox marker from this checkout and closes itself after twelve seconds.
Use the default dark palette or pass `--light`:

```sh
cargo build -p mandara-ui --example power_gauge
cargo xtask sandbox up
eval "$(cargo xtask sandbox env)"
"${CARGO_TARGET_DIR:-target}/debug/examples/power_gauge" --light
```

## Screenshot grids: `cargo xtask mosaic`

```bash
cargo xtask mosaic docs/screenshots/looks-dark.png --cols 2 --theme dark \
    look-corniche-dusk-dark.png="Corniche Dusk" look-pharos-ink-dark.png="Pharos Ink" …
```

Lays screenshots out as a grid of rounded tiles with a soft shadow and a
label under each, on a transparent background, so the image sits well on
GitHub's dark and light pages. `--theme dark|light` sets the label colour
and the shadow's strength; `--width`, `--gap` and `--radius` size the
tiles (1280, 56 and 28 px by default). Every tile is scaled to cover the
first tile's shape. The README's Looks image is two of these (dark and
light, shown with `<picture>`), converted to WebP.

## Resource budget: `cargo xtask resource-check`

```bash
cargo xtask resource-check target/release/mandara [config.toml]
```

This runs the binary in a throw-away headless MangoWM session (3840 × 2160
at 1.5, its own runtime dir and session bus, a dead system bus). It lets
the shell settle for 3 minutes, then checks resident memory, anonymous
memory, threads and idle CPU against their budgets. It also checks that a
Vulkan renderer didn't load the OpenGL driver. Exit status 1 means over
budget, and the per-thread-name breakdown is printed. Details, measurements
and environment overrides (`RSS_MAX_KIB`, `ANON_MAX_KIB`, `THREADS_MAX`,
`CPU_MAX_PCT`, `SETTLE_SECS`, `CPU_SECS`, `MESA_CACHE`,
`WLR_RENDER_DRM_DEVICE`) are in
[releasing.md](releasing.md#resource-budget).

Bar and sidebar plugin slots follow their placement lists, plugin switches,
factory generation and orientation. Theme and geometry edits repaint those
instances; bar geometry has its own change key. Studio’s wallpaper scheme
previews cache only their displayed color strips under the original seed and
mode keys, so revisiting a selection reuses its cached result.

## Native desktop tests (`#[ignore]`)

Tests marked `#[ignore = "requires the marked …"]` (in
`crates/mandara-ui/tests/`) drive a real shell with real input and
screenshots. They run only against a private desktop that an external
runner starts for them; `cargo test` alone skips them. The resource-desktop
tests (`beacon_keyboard`, `desktop_text`, `design_sharing`) share
`tests/support/private_desktop.rs`, which checks before every request, tool
and screenshot that:

- the runtime directory is `/tmp/mrc.*` (yours, mode 0700) with the test's
  marker file naming this checkout, and HOME is the paired throw-away
  `mandara-rc.*/home`;
- the session bus is private and the system bus is dead;
- the compositor is headless with one output;
- the shell and compositor are still the processes the runner recorded
  (`<PREFIX>_PID`, `_START`, `_BINARY` and `_COMPOSITOR_*`): same PID start
  time, same executable and the same socket peer.

A runner for any marked test must follow three rules:

- **One run owns the desktop.** Each runner invocation creates its own
  desktop, runs its tests against it and tears it down. Never share a
  desktop between concurrent runs, and never hand one to a later run: the
  tests change its configuration and send it input.
- **Tear down on forced interruption too.** On Ctrl+C, SIGTERM, a timeout
  or a runner error, stop the processes it started and delete the runtime
  and HOME directories, as on a normal exit. A test that is killed doesn't
  restore anything, so its desktop is thrown away, never reused.
- **Restoration stops when the desktop changes.** The resource-desktop
  tests snapshot the settings they change and put them back (and remove
  the notes and tasks they created) when they end, even after a failed
  assertion. Each restoring request goes through the
  same identity checks.
  If the desktop was torn down or replaced in the meantime, restoration
  stops at the first refused request and writes nothing to the other
  desktop.

## Build artefacts: `cargo xtask prune`

```bash
cargo xtask prune                          # delete debug artefacts first while over 50 GiB
cargo xtask prune --max-gib 30 --dry-run   # show what would go
cargo xtask prune --watch 5 --dir /var/tmp/mandara-target --cargo-cache
```

Debug builds of the whole workspace grow fast: incremental caches and one
target directory per tool or agent add up to hundreds of GiB. `prune`
measures every target directory of the project. That is `target/`, the
example plugins' and tools' `target/`, `$CARGO_TARGET_DIR` and each
`--dir`. Over the limit, it deletes until they are back under 60 % of it,
in this order:

1. debug-like `incremental/` caches;
2. whole debug-like profiles, including target directories that other
   tools left inside `target/`;
3. optimized profiles' `incremental/` caches;
4. optimized profiles other than `release`;
5. `release`;
6. with `--cargo-cache`, the unpacked crate sources in
   `~/.cargo/registry/src`, only while no build is running.

A profile whose `.cargo-lock` is held by a running build is skipped.
`target/sandbox` is never deleted. `--watch MINUTES` repeats the check;
run that from a copy of the binary outside the directories it prunes.

Sparkle icons are reserved for AI features. Use symbols for the function of
ordinary tools, automation and procedural generation.
