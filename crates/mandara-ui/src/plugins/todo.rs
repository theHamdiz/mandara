//! To-do plugin: tasks grouped into lists ("topics") — a sidebar card, a
//! panel and an optional bar counter — with a list switcher (chips), per-list
//! completed folds, drag-or-menu moves between lists and documented
//! commands. Data and rules live in [`crate::todo_lists`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::ToGpui;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use crate::bidi::Bidi as _;
use crate::icons::{any_icon, icon};
use crate::kit::{card, icon_button, reveal, text, tokens};
use crate::productivity::{Productivity, Todo, productivity};
use crate::text_field::TextField;
use crate::todo_lists::{Filter, INBOX_ID, ListColor, OnDelete, SWATCH_HUES, SortOrder, TONES, TodoBook, TodoList};

pub const ID: &str = "todo";

/// Plugin settings (`[plugins.settings.todo]`, Studio → the plugin's modal).
pub const DEFAULT_LIST_KEY: &str = "default_list";
pub const SHOW_COMPLETED_KEY: &str = "show_completed";
pub const SORT_KEY: &str = "sort";
pub const BAR_COUNT_KEY: &str = "bar_count";
pub const DEFAULT_SHOW_COMPLETED: bool = true;

/// Element-id namespaces of one list surface (static: no per-row
/// `format!` on every render).
struct Ids {
    row: &'static str,
    check: &'static str,
    remove: &'static str,
    more: &'static str,
    clear: &'static str,
    chips: &'static str,
    chip: &'static str,
    chip_all: &'static str,
    chip_new: &'static str,
    section: &'static str,
    fold: &'static str,
    menu: &'static str,
    swatch: &'static str,
    move_to: &'static str,
}

const PANEL_IDS: Ids = Ids {
    row: "panel-todo",
    check: "panel-check",
    remove: "panel-rm",
    more: "panel-more",
    clear: "panel-clear-done",
    chips: "panel-chips",
    chip: "panel-chip",
    chip_all: "panel-chip-all",
    chip_new: "panel-chip-new",
    section: "panel-section",
    fold: "panel-fold",
    menu: "panel-menu",
    swatch: "panel-swatch",
    move_to: "panel-move-to",
};
const CARD_IDS: Ids = Ids {
    row: "card-todo",
    check: "card-check",
    remove: "card-rm",
    more: "card-more",
    clear: "card-clear-done",
    chips: "card-chips",
    chip: "card-chip",
    chip_all: "card-chip-all",
    chip_new: "card-chip-new",
    section: "card-section",
    fold: "card-fold",
    menu: "card-menu",
    swatch: "card-swatch",
    move_to: "card-move-to",
};

// Rows.
const ROW_MIN_H_PX: f32 = 34.0;
const ROW_PAD_X_PX: f32 = 6.0;
const ROW_RADIUS_PX: f32 = 10.0;
const ROW_GAP_PX: f32 = 10.0;
const ROW_HOVER_ALPHA: f32 = 0.05;
const CHECK_PX: f32 = 20.0;
const CHECK_ICON_PX: f32 = 13.0;
const TEXT_PAD_Y_PX: f32 = 6.0;
const TEXT_LINE_PX: f32 = 18.0;
const DONE_ALPHA: f32 = 0.7;
const REMOVE_PX: f32 = 24.0;
// List.
const HEAD_ICON_PX: f32 = 16.0;
const LIST_ROW_GAP_PX: f32 = 2.0;
const EMPTY_PAD_Y_PX: f32 = 10.0;
const MORE_PAD_X_PX: f32 = 6.0;
const MORE_PAD_TOP_PX: f32 = 4.0;
/// Block gap: a step above the small spacing.
const BLOCK_GAP_PX: f32 = tokens::SM + 2.0;
// List switcher chips.
const CHIP_H_PX: f32 = 26.0;
const CHIP_PAD_X_PX: f32 = 10.0;
const CHIP_GAP_PX: f32 = 6.0;
const CHIP_DOT_PX: f32 = 8.0;
const CHIP_ICON_PX: f32 = 13.0;
const CHIP_IDLE_ALPHA: f32 = 0.06;
const CHIP_ACTIVE_ALPHA: f32 = 0.2;
const CHIP_COUNT_ALPHA: f32 = 0.75;
const CHIP_DROP_ALPHA: f32 = 0.35;
// Section headers (the All view) and completed folds.
const SECTION_PAD_TOP_PX: f32 = 6.0;
const SECTION_DOT_PX: f32 = 7.0;
const FOLD_ICON_PX: f32 = 12.0;
// The inline list / task menu.
const MENU_PAD_PX: f32 = 10.0;
const MENU_RADIUS_PX: f32 = 12.0;
const MENU_BG_ALPHA: f32 = 0.05;
const MENU_BUTTON_H_PX: f32 = 26.0;
const MENU_BUTTON_PAD_X_PX: f32 = 10.0;
const SWATCH_PX: f32 = 20.0;
const SWATCH_RING_PX: f32 = 2.0;
/// Lowest chroma of an OKLCH hue swatch (a grey wallpaper palette would
/// otherwise draw every hue alike).
const MIN_SWATCH_CHROMA: f32 = 0.11;
/// Rows shown in the panel and in the sidebar card (`panel_rows` /
/// `card_rows`).
pub const DEFAULT_PANEL_ROWS: i64 = 14;
pub const DEFAULT_CARD_ROWS: i64 = 8;
pub const ROWS_RANGE: (i64, i64) = (3, 30);
const PANEL_W_PX: f32 = 380.0;
const PANEL_H_PX: f32 = 520.0;
/// Bar chip.
const BAR_GAP_PX: f32 = 5.0;
const BAR_ICON_PX: f32 = 15.0;

