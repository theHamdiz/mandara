# Mandara patches to `gpui_linux`

Vendored from [zed-industries/zed](https://github.com/zed-industries/zed) at
`ec86047d67acd6fddd81392f44ed6ebcc6e7f531` (`crates/gpui_linux`, Apache-2.0, see
`LICENSE-APACHE`) and wired in through the workspace's
`[patch."https://github.com/zed-industries/zed"]`, next to the vendored GPUI
core (`vendor/gpui`) and renderer (`vendor/gpui_wgpu`), which this crate is
built against through the same patch. Every source change is marked
`Mandara patch` in a comment. Everything else under `src/` is byte-for-byte
upstream.

## 1. Touchscreen input on Wayland (`wayland/client.rs`, `wayland/touch.rs`, `wayland.rs`)

Upstream's Wayland client asks the seat only for a pointer and a keyboard
(`Dispatch<wl_seat::WlSeat>` handles `Capability::Keyboard` and
`Capability::Pointer`; there is no `wl_touch` anywhere in the crate). GPUI
core already turns raw touches into taps, scrolls, long presses and claimed
touch drags (`gpui::gestures::TouchGestureRecognizer`, run by
`Window::dispatch_event` for `PlatformInput::Touch`), but nothing on Linux fed
it, so a finger on any GPUI window or layer surface did nothing while the
mouse worked (theHamdiz/mandara#130). The X11 client is unchanged.

* **Device lifecycle.** When the seat's capabilities include `Touch` the
  client gets a `wl_touch` (once: a repeated capabilities event, which a
  hotplugged mouse causes, keeps it and the fingers already down). When the
  capability goes away, when a new `wl_seat` global replaces the seat, and
  when the seat's global is removed (`WaylandClientState::wl_seat_name` keeps
  its registry name for `global_remove`), the device is released and every
  finger still down ends as `TouchPhase::Cancelled`. `WaylandConnection`'s
  `Drop` releases it too.
* **Events (`wayland/touch.rs`, `TouchPoints<W>`).** The protocol-to-GPUI
  state machine, generic over the window type so it is unit-tested without a
  compositor:
  * `down` starts a point on the window whose surface it hit (points on
    surfaces that are not GPUI windows are not tracked) with a fresh,
    never-reused `TouchId` (Wayland reuses its ids; GPUI requires that a
    window never reuse one). A down for an id that is still tracked cancels
    the stale contact first.
  * `motion` updates the point's position (a point stays with the window of
    its `down`, in that surface's coordinates) and is reported once per
    `frame`, as one `Moved` per moved point, so a fast digitizer costs one
    GPUI dispatch per point per hardware report.
  * `up` flushes a pending motion and then ends the point at its last
    position (`up` has none).
  * `cancel` ends every point as `Cancelled`, which GPUI's recognizer treats
    as "no tap".
  * Positions are `wl_fixed` surface-local logical coordinates, the same space
    and conversion as `wl_pointer` (`touch_position`), so fractional scales,
    layer-shell surfaces and xdg toplevels all map correctly.
  * Shape and orientation (v6) are ignored.
* **Dispatch.** Each event becomes `PlatformInput::Touch(TouchEvent { .. })`
  for its window's `handle_input`, after the client state is released
  (`dispatch_touch`). GPUI then recognizes it: a tap is a left
  `MouseDown` + `MouseUp` at the lift position (click count from
  `GestureTuning::multi_tap_interval`), so `on_click` and `on_mouse_down`
  work; a pan past the touch slop is a `ScrollEvent` with
  `ScrollSource::Touch` (with fling momentum); a long press shows tooltips;
  an element can claim a finger as a `TouchDragEvent` stream before it
  becomes a tap or pan.
* **Pointer untouched.** Touch never sets `mouse_focused_window`,
  `mouse_location`, `button_pressed`, the cursor or window hover, so a later
  mouse motion, click or leave behaves exactly as before. GPUI marks the
  window's input modality as touch, which suppresses hover styles until the
  next mouse motion.
* **Serials.** A touch-down updates `SerialKind::MousePress` (and so the
  selection serial): it is the user's latest press, for activation tokens and
  clipboard ownership, and a window move requested after a tap carries it
  instead of an older pointer press.
* **Closing windows.** `drop_window` forgets the points on the closing
  window without sending it anything.
* **Cost.** `TouchPoints` keeps up to four fingers inline (`SmallVec`),
  as does each event's output buffer; nothing is allocated per event below
  five simultaneous fingers, and the struct is idle when no touchscreen
  exists (the device is never requested).

Mandara's side of the patch (not in this crate): the shared slider
(`crates/mandara-ui/src/slider_model.rs`) claims the touch drag of a finger
that lands on its track, so the finger drags it instead of scrolling, and
Studio's title bar ignores a tap as a window move
(`crates/mandara-studio/src/app.rs`, `begin_move`).

## 2. Data drops, typed drags and layer margins (`wayland/client.rs`, `wayland/window.rs`, `wayland/clipboard.rs`)

The Wayland side of GPUI core's patch 13 (Stash's shelves).

