//! Studio → Desktop: one page, three tabs (Standing Decision 3 keeps the
//! page count): the widgets on the wallpaper, the cards and visualizers,
//! and the right-click menu (`[desktop.menu]`).
//!
//! The Context menu tab draws the menu with the shell's own painter
//! ([`mandara_ui::desktop_menu::ring::Preview`]): point at an item to see
//! it lit, "Add widget" to see its outer ring, Replay for the bloom. Its
//! settings are registry rows; the items have their own list (switch,
//! order, custom programs). Every change goes through the shared config,
//! and the menu picks it up the next time it opens.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{AnyElement, App, Bounds, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::desktop_menu::{CustomError, DesktopMenuConfig, MenuItem, MenuItemId, MenuStyle, ranges};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::desktop_menu::ring::{Preview, Sel};
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{button, card, tip};
use super::launcher_page::page_section;
use super::themes_page::section_head;
use crate::registry::{self, Section, context_menu_settings::CONTEXT_MENU};
use crate::tokens::*;

/// The Desktop page's tabs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DesktopTab {
    #[default]
    Widgets,
    Cards,
    ContextMenu,
}

impl DesktopTab {
    pub const ALL: [DesktopTab; 3] = [DesktopTab::Widgets, DesktopTab::Cards, DesktopTab::ContextMenu];

    /// Deep-link name (`mandara studio desktop/context-menu`).
    pub fn slug(self) -> &'static str {
        match self {
            DesktopTab::Widgets => "widgets",
            DesktopTab::Cards => "cards",
            DesktopTab::ContextMenu => "context-menu",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.slug() == s).or(match s.as_str() {
            "widget" | "desktop-widgets" => Some(DesktopTab::Widgets),
            "card" | "cards-and-visualizers" | "visualizers" | "visualizer" => Some(DesktopTab::Cards),
            "menu" | "context" | "right-click" | "ring" | "ring-menu" => Some(DesktopTab::ContextMenu),
            _ => None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            DesktopTab::Widgets => t("Widgets"),
            DesktopTab::Cards => t("Cards & visualizers"),
            DesktopTab::ContextMenu => t("Context menu"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            DesktopTab::Widgets => "clock",
            DesktopTab::Cards => "waves",
            DesktopTab::ContextMenu => "pointer",
        }
    }
}

impl super::page_tabs::PageTab for DesktopTab {
    fn slug(self) -> &'static str {
        DesktopTab::slug(self)
    }
    fn label(self) -> String {
        DesktopTab::label(self).to_string()
    }
    fn icon(self) -> &'static str {
        DesktopTab::icon(self)
    }
}

/// Icons offered for a custom item (none of them the AI sparkles).
const CUSTOM_ICONS: [&str; 16] = ["terminal", "folder", "globe", "music", "camera", "note", "book", "package", "download", "search", "home", "git-branch", "heart", "message", "lightbulb", "bolt"];
/// Text inputs of the "Add an item" form.
const LABEL_INPUT: &str = "desktop-menu-custom-label";
const COMMAND_INPUT: &str = "desktop-menu-custom-command";
/// The preview's widest box.
const PREVIEW_MAX_W_PX: f32 = 680.0;
/// The preview's backdrop: OKLCH lightness of its two ends on dark and on
/// light themes, and their chroma against the accents'.
const BACKDROP_DARK_L: (f32, f32) = (0.36, 0.24);
const BACKDROP_LIGHT_L: (f32, f32) = (0.74, 0.6);
const BACKDROP_CHROMA: f32 = 0.7;

/// What the preview was built for: rebuilt when any of it changes.
#[derive(Clone, PartialEq)]
struct PreviewKey {
    items: Vec<MenuItem>,
    /// The rest of `[desktop.menu]` (its `items` left empty).
    rest: DesktopMenuConfig,
    widgets: (usize, bool),
    slideshow: (bool, bool),
    terminal: bool,
    lang: mandara_i18n::Lang,
    width: u32,
}

impl PreviewKey {
    fn of(cfg: &ShellConfig, width: f32) -> Self {
        Self { items: cfg.desktop.menu.items.clone(), ..Self::cheap(cfg, width) }
    }

