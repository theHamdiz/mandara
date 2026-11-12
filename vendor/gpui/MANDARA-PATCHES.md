# Mandara patches to `gpui`

Vendored from [zed-industries/zed](https://github.com/zed-industries/zed) at
`ec86047d67acd6fddd81392f44ed6ebcc6e7f531` (`crates/gpui`, Apache-2.0, see
`LICENSE-APACHE`) and wired in through the workspace's
`[patch."https://github.com/zed-industries/zed"]`, next to the vendored
renderer (`vendor/gpui_wgpu`, whose patch 9 is the GPU half of patch 1 here). Every
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

## 2. Hover styles without an id (`elements/div.rs`)

At this revision `Interactivity::paint` only notifies the view when an
element's `.hover()` state flips if the element has element state, that is an
`.id()`: the listener compares the hit test against `hover_state`, which lives
in the element state, and without it does nothing. The hover style itself is
still resolved at paint from the hitbox, so an id-less hoverable element (every
setting row in Mandara Studio, many shell list rows) showed its hover only when
something else re-rendered its view: a scroll, a timer, the pointer entering
the window. Moving the pointer from one row to the next left the highlight on
the first one (theHamdiz/mandara#2). `group_hover` without an id was already
covered (`paint_hover_group_handler`), and `.on_hover` listeners have their own
path.

* For an element with a hover style and no `hover_state`, the paint step records
  whether the hitbox is hovered in the frame being painted (the hit test is
  taken before paint, the same value the style was resolved from), and the
  mouse-move listener notifies the current view when that flips, exactly like
  upstream's text-element hover listener and group-hover handler. Elements with
  an id keep the `hover_state` path; cursor-only elements (no hover style)
  still never notify.
* Cost: one comparison per hoverable element per mouse move, as before, and a
  re-render of the containing view only on an enter or leave, never while the
  pointer moves within an element. A cached view (`.cached(..)`) re-renders on
  its own, without its parent's cache key changing.

## 3. Image decoders for GPUI's own formats only (`Cargo.toml`)

Upstream enables nearly every decoder of the `image` crate (Zed's image viewer
opens all of them). `Image` and `ImageFormat` here name PNG, JPEG, WebP, GIF,
SVG, BMP, TIFF, ICO and PNM, and Mandara itself only loads PNG, JPEG, WebP,
GIF, BMP and SVG (wallpapers, avatars, album art, icons, thumbnails), so the
DDS, EXR, farbfeld, HDR, QOI and TGA decoders are dropped. `img()` still
decodes every format `ImageFormat` names, so a clipboard or notification image
in one of them keeps its preview; a file in a dropped format fails to load like
any other unsupported file. Together with `vendor/pure-onnx-ocr`'s patch 1
(which stops the OCR engine turning on all of the `image` crate's default
formats) this takes `exr`, `qoi`, `pulp`, `zune-inflate`, `lebe`, `bit_field`
and their helpers out of the build.

## 4. Fractional bounds for SDF curves (`window.rs`, `window/subpixel.rs`)

`paint_quad` aligns layout rectangles to device pixels. Applying that rule
separately to every dot in the Power charge arc moves neighbouring centers
by different amounts and leaves an uneven contour, especially at 1× scale.

`Window::paint_subpixel_quad` keeps those bounds at their fractional device
coordinates. It shares insertion with `paint_quad`: opacity, content masks,
stroke widths and border handling use the same code. Ordinary layout quads
still use their existing pixel alignment. No primitive format or shader
changes are needed; SDF quads already accept floating-point bounds.

The Power painter uses this method for its round marks. It keeps the same
geometry, number of quads and colour interpolation.

## 5. Current atomic update API (`profiler.rs`, `profiler/journal.rs`)

The profiler's three atomic updates use `try_update`, the current stable
name for the deprecated `fetch_update` alias. Ordering and retry behavior
are unchanged. This keeps standalone renderer and GPUI tests free of
Rust 1.99 deprecation warnings without suppressing the lint.

## 6. Stop a frame when its window closes (`window.rs`)

A view can remove its window while drawing its final fade frame. The frame
callback now returns after that draw, before updating or waking the removed
window again. This avoids a spurious `window not found` error when leaving
the screenshot overlay. The regression test closes a view during a real
platform frame callback and checks that no further frame wake is sent.

## 7. Transition primitive (`scene.rs`, `window.rs`)

A wallpaper change used to be drawn as dozens to hundreds of image quads,
each the same full-screen image through its own clip: seams between the
pieces, axis-aligned "shards", block-shaped dissolves. The scene now has a
primitive for a per-pixel transition between two full-size images, and the
vendored renderer (`gpui_wgpu` patch 16) draws it with one fragment shader.

* `TransitionSprite { order, pad, bounds, content_mask, from, to, aux,
  params }`: three atlas tiles (outgoing image, incoming image, an optional
  helper image, `to` again without one) and `TransitionParams`, plain data:
  kind (`TRANSITION_KIND_*`, fade to sketch), `TRANSITION_FLAG_*` bits
  (helper bound, subject mask, RTL, glow), linear progress, seconds since
  the start, origin and focus points (fractions of the bounds), seed, motion
  intensity, device scale, line width, line / glow / wash colours, Sketch's
  drawing share, fill style and order, and Shatter's grid
  (`TRANSITION_MAX_SHARDS` = 128 cells at most).
* `Scene::transitions`, `Primitive::Transition`, `PrimitiveKind::Transition`
  and `PrimitiveBatch::Transitions`: one batch per sprite (each binds its
  own textures). Cleared, shrunk and sorted with the other lists.
* `Window::paint_transition(bounds, from, to, aux, params) -> Result<bool>`
  takes each image as a `TransitionImage { id, size, bytes }`: the atlas
  gets it the first time (`get_or_insert_with`, which since `gpui_wgpu`
  patch 13 copies to wgpu's staging memory before returning), so the
  caller frees its pixels after that frame. An image the atlas no longer
  has and whose bytes are gone (the atlas was cleared after a device loss)
  returns `Ok(false)` and paints nothing: the caller ends its transition.
  `Window::drop_transition_image(id)` removes one; a full-size image owns
  its atlas texture, which goes with it.
* Other renderers: only the vendored `gpui_wgpu` consumes `PrimitiveBatch`
  in Mandara's build; upstream's Metal and DirectX renderers are not built.

## Tests

`elements::div::tests::hover_without_an_id_updates_its_cached_view` (patch 2,
`TestAppContext`): a cached child view with one id-less `.hover(bg)` element
re-renders once on enter and once on leave, not on motion within it, and the
painted frame carries the hover background exactly while hovered. It fails
without the patch.

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

`window::subpixel::tests::fractional_sdf_bounds_keep_the_legacy_clip_and_opacity`
checks actual painted scene quads at 1×, 1.25×, 1.5× and 2×: precise bounds,
unchanged legacy snapping, rounded clipping, opacity, radii and border widths.

The library suite passes 413 tests with two existing ignores. Like `gpui_wgpu`, some tests `include_bytes!` Zed's
`assets/`, so run them from a copy laid out like Zed's tree (the vendored
renderer's `[patch]` points at `../gpui`, so copy both):

```sh
mkdir -p /tmp/zt/crates && ln -s ~/.cargo/git/checkouts/zed-*/ec86047/assets /tmp/zt/assets
cp -r vendor/gpui vendor/gpui_wgpu /tmp/zt/crates/
CARGO_TARGET_DIR=/tmp/zt/target cargo test --manifest-path /tmp/zt/crates/gpui/Cargo.toml --features test-support --lib
CARGO_TARGET_DIR=/tmp/zt/target cargo test --manifest-path /tmp/zt/crates/gpui_wgpu/Cargo.toml --features test-support
```

## 7. Shared empty font features (`text_system/font_features.rs`)

`font()` and `TextStyle::default()` previously allocated a new `Arc<Vec<_>>`
for every empty feature list. Their defaults now share one immutable list;
deserializing an empty list uses it too. The stored type, hashing, equality
and serialized values stay the same. In-tree readers never depend on unique
Arc ownership. Code editing a returned list must use `Arc::make_mut`, which
preserves the defaults and other fonts. Mandara also shares its immutable
tabular-number feature list instead of rebuilding its vector and tag string.

Mandara's `framework_allocations` tests exercise default construction with a
live allocating control, empty and configured wire values, hash equality,
and independent edits. Release frame and memory measurements remain a separate
acceptance step; operation allocation counts are not whole-frame counts.

## 8. Releasing decoded images (`app.rs`, `elements/img.rs`, `platform.rs`)

Upstream can drop an image's texture only through a `&mut Window` that is
being updated, and `remove_asset` alone leaves the texture in every atlas.
Views that release a page's pictures when it closes (Studio) run inside a
window update, so their own atlas kept every texture.

* `App::loaded_asset::<A>(source)` returns a finished asset's result without
  starting a load (`fetch_asset` starts one for an uncached source).
* `ImageSource::evict(cx)` drops the decoded image from every window's atlas
  that is not leased at the moment, then removes the asset. It never starts a
  decode; a pending load is forgotten, so its result is never uploaded. Call
  it from `App::defer` to reach the window that is updating.
* `decode_static_image_from_decoder` moves its one frame into the frame list.
  `SmallVec::from_elem(frame, 1)` cloned it, so every static decode (PNG,
  JPEG, WebP, BMP, TIFF, ICO) briefly held two full-size BGRA copies.

`elements::img` tests cover eviction of loaded and uncached images and the
single moved frame.

## 9. Painting a clipping container's own outline (`window.rs`)

A card that clips its children to rounded corners (patch 1) and draws its
own cut or squircle outline as a child canvas had that outline clipped by its
own rounded mask too, so the cut corners and squircles came out rounded.

* `ContentMask::<Pixels>::outline_within(outer)` is `outer` intersected with
  the rectangle of `self`: the clip of the ancestors within the container,
  without the container's own corner radii.
* `Window::with_outer_content_mask(f)` runs `f` under `outline_within` of the
  innermost content mask and the one before it, then restores the stack. Only
  a child painting its clipping container's own outline calls it, so the
  innermost mask is that container's.

Mandara's `components::surfaces` tests cover the mask (a card flush with a
rounded panel's corner keeps that corner and loses only its own rounding).

## 10. FreeBSD uses the Linux platform's layer shell and queue (`gpui.rs`, `platform.rs`, `window.rs`)

Upstream's Linux platform (`gpui_linux`, chosen by `gpui_platform` for
`any(target_os = "linux", target_os = "freebsd")`) also serves FreeBSD, but a
few items it needs from this crate were gated on `target_os = "linux"` alone,
so a FreeBSD build of `gpui_linux` failed: the priority queue its dispatcher
uses, and the Wayland layer-shell types its Wayland client creates layer
surfaces with. Each cfg now reads `any(target_os = "linux", target_os =
"freebsd")`:

* `mod queue` and `pub use queue::{PriorityQueueReceiver, PriorityQueueSender}`
  (`gpui.rs`);
* `pub mod layer_shell`, `PlatformWindow::set_exclusive_edge` and
  `WindowKind::LayerShell` (`platform.rs`), still behind `feature = "wayland"`;
* `Window::set_exclusive_edge` (`window.rs`);
* `PlatformScreenCaptureFrame` with `screen-capture` (`platform.rs`), whose
  `scap_screen_capture` module upstream already builds on FreeBSD.

Linux is unchanged: every item keeps its Linux cfg. Nothing in `gpui_linux`
or `gpui_wgpu` needed changing; both already treat FreeBSD like Linux
(Wayland client, `wl_touch`, fonts, input, the wgpu renderer). The FreeBSD
check in CI (`cargo clippy --workspace --all-targets --target
x86_64-unknown-freebsd`) covers this patch.

## 11. Forgetting dropped views (`app.rs`, `app/entity_map.rs`)

Mandara builds its sidebar cards when the sidebar opens and drops them when
it closes, so views come and go all session. Two things kept a trace of each
dropped view:

* `App::notify(id)` takes the entity's window invalidators and always put
  the entry back, so notifying an entity after it was released (a redraw
  timer that fires late, a model telling the views it remembers) left one
  empty `window_invalidators_by_entity` entry per dead id for good. An empty
  list is now removed instead; a live entity's list is put back as before.
* Hosts that remember views by `EntityId` to notify them later had no way to
  tell a dropped one. `App::entity_alive(id)` (through
  `EntityMap::is_alive`) says whether the entity still has a strong handle,
  so they can forget it.

Behaviour is otherwise unchanged: an empty entry and a missing one mean the
same to every reader of the map. `app::test::notifying_a_dropped_entity_leaves_no_trace`
covers both.

## 12. Giving back a large frame's room (`window.rs`, `arena.rs`, `taffy.rs`, `scene.rs`, `key_dispatch.rs`, `bounds_tree.rs`)

A window keeps the capacity its largest frame needed: the element arena's
1 MiB chunks, the layout tree's node storage, the dispatch tree, the scene's
primitive lists and the frame's listener and hitbox lists are cleared every
frame but never shrink. That is right while a surface shows (no allocation per
frame), but after Mandara's sidebar closes the room its cards needed (a Sudoku
board is thousands of elements) stayed for good, about 1.2 MiB for one card.

`Window::shrink_frame_buffers(cx)` frees that spare room once, at a lifecycle
boundary the host chooses: both frames shrink to their contents
(`Frame::shrink_to_fit`, with `DispatchTree`, `Scene` and `BoundsTree`
helpers), the layout engine starts from a fresh tree (it is rebuilt at the
start of every frame anyway) and the arena keeps only its first chunk
(`Arena::shrink`, which does nothing while a draw holds it or anything is
allocated in it). It does nothing during a draw. Frames are unchanged
otherwise, and a surface that shows again grows the room back on its first
frame.

## 13. Data drops, typed drags and moving layer surfaces (`interactive.rs`, `window.rs`, `platform.rs`, `platform/test/window.rs`)

Upstream takes only local files from other programs (`text/uri-list` as
`ExternalPaths`) and can hand only files to them (`ExternalDragPayload::Files`);
a layer surface can't move once mapped. Mandara's Stash shelves take anything
dragged in, give anything back out, and follow the pointer around the screen.

* **Data in.** `ExternalData` is the value of a drag of anything that is not
  a list of local files (text, HTML, links, pictures, any MIME type), over a
  window that opted in with `Window::set_accepts_data_drops(true)` (default
  off, so every other window still refuses such drags). It lists the offered
  MIME types and `receive(mime)` returns a stream to read off the UI thread;
  the platform's transfer (`ExternalDataSource`) stays open until the last
  clone is dropped. `FileDropEvent::DataEntered` becomes an internal drag of
  `ExternalData` exactly as `Entered` becomes one of `ExternalPaths`, so
  `.on_drop::<ExternalData>()`, `.drag_over::<ExternalData>()` and
  `.on_drag_move::<ExternalData>()` work. File drags are unchanged.
* **Data out.** `ExternalDragPayload::Data(DragData)` offers several MIME types
  at once (`DragOffer`, the first of a type wins), each from shared bytes or a
  file streamed when the receiver asks (`DragContent::File`), so a large
  picture is never held in memory. `FileDropEvent::Delivered` reaches the
  source window just before `Ended` when the receiver took the drop (the
  shelf removes items moved out).
* **Moving layer surfaces.** `Window::set_layer_margin((top, right, bottom,
  left))` sets a layer surface's margins and commits at once
  (`PlatformWindow::set_layer_margin`, a no-op elsewhere).

The test window ignores data drags; the platform side is in
`vendor/gpui_linux` (its patch 2), with tests.

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
