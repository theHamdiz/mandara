//! Notes plugin: Markdown notes in topics (folders of `.md` files, see
//! [`crate::notes_store`]) — a sidebar card, a panel and a bar button — with
//! a topic switcher (chips), fuzzy search across every note, a rendered
//! Markdown view with clickable task lists, and an editor with Markdown
//! keys (see [`crate::markdown`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::ToGpui;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Output, Zone};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::todo::color_of;
use crate::bidi::Bidi as _;
use crate::icons::{any_icon, icon};
use crate::kit::{card, icon_button, reveal, text, tokens};
use crate::markdown::{self, Doc, RenderCx};
use crate::notes_store::{DEFAULT_TOPIC, NoteStore, NotesSnapshot, Topic};
use crate::productivity::{Productivity, productivity};
use crate::text_field::TextField;
use crate::todo_lists::{OnDelete, SWATCH_HUES, TONES};

pub const ID: &str = crate::productivity::NOTES_ID;

/// Plugin settings (`[plugins.settings.notes]`, Studio → the plugin's modal).
pub const DEFAULT_TOPIC_KEY: &str = "default_topic";
pub const FOLDER_KEY: &str = crate::productivity::NOTES_FOLDER_KEY;
pub const PREVIEW_KEY: &str = "preview_by_default";
pub const FONT_KEY: &str = "editor_font";
pub const FONT_SIZE_KEY: &str = "editor_size";
pub const DEFAULT_PREVIEW: bool = true;
pub const DEFAULT_FONT_SIZE: i64 = 13;
pub const FONT_SIZE_RANGE: (i64, i64) = (10, 20);

struct Ids {
    new: &'static str,
    delete: &'static str,
    done: &'static str,
    mode: &'static str,
    search: &'static str,
    note: &'static str,
    chips: &'static str,
    chip: &'static str,
    chip_all: &'static str,
    chip_new: &'static str,
    menu: &'static str,
    swatch: &'static str,
    move_to: &'static str,
    view: &'static str,
}

const PANEL_IDS: Ids = Ids {
    new: "panel-new-note",
    delete: "panel-del-note",
    done: "panel-done-note",
    mode: "panel-mode-note",
    search: "panel-search-notes",
    note: "panel-note",
    chips: "panel-topics",
    chip: "panel-topic",
    chip_all: "panel-topic-all",
    chip_new: "panel-topic-new",
    menu: "panel-topic-menu",
    swatch: "panel-topic-swatch",
    move_to: "panel-note-move",
    view: "panel-note-md",
};
const CARD_IDS: Ids = Ids {
    new: "card-new-note",
    delete: "card-del-note",
    done: "card-done-note",
    mode: "card-mode-note",
    search: "card-search-notes",
    note: "card-note",
    chips: "card-topics",
    chip: "card-topic",
    chip_all: "card-topic-all",
    chip_new: "card-topic-new",
    menu: "card-topic-menu",
    swatch: "card-topic-swatch",
    move_to: "card-note-move",
    view: "card-note-md",
};

const HEAD_ICON_PX: f32 = 16.0;
/// Header buttons.
const BUTTON_PX: f32 = 26.0;
const NEW_FILL_ALPHA: f32 = 0.12;
const EDITOR_GAP_PX: f32 = 6.0;
const DONE_PAD_X_PX: f32 = 12.0;
/// Done label: a half step above the label size.
const DONE_TEXT_PX: f32 = text::LABEL + 0.5;
/// Note tiles.
const TILE_PAD_X_PX: f32 = 10.0;
const TILE_PAD_Y_PX: f32 = 8.0;
const TILE_RADIUS_PX: f32 = 12.0;
const TILE_ALPHA: f32 = 0.6;
const TILE_TEXT_GAP_PX: f32 = 2.0;
const TILE_DOT_PX: f32 = 7.0;
const LIST_GAP_PX: f32 = 6.0;
/// Block gap: a step above the small spacing.
const BLOCK_GAP_PX: f32 = tokens::SM + 2.0;
/// Topic chips (as the to-do list switcher).
const CHIP_H_PX: f32 = 26.0;
/// Room above and below the chips in their strip (the chevrons sit centred).
const CHIP_STRIP_PAD_PX: f32 = 4.0;
const CHIP_PAD_X_PX: f32 = 10.0;
const CHIP_GAP_PX: f32 = 6.0;
const CHIP_DOT_PX: f32 = 8.0;
const CHIP_ICON_PX: f32 = 13.0;
const CHIP_IDLE_ALPHA: f32 = 0.06;
const CHIP_ACTIVE_ALPHA: f32 = 0.2;
const CHIP_COUNT_ALPHA: f32 = 0.75;
const CHIP_DROP_ALPHA: f32 = 0.35;
const MENU_PAD_PX: f32 = 10.0;
const MENU_RADIUS_PX: f32 = 12.0;
const MENU_BG_ALPHA: f32 = 0.05;
const MENU_BUTTON_H_PX: f32 = 26.0;
const MENU_BUTTON_PAD_X_PX: f32 = 10.0;
const SWATCH_PX: f32 = 20.0;
const SWATCH_RING_PX: f32 = 2.0;
/// The rendered note scrolls past this height (card) / fills the panel.
const VIEW_MAX_H_PX: f32 = 360.0;
const VIEW_PAD_PX: f32 = 12.0;
const VIEW_RADIUS_PX: f32 = 12.0;
const PANEL_NOTES: usize = 8;
const CARD_NOTES: usize = 4;
const PANEL_W_PX: f32 = 420.0;
const PANEL_H_PX: f32 = 560.0;
const BAR_ICON_PX: f32 = 15.0;

/// A topic's name as shown (the shipped default follows the language).
pub fn topic_name(t: &Topic) -> String {
    if t.has_default_name() { mandara_i18n::t("Notes").to_string() } else { t.name.clone() }
}

