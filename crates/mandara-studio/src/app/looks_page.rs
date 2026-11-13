//! Studio → Themes: the live preview, the looks gallery, the theme gallery
//! (`themes_page.rs`), the Cursors and Icons tabs (`cursor_page.rs`, `icons_page.rs`)
//! and the color / style settings, plus the desktop cards / GPU
//! visualizers.
//!
//! * **Looks** — the built-in one-click looks (`mandara_config::looks`):
//!   bar or bar style preset, desktop widgets and cards, an HCT palette
//!   generated from an OKLCH seed, typography and the radius / opacity
//!   scale; then your own (`looks_yours.rs`). Each tile is a live mock
//!   drawn from the look's own palette (a handful of quads; only a text
//!   decoration is painted, once, and cached). Previews are the default.
//!   Applying one from your own setup first saves that setup as the
//!   "Before looks" profile; "Restore my look" brings it back (and the
//!   Profiles page's undo history covers every step).
//! * **Desktop cards & visualizers** — the five card kinds of
//!   `mandara_config::desktop_cards`, each on its own small surface; turn
//!   one on and pick where it sits (shown on Studio → Desktop → Cards &
//!   visualizers; they are items of the widgets' shared list).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px};
use mandara_config::desktop_cards::{self as dc, CALENDAR, FRAMED_BARS, MIRRORED_BARS, NOW_PLAYING, WAVE};
use mandara_config::desktop_widgets::{Anchor, ClockFace, Placement, WidgetKind, ranges};
use mandara_config::looks::{self, LOOKS, LookDef, LookOptions, LookVisualizer};
use mandara_config::user_looks::UserLook;
use mandara_config::{BarLayout, BarPosition, ShellConfig};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::backdrop::paint::FontNames;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::lock::paint::TextPainter;
use mandara_ui::state::update_config;

use std::rc::Rc;
use super::compositor_page::{badge, card, chip, heading};
use super::themes_page::section_head;
use super::{StudioView, swatch_list};
use crate::registry::desktop_settings::COMPONENTS;
use crate::registry::system_settings::CURSOR;
use crate::registry::{self, Section};
use crate::tokens::*;

/// Themes settings drawn by the galleries instead of as rows: light / dark
/// (the gallery's header) and the wallpaper scheme (the wallpaper card).
const SHOWN_ABOVE: &[&str] = &["/theme/mode", "/theme/source/variant"];
/// The app-theme switches, drawn as tiles by `app_themes_page.rs`.
const APP_THEMES_GROUP: &str = "App themes";

/// The looks grid and tile drawing.
const LOOK_GRID: CardGrid = CardGrid { min_w: 280.0, max_w: 460.0, gap: SPACE_16_PX };
const THUMB_H_PX: f32 = 150.0;
const THUMB_BAR_PX: f32 = 7.0;
const THUMB_HAIRLINE_PX: f32 = 4.0;
const THUMB_SIDE_PX: f32 = 8.0;
const THUMB_FRAME_PX: f32 = 2.0;
const THUMB_PILL_W_PX: [f32; 3] = [30.0, 22.0, 34.0];
const THUMB_MICRO_W_PX: f32 = 36.0;
const THUMB_CLOCK_PX: f32 = 20.0;
const THUMB_CARD: (f32, f32) = (62.0, 22.0);
const THUMB_CALENDAR_PX: f32 = 34.0;
const THUMB_MONITOR: (f32, f32) = (26.0, 3.0);
const THUMB_WAVE_BARS: usize = 36;
const THUMB_WAVE_H_PX: f32 = 16.0;
const THUMB_BOX: (f32, f32) = (46.0, 26.0);
const THUMB_INSET_PX: f32 = 8.0;
const THUMB_DECOR_PX: f32 = 9.0;
/// The decoration strip on the tile's end edge (logical px) and the
/// density it is painted at.
const THUMB_DECOR_STRIP_W_PX: f32 = 22.0;
const THUMB_DECOR_DENSITY: f32 = 2.0;
/// Background: how much of the accent tints the mock's sky.
const SKY_TINT: f32 = 0.35;
const WAVE_SHAPE: [f32; 8] = [0.3, 0.55, 0.9, 0.7, 1.0, 0.6, 0.8, 0.4];
const DIAGONAL_DEG: f32 = 135.0;
/// A look card's palette dots.
const LOOK_DOT_PX: f32 = DIM_10_PX;

/// The card kinds offered, with their label and description.
const CARDS: [(&str, &str, &str); 5] = [
    (NOW_PLAYING, tk("Now playing"), tk("Album art, title, a wavy progress line, controls and synced lyrics; shown while music plays")),
    (CALENDAR, tk("Calendar"), tk("This month with today marked, month paging and your next event")),
    (WAVE, tk("Glowing wave"), tk("A smoothed spectrum along the screen edge, with a soft glow; only while music plays")),
    (FRAMED_BARS, tk("Framed bars"), tk("A box of segmented bars, like a hardware spectrum display")),
    (MIRRORED_BARS, tk("Mirrored bars"), tk("A full-width strip of bars mirrored round its middle")),
];
const CARD_ANCHORS: [(Anchor, &str); 9] = [
    (Anchor::TopLeft, tk("Top left")),
    (Anchor::Top, tk("Top")),
    (Anchor::TopRight, tk("Top right")),
    (Anchor::Left, tk("Left")),
    (Anchor::Center, tk("Center")),
    (Anchor::Right, tk("Right")),
    (Anchor::BottomLeft, tk("Bottom left")),
    (Anchor::Bottom, tk("Bottom")),
    (Anchor::BottomRight, tk("Bottom right")),
];
const STRIP_ANCHORS: [(Anchor, &str); 2] = [(Anchor::Top, tk("Top edge")), (Anchor::Bottom, tk("Bottom edge"))];

