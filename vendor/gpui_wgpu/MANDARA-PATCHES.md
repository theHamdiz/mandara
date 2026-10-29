# Mandara patches to `gpui_wgpu`

Vendored from [zed-industries/zed](https://github.com/zed-industries/zed) at
`ec86047d67acd6fddd81392f44ed6ebcc6e7f531` (`crates/gpui_wgpu`, Apache-2.0, see
`LICENSE-APACHE`) and wired in through the workspace's
`[patch."https://github.com/zed-industries/zed"]`. Every source change is marked
`Mandara patch` in a comment. The goal is the per-window GPU memory of a desktop
shell: a few large, mostly transparent layer surfaces that rarely draw paths.

Measured with `cargo run -p mandara-core --example vram_probe -- W H 2`
(invisible background-layer windows on a 1.5× output, process VRAM from DRM
fdinfo; "second window" excludes the one-time device / pipeline cost):

| Window (logical px) | Upstream, 2nd window | Patched, 2nd window |
|---|---|---|
| 2560 × 1440 (3840 × 2160 device) | 290 MiB | 131 MiB |
| 1280 × 720 | 129 MiB | 37 MiB |
| 420 × 520 | 40 MiB | 12 MiB |
| 100 × 30 | 4 MiB | 4 MiB |

The first window also carries the device, pipelines and allocator blocks:
43 → 39 MiB at 100 × 30. The live shell (bar/panel surface + wallpaper
surface, both full-output, plus exclusion strips) went from 677 to 342 MiB.

`vram_probe … paths=<side>` keeps a small ring path drawn (like the bar's
progress rings) and animates a big one in a `side²` box (like an opened
sidebar), then reports what is left 150 frames later: 167–183 MiB after vs.
170–234 MiB with wgpu's `MemoryUsage` blocks (patch 5).

What remains at full size is the swapchain: Mesa's Wayland WSI reports
`minImageCount = 4`, so wgpu creates 4 images whatever the present mode or
frame latency (patch 3 cannot lower it without changes in `wgpu-hal`).

## 1. Lazy, bounding-box-sized path textures (`wgpu_renderer.rs`, `shaders.wgsl`)

Upstream `ensure_intermediate_textures` allocated, on the first frame of every
window, a full-frame path intermediate texture (4 B/px) plus a 4× MSAA target
(16 B/px) — ~166 MiB for a 3840 × 2160 surface that may never draw a path.

* `prepare_path_textures` replaces it: frames without paths allocate nothing;
  frames with paths get textures covering only the union of the paths'
  clipped bounds (clamped to the frame, rounded up to
  `PATH_TEXTURE_GRANULARITY_PX` so an animating sparkline reuses them).
* They are dropped after `PATH_TEXTURE_IDLE_FRAMES` frames without paths and
  reallocated smaller after the same number of frames in which they were more
  than `PATH_TEXTURE_SHRINK_FACTOR`× larger than needed.
* `GlobalParams` gains `path_texture_origin` / `path_texture_size`. Path
  rasterization shifts vertices into texture space (its globals' viewport is
  the texture) and evaluates gradients back in frame space; the composite pass
  maps frame positions to texture coordinates through the same rect. With
  origin 0 and the texture at frame size the shaders compute exactly what
  upstream did, and the integer origin keeps sampling texel-exact.

## 2. Configurable MSAA sample count (`memory_tuning.rs`)

`gpui_wgpu::set_path_sample_count(n)` (or `GPUI_PATH_SAMPLE_COUNT=1|2|4`, which
wins) picks the path MSAA count; the renderer uses the largest count ≤ `n`
the *device* accepts. WebGPU guarantees 1 and 4; 2 and 8 also need
`TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, which GPUI does not request, so
the candidates are filtered by it (taking the adapter's word for 2 produced a
validation error every frame on RADV). Default unchanged (4): at 1× the bar's
progress rings are visibly stair-stepped, and with patch 1 the MSAA target
only covers the paths' box (`n × 4` bytes per pixel of it), so 4× costs
~1 MiB for the bar.

## 3. Present mode / frame latency knobs (`memory_tuning.rs`)

`set_present_mode(Option<PresentMode>)` / `GPUI_PRESENT_MODE` override the
platform's choice (Wayland: `Mailbox`), `set_max_frame_latency(n)` /
`GPUI_MAX_FRAME_LATENCY` replace the hard-coded `desired_maximum_frame_latency:
2`. Defaults unchanged. On Mesa/RADV + Wayland every combination still yields 4
swapchain images (wgpu-hal sizes the swapchain from the mode-independent
`minImageCount`), so Mandara keeps the defaults; the knobs exist for drivers
where they do help and for experiments.

## 4. Smaller initial atlas textures and instance buffer

* `wgpu_atlas.rs`: the first atlas texture of each kind starts at
  `INITIAL_ATLAS_EDGE_PX` (256²) instead of 1024², and each further texture of
  that kind doubles the edge up to upstream's 1024 (`MAX_DEFAULT_ATLAS_EDGE_PX`);
  images larger than that still get a texture of their own size. Saves up to
  ~4 MiB per window and kind for windows with a few glyphs and icons.
* `wgpu_renderer.rs`: the instance buffer starts at `INITIAL_INSTANCE_DATA_SIZE`
  (256 KiB) instead of 2 MiB; it already doubles on demand.

There are no depth/stencil buffers to trim.

## 5. Smaller suballocation blocks (`wgpu_context.rs`)

GPUI asked wgpu for `MemoryHints::MemoryUsage`: resources are suballocated
from device-memory blocks growing to 64 MiB, and `gpu-allocator` keeps its last
block even when empty. A shell that briefly needed a large path texture (the
sidebar opening while the bar draws rings: a 3328 × 640 box) kept 64 MiB of
VRAM forever. `MemoryHints::Manual` with 4–16 MiB blocks
(`MIN/MAX_DEVICE_MEMORY_BLOCK_BYTES`) caps that at 16 MiB; larger resources
get their own allocation, returned to the driver when dropped.

## 6. Script fallback chain and variable font weights (`cosmic_text_system.rs`)

Arabic set in Inter at semibold (Studio section headings, the selected nav
row, card titles) rendered in *Reem Kufi*, a decorative Kufic display face:
upstream passes no fallback chain for fonts without `FontFallbacks`, and
cosmic-text's own fallback only accepts a face whose weight matches exactly
(or a variable face whose `wght` axis covers it). Noto Sans Arabic ships no
600, so the search fell through to *any* installed face with a 600 that covers
Arabic. Which face that is depends on the machine.

* `set_default_font_fallbacks(families)`: a process-wide chain used for every
  font that has no chain of its own. Mandara sets it from
  `mandara_ui::fonts::fallback_chain`: the configured Arabic font, then Cairo,
  Alexandria, Noto Sans Arabic, and so on.
* Chain spans ask each family for the weight it actually has (CSS Fonts 4
  § 5.2 matching, `css_nearest_weight`), or the exact weight when the family is
  variable, so cosmic-text never skips a chain family.
* ASCII spaces and punctuation between two chain spans join them
  (`absorb_neutral_spans`). cosmic-text resolves the direction of a
  punctuation-only span on its own, which moved the ":" of RTL text to the end
  of the line.
* Lines are laid out with `Wrap::Word` and no width instead of `Wrap::None`.
  cosmic-text 0.19's `Wrap::None` path gives a leading run whose direction
  differs from the line's an empty range, so the "5" of Arabic "5 د" (and the
  digits of every "{n} min" chip) vanished. With no width the word-wrapping path
  never wraps, so the output is still a single line.
* Word spaces drawn by a fallback-chain face are at least
  `FALLBACK_MIN_WORD_SPACE_EM` (0.34 em) wide; glyphs visually after a
  widened space shift by the difference. Cairo's 0.22 em space and its kerning
  around initial forms made «البحث في» read as one word at 12–13 px.
* Variable faces are shaped and drawn at the requested weight. Upstream
  shaped and drew every variable face at its default instance, so Alexandria
  semibold rendered as regular. `font_id` returns a per-weight *instance*
  (`LoadedFont::instance_weight` plus normalized `wght` coords), and glyphs
  from cosmic-text's own fallback map to an instance at the weight they were
  shaped at. Rasterization, advances and metrics use the instance's coords.
* `CosmicTextSystem::face_summary(font_id)` (family, drawn weight, GSUB
  feature tags) supports the regression tests in `mandara-ui/src/fonts.rs`.

## 7. Vulkan first, OpenGL only as a fallback (`wgpu_context.rs`, `wgpu_renderer.rs`)

Upstream creates one `wgpu::Instance` with `VULKAN | GL` and enumerates every
adapter. Creating the GL half initializes EGL and the Mesa GL driver even when
a Vulkan adapter is then selected: on the RX 9070 XT (RADV + radeonsi) that
mapped `libgallium` (~9.5 MiB resident), had radeonsi touch ~30 MiB of
`libLLVM` and started 7 driver threads (`:gl`, `:sh`, `:cs`, `:gdrv`,
`traceq`, `sh_opt`, one more disk-cache worker) that idle for the life of the
process. `WgpuContext::backend_attempts()` now tries a Vulkan-only instance
first and creates a GL-only one only when no Vulkan adapter can configure the
window's surface (`new_context_and_surface`, also used by device-loss
recovery). `WGPU_BACKEND` (`gl`, `vulkan`, `vulkan,gl`) replaces the list, so
`WGPU_BACKEND=gl` forces the GL path.

Measured with the release shell in a headless MangoWM session (3840 × 2160 at
1.5, 3 min settled, same config and data, only `WGPU_BACKEND` differing;
`cargo xtask resource-check` runs the same setup):

| | Threads | Anonymous | GL libraries mapped | VRAM |
|---|---|---|---|---|
| `VULKAN \| GL` instance (upstream) | 38 | 25.6 MiB | libgallium, libEGL_mesa, radeonsi's libLLVM pages | 176.6 MiB |
| Vulkan first (patch 7) | 31 | 22.3 MiB | none | 162.8 MiB |

Without the shell's cold-page drop the GL driver's file pages were resident
too: libLLVM 13–31 MiB and libgallium 2.7–9.5 MiB, depending on the page
cache. `WGPU_BACKEND=gl` still selects the radeonsi adapter and renders (the
fallback path); unit test `vulkan_is_tried_alone_before_gl` pins the order.

## 8. Colour emoji as a primary font (`cosmic_text_system.rs`)

`load_family` skips any face without a Latin `m` as a primary font (the
editor measures with it), and that included Noto Color Emoji, so asking for it
by name failed and the text fell back to the UI font, whose fallback reached a
flat symbol (DejaVu Sans, Noto Sans Symbols 2, Adwaita Mono) before the colour
font. A face `check_is_known_emoji_font` recognises is now kept. Mandara
requests the emoji family only for text that is nothing but emoji
(`mandara_ui::fonts::emoji_only`, e.g. a plugin's moon-phase glyphs), which
then draws in colour; every other run is unchanged.

## 9. Rounded content masks (`shaders*.wgsl`, `wgpu_renderer.rs`)

The GPU half of `vendor/gpui`'s patch 1: `gpui::ContentMask` gained
`corner_radii` after its bounds, so every primitive's mask is 8 words instead
of 4.

* WGSL `struct ContentMask { bounds: Bounds, corner_radii: Corners }` replaces
  `content_mask: Bounds` in `Quad`, `Shadow`, `Underline`, `MonochromeSprite`,
  `PolychromeSprite`, `SubpixelSprite` and `SurfaceParams`, matching the Rust
  `#[repr(C)]` layout (all fields 4-byte, no new padding). The vertex shaders
  still clip to `content_mask.bounds` with the existing `clip_distances`.
* `content_mask_alpha(position, mask)` returns 1 for a rectangular mask
  without touching the SDF (the branch every rectangular-mask primitive
  takes), otherwise `saturate(0.5 - quad_sdf(...))`: the same rounded-rect
  SDF and ~1 device px antialiasing as quad corners. Each fragment shader
  multiplies its coverage by it (quads also return early at 0). Glyph shaders
  (mono / subpixel sprites) get the mask as two flat varyings instead of
  re-loading the instance per fragment.
* Paths: `PathRasterizationVertex` gains the path's `content_mask` (34 words,
  was 26); rasterization multiplies coverage by the mask at the fragment's
  frame position, so the composite pass is unchanged.