* **Data in.** A window that accepts data drops
  (`WaylandWindowStatePtr::accepts_data_drops`, set through
  `PlatformWindow::set_accepts_data_drops`) takes any drag that carries no
  local files: when the offer has no `text/uri-list` (or its URIs are not
  `file://` ones) the enter becomes `FileDropEvent::DataEntered` with the
  offered MIME types (`begin_data_drag`). The offer accepts its first type
  as a copy only (a drop on a part of the window with no target is still
  taken, and must never make the source delete what it dragged), leaves the
  clipboard's rotating offer list (which would otherwise destroy it) and is
  owned by `WaylandDropData`, behind the drag's `ExternalData`. `receive`
  pipes a type and flushes; reads wait at most `PIPE_READ_TIMEOUT` per read
  (`TimedPipe`). `DragState` tracks it (`data`, the `DataDrop` trait):
  `handle_drop` marks it dropped and hands it to GPUI, `handle_leave` marks
  it left (receiving then returns `None`), and a stale enter is let go. The
  offer is finished (after a drop) and destroyed when the last clone of the
  drag's value goes, after the drop target has read what it needs. Windows
  that don't accept data drops behave exactly as upstream, and `http(s)`
  links in a `text/uri-list` are no longer logged as "not a file path".
* **Data out.** `start_external_drag` offers every type of an
  `ExternalDragPayload::Data` (`ExternalDragContent`); `Send` writes bytes
  through the clipboard's writer or streams a file on the background
  executor. `DndFinished` sends `FileDropEvent::Delivered` before `Ended` when the drop
  was a move (patch 5). A
  drag that replaces one that never ended destroys the old source, and the
  events of a replaced source are ignored.
* **Margins.** `set_layer_margin` sets the layer surface's margins and
  commits (`WaylandSurfaceState::set_margin`).

## 5. Drag and clipboard protocol robustness (`wayland/client.rs`, `wayland/clipboard.rs`)

* **Finishing a drop.** `wl_data_offer.finish` is a fatal protocol error
  (`invalid_finish`, the whole connection) unless the compositor chose an
  action. The offers ask for `copy` only, so a source that offers just `move`
  or `ask` (or a drop the compositor never settled) used to kill the shell's
  connection. The offer's user data (`DataOfferState`) records
  `wl_data_offer.action`, and `finish_offer` finishes (data drops and file
  drops alike) only when one was chosen; otherwise the offer is only destroyed.
* **Outbound bytes.** `send_bytes` (`write_when_writable`) makes the receiver's
  pipe non-blocking before registering its calloop write source, so the
  `WouldBlock` path works: a large or stalled receiver no longer blocks the
  event loop (and with it Wayland dispatch and rendering) in `write`. It takes
  the bytes by `AsRef<[u8]>`, so a drag's `Arc<[u8]>` is shared instead of
  copied per request.
* **Delivered means moved.** `wl_data_source.action` is recorded and
  `FileDropEvent::Delivered` is sent on `DndFinished` only when the final
  action is `move`; a copy leaves the dragged items where they were.
* **Clipboard reads.** `read_bytes` no longer unwraps `connection.flush()`: a
  full socket or a dead connection used to panic on a paste or a selection
  change.

## 6. Closing windows after the connection (`wayland/client.rs`)

A fatal protocol error ends the event loop, then the client and its window map
are dropped while windows may still be alive. `drop_window` and the frame timers
returned through `get_client`, which panics on a dead client, or unwrapped the
missing window, so the shutdown panicked in `Drop`. They now do nothing when
the client or the window is already gone.

## 7. Backing off failed draws (`wayland/window.rs`, `wayland/client.rs`)

