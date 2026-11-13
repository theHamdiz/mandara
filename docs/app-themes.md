# App themes

Open **Studio → Themes → Other apps** and enable the apps you want to
match Mandara's palette. Every integration starts off. Changing the shell's
theme or light/dark mode regenerates enabled theme files in the background.
The apps themselves are optional and installed separately; Mandara's installer
does not install or launch them.

The chevron beside each switch shows its setup information. Existing GTK,
Qt, Kvantum, KDE, terminal and editor integrations are described in
[Color science](color-science.md). Four more apps have native theme files:

| App | Generated file, relative to `$XDG_CONFIG_HOME` (normally `~/.config`) | Activate it |
|---|---|---|
| Cava | `cava/themes/mandara` | Set `theme = 'mandara'` in your config's `[color]` section; press `r` to reload that selection, then `c` for later palette changes. |
| tmux | `tmux/mandara.conf` | Add `source-file ~/.config/tmux/mandara.conf` to your tmux config, adapting the path if you use another config directory. Reload that file in tmux after a palette change. |
| Zellij | `zellij/themes/mandara.kdl` | Set `theme "mandara"` in `zellij/config.kdl`. Theme files are reloaded live. |
| Spicetify | `spicetify/Themes/Mandara/color.ini` and `user.css` | After installing and setting up Spicetify, select `current_theme Mandara` and `color_scheme Mandara` with its `config` command, then run `spicetify apply`. Use `spicetify refresh` after palette changes. |

These built-in switches write only generated files. They do not edit apps' main
configs, run Spicetify or signal other processes. Switching a built-in integration
off removes its unchanged generated files and restores a file it backed up.
If you edited a generated file, your changes are preserved instead. Remove
your own include or theme selection when you stop using an integration.
Firefox's export stays under Mandara's cache and leaves `~/.cache/wal` alone.

Compositor borders use **Studio → Compositor → Integration → Sync border
colors**, where the backend supports it. They use Mandara's validated generated
drop-in, keeping your own compositor settings intact.

## More apps and your own templates

In **Studio → Themes → Other apps**, choose **Browse app templates** to search
available catalogs. **Review** downloads just that app and lists the files and
commands it uses. Read the list, then choose **Enable** to apply your
palette. **Update template** fetches a newer revision for another review.
Approved revisions keep working offline. Changing a theme, a Look or light/dark mode uses installed files and works
offline. Templates use Mandara's palette, so they work with built-in themes,
wallpaper colours and your own themes without a separate theme for each app.

The catalog is searched as a whole and displayed a page at a time. Use
**Instructions** for each app's requirements and activation steps. Apps and
their helper programs remain separate installations. The installer offers Bash
for imported scripts; app-specific helpers are listed in each package’s instructions. **Template sources** lets
you edit the HTTPS catalogs in TOML; save with Ctrl+Enter. Nothing in a catalog
is downloaded or enabled merely by starting the shell.

**Run template setup commands** controls the selected templates' setup and
reload commands. Read each app's instructions before enabling its template.
Switch commands off to render files without running imported setup steps. They
run in the background with a deadline and bounded output; failures appear on
this page and in `theme apps status`. Managed file writes preserve symlinks,
back up existing files and undo only changes still owned by the template.
External setup commands follow the imported package's own undo instructions.
If commands are switched off before a template is disabled, its cleanup stays
pending and is retried when you allow commands again. Built-in integrations and
custom templates share an ownership record, so a whole-file template cannot
replace a file that contains managed imports.

Imported KDE schemes merge their colour keys into `kdeglobals`, preserving
unrelated settings. The browser integration can install an owned native-messaging
host on Linux, deliver the palette to the browser extension, and manage its
optional CSS across profiles. An existing host from another installation is
preserved. If that host requires the shared `~/.cache/wal/colors.json` palette,
it appears in the review with a warning: other Pywal apps will read those
colours too. The shared file is never replaced without that approval. In a
user-authored browser template, choose this output explicitly if you want to
share it with an existing host.
When you turn the template off, Mandara restores its files and
notifies connected extensions that its palette is unavailable. Use the browser
extension’s **Disable theme** control to reset its active browser colours;
its native messaging protocol does not provide a remote reset command.
Browser extensions and apps are never installed automatically.