/// The topic new notes go to: the one shown, else the configured default.
fn target_topic(cx: &App) -> u64 {
    let p = productivity(cx).read(cx);
    match p.notes_topic {
        Some(t) => t,
        None => {
            let cfg = crate::state::config(cx).read(cx).snapshot();
            p.notes.find_topic(&cfg.plugins.setting(ID, DEFAULT_TOPIC_KEY, String::new())).unwrap_or(DEFAULT_TOPIC)
        }
    }
}

fn editor_font(cx: &App) -> (Option<SharedString>, f32) {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let mono = cfg.plugins.setting(ID, FONT_KEY, String::new()) == "mono";
    let size = cfg.plugins.setting(ID, FONT_SIZE_KEY, DEFAULT_FONT_SIZE).clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1) as f32;
    (mono.then(|| crate::fonts::fonts(cx).mono), size)
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Menu {
    Topic { id: u64, confirm: bool },
    Note(u64),
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Naming {
    New,
    Rename(u64),
}

/// Per-surface UI state.
#[derive(Default)]
struct NotesUi {
    selected: Option<u64>,
    root: Option<PathBuf>,
    error: Option<String>,
    naming_pending: bool,
    creating: bool,
    transitioning: bool,
    /// The selected note shows its editor (else the rendered Markdown).
    editing: bool,
    searching: bool,
    query: String,
    menu: Option<Menu>,
    naming: Option<Naming>,
    /// Parsed notes keyed by their text (parsed once per edit).
    docs: HashMap<u64, (String, Rc<Doc>)>,
    /// The topic switcher (wheel and drag scroll it sideways).
    chips: crate::hstrip::HStrip,
}

type Ui = Rc<RefCell<NotesUi>>;

fn refresh(cx: &mut App) {
    productivity(cx).update(cx, |_, cx| cx.notify());
}

fn set_ui(ui: &Ui, cx: &mut App, f: impl FnOnce(&mut NotesUi)) {
    f(&mut ui.borrow_mut());
    refresh(cx);
}

/// One surface's fields (note editor, topic name, search).
#[derive(Clone)]
struct Fields {
    editor: Entity<TextField>,
    name: Entity<TextField>,
    search: Entity<TextField>,
}

fn fields(ui: &Ui, cx: &mut App) -> Fields {
    let sel = ui.clone();
    let editor = cx.new(|cx| {
        TextField::new(
            String::new(),
            mandara_i18n::t("Write in Markdown…"),
            true,
            Rc::new(move |value, _, cx| {
                let selected = sel.borrow().selected;
                if let Some(id) = selected {
                    productivity(cx).update(cx, |p, cx| {
                        if sel.borrow().root.as_ref() == Some(&p.notes.root) { p.edit_note(id, &value, cx); }
                    });
                }
            }),
            cx,
        )
        .multiline()
        .markdown()
    });
    let nm = ui.clone();
    let name = cx.new(|cx| {
        TextField::new(
            String::new(),
            mandara_i18n::t("Topic name"),
            false,
            Rc::new(move |value, _, cx| {
                if nm.borrow().naming_pending { return; }
                let naming = nm.borrow().naming;
                let value = value.trim().to_owned();
                if value.is_empty() { return; }
                let pending = productivity(cx).update(cx, |p, cx| match naming {
                    Some(Naming::New) => p.edit_notes(cx, move |store| store.add_topic(&value, None, None).map(Some)),
                    Some(Naming::Rename(id)) => p.edit_notes(cx, move |store| store.rename_topic(id, &value).map(|_| None)),
                    None => Err("no topic is being edited".to_owned()),
                });
                nm.borrow_mut().naming_pending = true;
                let nm = nm.clone();
                complete(pending, cx, move |result, cx| {
                    nm.borrow_mut().naming_pending = false;
                    match result {
                        Ok(id) => {
                            nm.borrow_mut().naming = None;
                            nm.borrow_mut().error = None;
                            if let Some(id) = id { productivity(cx).update(cx, |p, cx| p.set_notes_topic(Some(id), cx)); }
                        }
                        Err(error) => nm.borrow_mut().error = Some(error),
                    }
                    refresh(cx);
                });
                refresh(cx);
            }),
            cx,
        )
    });
    let q = ui.clone();
    let search = cx.new(|cx| {
        TextField::new(
            String::new(),
            mandara_i18n::t("Search every note"),
            true,
            Rc::new(move |value, _, cx| set_ui(&q, cx, |u| u.query = value)),
            cx,
        )
    });
    Fields { editor, name, search }
}

/// Opens a note in this surface (preview or editor per the setting).
fn open(ui: &Ui, f: &Fields, id: u64, edit: bool, window: &mut Window, cx: &mut App) {
    if ui.borrow().transitioning { return; }
    let previous = ui.borrow().selected;
    if let Some(previous) = previous.filter(|previous| *previous != id) {
        // The live field already updates the model; preview fields may be stale.
        let pending = productivity(cx).update(cx, |p, cx| p.settle_note(previous, cx));
        ui.borrow_mut().transitioning = true;
        let (ui, fields, handle) = (ui.clone(), f.clone(), window.window_handle());
        complete(pending, cx, move |result, cx| {
            ui.borrow_mut().transitioning = false;
            match result {
                Ok(_) if productivity(cx).read(cx).note_pending(previous) => refresh(cx),
                Ok(_) => { let _ = handle.update(cx, |_, window, cx| show_note(&ui, &fields, id, edit, window, cx)); }
                Err(error) => set_ui(&ui, cx, |ui| ui.error = Some(error)),
            }
        });
        return;
    }
    show_note(ui, f, id, edit, window, cx);
}

fn show_note(ui: &Ui, f: &Fields, id: u64, edit: bool, window: &mut Window, cx: &mut App) {
    let Some(text) = productivity(cx).read(cx).notes.note(id).map(|note| note.text.clone()) else {
        set_ui(ui, cx, |ui| ui.error = Some(mandara_i18n::t("This item was removed. Your text is still here.").into()));
        return;
    };
    let (font, size) = editor_font(cx);
    f.editor.update(cx, |e, _| {
        e.set(&text);
        e.set_font(font, Some(size));
    });
    if edit {
        f.editor.update(cx, |e, cx| e.focus(window, cx));
    }
    set_ui(ui, cx, |u| {
        u.selected = Some(id);
        u.editing = edit;
        u.menu = None;
    });
}

fn close(ui: &Ui, cx: &mut App) {
    if ui.borrow().transitioning { return; }
    let Some(id) = ui.borrow().selected else { return };
    let pending = productivity(cx).update(cx, |p, cx| p.settle_note(id, cx));
    ui.borrow_mut().transitioning = true;
    let ui = ui.clone();
    complete(pending, cx, move |result, cx| {
        ui.borrow_mut().transitioning = false;
        match result {
            Ok(_) if productivity(cx).read(cx).note_pending(id) => refresh(cx),
            Ok(_) => set_ui(&ui, cx, |ui| { ui.selected = None; ui.error = None; }),
            Err(error) => set_ui(&ui, cx, |ui| ui.error = Some(error)),
        }
    });
}

fn ago(ts: i64) -> String {
    super::notifications::ago(ts)
}

/// What's dragged when a note moves onto a topic chip.
#[derive(Clone)]
struct DragNote {
    id: u64,
    title: SharedString,
    bg: Color,
    fg: Color,
}

impl Render for DragNote {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().h(px(CHIP_H_PX)).px(px(CHIP_PAD_X_PX)).rounded_full().flex().items_center().bg(self.bg.hsla()).text_color(self.fg.hsla()).text_size(px(text::BODY_S)).shadow_lg().child(self.title.clone())
    }
}

