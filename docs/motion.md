# Motion

Open **Studio → Motion** to choose Default, Snappy, Smooth, Bouncy or Subtle.
Presets change stiffness, damping, mass and animation speed together. Editing
any of these shows **Custom**; your existing tuning is preserved on upgrade.

The live preview slides and fades a dot using your current spring. **Replay**
starts it again; **Reset preview** returns the dot to its starting point.
Changing the controls replays it automatically. The preview stops when hidden
and is released when you leave the page.

**Reduce motion** applies transitions instantly. **Follow system preference**
also honours the desktop portal's reduced-motion setting, including changes
made while Mandara is running. If the portal is unavailable or has no
preference, your own choices still apply. **Motion intensity** softens
overshoot as it approaches zero; at zero transitions are instant.

Under **Surface transitions**, adjust playback speed separately for the bar,
panels, sidebar, launcher, dock, notifications and on-screen displays. Zero
makes that surface's transitions instant; 1 keeps the global speed; 2 doubles
it. These controls cover surface opening, closing and movement. Plugin content
keeps its own animation settings and still respects global reduced motion.
The dock's opening-app animation and attention bounce are under **Dock**;
wallpaper depth is controlled by the **Wallpaper depth** plugin.

Mandara's springs can change target halfway through a transition while keeping
their velocity. Damping controls overshoot and Animation speed controls timing;
fixed durations, easing curves and artificial delays are not applied to springs.
Accessibility and per-surface choices are preserved when you select a preset.

The config lives under `[motion]`, with `follow_system = false` and
`intensity = 1.0` by default. The seven `[motion.surfaces]` speeds default to
`1.0` and accept `0.0` through `3.0`. For example:

```toml
[motion]
follow_system = true
intensity = 0.5

[motion.surfaces]
dock = 0.0
launcher = 1.5
```
