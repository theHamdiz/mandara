# Development tools

Everything here is Rust, run through `cargo xtask` (the `xtask/` crate). The
system programs it starts are MangoWM, `dbus-daemon`, `bwrap`, PipeWire
(`pipewire`, `pipewire-pulse`, `wireplumber`, `pactl`, `paplay`) and `grim`.

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
  wallpaper. `--config hero` is the README hero shot. `--config FILE` copies
  your own config. Both presets are small overlays on the shell's defaults
  (`xtask/src/sandbox/config.*.toml`).
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
