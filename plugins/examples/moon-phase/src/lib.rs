//! Example Mandara plugin (Rust → WebAssembly component). Build with
//! `cargo build --release --target wasm32-wasip2`, then copy the `.wasm`
//! next to `plugin.toml` as `plugin.wasm` in
//! `~/.local/share/mandara/plugins-dev/moon-phase/`.

use std::sync::atomic::{AtomicBool, Ordering};

wit_bindgen::generate!({ world: "plugin", path: "../../../crates/mandara-plugins/wit" });

use mandara::plugin::host;
use mandara::plugin::types::{Kind, Node, Tone};

/// Mean synodic month, days.
const SYNODIC: f64 = 29.530_588_853;
/// The eight phases as the colour emoji font draws them (as seen from the
/// northern hemisphere: a waxing moon lit on the right). Each ends in
/// U+FE0F so the emoji form is used, not a text font's flat symbol.
const PHASE_GLYPHS: [&str; 8] = ["🌑\u{FE0F}", "🌒\u{FE0F}", "🌓\u{FE0F}", "🌔\u{FE0F}", "🌕\u{FE0F}", "🌖\u{FE0F}", "🌗\u{FE0F}", "🌘\u{FE0F}"];
/// Sizes (logical px): tonight's moon, the stat cards' moons, the cycle
/// strip (tonight's phase stands out larger), the phase name.
const HERO_MOON_PX: f32 = 64.0;
const STAT_MOON_PX: f32 = 18.0;
const STRIP_MOON_PX: f32 = 16.0;
const STRIP_NOW_PX: f32 = 26.0;
const TITLE_PX: f32 = 17.0;
const SMALL_PX: f32 = 11.5;
/// Spacing (logical px) inside the hero, between the stat cards, in the strip.
const HERO_GAP: f32 = 14.0;
const STACK_GAP: f32 = 4.0;
const STAT_GAP: f32 = 8.0;
const STRIP_GAP: f32 = 6.0;
const SECTION_GAP: f32 = 12.0;
/// A reference new moon: 2000-01-06 18:14 UTC, as Unix milliseconds.
const NEW_MOON_MS: f64 = 947_182_440_000.0;

/// The phase cycle shows until the user hides it.
static DETAILED: AtomicBool = AtomicBool::new(true);
/// The moon age card's mark (a crescent, as an emoji like the phases).
const AGE_GLYPH: &str = "🌙\u{FE0F}";

struct Moon {
    /// Days since the last new moon.
    age: f64,
}

impl Moon {
    fn now() -> Self {
        let days = (host::now_unix_ms() as f64 - NEW_MOON_MS) / 86_400_000.0;
        Self { age: days.rem_euclid(SYNODIC) }
    }

    /// Lit fraction of the disc, 0..=1.
    fn illumination(&self) -> f64 {
        (1.0 - (std::f64::consts::TAU * self.age / SYNODIC).cos()) / 2.0
    }

    fn name(&self) -> &'static str {
        match self.age / SYNODIC * 8.0 {
            p if p < 0.5 => "New moon",
            p if p < 1.5 => "Waxing crescent",
            p if p < 2.5 => "First quarter",
            p if p < 3.5 => "Waxing gibbous",
            p if p < 4.5 => "Full moon",
            p if p < 5.5 => "Waning gibbous",
            p if p < 6.5 => "Last quarter",
            p if p < 7.5 => "Waning crescent",
            _ => "New moon",
        }
    }

    fn days_to_full(&self) -> f64 {
        (SYNODIC / 2.0 - self.age).rem_euclid(SYNODIC)
    }

    fn days_to_new(&self) -> f64 {
        SYNODIC - self.age
    }

    /// Which of the eight phases (0 = new, 4 = full).
    fn phase_index(&self) -> usize {
        ((self.age / SYNODIC * 8.0).round() as usize) % 8
    }

    fn waxing(&self) -> bool {
        self.age < SYNODIC / 2.0
    }

    /// Which side of the disc is lit, as seen from `hemisphere`: a waxing
    /// moon is lit on the right in the north and on the left in the south.
    fn lit_side(&self, hemisphere: Hemisphere) -> &'static str {
        let waxing = self.age < SYNODIC / 2.0;
        match (waxing, hemisphere) {
            (true, Hemisphere::North) | (false, Hemisphere::South) => "Lit on the right",
            _ => "Lit on the left",
        }
    }
}