Compositor templates use native validation and reversible changes. MangoWM,
Niri, Hyprland, Umbriel, Sway and Scroll read generated colour includes. Jay and
DriftWM receive edits to their colour keys; labwc uses `themerc-override`.
Mandara backs up existing files, preserves unrelated settings and symlinks, and
restores only the values it still owns when you disable the template. Imported
scripts never replace a compositor's primary config. DriftWM's older catalog
keys are mapped to its current decoration settings. Jay follows its existing
reload policy; if automatic reloading is off, reload its TOML config to see the
new colours.
If you edit or retarget the config file while its native check runs, Mandara
keeps your edit and asks you to apply the theme again.

### Author a template

Open **Your templates** for a TOML editor, or import a local document:

```bash
mandara msg theme apps import ~/Themes/apps.toml
```

Importing adds entries without enabling new ones. Reusing an existing template
ID or custom-colour name is rejected so an import cannot replace your settings.
In Studio, edit your document and save with Ctrl+Enter:

```toml
[templates.example]
enabled = true
output_path = '~/.config/example/colors.css'
content = '''
:root {
    --background: {{ colors.surface.default.hex }};
    --accent: {{ colors.primary.default.hex }};
}
'''
```

Use `input_path` instead of `content` for a template file; relative paths resolve
beside the imported document. `output_path` also accepts an array. Paths support
`~`, `$HOME` and the XDG directories. `input_path_modes` can select separate
`dark` and `light` files, and `requires_path` skips clients not installed yet.

The renderer exposes `colors.<role>.default`, `.dark` and `.light`, Material
colour roles, Mandara's semantic and terminal colours, tonal `palettes`, `mode`,
`image`, `config_dir`, `config_file` and `config.shell.font_family`. Formats
include `hex`, `hex_stripped`, `rgb`, `rgb_csv`, `rgba`, `hsl` and `hsla`.
Filters support channel changes, alpha, hue rotation, contrast and colour
blending. Conditions and loops use `<* ... *>` blocks.
Loops expose `loop.index` starting at zero, plus `loop.first` and `loop.last`.

Documents can declare `[config.custom_colors]`, including different light/dark
sources and optional harmonisation. Advanced entries support `index` ordering,
`compare_to` and `colors_to_compare`, GTK extensions, dynamic input/output paths,
and `pre_hook`, `post_hook` and `undo_hook`. Post hooks run with bounded
concurrency; `hook_async = false` waits for earlier hooks before running.
Commands require the setup-command
switch. Input and output sizes, render work and command runtimes are bounded;
invalid templates report an error without silently falling back to another
colour or replacing unrelated configuration.

Useful commands:

```bash
mandara msg theme apps catalog          # cached discovery; add refresh to fetch again
mandara msg theme apps enable community:rofi  # download and review; does not activate
# Set APPROVAL_TOKEN to the token returned by the review after reading its file and command list.
mandara msg theme apps enable community:rofi "$APPROVAL_TOKEN"
mandara msg theme apps disable community:rofi
mandara msg theme apps status
mandara msg theme export                # files, skipped entries and errors as JSON
```

Format references: [Cava](https://github.com/karlstav/cava#configuration),
[tmux](https://man.openbsd.org/tmux#STYLES),
[Zellij](https://zellij.dev/documentation/themes.html), and
[Spicetify](https://github.com/spicetify/cli/blob/main/Themes/SpicetifyDefault/color.ini).

The review names the selected package. Focus its file list with Tab, then use
the arrow keys, Page Up/Down or Home/End. Escape closes the review.

Native compositor theme edits keep the 20 newest timestamped recovery copies
per file. The original backup used by managed-file undo is kept separately.

Owned files and import lines record their recovery information before they change. If Mandara stops during an apply or undo, the next sync resumes from that record. Detected conflicts preserve personal edits, and a missing original backup is reported for repair. Browser CSS requests recheck the active integration after waiting for the theme worker, so a queued request cannot re-enable styling after it has been switched off.

Catalog and review controls follow the interface’s reading direction.
