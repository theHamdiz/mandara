//! The main menu's tile ornaments: every action tile wears its own motif,
//! drawn faintly behind its icon and label in the tile's own accent.
//!
//! Two families, one system:
//!
//! * **Kufic words** for the apps and tools: a large Fatimid Kufic word in
//!   the spirit of what the tile does (تطبيقات for the launcher, المندرة
//!   for Studio…), cropped by the tile's edge. The outlines are prepared
//!   at build time (`cargo xtask art --calligraphy`, Reem Kufi, OFL 1.1) so
//!   the shell needs no extra font.
//! * **Geometric ornament** for the look and session controls: Alexandrian
//!   and Mediterranean motifs (a shamsa sunburst, an eight-point star
//!   tiling, a mashrabiya screen, the Pharos and its beams, Corniche
//!   waves…), each hinting at what the tile does.
//!
//! Every ornament is a monochrome SVG mask served as `art/<name>.svg`:
//! GPUI rasterises it once per size and scale into its sprite atlas and
//! tints it like an icon, so a theme change, a hover or the hue costs no
//! new raster, and nothing runs while the menu is idle. Faith plugins never
//! get religious calligraphy here: the words are plain tool names.

use std::f32::consts::{FRAC_PI_4, FRAC_PI_8, PI, TAU};
use std::fmt::Write as _;
use std::sync::OnceLock;

use mandara_theme::{Color, MandaraPalette};

/// (key, word, width, height, path) in font units, generated.
type Word = (&'static str, &'static str, f32, f32, &'static str);
const WORDS: &[Word] = include!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/calligraphy/words.rs"));

/// The geometric motifs, one per look / session action.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Motif {
    /// A shamsa (sunburst medallion), half of its rays dimmed: light / dark.
    Shamsa,
    /// An eight-point star tiling (zellige) fading in: the next theme.
    Zellige,
    /// A mashrabiya screen of turned beads: privacy, the lock.
    Mashrabiya,
    /// A round-arched doorway: log out.
    Doorway,
    /// Calm Corniche waves under a few stars: sleep.
    Waves,
    /// Overlapping circles round a ring: restart.
    Rings,
    /// The sun setting on the sea: shut down.
    Sunset,
    /// The Pharos of Alexandria sweeping its beams: reload.
    Pharos,
    /// A grid of rounded squares, one picked out: the launcher.
    Grid,
    /// Stacked chevrons rising like hills: wallpapers.
    Chevrons,
    /// Tall panels side by side, the last one open: the sidebar.
    Columns,
    /// A honeycomb of hexagons: Mandara Studio.
    Honeycomb,
    /// Nested frames with crop corners: screenshot.
    Frames,
    /// Sheets fanned one over another: clipboard history.
    Sheets,
    /// A diagonal lattice with a few cells filled: edit the desktop.
    Lattice,
}

impl Motif {
    const ALL: [Motif; 15] = [
        Motif::Shamsa,
        Motif::Zellige,
        Motif::Mashrabiya,
        Motif::Doorway,
        Motif::Waves,
        Motif::Rings,
        Motif::Sunset,
        Motif::Pharos,
        Motif::Grid,
        Motif::Chevrons,
        Motif::Columns,
        Motif::Honeycomb,
        Motif::Frames,
        Motif::Sheets,
        Motif::Lattice,
    ];

    fn name(self) -> &'static str {
        match self {
            Motif::Shamsa => "shamsa",
            Motif::Zellige => "zellige",
            Motif::Mashrabiya => "mashrabiya",
            Motif::Doorway => "doorway",
            Motif::Waves => "waves",
            Motif::Rings => "rings",
            Motif::Sunset => "sunset",
            Motif::Pharos => "pharos",
            Motif::Grid => "grid",
            Motif::Chevrons => "chevrons",
            Motif::Columns => "columns",
            Motif::Honeycomb => "honeycomb",
            Motif::Frames => "frames",
            Motif::Sheets => "sheets",
            Motif::Lattice => "lattice",
        }
    }
}

/// What a tile wears.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Art {
    /// A Kufic word, by its key in the generated table.
    Word(&'static str),
    Motif(Motif),
}

/// A tile's ornament and its accent: the theme's primary turned by `hue`
/// degrees in OKLCH (same lightness and chroma, so every tile weighs the
/// same and the set stays in the theme's family).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ornament {
    pub art: Art,
    pub hue: f32,
    /// The geometric motif a word tile wears while calligraphy is off.
    pub geometric: Motif,
}

