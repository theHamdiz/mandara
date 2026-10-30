//! Studio → Themes: the live preview, the looks gallery, the theme gallery
//! (`themes_page.rs`), the cursor gallery and settings (`cursor_page.rs`)
//! and the color / style settings, plus the desktop cards / GPU
//! visualizers.
//!
//! * **Looks** — six original one-click looks (`mandara_config::looks`):
//!   bar style, desktop widgets and cards, an HCT palette generated from an
//!   OKLCH seed, typography and the radius / opacity scale. Each tile is a
//!   live mock drawn from the look's own palette (a handful of quads, no
//!   images). Applying one from your own setup first saves that setup as
//!   the "Before looks" profile; "Restore my look" brings it back (and the
//!   Profiles page's undo history covers every step).
//! * **Desktop cards & visualizers** — the five card kinds of
//!   `mandara_config::desktop_cards`, each on its own small surface; turn
//!   one on and pick where it sits (shown on Studio → Desktop widgets under the
//!   widget list: they are items of the same shared list).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px};
use mandara_config::desktop_cards::{self as dc, CALENDAR, FRAMED_BARS, MIRRORED_BARS, NOW_PLAYING, WAVE};
use mandara_config::desktop_widgets::{Anchor, ClockFace, Placement, WidgetKind, ranges};
use mandara_config::looks::{self, LOOKS, LookDef, LookOptions, LookVisualizer};
use mandara_config::{BarLayout, BarPosition, ShellConfig};
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::backdrop::paint::FontNames;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::lock::paint::TextPainter;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::{StudioView, swatch_list};
use super::compositor_page::{badge, button, card, chip, group_title, heading};
use super::themes_page::section_head;
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
fn blurb(look: &LookDef) -> String {
    let key = match look.id {
        "corniche-dusk" => tk("Rose evening glass: headline clock, system monitor, media card and glass pills."),
        "pharos-ink" => tk("Monochrome ink: a hairline bar dense with stats, a greeting and a calendar."),
        "montaza-crimson" => tk("Bold palace red: a framed spectrum box, an Arabic clock and calligraphy."),
        "qaitbay-forest" => tk("Deep green: a giant stacked clock behind the subject and a dock media chip."),
        "anfushi-wave" => tk("Open sea: a full-width glowing wave, a katakana accent and one tiny pill."),
        "night-frame" => tk("Midnight violet: a side bar framing the screen, a greeting, calendar and bars."),
        _ => look.blurb,
    };
    tr(key)
}

thread_local! {
    /// Each look's palette (dark), generated once from its seed.
    static PALETTES: RefCell<Vec<MandaraPalette>> = const { RefCell::new(Vec::new()) };
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
    let WidgetKind::Decoration(d) = &w.kind else { return None };
    let size = ((THUMB_DECOR_STRIP_W_PX * THUMB_DECOR_DENSITY) as u32, (THUMB_H_PX * THUMB_DECOR_DENSITY) as u32);
    // The tile's text size on a frame `size.1` tall.
    let scale = THUMB_DECOR_PX * THUMB_DECOR_DENSITY / (d.size.max(1.0) * size.1 as f32 / ranges::REFERENCE_HEIGHT_PX);
    // Same edge as on the desktop, so the sideways turn matches.
    w.place = Placement { anchor: w.place.anchor, scale, opacity: w.place.opacity, ..Placement::default() };
    let t = &cfg.typography;
    let fonts = FontNames { ui: look.ui_font.into(), display: look.display_font.into(), arabic: t.arabic_font.clone(), mono: t.mono_font.clone() };
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

fn look_palette(i: usize) -> MandaraPalette {
    PALETTES.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_empty() {
            *p = LOOKS.iter().map(|l| mandara_theme::material::palette_from_seed(l.seed_color(), l.variant, l.dark)).collect();
        }
        p[i].clone()
    })
}

