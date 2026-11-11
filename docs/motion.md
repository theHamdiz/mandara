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
preserved on upgrade. From the command line, `mandara msg motion presets`
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
The dock's opening-app animation and attention bounce are under **Dock**;
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