impl Ornament {
    pub const fn word(key: &'static str, geometric: Motif, hue: f32) -> Self {
        Self { art: Art::Word(key), hue, geometric }
    }

    pub const fn motif(motif: Motif, hue: f32) -> Self {
        Self { art: Art::Motif(motif), hue, geometric: motif }
    }

    /// What the tile draws: its word only with `calligraphy` on
    /// (`panels.menu_calligraphy`), otherwise its geometric motif.
    pub fn shown(self, calligraphy: bool) -> Self {
        if calligraphy || !self.is_word() { self } else { Self { art: Art::Motif(self.geometric), ..self } }
    }

    /// The `gpui::svg()` asset path.
    pub fn path(self) -> String {
        match self.art {
            Art::Word(key) => format!("art/{WORD_PREFIX}{key}.svg"),
            Art::Motif(m) => format!("art/{MOTIF_PREFIX}{}.svg", m.name()),
        }
    }

    /// A word's outline size in font units (drawn at one scale for every
    /// word, so their strokes match); `None` for a motif.
    pub fn units(self) -> Option<(f32, f32)> {
        match self.art {
            Art::Word(key) => word(key).map(|w| (w.2, w.3)),
            Art::Motif(_) => None,
        }
    }

    pub fn is_word(self) -> bool {
        matches!(self.art, Art::Word(_))
    }
}

const WORD_PREFIX: &str = "tile-word-";
const MOTIF_PREFIX: &str = "tile-motif-";

fn word(key: &str) -> Option<&'static Word> {
    WORDS.iter().find(|w| w.0 == key)
}

// ── Colour ───────────────────────────────────────────────────────────────

/// WCAG AA for body text: the label and hint keep at least this much
/// contrast over the busiest part of the ornament.
pub const TEXT_CONTRAST: f32 = 4.5;
/// WCAG's minimum for graphics: the tile's icon in its accent.
const ICON_CONTRAST: f32 = 3.0;
/// When a theme's own text sits below AA on the bare tile, the ornament
/// may take at most this share of that contrast.
const BELOW_AA_KEEP: f32 = 0.95;
/// OKLCH lightness step while pulling an accent icon to readable contrast.
const ICON_LIGHTNESS_STEP: f32 = 0.02;
/// Steps tried before settling for the theme's primary.
const ICON_LIGHTNESS_TRIES: usize = 20;
/// Bisection steps for the ink's alpha (1/2¹⁰ of the wanted alpha).
const ALPHA_SEARCH_STEPS: usize = 10;

/// The tile's accent: primary with its hue turned, kept displayable. An
/// achromatic (monochrome) primary stays as it is.
pub fn accent(palette: &MandaraPalette, hue: f32) -> Color {
    let p = palette.primary;
    if p.is_achromatic() {
        return p;
    }
    Color::oklch(p.lightness(), p.chroma(), p.hue() + hue).gamut_mapped()
}

/// The accent as an icon colour on `under`: lightness nudged away from the
/// surface until it reaches graphics contrast (or primary, if it can't).
pub fn icon_ink(accent: Color, primary: Color, under: Color) -> Color {
    if accent.contrast_ratio(under) >= ICON_CONTRAST {
        return accent;
    }
    let step = if under.lightness() < accent.lightness() || under.lightness() < 0.5 { ICON_LIGHTNESS_STEP } else { -ICON_LIGHTNESS_STEP };
    let mut c = accent;
    for _ in 0..ICON_LIGHTNESS_TRIES {
        c = c.lighten(step).gamut_mapped();
        if c.contrast_ratio(under) >= ICON_CONTRAST {
            return c;
        }
    }
    primary
}

/// An ornament is incised into its tile like an engraving in glass: a
/// `shade` body deeper than the surface and a `lip`, lighter than it,
/// peeking out a pixel toward the light (the specular rim's light comes
/// from the top-left, so a groove's lit wall faces bottom-right).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Inks {
    pub shade: Color,
    pub lip: Color,
    /// The text is lighter than the tile (a dark theme): the shade only
    /// adds to its contrast and needs more ink to read.
    pub carved: bool,
}

/// The inks of an ornament in `accent` on `under` behind `text`. On a dark
/// tile the shade is the accent's hue taken deeper than the surface and
/// the lip is the accent itself; on a light tile the shade is the accent
/// and the lip a near-white tint of it. Whichever ink moves toward the
/// text's lightness is kept faint by [`ink_alpha`].
pub fn inks(accent: Color, under: Color, text: Color) -> Inks {
    if text.lightness() > under.lightness() {
        Inks { shade: accent.with_lightness(under.lightness() * CARVE_DEPTH).gamut_mapped(), lip: accent, carved: true }
    } else {
        Inks { shade: accent, lip: accent.with_lightness(LIP_LIGHTNESS).gamut_mapped(), carved: false }
    }
}