/// A look's blurb in the interface language (the English text lives in
/// `mandara_config::looks`; these mark it for translation).
/// "Expressive components": the component designs a look brings.
fn components_line(look: &LookDef) -> String {
    tf("{preset} components", &[("preset", &tr(super::component_designs::preset_name(look.components)))])
}

fn blurb(look: &LookDef) -> String {
    let key = match look.id {
        "corniche-dusk" => {
            tk("Rose evening glass: headline clock, system monitor, media card and glass pills.")
        }
        "pharos-ink" => {
            tk("Monochrome ink: a hairline bar dense with stats, a greeting and a calendar.")
        }
        "montaza-crimson" => {
            tk("Bold palace red: a framed spectrum box, an Arabic clock and calligraphy.")
        }
        "qaitbay-forest" => {
            tk("Deep green: a giant stacked clock behind the subject and a dock media chip.")
        }
        "anfushi-wave" => {
            tk("Open sea: a full-width glowing wave, a katakana accent and one tiny pill.")
        }
        "night-frame" => {
            tk("Midnight violet: a side bar framing the screen, a greeting, calendar and bars.")
        }
        "redmond" => tk("A Windows 11-style desktop: a centred taskbar with your apps, a stacked clock, notifications in the corner and no dock."),
        "cupertino" => tk("A macOS-style desktop: a slim menu bar on top, a magnifying dock below and a Spotlight-style search."),
        "corniche-frame" => tk("Expressive: the whole screen framed, round buttons, a scalloped rail and a day-block clock over a glowing wave."),
        "raml-line" => tk("A tram line of flower stops along the top and down the left, a split-flap clock and the music card."),
        "lighthouse-rail" => tk("Only a wide rail of flower buttons in a coral screen frame, giant numerals and a calendar."),
        "phosphor" => tk("Terminal green: a monospace status line of stat blocks, a pixel clock and a system monitor column."),
        "pharos-lantern" => tk("Amber light: a lantern of scalloped buttons hanging from the top and a ring clock in the middle."),
        "stanley-arches" => tk("Three floating docks along the bottom like a bridge's arches, an analog clock and mirrored bars."),
        "souk-tiles" => tk("Bright spice colours: one island of big squircle tiles at the bottom and a clock of boxes."),
        "sea-glass" => tk("No bar, only islands: rows of connected tiles floating under the top edge, an orbit clock, music and a calendar."),
        "library-desk" => tk("Paper and ink for focused work: the productive bar at the bottom with tasks, notes and a focus timer."),
        "ink-rush" => tk("A cel-shaded comic desktop: thick ink outlines, hard shadows, halftone shading, bold colour and bouncy motion."),
        "zen" => tk("Nothing in the way: one tiny pill that hides until you touch the top edge, hot corners and a word clock."),
        _ => look.blurb,
    };
    tr(key)
}

thread_local! {
    /// Each look's palette (dark), generated from its seed the first time
    /// its card shows.
    static PALETTES: RefCell<Vec<Option<MandaraPalette>>> = const { RefCell::new(Vec::new()) };
    /// Each look's decoration as the desktop paints it (Arabic joined and
    /// turned, kana upright), keyed by look, fonts and colour; and the
    /// painter that drew them.
    static DECORATIONS: RefCell<DecorationCache> = RefCell::new((None, HashMap::new()));
}

/// The painter and the decoration images it drew, by key.
type DecorationCache = (Option<TextPainter>, HashMap<u64, Arc<RenderImage>>);

/// The look's text decoration drawn by the shell's own painters, sized for
/// the tile's end-edge strip.
fn decoration_image(look: &LookDef, lp: &MandaraPalette, cfg: &ShellConfig) -> Option<Arc<RenderImage>> {
    let mut w = look.widgets().into_iter().find(|w| matches!(w.kind, WidgetKind::Decoration(_)))?;
    let WidgetKind::Decoration(d) = &w.kind else { return None;
    };
    let size = ((THUMB_DECOR_STRIP_W_PX * THUMB_DECOR_DENSITY) as u32, (THUMB_H_PX * THUMB_DECOR_DENSITY) as u32);
    // The tile's text size on a frame `size.1` tall.
    let scale = THUMB_DECOR_PX * THUMB_DECOR_DENSITY / (d.size.max(1.0) * size.1 as f32 / ranges::REFERENCE_HEIGHT_PX);
    // Same edge as on the desktop, so the sideways turn matches.
    w.place = Placement { anchor: w.place.anchor, scale, opacity: w.place.opacity, ..Placement::default() };
    let t = &cfg.typography;
    let fonts = FontNames::new(look.ui_font, look.display_font, &t.arabic_font, &t.mono_font);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (look.id, &fonts, lp.primary.to_rgba8(), format!("{w:?}")).hash(&mut h);
    let key = h.finish();
    DECORATIONS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let (painter, images) = &mut *cache;
        if let Some(i) = images.get(&key) {
            return Some(i.clone());
        }
        let text = painter.get_or_insert_with(TextPainter::new);
        let rgba = mandara_ui::backdrop::widgets::thumbnail(&w, size, &fonts, lp.primary, text)?;
        text.release_glyphs();
        let image = mandara_ui::backdrop::widgets::render_image(rgba);
        images.insert(key, image.clone());
        Some(image)
    })
}