fn chip(topic: Option<&Topic>, count: usize, active: bool, palette: &MandaraPalette, ids: &'static Ids, ui: &Ui) -> AnyElement {
    let tint = topic.map_or(palette.primary, |t| color_of(palette, &t.color));
    let label = topic.map_or_else(|| mandara_i18n::t("All").to_string(), topic_name);
    let sel = topic.map(|t| t.id);
    let el = match topic {
        Some(t) => div().id((ids.chip, t.id)),
        None => div().id(ids.chip_all),
    };
    let marker = match topic.and_then(|t| t.icon.as_deref()) {
        Some(ic) => any_icon(ic, CHIP_ICON_PX, tint),
        None if topic.is_some() => div().size(px(CHIP_DOT_PX)).flex_none().rounded_full().bg(tint.hsla()).into_any_element(),
        None => icon("note", CHIP_ICON_PX, tint).into_any_element(),
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
        .when(count > 0, |el| el.child(div().font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.with_alpha(CHIP_COUNT_ALPHA).hsla()).child(count.to_string())))
        .on_click({
            let ui = ui.clone();
            move |_, _, cx| {
                if !ui.borrow().chips.dragged() {
                    productivity(cx).update(cx, |p, cx| p.set_notes_topic(sel, cx));
                }
            }
        })
        .when_some(sel, |el, id| {
            el.on_mouse_down(MouseButton::Right, move |_, _, cx| {
                cx.stop_propagation();
                set_ui(&ui_menu, cx, |u| u.menu = Some(Menu::Topic { id, confirm: false }));
            })
            .drag_over::<DragNote>(move |s, _, _, _| s.bg(tint.with_alpha(CHIP_DROP_ALPHA).hsla()))
            .on_drop(move |d: &DragNote, _, cx| {
                let note = d.id;
                store_edit(cx, move |store| store.move_note(note, id));
            })
        })
        .into_any_element()
}

fn menu_button(id: (&'static str, u64), label: &'static str, color: Color, palette: &MandaraPalette, on: impl Fn(&mut App) + 'static) -> AnyElement {
    use crate::components::buttons::{Ink, designed};
    let (fill, hover) = (palette.on_surface.with_alpha(CHIP_IDLE_ALPHA), color.with_alpha(CHIP_ACTIVE_ALPHA));
    let el = div().id(id).h(px(MENU_BUTTON_H_PX)).px(px(MENU_BUTTON_PAD_X_PX));
    designed(el, id, MENU_BUTTON_H_PX, false, Ink { bg: Some(fill), hover, outline: None }, Some(palette), |el| el.rounded_full().bg(fill.hsla()).hover(move |s| s.bg(hover.hsla())))
        .row()
        .items_center()
        .cursor_pointer()
        .text_size(px(text::BODY_S))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color.hsla())
        .child(mandara_i18n::t(label))
        .on_click(move |_, _, cx| on(cx))
        .into_any_element()
}

mod btn {
    pub const RENAME: u64 = 1;
    pub const LEFT: u64 = 2;
    pub const RIGHT: u64 = 3;
    pub const DELETE: u64 = 4;
    pub const TO_DEFAULT: u64 = 5;
    pub const DELETE_NOTES: u64 = 6;
    pub const CANCEL: u64 = 7;
    pub const CLOSE: u64 = 9;
}

fn store_edit(cx: &mut App, f: impl FnOnce(&mut NoteStore) -> anyhow::Result<()> + Send + 'static) {
    let result = productivity(cx).update(cx, |p, cx| p.edit_notes(cx, f));
    complete(result, cx, |result, _| {
        if let Err(error) = result { tracing::warn!("notes: {error}"); }
    });
}

