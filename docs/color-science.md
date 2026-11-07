# Color science

- **Material You** palettes (HCT/CAM16: Tonal Spot, Vibrant, Expressive,
  Fidelity, Fruit Salad, Monochrome) from your wallpaper. Seeds are re-ranked
  so vivid colors lead, never a grey sky.
- **Mandara presets** inspired by Alexandria: *Mandara Corniche*, *Montaza
  Twilight*, *Mandara Obsidian*, *Qaitbay Citadel*, *Pharos Night* and
  *Stanley Sunset*. Community favorites are included too: Catppuccin, Rosé
  Pine, Kanagawa, Tokyo Night, Nord, Gruvbox, Eldritch and Vesper.
- **OKLCH tweening**: theme changes crossfade without passing through muddy
  greys.
- **Light and dark** switch manually, **follow the system**, or run on a
  **daily schedule**.
- **Cross-app theming** (opt-in, per app): Ghostty, GTK 3/4, Qt5/Qt6
  (qt5ct/qt6ct), Kvantum, Zed, Fuzzel, btop, kitty, foot, Alacritty,
  WezTerm, Helix, Neovim, Vesktop / Vencord, KDE, Cava, tmux, Zellij and
  Spicetify colour schemes can
  follow Mandara live. **Every app is off until you switch it on** in
  Studio → Themes → Other apps (or `mandara msg config set
  /integrations/gtk4 true`): until then Mandara writes nothing outside
  `~/.config/mandara` and `~/.cache/mandara`. Switching an app off takes
  back exactly what Mandara changed (the import line in `gtk.css`, the
  `fuzzel.ini` block, the Kvantum link and selection, the theme files it
  created) and restores any file it had replaced from its
  `.mandara-backup`; it records what it applied in
  `~/.config/mandara/generated/app-themes.toml`. Compositor border colours
  are off by default too (Studio → Compositor). **Upgrading from 0.1:** if
  you never changed the app list, it is now recorded as off and the files
  0.1 wrote are left exactly as they are; to keep apps following Mandara,
  switch them back on (switching one on and off again also removes what
  0.1 added). A list you had changed is kept as you left it. Select them
  once: kitty `include themes/mandara.conf`, foot
  `include=~/.cache/mandara/themes/foot.ini`, Alacritty
  `general.import = ["~/.cache/mandara/themes/alacritty.toml"]`, WezTerm
  `color_scheme = "Mandara"`, Helix `theme = "mandara"`, Neovim
  `:colorscheme mandara`, KDE *Colors → Mandara*.
  **Firefox / Zen** (opt-in): `~/.cache/mandara/themes/firefox/colors.json`
  is pywal-format (point Pywalfox at it, e.g. symlink it to
  `~/.cache/wal/colors.json` if you don't use pywal) and `userChrome.css`
  there can be `@import`ed from your profile's `chrome/userChrome.css`.
  See [App themes](app-themes.md) for the additional apps' activation steps.
- **Accessibility & style**: high-contrast mode (WCAG AAA text on every
  surface), corner roundness, shadows, panel edges, backdrop dim, text
  weight and a global animation-speed multiplier.
