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
  than `PATH_TEXTURE_SHRINK_FACTOR`× larger than needed, or once either
  condition has lasted `PATH_TEXTURE_IDLE_TIME` (1.5 s), whichever comes
  first: an idle shell draws only an occasional frame, and a frame count
  alone kept textures sized for a closed sidebar for minutes (~60 MB at 4K).
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

## 10. Pipelines shared between windows (`wgpu_renderer.rs`, `wgpu_context.rs`)

Every `WgpuRendererCore` parsed and validated the WGSL shaders (naga keeps the
module and its validation info alive with the pipelines) and built all its
render pipelines and bind group layouts itself, so each window held its own
copy: the bar / panel surface, every exclusion strip, the dock, each popover.
They depend only on the device and on `PipelineKey` (target format, alpha
mode, path sample count, dual-source blending, WebGL instance data), so
`WgpuContext::shared_pipelines` now builds one `SharedPipelines` per key the
first time a renderer asks and hands every later renderer clones of the same
wgpu handles. A recovered device is a new `WgpuContext` with an empty cache.
`update_transparency` still rebuilds against the shared layouts.

Measured with `cargo xtask resource-check` (headless 4K at 1.5, Vulkan / RADV,
default config: bar surface and one exclusion strip), 15–90 s after start:
heap 10.0–10.2 → 8.7–8.8 MiB, VRAM unchanged (166 MiB). The demo sandbox
(OpenGL there, bar, dock and a strip) went from 36.4 to 32.4 MiB anonymous.
Windows opened later (popovers, the launcher, floating cards) also skip the
shader parse and pipeline build.

## 11. One face per default-chain family (`cosmic_text_system.rs`)

Patch 6 resolved each family of the default fallback chain with
`load_family`, which loads *every* face of the family (each weight, width
and slant: a `cosmic_text::Font` with its shaper data, cached by
cosmic-text for the life of the process, and the face's file mapped) only
to keep one of them: the upright face nearest to regular, whose charmap
decides coverage. Text in the chain's script asks cosmic-text for the face
it needs at its weight anyway (`fallback_weight`), so the rest were never
used unless that text appeared. For an English interface the chain is the
Arabic one: Cairo, Noto Sans Arabic (UI), Noto Naskh / Kufi Arabic, 53
files mapped and parsed at the first frame.

`load_chain_face` ranks faces upright first, then nearest to 400, keeping
font-database order for ties, and loads only the selected face. Fallback-only
faces can lack Latin glyphs; primary fonts still require the measurement glyph
`m`, with the existing emoji and icon-font exceptions. The same guard also
applies when a previously loaded fallback is requested as a primary font.

The original patch was measured in Mandara's demo sandbox (Vulkan / RADV, 3840 × 2160 at 1.5,
default config, release builds, three runs each; the shell's
`lock::paint::TextPainter` got the same treatment, see
`docs/releasing.md` § Start-up memory): font files mapped 52 → 2, their
resident pages 4 s after start 11.0 → 4.8 MiB, and the start-up peak
(`VmHWM`) 112.0 (112.0–112.1) → 101.0 (100.9–101.8) MiB. Arabic, Persian,
Urdu, Korean and Bengali text still renders in its faces at every weight
(the `mandara-ui` `fonts::tests` regression tests pass unchanged).

## 12. Explicit native backend features (`Cargo.toml`)

The native renderer exposes `vulkan` and `gles` features. Mandara requests
both with defaults disabled, retaining Vulkan-first selection and the GL
fallback. Linux no longer enables Metal or the browser WebGPU backend;
WebGPU otherwise enables Naga's WGSL writer even in a native build. Browser
targets retain WebGPU and WebGL. This removes unused shader translation
from native compilation without changing its rendering path. The workspace
`renderer_features` test checks the native and browser contracts.

## 13. Upload atlas pixels straight to wgpu (`wgpu_atlas.rs`)

The atlas copied every upload into a `PendingUpload` byte vector, then
copied it again into wgpu's staging memory at the next frame. It now calls
`Queue::write_texture` at insertion: wgpu copies the slice before returning
and schedules the transfer before the next submitted draw. Ordinary BGRA
textures borrow the original slice; devices using RGBA allocate only the
swizzled bytes. There is no second atlas-owned CPU copy of an image or
glyph. Texture removal and device recovery still drop their atlas handles;
wgpu keeps queued transfers alive until submission finishes.