fn menu(m: Menu, store: &NotesSnapshot, palette: &MandaraPalette, ids: &'static Ids, ui: &Ui, name_field: &Entity<TextField>) -> Option<AnyElement> {
    let shell = div().id(ids.menu).p(px(MENU_PAD_PX)).rounded(px(crate::style::radius(MENU_RADIUS_PX))).bg(palette.on_surface.with_alpha(MENU_BG_ALPHA).hsla()).flex().flex_col().gap(px(tokens::SM));
    let close = {
        let ui = ui.clone();
        icon_button((ids.menu, btn::CLOSE), "close", BUTTON_PX, palette.on_surface_variant, None, move |_, cx| set_ui(&ui, cx, |u| u.menu = None))
    };
    let title = |s: String| div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).child(s);
    match m {
        Menu::Topic { id, confirm } => {
            let topic = store.topic(id)?;
            let pos = store.topics.iter().position(|t| t.id == id)?;
            let count = store.count(Some(id));
            let head = div().row().items_center().gap(px(tokens::SM)).child(title(topic_name(topic))).child(close);
            if confirm {
                let ask = mandara_i18n::tf("Delete the topic “{name}”? It has {count} notes.", &[("name", &topic_name(topic)), ("count", &count)]);
                let del = |how: OnDelete| {
                    let ui = ui.clone();
                    move |cx: &mut App| {
                        store_edit(cx, move |s| s.remove_topic(id, how).map(drop));
                        set_ui(&ui, cx, |u| u.menu = None);
                    }
                };
                let cancel = {
                    let ui = ui.clone();
                    move |cx: &mut App| set_ui(&ui, cx, |u| u.menu = Some(Menu::Topic { id, confirm: false }))
                };
                let buttons = div()
                    .row()
                    .flex_wrap()
                    .gap(px(CHIP_GAP_PX))
                    .when(count > 0, |el| el.child(menu_button((ids.menu, btn::TO_DEFAULT), mandara_i18n::tk("Move notes to Notes"), palette.primary, palette, del(OnDelete::MoveToInbox))))
                    .child(menu_button((ids.menu, btn::DELETE_NOTES), if count > 0 { mandara_i18n::tk("Delete notes too") } else { mandara_i18n::tk("Delete") }, palette.urgent_coral, palette, del(OnDelete::DeleteTasks)))
                    .child(menu_button((ids.menu, btn::CANCEL), mandara_i18n::tk("Cancel"), palette.on_surface_variant, palette, cancel));
                return Some(shell.child(head).child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(ask)).child(buttons).into_any_element());
            }
            let rename = {
                let (ui, field, name) = (ui.clone(), name_field.clone(), topic.name.clone());
                move |cx: &mut App| {
                    field.update(cx, |f, _| f.set(&name));
                    set_ui(&ui, cx, |u| {
                        u.menu = None;
                        u.naming = Some(Naming::Rename(id));
                    });
                }
            };
            let shift = |delta: isize| move |cx: &mut App| store_edit(cx, move |store| {
                let from = store.topics.iter().position(|topic| topic.id == id).ok_or_else(|| anyhow::anyhow!("no such topic"))?;
                store.move_topic(id, from.saturating_add_signed(delta)).map(drop)
            });
            let mut buttons = div().row().flex_wrap().gap(px(CHIP_GAP_PX)).child(menu_button((ids.menu, btn::RENAME), mandara_i18n::tk("Rename"), palette.on_surface, palette, rename));
            if pos > 0 {
                buttons = buttons.child(menu_button((ids.menu, btn::LEFT), mandara_i18n::tk("Move left"), palette.on_surface, palette, shift(-1)));
            }
            if pos + 1 < store.topics.len() {
                buttons = buttons.child(menu_button((ids.menu, btn::RIGHT), mandara_i18n::tk("Move right"), palette.on_surface, palette, shift(1)));
            }
            if id != DEFAULT_TOPIC {
                let ui = ui.clone();
                buttons = buttons.child(menu_button((ids.menu, btn::DELETE), mandara_i18n::tk("Delete…"), palette.urgent_coral, palette, move |cx| set_ui(&ui, cx, |u| u.menu = Some(Menu::Topic { id, confirm: true }))));
            }
            let current = topic.color.clone();
            let values: Vec<String> = TONES.iter().map(|t| t.to_string()).chain(SWATCH_HUES.iter().map(|h| format!("hue:{h}"))).collect();
            let swatches = values.into_iter().enumerate().map(|(i, value)| {
                let active = value == current;
                div()
                    .id((ids.swatch, i as u64))
                    .size(px(SWATCH_PX))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(color_of(palette, &value).hsla())
                    .when(active, |el| el.border(px(SWATCH_RING_PX)).border_color(palette.on_surface.hsla()))
                    .on_click(move |_, _, cx| {
                        let v = value.clone();
                        store_edit(cx, move |s| s.recolor_topic(id, &v));
                    })
            });
            Some(shell.child(head).child(buttons).child(div().row().flex_wrap().gap(px(CHIP_GAP_PX)).children(swatches)).into_any_element())
        }
        Menu::Note(note) => {
            let n = store.note(note)?;
            let head = div().row().items_center().gap(px(tokens::SM)).child(title(mandara_i18n::t("Move to").to_string())).child(close);
            let targets = store.topics.iter().filter(|t| t.id != n.topic).map(|t| {
                let (tid, tint, ui) = (t.id, color_of(palette, &t.color), ui.clone());
                div()
                    .id((ids.move_to, tid))
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
                    .child(topic_name(t))
                    .on_click(move |_, _, cx| {
                        store_edit(cx, move |s| s.move_note(note, tid));
                        set_ui(&ui, cx, |u| u.menu = None);
                    })
            });
            Some(shell.child(head).child(div().row().flex_wrap().gap(px(CHIP_GAP_PX)).children(targets)).into_any_element())
        }
    }
}

/// The selected note's rendered Markdown (cached per text).
fn rendered(ui: &Ui, id: u64, text_now: &str, palette: &MandaraPalette, ids: &'static Ids, mono: SharedString) -> AnyElement {
    let doc = {
        let mut u = ui.borrow_mut();
        match u.docs.get(&id) {
            Some((t, d)) if t == text_now => d.clone(),
            _ => {
                let d = Rc::new(markdown::parse(text_now));
                u.docs.insert(id, (text_now.to_string(), d.clone()));
                d
            }
        }
    };
    let toggle: markdown::OnToggle = Rc::new(move |range, _, cx| {
        productivity(cx).update(cx, |p, cx| p.toggle_note_task(id, range, cx));
    });
    let rcx = RenderCx { id: SharedString::from(format!("{}-{id}", ids.view)), on_toggle: Some(toggle), mono };
    markdown::render(&doc, palette, &rcx)
}