/// A list's colour in the current palette.
pub fn list_color(palette: &MandaraPalette, list: &TodoList) -> Color {
    color_of(palette, &list.color)
}

/// A list / topic colour value (tone or `hue:<deg>`) in the current palette.
pub fn color_of(palette: &MandaraPalette, color: &str) -> Color {
    match ListColor::parse(color) {
        ListColor::Tone("secondary") => palette.secondary,
        ListColor::Tone("tertiary") => palette.tertiary,
        ListColor::Tone("success") => palette.success_emerald,
        ListColor::Tone("warning") => palette.warning_amber,
        ListColor::Tone("urgent") => palette.urgent_coral,
        ListColor::Tone(_) => palette.primary,
        ListColor::Hue(h) => Color::oklch(palette.primary.lightness(), palette.primary.chroma().max(MIN_SWATCH_CHROMA), f32::from(h)),
    }
}

/// A list's name as shown (the shipped Inbox name follows the language).
pub fn list_name(list: &TodoList) -> String {
    if list.has_default_name() { mandara_i18n::t("Inbox").to_string() } else { list.name.clone() }
}

/// Per-surface display settings.
#[derive(Copy, Clone)]
struct Prefs {
    limit: usize,
    show_completed: bool,
    sort: SortOrder,
}

fn prefs(cx: &App, rows_key: &str, rows_default: i64) -> Prefs {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let s = &cfg.plugins;
    Prefs {
        limit: s.setting(ID, rows_key, rows_default).clamp(ROWS_RANGE.0, ROWS_RANGE.1) as usize,
        show_completed: s.setting(ID, SHOW_COMPLETED_KEY, DEFAULT_SHOW_COMPLETED),
        sort: SortOrder::parse(&s.setting(ID, SORT_KEY, String::new())),
    }
}

/// Where a task typed into a surface goes: the list it shows, else the
/// configured default list (Inbox when unset or gone).
fn target_list(cx: &App) -> u64 {
    let p = productivity(cx).read(cx);
    match p.filter {
        Filter::List(id) => id,
        Filter::All => {
            let cfg = crate::state::config(cx).read(cx).snapshot();
            p.book.find_list(&cfg.plugins.setting(ID, DEFAULT_LIST_KEY, String::new())).unwrap_or(INBOX_ID)
        }
    }
}

/// What the bar counter counts (`bar_count`: empty / `all`, or a list).
fn bar_filter(cx: &App) -> Filter {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    productivity(cx).read(cx).book.filter_for(&cfg.plugins.setting(ID, BAR_COUNT_KEY, String::new())).unwrap_or(Filter::All)
}

/// Transient per-surface UI: the open menu and the inline list-name field.
#[derive(Default)]
struct TodoUi {
    menu: Option<Menu>,
    naming: Option<Naming>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Menu {
    /// A list's menu; `confirm` once Delete was pressed.
    List { id: u64, confirm: bool },
    /// A task's "move to" menu.
    Task(u64),
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Naming {
    New,
    Rename(u64),
}

type Ui = Rc<RefCell<TodoUi>>;

/// Re-renders every to-do surface (their UI state lives outside GPUI).
fn refresh(cx: &mut App) {
    productivity(cx).update(cx, |_, cx| cx.notify());
}

fn set_ui(ui: &Ui, cx: &mut App, f: impl FnOnce(&mut TodoUi)) {
    f(&mut ui.borrow_mut());
    refresh(cx);
}

/// What's dragged when a task moves onto a list chip.
#[derive(Clone)]
struct DragTask {
    id: u64,
    text: SharedString,
    bg: Color,
    fg: Color,
}

impl Render for DragTask {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(CHIP_H_PX))
            .px(px(CHIP_PAD_X_PX))
            .rounded_full()
            .flex()
            .items_center()
            .bg(self.bg.hsla())
            .text_color(self.fg.hsla())
            .text_size(px(text::BODY_S))
            .shadow_lg()
            .child(self.text.clone())
    }
}