#[derive(Copy, Clone)]
enum Hemisphere {
    North,
    South,
}

/// The `hemisphere` setting (plugin.toml `[[settings]]`); values arrive as JSON.
fn hemisphere() -> Hemisphere {
    match host::setting("hemisphere").as_deref() {
        Some("\"south\"") => Hemisphere::South,
        _ => Hemisphere::North,
    }
}

/// The `show_percent` setting (on unless set to `false`).
fn show_percent() -> bool {
    host::setting("show_percent").as_deref() != Some("false")
}

/// Tiny builder for the view arena.
#[derive(Default)]
struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    fn add(&mut self, kind: Kind, text: &str) -> u32 {
        self.nodes.push(Node { kind, text: text.into(), value: 0.0, children: Vec::new(), action: String::new(), tone: Tone::Normal, size: 0.0, gap: 6.0, bold: false, grow: false });
        (self.nodes.len() - 1) as u32
    }
    fn node(&mut self, i: u32) -> &mut Node {
        &mut self.nodes[i as usize]
    }
    fn children(&mut self, parent: u32, kids: &[u32]) -> u32 {
        self.node(parent).children = kids.to_vec();
        parent
    }
    fn view(self, root: u32) -> Option<View> {
        Some(View { nodes: self.nodes, root })
    }
}

/// The emoji of phase `i` as seen from `hemisphere` (the south sees the
/// disc mirrored).
fn glyph(i: usize, hemisphere: Hemisphere) -> &'static str {
    match hemisphere {
        Hemisphere::North => PHASE_GLYPHS[i % 8],
        Hemisphere::South => PHASE_GLYPHS[(8 - i % 8) % 8],
    }
}

/// "Tonight", "Tomorrow" or "In {days} days".
fn when(days: f64) -> String {
    match days.round() as i64 {
        n if n < 1 => host::translate("Tonight"),
        1 => host::translate("Tomorrow"),
        n => host::translate("In {days} days").replace("{days}", &n.to_string()),
    }
}

/// A small stat card: a moon emoji, a muted label and a bold value.
fn stat(t: &mut Tree, moon: &str, label: &str, value: &str) -> u32 {
    let mark = t.add(Kind::Text, moon);
    t.node(mark).size = STAT_MOON_PX;
    let l = t.add(Kind::Text, label);
    t.node(l).tone = Tone::Muted;
    t.node(l).size = SMALL_PX;
    let v = t.add(Kind::Text, value);
    t.node(v).bold = true;
    let card = t.add(Kind::Card, "");
    t.node(card).gap = STACK_GAP;
    t.node(card).grow = true;
    t.children(card, &[mark, l, v])
}