#[allow(clippy::too_many_arguments)]
fn body(p: &Productivity, ui: &Ui, f: &Fields, palette: &MandaraPalette, limit: usize, ids: &'static Ids, reveal_cx: Option<&PanelCx>, mono: SharedString) -> AnyElement {
    if !p.notes_ready {
        return div().text_color(palette.on_surface_variant.hsla()).child(
            p.notes_error.as_ref().map_or_else(|| mandara_i18n::t("Loading…").to_owned(), |error| mandara_i18n::tf("Couldn't load: {error}", &[("error", error)]))
        ).into_any_element();
    }

    {
        let mut state = ui.borrow_mut();
        if state.root.as_ref().is_some_and(|root| root != &p.notes.root) {
            state.selected = None;
            state.editing = false;
            state.menu = None;
            state.docs.clear();
        }
        state.root = Some(p.notes.root.clone());
    }
    let store = &p.notes;
    let state = ui.borrow();
    let selected = state.selected.filter(|id| store.note(*id).is_some());
    let shown = p.notes_topic;
    let (ui_new, f_new) = (ui.clone(), f.clone());
    let ui_search = ui.clone();
    let searching = state.searching;
    let head = div()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("note", HEAD_ICON_PX, palette.primary))
        .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::t("Notes")))
        .when(store.count(shown) > 0, |el| el.child(div().text_size(px(text::LABEL)).font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(store.count(shown).to_string())))
        .child(icon_button(ids.search, "search", BUTTON_PX, if searching { palette.primary } else { palette.on_surface_variant }, searching.then_some(palette.primary.with_alpha(NEW_FILL_ALPHA)), {
            let search = f.search.clone();
            move |window, cx| {
                let on = !ui_search.borrow().searching;
                if on {
                    search.update(cx, |s, cx| s.focus(window, cx));
                }
                set_ui(&ui_search, cx, |u| {
                    u.searching = on;
                    if !on {
                        u.query.clear();
                    }
                });
            }
        }))
        .child(icon_button(ids.new, "plus", BUTTON_PX, palette.primary, Some(palette.primary.with_alpha(NEW_FILL_ALPHA)), move |window, cx| {
            if ui_new.borrow().creating || ui_new.borrow().transitioning { return; }
            ui_new.borrow_mut().creating = true;
            let topic = target_topic(cx);
            let pending = productivity(cx).update(cx, |p, cx| p.add_note("", topic, cx));
            let handle = window.window_handle();
            let (ui, fields) = (ui_new.clone(), f_new.clone());
            complete(pending, cx, move |result, cx| {
                ui.borrow_mut().creating = false;
                match result {
                    Ok(id) => { let _ = handle.update(cx, |_, window, cx| open(&ui, &fields, id, true, window, cx)); }
                    Err(error) => set_ui(&ui, cx, |ui| ui.error = Some(error)),
                }
            });
        }));
    let mut blocks: Vec<AnyElement> = vec![head.into_any_element()];
    if let Some(error) = store.read_errors.as_ref().and_then(|errors| errors.first()).map(|error| &error.message) {
        blocks.push(div().text_color(palette.urgent_coral.hsla()).child(mandara_i18n::tf("Couldn't load: {error}", &[("error", error)])).into_any_element());
    }
    if p.has_note_conflicts() {
        blocks.push(div().text_color(palette.urgent_coral.hsla()).child(mandara_i18n::t("This item changed elsewhere. Your text is still here.")).into_any_element());
    }
    if let Some(error) = state.error.as_ref().or(p.notes_error.as_ref()) {
        blocks.push(div().text_color(palette.urgent_coral.hsla()).child(mandara_i18n::tf("Couldn't save: {error}", &[("error", error)])).into_any_element());
    }
    if searching {
        blocks.push(div().child(f.search.clone()).into_any_element());
    }
    // Topic switcher.
    let mut chips: Vec<AnyElement> = Vec::with_capacity(store.topics.len() + 2);
    chips.push(chip(None, store.count(None), shown.is_none(), palette, ids, ui));
    for t in &store.topics {
        chips.push(chip(Some(t), store.count(Some(t.id)), shown == Some(t.id), palette, ids, ui));
    }
    if state.naming.is_none() {
        let (ui, nf) = (ui.clone(), f.name.clone());
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
    let strip = ui.borrow().chips.clone();
    blocks.push(strip.with_chevrons(ids.chips, strip.row(ids.chips).row().items_center().gap(px(CHIP_GAP_PX)).children(chips), CHIP_H_PX + CHIP_STRIP_PAD_PX, palette));
    if state.naming.is_some() {
        blocks.push(div().child(f.name.clone()).into_any_element());
    }
    if let Some(m) = state.menu.and_then(|m| menu(m, store, palette, ids, ui, &f.name)) {
        blocks.push(m);
    }
    let editing = state.editing;
    let query = state.query.trim().to_string();
    drop(state);
    // The open note: rendered, or its editor.
    if let Some(id) = selected {
        let note = store.note(id).expect("checked");
        let (ui_done, ui_del, ui_mode, ui_move) = (ui.clone(), ui.clone(), ui.clone(), ui.clone());
        let f_mode = f.clone();
        let content = if editing {
            div().child(f.editor.clone()).into_any_element()
        } else {
            div()
                .id((ids.view, id))
                .max_h(px(VIEW_MAX_H_PX))
                .overflow_y_scroll()
                .p(px(VIEW_PAD_PX))
                .rounded(px(crate::style::radius(VIEW_RADIUS_PX)))
                .bg(palette.surface_container_high.with_alpha(TILE_ALPHA).hsla())
                .child(rendered(ui, id, &note.text, palette, ids, mono.clone()))
                .into_any_element()
        };
        let tint = store.topic(note.topic).map_or(palette.primary, |t| color_of(palette, &t.color));
        let topic_label = store.topic(note.topic).map(topic_name).unwrap_or_default();
        blocks.push(
            div()
                .flex()
                .flex_col()
                .gap(px(EDITOR_GAP_PX))
                .child(content)
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(EDITOR_GAP_PX))
                        .child(
                            div()
                                .id((ids.move_to, 0u64))
                                .flex_1()
                                .min_w(px(0.0))
                                .row()
                                .items_center()
                                .gap(px(tokens::XS))
                                .cursor_pointer()
                                .text_size(px(text::LABEL))
                                .text_color(palette.on_surface_variant.hsla())
                                .child(div().size(px(TILE_DOT_PX)).rounded_full().bg(tint.hsla()))
                                .child(div().truncate().child(topic_label))
                                .on_click(move |_, _, cx| set_ui(&ui_move, cx, |u| u.menu = Some(Menu::Note(id)))),
                        )
                        .child(icon_button(ids.mode, if editing { "eye" } else { "pencil" }, BUTTON_PX, palette.on_surface_variant, None, move |window, cx| {
                            let now_editing = ui_mode.borrow().editing;
                            if now_editing {
                                productivity(cx).update(cx, |p, cx| {
                                    if let Err(error) = p.flush_notes(cx) { tracing::warn!("saving notes: {error}"); }
                                });
                                set_ui(&ui_mode, cx, |u| u.editing = false);
                            } else {
                                open(&ui_mode, &f_mode, id, true, window, cx);
                            }
                        }))
                        .child(icon_button(ids.delete, "trash", BUTTON_PX, palette.on_surface_variant, None, move |_, cx| {
                            let pending = productivity(cx).update(cx, |p, cx| p.remove_note(id, cx));
                            let ui = ui_del.clone();
                            complete(pending, cx, move |result, cx| match result {
                                Ok(_) => set_ui(&ui, cx, |ui| ui.selected = None),
                                Err(error) => set_ui(&ui, cx, |ui| ui.error = Some(error)),
                            });
                        }))
                        .child(
                            div()
                                .id(ids.done)
                                .px(px(DONE_PAD_X_PX))
                                .h(px(BUTTON_PX))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .bg(palette.primary.hsla())
                                .text_color(palette.on_primary.hsla())
                                .text_size(px(DONE_TEXT_PX))
                                .font_weight(FontWeight::MEDIUM)
                                .child(mandara_i18n::t("Done"))
                                .on_click(move |_, _, cx| close(&ui_done, cx)),
                        ),
                )
                .into_any_element(),
        );
    }
    // The list: search hits, or the topic's notes, newest first.
    let order: Vec<u64> = if !query.is_empty() {
        store.search(&query)
    } else {
        store.notes.iter().filter(|n| shown.is_none_or(|t| n.topic == t) && !n.text.trim().is_empty()).map(|n| n.id).collect()
    };
    let list: Vec<AnyElement> = order
        .into_iter()
        .filter(|id| Some(*id) != selected)
        .take(limit)
        .filter_map(|id| store.note(id))
        .map(|n| {
            let (id, ui, f, ui_menu) = (n.id, ui.clone(), f.clone(), ui.clone());
            let preview = n.preview();
            let title = n.title();
            // Arabic (RTL) notes read from the right whatever the UI language.
            let rtl = crate::bidi::text_is_rtl(&title);
            let tint = store.topic(n.topic).map_or(palette.primary, |t| color_of(palette, &t.color));
            let drag = DragNote { id, title: title.clone().into(), bg: palette.primary, fg: palette.on_primary };
            div()
                .id((ids.note, id))
                .px(px(TILE_PAD_X_PX))
                .py(px(TILE_PAD_Y_PX))
                .rounded(px(crate::style::radius(TILE_RADIUS_PX)))
                .bg(palette.surface_container_high.with_alpha(TILE_ALPHA).hsla())
                .hover(|s| s.bg(palette.surface_container_high.hsla()))
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap(px(TILE_TEXT_GAP_PX))
                .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    cx.stop_propagation();
                    set_ui(&ui_menu, cx, |u| u.menu = Some(Menu::Note(id)));
                })
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(tokens::SM))
                        .when(shown.is_none(), |el| el.child(div().size(px(TILE_DOT_PX)).flex_none().rounded_full().bg(tint.hsla())))
                        .child(div().flex_1().min_w(px(0.0)).truncate().font_weight(FontWeight::MEDIUM).text_size(px(text::BODY)).when(rtl, |el| el.text_right()).when(!rtl, |el| el.text_left()).child(title))
                        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(ago(n.updated))),
                )
                .when(!preview.is_empty(), |el| el.child(div().truncate().text_size(px(text::BODY_S)).when(rtl, |el| el.text_right()).when(!rtl, |el| el.text_left()).text_color(palette.on_surface_variant.hsla()).child(preview)))
                .on_click(move |_, window, cx| {
                    let cfg = crate::state::config(cx).read(cx).snapshot();
                    let preview = cfg.plugins.setting(ID, PREVIEW_KEY, DEFAULT_PREVIEW);
                    open(&ui, &f, id, !preview, window, cx);
                })
                .into_any_element()
        })
        .collect();
    if list.is_empty() && selected.is_none() {
        let msg = if query.is_empty() { mandara_i18n::t("No notes yet. Press + to jot something down.").to_string() } else { mandara_i18n::tf("No note matches “{query}”", &[("query", &query)]) };
        blocks.push(div().py(px(LIST_GAP_PX)).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(msg).into_any_element());
    } else if !list.is_empty() {
        blocks.push(div().flex().flex_col().gap(px(LIST_GAP_PX)).children(list).into_any_element());
    }
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

