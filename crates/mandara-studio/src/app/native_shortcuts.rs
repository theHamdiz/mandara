//! A lazy native shortcut list and a draft that writes only on Save.

mod rows;
mod view;

use super::{
    StudioView, compositor_page,
    page_tabs::{self, PageTab},
};
use crate::{ShellQuery, latest::Latest, text_field::TextField, tokens::*};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement, IntoElement,
    ListSizingBehavior, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
    div, prelude::FluentBuilder as _, px, uniform_list,
};
use mandara_config::{
    ShellConfig,
    compositor::Keybind,
    keybinds::{Binding, BindingId},
};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tr};
use mandara_services::shortcuts::{self, Session, Undo};
use mandara_theme::MandaraPalette;
use mandara_ui::{bidi::Bidi as _, icons::icon};
use std::rc::Rc;

/// A long configuration lays out only its visible rows in this viewport.
const LIST_HEIGHT: f32 = 300.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ShortcutsTab {
    #[default]
    Mandara,
    Compositor,
}
impl ShortcutsTab {
    pub const ALL: [Self; 2] = [Self::Mandara, Self::Compositor];
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "mandara" | "shell" => Some(Self::Mandara),
            "compositor" => Some(Self::Compositor),
            _ => None,
        }
    }
}
impl PageTab for ShortcutsTab {
    fn slug(self) -> &'static str {
        match self {
            Self::Mandara => "mandara",
            Self::Compositor => "compositor",
        }
    }
    fn label(self) -> String {
        t(match self {
            Self::Mandara => "Mandara",
            Self::Compositor => "Compositor",
        })
        .into()
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Mandara => "keyboard",
            Self::Compositor => "monitor",
        }
    }
}

struct Data {
    session: Session,
    rows: Rc<[rows::Row]>,
}
struct Draft {
    id: BindingId,
    binding: Binding,
    fields: Vec<Option<Entity<TextField>>>,
}

enum SaveRequest {
    Values(BindingId, Vec<String>),
    Undo(Undo),
}

#[derive(Default)]
pub(super) struct Editor {
    requests: Latest<Vec<Keybind>>,
    data: Option<Rc<Data>>,
    filtered: Rc<[usize]>,
    query: String,
    search: Option<Entity<TextField>>,
    draft: Option<Draft>,
    undo: Option<Undo>,
    busy: bool,
    loading: bool,
    message: Option<(bool, String)>,
    visible: bool,
    cursor: usize,
    list_focus: Option<gpui::FocusHandle>,
    scroll: gpui::UniformListScrollHandle,
}
impl Editor {
    /// User drafts and the compact undo token survive hiding. Scan rows do not.
    pub fn release(&mut self) {
        self.visible = false;
        self.requests.set(None);
        self.data = None;
        self.filtered = Rc::default();
        self.search = None;
        self.loading = false;
        self.list_focus = None;
        self.scroll = Default::default();
    }
    fn refilter(&mut self) {
        self.filtered = self.data.as_ref().map_or_else(Rc::default, |data| {
            rows::filter(&data.rows, &self.query).into()
        });
        self.cursor = 0;
    }
}

fn button(
    id: &'static str,
    glyph: &'static str,
    text: impl Into<SharedString>,
    filled: bool,
    p: &MandaraPalette,
) -> gpui::Stateful<gpui::Div> {
    let focus = p.primary.hsla();
    compositor_page::button(id, glyph, text, filled, p)
        .focusable()
        .tab_index(0)
        .focus(move |style| style.border_2().border_color(focus))
}

impl StudioView {
    pub fn show_shortcuts_tab(&mut self, tab: ShortcutsTab, cx: &mut Context<Self>) {
        self.show(crate::registry::Section::Shortcuts, cx);
        self.shortcuts_tab = tab;
        self.native_shortcuts.release();
        cx.notify();
    }

