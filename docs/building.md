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

The Nix flake uses the official stable Rust binary toolchain, pinned through
`rust-overlay` in `flake.lock`. This keeps Rust's CPU intrinsics and LLVM
matched. `nix build .#checks.x86_64-linux.rust-intrinsics` checks the AVX-512
VNNI compiler path without requiring that instruction set on the builder.

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

`vendor/gpui_linux` is GPUI's Linux platform with touchscreen input: its
Wayland client takes the seat's touch device, so a tap on the bar, a panel
or Studio clicks like the left mouse button, a swipe scrolls, and a finger
on a slider drags it
([`MANDARA-PATCHES.md`](../vendor/gpui_linux/MANDARA-PATCHES.md)).

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
| Version | `0.22.1-dev` |
| Source fingerprint | `b59c219a62f6` |
| Toolchain | `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, LLVM 23.1.1 |
| Target · profile | `x86_64-unknown-linux-gnu` · `release` |
| Features | `alloc-system-tuned` (default) |
| `mandara` SHA-256 | `73d74433cd2fa300…` |
| Rust | 300,241 code · 40,980 comments · 25,425 blank · 17 crates · 876 files |
| Vendored Rust | 101,457 code · 17,415 comments · 12,683 blank · 155 files |
| Plugin examples | 265 code · 38 comments · 37 blank · 1 file |
| Locales · WIT | 164,768 lines in 38 catalogs · 83 lines in 1 WIT file |
| Built | 2026-11-08 14:29:05 UTC |

<details><summary>Full hashes and lines per crate</summary>

```text
source fingerprint  b59c219a62f6cfb6a69824a1cc44638aa885981ec78ba6d6802d7ac299e96874
mandara             73d74433cd2fa300db82a7cfaa2581a91c71f9ff3df15fa054b307c48f251131
```

| Crate | Files | Code | Comments | Blank |
|---|---:|---:|---:|---:|
| `mandara-app` | 13 | 2,127 | 359 | 172 |
| `mandara-compositor` | 15 | 4,242 | 517 | 509 |
| `mandara-config` | 50 | 21,916 | 4,689 | 2,164 |
| `mandara-core` | 16 | 4,497 | 843 | 478 |
| `mandara-greeter` | 21 | 8,267 | 1,079 | 722 |
| `mandara-i18n` | 6 | 1,372 | 355 | 158 |
| `mandara-install` | 16 | 4,534 | 722 | 447 |
| `mandara-media` | 10 | 2,133 | 296 | 204 |
| `mandara-plugins` | 12 | 2,552 | 499 | 338 |
| `mandara-services` | 201 | 55,950 | 7,710 | 5,760 |
| `mandara-studio` | 95 | 38,986 | 4,061 | 2,380 |
| `mandara-theme` | 11 | 3,514 | 447 | 326 |
| `mandara-ui` | 349 | 136,730 | 16,968 | 10,524 |
| `mandara-version` | 4 | 907 | 191 | 92 |
| `mandara-wallgen` | 31 | 5,108 | 1,290 | 461 |
| `mandara-webp` | 1 | 637 | 77 | 47 |
| `xtask` | 25 | 6,769 | 877 | 643 |
| **Total** | **876** | **300,241** | **40,980** | **25,425** |

</details>
<!-- build-info:end -->

Script-only fallback fonts do not need Latin glyphs. The vendored renderer
loads their selected face directly; its Devanagari regression font is test-only
and licensed under the SIL Open Font License. Primary font validation remains
separate from fallback selection.