/// One of your Looks' palette: its own colour source when it changed the
/// parent's (a seed or a built-in theme), else the parent's; cached by Look
/// and source.
fn user_palette(look: &UserLook) -> MandaraPalette {
    thread_local! {
        static USER_PALETTES: RefCell<HashMap<(String, String), MandaraPalette>> = RefCell::new(HashMap::new());
    }
    let parent_index = LOOKS.iter().position(|l| l.id == look.parent).unwrap_or(0);
    let source = mandara_config::profiles::get_path_pub(&look.settings, "theme.source").map(|v| v.to_string()).unwrap_or_default();
    if source.is_empty() {
        return look_palette(parent_index);
    }
    let key = (look.id.clone(), source);
    USER_PALETTES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(p) = cache.get(&key) {
            return p.clone();
        }
        let parent = &LOOKS[parent_index];
        let dark = mandara_config::profiles::get_path_pub(&look.settings, "theme.mode").and_then(|v| v.as_str()).map_or(parent.dark, |m| m != "light");
        let resolved = mandara_config::profiles::get_path_pub(&look.settings, "theme.source").cloned().and_then(|v| v.try_into::<mandara_theme::ThemeSource>().ok());
        let palette = match resolved {
            Some(mandara_theme::ThemeSource::Seed { color, variant }) => {
                mandara_theme::material::palette_from_seed(color, variant, dark)
            }
            Some(mandara_theme::ThemeSource::Preset { preset }) => {
                mandara_theme::catalog::find(&preset).map_or_else(|| look_palette(parent_index), |t| t.palette(dark))
            }
            _ => look_palette(parent_index),
        };
        if cache.len() >= mandara_config::user_looks::MAX_LOOKS {
            cache.clear();
        }
        cache.insert(key, palette.clone());
        palette
    })
}

/// One of your Looks as its mock draws it: the parent, with your bar
/// (layout and edge) and theme mode where you changed them.
fn user_view(mine: &UserLook, parent: &'static LookDef) -> LookDef {
    let get = |path: &str| mandara_config::profiles::get_path_pub(&mine.settings, path).cloned();
    let mut view = *parent;
    let layout = get("bar.layout").and_then(|v| v.try_into::<BarLayout>().ok()).unwrap_or(parent.layout());
    let position = get("bar.position").and_then(|v| v.try_into::<BarPosition>().ok()).unwrap_or(parent.position());
    if layout != parent.layout() || position != parent.position() {
        view.bar = looks::LookBar::Plain { layout, position, screen_frame: parent.screen_frame(), opacity: 1.0, capsules: false, height: None };
    }
    if let Some(mode) = get("theme.mode").and_then(|v| v.as_str().map(str::to_owned)) {
        view.dark = mode != "light";
    }
    view
}

thread_local! {
    /// The Look cards that were on screen last frame (by key).
    static SHOWN: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
}

/// Whether the card `key` was on screen last frame.
fn shown(key: &str) -> bool {
    SHOWN.with(|s| s.borrow().contains(key))
}

/// A card's stand-in mock while it is off screen.
fn placeholder(p: &MandaraPalette) -> AnyElement {
    div().h(px(THUMB_H_PX)).w_full().rounded(px(RADIUS_14_PX)).bg(p.surface_container_high.hsla()).into_any_element()
}