fn surface(limit: usize, ids: &'static Ids, in_card: bool, reveal_cx: Option<PanelCx>, window: &mut Window, cx: &mut App) -> gpui::AnyView {
    let ui: Ui = Rc::default();
    let f = fields(&ui, cx);
    fn_view(
        None,
        move |palette, _, cx| {
            let mono = crate::fonts::fonts(cx).mono;
            let p = productivity(cx).read(cx);
            let b = body(p, &ui, &f, palette, limit, ids, reveal_cx.as_ref(), mono);
            if in_card { card(palette).child(b).into_any_element() } else { div().size_full().p(px(tokens::LG)).child(b).into_any_element() }
        },
        window,
        cx,
    )
}

fn note_id(a: &mandara_plugins::Args, i: usize) -> Result<u64, String> {
    a.str(i)?.parse::<u64>().map_err(|_| "id must be a number".to_string())
}

fn topic_arg(store: &NotesSnapshot, key: &str) -> Result<u64, String> {
    store.find_topic(key).ok_or_else(|| mandara_i18n::t("No such topic (use its name or id; see `notes topics`)").to_string())
}

fn note_json(store: &NotesSnapshot, id: u64, full: bool) -> serde_json::Value {
    let Some(n) = store.note(id) else { return serde_json::Value::Null };
    let topic = store.topic(n.topic);
    let mut v = json!({
        "id": n.id, "title": n.title(), "topic": n.topic, "topic_name": topic.map(|t| t.name.clone()),
        "file": n.path.as_ref().map(|p| store.root.join(p)), "updated": n.updated,
    });
    if full {
        v["text"] = json!(n.text);
    }
    v
}