/// A cheap live mock of a look, drawn with its own palette (its text
/// decoration painted like the desktop's).
fn thumbnail(look: &LookDef, lp: &MandaraPalette, cfg: &ShellConfig) -> AnyElement {
    let bar_fill = lp.surface_container_high.hsla();
    let accent = lp.primary.hsla();
    let side = look.position.is_vertical();
    let mut root = div()
        .relative()
        .h(px(THUMB_H_PX))
        .w_full()
        .rounded(px(RADIUS_14_PX))
        .overflow_hidden()
        .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, lp.surface_dim, lp.surface_dim.mix(lp.primary, SKY_TINT)));
    // The bar.
    let pills = |el: gpui::Div| el.children(THUMB_PILL_W_PX.iter().map(|w| div().w(px(*w)).h(px(THUMB_BAR_PX)).rounded_full().bg(bar_fill)));
    root = root.child(match (side, look.layout) {
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
        (_, BarLayout::Hairline) => div().absolute().top_0().left_0().right_0().h(px(THUMB_HAIRLINE_PX)).bg(bar_fill).border_b_1().border_color(accent),
        (_, BarLayout::Micro) => div().absolute().top(px(THUMB_FRAME_PX * 2.0)).left_0().right_0().row().justify_center().child(div().w(px(THUMB_MICRO_W_PX)).h(px(THUMB_BAR_PX)).rounded_full().bg(bar_fill)),
        (_, BarLayout::FloatingPills) => pills(div().absolute().top(px(THUMB_FRAME_PX * 2.0)).left(px(THUMB_INSET_PX)).right(px(THUMB_INSET_PX)).ltr_row().justify_between()),
        (_, BarLayout::ContiguousIsland) => div().absolute().top(px(THUMB_FRAME_PX * 2.0)).left_0().right_0().row().justify_center().child(div().w(px(THUMB_PILL_W_PX[2] * 2.5)).h(px(THUMB_BAR_PX)).rounded_full().bg(bar_fill)),
        (_, BarLayout::MinimalEdge) => div().absolute().top_0().left_0().right_0().h(px(THUMB_BAR_PX)).bg(bar_fill),
    });
    if look.screen_frame && side {
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
                .children((0..4).map(|i| div().w(px(THUMB_MONITOR.0 - i as f32 * 4.0)).h(px(THUMB_MONITOR.1)).rounded_full().bg(lp.on_surface.with_alpha(ALPHA_50).hsla()))),
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
        LookVisualizer::Wave => root.child(div().absolute().left_0().right_0().bottom_0().h(px(THUMB_WAVE_H_PX)).ltr_row().items_end().children(bars(THUMB_WAVE_BARS, THUMB_WAVE_H_PX).map(|h| div().flex_1().h(px(h)).bg(lp.primary.with_alpha(ALPHA_75).hsla())))),
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
    let Some(before) = mandara_config::profiles::find(looks::BEFORE_PROFILE) else { return };
    let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
    match looks::restore(&cfg, &before) {
        Ok(next) => update_config(cx, move |c| *c = next),
        Err(err) => tracing::warn!("looks: restore failed: {err:#}"),
    }
}