/// An empty overlay that notes whether its card is inside the window this
/// frame, and redraws once when that changes (so a card's mock is drawn
/// only while it can be seen).
fn visibility_probe(key: String) -> AnyElement {
    gpui::canvas(
        move |bounds, window, _| {
            let view = window.viewport_size();
            let on = bounds.bottom() > px(0.0) && bounds.top() < view.height && bounds.right() > px(0.0) && bounds.left() < view.width;
            let changed = SHOWN.with(|s| {
                let mut s = s.borrow_mut();
                if on { s.insert(key.clone()) } else { s.remove(&key) }
            });
            if changed {
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

fn look_palette(i: usize) -> MandaraPalette {
    PALETTES.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_empty() {
            p.resize(LOOKS.len(), None);
        }
        p[i].get_or_insert_with(|| { let l = &LOOKS[i]; mandara_theme::material::palette_from_seed(l.seed_color(), l.variant, l.dark) }).clone()
    })
}

/// A cheap live mock of a look, drawn with its own palette (its text
/// decoration painted like the desktop's).
fn thumbnail(look: &LookDef, lp: &MandaraPalette, cfg: &ShellConfig) -> AnyElement {
    let bar_fill = lp.surface_container_high.hsla();
    let accent = lp.primary.hsla();
    let side = look.position().is_vertical();
    let mut root = div()
        .relative()
        .h(px(THUMB_H_PX))
        .w_full()
        .rounded(px(RADIUS_14_PX))
        .overflow_hidden()
        .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, lp.surface_dim, lp.surface_dim.mix(lp.primary, SKY_TINT)));
    // The bar.
    let pills = |el: gpui::Div| {
        el.children(THUMB_PILL_W_PX.iter().map(|w| {
            div().w(px(*w)).h(px(THUMB_BAR_PX)).rounded_full().bg(bar_fill)
        }))
    };
    // A bottom bar sits on the mock's bottom edge.
    let bottom = look.position() == BarPosition::Bottom;
    let edge = |el: gpui::Div, off: f32| {
        if bottom { el.bottom(px(off)) } else { el.top(px(off))
        }
    };
    root = root.child(match (side, look.layout()) {
        (true, _) => div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(THUMB_SIDE_PX))
            .bg(bar_fill)
            .row()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(THUMB_INSET_PX))
            .child(div().size(px(THUMB_FRAME_PX * 2.0)).rounded_full().bg(accent))
            .child(div().size(px(THUMB_FRAME_PX * 2.0)).rounded_full().bg(accent)),
        (_, BarLayout::Hairline) => edge(div().absolute(), 0.0).left_0().right_0().h(px(THUMB_HAIRLINE_PX)).bg(bar_fill).border_b_1().border_color(accent),
        (_, BarLayout::Micro) => edge(div().absolute(), THUMB_FRAME_PX * 2.0).left_0().right_0().row().justify_center().child(div().w(px(THUMB_MICRO_W_PX)).h(px(THUMB_BAR_PX)).rounded_full().bg(bar_fill)),
        (_, BarLayout::FloatingPills) => pills(edge(div().absolute(), THUMB_FRAME_PX * 2.0).left(px(THUMB_INSET_PX)).right(px(THUMB_INSET_PX)).ltr_row().justify_between()),
        (_, BarLayout::ContiguousIsland) => edge(div().absolute(), THUMB_FRAME_PX * 2.0).left_0().right_0().row().justify_center().child(div().w(px(THUMB_PILL_W_PX[2] * 2.5)).h(px(THUMB_BAR_PX)).rounded_full().bg(bar_fill)),
        (_, BarLayout::MinimalEdge) => edge(div().absolute(), 0.0).left_0().right_0().h(px(THUMB_BAR_PX)).bg(bar_fill),
        (_, BarLayout::Notch) => edge(div().absolute(), 0.0).left(px(THUMB_INSET_PX * 2.0)).right(px(THUMB_INSET_PX * 2.0)).h(px(THUMB_BAR_PX)).rounded_b(px(THUMB_BAR_PX / 2.0)).bg(bar_fill),
    });
    if look.screen_frame() && side {
        root = root.child(div().absolute().inset_0().border_t(px(THUMB_FRAME_PX)).border_r(px(THUMB_FRAME_PX)).border_b(px(THUMB_FRAME_PX)).border_color(bar_fill).rounded(px(RADIUS_14_PX)));
    }
    let left0 = if side { THUMB_SIDE_PX + THUMB_INSET_PX } else { THUMB_INSET_PX };
    // The clock.
    if let Some((face, anchor)) = look.clock {
        let text: SharedString = match face {
            ClockFace::Stacked => "12\n45".into(),
            ClockFace::Arabic => "١٢:٤٥".into(),
            ClockFace::Greeting => "Hello".into(),
            _ => "12:45".into(),
        };
        let (fx, fy) = anchor.fraction();
        let el = div().absolute().text_size(px(THUMB_CLOCK_PX)).font_weight(FontWeight::BOLD).text_color(lp.on_surface.with_alpha(if look.depth { ALPHA_75 } else { 1.0 }).hsla()).child(text);
        let el = if fx < 0.25 { el.left(px(left0)) } else if fx > 0.75 { el.right(px(THUMB_INSET_PX)) } else { el.left_0().right_0().row().justify_center() };
        let el = if fy < 0.25 { el.top(px(THUMB_INSET_PX * 2.0)) } else if fy > 0.75 { el.bottom(px(THUMB_INSET_PX)) } else { el.top(px(THUMB_H_PX / 2.0 - THUMB_CLOCK_PX)) };
        root = root.child(el);
    }
    if look.monitor.is_some() {
        root = root.child(
            div()
                .absolute()
                .left(px(left0))
                .top(px(THUMB_H_PX / 2.0))
                .row()
                .flex_col()
                .gap(px(THUMB_MONITOR.1))
                .children((0..4).map(|i| {
                    div().w(px(THUMB_MONITOR.0 - i as f32 * 4.0)).h(px(THUMB_MONITOR.1)).rounded_full().bg(lp.on_surface.with_alpha(ALPHA_50).hsla())
                })),
        );
    }
    if let Some(image) = decoration_image(look, lp, cfg) {
        root = root.child(img(image).absolute().top_0().right(px(THUMB_INSET_PX)).w(px(THUMB_DECOR_STRIP_W_PX)).h(px(THUMB_H_PX)).object_fit(gpui::ObjectFit::Fill));
    }
    if look.now_playing.is_some() {
        root = root.child(div().absolute().left(px(left0)).bottom(px(THUMB_INSET_PX)).w(px(THUMB_CARD.0)).h(px(THUMB_CARD.1)).rounded(px(RADIUS_6_PX)).bg(lp.surface_container.hsla()).p(px(SPACE_3_PX)).row().gap(px(SPACE_3_PX)).child(div().size(px(THUMB_CARD.1 - SPACE_3_PX * 2.0)).rounded(px(RADIUS_3_PX)).bg(accent)));
    }
    if look.calendar.is_some() {
        root = root.child(div().absolute().right(px(THUMB_INSET_PX)).top(px(THUMB_INSET_PX * 2.0)).size(px(THUMB_CALENDAR_PX)).rounded(px(RADIUS_6_PX)).bg(lp.surface_container.hsla()).p(px(SPACE_4_PX)).child(div().size(px(SPACE_6_PX)).rounded_full().bg(accent)));
    }
    let bars = |n: usize, h: f32| (0..n).map(move |i| WAVE_SHAPE[i % WAVE_SHAPE.len()] * h);
    root = match look.visualizer {
        LookVisualizer::None => root,
        LookVisualizer::Wave => root.child(div().absolute().left_0().right_0().bottom_0().h(px(THUMB_WAVE_H_PX)).ltr_row().items_end().children(bars(THUMB_WAVE_BARS, THUMB_WAVE_H_PX).map(|h| {
                    div().flex_1().h(px(h)).bg(lp.primary.with_alpha(ALPHA_75).hsla())
                }))),
        LookVisualizer::Mirrored => root.child(div().absolute().left(px(left0)).right(px(THUMB_FRAME_PX * 2.0)).bottom(px(THUMB_FRAME_PX * 2.0)).h(px(THUMB_WAVE_H_PX)).ltr_row().items_center().gap(px(1.0)).children(bars(THUMB_WAVE_BARS, THUMB_WAVE_H_PX).map(|h| div().flex_1().h(px(h)).rounded_full().bg(accent)))),
        LookVisualizer::Framed => root.child(
            div().absolute().left(px(left0)).bottom(px(THUMB_INSET_PX)).w(px(THUMB_BOX.0)).h(px(THUMB_BOX.1)).rounded(px(RADIUS_4_PX)).border_1().border_color(lp.on_surface.with_alpha(ALPHA_30).hsla()).p(px(SPACE_3_PX)).ltr_row().items_end().gap(px(1.0)).children(bars(8, THUMB_BOX.1 - SPACE_3_PX * 2.0).map(|h| div().flex_1().h(px(h)).bg(accent))),
        ),
    };
    root.into_any_element()
}