/// A carved shade's OKLCH lightness as a share of the surface's.
const CARVE_DEPTH: f32 = 0.5;
/// A light tile's lip: the accent's hue, all but white.
const LIP_LIGHTNESS: f32 = 0.985;

/// sRGB channels in `0..=1` (one gamut mapping; the searches below work
/// on these instead of converting a colour at every step).
type Rgb = [f32; 3];

fn rgb(c: Color) -> Rgb {
    let s = c.to_srgba();
    [s.red, s.green, s.blue]
}

/// WCAG relative luminance of sRGB channels.
fn luminance(c: Rgb) -> f32 {
    let lin = |v: f32| if v <= SRGB_LINEAR_KNEE { v / SRGB_LINEAR_SLOPE } else { ((v + SRGB_OFFSET) / (1.0 + SRGB_OFFSET)).powf(SRGB_GAMMA) };
    LUMA[0] * lin(c[0]) + LUMA[1] * lin(c[1]) + LUMA[2] * lin(c[2])
}

/// WCAG contrast of two relative luminances.
fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + WCAG_FLARE) / (a.min(b) + WCAG_FLARE)
}

/// The sRGB transfer function and WCAG's luma weights and flare term.
const SRGB_LINEAR_KNEE: f32 = 0.04045;
const SRGB_LINEAR_SLOPE: f32 = 12.92;
const SRGB_OFFSET: f32 = 0.055;
const SRGB_GAMMA: f32 = 2.4;
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
const WCAG_FLARE: f32 = 0.05;

/// `ink` at `alpha` over the opaque `under`, blended the way the GPU
/// blends the mask (straight sRGB).
fn mix(ink: Rgb, alpha: f32, under: Rgb) -> Rgb {
    [0, 1, 2].map(|k| ink[k] * alpha + under[k] * (1.0 - alpha))
}

/// The strongest alpha up to `want` at which `ink` over `under` keeps
/// every colour in `texts` at [`TEXT_CONTRAST`] (or, for a text already
/// below it on the bare tile, most of what it had).
pub fn ink_alpha(ink: Color, under: Color, texts: &[Color], want: f32) -> f32 {
    let (ink, under) = (rgb(ink), rgb(under));
    let base = luminance(under);
    let texts: Vec<(f32, f32)> = texts
        .iter()
        .map(|t| {
            let l = luminance(rgb(*t));
            let had = ratio(l, base);
            (l, if had >= TEXT_CONTRAST { TEXT_CONTRAST } else { had * BELOW_AA_KEEP })
        })
        .collect();
    let ok = |a: f32| {
        let mixed = luminance(mix(ink, a, under));
        texts.iter().all(|(l, floor)| ratio(*l, mixed) >= *floor)
    };
    if ok(want) {
        return want;
    }
    let (mut lo, mut hi) = (0.0, want);
    for _ in 0..ALPHA_SEARCH_STEPS {
        let mid = (lo + hi) / 2.0;
        if ok(mid) { lo = mid } else { hi = mid }
    }
    lo
}

// ── Drawing ──────────────────────────────────────────────────────────────

/// Every motif's canvas (square), in units of about a pixel at the size
/// the menu draws them.
const VIEW: f32 = 120.0;
/// One stroke weight for every motif.
const STROKE: f32 = 1.4;
/// Coverage of the dimmed parts (the night half of the shamsa, the inner
/// arches, the reflections).
const DIM: f32 = 0.45;

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {VIEW} {VIEW}" fill="none" stroke="black" stroke-width="{STROKE}" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#)
}

