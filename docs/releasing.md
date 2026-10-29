# Versioning and releases

Mandara releases itself. Every push to `main` runs the **Release** workflow:
changed components get new versions, and when the shell's own version changes,
GitHub builds, packages and publishes a release.

## Components and versions

Every workspace crate (`crates/*`, `xtask`) and every plugin
(`plugins/**/plugin.toml`) is a **component** with its own semantic version.
`versions.toml` records, for each component, the version and the **content
hash** it was released with.

The content hash (SHA-256, 128 bits) covers:

- the component's own files (build output and the `version = …` lines of its
  manifests excluded);
- the workspace `Cargo.toml`, `Cargo.lock` (minus workspace package versions),
  `.cargo/config.toml` and `rust-toolchain.toml`;
- recursively, the hashes of the workspace crates it depends on.

So a change to `mandara-core` changes the hash of everything built on it: if a
component's build changed, its version changes. A component whose dependencies
and sources are untouched keeps its version.

```bash
cargo xtask status          # versions, hashes, what changed and the bump it gets
cargo xtask bump --dry-run  # what a release would do
cargo xtask bump            # bump changed components, update manifests, Cargo.lock, versions.toml
cargo xtask bump --commit --tag   # …and commit "chore(release): …" with tags
cargo xtask verify          # fail if anything changed without a bump
```

### How much to bump

