# Usage

Starting Mandara, the commands you'll use most, default shortcuts and where settings live.

```bash
mandara                 # start (with crash-restart supervisor)
mandara studio          # settings & design studio (opens on Home)
mandara studio displays # straight to a page (docs/studio.md lists them)
mandara studio plugins --plugin weather --settings   # straight to a plugin's settings
mandara setup           # (re)write compositor integration
mandara install --dry-run   # what a (re)install would do: packages, compositor, login screen
mandara --version       # exact build: version, git revision, content hash
mandara msg <command>   # control the running shell
mandara commands        # every command, with arguments and examples
```

## Commands

Everything the UI can do is a documented command (`mandara commands
--json`), usable from scripts, keybinds and AI agents. A few:

```bash
mandara msg launcher toggle
mandara msg theme set mandara-corniche
mandara msg look apply corniche-dusk
mandara msg desktop edit
mandara msg bar position left
mandara msg wallpaper picker toggle
mandara msg screenshot region
mandara msg notifications dnd on 45m
mandara msg nightlight toggle
mandara msg display profile save Desk
mandara msg idle status
mandara msg network vpn up Office
mandara msg studio displays
```

→ **[The full command tour](commands.md)**, grouped by area.

## Default shortcuts

| Keys | Action |
|---|---|
| Super+Space | Launcher |
| Ctrl+Shift+W | Wallpaper picker |
| Super+B | Sidebar |
| Super+Shift+S | Screenshot (region) |
| Super+Shift+V | Clipboard history |
| Super+Shift+T | Light / dark |
| Super+Shift+E · Ctrl+Alt+Delete | Power menu |
| Alt+Tab · Alt+Shift+Tab | Window switcher (new installs; Studio → Window switcher → Bind Alt+Tab) |

Shortcuts can be changed in Studio → Keyboard shortcuts. Keys you already
use in your compositor are never overridden.

The color picker has no default key, so it can't clash with your binds. One
suggestion is Super+Shift+C running `mandara msg color-picker pick` (MangoWM:
`bind=SUPER+SHIFT,c,spawn,mandara msg color-picker pick`).

## Configuration

`~/.config/mandara/config.toml` is watched live, and Studio writes the same
file. Generated compositor drop-ins live in `~/.config/mandara/generated/`.
Plugins live in `~/.local/share/mandara/plugins/` (installed) and
`plugins-dev/` (your own).
