//! Interactive desktop cards and the GPU audio visualizers on small
//! surfaces, as [`crate::desktop_widgets`] items of kind `card`:
//!
//! ```toml
//! [[desktop.widgets.items]]
//! id = "card-1"
//! kind = "card"
//! card = "now-playing"
//! [desktop.widgets.items.options]
//! width = 360
//! lyric_lines = 3
//! [desktop.widgets.items.place]
//! anchor = "bottom-left"
//! x = 0.03
//! y = -0.05
//! ```
//!
//! Every piece is **off by default** (no item, nothing at all) and costs
//! nothing while off: each one owns its own small layer surface on the
//! Bottom layer, sized to the piece (never the whole output), created only
//! while it is visible and destroyed when it hides. Placement is the shared
//! [`Placement`] (anchor, offset as fractions of the display, displays,
//! scale, opacity); the full-width strips (wave, mirrored bars) use only the
//! anchor's top / bottom edge.
//!
//! `[visualizers]` holds the pacing every GPU visualizer shares.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::desktop_widgets::{Anchor, CardWidget, DesktopWidget, Placement, WidgetKind};
use crate::surfaces::Span;

/// Card ids (`card = "…"`).
pub const NOW_PLAYING: &str = "now-playing";
pub const CALENDAR: &str = "calendar";
pub const WAVE: &str = "wave";
pub const FRAMED_BARS: &str = "framed-bars";
pub const MIRRORED_BARS: &str = "mirrored-bars";
/// Every card the shell's card host draws.
pub const ALL: [&str; 5] = [NOW_PLAYING, CALENDAR, WAVE, FRAMED_BARS, MIRRORED_BARS];

/// Bounds of every numeric option (logical px unless noted).
pub mod ranges {
    use super::Span;

    pub const NOW_PLAYING_WIDTH: Span = Span::new(280.0, 520.0, 4.0);
    pub const CALENDAR_WIDTH: Span = Span::new(240.0, 420.0, 4.0);
    /// Lyric lines under the title (0 hides the lyrics).
    pub const LYRIC_LINES: Span = Span::new(0.0, 4.0, 1.0);
    /// Seconds a stopped / closed player's card lingers before it fades out.
    pub const CARD_GRACE_S: Span = Span::new(0.0, 60.0, 1.0);
    /// Height of the glowing wave.
    pub const WAVE_HEIGHT: Span = Span::new(40.0, 320.0, 4.0);
    /// Height of the mirrored strip.
    pub const STRIP_HEIGHT: Span = Span::new(24.0, 200.0, 2.0);
    /// The framed bar box.
    pub const BOX_WIDTH: Span = Span::new(160.0, 720.0, 4.0);
    pub const BOX_HEIGHT: Span = Span::new(60.0, 360.0, 4.0);
    pub const BARS: Span = Span::new(8.0, 96.0, 1.0);
    /// Frames per second while audio plays.
    pub const FPS: Span = Span::new(15.0, 60.0, 1.0);
    /// Seconds of silence after which a visualizer stops sampling and drawing.
    pub const SILENCE_S: Span = Span::new(1.0, 60.0, 1.0);
    /// Opacity of a visualizer's fill (0–1).
    pub const OPACITY: Span = Span::new(0.1, 1.0, 0.05);
}

/// Shipped defaults.
pub mod defaults {
    pub const NOW_PLAYING_WIDTH_PX: f32 = 360.0;
    pub const CALENDAR_WIDTH_PX: f32 = 300.0;
    pub const LYRIC_LINES: u32 = 3;
    pub const CARD_GRACE_S: u32 = 8;
    pub const WAVE_HEIGHT_PX: f32 = 140.0;
    pub const STRIP_HEIGHT_PX: f32 = 56.0;
    pub const BOX_WIDTH_PX: f32 = 360.0;
    pub const BOX_HEIGHT_PX: f32 = 150.0;
    pub const BOX_BARS: u32 = 24;
    pub const STRIP_BARS: u32 = 64;
    pub const FPS: u32 = 30;
    pub const SILENCE_S: u32 = 5;
    pub const OPACITY: f32 = 0.85;
    /// Offset of a new card from its anchored corner (fraction of the
    /// display's width / height).
    pub const CARD_OFFSET: (f32, f32) = (0.03, 0.05);
}

