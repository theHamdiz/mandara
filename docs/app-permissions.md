# App permissions

Enable **App permissions** in **Studio → Plugins** to review installed Flatpak
applications. Search by app name or ID, choose its installation and inspect
its native sandbox grants.

The editable controls cover explicit network, IPC, display, audio, device and
filesystem grants. **Audio and microphone** is one real Flatpak permission:
revoking it affects playback and recording together. **All devices** is broad
access, including cameras and graphics devices. Portals can still grant files
you explicitly choose.

Conditional, scoped and overlapping grants stay locked with an explanation.
Host-file access also locks the narrower system-folder controls. Full
`host-root` access locks overlapping filesystem controls until its native rule
is changed. Revoked
explicit permissions remain available to turn back on. Their native rules remain visible under **Native permission details**.
Equivalent absolute paths, home-relative paths and XDG folder aliases also lock
the affected controls when another grant keeps that folder accessible. Custom
XDG folder locations are read again during inspection and before saving.
X11 fallback depends on Wayland availability, so those display grants also
stay locked. The panel does not pretend that a broad device grant provides independent camera
or microphone switches, or silently replace a conditional grant with an
unconditional one.

Each edit changes only its selected key in your personal override file. Mandara
backs up existing content, preserves comments, unrelated groups and symlinks,
and rejects a file that changed since it was read. **Undo** restores the last
change only while Mandara's published file is still current. The footer names
the app that the saved Undo belongs to. A persistence error keeps its warning
and Undo receipt rather than hiding a change that already reached disk.
Before saving, Mandara checks the app's effective grants again. A changed
installation or global override can lock a previously editable control; refresh
the panel to review the new permissions.

Personal overrides apply to the app across all its installations. Restart the
app after editing or using Undo so the new sandbox is used. App permissions
requires Flatpak on Linux; it does not change native applications or manage
portal grants.

Custom installations selected through `FLATPAK_USER_DIR` use that same folder
for inspection and personal overrides.

The optional installer package is `flatpak`; on NixOS, enable Flatpak in your
system configuration. `mandara commands` documents `permissions apps`,
`permissions inspect`, `permissions set` and `permissions undo`. Select the
installation returned by `permissions apps`. IPC writes use the same guarded
transactions and Undo as the panel and return their actual completion.