* WebGL decoders: word strides 44 / 32 / 34 / 20 / 32 / 28 (quad, shadow, path
  vertex, underline, mono, poly), and the fixed quad decoder reads 11 texels.
* No new passes, bindings or allocations; the instance buffer grows by 16 B
  per primitive.
* `Cargo.toml` patches `gpui` to `../gpui` for this crate's own tests (ignored
  inside Mandara's workspace, whose root patch does the same).

New test `rounded_content_masks_clip_to_the_rounded_shape` (headless
renderer): a quad and a path drawn through a 64 px mask with 16 px corners
fill its inside and straight edges fully, leave all four corners outside the
arcs empty, antialias the arc pixel, and a zero-radius mask still clips hard.

## Tests

* New `paths_render_through_bounding_box_textures` (headless renderer): a solid
  and a gradient path away from the origin land on the right pixels, the
  gradient stays anchored to the path in frame space, the path texture is the
  rounded bounding box (256 × 128 for a 130 × 70 box in a 512 × 384 frame), no
  texture exists before the first path, and it is released after
  `PATH_TEXTURE_IDLE_FRAMES` path-less frames.
* `remove_deallocates_tile_space_for_reuse` sizes its two images relative to
  `INITIAL_ATLAS_EDGE_PX` (upstream hard-coded 64 + 700 px, which assumed the
  1024² first texture).