fn count(v: u32, span: Span, fallback: u32) -> u32 {
    span.clamp(v as f32, fallback as f32).round() as u32
}

/// `card = "now-playing"`: album art, title / artist, a wavy progress line,
/// transport buttons and optional lyric lines. Shown only while a player is
/// active; fades out `grace_s` after it stops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NowPlayingOptions {
    pub width: f32,
    /// Lyric lines under the title, current line highlighted (0 = none).
    pub lyric_lines: u32,
    /// A gently waving progress line (off: a straight one).
    pub squiggle: bool,
    /// Seconds a stopped player's card stays before fading out.
    pub grace_s: u32,
}

impl Default for NowPlayingOptions {
    fn default() -> Self {
        Self { width: defaults::NOW_PLAYING_WIDTH_PX, lyric_lines: defaults::LYRIC_LINES, squiggle: true, grace_s: defaults::CARD_GRACE_S }
    }
}

impl NowPlayingOptions {
    pub fn width(&self) -> f32 {
        ranges::NOW_PLAYING_WIDTH.clamp(self.width, defaults::NOW_PLAYING_WIDTH_PX)
    }
    pub fn lyric_lines(&self) -> u32 {
        count(self.lyric_lines, ranges::LYRIC_LINES, defaults::LYRIC_LINES)
    }
    pub fn grace(&self) -> std::time::Duration {
        std::time::Duration::from_secs(count(self.grace_s, ranges::CARD_GRACE_S, defaults::CARD_GRACE_S) as u64)
    }
}

/// `card = "calendar"`: a month grid with today highlighted, month
/// navigation and an optional next-event line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarOptions {
    pub width: f32,
    /// The next calendar event under the grid (`[calendar]` sources).
    pub agenda: bool,
}

impl Default for CalendarOptions {
    fn default() -> Self {
        Self { width: defaults::CALENDAR_WIDTH_PX, agenda: true }
    }
}

impl CalendarOptions {
    pub fn width(&self) -> f32 {
        ranges::CALENDAR_WIDTH.clamp(self.width, defaults::CALENDAR_WIDTH_PX)
    }
}

/// `card = "wave"`: a filled, smoothed spectrum curve along the screen
/// edge of the placement's anchor, accent gradient with a soft glow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveOptions {
    pub height: f32,
    /// Soft glow above the curve.
    pub glow: bool,
    pub opacity: f32,
}

impl Default for WaveOptions {
    fn default() -> Self {
        Self { height: defaults::WAVE_HEIGHT_PX, glow: true, opacity: defaults::OPACITY }
    }
}

impl WaveOptions {
    pub fn height(&self) -> f32 {
        ranges::WAVE_HEIGHT.clamp(self.height, defaults::WAVE_HEIGHT_PX)
    }
    pub fn opacity(&self) -> f32 {
        ranges::OPACITY.clamp(self.opacity, defaults::OPACITY)
    }
}

/// `card = "framed-bars"`: a bordered box of bars, like a hardware
/// spectrum display.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FramedBarsOptions {
    pub width: f32,
    pub height: f32,
    pub bars: u32,
    pub opacity: f32,
}

impl Default for FramedBarsOptions {
    fn default() -> Self {
        Self { width: defaults::BOX_WIDTH_PX, height: defaults::BOX_HEIGHT_PX, bars: defaults::BOX_BARS, opacity: defaults::OPACITY }
    }
}

impl FramedBarsOptions {
    pub fn size(&self) -> (f32, f32) {
        (ranges::BOX_WIDTH.clamp(self.width, defaults::BOX_WIDTH_PX), ranges::BOX_HEIGHT.clamp(self.height, defaults::BOX_HEIGHT_PX))
    }
    pub fn bars(&self) -> u32 {
        count(self.bars, ranges::BARS, defaults::BOX_BARS)
    }
    pub fn opacity(&self) -> f32 {
        ranges::OPACITY.clamp(self.opacity, defaults::OPACITY)
    }
}

/// `card = "mirrored-bars"`: a full-width strip of bars mirrored around
/// its middle line, along the anchor's screen edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MirroredBarsOptions {
    pub height: f32,
    pub bars: u32,
    pub opacity: f32,
}