fn apply_look(look: &'static LookDef, cx: &mut gpui::App) {
    let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
    if let Some(before) = looks::before_snapshot(&cfg, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string())
        && let Err(err) = mandara_config::profiles::save(&before)
    {
        tracing::warn!("looks: saving your setup failed: {err:#}");
        return;
    }
    let next = look.apply(&cfg, LookOptions { wallpaper_colors: cfg.looks.wallpaper_colors });
    update_config(cx, move |c| *c = next);
}

fn restore_look(cx: &mut gpui::App) {
    let Some(before) = mandara_config::profiles::find(looks::BEFORE_PROFILE) else { return;
    };
    let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
    match looks::restore(&cfg, &before) {
        Ok(next) => update_config(cx, move |c| *c = next),
        Err(err) => tracing::warn!("looks: restore failed: {err:#}"),
    }
}

/// The Themes page's tabs: its colours and style, the components'
/// designs, the cursor, icon and sound theme galleries, and the boot menu
/// and splash (one page, its tabs; Studio keeps its pages).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ThemesTab {
    #[default]
    Colors,
    Components,
    Cursors,
    Icons,
    Boot,
    /// The sound themes plugin (`sounds_page.rs`).
    Sounds,
}

impl ThemesTab {
    pub const ALL: [ThemesTab; 6] = [ThemesTab::Colors, ThemesTab::Components, ThemesTab::Cursors, ThemesTab::Icons, ThemesTab::Boot, ThemesTab::Sounds];

    /// Deep-link name (`mandara studio themes/components`).
    pub fn slug(self) -> &'static str {
        match self {
            ThemesTab::Colors => "colors",
            ThemesTab::Components => "components",
            ThemesTab::Cursors => "cursors",
            ThemesTab::Icons => "icons",
            ThemesTab::Boot => "boot",
            ThemesTab::Sounds => "sounds",
        }
    }

    /// The tab a deep link names (its slug, or a name it is known by).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.slug() == s).or(match s.as_str() {
            "colours" | "looks" | "style" | "themes" => Some(ThemesTab::Colors),
            "component" | "designs" | "presets" | "controls" | "sliders" | "buttons" => {
                    Some(ThemesTab::Components)
                }
                "cursor" | "cursor-themes" | "pointer" | "pointers" => Some(ThemesTab::Cursors),
            "icon" | "icon-themes" | "icon-theme" | "icon-sets" => Some(ThemesTab::Icons),
            "boot-look" | "boot-menu" | "bootloader" | "splash" | "plymouth" | "grub" | "limine" | "refind" => Some(ThemesTab::Boot),
            "sound" | "sound-themes" | "sound-theme" | "audio-theme" => Some(ThemesTab::Sounds),
            _ => None,
        })
    }

    pub fn title(self) -> &'static str {
        match self {
            ThemesTab::Colors => tk("Colors & style"),
            ThemesTab::Components => tk("Components"),
            ThemesTab::Cursors => tk("Cursors"),
            ThemesTab::Icons => tk("Icons"),
            ThemesTab::Boot => tk("Boot"),
            ThemesTab::Sounds => tk("Sounds"),
        }
    }
}