impl StudioView {
    /// The Themes page: the live preview (with light / dark), looks, the
    /// designed themes, the wallpaper's colors and schemes, the cursor
    /// gallery and settings (`cursor_page.rs`), then tuning,
    /// style, icons and app themes. Rows the galleries already show (light /
    /// dark, the wallpaper scheme) stay out of the list but remain searchable.
    pub(super) fn themes_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let preview = self.theme_preview(cfg, p, cx);
        let looks = self.looks_gallery(cfg, p, display.clone(), cx);
        let themes = self.designed_themes(cfg, p, cx);
        let wallpaper = self.wallpaper_schemes(cfg, p, cx);
        let apps = self.app_themes_section(cfg, p, cx);
        let cursor = self.cursor_section(cfg, p, cx);
        let tune_head = section_head(t("Fine-tune"), t("Schedule, tuning, style and icons"), Vec::new(), display, p);
        // Cursor rows sit in the Cursor section, app themes in their grid.
        let rows = self.rows(registry::in_section(Section::Themes).filter(|s| !SHOWN_ABOVE.contains(&s.path) && s.group != CURSOR && s.group != APP_THEMES_GROUP), cfg, p, cx);
        let tune = div().row().flex_col().gap(px(SPACE_14_PX)).child(tune_head).child(rows);
        div().row().flex_col().gap(px(PAGE_SECTION_GAP_PX)).child(preview).child(looks).child(themes).child(wallpaper).child(apps).child(cursor).child(tune).into_any_element()
    }

    /// Looks as a compact list (swatch, name in the look's face, its line,
    /// a hover preview of its mock), or with "Show previews" the large
    /// cards.
    fn looks_list(&self, cfg: &ShellConfig, p: &MandaraPalette) -> AnyElement {
        let active = cfg.looks.active.as_str();
        let snapshot = std::sync::Arc::new(cfg.clone());
        let rows = LOOKS.iter().enumerate().map(|(i, look)| {
            let lp = look_palette(i);
            let cfg = snapshot.clone();
            let tip_palette = lp.clone();
            swatch_list::row(("look-row", i), &lp, look.name.into(), Some(look.display_font.into()), blurb(look), look.id == active, p)
                .tooltip(swatch_list::preview_tip(p, move || div().w(px(swatch_list::TIP_MOCK_W_PX)).child(thumbnail(look, &tip_palette, &cfg)).into_any_element()))
                .on_click(move |_: &ClickEvent, _, cx| apply_look(look, cx))
                .into_any_element()
        });
        let rows: Vec<AnyElement> = rows.collect();
        super::grid::fit(self.page.column_w, swatch_list::SWATCH_GRID).balanced(rows.len()).render(rows)
    }

    fn looks_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, display: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let previews = self.ui.looks_previews;
        let toggle = swatch_list::previews_chip("looks-previews", previews, p)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.ui.looks_previews = !this.ui.looks_previews;
                crate::ui_state::save(&this.ui);
                cx.notify();
            }))
            .into_any_element();
        if !previews {
            let list = self.looks_list(cfg, p);
            return self.looks_section(cfg, p, display, list, toggle);
        }
        let active = cfg.looks.active.as_str();
        let mut tiles = Vec::with_capacity(LOOKS.len());
        for (i, look) in LOOKS.iter().enumerate() {
            let on = look.id == active;
            let lp = look_palette(i);
            let dot = |c: mandara_theme::Color| div().size(px(LOOK_DOT_PX)).rounded_full().bg(c.hsla());
            // The name in the look's own display face, its palette and, when
            // applied, a badge.
            let title = div()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .child(div().flex_1().min_w(px(0.0)).truncate().text_start().font_family(look.display_font).font_weight(FontWeight::SEMIBOLD).text_size(px(TEXT_15_PX)).child(look.name))
                .when(on, |el| el.child(badge(t("Active"), p.primary)))
                .child(div().row().gap(px(SPACE_3_PX)).child(dot(lp.primary)).child(dot(lp.secondary)).child(dot(lp.tertiary)));
            tiles.push(
                div()
                    .id(("look", i))
                    .flex_1()
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
                    .child(thumbnail(look, &lp, cfg))
                    .child(
                        div()
                            .px(px(SPACE_6_PX))
                            .pb(px(SPACE_6_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_4_PX))
                            .child(title)
                            .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(blurb(look)))),
                    )
                    .on_click(move |_: &ClickEvent, _, cx| apply_look(look, cx))
                    .into_any_element(),
            );
        }
        let grid = super::grid::fit(self.page.column_w, LOOK_GRID).balanced(tiles.len()).render(tiles);
        self.looks_section(cfg, p, display, grid, toggle)
    }

    /// The Looks heading (options, restore, the previews switch) over `body`.
    fn looks_section(&self, cfg: &ShellConfig, p: &MandaraPalette, display: SharedString, body: AnyElement, toggle: AnyElement) -> AnyElement {
        let active = cfg.looks.active.as_str();
        let wallpaper_colors = cfg.looks.wallpaper_colors;
        let has_before = !active.is_empty() && mandara_config::profiles::find(looks::BEFORE_PROFILE).is_some();
        let mut options = vec![chip("looks-wallpaper-colors", t("Use my wallpaper's colors"), wallpaper_colors, p)
            .child(icon(if wallpaper_colors { "check" } else { "image" }, ICON_12_PX, if wallpaper_colors { p.on_primary } else { p.primary }))
            .on_click(move |_, _, cx| update_config(cx, move |c| c.looks.wallpaper_colors = !wallpaper_colors))
            .into_any_element()];
        if has_before {
            options.push(button("looks-restore", "undo", t("Restore my look"), false, p).on_click(|_, _, cx| restore_look(cx)).into_any_element());
        }
        options.push(toggle);
        let head = section_head(t("Looks"), t("One click sets the bar, desktop widgets, colors, fonts and roundness together. Your own setup is saved first, so you can always go back. Looks never change your wallpaper."), options, display, p);
        div().row().flex_col().gap(px(SPACE_14_PX)).child(head).child(body).into_any_element()
    }

    pub(super) fn desktop_cards_section(&self, cfg: &ShellConfig, p: &MandaraPalette) -> AnyElement {
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
                update_config(cx, move |c| dc::set_card(&mut c.desktop.widgets, card_id, (!on).then_some(anchor.unwrap_or(default_anchor))));
            });
            let mut chips = div().row().flex_wrap().gap(px(SPACE_6_PX));
            if on {
                for (a, label) in anchors.iter().copied() {
                    chips = chips.child(chip(SharedString::from(format!("card-{card_id}-{a:?}")), tr(label), anchor == Some(a), p).on_click(move |_, _, cx| {
                        update_config(cx, move |c| dc::set_card(&mut c.desktop.widgets, card_id, Some(a)));
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
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Desktop cards & visualizers"), p))
            .child(
                card(p)
                    .child(heading(t("On the desktop, under your windows"), t("Each one is its own small surface, created only while it shows; visualizers listen only while music plays and stop after a moment of silence."), p))
                    .child(list)
                    .children(side_note),
            )
            .into_any_element()
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
