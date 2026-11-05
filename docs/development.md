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
documentation. The homepage uses `assets/css/landing.css`; the shared
`_includes/head-custom.html` supplies its phoenix favicon and light/dark
colours. Keep asset and guide links behind Jekyll's `relative_url` filter
so they also work under the repository's `/mandara/` path.

The Look selector uses native radio controls and the FAQ uses `details`,
so both work without JavaScript. Preview both colour schemes and narrow
screens, including keyboard selection and reduced motion. Run
`cargo test -p xtask pages_tests` to check the linked files, preview controls,
contrast, installation command and plugin count against the guides.

## Warnings are errors

`[workspace.lints]` in `Cargo.toml` denies every compiler warning and
clippy's default lints in every crate, and CI runs
`cargo clippy --workspace --all-targets`, so any warning fails the build.
Run it before you call a change done. Fix the cause; when an exception is
truly right, put it on that one item with its reason:

```rust
#[expect(clippy::large_enum_variant, reason = "one per frame; boxing would allocate per frame")]
```

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