impl super::page_tabs::PageTab for ThemesTab {
    fn slug(self) -> &'static str { ThemesTab::slug(self) }
    fn label(self) -> String { tr(self.title()) }
    fn icon(self) -> &'static str {
        match self {
            ThemesTab::Colors => "palette",
            ThemesTab::Components => "grid",
            ThemesTab::Cursors => "pointer",
            ThemesTab::Icons => "tiles",
            ThemesTab::Boot => "power",
            ThemesTab::Sounds => "volume-high",
        }
    }
}

impl StudioView {
    /// Opens Themes on `tab`, its content at the top.
    pub fn show_themes_tab(&mut self, tab: ThemesTab, cx: &mut Context<Self>) {
        self.show(Section::Themes, cx);
        if self.themes_tab != tab {
            // The sound gallery's lists, fields and cover go with its tab.
            if self.themes_tab == ThemesTab::Sounds {
                cx.defer(super::sounds_page::release);
            }
            self.themes_tab = tab;
            self.app_templates = Default::default();
            // The other tab's previews go: nothing of them stays alive.
            self.components = Default::default();
            cx.defer(super::theme_galleries::release);
            if self.themes_tab != ThemesTab::Boot {
                cx.defer(super::boot_page::release);
            }
            self.content_scroll.set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
        }
        cx.notify();
    }