/// A fade from full coverage at `(x1, y1)` to none at `(x2, y2)`
/// (fractions of the canvas), for motifs that dissolve into the tile.
fn fade(x1: f32, y1: f32, x2: f32, y2: f32) -> String {
    // User-space units: the fade spans the whole canvas, and straight lattice
    // lines (zero-height boxes) still get a stroke.
    let [x1, y1, x2, y2] = [x1, y1, x2, y2].map(|v| v * VIEW);
    format!(r#"<defs><linearGradient id="f" gradientUnits="userSpaceOnUse" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"><stop offset="0" stop-color="black"/><stop offset="1" stop-color="black" stop-opacity="0"/></linearGradient></defs>"#)
}

fn poly(pts: &[(f32, f32)]) -> String {
    let mut d = String::from("M");
    for (i, p) in pts.iter().enumerate() {
        let _ = write!(d, "{}{:.2} {:.2}", if i == 0 { "" } else { "L" }, p.0, p.1);
    }
    d.push('Z');
    d
}

/// An eight-point star (two squares 45° apart) at `c`, outer radius `r`.
fn star8(c: (f32, f32), r: f32) -> String {
    let inner = r * FRAC_PI_4.cos() / FRAC_PI_8.cos();
    let pts: Vec<(f32, f32)> = (0..16)
        .map(|i| {
            let a = -TAU / 4.0 + i as f32 * TAU / 16.0;
            let rad = if i % 2 == 0 { r } else { inner };
            (c.0 + rad * a.cos(), c.1 + rad * a.sin())
        })
        .collect();
    poly(&pts)
}

fn shamsa() -> String {
    let c = (VIEW / 2.0, VIEW / 2.0);
    let mut lit = String::new();
    let mut dim = String::new();
    // Two rings, then sixteen rays, long and short in turn.
    let _ = write!(lit, r#"<circle cx="{}" cy="{}" r="14"/><circle cx="{}" cy="{}" r="22"/>"#, c.0, c.1, c.0, c.1);
    lit.push_str(&format!(r#"<path d="{}"/>"#, star8(c, 30.0)));
    for i in 0..16 {
        let a = i as f32 * TAU / 16.0 - TAU / 4.0;
        let (r0, r1) = if i % 2 == 0 { (36.0, 56.0) } else { (36.0, 46.0) };
        let seg = format!(r#"<path d="M{:.2} {:.2}L{:.2} {:.2}"/>"#, c.0 + r0 * a.cos(), c.1 + r0 * a.sin(), c.0 + r1 * a.cos(), c.1 + r1 * a.sin());
        // The right half is day, the left half night.
        if a.cos() > -1e-3 { lit.push_str(&seg) } else { dim.push_str(&seg) }
    }
    svg(&format!(r#"{lit}<g stroke-opacity="{DIM}">{dim}</g>"#))
}

fn zellige() -> String {
    // Stars on a square grid, a turned square between each four.
    let step = 30.0;
    let mut body = String::new();
    let n = (VIEW / step) as i32;
    for row in 0..=n {
        for col in 0..=n {
            let c = (col as f32 * step, row as f32 * step);
            let _ = write!(body, r#"<path d="{}"/>"#, star8(c, 11.0));
            let m = (c.0 + step / 2.0, c.1 + step / 2.0);
            let s = 5.0;
            let _ = write!(body, r#"<path d="{}"/>"#, poly(&[(m.0, m.1 - s), (m.0 + s, m.1), (m.0, m.1 + s), (m.0 - s, m.1)]));
        }
    }
    svg(&format!(r#"{}<g stroke="url(#f)">{body}</g>"#, fade(1.0, 1.0, 0.1, 0.1)))
}

fn mashrabiya() -> String {
    // Turned beads on a lattice, a lozenge in every other cell.
    let step = 15.0;
    let mut body = String::new();
    let n = (VIEW / step) as i32;
    for row in 0..=n {
        for col in 0..=n {
            let (x, y) = (col as f32 * step, row as f32 * step);
            let _ = write!(body, r#"<circle cx="{x}" cy="{y}" r="3"/>"#);
            if col < n {
                let _ = write!(body, r#"<path d="M{} {y}H{}"/>"#, x + 3.0, x + step - 3.0);
            }
            if row < n {
                let _ = write!(body, r#"<path d="M{x} {}V{}"/>"#, y + 3.0, y + step - 3.0);
            }
            if row < n && col < n && (row + col) % 2 == 0 {
                let (cx, cy, s) = (x + step / 2.0, y + step / 2.0, 3.2);
                let _ = write!(body, r#"<path d="{}"/>"#, poly(&[(cx, cy - s), (cx + s, cy), (cx, cy + s), (cx - s, cy)]));
            }
        }
    }
    svg(&format!(r#"{}<g stroke="url(#f)">{body}</g>"#, fade(1.0, 1.0, 0.15, 0.2)))
}

/// A round arch over `(x, w)` springing at `spring`, feet at `foot`.
fn arch(x: f32, w: f32, spring: f32, foot: f32) -> String {
    let r = w / 2.0;
    format!("M{x} {foot}V{spring}A{r} {r} 0 0 1 {} {spring}V{foot}", x + w)
}

fn doorway() -> String {
    let foot = 112.0;
    let mut body = String::new();
    let _ = write!(body, r#"<path d="{}"/>"#, arch(24.0, 72.0, 52.0, foot));
    let _ = write!(body, r#"<g stroke-opacity="{DIM}"><path d="{}"/><path d="{}"/></g>"#, arch(32.0, 56.0, 56.0, foot), arch(40.0, 40.0, 60.0, foot));
    // Keystone, the open door's leaf, the threshold steps.
    let _ = write!(body, r#"<path d="M56 12L60 6L64 12"/><path d="M60 40V{foot}"/><path d="M12 {foot}H108M20 {}H100"/>"#, foot + 5.0);
    svg(&body)
}

fn waves() -> String {
    let mut body = String::new();
    for (i, y) in [62.0_f32, 74.0, 86.0, 98.0, 110.0].iter().enumerate() {
        let amp = 3.0 + i as f32 * 0.6;
        let len = 24.0;
        let shift = if i % 2 == 0 { 0.0 } else { len / 2.0 };
        let mut d = format!("M{} {y}", -shift);
        let mut x = -shift;
        while x < VIEW {
            let _ = write!(d, "q{} {} {} 0t{} 0", len / 4.0, -amp, len / 2.0, len / 2.0);
            x += len;
        }
        let _ = write!(body, r#"<path d="{d}"/>"#);
    }
    for (c, r) in [((30.0, 22.0), 5.0), ((76.0, 14.0), 3.5), ((100.0, 36.0), 4.5), ((52.0, 42.0), 2.5)] {
        let _ = write!(body, r#"<circle cx="{}" cy="{}" r="{r}" stroke-opacity="{DIM}"/>"#, c.0, c.1);
    }
    svg(&body)
}

fn rings() -> String {
    let c = (VIEW / 2.0, VIEW / 2.0);
    let r = 22.0;
    let mut body = format!(r#"<circle cx="{}" cy="{}" r="{}"/><circle cx="{}" cy="{}" r="{}"/>"#, c.0, c.1, r, c.0, c.1, r * 2.0 + 4.0);
    for i in 0..6 {
        let a = i as f32 * TAU / 6.0;
        let _ = write!(body, r#"<circle cx="{:.2}" cy="{:.2}" r="{r}" stroke-opacity="{}"/>"#, c.0 + r * a.cos(), c.1 + r * a.sin(), if i % 2 == 0 { 1.0 } else { DIM });
    }
    svg(&body)
}

fn sunset() -> String {
    let horizon = 70.0;
    let c = (VIEW / 2.0, horizon);
    let mut body = String::new();
    let _ = write!(body, r#"<path d="M{} {horizon}A26 26 0 0 1 {} {horizon}"/><path d="M{} {horizon}A16 16 0 0 1 {} {horizon}" stroke-opacity="{DIM}"/>"#, c.0 - 26.0, c.0 + 26.0, c.0 - 16.0, c.0 + 16.0);
    for i in 1..12 {
        let a = PI + i as f32 * PI / 12.0;
        let (r0, r1) = if i % 2 == 0 { (32.0, 50.0) } else { (32.0, 42.0) };
        let _ = write!(body, r#"<path d="M{:.2} {:.2}L{:.2} {:.2}"/>"#, c.0 + r0 * a.cos(), c.1 + r0 * a.sin(), c.0 + r1 * a.cos(), c.1 + r1 * a.sin());
    }
    let _ = write!(body, r#"<path d="M4 {horizon}H116"/>"#);
    // The sun's road on the water, shorter as it nears.
    let mut road = String::new();
    for (i, half) in [24.0_f32, 18.0, 13.0, 9.0, 5.0].iter().enumerate() {
        let y = horizon + 9.0 + i as f32 * 9.0;
        let _ = write!(road, "M{} {y}H{}", c.0 - half, c.0 + half);
    }
    let _ = write!(body, r#"<path d="{road}" stroke-opacity="{DIM}"/>"#);
    svg(&body)
}

fn pharos() -> String {
    // Three tiers (square, octagonal, round) on a base, the lantern on top.
    let x = 60.0;
    let mut body = String::new();
    let _ = write!(body, r#"<path d="M{} 116L{} 72H{}L{} 116Z"/>"#, x - 18.0, x - 13.0, x + 13.0, x + 18.0);
    let _ = write!(body, r#"<path d="M{} 72L{} 48H{}L{} 72"/>"#, x - 11.0, x - 8.0, x + 8.0, x + 11.0);
    let _ = write!(body, r#"<path d="M{} 48V34H{}V48"/>"#, x - 6.0, x + 6.0);
    let _ = write!(body, r#"<circle cx="{x}" cy="28" r="5"/><path d="M{} 116H{}"/>"#, x - 30.0, x + 30.0);
    // Windows.
    let _ = write!(body, r#"<path d="M{x} 84V92M{x} 100V108M{x} 56V62" stroke-opacity="{DIM}"/>"#);
    // Two beams sweeping out, and the arcs they paint.
    let mut beams = String::new();
    for side in [-1.0_f32, 1.0] {
        let _ = write!(beams, "M{} 26L{} 10M{} 30L{} 40", x + side * 7.0, x + side * 58.0, x + side * 7.0, x + side * 58.0);
    }
    let _ = write!(body, r#"<path d="{beams}" stroke-opacity="{DIM}"/>"#);
    let _ = write!(body, r#"<path d="M{} 12A44 44 0 0 1 {} 12" stroke-opacity="{DIM}"/>"#, x - 40.0, x + 40.0);
    svg(&body)
}

/// A rounded rectangle's outline.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> String {
    format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}"/>"#)
}

fn grid() -> String {
    // Four by four rounded squares, the one under the cursor picked out.
    let (step, size) = (26.0, 18.0);
    let mut lit = String::new();
    let mut dim = String::new();
    for row in 0..4 {
        for col in 0..4 {
            let (x, y) = (12.0 + col as f32 * step, 12.0 + row as f32 * step);
            let cell = rounded(x, y, size, size, 4.0);
            if (row, col) == (1, 2) || (row + col) % 3 == 0 { lit.push_str(&cell) } else { dim.push_str(&cell) }
        }
    }
    svg(&format!(r#"{lit}<g stroke-opacity="{DIM}">{dim}</g>"#))
}

fn chevrons() -> String {
    // Rows of chevrons, each row a little lower and fainter: far hills.
    let mut body = String::new();
    for (i, y) in [40.0_f32, 58.0, 76.0, 94.0, 112.0].iter().enumerate() {
        let (w, rise) = (24.0, 10.0 + i as f32 * 2.0);
        let shift = if i % 2 == 0 { 0.0 } else { w / 2.0 };
        let mut d = format!("M{} {y}", -shift);
        let mut x = -shift;
        while x < VIEW {
            let _ = write!(d, "L{} {}L{} {y}", x + w / 2.0, y - rise, x + w);
            x += w;
        }
        let o = 1.0 - i as f32 * 0.12;
        let _ = write!(body, r#"<path d="{d}" stroke-opacity="{o:.2}"/>"#);
    }
    svg(&body)
}

fn columns() -> String {
    // Tall panels; the last is wider, as if slid open.
    let mut body = String::new();
    for (i, (x, w)) in [(10.0_f32, 14.0_f32), (30.0, 14.0), (50.0, 14.0), (72.0, 38.0)].iter().enumerate() {
        let cell = rounded(*x, 14.0, *w, 92.0, 4.0);
        if i == 3 { body.push_str(&cell) } else { let _ = write!(body, r#"<g stroke-opacity="{DIM}">{cell}</g>"#); }
    }
    // Rows of content in the open panel.
    for y in [30.0_f32, 42.0, 54.0, 66.0] {
        let _ = write!(body, r#"<path d="M80 {y}H{}"/>"#, if y == 42.0 { 96.0 } else { 102.0 });
    }
    svg(&body)
}

fn honeycomb() -> String {
    // Flat-topped hexagons on an offset grid, fading in from a corner.
    let r = 12.0;
    let (dx, dy) = (r * 1.5, r * 3.0_f32.sqrt());
    let mut body = String::new();
    let n = (VIEW / dx) as i32 + 1;
    for col in 0..=n {
        let off = if col % 2 == 0 { 0.0 } else { dy / 2.0 };
        let mut y = off;
        while y <= VIEW + dy {
            let c = (col as f32 * dx, y);
            let pts: Vec<(f32, f32)> = (0..6).map(|i| { let a = i as f32 * TAU / 6.0; (c.0 + r * a.cos(), c.1 + r * a.sin()) }).collect();
            let _ = write!(body, r#"<path d="{}"/>"#, poly(&pts));
            y += dy;
        }
    }
    svg(&format!(r#"{}<g stroke="url(#f)">{body}</g>"#, fade(1.0, 1.0, 0.1, 0.15)))
}

fn frames() -> String {
    // Nested frames, then crop corners round the innermost.
    let mut body = String::new();
    for (i, inset) in [8.0_f32, 22.0, 36.0].iter().enumerate() {
        let side = VIEW - 2.0 * inset;
        let cell = rounded(*inset, *inset, side, side, 6.0);
        if i == 0 { let _ = write!(body, r#"<g stroke-opacity="{DIM}">{cell}</g>"#); } else { body.push_str(&cell) }
    }
    let (a, b, k) = (46.0, VIEW - 46.0, 10.0);
    let _ = write!(body, "<path d=\"M{a} {}V{a}H{}M{} {a}H{b}V{}M{b} {}V{b}H{}M{} {b}H{a}V{}\"/>", a + k, a + k, b - k, a + k, b - k, b - k, a + k, b - k);
    svg(&body)
}

fn sheets() -> String {
    // Three sheets fanned from the corner, lines on the top one.
    let mut body = String::new();
    for (i, (x, y)) in [(14.0_f32, 34.0_f32), (26.0, 22.0), (38.0, 10.0)].iter().enumerate() {
        let cell = rounded(*x, *y, 66.0, 80.0, 6.0);
        if i == 2 { body.push_str(&cell) } else { let _ = write!(body, r#"<g stroke-opacity="{DIM}">{cell}</g>"#); }
    }
    for (y, end) in [(28.0_f32, 92.0_f32), (40.0, 96.0), (52.0, 84.0), (64.0, 94.0)] {
        let _ = write!(body, r#"<path d="M48 {y}H{end}"/>"#);
    }
    svg(&body)
}

fn lattice() -> String {
    // A diagonal lattice; a few of its diamonds are inked in.
    let step = 20.0;
    let mut body = String::new();
    let mut k = -VIEW;
    while k <= VIEW * 2.0 {
        let _ = write!(body, r#"<path d="M{k} 0L{} {VIEW}"/><path d="M{k} {VIEW}L{} 0"/>"#, k + VIEW, k + VIEW);
        k += step;
    }
    let mut cells = String::new();
    for (cx, cy) in [(60.0_f32, 50.0_f32), (80.0, 70.0), (40.0, 90.0)] {
        let s = step / 2.0 - 3.0;
        let _ = write!(cells, r#"<path d="{}"/>"#, poly(&[(cx, cy - s), (cx + s, cy), (cx, cy + s), (cx - s, cy)]));
    }
    svg(&format!(r#"{}<g stroke="url(#f)">{body}</g><g fill="black" fill-opacity="{DIM}" stroke="none">{cells}</g>"#, fade(1.0, 1.0, 0.1, 0.1)))
}

fn word_svg(w: &Word) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {} {}"><path d="{}" fill="black"/></svg>"#, w.2, w.3, w.4)
}

fn motif_svg(m: Motif) -> String {
    match m {
        Motif::Shamsa => shamsa(),
        Motif::Zellige => zellige(),
        Motif::Mashrabiya => mashrabiya(),
        Motif::Doorway => doorway(),
        Motif::Waves => waves(),
        Motif::Rings => rings(),
        Motif::Sunset => sunset(),
        Motif::Pharos => pharos(),
        Motif::Grid => grid(),
        Motif::Chevrons => chevrons(),
        Motif::Columns => columns(),
        Motif::Honeycomb => honeycomb(),
        Motif::Frames => frames(),
        Motif::Sheets => sheets(),
        Motif::Lattice => lattice(),
    }
}

/// The SVG of `art/<name>.svg` for a tile ornament, built once.
pub fn asset(name: &str) -> Option<String> {
    static MOTIFS: OnceLock<Vec<String>> = OnceLock::new();
    if let Some(key) = name.strip_prefix(WORD_PREFIX) {
        return word(key).map(word_svg);
    }
    let m = name.strip_prefix(MOTIF_PREFIX)?;
    let i = Motif::ALL.iter().position(|x| x.name() == m)?;
    Some(MOTIFS.get_or_init(|| Motif::ALL.iter().map(|m| motif_svg(*m)).collect())[i].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blend(ink: Color, alpha: f32, under: Color) -> Rgb {
        mix(rgb(ink), alpha, rgb(under))
    }

    fn contrast_on(text: Color, under: Rgb) -> f32 {
        ratio(luminance(rgb(text)), luminance(under))
    }

    /// The accents the menu's tiles use, and a little beyond.
    const HUES: [f32; 9] = [-90.0, -72.0, -48.0, -24.0, 0.0, 24.0, 48.0, 72.0, 90.0];
    /// The strongest ink the menu asks for, and its wash.
    const MOST_INK: f32 = 0.34;
    const WASH: f32 = 0.08;
    /// Below this, an ornament would be too faint to see.
    const VISIBLE_INK: f32 = 0.04;

    fn ornaments() -> Vec<Ornament> {
        WORDS.iter().map(|w| Ornament::word(w.0, Motif::Grid, 0.0)).chain(Motif::ALL.iter().map(|m| Ornament::motif(*m, 0.0))).collect()
    }

    #[test]
    fn every_ornament_is_a_distinct_drawing() {
        let mut seen = std::collections::HashSet::new();
        for o in ornaments() {
            let name = o.path().strip_prefix("art/").and_then(|p| p.strip_suffix(".svg")).unwrap().to_string();
            let svg = asset(&name).unwrap_or_else(|| panic!("{name}: no asset"));
            let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(tree.root().has_children(), "{name} draws nothing");
            assert!(seen.insert(svg), "{name} repeats another ornament");
            assert!(o.units().is_none_or(|(w, h)| w > 0.0 && h > 0.0), "{name}");
        }
        assert!(asset("tile-word-nope").is_none() && asset("tile-motif-nope").is_none() && asset("jannah-crown").is_none());
    }

    /// Every action's word is in the generated table.
    #[test]
    fn the_words_cover_the_tiles() {
        for key in ["launcher", "wallpapers", "sidebar", "studio", "screenshot", "clipboard", "edit"] {
            assert!(word(key).is_some_and(|w| w.2 > 0.0 && w.3 > 0.0), "{key}");
        }
    }

    /// In every theme, dark and light, the label and hint keep AA contrast
    /// over the busiest spot of the ornament at its strongest ink, the icon
    /// keeps graphics contrast, and the ink stays visible nearly always.
    #[test]
    fn labels_stay_legible_over_the_ornaments() {
        let (mut faint, mut total) = (0, 0);
        for def in mandara_theme::catalog::all() {
            for dark in [true, false] {
                let p = def.palette(dark);
                for hue in HUES {
                    let tint = accent(&p, hue);
                    for under in [p.surface_container, p.surface_container_high] {
                        let washed = under.mix(tint, WASH);
                        let texts = [p.on_surface, p.on_surface_variant];
                        let inks = inks(tint, under, p.on_surface);
                        let mut seen = 0.0_f32;
                        for ink in [inks.shade, inks.lip] {
                            let a = ink_alpha(ink, washed, &texts, MOST_INK);
                            let c = blend(ink, a, washed);
                            for t in texts {
                                let base = contrast_on(t, rgb(washed));
                                let need = if base >= TEXT_CONTRAST { TEXT_CONTRAST } else { base * BELOW_AA_KEEP };
                                assert!(contrast_on(t, c) >= need, "{} dark={dark} hue={hue}: {} < {need}", def.id, contrast_on(t, c));
                            }
                            seen = seen.max(a);
                        }
                        total += 1;
                        faint += usize::from(seen < VISIBLE_INK);
                        let icon = icon_ink(tint, p.primary, under);
                        assert!(icon.contrast_ratio(under) >= ICON_CONTRAST || icon == p.primary, "{} dark={dark} hue={hue}: icon", def.id);
                    }
                }
            }
        }
        assert!(faint * 10 <= total, "{faint} of {total} ornaments too faint");
    }

    #[test]
    fn ink_alpha_backs_off_only_as_far_as_needed() {
        let under = Color::oklch(0.25, 0.02, 250.0);
        let text = Color::oklch(0.95, 0.01, 250.0);
        let ink = Color::oklch(0.9, 0.1, 200.0);
        let a = ink_alpha(ink, under, &[text], 1.0);
        assert!(a > 0.0 && a < 1.0);
        assert!(contrast_on(text, blend(ink, a, under)) >= TEXT_CONTRAST);
        assert!(contrast_on(text, blend(ink, (a + 0.05).min(1.0), under)) < TEXT_CONTRAST);
        // The fast luminance agrees with the theme's own.
        assert!((contrast_on(text, rgb(under)) - text.contrast_ratio(under)).abs() < 1e-3);
        assert_eq!(ink_alpha(ink, under, &[text], 0.01), 0.01);
    }
}