fn row(t: &Todo, palette: &MandaraPalette, ids: &'static Ids, ui: &Ui) -> AnyElement {
    let (id, done) = (t.id, t.done);
    let drag = DragTask { id, text: t.text.clone().into(), bg: palette.primary, fg: palette.on_primary };
    let (ui_menu, ui_more) = (ui.clone(), ui.clone());
    div()
        .id((ids.row, id))
        .group("todo-row")
        .min_h(px(ROW_MIN_H_PX))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .row()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .hover(|s| s.bg(palette.on_surface.with_alpha(ROW_HOVER_ALPHA).hsla()))
        .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
        .on_mouse_down(MouseButton::Right, move |_, _, cx| {
            cx.stop_propagation();
            set_ui(&ui_menu, cx, |u| u.menu = Some(Menu::Task(id)));
        })
        .child(
            div()
                .id((ids.check, id))
                .size(px(CHECK_PX))
                .flex_none()
                .rounded_full()
                .border_2()
                .border_color(if done { palette.primary.hsla() } else { palette.outline.hsla() })
                .when(done, |el| el.bg(palette.primary.hsla()))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(done, |el| el.child(icon("check", CHECK_ICON_PX, palette.on_primary)))
                .on_click(move |_, _, cx| {
                    productivity(cx).update(cx, |p, cx| {
                        p.set_done(id, !done, cx);
                    })
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .py(px(TEXT_PAD_Y_PX))
                .text_size(px(text::BODY))
                .line_height(px(TEXT_LINE_PX))
                .text_start()
                .when(done, |el| el.line_through().text_color(palette.on_surface_variant.with_alpha(DONE_ALPHA).hsla()))
                .child(t.text.clone()),
        )
        .child(
            div()
                .row()
                .invisible()
                .group_hover("todo-row", |s| s.visible())
                .child(icon_button((ids.more, id), "folder", REMOVE_PX, palette.on_surface_variant, None, move |_, cx| {
                    set_ui(&ui_more, cx, |u| u.menu = if u.menu == Some(Menu::Task(id)) { None } else { Some(Menu::Task(id)) });
                }))
                .child(icon_button((ids.remove, id), "close", REMOVE_PX, palette.on_surface_variant, None, move |_, cx| {
                    productivity(cx).update(cx, |p, cx| {
                        p.remove_todo(id, cx);
                    });
                })),
        )
        .into_any_element()
}

/// One list switcher chip (`list` None = "All").
fn chip(list: Option<&TodoList>, count: usize, active: bool, palette: &MandaraPalette, ids: &'static Ids, ui: &Ui) -> AnyElement {
    let tint = list.map_or(palette.primary, |l| list_color(palette, l));
    let label = list.map_or_else(|| mandara_i18n::t("All").to_string(), list_name);
    let filter = list.map_or(Filter::All, |l| Filter::List(l.id));
    let el = match list {
        Some(l) => div().id((ids.chip, l.id)),
        None => div().id(ids.chip_all),
    };
    let marker = match list.and_then(|l| l.icon.as_deref()) {
        Some(ic) => any_icon(ic, CHIP_ICON_PX, tint).into_any_element(),
        None if list.is_some() => div().size(px(CHIP_DOT_PX)).flex_none().rounded_full().bg(tint.hsla()).into_any_element(),
        None => icon("list-check", CHIP_ICON_PX, tint).into_any_element(),
    };
    let ui_menu = ui.clone();
    el.h(px(CHIP_H_PX))
        .px(px(CHIP_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .cursor_pointer()
        .text_size(px(text::BODY_S))
        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .bg(if active { tint.with_alpha(CHIP_ACTIVE_ALPHA).hsla() } else { palette.on_surface.with_alpha(CHIP_IDLE_ALPHA).hsla() })
        .text_color(if active { palette.on_surface.hsla() } else { palette.on_surface_variant.hsla() })
        .hover(|s| s.bg(tint.with_alpha(CHIP_ACTIVE_ALPHA).hsla()))
        .child(marker)
        .child(label)
        .when(count > 0, |el| {
            el.child(div().font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.with_alpha(CHIP_COUNT_ALPHA).hsla()).child(count.to_string()))
        })
        .on_click(move |_, _, cx| productivity(cx).update(cx, |p, cx| p.set_filter(filter, cx)))
        .when_some(list.map(|l| l.id), |el, id| {
            el.on_mouse_down(MouseButton::Right, move |_, _, cx| {
                cx.stop_propagation();
                set_ui(&ui_menu, cx, |u| u.menu = Some(Menu::List { id, confirm: false }));
            })
            // Dropping a task on a list moves it there.
            .drag_over::<DragTask>(move |s, _, _, _| s.bg(tint.with_alpha(CHIP_DROP_ALPHA).hsla()))
            .on_drop(move |d: &DragTask, _, cx| {
                productivity(cx).update(cx, |p, cx| {
                    let _ = p.edit_lists(cx, |b| b.move_task(d.id, id));
                })
            })
        })
        .into_any_element()
}

fn menu_button(id: (&'static str, u64), label: &'static str, color: Color, palette: &MandaraPalette, on: impl Fn(&mut App) + 'static) -> AnyElement {
    div()
        .id(id)
        .h(px(MENU_BUTTON_H_PX))
        .px(px(MENU_BUTTON_PAD_X_PX))
        .rounded_full()
        .row()
        .items_center()
        .cursor_pointer()
        .text_size(px(text::BODY_S))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color.hsla())
        .bg(palette.on_surface.with_alpha(CHIP_IDLE_ALPHA).hsla())
        .hover(|s| s.bg(color.with_alpha(CHIP_ACTIVE_ALPHA).hsla()))
        .child(mandara_i18n::t(label))
        .on_click(move |_, _, cx| on(cx))
        .into_any_element()
}

/// Stable small ids for menu buttons (one namespace per surface).
mod btn {
    pub const RENAME: u64 = 1;
    pub const LEFT: u64 = 2;
    pub const RIGHT: u64 = 3;
    pub const DELETE: u64 = 4;
    pub const TO_INBOX: u64 = 5;
    pub const DELETE_TASKS: u64 = 6;
    pub const CANCEL: u64 = 7;
    pub const DELETE_TASK: u64 = 8;
    pub const CLOSE: u64 = 9;
}

/// The inline menu of a list (rename, colour, move, delete) or a task
/// (move to another list, delete).
fn menu(m: Menu, book: &TodoBook, palette: &MandaraPalette, ids: &'static Ids, ui: &Ui, window: &mut Window, name_field: &Entity<TextField>) -> Option<AnyElement> {
    let shell = div()
        .id(ids.menu)
        .p(px(MENU_PAD_PX))
        .rounded(px(crate::style::radius(MENU_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(MENU_BG_ALPHA).hsla())
        .flex()
        .flex_col()
        .gap(px(tokens::SM));
    let close = {
        let ui = ui.clone();
        icon_button((ids.menu, btn::CLOSE), "close", REMOVE_PX, palette.on_surface_variant, None, move |_, cx| set_ui(&ui, cx, |u| u.menu = None))
    };
    let title = |s: String| div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).child(s);
    let _ = window;
    match m {
        Menu::List { id, confirm } => {
            let list = book.list(id)?;
            let pos = book.lists.iter().position(|l| l.id == id)?;
            let (open, done) = book.counts(Filter::List(id));
            let head = div().row().items_center().gap(px(tokens::SM)).child(title(list_name(list))).child(close);
            if confirm {
                let tasks = open + done;
                let ask = if tasks == 0 {
                    mandara_i18n::tf("Delete the list “{name}”?", &[("name", &list_name(list))])
                } else {
                    mandara_i18n::tf("Delete “{name}”? It has {count} tasks.", &[("name", &list_name(list)), ("count", &tasks)])
                };
                let del = |how: OnDelete| {
                    let ui = ui.clone();
                    move |cx: &mut App| {
                        productivity(cx).update(cx, |p, cx| {
                            let _ = p.edit_lists(cx, |b| b.remove_list(id, how));
                        });
                        set_ui(&ui, cx, |u| u.menu = None);
                    }
                };
                let cancel = {
                    let ui = ui.clone();
                    move |cx: &mut App| set_ui(&ui, cx, |u| u.menu = Some(Menu::List { id, confirm: false }))
                };
                let mut buttons = div().row().flex_wrap().gap(px(CHIP_GAP_PX));
                if tasks > 0 {
                    buttons = buttons
                        .child(menu_button((ids.menu, btn::TO_INBOX), mandara_i18n::tk("Move tasks to Inbox"), palette.primary, palette, del(OnDelete::MoveToInbox)))
                        .child(menu_button((ids.menu, btn::DELETE_TASKS), mandara_i18n::tk("Delete tasks too"), palette.urgent_coral, palette, del(OnDelete::DeleteTasks)));
                } else {
                    buttons = buttons.child(menu_button((ids.menu, btn::DELETE_TASKS), mandara_i18n::tk("Delete"), palette.urgent_coral, palette, del(OnDelete::DeleteTasks)));
                }
                buttons = buttons.child(menu_button((ids.menu, btn::CANCEL), mandara_i18n::tk("Cancel"), palette.on_surface_variant, palette, cancel));
                return Some(shell.child(head).child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(ask)).child(buttons).into_any_element());
            }
            let rename = {
                let (ui, field, name) = (ui.clone(), name_field.clone(), list.name.clone());
                move |cx: &mut App| {
                    field.update(cx, |f, _| f.set(&name));
                    set_ui(&ui, cx, |u| {
                        u.menu = None;
                        u.naming = Some(Naming::Rename(id));
                    });
                }
            };
            let shift = |to: usize| {
                move |cx: &mut App| {
                    productivity(cx).update(cx, |p, cx| {
                        let _ = p.edit_lists(cx, |b| b.move_list(id, to));
                    })
                }
            };
            let mut buttons = div().row().flex_wrap().gap(px(CHIP_GAP_PX)).child(menu_button((ids.menu, btn::RENAME), mandara_i18n::tk("Rename"), palette.on_surface, palette, rename));
            if pos > 0 {
                buttons = buttons.child(menu_button((ids.menu, btn::LEFT), mandara_i18n::tk("Move left"), palette.on_surface, palette, shift(pos - 1)));
            }
            if pos + 1 < book.lists.len() {
                buttons = buttons.child(menu_button((ids.menu, btn::RIGHT), mandara_i18n::tk("Move right"), palette.on_surface, palette, shift(pos + 1)));
            }
            if id != INBOX_ID {
                let ui = ui.clone();
                buttons = buttons.child(menu_button((ids.menu, btn::DELETE), mandara_i18n::tk("Delete…"), palette.urgent_coral, palette, move |cx| {
                    set_ui(&ui, cx, |u| u.menu = Some(Menu::List { id, confirm: true }))
                }));
            }
            // Colour: palette tones, then fixed OKLCH hues.
            let current = list.color.clone();
            let values: Vec<String> = TONES.iter().map(|t| t.to_string()).chain(SWATCH_HUES.iter().map(|h| format!("hue:{h}"))).collect();
            let swatches = values.into_iter().enumerate().map(|(i, value)| {
                let probe = TodoList { color: value.clone(), ..TodoList::default() };
                let c = list_color(palette, &probe);
                let active = value == current;
                div()
                    .id((ids.swatch, i as u64))
                    .size(px(SWATCH_PX))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(c.hsla())
                    .when(active, |el| el.border(px(SWATCH_RING_PX)).border_color(palette.on_surface.hsla()))
                    .on_click(move |_, _, cx| {
                        productivity(cx).update(cx, |p, cx| {
                            let _ = p.edit_lists(cx, |b| b.recolor_list(id, &value));
                        })
                    })
            });
            Some(shell.child(head).child(buttons).child(div().row().flex_wrap().gap(px(CHIP_GAP_PX)).children(swatches)).into_any_element())
        }
        Menu::Task(task) => {
            let t = book.todos.iter().find(|t| t.id == task)?;
            let head = div().row().items_center().gap(px(tokens::SM)).child(title(mandara_i18n::t("Move to").to_string())).child(close);
            let targets = book.lists.iter().filter(|l| l.id != t.list).map(|l| {
                let (lid, tint, ui) = (l.id, list_color(palette, l), ui.clone());
                div()
                    .id((ids.move_to, lid))
                    .h(px(CHIP_H_PX))
                    .px(px(CHIP_PAD_X_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(CHIP_GAP_PX))
                    .cursor_pointer()
                    .text_size(px(text::BODY_S))
                    .bg(palette.on_surface.with_alpha(CHIP_IDLE_ALPHA).hsla())
                    .hover(|s| s.bg(tint.with_alpha(CHIP_ACTIVE_ALPHA).hsla()))
                    .child(div().size(px(CHIP_DOT_PX)).rounded_full().bg(tint.hsla()))
                    .child(list_name(l))
                    .on_click(move |_, _, cx| {
                        productivity(cx).update(cx, |p, cx| {
                            let _ = p.edit_lists(cx, |b| b.move_task(task, lid));
                        });
                        set_ui(&ui, cx, |u| u.menu = None);
                    })
            });
            let delete = {
                let ui = ui.clone();
                menu_button((ids.menu, btn::DELETE_TASK), mandara_i18n::tk("Delete task"), palette.urgent_coral, palette, move |cx| {
                    productivity(cx).update(cx, |p, cx| {
                        p.remove_todo(task, cx);
                    });
                    set_ui(&ui, cx, |u| u.menu = None);
                })
            };
            Some(shell.child(head).child(div().row().flex_wrap().gap(px(CHIP_GAP_PX)).children(targets).child(delete)).into_any_element())
        }
    }
}

/// The "Completed (n)" fold of one list.
fn fold(list: &TodoList, done: usize, palette: &MandaraPalette, ids: &'static Ids) -> AnyElement {
    let (id, open) = (list.id, list.show_done);
    div()
        .id((ids.fold, id))
        .px(px(ROW_PAD_X_PX))
        .pt(px(MORE_PAD_TOP_PX))
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .cursor_pointer()
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(icon(if open { "minus" } else { "plus" }, FOLD_ICON_PX, palette.on_surface_variant))
        .child(mandara_i18n::tf("Completed ({count})", &[("count", &done)]))
        .on_click(move |_, _, cx| {
            productivity(cx).update(cx, |p, cx| {
                let _ = p.edit_lists(cx, |b| {
                    if let Some(l) = b.lists.iter_mut().find(|l| l.id == id) {
                        l.show_done = !l.show_done;
                    }
                    Ok(())
                });
            })
        })
        .into_any_element()
}

/// Rows of one list (open, then its completed fold), within `budget` rows.
#[allow(clippy::too_many_arguments)]
fn list_rows(book: &TodoBook, list: &TodoList, prefs: Prefs, palette: &MandaraPalette, ids: &'static Ids, ui: &Ui, budget: &mut usize, hidden: &mut usize, out: &mut Vec<AnyElement>) {
    let tasks = book.tasks(list.id, prefs.sort);
    let done = tasks.iter().filter(|t| t.done).count();
    for t in tasks.iter().filter(|t| !t.done) {
        if *budget == 0 {
            *hidden += 1;
            continue;
        }
        *budget -= 1;
        out.push(row(t, palette, ids, ui));
    }
    if done > 0 && prefs.show_completed {
        out.push(fold(list, done, palette, ids));
        if list.show_done {
            for t in tasks.iter().filter(|t| t.done) {
                if *budget == 0 {
                    *hidden += 1;
                    continue;
                }
                *budget -= 1;
                out.push(row(t, palette, ids, ui));
            }
        }
    }
}

/// The body shared by the card and the panel.
#[allow(clippy::too_many_arguments)]
fn list(p: &Productivity, ui: &Ui, field: &Entity<TextField>, name_field: &Entity<TextField>, palette: &MandaraPalette, prefs: Prefs, ids: &'static Ids, reveal_cx: Option<&PanelCx>, window: &mut Window) -> AnyElement {
    let book = &p.book;
    let filter = p.filter;
    let (open, done) = book.counts(filter);
    let state = ui.borrow();
    let head = div()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("list-check", HEAD_ICON_PX, palette.primary))
        .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::t("To-do")))
        .child(div().text_size(px(text::LABEL)).font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(if open == 0 { mandara_i18n::t("All done").to_string() } else { mandara_i18n::tf("{count} open", &[("count", &open)]) }))
        .when(done > 0, |el| {
            el.child(
                div()
                    .id(ids.clear)
                    .text_size(px(text::LABEL))
                    .cursor_pointer()
                    .text_color(palette.primary.hsla())
                    .child(mandara_i18n::t("Clear done"))
                    .on_click(move |_, _, cx| {
                        productivity(cx).update(cx, |p, cx| {
                            p.clear_done(filter, cx);
                        })
                    }),
            )
        });
    // List switcher: All, every list with its open count, then "+".
    let mut chips: Vec<AnyElement> = Vec::with_capacity(book.lists.len() + 2);
    chips.push(chip(None, book.counts(Filter::All).0, filter == Filter::All, palette, ids, ui));
    for l in &book.lists {
        chips.push(chip(Some(l), book.counts(Filter::List(l.id)).0, filter == Filter::List(l.id), palette, ids, ui));
    }
    let naming = state.naming;
    if naming.is_none() {
        let (ui, nf) = (ui.clone(), name_field.clone());
        chips.push(
            div()
                .id(ids.chip_new)
                .child(icon_button((ids.chip_new, 0u64), "plus", CHIP_H_PX, palette.on_surface_variant, Some(palette.on_surface.with_alpha(CHIP_IDLE_ALPHA)), move |window, cx| {
                    nf.update(cx, |f, cx| {
                        f.set("");
                        f.focus(window, cx);
                    });
                    set_ui(&ui, cx, |u| {
                        u.menu = None;
                        u.naming = Some(Naming::New);
                    });
                }))
                .into_any_element(),
        );
    }
    let switcher = div().id(ids.chips).overflow_x_scroll().row().items_center().gap(px(CHIP_GAP_PX)).children(chips);
    let name_row = naming.map(|_| div().child(name_field.clone()));
    let menu_el = state.menu.and_then(|m| menu(m, book, palette, ids, ui, window, name_field));
    drop(state);

    let mut rows: Vec<AnyElement> = Vec::new();
    let (mut budget, mut hidden) = (prefs.limit, 0usize);
    match filter {
        Filter::List(id) => {
            if let Some(l) = book.list(id) {
                list_rows(book, l, prefs, palette, ids, ui, &mut budget, &mut hidden, &mut rows);
            }
        }
        // Every list with tasks, under its own header.
        Filter::All => {
            for l in &book.lists {
                let (o, d) = book.counts(Filter::List(l.id));
                if o + d == 0 || (o == 0 && !prefs.show_completed) {
                    continue;
                }
                let (lid, tint) = (l.id, list_color(palette, l));
                rows.push(
                    div()
                        .id((ids.section, lid))
                        .px(px(ROW_PAD_X_PX))
                        .pt(px(SECTION_PAD_TOP_PX))
                        .row()
                        .items_center()
                        .gap(px(CHIP_GAP_PX))
                        .cursor_pointer()
                        .text_size(px(text::LABEL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.on_surface_variant.hsla())
                        .child(div().size(px(SECTION_DOT_PX)).rounded_full().bg(tint.hsla()))
                        .child(list_name(l))
                        .when(o > 0, |el| el.child(div().font_features(crate::fonts::tabular_nums()).child(o.to_string())))
                        .on_click(move |_, _, cx| productivity(cx).update(cx, |p, cx| p.set_filter(Filter::List(lid), cx)))
                        .into_any_element(),
                );
                list_rows(book, l, prefs, palette, ids, ui, &mut budget, &mut hidden, &mut rows);
            }
        }
    }
    let empty = rows.is_empty();
    let body = div()
        .flex()
        .flex_col()
        .gap(px(LIST_ROW_GAP_PX))
        .children(rows)
        .when(empty, |el| {
            el.child(div().py(px(EMPTY_PAD_Y_PX)).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::t("Nothing on your list. Type a task and press Enter.")))
        })
        .when(hidden > 0, |el| el.child(div().px(px(MORE_PAD_X_PX)).pt(px(MORE_PAD_TOP_PX)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::tf("+{count} more", &[("count", &hidden)]))));
    let mut blocks: Vec<AnyElement> = vec![head.into_any_element(), switcher.into_any_element()];
    blocks.extend(name_row.map(IntoElement::into_any_element));
    blocks.extend(menu_el);
    blocks.push(field.clone().into_any_element());
    blocks.push(body.into_any_element());
    div()
        .flex()
        .flex_col()
        .gap(px(BLOCK_GAP_PX))
        .children(blocks.into_iter().enumerate().map(|(i, b)| match reveal_cx {
            Some(r) => reveal(r, i, b),
            None => b,
        }))
        .into_any_element()
}

fn task_input(cx: &mut App) -> Entity<TextField> {
    cx.new(|cx| {
        TextField::new(
            String::new(),
            mandara_i18n::t("Add a task… (#list to file it)"),
            false,
            Rc::new(|value, _, cx| {
                let list = target_list(cx);
                productivity(cx).update(cx, |p, cx| {
                    p.add_todo(&value, list, cx);
                });
            }),
            cx,
        )
        .clear_on_commit()
    })
}

/// The inline field naming a new list or renaming one.
fn name_input(ui: &Ui, cx: &mut App) -> Entity<TextField> {
    let ui = ui.clone();
    cx.new(|cx| {
        TextField::new(
            String::new(),
            mandara_i18n::t("List name"),
            false,
            Rc::new(move |value, _, cx| {
                let naming = ui.borrow_mut().naming.take();
                let value = value.trim().to_string();
                if !value.is_empty() {
                    productivity(cx).update(cx, |p, cx| match naming {
                        Some(Naming::New) => {
                            if let Ok(id) = p.edit_lists(cx, |b| b.add_list(&value, None, None)) {
                                p.set_filter(Filter::List(id), cx);
                            }
                        }
                        Some(Naming::Rename(id)) => {
                            let _ = p.edit_lists(cx, |b| b.rename_list(id, &value));
                        }
                        None => {}
                    });
                }
                refresh(cx);
            }),
            cx,
        )
        .clear_on_commit()
    })
}

/// One surface (card or panel) of the to-do list.
fn surface(rows_key: &'static str, rows_default: i64, ids: &'static Ids, in_card: bool, reveal_cx: Option<PanelCx>, window: &mut Window, cx: &mut App) -> gpui::AnyView {
    let ui: Ui = Rc::default();
    let field = task_input(cx);
    let name_field = name_input(&ui, cx);
    fn_view(
        None,
        move |palette, window, cx| {
            let prefs = prefs(cx, rows_key, rows_default);
            let p = productivity(cx).read(cx);
            let body = list(p, &ui, &field, &name_field, palette, prefs, ids, reveal_cx.as_ref(), window);
            if in_card { card(palette).child(body).into_any_element() } else { div().size_full().p(px(tokens::LG)).child(body).into_any_element() }
        },
        window,
        cx,
    )
}

/// The list a command names (`a` index `i`), or an error naming the lists.
fn list_arg(book: &TodoBook, key: &str) -> Result<u64, String> {
    book.find_list(key).ok_or_else(|| mandara_i18n::t(crate::todo_lists::ListError::NoSuchList.message()).to_string())
}

fn list_json(book: &TodoBook, l: &TodoList) -> serde_json::Value {
    let (open, done) = book.counts(Filter::List(l.id));
    json!({ "id": l.id, "name": l.name, "color": l.color, "icon": l.icon, "open": open, "done": done })
}

fn err(e: crate::todo_lists::ListError) -> String {
    mandara_i18n::t(e.message()).to_string()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("To-do"), mandara_i18n::tk("Tasks grouped into lists, for the sidebar and bar"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let open = productivity(cx).read(cx).open_todos(bar_filter(cx));
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon("list-check", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface))
                    .when(open > 0, |el| el.child(div().font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(open.to_string())))
                    .into_any_element()
            },
            window,
            cx,
        );
        let panel = surface("panel_rows", DEFAULT_PANEL_ROWS, &PANEL_IDS, false, Some(panel_cx.clone()), window, cx);
        let sidebar = surface("card_rows", DEFAULT_CARD_ROWS, &CARD_IDS, true, None, window, cx);
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX)))) }),
            sidebar: Some(sidebar),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new("todo list", mandara_i18n::tk("Tasks (all lists, or one), open first"), |a, cx| {
            let p = productivity(cx).read(cx);
            let filter = match a.get(0) {
                Some(key) => p.book.filter_for(&a.rest(0)).ok_or_else(|| list_arg(&p.book, key).unwrap_err())?,
                None => Filter::All,
            };
            let tasks: Vec<serde_json::Value> = p
                .book
                .lists
                .iter()
                .flat_map(|l| p.book.tasks(l.id, SortOrder::Newest).into_iter().filter(|t| filter.shows(t)).map(move |t| (l, t)))
                .map(|(l, t)| json!({ "id": t.id, "text": t.text, "done": t.done, "created": t.created, "list": l.id, "list_name": l.name }))
                .collect();
            Ok(json!(tasks))
        })
        .arg(ArgSpec::optional("list", ArgKind::String, mandara_i18n::tk("list name or id (default: every list)")))
        .example("todo list")
        .example("todo list Work"),
    );
    r.command(
        ID,
        CommandSpec::new("todo add", mandara_i18n::tk("Add a task; a leading #List files it there (the list is created if missing)"), |a, cx| {
            let text = a.rest(0);
            let list = target_list(cx);
            let (id, list) = productivity(cx).update(cx, |p, cx| p.add_todo(&text, list, cx)).ok_or("task text is empty")?;
            let p = productivity(cx).read(cx);
            let task = p.book.todos.iter().find(|t| t.id == id).map(|t| t.text.clone()).unwrap_or_default();
            Ok(json!({ "id": id, "text": task, "list": list, "list_name": p.book.list(list).map(|l| l.name.clone()) }))
        })
        .arg(ArgSpec::required("text", ArgKind::String, mandara_i18n::tk("task text (rest of the line), optionally starting with #List")))
        .example("todo add Call the landlord")
        .example("todo add #Work send the report"),
    );
    for (name, summary, done) in [("todo done", mandara_i18n::tk("Mark a task complete"), true), ("todo undo", mandara_i18n::tk("Mark a task open again"), false)] {
        r.command(
            ID,
            CommandSpec::new(name, summary, move |a, cx| {
                let id = a.str(0)?.parse::<u64>().map_err(|_| "id must be a number")?;
                productivity(cx).update(cx, |p, cx| p.set_done(id, done, cx)).then_some(json!({ "id": id, "done": done })).ok_or(format!("no task {id}"))
            })
            .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("task id from `todo list`"))),
        );
    }
    r.command(
        ID,
        CommandSpec::new("todo move", mandara_i18n::tk("Move a task to another list"), |a, cx| {
            let id = a.str(0)?.parse::<u64>().map_err(|_| "id must be a number")?;
            let key = a.rest(1);
            let list = list_arg(&productivity(cx).read(cx).book, &key)?;
            productivity(cx).update(cx, |p, cx| p.edit_lists(cx, |b| b.move_task(id, list))).map_err(err)?;
            Ok(json!({ "id": id, "list": list }))
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("task id from `todo list`")))
        .arg(ArgSpec::required("list", ArgKind::String, mandara_i18n::tk("list name or id")))
        .example("todo move 12 Work"),
    );
    r.command(
        ID,
        CommandSpec::new("todo remove", mandara_i18n::tk("Delete a task"), |a, cx| {
            let id = a.str(0)?.parse::<u64>().map_err(|_| "id must be a number")?;
            productivity(cx).update(cx, |p, cx| p.remove_todo(id, cx)).then_some(json!({ "removed": id })).ok_or(format!("no task {id}"))
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("task id"))),
    );
    r.command(
        ID,
        CommandSpec::new("todo clear-done", mandara_i18n::tk("Delete completed tasks (every list, or one)"), |a, cx| {
            let filter = match a.get(0) {
                Some(_) => Filter::List(list_arg(&productivity(cx).read(cx).book, &a.rest(0))?),
                None => Filter::All,
            };
            Ok(json!({ "removed": productivity(cx).update(cx, |p, cx| p.clear_done(filter, cx)) }))
        })
        .arg(ArgSpec::optional("list", ArgKind::String, mandara_i18n::tk("list name or id (default: every list)"))),
    );
    // Lists.
    r.command(
        ID,
        CommandSpec::new("todo lists", mandara_i18n::tk("Lists in order, with open / done counts"), |_, cx| {
            let p = productivity(cx).read(cx);
            let lists: Vec<_> = p.book.lists.iter().map(|l| list_json(&p.book, l)).collect();
            let showing = match p.filter {
                Filter::All => json!("all"),
                Filter::List(id) => json!(id),
            };
            Ok(json!({ "lists": lists, "showing": showing }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("todo lists add", mandara_i18n::tk("Make a list (--color tone|hue:<deg>, --icon name)"), |a, cx| {
            let (words, color) = crate::sidebar_tabs::take_flag(&a.0, "--color");
            let (words, icon) = crate::sidebar_tabs::take_flag(&words, "--icon");
            let name = words.join(" ");
            let id = productivity(cx).update(cx, |p, cx| p.edit_lists(cx, |b| b.add_list(&name, color.as_deref(), icon.as_deref()))).map_err(err)?;
            let p = productivity(cx).read(cx);
            Ok(list_json(&p.book, p.book.list(id).expect("just added")))
        })
        .arg(ArgSpec::required("name", ArgKind::String, mandara_i18n::tk("list name (rest of the line)")))
        .example("todo lists add Side project --color hue:160"),
    );
    r.command(
        ID,
        CommandSpec::new("todo lists rename", mandara_i18n::tk("Rename a list (the Inbox too)"), |a, cx| {
            let list = list_arg(&productivity(cx).read(cx).book, a.str(0)?)?;
            let name = a.rest(1);
            productivity(cx).update(cx, |p, cx| p.edit_lists(cx, |b| b.rename_list(list, &name))).map_err(err)?;
            Ok(json!({ "id": list, "name": name.trim() }))
        })
        .arg(ArgSpec::required("list", ArgKind::String, mandara_i18n::tk("list name (one word) or id")))
        .arg(ArgSpec::required("name", ArgKind::String, mandara_i18n::tk("new name (rest of the line)")))
        .example("todo lists rename 0 Later"),
    );
    r.command(
        ID,
        CommandSpec::new("todo lists color", mandara_i18n::tk("Recolour a list"), |a, cx| {
            let list = list_arg(&productivity(cx).read(cx).book, a.str(0)?)?;
            let color = a.str(1)?.to_string();
            productivity(cx).update(cx, |p, cx| p.edit_lists(cx, |b| b.recolor_list(list, &color))).map_err(err)?;
            Ok(json!({ "id": list, "color": color }))
        })
        .arg(ArgSpec::required("list", ArgKind::String, mandara_i18n::tk("list name (one word) or id")))
        .arg(ArgSpec::required("color", ArgKind::String, mandara_i18n::tk("primary, secondary, tertiary, success, warning, urgent or hue:<0-359>")))
        .example("todo lists color Work tertiary"),
    );
    r.command(
        ID,
        CommandSpec::new("todo lists move", mandara_i18n::tk("Move a list to a 1-based position"), |a, cx| {
            let list = list_arg(&productivity(cx).read(cx).book, a.str(0)?)?;
            let to = usize::try_from(a.int(1)?.max(1) - 1).unwrap_or(0);
            let at = productivity(cx).update(cx, |p, cx| p.edit_lists(cx, |b| b.move_list(list, to))).map_err(err)?;
            Ok(json!({ "id": list, "position": at + 1 }))
        })
        .arg(ArgSpec::required("list", ArgKind::String, mandara_i18n::tk("list name (one word) or id")))
        .arg(ArgSpec::required("position", ArgKind::Integer, mandara_i18n::tk("1 = first")))
        .example("todo lists move Work 1"),
    );
    r.command(
        ID,
        CommandSpec::new("todo lists remove", mandara_i18n::tk("Delete a list; its tasks move to the Inbox unless `delete` is given"), |a, cx| {
            let list = list_arg(&productivity(cx).read(cx).book, a.str(0)?)?;
            let how = if a.get(1) == Some("delete") { OnDelete::DeleteTasks } else { OnDelete::MoveToInbox };
            let n = productivity(cx).update(cx, |p, cx| p.edit_lists(cx, |b| b.remove_list(list, how))).map_err(err)?;
            Ok(json!({ "removed": list, "tasks": n, "tasks_deleted": how == OnDelete::DeleteTasks }))
        })
        .arg(ArgSpec::required("list", ArgKind::String, mandara_i18n::tk("list name (one word) or id")))
        .arg(ArgSpec::optional("tasks", ArgKind::Enum, mandara_i18n::tk("what happens to its tasks")).choices(&["move", "delete"]))
        .example("todo lists remove Work")
        .example("todo lists remove Old delete"),
    );
    r.command(
        ID,
        CommandSpec::new("todo lists show", mandara_i18n::tk("Show one list (or all) in the card and panel"), |a, cx| {
            let key = a.rest(0);
            let p = productivity(cx);
            let filter = p.read(cx).book.filter_for(&key).ok_or_else(|| err(crate::todo_lists::ListError::NoSuchList))?;
            p.update(cx, |p, cx| p.set_filter(filter, cx));
            Ok(json!({ "showing": match filter { Filter::All => json!("all"), Filter::List(id) => json!(id) } }))
        })
        .arg(ArgSpec::required("list", ArgKind::String, mandara_i18n::tk("list name or id, or all")))
        .example("todo lists show Work"),
    );
}