A draw that failed (`Lost`, `Outdated`, `Timeout`, `Occluded`, a validation
error, a render error) re-armed the frame retry timer every 16.7 ms, with an
error logged each time, for as long as it kept failing: a surface that never
presented (a powered-off output) or a dead connection spun at 60 Hz per window.
The retry delay now doubles with every failed draw in a row (16.7 ms, 33 ms, …)
up to one second (`frame_retry_delay`), and the window logs once, after eight
failures, that it retries slowly. The first successful draw resets it.
`schedule_frame_retry` takes the delay. GPU device-loss recovery (`gpui_wgpu`
patch 19) hands the window its own delay the same way, instead of sleeping.
Each retry still inserts its own one-shot calloop timer; at one a second that
is not worth a persistent source.

## 8. Idle GPU memory released on a timer (`wayland/window.rs`, `wayland/client.rs`)

After a draw, a window asks the renderer when its idle path textures and
transition resources are due for release (`gpui_wgpu` patch 20) and arms one
timer (`schedule_idle_release`, `idle_release_fired`) that frees them without
drawing a frame and re-arms for whatever is left. The X11 window has no such
timer and keeps the release-on-next-draw behaviour.

## Tests

`linux::wayland::client::tests`: a data drag enters and drops without
finishing its offer itself, one that leaves (or goes stale) lets its transfer
go, and a typed drag offers each type once.

`linux::wayland::client::tests` (patch 5): an offer is finished only once the
compositor chose an action (none, `none` and a later `none` after a `copy` do
not count); only a `move` is delivered.

`linux::wayland::clipboard::tests` (patch 5): a 2 MiB payload written through
`write_when_writable` never blocks the event loop while nobody reads, arrives
intact as the reader drains it, and ends with the writer's close.

`linux::wayland::window::presentation_state_tests` (patch 7): the retry delay
starts at one frame interval, doubles per failed draw and stays at the cap.

`linux::wayland::touch::tests` (9 unit tests): a tap starts and ends where
the finger lifted; motions are coalesced per frame and flushed before `up`;
a motion to the same position is no event; `cancel` ends every point without
a tap and leaves nothing to end later; points keep their own window and ids
are never reused when Wayland reuses its own; unknown ids are ignored; a
reused id without `up` cancels the stale contact; a closing window forgets
its points; positions keep their fraction at 1×, 1.25×, 1.5× and 2×.

`linux::wayland::touch::tests::gpui_window` (`TestAppContext`): the events
`TouchPoints` emits, dispatched into a real GPUI window the way
`dispatch_touch` does, click a button only after the finger lifts (a jitter
within the slop still taps); a tap beside it, a swipe that starts on it and
a cancelled touch on it click nothing, and the next tap still does.

Run them (the crate is outside Mandara's workspace; seed its lock from the
workspace's so the dependency versions match the shipped build):

```sh
cp Cargo.lock vendor/gpui_linux/Cargo.lock
CARGO_TARGET_DIR=/var/tmp/mandara-gpui-linux cargo test \
  --manifest-path vendor/gpui_linux/Cargo.toml \
  --no-default-features --features wayland --lib
rm vendor/gpui_linux/Cargo.lock
```

## Packaging changes

`Cargo.toml` spells out the workspace-inherited fields and dependencies with
the exact specs from Zed's root manifest at that revision (sibling Zed crates
from the same git revision; the workspace's `Cargo.lock` resolves the same
versions as before, only the crate's source changes), declares its own empty
`[workspace]` so it stays outside Mandara's, sets `publish = false`, and keeps
only `unexpected_cfgs = "allow"` of Zed's workspace lints. Its own
`[patch]` points `gpui` and `gpui_wgpu` at the vendored copies for its tests;
Mandara's root patch does the same for the shipped build. Vendored: `src/`
and `LICENSE-APACHE` (upstream's is a symlink to the repository root's
licence; this is the file itself).

## Re-applying on an upgrade

Copy `crates/gpui_linux/src` from the new revision, then re-apply patch 1:
add `wayland/touch.rs` and `mod touch;` in `wayland.rs`, and in
`wayland/client.rs` the `wl_touch` import, the `wl_seat_name`, `wl_touch`
and `touch` state fields (initialized in `connect`, where the seat's
registry name is captured), `release_touch`, the release in
`WaylandConnection::drop`, `forget_window` in `drop_window`, the touch
release on seat replacement and seat `GlobalRemove`, the `Touch` capability
in `Dispatch<wl_seat::WlSeat>`, `dispatch_touch` and
`Dispatch<wl_touch::WlTouch>`. Search for `Mandara patch` to find each one.
Drop the patch once upstream's Wayland client handles `wl_touch` itself.
