# Motion

Open **Studio → Motion** to choose a preset. Each one is a tile with its name
and a line on how it feels; point at a tile to preview it without changing
anything, click it to use it.

| Preset | How it feels | What it sets |
| --- | --- | --- |
| **Default** | Mandara's balanced spring with a soft landing | The shipped spring |
| **Snappy** | Quick and firm, settling almost at once | Stiff, well damped, played 1.3× faster |
| **Smooth** | Slow and silky, with no bounce | Soft and fully damped |
| **Bouncy** | Playful springs that swing past and back | Lightly damped |
| **Subtle** | Brisk and calm, barely any overshoot | Well damped, played 1.4× faster |
| **Corniche tide** | Everything rises like a swell, lists rolling in wave after wave | A slow swell (a little extra mass); content rises from below, a strong cascade, some parallax |
| **Lighthouse sweep** | A quick beam sweeps across, lighting each row in turn | A fast, firm spring; content sweeps in from the side where reading starts, with the longest cascade |
| **Felucca drift** | Heavy and unhurried: surfaces glide in on a slant, content trailing like a sail | A heavy, slow spring played at 0.9×; content drifts in on a slant from the side where reading ends, with deep parallax |
| **Magnetic** | Pulled into place with one crisp tick past and back | A very stiff, lightly damped spring: a fast pull and one short overshoot, with a little stretch |
| **Mercury** | Liquid metal: shapes stretch as they travel and round off at rest | A lively spring with strong liquid stretch, a light cascade and parallax |
| **Raindrop** | Drops in from above and lands with an elastic bounce | The bounciest spring; content drops in from above and lands past its rest, with a cascade and some stretch |

A preset writes its values (stiffness, damping, mass, animation speed and the
choreography below) and nothing else: reduce motion, follow system
preference, motion intensity and the per-surface speeds stay yours. Editing
any of its values afterwards shows **Custom**, and your existing tuning is
preserved on upgrade.

The six choreographed presets reach further when you let them. Under
**Presets**, *Presets style the dock and wallpaper* (on by default) also sets
each one's [dock animations](#dock) and wallpaper transition: Corniche tide →
Tide, Lighthouse sweep → Wipe, Felucca drift → Dissolve, Magnetic → Glitch,
Mercury → Shatter, Raindrop → Ripple. *Presets style windows too* (off by
default) also sets the matching [window animations](#windows). The five
classic presets change only `[motion]`. The two switches are
`[motion] preset_desktop` and `preset_windows`. From the command line, `mandara msg motion presets`
lists them (with the one in use) and `mandara msg motion preset tide` applies
one by its id: `default`, `snappy`, `smooth`, `bouncy`, `subtle`, `tide`,
`lighthouse`, `felucca`, `magnetic`, `mercury` or `raindrop`.

The live preview slides a dot and brings in a row of tiles with your current
spring and choreography, or with the preset under the pointer. **Replay**
starts it again; **Reset preview** returns it to its starting point. Changing
the controls replays it automatically. The preview stops when hidden and is
released when you leave the page.

## Choreography

Under **Choreography**, four controls shape how content enters on top of the
spring. They are worked out from each surface's own spring as it moves, so
they add no timers, and every surface still comes to rest exactly where it
would without them.

- **Entrance**: where entering content comes from. *From its button or
  edge* (the default) keeps each surface's own path; *Rise from below*,
  *Drop from above* (it lands on the spring's overshoot, so a loose spring
  bounces), *Sweep from the reading side* and *Drift in on a slant* move the
  content of drop-down panels, the launcher, notification popups and
  on-screen displays from that direction instead. Sideways entrances mirror
  in right-to-left languages.
- **Cascade**: lists, tiles and cards arrive one after another instead of all
  at once: the blocks of every panel, sidebar cards, launcher rows and grid
  tiles, and notifications arriving together. The last one still lands as
  its surface finishes opening.
- **Liquid stretch**: moving shapes stretch along their travel and round off
  at rest: drop-down panels lead the way they pour, and the launcher's
  selection stretches and thins as it hops between rows.
- **Parallax**: content trails its surface and settles a beat later, inside
  drop-down panels, the launcher and notification popups.

All three amounts run from `0` (off, the default) to `1`. Motion intensity
scales them too, and with reduced motion they are off entirely: transitions
are instant.

## Accessibility and surfaces

**Reduce motion** applies transitions instantly. **Follow system preference**
also honours the desktop portal's reduced-motion setting, including changes
made while Mandara is running. If the portal is unavailable or has no
preference, your own choices still apply. **Motion intensity** softens
overshoot and choreography as it approaches zero; at zero transitions are
instant.

Under **Surface transitions**, adjust playback speed separately for the bar,
panels, sidebar, launcher, dock, notifications and on-screen displays. Zero
makes that surface's transitions instant; 1 keeps the global speed; 2 doubles
it. These controls cover surface opening, closing and movement. Plugin content
keeps its own animation settings and still respects global reduced motion.
The dock's own animations have their styles under **Dock** below, and
**Studio → Dock** still switches the launch and attention feedback off;
wallpaper depth is controlled by the **Wallpaper depth** plugin.

Mandara's springs can change target halfway through a transition while keeping
their velocity. Damping controls overshoot and Animation speed controls timing;
fixed durations, easing curves and artificial delays are not applied to springs.
Drop-down panels and the sidebar bounce only as they open: closing, they move
at the same pace but stop as soon as they reach the bar or screen edge, so they
never swing back into view.

The config lives under `[motion]`, with `follow_system = false` and
`intensity = 1.0` by default. The seven `[motion.surfaces]` speeds default to
`1.0` and accept `0.0` through `3.0`. `[motion.choreography]` holds
`entrance` (`anchor`, `rise`, `drop`, `sweep` or `drift`; default `anchor`)
and `stagger`, `stretch` and `parallax` (`0.0` to `1.0`; default `0.0`).
Files without it load as before. For example:

```toml
[motion]
follow_system = true
intensity = 0.5

[motion.surfaces]
dock = 0.0
launcher = 1.5

[motion.choreography]
entrance = "rise"
stagger = 0.55
parallax = 0.4
```

## Windows

**Window animations** (the *Windows* group) sets how windows open, close,
move and switch workspaces: *Your compositor's own* (the default: Mandara
writes nothing), *Fade*, *Swell* (rise from below), *Beam* (a quick sideways
sweep), *Drift* (a slow, eased glide), *Snap* (zoom past and back),
*Liquid* (a springy zoom) or *Drop* (fall in and bounce). **Window animation
speed** plays the style faster or slower (0.25× to 3×).

