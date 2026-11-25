//! Window open / close / move / workspace animations written to the
//! compositor drop-in (`[compositor.look.windows]`). Only compositors whose
//! own checker accepted these lines get them: MangoWM (`mango -p`), Niri
//! (`niri validate`) and Hyprland 0.55+ in its Lua format (`Hyprland
//! --verify-config`; the older hyprlang format could not be checked, so it
//! gets none). Every other compositor gets nothing and Studio says why.

use crate::compositor::Target;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// A window animation style. `Compositor` (the default) writes nothing, so
/// the compositor keeps its own animations.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowStyle {
    #[default]
    Compositor,
    /// A short, plain fade.
    Fade,
    /// Rises from below like a swell (Corniche tide).
    Swell,
    /// A quick sideways sweep (Lighthouse sweep).
    Beam,
    /// A slow, eased glide (Felucca drift).
    Drift,
    /// A fast zoom that snaps past its size and back (Magnetic).
    Snap,
    /// A springy zoom that settles softly (Mercury).
    Liquid,
    /// Drops in from above and bounces (Raindrop).
    Drop,
}

impl WindowStyle {
    pub const ALL: [Self; 8] = [Self::Compositor, Self::Fade, Self::Swell, Self::Beam, Self::Drift, Self::Snap, Self::Liquid, Self::Drop];
}

/// Range of [`WindowMotion::speed`].
pub const SPEED_RANGE: (f32, f32) = (0.25, 3.0);

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowMotion {
    pub style: WindowStyle,
    /// Plays the style faster (above 1) or slower (below 1).
    pub speed: f32,
}

impl Default for WindowMotion {
    fn default() -> Self {
        Self { style: WindowStyle::Compositor, speed: 1.0 }
    }
}

impl WindowMotion {
    fn speed(&self) -> f32 {
        if self.speed.is_finite() { self.speed.clamp(SPEED_RANGE.0, SPEED_RANGE.1) } else { 1.0 }
    }
}

/// Whether Mandara can write window animations for `target` (each
/// compositor's checker accepted the lines below).
pub fn supported(target: Target) -> bool {
    matches!(target, Target::Mango | Target::Niri | Target::Hyprland)
}

/// How a window enters or leaves, in the vocabulary all three share.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Path {
    Fade,
    /// From an edge (Hyprland's `slide <edge>`; Mango picks its own edge).
    Slide(Edge),
    /// Grows from `ratio` of its size.
    Zoom(f32),
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Edge {
    Top,
    Bottom,
    Left,
}

impl Edge {
    fn hypr(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
        }
    }
}

/// A cubic Bézier `(x1, y1, x2, y2)`.
type Curve = (f32, f32, f32, f32);

/// One animated transition: its path, duration and curve, and the spring
/// Niri uses where a spring reads better than a curve.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Leg {
    path: Path,
    ms: u32,
    curve: Curve,
}

/// Niri's spring: damping ratio and stiffness.
type Spring = (f32, u32);

struct Flavour {
    open: Leg,
    close: Leg,
    /// Window movement and workspace switches.
    move_ms: u32,
    move_curve: Curve,
    workspace_ms: u32,
    workspace_curve: Curve,
    /// Niri springs for movement and workspace switches.
    spring: Spring,
    /// Niri's easing name for opening / closing (it has no slide or zoom
    /// of its own without a shader).
    niri_open: &'static str,
    /// Workspaces slide vertically.
    vertical_workspaces: bool,
}

const EASE_OUT: Curve = (0.22, 1.0, 0.36, 1.0);
const EASE_IN: Curve = (0.55, 0.0, 1.0, 0.45);
const OVERSHOOT: Curve = (0.34, 1.56, 0.64, 1.0);

