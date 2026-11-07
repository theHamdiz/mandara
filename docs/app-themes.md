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
| Cava | `cava/themes/mandara` | Set `theme = 'mandara'` in your config's `[color]` section; press `c` to reload colours. |
| tmux | `tmux/mandara.conf` | Add `source-file ~/.config/tmux/mandara.conf` to your tmux config, adapting the path if you use another config directory. Reload that file in tmux after a palette change. |
| Zellij | `zellij/themes/mandara.kdl` | Set `theme "mandara"` in `zellij/config.kdl`. Theme files are reloaded live. |
| Spicetify | `spicetify/Themes/Mandara/color.ini` | After installing and setting up Spicetify, select `current_theme Mandara` and `color_scheme Mandara` with its `config` command, then run `spicetify apply`. Use `spicetify update` after palette changes. |

Mandara writes only the generated files. It does not edit those apps' main
configs, run Spicetify or signal other processes. Switching an integration
off removes its unchanged generated files and restores a file it backed up.
If you edited a generated file, your changes are preserved instead. Remove
your own include or theme selection when you stop using an integration.
Firefox's export stays under Mandara's cache and leaves `~/.cache/wal` alone.

Compositor borders use **Studio → Compositor → Integration → Sync border
colors**, where the backend supports it. They use Mandara's validated generated
drop-in, keeping your own compositor settings intact.

Format references: [Cava](https://github.com/karlstav/cava#configuration),
[tmux](https://man.openbsd.org/tmux#STYLES),
[Zellij](https://zellij.dev/documentation/themes.html), and
[Spicetify](https://github.com/spicetify/cli/blob/main/Themes/SpicetifyDefault/color.ini).
