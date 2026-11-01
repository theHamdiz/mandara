# Scripting, automation and agents

Mandara is built to be driven by you, your scripts or an AI agent.

## Agent-ready

Mandara is built to be driven from the command line, by you, your scripts
or an AI agent. Almost every action in the bar, panels and Studio is a
command on the running shell, and the catalog describes itself:

```bash
mandara commands --json                      # every command with its arguments and examples
mandara msg --json shell state               # what is open right now, as JSON
mandara msg config get /bar/height           # read any setting; config set /bar/height 30 changes it live
mandara msg --json plugins list              # every plugin, enabled or not, and why one failed
mandara studio plugins --plugin weather --settings   # open Studio exactly where a setting lives
```

Launcher skills chain commands into named routines, hooks run commands on
desktop events, and installs and setup have dry runs
(`mandara install --dry-run`, `mandara setup --check`). See the
[command tour](commands.md) and, for contributors and coding agents,
[AGENTS.md](../AGENTS.md).

## What you can build on it

Mandara is a platform as much as a shell:

- **Script everything.** Every action the UI can take is a documented
  command (`mandara commands --json`): bind it to a key, call it from a
  script, or let an AI agent drive your desktop through it.
- **React to your desktop.** Hooks run your commands on 27 events (locked,
  idle, plugged in, battery low, wallpaper or colors changed, before sleep…),
  with details in environment variables.
- **Name routines.** Launcher *skills* chain commands into one named step,
  and slash commands (`/g`, `/yt`, `/todo`, your own) turn the launcher into
  a command line.
- **Theme everything, if you want.** One palette drives the shell and, for each app you switch on, your terminal,
  editors, GTK and Qt apps, browsers and compositor borders.
- **Extend it safely.** Sandboxed plugins can add bar items, panels and
  sidebar cards, and reach users through the marketplace the day you tag a
  release.