fn details(t: &mut Tree, m: &Moon) -> Vec<u32> {
    let hemi = hemisphere();
    let now = m.phase_index();

    // Tonight's moon, large, beside its name, illumination and lit side.
    let moon = t.add(Kind::Text, glyph(now, hemi));
    t.node(moon).size = HERO_MOON_PX;
    let title = t.add(Kind::Text, m.name());
    t.node(title).bold = true;
    t.node(title).size = TITLE_PX;
    // Composed strings go through `host::translate` with a `{placeholder}`;
    // plain labels are translated by the host automatically.
    let pct = t.add(Kind::Text, &host::translate("{percent}% illuminated").replace("{percent}", &format!("{:.0}", m.illumination() * 100.0)));
    t.node(pct).tone = Tone::Muted;
    let meter = t.add(Kind::Meter, "");
    t.node(meter).value = m.illumination() as f32;
    t.node(meter).tone = Tone::Accent;
    let side = t.add(Kind::Text, &format!("{} · {}", host::translate(if m.waxing() { "Waxing" } else { "Waning" }), host::translate(m.lit_side(hemi))));
    t.node(side).tone = Tone::Muted;
    t.node(side).size = SMALL_PX;
    let facts = t.add(Kind::Column, "");
    t.node(facts).gap = STACK_GAP;
    t.node(facts).grow = true;
    t.children(facts, &[title, pct, meter, side]);
    let hero = t.add(Kind::Row, "");
    t.node(hero).gap = HERO_GAP;
    t.children(hero, &[moon, facts]);

    // Next full and new moon, and the moon's age.
    let full = stat(t, glyph(4, hemi), "Full moon", &when(m.days_to_full()));
    let new = stat(t, glyph(0, hemi), "New moon", &when(m.days_to_new()));
    let age = stat(t, AGE_GLYPH, "Moon age", &host::translate("{days} d").replace("{days}", &format!("{:.1}", m.age)));
    let stats = t.add(Kind::Row, "");
    t.node(stats).gap = STAT_GAP;
    t.children(stats, &[full, new, age]);

    let mut rows = vec![hero, stats];
    // The whole cycle, tonight's phase larger, with how far through it is.
    if DETAILED.load(Ordering::Relaxed) {
        let mut strip_kids = Vec::with_capacity(PHASE_GLYPHS.len() * 2 + 1);
        for i in 0..PHASE_GLYPHS.len() {
            if i > 0 {
                strip_kids.push(t.add(Kind::Spacer, ""));
            }
            let g = t.add(Kind::Text, glyph(i, hemi));
            t.node(g).size = if i == now { STRIP_NOW_PX } else { STRIP_MOON_PX };
            strip_kids.push(g);
        }
        let strip = t.add(Kind::Row, "");
        t.node(strip).gap = STRIP_GAP;
        t.children(strip, &strip_kids);
        let progress = t.add(Kind::Meter, "");
        t.node(progress).value = (m.age / SYNODIC) as f32;
        t.node(progress).tone = Tone::Accent;
        let caption = t.add(Kind::Text, &host::translate("Day {day} of {month}").replace("{day}", &format!("{:.1}", m.age)).replace("{month}", &format!("{SYNODIC:.1}")));
        t.node(caption).tone = Tone::Muted;
        t.node(caption).size = SMALL_PX;
        let cycle = t.add(Kind::Column, "");
        t.node(cycle).gap = STRIP_GAP;
        t.children(cycle, &[strip, progress, caption]);
        rows.push(cycle);
    }
    let toggle = t.add(Kind::Button, if DETAILED.load(Ordering::Relaxed) { "Hide the cycle" } else { "Show the cycle" });
    t.node(toggle).action = "toggle".into();
    let footer = t.add(Kind::Row, "");
    let push = t.add(Kind::Spacer, "");
    t.children(footer, &[push, toggle]);
    rows.push(footer);
    rows
}

struct Plugin;

impl Guest for Plugin {
    fn init() -> Result<(), String> {
        host::log("moon phase ready");
        Ok(())
    }

    fn bar(_theme: Palette) -> Option<View> {
        let m = Moon::now();
        let mut t = Tree::default();
        let row = t.add(Kind::Row, "");
        let icon = t.add(Kind::Icon, "moon");
        t.node(icon).tone = Tone::Accent;
        t.node(icon).size = 15.0;
        if show_percent() {
            let label = t.add(Kind::Text, &format!("{:.0}%", m.illumination() * 100.0));
            t.children(row, &[icon, label]);
        } else {
            t.children(row, &[icon]);
        }
        t.view(row)
    }

    fn panel(_theme: Palette) -> Option<View> {
        let m = Moon::now();
        let mut t = Tree::default();
        let col = t.add(Kind::Column, "");
        t.node(col).gap = SECTION_GAP;
        let rows = details(&mut t, &m);
        t.children(col, &rows);
        t.view(col)
    }

    fn sidebar(_theme: Palette) -> Option<View> {
        let m = Moon::now();
        let mut t = Tree::default();
        let card = t.add(Kind::Card, "");
        t.node(card).gap = SECTION_GAP;
        let rows = details(&mut t, &m);
        t.children(card, &rows);
        t.view(card)
    }

    fn on_event(action: String) {
        if action == "toggle" {
            DETAILED.fetch_xor(true, Ordering::Relaxed);
        }
    }

    fn refresh_ms() -> u32 {
        60_000
    }
}

export!(Plugin);
