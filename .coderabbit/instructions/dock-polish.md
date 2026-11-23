---
id: apple-grade-dock-polish
name: Apple-Grade Dock Polish, Snappy Motion & Memory Discipline
paths:
  - "**/dock/**"
  - "**/studio/**"
  - "**/config/**"
  - "**/msg/**"
---

Audit all changes with the bar of a Principal Apple macOS UI Engineer and Rust Systems Architect:

1. Reject slow/floaty/linear easing in dock animations; verify damped harmonic spring constants (`k`, `c`) settle crisply (~90–130ms), clamp `dt` (`<= 1/30s`), preserve momentum on cursor reversal, and halt `window.request_animation_frame()` at rest (`0 Hz` idle).
2. Flag any heap allocations (`String`, `Vec`, `format!`, `Box`, `HashMap`) inside GPUI `render()` or animation ticks; verify offscreen squircle icon buffers (`128x128` cap) are disk-cached and dropped/`shrink_to_fit()` immediately after GPU atlas upload.
3. Verify irregular `.desktop` icons are normalized into continuous-curvature squircles with uniform inset padding, matched freestanding weight (no per-icon plate/halo), `1px` top specular rim highlights, physical-pixel border snapping (`1.0 / scale_factor()`), and light/dark wallpaper indicator contrast without soft glow halos over icons.
4. Verify `wl_surface.set_input_region` dynamically shrinks to resting pill bounds when unhovered so transparent magnification headroom never blocks window clicks, and ensure compositor IPC never blocks the GPUI render thread.
5. Verify every dock setting is exposed in Mandara Studio with live preview, debounced `config.toml` saves, backward-compatible `#[serde(default)]` defaults, and matching `mandara msg` / `mandara commands --json` commands.