fn topic_json(store: &NotesSnapshot, t: &Topic) -> serde_json::Value {
    json!({ "id": t.id, "name": t.name, "color": t.color, "icon": t.icon, "folder": store.root.join(&t.dir), "notes": store.count(Some(t.id)) })
}

fn edit(cx: &mut App, f: impl FnOnce(&mut NoteStore) -> anyhow::Result<serde_json::Value> + Send + 'static) -> Result<Output, String> {
    let pending = productivity(cx).update(cx, |p, cx| p.edit_notes(cx, f))?;
    Ok(command_reply(pending, Ok))
}

fn command_reply<T: Send + 'static>(
    pending: async_channel::Receiver<crate::productivity::NoteReply<T>>,
    value: impl FnOnce(T) -> Result<serde_json::Value, String> + Send + 'static,
) -> Output {
    let (reply, result) = async_channel::bounded(1);
    mandara_services::runtime::spawn("notes-command", async move {
        let result = match pending.recv().await {
            Ok(done) => done.result().and_then(value).map(|value| value.to_string()),
            Err(error) => Err(error.to_string()),
        };
        let _ = reply.send(result).await;
    });
    Output::Pending(result)
}

fn complete<T: Send + 'static>(
    pending: Result<async_channel::Receiver<crate::productivity::NoteReply<T>>, String>,
    cx: &mut App,
    done: impl FnOnce(Result<T, String>, &mut App) + 'static,
) {
    match pending {
        Err(error) => done(Err(error), cx),
        Ok(pending) => cx.spawn(async move |cx| {
            let result = match pending.recv().await { Ok(reply) => reply.result(), Err(error) => Err(error.to_string()) };
            cx.update(|cx| done(result, cx));
        }).detach(),
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Notes"), mandara_i18n::tk("Markdown notes in topics, saved as .md files"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(None, |palette, _, _| div().flex().items_center().child(icon("note", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface)).into_any_element(), window, cx);
        let panel = surface(PANEL_NOTES, &PANEL_IDS, false, Some(panel_cx.clone()), window, cx);
        let sidebar = surface(CARD_NOTES, &CARD_IDS, true, None, window, cx);
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX)))) }),
            sidebar: Some(sidebar),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new("notes list", mandara_i18n::tk("Notes (every topic, or one), most recently edited first"), |a, cx| {
            let p = productivity(cx).read(cx);
            let topic = match a.get(0) {
                Some(_) => Some(topic_arg(&p.notes, &a.rest(0))?),
                None => None,
            };
            Ok(json!(p.notes.notes.iter().filter(|n| topic.is_none_or(|t| n.topic == t) && !n.text.trim().is_empty()).map(|n| note_json(&p.notes, n.id, false)).collect::<Vec<_>>()))
        })
        .arg(ArgSpec::optional("topic", ArgKind::String, mandara_i18n::tk("topic name or id (default: every topic)"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes add", mandara_i18n::tk("Create a note (Markdown); a leading #Topic files it there (made if missing)"), |a, cx| {
            let text = a.rest(0);
            if crate::todo_lists::parse_quick_add(&text).1.is_empty() {
                return Err("note text is empty".into());
            }
            let topic = target_topic(cx);
            edit(cx, move |store| {
                let (tag, body) = crate::todo_lists::parse_quick_add(&text);
                let topic = match tag { Some(tag) => match store.find_topic(tag) { Some(id) => id, None => store.add_topic(tag, None, None)? }, None => topic };
                let id = store.create(topic, body)?;
                Ok(note_json(store, id, false))
            })
        })
        .arg(ArgSpec::required("text", ArgKind::String, mandara_i18n::tk("note text (rest of the line), optionally starting with #Topic")))
        .example("notes add Buy saffron for Friday")
        .example("notes add #Recipes Koshary for six"),
    );
    r.command(
        ID,
        CommandSpec::new("notes show", mandara_i18n::tk("One note's full Markdown text"), |a, cx| {
            let id = note_id(a, 0)?;
            let p = productivity(cx).read(cx);
            p.notes.note(id).map(|_| note_json(&p.notes, id, true)).ok_or(format!("no note {id}"))
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("note id from `notes list`"))),
    );
    r.command(
        ID,
        CommandSpec::new("notes search", mandara_i18n::tk("Fuzzy search over every note's title and text"), |a, cx| {
            let q = a.rest(0);
            let p = productivity(cx).read(cx);
            Ok(json!(p.notes.search(&q).into_iter().map(|id| note_json(&p.notes, id, false)).collect::<Vec<_>>()))
        })
        .arg(ArgSpec::required("query", ArgKind::String, mandara_i18n::tk("search text (rest of the line)")))
        .example("notes search koshary"),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes append", mandara_i18n::tk("Append a line to a note"), |a, cx| {
            let id = note_id(a, 0)?;
            let line = a.rest(1);
            edit(cx, move |store| {
                let current = &store.note(id).ok_or_else(|| anyhow::anyhow!("no note {id}"))?.text;
                let text = if current.is_empty() { line } else { format!("{current}\n{line}") };
                store.write(id, &text)?;
                Ok(json!({ "id": id }))
            })
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("note id")))
        .arg(ArgSpec::required("text", ArgKind::String, mandara_i18n::tk("line to append"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes move", mandara_i18n::tk("Move a note (its file) to another topic"), |a, cx| {
            let id = note_id(a, 0)?;
            let topic = topic_arg(&productivity(cx).read(cx).notes, &a.rest(1))?;
            edit(cx, move |s| s.move_note(id, topic).map(|_| json!({ "id": id, "topic": topic })))
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("note id")))
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name or id")))
        .example("notes move 7 Recipes"),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes delete", mandara_i18n::tk("Delete a note (and its file)"), |a, cx| {
            let id = note_id(a, 0)?;
            let pending = productivity(cx).update(cx, |p, cx| p.remove_note(id, cx))?;
            Ok(command_reply(pending, move |removed| {
                removed.then_some(json!({ "deleted": id })).ok_or_else(|| format!("no note {id}"))
            }))
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, mandara_i18n::tk("note id"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes export", mandara_i18n::tk("Copy a topic's notes (or all) into a folder as .md files"), |a, cx| {
            let p = productivity(cx).read(cx);
            let key = a.str(0)?;
            let topic = if key.eq_ignore_ascii_case("all") { None } else { Some(topic_arg(&p.notes, key)?) };
            let dir: PathBuf = mandara_config::paths::expand_tilde(&a.rest(1));
            edit(cx, move |store| {
                let count = store.export(topic, &dir)?;
                Ok(json!({ "exported": count, "folder": dir }))
            })
        })
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name or id, or all")))
        .arg(ArgSpec::required("dir", ArgKind::String, mandara_i18n::tk("folder to write into (made if missing)")))
        .example("notes export Recipes ~/Documents/recipes"),
    );
    r.command(
        ID,
        CommandSpec::new("notes topics", mandara_i18n::tk("Topics in order, with their folders and note counts"), |_, cx| {
            let p = productivity(cx).read(cx);
            Ok(json!({ "root": p.notes.root, "topics": p.notes.topics.iter().map(|t| topic_json(&p.notes, t)).collect::<Vec<_>>(), "showing": p.notes_topic }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes topics add", mandara_i18n::tk("Make a topic (a folder; --color tone|hue:<deg>, --icon name)"), |a, cx| {
            let (words, color) = crate::sidebar_tabs::take_flag(&a.0, "--color");
            let (words, icon) = crate::sidebar_tabs::take_flag(&words, "--icon");
            let name = words.join(" ");
            edit(cx, move |s| {
                let id = s.add_topic(&name, color.as_deref(), icon.as_deref())?;
                Ok(topic_json(s, s.topic(id).expect("added")))
            })
        })
        .arg(ArgSpec::required("name", ArgKind::String, mandara_i18n::tk("topic name (rest of the line)")))
        .example("notes topics add Recipes --color warning"),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes topics rename", mandara_i18n::tk("Rename a topic (and its folder)"), |a, cx| {
            let t = topic_arg(&productivity(cx).read(cx).notes, a.str(0)?)?;
            let name = a.rest(1);
            edit(cx, move |s| s.rename_topic(t, &name).map(|_| json!({ "id": t, "name": name.trim() })))
        })
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name (one word) or id")))
        .arg(ArgSpec::required("name", ArgKind::String, mandara_i18n::tk("new name (rest of the line)"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes topics color", mandara_i18n::tk("Recolour a topic"), |a, cx| {
            let t = topic_arg(&productivity(cx).read(cx).notes, a.str(0)?)?;
            let color = a.str(1)?.to_string();
            edit(cx, move |s| s.recolor_topic(t, &color).map(|_| json!({ "id": t, "color": color })))
        })
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name (one word) or id")))
        .arg(ArgSpec::required("color", ArgKind::String, mandara_i18n::tk("primary, secondary, tertiary, success, warning, urgent or hue:<0-359>"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes topics move", mandara_i18n::tk("Move a topic to a 1-based position"), |a, cx| {
            let t = topic_arg(&productivity(cx).read(cx).notes, a.str(0)?)?;
            let to = usize::try_from(a.int(1)?.max(1) - 1).unwrap_or(0);
            edit(cx, move |s| s.move_topic(t, to).map(|at| json!({ "id": t, "position": at + 1 })))
        })
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name (one word) or id")))
        .arg(ArgSpec::required("position", ArgKind::Integer, mandara_i18n::tk("1 = first"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notes topics remove", mandara_i18n::tk("Delete a topic; its notes move to the default topic unless `delete` is given"), |a, cx| {
            let t = topic_arg(&productivity(cx).read(cx).notes, a.str(0)?)?;
            let how = if a.get(1) == Some("delete") { OnDelete::DeleteTasks } else { OnDelete::MoveToInbox };
            edit(cx, move |s| s.remove_topic(t, how).map(|n| json!({ "removed": t, "notes": n, "notes_deleted": how == OnDelete::DeleteTasks })))
        })
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name (one word) or id")))
        .arg(ArgSpec::optional("notes", ArgKind::Enum, mandara_i18n::tk("what happens to its notes")).choices(&["move", "delete"])),
    );
    r.command(
        ID,
        CommandSpec::new("notes topics show", mandara_i18n::tk("Show one topic (or all) in the card and panel"), |a, cx| {
            let key = a.rest(0);
            let topic = if key.trim().eq_ignore_ascii_case("all") { None } else { Some(topic_arg(&productivity(cx).read(cx).notes, &key)?) };
            productivity(cx).update(cx, |p, cx| p.set_notes_topic(topic, cx));
            Ok(json!({ "showing": topic }))
        })
        .arg(ArgSpec::required("topic", ArgKind::String, mandara_i18n::tk("topic name or id, or all"))),
    );
}