impl Default for MirroredBarsOptions {
    fn default() -> Self {
        Self { height: defaults::STRIP_HEIGHT_PX, bars: defaults::STRIP_BARS, opacity: defaults::OPACITY }
    }
}

impl MirroredBarsOptions {
    pub fn height(&self) -> f32 {
        ranges::STRIP_HEIGHT.clamp(self.height, defaults::STRIP_HEIGHT_PX)
    }
    pub fn bars(&self) -> u32 {
        count(self.bars, ranges::BARS, defaults::STRIP_BARS)
    }
    pub fn opacity(&self) -> f32 {
        ranges::OPACITY.clamp(self.opacity, defaults::OPACITY)
    }
}

/// `[visualizers]`: the pacing every GPU visualizer card shares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizersConfig {
    /// Frames per second while audio plays (15–60).
    pub fps: u32,
    /// Seconds of silence before sampling and drawing stop.
    pub silence_s: u32,
}

impl Default for VisualizersConfig {
    fn default() -> Self {
        Self { fps: defaults::FPS, silence_s: defaults::SILENCE_S }
    }
}

impl VisualizersConfig {
    /// Time between frames while playing.
    pub fn frame_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f32(1.0 / count(self.fps, ranges::FPS, defaults::FPS) as f32)
    }

    pub fn silence(&self) -> std::time::Duration {
        std::time::Duration::from_secs(count(self.silence_s, ranges::SILENCE_S, defaults::SILENCE_S) as u64)
    }
}

/// The typed options of a card (defaults for missing / invalid keys).
pub fn options<T: DeserializeOwned + Default>(card: &CardWidget) -> T {
    let table: toml::Table = card.options.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    toml::Value::Table(table).try_into().unwrap_or_default()
}

/// Stores typed `opts` as a card's options.
pub fn set_options<T: Serialize>(card: &mut CardWidget, opts: &T) {
    if let Ok(toml::Value::Table(t)) = toml::Value::try_from(opts) {
        card.options = t.into_iter().collect();
    }
}

/// The card part of a widget, when it is one.
pub fn as_card(w: &DesktopWidget) -> Option<&CardWidget> {
    match &w.kind {
        WidgetKind::Card(c) => Some(c),
        _ => None,
    }
}

/// A new card widget (enabled) of `card` at `anchor`, offset inwards.
pub fn new_card(card: &str, anchor: Anchor) -> DesktopWidget {
    let (fx, fy) = anchor.fraction();
    let inward = |f: f32, d: f32| if f < 0.25 { d } else if f > 0.75 { -d } else { 0.0 };
    let (dx, dy) = defaults::CARD_OFFSET;
    let mut c = CardWidget { card: card.to_string(), ..Default::default() };
    match card {
        NOW_PLAYING => set_options(&mut c, &NowPlayingOptions::default()),
        CALENDAR => set_options(&mut c, &CalendarOptions::default()),
        WAVE => set_options(&mut c, &WaveOptions::default()),
        FRAMED_BARS => set_options(&mut c, &FramedBarsOptions::default()),
        MIRRORED_BARS => set_options(&mut c, &MirroredBarsOptions::default()),
        _ => {}
    }
    let strip = matches!(card, WAVE | MIRRORED_BARS);
    DesktopWidget {
        id: String::new(),
        enabled: true,
        place: Placement { anchor, x: if strip { 0.0 } else { inward(fx, dx) }, y: if strip { 0.0 } else { inward(fy, dy) }, ..Placement::default() },
        kind: WidgetKind::Card(c),
    }
}

/// Whether a strip anchored at `anchor` runs along the top edge (else the
/// bottom one).
pub fn strip_on_top(anchor: Anchor) -> bool {
    anchor.fraction().1 < 0.25
}

/// Top-left of a `size` card (logical px) on an `output`, kept fully on
/// screen; `rtl` mirrors the placement left ↔ right.
pub fn card_origin(place: &Placement, output: (f32, f32), size: (f32, f32), rtl: bool) -> (f32, f32) {
    let mut p = place.clone();
    if rtl {
        let i = Anchor::ALL.iter().position(|a| *a == p.anchor).unwrap_or(4);
        p.anchor = Anchor::ALL[(i / 3) * 3 + (2 - i % 3)];
        p.x = -p.x;
    }
    let (x, y) = p.origin(output, size);
    (x.clamp(0.0, (output.0 - size.0).max(0.0)), y.clamp(0.0, (output.1 - size.1).max(0.0)))
}

