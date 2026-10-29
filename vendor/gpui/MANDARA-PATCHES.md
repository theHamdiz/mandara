# Mandara patches to `gpui`

Vendored from [zed-industries/zed](https://github.com/zed-industries/zed) at
`ec86047d67acd6fddd81392f44ed6ebcc6e7f531` (`crates/gpui`, Apache-2.0, see
`LICENSE-APACHE`) and wired in through the workspace's
`[patch."https://github.com/zed-industries/zed"]`, next to the vendored
renderer (`vendor/gpui_wgpu`, whose patch 9 is the GPU half of this one). Every
source change is marked `Mandara patch` in a comment. Only `crates/gpui` is
vendored; every other Zed crate (`gpui_platform`, `gpui_linux`, `gpui_macros`,
`refineable`, `collections`, …) still comes from the same git revision and is
built against this copy through the patch.

## 1. Rounded content masks (`window.rs`, `style.rs`)

Upstream `ContentMask` is only a rectangle, so a `.rounded_*()`
`.overflow_hidden()` card clipped its children to its *bounding box*: a cover
image, a colour band or a hover fill that reached the card's edge was drawn as
a square over the card's rounded corners.

* `ContentMask<P>` gains `corner_radii: Corners<P>` (`#[repr(C)]`, after
  `bounds`; all zero by default, which is exactly upstream's rectangle).
  `ContentMask::rect(bounds)` builds a rectangular mask, `is_rounded()` tells
  them apart, `scale()` scales the radii with the bounds.
* `Style::overflow_mask` returns the element's own radii when overflow is not
  visible on **both** axes, resolved and clamped exactly as `Style::paint`
  resolves them for the background (`rounded_full` becomes half the shorter
  side). When a visible border insets the mask, each radius shrinks by the
  wider of its two adjacent border widths (the circular arc inside the
  border's inner edge). One hidden axis keeps upstream's square clip.
* `ContentMask::intersect` (nesting in `with_content_mask`) intersects the
  bounds and gives each corner of the result the rounding of the mask(s) whose
  corner it is: the larger radius when both masks share it (within
  `CONTENT_MASK_CORNER_EPSILON`, 0.5 logical px), that mask's radius when only
  one does, and zero when neither does (one mask's straight edge cuts across
  the other). The SDF stays exact for a corner taken from one mask even when
  its radius exceeds half the intersected size, because the corner's circle
  keeps its centre. Approximation: a rounded corner that the other mask's edge
  crosses *near*, not at, the corner is treated as square there.
  `ContentMask<ScaledPixels>::intersect(other, scale_factor)` does the same in
  device pixels.
* `snapped_content_mask` snaps a rounded mask with `snap_bounds` (rounding,
  as `paint_quad` snaps the container's own background) instead of
  `cover_bounds` (floor / ceil), so the clip coincides with the painted edge
  rather than leaving up to a device pixel of child content outside it.
  Rectangular masks are unchanged.
* `paint_quad` does not split a border-only quad into edge strips when its
  mask is rounded (a strip-sized mask cannot carry the parent's arcs); the
  underline exclusion spans intersect with the mask instead of replacing it.
* The 13 `ContentMask { bounds }` literals (window, list, uniform list, debug
  overlay, style, tests) use `ContentMask::rect`.

The scene's primitive structs embed `ContentMask<ScaledPixels>`, so every one
grows by 16 bytes (`Quad` 160 → 176, `Shadow` / `MonochromeSprite` /
`SubpixelSprite` 112 → 128, `Underline` 64 → 80, `PolychromeSprite`
96 → 112). The vendored `gpui_wgpu` mirrors that layout; upstream's Metal
renderer (`gpui_apple`, macOS only, never built for Mandara) does not.

## Tests

`window::mandara_rounded_mask_tests` (unit and `TestAppContext`):

* intersection: rectangles stay rectangles; a child filling a rounded parent
  keeps the parent's corners; shared corners take the larger radius and the
  operation is symmetric; a card scrolled half out of a rectangular viewport
  keeps its top corners and gets square bottom ones; corners within the
  epsilon count as shared and 2 px away do not; disjoint masks are empty and
  unrounded; device-pixel intersection scales the epsilon;
* `scale` scales bounds and radii;
* a real `div().rounded(..).overflow_hidden()` gives its child a rounded mask,
  `rounded_full` clamps, a visible border insets the mask and shrinks the
  radii, at 1.5× the child's quad carries a device-pixel mask snapped onto the
  card's own background quad, and without overflow (or with one hidden axis)
  nothing is rounded.

All 410 library tests pass. Like `gpui_wgpu`, some tests `include_bytes!` Zed's
`assets/`, so run them from a copy laid out like Zed's tree (the vendored
renderer's `[patch]` points at `../gpui`, so copy both):

```sh
mkdir -p /tmp/zt/crates && ln -s ~/.cargo/git/checkouts/zed-*/ec86047/assets /tmp/zt/assets
cp -r vendor/gpui vendor/gpui_wgpu /tmp/zt/crates/
CARGO_TARGET_DIR=/tmp/zt/target cargo test --manifest-path /tmp/zt/crates/gpui/Cargo.toml --features test-support --lib
CARGO_TARGET_DIR=/tmp/zt/target cargo test --manifest-path /tmp/zt/crates/gpui_wgpu/Cargo.toml --features test-support
```

## Packaging changes

`Cargo.toml` spells out the workspace-inherited fields and dependencies with
the exact specs from Zed's root manifest at that revision (sibling Zed crates
from the same git revision), declares its own empty `[workspace]` so it stays
outside Mandara's, sets `publish = false`, turns doc tests off and keeps only
`unexpected_cfgs = "allow"` of Zed's workspace lints. Vendored: `src/`,
`build.rs`, `README.md` (the crate docs include it), `LICENSE-APACHE` and the
one 378-byte JPEG a unit test embeds (`examples/image/`). Not vendored:
examples, benches, docs, integration tests and the Windows resources (only
read on Windows). The dev-dependencies only examples, doc tests and benches use
(`gpui_platform`, `reqwest_client`, `bench_metrics`) are dropped; the rest are
kept for the library tests.

## Upstreaming notes

The change is behaviour-preserving for rectangular masks (identical clip
bounds and output; the shaders skip the SDF when every radius is zero) and
fixes a long-standing gap (the `ContentMask` doc comment always planned for
"more complex shapes"). Upstreaming needs the Metal shaders and
`gpui_apple`'s `ContentMask` literal updated the same way as `gpui_wgpu`'s.