fn flavour(style: WindowStyle) -> Option<Flavour> {
    let leg = |path, ms, curve| Leg { path, ms, curve };
    Some(match style {
        WindowStyle::Compositor => return None,
        WindowStyle::Fade => Flavour {
            open: leg(Path::Fade, 200, EASE_OUT),
            close: leg(Path::Fade, 160, EASE_IN),
            move_ms: 260,
            move_curve: EASE_OUT,
            workspace_ms: 240,
            workspace_curve: EASE_OUT,
            spring: (1.0, 800),
            niri_open: "ease-out-quad",
            vertical_workspaces: false,
        },
        WindowStyle::Swell => Flavour {
            open: leg(Path::Slide(Edge::Bottom), 460, (0.2, 1.0, 0.3, 1.03)),
            close: leg(Path::Slide(Edge::Bottom), 320, EASE_IN),
            move_ms: 420,
            move_curve: EASE_OUT,
            workspace_ms: 480,
            workspace_curve: (0.2, 1.0, 0.3, 1.03),
            spring: (0.78, 420),
            niri_open: "ease-out-cubic",
            vertical_workspaces: true,
        },
        WindowStyle::Beam => Flavour {
            open: leg(Path::Slide(Edge::Left), 240, EASE_OUT),
            close: leg(Path::Slide(Edge::Left), 180, EASE_IN),
            move_ms: 220,
            move_curve: EASE_OUT,
            workspace_ms: 260,
            workspace_curve: EASE_OUT,
            spring: (0.85, 1000),
            niri_open: "ease-out-expo",
            vertical_workspaces: false,
        },
        WindowStyle::Drift => Flavour {
            open: leg(Path::Fade, 620, (0.33, 1.0, 0.68, 1.0)),
            close: leg(Path::Fade, 420, (0.32, 0.0, 0.67, 0.0)),
            move_ms: 600,
            move_curve: (0.33, 1.0, 0.68, 1.0),
            workspace_ms: 640,
            workspace_curve: (0.33, 1.0, 0.68, 1.0),
            spring: (0.8, 260),
            niri_open: "ease-out-cubic",
            vertical_workspaces: false,
        },
        WindowStyle::Snap => Flavour {
            open: leg(Path::Zoom(0.7), 260, OVERSHOOT),
            close: leg(Path::Zoom(0.85), 160, EASE_IN),
            move_ms: 240,
            move_curve: OVERSHOOT,
            workspace_ms: 280,
            workspace_curve: EASE_OUT,
            spring: (0.55, 900),
            niri_open: "ease-out-expo",
            vertical_workspaces: false,
        },
        WindowStyle::Liquid => Flavour {
            open: leg(Path::Zoom(0.5), 420, (0.5, 1.4, 0.4, 1.0)),
            close: leg(Path::Zoom(0.8), 260, EASE_IN),
            move_ms: 380,
            move_curve: (0.5, 1.25, 0.4, 1.0),
            workspace_ms: 400,
            workspace_curve: (0.5, 1.25, 0.4, 1.0),
            spring: (0.55, 500),
            niri_open: "ease-out-cubic",
            vertical_workspaces: false,
        },
        WindowStyle::Drop => Flavour {
            open: leg(Path::Slide(Edge::Top), 440, (0.34, 1.8, 0.64, 1.0)),
            close: leg(Path::Slide(Edge::Top), 260, EASE_IN),
            move_ms: 360,
            move_curve: OVERSHOOT,
            workspace_ms: 420,
            workspace_curve: OVERSHOOT,
            spring: (0.42, 600),
            niri_open: "ease-out-expo",
            vertical_workspaces: true,
        },
    })
}

fn scaled(ms: u32, speed: f32) -> u32 {
    ((ms as f32 / speed).round() as u32).max(1)
}

fn curve((a, b, c, d): Curve) -> String {
    format!("{a},{b},{c},{d}")
}

/// MangoWM lines (`mango -p` accepted): animation types, durations and
/// curves. Mango slides from the edge it picks itself.
pub fn mango(m: &WindowMotion) -> String {
    let Some(f) = flavour(m.style) else { return String::new() };
    let k = m.speed();
    let kind = |p: Path| match p {
        Path::Fade => "fade",
        Path::Slide(_) => "slide",
        Path::Zoom(_) => "zoom",
    };
    let mut s = String::from("# Window animations (Studio → Motion)\n");
    let _ = writeln!(s, "animation_type_open={}\nanimation_type_close={}", kind(f.open.path), kind(f.close.path));
    let _ = writeln!(s, "animation_fade_in=1\nanimation_fade_out=1");
    if let Path::Zoom(r) = f.open.path {
        let _ = writeln!(s, "zoom_initial_ratio={r}");
    }
    if let Path::Zoom(r) = f.close.path {
        let _ = writeln!(s, "zoom_end_ratio={r}");
    }
    let _ = writeln!(s, "tag_animation_direction={}", if f.vertical_workspaces { 0 } else { 1 });
    let _ = writeln!(s, "animation_duration_open={}\nanimation_duration_close={}", scaled(f.open.ms, k), scaled(f.close.ms, k));
    let _ = writeln!(s, "animation_duration_move={}\nanimation_duration_tag={}", scaled(f.move_ms, k), scaled(f.workspace_ms, k));
    let _ = writeln!(s, "animation_curve_open={}\nanimation_curve_close={}", curve(f.open.curve), curve(f.close.curve));
    let _ = writeln!(s, "animation_curve_move={}\nanimation_curve_tag={}", curve(f.move_curve), curve(f.workspace_curve));
    s
}

/// Hyprland's leaves for one flavour: `(leaf, ms, curve name, style)`.
fn hypr_leaves(f: &Flavour) -> [(&'static str, u32, &'static str, Option<String>); 6] {
    let style = |p: Path| match p {
        Path::Fade => None,
        Path::Slide(e) => Some(format!("slide {}", e.hypr())),
        Path::Zoom(r) => Some(format!("popin {}%", (r * 100.0).round() as u32)),
    };
    let ws = if f.vertical_workspaces { "slidevert" } else { "slide" };
    [
        ("windowsIn", f.open.ms, "mandaraOpen", style(f.open.path)),
        ("windowsOut", f.close.ms, "mandaraClose", style(f.close.path)),
        ("windowsMove", f.move_ms, "mandaraMove", None),
        ("fadeIn", f.open.ms, "mandaraOpen", None),
        ("fadeOut", f.close.ms, "mandaraClose", None),
        ("workspaces", f.workspace_ms, "mandaraWorkspace", Some(ws.to_string())),
    ]
}