    pub(super) fn shortcuts_page(
        &mut self,
        cfg: &ShellConfig,
        p: &MandaraPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = page_tabs::bar(
            "shortcut-tabs",
            &ShortcutsTab::ALL,
            self.shortcuts_tab,
            p,
            |view, tab, _, cx| {
                view.shortcuts_tab = tab;
                view.capturing = None;
                view.native_shortcuts.release();
                super::keybind_conflicts::release(cx);
                cx.notify();
            },
            window,
            cx,
        );
        if !window.is_visible() {
            self.native_shortcuts.release();
            super::keybind_conflicts::release(cx);
            return tabs;
        }
        let body = match self.shortcuts_tab {
            ShortcutsTab::Mandara => self.shortcuts(cfg, p, cx),
            ShortcutsTab::Compositor => self.compositor_shortcuts(cfg, p, window, cx),
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_18_PX))
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    fn native_scan(&mut self, cfg: &ShellConfig, cx: &mut Context<Self>) {
        let state = &mut self.native_shortcuts;
        if !state.visible {
            return;
        }
        if state.requests.desired().map(Vec::as_slice) != Some(cfg.compositor.keybinds.as_slice()) {
            let keys = cfg.compositor.keybinds.clone();
            state.requests.set(Some(keys));
        }
        let Some(request) = state.requests.begin() else {
            return;
        };
        state.loading = true;
        let keys = request.key.clone();
        cx.spawn(async move |view, cx| {
            let result = mandara_services::runtime::unblock(move || {
                let session = shortcuts::load()?;
                let rows = rows::build(&session, &keys);
                Ok::<_, anyhow::Error>((session, rows))
            })
            .await;
            let _ = view.update(cx, |view, cx| {
                let state = &mut view.native_shortcuts;
                if state.requests.finish(request) {
                    state.loading = false;
                    match result {
                        Ok((session, rows)) => {
                            state.data = Some(Rc::new(Data {
                                session,
                                rows: rows.into(),
                            }));
                            state.refilter();
                        }
                        Err(error) => state.message = Some((false, format!("{error:#}"))),
                    }
                }
                let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
                view.native_scan(&cfg, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn native_select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.native_shortcuts.busy || self.native_shortcuts.draft.is_some() {
            return;
        }
        let Some(data) = &self.native_shortcuts.data else {
            return;
        };
        let Some(row) = data.rows.get(index) else {
            return;
        };
        let source = &data.session.scan.sources[row.source];
        let binding = source.bindings[row.binding].clone();
        let Some(id) = source.binding_id(row.binding) else {
            return;
        };
        self.native_shortcuts.cursor = self
            .native_shortcuts
            .filtered
            .iter()
            .position(|candidate| *candidate == index)
            .unwrap_or_default();
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let fields = binding
            .fields
            .iter()
            .map(|field| {
                if binding.read_only.is_some() || field.read_only.is_some() {
                    return None;
                }
                let view = cx.entity().downgrade();
                Some(cx.new(|cx| {
                    let mut input = TextField::new(
                        field.value.clone(),
                        t("Value"),
                        true,
                        Rc::new(move |_, _, cx| {
                            let _ = view.update(cx, |_, cx| cx.notify());
                        }),
                        cx,
                    )
                    .parent_commits();
                    input.set_font(Some(mono.clone()), Some(TEXT_12_5_PX));
                    input
                }))
            })
            .collect();
        self.native_shortcuts.draft = Some(Draft {
            id,
            binding,
            fields,
        });
        let first = self
            .native_shortcuts
            .draft
            .as_ref()
            .and_then(|draft| draft.fields.iter().flatten().next())
            .cloned();
        if let Some(input) = first {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn native_save(&mut self, undo: bool, cx: &mut Context<Self>) {
        if self.native_shortcuts.busy {
            return;
        }
        let Some(query) = cx.try_global::<ShellQuery>().cloned() else {
            self.native_shortcuts.message = Some((
                false,
                t("Connect to the running shell before saving shortcuts.").into(),
            ));
            cx.notify();
            return;
        };
        let request = if undo {
            let Some(edit) = &self.native_shortcuts.undo else {
                return;
            };
            SaveRequest::Undo(edit.clone())
        } else {
            let Some(draft) = &self.native_shortcuts.draft else {
                return;
            };
            if draft.binding.read_only.is_some() {
                return;
            }
            SaveRequest::Values(
                draft.id.clone(),
                draft
                    .fields
                    .iter()
                    .zip(&draft.binding.fields)
                    .map(|(input, field)| {
                        input.as_ref().map_or_else(
                            || field.value.clone(),
                            |input| input.read(cx).value.clone(),
                        )
                    })
                    .collect(),
            )
        };
        self.native_shortcuts.busy = true;
        self.native_shortcuts.message = None;
        cx.spawn(async move |view, cx| {
            let result = mandara_services::runtime::unblock(move || {
                let preferences = || {
                    let value = (query.0)(vec![
                        "compositor".into(),
                        "shortcuts".into(),
                        "state".into(),
                    ])
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                    serde_json::from_value(value).map_err(Into::into)
                };
                match request {
                    SaveRequest::Values(id, values) => shortcuts::save(&id, &values, &preferences),
                    SaveRequest::Undo(edit) => shortcuts::undo(&edit, &preferences),
                }
                .map_err(|error| format!("{error:#}"))
            })
            .await;
            let _ = view.update(cx, |view, cx| {
                let state = &mut view.native_shortcuts;
                state.busy = false;
                match result {
                    Ok(saved) => {
                        state.undo = saved.undo;
                        state.draft = None;
                        state.message = Some((
                            true,
                            if undo {
                                t("Shortcuts restored.")
                            } else {
                                t("Shortcuts saved.")
                            }
                            .into(),
                        ));
                        state.requests.refresh();
                    }
                    Err(error) => state.message = Some((false, error)),
                }
                let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
                view.native_scan(&cfg, cx);
                cx.notify();
            });
        })
        .detach();
    }
}

fn row_target(current: usize, count: usize, key: &str) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let page = (LIST_HEIGHT / DIM_64_PX) as usize;
    let last = count - 1;
    let current = current.min(last);
    match key {
        "up" => Some(current.saturating_sub(1)),
        "down" => Some((current + 1).min(last)),
        "home" => Some(0),
        "end" => Some(last),
        "pageup" => Some(current.saturating_sub(page)),
        "pagedown" => Some(current.saturating_add(page).min(last)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::keybinds;

    #[test]
    fn keyboard_navigation_reaches_offscreen_bindings_and_clamps_after_filtering() {
        assert_eq!(row_target(0, 4096, "end"), Some(4095));
        assert_eq!(row_target(4095, 4096, "down"), Some(4095));
        assert_eq!(row_target(4095, 2, "up"), Some(0));
        assert_eq!(row_target(0, 4096, "pagedown"), Some(4));
        assert_eq!(row_target(4, 4096, "pageup"), Some(0));
        assert_eq!(row_target(0, 0, "home"), None);
    }

    #[test]
    fn hiding_releases_scan_data_without_cancelling_a_save_or_losing_the_user_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wm.toml");
        std::fs::write(&path, "[keybinds]\nspawn_terminal='Super+T'\n").unwrap();
        let session = Session {
            primary_mod_is_alt: false,
            target: mandara_config::compositor::Target::Aqueous,
            root: path.clone(),
            scan: keybinds::scan_from(mandara_config::compositor::Target::Aqueous, &path),
        };
        let id = session.scan.sources[0].binding_id(0).unwrap();
        let binding = session.scan.sources[0].bindings[0].clone();
        let mut editor = Editor {
            data: Some(Rc::new(Data {
                rows: rows::build(&session, &[]).into(),
                session,
            })),
            draft: Some(Draft {
                id: id.clone(),
                binding,
                fields: vec![None, None],
            }),
            undo: Some(Undo::Native(mandara_config::keybinds::UndoEdit {
                id,
                values: vec!["spawn_terminal".into(), "Super+T".into()],
            })),
            busy: true,
            visible: true,
            ..Default::default()
        };
        editor.requests.set(Some(Vec::new()));
        let reply = editor.requests.begin().unwrap();
        editor.release();
        assert!(editor.data.is_none() && editor.filtered.is_empty() && !editor.visible);
        assert!(editor.draft.is_some() && editor.undo.is_some() && editor.busy);
        assert!(!editor.requests.finish(reply));
        assert!(editor.requests.begin().is_none());
    }
}