    /// Everything but the items (nothing allocated).
    fn cheap(cfg: &ShellConfig, width: f32) -> Self {
        let s = &cfg.wallpaper.slideshow;
        Self {
            items: Vec::new(),
            rest: DesktopMenuConfig { items: Vec::new(), ..cfg.desktop.menu },
            widgets: (cfg.desktop.widgets.items.iter().filter(|w| w.enabled).count(), cfg.desktop.widgets.hidden),
            slideshow: (s.enabled, s.directory.trim().is_empty()),
            terminal: cfg.apps.terminal_command().is_some(),
            lang: mandara_i18n::language(),
            width: width.round() as u32,
        }
    }

    /// Whether the preview built for this key still shows `cfg`, compared
    /// in place (a render allocates nothing while nothing changed).
    fn holds(&self, cfg: &ShellConfig, width: f32) -> bool {
        let now = Self::cheap(cfg, width);
        self.items == cfg.desktop.menu.items && (&self.rest, self.widgets, self.slideshow, self.terminal, self.lang, self.width) == (&now.rest, now.widgets, now.slideshow, now.terminal, now.lang, now.width)
    }
}

/// The Context menu tab's state (a global, dropped on leaving the tab).
#[derive(Default)]
struct MenuTab {
    preview: Option<(PreviewKey, Preview)>,
    /// The classic list's rows, for its preview.
    rows: Vec<mandara_ui::desktop_menu::Entry>,
    sel: Option<Sel>,
    /// The classic preview's hovered row.
    row: Option<usize>,
    replay: Option<Instant>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    icon: Option<&'static str>,
    note: Option<(String, bool)>,
}
impl Global for MenuTab {}

fn st(cx: &mut App) -> &mut MenuTab {
    cx.default_global::<MenuTab>()
}

/// Leaving the tab (or the page): the preview's items and pictures go.
pub(super) fn release(cx: &mut App) {
    if cx.has_global::<MenuTab>() {
        cx.remove_global::<MenuTab>();
    }
}

fn custom_error(e: CustomError) -> &'static str {
    match e {
        CustomError::NoLabel => t("Give the item a name"),
        CustomError::LongLabel => t("The name is too long"),
        CustomError::NoCommand => t("Type the program to run"),
        CustomError::TooManyArgs => t("The command has too many arguments"),
        CustomError::LongArg => t("An argument is too long"),
        CustomError::Nul => t("The command holds a character no program accepts"),
        CustomError::Unclosed => t("A quote isn't closed"),
    }
}

/// When a built-in item shows (`None`: always).
fn shows_when(id: MenuItemId) -> Option<&'static str> {
    match id {
        MenuItemId::ToggleWidgets => Some(tk("Shows when the desktop has widgets")),
        MenuItemId::NextWallpaper => Some(tk("Shows with a slideshow or its folder")),
        MenuItemId::Terminal => Some(tk("Needs a terminal (Studio → Applications)")),
        MenuItemId::AddWidget => Some(tk("Opens the widgets as an outer ring")),
        _ => None,
    }
}

impl StudioView {
    /// Opens the Desktop page on `tab` (deep links).
    pub fn show_desktop_tab(&mut self, tab: DesktopTab, cx: &mut Context<Self>) {
        self.show(Section::Desktop, cx);
        self.set_desktop_tab(tab, cx);
    }

    pub(super) fn set_desktop_tab(&mut self, tab: DesktopTab, cx: &mut Context<Self>) {
        if self.desktop_tab != tab {
            if self.desktop_tab == DesktopTab::ContextMenu {
                cx.defer(release);
            }
            self.desktop_tab = tab;
            self.content_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        }
        cx.notify();
    }