These lines go only into Mandara's generated drop-in, never into your own
config, and only for compositors whose own checker accepted them:

| Compositor | What Mandara writes | Checked with |
| --- | --- | --- |
| MangoWM | `animation_type_open/close` (slide, zoom or fade), fades, durations, Bézier curves, zoom ratios, tag direction | `mango -p` |
| Niri | `window-open` / `window-close` (duration and curve), springs for `window-movement` and `workspace-switch` | `niri validate` (26.04) |
| Hyprland 0.55+ (Lua config) | `hl.curve` Bézier curves and `hl.animation` for windowsIn/Out/Move, fadeIn/Out and workspaces | `Hyprland --verify-config` (0.57) |

Older Hyprland setups (hyprlang) get none, since no Hyprland that reads that
format was available to check them; Studio says so. Sway, River and labwc
have no window animations, COSMIC and Plasma set their own, and Mandara can't
set them on Umbriel or Aqueous yet: on those the controls stay visible with
the reason instead. With *Compositor → Effects → Animations* off, nothing is
written. MangoWM picks the edge a window slides from itself, and Niri has no
slide or zoom of its own, so it gets each style's timing and springs.

The config is `[compositor.look.windows]` with `style` (`compositor`, `fade`,
`swell`, `beam`, `drift`, `snap`, `liquid`, `drop`) and `speed`.

## Wallpaper change

The **Wallpaper change** card shows the same settings as *Studio →
Wallpaper*: the transition, where it starts and its length
(`wallpaper.transition`, `transition_origin`, `crossfade_ms`).