/// Enabled card widgets of the shell's card host, in list order.
pub fn enabled_cards(widgets: &crate::desktop_widgets::DesktopWidgetsConfig) -> impl Iterator<Item = (&DesktopWidget, &CardWidget)> {
    widgets.items.iter().filter(|w| w.enabled).filter_map(|w| as_card(w).filter(|c| ALL.contains(&c.card.as_str())).map(|c| (w, c)))
}

/// Turns the first `card` widget on (adding one at `anchor` if there is
/// none) or every `card` widget off.
pub fn set_card(widgets: &mut crate::desktop_widgets::DesktopWidgetsConfig, card: &str, on: Option<Anchor>) {
    let mut found = false;
    for w in widgets.items.iter_mut() {
        if as_card(w).is_some_and(|c| c.card == card) {
            w.enabled = on.is_some() && !found;
            if let Some(a) = on.filter(|_| !found) {
                w.place.anchor = a;
            }
            found = true;
        }
    }
    if let (false, Some(a)) = (found, on) {
        widgets.add(new_card(card, a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_widgets::DesktopWidgetsConfig;

    #[test]
    fn nothing_by_default() {
        assert_eq!(enabled_cards(&DesktopWidgetsConfig::default()).count(), 0);
    }

    #[test]
    fn options_round_trip_and_clamp() {
        let mut w = new_card(NOW_PLAYING, Anchor::BottomLeft);
        let WidgetKind::Card(c) = &mut w.kind else { unreachable!() };
        let mut o: NowPlayingOptions = options(c);
        assert_eq!(o, NowPlayingOptions::default());
        o.width = 9999.0;
        o.lyric_lines = 40;
        set_options(c, &o);
        let back: NowPlayingOptions = options(c);
        assert_eq!(back.width(), ranges::NOW_PLAYING_WIDTH.max);
        assert_eq!(back.lyric_lines(), 4);
        // Junk options fall back to the defaults.
        c.options.insert("width".into(), toml::Value::String("wide".into()));
        assert_eq!(options::<NowPlayingOptions>(c), NowPlayingOptions::default());
    }

    #[test]
    fn placement_anchors_mirrors_and_stays_on_screen() {
        let p = Placement { anchor: Anchor::BottomRight, x: -0.02, y: -0.05, ..Placement::default() };
        let (x, y) = card_origin(&p, (2000.0, 1000.0), (300.0, 200.0), false);
        assert!((x - 1660.0).abs() < 0.01 && (y - 750.0).abs() < 0.01, "{x} {y}");
        // Right-to-left: the bottom-right card moves to the bottom left.
        let (x, _) = card_origin(&p, (2000.0, 1000.0), (300.0, 200.0), true);
        assert!((x - 40.0).abs() < 0.01, "{x}");
        let off = Placement { anchor: Anchor::TopLeft, x: 0.9, y: 0.9, ..Placement::default() };
        assert_eq!(card_origin(&off, (800.0, 600.0), (300.0, 200.0), false), (500.0, 400.0));
    }

    #[test]
    fn set_card_adds_once_and_toggles() {
        let mut w = DesktopWidgetsConfig::default();
        set_card(&mut w, CALENDAR, Some(Anchor::TopRight));
        set_card(&mut w, CALENDAR, Some(Anchor::TopLeft));
        assert_eq!(w.items.len(), 1);
        assert_eq!(w.items[0].place.anchor, Anchor::TopLeft);
        assert_eq!(enabled_cards(&w).count(), 1);
        set_card(&mut w, CALENDAR, None);
        assert_eq!(enabled_cards(&w).count(), 0);
        assert!(strip_on_top(Anchor::Top) && !strip_on_top(Anchor::Bottom));
    }

    #[test]
    fn pacing_is_clamped() {
        let mut v = VisualizersConfig { fps: 500, ..Default::default() };
        assert!((v.frame_interval().as_secs_f32() - 1.0 / 60.0).abs() < 1e-4);
        v.fps = 1;
        assert!((v.frame_interval().as_secs_f32() - 1.0 / 15.0).abs() < 1e-4);
    }
}