/// Hyprland's speed unit: tenths of a second.
fn hypr_speed(ms: u32) -> String {
    format!("{:.2}", (ms as f32 / 100.0).max(0.01))
}

fn hypr_curves(f: &Flavour) -> [(&'static str, Curve); 4] {
    [("mandaraOpen", f.open.curve), ("mandaraClose", f.close.curve), ("mandaraMove", f.move_curve), ("mandaraWorkspace", f.workspace_curve)]
}

/// Hyprland Lua lines (`hl.curve` / `hl.animation`, as in Hyprland's own
/// example config; `Hyprland --verify-config` accepted), each in `pcall`.
pub fn hyprland_lua(m: &WindowMotion) -> String {
    let Some(f) = flavour(m.style) else { return String::new() };
    let k = m.speed();
    let mut s = String::from("\n-- Window animations (Studio → Motion)\n");
    for (name, (a, b, c, d)) in hypr_curves(&f) {
        let _ = writeln!(s, "pcall(hl.curve, \"{name}\", {{ type = \"bezier\", points = {{ {{{a}, {b}}}, {{{c}, {d}}} }} }})");
    }
    for (leaf, ms, curve, style) in hypr_leaves(&f) {
        let style = style.map(|st| format!(", style = \"{st}\"")).unwrap_or_default();
        let _ = writeln!(s, "pcall(hl.animation, {{ leaf = \"{leaf}\", enabled = true, speed = {}, bezier = \"{curve}\"{style} }})", hypr_speed(scaled(ms, k)));
    }
    s
}

/// Niri lines (`niri validate` accepted): eased opening and closing,
/// springs for movement and workspace switches.
pub fn niri(m: &WindowMotion) -> String {
    let Some(f) = flavour(m.style) else { return String::new() };
    let k = m.speed();
    // A faster spring at the same damping plays `k` times quicker.
    let stiffness = ((f.spring.1 as f32) * k * k).round().clamp(1.0, 100_000.0) as u32;
    let spring = format!("spring damping-ratio={:.2} stiffness={stiffness} epsilon=0.0001", f.spring.0);
    let mut s = String::from("\n// Window animations (Studio → Motion)\nanimations {\n");
    let _ = writeln!(s, "    window-open {{\n        duration-ms {}\n        curve \"{}\"\n    }}", scaled(f.open.ms, k), f.niri_open);
    let _ = writeln!(s, "    window-close {{\n        duration-ms {}\n        curve \"ease-out-quad\"\n    }}", scaled(f.close.ms, k));
    let _ = writeln!(s, "    window-movement {{\n        {spring}\n    }}");
    let _ = writeln!(s, "    workspace-switch {{\n        {spring}\n    }}");
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositor_style_writes_nothing_and_old_files_load() {
        let m = WindowMotion::default();
        for out in [mango(&m), hyprland_lua(&m), niri(&m)] {
            assert!(out.is_empty());
        }
        let look: crate::compositor::CompositorLook = toml::from_str("rounding = 8\n").unwrap();
        assert_eq!(look.windows, WindowMotion::default());
        let look: crate::compositor::CompositorLook = toml::from_str("[windows]\nstyle = \"drop\"\n").unwrap();
        assert_eq!(look.windows, WindowMotion { style: WindowStyle::Drop, speed: 1.0 });
    }

    #[test]
    fn every_style_writes_every_compositor_and_speed_scales_it() {
        for style in &WindowStyle::ALL[1..] {
            let m = WindowMotion { style: *style, speed: 1.0 };
            let fast = WindowMotion { style: *style, speed: 2.0 };
            assert!(mango(&m).contains("animation_duration_open="), "{style:?}");
            assert!(hyprland_lua(&m).contains("leaf = \"windowsIn\""), "{style:?}");
            assert!(niri(&m).contains("window-open"), "{style:?}");
            assert_ne!(mango(&m), mango(&fast));
        }
        // Out-of-range speeds are bounded.
        let wild = WindowMotion { style: WindowStyle::Fade, speed: f32::NAN };
        assert_eq!(mango(&wild), mango(&WindowMotion { style: WindowStyle::Fade, speed: 1.0 }));
        assert!(supported(Target::Mango) && supported(Target::Niri) && supported(Target::Hyprland));
        assert!(!supported(Target::Sway) && !supported(Target::River) && !supported(Target::Labwc) && !supported(Target::Cosmic) && !supported(Target::KWin));
    }
}
