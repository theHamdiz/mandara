# Building from source

```bash
cargo build --release -p mandara-app      # → target/release/mandara
cargo test --workspace
cargo xtask build                         # release build + fingerprint, hashes and line count
```

**Contributing**, by hand or with a coding agent: [AGENTS.md](../AGENTS.md)
covers the repository map, the checks to run, the hard rules (all-Rust,
every string translated, no magic numbers, non-destructive dotfiles), the
design principles and how to add a plugin, setting, command or compositor
feature. [docs/development.md](development.md) has the demo sandbox.

Or let cargo fetch and build it, then install the result like a release
(packages, compositor, login screen):

```bash
cargo install --locked --git https://github.com/theHamdiz/mandara mandara-app
mandara install
```

`cargo xtask build` is the full build recipe. It fingerprints every build
input (SHA-256 over the sources, lock file, toolchain, target, profile and
features; no git or timestamps involved), builds the release binary with
that fingerprint embedded (`mandara --version` shows it), hashes the
binary, counts the lines of code, writes a build manifest next to it and
refreshes the [build fingerprint](#build-fingerprint) table below.

The source build requires **Rust 1.97 or newer**, matching the WebAssembly
runtime’s minimum version. The installer updates an older rustup toolchain
before attempting a source build.

## Dependency updates

The committed lockfiles cover the shell and the standalone delta tool.
Use `--locked` for reproducible builds. When updating the OCR geometry
dependency, run `cargo test -p mandara-services --test ocr_geometry`: it
checks pixel padding, text orientation and detected region bounds without
downloading inference models. Keep the geometry adapter's constant-black
border and its parallel row processing.

## Build options: memory allocator

The `mandara` binary picks its global allocator at build time with exactly
one `alloc-*` feature of `mandara-app`:

| Feature | Allocator | Notes |
|---|---|---|
| `alloc-system-tuned` (default) | glibc malloc, tuned | no C allocator: at most 2 arenas, `malloc_trim(0)` every 30 s |
| `alloc-jemalloc` | jemalloc | C (`tikv-jemallocator`); background purge thread, 2 arenas, short decay |
| `alloc-mimalloc` | mimalloc v3 | C, vendored by the `mimalloc` crate |
| `alloc-mimalloc-v2` | mimalloc v2 | the previous mimalloc line |
| `alloc-system` | glibc malloc | untouched, for comparison |

```bash
# the default (tuned system allocator)
cargo build --release -p mandara-app
# jemalloc instead (needs a C compiler)
cargo build --release -p mandara-app --no-default-features --features alloc-jemalloc
```

The allocator only serves Rust allocations; the C libraries the shell loads
(Mesa, FreeType, fontconfig) keep using glibc malloc. The startup log names
the allocator (`config: … (allocator: …)`). The measurements behind the
default are in [docs/releasing.md](releasing.md#memory-allocator).

The workspace contains these crates:

| Crate | Role |
|---|---|
| `mandara-app` | binary, CLI, IPC, supervisor, setup |
| `mandara-core` | layer-shell surfaces, input masking, springs, OKLCH |
| `mandara-config` | reactive config store, compositor emitters |
| `mandara-theme` | HCT + OKLCH engine, presets, app theme templates |
| `mandara-compositor` | MangoWM, Niri, Hyprland, Umbriel, Sway, River and KWin drivers |
| `mandara-services` | D-Bus services, audio, spectrum, OCR, screenshots, recording, Wallhaven, marketplace |
| `mandara-plugins` | plugin registry, commands, WebAssembly host |
| `mandara-i18n` | localization |
| `mandara-version` | build identity: per-component versions and content hashes |
| `xtask` | dev and release tooling: `cargo xtask status / bump / verify / dist`, `resource-check`, the demo `sandbox` ([development.md](development.md)) |
| `mandara-ui` | the shell's views and built-in plugins |
| `mandara-studio` | Mandara Studio |

`vendor/gpui_wgpu` is GPUI's wgpu renderer with Mandara's video-memory
patches, applied through `[patch]` in the workspace manifest; its
`MANDARA-PATCHES.md` lists each change and its measurements. `vendor/gpui`
is GPUI itself with rounded content masks, so a rounded card that hides its
overflow clips covers and art to its corners, and hover styles that follow
the pointer on every element, with or without an id
([`MANDARA-PATCHES.md`](../vendor/gpui/MANDARA-PATCHES.md)).
`vendor/pure-onnx-ocr` is the OCR engine with upstream's sources and a
manifest that no longer turns on every codec of the `image` crate
([`MANDARA-PATCHES.md`](../vendor/pure-onnx-ocr/MANDARA-PATCHES.md)).

`vendor/xkbcommon` keeps GPUI’s keyboard API while loading the system
library only when keyboard input is used. This lets `mandara install`
start before the runtime packages have been installed
([`MANDARA-PATCHES.md`](../vendor/xkbcommon/MANDARA-PATCHES.md)).

## Build fingerprint

The last `cargo xtask build`: source fingerprint, toolchain, binary hashes and
lines of code ([`docs/releasing.md`](releasing.md#build-fingerprint)).

<!-- build-info:start -->
<!-- Generated by `cargo xtask build`; edits here are overwritten. -->
| Build | |
|---|---|
| Version | `0.1.0` |
| Source fingerprint | `aaecce37fdb5` |
| Toolchain | `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, LLVM 23.1.1 |
| Target · profile | `x86_64-unknown-linux-gnu` · `release` |
| Features | `alloc-system-tuned` (default) |
| `mandara` SHA-256 | `84b3fb54688e0204…` |
| Rust | 177,066 code · 22,447 comments · 14,589 blank · 14 crates · 424 files |
| Vendored Rust | 6,044 code · 431 comments · 588 blank · 6 files |
| Plugin examples | 235 code · 31 comments · 34 blank · 1 file |
| Locales · WIT | 48,344 lines in 18 catalogs · 60 lines in 1 WIT file |
| Built | 2026-10-29 09:25:37 UTC |

<details><summary>Full hashes and lines per crate</summary>

```text
source fingerprint  aaecce37fdb5505a6d92018aad30c989f7a780e0c67abf331cc98466161c4404
mandara             84b3fb54688e02045176ef3e74f124394af3252ea77d7168e21aa660dcec8b41
```

| Crate | Files | Code | Comments | Blank |
|---|---:|---:|---:|---:|
| `mandara-app` | 6 | 1,278 | 310 | 94 |
| `mandara-compositor` | 6 | 1,457 | 160 | 185 |
| `mandara-config` | 22 | 11,912 | 2,442 | 1,212 |
| `mandara-core` | 12 | 3,355 | 612 | 342 |
| `mandara-greeter` | 29 | 8,999 | 1,106 | 800 |
| `mandara-i18n` | 2 | 374 | 55 | 43 |
| `mandara-install` | 11 | 2,546 | 364 | 277 |
| `mandara-plugins` | 8 | 1,890 | 318 | 239 |
| `mandara-services` | 100 | 34,346 | 4,531 | 3,427 |
| `mandara-studio` | 53 | 26,482 | 2,513 | 1,488 |
| `mandara-theme` | 8 | 2,802 | 330 | 266 |
| `mandara-ui` | 150 | 76,817 | 9,037 | 5,725 |
| `mandara-version` | 3 | 470 | 99 | 50 |
| `xtask` | 14 | 4,338 | 570 | 441 |
| **Total** | **424** | **177,066** | **22,447** | **14,589** |

</details>
<!-- build-info:end -->

Script-only fallback fonts do not need Latin glyphs. The vendored renderer
loads their selected face directly; its Devanagari regression font is test-only
and licensed under the SIL Open Font License. Primary font validation remains
separate from fallback selection.