    /// The whole Desktop page: the tabs, then the shown tab.
    pub(super) fn desktop_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.desktop_tab;
        let bar = super::page_tabs::bar("desktop-tabs", &DesktopTab::ALL, tab, p, |view, tab, _, cx| view.set_desktop_tab(tab, cx), window, cx);
        let body = match tab {
            DesktopTab::Widgets => self.desktop_widgets_page(cfg, p, window, cx),
            // The interactive cards and GPU visualizers (looks_page.rs).
            DesktopTab::Cards => self.desktop_cards_section(cfg, p, mandara_ui::fonts::fonts(cx).display.clone()),
            DesktopTab::ContextMenu => self.context_menu_tab(cfg, p, window, cx),
        };
        div().row().flex_col().gap(px(SPACE_22_PX)).child(bar).child(body).into_any_element()
    }

    // ── Context menu ─────────────────────────────────────────────────────

    fn context_menu_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let preview = div().row().justify_center().child(self.context_menu_preview(cfg, p, window, cx));
        let replay = button("desktop-menu-replay", "play", t("Replay"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            st(cx).replay = Some(Instant::now());
            cx.notify();
        }));
        let try_it = div().row().items_center().gap(px(SPACE_8_PX)).child(icon("pointer", ICON_12_PX, p.on_surface_variant)).child(
            div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Right-click empty desktop to open it; changes apply the next time it opens."))),
        );
        let hero = page_section(
            section_head(t("Context menu"), t("The menu a right-click on empty desktop opens: a ring of icons around the pointer, or the classic list."), vec![replay.into_any_element()], display.clone(), p),
            [card(p).child(preview).child(try_it).into_any_element()],
        );
        let rows = self.rows(registry::in_section(Section::Desktop).filter(|s| s.group == CONTEXT_MENU), cfg, p, cx);
        let items = self.menu_items_editor(cfg, p, cx);
        div().row().flex_col().gap(px(PAGE_SECTION_GAP_PX)).child(hero).child(rows).child(items).into_any_element()
    }

    /// The live preview: the ring (or the list) as the shell draws it.
    fn context_menu_preview(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let width = (self.page.column_w - 2.0 * SPACE_18_PX).clamp(DIM_320_PX, PREVIEW_MAX_W_PX);
        if st(cx).preview.as_ref().is_none_or(|(k, _)| !k.holds(cfg, width)) {
            let key = PreviewKey::of(cfg, width);
            let entries = mandara_ui::desktop_menu::desktop_entries(cfg, "");
            let outer = mandara_ui::desktop_menu::add_tree(cx);
            let preview = Preview::new(&cfg.desktop.menu, entries.clone(), outer, width, mandara_i18n::language().is_rtl());
            let s = st(cx);
            s.rows = entries;
            s.preview = Some((key, preview));
            s.sel = None;
            s.row = None;
        }
        // A wallpaper-like backdrop in the theme's colours, so the glass
        // reads as it will on the desktop.
        let (l0, l1) = if p.is_dark { BACKDROP_DARK_L } else { BACKDROP_LIGHT_L };
        let backdrop = mandara_core::oklab_gradient(mandara_ui::plugins::panel_fx::ANGLE_DIAGONAL_DEG, p.primary.with_lightness(l0).scale_chroma(BACKDROP_CHROMA), p.tertiary.with_lightness(l1).scale_chroma(BACKDROP_CHROMA));
        let bounds = st(cx).bounds.clone();
        let track = canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full();
        if cfg.desktop.menu.style == MenuStyle::Classic {
            let rows = st(cx).rows.clone();
            let hover = st(cx).row;
            let on_pick: mandara_ui::desktop_menu::OnRow<Self> = Rc::new(|_, _, _, _| {});
            let on_hover: mandara_ui::desktop_menu::OnHover<Self> = Rc::new(|_, i, cx| {
                if st(cx).row != Some(i) {
                    st(cx).row = Some(i);
                    cx.notify();
                }
            });
            let h = mandara_ui::desktop_menu::height(&rows);
            let list = mandara_ui::desktop_menu::card("desktop-menu-preview-row", &rows, hover, mandara_ui::desktop_menu::MENU_W, 1.0, p, cx, on_pick, on_hover);
            return div()
                .id("desktop-menu-preview")
                .relative()
                .w(px(width))
                .h(px(h + 2.0 * SPACE_26_PX))
                .rounded(px(RADIUS_16_PX))
                .overflow_hidden()
                .bg(backdrop)
                .row()
                .items_center()
                .justify_center()
                .child(track)
                .child(list)
                .into_any_element();
        }
        let (sel, replay) = (st(cx).sel.unwrap_or(Sel::None), st(cx).replay);
        let t = replay.map_or(f32::INFINITY, |at| at.elapsed().as_secs_f32() * mandara_core::spring::time_scale());
        // Out of the global while it draws (drawing reads the app).
        let Some((k, mut preview)) = st(cx).preview.take() else { return div().into_any_element() };
        let (size, animating) = (preview.size(), preview.animating(t));
        let ring = preview.element(sel, t, p, cx);
        st(cx).preview = Some((k, preview));
        if animating {
            window.request_animation_frame();
        } else if replay.is_some() {
            st(cx).replay = None;
        }
        div()
            .id("desktop-menu-preview")
            .relative()
            .w(px(size.0))
            .h(px(size.1))
            .rounded(px(RADIUS_16_PX))
            .overflow_hidden()
            .bg(backdrop)
            .child(track)
            .child(ring)
            .on_mouse_move(cx.listener(|_, e: &MouseMoveEvent, _, cx| {
                let b = st(cx).bounds.get();
                let at = (f32::from(e.position.x - b.origin.x), f32::from(e.position.y - b.origin.y));
                let s = st(cx);
                let cur = s.sel.unwrap_or(Sel::None);
                let Some((_, preview)) = s.preview.as_mut() else { return };
                let next = preview.hit(at, cur);
                if next != cur {
                    s.sel = Some(next);
                    cx.notify();
                }
            }))
            // Leaving the box puts the light out (a re-render reports a
            // brief leave too: only a pointer really outside counts).
            .on_hover(cx.listener(|_, hovered: &bool, window, cx| {
                let outside = !st(cx).bounds.get().contains(&window.mouse_position());
                if !*hovered && outside && st(cx).sel.is_some_and(|s| s != Sel::None) {
                    st(cx).sel = None;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// The items: on / off, their order, and programs of your own.
    fn menu_items_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let items = cfg.desktop.menu.items();
        let n = items.len();
        let mut list = div().row().flex_col().gap(px(SPACE_6_PX));
        for (i, item) in items.iter().enumerate() {
            let (glyph, label) = mandara_ui::desktop_menu::item_face(item);
            let on = item.enabled;
            let arrow = |id: String, g: &'static str, enabled: bool, up: bool, cx: &mut Context<Self>| {
                div()
                    .id(SharedString::from(id))
                    .size(px(DIM_28_PX))
                    .flex_none()
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .when(enabled, |el| el.cursor_pointer().hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
                    .when(!enabled, |el| el.opacity(ALPHA_30))
                    .child(icon(g, ICON_14_PX, p.on_surface_variant))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        if enabled {
                            update_config(cx, move |c| {
                                c.desktop.menu.move_item(i, up);
                            });
                        }
                    }))
            };
            let up = arrow(format!("desktop-menu-{i}-up"), "arrow-up", i > 0, true, cx);
            let down = arrow(format!("desktop-menu-{i}-down"), "arrow-down", i + 1 < n, false, cx);
            let custom = item.id == MenuItemId::Custom;
            let detail = if custom { Some(item.command_line()) } else { shows_when(item.id).map(mandara_i18n::tr) };
            let row = div()
                .p(px(SPACE_10_PX))
                .rounded(px(RADIUS_14_PX))
                .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .when(!on, |el| el.opacity(ALPHA_70))
                .child(up)
                .child(down)
                .child(div().size(px(DIM_30_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(p.primary.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_15_PX, p.primary)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_3_PX))
                        .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).truncate().child(label))
                        .children(detail.map(|d| {
                            let mono = custom.then(|| mandara_ui::fonts::fonts(cx).mono.clone());
                            div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().when_some(mono, |el, f| el.font_family(f)).child(d)
                        })),
                )
                .child(div().id(SharedString::from(format!("desktop-menu-{i}-on"))).cursor_pointer().child(mandara_ui::plugins::network::switch(on, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_config(cx, move |c| c.desktop.menu.set_enabled(i, !on));
                })))
                .when(custom, |el| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("desktop-menu-{i}-del")))
                            .size(px(DIM_32_PX))
                            .flex_none()
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .tooltip(tip(t("Remove"), p))
                            .child(icon("trash", ICON_15_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                update_config(cx, move |c| {
                                    c.desktop.menu.remove_custom(i);
                                });
                            })),
                    )
                });
            list = list.child(row);
        }
        let add = self.custom_item_form(cfg, p, cx);
        let reset = button("desktop-menu-reset", "undo", t("Default items"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            update_config(cx, |c| {
                let custom: Vec<MenuItem> = c.desktop.menu.items().into_iter().filter(|i| i.id == MenuItemId::Custom).collect();
                c.desktop.menu.items = DesktopMenuConfig::default().items;
                c.desktop.menu.items.extend(custom);
            });
        }));
        page_section(
            section_head(t("Items"), t("Switch items on or off and put them in order: the first sits at the top of the ring, the rest follow clockwise (the other way in right-to-left languages)."), vec![reset.into_any_element()], mandara_ui::fonts::fonts(cx).display.clone(), p),
            [card(p).child(list).child(add).into_any_element()],
        )
    }

    /// "Add an item": a name, an icon and a program with its arguments.
    fn custom_item_form(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let full = cfg.desktop.menu.custom_count() >= ranges::MAX_CUSTOM;
        let chosen = st(cx).icon.unwrap_or(CUSTOM_ICONS[0]);
        let name = super::form::input(LABEL_INPUT, t("Name, e.g. Files"), false, cx);
        let command = super::form::input(COMMAND_INPUT, t("Program and arguments, e.g. nautilus --new-window"), false, cx);
        let mut icons = div().row().flex_wrap().gap(px(SPACE_4_PX));
        for g in CUSTOM_ICONS {
            let on = g == chosen;
            icons = icons.child(
                div()
                    .id(SharedString::from(format!("desktop-menu-icon-{g}")))
                    .size(px(DIM_32_PX))
                    .rounded(px(RADIUS_10_PX))
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(if on { p.primary.with_alpha(ALPHA_16) } else { p.on_surface.with_alpha(ALPHA_5) }.hsla())
                    .when(on, |el| el.border_1().border_color(p.primary.with_alpha(ALPHA_55).hsla()))
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
                    .child(icon(g, ICON_15_PX, if on { p.primary } else { p.on_surface_variant }))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        st(cx).icon = Some(g);
                        cx.notify();
                    })),
            );
        }
        let add = button("desktop-menu-add", "plus", if full { t("The menu has its most custom items") } else { t("Add item") }, !full, p).when(!full, |el| {
            el.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let (label, line) = (super::form::value(LABEL_INPUT, cx), super::form::value(COMMAND_INPUT, cx));
                let icon = st(cx).icon.unwrap_or(CUSTOM_ICONS[0]);
                match MenuItem::custom(&label, icon, &line) {
                    Ok(item) => {
                        update_config(cx, move |c| {
                            c.desktop.menu.add_custom(item);
                        });
                        super::form::clear(LABEL_INPUT, cx);
                        super::form::clear(COMMAND_INPUT, cx);
                        st(cx).note = Some((tf("Added {name}", &[("name", &label)]), false));
                    }
                    Err(e) => st(cx).note = Some((custom_error(e).to_string(), true)),
                }
                cx.notify();
            }))
        });
        let note = st(cx).note.clone().map(|(text, error)| super::form::note(&text, error, p));
        div()
            .pt(px(SPACE_8_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(super::compositor_page::group_title(t("Add an item of your own"), p))
            .child(div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("It runs the program directly with its arguments, never through a shell. Quote an argument that holds spaces."))))
            .child(super::grid::pair(self.page.column_w, DIM_220_PX, SPACE_8_PX, div().flex_1().child(name), div().flex_1().child(command)))
            .child(icons)
            .children(note)
            .child(div().row().child(add))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_parse_their_slugs_and_aliases() {
        for tab in DesktopTab::ALL {
            assert_eq!(DesktopTab::parse(tab.slug()), Some(tab));
        }
        assert_eq!(DesktopTab::parse("Context-Menu"), Some(DesktopTab::ContextMenu));
        assert_eq!(DesktopTab::parse("right-click"), Some(DesktopTab::ContextMenu));
        assert_eq!(DesktopTab::parse("visualizers"), Some(DesktopTab::Cards));
        assert_eq!(DesktopTab::parse("clock-1"), None, "a widget id is no tab");
    }

    #[test]
    fn the_context_menu_tab_shows_every_menu_setting() {
        let rows: Vec<&str> = registry::in_section(Section::Desktop).filter(|s| s.group == CONTEXT_MENU).map(|s| s.path).collect();
        assert_eq!(rows.len(), 9);
        assert!(rows.iter().all(|p| p.starts_with("/desktop/menu/")));
        // Every custom-item problem has its own words.
        for e in [CustomError::NoLabel, CustomError::LongLabel, CustomError::NoCommand, CustomError::TooManyArgs, CustomError::LongArg, CustomError::Nul, CustomError::Unclosed] {
            assert!(!custom_error(e).is_empty());
        }
        assert!(CUSTOM_ICONS.iter().all(|g| mandara_ui::icons::ICON_NAMES.contains(g) && !g.contains("spark")), "bundled icons, no AI sparkles");
    }
}
