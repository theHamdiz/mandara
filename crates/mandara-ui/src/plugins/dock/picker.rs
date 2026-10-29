//! "Add app…": a searchable picker of installed apps (the launcher's app
//! list — your hidden apps, names and icons applied — fuzzy-matched with
//! `nucleo` like the launcher). Pinned apps show checked; check several
//! and "Keep in Dock" pins them in one go, unchecking a pinned app
//! unpins it. Opened from the dock (right-click empty dock space) as a
//! small overlay; Studio → Dock embeds the same ranking in its own modal.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::layer_shell::KeyboardInteractivity;
use gpui::{App, AppContext as _, Context, Entity, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseDownEvent, ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, Window, WindowHandle, div, prelude::FluentBuilder as _, px};
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tf};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::apps::App as DesktopApp;
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{elevation, readable, rim};
use crate::state::{config, update_config};
use crate::text_field::TextField;

/// Card geometry.
const CARD_W: f32 = 640.0;
const CARD_H: f32 = 540.0;
const CARD_PAD: f32 = 18.0;
const CARD_RADIUS: f32 = 24.0;
const TILE_W: f32 = 112.0;
const TILE_H: f32 = 104.0;
const TILE_ICON: f32 = 48.0;
const TILE_RADIUS: f32 = 14.0;
const CHECK_PX: f32 = 18.0;
const CHECK_INSET: f32 = 6.0;
const GAP: f32 = 10.0;
const FOOTER_H: f32 = 44.0;
const BUTTON_H: f32 = 34.0;
const BUTTON_PAD: f32 = 16.0;
const SEARCH_H: f32 = 40.0;
const SELECTED_ALPHA: f32 = 0.16;
const HOVER_ALPHA: f32 = 0.07;
const NAME_CHARS: usize = 16;
/// Results drawn at once (the list is filtered as you type).
pub const MAX_RESULTS: usize = 120;
const OPEN_SPRING: SpringConfig = SpringConfig::new(420.0, 34.0, 1.0);
/// Dim behind the picker card.
const SCRIM_ALPHA: f32 = 0.28;

/// Installed apps as the launcher lists them.
pub fn installed_apps(cx: &App) -> Vec<DesktopApp> {
    crate::launcher_ext::customize(crate::apps::discover(), &config(cx).read(cx).get().launcher)
}

/// Indices of `apps` matching `query`, best first (all of them, in
/// order, for an empty query). Fuzzy over the name, id and keywords;
/// the description only counts when it contains the query as written
/// (fuzzy matching a long description finds the letters almost anywhere),
/// and those hits come after every name match.
pub fn rank_apps(apps: &[DesktopApp], query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..apps.len()).collect();
    }
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let hay: Vec<Hay> = apps.iter().enumerate().map(|(i, a)| Hay(i, format!("{} {} {}", a.name, a.id, a.keywords))).collect();
    let mut scored: Vec<(usize, u32)> = pattern.match_list(hay.iter(), &mut matcher).into_iter().map(|(h, s)| (h.0, s)).collect();
    // Name hits first, then the score.
    let lower = query.to_lowercase();
    let starts = |i: usize| apps[i].name.to_lowercase().starts_with(&lower);
    scored.sort_by(|a, b| starts(b.0).cmp(&starts(a.0)).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
    let mut out: Vec<usize> = scored.into_iter().map(|(i, _)| i).collect();
    let by_description: Vec<usize> = (0..apps.len()).filter(|i| !out.contains(i) && apps[*i].comment.to_lowercase().contains(&lower)).collect();
    out.extend(by_description);
    out
}

struct Hay(usize, String);
impl AsRef<str> for Hay {
    fn as_ref(&self) -> &str {
        &self.1
    }
}

/// What applying a picker selection does: `(pin, unpin)` desktop ids.
pub fn selection_changes(pinned: &[String], selected: &HashSet<String>, shown: &[DesktopApp]) -> (Vec<String>, Vec<String>) {
    let is_pinned = |id: &str| pinned.iter().any(|p| p.eq_ignore_ascii_case(id));
    let mut pin: Vec<String> = shown.iter().filter(|a| selected.contains(&a.id) && !is_pinned(&a.id)).map(|a| a.id.clone()).collect();
    pin.dedup();
    let unpin = shown.iter().filter(|a| is_pinned(&a.id) && !selected.contains(&a.id)).map(|a| a.id.clone()).collect();
    (pin, unpin)
}

pub struct DockPicker {
    apps: Rc<[DesktopApp]>,
    query: String,
    results: Vec<usize>,
    selected: HashSet<String>,
    search: Entity<TextField>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    sheet: super::sheet::Sheet,
}