Tests check borrowed BGRA allocation identity, RGBA channel conversion,
removal before submission, and actual headless GPU pixels after changing
the source bytes and reusing the released texture id, in both formats.

## 14. Accept script-only fallback faces (`cosmic_text_system.rs`)

Devanagari and Telugu faces often lack Latin `m`. Applying the primary-font
measurement guard to the default fallback chain rejected these faces and left
cosmic-text to search unrelated families. Fallback faces now join the chain
without that guard. Requesting such a face as a primary font still fails,
including when it was already cached as a fallback. Face selection uses one borrowed pass without a candidate vector or sorting;
the selected face supplies its emoji classification directly from the database.

Regression tests use a small SIL Open Font License Devanagari fixture, loaded
only in tests. They cover fallback acceptance, cached primary rejection and
actual Hindi/Marathi shaping with a Latin primary and no extra loaded faces.
The font and its license live under `test_data/`; neither enters the release
binary. Release measurements are required before claiming a memory saving.

## 15. Image quads clipped in the vertex stage (`shaders.wgsl`)

Upstream rasterizes a polychrome sprite (an image) over its whole bounds and
discards the fragments outside its content mask. An image drawn through a
small clip still cost a full-size quad: the wallpaper transitions draw the
same full-screen image through dozens to hundreds of tile clips (Dissolve,
Shatter, Tide, Tear, Glitch, Sketch's fill), each a full-screen pass, which
kept a 2560 × 1440 headless frame at ~50 ms. `vs_poly_sprite` now emits the
intersection of the bounds and the mask's rectangle, with texture
coordinates interpolated from the same unit position, so each surviving
fragment samples exactly as before and the per-fragment clip (and rounded
mask) still applies. No new bindings, buffers or passes.

Re-checked with patch 16: `storage_buffer_shader_is_valid_wgsl` and
`webgl_shader_is_valid_wgsl_without_storage_buffers` validate the patched
`vs_poly_sprite` with naga (both shader variants), and the new headless test
`poly_sprites_rasterize_only_their_clip` draws a two-colour image through a
small clip: each half lands on its own pixels inside the clip (the texture
coordinates follow the clipped corners) and nothing is drawn outside it.
The wallpaper transitions no longer use tiled image quads (patch 16 replaced
them), so the patch now serves scrolled thumbnails and clipped images.

## 16. Transition primitive (`transition.rs`, `shaders_transition.wgsl`, `wgpu_renderer.rs`, `wgpu_atlas.rs`)

The GPU half of `vendor/gpui`'s patch 7: `PrimitiveBatch::Transitions`
draws a `gpui::TransitionSprite` as one quad over its bounds (clipped to its
content mask in the vertex stage), whose fragment shader samples the
outgoing, incoming and helper images and evaluates the transition per
pixel. Every kind returns exactly the outgoing image at progress 0 and
exactly the incoming one at 1.

* **Its own module and bind group.** `shaders_transition.wgsl` is a
  separate shader module with one bind group: a uniform block
  (`TransitionUniforms`, only `vec4`s, std140-compatible and readable by
  WebGL2 too), the three atlas textures and the atlas sampler. The renderer
  computes the tiles' texel rectangles and inverse texture sizes, so the
  shader samples each tile clamped to its texel centres; displaced samples
  never bleed into a neighbouring tile.
* **Lazy and released.** Nothing exists until a frame draws a transition:
  then `TransitionResources` builds the module, pipeline (no blending: the
  output is opaque and replaces what is under it), a uniform buffer with
  `MAX_PER_FRAME` = 4 aligned slots and one cached bind group per slot,
  keyed by the textures' ids and generations. Frames without transitions
  release all of it after `IDLE_RELEASE` (1.5 s), and device recovery drops
  it with the other resources. `WgpuTextureInfo` gained the texture's size.
  A transition whose texture the atlas no longer has is skipped.
* **Shatter's shards** are moved on the CPU: `transition::shards` computes,
  per frame, each of up to 128 cells' rest centre (a jittered grid), the
  inverse of its rotation and scale, its current centre, alpha, depth and
  how far it has gone, into the uniform block. Each pixel then tests every
  shard's inverse transform (a squared-distance rejection first) and keeps
  the frontmost shard whose own Voronoi cell (a 3 × 3 search of rest
  centres) contains the mapped point. Testing every shard, rather than only
  a neighbourhood around the pixel, stays exact however far a shard has
  flown, and the motion is a pure, tested function of the progress.
* **Kinds.** Fade; Wipe (soft edge, RTL aware); Grow (rounded window,
  settling zoom); Ripple (radial wave front refracting both images, a
  light ring on the crest); Tide (fbm-edged wave with foam and a wet band);
  Tear (an fbm seam with fine fibres runs down the old image, both halves
  bend apart above the rip's front, then slide out, turn and drop, with a
  paper-white rim of uneven width and drop shadows); Glitch (bands re-cut
  12 times a second, displaced, a channel split in the first 30 %, every
  band switching to the new image once, luminance pulses at most 3 a
  second and at most 6 %); Shatter (above); Dissolve (an fbm threshold
  burning with a glowing edge in the given colour, charring ahead of it);
  Sketch (Sobel edges of the new image at two scales, the subject mask's
  0.5 contour from the helper image, a drawing front following the helper's
  distance-from-subject field or a top-down / centre / point order, a wash
  over the old image, then a fade, a noise-edged watercolour bleed out of
  the lines, or a wipe along the drawing order).

New tests: `transition_shader_is_valid_wgsl` (naga, no capabilities),
`transitions_start_on_the_old_image_and_end_on_the_new_one` (headless: all
ten kinds give the old image at 0 and the new one at 1, mix at 70 %, are
opaque, and the resources exist only after a transition and go after the
idle period), `transitions_skip_released_textures`, and in `transition.rs`
the uniform block's size, shards resting at 0 and gone at 1, leaving from
the origin outward first, deterministic per seed, grids bounded.
`render_transition_frames` (ignored) writes frames of real images for
looking at them (`MANDARA_TRANSITION_FRAMES`, `_FROM`, `_TO`, `_AUX`,
`_KIND`, `_STEPS`).

## 17. Hardware before software on every backend (`wgpu_context.rs`, `wgpu_renderer.rs`)

Patch 7 tries a Vulkan-only instance first and accepts any Vulkan adapter
that configures the surface, software ones included. In a virtual machine
with virgl OpenGL but no virtio Vulkan (GNOME Boxes' QEMU 7.2, for one), the
only Vulkan adapter is lavapipe, so every frame was drawn on the CPU and the
desktop, at 3840 × 2160, barely responded, while hardware OpenGL went unused.
`new_context_and_surface` now makes the passes `WgpuContext::software_passes`
gives: every backend is tried for a hardware adapter first (Vulkan, then GL),
and a software adapter is accepted only after that, unless the caller refuses
software outright (device-loss recovery, as before). A machine with a Vulkan
GPU takes the same path as before, and a machine with no hardware adapter at
all still ends on lavapipe or llvmpipe. `WGPU_BACKEND` still replaces the
backend list.

## 18. Native OpenGL compatibility (`compatibility.rs`, `wgpu_context.rs`)

Native GL requested storage-buffer and compute limits even on adapters that
cannot provide them. Device creation failed, leaving a CPU Vulkan renderer
as the fallback. The existing integer-texture instance transport already
works without those features, but selection was restricted to WebGL2.

GL adapters without vertex storage or the standard device limits now use
that transport with WebGL2 limits and the RGBA atlas. Features unused by
that transport are not requested. Vulkan, Metal, DX12 and fully capable
native GL retain their existing limits and storage-buffer path. The
capability policy and its regression tests live in one module.

The compatibility transition shader replaces `firstTrailingBit`, which
GLSL 3.3 cannot compile, with a bounded bit scan. Modern adapters retain
the original shader. Pixel tests cover the legacy transport, clipping,
atlas uploads and all wallpaper transitions with storage and compute
extensions disabled; the atlas tests request the same limits as the renderer.

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
* New `hardware_on_any_backend_comes_before_software`: the passes are
  hardware then software, and recovery keeps refusing software.
* All crate tests pass (65 with patch 17), including naga validation of every shader variant
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

Patches 1, 2, 4, 5 and 10 are self-contained and behaviour-preserving
(identical output, less memory); patch 2 also fixes a latent validation error. Patch 3's
knobs are opt-in. Further steps not taken here:

* one path texture per path *batch* instead of the frame's union: paths in
  opposite corners (bar rings top-right, sidebar top-left) currently share one
  wide box;
* share one atlas between all windows of a `GpuContext` (glyphs and icons are
  uploaded once per window);
* in wgpu-hal, size the swapchain from the present mode's own `minImageCount`
  (`VK_EXT_surface_maintenance1`): FIFO needs 3 images, not 4.