    /// The Themes page: its tabs, then the selected one ([`Self::themes_colors`]
    /// or the Components designs).
    pub(super) fn themes_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.themes_tab;
        let tabs = super::page_tabs::bar("themes-tabs", &ThemesTab::ALL, tab, p, |view, tab, _, cx| view.show_themes_tab(tab, cx), window, cx);
        let body = match tab {
            ThemesTab::Colors => self.themes_colors(cfg, p, cx),
            ThemesTab::Components => self.components_section(cfg, p, cx),
            ThemesTab::Cursors => self.cursor_section(cfg, p, window, cx),
            ThemesTab::Icons => self.icons_section(cfg, p, window, cx),
            ThemesTab::Boot => self.boot_tab(cfg, p, window, cx),
            ThemesTab::Sounds => self.sounds_tab(cfg, p, window, cx),
        };
        div().row().flex_col().gap(px(SPACE_22_PX)).child(tabs).child(body).into_any_element()
    }

    /// Themes → Colors & style: the live preview (with light / dark), looks,
    /// the designed themes, the wallpaper's colors and schemes, then
    /// tuning, style, icons and
    /// app themes. Rows the galleries already show (light / dark, the
    /// wallpaper scheme) stay out of the list but remain searchable.
    fn themes_colors(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let preview = self.theme_preview(cfg, p, cx);
        let looks = self.looks_gallery(cfg, p, display.clone(), cx);
        let head = super::fold::FoldHead { title: t("Designed themes").into(), preview: t("Hand-made palettes, Mandara's own and community favourites").to_string(), badge: None, trailing: None, closed: true };
        let themes = self.fold("Designed themes", head, p, cx, |this, cx| {
            this.designed_themes(cfg, p, cx)
        });
        let wallpaper = self.wallpaper_schemes(cfg, p, cx);
        let apps = self.app_themes_section(cfg, p, cx);
        let tune_head = section_head(t("Fine-tune"), t("Schedule, tuning, style and icons"), Vec::new(), display, p);
        // Cursor rows sit in the Cursor section, component designs in
        // theirs, app themes in their grid.
        let rows = self.rows(registry::in_section(Section::Themes).filter(|s| {
                !SHOWN_ABOVE.contains(&s.path) && s.group != CURSOR && s.group != APP_THEMES_GROUP && s.group != COMPONENTS
            }), cfg, p, cx);
        let tune = div().row().flex_col().gap(px(SPACE_14_PX)).child(tune_head).child(rows);
        div().row().flex_col().gap(px(PAGE_SECTION_GAP_PX)).child(preview).child(looks).child(themes).child(wallpaper).child(apps).child(tune).into_any_element()
    }

    /// Looks as a compact list (swatch, name in the look's face, its line,
    /// a hover preview of its mock), or with "Show previews" the large
    /// cards.
    fn looks_list(&self, cfg: &ShellConfig, p: &MandaraPalette, yours: &[UserLook]) -> AnyElement {
        let active = cfg.looks.active.as_str();
        let keep = cfg.looks.keep_components;
        let snapshot = std::sync::Arc::new(cfg.clone());
        let own = yours.iter().enumerate().filter_map(|(i, mine)| {
            let parent = mine.parent()?;
            let view = user_view(mine, parent);
            let lp = user_palette(mine);
            let (cfg, tip_palette, apply) = (snapshot.clone(), lp.clone(), mine.clone());
            let line = format!("{} · {}", t("Yours"), tf("From {look}", &[("look", &parent.name)]));
            Some(
                swatch_list::row(("look-row-yours", i), &lp, mine.name.clone().into(), Some(parent.display_font.into()), line, mine.id == active, p)
                    .tooltip(swatch_list::preview_tip(p, move || {
                    div().w(px(swatch_list::TIP_MOCK_W_PX)).child(thumbnail(&view, &tip_palette, &cfg)).into_any_element()
                }))
                    .on_click(move |_: &ClickEvent, _, cx| super::looks_yours::apply(&apply, cx))
                    .into_any_element(),
            )
        });
        let rows = LOOKS.iter().enumerate().map(|(i, look)| {
            let lp = look_palette(i);
            let cfg = snapshot.clone();
            let tip_palette = lp.clone();
            // Its line says which component designs it brings along.
            let line = if keep { blurb(look) } else { format!("{} · {}", components_line(look), blurb(look)) };
            swatch_list::row(("look-row", i), &lp, look.name.into(), Some(look.display_font.into()), line, look.id == active, p)
                .tooltip(swatch_list::preview_tip(p, move || {
                div().w(px(swatch_list::TIP_MOCK_W_PX)).child(thumbnail(look, &tip_palette, &cfg)).into_any_element()
            }))
                .on_click(move |_: &ClickEvent, _, cx| apply_look(look, cx))
                .into_any_element()
        });
        let rows: Vec<AnyElement> = rows.chain(own).collect();
        super::grid::fit(self.page.column_w, swatch_list::SWATCH_GRID).balanced(rows.len()).render(rows)
    }

    fn looks_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, display: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let previews = !self.ui.looks_compact;
        let toggle = swatch_list::previews_chip("looks-previews", previews, p)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.ui.looks_compact = !this.ui.looks_compact;
                crate::ui_state::save(&this.ui);
                cx.notify();
            }))
            .into_any_element();
        let yours = super::looks_yours::looks(cx).unwrap_or_else(|| Rc::from(Vec::new()));
        if !previews {
            let list = self.looks_list(cfg, p, &yours);
            return self.looks_section(cfg, p, display, list, toggle, cx);
        }
        let active = cfg.looks.active.as_str();
        let mut tiles = Vec::with_capacity(LOOKS.len() + yours.len());
        for (i, look) in LOOKS.iter().enumerate() {
            let on = look.id == active;
            // Off screen, a card is its frame and words only: its palette,
            // mock, decoration and display face come when it scrolls in.
            let key = look.id;
            let lp = shown(key).then(|| look_palette(i));
            let dot = |c: mandara_theme::Color| div().size(px(LOOK_DOT_PX)).rounded_full().bg(c.hsla());
            // The name in the look's own display face, its palette and, when
            // applied, a badge.
            let title = div()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .child(div().flex_1().min_w(px(0.0)).truncate().text_start().when(lp.is_some(), |el| el.font_family(look.display_font)).font_weight(FontWeight::SEMIBOLD).text_size(px(TEXT_15_PX)).child(look.name))
                .when(on, |el| el.child(badge(t("Active"), p.primary)))
                .when_some(lp.as_ref(), |el, lp| {
                    el.child(div().row().gap(px(SPACE_3_PX)).child(dot(lp.primary)).child(dot(lp.secondary)).child(dot(lp.tertiary)))
                })
                .child(Self::look_actions(look.id, false, p, cx));
            tiles.push(
                div()
                    .id(("look", i))
                    .flex_1()
                    .relative()
                    .rounded(px(RADIUS_22_PX))
                    .p(px(SPACE_8_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .cursor_pointer()
                    .bg(if on { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container.hsla() })
                    .border_2()
                    .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                    .child(match &lp {
                        Some(lp) => thumbnail(look, lp, cfg),
                        None => placeholder(p),
                    })
                    .child(visibility_probe(key.to_string()))
                    .child(
                        div()
                            .px(px(SPACE_6_PX))
                            .pb(px(SPACE_6_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_4_PX))
                            .child(title)
                            .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(blurb(look))))
                            // The component designs it brings (not while you keep yours).
                            .when(!cfg.looks.keep_components, |el| {
                                el.child(div().row().child(badge(components_line(look), p.secondary).gap(px(SPACE_4_PX)).child(icon("grid", ICON_11_PX, p.secondary))))
                            }),
                    )
                    .on_click(move |_: &ClickEvent, _, cx| apply_look(look, cx))
                    .into_any_element(),
            );
        }
        for (i, mine) in yours.iter().enumerate() {
            let Some(parent) = mine.parent() else { continue;
            };
            let view = user_view(mine, parent);
            let look = &view;
            let on = mine.id == active;
            let key = format!("yours:{}", mine.id);
            let lp = shown(&key).then(|| user_palette(mine));
            let dot = |c: mandara_theme::Color| div().size(px(LOOK_DOT_PX)).rounded_full().bg(c.hsla());
            let title = div()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .child(div().flex_1().min_w(px(0.0)).truncate().text_start().when(lp.is_some(), |el| el.font_family(look.display_font)).font_weight(FontWeight::SEMIBOLD).text_size(px(TEXT_15_PX)).child(mine.name.clone()))
                .when(on, |el| el.child(badge(t("Active"), p.primary)))
                .when_some(lp.as_ref(), |el, lp| {
                    el.child(div().row().gap(px(SPACE_3_PX)).child(dot(lp.primary)).child(dot(lp.secondary)).child(dot(lp.tertiary)))
                });
            // Its badges, then its actions, on a line of their own (the
            // name keeps the title line).
            let meta = div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).child(super::looks_yours::yours_badges(mine, p))).child(Self::look_actions(&mine.id, true, p, cx));
            let rename = self.look_rename_row(&mine.id, p, cx);
            let apply = mine.clone();
            tiles.push(
                div()
                    .id(("look-yours", i))
                    .flex_1()
                    .relative()
                    .rounded(px(RADIUS_22_PX))
                    .p(px(SPACE_8_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .cursor_pointer()
                    .bg(if on { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container.hsla() })
                    .border_2()
                    .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                    .child(match &lp {
                        Some(lp) => thumbnail(look, lp, cfg),
                        None => placeholder(p),
                    })
                    .child(visibility_probe(key))
                    .child(div().px(px(SPACE_6_PX)).pb(px(SPACE_6_PX)).row().flex_col().gap(px(SPACE_4_PX)).child(title).children(rename).child(meta))
                    .on_click(move |_: &ClickEvent, _, cx| super::looks_yours::apply(&apply, cx))
                    .into_any_element(),
            );
        }
        let grid = super::grid::fit(self.page.column_w, LOOK_GRID).balanced(tiles.len()).render(tiles);
        self.looks_section(cfg, p, display, grid, toggle, cx)
    }

    /// The Looks heading (options, restore, the previews switch) over `body`.
    fn looks_section(&self, cfg: &ShellConfig, p: &MandaraPalette, display: SharedString, body: AnyElement, toggle: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let active = cfg.looks.active.as_str();
        let wallpaper_colors = cfg.looks.wallpaper_colors;
        let has_before = !active.is_empty() && mandara_config::profiles::find(looks::BEFORE_PROFILE).is_some();
        let mut options = vec![chip("looks-wallpaper-colors", t("Use my wallpaper's colors"), wallpaper_colors, p)
            .child(icon(if wallpaper_colors { "check" } else { "image" }, ICON_12_PX, if wallpaper_colors { p.on_primary } else { p.primary }))
            .on_click(move |_, _, cx| {
                update_config(cx, move |c| c.looks.wallpaper_colors = !wallpaper_colors)
            })
            .into_any_element()];
        if has_before {
            // A chip like its neighbours: one height for the whole row.
            options.push(chip("looks-restore", t("Restore my look"), false, p).child(icon("undo", ICON_12_PX, p.primary)).on_click(|_, _, cx| restore_look(cx)).into_any_element());
        }
        options.extend(Self::looks_yours_options(cfg, p, cx));
        options.push(toggle);
        let head = section_head(t("Looks"), t("One click sets the bar, its own desktop widgets, colors, fonts and roundness together; your own widgets and bar items stay. Change anything and it is kept as a look of your own. Your setup is saved first, so you can always go back. Looks never change your wallpaper."), options, display, p);
        let note = Self::looks_note(p, cx);
        div().row().flex_col().gap(px(SPACE_14_PX)).child(head).children(note).child(body).into_any_element()
    }

    pub(super) fn desktop_cards_section(&self, cfg: &ShellConfig, p: &MandaraPalette, display: SharedString) -> AnyElement {
        let mut list = div().row().flex_col().gap(px(SPACE_10_PX));
        for (card_id, title, detail) in CARDS {
            let placed = cfg.desktop.widgets.items.iter().find(|w| dc::as_card(w).is_some_and(|c| c.card == card_id));
            let on = placed.is_some_and(|w| w.enabled);
            let anchor = placed.map(|w| w.place.anchor);
            let strip = matches!(card_id, WAVE | MIRRORED_BARS);
            let default_anchor = match card_id {
                CALENDAR => Anchor::TopRight,
                WAVE | MIRRORED_BARS => Anchor::Bottom,
                _ => Anchor::BottomLeft,
            };
            let anchors: &[(Anchor, &str)] = if strip { &STRIP_ANCHORS } else { &CARD_ANCHORS };
            let toggle = chip(SharedString::from(format!("card-{card_id}")), if on { t("On") } else { t("Off") }, on, p).on_click(move |_, _, cx| {
                update_config(cx, move |c| {
                    dc::set_card(&mut c.desktop.widgets, card_id, (!on).then_some(anchor.unwrap_or(default_anchor)))
                });
            });
            let mut chips = div().row().flex_wrap().gap(px(SPACE_6_PX));
            if on {
                for (a, label) in anchors.iter().copied() {
                    chips = chips.child(chip(SharedString::from(format!("card-{card_id}-{a:?}")), tr(label), anchor == Some(a), p).on_click(move |_, _, cx| {
                        update_config(cx, move |c| {
                                dc::set_card(&mut c.desktop.widgets, card_id, Some(a))
                            });
                    }));
                }
            }
            list = list.child(
                div()
                    .p(px(SPACE_12_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(div().row().items_center().gap(px(SPACE_10_PX)).child(div().flex_1().child(heading(tr(title), tr(detail), p))).child(toggle))
                    .when(on, |el| el.child(chips)),
            );
        }
        let side_note = (cfg.bar.position.is_vertical() || cfg.bar.position == BarPosition::Bottom).then(|| {
            div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Cards keep clear of the bar's edge automatically."))
        });
        super::launcher_page::page_section(
            section_head(t("Desktop cards & visualizers"), t("Each one is its own small surface, created only while it shows; visualizers listen only while music plays and stop after a moment of silence."), vec![], display, p),
            [card(p).child(list).children(side_note).into_any_element()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_blurb_is_marked_for_translation() {
        for look in &LOOKS {
            assert_eq!(blurb(look), look.blurb, "{}: the marked text must match the look's", look.id);
        }
    }
}
