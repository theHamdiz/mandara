# Panel images

Hover a panel and click the **camera** beside its pin and float buttons.
A green check confirms that its image is on the clipboard. Floating panels
have the camera in their top control strip; the main menu has a camera in
its corner tab. The host supplies it for every panel, including third-party
plugins.

The copy shows the visible panel rectangle at native display resolution,
with its current colours and scroll position. Docked panels exclude their
external control tab; floating panels include their frame. Capture starts
on the click and copies a PNG directly to the clipboard.

The same action is available for an already visible panel:

```sh
mandara msg panel capture clock
```

Capture and encoding run off the UI thread in a short-lived worker. It
requests only that panel's region, converts the owned pixels in place and
shares one image-producing slot with complete reading-card exports. Closing
or hiding the panel cancels its work; a stalled capture times out. Working
pixels and Wayland buffers are released, with only the compressed clipboard
image retained by the clipboard service. No GPU image cache is created.

Raw pixels and the compressed clipboard image each have a 32 MiB limit.
Oversized captures report an error instead of growing without a bound.
Confirmation timing follows the existing panel toast-duration setting.
