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
since the component's last tag, counting commits that touch its folder or
the folder of any component it's built from (the same set its content hash
covers), so a `feat:` in the updater makes a minor shell release:

| Commits | Bump | Pre-1.0 |
|---|---|---|
| `fix:`, `perf:`, `refactor:`, anything else | patch | patch |
| `feat:` | minor | minor |
| `feat!:`, `fix!:`, `BREAKING CHANGE:` in the body | major | minor |

A component whose own folder and dependencies only saw `fix:`-type commits
gets a patch bump.

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
6. **Fingerprint table:** the block between `<!-- build-info:start -->` and
   `<!-- build-info:end -->` in [`docs/building.md`](building.md#build-fingerprint)
   is rewritten as a table, with full hashes and the per-crate counts in a
   collapsed section. The command adds a `## Build fingerprint` section at
   the end if the markers are missing, and changes nothing else.
   `--no-readme` (the name is historical) skips this step.

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
   then the release commit and tags are pushed. When the shell's version
   changes, the bump also turns `CHANGELOG.md`'s **Unreleased** notes into
   the new version's entry (dated today) and starts a fresh Unreleased
   section. If nobody wrote notes, the release's commit subjects fill a
   **Changes** list and the job shows a warning.
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
   | `mandara-<old>-to-<v>-<target>[-<variant>].patch.zst` | delta update from each of the last five releases (see below) |
   | `mandara-<v>-<code>.mlang`, `mandara-<v>-<code>.mlang.zst` | language packs, one per language (see [Language packs](#language-packs)); the `.zst` is what the shell downloads |
   | `update.json` | update manifest: per target the tarball, `bin/mandara` and patches, with SHA-256s and sizes; `locales`: the language packs |
   | `SHA256SUMS` | checksums of everything |
   | `SHA256SUMS.sig` | Ed25519 signature of `SHA256SUMS` by the release key (see [Signed releases](#signed-releases)) |

3. **Publish:** a GitHub release `v<version>` whose notes are the
   version's `CHANGELOG.md` entry (`cargo xtask changelog <version>`),
   followed by the install command, GitHub's generated notes, all
   artifacts, and a signed **build provenance attestation** (verify with
   `gh attestation verify <file> --repo <owner>/<repo>`).

   Build uploads use artifact v7 and the publish job downloads them with
   the pinned v8 action. It merges `dist-*` from this run into `dist/` and
   keeps the default error on a digest mismatch, before signing any files.
   The Node 24 action needs Actions Runner 2.327.1 or newer; the workflow
   uses GitHub-hosted Ubuntu runners.

Mandara is a Wayland desktop shell, so Linux is the only operating system it
runs on. macOS and Windows have no Wayland compositors to host it.

To re-publish artifacts for the current version without a code change, run
the Release workflow manually with **rebuild** enabled.

## Changelog

`CHANGELOG.md` is the release notes, written for the people who use
Mandara: what they'll notice, in plain, friendly sentences, not commit
messages. It is compiled into the binary (`mandara_version::changelog`), so
`mandara changelog`, Studio → About → What's new and the update card all
read it, and its entry for each version becomes that GitHub release's
notes.

Every user-facing change adds a line under `## Unreleased` in the same
commit:

```markdown
## Unreleased

One short paragraph saying what the release is about.

### New          ← also: Improved, Fixed, Under the hood
- **Short lead.** What changed and why it matters, wrapped over indented
  lines as needed.
```

Keep the format strict (it is parsed): `##` version headings, a summary
paragraph, `###` sections and `-` items. The sections are New, Improved,
Fixed and Under the hood only (Studio styles and translates just those).
A unit test checks the shipped file, headings included. Text after a `---` rule in a release body (the install command) isn't
shown in Studio. Don't edit released entries except to fix mistakes, and
never add the version heading yourself: the release bump does it.

## Signed releases

Every release after 0.3.3 signs its `SHA256SUMS` with Mandara's release key
(Ed25519). The listing covers every asset, `update.json` and the patches
included, so one signature vouches for the whole release. Someone who can
replace release files but doesn't hold the key can't get them installed:

- **Updates** (`mandara update`, Studio → About → Update now) always require
  a valid `SHA256SUMS.sig`. `update.json` is used only when its hash matches
  its line in the signed listing (otherwise the tarball's signed line is
  used and no patch is trusted), and every later hash comes from there.
- **`mandara install`** requires the signature for releases after 0.3.3
  (`--version 0.3.3` or older installs with a note that it came out before
  signing). Its self-check trusts `update.json` or a local tarball only
  through the signed listing.

The trusted public keys are compiled in from
`tools/mandara-delta/release-keys.txt` (`mandara_delta::signing`). The
private key exists only in the `MANDARA_RELEASE_KEY` repository secret and
the owner's backup (`~/.local/share/mandara-release/release-key`, mode
0600). The publish job's **Sign checksums** step runs
`cargo xtask release-key sign dist/SHA256SUMS`, which also checks the
signature against the shipped keys, and the release fails without the
secret.

`cargo xtask release-key new` created the key: it hands the private half to
`gh secret set` on stdin (never printed), writes the backup, and adds the
public half to `release-keys.txt`. It refuses to replace an existing key,
secret or backup. **To change keys**, run it with `--force` (after moving
the old backup away), release once while the old key still signs (put the
old private key back in the secret for that release), then remove the old
line from `release-keys.txt`: clients only trust a new key after they've
installed a release that lists it.

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
`SHA256SUMS` for releases without one); both come from the signed listing. The new binary replaces the old one
atomically (`mandara.new` → fsync → rename) and the old one stays as
`mandara.old` (`mandara update rollback`). Package-managed installs (`/usr`,
`pacman -Qo` / `dpkg -S` / `rpm -qf`) and cargo builds are never replaced.

**In CI**, each build job runs, after `cargo xtask dist`:

```bash
# for each of the last DELTA_FROM_RELEASES (5) published releases:
gh release download <earlier tag> --pattern 'mandara-<old>-<target>.tar.gz'
cargo xtask update-part --target <triple> [--variant v3] --out dist \
  --previous mandara-<old1>-<target>.tar.gz --previous mandara-<old2>-<target>.tar.gz …
```

`update-part` extracts `bin/mandara` from each tarball, creates a patch
from every earlier release given (one per version), verifies each by
applying it with the updater's decoder, and writes this target's part of
the manifest (`*.update-part.json`) listing all of them. The earlier
releases are the newest `DELTA_FROM_RELEASES` (5, set at the top of
`release.yml`) published, non-pre-release `v*` releases other than the one
being built; on the first release, or for a target none of them had, the
part lists the full download only. The updater takes the patch whose
`from` is the installed version, so an install that skipped up to four
releases still updates by patch; anything older takes the full download.
The publish job merges the parts with `jq` into `update.json`, and patches
and manifest go into `SHA256SUMS` and the provenance attestation like
every other artifact. Each extra patch adds about a minute to a build job.

```json
{
  "schema": 1, "name": "mandara", "version": "0.2.0",
  "targets": {
    "x86_64-linux-gnu-x86-64-v3": {
      "archive": { "file": "mandara-0.2.0-x86_64-linux-gnu-x86-64-v3.tar.gz", "sha256": "…", "size": 41958862 },
      "binary": { "path": "bin/mandara", "sha256": "…", "size": 103729296 },
      "patches": [ { "from": "0.1.9", "from_sha256": "…", "file": "mandara-0.1.9-to-0.2.0-x86_64-linux-gnu-x86-64-v3.patch.zst", "sha256": "…", "size": 7223906 } ]
    }
  },
  "locales": {
    "ar": { "file": "mandara-0.2.0-ar.mlang.zst", "sha256": "…", "size": 214377, "pack_sha256": "…", "pack_size": 1015805 }
  }
}
```

A new binary can be **smaller** than the installed one (0.10.3 moved the
translations out). Patches don't care which way the size goes, sizes are
compared as signed numbers everywhere, and Studio's update card and
`mandara update` say how the new binary compares: "The new version is
9.2 MB smaller". `tests/self_update.rs` publishes a shrinking release and
checks the patch rebuilds it byte for byte.

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

## Language packs

Translations aren't compiled into the binary. Each language's
`crates/mandara-i18n/locales/<code>.toml` becomes a **language pack**
(`.mlang`) for one Mandara version, built by Rust code in the repository
(`mandara_i18n::pack`, driven by `cargo xtask langpacks`). English is the
keys and needs none.

**Format** (version 1, little-endian; the full table is in
`crates/mandara-i18n/src/pack.rs`): a 48-byte header (magic `MLNG`,
format, entry count, file length, slot counts, section offsets), the exact
Mandara version and the language code, then an index of 24-byte slots
(`key_hash u64, key_off u32, key_len u32, val_off u32, val_len u32`) and a
blob of UTF-8 strings (every key, then every translation). The index is a
hash table **sorted by hash**: each entry sits in its home slot
(`(hash × m) >> 64`, twice as many slots as entries) or just after the
entry before it, so a lookup reads one slot, rarely two, then compares the
stored key with the one asked for. A hash collision therefore can't return
the wrong string, and building a pack fails if two keys share a hash. The
hash is a folded-multiply hash over 8-byte words, documented with its
constants in the module (byte-at-a-time hashes like FNV-1a made lookups
slower than the old `HashMap`).

**Loading** maps the file read-only (`memmap2`) and validates it once:
magic, format, lengths and offsets, the version (it must equal the running
`mandara --version`) and language, the index's order and placement, every
string's bounds and UTF-8, and every key's hash. Anything wrong rejects the
pack (English is used), never a panic. Then strings are served as slices of
the mapping: no parsing, no allocation per string. Measured on the
development machine (release build, Arabic, 7,929 strings): validating the
pack takes 0.75 ms where parsing the TOML took 5 ms; a lookup takes 23 ns
against 27 ns for the old `HashMap` (14 against 17 ns for a hot set of 200
strings); the shell's heap in Arabic is about 2 MB smaller (sandbox, idle,
anonymous RSS 33.3 against 35.3 MB). Until a language's pack is loaded,
`mandara_i18n::language()` answers English, so layout direction, dates and
digits match the English text shown meanwhile. `cargo test --release -p mandara-i18n --test packs --
--ignored --nocapture` repeats the measurement.

**Never changed under a live mapping.** Mandara writes a pack to a
temporary file in the same folder, syncs it and renames it over the old
one, and pruning only unlinks: a process that has the old file mapped keeps
reading the old inode, so a download or update can never truncate a mapped
file (`SIGBUS`). Package managers replace files the same way.

**Where they live**, first match wins:
`$XDG_DATA_HOME/mandara/locales/<v>/<code>.mlang` (downloads, and what
`mandara install` copies from a tarball), `<exe dir>/../share/mandara/locales/<v>/`
(an unpacked tarball, or `/usr/bin` → `/usr/share`), then every
`$XDG_DATA_DIRS` entry plus `/usr/local/share` and `/usr/share`
(packages; Mandara Login's copy reads `/usr/local/share`, where
`mandara greeter use mandara` and `greeter apply-config` install its
language's pack). Debug builds without a pack compile the source catalog
in memory, so `cargo test` and `cargo run` stay translated; release builds
never read the sources.

**In CI:** the publish job runs
`cargo xtask langpacks --version <v> --out dist`, which writes
`mandara-<v>-<code>.mlang`, its zstd-compressed download
`mandara-<v>-<code>.mlang.zst` (checked by decompressing it with the
client's pure-Rust decoder) and the `locales` part of `update.json`
(`file`, `sha256` and `size` of the `.zst`; `pack_sha256` and `pack_size`
of the `.mlang`). The packs are architecture-independent and reproducible,
so they are built once. `cargo xtask dist` puts every pack in each tarball
(`share/mandara/locales/<v>/`), and the package jobs stage them in
`dist/.package-inputs/langpacks/<v>/` for the `.deb` and `.rpm` (installed in
`/usr/share/mandara/locales/<v>/`); the Arch recipes copy them from the
tarball or build them with `cargo xtask langpacks`.
These are generated package inputs rather than Cargo build products.
The artifact upload excludes the hidden staging tree; only finished
packages and release files leave the job.

**Downloading** (`mandara_services::self_update::langpacks`): a language
used without its pack (the shell's, a plugin's own, a desktop widget's) or
one just chosen whose pack differs from the published one is fetched from
**the running version's release** (`releases/tags/v<running>`), whose
`SHA256SUMS` must carry a valid signature; `update.json` must match its
signed line; the download must match `sha256`, the decompressed pack
`pack_sha256` and its header the version and language. A lock file next to
the pack keeps the shell and Studio from downloading it twice. After an
update installs a new binary, the updater downloads the new version's
packs for every language in use into its folder, then prunes folders older
than the version it replaced (that one stays for a rollback).
`crates/mandara-services/tests/langpacks.rs` runs all of this against a
local mock release, including a tampered download, a pack built for
another version and an unsigned release.

## Memory allocator

Release and CI builds use `mandara-app`'s default feature,
`alloc-system-tuned`: glibc malloc limited to 2 arenas (`mallopt(M_ARENA_MAX)`
at startup) with a fixed 512 KiB mmap threshold (see [Resource
budget](#resource-budget)). Clipboard and media views request `malloc_trim(0)`
after releasing their cached images; a 30 s fallback also catches other freed
storage. Requests coalesce and run on a short-lived blocking worker, outside
the UI and shared service executor threads. No second allocator is loaded.
The other allocators remain
selectable (`--no-default-features --features alloc-jemalloc |
alloc-mimalloc | alloc-mimalloc-v2 | alloc-system`), and CI checks that each
one builds. mimalloc and jemalloc compile C code with the runner's
`build-essential` (native x86_64 and aarch64 runners, no cross-compiling).

Thumbnail batches decode sequentially and deliver each result through a bounded
channel. Page changes wait for the current worker; closing invalidates its
generation so old results cannot refill an evicted cache. Cache capacity and
retained image headers are allocated before transient decode buffers. Bulk
eviction releases the images and spare cache capacity before requesting a trim.
glibc can then return whole free pages from all arenas, though live allocations
and fragmentation can still keep pages resident.

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
  `residency.rs`, 8 s and 45 s after start, then every 10 min; the 8 s
  drop is described under [Start-up memory](#start-up-memory)): clean read-only file
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
* **RELR relocations** (`mandara-app/build.rs`, `-z pack-relative-relocs`,
  x86 GNU ld targets only):
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
playing. This historical comparison includes the executable-cache discard
that has since been removed; it does not describe the current memory limit.
RSS / file-backed / code, MiB:

| Build | Page cache | Settled | After activity | Panel frames p50 / build p95 | Launcher IPC | Audio CPU | Start → IPC |
|---|---|---|---|---|---|---|---|
| before | read from disk | 43.7 / 23.6 / 15.8 | 76.9 / 52.5 / 30.1 | 6.08 / 0.056 ms | 20 ms | 1.98 % | 300 ms |
| before | just written (`cp`) | 73.8 / 54.3 / 37.7 | — | — | — | — | — |
| page-cache refresh + cold sections | just written | 52.6 / 32.6 / 18.3 | 76.4 / 52.2 / 30.6 | 6.07 / 0.050 ms | 20 ms | see below | see below |

Settled code varies 15–23 MiB between runs (which minute boundary, weather
fetch or D-Bus burst falls before the snapshot); file-backed memory also
holds libLLVM / RADV / font pages. After activity the code stays until the
next 10-minute drop. These historical runs reduced the resident code of a
freshly written binary without shrinking its working set (see PGO below).
They do not establish the memory use after removing executable-cache discard.
Start-up and audio-visualizer CPU of that earlier pair of changes on top
of the `opt-level` list below, on a quiet machine: 321 ms and 1.73 % (before: 300 ms,
1.98 %); the final binary was only re-timed under an unrelated 27-core
build, so re-check those two on a quiet machine before quoting them.

* **Clean mapping cleanup** (`residency.rs`): read-only file mappings
  without private copies or dirty pages are still advised with
  `MADV_DONTNEED` after 8 s, after 45 s and every 10 minutes afterwards.
  The shell no longer probes or discards the running executable's file
  cache. That forced discard caused disk faults and slow frames at the
  45 s cleanup in release tests. The table above describes the older
  page-cache-refresh experiment; it is not a memory ceiling for the
  current behavior. The kernel controls ordinary file-cache reclaim.
* **Cold functions grouped** (`mandara-app/build.rs`,
  `-z keep-text-section-prefix`): what LLVM marks cold (`#[cold]`, panic and
  error paths) goes to its own 2.2 MB `.text.unlikely` instead of sitting
  between live functions. Builds probe the configured toolchain before
  enabling this flag; linkers that do not support it keep their normal
  section layout.

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

### Start-up memory

The idle numbers above are taken once the shell has settled. In the first
45 seconds it held far more: the start-up code of the binary, the pages
RADV's libLLVM touches when it loads, and the fonts it scanned stayed
resident until the first cold-page drop, and loading a wallpaper briefly
held several full-size copies of it. Measured 2026-11-03 in the demo
sandbox (`cargo xtask sandbox up`, Vulkan, scale 1.5, three runs each,
median and range; *peak* is `VmHWM` 20 s after start, *at 15 s* and
*settled* (median over 60–90 s) are `smaps_rollup` RSS sampled every
~0.12 s), release builds, MiB:

| Setup | Peak before → after | At 15 s before → after | Settled before → after |
|---|---|---|---|
| 3840 × 2160, default config | 112.0 (112.0–112.1) → 101.0 (100.9–101.8) | 111.1 (110.3–111.3) → 37.3 (37.3–37.9) | 45.8 (40.2–45.8) → 42.0 (41.0–45.8) |
| 1600 × 1000, default config | 111.8 (111.3–112.2) → 102.4 (101.9–103.5) | 111.3 (110.2–111.5) → 37.5 (36.9–37.5) | 42.2 (42.0–45.7) → 45.3 (42.2–45.8) |
| 3840 × 2160, demo config (1080p wallpaper, depth clock, dock) | 240.8 (240.7–241.0) → 203.6 (202.9–204.6) | 122.6 (122.5–122.6) → 39.3 (39.1–39.9) | 45.0 (44.2–54.2) → 43.4 (43.1–50.8) |
| 3840 × 2160, demo config with a 4K JPEG wallpaper | 198.4 (198.2–198.8) → 168.3 (167.3–169.3) | — | — |

Settled RSS is within run-to-run noise, and anonymous memory (18.1–18.3
MiB default, 19.8–20.3 MiB demo) and video memory (163.5 MiB default and
178.5 MiB demo at 4K, 57.7 MiB at 1600 × 1000) are unchanged, also after
opening and closing the sidebar, the launcher and a panel.

* **An early cold-page drop** (`residency.rs`, `EARLY` = 8 s): the drop
  described above also runs 8 s after start. The shell is up and has drawn
  within about a second; what the idle shell still runs maps back with a
  minor fault. The 45 s and 10-minute clean-mapping drops remain;
  the executable-cache discard has since been removed. −73 to −83 MiB resident from 8 s to 45 s.
* **One face per fallback-chain family** (`vendor/gpui_wgpu` patch 11, and
  `lock::paint::TextPainter` asking a fallback family for its weight only
  when the text reaches it): an English desktop mapped and parsed every
  face of the five installed Arabic families at its first frame. 52 → 2
  font files mapped (default config; 53 → 4 with the desktop clock), font
  pages resident at 4 s 11.0 → 4.8 MiB (17.5 → 8.2 MiB). It is all of
  the −9 to −11 MiB peak with the default config (no wallpaper there).
* **A wallpaper placed without full-size copies** (`backdrop::paint::background`):
  the picture is resampled before the canvas is allocated, the source
  dropped once resampled, and copied onto the canvas from its own RGB or
  RGBA pixels instead of an RGBA copy. At 4K that is one 31.6 MiB buffer
  less at the peak of a wallpaper load (a same-size wallpaper: decoded
  picture + canvas instead of picture + RGBA copy + canvas; an upscaled
  one: the canvas no longer exists beside the resampler's float buffer).
  The rest of the demo rows' drop: −20 to −28 MiB peak, at start and on
  every wallpaper change. Pixels are identical (unit test
  `rgb_pictures_are_placed_like_their_rgba_copies`).

Frame times are unchanged: opening and closing the sidebar, launcher and
media panel (demo config at 4K, three runs each), view build p50 0.096–0.123
→ 0.093–0.118 ms and p95 0.158–0.187 → 0.140–0.183 ms, frame interval p50
16.26–16.33 → 16.27–16.30 ms (the sandbox output runs at 60 Hz).

Guards: `crates/mandara-ui/tests/startup_budget.rs` (registering every
built-in plugin keeps ≤ 4 MiB, allocates ≤ 16 MiB and starts no thread;
today 0.2 / 9.2 MiB), `residency` unit tests (classification, private
copies survive a drop, the folio probe unmaps its mapping),
`vulkan_is_tried_alone_before_gl`,
`cut_source_crops_to_the_subject` (released cut keeps geometry), the
`icon_raster` decode-size tests, and `cargo xtask resource-check` before a
release (see [development.md](development.md)).

### Settled memory, video memory and residue

Measured 2026-11-03 in the demo sandbox (Vulkan / RADV, scale 1.5, the
shell in a scope that cannot swap, the binary evicted from the page cache
before each start, base and new binary alternating, median and range),
release builds, MiB.

What the settled shell holds, default config at 3840 × 2160: anonymous
memory 18.2 (18.1–18.3). By mapping, 40 s after start: the main malloc heap
10.1, the binary's relocated read-only data 3.6, other anonymous mappings
3.2 (a second malloc arena's 1.3 and GPUI's 1 MiB arena chunks among them),
libLLVM's relocated data 1.3 (RADV links it), the rest under 1. A
sampling heap profiler (a sandbox-only build, not shipped) puts the Rust
heap at 7.2 MiB live: GPUI's element and event arenas 2.0 (allocated,
touched as used), the shaders' naga IR 1.1 (wgpu keeps it beside the
pipelines because its Vulkan backend compiles each entry point
separately), the font database 0.8 (1,421 installed faces), GPUI's frame
structures 0.6, D-Bus message queues 0.55, the command and plugin
registry 0.44, taffy 0.37. The rest of the heap is Mesa's. Nothing there is
built for something the default config does not use; the language
catalogs, for one, are parsed only for the active language (none for
English).

Video memory: 143.4 at 3840 × 2160 and 37.8 at 1600 × 1000, the same in
every run. Most of the 105.6 MiB difference is the full-output bar
surface's four swapchain images (4 bytes per pixel each): they are
102.2 MiB larger at 3840 × 2160 than at 1600 × 1000. Mesa's Wayland WSI
requires four images for mailbox presentation; FIFO could use two but
blocks the UI thread whenever the output stops sending frame callbacks, so
it stays as it is.

Residue: opening and closing the launcher, the sidebar and the media,
quick settings, clipboard and notification panels three times, plus three
notifications, leaves anonymous memory at 23.7 (23.5–24.8) 30 s later
(+5.5), with video memory back where it was. Ten rounds level off at the
same height from the second round on, so it's bounded: GPUI keeps frame
buffers and layout storage at their peak (+1.9 live in the heap profile),
and the rest is malloc fragmentation. Giving it back would make the next
open pay for it again. Resident code grows by about 25 MiB after such use
until the next 10-minute drop (see [Resident code](#resident-code)). A
wallpaper switch leaves nothing behind. Studio runs as its own process.

Threads: 27–28 with the default config (a job thread comes and goes). The main
thread; GPUI's four workers, its timer and three threads of its
accessibility bridge and power listener (an executor and a D-Bus
connection among them); two service executors, the async-io reactor and
one blocking-pool thread; four Mesa shader-cache threads (two per Vulkan
GPU, and the test machine has an integrated one too); three file watchers
(config, plugin folders, notes); and one thread each for IPC, the
wallpaper, toplevels, the clipboard, compositor events, audio events and
clock steps. The file watchers could share one thread and the clock-step
timer could run on a service executor; neither changes memory measurably,
so they were left.

* **Wallpapers and depth masks resampled one row at a time**
  (`resample::triangle`, used by `wallpaper::shrink_region`,
  `backdrop::paint::background` and the depth mask's `align`). `image`'s
  triangle-filter resize first filters the whole picture vertically into
  an `f32` RGBA copy of *source width × new height* (16 bytes a pixel),
  plus a cropped copy of the source: 63 MiB to bring a 1080p wallpaper to
  4K, 95 MiB for a 2880 × 1620 one, 53 MiB for a 1600 × 900 depth mask.
  The new resampler does the same arithmetic in the same order with one
  filtered row alive at a time and reads the crop in place. Pixels are
  identical (unit test `matches_image_resize_exactly`), and it is faster:
  1920 × 1080 → 3840 × 2160 in 53 instead of 96 ms, 1920 × 1080 →
  1600 × 1000 in 13 instead of 24 ms. Peak RSS, demo config (a 1080p
  wallpaper with the depth clock); a switch's peak is `VmHWM` reset just
  before it:

  | Setup | Before | After |
  |---|---|---|
  | 3840 × 2160, start-up (`VmHWM` at 20 s) | 204.9 (204.1–205.0) | 169.9 (169.5–170.8) |
  | 3840 × 2160, switch to a 2880 × 1620 wallpaper | 201.6 (193.6–205.3) | 124.7 (123.1–125.4) |
  | 3840 × 2160, switch to one with a depth mask | 238.0 (229.5–239.9) | 149.4 (149.3–149.8) |
  | 3840 × 2160, switch to the 1080p wallpaper | 160.5 (152.1–161.8) | 127.2 (126.9–127.3) |
  | 1600 × 1000, start-up | 147.1 (145.6–148.0) | 116.2 (115.6–117.2) |
  | 1600 × 1000, switch to one with a depth mask | 102.3 (102.2–103.1) | 86.9 (86.1–87.1) |
  | 1600 × 1000, switch to the 1080p wallpaper | 104.8 (104.7–106.0) | 74.0 (73.8–74.6) |

  A 2880 × 1620 wallpaper on the 1600 × 1000 output is box-averaged as
  before (71.6 → 71.4). Settled anonymous memory (21.1 at 4K, 21.0 at
  1600 × 1000), video memory (158.4 / 44.4) and the default config's
  numbers above are unchanged, and so are frame times: panel workload at
  4K, view build p50 0.060 (0.058–0.062) → 0.061 (0.058–0.062) ms, p95
  0.095 (0.091–0.101) → 0.091 (0.091–0.098) ms, frame interval p50 16.1 ms
  in every run (the sandbox output runs at 60 Hz).

## The installer and releases

The Rust binary performs the installation (`crates/mandara-install`). A
small POSIX bootstrap downloads and verifies the release before opening
`bin/mandara install`; users can also extract a tarball and run it directly:

1. It checks its own SHA-256 against the release's `update.json` (the
   binary hash of its target), else against a tarball sitting next to the
   extracted folder and `SHA256SUMS`, refusing a mismatch (`--no-verify`
   overrides). For signed releases both count only through the signed
   `SHA256SUMS`. Cargo builds from a checkout skip the check; debug builds are
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
downloads the tarball and checks it against `SHA256SUMS` and its signature,
refusing to install on a mismatch or a missing or wrong signature. It falls back to building from source when there is no
matching binary (other architectures). `--from-source` builds with
`cargo xtask build` (installing rustup's own `rustup-init` binary, checksum
verified, when Rust is missing). `MANDARA_RELEASE_BASE=<url>` (with
`--version`) installs from a mirror. `--dry-run` prints every step and
changes nothing.

Every release also carries version-less copies of the tarballs
(`mandara-<target>.tar.gz`), so
`https://github.com/theHamdiz/mandara/releases/latest/download/mandara-x86_64-linux-gnu.tar.gz`
always points at the newest build.

The public download command is
`sh -c 'f=$(mktemp) && trap "rm -f -- \"\$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/mandara/install.sh -o "$f" && sh "$f" "$@"' sh`.
Pages serves the tracked POSIX bootstrap, which checks `SHA256SUMS.sig`
against the same public keys as the binary, then checks the archive before
extraction. Its loopback integration tests cover argument forwarding,
installer exit codes, cleanup, invalid signatures, damaged archives and
missing metadata. `cargo xtask bootstrap` refreshes its public keys and
translated messages from the same sources as the application; `--check`
and the tests reject a stale script. The actual installer and release tooling
remain Rust.