| Transition | How it looks | Software wallpaper |
| --- | --- | --- |
| Fade, Wipe, Grow | As before | Same |
| Ripple | A circle opens from the pointer or the centre | A rectangle opening from the centre |
| Tide | A wave rises from the bottom and washes the new image in | A straight edge rising from the bottom |
| Tear | The old image rips along a jagged seam and both halves peel away | The new image opening outward from the middle |
| Glitch | Slices shear sideways with a brief red and cyan split, then settle on the new image | Fade |
| Shatter | The old image breaks into shards that burst from the origin and fall | Fade |
| Dissolve | The new image arrives tile by tile | Fade |
| Sketch | See [below](#sketch-wallpaper-transition) | Fade |

The desktop normally uses the software wallpaper, where the compositor crops
and fades whole planes: it can't move pieces of the old image, so the pieced
transitions are drawn as the cheaper equivalent in the last column. The GPU
wallpaper (`MANDARA_WALLPAPER_RENDERER=gpui`) draws every one. Each piece is
a clip of the old or new image placed by a pure function of the progress,
with seams, slices and shards from a hash of their index: nothing is stored
between frames, and every transition ends on the new image exactly in place.
Glitch moves smoothly and never flashes. With reduced motion every change
cuts at once (Sketch becomes a plain fade). `transition_origin` is `center`
or `pointer` (the pointer's place on the desktop, else the centre).

## Dock

When the dock is on, the **Dock** group on the Motion page picks a style for
each of its animations. They play at the dock's speed under **Surface
transitions** (zero makes them instant), stop while the dock is hidden, and
with reduced motion every one of them is off: changes apply at once. At rest
the dock always sits exactly where and how it would without them.

| Setting | Styles (default first) | What moves |
| --- | --- | --- |
| **Opening apps** | *Bounce*, *Pulse*, *Ripple*, *Off* | A launching app's icon hops, swells and settles, or sends rings out from under it, until its first window appears |
| **Apps asking for attention** | *Bounce*, *Pulse*, *Wiggle*, *Off* | An app with an urgent window or a critical notification calls you in short bursts with a rest between them |
| **Running indicators** | *Grow*, *Fade*, *Instant* | The dots (or bar) under an app grow in as it opens windows and shrink away as it closes them |
| **Minimizing** | *Dip*, *Pulse*, *Off* | The app's icon squashes into the dock and springs back, or swells once, when one of its windows minimizes (the window's own animation is the compositor's) |
| **Adding and removing** | *Grow*, *Fade*, *Instant* | New icons grow in, removed ones shrink out, and the others slide over, also after you drag one to a new place |
| **Showing and hiding** | *Slide*, *Fade*, *Zoom* | The dock slides in from its edge, fades in place, or grows out of its edge while fading in, for auto-hide too |

**Animate opening applications** and **Bounce for attention** in **Studio →
Dock** still turn the first two off, whatever their style. The icons the dock
shows when it opens appear at once; only icons that come and go afterwards
are animated.

The six presets from **Corniche tide** on each carry a matching set of dock
animations: Tide pulses and rolls icons in on springs, Lighthouse sends
ripples and fades, Felucca fades and sways for attention, Magnetic bounces,
wiggles and dips, Mercury pulses and zooms the dock out of its edge, and
Raindrop bounces, dips and zooms. The classic presets leave the dock as it is.

They live under `[dock.motion]`: `launch` (`bounce`, `pulse`, `ripple`,
`none`), `attention` (`bounce`, `pulse`, `wiggle`, `none`), `indicator`
(`spring`, `fade`, `none`), `minimize` (`dip`, `pulse`, `none`), `items`
(`spring`, `fade`, `none`) and `reveal` (`slide`, `fade`, `scale`). Docks
from before keep their bounces and slide, and gain the indicator, minimize
and icon animations:

```toml
[dock.motion]
launch = "ripple"
attention = "wiggle"
reveal = "scale"
```

## Sketch wallpaper transition

**Sketch** (the *Transition* choice in the **Wallpaper change** card, and in
**Studio → Wallpaper → Fit & effects**) draws the new wallpaper's outline
first, as lines that grow along their length, then fills the image in. When
Mandara already has a mask of the wallpaper's subject (one you made, or one
the depth effect found the last time the wallpaper was shown), the subject's
outline is drawn first; otherwise the lines come from the picture's edges. The
lines are traced once per wallpaper and screen size in the background, from a
small copy of the image, and kept for the next change back.

| Setting | Choices (default first) | What it does |
| --- | --- | --- |
| **Line colour** | *Theme accent*, *Ink black*, *White*, *The wallpaper's own colour* | Dark lines draw over a paper-light wash, light ones over a dark wash |
| **Line thickness** | 2 px (0.5 to 6) | How thick the strokes are |
| **Glow** | on | A soft halo under the lines |
| **Drawing order** | *Subject first, then the background*, *Top to bottom*, *From the centre*, *From the pointer* | Which lines are drawn first |
| **Drawing vs filling** | 0.55 (0.2 to 0.85) | The share of **Transition length** spent drawing; the rest fills the image in |
| **Fill style** | *Watercolour bleed from the lines*, *Fade in*, *Wipe along the strokes* | How the image arrives once its lines are drawn |
| **Keep the outline** | off | Leaves a faint copy of the lines over the wallpaper afterwards (it does not animate) |

Sketch reads best with a longer **Transition length** (1.5 to 2 s). With
reduced motion it is a plain fade, and the default software wallpaper
renderer always draws it as a fade (the lines need the GPU wallpaper,
`MANDARA_WALLPAPER_RENDERER=gpui`). A tiled wallpaper fades too. The options
live under `[wallpaper.sketch]`:

```toml
[wallpaper]
transition = "sketch"
crossfade_ms = 1800

[wallpaper.sketch]
ink = "wallpaper"        # accent, ink, white, wallpaper
line_width = 2.5
glow = true
draw_order = "subject"   # subject, top-down, center, pointer
draw_share = 0.55
fill_style = "bleed"     # fade, bleed, wipe
keep_outline = false
```