impl DockPicker {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let apps: Rc<[DesktopApp]> = installed_apps(cx).into();
        let pinned = config(cx).read(cx).get().dock.pinned.clone();
        let selected = apps.iter().filter(|a| pinned.iter().any(|p| p.eq_ignore_ascii_case(&a.id))).map(|a| a.id.clone()).collect();
        let this = cx.entity().downgrade();
        let search = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search apps"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = this.update(cx, |p, cx| {
                        p.query = value;
                        p.results = rank_apps(&p.apps, &p.query);
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        search.update(cx, |f, cx| f.focus(window, cx));
        let results = rank_apps(&apps, "");
        Self { apps, query: String::new(), results, selected, search, focus: cx.focus_handle(), scroll: ScrollHandle::new(), sheet: super::sheet::Sheet::new(OPEN_SPRING) }
    }

    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.selected.remove(id) {
            self.selected.insert(id.to_string());
        }
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let pinned = config(cx).read(cx).get().dock.pinned.clone();
        let (pin, unpin) = selection_changes(&pinned, &self.selected, &self.apps);
        if !pin.is_empty() || !unpin.is_empty() {
            update_config(cx, move |c| {
                for id in &unpin {
                    c.dock.unpin(id);
                }
                for id in &pin {
                    c.dock.pin(id);
                }
            });
        }
        self.close(cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.sheet.close();
        cx.notify();
    }
}

impl Render for DockPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        let p = self.sheet.step(false, window, cx).clamp(0.0, 1.0);
        let accent = readable(palette.primary, &palette);
        let hover = palette.on_surface.with_alpha(HOVER_ALPHA).hsla();
        let changed = {
            let pinned = &config(cx).read(cx).get().dock.pinned;
            let (a, b) = selection_changes(pinned, &self.selected, &self.apps);
            a.len() + b.len()
        };
        let mut grid = div().flex().flex_row().flex_wrap().gap(px(GAP));
        for &i in self.results.iter().take(MAX_RESULTS) {
            let a = &self.apps[i];
            let checked = self.selected.contains(&a.id);
            let id = a.id.clone();
            grid = grid.child(
                div()
                    .id(gpui::SharedString::from(format!("pick-{}", a.id)))
                    .relative()
                    .w(px(TILE_W))
                    .h(px(TILE_H))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(GAP / 2.0))
                    .rounded(px(crate::style::radius(TILE_RADIUS)))
                    .cursor_pointer()
                    .when(checked, |el| el.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
                    .hover(|s| s.bg(hover))
                    .child(crate::plugins::workspaces::app_icon(a.wm_class.as_deref().unwrap_or(&a.id), TILE_ICON, palette.on_surface, false, &palette))
                    .child(div().w_full().px(px(GAP / 2.0)).text_center().truncate().text_size(px(text::LABEL)).child(crate::kit::ellipsize(&a.name, NAME_CHARS)))
                    .child(
                        div()
                            .absolute()
                            .top(px(CHECK_INSET))
                            .right(px(CHECK_INSET))
                            .size(px(CHECK_PX))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(checked, |el| el.bg(accent.hsla()).child(icon("check", CHECK_PX * 0.7, accent.best_on(mandara_theme::Color::hex("#ffffff"), mandara_theme::Color::hex("#101010")))))
                            .when(!checked, |el| el.border_1().border_color(palette.on_surface.with_alpha(SELECTED_ALPHA).hsla())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx))),
            );
        }
        let empty = self.results.is_empty();
        let (x, y) = ((sw - CARD_W) / 2.0, (sh - CARD_H) / 2.0);
        let mut shadow = elevation(&palette, p);
        shadow.extend(rim(&palette));
        let label = if changed == 0 { t("Done").to_string() } else { tf("Apply {n} changes", &[("n", &changed)]) };
        div()
            .id("picker-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                "escape" => this.close(cx),
                "enter" => {
                    if let Some(id) = this.results.first().map(|&i| this.apps[i].id.clone()) {
                        this.toggle(&id, cx);
                    }
                }
                _ => {}
            }))
            .child(div().id("picker-outside").absolute().inset_0().bg(mandara_theme::Color::hex("#000000").with_alpha(p * SCRIM_ALPHA).hsla()).on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| this.close(cx))))
            .child(
                div()
                    .id("picker-card")
                    .occlude()
                    .absolute()
                    .left(px(x))
                    .top(px(y + (1.0 - p) * GAP))
                    .w(px(CARD_W))
                    .h(px(CARD_H))
                    .opacity(p)
                    .rounded(px(crate::style::radius(CARD_RADIUS)))
                    .bg(crate::shell::frame_color(&palette).hsla())
                    .shadow(shadow)
                    .p(px(CARD_PAD))
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .child(div().flex().items_center().gap(px(GAP)).child(icon("plus", text::BODY, accent)).child(div().font_weight(FontWeight::SEMIBOLD).child(t("Add apps to the Dock"))))
                    .child(div().h(px(SEARCH_H)).child(self.search.clone()))
                    .child(
                        div()
                            .id("picker-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(grid)
                            .when(empty, |el| el.child(div().p(px(GAP)).text_color(palette.on_surface_variant.hsla()).child(tf("No apps match “{query}”", &[("query", &self.query)])))),
                    )
                    .child(
                        div()
                            .h(px(FOOTER_H))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Checked apps stay in the Dock")))
                            .child(
                                div()
                                    .id("picker-apply")
                                    .h(px(BUTTON_H))
                                    .px(px(BUTTON_PAD))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .bg(accent.hsla())
                                    .text_color(accent.best_on(mandara_theme::Color::hex("#ffffff"), mandara_theme::Color::hex("#101010")).hsla())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(text::BODY_S))
                                    .child(label)
                                    .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                            ),
                    ),
            )
    }
}