The bump level comes from [Conventional Commits](https://www.conventionalcommits.org)
since the component's last tag, counting only commits that touch its folder:

| Commits | Bump | Pre-1.0 |
|---|---|---|
| `fix:`, `perf:`, `refactor:`, anything else | patch | patch |
| `feat:` | minor | minor |
| `feat!:`, `fix!:`, `BREAKING CHANGE:` in the body | major | minor |

A component that changed only because a dependency changed gets a patch bump.

### Tags

| Component | Tag |
|---|---|
| the shell (`mandara-app`) | `v0.4.2`, which is the GitHub release |
| any other crate | `mandara-ui-v0.3.1` |
| a plugin | `plugin-moon-phase-v1.2.0` |

### Build identity

Every build embeds each component's version and hash, the source fingerprint
(see below), the git revision (with a dirty flag), the target, the profile and
the build date:

```
$ mandara --version
mandara 0.4.2 (fingerprint 5d41402abc4b, 3f9c2a1b7e40, content 9f8e17ab4c21, 2026-10-27, x86_64-unknown-linux-gnu)
```

A local build of changed sources reports the **next** patch as a pre-release
(`0.4.3-dev`), so it can never pass for the released version. `mandara msg info`
lists every component's version; Studio → About shows the full build string.
Release builds use `SOURCE_DATE_EPOCH` (the release commit's time) so archives
are reproducible.

### Build fingerprint

`cargo xtask build` is the fingerprinted release build. The installer's source
builds (`mandara install --from-source`) and the Release workflow both use it.

```bash
cargo xtask build                       # host target, release profile
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo xtask build --target x86_64-unknown-linux-gnu --variant x86-64-v3 \
    --profile release --features alloc-jemalloc --no-default-features \
    --out dist --no-readme              # every option
```

It runs these steps:

1. **Source fingerprint:** one SHA-256 over every build input: the workspace
   `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml`,
   `PLUGINS.md` (embedded), `versions.toml`, and every file under `crates/`,
   `vendor/`, `assets/`, `xtask/` and `plugins/`. That includes sources, build
   scripts, locales, WIT and embedded assets. `xtask` and `plugins` count
   because the binary embeds every component's content hash. Each file contributes its `/`-separated relative path and the
   SHA-256 of its content, sorted by path. Then come `rustc -vV`, the target
   triple, the profile, the enabled features and `RUSTFLAGS`. Mtimes, the
   checkout location and git don't count, so a fresh clone, a tarball or a
   re-initialized history of the same sources fingerprints the same. The
   binary itself still embeds the git revision and the build date, so binary
   hashes match across builds only with the same commit and
   `SOURCE_DATE_EPOCH`, as in the Release workflow. A fixed
   list skips build output (`target/`, `dist/`), hidden folders (except
   `.cargo`), editor leftovers and agent notes. `.gitignore` isn't used
   because it stays local and CI checkouts don't have it.
2. **Build:** `cargo build --locked --profile release -p mandara-app` with the
   fingerprint in `MANDARA_SOURCE_FINGERPRINT`. `mandara-version`'s build
   script embeds it, so `mandara --version` shows the first 12 hex digits.
   Plain `cargo build` and `cargo check` never hash the tree and show
   `fingerprint dev`. Switching between the two rebuilds `mandara-version` and
   relinks.
3. **Binary hashes:** SHA-256 of every shipped binary (`mandara`). The
   command fails if `mandara --version` doesn't report the fingerprint it was
   built from.
4. **Lines of code:** code, comment and blank lines of the workspace's Rust
   (each crate under `crates/` plus `xtask`), counted by a small lexer in
   `xtask/src/loc.rs`. It knows line and nested block comments, strings, raw
   strings and char literals versus lifetimes. Vendored Rust (`vendor/`), the
   example plugins, the locale catalogs and WIT are counted separately.
5. **Manifest:** `dist/mandara-<version>-<target>[-<variant>].build.toml`
   holds all of the above, plus `mandara --version` and the build time (UTC,
   from `SOURCE_DATE_EPOCH` when it's set).
6. **README:** the block between `<!-- build-info:start -->` and
   `<!-- build-info:end -->` in `README.md` is rewritten as a table, with full
   hashes and the per-crate counts in a collapsed section. The command creates
   a `## Build fingerprint` section before `## License` if the markers are
   missing, and changes nothing else. `--no-readme` skips this step.

To check a downloaded release, compare `mandara --version` and
`sha256sum mandara` with the release's `.build.toml`. Rebuild the tagged
sources with the same toolchain and `cargo xtask build` to get the same source
fingerprint.

## CI (`.github/workflows/ci.yml`)

Every push and pull request runs:

- **Build and test** on x86_64 and aarch64: `cargo build` and `cargo test`
  with `--locked`, plus `cargo xtask status`. Clippy runs as a report only.
- **Plugins**: example plugins build for `wasm32-wasip2` and are packaged in
  marketplace format.
- **Installer**: the installer is the binary (`crates/mandara-install`); its
  unit tests (argument parsing, per-family dry-run plans, package-manager
  argv, SHA-256 checks, desktop entry, uninstall) run with the workspace
  tests, then `mandara install --dry-run --no-deps --yes` walks every step
  on the runner.

## Release (`.github/workflows/release.yml`)

On every push to `main` (not on its own `chore(release)` commits):

1. **Version:** `cargo xtask bump --commit --tag` and `cargo xtask verify`,
   then the release commit and tags are pushed.
2. **Build** (only when the shell's version changed), on Ubuntu 22.04 runners
   so the binaries need glibc ≥ 2.35. Builds are fingerprinted
   (`cargo xtask build --no-readme`) optimized release builds: fat LTO, one
   codegen unit, stripped. Each build's manifest is added to the job summary.

   | Artifact | Platform |
   |---|---|
   | `mandara-<v>-x86_64-linux-gnu.tar.gz` | any x86_64 |
   | `mandara-<v>-x86_64-linux-gnu-x86-64-v3.tar.gz` | x86_64 with AVX2 (Haswell, Zen and later) |
   | `mandara-<v>-aarch64-linux-gnu.tar.gz` | ARM64 |
   | `mandara_<v>-1_amd64.deb`, `mandara_<v>-1_arm64.deb` | Debian, Ubuntu |
   | `mandara-<v>-1.x86_64.rpm`, `mandara-<v>-1.aarch64.rpm` | Fedora, openSUSE |
   | `PKGBUILD` | Arch (`mandara-shell-bin`, the same recipe as the AUR package) |
   | `<plugin>.mandara.tar.gz` | bundled plugins |
   | `mandara-<v>-<target>[-<variant>].build.toml` | build manifest: source fingerprint, toolchain, binary SHA-256, lines of code |
   | `mandara-<old>-to-<v>-<target>[-<variant>].patch.zst` | delta update from the previous release (see below) |
   | `update.json` | update manifest: per target the tarball, `bin/mandara` and patches, with SHA-256s and sizes |
   | `SHA256SUMS` | checksums of everything |

3. **Publish:** a GitHub release `v<version>` with generated notes, all
   artifacts, and a signed **build provenance attestation** (verify with
   `gh attestation verify <file> --repo <owner>/<repo>`).

Mandara is a Wayland desktop shell, so Linux is the only operating system it
runs on. macOS and Windows have no Wayland compositors to host it.

To re-publish artifacts for the current version without a code change, run
the Release workflow manually with **rebuild** enabled.

## Delta updates

Binary patches are Mandara's main update method (`mandara update`, Studio →
About); the full tarball is the fallback. A patch between two consecutive
fat-LTO release builds is typically 5–15% of the binary, so a 40–50 MB
download becomes a few MB.

**Format** (`tools/mandara-delta`, shared by CI and the updater): a
bsdiff-style delta from [bidiff](https://crates.io/crates/bidiff)
(suffix-array matching; an "add" stream of byte differences that is mostly
zeros, plus new bytes), compressed with zstd level 19. The updater applies
it with pure-Rust code only (bipatch + ruzstd) and checks every step:

1. the installed binary's SHA-256 equals `from_sha256` of the patch (the
   published hash of that version), otherwise the patch is skipped without
   downloading it;
2. the patch's SHA-256;
3. the rebuilt binary's SHA-256 and size equal `binary` in `update.json`.

Any failure falls back to the tarball, verified against `update.json` (or
`SHA256SUMS` for releases without one). The new binary replaces the old one
atomically (`mandara.new` → fsync → rename) and the old one stays as
`mandara.old` (`mandara update rollback`). Package-managed installs (`/usr`,
`pacman -Qo` / `dpkg -S` / `rpm -qf`) and cargo builds are never replaced.

**In CI**, each build job runs, after `cargo xtask dist`:

```bash
gh release download <previous tag> --pattern 'mandara-<old>-<target>.tar.gz'
cargo xtask update-part --target <triple> [--variant v3] --out dist --previous mandara-<old>-<target>.tar.gz
```

`update-part` extracts `bin/mandara` from both tarballs, creates the patch,
verifies it by applying it with the updater's decoder, and writes this
target's part of the manifest (`*.update-part.json`). The previous release
is the newest published, non-pre-release `v*` release other than the one
being built; on the first release, or when it lacks the target, the part
lists the full download only. The publish job merges the parts with `jq`
into `update.json`, and patches and manifest go into `SHA256SUMS` and the
provenance attestation like every other artifact. Patches are only made
from the immediately previous release; older installs take the full
download.

```json
{
  "schema": 1, "name": "mandara", "version": "0.2.0",
  "targets": {
    "x86_64-linux-gnu-x86-64-v3": {
      "archive": { "file": "mandara-0.2.0-x86_64-linux-gnu-x86-64-v3.tar.gz", "sha256": "…", "size": 41958862 },
      "binary": { "path": "bin/mandara", "sha256": "…", "size": 103729296 },
      "patches": [ { "from": "0.1.9", "from_sha256": "…", "file": "mandara-0.1.9-to-0.2.0-x86_64-linux-gnu-x86-64-v3.patch.zst", "sha256": "…", "size": 7223906 } ]
    }
  }
}
```

Try it locally:

```bash
cargo xtask delta --old old/mandara --new target/release/mandara --out /tmp/u.patch.zst
cargo xtask apply-delta --old old/mandara --patch /tmp/u.patch.zst --out /tmp/mandara --sha256 <hex>
```

The updater reads the API base from `MANDARA_UPDATE_API` (default
`https://api.github.com`); `crates/mandara-services/tests/self_update.rs`
runs check → delta → fallbacks → rollback against a local mock.

Marketplace plugins use the same format (`cargo xtask plugin-delta`, or
`mandara-delta plugin` for plugin authors; see PLUGINS.md). A plugin patch
rebuilds `plugin.wasm`; every other file comes from a small meta archive
listed in the plugin's `update.json`.

## Memory allocator

Release and CI builds use `mandara-app`'s default feature,
`alloc-system-tuned`: glibc malloc limited to 2 arenas (`mallopt(M_ARENA_MAX)`
at startup) with a fixed 512 KiB mmap threshold (see [Resource
budget](#resource-budget)), plus `malloc_trim(0)` every 30 s on the service
runtime. There's
no C allocator, so the shell stays pure Rust. The other allocators remain
selectable (`--no-default-features --features alloc-jemalloc |
alloc-mimalloc | alloc-mimalloc-v2 | alloc-system`), and CI checks that each
one builds. mimalloc and jemalloc compile C code with the runner's
`build-essential` (native x86_64 and aarch64 runners, no cross-compiling).

Measured on 2026-10-27 on the development machine: dev profile (dependencies
at opt-level 3), MangoWM at 2560×1440 logical, the real shell restarted once
per allocator. The workload opened and closed all 31 plugin panels, the
sidebar and the launcher, three rounds. RSS is in MB, CPU is % of one core
while idle, and build times are the p95 / p99 view-build ms from
`mandara msg diagnostics frames`. Two rows per allocator mean two separate
runs. Other engineers were rebuilding and restarting at the same time, so
treat differences under about 30 MB as noise.

| Allocator | Idle RSS | After workload | 60 s later | Peak | Idle CPU | Build p95/p99 | Startup |
|---|---|---|---|---|---|---|---|
| glibc (`alloc-system`) | 215 | 241 | 241 | 246 | 0.3 | 0.22 / 0.27 | 416 ms |
| glibc tuned (default) | 100 | 101 | **75** | 211 | 0.2–2.4* | 0.19 / 0.24 | 394 ms |
| glibc tuned (default) | 152 | 170 | 168 | 206 | 0.2* | 0.15 / 0.18 | 377 ms |
| jemalloc | 160 | 126 | 119 | 258 | 0.2 | 0.16 / 0.20 | 464 ms |
| jemalloc | 163 | 193 | 191 | 260 | 0.2 | 0.16 / 0.23 | 377 ms |
| mimalloc v3 | 219 | 247 | 247 | 324 | 0.2 | 0.16 / 0.18 | 603 ms |
| mimalloc v3, `MIMALLOC_PURGE_DELAY=0 MIMALLOC_ARENA_EAGER_COMMIT=0` | 188 | 221 | 221 | 231 | 0.3 | 0.16 / 0.18 | 389 ms |
| mimalloc v2 | 318 | 256 | 254 | 363 | 0.3 | 0.14 / 0.17 | 505 ms |

\* A per-thread check over a separate quiet 60 s measured 0.18 % for the
whole tuned process. Each trim costs about 15 ms on a service thread. The
higher readings coincided with other engineers driving the shell.

Pure-Rust allocators were evaluated but not wired up. `talc` targets
`no_std`/WebAssembly and `rlsf` is a real-time TLSF. Both serialize every
allocation behind one global lock, have no thread caches, and don't return
memory to the OS. That would raise both RSS and contention for a 30-thread
desktop process.

## Resource budget

What the idle shell may cost, how to measure it, and what keeps it there.
`cargo xtask resource-check <binary> [config.toml]` runs the binary in a
throw-away headless MangoWM session (3840 × 2160 at 1.5, its own runtime
dir and session bus, a dead system bus), lets it settle for 3 minutes and
fails when resident or anonymous memory, the thread count or idle CPU is
over budget, or when a Vulkan renderer also loaded the OpenGL driver.

Measured on 2026-10-28 that way on the development machine (RX 9070 XT,
RADV), with copies of the real config, notes, to-dos and clipboard history,
the binary evicted from the page cache before each start (resident file
pages otherwise depend on what the cache holds), release builds:

| Build | RSS | Anonymous | Threads | Idle CPU | VRAM |
|---|---|---|---|---|---|
| `1d80c15` (0.1.0) | 158 MiB | 48.8 MiB | 37 | 0.10 % | 174.7 MiB |
| 0.1.0 + the sidebar tabs, dock, installer, keyrings… | 161 MiB | 49.3 MiB | 38 | 0.12 % | 174.7 MiB |
| with the changes below | 41–51 MiB | 18.9–19.6 MiB | 29 | 0.10–0.12 % | 162.4 MiB |

Opening panels and the launcher is unchanged: 0.062 ms p95 view build and a
6.08 ms median frame interval before and after (`mandara msg diagnostics
frames` over 15 openings), launcher IPC 21 ms.

What made the difference, each measured by switching it off alone:

* **Depth subject pixels released after painting** (`backdrop/mod.rs`,
  `RELEASE_CUT_PIXELS`): the wallpaper's cut-out subject (premultiplied, its
  bounding box at output resolution) stayed on the heap for repaints that
  rarely come; with transparent huge pages on it also grew 22 MiB of huge
  pages around it. The compositor holds the plane; a repaint cuts it out
  again on the loader thread while the old plane stays up. −24 MiB
  anonymous.
* **Vulkan-first wgpu instance** (`vendor/gpui_wgpu`, patch 7): the GL half
  of the instance initialized EGL and radeonsi on every start. −7 threads,
  −3.2 MiB anonymous, −13.8 MiB VRAM, no libgallium / libLLVM pages from GL.
* **Startup-only file pages unmapped once settled** (`mandara-app`
  `residency.rs`, 45 s after start and every 10 min): clean read-only file
  mappings (start-up code of the binary, libLLVM pages touched by its static
  initializers when RADV loads it, font files scanned once) are unmapped
  with `MADV_DONTNEED`; code still in use faults back from the page cache.
  Mappings with private copies (RELRO) are skipped. −40 to −62 MiB RSS at
  3 minutes.
* **App icons decoded at the size they are drawn** (`icon_raster.rs`):
  `img(path)` rendered SVG app icons at twice their canvas (a 512 px canvas
  is a 4 MiB bitmap). Opening the dock's Applications grid (~100 apps)
  went from +126 to +26 MiB anonymous and +258 to +136 MiB VRAM while open
  (with the mmap threshold below: +8.6 MiB anonymous). Applies to every app
  icon (launcher, dock, switcher, bar).
* **Fixed glibc mmap threshold** (`alloc.rs`, `MMAP_THRESHOLD` = 512 KiB):
  glibc raises its mmap threshold to the size of the first large block
  freed (up to 32 MiB) and doubles the trim threshold with it. Large
  buffers then came from a malloc arena whose free top `malloc_trim` never
  returns. Opening and closing the Applications grid three times left
  +16.2 MiB anonymous 40 s later; now +3.4–6.1 MiB (and +7.5–8.6 instead
  of +24–26 MiB while open). It also removes the 30 MiB huge-page arena seen in two of
  about ten settled runs. Panel frame times unchanged.
* **RELR relocations** (`mandara-app/build.rs`, `-z pack-relative-relocs`):
  `.rela.dyn` 4.0 MiB → 6 KiB plus a 52 KiB `.relr.dyn`, read at every
  start. Needs glibc ≥ 2.36 at run time (release builds already need 2.39).
* **No idle blocking-pool threads** (`mandara-services` `runtime.rs`):
  `unblock` runs each job on a thread that ends with it; the `blocking`
  crate's pool (zbus, smol) never shrinks below its 500-thread limit, so
  every thread it ever started woke twice a second forever. Capped at 2.
  −2 threads, −4 wake-ups/s.
* **Smaller retained caches**: a weather HTTP agent per refresh instead of
  one kept with its connection and ~0.5 MiB of buffers, and the desktop
  clock's glyph bitmaps released after each paint.

Measured and dropped: `MADV_NOHUGEPAGE` on the heap before each trim (no
measurable effect once the cut-out was released). Ruled out without
shipping: `PR_SET_THP_DISABLE` (inherited by every app the shell launches),
`MADV_PAGEOUT` instead of
`MADV_DONTNEED` (drops the pages from the page cache: later use would read
the disk); `panic = "abort"` (plugin and PAM boundaries rely on
`catch_unwind`). Left as they are: GPUI keeps taffy node storage, element
arenas and decoded images at their peak per window (~10 MiB after opening
every panel, bounded), Mesa's Wayland WSI makes wgpu create 4 swapchain
images (patch notes in `vendor/gpui_wgpu/MANDARA-PATCHES.md`), and the
dock's popovers are full-output overlay surfaces so a click anywhere
outside dismisses them: +136 MiB VRAM while one is open at 4K, back to
within 1–9 MiB of idle once it closes (one GPU allocator block). Shrinking them to the card needs another way to
see outside clicks (keyboard-focus loss works only for on-demand
popovers; the Applications grid and the picker take the keyboard
exclusively).

### Resident code

Once the anonymous memory was down, most of what remained was the binary's
own code: a real session kept 36–45 MiB of its 100 MB `.text` resident. A
page fault maps the whole page-cache folio it lands in (around a small one,
the kernel's 64 KiB fault-around window), so resident code is roughly
*distinct 64 KiB pieces touched × 64 KiB*, and the code that runs is
scattered over the whole segment. Measured 2026-10-29 in the headless
session above at 2560 × 1440 (3840 × 2160 at 1.5 within the noise), copies
of the real config, a demo MPRIS player and private PipeWire, the shell in
a scope that cannot swap; *settled* is 80 s after start (35 s after the
first drop), *activity* is 5 minutes of opening media and quick settings,
the launcher, a notification and a track change every 25 s with audio
playing. RSS / file-backed / code, MiB:

| Build | Page cache | Settled | After activity | Panel frames p50 / build p95 | Launcher IPC | Audio CPU | Start → IPC |
|---|---|---|---|---|---|---|---|
| before | read from disk | 43.7 / 23.6 / 15.8 | 76.9 / 52.5 / 30.1 | 6.08 / 0.056 ms | 20 ms | 1.98 % | 300 ms |
| before | just written (`cp`) | 73.8 / 54.3 / 37.7 | — | — | — | — | — |
| page-cache refresh + cold sections | just written | 52.6 / 32.6 / 18.3 | 76.4 / 52.2 / 30.6 | 6.07 / 0.050 ms | 20 ms | see below | see below |

Settled code varies 15–23 MiB between runs (which minute boundary, weather
fetch or D-Bus burst falls before the snapshot); file-backed memory also
holds libLLVM / RADV / font pages. After activity the code stays until the
next 10-minute drop: these two changes fix the 2× penalty of a freshly
written binary, not the working set itself (see PGO below). Start-up and
audio-visualizer CPU of the same two changes on top of the `opt-level`
list below, on a quiet machine: 321 ms and 1.73 % (before: 300 ms,
1.98 %); the final binary was only re-timed under an unrelated 27-core
build, so re-check those two on a quiet machine before quoting them.

* **Page-cache refresh** (`residency.rs`,
  `refresh_executable_page_cache`): writing the binary (`mandara install`'s copy,
  the self-updater, a package manager, the linker) leaves it in the page
  cache as 256 KiB and larger folios, and every function run then keeps the
  whole folio around it resident — twice the code — until the file is
  evicted, which on a machine with free memory means until reboot. Right
  after the first drop (45 s) the shell probes its own mapping (one
  already-cached page touched at 32 places of a throw-away private mapping,
  `/proc/self/pagemap` read around each) and, when a fault mapped more than
  the 64 KiB window, writes back and evicts the binary's page cache
  (`POSIX_FADV_DONTNEED`; pages other processes map stay). The code in use
  is read back from disk once, in small folios: once per install, like the
  first start after a reboot. Doing it at start instead cost 2.9 s of
  start-up while the install's write-back was pending.
* **Cold functions grouped** (`mandara-app/build.rs`,
  `-z keep-text-section-prefix`): what LLVM marks cold (`#[cold]`, panic and
  error paths) goes to its own 2.2 MB `.text.unlikely` instead of sitting
  between live functions.

Measured and not shipped, speed first:

* `opt-level = "s"` for ~100 cold crates (OCR, plugin compilation, D-Bus,
  HTTPS, config/IPC/CLI parsing, archive and rare image formats): −12.7 MB
  of `.text` but under 1 MiB less resident code, panel frames / audio
  unchanged — yet several of those crates sit on user-triggered paths
  (Studio's `config apply`, compositor events, notifications, OCR, first
  plugin load) that were not benchmarked, so it stays at 3.
* An earlier drop when `RssFile` regrows 8 MiB over the idle level (sheds a
  panel's code a minute after use instead of up to ten): the next panel
  opened after such a drop pays its minor faults again.
* LLVM hot/cold splitting (`-C llvm-args=-hot-cold-split`): 0.4 MB more out
  of `.text`, no resident difference, and needs `RUSTFLAGS` (the release
  workflow sets its own).
* **PGO** (`-Cprofile-generate` → one 5-minute training run here →
  `-Cprofile-use`, with the prefix flag): the 10.4 MB of code the training
  ran lands in `.text` / `.text.hot`, the other 62 MB in `.text.unlikely`;
  resident code 9.4 MiB settled and 14.5 after activity (−41 % / −51 %),
  RSS 55.8 MiB after activity, frames (6.08 / 0.059 ms) and audio CPU
  unchanged. It needs a training run per release (headless MangoWM, a GPU
  or lavapipe) in the release pipeline, and code the training does not run
  — Studio, the greeter's AV1 video, the lock screen, recording, OCR — is
  then treated as cold and optimized for size, so the training would have
  to cover all of them first.

Guards: `crates/mandara-ui/tests/startup_budget.rs` (registering every
built-in plugin keeps ≤ 4 MiB, allocates ≤ 16 MiB and starts no thread;
today 0.2 / 9.2 MiB), `residency` unit tests (classification, private
copies survive a drop, the folio probe unmaps its mapping),
`vulkan_is_tried_alone_before_gl`,
`cut_source_crops_to_the_subject` (released cut keeps geometry), the
`icon_raster` decode-size tests, and `cargo xtask resource-check` before a
release (see [development.md](development.md)).

## The installer and releases

The installer is the binary itself (`crates/mandara-install`); there is no
shell script. Users download a release tarball, extract it and run
`bin/mandara install`:

1. It checks its own SHA-256 against the release's `update.json` (the
   binary hash of its target), else against a tarball sitting next to the
   extracted folder and `SHA256SUMS`, refusing a mismatch (`--no-verify`
   overrides). Cargo builds from a checkout skip the check; debug builds are
   refused.
2. It installs the runtime packages of the distribution family (tables in
   `crates/mandara-install/src/packages.rs`, unit-tested: every family has
   run and svc lists, no duplicates, Gentoo atoms have a category), through
   sudo / doas / run0 / pkexec.
3. It copies itself to `~/.local/bin/mandara` (atomically), installs the
   Studio desktop entry and icon (embedded in the binary), runs
   `mandara setup`, offers Mandara Login and keyring unlock, and DDC/CI
   brightness.

`--latest` / `--version X.Y.Z` download a release instead: it asks the GitHub
API for the latest tag, picks `x86_64`, `x86_64-v3` (when the CPU has
AVX2/BMI2/FMA/MOVBE/F16C) or `aarch64` (glibc or musl, as the running binary),
downloads the tarball and checks it against `SHA256SUMS`, refusing to install
on a mismatch. It falls back to building from source when there is no
matching binary (other architectures). `--from-source` builds with
`cargo xtask build` (installing rustup's own `rustup-init` binary, checksum
verified, when Rust is missing). `MANDARA_RELEASE_BASE=<url>` (with
`--version`) installs from a mirror. `--dry-run` prints every step and
changes nothing.

Every release also carries version-less copies of the tarballs
(`mandara-<target>.tar.gz`), so
`https://github.com/theHamdiz/mandara/releases/latest/download/mandara-x86_64-linux-gnu.tar.gz`
always points at the newest build.