* All 50 crate tests pass, including naga validation of every shader variant
  and the updated WebGL record strides and fixed quad decoder.
  The cosmic-text tests `include_bytes!` Zed's `assets/`, so run them from a
  copy laid out like Zed's tree:

  ```sh
  mkdir -p /tmp/zt/crates && ln -s ~/.cargo/git/checkouts/zed-*/ec86047/assets /tmp/zt/assets
  cp -r vendor/gpui vendor/gpui_wgpu /tmp/zt/crates/
  CARGO_TARGET_DIR=/tmp/zt/target cargo test --manifest-path /tmp/zt/crates/gpui_wgpu/Cargo.toml --features test-support
  ```

## Packaging changes

`Cargo.toml` spells out the workspace-inherited fields and dependencies with
the exact specs from Zed's root manifest at that revision and declares its own
empty `[workspace]` so it stays outside Mandara's. Benches and the `criterion`
dev-dependency are dropped; the other dev-dependencies (only resolved when the
crate's own tests run) are kept.

## Upstreaming notes

Patches 1, 2, 4 and 5 are self-contained and behaviour-preserving (identical
output, less memory); patch 2 also fixes a latent validation error. Patch 3's
knobs are opt-in. Further steps not taken here:

* one path texture per path *batch* instead of the frame's union: paths in
  opposite corners (bar rings top-right, sidebar top-left) currently share one
  wide box;
* share one atlas between all windows of a `GpuContext` (glyphs and icons are
  uploaded once per window);
* in wgpu-hal, size the swapchain from the present mode's own `minImageCount`
  (`VK_EXT_surface_maintenance1`): FIFO needs 3 images, not 4.