#[derive(Default)]
struct Picker(Option<WindowHandle<DockPicker>>);
impl Global for Picker {}

/// Opens the picker over `monitor` (closing an open one).
pub fn open(monitor: &str, cx: &mut App) {
    if let Some(h) = cx.try_global::<Picker>().and_then(|m| m.0) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
        super::popover_closed(h.into(), cx);
    }
    let options = super::sheet::popover_options(monitor, KeyboardInteractivity::Exclusive, cx);
    match cx.open_window(options, |window, cx| cx.new(|cx| DockPicker::new(window, cx))) {
        Ok(h) => {
            cx.set_global(Picker(Some(h)));
            super::popover_opened(monitor, h.into(), cx);
        }
        Err(err) => tracing::warn!("dock picker: {err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, keywords: &str) -> DesktopApp {
        DesktopApp { id: id.into(), name: name.into(), keywords: keywords.into(), ..Default::default() }
    }

    #[test]
    fn fuzzy_search_ranks_name_hits_first() {
        let apps = vec![app("org.demo.alpha", "Alpha Editor", "text"), app("org.demo.beta", "Beta Browser", "web internet"), app("org.gnome.TextEditor", "Text Editor", "notepad")];
        assert_eq!(rank_apps(&apps, ""), [0, 1, 2]);
        assert_eq!(rank_apps(&apps, "brow"), [1]);
        assert_eq!(rank_apps(&apps, "internet"), [1], "keywords count");
        let editors = rank_apps(&apps, "edit");
        assert!(editors[..2].contains(&0) && editors[..2].contains(&2), "{editors:?}");
        assert_eq!(rank_apps(&apps, "text")[0], 2, "a name that starts with the query wins");
        assert_eq!(rank_apps(&apps, "bta brw"), [1], "fuzzy");
        assert!(rank_apps(&apps, "zzzz").is_empty());
    }

    #[test]
    fn descriptions_match_only_as_written() {
        let mut gps = app("xgps", "xgps", "");
        gps.comment = "Display data from a GPS daemon with satellite positions".into();
        let mut vnc = app("tigervnc", "TigerVNC", "");
        vnc.comment = "Remote desktop viewer".into();
        let apps = vec![app("org.demo.alpha", "Alpha Editor", ""), gps, vnc];
        // The letters d-e-m-o appear in order in the GPS description; that
        // is not a match. The word "demo" in the id is.
        assert_eq!(rank_apps(&apps, "demo"), [0]);
        // A description phrase still finds its app, after name matches.
        assert_eq!(rank_apps(&apps, "remote desktop"), [2]);
        assert_eq!(rank_apps(&apps, "satellite"), [1]);
    }

    #[test]
    fn applying_a_selection_pins_checked_and_unpins_unchecked() {
        let apps = vec![app("a", "A", ""), app("b", "B", ""), app("c", "C", "")];
        let pinned = vec!["A".to_string(), "x".into()];
        let selected: HashSet<String> = ["b".to_string(), "c".into()].into_iter().collect();
        let (pin, unpin) = selection_changes(&pinned, &selected, &apps);
        assert_eq!(pin, ["b", "c"]);
        assert_eq!(unpin, ["a"], "pins of apps not listed (x) are left alone");
        let same: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert_eq!(selection_changes(&["a".to_string()], &same, &apps), (vec![], vec![]));
    }
}
