<!-- One topic per pull request. AGENTS.md has the full guide; this is the
     checklist it asks for, in the order a reviewer checks it. -->

## What this changes

<!-- What people will notice, in a sentence or two, and why. -->

## Checklist

- [ ] **One topic**, in [Conventional Commits](https://www.conventionalcommits.org)
      style (``feat:``, ``fix:``, ``perf:``, ``docs:``…). A breaking change says
      so with ``!`` or a ``BREAKING CHANGE:`` footer.
- [ ] **No hand-made version bump:** crate versions and `versions.toml` are
      released by `cargo xtask bump` on `main`.
- [ ] **No shell, Python or other scripts:** tooling goes in `cargo xtask`,
      plugins are Rust compiled to WebAssembly.
- [ ] `cargo check --workspace --all-targets`, `cargo test --workspace` and
      `cargo clippy --workspace --all-targets` pass (warnings are errors).
- [ ] **Docs in the same change:** a line under `## Unreleased` in
      `CHANGELOG.md`, plus `README.md`, `docs/`, `PLUGINS.md` or the installer's
      package tables wherever they describe this.
- [ ] **Every new user-visible string** is in `t` / `tf` / `tr` / `tk` and
      translated in all fourteen catalogs; `cargo test -p mandara-i18n` passes.
- [ ] **No new magic numbers:** sizes, spacing, radii, delays, intervals and
      thresholds are settings with a default and a bounded range, or a named
      `const` with a reason. `cargo test -p mandara-ui --test no_hardcoded`
      passes (its counts are a ratchet).
- [ ] **New settings have a Studio control** in
      `crates/mandara-studio/src/registry/`; new commands are added to
      `docs/commands.md`; new built-in plugins have art in
      `assets/plugins/<id>/`, a `assets/catalog.json` entry and `cargo xtask art`
      re-run.
- [ ] **Compositors:** a compositor feature works on MangoWM, Niri, Hyprland,
      Umbriel, Sway and River Classic, or is locked in Studio with a sentence saying why.
- [ ] **User files untouched:** generated config goes in
      `~/.config/mandara/generated/`, one include line is added to the user's
      file (River: the user adds one startup line themselves), and any edit is validated by the compositor's own checker and rolled
      back if it is worse.

## How it was tested

<!-- The commands you ran and where. Bugs, UI and end-to-end checks belong in
     the demo sandbox (`cargo xtask sandbox up|shot|msg|pointer`), never in a
     real session. -->

## Performance (only when a change claims a win)

<!-- Before and after, from release builds: `cargo xtask resource-check
     target/release/mandara`, `mandara msg diagnostics frames`. Doc figures are
     rounded ceilings, never guesses. -->

## Screenshots (only when the interface changed)

<!-- From `cargo xtask sandbox shot`, with demo data: no other apps behind
     Mandara, no personal names, networks, IP addresses, locations or project
     names. -->
